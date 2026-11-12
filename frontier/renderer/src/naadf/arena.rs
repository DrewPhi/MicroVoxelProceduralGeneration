//! Grow backing buffers without regenerating voxel contents or changing addresses.
use super::{memory_headroom, scatter, stream::Change, upload, Gpu, Volume};
const MIB: usize = 1048576;
impl Volume {
    pub(super) fn resident_radius(&self) -> u32 {
        ((self.bounds[1][0] - self.bounds[0][0]) * 0.5) as u32
    }
    /// CPU slot reservations may exceed current GPU allocation. Before a write
    /// can upload, ensure_arenas creates/copies enough actual backing storage.
    pub(super) fn arena_limits(&self, gpu: &Gpu) -> [usize; 3] {
        let limits = gpu.device.limits();
        let max = (limits
            .max_buffer_size
            .min(limits.max_storage_buffer_binding_size as u64) as usize)
            & !255;
        [
            max,
            max.min((1usize << 30).saturating_mul(self.leaf_encoding.bits() as usize / 8)),
            max,
        ]
    }
    pub(super) fn ensure_arenas(
        &mut self,
        gpu: &Gpu,
        change: &Change,
        budget: usize,
    ) -> Result<(), String> {
        let limits = self.arena_limits(gpu);
        let mut required = self.capacities;
        for w in &change.writes {
            let end = w
                .word
                .checked_add(w.data.len())
                .and_then(|n| n.checked_mul(4))
                .ok_or("NAADF arena address overflow")?;
            required[w.kind as usize] = required[w.kind as usize].max(end);
        }
        let mut next = self.capacities;
        for i in 0..3 {
            if required[i] > limits[i] {
                return Err("NAADF arena exceeds device/address limit".into());
            }
            if required[i] > next[i] {
                next[i] = required[i]
                    .max(next[i].saturating_add(next[i] / 2))
                    .max(4 * MIB)
                    .div_ceil(256)
                    .saturating_mul(256)
                    .min(limits[i]);
            }
        }
        if next == self.capacities {
            return Ok(());
        }
        let headroom = memory_headroom(crate::settings::MemorySnapshot::detect());
        let fits = |plan: [usize; 3]| {
            let allocated = plan
                .iter()
                .enumerate()
                .filter(|&(i, n)| *n != self.capacities[i])
                .map(|(_, n)| *n)
                .sum::<usize>();
            self.root_bytes().saturating_add(plan.iter().sum::<usize>()) <= budget
                && headroom
                    .is_none_or(|h| allocated + upload::scatter_allowance().unwrap_or(0) <= h)
        };
        if !fits(next) {
            // Optional growth slack must not reject a payload that actually fits.
            for i in 0..3 {
                if required[i] > self.capacities[i] {
                    next[i] = required[i].div_ceil(256).saturating_mul(256).min(limits[i]);
                }
            }
            if !fits(next) {
                return Err(format!("NAADF arena growth paused: {} MiB payload budget, {} MiB live allocation headroom; rebuild with a smaller radius",
                    budget / MIB, headroom.map_or(0, |h| h / MIB)));
            }
        }
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("NAADF preserve arena contents while growing"),
            });
        for (i, buffer) in [&mut self.blocks, &mut self.voxels, &mut self.palette]
            .into_iter()
            .enumerate()
        {
            if next[i] == self.capacities[i] {
                continue;
            }
            let new = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("NAADF growing arena"),
                size: next[i] as u64,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            encoder.copy_buffer_to_buffer(buffer, 0, &new, 0, buffer.size());
            self.arena_copy_bytes += buffer.size();
            *buffer = new;
        }
        gpu.queue.submit(Some(encoder.finish()));
        self.capacities = next;
        self.arena_resizes += 1;
        self.resources.blocks = self.blocks.clone();
        self.resources.voxels = self.voxels.clone();
        self.resources.palette = self.palette.clone();
        self.bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("NAADF resized arenas"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &self.resources.entries(0),
        });
        self.scatter = upload::scatter_allowance().map(|allowance| {
            let mut s = scatter::Scatter::new(
                &gpu.device,
                [&self.blocks, &self.voxels, &self.palette],
                allowance,
            );
            s.stages = gpu.naadf_gpu_stages.clone();
            s
        });
        Ok(())
    }
}
