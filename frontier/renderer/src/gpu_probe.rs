//! Optional render-pass timestamps. Three asynchronous readback slots avoid
//! waiting for the GPU; saturated profiling drops measurements, never frames.
use std::sync::mpsc;

struct Slot {
    readback: wgpu::Buffer,
    pending: Option<mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
    record: bool,
}

pub struct GpuProbe {
    query: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    slots: [Slot; 3],
    period_ns: f64,
    pub stats: frontier_runtime::FrameStats,
    pub dropped: u64,
    pub full_stats: frontier_runtime::FrameStats,
    pub full_supported: bool,
    pub vegetation_supported: bool,
    pub plant_color_stats: frontier_runtime::FrameStats,
    pub plant_shadow_stats: frontier_runtime::FrameStats,
}

pub(crate) struct VegetationTiming<'a> {
    pub query: &'a wgpu::QuerySet,
    pub first: u32,
}
impl VegetationTiming<'_> {
    pub fn shadow_start(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.write_timestamp(self.query, self.first);
    }
    pub fn shadow_end(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.write_timestamp(self.query, self.first + 1);
    }
    pub fn color_start(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.write_timestamp(self.query, self.first + 2);
    }
    pub fn color_end(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.write_timestamp(self.query, self.first + 3);
    }
}

impl GpuProbe {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        Some(Self {
            query: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("render pass timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 24,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("timestamp resolve"),
                size: 256,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            slots: std::array::from_fn(|_| Slot {
                readback: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("timestamp asynchronous readback"),
                    size: 64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                pending: None,
                record: false,
            }),
            period_ns: queue.get_timestamp_period() as f64,
            stats: frontier_runtime::FrameStats::new(10000),
            dropped: 0,
            full_stats: frontier_runtime::FrameStats::new(10000),
            plant_color_stats: frontier_runtime::FrameStats::new(10000),
            plant_shadow_stats: frontier_runtime::FrameStats::new(10000),
            vegetation_supported: device.features().contains(
                wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
                    | wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES,
            ),
            full_supported: device
                .features()
                .contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS),
        })
    }

    pub fn poll(&mut self, device: &wgpu::Device) {
        device.poll(wgpu::Maintain::Poll);
        for slot in &mut self.slots {
            let Some(rx) = &slot.pending else { continue };
            match rx.try_recv() {
                Ok(Ok(())) => {
                    {
                        let bytes = slot.readback.slice(..).get_mapped_range();
                        let start = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
                        let end = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
                        if slot.record && end >= start {
                            self.stats
                                .record((end - start) as f64 * self.period_ns / 1_000_000.);
                        }
                        if slot.record && self.full_supported {
                            let start = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
                            let end = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
                            if end >= start {
                                self.full_stats
                                    .record((end - start) as f64 * self.period_ns / 1_000_000.);
                            }
                        }
                        if slot.record && self.vegetation_supported {
                            for (offset, stats) in [
                                (32, &mut self.plant_shadow_stats),
                                (48, &mut self.plant_color_stats),
                            ] {
                                let start = u64::from_le_bytes(
                                    bytes[offset..offset + 8].try_into().unwrap(),
                                );
                                let end = u64::from_le_bytes(
                                    bytes[offset + 8..offset + 16].try_into().unwrap(),
                                );
                                if end >= start {
                                    stats
                                        .record((end - start) as f64 * self.period_ns / 1_000_000.);
                                }
                            }
                        }
                    }
                    slot.readback.unmap();
                    slot.pending = None;
                }
                Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                    slot.pending = None;
                    self.dropped += 1;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    pub fn acquire(&mut self, record: bool) -> Option<usize> {
        if let Some(index) = self.slots.iter().position(|s| s.pending.is_none()) {
            self.slots[index].record = record;
            Some(index)
        } else {
            self.dropped += 1;
            None
        }
    }

    pub fn writes(&self, slot: usize) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.query,
            beginning_of_pass_write_index: Some(slot as u32 * 8),
            end_of_pass_write_index: Some(slot as u32 * 8 + 1),
        }
    }

    pub fn begin_frame(&self, encoder: &mut wgpu::CommandEncoder, slot: usize) {
        if self.full_supported {
            encoder.write_timestamp(&self.query, slot as u32 * 8 + 2);
        }
    }
    pub fn end_frame(&self, encoder: &mut wgpu::CommandEncoder, slot: usize) {
        if self.full_supported {
            encoder.write_timestamp(&self.query, slot as u32 * 8 + 3);
        }
    }
    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder, slot: usize) {
        let first = slot as u32 * 8;
        encoder.resolve_query_set(
            &self.query,
            first
                ..first
                    + if self.vegetation_supported {
                        8
                    } else if self.full_supported {
                        4
                    } else {
                        2
                    },
            &self.resolve,
            0,
        );
        encoder.copy_buffer_to_buffer(
            &self.resolve,
            0,
            &self.slots[slot].readback,
            0,
            if self.vegetation_supported {
                64
            } else if self.full_supported {
                32
            } else {
                16
            },
        );
    }

    pub fn vegetation_writes(&self, slot: usize) -> Option<VegetationTiming<'_>> {
        self.vegetation_supported.then_some(VegetationTiming {
            query: &self.query,
            first: slot as u32 * 8 + 4,
        })
    }

    /// Call after submitting the encoder that resolves this slot.
    pub fn submitted(&mut self, slot: usize) {
        let (tx, rx) = mpsc::channel();
        self.slots[slot]
            .readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        self.slots[slot].pending = Some(rx);
    }
}
