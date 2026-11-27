//! A fixed world-space control lattice defining exact integer voxel columns.
use crate::{terrain::TerrainColumn, FbmTerrain};
use std::cell::RefCell;

pub mod detail;

pub const PITCH_CELLS: i64 = 128;
pub const RELIEF_MAX: i64 = 3;
pub const DRY_HEAD_DROP: i64 = 12;

pub fn relief_seed(seed: u64) -> u32 {
    (seed ^ (seed >> 32)) as u32
}

pub fn interpolate(corners: [i64; 4], local: [i64; 2], width: i64) -> i64 {
    let [u, v] = local.map(|p| 2 * p + 1);
    let d = 2 * width;
    let [a, b, c, e] = corners;
    let north = a * (d - u) + b * u;
    let south = c * (d - u) + e * u;
    (north * (d - v) + south * v).div_euclid(d * d)
}

fn hash(x: i64, z: i64, seed: u32) -> i32 {
    let mut v = (x as u32).wrapping_mul(0x9e3779b9) ^ (z as u32).wrapping_mul(0x85ebca6b) ^ seed;
    v = (v ^ (v >> 16)).wrapping_mul(0x7feb352d);
    v = (v ^ (v >> 15)).wrapping_mul(0x846ca68b);
    ((v ^ (v >> 16)) & 255) as i32 - 128
}

fn band([x, z]: [i64; 2], power: u32, seed: u32) -> i32 {
    let width = 1 << power;
    let [u, v] = [x, z].map(|c| c as i32 & (width - 1));
    let [x, z] = [x >> power, z >> power];
    let a = hash(x, z, seed) * (width - u) + hash(x + 1, z, seed) * u;
    let b = hash(x, z + 1, seed) * (width - u) + hash(x + 1, z + 1, seed) * u;
    (a * (width - v) + b * v) >> (2 * power)
}

pub fn relief(cell: [i64; 2], seed: u32) -> i64 {
    i64::from((3 * band(cell, 4, seed) + band(cell, 2, seed ^ 0x2d1189a1)) >> 7)
}

#[derive(Clone, Copy)]
struct CachedControl {
    origin: [i64; 2],
    column: TerrainColumn,
}

thread_local! {
    // Thread-local sampling also serves physics and the parallel native
    // caches. Identical source clones share the key without holding a lock.
    static CONTROLS: RefCell<(u64, Vec<Option<CachedControl>>)> =
        RefCell::new((0, vec![None; 4096]));
}

pub(crate) fn column(terrain: &FbmTerrain, identity: u64, cell: [i64; 2]) -> TerrainColumn {
    let local = cell.map(|p| p.rem_euclid(PITCH_CELLS));
    let origin = [cell[0] - local[0], cell[1] - local[1]];
    let corners = CONTROLS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.0 != identity {
            cache.0 = identity;
            cache.1.fill(None);
        }
        [
            [0, 0],
            [PITCH_CELLS, 0],
            [0, PITCH_CELLS],
            [PITCH_CELLS, PITCH_CELLS],
        ]
        .map(|delta| {
            let point = [origin[0] + delta[0], origin[1] + delta[1]];
            let slot = &mut cache.1[hash(point[0] / PITCH_CELLS, point[1] / PITCH_CELLS, 0)
                as usize
                & 255
                | ((point[1] / PITCH_CELLS) as usize & 15) << 8];
            if let Some(sample) = slot.filter(|sample| sample.origin == point) {
                return sample.column;
            }
            let column = terrain.lattice_control(point);
            *slot = Some(CachedControl {
                origin: point,
                column,
            });
            column
        })
    });
    let mut result = corners
        [usize::from(local[0] >= PITCH_CELLS / 2) + 2 * usize::from(local[1] >= PITCH_CELLS / 2)];
    result.ground_top = interpolate(corners.map(|c| c.ground_top), local, PITCH_CELLS);
    let carrier = result.ground_top;
    let detailed = terrain.native_detail_enabled();
    if terrain.lattice_relief_at(origin) {
        let seed = relief_seed(terrain.seed());
        result.ground_top += if detailed {
            let tile = detail::tile_index(origin.map(|v| v / PITCH_CELLS), seed);
            detail::column(tile, local[0] as usize, local[1] as usize)
        } else {
            relief(cell, seed)
        };
    }
    result.visible_top = result.ground_top;
    if corners.iter().any(|c| c.visible_top > c.ground_top) {
        let heads = corners.map(|c| {
            if c.visible_top > c.ground_top {
                c.visible_top
            } else {
                c.ground_top - DRY_HEAD_DROP
            }
        });
        let water = interpolate(heads, local, PITCH_CELLS);
        if detailed && water > carrier {
            // Maintain connected channels through small-scale bed relief.
            result.ground_top = result.ground_top.min(water - 1);
        }
        result.visible_top = result.ground_top.max(water);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::affine_bounds::{bound_native_height, BoundBudget, ColumnBox};
    use crate::{voxel_size, DirtRunway, FbmParams, FbmRecipe, Material};

    #[test]
    fn native_lattice_covers_negative_coast_and_flattens_runway() {
        let terrain = FbmTerrain::new(
            7,
            FbmParams {
                recipe: FbmRecipe::Eroded,
                ..Default::default()
            },
        )
        .unwrap()
        .with_native_lattice();
        let coast = [-144 * 16 + 9, 24 * 16 + 9];
        let wet = terrain.column_cells(coast[0], coast[1]);
        assert!(wet.visible_top > wet.ground_top);
        assert_eq!(wet.material(wet.visible_top - 1), Material::Water);
        let bounds = bound_native_height(
            &terrain,
            ColumnBox {
                min: coast.map(|v| v - 90),
                size_cells: [180; 2],
            },
            BoundBudget::default(),
        )
        .unwrap();
        for dz in (-90..90).step_by(7) {
            for dx in (-90..90).step_by(7) {
                let column = terrain.column_cells(coast[0] + dx, coast[1] + dz);
                assert!(column.visible_top >= column.ground_top);
                assert!(column.ground_top as f64 * voxel_size() >= bounds.height_range[0]);
                assert!(column.visible_top as f64 * voxel_size() <= bounds.height_range[1]);
            }
        }
        // Source clones can share control samples. Grading the runway must
        // invalidate those same controls even with the same seed.
        let top = 640;
        let graded = terrain
            .clone()
            .with_dirt_runway(DirtRunway::new(coast, top, true));
        for dz in [-190, 0, 190] {
            for dx in [-4100, 0, 4100] {
                let column = graded.column_cells(coast[0] + dx, coast[1] + dz);
                assert_eq!((column.ground_top, column.visible_top), (top, top));
                assert_eq!(column.ground_surface, Material::Soil);
            }
        }
        assert_eq!(terrain.column_cells(coast[0], coast[1]), wet);
    }
}
