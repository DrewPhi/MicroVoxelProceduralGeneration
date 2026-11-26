use crate::{
    destruction::{Fragment, Shape, Vertex},
    Camera,
};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct View {
    view: [[f32; 4]; 4],
    sun: [f32; 4],
}

struct Mesh {
    owner: Weak<Shape>,
    buffer: wgpu::Buffer,
    vertices: u32,
}

pub struct Display {
    pipeline: wgpu::RenderPipeline,
    view: wgpu::Buffer,
    bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    capacity: usize,
    models: Vec<[[f32; 4]; 4]>,
    visible: Vec<usize>,
    meshes: HashMap<usize, Mesh>,
    pub samples: u32,
}

fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("wreckage transform instances"),
        size: (capacity * size_of::<[[f32; 4]; 4]>()) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::VERTEX,
        mapped_at_creation: false,
    })
}

impl Display {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("moving voxel debris"),
            source: wgpu::ShaderSource::Wgsl(include_str!("destruction_gpu.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("moving voxel debris"), layout: None,
            vertex: wgpu::VertexState {
                module: &shader, entry_point: Some("vertex"), compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout { array_stride: size_of::<Vertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3] },
                    wgpu::VertexBufferLayout { array_stride: 64, step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4] },
                ],
            },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fragment"), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true, depth_compare: wgpu::CompareFunction::Less, stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            multiview: None, cache: None,
        });
        let view = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wreckage view"),
            size: size_of::<View>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wreckage view"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view.as_entire_binding(),
            }],
        });
        Self {
            pipeline,
            view,
            bind,
            capacity: 16,
            instances: instance_buffer(device, 16),
            models: Vec::new(),
            visible: Vec::new(),
            meshes: HashMap::new(),
            samples,
        }
    }

    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bodies: &[Fragment],
        camera: Camera,
    ) {
        self.models.clear();
        self.visible.clear();
        self.meshes.retain(|_, m| m.owner.strong_count() > 0);
        for body in bodies {
            let radius = body.shape.size.length();
            let center = (body.pose.center - camera.position).as_vec3();
            if center.length_squared() > (camera.far + radius).powi(2)
                || center.dot(camera.direction()) < -radius
            {
                continue;
            }
            let key = Arc::as_ptr(&body.shape) as usize;
            self.meshes.entry(key).or_insert_with(|| Mesh {
                owner: Arc::downgrade(&body.shape),
                vertices: body.shape.vertices.len() as u32,
                buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("solid debris voxel faces"),
                    contents: bytemuck::cast_slice(&body.shape.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
            });
            self.models.push(
                Mat4::from_rotation_translation(body.pose.orientation, center).to_cols_array_2d(),
            );
            self.visible.push(key);
        }
        if self.models.len() > self.capacity {
            self.capacity = self.models.len().next_power_of_two();
            self.instances = instance_buffer(device, self.capacity);
        }
        if !self.models.is_empty() {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.models));
        }
    }

    pub fn prepare(&self, queue: &wgpu::Queue, camera: Camera, aspect: f32, sun: Vec3) {
        queue.write_buffer(
            &self.view,
            0,
            bytemuck::bytes_of(&View {
                view: camera.matrix(camera.position, aspect).to_cols_array_2d(),
                sun: sun.extend(0.).to_array(),
            }),
        );
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.visible.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(1, self.instances.slice(..));
        for (i, key) in self.visible.iter().enumerate() {
            let mesh = &self.meshes[key];
            pass.set_vertex_buffer(0, mesh.buffer.slice(..));
            pass.draw(0..mesh.vertices, i as u32..i as u32 + 1);
        }
    }
}
