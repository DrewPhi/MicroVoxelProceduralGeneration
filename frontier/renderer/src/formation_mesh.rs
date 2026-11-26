//! A bounded exact surface owner for the procedural formation study.
use crate::{mesh, Result};
use frontier_world::{voxel_size, Cell, Material, World};

#[derive(Default)]
pub(crate) struct FormationMesh {
    pub(crate) revision: Option<u64>,
    edits: Vec<(Cell, Material)>,
}

impl FormationMesh {
    pub(crate) fn update(&mut self, world: &World, gray: bool) -> Result<Option<mesh::Mesh>> {
        if self.revision == Some(world.revision()) || world.rock_formations().is_empty() {
            return Ok(None);
        }
        let mut edits = Vec::new();
        for rock in world.rock_formations() {
            let (min, max) = rock.bounds();
            edits.extend(
                world
                    .edit_cells_in_bounds(min, max)
                    .into_iter()
                    .map(|c| (c, world.material_without_fluid(c))),
            );
        }
        if self.revision.is_some() && self.edits == edits {
            self.revision = Some(world.revision());
            return Ok(None);
        }
        let anchor = world.rock_formations()[0].bounds().0;
        let mut out = mesh::Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
            origin: anchor,
            exposed_native_faces: 0,
        };
        for rock in world.rock_formations() {
            let (min, max) = rock.bounds();
            let size = [max.x - min.x, max.y - min.y, max.z - min.z].map(|v| v as usize);
            if size
                .into_iter()
                .try_fold(1usize, usize::checked_mul)
                .is_none_or(|n| n > 24_000_000)
            {
                return Err(
                    "formation volume exceeds the native extraction budget; increase voxel size"
                        .into(),
                );
            }
            let mut indexed = std::collections::HashMap::new();
            indexed.extend(edits.iter().copied().filter(|(c, _)| {
                c.x >= min.x
                    && c.x < max.x
                    && c.y >= min.y
                    && c.y < max.y
                    && c.z >= min.z
                    && c.z < max.z
            }));
            let mut shape = mesh::extract(
                |c| {
                    if rock.contains(c) {
                        indexed.get(&c).copied().unwrap_or(Material::Stone)
                    } else {
                        Material::Air
                    }
                },
                min,
                size,
            );
            let shift = [min.x - anchor.x, min.y - anchor.y, min.z - anchor.z]
                .map(|d| (d as f64 * voxel_size()) as f32);
            let origin = [min.x, min.y, min.z].map(|v| v as f64 * voxel_size());
            for vertex in &mut shape.vertices {
                let point: [f64; 3] =
                    std::array::from_fn(|i| origin[i] + f64::from(vertex.position[i]));
                // Bake broad cavity occlusion using actual occupied voxels.
                // The same occlusion stays visible on the far mesh.
                let above = [0.30, 0.65, 1.10, 1.85, 2.65, 3.25]
                    .into_iter()
                    .any(|d| rock.contains(Cell::from_meters([point[0], point[1] + d, point[2]])));
                let air_face = vertex.normal[1] < 0. || (above && vertex.normal[1].abs() < 0.1);
                let dark = if air_face { 0.66 } else { 1. };
                let rock_shade = if gray {
                    [0.38; 3]
                } else {
                    let bed = ((point[1] + point[0] * 0.19 - point[2] * 0.08) / 0.66).floor();
                    let tone = (bed * 0.618_034).rem_euclid(1.);
                    [
                        (0.31 + 0.16 * tone) as f32,
                        (0.29 + 0.13 * tone) as f32,
                        (0.27 + 0.10 * tone) as f32,
                    ]
                };
                vertex.color = rock_shade.map(|v| v * dark);
                for (value, offset) in vertex.position.iter_mut().zip(shift) {
                    *value += offset;
                }
            }
            let base = out.vertices.len() as u32;
            out.vertices.extend(shape.vertices);
            out.indices
                .extend(shape.indices.into_iter().map(|i| base + i));
            out.exposed_native_faces += shape.exposed_native_faces;
        }
        self.revision = Some(world.revision());
        self.edits = edits;
        Ok(Some(out))
    }
}
