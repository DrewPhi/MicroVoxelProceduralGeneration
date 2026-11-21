//! Kilometre-scale relief beyond the active terrain clip. The lattice is
//! anchored in world metres so moving the window preserves shared vertices.
use crate::{atmosphere::texture, Camera};
use frontier_world::{voxel_size, Cell, FbmParams, FbmTerrain, Material};
use glam::{DVec3, Mat4, Vec3, Vec4};
use std::ops::Range;
#[path = "horizon_blocks.rs"]
mod blocks;
use std::sync::{
    mpsc::{self, Receiver},
    Arc,
};
use wgpu::util::DeviceExt;

// Ground/bed RGB, visible elevation, wet coverage, weighted optical depth,
// then subsurface and bedrock RGB from the native material column.
type Sample = [f32; 12];

const SIDE: usize = 161;
const SPACING: f64 = 512.;
const LEVELS: usize = 3;
const HALF: i64 = SIDE as i64 / 2;
struct Lattice {
    center: [i64; 2],
    samples: Vec<Sample>,
}
struct Patch {
    lattice: Arc<Lattice>,
    mesh: blocks::Mesh,
}

fn lattice(
    center: [i64; 2],
    previous: Option<&Lattice>,
    mut sample: impl FnMut(f64, f64) -> Sample,
) -> Lattice {
    let mut out = vec![[0.; 12]; SIDE * SIDE * LEVELS];
    for level in 0..LEVELS {
        let step = SPACING / (1 << (LEVELS - level - 1)) as f64;
        let base = level * SIDE * SIDE;
        let old = previous.and_then(|l| {
            let scale = 1_i128 << (LEVELS - level - 1);
            let dx = (i128::from(center[0]) - i128::from(l.center[0])) * scale;
            let dz = (i128::from(center[1]) - i128::from(l.center[1])) * scale;
            (dx.abs() < SIDE as i128 && dz.abs() < SIDE as i128)
                .then_some((l, dx as i64, dz as i64))
        });
        for z in -HALF..=HALF {
            for x in -HALF..=HALF {
                // Keep a one-vertex halo for the vertex normals beside the hole.
                if level > 0 && x.abs().max(z.abs()) < HALF / 2 - 1 {
                    continue;
                }
                let i = base + (z + HALF) as usize * SIDE + (x + HALF) as usize;
                if let Some((l, dx, dz)) = old {
                    let (ox, oz) = (x + dx, z + dz);
                    let radius = ox.abs().max(oz.abs());
                    // Odd vertices on the old outer edge were averaged for
                    // the ring join. Resample them when they become interior.
                    let averaged = level + 1 < LEVELS && radius == HALF && (ox | oz) & 1 != 0;
                    if radius <= HALF && (level == 0 || radius >= HALF / 2 - 1) && !averaged {
                        out[i] =
                            l.samples[base + (oz + HALF) as usize * SIDE + (ox + HALF) as usize];
                        continue;
                    }
                }
                out[i] = sample(
                    center[0] as f64 * SPACING + x as f64 * step,
                    center[1] as f64 * SPACING + z as f64 * step,
                );
            }
        }
        // Anchor odd fine edge vertices to the enclosing coarse edge.
        if level + 1 < LEVELS {
            for i in (1..SIDE - 1).step_by(2) {
                for (index, stride) in [
                    (i, 1),
                    ((SIDE - 1) * SIDE + i, 1),
                    (i * SIDE, SIDE),
                    (i * SIDE + SIDE - 1, SIDE),
                ] {
                    out[base + index] = std::array::from_fn(|k| {
                        (out[base + index - stride][k] + out[base + index + stride][k]) * 0.5
                    });
                }
            }
        }
    }
    Lattice {
        center,
        samples: out,
    }
}

