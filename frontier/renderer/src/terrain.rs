//! Moving surface baseline: immutable nested footprints, worker-generated canonical
//! samples. Independent level centers keep canonical parent anchors stationary.
//! Four immutable trim variants preserve nested joins; no native/volume handoff.
use frontier_world::FbmTerrain;
use wgpu::util::DeviceExt;
const N: i32 = 64;
pub const MIN_LEVELS: u32 = 7;
pub const MAX_LEVELS: u32 = 11;
#[cfg(test)]
const LEVELS: u32 = MIN_LEVELS;
/// Outer square radius; callers validate `MIN_LEVELS..=MAX_LEVELS`.
pub const fn half_extent(levels: u32) -> f64 {
    32. * (1u32 << (levels - 1)) as f64
}
pub const fn page_bytes(levels: u32) -> u64 {
    levels as u64 * 129 * 129 * 80
}
#[cfg(test)]
pub const HALF_EXTENT: f64 = half_extent(MIN_LEVELS);
#[cfg(test)]
pub const PAGE_BYTES: u64 = page_bytes(MIN_LEVELS);
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GridVertex {
    position: [f32; 2],
    sample: u32,
    morph: f32,
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Sample {
    pub(crate) heights: [f32; 4],
    pub(crate) normal: [f32; 4],
    pub(crate) parent_normal: [f32; 4],
    pub(crate) color: [f32; 4],
    pub(crate) parent_color: [f32; 4],
}
pub struct TerrainData {
    vertices: &'static [GridVertex],
    indices: &'static [u32],
    pub(crate) samples: Vec<Sample>,
    pub(crate) centers: [[f64; 2]; MAX_LEVELS as usize],
    pub(crate) level_count: u32,
    ranges: Vec<std::ops::Range<u32>>,
}
#[path = "terrain_stream.rs"]
mod streaming;
use streaming::SampleCache;
pub use streaming::{TerrainStreamer, TerrainUpdate};
impl TerrainData {
    #[cfg(test)]
    pub(crate) fn at_center(source: &FbmTerrain, center: [f64; 2]) -> Self {
        Self::generate(source, center, &mut SampleCache::new(), || false).unwrap()
    }
    /// Read the exact unmorphed level-one endpoint used by the GPU page.
    /// The native collar stays inside this level's unmorphed inner annulus.
    pub(crate) fn collar_sample(&self, x: f64, z: f64) -> Option<(f64, [f32; 3], [f32; 3])> {
        let center = self.centers[1];
        let gx = x - center[0];
        let gz = z - center[1];
        if gx.fract() != 0. || gz.fract() != 0. || gx.abs() > 64. || gz.abs() > 64. {
            return None;
        }
        let count = ((N * 2 + 1).pow(2)) as usize;
        let i = count + ((gz as i32 + N) * (2 * N + 1) + gx as i32 + N) as usize;
        let s = self.samples[i];
        Some((
            s.heights[0] as f64,
            s.normal[..3].try_into().unwrap(),
            s.color[..3].try_into().unwrap(),
        ))
    }
    pub(crate) fn collar_center(&self) -> [f64; 2] {
        self.centers[1]
    }
    #[cfg(test)]
    pub fn new(source: &FbmTerrain) -> Self {
        Self::generate(source, [0., 0.], &mut SampleCache::new(), || false).unwrap()
    }
    #[cfg(test)]
    fn generate(
        source: &FbmTerrain,
        center: [f64; 2],
        cache: &mut SampleCache,
        cancelled: impl Fn() -> bool,
    ) -> Option<Self> {
        Self::generate_levels(source, center, cache, LEVELS, cancelled)
    }
    fn generate_levels(
        source: &FbmTerrain,
        center: [f64; 2],
        cache: &mut SampleCache,
        level_count: u32,
        cancelled: impl Fn() -> bool,
    ) -> Option<Self> {
        assert!((MIN_LEVELS..=MAX_LEVELS).contains(&level_count));
        type Topology = (Vec<GridVertex>, Vec<u32>, Vec<Vec<std::ops::Range<u32>>>);
        // Bounded shared topology: four trim variants for at most eleven levels.
        // Active pages and draw ranges allocate only the requested level count.
        static TOPOLOGY: std::sync::OnceLock<Topology> = std::sync::OnceLock::new();
        let (vertices, indices, variants) = TOPOLOGY.get_or_init(|| {
            let mut vertices = Vec::new();
            let mut indices = Vec::new();
            let mut variants = Vec::new();
            for level in 0..MAX_LEVELS {
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
                            morph: if level + 1 == MAX_LEVELS {
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
        let ranges = (0..level_count as usize)
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
        let mut samples = Vec::with_capacity(count * level_count as usize);
        for level in 0..level_count as usize {
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
            level_count,
            centers,
            ranges,
        })
    }
}

/// Carry camera translation immediately; only relax the residual introduced by
/// newly available page bounds. World-space anchors survive origin publication.
fn advance_morph_anchors(
    anchors: [[f64; 2]; MAX_LEVELS as usize],
    previous_eye: [f64; 2],
    eye: [f64; 2],
    centers: [[f64; 2]; MAX_LEVELS as usize],
    dt: f64,
) -> [[f64; 2]; MAX_LEVELS as usize] {
    let dt = dt.clamp(0., 0.05);
    let alpha = 1. - (-dt / 0.15).exp();
    std::array::from_fn(|level| {
        let guard = 4. * (1u32 << level) as f64;
        std::array::from_fn(|axis| {
            let min = centers[level][axis] - guard;
            let max = centers[level][axis] + guard;
            let anchor = (anchors[level][axis] + eye[axis] - previous_eye[axis]).clamp(min, max);
            let target = eye[axis].clamp(min, max);
            anchor + ((target - anchor) * alpha).clamp(-32. * dt, 32. * dt)
        })
    })
}
struct MorphState {
    anchors: [[f64; 2]; MAX_LEVELS as usize],
    eye: [f64; 2],
    drawn: Option<std::time::Instant>,
}
pub struct TerrainGpu {
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    samples: wgpu::Buffer,
    center: [f64; 2],
    centers: [[f64; 2]; MAX_LEVELS as usize],
    level_count: u32,
    ranges: Vec<std::ops::Range<u32>>,
    pub index_count: u32,
    hybrid_cutout: Option<[f64; 2]>,
    volume_owner: bool,
    camera_position: std::cell::Cell<[f64; 3]>,
    voxel_filter: std::cell::Cell<bool>,
    morph: std::cell::RefCell<MorphState>,
}
impl TerrainGpu {
    pub fn appearance_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        })
    }
    pub fn new_with_samples(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        data: &TerrainData,
        sample_count: u32,
    ) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain camera"),
            size: 64 + 32 * MAX_LEVELS as u64 + 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let samples = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("canonical height pages"),
            contents: bytemuck::cast_slice(&data.samples),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let layout = Self::appearance_layout(device);
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
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
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
            level_count: data.level_count,
            center: data.centers[0],
            centers: data.centers,
            ranges: data.ranges.clone(),
            index_count: data.ranges.iter().map(|r| r.end - r.start).sum(),
            hybrid_cutout: None,
            volume_owner: false,
            camera_position: std::cell::Cell::new([data.centers[0][0], 0., data.centers[0][1]]),
            voxel_filter: std::cell::Cell::new(true),
            morph: std::cell::RefCell::new(MorphState {
                anchors: data.centers,
                eye: data.centers[0],
                drawn: None,
            }),
        }
    }
    pub fn set_hybrid_cutout(&mut self, center: Option<[f64; 2]>) {
        self.hybrid_cutout = center;
    }
    pub fn set_volume_owner(&mut self, enabled: bool) {
        self.volume_owner = enabled;
    }
    pub fn set_voxel_filter(&self, enabled: bool) {
        self.voxel_filter.set(enabled);
    }
    pub fn set_view_position(&self, position: [f64; 3]) {
        self.camera_position.set(position);
    }
    pub fn appearance_bind(&self) -> &wgpu::BindGroup {
        &self.bind
    }
    pub fn center(&self) -> [f64; 2] {
        self.center
    }
    pub fn upload(&mut self, queue: &wgpu::Queue, update: &TerrainUpdate) -> u64 {
        let mut bytes = 0;
        let count = ((N * 2 + 1).pow(2)) as usize;
        assert_eq!(self.level_count, update.data.level_count);
        for level in 0..self.level_count as usize {
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
        let offsets: [[f32; 4]; MAX_LEVELS as usize] = std::array::from_fn(|i| {
            [
                (self.centers[i][0] - self.center[0]) as f32,
                (self.centers[i][1] - self.center[1]) as f32,
                // The active outer ring has no displayed parent to morph toward.
                if i + 1 < self.level_count as usize {
                    1.
                } else {
                    0.
                },
                0.,
            ]
        });
        queue.write_buffer(&self.uniform, 64, bytemuck::cast_slice(&offsets));
        let cutout = self.hybrid_cutout.map_or([0.; 4], |c| {
            [
                (c[0] - self.center[0]) as f32,
                (c[1] - self.center[1]) as f32,
                34.,
                1.,
            ]
        });
        let camera_position = self.camera_position.get();
        let morph_centers = {
            let mut state = self.morph.borrow_mut();
            let now = std::time::Instant::now();
            let position = [camera_position[0], camera_position[2]];
            state.anchors = if let Some(previous) = state.drawn {
                advance_morph_anchors(
                    state.anchors,
                    state.eye,
                    position,
                    self.centers,
                    now.duration_since(previous).as_secs_f64(),
                )
            } else {
                // Cold/headless captures start at their exact camera target.
                std::array::from_fn(|level| {
                    let guard = 4. * (1u32 << level) as f64;
                    std::array::from_fn(|axis| {
                        position[axis].clamp(
                            self.centers[level][axis] - guard,
                            self.centers[level][axis] + guard,
                        )
                    })
                })
            };
            state.eye = position;
            state.drawn = Some(now);
            state.anchors.map(|p| {
                [
                    (p[0] - self.center[0]) as f32,
                    (p[1] - self.center[1]) as f32,
                    0.,
                    0.,
                ]
            })
        };
        let eye = [
            (camera_position[0] - self.center[0]) as f32,
            (camera_position[2] - self.center[1]) as f32,
            camera_position[1] as f32,
            if self.voxel_filter.get() { 1. } else { 0. },
        ];
        queue.write_buffer(
            &self.uniform,
            64 + 16 * MAX_LEVELS as u64,
            bytemuck::cast_slice(&[
                cutout,
                eye,
                [
                    (-self.center[0]) as f32,
                    (-self.center[1]) as f32,
                    8.,
                    if self.volume_owner { 1. } else { 0. },
                ],
            ]),
        );
        queue.write_buffer(
            &self.uniform,
            64 + 16 * MAX_LEVELS as u64 + 48,
            bytemuck::cast_slice(&morph_centers),
        );
        let paint_origin = [
            (self.center[0] * 16.) as i64 as u32,
            0u32,
            (self.center[1] * 16.) as i64 as u32,
            0u32,
        ];
        queue.write_buffer(
            &self.uniform,
            64 + 32 * MAX_LEVELS as u64 + 48,
            bytemuck::cast_slice(&paint_origin),
        );
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        for range in self
            .ranges
            .iter()
            .skip(usize::from(self.hybrid_cutout.is_some()))
        {
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
    fn morph_weight(point: [f64; 2], eye: [f64; 2], center: [f64; 2], step: f64) -> f64 {
        let eye = std::array::from_fn::<_, 2, _>(|i| {
            eye[i].clamp(center[i] - 8. * step, center[i] + 8. * step)
        });
        let distance = ((point[0] - eye[0]).abs().max((point[1] - eye[1]).abs())) / step;
        let t = ((distance - 42.) / 12.).clamp(0., 1.);
        t * t * (3. - 2. * t)
    }
    #[test]
    fn temporal_morph_retains_publication_anchor_and_preserves_all_joins() {
        use super::*;
        let before = [[0.; 2]; MAX_LEVELS as usize];
        let after = std::array::from_fn(|level| {
            let snap = (1u32 << level) as f64;
            [(8. / snap).floor() * snap, 0.]
        });
        let eye: [f64; 2] = [24., -2.];
        let initial = std::array::from_fn(|level| {
            let guard = 4. * (1u32 << level) as f64;
            eye.map(|p| p.clamp(-guard, guard))
        });
        let mut anchors = advance_morph_anchors(initial, eye, eye, after, 0.);
        assert_eq!(initial, anchors, "publication alone must not move the eye");
        for _ in 0..240 {
            let next = advance_morph_anchors(anchors, eye, eye, after, 1. / 60.);
            for level in 0..MAX_LEVELS as usize {
                let step = 0.5 * (1u32 << level) as f64;
                for axis in 0..2 {
                    assert!((next[level][axis] - after[level][axis]).abs() <= 8. * step);
                    assert!((next[level][axis] - anchors[level][axis]).abs() <= 32. / 60. + 1e-12);
                }
                if level + 1 < MAX_LEVELS as usize {
                    for x in [-64., 0., 64.] {
                        for z in [-64., 0., 64.] {
                            if x == 0. && z == 0. {
                                continue;
                            }
                            let p = [after[level][0] + x * step, after[level][1] + z * step];
                            assert_eq!(morph_weight(p, next[level], after[level], step), 1.);
                            assert_eq!(
                                morph_weight(p, next[level + 1], after[level + 1], step * 2.),
                                0.
                            );
                        }
                    }
                }
            }
            anchors = next;
        }
        for level in 0..MAX_LEVELS as usize {
            let guard = 4. * (1u32 << level) as f64;
            for axis in 0..2 {
                let target =
                    eye[axis].clamp(after[level][axis] - guard, after[level][axis] + guard);
                assert!((anchors[level][axis] - target).abs() < 1e-8);
            }
        }
        assert_eq!(
            advance_morph_anchors(before, [0., 0.], [1., -1.], before, 1. / 60.),
            [[1., -1.]; MAX_LEVELS as usize],
            "ordinary camera translation must not acquire smoothing lag"
        );
        let stalled = advance_morph_anchors(initial, eye, eye, after, 30.);
        assert!((stalled[1][0] - initial[1][0]).abs() <= 1.6);
        let destination = [[10000., -10000.]; MAX_LEVELS as usize];
        let teleported = advance_morph_anchors(initial, eye, eye, destination, 0.);
        for level in 0..MAX_LEVELS as usize {
            let guard = 4. * (1u32 << level) as f64;
            for axis in 0..2 {
                assert!((teleported[level][axis] - destination[level][axis]).abs() <= guard);
            }
        }
    }
    #[test]
    fn continuous_camera_morph_preserves_joins_and_collar_during_grid_snaps() {
        use super::*;
        let source = frontier_world::FbmTerrain::new(
            7,
            frontier_world::FbmParams {
                octaves: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let mut max_delta = 0f64;
        for (before, after, eye) in [
            ([0., 0.], [8., 8.], [8., 8.]),
            ([-8., -8.], [0., 0.], [0., 0.]),
            ([0., 0.], [-8., -8.], [-0.000001, -0.000001]),
        ] {
            let a = TerrainData::at_center(&source, before);
            let b = TerrainData::at_center(&source, after);
            let count = ((N * 2 + 1).pow(2)) as usize;
            for level in 1..LEVELS as usize - 1 {
                let step = 0.5 * (1u32 << level) as f64;
                for v in &a.vertices[level * count..(level + 1) * count] {
                    let point = [
                        v.position[0] as f64 + a.centers[level][0],
                        v.position[1] as f64 + a.centers[level][1],
                    ];
                    if v.position.iter().any(|v| v.abs() >= 60. * step as f32) {
                        continue;
                    }
                    let q = [
                        (point[0] - b.centers[level][0]) / step,
                        (point[1] - b.centers[level][1]) / step,
                    ];
                    if q.iter().any(|q| q.abs() >= 60.) {
                        continue;
                    }
                    let bi = level * count
                        + ((q[1] as i32 + N) * (2 * N + 1) + q[0] as i32 + N) as usize;
                    let sa = a.samples[v.sample as usize];
                    let sb = b.samples[bi];
                    let wa = morph_weight(point, eye, a.centers[level], step);
                    let wb = morph_weight(point, eye, b.centers[level], step);
                    let ha = sa.heights[0] as f64 * (1. - wa) + sa.heights[1] as f64 * wa;
                    let hb = sb.heights[0] as f64 * (1. - wb) + sb.heights[1] as f64 * wb;
                    max_delta = max_delta.max((ha - hb).abs());
                }
                for eye in [[1e6, -1e6], [-1e6, 1e6], [0., 0.]] {
                    for x in [-64., 64.] {
                        for z in [-64., 0., 64.] {
                            let point = [
                                a.centers[level][0] + x * step,
                                a.centers[level][1] + z * step,
                            ];
                            assert_eq!(morph_weight(point, eye, a.centers[level], step), 1.);
                            assert_eq!(
                                morph_weight(point, eye, a.centers[level + 1], step * 2.),
                                0.
                            );
                        }
                    }
                }
            }
        }
        assert!(
            max_delta < 1e-9,
            "common-vertex snap displacement {max_delta}m"
        );
        for eye in [[-1000., 1000.], [1000., -1000.], [8., 8.]] {
            for point in [[34., -34.], [-34., 34.]] {
                assert_eq!(morph_weight(point, eye, [0., 0.], 1.), 0.);
            }
        }
        eprintln!("camera-relative morph repeated8m/negative snaps common-vertex height delta={max_delta}m; all fine/coarse edges and collar pinned");
    }
    #[test]
    fn historical_direct_clamp_publication_diagnostic_and_temporal_regression() {
        use super::*;
        // The <=8m result above is conditional on timely publication. Record the
        // historical failure outside that guard: clamping the eye independently
        // to old/new page centers changes weights at the same world vertex.
        // Compare that baseline to retained anchors used by the active renderer.
        let source = FbmTerrain::new(7, Default::default()).unwrap();
        let before = TerrainData::at_center(&source, [0., 0.]);
        let after = TerrainData::at_center(&source, [8., 0.]);
        let count = ((N * 2 + 1).pow(2)) as usize;
        let mut observed_displacement = false;
        for lag in [8., 16., 24., 30.] {
            let eye: [f64; 2] = [lag, 0.];
            let mut max_delta = 0f64;
            let initial = std::array::from_fn(|level| {
                let guard = 4. * (1u32 << level) as f64;
                std::array::from_fn(|axis| eye[axis].clamp(-guard, guard))
            });
            let retained = advance_morph_anchors(initial, eye, eye, after.centers, 0.);
            assert_eq!(initial, retained);
            let mut temporal_delta = 0f64;
            for level in 1..LEVELS as usize - 1 {
                let step = 0.5 * (1u32 << level) as f64;
                for v in &before.vertices[level * count..(level + 1) * count] {
                    let point = [
                        v.position[0] as f64 + before.centers[level][0],
                        v.position[1] as f64 + before.centers[level][1],
                    ];
                    let q = [
                        (point[0] - after.centers[level][0]) / step,
                        (point[1] - after.centers[level][1]) / step,
                    ];
                    // Exclude collapsed outer vertices and newly exposed area;
                    // only compare identical canonical vertices in both pages.
                    if v.position.iter().any(|p| p.abs() >= 60. * step as f32)
                        || q.iter().any(|p| p.abs() >= 60.)
                    {
                        continue;
                    }
                    let bi = level * count
                        + ((q[1] as i32 + N) * (2 * N + 1) + q[0] as i32 + N) as usize;
                    let a = before.samples[v.sample as usize];
                    let b = after.samples[bi];
                    assert_eq!(a.heights, b.heights, "canonical samples remain fixed");
                    let wa = morph_weight(point, eye, before.centers[level], step);
                    let wb = morph_weight(point, eye, after.centers[level], step);
                    let ha = a.heights[0] as f64 * (1. - wa) + a.heights[1] as f64 * wa;
                    let hb = b.heights[0] as f64 * (1. - wb) + b.heights[1] as f64 * wb;
                    max_delta = max_delta.max((ha - hb).abs());
                    let wa = morph_weight(point, initial[level], before.centers[level], step);
                    let wb = morph_weight(point, retained[level], after.centers[level], step);
                    let ha = a.heights[0] as f64 * (1. - wa) + a.heights[1] as f64 * wa;
                    let hb = b.heights[0] as f64 * (1. - wb) + b.heights[1] as f64 * wb;
                    temporal_delta = temporal_delta.max((ha - hb).abs());
                }
            }
            assert!(max_delta.is_finite());
            if lag == 8. {
                assert!(
                    max_delta < 1e-9,
                    "timely-publication displacement {max_delta}m"
                );
            } else {
                observed_displacement |= max_delta > 1e-6;
            }
            assert!(temporal_delta < 1e-9);
            eprintln!("publication seed7/8oct lag={lag}m: historical direct clamp displacement={max_delta:.8}m; retained anchor instantaneous displacement={temporal_delta:.8}m");
        }
        assert!(
            observed_displacement,
            "historical baseline should continue to expose its original failure"
        );
    }
    #[test]
    fn native_and_proxy_share_one_projected_face_transfer() {
        let terrain = include_str!("terrain.wgsl")
            .split("struct Camera")
            .next()
            .unwrap();
        let native = include_str!("voxel.wgsl")
            .split("struct Camera")
            .next()
            .unwrap();
        assert_eq!(
            terrain, native,
            "native and proxy face/material lighting diverged"
        );
        // Independent finite staircase oracle: sum the projected area of actual
        // quantized top/side quads, then compare the macro projected-face model.
        let sun = glam::DVec3::new(-0.5, 0.8, -0.3).normalize();
        let grass = glam::DVec3::new(0.26, 0.53, 0.16);
        let soil = glam::DVec3::new(0.4, 0.27, 0.15);
        let mut max_error = 0f64;
        for (p, q) in [
            (0.1, 0.25),
            (0.5, -0.25),
            (1., 0.25),
            (-1.5, 0.25),
            (3., -0.25),
            (-15., 3.),
        ] {
            let h = |x: i32, z: i32| (p * x as f64 + q * z as f64).floor() as i32;
            for view in [
                glam::DVec3::new(-0.8, 0.4, -0.4),
                glam::DVec3::new(0.8, 0.5, 0.3),
                glam::DVec3::new(-0.1, 0.9, 0.4),
            ] {
                let view = view.normalize();
                let mut sum = glam::DVec3::ZERO;
                let mut area = 0.;
                let mut face = |normal: glam::DVec3, color: glam::DVec3, size: f64| {
                    let a = size * normal.dot(view).max(0.);
                    sum += color * (0.28 + 0.72 * normal.dot(sun).max(0.)) * a;
                    area += a;
                };
                for z in 0..80 {
                    for x in 0..80 {
                        face(glam::DVec3::Y, grass, 1.);
                        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                            let drop = h(x, z) - h(x + dx, z + dz);
                            if drop > 0 {
                                // Enumerate each actual exposed voxel face using
                                // canonical column depth/material rules.
                                for depth in 0..drop {
                                    let color = if depth == 0 {
                                        grass
                                    } else if depth < 12 {
                                        soil
                                    } else {
                                        glam::DVec3::new(0.48, 0.50, 0.54)
                                    };
                                    face(glam::DVec3::new(dx as f64, 0., dz as f64), color, 1.);
                                }
                            }
                        }
                    }
                }
                let oracle = sum / area;
                let n = glam::DVec3::new(-p, 1., -q);
                let normals = [
                    glam::DVec3::new(n.x.signum(), 0., 0.),
                    glam::DVec3::Y,
                    glam::DVec3::new(0., 0., n.z.signum()),
                ];
                let sizes = [n.x.abs(), 1., n.z.abs()];
                let mut sum = glam::DVec3::ZERO;
                let mut area = 0.;
                for i in 0..3 {
                    let a = sizes[i] * normals[i].dot(view).max(0.);
                    let span = sizes[i];
                    let side = (grass * span.min(1.)
                        + soil * (span - 1.).clamp(0., 11.)
                        + glam::DVec3::new(0.48, 0.50, 0.54) * (span - 12.).max(0.))
                        / span.max(1e-10);
                    let color = if i == 1 { grass } else { side };
                    sum += color * (0.28 + 0.72 * normals[i].dot(sun).max(0.)) * a;
                    area += a;
                }
                max_error = max_error.max((oracle - sum / area).abs().max_element());
            }
        }
        assert!(
            max_error < 1e-10,
            "projected staircase mean linear RGB mismatch {max_error}"
        );
        eprintln!("projected face appearance vs6400-column canonical-layer finite stairs:18slope/view cases maxmeanlinearRGBerror={max_error:.12}");
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
