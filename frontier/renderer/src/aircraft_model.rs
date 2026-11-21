//! Lightweight high-wing bush plane, authored in metres around its pilot.
//! The same depth test as terrain handles ridgelines and ground occlusion.
use crate::{mesh::Vertex, Camera};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

const WHITE: [f32; 3] = [0.79, 0.81, 0.78];
const ORANGE: [f32; 3] = [0.89, 0.145, 0.027];
const GLASS: [f32; 3] = [0.055, 0.14, 0.19];
const BLACK: [f32; 3] = [0.035, 0.042, 0.047];
const METAL: [f32; 3] = [0.27, 0.31, 0.33];

fn quad(out: &mut Vec<Vertex>, p: [Vec3; 4], color: [f32; 3]) {
    for [a, b, c] in [[0, 1, 2], [0, 2, 3]] {
        let n = (p[b] - p[a]).cross(p[c] - p[a]).normalize_or_zero();
        if n == Vec3::ZERO {
            continue;
        }
        for i in [a, b, c] {
            out.push(Vertex {
                position: p[i].to_array(),
                normal: n.to_array(),
                color,
            });
        }
    }
}

fn loft(
    out: &mut Vec<Vertex>,
    sections: usize,
    sides: usize,
    point: impl Fn(usize, f32, f32) -> Vec3,
    color: impl Fn(usize, usize) -> [f32; 3],
) {
    for i in 0..sections - 1 {
        for j in 0..sides {
            let (s, c) = (std::f32::consts::TAU * j as f32 / sides as f32).sin_cos();
            let (s1, c1) =
                (std::f32::consts::TAU * ((j + 1) % sides) as f32 / sides as f32).sin_cos();
            quad(
                out,
                [
                    point(i, s, c),
                    point(i, s1, c1),
                    point(i + 1, s1, c1),
                    point(i + 1, s, c),
                ],
                color(i, j),
            );
        }
    }
}

fn tube(out: &mut Vec<Vertex>, a: Vec3, b: Vec3, radius: f32, color: [f32; 3]) {
    let axis = (b - a).normalize();
    let u = axis.any_orthonormal_vector();
    let v = axis.cross(u);
    loft(
        out,
        2,
        8,
        |i, s, c| {
            let center = if i == 0 { a } else { b };
            center + (u * c + v * s) * radius
        },
        |_, _| color,
    );
}