fn samples(
    terrain: &FbmTerrain,
    vegetation: bool,
    center: [i64; 2],
    old: Option<&Lattice>,
) -> Patch {
    let lattice = Arc::new(lattice(center, old, |x, z| {
        let cell = Cell::from_meters([x, 0., z]);
        let (col, surface) = terrain.landscape_column(cell.x, cell.z, vegetation);
        let wet = col.visible_top > col.ground_top;
        let [r, g, b] = if wet {
            col.ground_surface.color()
        } else {
            surface
        };
        let depth =
            ((col.visible_top - col.ground_top) as f64 * voxel_size()).clamp(0., 12.) as f32;
        let soil = col.ground_subsurface.color();
        let rock = if col.ground_surface == Material::Stone && !wet {
            surface
        } else {
            Material::Stone.color()
        };
        [
            r,
            g,
            b,
            (col.visible_top as f64 * voxel_size()) as f32,
            u8::from(wet) as f32,
            depth,
            soil[0],
            soil[1],
            soil[2],
            rock[0],
            rock[1],
            rock[2],
        ]
    }));
    let mesh = blocks::build(&lattice.samples, voxel_size() as f32);
    Patch { lattice, mesh }
}

pub(crate) struct Horizon {
    pub(crate) color: wgpu::TextureView,
    depth: wgpu::TextureView,
    params: wgpu::Buffer,
    faces: Option<wgpu::Buffer>,
    batches: Vec<blocks::Batch>,
    visible: Vec<Range<u32>>,
    view: Option<[f32; 28]>,
    target_height: u32,
    dirty: std::cell::Cell<bool>,
    pipeline: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
    lattice: Option<Arc<Lattice>>,
    pending: Option<Receiver<Patch>>,
    source_key: Option<(u64, FbmParams, f64, bool)>,
}

