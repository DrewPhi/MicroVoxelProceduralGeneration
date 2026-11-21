//! Coarse sunlight occluders derived from resident voxel tree surfaces. The
//! terrain light map resolves one metre per texel, so it can share a single
//! draw for whole stands without resubmitting every branch and leaf.
use super::{AdaptiveGpu, Camera, LightFrame};
use crate::{
    adaptive_geometry::NativeQuad,
    vegetation::{VegetationFrame, VegetationMesh},
};
use frontier_world::{vegetation::PlantRecord, voxel_size, Cell, Material};
use glam::DVec3;
use std::collections::{BTreeMap, BTreeSet};

const MAX_TREES: usize = 256;
type Identity = (u64, u8, u64); // seed, variant, local edits
type Caster = (Cell, u8, Identity);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bounds {
    lo: [i16; 3],
    hi: [i16; 3],
}

fn column_bounds(mesh: &VegetationMesh) -> Vec<Bounds> {
    // Woody and leafy intervals never merge. This preserves the open space
    // below a crown and lets removing the trunk or the canopy affect its own
    // part of the shadow. Independent of the sun and all camera transforms.
    let cell = voxel_size() as f32;
    let mut columns = [None::<Bounds>; 6 * 6 * 2];
    for quad in mesh.vertices.chunks_exact(4) {
        if quad[0].normal[1] == 0. {
            continue;
        }
        let mut lo = [i16::MAX; 3];
        let mut hi = [i16::MIN; 3];
        for v in quad {
            for i in 0..3 {
                let p = (v.position[i] / cell).round() as i16;
                lo[i] = lo[i].min(p);
                hi[i] = hi[i].max(p);
            }
        }
        let bucket = |p: i16| (p.div_euclid(16) + 3).clamp(0, 5) as usize;
        let wood = usize::from(quad[0].transmission == 0.) * 36;
        // Split long greedy faces across columns. Interior column boundaries
        // lie on exactly the same integer lattice as the published tree.
        for x in bucket(lo[0])..=bucket(hi[0] - 1) {
            for z in bucket(lo[2])..=bucket(hi[2] - 1) {
                let mut b = Bounds { lo, hi };
                for (axis, k) in [(0, x), (2, z)] {
                    if k > 0 {
                        b.lo[axis] = b.lo[axis].max((k as i16 - 3) * 16);
                    }
                    if k < 5 {
                        b.hi[axis] = b.hi[axis].min((k as i16 - 2) * 16);
                    }
                }
                let old = columns[wood + x * 6 + z].get_or_insert(b);
                for i in 0..3 {
                    old.lo[i] = old.lo[i].min(b.lo[i]);
                    old.hi[i] = old.hi[i].max(b.hi[i]);
                }
            }
        }
    }
    columns.into_iter().flatten().collect()
}

impl Bounds {
    fn faces(self, root: Cell, turn: u8, origin: Cell, output: &mut Vec<NativeQuad>) {
        let rotate = |p: [i16; 3]| {
            let [x, y, z] = p.map(i32::from);
            match turn & 3 {
                1 => [1 - z, y, x],
                2 => [1 - x, y, 1 - z],
                3 => [z, y, 1 - x],
                _ => [x, y, z],
            }
        };
        let a = rotate(self.lo);
        let b = rotate(self.hi);
        let edges: [i32; 3] = std::array::from_fn(|i| (b[i] - a[i]).abs());
        let offset = [
            root.x as i128 - origin.x as i128,
            root.y as i128 - origin.y as i128,
            root.z as i128 - origin.z as i128,
        ];
        let cell = voxel_size();
        for axis in 0..3 {
            let extent = [edges[(axis + 1) % 3], edges[(axis + 2) % 3]];
            if extent.contains(&0) {
                continue;
            }
            for negative in [false, true] {
                let anchor = std::array::from_fn(|i| {
                    let p = if i == axis && !negative {
                        a[i].max(b[i])
                    } else {
                        a[i].min(b[i])
                    };
                    ((offset[i] + i128::from(p)) as f64 * cell) as f32
                });
                output.push(NativeQuad {
                    anchor,
                    extent: extent.map(|s| (s as f64 * cell) as f32),
                    axis_sign: axis as u32 | (u32::from(negative) << 2),
                    material: Material::Wood as u32,
                    profile_index: 0,
                });
            }
        }
    }
}

