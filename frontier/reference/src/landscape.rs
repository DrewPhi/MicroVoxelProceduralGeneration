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
        if !size.is_power_of_two() || size < BLOCK || size > 32768 || threads == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "size must be power of two in16..32768 cells; threads>0",
            ));
        }
        let identity = format!(
            "native-landscape-v1:{:?}:{:?}:{}:{min:?}:{size}:{}:{}",
            terrain.params(),
            terrain.seed(),
            terrain.source_version(),
            include_str!("../../world/src/terrain.rs"),
            include_str!("../../world/src/lib.rs")
        );
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
                            *v = (terrain.height_m(
                                (x as f64 + 0.5) * VOXEL_SIZE,
                                (z as f64 + 0.5) * VOXEL_SIZE,
                            ) / VOXEL_SIZE) as i32;
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
            heights,
            levels,
            terrain,
        })
    }
    pub fn memory_bytes(&self) -> usize {
        self.heights.len() * 4 + self.levels.iter().map(|(_, v)| v.len() * 4).sum::<usize>()
    }
    fn height(&self, x: i64, z: i64) -> i64 {
        let a = x - self.min[0];
        let b = z - self.min[1];
        if a >= 0 && b >= 0 && a < self.size as i64 && b < self.size as i64 {
            self.heights[b as usize * self.size + a as usize] as i64
        } else {
            (self
                .terrain
                .height_m((x as f64 + 0.5) * VOXEL_SIZE, (z as f64 + 0.5) * VOXEL_SIZE)
                / VOXEL_SIZE) as i64
        }
    }
    fn hit(&self, x: i64, z: i64, y: i64, t: f64, face: usize, h: i64) -> Hit {
        let material = if y == h - 1 {
            self.terrain.surface_material(
                (x as f64 + 0.5) * VOXEL_SIZE,
                (z as f64 + 0.5) * VOXEL_SIZE,
                h as f64 * VOXEL_SIZE,
            )
        } else if y >= h - 12 {
            Material::Soil
        } else {
            Material::Stone
        };
        Hit {
            cell: [x, y, z],
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
            let a = (m + (p * span) as i64) as f64 * VOXEL_SIZE;
            let b = a + span as f64 * VOXEL_SIZE;
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
        let ox = o[0] / VOXEL_SIZE;
        let oz = o[2] / VOXEL_SIZE;
        let dx = d[0] / VOXEL_SIZE;
        let dz = d[2] / VOXEL_SIZE;
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
                let tt = (top - o[1]) / d[1];
                if tt >= t && tt <= end && tt < tx.min(tz) {
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
}
