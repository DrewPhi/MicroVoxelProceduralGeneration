//! GPU resources and drawing. World generation never touches this module.
use crate::camera::Camera;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec4};
use microvoxel::render::{
    lod::{MeshKey, EXTENT},
    terrain::TerrainMesh,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::Path,
    sync::mpsc,
};
use wgpu::util::DeviceExt;

pub const GPU_BUDGET: usize = 128 * 1024 * 1024;
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    vp: [f32; 16],
    selection: [f32; 4],
    viewport: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuQuad {
    origin: [i32; 3],
    face: u32,
    size: [u32; 2],
    color_offset: u32,
    padding: u32,
}
struct Part {
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
    count: u32,
    step: i64,
}
pub struct Tile {
    parts: Vec<Part>,
    pub bytes: usize,
    pub quads: usize,
    pub faces: usize,
    heights: [i64; 2],
    used: u64,
}
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
    pub adapter_name: String,
    pipeline: wgpu::RenderPipeline,
    hud: wgpu::RenderPipeline,
    frame_buffer: wgpu::Buffer,
    frame_bind: wgpu::BindGroup,
    tile_layout: wgpu::BindGroupLayout,
    depth: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub tiles: BTreeMap<MeshKey, Tile>,
    pub bytes: usize,
    frame: u64,
}
fn layout_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BufferBindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
impl Gpu {
    pub async fn new(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        width: u32,
        height: u32,
    ) -> microvoxel::Result<Self> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: surface,
            })
            .await
            .ok_or("No compatible GPU adapter. Check your graphics driver.")?;
        let info = adapter.get_info();
        let adapter_name = format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type);
        println!("GPU: {adapter_name}");
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("MicroVoxel GPU"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                },
                None,
            )
            .await?;
        let format = surface
            .map(|s| {
                let caps = s.get_capabilities(&adapter);
                caps.formats
                    .iter()
                    .copied()
                    .find(|f| f.is_srgb())
                    .unwrap_or(caps.formats[0])
            })
            .unwrap_or(wgpu::TextureFormat::Rgba8UnormSrgb);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voxel surfaces"),
            source: wgpu::ShaderSource::Wgsl(include_str!("terrain.wgsl").into()),
        });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera"),
            entries: &[layout_entry(
                0,
                wgpu::ShaderStages::VERTEX_FRAGMENT,
                wgpu::BufferBindingType::Uniform,
            )],
        });
        let tile_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("merged voxel faces"),
            entries: &[
                layout_entry(
                    0,
                    wgpu::ShaderStages::VERTEX,
                    wgpu::BufferBindingType::Uniform,
                ),
                layout_entry(
                    1,
                    wgpu::ShaderStages::VERTEX,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
                layout_entry(
                    2,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain layout"),
            bind_group_layouts: &[&frame_layout, &tile_layout],
            push_constant_ranges: &[],
        });
        let hud_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("HUD layout"),
            bind_group_layouts: &[&frame_layout],
            push_constant_ranges: &[],
        });
        let make_pipeline = |label, layout: &wgpu::PipelineLayout, vs, fs, terrain: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: if terrain {
                        Some(wgpu::Face::Back)
                    } else {
                        None
                    },
                    ..Default::default()
                },
                depth_stencil: if terrain {
                    Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: true,
                        depth_compare: wgpu::CompareFunction::Less,
                        stencil: Default::default(),
                        bias: Default::default(),
                    })
                } else {
                    Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: false,
                        depth_compare: wgpu::CompareFunction::Always,
                        stencil: Default::default(),
                        bias: Default::default(),
                    })
                },
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        let pipeline = make_pipeline(
            "voxel terrain",
            &pipeline_layout,
            "vs_main",
            "fs_main",
            true,
        );
        let hud = make_pipeline("crosshair", &hud_layout, "vs_hud", "fs_hud", false);
        let frame_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera uniform"),
            contents: bytemuck::bytes_of(&FrameUniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let frame_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buffer.as_entire_binding(),
            }],
        });
        let depth = depth(&device, width, height);
        Ok(Self {
            device,
            queue,
            format,
            adapter_name,
            pipeline,
            hud,
            frame_buffer,
            frame_bind,
            tile_layout,
            depth,
            width,
            height,
            tiles: BTreeMap::new(),
            bytes: 0,
            frame: 0,
        })
    }
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.depth = depth(&self.device, self.width, self.height);
    }
    pub fn upload(
        &mut self,
        key: MeshKey,
        mesh: TerrainMesh,
        protected: &BTreeSet<MeshKey>,
    ) -> Result<(), String> {
        let size = mesh.bytes()
            + mesh
                .parts
                .iter()
                .filter(|p| !p.surface.quads.is_empty())
                .count()
                * 32;
        while self.bytes + size > GPU_BUDGET {
            let candidate = self
                .tiles
                .iter()
                .filter(|(key, _)| !protected.contains(key))
                .min_by_key(|(_, tile)| tile.used)
                .map(|(key, _)| *key);
            let Some(candidate) = candidate else {
                return Err("GPU mesh budget reached; reduce LOD tile budget".into());
            };
            self.bytes -= self.tiles.remove(&candidate).unwrap().bytes;
        }
        let heights = mesh.heights();
        let quads = mesh.quads();
        let faces = mesh.faces();
        let mut parts = Vec::new();
        for part in mesh.parts {
            if part.surface.quads.is_empty() {
                continue;
            }
            let records: Vec<_> = part
                .surface
                .quads
                .iter()
                .map(|q| GpuQuad {
                    origin: q.origin,
                    face: q.face,
                    size: q.size,
                    color_offset: q.color_offset,
                    padding: 0,
                })
                .collect();
            let uniform = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("tile origin and grid step"),
                    contents: bytemuck::cast_slice(&[0f32; 8]),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                });
            let geometry = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("merged rectangles"),
                    contents: bytemuck::cast_slice(&records),
                    usage: wgpu::BufferUsages::STORAGE,
                });
            let colors = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("exact cell colors"),
                    contents: bytemuck::cast_slice(&part.surface.colors),
                    usage: wgpu::BufferUsages::STORAGE,
                });
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("tile"),
                layout: &self.tile_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: geometry.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: colors.as_entire_binding(),
                    },
                ],
            });
            parts.push(Part {
                uniform,
                bind,
                count: records.len() as u32,
                step: part.step,
            });
        }
        if let Some(old) = self.tiles.insert(
            key,
            Tile {
                parts,
                bytes: size,
                quads,
                faces,
                heights,
                used: self.frame,
            },
        ) {
            self.bytes -= old.bytes;
        }
        self.bytes += size;
        Ok(())
    }
    pub fn render(
        &mut self,
        target: &wgpu::TextureView,
        camera: &Camera,
        cover: &BTreeSet<MeshKey>,
        selection: Option<[i64; 3]>,
    ) -> usize {
        self.frame += 1;
        let eye = camera.eye();
        let matrix = camera.matrix(self.width, self.height);
        let selection = selection.map_or([0.; 4], |p| {
            [
                (p[0] as f64 / 16. - eye[0]) as f32,
                (p[1] as f64 / 16. - eye[1]) as f32,
                (p[2] as f64 / 16. - eye[2]) as f32,
                1.,
            ]
        });
        let frame = FrameUniform {
            vp: matrix.to_cols_array(),
            selection,
            viewport: [self.width as f32, self.height as f32, 0., 0.],
        };
        self.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame));
        let mut visible = Vec::new();
        for key in cover {
            let Some(tile) = self.tiles.get_mut(key) else {
                continue;
            };
            let [x, z] = key.tile.origin();
            let e = key.tile.edge();
            let low = [
                x as f64 / 16. - eye[0],
                tile.heights[0] as f64 / 16. - eye[1],
                z as f64 / 16. - eye[2],
            ];
            let high = [
                (x + e).min(EXTENT) as f64 / 16. - eye[0],
                tile.heights[1] as f64 / 16. - eye[1],
                (z + e).min(EXTENT) as f64 / 16. - eye[2],
            ];
            if !in_frustum(matrix, low, high) {
                continue;
            }
            tile.used = self.frame;
            for part in &tile.parts {
                self.queue.write_buffer(
                    &part.uniform,
                    0,
                    bytemuck::cast_slice(&[
                        (x as f64 / 16. - eye[0]) as f32,
                        -eye[1] as f32,
                        (z as f64 / 16. - eye[2]) as f32,
                        part.step as f32,
                        (EXTENT - x) as f32,
                        32768.,
                        (EXTENT - z) as f32,
                        0.,
                    ]),
                );
            }
            visible.push(*key);
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("voxel frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terrain + crosshair"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.435,
                            g: 0.590,
                            b: 0.729,
                            a: 1.,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.frame_bind, &[]);
            for key in &visible {
                for part in &self.tiles[key].parts {
                    pass.set_bind_group(1, &part.bind, &[]);
                    pass.draw(0..6, 0..part.count);
                }
            }
            pass.set_pipeline(&self.hud);
            pass.set_bind_group(0, &self.frame_bind, &[]);
            pass.draw(0..6, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        visible.len()
    }
    pub fn offscreen(&self) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native screenshot"),
            size: wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }
    pub fn screenshot(&self, texture: &wgpu::Texture, path: &Path) -> microvoxel::Result<()> {
        let padded = (self.width * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PNG readback"),
            size: padded as u64 * self.height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        let (tx, rx) = mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let mapped = buffer.slice(..).get_mapped_range();
        let mut pixels = Vec::with_capacity((self.width * self.height * 4) as usize);
        for row in mapped.chunks_exact(padded as usize) {
            pixels.extend_from_slice(&row[..(self.width * 4) as usize]);
        }
        if self.format == wgpu::TextureFormat::Bgra8UnormSrgb
            || self.format == wgpu::TextureFormat::Bgra8Unorm
        {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        let mut encoder = png::Encoder::new(
            std::io::BufWriter::new(File::create(path)?),
            self.width,
            self.height,
        );
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&pixels)?;
        drop(mapped);
        buffer.unmap();
        println!("Saved {}", path.display());
        Ok(())
    }
}
fn depth(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}
fn in_frustum(matrix: Mat4, low: [f64; 3], high: [f64; 3]) -> bool {
    let corners: Vec<_> = (0..8)
        .map(|i| {
            matrix
                * Vec4::new(
                    if i & 1 == 0 { low[0] } else { high[0] } as f32,
                    if i & 2 == 0 { low[1] } else { high[1] } as f32,
                    if i & 4 == 0 { low[2] } else { high[2] } as f32,
                    1.,
                )
        })
        .collect();
    !(corners.iter().all(|p| p.x < -p.w)
        || corners.iter().all(|p| p.x > p.w)
        || corners.iter().all(|p| p.y < -p.w)
        || corners.iter().all(|p| p.y > p.w)
        || corners.iter().all(|p| p.z < 0.)
        || corners.iter().all(|p| p.z > p.w))
}

