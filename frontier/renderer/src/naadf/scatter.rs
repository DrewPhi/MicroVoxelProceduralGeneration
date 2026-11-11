//! Reusable optional staging/scatter path for newly reserved, unreachable slots.
use super::stream::Write;
use std::{cell::RefCell, time::Instant};

pub(super) const DESCRIPTOR_BYTES: usize = 16;
const SHADER: &str = include_str!("scatter.wgsl");

#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub packet_bytes: usize,
    pub descriptors: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            packet_bytes: 64 * 1024 * 1024,
            descriptors: 65535,
        }
    }
}

/// One exact slice of an original ordered source list, possibly ending partway
/// through its final range. No arena gaps or old destination data are packed.
pub(super) struct Packet {
    pub first: usize,
    pub last: usize,
    pub start: usize,
    pub bytes: usize,
    pub end: usize,
}
impl Packet {
    pub fn descriptors(&self) -> usize {
        self.last - self.first + 1
    }
    pub fn transfer_bytes(&self) -> usize {
        self.bytes + DESCRIPTOR_BYTES * self.descriptors()
    }
    pub fn plan(
        writes: &[Write],
        first: usize,
        start: usize,
        remaining: usize,
        limits: Limits,
    ) -> Option<Self> {
        let mut available = remaining.min(limits.packet_bytes) / 4 * 4;
        let mut payload = 0;
        let mut last = first;
        let mut offset = start;
        for index in first..writes.len().min(first + limits.descriptors) {
            if available < DESCRIPTOR_BYTES + 4 {
                break;
            }
            let source = &writes[index];
            let source_bytes = source.data.len() * 4;
            let count = (available - DESCRIPTOR_BYTES).min(source_bytes - offset);
            debug_assert!(count > 0 && count % 4 == 0);
            available -= DESCRIPTOR_BYTES + count;
            payload += count;
            last = index;
            offset += count;
            if offset < source_bytes {
                break;
            }
            // Save the inclusive final endpoint before beginning another range.
            if index + 1 < writes.len().min(first + limits.descriptors)
                && available >= DESCRIPTOR_BYTES + 4
            {
                offset = 0;
            } else {
                break;
            }
        }
        (payload != 0).then_some(Self {
            first,
            last,
            start,
            bytes: payload,
            end: offset,
        })
    }

    pub fn pack(&self, writes: &[Write], words: &mut Vec<u32>) {
        words.clear();
        words.resize(self.descriptors() * 4, 0);
        let mut remaining = self.bytes / 4;
        for (range, source) in writes[self.first..=self.last].iter().enumerate() {
            let offset = if range == 0 { self.start / 4 } else { 0 };
            let count = remaining.min(source.data.len() - offset);
            let descriptor = range * 4;
            words[descriptor] = u32::from(source.kind);
            words[descriptor + 1] = (source.word + offset).try_into().unwrap();
            words[descriptor + 2] = words.len().try_into().unwrap();
            words[descriptor + 3] = count.try_into().unwrap();
            words.extend_from_slice(&source.data[offset..offset + count]);
            remaining -= count;
        }
        debug_assert_eq!(remaining, 0);
        debug_assert_eq!(words.len() * 4, self.transfer_bytes());
    }
}

