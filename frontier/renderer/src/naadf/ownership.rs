//! Canonical certificate for replacing complete heightfield tiles, not sampled
//! screen visibility. A one-chunk apron protects exposed walls at tile edges.
use crate::adaptive_geometry::TileKey;
use frontier_world::{
    vegetation::{PlantRecord, Species, MAX_PLANT_HEIGHT_CELLS, MAX_PLANT_RADIUS_CELLS},
    VOXEL_SIZE,
};
use std::sync::atomic::{AtomicU64, Ordering};
/// A raster fallback is only needed for objects intersecting the part of
/// the old exact region that will no longer be owned after publication.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Retirement {
    pub old: [[f64; 3]; 2],
    pub kept: [[f64; 3]; 2],
}
impl Retirement {
    pub fn touches(self, bounds: [[f64; 3]; 2]) -> bool {
        let lo: [f64; 3] = std::array::from_fn(|a| (self.old[0][a] + VOXEL_SIZE).max(bounds[0][a]));
        let hi: [f64; 3] = std::array::from_fn(|a| (self.old[1][a] - VOXEL_SIZE).min(bounds[1][a]));
        (0..3).all(|a| lo[a] < hi[a])
            && (0..3).any(|a| {
                lo[a] < self.kept[0][a] + VOXEL_SIZE || hi[a] > self.kept[1][a] - VOXEL_SIZE
            })
    }
}
#[derive(Debug)]
pub(crate) struct Ownership {
    pub id: u64,
    pub bounds: [[f64; 3]; 2],
    n: usize,
    grid_origin: [f64; 2],
    unsafe_prefix: std::sync::Arc<Vec<u32>>,
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
            unsafe_prefix: std::sync::Arc::new(prefix),
            grid_origin: [bounds[0][0], bounds[0][2]],
        }
    }
    pub fn crop(&self, bounds: [[f64; 3]; 2]) -> std::sync::Arc<Self> {
        let mut s = Self::new(bounds, 0, &[]);
        s.n = self.n;
        s.grid_origin = self.grid_origin;
        s.unsafe_prefix = self.unsafe_prefix.clone();
        std::sync::Arc::new(s)
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
            if min[a] - 1. < self.bounds[0][a * 2] || min[a] + width + 1. > self.bounds[1][a * 2] {
                return false;
            }
            let origin = self.grid_origin[a];
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
        self.contains(Self::plant_bounds(p))
    }
    pub fn plant_bounds(p: &PlantRecord) -> [[f64; 3]; 2] {
        let (r, h) = if p.species == Species::Rowan {
            (MAX_PLANT_RADIUS_CELLS, MAX_PLANT_HEIGHT_CELLS)
        } else {
            (8, 12)
        };
        [
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
        ]
    }
    pub fn ground(&self, x: i64, z: i64, low: i64, high: i64) -> bool {
        self.contains(Self::ground_bounds(x, z, low, high))
    }
    pub fn ground_bounds(x: i64, z: i64, low: i64, high: i64) -> [[f64; 3]; 2] {
        [
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
        ]
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
    #[test]
    fn retirement_only_blocks_objects_touching_outgoing_voxels() {
        let r = Retirement {
            old: [[0.; 3], [16.; 3]],
            kept: [[2., 0., 0.], [16.; 3]],
        };
        assert!(r.touches([[1., 4., 4.], [3., 5., 5.]]));
        assert!(!r.touches([[4.; 3], [6.; 3]]));
        assert!(!r.touches([[17.; 3], [20.; 3]]));
        assert!(!r.touches([[4., 4., 4.], [20., 5., 5.]]));
    }
}
