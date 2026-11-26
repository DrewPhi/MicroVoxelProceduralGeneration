//! Inexpensive, independently switchable sky / height fog / cloud composition.
//! Base aerial perspective uses scene depth. Temporary refinement haze uses published terrain metadata.
use crate::{settings::VisualSettings, Camera};
use glam::{DVec3, Mat4, Vec3};
use wgpu::util::DeviceExt;

const SKY_SHADER: &str = concat!(
    include_str!("atmosphere_sky.wgsl"),
    include_str!("atmosphere_cloud_density.wgsl"),
);
const SHADER: &str = concat!(
    include_str!("atmosphere.wgsl"),
    include_str!("atmosphere_cloud_density.wgsl"),
);
const BLIT: &str = r#"
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@group(0) @binding(2) var<uniform> filter_settings: vec4f;
struct Output { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs_main(@builtin(vertex_index) i:u32)->Output {
 var p=array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
 var o:Output; o.position=vec4(p[i],0.0,1.0); o.uv=vec2(p[i].x*0.5+0.5,0.5-p[i].y*0.5); return o;
}
fn luminance(c:vec3f)->f32 {return sqrt(max(0.,dot(c,vec3f(.2126,.7152,.0722))));}
@fragment fn fs_main(i:Output)->@location(0) vec4<f32> {
    let center=textureSample(image,image_sampler,i.uv);
    if filter_settings.x<=0. {return center;}
    let size=vec2i(textureDimensions(image));
    let pixel=vec2i(i.uv*vec2f(size));
    let n=textureLoad(image,clamp(pixel-vec2i(0,1),vec2i(0),size-1),0).rgb;
    let s=textureLoad(image,clamp(pixel+vec2i(0,1),vec2i(0),size-1),0).rgb;
    let e=textureLoad(image,clamp(pixel+vec2i(1,0),vec2i(0),size-1),0).rgb;
    let w=textureLoad(image,clamp(pixel-vec2i(1,0),vec2i(0),size-1),0).rgb;
    let lum=luminance(center.rgb);
    let outer=vec4f(luminance(n),luminance(s),luminance(e),luminance(w));
    let low=min(lum,min(min(outer.x,outer.y),min(outer.z,outer.w)));
    let high=max(lum,max(max(outer.x,outer.y),max(outer.z,outer.w)));
    let contrast=high-low;
    if contrast<max(.037,high*.16) {return center;}
    let vertical=abs(outer.x-outer.y);
    let horizontal=abs(outer.z-outer.w);
    let neighbor=mix(.5*(e+w),.5*(n+s),smoothstep(-.025,.025,vertical-horizontal));
    let isolation=clamp(abs(luminance(neighbor)-lum)/contrast,0.,1.);
    // Filter small isolated steps once on the final composited image.
    return vec4f(mix(center.rgb,neighbor,.32*filter_settings.x*isolation*isolation),center.a);
}
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
    // Contract v2: beauty enabled, shadowed air enabled, debug mode, reserved.
    beauty: [f32; 4],
    lighting: [f32; 4],
    far_shadow: [[f32; 4]; 4],
    haze_regions: [[f32; 4]; crate::streaming_haze::REGIONS],
    storm_origin: [f32; 4],
    storm_extent: [f32; 4],
    storm_low: [f32; 4],
    storm_high: [f32; 4],
    celestial: [f32; 4],
    moon: [f32; 4],
    regional_origin: [f32; 4],
    regional_extent: [f32; 4],
    cloud_phase: [f32; 4],
}

/// Opt-in shader diagnostics; default presentation and legacy profile are unchanged.
fn parse_atmosphere_debug(value: Option<&str>) -> u32 {
    match value {
        Some("air") => 1,
        Some("visibility") => 2,
        Some("no-air") => 3,
        _ => 0,
    }
}
fn atmosphere_debug() -> u32 {
    static MODE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| {
        parse_atmosphere_debug(std::env::var("FRONTIER_ATMOSPHERE_DEBUG").ok().as_deref())
    })
}
pub(crate) fn sun_direction(elevation_degrees: f32, azimuth_degrees: f32) -> Vec3 {
    let (elevation, azimuth) = (elevation_degrees.to_radians(), azimuth_degrees.to_radians());
    Vec3::new(
        azimuth.sin() * elevation.cos(),
        elevation.sin(),
        azimuth.cos() * elevation.cos(),
    )
}

