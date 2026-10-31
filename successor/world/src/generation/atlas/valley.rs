//! Fixed physical-space valley amplification, compiled before the world is exposed.
//! Two bounded raster scales add drainage-shaped local relief. Receiver rivers
//! remain constraints; runtime queries only interpolate immutable displacements.
use super::{data::Data, refine::Patch};
use crate::generation::noise::{noise, smoothstep};
use std::collections::HashMap;
const SIDE: usize = 513;
const LO: [f64; 2] = [960_000., 465_000.];
const EDGE: f64 = 20_000.;
#[derive(Clone, Copy, Default)]
pub struct Detail {
    pub delta: f64,
    pub deposit: f64,
}
pub struct Field {
    values: Vec<f32>,
    deposit: Vec<f32>,
    max: f64,
}
impl Field {
    pub fn build(data: &Data) -> Self {
        let start = std::time::Instant::now();
        let key = data.fingerprint ^ data.seed ^ 0x56414c4c45590008;
        let directory =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../output/atlas-refinement");
        let path = directory.join(format!("valley-v8-{key:016x}.bin"));
        if let Some(field) = Self::read_cache(&path, key) {
            eprintln!(
                "Valley refinement: cached immutable field ({:.3}s)",
                start.elapsed().as_secs_f64()
            );
            return field;
        }
        let mut patches = HashMap::new();
        let sample = |p: [f64; 2], patches: &mut HashMap<u32, Patch>| {
            let Some((id, w)) = data.locate(p) else {
                return (0., 0., 0.);
            };
            let patch = patches.entry(id).or_insert_with(|| Patch::build(data, id));
            let surface = patch.sample(p, w, data.seed, 0.001);
            let mut protect = if surface.water > surface.height {
                0.
            } else {
                1.
            };
            for c in &patch.channels {
                let [lo, hi] = c.bounds();
                if (0..2).any(|axis| p[axis] < lo[axis] - 160. || p[axis] > hi[axis] + 160.) {
                    continue;
                }
                let (_, d) = c.nearest(p);
                protect *= smoothstep(c.width * 0.65, c.influence() + 160., d);
            }
            (surface.height, protect, data.context(id, w).moisture)
        };
        let mut base = vec![0f64; SIDE * SIDE];
        let mut mask = base.clone();
        let mut rain = base.clone();
        let dx = EDGE / (SIDE - 1) as f64;
        for z in 0..SIDE {
            for x in 0..SIDE {
                let i = z * SIDE + x;
                let p = [LO[0] + x as f64 * dx, LO[1] + z as f64 * dx];
                let (h, m, r) = sample(p, &mut patches);
                base[i] = h;
                rain[i] = r;
                let border = (x.min(z).min(SIDE - 1 - x).min(SIDE - 1 - z)) as f64 * dx;
                mask[i] = m * smoothstep(0., 1600., border);
            }
        }
        let mut fine = base.clone();
        // Round construction-edge changes in slope before adding local relief.
        // Fixed channel/boundary constraints remain untouched; this solve is
        // compiled once across the whole valley, never independently per tile.
        let mut buffer = fine.clone();
        for _ in 0..32 {
            for z in 1..SIDE - 1 {
                for x in 1..SIDE - 1 {
                    let i = z * SIDE + x;
                    let average =
                        (fine[i - 1] + fine[i + 1] + fine[i - SIDE] + fine[i + SIDE]) * 0.25;
                    buffer[i] = fine[i] + (average - fine[i]) * 0.42 * mask[i];
                }
            }
            std::mem::swap(&mut fine, &mut buffer);
        }
        for z in 1..SIDE - 1 {
            for x in 1..SIDE - 1 {
                let i = z * SIDE + x;
                let slope = ((base[i + 1] - base[i - 1]) / (2. * dx))
                    .hypot((base[i + SIDE] - base[i - SIDE]) / (2. * dx));
                let p = [LO[0] + x as f64 * dx, LO[1] + z as f64 * dx];
                // Initial relief is constrained by local slope and then evolved below.
                let relief = (noise(p[0], p[1], 1200., data.seed ^ 711) * 65.
                    + noise(p[0], p[1], 400., data.seed ^ 712) * 24.
                    + noise(p[0], p[1], 120., data.seed ^ 713) * 6.)
                    * smoothstep(0.015, 0.28, slope);
                fine[i] += mask[i] * relief;
            }
        }
        let n = (SIDE + 1) / 2;
        let mut coarse = vec![0.; n * n];
        let mut coarse_mask = coarse.clone();
        let mut coarse_rain = coarse.clone();
        for z in 0..n {
            for x in 0..n {
                let k = z * n + x;
                let i = z * 2 * SIDE + x * 2;
                coarse[k] = fine[i];
                coarse_mask[k] = mask[i];
                coarse_rain[k] = rain[i];
            }
        }
        let original = coarse.clone();
        evolve(&mut coarse, &coarse_mask, &coarse_rain, n, dx * 2., 16);
        for z in 0..SIDE {
            for x in 0..SIDE {
                let u = x as f64 / 2.;
                let v = z as f64 / 2.;
                fine[z * SIDE + x] +=
                    bilinear_difference(&coarse, &original, n, u, v) * mask[z * SIDE + x];
            }
        }
        let deposit = evolve(&mut fine, &mask, &rain, SIDE, dx, 24);
        let values: Vec<f32> = fine
            .iter()
            .zip(&base)
            .zip(&mask)
            .map(|((&h, &b), &m)| ((h - b) * m) as f32)
            .collect();
        let max = values.iter().map(|v| v.abs() as f64).fold(0., f64::max);
        eprintln!(
            "Valley refinement: {} samples, {:.2}s, max displacement {:.2} source m",
            values.len(),
            start.elapsed().as_secs_f64(),
            max
        );
        let field = Self {
            values,
            deposit: deposit.into_iter().map(|v| v as f32).collect(),
            max,
        };
        if let Err(error) = field.write_cache(&path, key) {
            eprintln!("Valley cache unavailable: {error}");
        }
        field
    }
    fn read_cache(path: &std::path::Path, key: u64) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        let expected = 16 + SIDE * SIDE * 8 + 8;
        if bytes.len() != expected
            || &bytes[..8] != b"MVVAL008"
            || u64::from_le_bytes(bytes[8..16].try_into().ok()?) != key
        {
            return None;
        }
        let checksum = u64::from_le_bytes(bytes[expected - 8..].try_into().ok()?);
        if checksum != cache_hash(&bytes[..expected - 8]) {
            return None;
        }
        let mut arrays = bytes[16..expected - 8]
            .chunks_exact(SIDE * SIDE * 4)
            .map(|part| {
                part.chunks_exact(4)
                    .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                    .collect::<Vec<_>>()
            });
        let values = arrays.next()?;
        let deposit = arrays.next()?;
        if values.iter().chain(&deposit).any(|v| !v.is_finite()) {
            return None;
        }
        let max = values.iter().map(|v| v.abs() as f64).fold(0., f64::max);
        Some(Self {
            values,
            deposit,
            max,
        })
    }
    fn write_cache(&self, path: &std::path::Path, key: u64) -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let mut bytes = Vec::with_capacity(24 + SIDE * SIDE * 8);
        bytes.extend_from_slice(b"MVVAL008");
        bytes.extend_from_slice(&key.to_le_bytes());
        for &v in self.values.iter().chain(&self.deposit) {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.extend_from_slice(&cache_hash(&bytes).to_le_bytes());
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temporary = path.with_extension(format!("{}-{nonce}.tmp", std::process::id()));
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(temporary, path)
    }
    pub fn bounds(&self) -> ([f64; 2], [f64; 2]) {
        (LO, [LO[0] + EDGE, LO[1] + EDGE])
    }
    pub fn max_delta(&self) -> f64 {
        self.max
    }
    /// Source-meter displacement extrema and maximum error from the query's
    /// bilinear corner interpolant. Within each field cell both functions are
    /// bilinear, so extrema of their difference occur at clipped cell corners.
    /// Small queries inspect at most 64 cells; larger spans use global bounds.
    pub fn summary(&self, lo: [f64; 2], hi: [f64; 2]) -> (f64, f64, f64) {
        let (field_lo, field_hi) = self.bounds();
        if (0..2).any(|a| hi[a] < field_lo[a] || lo[a] > field_hi[a] || hi[a] < lo[a]) {
            return (0., 0., 0.);
        }
        let dx = EDGE / (SIDE - 1) as f64;
        let axis_points = |a: usize| {
            let mut points = vec![lo[a]];
            let first = (((lo[a] - LO[a]) / dx).ceil() as isize).max(0);
            let last = (((hi[a] - LO[a]) / dx).floor() as isize).min((SIDE - 1) as isize);
            for i in first..=last {
                let p = LO[a] + i as f64 * dx;
                if p > lo[a] && p < hi[a] {
                    points.push(p);
                }
            }
            if hi[a] > lo[a] {
                points.push(hi[a]);
            }
            points
        };
        let xs = axis_points(0);
        let zs = axis_points(1);
        if xs.len().saturating_sub(1).max(1) * zs.len().saturating_sub(1).max(1) > 64 {
            let max = self.max_delta();
            return (-max, max, 2. * max);
        }
        let value = |p: [f64; 2]| {
            let u = (p[0] - LO[0]) / dx;
            let v = (p[1] - LO[1]) / dx;
            if u <= 0. || v <= 0. || u >= (SIDE - 1) as f64 || v >= (SIDE - 1) as f64 {
                0.
            } else {
                sample_f32(&self.values, SIDE, u, v)
            }
        };
        let corners = [
            value(lo),
            value([hi[0], lo[1]]),
            value([lo[0], hi[1]]),
            value(hi),
        ];
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        let mut error = 0f64;
        for z in zs {
            let tz = if hi[1] > lo[1] {
                (z - lo[1]) / (hi[1] - lo[1])
            } else {
                0.
            };
            for &x in &xs {
                let tx = if hi[0] > lo[0] {
                    (x - lo[0]) / (hi[0] - lo[0])
                } else {
                    0.
                };
                let h = value([x, z]);
                let interpolated = (corners[0] * (1. - tx) + corners[1] * tx) * (1. - tz)
                    + (corners[2] * (1. - tx) + corners[3] * tx) * tz;
                min = min.min(h);
                max = max.max(h);
                error = error.max((h - interpolated).abs());
            }
        }
        (min, max, error)
    }
    pub fn memory_bytes(&self) -> usize {
        (self.values.capacity() + self.deposit.capacity()) * 4
    }
    pub fn sample(&self, p: [f64; 2]) -> Detail {
        let dx = EDGE / (SIDE - 1) as f64;
        let u = (p[0] - LO[0]) / dx;
        let v = (p[1] - LO[1]) / dx;
        if u <= 0. || v <= 0. || u >= (SIDE - 1) as f64 || v >= (SIDE - 1) as f64 {
            return Detail::default();
        }
        let value = |u: f64, v: f64| {
            sample_f32(
                &self.values,
                SIDE,
                u.clamp(0., (SIDE - 1) as f64),
                v.clamp(0., (SIDE - 1) as f64),
            )
        };
        let h = value(u, v);
        Detail {
            delta: h,
            deposit: sample_f32(&self.deposit, SIDE, u, v),
        }
    }
}
fn sample_f32(a: &[f32], n: usize, u: f64, v: f64) -> f64 {
    let x = (u.floor() as usize).min(n - 2);
    let z = (v.floor() as usize).min(n - 2);
    let tx = u - x as f64;
    let tz = v - z as f64;
    (a[z * n + x] as f64 * (1. - tx) + a[z * n + x + 1] as f64 * tx) * (1. - tz)
        + (a[(z + 1) * n + x] as f64 * (1. - tx) + a[(z + 1) * n + x + 1] as f64 * tx) * tz
}
fn bilinear_difference(a: &[f64], b: &[f64], n: usize, u: f64, v: f64) -> f64 {
    let x = (u.floor() as usize).min(n - 2);
    let z = (v.floor() as usize).min(n - 2);
    let tx = u - x as f64;
    let tz = v - z as f64;
    let d = |i: usize| a[i] - b[i];
    (d(z * n + x) * (1. - tx) + d(z * n + x + 1) * tx) * (1. - tz)
        + (d((z + 1) * n + x) * (1. - tx) + d((z + 1) * n + x + 1) * tx) * tz
}
/// Downstream-first material transport uses the reverse height ordering. Water
/// uses multiple downhill directions; removed material follows the same weights.
/// Boundary export is allowed; fixed river/boundary cells do not change height.
fn evolve(h: &mut [f64], mask: &[f64], rain: &[f64], n: usize, dx: f64, steps: usize) -> Vec<f64> {
    let mut deposit = vec![0.; h.len()];
    let mut order: Vec<usize> = (0..h.len()).collect();
    let mut flow = vec![0.; h.len()];
    let mut sediment = flow.clone();
    let mut change = flow.clone();
    for _ in 0..steps {
        order.sort_unstable_by(|&a, &b| h[b].total_cmp(&h[a]).then(a.cmp(&b)));
        for i in 0..h.len() {
            flow[i] = dx * dx * (0.1 + rain[i]);
            sediment[i] = 0.;
            change[i] = 0.;
        }
        for &i in &order {
            let x = i % n;
            let z = i / n;
            if x == 0 || z == 0 || x + 1 == n || z + 1 == n {
                continue;
            }
            let mut links = [(0usize, 0f64); 8];
            let mut count = 0;
            let mut sum = 0.;
            let mut max_slope = 0f64;
            for oz in -1isize..=1 {
                for ox in -1isize..=1 {
                    if ox == 0 && oz == 0 {
                        continue;
                    }
                    let j = (i as isize + oz * n as isize + ox) as usize;
                    let length = dx * if ox != 0 && oz != 0 { 2f64.sqrt() } else { 1. };
                    let s = ((h[i] - h[j]) / length).max(0.);
                    if s > 0. {
                        let weight = s.powf(1.1);
                        links[count] = (j, weight);
                        count += 1;
                        sum += weight;
                        max_slope = max_slope.max(s);
                    }
                }
            }
            if count == 0 {
                let d = sediment[i].min(1.) * mask[i];
                change[i] += d;
                deposit[i] += d;
                continue;
            }
            let capacity = (flow[i] / (dx * dx)).sqrt() * max_slope * 0.8;
            let erode = (capacity - sediment[i]).max(0.).min(1.2) * mask[i];
            let lay = (sediment[i] - capacity).max(0.).min(0.6) * mask[i];
            change[i] += lay - erode;
            deposit[i] += lay;
            let carried = sediment[i] + erode - lay;
            for &(j, w) in &links[..count] {
                flow[j] += flow[i] * w / sum;
                sediment[j] += carried * w / sum;
            }
        }
        for i in 0..h.len() {
            h[i] += change[i];
        }
        // Conservative pairwise relaxation of steep loose slopes, bounded per pass.
        change.fill(0.);
        for z in 1..n - 1 {
            for x in 1..n - 1 {
                let i = z * n + x;
                for j in [i + 1, i + n] {
                    let diff = h[i] - h[j];
                    let transport =
                        ((diff.abs() - dx * 0.65).max(0.) * 0.08).min(0.5) * mask[i].min(mask[j]);
                    let t = transport * diff.signum();
                    change[i] -= t;
                    change[j] += t;
                }
            }
        }
        for i in 0..h.len() {
            h[i] += change[i];
        }
    }
    deposit
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_summary_has_no_global_error_floor_and_bounds_grid_crossings() {
        let mut f = Field {
            values: vec![0.; SIDE * SIDE],
            deposit: vec![0.; SIDE * SIDE],
            max: 10.,
        };
        for z in 19..=22 {
            f.values[z * SIDE + 20] = 10.;
        }
        let dx = EDGE / (SIDE - 1) as f64;
        let point = |x: f64, z: f64| [LO[0] + x * dx, LO[1] + z * dx];
        let (min, max, error) = f.summary(point(19.2, 20.2), point(19.8, 20.8));
        assert!((min - 2.).abs() < 1e-9);
        assert!((max - 8.).abs() < 1e-9);
        assert!(
            error < 1e-9,
            "a single bilinear field cell has zero residual"
        );
        let lo = point(19., 20.);
        let hi = point(21., 21.);
        let (min, max, error) = f.summary(lo, hi);
        assert_eq!((min, max, error), (0., 10., 10.));
        for z in 0..=20 {
            for x in 0..=20 {
                let p = [
                    lo[0] + (hi[0] - lo[0]) * x as f64 / 20.,
                    lo[1] + (hi[1] - lo[1]) * z as f64 / 20.,
                ];
                let delta = f.sample(p).delta;
                assert!(delta >= min && delta <= max);
                assert!(delta.abs() <= error);
            }
        }
        assert_eq!(f.summary([0.; 2], [1.; 2]), (0., 0., 0.));
        assert_eq!(
            f.summary(LO, [LO[0] + EDGE, LO[1] + EDGE]),
            (-10., 10., 20.)
        );
    }
    #[test]
    fn cache_rejects_wrong_identity_and_corrupted_values() {
        let path =
            std::env::temp_dir().join(format!("valley-cache-test-{}.bin", std::process::id()));
        let f = Field {
            values: vec![0.25; SIDE * SIDE],
            deposit: vec![0.5; SIDE * SIDE],
            max: 0.25,
        };
        f.write_cache(&path, 42).unwrap();
        let loaded = Field::read_cache(&path, 42).unwrap();
        assert_eq!(loaded.values, f.values);
        assert!(Field::read_cache(&path, 43).is_none());
        let mut b = std::fs::read(&path).unwrap();
        b[20] ^= 1;
        std::fs::write(&path, b).unwrap();
        assert!(Field::read_cache(&path, 42).is_none());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn immutable_field_is_zero_outside_and_matches_shared_edges() {
        let f = Field {
            values: vec![0.; SIDE * SIDE],
            deposit: vec![0.; SIDE * SIDE],
            max: 0.,
        };
        for p in [LO, [LO[0] - 1., LO[1] + 100.], [LO[0] + EDGE, LO[1] + EDGE]] {
            assert_eq!(f.sample(p).delta, 0.);
        }
    }
    #[test]
    fn dry_flat_solver_does_not_invent_relief() {
        let n = 17;
        let mut h = vec![20.; n * n];
        let m = vec![1.; n * n];
        let r = vec![0.5; n * n];
        let d = evolve(&mut h, &m, &r, n, 10., 5);
        assert!(h.iter().all(|v| *v == 20.));
        assert!(d.iter().all(|v| *v == 0.));
    }
    #[test]
    fn fixed_boundaries_and_channels_do_not_evolve() {
        let n = 17;
        let mut h: Vec<_> = (0..n * n).map(|i| 100. - (i % n) as f64 * 2.).collect();
        let old = h.clone();
        let mut m = vec![1.; n * n];
        for z in 0..n {
            m[z * n + 8] = 0.;
        }
        let r = vec![0.6; n * n];
        evolve(&mut h, &m, &r, n, 5., 4);
        for z in 0..n {
            assert_eq!(h[z * n + 8], old[z * n + 8]);
            assert_eq!(h[z * n], old[z * n]);
        }
    }
}

fn cache_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, &v| {
        (h ^ v as u64).wrapping_mul(0x100000001b3)
    })
}
