//! Inexpensive, independently switchable sky / height fog / cloud composition.
//! This pass uses physical scene depth, never streaming readiness or LOD state.
use crate::settings::VisualSettings;
use glam::{DVec3, Mat4, Vec3};
use wgpu::util::DeviceExt;

const SHADER: &str = include_str!("atmosphere.wgsl");
const BLIT: &str = r#"
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
struct Output { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs_main(@builtin(vertex_index) i:u32)->Output {
 var p=array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
 var o:Output; o.position=vec4(p[i],0.0,1.0); o.uv=vec2(p[i].x*0.5+0.5,0.5-p[i].y*0.5); return o;
}
@fragment fn fs_main(i:Output)->@location(0) vec4<f32> { return textureSample(image,image_sampler,i.uv); }
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    inverse_view_projection: [[f32; 4]; 4],
    relative_eye: [f32; 4],
    world_eye: [f32; 4],
    sun: [f32; 4],
    fog: [f32; 4],
    weather: [f32; 4],
}

pub(crate) fn sun_direction(elevation_degrees: f32, azimuth_degrees: f32) -> Vec3 {
    let (elevation, azimuth) = (elevation_degrees.to_radians(), azimuth_degrees.to_radians());
    Vec3::new(
        azimuth.sin() * elevation.cos(),
        elevation.sin(),
        azimuth.cos() * elevation.cos(),
    )
}

pub(crate) struct Atmosphere {
    scene: wgpu::TextureView,
    depth: wgpu::TextureView,
    msaa: Option<wgpu::TextureView>,
    composite: wgpu::TextureView,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    blit_group: wgpu::BindGroup,
    clear_air_group: wgpu::BindGroup,
    effects_enabled: std::cell::Cell<bool>,
    pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    scale: f32,
    sample_count: u32,
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    sample_count: u32,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}
fn make_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    source: &str,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
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
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
        cache: None,
    })
}
fn shader_source(samples: u32) -> String {
    let (bindings, body) = if samples > 1 {
        ("@group(0) @binding(1) var scene: texture_multisampled_2d<f32>;\n@group(0) @binding(2) var depth: texture_depth_multisampled_2d;",
         "var color=vec3<f32>(0.0);\nfor (var i=0i;i<4i;i+=1i) { color+=composite(textureLoad(scene,pixel,i).rgb,textureLoad(depth,pixel,i),in.uv); }\nreturn vec4(color*0.25,1.0);")
    } else {
        ("@group(0) @binding(1) var scene: texture_2d<f32>;\n@group(0) @binding(2) var depth: texture_depth_2d;",
         "return vec4(composite(textureLoad(scene,pixel,0).rgb,textureLoad(depth,pixel,0),in.uv),1.0);")
    };
    SHADER
        .replace("// SCENE_BINDINGS", bindings)
        .replace("// SAMPLE_COMPOSITE", body)
}
impl Atmosphere {
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        scale: f32,
        sample_count: u32,
    ) -> Self {
        let scale = scale.clamp(0.5, 1.0);
        let width = ((width as f32 * scale).round() as u32).max(1);
        let height = ((height as f32 * scale).round() as u32).max(1);
        let sample_count = if sample_count == 4 { 4 } else { 1 };
        let scene = texture(device, "scene resolved", format, width, height, 1);
        let msaa = (sample_count > 1)
            .then(|| texture(device, "scene MSAA", format, width, height, sample_count));
        let depth = texture(
            device,
            "atmosphere scene depth",
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
            sample_count,
        );
        let composite = texture(device, "atmosphere composite", format, width, height, 1);
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("atmosphere parameters"),
            contents: bytemuck::bytes_of(&Uniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atmosphere"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: sample_count > 1,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: sample_count > 1,
                    },
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atmosphere"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(msaa.as_ref().unwrap_or(&scene)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&depth),
                },
            ],
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene upscale"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene upscale"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blit_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene upscale"),
            layout: &blit_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&composite),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let clear_air_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene upscale"),
            layout: &blit_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&scene),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let pipeline = make_pipeline(
            device,
            "atmosphere",
            &layout,
            &shader_source(sample_count),
            format,
        );
        let blit_pipeline = make_pipeline(device, "scene upscale", &blit_layout, BLIT, format);
        Self {
            scene,
            depth,
            msaa,
            composite,
            uniform,
            bind_group,
            blit_group,
            clear_air_group,
            effects_enabled: std::cell::Cell::new(true),
            pipeline,
            blit_pipeline,
            format,
            width,
            height,
            scale,
            sample_count,
        }
    }
    pub(crate) fn resize(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        scale: f32,
        sample_count: u32,
    ) {
        let scale = scale.clamp(0.5, 1.0);
        if self.width == ((width as f32 * scale).round() as u32).max(1)
            && self.height == ((height as f32 * scale).round() as u32).max(1)
            && self.scale == scale
            && self.sample_count == sample_count
        {
            return;
        }
        *self = Self::new(device, self.format, width, height, scale, sample_count);
    }
    pub(crate) fn scene_view(&self) -> &wgpu::TextureView {
        &self.scene
    }
    pub(crate) fn depth_view(&self) -> &wgpu::TextureView {
        &self.depth
    }
    pub(crate) fn msaa_view(&self) -> Option<&wgpu::TextureView> {
        self.msaa.as_ref()
    }
    pub(crate) fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    pub(crate) fn prepare(
        &self,
        queue: &wgpu::Queue,
        inverse_view_projection: Mat4,
        relative_eye: Vec3,
        world_eye: DVec3,
        settings: &VisualSettings,
        time_seconds: f32,
    ) {
        self.effects_enabled
            .set(settings.atmosphere || settings.exposure != 1.0);
        let sun = sun_direction(settings.sun_elevation, settings.sun_azimuth);
        let uniform = Uniform {
            inverse_view_projection: inverse_view_projection.to_cols_array_2d(),
            relative_eye: relative_eye.extend(0.0).to_array(),
            world_eye: world_eye.as_vec3().extend(0.0).to_array(),
            sun: sun.extend(0.0).to_array(),
            fog: [
                settings.fog_density.max(0.0),
                settings.fog_height_falloff.max(0.0),
                settings.exposure.max(0.0),
                if settings.atmosphere { 1.0 } else { 0.0 },
            ],
            weather: [
                settings.cloud_coverage.clamp(0.0, 1.0),
                settings.cloud_speed,
                settings.overcast.clamp(0.0, 1.0),
                time_seconds,
            ],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
    }
    pub(crate) fn encode(&self, encoder: &mut wgpu::CommandEncoder, output: &wgpu::TextureView) {
        for (view, pipeline, group, label) in [
            (
                &self.composite,
                &self.pipeline,
                &self.bind_group,
                "atmosphere",
            ),
            (
                output,
                &self.blit_pipeline,
                if self.effects_enabled.get() {
                    &self.blit_group
                } else {
                    &self.clear_air_group
                },
                "scene upscale",
            ),
        ] {
            if label == "atmosphere" && !self.effects_enabled.get() {
                continue;
            }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
use bytemuck::Zeroable;

#[cfg(test)]
mod tests {
    #[test]
    fn atmosphere_shaders_validate() {
        for source in [
            super::shader_source(1),
            super::shader_source(4),
            super::BLIT.to_owned(),
        ] {
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .unwrap();
        }
    }
}
