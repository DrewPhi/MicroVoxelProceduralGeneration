//! Direct voxel traversal resources. One bounded arena holds immutable bricks;
//! the small top-level BVH changes only when the resident cover changes.
use microvoxel::render::{lod::MeshKey, ray::Brick};
use std::collections::BTreeMap;
use wgpu::util::DeviceExt;
const TOP: u32 = 262144; // Covers 8192 leaves plus transient balanced-patch headroom.
#[derive(Clone, Copy, Debug)]
pub struct Root {
    pub key: MeshKey,
    pub offset: u32,
    pub bounds: [[i32; 3]; 2],
}
pub struct Raycaster {
    pub pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    pub bind: wgpu::BindGroup,
    layout: wgpu::BindGroupLayout,
    free: BTreeMap<u32, u32>,
    capacity: u32,
    pub roots: Vec<Root>,
    pub allocated_bytes: usize,
}
pub fn shader_source() -> String {
    include_str!("ray.wgsl").replace(
        "// SHARED_SKY",
        include_str!("sky.wgsl")
            .split("fn sky_color")
            .nth(1)
            .unwrap()
            .split("@vertex")
            .next()
            .map(|s| format!("fn sky_color{s}"))
            .unwrap()
            .as_str(),
    )
}
fn make_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    frame: &wgpu::BindGroupLayout,
    layout: &wgpu::BindGroupLayout,
    source: String,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("hierarchical voxel ray caster"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ray terrain"),
        bind_group_layouts: &[frame, layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("voxel ray casting"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_ray"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_ray"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview: None,
        cache: None,
    })
}
impl Raycaster {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        frame: &wgpu::BindGroupLayout,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sparse voxel arena"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let source = shader_source();
        let pipeline = make_pipeline(device, format, frame, &layout, source);
        let capacity = TOP + 8;
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("voxel arena (grows on demand)"),
            contents: bytemuck::cast_slice(&vec![0u32; capacity as usize]),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        });
        let bind = Self::bind(device, &layout, &buffer);
        Self {
            pipeline,
            buffer,
            bind,
            layout,
            free: [(TOP, 8)].into(),
            capacity,
            roots: vec![],
            allocated_bytes: capacity as usize * 4,
        }
    }
    /// Opt-in benchmark variant, using the identical traversal and depth output.
    pub fn study_geometry_only(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        enabled: bool,
    ) {
        let source = shader_source();
        let source = if enabled {
            let prefix = source
                .split(" let relative=direction*closest")
                .next()
                .unwrap();
            format!("{prefix} let relative=direction*closest*camera.viewport.w;let clip=camera.vp*vec4<f32>(relative,1.);out.depth=clip.z/clip.w;out.color=vec4<f32>(0.5,0.5,0.5,1.);return out;}}")
        } else {
            source
        };
        let frame = self.pipeline.get_bind_group_layout(0);
        self.pipeline = make_pipeline(device, format, &frame, &self.layout, source);
    }
    fn bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxel arena"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        })
    }
    fn grow(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        need: u32,
        limit: usize,
    ) -> bool {
        let max_words = (limit.min(128 * 1048576) / 4) as u32;
        let wanted = self
            .capacity
            .saturating_add(need)
            .next_power_of_two()
            .min(max_words);
        if wanted <= self.capacity {
            return false;
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voxel arena"),
            size: wanted as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.buffer, 0, &buffer, 0, self.capacity as u64 * 4);
        queue.submit(Some(encoder.finish()));
        self.release(self.capacity, wanted - self.capacity);
        self.capacity = wanted;
        self.allocated_bytes = wanted as usize * 4;
        self.buffer = buffer;
        self.bind = Self::bind(device, &self.layout, &self.buffer);
        true
    }
    /// Shrink a free tail without relocating brick-internal absolute pointers.
    pub fn shrink(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        available: usize,
    ) -> Option<usize> {
        let (&start, &len) = self.free.iter().next_back()?;
        if start + len != self.capacity {
            return None;
        }
        let minimum = start.max(TOP + 8);
        let wanted = minimum.saturating_add(minimum / 8).next_multiple_of(64);
        if wanted >= self.capacity * 3 / 4 || wanted as usize * 4 > available {
            return None;
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voxel arena shrink"),
            size: wanted as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.buffer, 0, &buffer, 0, minimum as u64 * 4);
        queue.submit(Some(encoder.finish()));
        let old = self.allocated_bytes;
        self.free.remove(&start);
        if wanted > start {
            self.free.insert(start, wanted - start);
        }
        self.capacity = wanted;
        self.allocated_bytes = wanted as usize * 4;
        self.buffer = buffer;
        self.bind = Self::bind(device, &self.layout, &self.buffer);
        Some(old)
    }
    pub fn used_bytes(&self) -> usize {
        (self.capacity - self.free.values().sum::<u32>()) as usize * 4
    }
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        brick: &Brick,
        limit: usize,
    ) -> Option<(u32, u32)> {
        let count = (brick.words.len() as u32).max(8);
        let find = |free: &BTreeMap<u32, u32>| {
            free.iter()
                .find(|(_, len)| **len >= count)
                .map(|(p, n)| (*p, *n))
        };
        if find(&self.free).is_none() && !self.grow(device, queue, count, limit) {
            return None;
        }
        let (offset, available) = find(&self.free)?;
        self.free.remove(&offset);
        if available > count {
            self.free.insert(offset + count, available - count);
        }
        if !brick.words.is_empty() {
            queue.write_buffer(
                &self.buffer,
                offset as u64 * 4,
                bytemuck::cast_slice(&brick.relocated(offset)),
            );
        }
        Some((offset, count))
    }
    pub fn release(&mut self, mut offset: u32, mut count: u32) {
        if let Some((&p, &n)) = self.free.range(..offset).next_back() {
            if p + n == offset {
                self.free.remove(&p);
                offset = p;
                count += n;
            }
        }
        if let Some(n) = self.free.remove(&(offset + count)) {
            count += n;
        }
        self.free.insert(offset, count);
    }
    pub fn cover(&mut self, queue: &wgpu::Queue, roots: Vec<Root>) {
        if roots.len() == self.roots.len()
            && roots
                .iter()
                .zip(&self.roots)
                .all(|(a, b)| a.key == b.key && a.offset == b.offset)
        {
            return;
        }
        let mut sorted = roots.clone();
        let mut nodes = Vec::new();
        if !sorted.is_empty() {
            tree(&mut sorted, &mut nodes, 0);
            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&nodes));
        }
        self.roots = roots;
    }
}
fn tree(roots: &mut [Root], out: &mut Vec<[u32; 8]>, depth: usize) -> u32 {
    let index = out.len() as u32;
    out.push([0; 8]);
    let low: [i32; 3] =
        std::array::from_fn(|i| roots.iter().map(|r| r.bounds[0][i]).min().unwrap());
    let high: [i32; 3] =
        std::array::from_fn(|i| roots.iter().map(|r| r.bounds[1][i]).max().unwrap());
    let (a, b) = if roots.len() == 1 {
        (0x40000000 | roots[0].offset, 0)
    } else {
        let axis = if depth % 2 == 0 { 0 } else { 2 };
        let mid = roots.len() / 2;
        roots.select_nth_unstable_by_key(mid, |r| r.bounds[0][axis]);
        let (left, right) = roots.split_at_mut(mid);
        (tree(left, out, depth + 1), tree(right, out, depth + 1))
    };
    out[index as usize] = [
        low[0] as u32,
        low[1] as u32,
        low[2] as u32,
        a,
        high[0] as u32,
        high[1] as u32,
        high[2] as u32,
        b,
    ];
    index * 8
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn top_level_reservation_includes_transient_patch_headroom() {
        use microvoxel::render::lod::TileKey;
        let mut roots: Vec<_> = (0..8256)
            .map(|i| Root {
                key: MeshKey {
                    corner_levels: None,
                    tile: TileKey {
                        level: 13,
                        x: i % 128,
                        z: i / 128,
                    },
                    neighbors: [13; 4],
                    revision: 0,
                },
                offset: TOP,
                bounds: [
                    [(i % 128 * 32) as i32, 0, (i / 128 * 32) as i32],
                    [(i % 128 * 32 + 32) as i32, 16, (i / 128 * 32 + 32) as i32],
                ],
            })
            .collect();
        let mut nodes = Vec::new();
        tree(&mut roots, &mut nodes, 0);
        assert!(nodes.len() * 8 <= TOP as usize);
    }
}
