//! A small directional shadow map shared by the rolling and exact terrain.
use crate::Camera;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use std::cell::Cell;
use wgpu::util::DeviceExt;

const SIZE: u32 = 256;
const HALF_SPAN: f32 = 9.;
const LIGHT_OFFSET: f32 = 16.;
const LIGHT_RANGE: f32 = 144.;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Receiver {
    projection: [[f32; 4]; 4],
    parameters: [f32; 4],
}

pub(crate) struct AircraftShadow {
    pub(crate) depth: wgpu::TextureView,
    receiver: wgpu::Buffer,
    sampler: wgpu::Sampler,
    active: Cell<bool>,
}

impl AircraftShadow {
    pub(crate) const BYTES: usize = SIZE as usize * SIZE as usize * 4 + size_of::<Receiver>();

    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self::with_size(device, SIZE)
    }

    pub(crate) fn with_size(device: &wgpu::Device, size: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("aircraft directional shadow map"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("aircraft shadow comparison"),
            compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let receiver = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("aircraft shadow receiver projection"),
            contents: bytemuck::bytes_of(&Receiver::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        Self {
            depth: texture.create_view(&Default::default()),
            receiver,
            sampler,
            active: Cell::new(false),
        }
    }

    pub(crate) fn prepare(
        &self,
        queue: &wgpu::Queue,
        viewer: Camera,
        aircraft: Option<Camera>,
        sun: Vec3,
    ) -> Option<Mat4> {
        let active_aircraft = aircraft.filter(|pose| {
            sun.y > 0. && (viewer.position - pose.position).length_squared() < 300. * 300.
        });
        let Some(aircraft) = active_aircraft else {
            if self.active.replace(false) {
                queue.write_buffer(&self.receiver, 0, bytemuck::bytes_of(&Receiver::zeroed()));
            }
            return None;
        };
        let up = if sun.y > 0.95 { Vec3::Z } else { Vec3::Y };
        let projection = Mat4::orthographic_rh(
            -HALF_SPAN,
            HALF_SPAN,
            -HALF_SPAN,
            HALF_SPAN,
            0.,
            LIGHT_RANGE,
        ) * Mat4::look_to_rh(sun * LIGHT_OFFSET, -sun, up);
        let relative_eye = (viewer.position - aircraft.position).as_vec3();
        queue.write_buffer(
            &self.receiver,
            0,
            bytemuck::bytes_of(&Receiver {
                projection: (projection * Mat4::from_translation(relative_eye)).to_cols_array_2d(),
                parameters: [1., 0.025 / LIGHT_RANGE, 0., 0.],
            }),
        );
        self.active.set(true);
        Some(projection * Mat4::from_mat3(crate::aircraft::axes(aircraft)))
    }

    pub(crate) fn set_receiver(&self, queue: &wgpu::Queue, projection: Option<Mat4>, bias: f32) {
        if let Some(projection) = projection {
            queue.write_buffer(
                &self.receiver,
                0,
                bytemuck::bytes_of(&Receiver {
                    projection: projection.to_cols_array_2d(),
                    parameters: [1., bias, 0., 0.],
                }),
            );
            self.active.set(true);
        } else if self.active.replace(false) {
            queue.write_buffer(&self.receiver, 0, bytemuck::bytes_of(&Receiver::zeroed()));
        }
    }

    pub(crate) fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
        Self::layout_entries_at(15)
    }

    pub(crate) fn layout_entries_at(mut binding: u32) -> [wgpu::BindGroupLayoutEntry; 3] {
        [
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
        ]
        .map(|ty| {
            let entry = wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty,
                count: None,
            };
            binding += 1;
            entry
        })
    }

    pub(crate) fn bindings(&self) -> [wgpu::BindGroupEntry<'_>; 3] {
        self.bindings_at(15)
    }

    pub(crate) fn bindings_at(&self, mut binding: u32) -> [wgpu::BindGroupEntry<'_>; 3] {
        [
            self.receiver.as_entire_binding(),
            wgpu::BindingResource::TextureView(&self.depth),
            wgpu::BindingResource::Sampler(&self.sampler),
        ]
        .map(|resource| {
            let entry = wgpu::BindGroupEntry { binding, resource };
            binding += 1;
            entry
        })
    }
}
