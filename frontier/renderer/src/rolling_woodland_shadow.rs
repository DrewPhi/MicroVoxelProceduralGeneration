//! Bounded, world-snapped shadows from streamed tree geometry.
use crate::{aircraft_shadow::AircraftShadow, settings::VisualSettings, Camera};
use glam::{DVec3, Mat4, Vec3};
use std::cell::RefCell;

pub(crate) const BINDING: u32 = 25;
const RESOLUTION: u32 = 1024;
const HALF_SPAN_M: f32 = 54.;
const DEPTH_M: f32 = 240.;

struct Caster {
    uniform: wgpu::Buffer,
    pub bind: wgpu::BindGroup,
    last: RefCell<Option<ShadowKey>>,
}

#[derive(PartialEq)]
struct ShadowKey {
    center: DVec3,
    light: Vec3,
    casters: Vec<(crate::vegetation::MeshKey, Vec<u8>)>,
}

pub(crate) struct WoodlandShadow {
    pub receiver: AircraftShadow,
    caster: Option<Caster>,
}

impl WoodlandShadow {
    pub(crate) fn new(device: &wgpu::Device, living_woods: bool) -> Self {
        let enabled =
            crate::woodland_volume::render_option("FRONTIER_ROLLING_PLANT_SHADOWS", living_woods);
        let receiver = AircraftShadow::with_size(device, if enabled { RESOLUTION } else { 1 });
        let caster = enabled.then(|| {
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rolling woodland caster projection"),
                size: 80,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rolling woodland caster projection"),
                layout: &crate::adaptive_gpu::adaptive_shadow::near_depth_layout(device),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                }],
            });
            Caster {
                uniform,
                bind,
                last: RefCell::new(None),
            }
        });
        Self { receiver, caster }
    }

    pub(crate) fn gpu_bytes(&self) -> usize {
        if self.caster.is_some() {
            4 * RESOLUTION as usize * RESOLUTION as usize
        } else {
            4
        }
    }

    pub(crate) fn prepare(
        &self,
        queue: &wgpu::Queue,
        camera: Camera,
        ground_y: f64,
        settings: &VisualSettings,
        tree_casters: impl FnOnce() -> Vec<(crate::vegetation::MeshKey, Vec<u8>)>,
    ) -> Option<&wgpu::BindGroup> {
        let caster = self.caster.as_ref()?;
        let light = settings.terrain_light_direction();
        if !settings.terrain_shadows
            || !settings.beauty
            || settings.visibility_benchmark
            || settings.terrain_gray
            || light.y < 0.13
            || (camera.position.y - ground_y).abs() > 78.
        {
            self.receiver.set_receiver(queue, None, 0.);
            return None;
        }
        let casters = tree_casters();
        if casters.is_empty() {
            self.receiver.set_receiver(queue, None, 0.);
            return None;
        }
        // Daylight ticks smaller than a fraction of a tree texel do not
        // invalidate the world-space depth map.
        let light = ((light * 2048.).round() / 2048.).normalize();
        let up = if light.y > 0.97 { Vec3::Z } else { Vec3::Y };
        let right = up.cross(light).normalize();
        let vertical = light.cross(right);
        let texel = f64::from(2. * HALF_SPAN_M / RESOLUTION as f32);
        let origin = DVec3::new(camera.position.x, ground_y + 10., camera.position.z);
        let snap = |axis: Vec3, step: f64| {
            let axis = axis.as_dvec3();
            axis * ((origin.dot(axis) / step).round() * step)
        };
        let center = snap(right, texel) + snap(vertical, texel) + snap(light, 1.);
        let projection = Mat4::orthographic_rh(
            -HALF_SPAN_M,
            HALF_SPAN_M,
            -HALF_SPAN_M,
            HALF_SPAN_M,
            0.,
            DEPTH_M,
        ) * Mat4::look_to_rh(light * (DEPTH_M * 0.5), -light, up)
            * Mat4::from_translation((camera.position - center).as_vec3());
        self.receiver
            .set_receiver(queue, Some(projection), 0.16 / DEPTH_M);
        let key = ShadowKey {
            center,
            light,
            casters,
        };
        if caster.last.borrow().as_ref() == Some(&key) {
            return None;
        }
        caster.last.replace(Some(key));
        queue.write_buffer(
            &caster.uniform,
            0,
            bytemuck::cast_slice(&projection.to_cols_array()),
        );
        Some(&caster.bind)
    }
}

