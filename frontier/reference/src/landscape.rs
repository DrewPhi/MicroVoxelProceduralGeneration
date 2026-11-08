//! Exact heightfield-only native voxel oracle. Every 6.25cm column is sampled;
//! the max pyramid only rejects empty space and never substitutes coarse geometry.
use crate::{forward_index, material_id, Hit};
use frontier_world::{FbmTerrain, Material, VOXEL_SIZE};
use std::{
    fs::File,
    io::{self, BufReader, BufWriter, Read, Write},
    path::Path,
};
const BLOCK: usize = 16;
pub struct Landscape {
    pub min: [i64; 2],
    pub size: usize,
    pub cache_identity: u64,
    /// Horizontal grid spacing; native vertical quantization always remains 6.25 cm.
    pub sample_stride: usize,
    heights: Vec<i32>,
    levels: Vec<(usize, Vec<i32>)>,
    terrain: FbmTerrain,
}
impl Landscape {
    /// Square power-of-two native footprint. Persistent cache identity includes
    /// the complete canonical source text, recipe/params, seed and integer crop.
    pub fn build(
        terrain: FbmTerrain,
        min: [i64; 2],
        size: usize,
        threads: usize,
        cache: Option<&Path>,
    ) -> io::Result<Self> {
        Self::build_sampled(terrain, min, size, 1, threads, cache)
    }
    /// Explicit artistic approximation: x/z are grid coordinates with spacing
    /// `sample_stride * VOXEL_SIZE`. Heights retain native vertical quantization.
    /// A stride above one is never an exact voxel oracle or a certified bound.
    pub fn build_sampled(
        terrain: FbmTerrain,
        min: [i64; 2],
        size: usize,
        sample_stride: usize,
        threads: usize,
        cache: Option<&Path>,
    ) -> io::Result<Self> {
        if !sample_stride.is_power_of_two() || sample_stride > 256 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sample stride must be a power of two in 1..256 native cells",
            ));
        }
        if !size.is_power_of_two() || size < BLOCK || size > 32768 || threads == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "size must be power of two in16..32768 cells; threads>0",
            ));
        }
        let spacing = VOXEL_SIZE * sample_stride as f64;
        let mut identity = format!(
            "native-landscape-v1:{:?}:{:?}:{}:{min:?}:{size}:{}:{}",
            terrain.params(),
            terrain.seed(),
            terrain.source_version(),
            include_str!("../../world/src/terrain.rs"),
            include_str!("../../world/src/lib.rs")
        );
        if terrain.params().recipe == frontier_world::FbmRecipe::Alpine {
            identity.push_str(include_str!("../../world/src/alpine.rs"));
        }
        if sample_stride != 1 {
            identity.push_str(&format!(":sample-stride:{sample_stride}"));
        }
        let hash = identity.bytes().fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100000001b3)
        });
        let mut heights = vec![0i32; size * size];
        let cached = if let Some(path) = cache.filter(|p| p.exists()) {
            let mut reader = BufReader::new(File::open(path)?);
            let mut header = [0u8; 16];
            reader.read_exact(&mut header)?;
            if &header[..8] != b"FVNAT001"
                || u64::from_le_bytes(header[8..].try_into().unwrap()) != hash
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "native cache identity mismatch; use a new cache path",
                ));
            }
            for chunk in heights.chunks_mut(65536) {
                let mut bytes = vec![0u8; chunk.len() * 4];
                reader.read_exact(&mut bytes)?;
                for (v, b) in chunk.iter_mut().zip(bytes.chunks_exact(4)) {
                    *v = i32::from_le_bytes(b.try_into().unwrap());
                }
            }
            true
        } else {
            false
        };
        if !cached {
            let rows = size.div_ceil(threads.min(size));
            std::thread::scope(|s| {
                for (part, out) in heights.chunks_mut(rows * size).enumerate() {
                    let terrain = &terrain;
                    s.spawn(move || {
                        for (i, v) in out.iter_mut().enumerate() {
                            let x = min[0] + (i % size) as i64;
                            let z = min[1] + (part * rows + i / size) as i64;
                            *v = (terrain
                                .height_m((x as f64 + 0.5) * spacing, (z as f64 + 0.5) * spacing)
                                / VOXEL_SIZE) as i32;
                        }
                    });
                }
            });
            if let Some(path) = cache {
                let temporary = path.with_extension("partial");
                let mut writer = BufWriter::new(File::create(&temporary)?);
                writer.write_all(b"FVNAT001")?;
                writer.write_all(&hash.to_le_bytes())?;
                for chunk in heights.chunks(65536) {
                    let bytes: Vec<u8> = chunk.iter().flat_map(|v| v.to_le_bytes()).collect();
                    writer.write_all(&bytes)?;
                }
                writer.flush()?;
                std::fs::rename(temporary, path)?;
            }
        }
        let n = size / BLOCK;
        let mut base = vec![i32::MIN; n * n];
        for z in 0..size {
            for x in 0..size {
                let v = &mut base[(z / BLOCK) * n + x / BLOCK];
                *v = (*v).max(heights[z * size + x]);
            }
        }
        let mut levels = vec![(n, base)];
        while levels.last().unwrap().0 > 1 {
            let (old, data) = levels.last().unwrap();
            let n = old / 2;
            let mut next = vec![i32::MIN; n * n];
            for z in 0..n {
                for x in 0..n {
                    next[z * n + x] = [
                        data[(z * 2) * old + x * 2],
                        data[(z * 2) * old + x * 2 + 1],
                        data[(z * 2 + 1) * old + x * 2],
                        data[(z * 2 + 1) * old + x * 2 + 1],
                    ]
                    .into_iter()
                    .max()
                    .unwrap();
                }
            }
            levels.push((n, next));
        }
        Ok(Self {
            min,
            size,
            cache_identity: hash,
            sample_stride,
            heights,
            levels,
            terrain,
        })
    }
    pub fn memory_bytes(&self) -> usize {
        self.heights.len() * 4 + self.levels.iter().map(|(_, v)| v.len() * 4).sum::<usize>()
    }
    fn spacing(&self) -> f64 {
        VOXEL_SIZE * self.sample_stride as f64
    }
    fn height(&self, x: i64, z: i64) -> i64 {
        let a = x - self.min[0];
        let b = z - self.min[1];
        if a >= 0 && b >= 0 && a < self.size as i64 && b < self.size as i64 {
            self.heights[b as usize * self.size + a as usize] as i64
        } else {
            (self.terrain.height_m(
                (x as f64 + 0.5) * self.spacing(),
                (z as f64 + 0.5) * self.spacing(),
            ) / VOXEL_SIZE) as i64
        }
    }
    fn hit(&self, x: i64, z: i64, y: i64, t: f64, face: usize, h: i64) -> Hit {
        let surface = || {
            self.terrain.surface_material(
                (x as f64 + 0.5) * self.spacing(),
                (z as f64 + 0.5) * self.spacing(),
                h as f64 * VOXEL_SIZE,
            )
        };
        let material = if y == h - 1 {
            surface()
        } else if y >= h - 12 {
            if self.terrain.params().recipe == frontier_world::FbmRecipe::Alpine
                && surface() == Material::Stone
            {
                Material::Stone
            } else {
                Material::Soil
            }
        } else {
            Material::Stone
        };
        Hit {
            cell: [
                x * self.sample_stride as i64 + (self.sample_stride / 2) as i64,
                y,
                z * self.sample_stride as i64 + (self.sample_stride / 2) as i64,
            ],
            distance: t,
            face,
            material: material_id(material),
        }
    }
    pub fn ray(&self, o: [f64; 3], d: [f64; 3], near: f64, far: f64) -> Option<Hit> {
        if !o
            .iter()
            .chain(d.iter())
            .chain([near, far].iter())
            .all(|v| v.is_finite())
            || near < 0.
            || far < near
        {
            return None;
        }
        let len = d.iter().map(|v| v * v).sum::<f64>().sqrt();
        if len == 0. {
            return None;
        }
        let d = d.map(|v| v / len);
        self.node(self.levels.len() - 1, 0, 0, o, d, near, far)
    }
    fn interval(
        &self,
        level: usize,
        x: usize,
        z: usize,
        o: [f64; 3],
        d: [f64; 3],
        near: f64,
        far: f64,
    ) -> Option<(f64, f64)> {
        let span = BLOCK << level;
        let mut lo = near;
        let mut hi = far;
        for (axis, p, m) in [(0, x, self.min[0]), (2, z, self.min[1])] {
            let a = (m + (p * span) as i64) as f64 * self.spacing();
            let b = a + span as f64 * self.spacing();
            if d[axis] == 0. {
                if o[axis] < a || o[axis] >= b {
                    return None;
                }
            } else {
                let u = (a - o[axis]) / d[axis];
                let v = (b - o[axis]) / d[axis];
                lo = lo.max(u.min(v));
                hi = hi.min(u.max(v));
            }
        }
        let (n, maxima) = &self.levels[level];
        let top = maxima[z * n + x] as f64 * VOXEL_SIZE;
        if d[1] == 0. {
            if o[1] >= top {
                return None;
            }
        } else if d[1] < 0. {
            lo = lo.max((top - o[1]) / d[1]);
        } else {
            hi = hi.min((top - o[1]) / d[1]);
        }
        if lo <= hi {
            Some((lo, hi))
        } else {
            None
        }
    }
    fn node(
        &self,
        level: usize,
        x: usize,
        z: usize,
        o: [f64; 3],
        d: [f64; 3],
        near: f64,
        far: f64,
    ) -> Option<Hit> {
        let (lo, hi) = self.interval(level, x, z, o, d, near, far)?;
        if level == 0 {
            return self.leaf(x, z, o, d, lo, hi);
        }
        let mut children = [(f64::INFINITY, 0usize, 0usize); 4];
        let mut i = 0;
        for dz in 0..2 {
            for dx in 0..2 {
                let xx = x * 2 + dx;
                let zz = z * 2 + dz;
                if let Some((t, _)) = self.interval(level - 1, xx, zz, o, d, lo, hi) {
                    children[i] = (t, xx, zz);
                    i += 1;
                }
            }
        }
        children[..i].sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        for &(_, xx, zz) in &children[..i] {
            if let Some(hit) = self.node(level - 1, xx, zz, o, d, lo, hi) {
                return Some(hit);
            }
        }
        None
    }
    fn leaf(
        &self,
        bx: usize,
        bz: usize,
        o: [f64; 3],
        d: [f64; 3],
        lo: f64,
        hi: f64,
    ) -> Option<Hit> {
        let ox = o[0] / self.spacing();
        let oz = o[2] / self.spacing();
        let dx = d[0] / self.spacing();
        let dz = d[2] / self.spacing();
        let mut x = forward_index(ox, dx, lo).0;
        let mut z = forward_index(oz, dz, lo).0;
        let mut t = lo;
        let minx = self.min[0] + (bx * BLOCK) as i64;
        let minz = self.min[1] + (bz * BLOCK) as i64;
        while x >= minx
            && x < minx + BLOCK as i64
            && z >= minz
            && z < minz + BLOCK as i64
            && t <= hi
        {
            let h = self.height(x, z);
            let top = h as f64 * VOXEL_SIZE;
            let tx = if dx == 0. {
                f64::INFINITY
            } else {
                ((if dx > 0. { x + 1 } else { x }) as f64 - ox) / dx
            };
            let tz = if dz == 0. {
                f64::INFINITY
            } else {
                ((if dz > 0. { z + 1 } else { z }) as f64 - oz) / dz
            };
            let end = hi.min(tx.min(tz));
            // Entering a column side is a hit only when its canonical neighbor
            // is air. This also prevents artificial walls at crop/block edges.
            let mut candidate: Option<Hit> = None;
            for (axis, c, dc, oc) in [(0, x, dx, ox), (2, z, dz, oz)] {
                if dc == 0. {
                    continue;
                }
                let plane = if dc > 0. { c } else { c + 1 };
                let entry = (plane as f64 - oc) / dc;
                if entry == t {
                    let y = forward_index(o[1] / VOXEL_SIZE, d[1] / VOXEL_SIZE, t).0;
                    let step = if dc > 0. { 1 } else { -1 };
                    let neighbor = if axis == 0 {
                        self.height(x - step, z)
                    } else {
                        self.height(x, z - step)
                    };
                    if y < h && y >= neighbor && t < tx.min(tz) {
                        let hit = self.hit(x, z, y, t, axis * 2 + usize::from(dc < 0.), h);
                        if candidate.is_none_or(|old| hit.face < old.face) {
                            candidate = Some(hit);
                        }
                    }
                }
            }
            if d[1] < 0. {
                let mut tt = (top - o[1]) / d[1];
                // Anisotropic sampled columns can round a simultaneous top/side
                // crossing a few ulps apart. Close the sampled target's cracks;
                // retain the exact oracle's historical native tie semantics.
                let epsilon = if self.sample_stride > 1 {
                    8. * f64::EPSILON * t.abs().max(end.abs()).max(1.)
                } else {
                    0.
                };
                if tt < t && t - tt <= epsilon {
                    tt = t;
                }
                let owns_top =
                    tt < tx.min(tz) || (self.sample_stride > 1 && tt <= tx.min(tz) + epsilon);
                if tt >= t && tt <= end + epsilon && owns_top {
                    let hit = self.hit(x, z, h - 1, tt, 3, h);
                    if candidate.is_none_or(|old| {
                        tt < old.distance || (tt == old.distance && hit.face < old.face)
                    }) {
                        candidate = Some(hit);
                    }
                }
            }
            if candidate.is_some() {
                return candidate;
            }
            let next = tx.min(tz);
            if next > hi || !next.is_finite() {
                break;
            }
            if tx == next {
                x += if dx > 0. { 1 } else { -1 };
            }
            if tz == next {
                z += if dz > 0. { 1 } else { -1 };
            }
            t = next;
        }
        None
    }
}

