//! Moving surface baseline: immutable nested footprints, worker-generated canonical
//! samples. Independent level centers keep canonical parent anchors stationary.
//! Four immutable trim variants preserve nested joins; no native/volume handoff.
use frontier_world::FbmTerrain;
use wgpu::util::DeviceExt;
const N: i32 = 64;
const LEVELS: u32 = 7;
pub const HALF_EXTENT: f64 = 2048.;
pub const PAGE_BYTES: u64 = 7 * 129 * 129 * 80;
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GridVertex {
    position: [f32; 2],
    sample: u32,
    morph: f32,
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Sample {
    heights: [f32; 4],
    normal: [f32; 4],
    parent_normal: [f32; 4],
    color: [f32; 4],
    parent_color: [f32; 4],
}
pub struct TerrainData {
    vertices: &'static [GridVertex],
    indices: &'static [u32],
    samples: Vec<Sample>,
    centers: [[f64; 2]; LEVELS as usize],
    ranges: Vec<std::ops::Range<u32>>,
}
#[path = "terrain_stream.rs"]
mod streaming;
use streaming::SampleCache;
pub use streaming::{TerrainStreamer, TerrainUpdate};
impl TerrainData {
    #[cfg(test)]
    pub fn new(source: &FbmTerrain) -> Self {
        Self::generate(source, [0., 0.], &mut SampleCache::new(), || false).unwrap()
    }
    fn generate(
        source: &FbmTerrain,
        center: [f64; 2],
        cache: &mut SampleCache,
        cancelled: impl Fn() -> bool,
    ) -> Option<Self> {
        type Topology = (Vec<GridVertex>, Vec<u32>, Vec<Vec<std::ops::Range<u32>>>);
        static TOPOLOGY: std::sync::OnceLock<Topology> = std::sync::OnceLock::new();
        let (vertices, indices, variants) = TOPOLOGY.get_or_init(|| {
            let mut vertices = Vec::new();
            let mut indices = Vec::new();
            let mut variants = Vec::new();
            for level in 0..LEVELS {
                let step = 0.5 * (1u32 << level) as f64;
                let base = vertices.len() as u32;
                for z in -N..=N {
                    for x in -N..=N {
                        let (mut gx, mut gz) = (x, z);
                        // Global even parent anchors: center is a multiple of 2*step.
                        if x.abs() == N && z % 2 != 0 {
                            gz -= 1;
                        }
                        if z.abs() == N && x % 2 != 0 {
                            gx -= 1;
                        }
                        vertices.push(GridVertex {
                            position: [gx as f32 * step as f32, gz as f32 * step as f32],
                            sample: vertices.len() as u32,
                            morph: if level + 1 == LEVELS {
                                0.
                            } else {
                                ((x.abs().max(z.abs()) as f32 - 48.) / 16.).clamp(0., 1.)
                            },
                        });
                    }
                }
                let row = (2 * N + 1) as u32;
                let mut ranges = Vec::new();
                for parity in 0..4 {
                    let start = indices.len() as u32;
                    let dx = parity % 2;
                    let dz = parity / 2;
                    for z in -N..N {
                        for x in -N..N {
                            if level > 0
                                && (-N / 2 + dx..N / 2 + dx).contains(&x)
                                && (-N / 2 + dz..N / 2 + dz).contains(&z)
                            {
                                continue;
                            }
                            let a = base + (z + N) as u32 * row + (x + N) as u32;
                            if x == N - 1 && z == N - 1 {
                                indices.extend_from_slice(&[
                                    a,
                                    a + row,
                                    a + row + 1,
                                    a,
                                    a + row + 1,
                                    a + 1,
                                ]);
                            } else {
                                indices.extend_from_slice(&[
                                    a,
                                    a + row,
                                    a + 1,
                                    a + 1,
                                    a + row,
                                    a + row + 1,
                                ]);
                            }
                        }
                    }
                    ranges.push(start..indices.len() as u32);
                }
                variants.push(ranges);
            }
            (vertices, indices, variants)
        });
        let centers = std::array::from_fn(|level| {
            let snap = (1u32 << level) as f64;
            center.map(|v| (v / snap).floor() * snap)
        });
        let ranges = (0..LEVELS as usize)
            .map(|level| {
                let parity = if level == 0 {
                    0
                } else {
                    let step = 0.5 * (1u32 << level) as f64;
                    let dx = ((centers[level - 1][0] - centers[level][0]) / step).round() as usize;
                    let dz = ((centers[level - 1][1] - centers[level][1]) / step).round() as usize;
                    dx + 2 * dz
                };
                variants[level][parity].clone()
            })
            .collect();
        let count = ((N * 2 + 1).pow(2)) as usize;
        let mut samples = Vec::with_capacity(vertices.len());
        for level in 0..LEVELS as usize {
            if cancelled() {
                return None;
            }
            if let Some((old_center, page)) = &cache.pages[level] {
                if *old_center == centers[level] {
                    samples.extend_from_slice(page);
                    cache.reused_samples += count as u64;
                    continue;
                }
            }
            let previous = cache.pages[level].clone();
            let mut page = Vec::with_capacity(count);
            for (i, vertex) in vertices[level * count..(level + 1) * count]
                .iter()
                .enumerate()
            {
                if i % (N * 2 + 1) as usize == 0 && cancelled() {
                    return None;
                }
                let step = 0.5 * (1u32 << level) as f64;
                let x = centers[level][0] + vertex.position[0] as f64;
                let z = centers[level][1] + vertex.position[1] as f64;
                // Whole Sample content is keyed by canonical position and level,
                // not by the camera. Copy overlapping rows/columns directly.
                // A nominal point on the old collapsed outer edge may represent
                // its even neighbor; verify the physical point before reuse.
                let reused = previous.as_ref().and_then(|(old_center, old_page)| {
                    let ox = ((x - old_center[0]) / step) as i32;
                    let oz = ((z - old_center[1]) / step) as i32;
                    if !(-N..=N).contains(&ox) || !(-N..=N).contains(&oz) {
                        return None;
                    }
                    let old_index = ((oz + N) * (2 * N + 1) + ox + N) as usize;
                    let old_vertex = &vertices[level * count + old_index];
                    if old_vertex.position[0] as f64 + old_center[0] != x
                        || old_vertex.position[1] as f64 + old_center[1] != z
                    {
                        return None;
                    }
                    Some(old_page[old_index])
                });
                if let Some(sample) = reused {
                    cache.reused_samples += 1;
                    page.push(sample);
                } else {
                    page.push(cache.sample(source, x, z, step, [0., 0.]));
                }
            }
            cache.generated_pages += 1;
            samples.extend_from_slice(&page);
            cache.pages[level] = Some((centers[level], page.into()));
        }
        Some(Self {
            vertices,
            indices,
            samples,
            centers,
            ranges,
        })
    }
}

pub struct TerrainGpu {
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    samples: wgpu::Buffer,
    center: [f64; 2],
    centers: [[f64; 2]; LEVELS as usize],
    ranges: Vec<std::ops::Range<u32>>,
    pub index_count: u32,
}
impl TerrainGpu {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, data: &TerrainData) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain camera"),
            size: 64 + 16 * LEVELS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let samples = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("canonical height pages"),
            contents: bytemuck::cast_slice(&data.samples),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: samples.as_entire_binding(),
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain"),
            source: wgpu::ShaderSource::Wgsl(include_str!("terrain.wgsl").into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let attrs = wgpu::vertex_attr_array![0=>Float32x2,1=>Uint32,2=>Float32];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fixed ring terrain"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
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
        });
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("immutable ring footprint"),
            contents: bytemuck::cast_slice(&data.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("immutable ring indices"),
            contents: bytemuck::cast_slice(&data.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            pipeline,
            bind,
            uniform,
            vertices,
            indices,
            samples,
            center: data.centers[0],
            centers: data.centers,
            ranges: data.ranges.clone(),
            index_count: data.ranges.iter().map(|r| r.end - r.start).sum(),
        }
    }
    pub fn center(&self) -> [f64; 2] {
        self.center
    }
    pub fn upload(&mut self, queue: &wgpu::Queue, update: &TerrainUpdate) -> u64 {
        let mut bytes = 0;
        let count = ((N * 2 + 1).pow(2)) as usize;
        for level in 0..LEVELS as usize {
            if self.centers[level] != update.data.centers[level] {
                bytes += (count * std::mem::size_of::<Sample>()) as u64;
                queue.write_buffer(
                    &self.samples,
                    (level * count * std::mem::size_of::<Sample>()) as u64,
                    bytemuck::cast_slice(&update.data.samples[level * count..(level + 1) * count]),
                );
            }
        }
        self.center = update.center;
        self.centers = update.data.centers;
        self.ranges = update.data.ranges.clone();
        bytes
    }
    pub fn draw<'a>(
        &'a self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        matrix: glam::Mat4,
    ) {
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::cast_slice(&matrix.to_cols_array()),
        );
        let offsets: [[f32; 4]; LEVELS as usize] = std::array::from_fn(|i| {
            [
                (self.centers[i][0] - self.center[0]) as f32,
                (self.centers[i][1] - self.center[1]) as f32,
                0.,
                0.,
            ]
        });
        queue.write_buffer(&self.uniform, 64, bytemuck::cast_slice(&offsets));
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        for range in &self.ranges {
            pass.draw_indexed(range.clone(), 0, 0..1);
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn nested_boundaries_are_identical_and_triangles_cover_without_reversal() {
        for (seed, scale) in [(7, 1300.), (91, 1800.)] {
            let source = frontier_world::FbmTerrain::new(
                seed,
                frontier_world::FbmParams {
                    octaves: 1,
                    horizontal_scale: scale,
                    ..Default::default()
                },
            )
            .unwrap();
            let data = super::TerrainData::new(&source);
            let count = ((super::N * 2 + 1).pow(2)) as usize;
            for level in 0..super::LEVELS as usize - 1 {
                let fine = &data.vertices[level * count..(level + 1) * count];
                let coarse = &data.vertices[(level + 1) * count..(level + 2) * count];
                for v in fine.iter().filter(|v| v.morph == 1.) {
                    let c = coarse.iter().find(|c| c.position == v.position).unwrap();
                    let a = data.samples[v.sample as usize];
                    let b = data.samples[c.sample as usize];
                    assert_eq!(a.heights[1], b.heights[0]);
                    assert_eq!(a.parent_color, b.color);
                    assert_eq!(a.parent_normal, b.normal);
                }
            }
            let mut area = 0f64;
            let mut edges = std::collections::HashMap::new();
            for t in data
                .ranges
                .iter()
                .flat_map(|r| data.indices[r.start as usize..r.end as usize].chunks_exact(3))
            {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| data.vertices[i as usize].position);
                let signed = (b[1] - a[1]) as f64 * (c[0] - a[0]) as f64
                    - (b[0] - a[0]) as f64 * (c[1] - a[1]) as f64;
                assert!(signed >= 0., "triangle reverses its upward normal");
                area += signed * 0.5;
                if signed > 0. {
                    let points = [a, b, c].map(|p| ((p[0] * 2.) as i64, (p[1] * 2.) as i64));
                    for (a, b) in [
                        (points[0], points[1]),
                        (points[1], points[2]),
                        (points[2], points[0]),
                    ] {
                        let key = if a < b { (a, b) } else { (b, a) };
                        *edges.entry(key).or_insert(0) += 1;
                    }
                }
            }
            assert_eq!(area, (super::HALF_EXTENT * 2.).powi(2));
            for ((a, b), count) in edges {
                let edge = (super::HALF_EXTENT * 2.) as i64;
                let outer = (a.0.abs() == edge && a.0 == b.0) || (a.1.abs() == edge && a.1 == b.1);
                assert_eq!(
                    count,
                    if outer { 1 } else { 2 },
                    "unexpected open or overlapping edge {a:?} {b:?}"
                );
            }
        }
    }
    #[test]
    fn shader_valid() {
        let m = naga::front::wgsl::parse_str(include_str!("terrain.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&m)
        .unwrap();
    }
}

#[cfg(test)]
#[path = "terrain_tests.rs"]
mod moving_tests;