/// Low-resolution, horizontal canopy transmittance for the aerial view.
/// The close directional depth map still owns precise local shadows.
pub(crate) struct CanopyGroundShade {
    view: wgpu::TextureView,
    uniform: wgpu::Buffer,
    sampler: wgpu::Sampler,
    draw_bind: wgpu::BindGroup,
    enabled: bool,
    has_shade: std::cell::Cell<bool>,
}

impl CanopyGroundShade {
    const SIZE: u32 = 1024;
    const HALF_SPAN: f64 = 1600.;

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn draw_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("aerial forest shadow draw uniform"),
            entries: &[Self::uniform_entry(0, wgpu::ShaderStages::VERTEX)],
        })
    }

    fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }
    }

    pub(crate) fn read_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
        [
            Self::uniform_entry(28, wgpu::ShaderStages::FRAGMENT),
            wgpu::BindGroupLayoutEntry {
                binding: 29,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 30,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ]
    }

    pub(crate) fn bindings(&self) -> [wgpu::BindGroupEntry<'_>; 3] {
        [
            wgpu::BindGroupEntry {
                binding: 28,
                resource: self.uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 29,
                resource: wgpu::BindingResource::TextureView(&self.view),
            },
            wgpu::BindGroupEntry {
                binding: 30,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            },
        ]
    }

    pub(crate) fn new(device: &wgpu::Device, enabled: bool) -> Self {
        use wgpu::util::DeviceExt;
        let side = if enabled { Self::SIZE } else { 1 };
        let view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("projected forest canopy opacity"),
                size: wgpu::Extent3d {
                    width: side,
                    height: side,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("projected forest shadow bounds and light"),
            contents: bytemuck::bytes_of(&[0f32; 8]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let draw_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("projected forest shadow draw"),
            layout: &Self::draw_layout(device),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("projected forest shadow filter"),
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            view,
            uniform,
            sampler,
            draw_bind,
            enabled,
            has_shade: std::cell::Cell::new(false),
        }
    }

    pub(crate) fn bytes(&self) -> usize {
        if self.enabled {
            (Self::SIZE * Self::SIZE) as usize
        } else {
            1
        }
    }

    pub(crate) fn reset(&self, queue: &wgpu::Queue) {
        if self.has_shade.replace(false) {
            queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&[0f32; 8]));
        }
    }

    pub(crate) fn prepare(
        &self,
        queue: &wgpu::Queue,
        camera: Camera,
        ground_y: f64,
        settings: &VisualSettings,
    ) -> Option<(&wgpu::TextureView, &wgpu::BindGroup)> {
        let sun = settings.terrain_light_direction();
        let altitude = camera.position.y - ground_y;
        if !self.enabled
            || !settings.beauty
            || !settings.terrain_shadows
            || settings.visibility_benchmark
            || settings.terrain_gray
            || sun.y < 0.22
            || altitude <= 52.
        {
            self.reset(queue);
            return None;
        }
        let spacing = 2. * Self::HALF_SPAN / f64::from(Self::SIZE);
        let snap = |m: f64| ((m / spacing).round() * spacing - m) as f32;
        let altitude_weight = ((altitude - 52.) / 71.).clamp(0., 1.) as f32;
        let settings = [
            snap(camera.position.x),
            snap(camera.position.z),
            Self::HALF_SPAN.recip() as f32,
            0.57 * altitude_weight,
            sun.x,
            sun.y,
            sun.z,
            0.,
        ];
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&settings));
        self.has_shade.set(true);
        Some((&self.view, &self.draw_bind))
    }
}
