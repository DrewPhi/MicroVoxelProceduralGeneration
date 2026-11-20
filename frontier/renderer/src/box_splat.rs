//! Direct projection of bounded canonical surface cells. This comparison uses
//! the same immutable occupancy snapshot and halo masks as the brick tracer.
use crate::brick_ray::{BrickSnapshot, RayCamera};
use glam::{Mat4, Vec3};
use std::mem::size_of;
use wgpu::util::DeviceExt;

pub(crate) struct BoxSplatGpu {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    camera: wgpu::Buffer,
    size: [usize; 3],
    submitted_cells: u32,
    exposed_faces: usize,
    gpu_bytes: usize,
}

impl BoxSplatGpu {
    pub(crate) fn new(
        device: &wgpu::Device,
        snapshot: &BrickSnapshot,
        format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Result<Self, String> {
        let cells = snapshot.collect_surface_cells();
        let submitted_cells = u32::try_from(cells.len()).map_err(|e| e.to_string())?;
        let exposed_faces = cells
            .iter()
            .map(|cell| (cell[3] >> 8).count_ones() as usize)
            .sum();
        let storage_cells = if cells.is_empty() {
            &[[0; 4]][..]
        } else {
            &cells
        };
        let cell_bytes = size_of_val(storage_cells);
        let limits = device.limits();
        if cell_bytes as u64
            > u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size)
        {
            return Err(format!(
                "box splat surface cells exceed the GPU storage buffer limit ({cell_bytes} bytes)"
            ));
        }
        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("box splat camera"),
            size: size_of::<RayCamera>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cells = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("canonical surface cell words"),
            contents: bytemuck::cast_slice(storage_cells),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("box splat shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("box_splat.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bounded voxel box splats"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bounded box splat resources"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: cells.as_entire_binding(),
                },
            ],
        });
        Ok(Self {
            pipeline,
            bind_group,
            camera,
            size: snapshot.size,
            submitted_cells,
            exposed_faces,
            gpu_bytes: cell_bytes + size_of::<RayCamera>(),
        })
    }

    pub(crate) fn update_camera(
        &self,
        queue: &wgpu::Queue,
        matrix: Mat4,
        eye_local: Vec3,
        viewport: [u32; 2],
    ) {
        let data = RayCamera::new(matrix, eye_local, viewport, self.size, [0; 3], 0);
        queue.write_buffer(&self.camera, 0, bytemuck::bytes_of(&data));
    }

    pub(crate) fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..4, 0..self.submitted_cells);
    }

    pub(crate) fn triangle_count(&self) -> u32 {
        self.submitted_cells * 2
    }

    pub(crate) fn json(&self) -> String {
        format!(
            "{{\"submitted_cells\":{},\"exposed_faces\":{},\"gpu_bytes\":{},\"selection\":\"cpu_surface_cells_from_immutable_brick_snapshot\"}}",
            self.submitted_cells, self.exposed_faces, self.gpu_bytes
        )
    }
}
