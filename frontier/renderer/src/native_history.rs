//! Reproject the native pixels marked by the voxel pass using fresh scene
//! depth each frame. Raster geometry, sky and text bypass this history.
use crate::Camera;
use std::cell::Cell;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    inverse: [[f32; 4]; 4],
    previous: [[f32; 4]; 4],
    eye_delta: [f32; 4],
}

pub(crate) struct NativeHistory {
    pub views: [wgpu::TextureView; 2],
    groups: [wgpu::BindGroup; 2],
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    previous: Cell<Option<(Camera, u64)>>,
    next: Cell<u32>,
    stationary_samples: Cell<u32>,
}

impl NativeHistory {
    pub fn new(
        device: &wgpu::Device,
        scene: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> Self {
        let views = std::array::from_fn(|_| {
            super::atmosphere::texture(
                device,
                "native sample history",
                wgpu::TextureFormat::Rgba16Float,
                width,
                height,
                1,
            )
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native sample history reprojection"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native sample history inputs"),
            entries: &std::array::from_fn::<_, 4, _>(|binding| wgpu::BindGroupLayoutEntry {
                binding: binding as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: if binding == 0 {
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    }
                } else {
                    wgpu::BindingType::Texture {
                        sample_type: if binding == 2 {
                            wgpu::TextureSampleType::Depth
                        } else {
                            wgpu::TextureSampleType::Float { filterable: false }
                        },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    }
                },
                count: None,
            }),
        });
        let groups = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native sample history inputs"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(scene),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&views[1 - i]),
                    },
                ],
            })
        });
        let pipeline = super::atmosphere::make_pipeline(
            device,
            "native sample history resolve",
            &layout,
            include_str!("native_history.wgsl"),
            wgpu::TextureFormat::Rgba16Float,
        );
        Self {
            views,
            groups,
            pipeline,
            uniform,
            previous: Cell::new(None),
            next: Cell::new(0),
            stationary_samples: Cell::new(0),
        }
    }

    pub fn prepare(
        &self,
        queue: &wgpu::Queue,
        camera: Camera,
        aspect: f32,
        enabled: bool,
        revision: u64,
    ) -> u32 {
        if !enabled {
            self.previous.set(None);
            self.stationary_samples.set(0);
            return 0;
        }
        let matrix = camera.matrix(camera.position, aspect);
        let mut previous = matrix;
        let mut eye_delta = [0.; 4];
        let mut stationary = false;
        if let Some((old, old_revision)) = self.previous.replace(Some((camera, revision))) {
            let delta = camera.position - old.position;
            if old_revision == revision
                && delta.length_squared() < 4.
                && old.direction().dot(camera.direction()) > 0.98
                && old.fov_y == camera.fov_y
                && old.far == camera.far
            {
                previous = old.matrix(old.position, aspect);
                stationary = delta.length_squared() < 1e-8
                    && old.yaw == camera.yaw
                    && old.pitch == camera.pitch;
                let retention = if stationary {
                    let samples = (self.stationary_samples.get() + 1).min(16);
                    self.stationary_samples.set(samples);
                    // A negative weight identifies a stationary frame. The
                    // history shader can then average exact pixel locations.
                    -(samples as f32) / (samples as f32 + 1.)
                } else {
                    0.78
                };
                eye_delta = delta.as_vec3().extend(retention).to_array();
            }
        }
        if !stationary {
            self.stationary_samples.set(0);
        }
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&Params {
                inverse: matrix.inverse().to_cols_array_2d(),
                previous: previous.to_cols_array_2d(),
                eye_delta,
            }),
        );
        1 + (self.next.get() & 15)
    }

    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) -> Option<usize> {
        self.previous.get()?;
        let index = (self.next.get() & 1) as usize;
        self.next.set(self.next.get().wrapping_add(1));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native sample history resolve"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.views[index],
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.groups[index], &[]);
        pass.draw(0..3, 0..1);
        Some(index)
    }
}