#[derive(Clone, Copy, Default)]
struct LightingContext {
    shadow_projection: [[f32; 4]; 4],
    shadow_active: bool,
    submerged: bool,
    regions: [[f32; 4]; crate::streaming_haze::REGIONS],
    region_count: usize,
}

pub(crate) struct Atmosphere {
    moon_volume: wgpu::Texture,
    moon_ready: std::cell::Cell<bool>,
    scene: wgpu::TextureView,
    depth: wgpu::TextureView,
    msaa: Option<wgpu::TextureView>,
    composite: wgpu::TextureView,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    storm_composite: wgpu::BindGroup,
    blit_group: wgpu::BindGroup,
    clear_air_group: wgpu::BindGroup,
    effects_enabled: std::cell::Cell<bool>,
    pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    blit_settings: wgpu::Buffer,
    format: wgpu::TextureFormat,
    scene_format: wgpu::TextureFormat,
    sky: wgpu::TextureView,
    storm_sky: wgpu::TextureView,
    compact_storm: std::cell::Cell<bool>,
    sky_bind: wgpu::BindGroup,
    sky_pipeline: wgpu::RenderPipeline,
    cloud_shape: wgpu::Texture,
    cloud_shape_ready: std::cell::Cell<bool>,
    storm_volumes: [wgpu::Texture; 2],
    regional_volumes: [wgpu::Texture; 2],
    regional_stamp: std::cell::Cell<Option<f64>>,
    regional: std::cell::Cell<[[f32; 4]; 2]>,
    storm_stamp: std::cell::Cell<Option<(f64, DVec3)>>,
    storm_phase: std::cell::Cell<Option<f32>>,
    storm: std::cell::Cell<[[f32; 4]; 4]>,
    air: wgpu::TextureView,
    air_bind: wgpu::BindGroup,
    air_pipeline: wgpu::RenderPipeline,
    lighting_context: std::cell::Cell<LightingContext>,
    streaming_haze: std::cell::RefCell<crate::streaming_haze::StreamingHaze>,
    draw_air: std::cell::Cell<bool>,
    peak_haze_regions: std::cell::Cell<usize>,
    beauty_enabled: std::cell::Cell<bool>,
    medium_ground_height: std::cell::Cell<f32>,
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
    make_pipeline_groups(device, label, &[layout], source, format, "fs_main")
}
fn make_pipeline_groups(
    device: &wgpu::Device,
    label: &str,
    layouts: &[&wgpu::BindGroupLayout],
    source: &str,
    format: wgpu::TextureFormat,
    entry: &str,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(crate::voxel_shader(source)),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: layouts,
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
            entry_point: Some(entry),
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
/// Optional bounded spatial experiment; zero leaves the existing output path
/// intact. No temporal history, reprojection or extra scene rays are involved.
pub(crate) fn post_aa_strength() -> f32 {
    static STRENGTH: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *STRENGTH.get_or_init(|| {
        std::env::var("FRONTIER_POST_AA_STRENGTH")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(0.)
            .clamp(0., 1.)
    })
}

fn cloud_thermal_shader(source: String) -> String {
    if crate::woodland_volume::render_option("FRONTIER_THERMAL_CLOUD_DETAIL", true) {
        source.replace(
            "THERMAL_CLOUD_DETAIL:bool=false",
            "THERMAL_CLOUD_DETAIL:bool=true",
        )
    } else {
        source
    }
}

fn shader_source(samples: u32) -> String {
    cloud_thermal_shader(shader_source_with_post_aa(samples, post_aa_strength() > 0.))
}
fn shader_source_with_post_aa(samples: u32, post_aa: bool) -> String {
    let (bindings, body) = if samples > 1 {
        ("@group(0) @binding(1) var scene: texture_multisampled_2d<f32>;\n@group(0) @binding(2) var depth: texture_depth_multisampled_2d;",
         "if atmosphere.beauty.x > 0.5 && atmosphere.fog.w > 0.5 {\nlet all_sky = min(min(textureLoad(depth,pixel,0),textureLoad(depth,pixel,1)),min(textureLoad(depth,pixel,2),textureLoad(depth,pixel,3))) >= 1.0;\nif all_sky { return vec4(smooth_horizon(pixel,in.uv,composite(vec4(0.0),1.0,in.uv)),1.0); }\n}\nvar color=vec3<f32>(0.0);\nfor (var i=0i;i<4i;i+=1i) { color+=composite(textureLoad(scene,pixel,i),textureLoad(depth,pixel,i),in.uv); }\nreturn vec4(smooth_horizon(pixel,in.uv,color*0.25),1.0);")
    } else {
        ("@group(0) @binding(1) var scene: texture_2d<f32>;\n@group(0) @binding(2) var depth: texture_depth_2d;",
         "return vec4(smooth_horizon(pixel,in.uv,composite(textureLoad(scene,pixel,0),textureLoad(depth,pixel,0),in.uv)),1.0);")
    };
    let mut source = SHADER.replace("// SCENE_BINDINGS", bindings);
    if !post_aa {
        // Representative depth is sufficient for the distant rolling pass,
        // which currently shades at the pixel centre even with MSAA enabled.
        source.push_str("\nfn horizon_depth(p:vec2i)->f32 {return textureLoad(depth,p,0); }\nfn horizon_color(p:vec2i)->vec4f {return textureLoad(scene,p,0); }\n");
        source.push_str(include_str!("horizon_aa.wgsl"));
    }

    if post_aa {
        // Composite per sample before spatial filtering, matching the ordinary
        // MSAA resolve and preserving fog/depth at each neighboring location.
        let load = if samples > 1 {
            "var color=vec3<f32>(0.); for(var s=0i;s<4i;s+=1i) { color+=composite(textureLoad(scene,p,s),textureLoad(depth,p,s),uv); } return color*.25;"
        } else {
            "return composite(textureLoad(scene,p,0),textureLoad(depth,p,0),uv);"
        };
        source.push_str(&format!("\nfn aa_color(p:vec2<i32>)->vec3<f32> {{ let uv=(vec2<f32>(p)+.5)/vec2<f32>(textureDimensions(scene)); {load} }}\n"));
        source = source.replace(
            "// SAMPLE_COMPOSITE",
            "return vec4(edge_post_aa(pixel),1.);",
        );
        source.push_str(include_str!("edge_post_aa.wgsl"));
        source
    } else {
        source.replace("// SAMPLE_COMPOSITE", body)
    }
}
fn sky_shader_source(samples: u32) -> String {
    let (binding, load) = if samples > 1 {
        ("@group(0) @binding(1) var scene_depth:texture_depth_multisampled_2d;",
         "min(min(textureLoad(scene_depth,pixel,0),textureLoad(scene_depth,pixel,1)),min(textureLoad(scene_depth,pixel,2),textureLoad(scene_depth,pixel,3)))")
    } else {
        (
            "@group(0) @binding(1) var scene_depth:texture_depth_2d;",
            "textureLoad(scene_depth,pixel,0)",
        )
    };
    cloud_thermal_shader(
        SKY_SHADER
            .replace("// STORM_DEPTH_BINDING", binding)
            .replace("STORM_DEPTH_LOAD", load),
    )
}

fn air_shader_source(samples: u32) -> String {
    let (binding, load) = if samples > 1 {
        ("@group(0) @binding(2) var depth:texture_depth_multisampled_2d;","min(min(textureLoad(depth,pixel,0),textureLoad(depth,pixel,1)),min(textureLoad(depth,pixel,2),textureLoad(depth,pixel,3)))")
    } else {
        (
            "@group(0) @binding(2) var depth:texture_depth_2d;",
            "textureLoad(depth,pixel,0)",
        )
    };
    let source = concat!(
        "const NAADF_GROUP:u32=1u;const NAADF_BASE:u32=3u;\n",
        include_str!("naadf/trace.wgsl"),
        include_str!("ground_light.wgsl"),
        include_str!("atmosphere_air.wgsl"),
    );
    source
        .replace("// DEPTH_BINDING", binding)
        .replace("DEPTH_LOAD", load)
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
        Self::new_with_scene_format(device, format, width, height, scale, sample_count, format)
    }
    pub(crate) fn new_with_scene_format(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        scale: f32,
        sample_count: u32,
        scene_format: wgpu::TextureFormat,
    ) -> Self {
        let scale = scale.clamp(0.5, 1.0);
        let width = ((width as f32 * scale).round() as u32).max(1);
        let height = ((height as f32 * scale).round() as u32).max(1);
        let sample_count = if sample_count == 4 { 4 } else { 1 };
        let scene = texture(device, "scene resolved", scene_format, width, height, 1);
        let msaa = (sample_count > 1).then(|| {
            texture(
                device,
                "scene MSAA",
                scene_format,
                width,
                height,
                sample_count,
            )
        });
        let depth = texture(
            device,
            "atmosphere scene depth",
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
            sample_count,
        );
        let sky = texture(
            device,
            "half resolution volumetric clouds",
            wgpu::TextureFormat::Rgba16Float,
            width.div_ceil(2),
            height.div_ceil(2),
            1,
        );
        let storm_sky = texture(
            device,
            "quarter resolution simulated weather",
            wgpu::TextureFormat::Rgba16Float,
            width.div_ceil(4),
            height.div_ceil(4),
            1,
        );
        let air = texture(
            device,
            "quarter resolution shadowed world air",
            wgpu::TextureFormat::Rgba16Float,
            width.div_ceil(4),
            height.div_ceil(4),
            1,
        );
        let composite = texture(device, "atmosphere composite", format, width, height, 1);
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("atmosphere parameters"),
            contents: bytemuck::bytes_of(&Uniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear atmosphere sampling"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let moon_size = crate::celestial::MOON_SIZE;
        let moon_volume = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lunar surface voxels"),
            size: wgpu::Extent3d {
                width: moon_size,
                height: moon_size,
                depth_or_array_layers: moon_size,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let moon_view = moon_volume.create_view(&Default::default());
        let weather_size = frontier_runtime::weather::SIZE;
        let storm_volumes = std::array::from_fn(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("simulated cloud condensate and precipitation"),
                size: wgpu::Extent3d {
                    width: weather_size[0] as u32,
                    height: weather_size[1] as u32,
                    depth_or_array_layers: weather_size[2] as u32,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rg8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        });
        let storm_views = storm_volumes
            .each_ref()
            .map(|volume| volume.create_view(&Default::default()));
        let regional_size = frontier_runtime::weather::regional::SIZE;
        let regional_volumes = std::array::from_fn(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("regional cloud condensate and precipitation"),
                size: wgpu::Extent3d {
                    width: regional_size[0] as u32,
                    height: regional_size[1] as u32,
                    depth_or_array_layers: regional_size[2] as u32,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rg8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        });
        let regional_views = regional_volumes
            .each_ref()
            .map(|volume| volume.create_view(&Default::default()));
        let volumetric_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            count: None,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
        };
        let shape_side = crate::cloud_shape::SIDE;
        let cloud_shape = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("periodic cloud density detail"),
            size: wgpu::Extent3d {
                width: shape_side,
                height: shape_side,
                depth_or_array_layers: shape_side,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let cloud_shape_view = cloud_shape.create_view(&Default::default());
        let cloud_shape_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("periodic cloud density sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shape_sampler_layout = wgpu::BindGroupLayoutEntry {
            binding: 12,
            visibility: wgpu::ShaderStages::FRAGMENT,
            count: None,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        };
        let shape_bindings = [
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::TextureView(&cloud_shape_view),
            },
            wgpu::BindGroupEntry {
                binding: 12,
                resource: wgpu::BindingResource::Sampler(&cloud_shape_sampler),
            },
        ];
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
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                volumetric_entry(7),
                volumetric_entry(8),
                volumetric_entry(9),
                volumetric_entry(10),
                volumetric_entry(11),
                shape_sampler_layout,
            ],
        });
        let compose = |clouds: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("atmosphere"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            msaa.as_ref().unwrap_or(&scene),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&air),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(clouds),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&moon_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&storm_views[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(&storm_views[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: wgpu::BindingResource::TextureView(&regional_views[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: wgpu::BindingResource::TextureView(&regional_views[1]),
                    },
                    shape_bindings[0].clone(),
                    shape_bindings[1].clone(),
                ],
            })
        };
        let bind_group = compose(&sky);
        let storm_composite = compose(&storm_sky);
        let sky_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky inputs"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    count: None,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    count: None,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: sample_count > 1,
                    },
                },
                volumetric_entry(2),
                volumetric_entry(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    count: None,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                },
                volumetric_entry(5),
                volumetric_entry(6),
                volumetric_entry(11),
                shape_sampler_layout,
            ],
        });
        let sky_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky inputs"),
            layout: &sky_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&depth),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&storm_views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&storm_views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&regional_views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&regional_views[1]),
                },
                shape_bindings[0].clone(),
                shape_bindings[1].clone(),
            ],
        });
        let sky_pipeline = make_pipeline(
            device,
            "half resolution volumetric clouds",
            &sky_layout,
            &sky_shader_source(sample_count),
            wgpu::TextureFormat::Rgba16Float,
        );
        let air_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("air inputs"),
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
        let air_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("air inputs"),
            layout: &air_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&depth),
                },
            ],
        });
        let near_layout = crate::adaptive_gpu::adaptive_shadow::near_sampling_layout(device);
        let far_layout = crate::adaptive_gpu::adaptive_shadow::sampling_layout(device);
        let air_source = air_shader_source(sample_count);
        let air_pipeline = make_pipeline_groups(
            device,
            "shadowed world air",
            &[&air_layout, &near_layout, &far_layout],
            &air_source,
            wgpu::TextureFormat::Rgba16Float,
            "fs_main",
        );
        let blit_settings = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewport edge filter strength"),
            contents: bytemuck::bytes_of(&[0f32; 4]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
            ],
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
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: blit_settings.as_entire_binding(),
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
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: blit_settings.as_entire_binding(),
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
            moon_volume,
            moon_ready: std::cell::Cell::new(false),
            sky,
            storm_sky,
            compact_storm: Default::default(),
            storm_composite,
            sky_bind,
            sky_pipeline,
            cloud_shape,
            cloud_shape_ready: Default::default(),
            storm_volumes,
            regional_volumes,
            regional_stamp: Default::default(),
            regional: Default::default(),
            storm_stamp: Default::default(),
            storm_phase: Default::default(),
            storm: Default::default(),
            air,
            air_bind,
            air_pipeline,
            lighting_context: Default::default(),
            streaming_haze: Default::default(),
            draw_air: Default::default(),
            peak_haze_regions: Default::default(),
            scene_format,
            beauty_enabled: std::cell::Cell::new(false),
            medium_ground_height: std::cell::Cell::new(0.0),
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
            blit_settings,
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
        *self = Self::new_with_scene_format(
            device,
            self.format,
            width,
            height,
            scale,
            sample_count,
            self.scene_format,
        );
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
    /// Logical texel storage for every texture owned by this pass, including
    /// resolved colour, optional MSAA colour, sampled depth, composite, the two
    /// half-resolution RGBA16F lighting targets and disabled 1x1 shadow depth.
    /// Driver alignment/metadata and the caller's output target are excluded.
    pub(crate) fn attachment_bytes(&self) -> u64 {
        let pixels = u64::from(self.width) * u64::from(self.height);
        let half_pixels = u64::from(self.width.div_ceil(2)) * u64::from(self.height.div_ceil(2));
        let scene_bpp = u64::from(
            self.scene_format
                .block_copy_size(None)
                .expect("uncompressed scene colour format"),
        );
        let composite_bpp = u64::from(
            self.format
                .block_copy_size(None)
                .expect("uncompressed composite colour format"),
        );
        let samples = u64::from(self.sample_count);
        let scene_samples = 1 + if self.msaa.is_some() { samples } else { 0 };
        pixels * (scene_bpp * scene_samples + 4 * samples + composite_bpp)
            + half_pixels * 8
            + u64::from(self.width.div_ceil(4)) * u64::from(self.height.div_ceil(4)) * 16
            + frontier_runtime::weather::SIZE
                .into_iter()
                .product::<usize>() as u64
                * 4
            + frontier_runtime::weather::regional::SIZE
                .into_iter()
                .product::<usize>() as u64
                * 4
            + 4
            + crate::cloud_shape::BYTES
    }
    pub(crate) fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    /// Local terrain-plane height for meadow mist. Supplying an absolute
    /// height keeps the participating medium near land when the camera flies.
    pub(crate) fn set_medium_ground_height(&self, height_m: f32) {
        self.medium_ground_height
            .set(if height_m.is_finite() { height_m } else { 0.0 });
    }
    pub(crate) fn set_lighting_context(
        &self,
        camera: &Camera,
        shadows: Option<&crate::adaptive_gpu::AdaptiveGpu>,
        terrain: Option<&crate::adaptive_view::AdaptiveView>,
        submerged: bool,
        time: f32,
    ) {
        let issues = terrain
            .filter(|v| v.pending_count > 0 && !v.quality_ready)
            .map(|v| (v.stats.current_offenders.as_slice(), v.pixel_error));
        let (regions, region_count) =
            self.streaming_haze
                .borrow_mut()
                .prepare(issues, camera.position, time);
        self.lighting_context.set(LightingContext {
            shadow_projection: shadows
                .map_or(glam::Mat4::IDENTITY, |a| a.far_shadow_projection(camera))
                .to_cols_array_2d(),
            shadow_active: shadows.is_some_and(|a| a.near_shadows_active()),
            submerged,
            regions,
            region_count,
        });
    }

    pub(crate) fn peak_haze_regions(&self) -> usize {
        self.peak_haze_regions.get()
    }

    pub(crate) fn set_world_weather(
        &self,
        queue: &wgpu::Queue,
        frame: Option<&crate::world_weather::Frame>,
        eye: DVec3,
        fraction: f32,
        compact: bool,
    ) {
        self.regional.set([[0.; 4]; 2]);
        let Some(frame) = frame else {
            self.compact_storm.set(false);
            self.storm.set([[0.; 4]; 4]);
            self.storm_phase.set(None);
            return;
        };
        // Cloud-top relief benefits from the half-resolution target when
        // seen from the air. Keep quarter resolution for ground and in-cloud
        // views, and retain the current target across small altitude changes.
        let top = f64::from(
            frame
                .current
                .metrics
                .cloud_top_m
                .max(frame.previous.metrics.cloud_top_m),
        );
        let altitude_ratio = if self.compact_storm.get() { 0.92 } else { 0.84 };
        let view_of_deck = frame.current.regional.is_some()
            && top > 0.
            && eye.y >= (top * altitude_ratio).max(2000.)
            && eye.y <= top + 5000.;
        self.compact_storm.set(compact && !view_of_deck);
        let seconds =
            frame.previous.time + (frame.current.time - frame.previous.time) * f64::from(fraction);
        self.storm_phase
            .set(Some((seconds * 1.55).rem_euclid(1024.) as f32));
        use frontier_runtime::weather::{EXTENT, SIZE};
        let stamp = (frame.current.time, frame.current.origin);
        if self.storm_stamp.get() != Some(stamp) {
            let extent = wgpu::Extent3d {
                width: SIZE[0] as u32,
                height: SIZE[1] as u32,
                depth_or_array_layers: SIZE[2] as u32,
            };
            for (volume, weather) in self.storm_volumes.iter().zip(&frame.packed_volume) {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: volume,
                        mip_level: 0,
                        origin: Default::default(),
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytemuck::cast_slice(weather),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(extent.width * 2),
                        rows_per_image: Some(extent.height),
                    },
                    extent,
                );
            }
            self.storm_stamp.set(Some(stamp));
        }
        if let Some(regional) = &frame.current.regional {
            use frontier_runtime::weather::regional;
            let extent = wgpu::Extent3d {
                width: regional::SIZE[0] as u32,
                height: regional::SIZE[1] as u32,
                depth_or_array_layers: regional::SIZE[2] as u32,
            };
            let visible = regional.current.weather_visible(eye, 120_000.)
                || regional.previous.weather_visible(eye, 120_000.);
            if visible && self.regional_stamp.get() != Some(regional.current.time) {
                let states = [&regional.previous, &regional.current];
                for (volume, state) in self.regional_volumes.iter().zip(states) {
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: volume,
                            mip_level: 0,
                            origin: Default::default(),
                            aspect: wgpu::TextureAspect::All,
                        },
                        bytemuck::cast_slice(state.volume()),
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(extent.width * 2),
                            rows_per_image: Some(extent.height),
                        },
                        extent,
                    );
                }
                self.regional_stamp.set(Some(regional.current.time));
            }
            self.regional.set([
                regional
                    .current
                    .origin
                    .as_vec3()
                    .extend(regional.fraction(seconds))
                    .to_array(),
                [
                    extent.width as f32 * regional::SPACING,
                    extent.height as f32 * regional::LAYER,
                    extent.depth_or_array_layers as f32 * regional::SPACING,
                    if visible { 2. } else { 1. },
                ],
            ]);
        }
        let bounds = match (frame.previous.wet_bounds, frame.current.wet_bounds) {
            (Some((a, b)), Some((c, d))) => Some((
                Vec3::from(a).min(Vec3::from(c)),
                Vec3::from(b).max(Vec3::from(d)),
            )),
            (Some((a, b)), None) | (None, Some((a, b))) => Some((a.into(), b.into())),
            _ => None,
        };
        let (min, max, wet) = bounds.map_or((Vec3::ZERO, Vec3::ZERO, 0.), |(lo, hi)| (lo, hi, 1.));
        let local_rain = frame.previous.sample(eye).rain_rate_mm_h() * (1. - fraction)
            + frame.current.sample(eye).rain_rate_mm_h() * fraction;
        self.storm.set([
            frame
                .current
                .origin
                .as_vec3()
                .extend(if compact && view_of_deck {
                    3.
                } else if compact {
                    2.
                } else {
                    1.
                })
                .to_array(),
            EXTENT.extend(local_rain).to_array(),
            min.extend(fraction).to_array(),
            max.extend(wet).to_array(),
        ]);
    }
    pub(crate) fn prepare(
        &self,
        queue: &wgpu::Queue,
        inverse_view_projection: Mat4,
        relative_eye: Vec3,
        world_eye: DVec3,
        settings: &VisualSettings,
        time_seconds: f32,
        rolling_horizon: f32,
    ) {
        self.effects_enabled.set(
            settings.atmosphere
                || settings.exposure != 1.0
                || self.scene_format != self.format
                || post_aa_strength() > 0.,
        );
        let [storm_origin, storm_extent, storm_low, storm_high] = self.storm.get();
        let [regional_origin, regional_extent] = self.regional.get();
        let physical_weather = storm_origin[3] > 0.5;
        let beauty = (settings.beauty || physical_weather || settings.celestial.is_some())
            && settings.atmosphere;
        self.beauty_enabled.set(beauty);
        let shaped_clouds =
            settings.natural_terrain_light || (physical_weather && regional_extent[3] > 0.5);
        if beauty && shaped_clouds && !self.cloud_shape_ready.replace(true) {
            let side = crate::cloud_shape::SIDE;
            queue.write_texture(
                self.cloud_shape.as_image_copy(),
                crate::cloud_shape::samples(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * side),
                    rows_per_image: Some(side),
                },
                self.cloud_shape.size(),
            );
        }
        let sun = settings.celestial.map_or_else(
            || sun_direction(settings.sun_elevation, settings.sun_azimuth),
            |light| light.sun,
        );
        let (celestial, moon) = settings
            .celestial
            .map_or(([1., 0., 0., 0.], [0.; 4]), |light| {
                if !self.moon_ready.replace(true) {
                    let size = crate::celestial::MOON_SIZE;
                    queue.write_texture(
                        self.moon_volume.as_image_copy(),
                        crate::celestial::moon_voxels(),
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(size),
                            rows_per_image: Some(size),
                        },
                        self.moon_volume.size(),
                    );
                }
                (
                    [light.daylight, light.twilight, light.moonlight, 1.],
                    light.moon.extend(0.0175).to_array(),
                )
            });
        let context = self.lighting_context.get();
        let beams = settings.beauty
            && settings.atmosphere
            && settings.terrain_shadows
            && !settings.visibility_benchmark
            && settings.fog_density > 0.
            && if settings.celestial.is_some() {
                sun.y > 0.05
            } else {
                settings.sun_elevation > 0.
            }
            && context.shadow_active
            && !context.submerged
            && atmosphere_debug() != 3
            && (settings.sunbeams > 0. || matches!(atmosphere_debug(), 1 | 2));
        self.draw_air.set(beams);
        let haze_strength = if settings.beauty
            && settings.atmosphere
            && !settings.visibility_benchmark
            && !context.submerged
        {
            settings.streaming_haze
        } else {
            0.
        };
        if haze_strength > 0. {
            self.peak_haze_regions
                .set(self.peak_haze_regions.get().max(context.region_count));
        }
        let uniform = Uniform {
            inverse_view_projection: inverse_view_projection.to_cols_array_2d(),
            relative_eye: relative_eye
                .extend(f32::from(
                    rolling_horizon > 0. && settings.terrain_shadows && !context.submerged,
                ))
                .to_array(),
            world_eye: world_eye
                .as_vec3()
                .extend(self.medium_ground_height.get())
                .to_array(),
            sun: sun
                .extend(f32::from(settings.natural_terrain_light))
                .to_array(),
            fog: [
                settings.fog_density.max(0.0),
                settings.fog_height_falloff.max(0.0),
                settings.exposure.max(0.0),
                if settings.atmosphere { 1.0 } else { 0.0 },
            ],
            weather: [
                settings.cloud_coverage.clamp(0.0, 1.0),
                settings.cloud_speed,
                if physical_weather {
                    (storm_extent[3] / 28.).min(0.75)
                } else {
                    settings.overcast.clamp(0.0, 1.0)
                },
                self.storm_phase.get().unwrap_or(time_seconds),
            ],
            beauty: [
                f32::from(beauty),
                if beams { 1. } else { 0. },
                atmosphere_debug() as f32,
                post_aa_strength(),
            ],
            lighting: [
                settings.sunbeams,
                haze_strength,
                context.region_count as f32,
                if settings.atmosphere
                    && settings.fog_density > 0.
                    && !settings.visibility_benchmark
                    && !context.submerged
                {
                    rolling_horizon
                } else {
                    0.
                },
            ],
            far_shadow: context.shadow_projection,
            haze_regions: context.regions,
            storm_origin,
            storm_extent,
            storm_low,
            storm_high,
            celestial,
            moon,
            regional_origin,
            regional_extent,
            cloud_phase: settings.cloud_phase,
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        queue.write_buffer(
            &self.blit_settings,
            0,
            bytemuck::bytes_of(&[settings.edge_aa, 0., 0., 0.]),
        );
    }
    pub(crate) fn encode(&self, encoder: &mut wgpu::CommandEncoder, output: &wgpu::TextureView) {
        self.encode_with_shadows(encoder, output, None);
    }
    pub(crate) fn encode_with_shadows(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        shadows: Option<&crate::adaptive_gpu::AdaptiveGpu>,
    ) {
        if self.beauty_enabled.get() {
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("half resolution volumetric clouds"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: if self.compact_storm.get() {
                            &self.storm_sky
                        } else {
                            &self.sky
                        },
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
                pass.set_pipeline(&self.sky_pipeline);
                pass.set_bind_group(0, &self.sky_bind, &[]);
                pass.draw(0..3, 0..1);
            }
            if let Some(shadows) = shadows.filter(|_| self.draw_air.get()) {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("quarter resolution shadowed world air"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.air,
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
                pass.set_pipeline(&self.air_pipeline);
                pass.set_bind_group(0, &self.air_bind, &[]);
                pass.set_bind_group(1, shadows.near_shadow_bind(), &[]);
                pass.set_bind_group(2, shadows.far_shadow_bind(), &[]);
                pass.draw(0..3, 0..1);
            }
        }
        for (view, pipeline, group, label) in [
            (
                &self.composite,
                &self.pipeline,
                if self.compact_storm.get() {
                    &self.storm_composite
                } else {
                    &self.bind_group
                },
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
    fn atmosphere_debug_modes_parse_without_environment_mutation() {
        for (value, expected) in [
            (None, 0),
            (Some("air"), 1),
            (Some("visibility"), 2),
            (Some("no-air"), 3),
            (Some("normal"), 0),
            (Some(""), 0),
            (Some("AIR"), 0),
        ] {
            assert_eq!(super::parse_atmosphere_debug(value), expected);
        }
    }
    #[test]
    fn atmosphere_uniform_contract() {
        assert_eq!(std::mem::size_of::<super::Uniform>(), 512);
        assert_eq!(std::mem::offset_of!(super::Uniform, beauty), 144);
        let module =
            naga::front::wgsl::parse_str(&crate::voxel_shader(super::shader_source(1))).unwrap();
        let (_, uniform) = module
            .types
            .iter()
            .find(|(_, ty)| ty.name.as_deref() == Some("AtmosphereUniform"))
            .unwrap();
        let naga::TypeInner::Struct { members, span } = &uniform.inner else {
            panic!("atmosphere uniform must be a struct");
        };
        assert_eq!(*span as usize, std::mem::size_of::<super::Uniform>());
        assert_eq!(
            members.last().unwrap().offset as usize,
            std::mem::offset_of!(super::Uniform, cloud_phase)
        );
    }

    #[test]
    fn atmosphere_shaders_validate() {
        for source in [
            super::shader_source(1),
            super::shader_source(4),
            super::shader_source_with_post_aa(1, true),
            super::shader_source_with_post_aa(4, true),
            super::BLIT.to_owned(),
            super::air_shader_source(1),
            super::air_shader_source(4),
            super::sky_shader_source(1),
            super::sky_shader_source(4),
        ] {
            let source = crate::voxel_shader(source);
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
