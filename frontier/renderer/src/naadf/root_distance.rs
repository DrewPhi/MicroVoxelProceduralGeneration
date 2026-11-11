//! Disposable acceleration for exact roots, computed in fixed-size GPU tiles.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use wgpu::util::DeviceExt;

const SIDE: u32 = 32;
const MAX_EDGE: u32 = 128;
const SCRATCH_BYTES: usize = SIDE.pow(3) as usize * 4;

pub(super) struct Cache {
    edge: u32,
    ready: u32,
    armed: bool,
    in_flight: Arc<AtomicBool>,
    uniform: wgpu::Buffer,
    _texture: wgpu::Texture,
    groups: [wgpu::BindGroup; 2],
    layout: wgpu::PipelineLayout,
    kernels: Option<[wgpu::ComputePipeline; 5]>,
}

fn edge(n: u32) -> u32 {
    n.next_multiple_of(SIDE).min(MAX_EDGE)
}

pub(super) fn bytes(n: u32) -> usize {
    4 * edge(n).pow(3) as usize + 2 * SCRATCH_BYTES + 48
}

fn texture(device: &wgpu::Device, edge: u32, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("NAADF root distance tiles"),
        size: wgpu::Extent3d {
            width: edge,
            height: edge,
            depth_or_array_layers: edge,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::R32Uint,
        usage,
        view_formats: &[],
    })
}

pub(super) fn empty(device: &wgpu::Device) -> wgpu::TextureView {
    texture(device, 1, wgpu::TextureUsages::TEXTURE_BINDING).create_view(&Default::default())
}

// First compute the camera tile, then alternate either side along each axis.
fn centered(rank: u32, side: u32) -> u32 {
    if rank % 2 == 0 {
        side / 2 + rank / 2
    } else {
        side / 2 - rank.div_ceil(2)
    }
}

impl Cache {
    pub(super) fn new(
        device: &wgpu::Device,
        n: u32,
        roots: &wgpu::TextureView,
    ) -> (Self, wgpu::TextureView) {
        let edge = edge(n);
        let texture = texture(
            device,
            edge,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        );
        let view = texture.create_view(&Default::default());
        let scratch: [_; 2] = std::array::from_fn(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("NAADF distance ping pong"),
                size: SCRATCH_BYTES as u64,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("NAADF distance tile coordinates"),
            contents: &[0; 48],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let buffer = |ty| wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("NAADF root distance build resources"),
            entries: &[
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                },
                buffer(wgpu::BufferBindingType::Storage { read_only: true }),
                buffer(wgpu::BufferBindingType::Storage { read_only: false }),
                wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::R32Uint,
                    view_dimension: wgpu::TextureViewDimension::D3,
                },
                buffer(wgpu::BufferBindingType::Uniform),
            ]
            .into_iter()
            .enumerate()
            .map(|(binding, ty)| wgpu::BindGroupLayoutEntry {
                binding: binding as u32,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty,
                count: None,
            })
            .collect::<Vec<_>>(),
        });
        let groups = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("NAADF root distance tile"),
                layout: &layout,
                entries: &[
                    wgpu::BindingResource::TextureView(roots),
                    scratch[i].as_entire_binding(),
                    scratch[1 - i].as_entire_binding(),
                    wgpu::BindingResource::TextureView(&view),
                    uniform.as_entire_binding(),
                ]
                .into_iter()
                .enumerate()
                .map(|(binding, resource)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource,
                })
                .collect::<Vec<_>>(),
            })
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("NAADF root distance build"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        (
            Self {
                edge,
                ready: 0,
                armed: false,
                in_flight: Arc::new(AtomicBool::new(false)),
                uniform,
                _texture: texture,
                groups,
                layout: pipeline_layout,
                kernels: None,
            },
            view,
        )
    }

    pub(super) fn invalidate(&mut self) {
        // An old in-flight tile may finish, but its rank is no longer valid.
        // Keep the fence so the next generation can't queue additional work
        // while that tile is still running.
        self.ready = 0;
        self.armed = false;
    }

    pub(super) fn progress(&self) -> (u32, u32) {
        (self.ready, (self.edge / SIDE).pow(3))
    }

    pub(super) fn advance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        n: u32,
        ring: [u32; 3],
    ) {
        // Keep the first frame after publication on exact fallback.
        if !self.armed {
            self.armed = true;
            return;
        }
        let side = self.edge / SIDE;
        let rank = self.ready;
        if rank >= side.pow(3) || self.in_flight.load(Ordering::Acquire) {
            return;
        }
        let base = [rank % side, (rank / side) % side, rank / side.pow(2)]
            .map(|v| centered(v, side) * SIDE);
        let offset = n.saturating_sub(MAX_EDGE) / 2;
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::cast_slice(&[
                n, offset, 0, 0, ring[0], ring[1], ring[2], 0, base[0], base[1], base[2], 0,
            ]),
        );
        let kernels = self.kernels.get_or_insert_with(|| {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("NAADF root distance build"),
                source: wgpu::ShaderSource::Wgsl(include_str!("root_distance.wgsl").into()),
            });
            ["seed", "expand_x", "expand_y", "expand_z", "publish"].map(|entry_point| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry_point),
                    layout: Some(&self.layout),
                    module: &shader,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("NAADF bounded root distance tile"),
        });
        // One seed, five rounds of three-axis Jacobi, one publication. A
        // 32-cube has a maximum face span of 31 root cells.
        let mut ping = 0;
        for kernel in std::iter::once(0)
            .chain((0..5).flat_map(|_| 1..=3))
            .chain(std::iter::once(4))
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("NAADF distance stage"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&kernels[kernel]);
            pass.set_bind_group(0, &self.groups[ping], &[]);
            pass.dispatch_workgroups(SIDE / 4, SIDE / 4, SIDE / 4);
            ping ^= 1;
        }
        self.in_flight.store(true, Ordering::Release);
        queue.submit(Some(encoder.finish()));
        let in_flight = Arc::clone(&self.in_flight);
        queue.on_submitted_work_done(move || in_flight.store(false, Ordering::Release));
        // Later render submissions can use the tile immediately by queue order.
        self.ready += 1;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn centered_rank_matches_traversal_shader_inverse() {
        for side in 1..=4 {
            for rank in 0..side {
                let p = super::centered(rank, side);
                assert!(p < side);
                let centre = side / 2;
                let inverse = if p < centre {
                    2 * (centre - p) - 1
                } else {
                    2 * (p - centre)
                };
                assert_eq!(rank, inverse);
            }
        }
    }

    #[test]
    fn compute_shader_validates() {
        let shader = naga::front::wgsl::parse_str(include_str!("root_distance.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&shader)
        .unwrap();
    }
}