pub(super) struct Scatter {
    pipeline: wgpu::ComputePipeline,
    staging: wgpu::Buffer,
    bind: wgpu::BindGroup,
    words: RefCell<Vec<u32>>,
    pub limits: Limits,
    pub stages: Option<std::sync::Arc<super::gpu_stages::Profiler>>,
}
impl Scatter {
    pub fn new(device: &wgpu::Device, arenas: [&wgpu::Buffer; 3], allowance: usize) -> Self {
        let device_limits = device.limits();
        let capacity = allowance
            .min(device_limits.max_storage_buffer_binding_size as usize)
            .min(device_limits.max_buffer_size as usize)
            / 4
            * 4;
        assert!(capacity >= DESCRIPTOR_BYTES + 4);
        let limits = Limits {
            packet_bytes: capacity,
            descriptors: device_limits.max_compute_workgroups_per_dimension as usize,
        };
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("NAADF reusable scatter staging"),
            size: capacity as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("NAADF exact disjoint arena scatter"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("NAADF exact disjoint arena scatter"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("NAADF reusable scatter arenas"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                staging.as_entire_binding(),
                arenas[0].as_entire_binding(),
                arenas[1].as_entire_binding(),
                arenas[2].as_entire_binding(),
            ]
            .into_iter()
            .enumerate()
            .map(|(binding, resource)| wgpu::BindGroupEntry {
                binding: binding as u32,
                resource,
            })
            .collect::<Vec<_>>(),
        });
        Self {
            stages: None,
            pipeline,
            staging,
            bind,
            limits,
            words: RefCell::new(Vec::with_capacity(capacity / 4)),
        }
    }
    pub fn gpu_bytes(&self) -> usize {
        self.staging.size() as usize
    }
    pub fn cpu_bytes(&self) -> usize {
        self.words.borrow().capacity() * 4
    }
    pub fn submit(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        writes: &[Write],
        packet: &Packet,
    ) -> f64 {
        let start = Instant::now();
        let mut words = self.words.borrow_mut();
        packet.pack(writes, &mut words);
        let pack_ms = start.elapsed().as_secs_f64() * 1000.;
        queue.write_buffer(&self.staging, 0, bytemuck::cast_slice(&words));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("NAADF scatter upload submission"),
        });
        let timing = self
            .stages
            .as_ref()
            .and_then(|p| p.begin(super::gpu_stages::Stage::Scatter));
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("NAADF scatter upload"),
                timestamp_writes: timing.as_ref().map(|t| t.writes(true, true)),
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.dispatch_workgroups(packet.descriptors() as u32, 1, 1);
        }
        // Submit each packet before reusing its staging buffer. This places
        // the SSBO upload before scatter, and scatter before root publication.
        if let Some(t) = &timing {
            t.resolve(&mut encoder);
        }
        queue.submit([encoder.finish()]);
        if let Some(t) = timing {
            t.submitted();
        }
        pack_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scatter_shader_validates() {
        let module = naga::front::wgsl::parse_str(SHADER).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn scatter_packet_counts_descriptor_bytes_and_preserves_all_arena_holes() {
        let writes = [
            Write {
                kind: 0,
                word: 7,
                data: vec![1, 2, 3, 4, 5],
            },
            Write {
                kind: 2,
                word: 20,
                data: vec![6, 7],
            },
            Write {
                kind: 1,
                word: 19,
                data: vec![8, 9, 10],
            },
        ];
        let mut destination = [[99_u32; 32]; 3];
        let mut expected = destination;
        for w in &writes {
            expected[w.kind as usize][w.word..w.word + w.data.len()].copy_from_slice(&w.data);
        }
        let mut cursor = (0, 0);
        let mut staging = Vec::new();
        let mut packets = 0;
        while let Some(packet) = Packet::plan(
            &writes,
            cursor.0,
            cursor.1,
            48,
            Limits {
                packet_bytes: 48,
                descriptors: 2,
            },
        ) {
            assert!(packet.transfer_bytes() <= 48 && packet.descriptors() <= 2);
            packet.pack(&writes, &mut staging);
            for descriptor in staging[..packet.descriptors() * 4].chunks_exact(4) {
                let [kind, target, source, count]: [usize; 4] =
                    std::array::from_fn(|i| descriptor[i] as usize);
                destination[kind][target..target + count]
                    .copy_from_slice(&staging[source..source + count]);
            }
            cursor = if packet.end == writes[packet.last].data.len() * 4 {
                (packet.last + 1, 0)
            } else {
                (packet.last, packet.end)
            };
            packets += 1;
            assert!(packets < 10);
        }
        assert_eq!(cursor, (writes.len(), 0));
        assert_eq!(destination, expected);
        assert!(Packet::plan(&writes, 0, 0, 19, Limits::default()).is_none());
    }
}

#[cfg(test)]
#[test]
#[ignore = "explicit GPU parity: exact scatter holes, all arenas, partial ranges, and staging reuse"]
fn scatter_gpu_preserves_holes_and_reuses_staging_in_queue_order() {
    use wgpu::util::DeviceExt;
    pollster::block_on(async {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .expect("scatter oracle requires a Vulkan GPU");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await
            .unwrap();
        let initial = [0xfeed_abcd_u32; 256];
        let arenas: Vec<_> = (0..3)
            .map(|_| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("NAADF scatter oracle arena"),
                    contents: bytemuck::cast_slice(&initial),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                })
            })
            .collect();
        let writes = [
            Write {
                kind: 0,
                word: 7,
                data: (0..150).collect(),
            },
            Write {
                kind: 2,
                word: 20,
                data: vec![400, 401],
            },
            Write {
                kind: 1,
                word: 19,
                data: (500..660).collect(),
            },
            Write {
                kind: 2,
                word: 200,
                data: (700..708).collect(),
            },
        ];
        let mut expected = [initial; 3];
        for w in &writes {
            expected[w.kind as usize][w.word..w.word + w.data.len()].copy_from_slice(&w.data);
        }
        let scatter = Scatter::new(&device, [&arenas[0], &arenas[1], &arenas[2]], 640);
        let mut cursor = (0, 0);
        let mut packets = 0;
        while let Some(packet) = Packet::plan(&writes, cursor.0, cursor.1, 640, scatter.limits) {
            scatter.submit(&device, &queue, &writes, &packet);
            cursor = if packet.end == writes[packet.last].data.len() * 4 {
                (packet.last + 1, 0)
            } else {
                (packet.last, packet.end)
            };
            packets += 1;
            assert!(packets < 10);
        }
        assert!(packets >= 3);
        assert_eq!(cursor, (writes.len(), 0));
        assert_eq!(scatter.cpu_bytes(), 640);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("NAADF scatter oracle readback"),
            size: 3 * 256 * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        for (index, arena) in arenas.iter().enumerate() {
            encoder.copy_buffer_to_buffer(arena, 0, &readback, (index * 256 * 4) as u64, 256 * 4);
        }
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        let _ = device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap().unwrap();
        let bytes = readback.slice(..).get_mapped_range();
        let words = bytemuck::cast_slice::<u8, u32>(&bytes);
        for (actual, expected) in words.chunks_exact(256).zip(expected) {
            assert_eq!(actual, expected);
        }
    });
}
