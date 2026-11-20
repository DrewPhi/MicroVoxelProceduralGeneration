//! Decorative microvoxel ground plants for coarse terrain grids. Placement
//! uses native terrain columns. Geometry is shared by all instances of a variant.
use super::{
    mesh_cells, mix, CullView, MeshKey, VegetationBatch, VegetationInstance, VegetationMesh,
    VegetationStats,
};
use frontier_world::{
    vegetation::{PlantRecord, Species},
    voxel_size, Cell, Material, World, VOXEL_SIZE,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

pub(super) const TILE_SPECIES: u8 = 3;
const DETAIL_LOD: u8 = 32;
pub(crate) const INSTANCE_FLAG: u32 = 8;
pub(crate) const QUAD_FLAG: u32 = 8;
pub(crate) const DETAIL_VOXEL_M: f32 = VOXEL_SIZE as f32;
const GRASS_VARIANTS: usize = 4;
const FLOWER_VARIANTS: usize = 2;

pub(crate) fn is_detail(key: MeshKey) -> bool {
    key.lod == DETAIL_LOD
}

pub(super) fn tile_cells() -> i64 {
    (8. / voxel_size()).ceil() as i64
}

pub(super) fn records(
    world: &World,
    x: i64,
    z: i64,
    cancel: impl Fn() -> bool,
) -> Vec<PlantRecord> {
    let width = tile_cells();
    let pitch = voxel_size();
    let density = (0.65 * pitch * pitch).min(1.);
    let mut records = Vec::new();
    for z in z * width..(z + 1) * width {
        if cancel() {
            return Vec::new();
        }
        for x in x * width..(x + 1) * width {
            let seed = mix(world.seed()
                ^ 0x6d65_6164_6f77_5eed
                ^ mix(x as u64)
                ^ mix(z as u64).rotate_left(27));
            let roll = |bits: u64| (bits >> 11) as f64 * (1. / (1u64 << 53) as f64);
            if roll(seed) >= density {
                continue;
            }
            let column = world.column(x, z);
            let y = column.ground_height_cells();
            let root = Cell { x, y, z };
            if column.material(y - 1) != Material::Grass
                || column.material(y) != Material::Air
                || world.volume_owned(root)
            {
                continue;
            }
            // Cluster rare flowers into small patches, with sparse outliers.
            let colony = mix(world.seed()
                ^ mix((x as f64 * pitch / 6.).floor() as i64 as u64)
                ^ mix((z as f64 * pitch / 6.).floor() as i64 as u64).rotate_left(29));
            let bloom = if roll(colony) > 0.77 { 0.24 } else { 0.025 };
            let (species, count) = if roll(mix(seed)) < bloom {
                (Species::Edelweiss, FLOWER_VARIANTS)
            } else {
                (Species::Grass, GRASS_VARIANTS)
            };
            records.push(PlantRecord {
                species,
                seed,
                root,
                variant: ((seed >> 12) as usize % count) as u8,
                rotation: ((seed >> 30) & 3) as u8,
            });
        }
    }
    records
}

pub(crate) fn root_offset(root: [i64; 3]) -> [f32; 2] {
    let seed = mix(mix(root[0] as u64) ^ mix(root[2] as u64).rotate_left(29));
    [seed as u16, (seed >> 16) as u16].map(|bits| {
        (0.3 + 0.4 * f32::from(bits) / 65535.) * voxel_size() as f32 - DETAIL_VOXEL_M * 0.5
    })
}

fn template(species: Species, variant: u8) -> VegetationMesh {
    let mut voxels = BTreeMap::new();
    let green = [
        [0.108, 0.30, 0.057],
        [0.145, 0.36, 0.074],
        [0.18, 0.39, 0.067],
        [0.105, 0.32, 0.085],
    ][usize::from(variant)];
    let mut put = |x, y, z, material: Material, color| {
        voxels.insert(Cell { x, y, z }, (material as u8, color));
    };
    if species == Species::Grass {
        for (x, z, h) in [
            (0, 0, 3 + i64::from(variant % 3)),
            (-1, 1, 2),
            (1, -1, 2 + i64::from(variant % 2)),
        ] {
            for y in 0..h {
                let color = green.map(|c| c * if y == 0 { 0.81 } else { 1. });
                put(x, y, z, Material::Leaf, color);
            }
        }
    } else {
        let height = 4 + i64::from(variant);
        for y in 0..height {
            put(0, y, 0, Material::Leaf, green);
        }
        put(1, 1, 0, Material::Leaf, green);
        put(-1, 2, 0, Material::Leaf, green);
        let petal = [[0.91, 0.86, 0.65], [0.85, 0.47, 0.24]][usize::from(variant)];
        for (x, z) in [(-1, 0), (0, -1), (1, 0), (0, 1)] {
            put(x, height, z, Material::Flower, petal);
        }
        put(0, height, 0, Material::Flower, [0.90, 0.66, 0.14]);
    }
    let mut mesh = mesh_cells(
        voxels
            .into_iter()
            .map(|(cell, (m, color))| (cell, m, color)),
        1,
        || false,
    );
    let pitch = voxel_size() as f32;
    for vertex in &mut mesh.vertices {
        vertex.position = vertex
            .position
            .map(|m| (m / pitch).round() * DETAIL_VOXEL_M);
    }
    mesh
}

pub(super) fn admit(
    records: &[PlantRecord],
    camera: [f64; 3],
    view: Option<&CullView>,
    radii: [f64; 2],
    limits: [usize; 2],
    batches: &mut BTreeMap<MeshKey, VegetationBatch>,
    stats: &mut VegetationStats,
) {
    if records.is_empty() {
        return;
    }
    static MESHES: OnceLock<[Arc<VegetationMesh>; GRASS_VARIANTS + FLOWER_VARIANTS]> =
        OnceLock::new();
    let meshes = MESHES.get_or_init(|| {
        std::array::from_fn(|index| {
            let (species, variant) = if index < GRASS_VARIANTS {
                (Species::Grass, index)
            } else {
                (Species::Edelweiss, index - GRASS_VARIANTS)
            };
            Arc::new(template(species, variant as u8))
        })
    });
    let pitch = voxel_size();
    let eye = glam::DVec3::from_array(camera);
    let mut visible = Vec::new();
    for plant in records {
        let root = [plant.root.x, plant.root.y, plant.root.z];
        let offset = root_offset(root);
        let center = glam::DVec3::new(
            root[0] as f64 * pitch + f64::from(offset[0]),
            root[1] as f64 * pitch + 0.18,
            root[2] as f64 * pitch + f64::from(offset[1]),
        );
        let distance = center.distance(eye);
        let species = usize::from(plant.species == Species::Edelweiss);
        if distance < radii[species] && view.is_none_or(|v| v.intersects_sphere(center, 0.36)) {
            visible.push((distance, root, species, plant));
        }
    }
    visible.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    for (distance, root_cell, species, plant) in visible {
        let index = usize::from(plant.variant) + species * GRASS_VARIANTS;
        let mesh = &meshes[index];
        let cost = mesh.indices.len() / 3;
        if stats.selected_instances >= limits[0] || cost > limits[1].saturating_sub(stats.triangles)
        {
            break;
        }
        let key = MeshKey {
            species: species as u8,
            variant: plant.variant,
            seed: 0,
            tile: [0; 2],
            lod: DETAIL_LOD,
            edit_id: 0,
        };
        let batch = batches.entry(key).or_insert_with(|| VegetationBatch {
            key,
            mesh: mesh.clone(),
            instances: Vec::new(),
        });
        batch.instances.push(VegetationInstance {
            root_cell,
            quarter_turn: u32::from(plant.rotation),
            fade: ((radii[species] - distance) / 5.).clamp(0., 1.) as f32,
        });
        stats.selected_instances += 1;
        stats.triangles += cost;
        stats.selected_by_species[species] += 1;
        stats.triangles_by_species[species] += cost;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoration_templates_stay_below_half_a_meter_and_fit_compact_packing() {
        for (species, count) in [
            (Species::Grass, GRASS_VARIANTS),
            (Species::Edelweiss, FLOWER_VARIANTS),
        ] {
            for variant in 0..count {
                let m = template(species, variant as u8);
                let height = m.vertices.iter().map(|v| v.position[1]).fold(0., f32::max);
                assert!((0.12..=0.4).contains(&height));
                assert!(m.indices.len() < 256);
            }
        }
    }
}
