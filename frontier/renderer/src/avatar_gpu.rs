//! Small camera-relative mesh for the articulated player, bowl, and droplets.
use crate::{
    avatar::{heading, Avatar, BOOT_FORWARD, BOOT_HALF},
    bowl_live::BowlLive,
    mesh::Vertex,
    vessel, Camera, Gpu,
};
use frontier_world::{Cell, VOXEL_SIZE};
use glam::{DQuat, DVec3, Vec3};
use std::collections::BTreeMap;

const VERTICES: u64 = 64_000;

pub struct AvatarGpu {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    format: wgpu::TextureFormat,
    samples: u32,
}

impl AvatarGpu {
    fn new(gpu: &Gpu) -> Self {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::include_wgsl!("avatar.wgsl"));
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("world-space player and held bowl"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(gpu.scene_format.into())],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: gpu.sample_count, ..Default::default() },
            multiview: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("player camera and sun"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("player uniforms"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("player geometry"),
            size: VERTICES * std::mem::size_of::<Vertex>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            uniform,
            group,
            vertices,
            format: gpu.scene_format,
            samples: gpu.sample_count,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        cache: &mut Option<Self>,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        resolve: Option<&wgpu::TextureView>,
        depth: &wgpu::TextureView,
        avatar: &Avatar,
        show_body: bool,
        bowl: &BowlLive,
        aim: Camera,
        view: Camera,
        aspect: f32,
        sun: Vec3,
    ) {
        if cache
            .as_ref()
            .is_none_or(|p| p.format != gpu.scene_format || p.samples != gpu.sample_count)
        {
            *cache = Some(Self::new(gpu));
        }
        let pass = cache.as_ref().unwrap();
        let mut model = build(avatar, show_body, bowl, aim, view.position);
        model.exposed_faces();
        if model.data.is_empty() {
            return;
        }
        assert!(model.data.len() as u64 <= VERTICES);
        gpu.queue
            .write_buffer(&pass.vertices, 0, bytemuck::cast_slice(&model.data));
        gpu.queue.write_buffer(
            &pass.uniform,
            0,
            bytemuck::cast_slice(&view.matrix(view.position, aspect).to_cols_array()),
        );
        gpu.queue.write_buffer(
            &pass.uniform,
            64,
            bytemuck::cast_slice(&sun.extend(0.).to_array()),
        );
        let mut render = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("player geometry after terrain"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                resolve_target: resolve,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        render.set_pipeline(&pass.pipeline);
        render.set_bind_group(0, &pass.group, &[]);
        render.set_vertex_buffer(0, pass.vertices.slice(..));
        render.draw(0..model.data.len() as u32, 0..1);
    }
}

struct Model {
    origin: DVec3,
    data: Vec<Vertex>,
    cells: BTreeMap<Cell, [f32; 3]>,
}

impl Model {
    fn tri(&mut self, face: [DVec3; 3], normal: DVec3, color: [f32; 3]) {
        self.data.extend(face.map(|p| Vertex {
            position: (p - self.origin).as_vec3().into(),
            color,
            normal: normal.as_vec3().into(),
        }));
    }

    fn quad(&mut self, [a, b, c, d]: [DVec3; 4], normal: DVec3, color: [f32; 3]) {
        self.tri([a, b, c], normal, color);
        self.tri([a, c, d], normal, color);
    }

    fn cuboid(&mut self, center: DVec3, rotation: DQuat, size: DVec3, color: [f32; 3]) {
        let half = size * 0.5;
        let bounds = (rotation * DVec3::X).abs() * half.x
            + (rotation * DVec3::Y).abs() * half.y
            + (rotation * DVec3::Z).abs() * half.z;
        let inverse = rotation.inverse();
        self.sample(center, bounds, |point| {
            let local = (inverse * (point - center)).abs();
            local.cmple(half).all().then_some(color)
        });
    }

    fn sample(&mut self, center: DVec3, half: DVec3, color: impl Fn(DVec3) -> Option<[f32; 3]>) {
        let min = Cell::from_meters((center - half).to_array());
        let max = Cell::from_meters((center + half).to_array());
        for z in min.z..=max.z {
            for y in min.y..=max.y {
                for x in min.x..=max.x {
                    let cell = Cell { x, y, z };
                    if let Some(color) = color(DVec3::from_array(cell.center_m())) {
                        self.cells.insert(cell, color);
                    }
                }
            }
        }
    }

    fn exposed_faces(&mut self) {
        let cells = std::mem::take(&mut self.cells);
        for (&cell, &color) in &cells {
            let center = DVec3::from_array(cell.center_m());
            for axis in 0..3 {
                for sign in [-1, 1] {
                    let mut adjacent = [cell.x, cell.y, cell.z];
                    adjacent[axis] += sign;
                    if cells.contains_key(&Cell {
                        x: adjacent[0],
                        y: adjacent[1],
                        z: adjacent[2],
                    }) {
                        continue;
                    }
                    let mut normal = DVec3::ZERO;
                    normal[axis] = sign as f64;
                    let mut u = DVec3::ZERO;
                    u[(axis + 1) % 3] = 1.;
                    let v = normal.cross(u);
                    let half = VOXEL_SIZE * 0.5;
                    self.quad(
                        [
                            center + (normal - u - v) * half,
                            center + (normal + u - v) * half,
                            center + (normal + u + v) * half,
                            center + (normal - u + v) * half,
                        ],
                        normal,
                        color,
                    );
                }
            }
        }
    }

    fn limb(&mut self, a: DVec3, b: DVec3, thickness: f64, color: [f32; 3]) {
        let line = b - a;
        let axis = line.normalize_or_zero();
        if axis != DVec3::ZERO {
            self.cuboid(
                (a + b) * 0.5,
                DQuat::from_rotation_arc(DVec3::Y, axis),
                DVec3::new(thickness, line.length() + thickness * 0.36, thickness),
                color,
            );
        }
    }
}

/// Equal-length two-bone chain. The pole controls which way the joint bends.
fn joint(a: DVec3, b: DVec3, bone: f64, pole: DVec3) -> DVec3 {
    let delta = b - a;
    let axis = delta.normalize_or_zero();
    let bend = (pole - axis * pole.dot(axis)).normalize_or_zero();
    let rise = (bone * bone - 0.25 * delta.length_squared()).max(0.).sqrt();
    (a + b) * 0.5 + bend * rise
}

fn build(avatar: &Avatar, show_body: bool, bowl: &BowlLive, aim: Camera, origin: DVec3) -> Model {
    let mut mesh = Model {
        origin,
        data: Vec::with_capacity(16_000),
        cells: BTreeMap::new(),
    };
    let hands = avatar.hands(aim);
    let forward = heading(aim.yaw);
    let right = forward.cross(DVec3::Y);
    let yaw = DQuat::from_rotation_y(aim.yaw as f64);
    let tilt = hands.torso();
    let down = DVec3::Y * hands.crouch;
    let shirt = [0.056, 0.24, 0.21];
    let sleeve = [0.085, 0.37, 0.30];
    let skin = [0.59, 0.28, 0.135];
    let pants = [0.12, 0.16, 0.23];
    let boots = [0.087, 0.046, 0.026];
    let hat = [0.65, 0.42, 0.19];
    let chest = hands.chest();
    let hip = aim.position
        - DVec3::Y * 0.82
        - down * 0.45
        - DVec3::Y * (hands.lean * 0.16)
        - forward * (hands.crouch * 0.07);
    if show_body {
        mesh.cuboid(chest, tilt, DVec3::new(0.42, 0.45, 0.25), shirt);
        mesh.cuboid(hip, yaw, DVec3::new(0.37, 0.21, 0.24), pants);
        mesh.cuboid(
            chest - forward * 0.16,
            tilt,
            DVec3::new(0.31, 0.32, 0.14),
            [0.33, 0.17, 0.067],
        );
        mesh.cuboid(
            chest - forward * 0.24 + DVec3::Y * 0.047,
            tilt,
            DVec3::new(0.23, 0.18, 0.0625),
            [0.53, 0.31, 0.13],
        );
        let head = chest + tilt * DVec3::new(0., 0.317, 0.028);
        mesh.cuboid(head, tilt, DVec3::new(0.217, 0.228, 0.2), skin);
        mesh.cuboid(
            head + tilt * DVec3::Y * 0.106,
            tilt,
            DVec3::new(0.381, 0.0625, 0.335),
            [0.42, 0.24, 0.078],
        );
        mesh.cuboid(
            head + tilt * DVec3::Y * 0.167,
            tilt,
            DVec3::new(0.257, 0.11, 0.227),
            hat,
        );
        for (i, boot) in avatar.boots().into_iter().enumerate() {
            let side = if i == 0 { -1. } else { 1. };
            let socket = hip + right * (0.122 * side);
            let ankle = boot + DVec3::Y * 0.11;
            let knee = joint(socket, ankle, 0.405, forward);
            mesh.limb(socket, knee, 0.153, pants);
            mesh.limb(knee, ankle, 0.132, [0.16, 0.21, 0.28]);
            mesh.cuboid(
                boot + DVec3::Y * BOOT_HALF.y + forward * BOOT_FORWARD,
                yaw,
                BOOT_HALF * 2.,
                boots,
            );
            mesh.cuboid(
                boot + DVec3::Y * 0.03125 + forward * BOOT_FORWARD,
                yaw,
                DVec3::new(0.166, 0.0625, 0.292),
                [0.031, 0.024, 0.017],
            );
        }
    }
    let pose = bowl.vessel.pose;
    if show_body || pose.is_some() {
        for side in [-1., 1.] {
            let shoulder = chest + right * (0.238 * side) + DVec3::Y * 0.125;
            let hand = pose.map_or(
                shoulder - DVec3::Y * 0.51 + forward * (0.11 + hands.lean * 0.16),
                |p| p.point(vessel::grip(side)),
            );
            let elbow = joint(shoulder, hand, 0.345, -DVec3::Y + right * (side * 0.6));
            mesh.limb(shoulder, elbow, 0.131, sleeve);
            mesh.limb(elbow, hand, 0.087, skin);
            mesh.cuboid(
                hand,
                pose.map_or(yaw, |p| p.rotation),
                DVec3::new(0.093, 0.075, 0.114),
                skin,
            );
        }
    }
    if let Some(pose) = pose {
        let radius = DVec3::splat(vessel::RIM + 0.075);
        mesh.sample(pose.center, radius, |point| {
            let local = pose.local(point);
            vessel::wood(local).then_some(if local.y > -VOXEL_SIZE {
                [0.52, 0.238, 0.074]
            } else {
                [0.37, 0.145, 0.042]
            })
        });
        for cell in bowl
            .vessel
            .display_cells(|cell| mesh.cells.contains_key(&cell))
        {
            mesh.cells.insert(cell, [0.043, 0.41, 0.57]);
        }
    }
    for drop in &bowl.state.drops {
        mesh.cells.entry(drop.cell).or_insert([0.066, 0.48, 0.67]);
    }
    mesh
}
