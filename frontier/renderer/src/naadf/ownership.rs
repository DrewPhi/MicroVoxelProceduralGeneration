//! Canonical certificate for replacing complete heightfield tiles, not sampled
//! screen visibility. A one-chunk apron protects exposed walls at tile edges.
use crate::adaptive_geometry::TileKey;
use frontier_world::{
    vegetation::{PlantRecord, Species, MAX_PLANT_HEIGHT_CELLS, MAX_PLANT_RADIUS_CELLS},
    VOXEL_SIZE,
};
use std::sync::atomic::{AtomicU64, Ordering};
#[derive(Debug)]
pub(crate) struct Ownership {
    pub id: u64,
    pub bounds: [[f64; 3]; 2],
    n: usize,
    unsafe_prefix: Vec<u32>,
}
impl Ownership {
    pub fn new(bounds: [[f64; 3]; 2], n: usize, safe: &[bool]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let mut prefix = vec![0; (n + 1) * (n + 1)];
        for z in 0..n {
            for x in 0..n {
                let i = (z + 1) * (n + 1) + x + 1;
                prefix[i] = u32::from(!safe[z * n + x]) + prefix[i - 1] + prefix[i - n - 1]
                    - prefix[i - n - 2];
            }
        }
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            bounds,
            n,
            unsafe_prefix: prefix,
        }
    }
    pub fn contains(&self, b: [[f64; 3]; 2]) -> bool {
        (0..3).all(|a| {
            b[0][a].is_finite()
                && b[1][a].is_finite()
                && b[0][a] >= self.bounds[0][a] + VOXEL_SIZE
                && b[1][a] <= self.bounds[1][a] - VOXEL_SIZE
        })
    }
    pub fn terrain(&self, key: TileKey) -> bool {
        let min = [
            key.min[0] as f64 * VOXEL_SIZE,
            key.min[1] as f64 * VOXEL_SIZE,
        ];
        let width = key.width_cells as f64 * VOXEL_SIZE;
        let mut lo = [0usize; 2];
        let mut hi = lo;
        for a in 0..2 {
            let origin = self.bounds[0][a * 2];
            let l = (min[a] - origin).floor() as i64 - 1;
            let h = (min[a] + width - origin).ceil() as i64 + 1;
            if l < 0 || h > self.n as i64 {
                return false;
            }
            lo[a] = l as usize;
            hi[a] = h as usize;
        }
        let stride = self.n + 1;
        self.unsafe_prefix[hi[1] * stride + hi[0]] + self.unsafe_prefix[lo[1] * stride + lo[0]]
            == self.unsafe_prefix[hi[1] * stride + lo[0]]
                + self.unsafe_prefix[lo[1] * stride + hi[0]]
    }
    pub fn plant(&self, p: &PlantRecord) -> bool {
        let (r, h) = if p.species == Species::Rowan {
            (MAX_PLANT_RADIUS_CELLS, MAX_PLANT_HEIGHT_CELLS)
        } else {
            (8, 12)
        };
        self.contains([
            [
                p.root.x as f64 * VOXEL_SIZE - r as f64 * VOXEL_SIZE,
                p.root.y as f64 * VOXEL_SIZE,
                p.root.z as f64 * VOXEL_SIZE - r as f64 * VOXEL_SIZE,
            ],
            [
                (p.root.x + r + 1) as f64 * VOXEL_SIZE,
                (p.root.y + h + 1) as f64 * VOXEL_SIZE,
                (p.root.z + r + 1) as f64 * VOXEL_SIZE,
            ],
        ])
    }
    pub fn ground(&self, x: i64, z: i64, low: i64, high: i64) -> bool {
        self.contains([
            [
                (x * 128 - 8) as f64 * VOXEL_SIZE,
                low as f64 * VOXEL_SIZE,
                (z * 128 - 8) as f64 * VOXEL_SIZE,
            ],
            [
                ((x + 1) * 128 + 8) as f64 * VOXEL_SIZE,
                (high + 13) as f64 * VOXEL_SIZE,
                ((z + 1) * 128 + 8) as f64 * VOXEL_SIZE,
            ],
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(x: i64, z: i64, width: u32) -> TileKey {
        TileKey {
            min: [x, z],
            width_cells: width,
            step_cells: 1,
            edge_steps: [1; 4],
            source_revision: 0,
        }
    }
    #[test]
    fn unsafe_columns_apron_and_negative_world_coordinates_are_conservative() {
        let mut safe = vec![true; 16 * 16];
        let bounds = [[-8., -10., -8.], [8., 10., 8.]];
        let clean = Ownership::new(bounds, 16, &safe);
        assert!(clean.terrain(key(-64, -64, 64)));
        assert!(!clean.terrain(key(-128, -64, 64))); // no outside apron
        safe[5 * 16 + 5] = false;
        let dirty = Ownership::new(bounds, 16, &safe);
        assert!(!dirty.terrain(key(-64, -64, 64)));
        assert!(dirty.terrain(key(32, 32, 32)));
    }
    #[test]
    fn delegated_terrain_builds_no_mesh_and_can_restore_the_normal_cache() {
        let world = frontier_world::World::fbm(
            7,
            frontier_world::FbmParams {
                amplitude: 1.,
                ..Default::default()
            },
        )
        .unwrap();
        let mut cache = crate::adaptive_geometry::GeometryCache::new(world, 64 * 1048576);
        let owner = std::sync::Arc::new(Ownership::new(
            [[-8., -20., -8.], [8., 20., 8.]],
            16,
            &vec![true; 256],
        ));
        cache.set_naadf_owner(Some(owner));
        let k = key(0, 0, 32);
        let tile = cache.get_or_build(k, || false).unwrap();
        assert!(tile.naadf_owned && tile.is_empty() && tile.sampled_columns == 0);
        assert_eq!(cache.stats().builds, 0);
        cache.set_naadf_owner(None);
        let restored = cache.get_or_build(k, || false).unwrap();
        assert!(!restored.naadf_owned && !restored.is_empty());
    }
}
