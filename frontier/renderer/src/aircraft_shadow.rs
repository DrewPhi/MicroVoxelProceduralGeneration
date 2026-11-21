//! Orthographic view of the airframe along the sun. Translation leaves this
//! small texture unchanged, including when the world origin is rebased.
use crate::{aircraft, aircraft_model, atmosphere, Camera};
use glam::{DVec3, Mat3, Mat4, Vec3};
use std::cell::Cell;
use wgpu::util::DeviceExt;

const SIZE: u32 = 256;
const SHADER: &str = r#"
@group(0) @binding(0) var<uniform> light:mat4x4<f32>;
struct Vertex { @builtin(position) clip:vec4<f32>, @location(0) height:f32 };
@vertex fn vs(@location(0) position:vec3<f32>)->Vertex {
    let p=(light*vec4(position,1.)).xyz;
    return Vertex(vec4(p.xy/8.,.5,1.),.5+p.z/16.);
}
@fragment fn fs(v:Vertex)->@location(0) vec4<f32> {return vec4(v.height);}
"#;

pub(crate) struct Shadow {
    pub view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    light: wgpu::Buffer,
    positions: wgpu::Buffer,
    count: u32,
    last: Cell<Option<Mat3>>,
    dirty: Cell<bool>,
}

impl Shadow {
    pub fn new(device: &wgpu::Device) -> Self {
        let format = wgpu::TextureFormat::R8Unorm;
        let view = atmosphere::texture(device, "aircraft sun depth", format, SIZE, SIZE, 1);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("airframe sun projection"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let max = wgpu::BlendComponent {
            operation: wgpu::BlendOperation::Max,
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("airframe shadow"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: max,
                        alpha: max,
                    }),
                    write_mask: wgpu::ColorWrites::RED,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let light = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aircraft to sun"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("aircraft sun"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: light.as_entire_binding(),
            }],
        });
        let (mesh, count) = aircraft_model::geometry();
        // Use the opaque airframe up to the rotating propeller. A spinning
        // blade sweeps through a texel for only a small part of an exposure.
        let vertices: Vec<_> = mesh[..count as usize].iter().map(|v| v.position).collect();
        let positions = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("aircraft caster"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Self {
            view,
            pipeline,
            bind,
            light,
            positions,
            count,
            last: Cell::new(None),
            dirty: Cell::new(false),
        }
    }

    pub fn bytes(&self) -> u64 {
        u64::from(SIZE).pow(2) + self.positions.size() + self.light.size()
    }

    pub fn prepare(
        &self,
        queue: &wgpu::Queue,
        pose: Camera,
        eye: DVec3,
        sun: Vec3,
    ) -> [[f32; 4]; 4] {
        let up = if sun.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
        let right = up.cross(sun).normalize();
        let vertical = sun.cross(right);
        let axes = Mat3::from_cols(right, vertical, sun).transpose() * aircraft::axes(pose);
        if self.last.replace(Some(axes)) != Some(axes) {
            queue.write_buffer(
                &self.light,
                0,
                bytemuck::cast_slice(&Mat4::from_mat3(axes).to_cols_array()),
            );
            self.dirty.set(true);
        }
        [
            right.extend(0.).to_array(),
            vertical.extend(0.).to_array(),
            sun.extend(0.).to_array(),
            (pose.position - eye).as_vec3().extend(1.).to_array(),
        ]
    }

    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder) {
        if !self.dirty.replace(false) {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("aircraft sun depth"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, self.positions.slice(..));
        pass.draw(0..self.count, 0..1);
    }
}