#[cfg(test)]
mod tests {
    #[test]
    fn terrain_and_hud_shaders_validate_without_a_graphics_device() {
        let shader = naga::front::wgsl::parse_str(include_str!("terrain.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&shader)
        .unwrap();
    }
    #[test]
    #[ignore = "needs a Vulkan/Metal/DX12 device; run explicitly"]
    fn gpu_draws_the_exposed_top_of_a_flat_voxel_surface() {
        use super::*;
        use microvoxel::{
            generation::FlatTerrain,
            render::{lod::TileKey, terrain::Sampler},
            CellPos, World,
        };
        use std::sync::Arc;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let mut gpu = pollster::block_on(Gpu::new(&instance, None, 256, 256)).unwrap();
        let world = Arc::new(World::new(FlatTerrain {
            height: 5721,
            water: 5721,
            seed: 42,
        }));
        let mut sampler = Sampler::new(world);
        let key = MeshKey {
            tile: TileKey {
                level: 10,
                x: 582,
                z: 314,
            },
            neighbors: [10; 4],
            revision: 0,
        };
        gpu.upload(key, sampler.mesh(key).unwrap(), &BTreeSet::new())
            .unwrap();
        let camera = Camera::at(CellPos::new(149056, 5748, 80592));
        let texture = gpu.offscreen();
        let view = texture.create_view(&Default::default());
        gpu.render(&view, &camera, &[key].into(), None);
        let path = std::env::temp_dir().join("microvoxel-gpu-surface-test.png");
        gpu.screenshot(&texture, &path).unwrap();
        let decoder = png::Decoder::new(std::io::BufReader::new(File::open(&path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut bytes = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut bytes).unwrap();
        // Away from the crosshair, the center must be grass, not clear sky.
        let offset = (176 * 256 + 128) * 4;
        let pixel = &bytes[offset..offset + 3];
        assert!(
            pixel[1] > pixel[0] && pixel[1] > pixel[2],
            "surface missing: {pixel:?}"
        );
        let top = (80 * 256 + 128) * 4;
        assert!(
            bytes[top + 2] > bytes[top + 1],
            "sky must be above the ground"
        );
        let _ = std::fs::remove_file(path);
    }
}
