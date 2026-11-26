//! Directional horizon and broad sky occlusion from the resident heightfield.
use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3};
use std::cell::Cell;

const SIDE: u32 = 512;
pub(crate) const BINDING: u32 = 23;
pub(crate) const SAMPLER_BINDING: u32 = 24;
pub(crate) const GPU_BYTES: usize = (SIDE * SIDE * 8) as usize;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    grid: [f32; 4],
    sun: [f32; 4],
    cache: [i32; 4],
}

pub(crate) struct TerrainHorizon {
    pub view: wgpu::TextureView,
    _texture: wgpu::Texture,
    uniform: wgpu::Buffer,
    pipeline: wgpu::ComputePipeline,
    bind: wgpu::BindGroup,
    previous: Cell<Option<([f32; 4], Vec2)>>,
    builds: Cell<u64>,
}

impl TerrainHorizon {
    pub(crate) fn new(device: &wgpu::Device, heights: &wgpu::TextureView) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terrain horizon and sky visibility"),
            size: wgpu::Extent3d {
                width: SIDE,
                height: SIDE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain horizon parameters"),
            size: size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            ty,
            visibility: wgpu::ShaderStages::COMPUTE,
            count: None,
        };
        let group = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain horizon resources"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<Uniform>() as u64),
                    },
                ),
                entry(
                    1,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(
                    2,
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                ),
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain horizon cache"),
            source: wgpu::ShaderSource::Wgsl(include_str!("terrain_horizon.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain horizon"),
            bind_group_layouts: &[&group],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("terrain horizon"),
            layout: Some(&layout),
            module: &shader,
            entry_point: Some("build"),
            compilation_options: Default::default(),
            cache: None,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain horizon"),
            layout: &group,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(heights),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        Self {
            view,
            _texture: texture,
            uniform,
            pipeline,
            bind,
            previous: Cell::new(None),
            builds: Cell::new(0),
        }
    }

    pub(crate) fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        grid: [f32; 4],
        cache: [i32; 4],
        sun: Vec3,
    ) {
        let heading = Vec2::new(sun.x, sun.z).try_normalize().unwrap_or(Vec2::Y);
        if self.previous.get().is_some_and(|(old_grid, old_heading)| {
            grid == old_grid && old_heading.dot(heading) >= 0.99996
        }) {
            return;
        }
        self.previous.set(Some((grid, heading)));
        self.builds.set(self.builds.get() + 1);
        let light = Uniform {
            grid,
            sun: [heading.x, heading.y, 0., 0.],
            cache,
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&light));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("refresh terrain horizon"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.dispatch_workgroups(SIDE / 8, SIDE / 8, 1);
    }

    pub(crate) fn builds(&self) -> u64 {
        self.builds.get()
    }
}
