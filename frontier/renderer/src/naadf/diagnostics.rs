//! Explicit diagnostic counters. Atomic shaders and blocking readback are never
//! enabled for normal rendering; captures opt in separately from timing runs.
use wgpu::util::DeviceExt;

pub(super) const WORDS: usize = 96;
pub(super) const BYTES: usize = WORDS * 4;
pub(super) struct Diagnostics {
    pub(super) buffer: wgpu::Buffer,
    pub(super) enabled: bool,
}
impl Diagnostics {
    pub(super) fn new(device: &wgpu::Device, enabled: bool) -> Self {
        let bytes = if enabled { BYTES } else { 4 };
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("NAADF opt-in traversal counters"),
            contents: &vec![0; bytes],
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        });
        Self { buffer, enabled }
    }
    pub(super) fn bytes(&self) -> usize {
        if self.enabled {
            BYTES
        } else {
            4
        }
    }
    pub(super) fn read_words(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Option<[u32; WORDS]>, String> {
        if !self.enabled {
            return Ok(None);
        }
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("NAADF diagnostic-only blocking readback"),
            size: BYTES as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.buffer, 0, &readback, 0, BYTES as u64);
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let mapped = readback.slice(..).get_mapped_range();
        let values: &[u32] = bytemuck::cast_slice(&mapped);
        Ok(Some(
            values.try_into().expect("counter payload has fixed size"),
        ))
    }
    pub(super) fn read(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Option<String>, String> {
        let Some(values) = self.read_words(device, queue)? else {
            return Ok(None);
        };
        let json = format!(
            r#"{{"instrumented":true,"valid_for_timing":false,"scope":"cumulative since volume creation; includes numerical replays; shadow lane includes primary-hit and mesh-receiver queries","distance_units":"native cells rounded down per event; 0.0625 metres/cell","words_per_lane":32,"primary":{:?},"shadow":{:?},"beam":{:?}}}"#,
            &values[..32],
            &values[32..64],
            &values[64..]
        );
        Ok(Some(json))
    }
}
