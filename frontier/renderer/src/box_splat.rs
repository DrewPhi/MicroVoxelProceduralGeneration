//! Direct projection of bounded canonical surface cells. This comparison uses
//! the same immutable occupancy snapshot and halo masks as the brick tracer.
use crate::brick_ray::{BrickSnapshot, RayCamera};
use frontier_world::World;
use glam::{Mat4, Vec3};
use std::mem::size_of;
use wgpu::util::DeviceExt;

#[path = "box_splat_edits.rs"]
mod edits;
use edits::{EditableSplats, Splat};

#[derive(Default)]
struct EditMetrics {
    updates: u64,
    logical_writes: u64,
    batches: u64,
    upload_bytes: u64,
    last_affected_cells: usize,
    last_changed_records: usize,
    last_logical_writes: usize,
    last_batches: usize,
    last_upload_bytes: usize,
}

pub(crate) struct BoxSplatGpu {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    camera: wgpu::Buffer,
    storage: wgpu::Buffer,
    size: [usize; 3],
    submitted_cells: u32,
    exposed_faces: usize,
    gpu_bytes: usize,
    editable: Option<EditableSplats>,
    edits: EditMetrics,
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
        let active = cells.len().max(1);
        let limits = device.limits();
        let max_capacity = (u64::from(limits.max_storage_buffer_binding_size)
            .min(limits.max_buffer_size)
            / size_of::<Splat>() as u64) as usize;
        if active > max_capacity {
            return Err(format!(
                "box splat surface cells exceed the GPU storage buffer limit ({} bytes)",
                active * size_of::<Splat>(),
            ));
        }
        let incremental = std::env::var("FRONTIER_BOX_SPLAT_INCREMENTAL").as_deref() == Ok("1");
        let capacity = if incremental {
            (active + active.div_ceil(4).max(64)).min(max_capacity)
        } else {
            active
        };
        let cell_bytes = capacity * size_of::<Splat>();
        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("box splat camera"),
            size: size_of::<RayCamera>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let storage = if incremental {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("editable canonical surface cell words"),
                size: cell_bytes as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: true,
            });
            if !cells.is_empty() {
                let used = (cells.len() * size_of::<Splat>()) as u64;
                buffer
                    .slice(..used)
                    .get_mapped_range_mut()
                    .copy_from_slice(bytemuck::cast_slice(&cells));
            }
            buffer.unmap();
            buffer
        } else {
            let storage_cells = if cells.is_empty() {
                &[[0; 4]][..]
            } else {
                &cells
            };
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("canonical surface cell words"),
                contents: bytemuck::cast_slice(storage_cells),
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
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
                    resource: storage.as_entire_binding(),
                },
            ],
        });
        Ok(Self {
            pipeline,
            bind_group,
            camera,
            storage,
            size: snapshot.size,
            submitted_cells,
            exposed_faces,
            gpu_bytes: cell_bytes + size_of::<RayCamera>(),
            editable: incremental
                .then(|| EditableSplats::new(cells, snapshot.origin, snapshot.size, capacity)),
            edits: EditMetrics::default(),
        })
    }

    pub(crate) fn try_apply_edit(&mut self, queue: &wgpu::Queue, old: &World, new: &World) -> bool {
        let Some(editable) = &mut self.editable else {
            return false;
        };
        let Some(uploads) = editable.apply(old, new) else {
            return false;
        };
        let mut upload_bytes = 0;
        for range in &uploads.ranges {
            let bytes = bytemuck::cast_slice(&editable.records[range.clone()]);
            queue.write_buffer(
                &self.storage,
                (range.start * size_of::<Splat>()) as u64,
                bytes,
            );
            upload_bytes += bytes.len();
        }
        self.submitted_cells = editable.records.len() as u32;
        self.exposed_faces = editable.exposed_faces();
        self.edits.updates += 1;
        self.edits.logical_writes += uploads.logical_writes as u64;
        self.edits.batches += uploads.ranges.len() as u64;
        self.edits.upload_bytes += upload_bytes as u64;
        self.edits.last_affected_cells = uploads.affected_cells;
        self.edits.last_changed_records = uploads.changed_records;
        self.edits.last_logical_writes = uploads.logical_writes;
        self.edits.last_batches = uploads.ranges.len();
        self.edits.last_upload_bytes = upload_bytes;
        true
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
        let edits = &self.edits;
        format!(
            concat!(
                "{{\"submitted_cells\":{},\"exposed_faces\":{},\"gpu_bytes\":{},",
                "\"selection\":\"cpu_surface_cells_from_immutable_brick_snapshot\",\"incremental\":{{",
                "\"enabled\":{},\"capacity_cells\":{},\"cpu_sparse_slot_capacity\":{},",
                "\"updates\":{},\"logical_writes\":{},\"batches\":{},\"upload_bytes\":{},",
                "\"last_affected_cells\":{},\"last_changed_records\":{},\"last_logical_writes\":{},",
                "\"last_batches\":{},\"last_upload_bytes\":{}}}}}"
            ),
            self.submitted_cells,
            self.exposed_faces,
            self.gpu_bytes,
            self.editable.is_some(),
            self.editable.as_ref().map_or(self.submitted_cells.max(1) as usize, |s| s.capacity),
            self.editable.as_ref().map_or(0, EditableSplats::slot_capacity),
            edits.updates,
            edits.logical_writes,
            edits.batches,
            edits.upload_bytes,
            edits.last_affected_cells,
            edits.last_changed_records,
            edits.last_logical_writes,
            edits.last_batches,
            edits.last_upload_bytes,
        )
    }
}
