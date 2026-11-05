//! Experimental fixed-center surface proxy. Immutable nested grid footprints and
//! CPU-sampled canonical source pages; deliberately no streaming or native handoff.
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
    vertices: Vec<GridVertex>,
    indices: Vec<u32>,
    samples: Vec<Sample>,
}
fn filtered(source: &FbmTerrain, x: f64, z: f64, step: f64) -> f64 {
    // Box footprint filter is empirical, not a silhouette-error bound.
    let d = step * 0.25;
    [
        (x - d, z - d),
        (x + d, z - d),
        (x - d, z + d),
        (x + d, z + d),
    ]
    .into_iter()
    .map(|(x, z)| source.height_m(x, z))
    .sum::<f64>()
        * 0.25
}
fn parent_height(source: &FbmTerrain, x: f64, z: f64, step: f64) -> f64 {
    let px = (x / step).floor() * step;
    let pz = (z / step).floor() * step;
    let u = (x - px) / step;
    let v = (z - pz) / step;
    let a = filtered(source, px, pz, step);
    let b = filtered(source, px + step, pz, step);
    let c = filtered(source, px, pz + step, step);
    let d = filtered(source, px + step, pz + step, step);
    // Matches the grid's a-b-c / b-d-c diagonal exactly, including odd edge points.
    if u + v <= 1. {
        a + (b - a) * u + (c - a) * v
    } else {
        d + (c - d) * (1. - u) + (b - d) * (1. - v)
    }
}
fn filtered_color(source: &FbmTerrain, x: f64, z: f64, step: f64) -> [f32; 3] {
    let d = step * 0.25;
    let mut color = [0.; 3];
    for (x, z) in [
        (x - d, z - d),
        (x + d, z - d),
        (x - d, z + d),
        (x + d, z + d),
    ] {
        let c = source.surface_color(x, z, source.height_m(x, z));
        for i in 0..3 {
            color[i] += c[i] * 0.25;
        }
    }
    color
}
fn parent_color(source: &FbmTerrain, x: f64, z: f64, step: f64) -> [f32; 4] {
    let px = (x / step).floor() * step;
    let pz = (z / step).floor() * step;
    let u = ((x - px) / step) as f32;
    let v = ((z - pz) / step) as f32;
    let colors = [
        (px, pz),
        (px + step, pz),
        (px, pz + step),
        (px + step, pz + step),
    ]
    .map(|(x, z)| filtered_color(source, x, z, step));
    let mut out = [0.; 4];
    for i in 0..3 {
        let [a, b, c, d] = colors.map(|c| c[i]);
        out[i] = if u + v <= 1. {
            a + (b - a) * u + (c - a) * v
        } else {
            d + (c - d) * (1. - u) + (b - d) * (1. - v)
        };
    }
    out[3] = 1.;
    out
}
fn normal(source: &FbmTerrain, x: f64, z: f64, step: f64) -> [f32; 4] {
    let dx = filtered(source, x - step, z, step) - filtered(source, x + step, z, step);
    let dz = filtered(source, x, z - step, step) - filtered(source, x, z + step, step);
    let n = glam::DVec3::new(dx, 2. * step, dz).normalize();
    [n.x as f32, n.y as f32, n.z as f32, 0.]
}
impl TerrainData {
    pub fn new(source: &FbmTerrain) -> Self {
        let mut data = Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            samples: Vec::new(),
        };
        for level in 0..LEVELS {
            let step = 0.5 * (1u32 << level) as f64;
            let base = data.vertices.len() as u32;
            for z in -N..=N {
                for x in -N..=N {
                    // Collapse every odd outer-edge point onto an even parent anchor.
                    // This removes the T junction rather than merely hiding it with skirts.
                    let (mut gx, mut gz) = (x, z);
                    if x.abs() == N && z % 2 != 0 {
                        gz -= 1;
                    }
                    if z.abs() == N && x % 2 != 0 {
                        gx -= 1;
                    }
                    let wx = gx as f64 * step;
                    let wz = gz as f64 * step;
                    let morph = if level + 1 == LEVELS {
                        0.
                    } else {
                        ((x.abs().max(z.abs()) as f32 - 48.) / 16.).clamp(0., 1.)
                    };
                    let h = filtered(source, wx, wz, step);
                    let parent = parent_height(source, wx, wz, step * 2.);
                    let color = filtered_color(source, wx, wz, step);
                    data.vertices.push(GridVertex {
                        position: [wx as f32, wz as f32],
                        sample: data.samples.len() as u32,
                        morph,
                    });
                    data.samples.push(Sample {
                        heights: [h as f32, parent as f32, 0., 0.],
                        normal: normal(source, wx, wz, step),
                        parent_normal: normal(source, wx, wz, step * 2.),
                        color: [color[0], color[1], color[2], 1.],
                        parent_color: parent_color(source, wx, wz, step * 2.),
                    });
                }
            }
            let row = (2 * N + 1) as u32;
            for z in -N..N {
                for x in -N..N {
                    if level > 0 && (-N / 2..N / 2).contains(&x) && (-N / 2..N / 2).contains(&z) {
                        continue;
                    }
                    let a = base + (z + N) as u32 * row + (x + N) as u32;
                    if x == N - 1 && z == N - 1 {
                        // At the corner, two collapsed edge points and the inner vertex
                        // are collinear. Split the remaining face through that inner
                        // vertex to avoid a diagonal T junction.
                        data.indices.extend_from_slice(&[
                            a,
                            a + row,
                            a + row + 1,
                            a,
                            a + row + 1,
                            a + 1,
                        ]);
                    } else {
                        data.indices.extend_from_slice(&[
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
        }
        eprintln!("FBM fixed rings: {} vertices, {} triangles, {:.2} MiB pages; coverage ±{}m; no recenter/edits/native handoff",data.vertices.len(),data.indices.len()/3,data.samples.len() as f64*80./1048576.,HALF_EXTENT);
        data
    }
}
pub struct TerrainGpu {
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    pub index_count: u32,
}
impl TerrainGpu {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, data: &TerrainData) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain camera"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let samples = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("canonical height pages"),
            contents: bytemuck::cast_slice(&data.samples),
            usage: wgpu::BufferUsages::STORAGE,
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
            index_count: data.indices.len() as u32,
        }
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
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
            for t in data.indices.chunks_exact(3) {
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