/// Explicitly partitioned offline artistic target. The native square owns every
/// ray segment inside its footprint; the sampled far surface owns only outside.
/// This is an approximate heightfield target, never a full-scene native oracle.
pub struct MultiScaleLandscape {
    pub far: Landscape,
    pub near: Option<Landscape>,
}
impl MultiScaleLandscape {
    pub fn memory_bytes(&self) -> usize {
        self.far.memory_bytes() + self.near.as_ref().map_or(0, Landscape::memory_bytes)
    }
    pub fn ray(&self, o: [f64; 3], d: [f64; 3], near: f64, far: f64) -> Option<Hit> {
        let Some(native) = &self.near else {
            return self.far.ray(o, d, near, far);
        };
        let length = d.iter().map(|v| v * v).sum::<f64>().sqrt();
        if !length.is_finite() || length == 0. {
            return None;
        }
        let d = d.map(|v| v / length);
        let mut enter = near;
        let mut exit = far;
        for (axis, index) in [(0, 0), (2, 1)] {
            let a = native.min[index] as f64 * VOXEL_SIZE;
            let b = a + native.size as f64 * VOXEL_SIZE;
            if d[axis] == 0. {
                if o[axis] < a || o[axis] >= b {
                    return self.far.ray(o, d, near, far);
                }
            } else {
                let t0 = (a - o[axis]) / d[axis];
                let t1 = (b - o[axis]) / d[axis];
                enter = enter.max(t0.min(t1));
                exit = exit.min(t0.max(t1));
            }
        }
        if enter > exit {
            return self.far.ray(o, d, near, far);
        }
        if enter > near {
            if let Some(hit) = self.far.ray(o, d, near, enter) {
                if hit.distance < enter {
                    return Some(hit);
                }
            }
        }
        if let Some(hit) = native.ray(o, d, enter, exit) {
            if hit.distance < exit || exit == far {
                return Some(hit);
            }
        }
        if exit < far {
            self.far.ray(o, d, exit, far)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{normalized, Snapshot};
    use frontier_world::{FbmParams, FbmRecipe, World};
    #[test]
    fn exact_columns_match_dense_native_snapshot() {
        let params = FbmParams {
            recipe: FbmRecipe::Regional,
            amplitude: 12.,
            horizontal_scale: 16.,
            ..Default::default()
        };
        let world = World::fbm(7, params).unwrap();
        let terrain = world.fbm_terrain().unwrap().clone();
        let landscape = Landscape::build(terrain, [-32, -32], 64, 2, None).unwrap();
        let dense = Snapshot::from_world(&world, [-32, -256, -32], [64, 512, 64]);
        for i in 0..2500 {
            let a = i as f64 * 2.399963229728653;
            let o = [a.cos() * 5., 6. + (i % 17) as f64 * 0.01, a.sin() * 5.];
            let target = [a.sin() * 1.8, 0.2, a.cos() * 1.8];
            let d = normalized(std::array::from_fn(|a| target[a] - o[a]));
            let exact = dense.ray(o, d, 0.025, 30., true);
            let column = landscape.ray(o, d, 0.025, 30.);
            assert_eq!(
                exact.is_some(),
                column.is_some(),
                "ray{i}: {exact:?} {column:?}"
            );
            if let (Some(a), Some(b)) = (exact, column) {
                assert_eq!(a.cell, b.cell, "ray{i}");
                assert_eq!(a.face, b.face, "ray{i}");
                assert_eq!(a.material, b.material, "ray{i}");
                assert!(
                    (a.distance - b.distance).abs() < 1e-9,
                    "ray{i}: {a:?} {b:?}"
                );
            }
        }
    }
    #[test]
    fn crop_does_not_invent_walls_and_vertical_top_is_exact() {
        let terrain = FbmTerrain::new(
            7,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        let scene = Landscape::build(terrain, [-16, -16], 32, 1, None).unwrap();
        assert!(scene.ray([-2., -1., 0.], [1., 0., 0.], 0., 10.).is_none());
        let hit = scene.ray([0., 2., 0.], [0., -1., 0.], 0., 10.).unwrap();
        assert_eq!(hit.face, 3);
        assert_eq!(hit.cell, [0, -1, 0]);
        assert_eq!(hit.distance, 2.);
    }
    #[test]
    fn tied_top_and_horizontal_planes_match_dense() {
        for split_axis in [0, 2] {
            let terrain = FbmTerrain::new(
                7,
                FbmParams {
                    amplitude: 0.,
                    ..Default::default()
                },
            )
            .unwrap();
            let mut scene = Landscape::build(terrain, [-16, -16], 32, 1, None).unwrap();
            for z in 0..32 {
                for x in 0..32 {
                    scene.heights[z * 32 + x] = if if split_axis == 0 { x < 16 } else { z >= 16 } {
                        2
                    } else {
                        0
                    };
                }
            }
            for z in 0..2 {
                for x in 0..2 {
                    let mut top = i32::MIN;
                    for dz in 0..16 {
                        for dx in 0..16 {
                            top = top.max(scene.heights[(z * 16 + dz) * 32 + x * 16 + dx]);
                        }
                    }
                    scene.levels[0].1[z * 2 + x] = top;
                }
            }
            scene.levels[1].1[0] = 2;
            let dense = Snapshot::build([-16, -16, -16], [32, 32, 32], |p| {
                let h = if if split_axis == 0 { p[0] < 0 } else { p[2] >= 0 } {
                    2
                } else {
                    0
                };
                if p[1] >= h {
                    0
                } else {
                    scene.hit(p[0], p[2], p[1], 0., 3, h).material
                }
            });
            let mut o = [0.03125, 0.25, 0.03125];
            o[split_axis] = -0.125;
            let mut d = [0., -1., 0.];
            d[split_axis] = 1.;
            let a = dense.ray(o, d, 0., 10., true);
            let b = scene.ray(o, d, 0., 10.);
            assert_eq!(a, b, "axis{split_axis}");
        }
    }
    #[test]
    fn disk_cache_rejects_other_seed() {
        let path = std::env::temp_dir().join(format!(
            "frontier-landscape-cache-{}.bin",
            std::process::id()
        ));
        let make = |seed| FbmTerrain::new(seed, FbmParams::default()).unwrap();
        let a = Landscape::build(make(7), [0, 0], 16, 1, Some(&path)).unwrap();
        let b = Landscape::build(make(7), [0, 0], 16, 1, Some(&path)).unwrap();
        assert_eq!(a.heights, b.heights);
        assert!(Landscape::build(make(8), [0, 0], 16, 1, Some(&path)).is_err());
        std::fs::remove_file(path).unwrap();
    }
    fn flat(stride: usize, min: [i64; 2], size: usize) -> Landscape {
        let terrain = FbmTerrain::new(
            7,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        Landscape::build_sampled(terrain, min, size, stride, 1, None).unwrap()
    }
    fn set_height(scene: &mut Landscape, height: i32) {
        scene.heights.fill(height);
        for (_, level) in &mut scene.levels {
            level.fill(height);
        }
    }
    #[test]
    fn sampled_grid_matches_native_height_and_world_footprint() {
        let scene = flat(16, [-16, -16], 32);
        for x in [-15.5, -0.5, 0.5, 15.5] {
            let hit = scene.ray([x, 2., 0.5], [0., -1., 0.], 0., 10.).unwrap();
            assert_eq!(hit.distance, 2.);
            assert_eq!(hit.cell[0], (x.floor() as i64) * 16 + 8);
            assert_eq!(hit.cell[1], -1);
        }
        assert!(scene.ray([16.5, 2., 0.5], [0., -1., 0.], 0., 10.).is_none());
        assert!(
            scene
                .ray([-16., -1., 0.5], [1., 0., 0.], 0., 100.)
                .is_none(),
            "no artificial crop wall"
        );
    }
    #[test]
    fn hybrid_ownership_excludes_coarse_occluder_inside_native_square() {
        let mut far = flat(16, [-16, -16], 32);
        set_height(&mut far, 16);
        let near = flat(1, [-16, -16], 32);
        let scene = MultiScaleLandscape {
            far,
            near: Some(near),
        };
        let hit = scene.ray([0.5, 2., 0.5], [0., -1., 0.], 0., 10.).unwrap();
        assert_eq!(
            hit.distance, 2.,
            "coarse 1m top must not occlude owned native surface"
        );
        let outside = scene.ray([1.5, 2., 0.5], [0., -1., 0.], 0., 10.).unwrap();
        assert_eq!(outside.distance, 1.);
        let before_entry = scene.ray([-3., 2., 0.5], [1., -1., 0.], 0., 10.).unwrap();
        assert!((before_entry.distance - 2f64.sqrt()).abs() < 1e-12);
        let after_exit = scene.ray([0.5, 3., 0.5], [1., -1., 0.], 0., 10.).unwrap();
        assert!((after_exit.distance - 2. * 2f64.sqrt()).abs() < 1e-12);
    }
    #[test]
    fn sampled_cache_rejects_other_stride() {
        let path =
            std::env::temp_dir().join(format!("frontier-sampled-cache-{}.bin", std::process::id()));
        let terrain = FbmTerrain::new(7, FbmParams::default()).unwrap();
        Landscape::build_sampled(terrain.clone(), [0, 0], 16, 8, 1, Some(&path)).unwrap();
        assert!(Landscape::build_sampled(terrain, [0, 0], 16, 16, 1, Some(&path)).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn sampled_oblique_rays_match_independent_column_boxes() {
        let mut scene = flat(16, [-16, -16], 32);
        for z in 0..32 {
            for x in 0..32 {
                scene.heights[z * 32 + x] = if (x / 3 + z / 2) % 2 == 0 { 16 } else { 0 };
            }
        }
        // All cells are at most 1m; conservative hierarchy suffices for this test.
        for (_, level) in &mut scene.levels {
            level.fill(16);
        }
        for i in 0..100 {
            let o = [
                (i % 7) as f64 * 0.41 - 1.3,
                2.,
                (i % 11) as f64 * 0.27 - 1.2,
            ];
            let d = crate::normalized([
                (i % 9) as f64 * 0.17 - 0.6,
                -1.,
                (i % 13) as f64 * 0.11 - 0.6,
            ]);
            let mut expected = f64::INFINITY;
            for z in 0..32 {
                for x in 0..32 {
                    let a = [x as f64 - 16., -10., z as f64 - 16.];
                    let b = [
                        a[0] + 1.,
                        scene.heights[z * 32 + x] as f64 * VOXEL_SIZE,
                        a[2] + 1.,
                    ];
                    let mut lo = 0f64;
                    let mut hi = 5f64;
                    for axis in 0..3 {
                        if d[axis] == 0. {
                            if o[axis] < a[axis] || o[axis] >= b[axis] {
                                hi = -1.;
                            }
                        } else {
                            let t0 = (a[axis] - o[axis]) / d[axis];
                            let t1 = (b[axis] - o[axis]) / d[axis];
                            lo = lo.max(t0.min(t1));
                            hi = hi.min(t0.max(t1));
                        }
                    }
                    if lo <= hi {
                        expected = expected.min(lo);
                    }
                }
            }
            let hit = scene.ray(o, d, 0., 5.).unwrap_or_else(|| {
                panic!("miss ray {i}, origin {o:?} direction {d:?} expected {expected}")
            });
            assert!(
                (hit.distance - expected).abs() < 1e-10,
                "ray {i}: {hit:?}, expected {expected}"
            );
        }
    }
    #[test]
    fn alpine_reference_subsurface_material_matches_native_world() {
        let params = FbmParams {
            recipe: FbmRecipe::Alpine,
            ..Default::default()
        };
        let world = World::fbm(7, params).unwrap();
        let terrain = world.fbm_terrain().unwrap().clone();
        let scene = Landscape::build(terrain.clone(), [0, 0], 16, 1, None).unwrap();
        let mut stone_columns = 0;
        let mut covered_columns = 0;
        for z in 0..16 {
            for x in -8..8 {
                let cx = x * 4096 + 1;
                let cz = z * 4096 + 1;
                let h = (terrain.height_m(
                    (cx as f64 + 0.5) * VOXEL_SIZE,
                    (cz as f64 + 0.5) * VOXEL_SIZE,
                ) / VOXEL_SIZE) as i64;
                let material = terrain.surface_material(
                    (cx as f64 + 0.5) * VOXEL_SIZE,
                    (cz as f64 + 0.5) * VOXEL_SIZE,
                    h as f64 * VOXEL_SIZE,
                );
                stone_columns += usize::from(material == Material::Stone);
                covered_columns += usize::from(material != Material::Stone);
                for dy in [1, 2, 11, 12, 13, 24] {
                    let cell = frontier_world::Cell {
                        x: cx,
                        y: h - dy,
                        z: cz,
                    };
                    assert_eq!(
                        scene.hit(cx, cz, cell.y, 1., 0, h).material,
                        material_id(world.material(cell)),
                        "{cell:?}"
                    );
                }
            }
        }
        assert!(
            stone_columns > 0 && covered_columns > 0,
            "both exposed and covered landforms must be checked"
        );
    }
}
