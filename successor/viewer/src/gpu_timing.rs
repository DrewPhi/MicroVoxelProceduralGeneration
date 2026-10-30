//! Optional, nonblocking GPU render-pass timings for wgpu 24.
//!
//! Integration:
//! 1. Include `required_features(adapter.features())` in `request_device`.
//! 2. Construct `GpuTiming::new(&device, &queue)` after device creation.
//! 3. Once per `render_scene`, call `begin(frame)`; put `pass_writes(token)` in
//!    the terrain render pass descriptor's `timestamp_writes` field.
//! 4. After ending that pass, call `resolve(&mut encoder, token)`. Submit the
//!    encoder, then call `after_submit(token)`. Call `poll(&device)` each frame.
//! `latest()` is GPU pass time in ms, not synchronized CPU frame time. Each
//! sample uses its own query set and readback buffer until mapping completes.
use std::sync::mpsc;

const SLOTS: usize = 3;
const INTERVAL: u64 = 32;
const QUERY_BYTES: u64 = 2 * std::mem::size_of::<u64>() as u64;

/// Request only the base feature: pass boundary timestamps require no inside
/// encoder or inside pass timestamp feature.
pub fn required_features(available: wgpu::Features) -> wgpu::Features {
    available & wgpu::Features::TIMESTAMP_QUERY
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    slot: usize,
    frame: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub frame: u64,
    pub pass_ms: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Recording,
    Mapping,
}

struct Slot {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    state: State,
    frame: u64,
}

pub struct GpuTiming {
    slots: Vec<Slot>,
    period_ns: f64,
    done_tx: mpsc::Sender<(usize, Result<(), wgpu::BufferAsyncError>)>,
    done_rx: mpsc::Receiver<(usize, Result<(), wgpu::BufferAsyncError>)>,
    latest: Option<Sample>,
    dropped_samples: u64,
    map_failures: u64,
}

impl GpuTiming {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let (done_tx, done_rx) = mpsc::channel();
        let slots = if device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            (0..SLOTS)
                .map(|_| Slot {
                    queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                        label: Some("terrain pass timestamps"),
                        ty: wgpu::QueryType::Timestamp,
                        count: 2,
                    }),
                    resolve: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("terrain timestamp resolve"),
                        size: QUERY_BYTES,
                        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                        mapped_at_creation: false,
                    }),
                    readback: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("terrain timestamp readback"),
                        size: QUERY_BYTES,
                        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    state: State::Idle,
                    frame: 0,
                })
                .collect()
        } else {
            Vec::new()
        };
        Self {
            slots,
            period_ns: queue.get_timestamp_period() as f64,
            done_tx,
            done_rx,
            latest: None,
            dropped_samples: 0,
            map_failures: 0,
        }
    }

    pub fn available(&self) -> bool {
        !self.slots.is_empty()
    }
    pub fn status(&self) -> &'static str {
        if self.available() {
            "GPU timestamp query available"
        } else {
            "GPU timestamp query unavailable"
        }
    }
    pub fn latest(&self) -> Option<Sample> {
        self.latest
    }
    pub fn dropped_samples(&self) -> u64 {
        self.dropped_samples
    }
    pub fn map_failures(&self) -> u64 {
        self.map_failures
    }
    /// Managed buffer capacity; driver/query-object overhead is unknown.
    pub fn allocated_buffer_bytes(&self) -> u64 {
        self.slots.len() as u64 * QUERY_BYTES * 2
    }

    pub fn begin(&mut self, frame: u64) -> Option<Token> {
        if self.slots.is_empty() || frame % INTERVAL != 0 {
            return None;
        }
        let Some((index, slot)) = self
            .slots
            .iter_mut()
            .enumerate()
            .find(|(_, s)| s.state == State::Idle)
        else {
            self.dropped_samples += 1;
            return None;
        };
        slot.state = State::Recording;
        slot.frame = frame;
        Some(Token { slot: index, frame })
    }
    pub fn pass_writes(&self, token: Option<Token>) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let token = token?;
        let slot = self.slots.get(token.slot)?;
        if slot.state != State::Recording || slot.frame != token.frame {
            return None;
        }
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &slot.queries,
            beginning_of_pass_write_index: Some(0),
            end_of_pass_write_index: Some(1),
        })
    }
    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder, token: Option<Token>) {
        let Some(token) = token else {
            return;
        };
        let slot = &self.slots[token.slot];
        assert!(slot.state == State::Recording && slot.frame == token.frame);
        encoder.resolve_query_set(&slot.queries, 0..2, &slot.resolve, 0);
        encoder.copy_buffer_to_buffer(&slot.resolve, 0, &slot.readback, 0, QUERY_BYTES);
    }
    /// Call only after submitting the encoder that contains `resolve`.
    pub fn after_submit(&mut self, token: Option<Token>) {
        let Some(token) = token else {
            return;
        };
        let slot = &mut self.slots[token.slot];
        assert!(slot.state == State::Recording && slot.frame == token.frame);
        slot.state = State::Mapping;
        let tx = self.done_tx.clone();
        slot.readback
            .slice(..QUERY_BYTES)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send((token.slot, result));
            });
    }
    pub fn poll(&mut self, device: &wgpu::Device) {
        device.poll(wgpu::Maintain::Poll);
        while let Ok((index, result)) = self.done_rx.try_recv() {
            let slot = &mut self.slots[index];
            if slot.state != State::Mapping {
                continue;
            }
            if result.is_ok() {
                let data = slot.readback.slice(..QUERY_BYTES).get_mapped_range();
                let start = u64::from_le_bytes(data[0..8].try_into().unwrap());
                let end = u64::from_le_bytes(data[8..16].try_into().unwrap());
                drop(data);
                slot.readback.unmap();
                if end >= start {
                    self.latest = Some(Sample {
                        frame: slot.frame,
                        pass_ms: (end - start) as f64 * self.period_ns / 1_000_000.0,
                    });
                } else {
                    self.map_failures += 1;
                }
            } else {
                self.map_failures += 1;
            }
            slot.state = State::Idle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn feature_request_is_optional_and_narrow() {
        assert_eq!(
            required_features(wgpu::Features::empty()),
            wgpu::Features::empty()
        );
        assert_eq!(
            required_features(
                wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::INDIRECT_FIRST_INSTANCE
            ),
            wgpu::Features::TIMESTAMP_QUERY
        );
    }
}
