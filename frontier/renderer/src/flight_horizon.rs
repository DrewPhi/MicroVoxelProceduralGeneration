//! Kilometre-scale relief beyond the active terrain clip. The lattice is
//! anchored in world metres so moving the window preserves shared vertices.
use crate::{atmosphere::texture, Camera};
use frontier_world::{voxel_size, Cell, FbmTerrain};
use glam::{DVec3, Mat4, Vec3};
use std::sync::mpsc::{self, Receiver};
use wgpu::util::DeviceExt;

const SIDE: usize = 161;
const SPACING: f64 = 512.;
const LEVELS: usize = 3;
const HALF: i64 = SIDE as i64 / 2;
type Patch = ([i64; 2], Vec<[f32; 4]>);

fn samples(terrain: &FbmTerrain, center: [i64; 2]) -> Patch {
    let mut out = vec![[0.; 4]; SIDE * SIDE * LEVELS];
    for level in 0..LEVELS {
        let step = SPACING / (1 << (LEVELS - level - 1)) as f64;
        let base = level * SIDE * SIDE;
        for z in -HALF..=HALF {
            for x in -HALF..=HALF {
                // Keep a one-vertex halo for the vertex normals beside the hole.
                if level > 0 && x.abs().max(z.abs()) < HALF / 2 - 1 {
                    continue;
                }
                let cell = Cell::from_meters([
                    center[0] as f64 * SPACING + x as f64 * step,
                    0.,
                    center[1] as f64 * SPACING + z as f64 * step,
                ]);
                let col = terrain.column_cells(cell.x, cell.z);
                let [r, g, b] = col.top_material().color();
                out[base + (z + HALF) as usize * SIDE + (x + HALF) as usize] =
                    [r, g, b, (col.visible_top as f64 * voxel_size()) as f32];
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
    (center, out)
}

pub(crate) struct Horizon {
    pub(crate) color: wgpu::TextureView,
    depth: wgpu::TextureView,
    params: wgpu::Buffer,
    heights: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    pipeline: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
    center: Option<[i64; 2]>,
    pending: Option<Receiver<Patch>>,
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
            96,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let heights = buffer(
            "horizon lattice",
            (LEVELS * SIDE * SIDE * 16) as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let mut tris = Vec::with_capacity((SIDE - 1) * (SIDE - 1) * 6);
        for level in 0..LEVELS {
            let hole = SIDE / 4..SIDE - 1 - SIDE / 4;
            for z in 0..SIDE - 1 {
                for x in 0..SIDE - 1 {
                    if level > 0 && hole.contains(&z) && hole.contains(&x) {
                        continue;
                    }
                    let i = (level * SIDE * SIDE + z * SIDE + x) as u32;
                    let j = i + SIDE as u32;
                    tris.extend([i, j, i + 1, j, j + 1, i + 1]);
                }
            }
        }
        let index_count = tris.len() as u32;
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("horizon indices"),
            contents: bytemuck::cast_slice(&tris),
            usage: wgpu::BufferUsages::INDEX,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flight horizon"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("flight_horizon.wgsl")
                    .replace("GRID_SIDE", &SIDE.to_string())
                    .replace("GRID_STEP", &format!("{SPACING:.1}"))
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
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
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
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: heights.as_entire_binding(),
                },
            ],
        });
        Self {
            color,
            depth,
            params,
            heights,
            indices,
            index_count,
            pipeline,
            group,
            center: None,
            pending: None,
        }
    }

    pub(crate) fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        source: &FbmTerrain,
        camera: Camera,
        aspect: f32,
        sun: Vec3,
        capture: bool,
    ) {
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
        if let Some((center, data)) = ready {
            self.center = Some(center);
            queue.write_buffer(&self.heights, 0, bytemuck::cast_slice(&data));
        }
        if self.pending.is_none()
            && self
                .center
                .is_none_or(|old| (0..2).any(|a| old[a].abs_diff(center[a]) >= 4))
        {
            if capture {
                let (_, data) = samples(source, center);
                self.center = Some(center);
                queue.write_buffer(&self.heights, 0, bytemuck::cast_slice(&data));
            } else {
                let (tx, rx) = mpsc::sync_channel(1);
                self.pending = Some(rx);
                let source = source.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(samples(&source, center));
                });
            }
        }
        if let Some(center) = self.center {
            let origin = DVec3::new(center[0] as f64 * SPACING, 0., center[1] as f64 * SPACING);
            let eye = (camera.position - origin).as_vec3();
            let matrix = Mat4::perspective_rh(camera.fov_y.to_radians(), aspect, 1., 75000.)
                * Mat4::look_to_rh(eye, camera.direction(), camera.view_up());
            let mut data = [0f32; 24];
            data[..16].copy_from_slice(&matrix.to_cols_array());
            data[16..20].copy_from_slice(&eye.extend(camera.far * 0.52).to_array());
            data[20..].copy_from_slice(&sun.extend(0.).to_array());
            queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&data));
        }
    }
    pub(crate) fn draw(&self, encoder: &mut wgpu::CommandEncoder) {
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
                    load: wgpu::LoadOp::Clear(1.),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            occlusion_query_set: None,
            timestamp_writes: None,
        });
        if self.center.is_some() {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.group, &[]);
            pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.index_count, 0, 0..1);
        }
    }
}
