//! Canonical flora for the native terrain lattice. Positions are integer cells,
//! so terrain streaming, ray visibility and gameplay address the same plant.
use crate::{
    native_lattice::relief_seed,
    terrain::TerrainColumn,
    vegetation::{living_flora, model_for, PlantModel, PlantRecord, PlantVoxel, Species},
    Cell, FbmTerrain, Material,
};
use std::sync::{Arc, OnceLock};

pub const SITE_CELLS: i64 = 256;
pub const TREE_RADIUS: i64 = 96;
pub const GRASS_HEIGHT: i64 = 5;
pub const TEMPLATE_COUNT: usize = 16;

pub fn hash(x: i64, z: i64, seed: u32) -> u32 {
    let mut v = (x as u32).wrapping_mul(0x9e37_79b9) ^ (z as u32).wrapping_mul(0x85eb_ca6b) ^ seed;
    v = (v ^ (v >> 16)).wrapping_mul(0x7feb_352d);
    v = (v ^ (v >> 15)).wrapping_mul(0x846c_a68b);
    v ^ (v >> 16)
}

/// Every potential woody site is assigned a root and a shared native model
/// before terrain placement is evaluated. The jitter is inside its own site.
pub fn woody_site(site: [i64; 2], seed: u32) -> Option<([i64; 2], usize, u8)> {
    let [x, z] = site;
    let h = hash(x, z, seed ^ 0x7e01_3e59);
    let sx = x.rem_euclid(16);
    let sz = z.rem_euclid(16);
    let [gx, gz] = [x.div_euclid(16), z.div_euclid(16)];
    let groves = [[gx, gz], [gx + 1, gz], [gx, gz + 1], [gx + 1, gz + 1]]
        .map(|[x, z]| i64::from(hash(x, z, seed ^ 0xf7ba_a127) & 255));
    let [a, b, c, d] = groves;
    let grove = ((a * (16 - sx) + b * sx) * (16 - sz) + (c * (16 - sx) + d * sx) * sz) >> 8;
    if (h >> 24) as i64 >= 18 + (grove * 225 >> 8) {
        return None;
    }
    let root = [
        256 * x + 48 + i64::from((h & 255) * 160 >> 8),
        256 * z + 48 + i64::from(((h >> 8) & 255) * 160 >> 8),
    ];
    let kind = ((h >> 16) & 15) as usize;
    let rotation = ((h >> 20) & 3) as u8;
    Some((root, kind, rotation))
}

fn ground_pattern(tile: usize, x: usize, z: usize) -> u8 {
    let seed = (tile as u32).wrapping_mul(0x63a9_60ef);
    let tuft = hash((x >> 2) as i64, (z >> 2) as i64, seed ^ 0x502c_a93d);
    let blade = hash(x as i64, z as i64, tuft ^ 0xa832_137f);
    let density = hash((x >> 6) as i64, (z >> 6) as i64, seed ^ 0xc28d_b949) & 255;
    if (blade & 255) >= 43 + (density * 108 >> 8) {
        return 0;
    }
    let flower = (tuft & 127) == 0 && ((blade >> 8) & 15) <= 4;
    if flower {
        8 | GRASS_HEIGHT as u8
    } else {
        1 + ((blade >> 8) & 3) as u8
    }
}

pub fn grass(seed: u32, x: i64, z: i64, column: TerrainColumn) -> (i64, bool) {
    if column.visible_top != column.ground_top || column.ground_surface != Material::Grass {
        return (0, false);
    }
    use crate::native_lattice::{detail, PITCH_CELLS};
    let tile = detail::tile_index([x.div_euclid(PITCH_CELLS), z.div_euclid(PITCH_CELLS)], seed);
    let code = ground_pattern(
        tile,
        x.rem_euclid(PITCH_CELLS) as usize,
        z.rem_euclid(PITCH_CELLS) as usize,
    );
    (i64::from(code & 7), code & 8 != 0)
}

pub fn grass_material(c: Cell, seed: u32, column: TerrainColumn) -> Option<Material> {
    let y = c.y - column.ground_top;
    if !(0..GRASS_HEIGHT).contains(&y) {
        return None;
    }
    let (height, flower) = grass(seed, c.x, c.z, column);
    if y >= height {
        return None;
    }
    Some(if flower && y == height - 1 {
        Material::Flower
    } else {
        Material::Leaf
    })
}