impl Horizon {
    pub(crate) fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let format = wgpu::TextureFormat::Rgba16Float;
        let color = texture(device, "flight horizon", format, width, height, 1);
        let depth = texture(
            device,
            "horizon depth",
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
            1,
        );
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let params = buffer(
            "horizon camera",
            112,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flight horizon"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("water.wgsl"),
                    include_str!("flight_horizon.wgsl")
                )
                .replace("GRID_UNIT", &format!("{:.1}", blocks::UNIT))
                .into(),
            ),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("flight horizon"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Uint32x2, 1 => Float32x2],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Greater,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flight horizon"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            }],
        });
        Self {
            color,
            depth,
            params,
            faces: None,
            batches: Vec::new(),
            visible: Vec::new(),
            view: None,
            target_height: height,
            dirty: std::cell::Cell::new(true),
            pipeline,
            group,
            lattice: None,
            pending: None,
            source_key: None,
        }
    }

    pub(crate) fn geometry_bytes(&self) -> u64 {
        self.faces.as_ref().map_or(0, wgpu::Buffer::size)
    }

    fn publish(&mut self, device: &wgpu::Device, Patch { lattice, mesh }: Patch) {
        self.faces = Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("stepped horizon faces"),
                contents: bytemuck::cast_slice(&mesh.faces),
                usage: wgpu::BufferUsages::VERTEX,
            }),
        );
        self.batches = mesh.batches;
        self.lattice = Some(lattice);
        self.view = None;
        self.dirty.set(true);
    }

    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &FbmTerrain,
        vegetation: bool,
        camera: Camera,
        aspect: f32,
        sun: Vec3,
        overcast: f32,
        capture: bool,
    ) {
        let key = (source.seed(), source.params(), voxel_size(), vegetation);
        if self.source_key != Some(key) {
            self.lattice = None;
            self.view = None;
            self.visible.clear();
            self.dirty.set(true);
            self.pending = None;
            self.source_key = Some(key);
        }
        let center = [camera.position.x, camera.position.z].map(|v| (v / SPACING).round() as i64);
        let mut ready = None;
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(patch) => {
                    ready = Some(patch);
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.pending = None,
                Err(mpsc::TryRecvError::Empty) => (),
            }
        }
        if let Some(patch) = ready {
            self.publish(device, patch);
        }
        if (capture || self.pending.is_none())
            && self
                .lattice
                .as_ref()
                .is_none_or(|old| (0..2).any(|a| old.center[a].abs_diff(center[a]) >= 4))
        {
            if capture {
                self.pending = None;
                self.publish(
                    device,
                    samples(source, vegetation, center, self.lattice.as_deref()),
                );
            } else {
                let (tx, rx) = mpsc::sync_channel(1);
                self.pending = Some(rx);
                let source = source.clone();
                let old = self.lattice.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(samples(&source, vegetation, center, old.as_deref()));
                });
            }
        }
        if let Some(lattice) = &self.lattice {
            let center = lattice.center;
            let origin = DVec3::new(center[0] as f64 * SPACING, 0., center[1] as f64 * SPACING);
            let eye = (camera.position - origin).as_vec3();
            let matrix = Mat4::perspective_rh(camera.fov_y.to_radians(), aspect, 75000., 1.)
                * Mat4::look_to_rh(eye, camera.direction(), camera.view_up());
            let mut data = [0f32; 28];
            data[..16].copy_from_slice(&matrix.to_cols_array());
            data[16..20].copy_from_slice(&eye.extend(camera.far * 0.52).to_array());
            data[20..24].copy_from_slice(&sun.extend(overcast).to_array());
            data[24] = 0.5 * self.target_height as f32 / (0.5 * camera.fov_y.to_radians()).tan();
            if self.view == Some(data) {
                return;
            }
            self.view = Some(data);
            self.dirty.set(true);
            let rows = matrix.transpose();
            let near = rows.w_axis - Vec4::W * data[19];
            let planes = [
                rows.w_axis + rows.x_axis,
                rows.w_axis - rows.x_axis,
                rows.w_axis + rows.y_axis,
                rows.w_axis - rows.y_axis,
                near,
                rows.z_axis,
            ];
            self.visible.clear();
            for batch in &self.batches {
                let lo = Vec3::from(batch.min);
                let hi = Vec3::from(batch.max);
                if planes.iter().any(|p| {
                    let n = p.truncate();
                    Vec3::select(n.cmpge(Vec3::ZERO), hi, lo).dot(n) + p.w < 0.
                }) {
                    continue;
                }
                // A camera outside a chunk cannot see a wall facing away
                // from the whole chunk. Submit adjacent visible bins together.
                let facing = [
                    eye.y >= lo.y,
                    eye.x >= lo.x,
                    eye.x <= hi.x,
                    eye.z >= lo.z,
                    eye.z <= hi.z,
                ];
                for (range, visible) in batch.faces.iter().zip(facing) {
                    if !visible || range.is_empty() {
                        continue;
                    }
                    if let Some(previous) = self.visible.last_mut().filter(|r| r.end == range.start)
                    {
                        previous.end = range.end;
                    } else {
                        self.visible.push(range.clone());
                    }
                }
            }
            queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&data));
        }
    }
    pub(crate) fn draw(&self, encoder: &mut wgpu::CommandEncoder) {
        if !self.dirty.replace(false) {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("flight horizon"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.color,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            occlusion_query_set: None,
            timestamp_writes: None,
        });
        if let Some(faces) = &self.faces {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.group, &[]);
            pass.set_vertex_buffer(0, faces.slice(..));
            for range in &self.visible {
                pass.draw(0..4, range.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recentered_horizon_matches_a_cold_lattice_in_both_directions() {
        let sample = |x: f64, z: f64| {
            [
                x as f32,
                z as f32,
                (x + z) as f32,
                (x * 0.003 + z * 0.007).sin() as f32,
                0.,
                0.,
                x as f32,
                z as f32,
                0.,
                z as f32,
                0.,
                x as f32,
            ]
        };
        let old = lattice([-3, 7], None, sample);
        for center in [[1, 7], [-8, 3], [5000, -5000]] {
            let mut misses = 0;
            let moved = lattice(center, Some(&old), |x, z| {
                misses += 1;
                sample(x, z)
            });
            let mut cold_count = 0;
            let cold = lattice(center, None, |x, z| {
                cold_count += 1;
                sample(x, z)
            });
            assert_eq!(moved.samples, cold.samples);
            if center[0].abs() < 10 {
                assert!(misses < cold_count / 4);
            } else {
                assert_eq!(misses, cold_count);
            }
        }
    }
}