fn skin() -> (Vec<Vertex>, u32) {
    let mut out = Vec::new();
    // Longitudinal stations give the cowling, glazed cabin and tapered tail.
    let body = [
        (-4.65, 0.03, 0.08, 0.14),
        (-3.65, 0.23, 0.28, 0.06),
        (-1.6, 0.48, 0.51, -0.02),
        (-0.65, 0.61, 0.76, 0.10),
        (0.48, 0.62, 0.79, 0.08),
        (1.18, 0.53, 0.52, -0.04),
        (2.05, 0.46, 0.43, -0.07),
        (2.23, 0.31, 0.32, -0.06),
        (2.28, 0.01, 0.03, -0.06),
    ];
    loft(
        &mut out,
        body.len(),
        16,
        |i, s, c| {
            let (z, w, h, y) = body[i];
            Vec3::new(w * c, y + h * s, z)
        },
        |i, j| {
            if (2..=4).contains(&i) && (1..7).contains(&j) {
                GLASS
            } else if matches!(j, 0 | 7 | 8 | 15) || i >= 6 {
                ORANGE
            } else {
                WHITE
            }
        },
    );
    // Airfoil stations are ordered from port tip to starboard tip.
    for (span, z, chord, root_y, dihedral) in [
        (5.55, 0.08, 1.58, 0.76, 0.36),
        (2.05, -3.67, 0.90, 0.15, 0.12),
    ] {
        let stations = [-1., -0.91, -0.78, -0.11, 0.11, 0.78, 0.91, 1.];
        loft(
            &mut out,
            stations.len(),
            12,
            |i, s, c| {
                let t: f32 = stations[i];
                let a = t.abs();
                let taper = 1. - 0.44 * a.powi(3);
                Vec3::new(
                    t * span,
                    root_y + dihedral * a - s * 0.085 * chord * taper,
                    z - 0.22 * a * a + c * chord * 0.5 * taper,
                )
            },
            |i, _| if i == 0 || i == 6 { ORANGE } else { WHITE },
        );
    }
    let fin = [
        (0.10, -3.63, 0.94, 0.12),
        (0.90, -3.80, 0.61, 0.08),
        (1.56, -4.04, 0.39, 0.045),
        (1.68, -4.09, 0.22, 0.025),
    ];
    loft(
        &mut out,
        fin.len(),
        10,
        |i, s, c| {
            let (y, z, r, w) = fin[i];
            Vec3::new(s * w, y, z + c * r)
        },
        |i, _| if i >= 1 { ORANGE } else { WHITE },
    );
    for side in [-1., 1.] {
        let strut = Vec3::new(side * 0.50, -0.42, 0.08);
        tube(
            &mut out,
            strut,
            Vec3::new(side * 3.60, 0.99, 0.30),
            0.039,
            METAL,
        );
        let wheel = Vec3::new(side * 0.94, -1.07, 0.34);
        tube(&mut out, strut, wheel, 0.067, METAL);
        loft(
            &mut out,
            5,
            14,
            |i, s, c| {
                let (dx, r) = [
                    (-0.105, 0.),
                    (-0.08, 0.23),
                    (0., 0.25),
                    (0.08, 0.23),
                    (0.105, 0.),
                ][i];
                wheel + Vec3::new(dx, -s * r, c * r)
            },
            |_, _| BLACK,
        );
    }
    let prop_start = out.len() as u32;
    for j in 0..3 {
        let angle = j as f32 * std::f32::consts::TAU / 3.;
        let r = Vec3::new(angle.cos(), angle.sin(), 0.);
        let t = Vec3::new(-r.y, r.x, 0.);
        let hub = Vec3::new(0., -0.06, 2.31);
        for (a, b, w0, w1, color) in [
            (0.10, 0.90, 0.07, 0.095, BLACK),
            (0.90, 1.04, 0.095, 0.058, WHITE),
        ] {
            quad(
                &mut out,
                [
                    hub + r * a - t * w0,
                    hub + r * b - t * w1,
                    hub + r * b + t * w1,
                    hub + r * a + t * w0,
                ],
                color,
            );
        }
    }
    (out, prop_start)
}

#[path = "aircraft_voxels.rs"]
mod voxels;

pub(super) fn geometry() -> (&'static [Vertex], u32) {
    static MESH: std::sync::OnceLock<(Vec<Vertex>, u32)> = std::sync::OnceLock::new();
    let (mesh, prop) = MESH.get_or_init(|| {
        let (skin, split) = skin();
        let mut model = voxels::rasterize(&skin[..split as usize]);
        let prop = model.len() as u32;
        model.extend(voxels::rasterize(&skin[split as usize..]));
        (model, prop)
    });
    (mesh, *prop)
}

pub(crate) struct Model {
    pub samples: u32,
    pipeline: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    vertices: wgpu::Buffer,
    count: u32,
    prop_start: u32,
}
impl Model {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("aircraft enamel and glazing"),
            source: wgpu::ShaderSource::Wgsl(include_str!("aircraft_model.wgsl").into()),
        });
        let attributes = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x3];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bush plane"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });
        let (mesh, prop_start) = geometry();
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("aircraft skin"),
            contents: bytemuck::cast_slice(mesh),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aircraft view"),
            size: 96,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("aircraft view"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        Self {
            pipeline,
            group,
            uniform,
            vertices,
            count: mesh.len() as u32,
            prop_start,
            samples,
        }
    }
    pub(crate) fn prepare(
        &self,
        queue: &wgpu::Queue,
        camera: Camera,
        plane: Camera,
        aspect: f32,
        sun: Vec3,
        time: f32,
    ) {
        let axes = crate::aircraft::axes(plane);
        let matrix = camera.matrix(plane.position, aspect) * Mat4::from_mat3(axes);
        let mut data = [0f32; 24];
        data[..16].copy_from_slice(&matrix.to_cols_array());
        data[16..20].copy_from_slice(&(axes.transpose() * sun).extend(time * 45.).to_array());
        data[20..].copy_from_slice(
            &(axes.transpose() * (camera.position - plane.position).as_vec3())
                .extend(self.prop_start as f32)
                .to_array(),
        );
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&data));
    }
    pub(crate) fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..self.count, 0..1);
    }
}
