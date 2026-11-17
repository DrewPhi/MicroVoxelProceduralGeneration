//! Complete, immutable native occupancy snapshots independent of static roots.
use super::*;
use frontier_world::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) fn empty(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("inactive native water occupancy"),
        contents: &[0; 48],
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    })
}
pub(super) fn binding_id() -> u64 {
    static BINDING: AtomicU64 = AtomicU64::new(1 << 63);
    BINDING.fetch_add(1, Ordering::Relaxed)
}
impl Experiment {
    /// Publish a complete x-fast bounded occupancy snapshot. 0 inherits static
    /// terrain, 1 suppresses source Water, 2 adds Water. Solids retain authority.
    /// `generation` must change whenever the contents change. Empty dimensions
    /// disable the overlay. Each publication owns a new immutable GPU buffer.
    pub(crate) fn set_dynamic_water(
        &mut self,
        gpu: &Gpu,
        min: Cell,
        size: [u32; 3],
        generation: u64,
        states: &[u32],
    ) -> Result<(), String> {
        let count = size
            .into_iter()
            .try_fold(1usize, |n, s| n.checked_mul(s as usize))
            .ok_or("native water snapshot dimensions overflow")?;
        if count != states.len() {
            return Err("native water snapshot requires complete ternary occupancy".into());
        }
        let bytes = count
            .checked_add(8)
            .and_then(|n| n.checked_mul(4))
            .ok_or("native water snapshot bytes overflow")?;
        if bytes > gpu.device.limits().max_storage_buffer_binding_size as usize {
            return Err("native water snapshot exceeds GPU storage binding limit".into());
        }
        // The private caller owns generation identity. Retained snapshots need
        // only constant-time shape/key checks, with no occupancy scan per frame.
        if self
            .active
            .iter()
            .chain(self.pending.iter().map(|p| &p.volume))
            .all(|volume| {
                let origin = volume.bounds[0].map(|v| (v * 16.).round() as i64);
                let relative = Cell {
                    x: min.x - origin[0],
                    y: min.y - origin[1],
                    z: min.z - origin[2],
                };
                volume.dynamic_water_key == Some((relative, size, generation))
            })
        {
            return Ok(());
        }
        if states.iter().any(|&s| s > 2) {
            return Err("native water snapshot requires complete ternary occupancy".into());
        }
        for volume in self
            .active
            .iter_mut()
            .chain(self.pending.iter_mut().map(|p| &mut p.volume))
        {
            let origin = volume.bounds[0].map(|v| (v * 16.).round() as i64);
            let relative = Cell {
                x: min.x - origin[0],
                y: min.y - origin[1],
                z: min.z - origin[2],
            };
            let key = (relative, size, generation);
            if volume.dynamic_water_key == Some(key) {
                continue;
            }
            let components = [relative.x, relative.y, relative.z];
            if components.iter().any(|&c| i32::try_from(c).is_err()) {
                return Err("native water origin exceeds relative integer range".into());
            }
            let mut words = Vec::with_capacity(count + 8);
            words.extend(components.map(|v| v as i32 as u32));
            words.push(u32::from(states.contains(&2)));
            words.extend(size);
            words.push(0);
            words.extend_from_slice(states);
            if states.is_empty() {
                words.extend([0; 4]);
            }
            volume.resources.dynamic_water =
                gpu.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("complete native water occupancy snapshot"),
                        contents: bytemuck::cast_slice(&words),
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    });
            volume.bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("NAADF with dynamic native water"),
                layout: &volume.pipeline.get_bind_group_layout(0),
                entries: &volume.resources.entries(0),
            });
            volume.dynamic_water_key = Some(key);
            volume.dynamic_water_binding = binding_id();
        }
        Ok(())
    }
}

/// Rebase the retained immutable snapshot during the same submission that
/// publishes a shifted static window. This also runs while newer solids/water
/// are withheld, so an old valid snapshot never moves with the native window.
pub(super) fn rebase_relative(relative: Cell, old: [f64; 3], new: [f64; 3]) -> Cell {
    let delta = std::array::from_fn::<_, 3, _>(|a| ((old[a] - new[a]) * 16.).round() as i64);
    Cell {
        x: relative.x + delta[0],
        y: relative.y + delta[1],
        z: relative.z + delta[2],
    }
}
pub(super) fn retarget_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    previous: &wgpu::Buffer,
    relative: Cell,
) -> wgpu::Buffer {
    let next = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("retargeted immutable native water snapshot"),
        size: previous.size(),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    // Preserve wet flag, shape and every occupancy byte; only the origin moves.
    encoder.copy_buffer_to_buffer(previous, 12, &next, 12, previous.size() - 12);
    let origin = [relative.x, relative.y, relative.z]
        .map(|v| i32::try_from(v).expect("native water relative origin range"));
    queue.write_buffer(&next, 0, bytemuck::cast_slice(&origin));
    next
}
impl Volume {
    pub(super) fn retarget_dynamic_water(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        new_origin: [f64; 3],
    ) {
        let Some((relative, size, generation)) = self.dynamic_water_key else {
            return;
        };
        let relative_next = rebase_relative(relative, self.bounds[0], new_origin);
        if relative_next == relative {
            return;
        }
        self.resources.dynamic_water = retarget_buffer(
            &gpu.device,
            &gpu.queue,
            encoder,
            &self.resources.dynamic_water,
            relative_next,
        );
        self.dynamic_water_key = Some((relative_next, size, generation));
        self.dynamic_water_binding = binding_id();
        self.bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("NAADF retargeted native water"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &self.resources.entries(0),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_snapshot_origin_rebases_without_changing_global_cells() {
        let relative = Cell { x: 3, y: -5, z: 9 };
        let old = [-8., 3., -2.];
        let new = [-7., 1., -4.];
        let rebased = rebase_relative(relative, old, new);
        assert_eq!(
            rebased,
            Cell {
                x: -13,
                y: 27,
                z: 41
            }
        );
        assert_eq!(rebase_relative(rebased, new, old), relative);
        for (r, next, a) in [
            (relative.x, rebased.x, 0),
            (relative.y, rebased.y, 1),
            (relative.z, rebased.z, 2),
        ] {
            assert_eq!(r + (old[a] * 16.) as i64, next + (new[a] * 16.) as i64);
        }
    }
}
