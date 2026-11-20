//! Small camera-relative mesh for the articulated player, bowl, and droplets.
use crate::{
    avatar::{heading, Avatar, BASE_RADIUS, DEPTH, RIM},
    bowl_live::BowlLive,
    mesh::Vertex,
    vessel, Camera, Gpu,
};
use glam::{DQuat, DVec2, DVec3, Vec3};

const VERTICES: u64 = 8_192;

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
        let model = build(avatar, show_body, bowl, aim, view.position);
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
        let corners: [DVec3; 8] = std::array::from_fn(|i| {
            let p = DVec3::new(
                (i & 1) as f64 - 0.5,
                ((i >> 1) & 1) as f64 - 0.5,
                ((i >> 2) & 1) as f64 - 0.5,
            );
            center + rotation * (size * p)
        });
        for (face, normal) in [
            ([0, 2, 6, 4], -DVec3::X),
            ([1, 5, 7, 3], DVec3::X),
            ([0, 4, 5, 1], -DVec3::Y),
            ([2, 3, 7, 6], DVec3::Y),
            ([0, 1, 3, 2], -DVec3::Z),
            ([4, 6, 7, 5], DVec3::Z),
        ] {
            self.quad(face.map(|i| corners[i]), rotation * normal, color);
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
        data: Vec::with_capacity(1800),
    };
    let hands = avatar.hands(aim);
    let forward = heading(aim.yaw);
    let right = forward.cross(DVec3::Y);
    let yaw = DQuat::from_rotation_y(aim.yaw as f64);
    let tilt = yaw * DQuat::from_rotation_x(hands.lean * 0.78);
    let down = DVec3::Y * hands.crouch;
    let shirt = [0.056, 0.24, 0.21];
    let sleeve = [0.085, 0.37, 0.30];
    let skin = [0.59, 0.28, 0.135];
    let pants = [0.12, 0.16, 0.23];
    let boots = [0.087, 0.046, 0.026];
    let hat = [0.65, 0.42, 0.19];
    let chest = aim.position - DVec3::Y * (0.43 + hands.lean * 0.31) - down * 0.51
        + forward * (hands.lean * 0.36);
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
            DVec3::new(0.23, 0.18, 0.025),
            [0.53, 0.31, 0.13],
        );
        let head = chest + tilt * DVec3::new(0., 0.317, 0.028);
        mesh.cuboid(head, tilt, DVec3::new(0.217, 0.228, 0.2), skin);
        mesh.cuboid(
            head + tilt * DVec3::Y * 0.106,
            tilt,
            DVec3::new(0.381, 0.030, 0.335),
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
                boot + DVec3::Y * 0.065 + forward * 0.042,
                yaw,
                DVec3::new(0.153, 0.121, 0.277),
                boots,
            );
            mesh.cuboid(
                boot + DVec3::Y * 0.019 + forward * 0.045,
                yaw,
                DVec3::new(0.166, 0.032, 0.292),
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
                |p| p.point(DVec3::new(-side * (RIM + 0.006), -0.031, -0.10)),
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
        let polygon: [DVec2; 14] = std::array::from_fn(|i| {
            let (s, c) = (i as f64 * std::f64::consts::TAU / 14.).sin_cos();
            DVec2::new(c, s)
        });
        let ring = |p: DVec2, r, y| pose.point(DVec3::new(p.x * r, y, p.y * r));
        let inner = RIM - 0.015;
        let bottom = RIM * BASE_RADIUS;
        for i in 0..polygon.len() {
            let a = polygon[i];
            let b = polygon[(i + 1) % polygon.len()];
            let normal = pose.rotation * DVec3::new(a.x + b.x, -0.9, a.y + b.y).normalize();
            let [ra, rb, ia, ib] = [
                ring(a, RIM, 0.),
                ring(b, RIM, 0.),
                ring(a, inner, 0.),
                ring(b, inner, 0.),
            ];
            let wood = match i % 3 {
                0 => [0.43, 0.19, 0.062],
                1 => [0.37, 0.15, 0.042],
                _ => [0.40, 0.17, 0.054],
            };
            mesh.quad(
                [
                    ra,
                    rb,
                    ring(b, bottom, -DEPTH - 0.024),
                    ring(a, bottom, -DEPTH - 0.024),
                ],
                normal,
                wood,
            );
            mesh.quad(
                [
                    ia,
                    ib,
                    ring(b, bottom - 0.013, -DEPTH),
                    ring(a, bottom - 0.013, -DEPTH),
                ],
                -normal,
                [0.32, 0.123, 0.035],
            );
            mesh.quad(
                [ra, rb, ib, ia],
                pose.rotation * DVec3::Y,
                [0.59, 0.29, 0.095],
            );
            mesh.tri(
                [
                    pose.point(-DVec3::Y * DEPTH),
                    ring(a, bottom, -DEPTH),
                    ring(b, bottom, -DEPTH),
                ],
                pose.rotation * DVec3::Y,
                [0.36, 0.15, 0.044],
            );
            mesh.tri(
                [
                    pose.point(-DVec3::Y * (DEPTH + 0.024)),
                    ring(a, bottom, -DEPTH - 0.024),
                    ring(b, bottom, -DEPTH - 0.024),
                ],
                pose.rotation * -DVec3::Y,
                wood,
            );
        }
        if bowl.state.units > 0 {
            let shape = vessel::surface(bowl.vessel.slope, bowl.vessel.level, &polygon);
            let normal = pose.rotation
                * DVec3::new(-bowl.vessel.slope.x, 1., -bowl.vessel.slope.y).normalize();
            for i in 1..shape.len().saturating_sub(1) {
                mesh.tri(
                    [shape[0], shape[i], shape[i + 1]].map(|p| pose.point(p)),
                    normal,
                    [0.042, 0.39, 0.57],
                );
            }
        }
    }
    for drop in &bowl.state.drops {
        let cell = DVec3::from_array(drop.cell.center_m());
        let velocity = DVec3::from_array(drop.velocity);
        let axis = velocity.normalize_or_zero();
        let turn = if axis == DVec3::ZERO {
            DQuat::IDENTITY
        } else {
            DQuat::from_rotation_arc(DVec3::Y, axis)
        };
        mesh.cuboid(
            cell,
            turn,
            DVec3::new(0.052, 0.074, 0.052),
            [0.066, 0.48, 0.67],
        );
    }
    mesh
}
