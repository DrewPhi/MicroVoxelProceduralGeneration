//! Bounded adaptive quadtree probe over the exact same immutable, canonical
//! filtered height pages as the clipmap. Greenfield CDLOD-inspired selection;
//! no claim of paper-faithful CDLOD, certified screen-space error or streaming.
//! The only per-camera CPU work selects small patch descriptors, never terrain
//! vertices. Hard LOD switches remain a temporal limitation of this probe.
use crate::terrain::{TerrainData, MAX_LEVELS};
use bytemuck::{Pod, Zeroable};
use std::{
    cell::{Cell, RefCell},
    time::Instant,
};
use wgpu::util::DeviceExt;
const PATCH: i32 = 16;
const CAPACITY: usize = 4096;
#[cfg(test)]
const PAGE: usize = 129 * 129;
const SHADER: &str = include_str!("surface_compare.wgsl");
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Leaf {
    x: i32,
    z: i32,
    level: u32,
}
impl Leaf {
    fn side(self) -> i32 {
        PATCH * (1 << self.level)
    } // half-meter units
    fn children(self) -> [Self; 4] {
        let h = self.side() / 2;
        std::array::from_fn(|i| Self {
            x: self.x + (i % 2) as i32 * h,
            z: self.z + (i / 2) as i32 * h,
            level: self.level - 1,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Patch {
    offset: [f32; 2],
    level: u32,
    edges: u32,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct SurfaceStats {
    pub leaves: u32,
    pub triangles: u32,
    pub selection_ms: f64,
    pub page_bytes: u64,
    pub instance_upload_bytes: u64,
}
#[derive(Clone)]
struct Layout {
    centers: [[i32; 2]; MAX_LEVELS as usize],
    levels: u32,
    origin: [f64; 2],
}
impl Layout {
    fn new(data: &TerrainData) -> Self {
        let origin = data.centers[0];
        Self {
            centers: std::array::from_fn(|i| {
                std::array::from_fn(|a| ((data.centers[i][a] - origin[a]) * 2.) as i32)
            }),
            levels: data.level_count,
            origin,
        }
    }
    fn contains(&self, p: Leaf) -> bool {
        let c = self.centers[p.level as usize];
        let r = 64 * (1 << p.level);
        p.x >= c[0] - r
            && p.z >= c[1] - r
            && p.x + p.side() <= c[0] + r
            && p.z + p.side() <= c[1] + r
    }
    fn select(&self, eye: [f64; 2]) -> Vec<Leaf> {
        let level = self.levels - 1;
        let side = PATCH * (1 << level);
        let c = self.centers[level as usize];
        let r = 64 * (1 << level);
        let mut leaves = Vec::new();
        fn visit(layout: &Layout, p: Leaf, eye: [f64; 2], leaves: &mut Vec<Leaf>) {
            let side = p.side() as f64;
            let dx = (p.x as f64 - eye[0])
                .max(0.)
                .max(eye[0] - (p.x as f64 + side));
            let dz = (p.z as f64 - eye[1])
                .max(0.)
                .max(eye[1] - (p.z as f64 + side));
            if p.level > 0
                && dx.max(dz) < side * 1.5
                && p.children().iter().all(|&c| layout.contains(c))
            {
                for child in p.children() {
                    visit(layout, child, eye, leaves);
                }
            } else {
                leaves.push(p);
            }
        }
        for z in 0..8 {
            for x in 0..8 {
                visit(
                    self,
                    Leaf {
                        x: c[0] - r + x * side,
                        z: c[1] - r + z * side,
                        level,
                    },
                    eye,
                    &mut leaves,
                );
            }
        }
        // Coarsen complete fine sibling groups until all neighboring patches are
        // 2:1. Coarsening cannot demand an unavailable fine page or make holes.
        loop {
            let mut replace = None;
            'outer: for (i, &a) in leaves.iter().enumerate() {
                for &b in &leaves {
                    if a.level + 1 < b.level && neighbors(a, b) {
                        replace = Some((i, a));
                        break 'outer;
                    }
                }
            }
            let Some((_, fine)) = replace else { break };
            let parent_side = fine.side() * 2;
            let top = self.centers[(self.levels - 1) as usize];
            let root_min = [
                top[0] - 64 * (1 << (self.levels - 1)),
                top[1] - 64 * (1 << (self.levels - 1)),
            ];
            let parent = Leaf {
                x: (fine.x - root_min[0]).div_euclid(parent_side) * parent_side + root_min[0],
                z: (fine.z - root_min[1]).div_euclid(parent_side) * parent_side + root_min[1],
                level: fine.level + 1,
            };
            leaves.retain(|p| {
                !(p.x >= parent.x
                    && p.z >= parent.z
                    && p.x + p.side() <= parent.x + parent.side()
                    && p.z + p.side() <= parent.z + parent.side())
            });
            leaves.push(parent);
        }
        leaves
    }
    fn patches(&self, leaves: &[Leaf]) -> Vec<Patch> {
        leaves
            .iter()
            .map(|&p| {
                let mut edges = 0;
                for &q in leaves {
                    if q.level == p.level + 1 {
                        if overlap(p.z, p.z + p.side(), q.z, q.z + q.side()) {
                            if p.x == q.x + q.side() {
                                edges |= 1;
                            }
                            if p.x + p.side() == q.x {
                                edges |= 2;
                            }
                        }
                        if overlap(p.x, p.x + p.side(), q.x, q.x + q.side()) {
                            if p.z == q.z + q.side() {
                                edges |= 4;
                            }
                            if p.z + p.side() == q.z {
                                edges |= 8;
                            }
                        }
                    }
                }
                for (corner, [x, z]) in [
                    [p.x, p.z],
                    [p.x + p.side(), p.z],
                    [p.x, p.z + p.side()],
                    [p.x + p.side(), p.z + p.side()],
                ]
                .into_iter()
                .enumerate()
                {
                    if leaves.iter().any(|q| {
                        q.level == p.level + 1
                            && x >= q.x
                            && x <= q.x + q.side()
                            && z >= q.z
                            && z <= q.z + q.side()
                    }) {
                        edges |= 1 << (4 + corner);
                    }
                }
                Patch {
                    offset: [p.x as f32 * 0.5, p.z as f32 * 0.5],
                    level: p.level,
                    edges,
                }
            })
            .collect()
    }
}
fn overlap(a: i32, b: i32, c: i32, d: i32) -> bool {
    a < d && c < b
}
fn neighbors(a: Leaf, b: Leaf) -> bool {
    ((a.x + a.side() == b.x || b.x + b.side() == a.x)
        && overlap(a.z, a.z + a.side(), b.z, b.z + b.side()))
        || ((a.z + a.side() == b.z || b.z + b.side() == a.z)
            && overlap(a.x, a.x + a.side(), b.x, b.x + b.side()))
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Camera {
    matrix: [[f32; 4]; 4],
    centers: [[f32; 4]; MAX_LEVELS as usize],
    eye: [f32; 4],
}
pub struct SurfaceCompareGpu {
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    instances: wgpu::Buffer,
    layout: Layout,
    view: Cell<[f64; 3]>,
    filter: Cell<bool>,
    stats: Cell<SurfaceStats>,
    selection: RefCell<Vec<Patch>>,
}
impl SurfaceCompareGpu {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        sample_count: u32,
        data: &TerrainData,
    ) -> Self {
        let layout = Layout::new(data);
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bounded quadtree camera"),
            size: std::mem::size_of::<Camera>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let samples = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("same clipmap height pages"),
            contents: bytemuck::cast_slice(&data.samples),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let entries = [
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
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ];
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quadtree pages"),
            entries: &entries,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quadtree snapshot"),
            layout: &bind_layout,
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
            label: Some("quadtree reusable grid"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quadtree"),
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });
        let vertex_attrs = wgpu::vertex_attr_array![0=>Uint32x2];
        let instance_attrs = wgpu::vertex_attr_array![1=>Float32x2,2=>Uint32,3=>Uint32];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("adaptive quadtree probe"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &vertex_attrs,
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: 16,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &instance_attrs,
                    },
                ],
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
        let points = (0..=16u32)
            .flat_map(|z| (0..=16u32).map(move |x| [x, z]))
            .collect::<Vec<_>>();
        let mut indices = Vec::<u32>::new();
        for z in 0..16 {
            for x in 0..16 {
                let a = z * 17 + x;
                indices.extend_from_slice(&[a, a + 17, a + 1, a + 1, a + 17, a + 18]);
            }
        }
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("immutable 16 square patch"),
            contents: bytemuck::cast_slice(&points),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("immutable patch indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bounded selected patches"),
            size: (CAPACITY * std::mem::size_of::<Patch>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            bind,
            uniform,
            vertices,
            indices,
            instances,
            view: Cell::new([layout.origin[0], 0., layout.origin[1]]),
            filter: Cell::new(true),
            stats: Cell::new(SurfaceStats {
                page_bytes: data.samples.len() as u64 * 80,
                ..Default::default()
            }),
            selection: RefCell::new(Vec::new()),
            layout,
        }
    }
    pub fn center(&self) -> [f64; 2] {
        self.layout.origin
    }
    pub fn set_view_position(&self, p: [f64; 3]) {
        self.view.set(p);
    }
    pub fn set_voxel_filter(&self, enabled: bool) {
        self.filter.set(enabled);
    }
    pub fn stats(&self) -> SurfaceStats {
        self.stats.get()
    }
    pub fn draw<'a>(
        &'a self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        matrix: glam::Mat4,
    ) {
        let started = Instant::now();
        let view = self.view.get();
        let leaves = self.layout.select([
            (view[0] - self.layout.origin[0]) * 2.,
            (view[2] - self.layout.origin[1]) * 2.,
        ]);
        let patches = self.layout.patches(&leaves);
        assert!(
            patches.len() <= CAPACITY,
            "bounded quadtree patch budget exceeded"
        );
        let mut previous = self.selection.borrow_mut();
        let changed = bytemuck::cast_slice::<Patch, u8>(&previous)
            != bytemuck::cast_slice::<Patch, u8>(&patches);
        if changed {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&patches));
            *previous = patches;
        }
        self.stats.set(SurfaceStats {
            leaves: leaves.len() as u32,
            triangles: leaves.len() as u32 * 512,
            selection_ms: started.elapsed().as_secs_f64() * 1000.,
            page_bytes: self.stats.get().page_bytes,
            instance_upload_bytes: if changed {
                (leaves.len() * 16) as u64
            } else {
                0
            },
        });
        let data = Camera {
            matrix: matrix.to_cols_array_2d(),
            centers: std::array::from_fn(|i| {
                [
                    self.layout.centers[i][0] as f32 * 0.5,
                    self.layout.centers[i][1] as f32 * 0.5,
                    0.,
                    0.,
                ]
            }),
            eye: [
                (view[0] - self.layout.origin[0]) as f32,
                (view[2] - self.layout.origin[1]) as f32,
                view[1] as f32,
                if self.filter.get() { 1. } else { 0. },
            ],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&data));
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_vertex_buffer(1, self.instances.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..1536, 0, 0..leaves.len() as u32);
    }
}
#[cfg(test)]
#[path = "surface_compare_tests.rs"]
mod tests;
