//! Cached direct shadows from the same exact triangles that own the rock surface.
use crate::{aircraft_shadow::AircraftShadow, mesh, Camera};
use glam::{DVec3, Mat4, Vec3};
use std::cell::Cell;

pub(crate) const BINDING: u32 = 20;
const MAP_SIZE: u32 = 768;

struct Caster {
    center: DVec3,
    pipeline: wgpu::RenderPipeline,
    matrix: wgpu::Buffer,
    bind: wgpu::BindGroup,
    last: Cell<Option<(u64, Vec3, Mat4)>>,
}
pub(crate) struct FormationShadow {
    pub receiver: AircraftShadow,
    caster: Option<Caster>,
}

impl FormationShadow {
    pub(crate) fn new(device: &wgpu::Device, center: Option<DVec3>) -> Self {
        let receiver =
            AircraftShadow::with_size(device, if center.is_some() { MAP_SIZE } else { 1 });
        let caster = center.map(|center| {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native rock caster"),
                source: wgpu::ShaderSource::Wgsl(
                    r#"
@group(0) @binding(0) var<uniform> m:mat4x4<f32>;
@vertex fn vs(@location(0) p:vec3f)->@builtin(position) vec4f {return m*vec4f(p,1.);}
"#
                    .into(),
                ),
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("native rock caster"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: size_of::<mesh::Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                    }],
                },
                fragment: None,
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
            });
            let matrix = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("native rock caster matrix"),
                size: 64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native rock caster matrix"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: matrix.as_entire_binding(),
                }],
            });
            Caster {
                center,
                pipeline,
                matrix,
                bind,
                last: Cell::new(None),
            }
        });
        Self { receiver, caster }
    }

    pub(crate) fn gpu_bytes(&self) -> usize {
        if self.caster.is_some() {
            MAP_SIZE as usize * MAP_SIZE as usize * 4
        } else {
            4
        }
    }

    pub(crate) fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: Camera,
        sun: Vec3,
        mesh: Option<(u64, DVec3, &wgpu::Buffer, &wgpu::Buffer, u32)>,
    ) {
        let Some(caster) = &self.caster else {
            return;
        };
        let Some((revision, origin, vertices, indices, count)) = mesh.filter(|_| sun.y > 0.035)
        else {
            self.receiver.set_receiver(queue, None, 0.);
            return;
        };
        let projection = if let Some((_, _, p)) = caster
            .last
            .get()
            .filter(|(rev, s, _)| *rev == revision && s.dot(sun) > 0.99998)
        {
            p
        } else {
            let up = if sun.y > 0.97 { Vec3::Z } else { Vec3::Y };
            let p = Mat4::orthographic_rh(-25., 25., -20., 20., 1., 125.)
                * Mat4::look_to_rh(sun * 55., -sun, up);
            let model = Mat4::from_translation((origin - caster.center).as_vec3());
            queue.write_buffer(
                &caster.matrix,
                0,
                bytemuck::cast_slice(&(p * model).to_cols_array()),
            );
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("formation directional shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.receiver.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&caster.pipeline);
            pass.set_bind_group(0, &caster.bind, &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..count, 0, 0..1);
            caster.last.set(Some((revision, sun, p)));
            p
        };
        self.receiver.set_receiver(
            queue,
            Some(projection * Mat4::from_translation((camera.position - caster.center).as_vec3())),
            0.00055,
        );
    }
}
