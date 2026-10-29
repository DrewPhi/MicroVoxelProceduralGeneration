//! GPU compute harness for identical ray batches. Optional hardware timestamps;
//! fixed integer counters expose logical traversal work without guessing DRAM use.
use bytemuck::{Pod, Zeroable};
use std::time::Instant;
use wgpu::util::DeviceExt;
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Ray {
    pub origin: [f32; 4],
    pub direction: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct Hit {
    pub cell: [i32; 3],
    pub hit: u32,
    pub color: u32,
    pub visits: u32,
    pub tests: u32,
    pub loads: u32,
}
pub struct Measurements {
    pub gpu: [f64; 2],
    pub wall: [f64; 2],
    pub hits: Vec<Hit>,
}
pub struct Bench {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    pub adapter: String,
    pub timestamps: bool,
}
impl Bench {
    pub fn new() -> microvoxel::Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .ok_or("No study GPU")?;
        let features = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("voxel study"),
                required_features: features,
                required_limits:
                    wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
            },
            None,
        ))?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voxel study counters"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ray_study.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("column SVO DAG comparison"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Ok(Self {
            device,
            queue,
            pipeline,
            adapter: format!(
                "{} ({:?})",
                adapter.get_info().name,
                adapter.get_info().backend
            ),
            timestamps: !features.is_empty(),
        })
    }
    fn buffer(&self, label: &str, words: &[u32]) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(if words.is_empty() { &[0u32] } else { words }),
                usage: wgpu::BufferUsages::STORAGE,
            })
    }
    fn read(&self, buffer: &wgpu::Buffer) -> microvoxel::Result<Vec<u8>> {
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let bytes = buffer.slice(..).get_mapped_range().to_vec();
        buffer.unmap();
        Ok(bytes)
    }
    pub fn run(
        &self,
        source: &[u32],
        tree: &[u32],
        params: [u32; 8],
        rays: &[Ray],
    ) -> microvoxel::Result<Measurements> {
        let source = self.buffer("source/attributes", source);
        let tree = self.buffer("compact occupancy", tree);
        let rays_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("fixed rays"),
                contents: bytemuck::cast_slice(rays),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let output = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hit and work counters"),
            size: rays.len() as u64 * 32,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let params = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("study parameters"),
                contents: bytemuck::cast_slice(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("study data"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: source.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: tree.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: rays_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: output.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: params.as_entire_binding(),
                },
            ],
        });
        let queries = self.timestamps.then(|| {
            self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("kernel timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            })
        });
        let resolve = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("timestamp resolve"),
            size: 256,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let times_read = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("timestamp read"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut gpu = Vec::new();
        let mut wall = Vec::new();
        for iteration in 0..35 {
            let start = Instant::now();
            let mut encoder = self.device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("voxel benchmark"),
                    timestamp_writes: queries.as_ref().map(|query_set| {
                        wgpu::ComputePassTimestampWrites {
                            query_set,
                            beginning_of_pass_write_index: Some(0),
                            end_of_pass_write_index: Some(1),
                        }
                    }),
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups((rays.len() as u32).div_ceil(64), 1, 1);
            }
            if let Some(query) = &queries {
                encoder.resolve_query_set(query, 0..2, &resolve, 0);
                encoder.copy_buffer_to_buffer(&resolve, 0, &times_read, 0, 16);
            }
            self.queue.submit(Some(encoder.finish()));
            self.device.poll(wgpu::Maintain::Wait);
            let elapsed = start.elapsed().as_secs_f64() * 1000.;
            let time = if self.timestamps {
                let bytes = self.read(&times_read)?;
                let a = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
                let b = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
                (b - a) as f64 * self.queue.get_timestamp_period() as f64 / 1e6
            } else {
                f64::NAN
            };
            if iteration >= 5 {
                wall.push(elapsed);
                gpu.push(time);
            }
        }
        let read = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hit readback"),
            size: rays.len() as u64 * 32,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&output, 0, &read, 0, rays.len() as u64 * 32);
        self.queue.submit(Some(encoder.finish()));
        let bytes = self.read(&read)?;
        let hits = bytes
            .chunks_exact(32)
            .map(bytemuck::pod_read_unaligned)
            .collect();
        let percentiles = |mut values: Vec<f64>| {
            values.sort_by(f64::total_cmp);
            [
                values[values.len() / 2],
                values[(values.len() * 95 / 100).min(values.len() - 1)],
            ]
        };
        Ok(Measurements {
            gpu: percentiles(gpu),
            wall: percentiles(wall),
            hits,
        })
    }
}
