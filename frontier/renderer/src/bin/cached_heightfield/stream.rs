use super::{elapsed, fetch, readback, Result};
use bytemuck::{Pod, Zeroable};
use glam::{DVec2, IVec2};
use std::time::Instant;

const ALIGN: i32 = 64;
const SOURCE: &str = concat!(
    include_str!("stream.wgsl"),
    "\n",
    include_str!("../alpine_source.wgsl")
);

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Job {
    params: [u32; 4],
    base: [i32; 4],
    rect: [u32; 4],
    metrics: [f32; 4],
    lattice: [[i32; 4]; 9],
}

#[derive(Debug)]
pub struct Update {
    pub new_samples: u64,
    pub halo_samples: u64,
    pub bounds_nodes: u64,
    pub gpu_ms: f64,
    pub wall_ms: f64,
}

pub struct Stream {
    side: u32,
    seed: u64,
    pitch: f64,
    sample_shift: f32,
    origin: Option<IVec2>,
    uniform: wgpu::Buffer,
    stride: u32,
    jobs: Vec<(wgpu::ComputePipeline, wgpu::BindGroup)>,
    query: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
}

fn squared_difference(old: Option<IVec2>, new: IVec2, n: i32) -> Vec<[u32; 4]> {
    let Some(old) = old else {
        return vec![[0, 0, n as u32, n as u32]];
    };
    let lo = (old - new).max(IVec2::ZERO);
    let hi = (old - new + IVec2::splat(n)).min(IVec2::splat(n));
    if lo.x >= hi.x || lo.y >= hi.y {
        return vec![[0, 0, n as u32, n as u32]];
    }
    let as_rect = |x: i32, y: i32, w: i32, h: i32| [x as u32, y as u32, w as u32, h as u32];
    [
        as_rect(0, 0, lo.x, n),
        as_rect(hi.x, 0, n - hi.x, n),
        as_rect(lo.x, 0, hi.x - lo.x, lo.y),
        as_rect(lo.x, hi.y, hi.x - lo.x, n - hi.y),
    ]
    .into_iter()
    .filter(|r| r[2] > 0 && r[3] > 0)
    .collect()
}