struct Draw {
    faces: wgpu::Buffer,
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
    origin: DVec3,
    count: u32,
}

#[derive(Default)]
pub(crate) struct Canopy {
    shapes: BTreeMap<Identity, (u8, Vec<Bounds>)>,
    casters: Vec<Caster>,
    draw: Option<Draw>,
    pub(super) dirty: std::cell::Cell<bool>,
}

impl AdaptiveGpu {
    pub(crate) fn clear_canopy_shadows(&mut self) {
        let canopy = &mut self.shadows.canopy;
        if canopy.draw.take().is_some() {
            canopy.dirty.set(true);
        }
        canopy.shapes.clear();
        canopy.casters.clear();
    }
    pub(crate) fn update_canopy_shadows(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        roots: &[(PlantRecord, u64)],
        frame: &VegetationFrame,
    ) {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if roots.is_empty()
            || !self.near_shadows_active()
            || !*ENABLED
                .get_or_init(|| std::env::var("FRONTIER_CANOPY_SHADOWS").as_deref() != Ok("0"))
        {
            self.clear_canopy_shadows();
            return;
        }
        let canopy = &mut self.shadows.canopy;
        let mut nearby: Vec<_> = roots
            .iter()
            .map(|(p, id)| (p.root, p.rotation, (p.seed, p.variant, *id)))
            .collect();
        if nearby.len() > MAX_TREES {
            let distance = |p: &Caster| {
                let d = [p.0.x, p.0.y, p.0.z]
                    .into_iter()
                    .zip(frame.origin)
                    .map(|(a, b)| (a as i128 - b as i128) as f64);
                d.map(|v| v * v).sum::<f64>()
            };
            nearby.select_nth_unstable_by(MAX_TREES, |a, b| {
                distance(a).total_cmp(&distance(b)).then(a.cmp(b))
            });
            nearby.truncate(MAX_TREES);
        }
        nearby.sort_unstable();
        let wanted: BTreeSet<_> = nearby.iter().map(|v| v.2).collect();
        canopy.shapes.retain(|key, _| wanted.contains(key));
        let mut changed = false;
        for batch in frame.batches.iter().chain(&frame.shadow_batches) {
            let key = batch.key;
            let id = (key.seed, key.variant, key.edit_id);
            if key.species != 2 || !wanted.contains(&id) {
                continue;
            }
            // Prefer the half-metre baseline. Finer color refinement does not
            // republish a tree's terrain-map silhouette.
            let rank = match key.lod {
                8 => 0,
                16 => 2,
                _ => 1,
            };
            if canopy.shapes.get(&id).is_none_or(|(old, _)| rank < *old) {
                canopy.shapes.insert(id, (rank, column_bounds(&batch.mesh)));
                changed = true;
            }
        }
        nearby.retain(|p| canopy.shapes.get(&p.2).is_some_and(|(_, b)| !b.is_empty()));
        if !changed && nearby == canopy.casters {
            return;
        }
        canopy.dirty.set(true);
        canopy.casters = nearby;
        let Some(&(origin, _, _)) = canopy.casters.first() else {
            canopy.draw = None;
            return;
        };
        let mut faces = Vec::new();
        for &(root, turn, id) in &canopy.casters {
            for b in &canopy.shapes[&id].1 {
                b.faces(root, turn, origin, &mut faces);
            }
        }
        let bytes = bytemuck::cast_slice(&faces);
        if canopy
            .draw
            .as_ref()
            .is_none_or(|d| d.faces.size() < bytes.len() as u64)
        {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("woodland shadow faces"),
                size: (bytes.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("woodland light transform"),
                size: super::super::UNIFORM_STRIDE,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind = super::super::profile_bind(
                device,
                &self.layout,
                &uniform,
                &self.shadows.zero_table,
            );
            canopy.draw = Some(Draw {
                faces: buffer,
                uniform,
                bind,
                origin: DVec3::ZERO,
                count: 0,
            });
        }
        let draw = canopy.draw.as_mut().unwrap();
        queue.write_buffer(&draw.faces, 0, bytes);
        draw.count = faces.len() as u32;
        draw.origin =
            DVec3::from_array([origin.x, origin.y, origin.z].map(|p| p as f64 * voxel_size()));
    }
    pub(crate) fn canopy_stats(&self) -> (usize, u32) {
        (
            self.shadows.canopy.casters.len(),
            self.shadows.canopy.draw.as_ref().map_or(0, |d| d.count),
        )
    }
}

impl Canopy {
    pub(super) fn allocated_bytes(&self) -> u64 {
        self.draw
            .as_ref()
            .map_or(0, |d| d.faces.size() + d.uniform.size())
    }
    pub(super) fn prepare(&self, queue: &wgpu::Queue, camera: &Camera, lighting: [f32; 12]) {
        if let Some(d) = &self.draw {
            queue.write_buffer(
                &d.uniform,
                0,
                bytemuck::cast_slice(
                    &LightFrame::new(camera, lighting)
                        .matrix(d.origin)
                        .to_cols_array(),
                ),
            );
        }
    }
    pub(super) fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        pipeline: &'a wgpu::RenderPipeline,
        indices: &'a wgpu::Buffer,
    ) {
        if let Some(d) = &self.draw {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &d.bind, &[0]);
            pass.set_vertex_buffer(0, d.faces.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..6, 0, 0..d.count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vegetation::VegetationVertex;

    fn plate(mesh: &mut VegetationMesh, lo: [i16; 3], hi: [i16; 3], leaf: bool) {
        for (y, sign) in [(lo[1], -1.), (hi[1], 1.)] {
            for (x, z) in [
                (lo[0], lo[2]),
                (lo[0], hi[2]),
                (hi[0], hi[2]),
                (hi[0], lo[2]),
            ] {
                mesh.vertices.push(VegetationVertex {
                    position: [x, y, z].map(|v| f32::from(v) * voxel_size() as f32),
                    normal: [0., sign, 0.],
                    color: [0.; 3],
                    ao: 1.,
                    transmission: f32::from(u8::from(leaf)),
                });
            }
        }
    }

    #[test]
    fn occupied_intervals_preserve_air_below_crowns_after_edits() {
        let mut mesh = VegetationMesh::default();
        plate(&mut mesh, [-21, 32, -18], [20, 54, 19], true);
        let crown = column_bounds(&mesh);
        assert_eq!(crown.len(), 16);
        assert!(crown.iter().all(|b| b.lo[1] == 32 && b.hi[1] == 54));
        assert_eq!(
            crown
                .iter()
                .map(|b| i32::from(b.hi[0] - b.lo[0]) * i32::from(b.hi[2] - b.lo[2]))
                .sum::<i32>(),
            41 * 37
        );
        plate(&mut mesh, [1, 0, 1], [3, 33, 4], false);
        let complete = column_bounds(&mesh);
        assert!(complete.contains(&Bounds {
            lo: [1, 0, 1],
            hi: [3, 33, 4]
        }));
        assert_eq!(complete.len(), crown.len() + 1);
        assert!(column_bounds(&VegetationMesh::default()).is_empty());
    }

    #[test]
    fn rotated_shadow_faces_share_the_native_cell_and_rebase_before_float_conversion() {
        use frontier_world::vegetation::{PlantRecord, Species};
        let root = Cell {
            x: -(1i64 << 54) + 8,
            y: 7,
            z: (1i64 << 54) - 9,
        };
        let origin = Cell {
            x: root.x + 4,
            y: 12,
            z: root.z - 3,
        };
        let local = Cell { x: -5, y: 3, z: 9 };
        let b = Bounds {
            lo: [-5, 3, 9],
            hi: [-4, 4, 10],
        };
        for rotation in 0..4 {
            let plant = PlantRecord {
                species: Species::Rowan,
                seed: 7,
                root,
                variant: 0,
                rotation,
            };
            let w = plant.world_cell(local);
            let low = [w.x - origin.x, w.y - origin.y, w.z - origin.z];
            let mut faces = Vec::new();
            b.faces(root, rotation, origin, &mut faces);
            assert_eq!(faces.len(), 6);
            for face in faces {
                let axis = (face.axis_sign & 3) as usize;
                let positive = face.axis_sign & 4 == 0;
                for i in 0..3 {
                    let p = low[i] + i64::from(i == axis && positive);
                    assert_eq!(face.anchor[i], (p as f64 * voxel_size()) as f32);
                }
                assert_eq!(face.extent, [voxel_size() as f32; 2]);
            }
        }
    }
}
