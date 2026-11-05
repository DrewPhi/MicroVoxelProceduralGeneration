//! Independent f64 native-cell offline oracle. No renderer/world ray traversal is used.
//! Exact canonical material snapshots and halo face ownership; no display LOD.
use frontier_world::{Material, World, VOXEL_SIZE};

pub const FIXTURE_MIN: [i64; 3] = [-128, -32, -128];
pub const FIXTURE_SIZE: [usize; 3] = [256, 224, 256];
pub fn material_id(m: Material) -> u8 {
    match m {
        Material::Air => 0,
        Material::Grass => 1,
        Material::Soil => 2,
        Material::Stone => 3,
        Material::Wood => 4,
    }
}
pub fn material(id: u8) -> Material {
    match id {
        1 => Material::Grass,
        2 => Material::Soil,
        3 => Material::Stone,
        4 => Material::Wood,
        _ => Material::Air,
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub cell: [i64; 3],
    pub distance: f64,
    pub face: usize,
    pub material: u8,
}
/// Dense exact cells with independent hierarchical empty-surface acceleration.
/// Each u16 stores material and the six exposed faces. Halo neighbors remain
/// canonical even outside the crop, so the crop never invents boundary walls.
pub struct Snapshot {
    pub min: [i64; 3],
    pub size: [usize; 3],
    cells: Vec<u16>,
    // Levels have 2^level cells per block; true means at least one exposed face.
    levels: Vec<(usize, [usize; 3], Vec<bool>)>,
}
fn index(p: [usize; 3], s: [usize; 3]) -> usize {
    (p[2] * s[1] + p[1]) * s[0] + p[0]
}
// Ownership comes from plane-crossing times, not rounded o+d*t. Near a
// corner the multiply-add can round onto a plane crossed a few ulps later.
fn forward_index(o: f64, d: f64, t: f64) -> (i64, bool) {
    let q = o + d * t;
    let mut cell = q.floor() as i64;
    if d == 0. {
        return (cell, false);
    }
    let step = if d > 0. { 1 } else { -1 };
    if step < 0 && q == q.floor() {
        cell -= 1;
    }
    let entry = if step > 0 { cell } else { cell + 1 };
    if (entry as f64 - o) / d > t {
        cell -= step;
    } else {
        let exit = if step > 0 { cell + 1 } else { cell };
        if (exit as f64 - o) / d <= t {
            cell += step;
        }
    }
    let entry = if step > 0 { cell } else { cell + 1 };
    (cell, (entry as f64 - o) / d == t)
}
impl Snapshot {
    pub fn from_world(w: &World, min: [i64; 3], size: [usize; 3]) -> Self {
        // Cached WorldColumn avoids evaluating the source height once per voxel.
        let hs = size.map(|n| n + 2);
        let mut halo = vec![0u8; hs.iter().product()];
        for z in 0..hs[2] {
            for x in 0..hs[0] {
                let column = w.column(min[0] + x as i64 - 1, min[2] + z as i64 - 1);
                for y in 0..hs[1] {
                    halo[index([x, y, z], hs)] =
                        material_id(column.material(min[1] + y as i64 - 1));
                }
            }
        }
        Self::from_halo(min, size, &halo)
    }
    pub fn build(min: [i64; 3], size: [usize; 3], mut sample: impl FnMut([i64; 3]) -> u8) -> Self {
        assert!(size.iter().all(|&n| n > 0 && n <= 4096));
        let hs = size.map(|n| n + 2);
        let mut halo = vec![0u8; hs.iter().product()];
        for z in 0..hs[2] {
            for y in 0..hs[1] {
                for x in 0..hs[0] {
                    halo[index([x, y, z], hs)] = sample([
                        min[0] + x as i64 - 1,
                        min[1] + y as i64 - 1,
                        min[2] + z as i64 - 1,
                    ]);
                }
            }
        }
        Self::from_halo(min, size, &halo)
    }
    fn from_halo(min: [i64; 3], size: [usize; 3], halo: &[u8]) -> Self {
        let hs = size.map(|n| n + 2);
        let mut cells = vec![0u16; size.iter().product()];
        for z in 0..size[2] {
            for y in 0..size[1] {
                for x in 0..size[0] {
                    let p = [x + 1, y + 1, z + 1];
                    let m = halo[index(p, hs)];
                    if m == 0 {
                        continue;
                    }
                    let mut word = m as u16;
                    for face in 0..6 {
                        let mut q = p;
                        if face % 2 == 0 {
                            q[face / 2] -= 1;
                        } else {
                            q[face / 2] += 1;
                        }
                        if halo[index(q, hs)] == 0 {
                            word |= 1 << (8 + face);
                        }
                    }
                    cells[index([x, y, z], size)] = word;
                }
            }
        }
        let mut levels = vec![];
        for b in [2usize, 4, 8, 16, 32, 64] {
            let dims = size.map(|n| n.div_ceil(b));
            let mut occupied = vec![false; dims.iter().product()];
            for z in 0..size[2] {
                for y in 0..size[1] {
                    for x in 0..size[0] {
                        if cells[index([x, y, z], size)] >> 8 != 0 {
                            occupied[index([x / b, y / b, z / b], dims)] = true;
                        }
                    }
                }
            }
            levels.push((b, dims, occupied));
        }
        Self {
            min,
            size,
            cells,
            levels,
        }
    }
    pub fn memory_bytes(&self) -> usize {
        self.cells.capacity() * 2
            + self.levels.iter().map(|l| l.2.capacity()).sum::<usize>()
            + self.levels.capacity() * std::mem::size_of::<(usize, [usize; 3], Vec<bool>)>()
    }
    pub fn ray(
        &self,
        o: [f64; 3],
        d: [f64; 3],
        near: f64,
        far: f64,
        accelerated: bool,
    ) -> Option<Hit> {
        if !o.iter().chain(d.iter()).all(|v| v.is_finite())
            || !near.is_finite()
            || !far.is_finite()
            || near < 0.
            || far < near
        {
            return None;
        }
        let scale = d.iter().fold(0f64, |a, v| a.max(v.abs()));
        if scale == 0. {
            return None;
        }
        let d = d.map(|v| v / scale);
        let norm = d.iter().map(|v| v * v).sum::<f64>().sqrt();
        let d = d.map(|v| v / norm);
        // Local coordinates in cell units preserve exact native planes.
        let o = std::array::from_fn::<_, 3, _>(|a| o[a] / VOXEL_SIZE - self.min[a] as f64);
        let dc = d.map(|v| v / VOXEL_SIZE);
        let mut enter = f64::NEG_INFINITY;
        let mut exit = far;
        let mut entries = [f64::NEG_INFINITY; 3];
        for a in 0..3 {
            if dc[a] == 0. {
                if o[a] < 0. || o[a] >= self.size[a] as f64 {
                    return None;
                }
            } else {
                let t0 = -o[a] / dc[a];
                let t1 = (self.size[a] as f64 - o[a]) / dc[a];
                entries[a] = t0.min(t1);
                enter = enter.max(entries[a]);
                exit = exit.min(t0.max(t1));
            }
        }
        let mut t = near.max(enter);
        if t >= exit {
            return None;
        }
        let step = dc.map(|v| {
            if v > 0. {
                1i64
            } else if v < 0. {
                -1
            } else {
                0
            }
        });
        let mut p = [0i64; 3];
        let mut initial_entered = [false; 3];
        for a in 0..3 {
            (p[a], initial_entered[a]) = forward_index(o[a], dc[a], t);
            if enter >= near && entries[a] == enter {
                p[a] = if step[a] > 0 {
                    0
                } else {
                    self.size[a] as i64 - 1
                };
            }
        }
        let mut faces = 0u16;
        for a in 0..3 {
            if step[a] != 0 && ((enter >= near && entries[a] == enter) || initial_entered[a]) {
                faces |= 1 << (a * 2 + usize::from(step[a] < 0));
            }
        }
        loop {
            if (0..3).any(|a| p[a] < 0 || p[a] >= self.size[a] as i64) {
                return None;
            }
            let u = p.map(|v| v as usize);
            let word = self.cells[index(u, self.size)];
            let visible = (word >> 8) & faces;
            if visible != 0 {
                let face = visible.trailing_zeros() as usize;
                return Some(Hit {
                    cell: std::array::from_fn(|a| p[a] + self.min[a]),
                    distance: t,
                    face,
                    material: word as u8,
                });
            }
            let mut block = 1;
            if accelerated {
                for (b, dims, occupied) in self.levels.iter().rev() {
                    if !occupied[index(u.map(|v| v / b), *dims)] {
                        block = *b;
                        break;
                    }
                }
            }
            // Compute crossings directly from integer planes, never accumulate
            // t deltas. Tied axes advance together, excluding edge-only cells.
            let lo = u.map(|v| v / block * block);
            let hi = std::array::from_fn::<_, 3, _>(|a| (lo[a] + block).min(self.size[a]));
            let mut next = [f64::INFINITY; 3];
            for a in 0..3 {
                if step[a] != 0 {
                    let plane = if step[a] > 0 { hi[a] } else { lo[a] };
                    next[a] = (plane as f64 - o[a]) / dc[a];
                }
            }
            let nt = next.iter().copied().fold(f64::INFINITY, f64::min);
            if !nt.is_finite() || nt > exit || nt > far {
                return None;
            }
            // Re-evaluate all coordinates after skips; exact crossing axes use
            // their integer plane rather than a rounded multiply-add result.
            faces = 0;
            for a in 0..3 {
                if next[a] == nt {
                    p[a] = if step[a] > 0 {
                        hi[a] as i64
                    } else {
                        lo[a] as i64 - 1
                    };
                    faces |= 1 << (a * 2 + usize::from(step[a] < 0));
                } else {
                    let (cell, entered) = forward_index(o[a], dc[a], nt);
                    p[a] = cell;
                    if entered {
                        faces |= 1 << (a * 2 + usize::from(step[a] < 0));
                    }
                }
            }
            t = nt;
        }
    }
}
pub fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
pub fn normalized(a: [f64; 3]) -> [f64; 3] {
    let n = dot(a, a).sqrt();
    a.map(|v| v / n)
}
pub fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
#[derive(Clone, Debug)]
pub struct Camera {
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub fov_y_degrees: f64,
    pub near: f64,
    pub far: f64,
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            eye: [11., 9., -15.],
            target: [-1., 3., 1.],
            fov_y_degrees: 60.,
            near: 0.025,
            far: 20000.,
        }
    }
}
impl Camera {
    pub fn basis(&self) -> ([f64; 3], [f64; 3], [f64; 3]) {
        let forward = normalized(std::array::from_fn(|i| self.target[i] - self.eye[i]));
        let right = normalized(cross(forward, [0., 1., 0.]));
        (forward, right, cross(right, forward))
    }
    pub fn ray(&self, x: f64, y: f64, width: usize, height: usize) -> ([f64; 3], f64, f64) {
        let (f, r, u) = self.basis();
        let h = (self.fov_y_degrees.to_radians() * 0.5).tan();
        let sx = (2. * x / width as f64 - 1.) * h * width as f64 / height as f64;
        let sy = (1. - 2. * y / height as f64) * h;
        let d = normalized(std::array::from_fn(|a| f[a] + r[a] * sx + u[a] * sy));
        let cosine = dot(f, d);
        (d, self.near / cosine, self.far / cosine)
    }
}
pub const SKY: [f64; 3] = [0.34, 0.57, 0.80];
pub fn shade(hit: Hit) -> [f64; 3] {
    let mut n = [0.; 3];
    n[hit.face / 2] = if hit.face % 2 == 0 { -1. } else { 1. };
    let sun = normalized([-0.5, 0.8, -0.3]);
    let sky = 0.5 + 0.5 * n[1];
    let low = [0.12, 0.105, 0.085];
    let high = [0.28, 0.33, 0.40];
    let direct = [0.78, 0.70, 0.58];
    let c = material(hit.material).color();
    std::array::from_fn(|a| {
        c[a] as f64 * (low[a] + (high[a] - low[a]) * sky + direct[a] * dot(n, sun).max(0.))
    })
}
pub fn srgb(v: f64) -> u8 {
    let v = v.clamp(0., 1.);
    let s = if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    };
    (s * 255.).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    // Independent exhaustive slab intersections against every occupied voxel.
    // Half-open parallel slabs and strictly positive-length overlap reject
    // cells touched only at an edge/corner. Lowest entry axis defines normals.
    fn brute(s: &Snapshot, o: [f64; 3], direction: [f64; 3], far: f64) -> Option<Hit> {
        let d = normalized(direction);
        let mut best: Option<Hit> = None;
        for z in 0..s.size[2] {
            for y in 0..s.size[1] {
                for x in 0..s.size[0] {
                    let p = [x, y, z];
                    let word = s.cells[index(p, s.size)];
                    if word & 255 == 0 {
                        continue;
                    }
                    let mut low = 0f64;
                    let mut high = far;
                    let mut faces = 0u16;
                    for a in 0..3 {
                        let lo = (s.min[a] + p[a] as i64) as f64 * VOXEL_SIZE;
                        let hi = lo + VOXEL_SIZE;
                        if d[a] == 0. {
                            if o[a] < lo || o[a] >= hi {
                                high = -1.;
                                break;
                            }
                        } else {
                            let t0 = (lo - o[a]) / d[a];
                            let t1 = (hi - o[a]) / d[a];
                            let en = t0.min(t1);
                            if en > low {
                                low = en;
                                faces = 0;
                            }
                            if en == low {
                                faces |= 1 << (a * 2 + usize::from(d[a] < 0.));
                            }
                            high = high.min(t0.max(t1));
                        }
                    }
                    if low < high {
                        let visible = faces & (word >> 8);
                        if visible != 0 && best.is_none_or(|b| low < b.distance) {
                            best = Some(Hit {
                                cell: std::array::from_fn(|a| s.min[a] + p[a] as i64),
                                distance: low,
                                face: visible.trailing_zeros() as usize,
                                material: word as u8,
                            });
                        }
                    }
                }
            }
        }
        best
    }
    #[test]
    fn independent_boxes_and_acceleration() {
        let s = Snapshot::build([-8; 3], [16; 3], |p| {
            if p[0] * p[0] + p[1] * p[1] + p[2] * p[2] < 20 && (p[0] + p[1] + p[2]) % 3 != 0 {
                1
            } else {
                0
            }
        });
        let mut state = 19u64;
        for _ in 0..3000 {
            let mut r = || {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (state >> 32) as f64 / u32::MAX as f64 * 2. - 1.
            };
            let o = [r(), r(), r()];
            let d = [r(), r(), r()];
            let a = s.ray(o, d, 0., 10., true);
            let b = s.ray(o, d, 0., 10., false);
            let c = brute(&s, o, d, 10.);
            assert_eq!(a, b, "{o:?} {d:?}");
            assert_eq!(a.is_some(), c.is_some());
            if let (Some(a), Some(c)) = (a, c) {
                assert_eq!(a.cell, c.cell);
                assert_eq!(a.face, c.face);
                assert!((a.distance - c.distance).abs() < 1e-12);
            }
        }
    }
    #[test]
    fn ties_negative_planes_and_signed_zero() {
        let s = Snapshot::build([-16; 3], [32; 3], |p| {
            if p == [0, 0, 0] || p == [-1, -1, -1] || p == [1, 0, 0] {
                4
            } else {
                0
            }
        });
        for (o, d) in [
            ([-0.125; 3], [1.; 3]),
            ([0.125; 3], [-1.; 3]),
            ([0.; 3], [-1.; 3]),
            ([-0.125, 0.03125, 0.03125], [1., -0., 0.]),
            ([0.125, 0.03125, 0.03125], [-1., 0., -0.]),
        ] {
            let a = s.ray(o, d, 0., 10., true);
            let b = s.ray(o, d, 0., 10., false);
            let c = brute(&s, o, d, 10.);
            assert_eq!(a, b);
            assert_eq!(a, c, "{o:?} {d:?}");
        }
    }
    #[test]
    fn near_ties_do_not_advance_non_crossing_axes() {
        let t = f64::from_bits(1f64.to_bits() - 1);
        // The multiply-add rounds onto 81, but the plane is reached at t=1.
        assert_eq!(80. + t, 81.);
        assert_eq!(forward_index(80., 1., t), (80, false));
        let s = Snapshot::build([-4; 3], [8; 3], |p| {
            if [[0, -1, 0], [-1, 0, 0], [0, 1, 0], [1, 0, 0]].contains(&p) {
                4
            } else {
                0
            }
        });
        for sign in [-1., 1.] {
            for e in [-4i32, -2, -1, 0, 1, 2, 4] {
                let d = [sign, sign * (1. + e as f64 * f64::EPSILON), 0.];
                let o = [-sign * 0.125, -sign * 0.125, 0.03125];
                let plain = s.ray(o, d, 0., 1., false);
                let fast = s.ray(o, d, 0., 1., true);
                let boxes = brute(&s, o, d, 1.);
                assert_eq!(fast, plain, "{d:?}");
                assert_eq!(plain.is_some(), boxes.is_some(), "{d:?}");
                if let (Some(a), Some(b)) = (plain, boxes) {
                    assert_eq!(a.cell, b.cell);
                    assert_eq!(a.face, b.face);
                    assert!((a.distance - b.distance).abs() < 1e-14);
                }
            }
        }
    }
    #[test]
    fn cave_camera_near_tie_rays_match_independent_boxes() {
        let s = Snapshot::build([-64, 32, -128], [32; 3], |p| {
            let x = p[0] + 48;
            let y = p[1] - 48;
            let z = p[2] + 112;
            if x * x + y * y + z * z >= 36 && (x + 2 * y + 3 * z).rem_euclid(5) == 0 {
                3
            } else {
                0
            }
        });
        let camera = Camera {
            eye: [-3., 3., -7.],
            target: [-3., 3., 6.],
            ..Default::default()
        };
        for (x, y) in [
            (896., 103.),
            (849., 150.),
            (999., 0.),
            (941., 58.),
            (383., 103.),
            (430., 150.),
            (280., 719.),
            (338., 661.),
        ] {
            let (d, _, _) = camera.ray(x + 0.5, y + 0.5, 1280, 720);
            for sign in [-1., 1.] {
                let d = d.map(|v| v * sign);
                let got = s.ray(camera.eye, d, 0., 10., true);
                let plain = s.ray(camera.eye, d, 0., 10., false);
                let expected = brute(&s, camera.eye, d, 10.);
                assert_eq!(got, plain);
                assert_eq!(got.is_some(), expected.is_some(), "{x} {y} {sign}");
                if let (Some(a), Some(b)) = (got, expected) {
                    assert_eq!(a.cell, b.cell, "{x} {y} {sign}");
                    assert_eq!(a.face, b.face);
                    assert!((a.distance - b.distance).abs() < 1e-12);
                }
            }
        }
    }
    #[test]
    fn inclusive_near_and_partial_blocks() {
        let s = Snapshot::build([-9, -7, -5], [19, 17, 13], |p| {
            if p == [0, 0, 0] {
                1
            } else {
                0
            }
        });
        for accel in [false, true] {
            for (o, d, near) in [
                ([-0.125, 0.03125, 0.03125], [1., 0., 0.], 0.125),
                ([0.1875, 0.03125, 0.03125], [-1., 0., 0.], 0.125),
                ([0., 0.03125, 0.03125], [1., 0., 0.], 0.),
                ([0.0625, 0.03125, 0.03125], [-1., 0., 0.], 0.),
            ] {
                let h = s.ray(o, d, near, 10., accel).unwrap();
                assert_eq!(h.cell, [0; 3]);
                assert_eq!(h.distance, near);
                assert!(s.ray(o, d, near + 1e-10, 10., accel).is_none());
            }
            assert!(s.ray([0.03125; 3], [1., 0., 0.], 0., 10., accel).is_none());
        }
    }
    #[test]
    fn crop_never_creates_surface() {
        let s = Snapshot::build([0; 3], [16; 3], |_| 3);
        assert!(s
            .ray([-1., 0.1, 0.1], [1., 0., 0.], 0., 10., true)
            .is_none());
    }
    #[test]
    fn srgb_transfer() {
        assert_eq!(srgb(0.), 0);
        assert_eq!(srgb(1.), 255);
        assert_eq!(srgb(0.21404114048223255), 128);
    }
}