impl Stream {
    pub fn new(
        device: &wgpu::Device,
        side: u32,
        seed: u64,
        pitch: f64,
        heights: &wgpu::Texture,
        maxima: &wgpu::Texture,
    ) -> Self {
        let levels = maxima.mip_level_count();
        let root = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("procedural height strips and maximum mips"),
            source: wgpu::ShaderSource::Wgsl(SOURCE.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    count: None,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Job>() as u64),
                    },
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    count: None,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    count: None,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::R32Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let stride = (std::mem::size_of::<Job>() as u32)
            .next_multiple_of(device.limits().min_uniform_buffer_offset_alignment);
        // Four disjoint strips per mip, plus the two halo strips at level zero.
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(stride * (4 * levels + 8)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mips: Vec<_> = (0..levels)
            .map(|i| {
                maxima.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: i,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let height = heights.create_view(&Default::default());
        let mut jobs = Vec::new();
        for level in 0..=levels {
            let (entry, source, dest) = match level {
                0 => ("emit_height", &mips[0], &height),
                1 => ("emit_leaf", &height, &mips[0]),
                _ => (
                    "emit_parent",
                    &mips[(level - 2) as usize],
                    &mips[(level - 1) as usize],
                ),
            };
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &root,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(entry),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &uniform,
                            offset: 0,
                            size: wgpu::BufferSize::new(std::mem::size_of::<Job>() as u64),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(dest),
                    },
                ],
            });
            jobs.push((pipeline, bind));
        }
        let query = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: None,
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Self {
            side,
            seed,
            pitch,
            sample_shift: 0.,
            origin: None,
            uniform,
            stride,
            jobs,
            query,
            resolve,
            read: readback(device, 16),
        }
    }

    pub fn at_native_centers(mut self) -> Self {
        self.sample_shift = 0.5;
        self
    }

    pub fn target(&self, eye: DVec2) -> IVec2 {
        (eye / (self.pitch * f64::from(ALIGN))).floor().as_ivec2() * ALIGN
            - IVec2::splat(self.side as i32 / 2)
    }

    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        base: IVec2,
    ) -> Result<Option<Update>> {
        if self.origin == Some(base) {
            return Ok(None);
        }
        let timer = Instant::now();
        let n = self.side;
        let mut regions = Vec::with_capacity(self.jobs.len());
        let mut raw = squared_difference(self.origin, base, n as i32);
        let count = |rs: &Vec<[u32; 4]>| {
            rs.iter()
                .map(|r| u64::from(r[2]) * u64::from(r[3]))
                .sum::<u64>()
        };
        let new_samples = count(&raw);
        raw.extend([[n, 0, 1, n], [0, n, n + 1, 1]]);
        regions.push(raw);
        for mip in 0..n.ilog2() + 1 {
            let old = self.origin.filter(|_| mip <= 6).map(|p| p >> mip as i32);
            regions.push(squared_difference(
                old,
                base >> mip as i32,
                (n >> mip) as i32,
            ));
        }
        let bounds_nodes = regions.iter().skip(1).map(count).sum::<u64>();
        let total = regions.iter().map(Vec::len).sum::<usize>();
        let mut uniforms = Vec::with_capacity(total * self.stride as usize);
        for (kernel, rects) in regions.iter().enumerate() {
            for &rect in rects {
                let job = Job {
                    params: [
                        self.seed as u32,
                        (self.seed >> 32) as u32,
                        n,
                        kernel.saturating_sub(1) as u32,
                    ],
                    base: [base.x, base.y, 0, 0],
                    rect,
                    metrics: [self.pitch as f32, self.sample_shift, 0., 0.],
                    lattice: [[0; 4]; 9],
                };
                uniforms.extend_from_slice(bytemuck::bytes_of(&job));
                uniforms.resize(uniforms.len().next_multiple_of(self.stride as usize), 0);
            }
        }
        queue.write_buffer(&self.uniform, 0, &uniforms);
        let mut cmd = device.create_command_encoder(&Default::default());
        let mut job_index = 0u32;
        for (kernel, rects) in regions.iter().enumerate() {
            if rects.is_empty() {
                continue;
            }
            let mut pass = cmd.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("rebuild exposed heightfield strips"),
                timestamp_writes: (kernel == 0 || kernel + 1 == regions.len()).then_some(
                    wgpu::ComputePassTimestampWrites {
                        query_set: &self.query,
                        beginning_of_pass_write_index: (kernel == 0).then_some(0),
                        end_of_pass_write_index: (kernel + 1 == regions.len()).then_some(1),
                    },
                ),
            });
            let (pipeline, bind) = &self.jobs[kernel];
            pass.set_pipeline(pipeline);
            for r in rects {
                pass.set_bind_group(0, bind, &[job_index * self.stride]);
                pass.dispatch_workgroups(r[2].div_ceil(8), r[3].div_ceil(8), 1);
                job_index += 1;
            }
        }
        cmd.resolve_query_set(&self.query, 0..2, &self.resolve, 0);
        cmd.copy_buffer_to_buffer(&self.resolve, 0, &self.read, 0, 16);
        // All pages and bounds belong to the new origin before any visibility
        // pass can observe it. This prototype measures the synchronous wait.
        queue.submit([cmd.finish()]);
        let data = fetch(device, &self.read)?;
        let times = bytemuck::cast_slice::<u8, u64>(&data);
        self.origin = Some(base);
        Ok(Some(Update {
            new_samples,
            halo_samples: u64::from(2 * n + 1),
            bounds_nodes,
            gpu_ms: times[1].saturating_sub(times[0]) as f64
                * f64::from(queue.get_timestamp_period())
                * 1e-6,
            wall_ms: elapsed(timer),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_cover_only_new_world_samples() {
        for x in -9..=9 {
            for y in -9..=9 {
                let old = IVec2::new(x, y);
                let strips = squared_difference(Some(old), IVec2::ZERO, 8);
                for j in 0..8 {
                    for i in 0..8 {
                        let reused = i >= x && i < x + 8 && j >= y && j < y + 8;
                        let included = strips
                            .iter()
                            .filter(|r| {
                                let [a, b, w, h] = r.map(|x| x as i32);
                                i >= a && i < a + w && j >= b && j < b + h
                            })
                            .count();
                        assert_eq!(included, usize::from(!reused));
                    }
                }
            }
        }
    }

    #[test]
    fn shader_validates() {
        let m = naga::front::wgsl::parse_str(SOURCE)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(SOURCE)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&m)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(SOURCE)));
    }
}