/// Lower 16 bits preserve the original terrain bound and mean. The next
/// byte bounds the combined terrain and vegetation. The high byte records
/// the actual native plant column at the leaves.
pub fn cover_atlas() -> &'static [u32] {
    use crate::native_lattice::{detail, PITCH_CELLS};
    static ATLAS: OnceLock<Vec<u32>> = OnceLock::new();
    ATLAS.get_or_init(|| {
        let original = detail::atlas();
        let mut result = original.iter().copied().map(u32::from).collect::<Vec<_>>();
        let offset = |width: usize| (width * width - 1) / 3;
        std::thread::scope(|scope| {
            for (group, layers) in result.chunks_mut(32 * detail::NODES).enumerate() {
                scope.spawn(move || {
                    let side = PITCH_CELLS as usize;
                    for (i, levels) in layers.chunks_mut(detail::NODES).enumerate() {
                        let tile = group * 32 + i;
                        for z in 0..side {
                            for x in 0..side {
                                let node = &mut levels[offset(side) + z * side + x];
                                let code = ground_pattern(tile, x, z);
                                let top = (*node as i8) + (code & 7) as i8;
                                *node |= u32::from(top as u8) << 16 | u32::from(code) << 24;
                            }
                        }
                        let mut n = side;
                        while n > 1 {
                            let k = n / 2;
                            for z in 0..k {
                                for x in 0..k {
                                    let index = offset(n) + 2 * (x + z * n);
                                    let children = [index, index + 1, index + n, index + n + 1]
                                        .map(|j| levels[j]);
                                    let max = children
                                        .map(|v| (v >> 16) as i8)
                                        .into_iter()
                                        .max()
                                        .unwrap();
                                    let lower_shift = if n == side { 16 } else { 24 };
                                    let min = children
                                        .map(|v| (v >> lower_shift) as i8)
                                        .into_iter()
                                        .min()
                                        .unwrap();
                                    levels[offset(k) + z * k + x] |=
                                        u32::from(max as u8) << 16 | u32::from(min as u8) << 24;
                                }
                            }
                            n = k;
                        }
                    }
                });
            }
        });
        result
    })
}

/// Seed-grown reference archetypes shared by all worlds. A site's seed
/// chooses its archetype and quarter turn at every viewing distance.
pub fn templates() -> &'static [Arc<PlantModel>; TEMPLATE_COUNT] {
    static MODELS: OnceLock<[Arc<PlantModel>; TEMPLATE_COUNT]> = OnceLock::new();
    MODELS.get_or_init(|| {
        // Each slot combines the original canonical branch strokes and
        // foliage clusters at native resolution.
        let variants = [
            living_flora::CONIFER,
            living_flora::CONIFER + 1,
            living_flora::CONIFER + 2,
            living_flora::CONIFER + 3,
            living_flora::CONIFER + 5,
            living_flora::CONIFER + 7,
            living_flora::ASPEN,
            living_flora::ASPEN + 1,
            living_flora::ASPEN + 3,
            living_flora::ALDER,
            living_flora::ALDER + 2,
            living_flora::SAPLING + 2,
            living_flora::SHRUB,
            living_flora::SHRUB + 1,
            living_flora::SHRUB + 3,
            living_flora::SHRUB + 5,
        ];
        std::array::from_fn(|i| {
            model_for(&PlantRecord {
                species: Species::Rowan,
                root: Cell { x: 0, y: 0, z: 0 },
                seed: 0xad31_3282_5c16_65d7u64.wrapping_mul(i as u64 + 1),
                variant: variants[i],
                rotation: 0,
            })
        })
    })
}

pub fn voxel(terrain: &FbmTerrain, cell: Cell, blocking_only: bool) -> Option<PlantVoxel> {
    let seed = relief_seed(terrain.seed());
    let column = terrain.column_cells(cell.x, cell.z);
    // Plant roots in adjacent sites can overlap. Wood takes precedence
    // independently of query order, and grass occupies the underlying band.
    let mut leaf = None;
    for x in (cell.x - TREE_RADIUS).div_euclid(SITE_CELLS)
        ..=(cell.x + TREE_RADIUS).div_euclid(SITE_CELLS)
    {
        for z in (cell.z - TREE_RADIUS).div_euclid(SITE_CELLS)
            ..=(cell.z + TREE_RADIUS).div_euclid(SITE_CELLS)
        {
            let Some(([root_x, root_z], kind, rotation)) = woody_site([x, z], seed) else {
                continue;
            };
            if (cell.x - root_x).abs() > TREE_RADIUS || (cell.z - root_z).abs() > TREE_RADIUS {
                continue;
            }
            let surface = if cell.x == root_x && cell.z == root_z {
                column
            } else {
                terrain.column_cells(root_x, root_z)
            };
            if surface.visible_top != surface.ground_top
                || surface.ground_surface != Material::Grass
            {
                continue;
            }
            let record = PlantRecord {
                species: Species::Rowan,
                seed: 0,
                variant: 0,
                rotation,
                root: Cell {
                    x: root_x,
                    y: surface.ground_top,
                    z: root_z,
                },
            };
            if let Some(mut voxel) = templates()[kind].voxel(record.local_cell(cell)) {
                voxel.cell = cell;
                if voxel.material == Material::Wood {
                    return Some(voxel);
                }
                leaf = leaf.or(Some(voxel));
            }
        }
    }
    if blocking_only {
        return None;
    }
    leaf.or_else(|| {
        grass_material(cell, seed, column).map(|material| PlantVoxel {
            cell,
            material,
            color: material.color(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_models_and_grass_fit_their_conservative_bounds() {
        for model in templates() {
            for v in &model.voxels {
                assert!(v.cell.x.abs() <= TREE_RADIUS && v.cell.z.abs() <= TREE_RADIUS);
                assert!(v.cell.y >= 0);
            }
        }
        for x in -40..40 {
            for z in -40..40 {
                let height =
                    i64::from(ground_pattern(17, (x + 40) as usize, (z + 40) as usize) & 7);
                assert!((0..=GRASS_HEIGHT).contains(&height));
                if let Some(([rx, rz], kind, rotation)) = woody_site([x, z], 17) {
                    assert_eq!(
                        [rx.div_euclid(SITE_CELLS), rz.div_euclid(SITE_CELLS)],
                        [x, z]
                    );
                    assert!(kind < TEMPLATE_COUNT && rotation < 4);
                }
            }
        }
    }
}
