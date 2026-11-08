//! Greenfield native fixture and experimental moving FBM surface viewer.
mod adaptive_geometry;
mod adaptive_gpu;
mod adaptive_stream;
mod adaptive_view;
mod appearance_hierarchy;
mod appearance_map;
mod appearance_method;
mod appearance_visibility;
mod atmosphere;
pub mod brick_ray;
mod build_memory;
mod cold_source_cache;
pub mod column_mesh;
mod derived_disk_cache;
mod derived_disk_writeback;
pub mod gpu_ordered_profiles;
mod gpu_probe;
pub mod gpu_summary_probe;
mod hybrid;
mod hybrid_gpu;
mod integrated_profiles;
pub mod mesh;
mod native_fbm;
mod native_stream;
mod options;
mod page_terrain;
mod playability;
mod settings;
mod settings_ui;
#[cfg(test)]
mod source_cache_probe;
mod source_page_cache;
mod spatial_appearance;
mod surface_compare;
mod terrain;
mod vegetation;
mod vegetation_gpu;
mod vegetation_mesher;
mod vegetation_scene;
mod viewer_visual;
pub mod volume;
mod walking;

use frontier_world::{Cell, Material, World, VOXEL_SIZE};
use glam::{DVec3, Mat4, Vec3};
use std::{
    collections::HashSet,
    error::Error,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use wgpu::util::DeviceExt;
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

pub const SEED: u64 = 7;
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const SHADER: &str = include_str!("voxel.wgsl");
type Result<T> = std::result::Result<T, Box<dyn Error>>;
/// Renderer shader/filter contract; independent of canonical source appearance.
const ADAPTIVE_RENDER_APPEARANCE_VERSION: u32 = 2;

fn renderer_appearance_json(world: &World, adaptive: bool) -> String {
    if !adaptive {
        return "\"render_appearance_version\":1".into();
    }
    let mode = adaptive_geometry::appearance_mode(world);
    let version = if appearance_method::debug_view() != 0 {
        5
    } else if adaptive_geometry::near_profile_shared()
        || appearance_method::near_blend() != 0
        || appearance_map::effective_filter_radius(16) > 0
        || appearance_method::map_anisotropy() > 1
    {
        6
    } else if appearance_method::voxel_grain().is_some() {
        4
    } else if appearance_method::selected() == appearance_method::Method::Baseline {
        ADAPTIVE_RENDER_APPEARANCE_VERSION
    } else {
        3
    };
    format!(
        "\"render_appearance_version\":{version},\"adaptive_appearance\":{{\"spatial_profiles\":{},\"profile_probe_cells\":{},\"profile_footprint\":\"{}\",\"near_detail\":{},\"method\":\"{}\",\"map_intervals\":{},\"visibility_grid\":{},\"visibility_spatial\":{},\"voxel_grain_amplitude\":{},\"debug_view\":{},\"near_profile_shared\":{},\"near_blend\":{},\"map_prefilter_radius_cells\":{},\"map_anisotropy_taps\":{},\"map_prefilter_mode\":{:?},\"map_filter_soft_surfaces\":{}}}",
        mode.spatial_profiles,
        mode.probe_cells,
        if mode.half_footprint { "half" } else { "full" },
        adaptive_stream::near_detail_setting(world),
        appearance_method::selected().name(),
        appearance_map::map_intervals(),
        appearance_visibility::grid_side(),
        appearance_visibility::spatial_enabled(),
        appearance_method::voxel_grain().unwrap_or(0.055),
        appearance_method::debug_view(),
        adaptive_geometry::near_profile_shared(),
        appearance_method::near_blend(),
        appearance_map::effective_filter_radius(16),
        appearance_method::map_anisotropy(),
        match appearance_map::map_prefilter() {appearance_map::MapPrefilter::Off=>"off",appearance_map::MapPrefilter::NativeBox{..}=>"native-box",appearance_map::MapPrefilter::Footprint=>"footprint"},
        appearance_method::map_filter_soft_surfaces(),
    )
}

/// CPU command encoding only; GPU execution is measured separately.
struct EncodeTimer<'a>(&'a std::cell::Cell<f64>, &'a std::cell::Cell<f64>, Instant);
impl Drop for EncodeTimer<'_> {
    fn drop(&mut self) {
        let ms = self.2.elapsed().as_secs_f64() * 1000.;
        self.0.set(self.0.get() + ms);
        self.1.set(ms);
    }
}

/// Pair the preceding presented frame's CPU draw work with its frame interval.
/// Captures can invoke Gpu::draw independently, so retain a value in the viewer
/// rather than reading the GPU's last duration when the next interval arrives.
struct DrawEncodeFrames {
    pending_ms: Option<f64>,
    encode: frontier_runtime::FrameStats,
    frame: frontier_runtime::FrameStats,
}
impl DrawEncodeFrames {
    fn new() -> Self {
        Self {
            pending_ms: None,
            encode: frontier_runtime::FrameStats::new(10000),
            frame: frontier_runtime::FrameStats::new(10000),
        }
    }
    fn record_interval(&mut self, measured: bool, frame_ms: f64) {
        if let Some(encode_ms) = self.pending_ms.take() {
            if measured
                && frame_ms.is_finite()
                && frame_ms >= 0.
                && encode_ms.is_finite()
                && encode_ms >= 0.
            {
                self.encode.record(encode_ms);
                self.frame.record(frame_ms);
            }
        }
    }
    fn json(&self) -> String {
        let encode = self.encode.summary();
        let frame = self.frame.summary();
        let number = |x: Option<f64>| x.map_or("null".into(), |v| v.to_string());
        format!(
            "{{\"samples\":{},\"accepted_frames\":{},\"p50_ms\":{},\"p95_ms\":{},\"p99_ms\":{},\"includes_warmup_capture\":false,\"scope\":\"Gpu::draw wall time: queue writes, optional summary dispatch encoding, render pass begin/end, bundle replay or direct draw encoding; excludes adaptive.prepare, command finish/submit, present, atmosphere and UI\",\"sample_policy\":\"preceding presented draw paired with the next accepted frame interval after route start and warmup; bounded rolling 10000 samples\",\"matched_frame_interval\":{{\"samples\":{},\"accepted_frames\":{},\"p50_ms\":{},\"p95_ms\":{},\"p99_ms\":{}}},\"gpu_samples_paired\":false,\"gpu_scope\":\"gpu_render_pass is a separately counted asynchronous terrain-pass subset; not whole-frame GPU time\"}}",
            encode.sample_count, encode.accepted_frames,
            number(encode.p50_ms), number(encode.p95_ms), number(encode.p99_ms),
            frame.sample_count, frame.accepted_frames,
            number(frame.p50_ms), number(frame.p95_ms), number(frame.p99_ms),
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Camera {
    position: DVec3,
    yaw: f32,
    pitch: f32,
    far: f32,
    fov_y: f32,
}
impl Camera {
    fn fixture() -> Self {
        let position = DVec3::new(11., 9., -15.);
        let direction = (DVec3::new(-1., 3., 1.) - position).normalize();
        Self {
            position,
            far: 20000.,
            fov_y: 60.,
            yaw: direction.x.atan2(direction.z) as f32,
            pitch: direction.y.asin() as f32,
        }
    }
    /// Headless comparisons must apply the requested eye before planning coverage.
    fn set_explicit_pose(&mut self, eye: Option<[f64; 3]>, target: Option<[f64; 3]>) {
        if let (Some(eye), Some(target)) = (eye, target) {
            self.position = DVec3::from_array(eye);
            let direction = (DVec3::from_array(target) - self.position).normalize();
            self.yaw = direction.x.atan2(direction.z) as f32;
            self.pitch = direction.y.asin() as f32;
        }
    }
    fn direction(&self) -> Vec3 {
        Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.cos() * self.pitch.cos(),
        )
    }
    fn matrix(&self, origin: DVec3, aspect: f32) -> Mat4 {
        let eye = (self.position - origin).as_vec3();
        Mat4::perspective_rh(self.fov_y.to_radians(), aspect, 0.025, self.far)
            * Mat4::look_to_rh(eye, self.direction(), Vec3::Y)
    }
}

fn brick_skip_mode(renderer: &str) -> u32 {
    match renderer {
        "brick-skip" => 1,
        "brick-directional" => 2,
        _ => 0,
    }
}

#[derive(Default)]
struct BrickCacheMetrics {
    builds: u64,
    latest_build_ms: f64,
    total_build_ms: f64,
    latest_gpu_bytes: usize,
    uploaded_bytes: u64,
    latest_scratch_bytes: usize,
    peak_scratch_bytes: usize,
}

struct Gpu {
    visual_settings: Option<settings::VisualSettings>,
    regional_appearance: bool,
    alpine_appearance: bool,
    terrain_amplitude: f32,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    native_layout: wgpu::BindGroupLayout,
    hybrid_pipeline: wgpu::RenderPipeline,
    hybrid: Option<hybrid_gpu::HybridGpu>,
    pages: Option<page_terrain::PageGpu>,
    adaptive: Option<adaptive_gpu::AdaptiveGpu>,
    vegetation: Option<vegetation_gpu::VegetationGpu>,
    vegetation_stream: Option<vegetation::VegetationStream>,
    vegetation_stats: vegetation::VegetationStats,
    vegetation_revision: u64,
    vegetation_samples: u32,
    summary_contention: Option<gpu_summary_probe::GpuSummaryProbe>,
    summary_contention_startup_ms: f64,
    draw_encode_cpu_ms: std::cell::Cell<f64>,
    last_draw_encode_cpu_ms: std::cell::Cell<f64>,
    draw_encode_calls: std::cell::Cell<u64>,
    summary_contention_dispatches: std::cell::Cell<u64>,
    volume_owner: bool,
    brick: Option<brick_ray::BrickRayGpu>,
    brick_skip_mode: u32,
    brick_cache_metrics: BrickCacheMetrics,
    surface_probe: Option<surface_compare::SurfaceCompareGpu>,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    origin: DVec3,
    format: wgpu::TextureFormat,
    /// Linear scene target; output/UI/capture retain `format`.
    scene_format: wgpu::TextureFormat,
    medium_ground_height: f32,
    sample_count: u32,
    voxel_filter: bool,
    terrain: Option<terrain::TerrainGpu>,
    adapter_info: String,
}
impl Gpu {
    async fn new(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        format: Option<wgpu::TextureFormat>,
        mesh: &mesh::Mesh,
        sample_count: u32,
        voxel_filter: bool,
        hdr_scene: bool,
    ) -> Result<Self> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: surface,
                force_fallback_adapter: false,
            })
            .await
            .ok_or("no compatible GPU adapter (try Vulkan software ICD for headless capture)")?;
        eprintln!("adapter: {:?}", adapter.get_info());
        let output_format = format.unwrap_or_else(|| {
            surface
                .map(|s| {
                    s.get_capabilities(&adapter)
                        .formats
                        .into_iter()
                        .find(|f| f.is_srgb())
                        .unwrap_or(wgpu::TextureFormat::Bgra8UnormSrgb)
                })
                .unwrap_or(wgpu::TextureFormat::Rgba8UnormSrgb)
        });
        let format = if hdr_scene && output_format != wgpu::TextureFormat::Rgba32Float {
            wgpu::TextureFormat::Rgba16Float
        } else {
            output_format
        };
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("Frontier device"),
                    required_features: adapter.features()
                        & (wgpu::Features::TIMESTAMP_QUERY
                            | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
                            | wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                },
                None,
            )
            .await?;
        let vertex_bytes = mesh.vertices.len() as u64 * std::mem::size_of::<mesh::Vertex>() as u64;
        let index_bytes = mesh.indices.len() as u64 * std::mem::size_of::<u32>() as u64;
        let limit = device.limits().max_buffer_size;
        if vertex_bytes > limit || index_bytes > limit {
            return Err(format!("scene exceeds GPU buffer budget: vertices={vertex_bytes} bytes, indices={index_bytes} bytes; per-buffer device limit={limit} bytes. Reduce source relief or native fixture extent.").into());
        }
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera relative matrix"),
            size: 96,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native voxel"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native voxel"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("native voxel"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<mesh::Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                }],
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
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });
        let appearance_layout = terrain::TerrainGpu::appearance_layout(&device);
        let hybrid_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("hybrid appearance"),
                bind_group_layouts: &[&layout, &appearance_layout],
                push_constant_ranges: &[],
            });
        let hybrid_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("native voxel"),
            layout: Some(&hybrid_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<mesh::Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_hybrid"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fixture vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fixture indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Ok(Self {
            visual_settings: None,
            regional_appearance: false,
            alpine_appearance: false,
            terrain_amplitude: 1000.,
            device,
            queue,
            pipeline,
            native_layout: layout,
            hybrid_pipeline,
            hybrid: None,
            pages: None,
            adaptive: None,
            vegetation: None,
            vegetation_stream: None,
            vegetation_stats: Default::default(),
            vegetation_revision: 0,
            vegetation_samples: sample_count,
            summary_contention: None,
            summary_contention_startup_ms: 0.,
            draw_encode_cpu_ms: std::cell::Cell::new(0.),
            last_draw_encode_cpu_ms: std::cell::Cell::new(0.),
            draw_encode_calls: std::cell::Cell::new(0),
            summary_contention_dispatches: std::cell::Cell::new(0),
            volume_owner: false,
            brick: None,
            brick_skip_mode: 0,
            brick_cache_metrics: Default::default(),
            surface_probe: None,
            uniform,
            bind_group,
            vertices,
            indices,
            index_count: mesh.indices.len() as u32,
            origin: DVec3::new(
                mesh.origin.x as f64,
                mesh.origin.y as f64,
                mesh.origin.z as f64,
            ) * VOXEL_SIZE,
            format: output_format,
            scene_format: format,
            medium_ground_height: 0.,
            sample_count,
            voxel_filter,
            terrain: None,
            adapter_info: format!("{:?}", adapter.get_info()),
        })
    }
    fn install_bricks(
        &mut self,
        snapshot: &brick_ray::BrickSnapshot,
        skip_mode: u32,
    ) -> Result<()> {
        self.brick = Some(brick_ray::BrickRayGpu::new(
            &self.device,
            snapshot,
            self.scene_format,
            self.sample_count,
        )?);
        self.brick_skip_mode = skip_mode;
        let metrics = &mut self.brick_cache_metrics;
        metrics.latest_build_ms = snapshot.directional_build_ms;
        metrics.latest_gpu_bytes = snapshot.directional_bytes();
        metrics.latest_scratch_bytes = snapshot.directional_scratch_bytes;
        metrics.peak_scratch_bytes = metrics
            .peak_scratch_bytes
            .max(snapshot.directional_scratch_bytes);
        if !snapshot.directional_bounds.is_empty() {
            metrics.builds += 1;
            metrics.total_build_ms += snapshot.directional_build_ms;
            metrics.uploaded_bytes += snapshot.directional_bytes() as u64;
        }
        self.origin = DVec3::new(
            snapshot.origin.x as f64,
            snapshot.origin.y as f64,
            snapshot.origin.z as f64,
        ) * VOXEL_SIZE;
        Ok(())
    }
    /// Opt-in contention experiment. Startup checks exact parity; frames only
    /// enqueue resident compute and never wait for or consume its output.
    fn init_summary_contention(&mut self, world: &World) -> Result<()> {
        if std::env::var_os("FRONTIER_GPU_SUMMARY_CONTENTION").is_none_or(|v| v != "1") {
            return Ok(());
        }
        let started = Instant::now();
        let pages = (0..gpu_summary_probe::MAX_PAGES)
            .map(|i| gpu_summary_probe::CanonicalPage::sample(world, [i as i64 * 128, -512]))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let expected = pages
            .iter()
            .map(|p| p.cpu_patch_counts(16))
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let mut probe = gpu_summary_probe::GpuSummaryProbe::new(&self.device, pages.len())?;
        probe.upload(&self.queue, &pages)?;
        let (actual, _) = probe.run_blocking(&self.device, &self.queue)?;
        if actual != expected {
            return Err("GPU contention probe canonical parity failed".into());
        }
        self.summary_contention_startup_ms = started.elapsed().as_secs_f64() * 1000.;
        self.summary_contention = Some(probe);
        Ok(())
    }
    fn init_hybrid(&mut self, prepared: hybrid_gpu::Prepared) -> Result<()> {
        let terrain = terrain::TerrainGpu::new_with_samples(
            &self.device,
            self.scene_format,
            &prepared.terrain.data,
            self.sample_count,
        );
        terrain.set_voxel_filter(self.voxel_filter);
        let mut terrain = terrain;
        terrain.set_volume_owner(self.volume_owner);
        self.terrain = Some(terrain);
        let center = prepared.native.center;
        let revision = prepared.native.revision;
        let mut hybrid = hybrid_gpu::HybridGpu::new();
        hybrid.stage(prepared)?;
        while !hybrid.upload(
            &self.device,
            &self.queue,
            &self.native_layout,
            self.terrain.as_mut().unwrap(),
            center,
            revision,
        ) {
            self.queue.submit(std::iter::empty());
            self.device.poll(wgpu::Maintain::Wait);
        }
        self.hybrid = Some(hybrid);
        Ok(())
    }
    fn init_pages(&mut self, cover: &page_terrain::Cover) {
        let mut pages =
            page_terrain::PageGpu::new(&self.device, self.scene_format, self.sample_count);
        pages.upload(&self.queue, cover);
        self.pages = Some(pages);
    }
    fn replace_mesh(&mut self, mesh: &mesh::Mesh) {
        self.vertices = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("edited vertices"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        self.indices = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("edited indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        self.index_count = mesh.indices.len() as u32;
        self.origin = DVec3::new(
            mesh.origin.x as f64,
            mesh.origin.y as f64,
            mesh.origin.z as f64,
        ) * VOXEL_SIZE;
    }
    fn texture(&self, width: u32, height: u32, depth: bool) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(if depth { "depth" } else { "capture" }),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: if depth { self.sample_count } else { 1 },
            dimension: wgpu::TextureDimension::D2,
            format: if depth {
                wgpu::TextureFormat::Depth32Float
            } else {
                self.format
            },
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | if depth && self.sample_count > 1 {
                    wgpu::TextureUsages::empty()
                } else {
                    wgpu::TextureUsages::COPY_SRC
                },
            view_formats: &[],
        })
    }
    fn multisample_texture(&self, width: u32, height: u32) -> Option<wgpu::Texture> {
        if self.sample_count == 1 {
            return None;
        }
        Some(self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("MSAA coverage"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: self.sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        }))
    }
    fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        resolve: Option<&wgpu::TextureView>,
        depth: &wgpu::TextureView,
        camera: &Camera,
        width: u32,
        height: u32,
        timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
        plant_timings: Option<gpu_probe::VegetationTiming<'_>>,
    ) {
        let _encode_timer = EncodeTimer(
            &self.draw_encode_cpu_ms,
            &self.last_draw_encode_cpu_ms,
            Instant::now(),
        );
        self.draw_encode_calls.set(self.draw_encode_calls.get() + 1);
        self.queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::cast_slice(
                &camera
                    .matrix(self.origin, width as f32 / height as f32)
                    .to_cols_array(),
            ),
        );
        self.queue.write_buffer(
            &self.uniform,
            64,
            bytemuck::cast_slice(
                &(camera.position - self.origin)
                    .as_vec3()
                    .extend(if self.volume_owner { 1. } else { 0. })
                    .to_array(),
            ),
        );
        let paint_origin = [
            (self.origin.x * 16.) as i64 as u32,
            (self.origin.y * 16.) as i64 as u32,
            (self.origin.z * 16.) as i64 as u32,
            0u32,
        ];
        self.queue
            .write_buffer(&self.uniform, 80, bytemuck::cast_slice(&paint_origin));
        if let Some(brick) = &self.brick {
            brick.update_camera_mode(
                &self.queue,
                camera.matrix(self.origin, width as f32 / height as f32),
                (camera.position - self.origin).as_vec3(),
                width,
                height,
                self.brick_skip_mode,
            );
        }
        if let Some(probe) = &self.summary_contention {
            probe
                .encode_resident(encoder)
                .expect("initialized bounded summary diagnostic");
            self.summary_contention_dispatches
                .set(self.summary_contention_dispatches.get() + 1);
        }
        if let (Some(pages), Some(hybrid)) = (&self.pages, &self.hybrid) {
            pages.prepare(
                &self.queue,
                camera,
                width as f32 / height as f32,
                hybrid.center(),
                self.volume_owner,
                self.voxel_filter,
            );
        }
        if let Some(adaptive) = &self.adaptive {
            adaptive.draw_shadows(encoder);
            if let Some(t) = &plant_timings {
                t.shadow_start(encoder);
            }
            if adaptive.near_shadows_active() {
                if let Some(vegetation) = &self.vegetation {
                    vegetation.draw_shadows(
                        encoder,
                        &self.bind_group,
                        adaptive.near_shadow_view(),
                        adaptive.near_shadow_depth_bind(),
                    );
                }
            }
            if let Some(t) = &plant_timings {
                t.shadow_end(encoder);
            }
        } else if let Some(t) = &plant_timings {
            t.shadow_start(encoder);
            t.shadow_end(encoder);
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native fixture"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: resolve,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.34,
                        g: 0.57,
                        b: 0.80,
                        a: 1.,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: timestamps,
            occlusion_query_set: None,
        });
        // Dense opaque vegetation occludes expensive terrain shading here.
        // Keep terrain-first as a diagnostic A/B; it was slower on the laptop.
        static PLANTS_FIRST: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let plants_first = *PLANTS_FIRST.get_or_init(|| {
            std::env::var("FRONTIER_VEGETATION_DRAW_ORDER").as_deref() != Ok("after")
        });
        if !plants_first {
            if let Some(adaptive) = &self.adaptive {
                adaptive.draw(&mut pass);
            }
        }
        if let Some(t) = &plant_timings {
            t.color_start(&mut pass);
        }
        if let (Some(vegetation), Some(adaptive)) = (&self.vegetation, &self.adaptive) {
            vegetation.draw(&mut pass, &self.bind_group, adaptive.near_shadow_bind());
        }
        if let Some(t) = &plant_timings {
            t.color_end(&mut pass);
        }
        if let Some(adaptive) = &self.adaptive {
            if plants_first {
                adaptive.draw(&mut pass);
            }
            if !self.volume_owner {
                return;
            }
        }
        if let Some(brick) = &self.brick {
            brick.draw(&mut pass);
            return;
        }
        if let Some(probe) = &self.surface_probe {
            probe.set_view_position(camera.position.to_array());
            let center = probe.center();
            probe.draw(
                &self.queue,
                &mut pass,
                camera.matrix(
                    DVec3::new(center[0], 0., center[1]),
                    width as f32 / height as f32,
                ),
            );
            return;
        }
        if let Some(terrain) = &self.terrain {
            terrain.set_view_position(camera.position.to_array());
            if let Some(pages) = &self.pages {
                terrain.prepare(
                    &self.queue,
                    camera.matrix(
                        DVec3::new(terrain.center()[0], 0., terrain.center()[1]),
                        width as f32 / height as f32,
                    ),
                );
                pages.draw(&mut pass);
            } else {
                terrain.draw(
                    &self.queue,
                    &mut pass,
                    camera.matrix(
                        DVec3::new(terrain.center()[0], 0., terrain.center()[1]),
                        width as f32 / height as f32,
                    ),
                );
            }
            if let Some(hybrid) = &self.hybrid {
                pass.set_bind_group(1, terrain.appearance_bind(), &[]);
                hybrid.draw(
                    &self.queue,
                    &mut pass,
                    &self.hybrid_pipeline,
                    camera,
                    width as f32 / height as f32,
                );
            }
            if !self.volume_owner {
                return;
            }
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
    fn capture(&self, camera: &Camera, path: &Path, width: u32, height: u32) -> Result<()> {
        let texture = self.texture(width, height, false);
        let depth = self.texture(width, height, true);
        let floating = self.format == wgpu::TextureFormat::Rgba32Float;
        let bpp = if floating { 16 } else { 4 };
        let row = (width * bpp + 255) & !255;
        let depth_row = (width * 4 + 255) & !255;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture readback"),
            size: row as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let target = texture.create_view(&Default::default());
        let multisample = self.multisample_texture(width, height);
        let multisample_view = multisample
            .as_ref()
            .map(|t| t.create_view(&Default::default()));
        let post_settings = self.visual_settings.as_ref().filter(|s| {
            (s.atmosphere
                || s.exposure != 1.
                || s.render_scale != 1.
                || self.scene_format != self.format)
                && !floating
        });
        let mut post_attachment_bytes = 0u64;
        if let Some(settings) = post_settings {
            let atmosphere = atmosphere::Atmosphere::new_with_scene_format(
                &self.device,
                self.format,
                width,
                height,
                settings.render_scale,
                self.sample_count,
                self.scene_format,
            );
            post_attachment_bytes = atmosphere.attachment_bytes();
            let (rw, rh) = atmosphere.dimensions();
            atmosphere.set_medium_ground_height(self.medium_ground_height);
            atmosphere.prepare(
                &self.queue,
                camera
                    .matrix(camera.position, rw as f32 / rh as f32)
                    .inverse(),
                Vec3::ZERO,
                camera.position,
                settings,
                0.,
            );
            self.draw(
                &mut encoder,
                atmosphere.msaa_view().unwrap_or(atmosphere.scene_view()),
                atmosphere.msaa_view().map(|_| atmosphere.scene_view()),
                atmosphere.depth_view(),
                camera,
                rw,
                rh,
                None,
                None,
            );
            atmosphere.encode_with_shadows(
                &mut encoder,
                &target,
                self.adaptive.as_ref().map(|a| a.near_shadow_bind()),
            );
        } else {
            self.draw(
                &mut encoder,
                multisample_view.as_ref().unwrap_or(&target),
                multisample_view.as_ref().map(|_| &target),
                &depth.create_view(&Default::default()),
                camera,
                width,
                height,
                None,
                None,
            );
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let depth_buffer = (self.sample_count == 1 && post_settings.is_none()).then(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("depth capture readback"),
                size: depth_row as u64 * height as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });
        if let Some(buffer) = &depth_buffer {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &depth,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::DepthOnly,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(depth_row),
                        rows_per_image: Some(height),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.queue.submit(Some(encoder.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let mapped = buffer.slice(..).get_mapped_range();
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for scanline in mapped.chunks_exact(row as usize) {
            pixels.extend_from_slice(&scanline[..(width * bpp) as usize]);
        }
        if self.format == wgpu::TextureFormat::Bgra8UnormSrgb
            || self.format == wgpu::TextureFormat::Bgra8Unorm
        {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        if let Some(buffer) = &depth_buffer {
            let (tx, rx) = std::sync::mpsc::channel();
            buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            self.device.poll(wgpu::Maintain::Wait);
            rx.recv()??;
            let bytes = buffer.slice(..).get_mapped_range();
            let mut packed = Vec::with_capacity((width * height * 4) as usize);
            for row_bytes in bytes.chunks_exact(depth_row as usize) {
                packed.extend_from_slice(&row_bytes[..width as usize * 4]);
            }
            let coverage: Vec<u8> = packed
                .chunks_exact(4)
                .flat_map(|bytes| {
                    let d = f32::from_le_bytes(bytes.try_into().unwrap());
                    (if d.is_finite() && d >= 0. && d < 1. {
                        1f32
                    } else {
                        0f32
                    })
                    .to_le_bytes()
                })
                .collect();
            std::fs::write(path.with_extension("coverage.f32"), coverage)?;
            std::fs::write(path.with_extension("depth.f32"), packed)?;
        } else {
            // Do not leave diagnostic files from an earlier non-postprocessed capture.
            for extension in ["depth.f32", "coverage.f32"] {
                match std::fs::remove_file(path.with_extension(extension)) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e.into()),
                }
            }
        }
        // Float captures preserve unclipped shader radiance. Ordinary captures
        // decode their actual quantized target, retaining that limitation.
        let srgb = matches!(
            self.format,
            wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Rgba8UnormSrgb
        );
        let mut linear = Vec::with_capacity(width as usize * height as usize * 12);
        if floating {
            let mut preview = Vec::with_capacity(width as usize * height as usize * 4);
            for pixel in pixels.chunks_exact(16) {
                for channel in pixel[..12].chunks_exact(4) {
                    linear.extend_from_slice(channel);
                    let v = f32::from_le_bytes(channel.try_into().unwrap()).max(0.);
                    let v = if v <= 0.0031308 {
                        12.92 * v
                    } else {
                        1.055 * v.powf(1. / 2.4) - 0.055
                    };
                    preview.push((v.clamp(0., 1.) * 255. + 0.5) as u8);
                }
                preview.push(255);
            }
            pixels = preview;
        } else {
            for pixel in pixels.chunks_exact(4) {
                for &byte in &pixel[..3] {
                    let v = byte as f32 / 255.;
                    let v = if srgb {
                        if v <= 0.04045 {
                            v / 12.92
                        } else {
                            ((v + 0.055) / 1.055).powf(2.4)
                        }
                    } else {
                        v
                    };
                    linear.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        std::fs::write(path.with_extension("linear_rgb.f32"), linear)?;
        let color_encoding = if floating {
            "linear_rgb_float32_render_target"
        } else {
            "linear_rgb_decoded_from_8bit_target"
        };
        let samples = self.sample_count as u64;
        let mut capture_render_texture_bytes = width as u64
            * height as u64
            * (bpp as u64 + samples * 4 + if samples > 1 { samples * bpp as u64 } else { 0 });
        capture_render_texture_bytes += post_attachment_bytes;
        let capture_readback_bytes = (row as u64
            + if depth_buffer.is_some() {
                depth_row as u64
            } else {
                0
            })
            * height as u64;
        let eye = camera.position.to_array();
        let target = (camera.position + camera.direction().as_dvec3()).to_array();
        std::fs::write(path.with_extension("capture.json"), format!(
            "{{\"width\":{width},\"height\":{height},\"eye\":{eye:?},\"target\":{target:?},\"fov_y_degrees\":{},\"near\":0.025,\"far\":{},\"samples\":{},\"depth_encoding\":\"ndc_0_1\",\"color_encoding\":{color_encoding:?},\"voxel_filter\":{},\"adapter\":{:?},\"capture_render_texture_bytes\":{capture_render_texture_bytes},\"capture_readback_bytes\":{capture_readback_bytes}}}",
            camera.fov_y, camera.far, self.sample_count, self.voxel_filter, self.adapter_info
        ))?;
        if let Some(settings) = &self.visual_settings {
            let metadata_path = path.with_extension("capture.json");
            let mut metadata = std::fs::read_to_string(&metadata_path)?;
            metadata.pop();
            let shadows_active = self.adaptive.as_ref().is_some_and(|a| a.shadows_active());
            metadata.push_str(&format!(
                ",\"atmosphere\":{},\"render_scale\":{},\"postprocessed\":{},\"depth_available\":{},\"visual\":{{\"atmosphere\":{},\"render_scale\":{},\"exposure\":{},\"fog_density\":{},\"fog_height_falloff\":{},\"sun_elevation\":{},\"sun_azimuth\":{},\"cloud_coverage\":{},\"cloud_speed\":{},\"overcast\":{},\"terrain_shadows_requested\":{},\"terrain_shadows_active\":{},\"shadow_map_size\":2048,\"shadow_footprint_m\":2048,\"shadow_casters\":\"published_adaptive_terrain_owners_only\",\"gpu_timing_scope\":\"opaque_scene_pass_without_atmosphere_or_ui\"}}}}",
                settings.atmosphere, settings.render_scale, post_settings.is_some(), depth_buffer.is_some(),
                settings.atmosphere, settings.render_scale, settings.exposure,
                settings.fog_density, settings.fog_height_falloff, settings.sun_elevation,
                settings.sun_azimuth, settings.cloud_coverage, settings.cloud_speed,
                settings.overcast, settings.terrain_shadows, shadows_active
            ));
            metadata.pop();
            metadata.push_str(&format!(
                ",\"beauty\":{},\"scene_color_format\":{:?}}}",
                settings.beauty,
                format!("{:?}", self.scene_format)
            ));
            let atmosphere_debug = match std::env::var("FRONTIER_ATMOSPHERE_DEBUG").as_deref() {
                Ok("air") => 1,
                Ok("visibility") => 2,
                Ok("no-air") => 3,
                _ => 0,
            };
            metadata.pop();
            metadata.push_str(&format!(
                ",\"medium_ground_height_m\":{},\"atmosphere_debug_code\":{atmosphere_debug}}}",
                self.medium_ground_height
            ));
            std::fs::write(metadata_path, metadata)?;
        }
        let metadata_path = path.with_extension("capture.json");
        let mut metadata = std::fs::read_to_string(&metadata_path)?;
        metadata.pop();
        metadata.push_str(&format!(",{}}}", self.vegetation_json()));
        std::fs::write(metadata_path, metadata)?;
        let mut png = png::Encoder::new(std::fs::File::create(path)?, width, height);
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        png.write_header()?.write_image_data(&pixels)?;
        eprintln!("captured {} ({}x{})", path.display(), width, height);
        Ok(())
    }
}

const ARENA_MIN: Cell = Cell {
    x: -128,
    y: -32,
    z: -128,
};
const ARENA_SIZE: [usize; 3] = [256, 224, 256];
fn fixture_mesh(world: &World) -> mesh::Mesh {
    mesh::extract(|p| world.material(p), ARENA_MIN, ARENA_SIZE)
}
fn inside_arena(cell: Cell) -> bool {
    (ARENA_MIN.x..ARENA_MIN.x + ARENA_SIZE[0] as i64).contains(&cell.x)
        && (ARENA_MIN.y..ARENA_MIN.y + ARENA_SIZE[1] as i64).contains(&cell.y)
        && (ARENA_MIN.z..ARENA_MIN.z + ARENA_SIZE[2] as i64).contains(&cell.z)
}
/// Picking always queries the canonical source. A pending rebuild permits no second edit.
fn edit_target(
    world: &World,
    origin: [f64; 3],
    direction: [f64; 3],
    place: bool,
    pending: bool,
) -> Option<Cell> {
    if pending {
        return None;
    }
    let hit = frontier_world::raycast(world, origin, direction, 8.)?;
    let cell = if place {
        if hit.normal == [0; 3] {
            return None;
        }
        Cell {
            x: hit.cell.x + hit.normal[0] as i64,
            y: hit.cell.y + hit.normal[1] as i64,
            z: hit.cell.z + hit.normal[2] as i64,
        }
    } else {
        hit.cell
    };
    (if let Some(base) = world.topology_base_cells() {
        world.volume_owned(cell) && (base - 80..base + 160).contains(&cell.y)
    } else {
        inside_arena(cell)
    })
    .then_some(cell)
}
struct PendingEdit {
    receiver: std::sync::mpsc::Receiver<(
        mesh::Mesh,
        u64,
        std::time::Duration,
        Option<brick_ray::BrickSnapshot>,
    )>,
    started: Instant,
    staged_world: Option<World>,
}
impl PendingEdit {
    fn start_volume(snapshot: World) -> Self {
        let revision = snapshot.revision();
        let (sender, receiver) = std::sync::mpsc::channel();
        let started = Instant::now();
        let staged_world = Some(snapshot.clone());
        std::thread::spawn(move || match volume::build(&snapshot) {
            Ok(mesh) => {
                let _ = sender.send((mesh, revision, started.elapsed(), None));
            }
            Err(error) => eprintln!("volume edit build failed: {error}"),
        });
        Self {
            receiver,
            started,
            staged_world,
        }
    }
    #[cfg(test)]
    fn start(snapshot: World, min: Cell, size: [usize; 3]) -> Self {
        Self::start_backend(snapshot, min, size, false, false)
    }
    fn start_backend(
        snapshot: World,
        min: Cell,
        size: [usize; 3],
        ray: bool,
        directional: bool,
    ) -> Self {
        let revision = snapshot.revision();
        let (sender, receiver) = std::sync::mpsc::channel();
        let started = Instant::now();
        let staged_world = Some(snapshot.clone());
        std::thread::spawn(move || {
            let mesh = mesh::extract(|p| snapshot.material(p), min, size);
            let bricks = ray.then(|| {
                let mut bricks =
                    brick_ray::BrickSnapshot::build(|p| snapshot.material(p), min, size);
                if directional {
                    bricks.build_directional_bounds();
                }
                bricks
            });
            let _ = sender.send((mesh, revision, started.elapsed(), bricks));
        });
        Self {
            receiver,
            started,
            staged_world,
        }
    }
}

/// Deterministic edit transaction: save every original canonical material,
/// including existing player overrides, before placing a 16³-cell stone cube.
fn edit_test_addition(world: &World) -> (World, Vec<(Cell, Material)>) {
    let bottom = world.topology_base_cells().map_or(84, |base| base + 36);
    let mut snapshot = world.clone();
    let mut original = Vec::with_capacity(16 * 16 * 16);
    for z in -32..-16 {
        for y in bottom..bottom + 16 {
            for x in 64..80 {
                let cell = Cell { x, y, z };
                original.push((cell, world.material(cell)));
                snapshot.set(cell, Material::Stone);
            }
        }
    }
    (snapshot, original)
}
fn edit_test_restore(world: &World, original: &[(Cell, Material)]) -> World {
    let mut snapshot = world.clone();
    for &(cell, material) in original {
        snapshot.set(cell, material);
    }
    snapshot
}
struct EditTest {
    /// 0 waiting to add, 1 addition queued/published, 2 restoration queued,
    /// 3 restoration published. Publication, rather than request, starts delay.
    phase: u8,
    next_seconds: f64,
    original: Vec<(Cell, Material)>,
}
impl Default for EditTest {
    fn default() -> Self {
        Self {
            phase: 0,
            next_seconds: 2.,
            original: Vec::new(),
        }
    }
}

struct Viewer {
    visual_settings: settings::VisualSettings,
    settings_ui: Option<settings_ui::SettingsUi>,
    atmosphere: Option<atmosphere::Atmosphere>,
    visual_menu_enabled: bool,
    visual_started: Instant,
    display_frame_ms: f64,
    visual_build_mib: usize,
    visual_disk_cache: Option<PathBuf>,
    visual_disk_mib: usize,

    adaptive_view: Option<adaptive_view::AdaptiveView>,
    adaptive_route_quality: adaptive_view::RouteQualityStats,
    adaptive_route_started: bool,
    adaptive_wait_quality: bool,
    last_preparation_present: Option<Instant>,
    benchmark_input_lock: bool,
    route_frame_origin: u64,
    renderer: String,
    surface_layout: String,
    brick_initial: Option<brick_ray::BrickSnapshot>,
    gpu_probe: Option<gpu_probe::GpuProbe>,
    pending_teleport: Option<(u64, Camera, Instant)>,
    teleport_destinations: std::collections::VecDeque<[f64; 2]>,
    next_teleport_seconds: f64,
    teleport_count: u64,
    teleport_last_ms: f64,
    teleport_start_jobs: u64,
    teleport_start_upload: u64,
    teleport_records: Vec<String>,
    edit_test: Option<EditTest>,
    edit_published_count: u64,
    edit_build_ms: f64,
    edit_input_to_publish_ms: f64,
    edit_records: Vec<String>,

    mesh: Option<mesh::Mesh>,
    window: Option<Arc<Window>>,
    surface: Option<wgpu::Surface<'static>>,
    gpu: Option<Gpu>,
    config: Option<wgpu::SurfaceConfiguration>,
    depth: Option<wgpu::Texture>,
    multisample: Option<wgpu::Texture>,
    camera: Camera,
    walking: walking::WalkingCamera,
    frame_trace: playability::FrameTrace,
    keys: HashSet<KeyCode>,
    grabbed: bool,
    minimized: bool,
    last_frame: Instant,
    fps_start: Instant,
    frames: u32,
    capture_number: u32,
    error: Option<String>,
    width: u32,
    height: u32,
    frame_limit: Option<u64>,
    seconds: Option<f64>,
    total_frames: u64,
    stats: frontier_runtime::FrameStats,
    draw_encode_frames: DrawEncodeFrames,
    world: World,
    pending_edit: Option<PendingEdit>,
    terrain_data: Option<terrain::TerrainUpdate>,
    hybrid_initial: Option<hybrid_gpu::Prepared>,
    page_initial: Option<page_terrain::Cover>,
    page_stream: Option<page_terrain::Stream>,
    page_pending: Option<page_terrain::Cover>,
    page_spacing: u32,
    fbm: bool,
    hybrid: bool,
    hybrid_stream: Option<hybrid_gpu::Stream>,
    sample_count: u32,
    voxel_filter: bool,
    initial_camera: Camera,
    route: frontier_runtime::routes::Route,
    uncapped: bool,
    warmup: u64,
    route_step: f64,
    route_speed: f64,
    route_realtime: bool,
    route_started: Instant,
    terrain_streamer: Option<terrain::TerrainStreamer>,
    terrain_static: bool,
    terrain_uploads: u64,
    terrain_upload_bytes: u64,
    terrain_upload_ms: f64,
    terrain_prepare_ms: f64,
    terrain_center_lag_max_m: f64,
    terrain_center_lag_end_m: f64,
}
impl Viewer {
    fn begin_teleport(&mut self, destination: [f64; 2]) -> Result<()> {
        let Some(stream) = &mut self.hybrid_stream else {
            return Ok(());
        };
        self.teleport_start_jobs = stream.native.metrics().jobs;
        self.teleport_start_upload = self
            .gpu
            .as_ref()
            .and_then(|g| g.hybrid.as_ref())
            .map_or(0, |g| g.upload_bytes);
        let id = stream.request_teleport(destination)?;
        let mut target = self.camera.clone();
        target.position = DVec3::new(
            destination[0],
            self.world.height_m(destination[0], destination[1]) + 1.7,
            destination[1],
        );
        self.pending_teleport = Some((id, target, Instant::now()));
        self.route = frontier_runtime::routes::Route::Stationary;
        self.keys.clear();
        if let Some(window) = &self.window {
            window.set_title("Frontier — preparing teleport destination");
        }
        Ok(())
    }
    fn poll_terrain(&mut self) {
        if let (Some(view), Some(gpu)) = (&mut self.adaptive_view, &mut self.gpu) {
            let (width, height) = self
                .atmosphere
                .as_ref()
                .map_or((self.width, self.height), |a| a.dimensions());
            if let Err(error) = view.poll(gpu, self.camera, width, height) {
                self.error = Some(error);
            }
            return;
        }
        if let Some(stream) = &mut self.hybrid_stream {
            if let Err(error) = stream.poll(
                [self.camera.position.x, self.camera.position.z],
                !self.terrain_static,
            ) {
                self.error = Some(error);
                return;
            }
            let desired = stream.native.desired_center();
            // Native desired_revision is a camera/cover transaction epoch, not
            // a change to canonical source content.
            let native_epoch = stream.native.desired_revision();
            let source_revision = self.world.revision();
            let mut pages_ready = true;
            if let Some(pages) = &mut self.page_stream {
                pages.request(desired, source_revision, native_epoch);
                if self.page_pending.as_ref().is_some_and(|c| {
                    !c.matches(desired, source_revision, native_epoch, self.page_spacing)
                }) {
                    self.page_pending = None;
                }
                if let Some(cover) = pages.poll() {
                    self.page_pending = Some(cover);
                }
                pages_ready = self.page_pending.as_ref().is_some_and(|c| {
                    c.matches(desired, source_revision, native_epoch, self.page_spacing)
                }) || self
                    .gpu
                    .as_ref()
                    .and_then(|g| g.pages.as_ref())
                    .is_some_and(|p| {
                        p.center == page_terrain::snapped_center(desired, self.page_spacing)
                            && p.source_revision == source_revision
                    });
            }
            if let Some(gpu) = &mut self.gpu {
                if let (Some(hybrid), Some(terrain)) = (&mut gpu.hybrid, &mut gpu.terrain) {
                    if !hybrid.has_pending() {
                        if let Some(ready) = stream.ready.take() {
                            if ready.native.center == stream.native.desired_center() {
                                if let Err(error) = hybrid.stage(ready) {
                                    self.error = Some(error);
                                    return;
                                }
                            } else {
                                stream.discard(ready);
                            }
                        }
                    }
                    if pages_ready
                        && hybrid.upload(
                            &gpu.device,
                            &gpu.queue,
                            &gpu.native_layout,
                            terrain,
                            stream.native.desired_center(),
                            stream.native.desired_revision(),
                        )
                    {
                        if let Some(cover) = self.page_pending.take() {
                            if let Some(pages) = &mut gpu.pages {
                                pages.upload(&gpu.queue, &cover);
                            }
                        }
                        stream.published(hybrid.center());
                    }
                    // Edits can invalidate derived pages without creating a new
                    // native camera transaction. Do not strand that refresh:
                    // its existing native owner is already coherent and current.
                    if self.page_pending.as_ref().is_some_and(|cover| {
                        cover.can_publish(
                            source_revision,
                            self.page_spacing,
                            (hybrid.center(), hybrid.revision()),
                            (desired, native_epoch),
                        )
                    }) {
                        if let (Some(cover), Some(pages)) =
                            (self.page_pending.take(), &mut gpu.pages)
                        {
                            pages.upload(&gpu.queue, &cover);
                        }
                    }
                }
            }
            if self
                .pending_teleport
                .as_ref()
                .is_some_and(|(id, _, _)| stream.teleport_ready(*id))
            {
                let (id, camera, started) = self.pending_teleport.take().unwrap();
                self.camera = camera;
                stream.finish_teleport(id);
                self.teleport_count += 1;
                self.teleport_last_ms = started.elapsed().as_secs_f64() * 1000.;
                let jobs = stream.native.metrics().jobs - self.teleport_start_jobs;
                let upload = self
                    .gpu
                    .as_ref()
                    .and_then(|g| g.hybrid.as_ref())
                    .map_or(0, |g| g.upload_bytes)
                    - self.teleport_start_upload;
                self.teleport_records.push(format!(
                    "{{\"readiness_ms\":{},\"native_jobs\":{},\"native_upload_bytes\":{}}}",
                    self.teleport_last_ms, jobs, upload
                ));
                self.next_teleport_seconds = self.route_started.elapsed().as_secs_f64() + 2.;
                eprintln!(
                    "teleport {} ready in {:.1}ms at {:?}",
                    id, self.teleport_last_ms, self.camera.position
                );
                if let Some(window) = &self.window {
                    window.set_title("Frontier — destination ready");
                }
            }
            return;
        }
        let Some(streamer) = &mut self.terrain_streamer else {
            return;
        };
        if !self.terrain_static {
            streamer.request([self.camera.position.x, self.camera.position.z]);
        }
        if let Some(update) = streamer.poll() {
            if let Some(gpu) = &mut self.gpu {
                if let Some(terrain) = &mut gpu.terrain {
                    let started = Instant::now();
                    let upload_bytes = terrain.upload(&gpu.queue, &update);
                    self.terrain_upload_ms += started.elapsed().as_secs_f64() * 1000.;
                    self.terrain_uploads += 1;
                    self.terrain_upload_bytes += upload_bytes;
                }
            }
        }
    }
    fn edit(&mut self, place: bool) {
        if self.world.fbm_terrain().is_some() && self.world.topology_base_cells().is_none() {
            eprintln!(
                "FBM comparison scenes have edits disabled; use --scene fixture for exact editing"
            );
            return;
        }
        let Some(cell) = edit_target(
            &self.world,
            self.camera.position.to_array(),
            self.camera.direction().as_dvec3().to_array(),
            place,
            self.pending_edit.is_some(),
        ) else {
            return;
        };
        let mut snapshot = self.world.clone();
        snapshot.set(
            cell,
            if place {
                Material::Stone
            } else {
                Material::Air
            },
        );
        self.queue_edit(
            snapshot,
            if place { "place" } else { "remove" },
            Instant::now(),
        );
    }
    fn queue_edit(&mut self, snapshot: World, label: &str, input: Instant) {
        let revision = snapshot.revision();
        let mut pending = if self.world.topology_base_cells().is_some() {
            PendingEdit::start_volume(snapshot)
        } else {
            PendingEdit::start_backend(
                snapshot,
                ARENA_MIN,
                ARENA_SIZE,
                self.renderer != "mesh",
                self.renderer == "brick-directional",
            )
        };
        pending.started = input;
        self.pending_edit = Some(pending);
        eprintln!("edit {label}: revision {revision} rebuilding; further edits wait");
    }
    fn drive_edit_test(&mut self) {
        if self.pending_edit.is_some() {
            return;
        }
        let Some(test) = &mut self.edit_test else {
            return;
        };
        if self.route_started.elapsed().as_secs_f64() < test.next_seconds {
            return;
        }
        let input = Instant::now();
        match test.phase {
            0 => {
                let (snapshot, original) = edit_test_addition(&self.world);
                test.original = original;
                test.phase = 1;
                test.next_seconds = f64::INFINITY;
                self.queue_edit(snapshot, "test add 1m cube", input);
            }
            1 => {
                let snapshot = edit_test_restore(&self.world, &test.original);
                test.phase = 2;
                test.next_seconds = f64::INFINITY;
                self.queue_edit(snapshot, "test restore original materials", input);
            }
            _ => {}
        }
    }
    fn poll_edit(&mut self) {
        let Some(pending) = &self.pending_edit else {
            return;
        };
        match pending.receiver.try_recv() {
            Ok((mesh, revision, build_time, bricks)) => {
                if revision
                    == pending
                        .staged_world
                        .as_ref()
                        .unwrap_or(&self.world)
                        .revision()
                {
                    if let Some(gpu) = &mut self.gpu {
                        if let Some(bricks) = &bricks {
                            if let Err(error) =
                                gpu.install_bricks(bricks, brick_skip_mode(&self.renderer))
                            {
                                self.error = Some(error.to_string());
                                return;
                            }
                        } else {
                            gpu.replace_mesh(&mesh);
                        }
                    }
                    if let Some(staged) = &pending.staged_world {
                        self.world = staged.clone();
                        if let Some(view) = &mut self.adaptive_view {
                            view.publish_volume_revision(&self.world);
                        }
                    }
                    self.edit_published_count += 1;
                    self.edit_build_ms = build_time.as_secs_f64() * 1000.;
                    self.edit_input_to_publish_ms = pending.started.elapsed().as_secs_f64() * 1000.;
                    self.edit_records.push(format!(
                        "{{\"revision\":{},\"build_ms\":{},\"input_to_publish_ms\":{}}}",
                        revision, self.edit_build_ms, self.edit_input_to_publish_ms
                    ));
                    if let Some(test) = &mut self.edit_test {
                        if test.phase == 1 && !test.next_seconds.is_finite() {
                            test.next_seconds = self.route_started.elapsed().as_secs_f64() + 2.;
                        } else if test.phase == 2 {
                            test.phase = 3;
                        }
                    }
                    eprintln!(
                        "published edit revision {}: build {:.3}s, input-to-publish {:.3}s",
                        revision,
                        build_time.as_secs_f64(),
                        pending.started.elapsed().as_secs_f64()
                    );
                } else {
                    eprintln!("discarded obsolete edit mesh revision {}", revision);
                }
                self.pending_edit = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                eprintln!("edit mesh worker disconnected");
                self.pending_edit = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => (),
        }
    }
    fn grab(&mut self, grabbed: bool) {
        if let Some(window) = &self.window {
            if grabbed {
                self.grabbed = window
                    .set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined))
                    .is_ok();
            } else {
                let _ = window.set_cursor_grab(CursorGrabMode::None);
                self.grabbed = false;
            }
            window.set_cursor_visible(!self.grabbed);
        }
    }
    fn resize(&mut self, width: u32, height: u32) {
        self.minimized = width == 0 || height == 0;
        self.width = width.max(1);
        self.height = height.max(1);
        if width == 0 || height == 0 {
            return;
        }
        let (Some(gpu), Some(surface), Some(config)) = (&self.gpu, &self.surface, &mut self.config)
        else {
            return;
        };
        config.width = width;
        config.height = height;
        surface.configure(&gpu.device, config);
        self.depth = Some(gpu.texture(width, height, true));
        self.multisample = gpu.multisample_texture(width, height);
        if let Some(atmosphere) = &mut self.atmosphere {
            atmosphere.resize(
                &gpu.device,
                width,
                height,
                self.visual_settings.render_scale,
                gpu.sample_count,
            );
        }
    }
    fn render(&mut self) -> Result<()> {
        if self.minimized {
            return Ok(());
        }
        self.poll_edit();
        if self.adaptive_route_started {
            self.drive_edit_test();
        }
        if self.pending_teleport.is_none()
            && self.route_started.elapsed().as_secs_f64() >= self.next_teleport_seconds
        {
            if let Some(destination) = self.teleport_destinations.pop_front() {
                self.begin_teleport(destination)?;
            }
        }
        let now = Instant::now();
        let interval = now.duration_since(self.last_frame);
        let measure_interval = self.adaptive_route_started
            && self.total_frames > self.warmup + self.route_frame_origin;
        self.draw_encode_frames
            .record_interval(measure_interval, interval.as_secs_f64() * 1000.);
        if measure_interval {
            self.stats.record(interval.as_secs_f64() * 1000.);
        }
        let dt = interval.as_secs_f32().min(0.1);
        self.last_frame = now;
        self.update_visual_ui(interval.as_secs_f64() * 1000.)?;
        if !self.benchmark_input_lock && self.grabbed && self.pending_teleport.is_none() {
            let forward = Vec3::new(self.camera.yaw.sin(), 0., self.camera.yaw.cos());
            let right = forward.cross(Vec3::Y);
            let mut movement = Vec3::ZERO;
            for (key, direction) in [
                (KeyCode::KeyW, forward),
                (KeyCode::KeyS, -forward),
                (KeyCode::KeyD, right),
                (KeyCode::KeyA, -right),
                (KeyCode::Space, Vec3::Y),
                (KeyCode::ControlLeft, -Vec3::Y),
                (KeyCode::ControlRight, -Vec3::Y),
            ] {
                if self.keys.contains(&key) {
                    movement += direction;
                }
            }
            let speed: f32 = if self.keys.contains(&KeyCode::ShiftLeft)
                || self.keys.contains(&KeyCode::ShiftRight)
            {
                if self.fbm {
                    80.
                } else {
                    15.
                }
            } else {
                if self.fbm {
                    35.
                } else {
                    3.
                }
            };
            if self.visual_settings.walking && self.world.fbm_terrain().is_some() {
                let horizontal =
                    Vec3::new(movement.x, 0., movement.z).normalize_or_zero() * speed.min(6.);
                self.camera.position = DVec3::from_array(self.walking.advance_presented(
                    &self.world,
                    self.camera.position.to_array(),
                    [horizontal.x as f64, horizontal.z as f64],
                    self.keys.contains(&KeyCode::Space),
                    dt as f64,
                    self.visual_settings.step_smoothing,
                ));
            } else {
                self.camera.position +=
                    movement.normalize_or_zero().as_dvec3() * (dt * speed) as f64;
            }
        }
        let pose = self.route.pose_at(
            if !self.adaptive_route_started {
                0.
            } else if self.route_realtime {
                self.route_started.elapsed().as_secs_f64()
            } else {
                self.total_frames.saturating_sub(self.route_frame_origin) as f64 * self.route_step
            },
            self.initial_camera.position.to_array(),
            self.initial_camera.yaw as f64,
            self.initial_camera.pitch as f64,
            self.route_speed,
        );
        if self.adaptive_route_started
            && self.pending_teleport.is_none()
            && !matches!(self.route, frontier_runtime::routes::Route::Stationary)
        {
            if self.route.grounded() {
                let t = if self.route_realtime {
                    self.route_started.elapsed().as_secs_f64()
                } else {
                    self.total_frames.saturating_sub(self.route_frame_origin) as f64
                        * self.route_step
                };
                let previous = self.route.pose_at(
                    (t - dt as f64).max(0.),
                    self.initial_camera.position.to_array(),
                    self.initial_camera.yaw as f64,
                    self.initial_camera.pitch as f64,
                    self.route_speed,
                );
                let velocity = [
                    (pose.position[0] - previous.position[0]) / dt.max(0.000001) as f64,
                    (pose.position[2] - previous.position[2]) / dt.max(0.000001) as f64,
                ];
                self.camera.position = DVec3::from_array(self.walking.advance_presented(
                    &self.world,
                    self.camera.position.to_array(),
                    velocity,
                    false,
                    dt as f64,
                    self.visual_settings.step_smoothing,
                ));
            } else {
                self.camera.position = DVec3::from_array(pose.position);
                if self.fbm || self.hybrid {
                    let floor = self
                        .world
                        .fbm_terrain()
                        .unwrap()
                        .height_m(self.camera.position.x, self.camera.position.z)
                        + if self.hybrid { 1.7 } else { 10. };
                    self.camera.position.y = self.camera.position.y.max(floor);
                }
            }
            self.camera.yaw = pose.yaw as f32;
            self.camera.pitch = pose.pitch as f32;
        }
        let terrain_started = Instant::now();
        self.poll_terrain();
        let terrain_ms = terrain_started.elapsed().as_secs_f64() * 1000.;
        let plants_started = Instant::now();
        if let Some(gpu) = &mut self.gpu {
            let (width, height) = self
                .atmosphere
                .as_ref()
                .map_or((self.width, self.height), |a| a.dimensions());
            gpu.update_vegetation(&self.world, self.camera, width, height)?;
        }
        let vegetation_ms = plants_started.elapsed().as_secs_f64() * 1000.;
        if let Some(pages) = self.gpu.as_ref().and_then(|g| g.pages.as_ref()) {
            if !pages.contains(self.camera.position, self.camera.far) {
                return Err("page experiment camera outran its coherent finite cover; readiness failed (no incomplete cover presented)".into());
            }
        }
        if measure_interval {
            if let Some(view) = &self.adaptive_view {
                self.adaptive_route_quality.record(view);
            }
        }
        if let Some(error) = &self.error {
            return Err(error.clone().into());
        }
        if !self.adaptive_route_started && self.adaptive_wait_quality {
            if let Some(view) = &self.adaptive_view {
                if (!view.refinement_ready
                    || self.gpu.as_ref().is_some_and(|g| {
                        g.vegetation_stats.pending_jobs > 0
                            || g.vegetation_stats.coverage_unresolved > 0
                    }))
                    && (view.preparation_refused() || view.preparation_elapsed_seconds() >= 120.)
                {
                    return Err(format!(
                        "adaptive settled benchmark could not meet initial quality: {}",
                        view.json()
                    )
                    .into());
                }
            }
        }
        if !self.adaptive_route_started
            && (!self.adaptive_wait_quality
                || self.gpu.as_ref().is_none_or(|g| {
                    g.vegetation_stats.pending_jobs == 0
                        && g.vegetation_stats.coverage_unresolved == 0
                }))
            && self.adaptive_view.as_ref().is_some_and(|v| {
                v.coverage_ready && (!self.adaptive_wait_quality || v.refinement_ready)
            })
        {
            self.adaptive_route_started = true;
            self.route_started = Instant::now();
            self.route_frame_origin = self.total_frames;
            self.stats = frontier_runtime::FrameStats::new(10000);
            self.draw_encode_frames = DrawEncodeFrames::new();
            if let Some(probe) = &mut self.gpu_probe {
                probe.stats = frontier_runtime::FrameStats::new(10000);
                probe.full_stats = frontier_runtime::FrameStats::new(10000);
                probe.plant_color_stats = frontier_runtime::FrameStats::new(10000);
                probe.plant_shadow_stats = frontier_runtime::FrameStats::new(10000);
            }
            eprintln!(
                "adaptive route starts after {} publication",
                if self.adaptive_wait_quality {
                    "settled quality with refinement reserve"
                } else {
                    "coarse coverage"
                }
            );
        }
        if let Some(terrain) = self.gpu.as_ref().and_then(|g| g.terrain.as_ref()) {
            let center = terrain.center();
            self.terrain_center_lag_end_m =
                (self.camera.position.x - center[0]).hypot(self.camera.position.z - center[1]);
            self.terrain_center_lag_max_m = self
                .terrain_center_lag_max_m
                .max(self.terrain_center_lag_end_m);
        }
        let (Some(gpu), Some(surface), Some(config), Some(depth)) =
            (&self.gpu, &self.surface, &self.config, &self.depth)
        else {
            return Ok(());
        };
        let window = self.window.as_ref().unwrap();
        // Resize events own dimensions. X11 inner_size() performs a server
        // round trip and can serialize CPU preparation against presentation.
        if self.minimized {
            return Ok(());
        }
        let benchmark_preparing = self.adaptive_wait_quality && !self.adaptive_route_started;
        if benchmark_preparing {
            // Explicit settled-benchmark mode gives source preparation the UMA
            // budget. Normal interactive coarse startup still renders normally.
            gpu.queue.submit(std::iter::empty());
            gpu.device.poll(wgpu::Maintain::Poll);
            if self
                .last_preparation_present
                .is_some_and(|last| last.elapsed().as_secs_f64() < 1.)
            {
                std::thread::sleep(std::time::Duration::from_millis(50));
                return Ok(());
            }
            self.last_preparation_present = Some(Instant::now());
        }
        let preparation_ms = now.elapsed().as_secs_f64() * 1000.;
        let frame = match surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                surface.configure(&gpu.device, config);
                return Ok(());
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        if benchmark_preparing {
            let target = frame.texture.create_view(&Default::default());
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("settled benchmark preparation clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.34,
                                g: 0.57,
                                b: 0.80,
                                a: 1.,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
            }
            gpu.queue.submit(Some(encoder.finish()));
            frame.present();
            std::thread::sleep(std::time::Duration::from_millis(50));
            return Ok(());
        }
        let timing = self.gpu_probe.as_mut().and_then(|probe| {
            probe.poll(&gpu.device);
            probe.acquire(
                self.adaptive_route_started
                    && self.total_frames >= self.warmup + self.route_frame_origin,
            )
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        if let (Some(probe), Some(slot)) = (&self.gpu_probe, timing) {
            probe.begin_frame(&mut encoder, slot);
        }
        let target = frame.texture.create_view(&Default::default());
        let multisample_view = self
            .multisample
            .as_ref()
            .map(|t| t.create_view(&Default::default()));
        if let Some(atmosphere) = &self.atmosphere {
            let (width, height) = atmosphere.dimensions();
            atmosphere.set_medium_ground_height(gpu.medium_ground_height);
            atmosphere.prepare(
                &gpu.queue,
                self.camera
                    .matrix(self.camera.position, width as f32 / height as f32)
                    .inverse(),
                Vec3::ZERO,
                self.camera.position,
                &self.visual_settings,
                self.visual_started.elapsed().as_secs_f32(),
            );
            gpu.draw(
                &mut encoder,
                atmosphere.msaa_view().unwrap_or(atmosphere.scene_view()),
                atmosphere.msaa_view().map(|_| atmosphere.scene_view()),
                atmosphere.depth_view(),
                &self.camera,
                width,
                height,
                timing.and_then(|slot| self.gpu_probe.as_ref().map(|p| p.writes(slot))),
                timing.and_then(|slot| {
                    self.gpu_probe
                        .as_ref()
                        .and_then(|p| p.vegetation_writes(slot))
                }),
            );
            atmosphere.encode_with_shadows(
                &mut encoder,
                &target,
                gpu.adaptive.as_ref().map(|a| a.near_shadow_bind()),
            );
        } else {
            gpu.draw(
                &mut encoder,
                multisample_view.as_ref().unwrap_or(&target),
                multisample_view.as_ref().map(|_| &target),
                &depth.create_view(&Default::default()),
                &self.camera,
                config.width,
                config.height,
                timing.and_then(|slot| self.gpu_probe.as_ref().map(|p| p.writes(slot))),
                timing.and_then(|slot| {
                    self.gpu_probe
                        .as_ref()
                        .and_then(|p| p.vegetation_writes(slot))
                }),
            );
        }
        if let Some(ui) = &self.settings_ui {
            ui.render(&mut encoder, &target);
        }
        if let (Some(probe), Some(slot)) = (&self.gpu_probe, timing) {
            probe.end_frame(&mut encoder, slot);
            probe.resolve(&mut encoder, slot);
        }
        gpu.queue.submit(Some(encoder.finish()));
        if let (Some(probe), Some(slot)) = (&mut self.gpu_probe, timing) {
            probe.submitted(slot);
        }
        frame.present();
        self.frame_trace.record(playability::Observation {
            seconds: self.visual_started.elapsed().as_secs_f64(),
            frame_ms: interval.as_secs_f64() * 1000.,
            prepare_ms: preparation_ms,
            terrain_ms,
            vegetation_ms,
            yaw: self.camera.yaw,
            eye: self.camera.position.to_array(),
            physical_eye: if self.visual_settings.walking {
                self.walking.physical_eye()
            } else {
                None
            },
            terrain_pending: self.adaptive_view.as_ref().map_or(0, |v| v.pending_count),
            terrain_ready: self.adaptive_view.as_ref().is_none_or(|v| v.quality_ready),
            native_clusters: self.adaptive_view.as_ref().map_or(0, |v| v.native_clusters),
            plant_pending: gpu.vegetation_stats.pending_jobs,
            plant_triangles: gpu.vegetation_stats.triangles,
            plant_tree_lods: gpu.vegetation_stats.tree_lod_histogram,
            plant_lod_changes: gpu.vegetation_stats.tree_lod_changes,
            plant_coarsenings: gpu.vegetation_stats.tree_coarsenings,
            plant_demand: gpu.vegetation_stats.demanded_by_species,
            plant_ready: gpu.vegetation_stats.ready_by_species,
            shadow_admitted: gpu
                .vegetation
                .as_ref()
                .map_or(0, |v| v.stats().shadow_admitted),
            shadow_removed: gpu
                .vegetation
                .as_ref()
                .map_or(0, |v| v.stats().shadow_removed),
            shadow_triangles: gpu
                .vegetation
                .as_ref()
                .map_or(0, |v| v.stats().shadow_triangles),
            shadow_lod_changes: gpu.vegetation_stats.shadow_lod_changes,
            shadow_lods: gpu.vegetation_stats.shadow_lod_histogram,
            plant_selected_mesh_evictions: gpu.vegetation_stats.selected_mesh_evictions,
            plant_cpu_bytes: gpu.vegetation_stats.total_cpu_geometry_bytes,
        });
        self.draw_encode_frames.pending_ms = Some(gpu.last_draw_encode_cpu_ms.get());
        if let Some(ui) = &mut self.settings_ui {
            ui.finish_frame();
        }
        self.frames += 1;
        self.total_frames += 1;
        let elapsed = now.duration_since(self.fps_start).as_secs_f64();
        if elapsed >= 0.5 {
            if let Some(view) = &self.adaptive_view {
                let state = if self.pending_edit.is_some() {
                    "edit building"
                } else if !view.coverage_ready {
                    "loading coverage"
                } else if view.quality_ready {
                    "quality ready"
                } else {
                    "refining"
                };
                let triangles = view.stats.triangles
                    + if gpu.volume_owner {
                        u64::from(gpu.index_count / 3)
                    } else {
                        0
                    };
                window.set_title(&format!(
                    "Frontier adaptive clusters{} | {:.0} FPS | {} triangles | {} px error | {} | Esc settings / F12",
                    if gpu.volume_owner { " + exact cave/bridge owner" } else { "" },
                    self.frames as f64 / elapsed, triangles, view.pixel_error, state
                ));
            } else {
                window.set_title(&format!("Frontier {} | {:.0} FPS | {} triangles | center aim: L remove / R stone | {} | Esc settings / F12", if self.fbm { "FBM experimental rings (surface only; bounded coverage)" } else { if self.hybrid {if self.world.topology_base_cells().is_some() { "bounded exact cave/bridge owner + moving terrain" } else { "moving 64m native chunks + coherent collar + GPU proxy (heightfield only)" }} else if self.world.fbm_terrain().is_some() {"exact FBM native crop (cutaway; edits disabled)"} else {"native fixture"} }, self.frames as f64 / elapsed, gpu.terrain.as_ref().map_or(gpu.index_count, |t| t.index_count) / 3, if self.pending_teleport.is_some() { "preparing teleport destination" } else if self.pending_edit.is_some() { "edit building" } else { "ready" }));
            }
            self.frames = 0;
            self.fps_start = now;
        }
        Ok(())
    }
}
impl ApplicationHandler for Viewer {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let setup = (|| -> Result<()> {
            let window = Arc::new(
                event_loop.create_window(
                    Window::default_attributes()
                        .with_title("Frontier native fixture — click to capture mouse")
                        .with_inner_size(winit::dpi::PhysicalSize::new(self.width, self.height)),
                )?,
            );
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
            let surface = instance.create_surface(window.clone())?;
            let mut gpu = pollster::block_on(Gpu::new(
                &instance,
                Some(&surface),
                None,
                self.mesh.as_ref().unwrap(),
                self.sample_count,
                self.voxel_filter,
                self.world
                    .fbm_terrain()
                    .is_some_and(|s| s.params().recipe == frontier_world::FbmRecipe::Alpine),
            ))?;
            gpu.init_summary_contention(&self.world)?;
            if let Some(snapshot) = self.brick_initial.take() {
                gpu.install_bricks(&snapshot, brick_skip_mode(&self.renderer))?;
            }
            gpu.configure_visuals(&self.world, &self.visual_settings);
            self.gpu_probe = gpu_probe::GpuProbe::new(&gpu.device, &gpu.queue);
            if let Some(view) = &mut self.adaptive_view {
                view.poll(&mut gpu, self.camera, self.width, self.height)?;
            }
            if let Some(data) = self.terrain_data.take() {
                if self.surface_layout == "quadtree" {
                    let probe = surface_compare::SurfaceCompareGpu::new(
                        &gpu.device,
                        gpu.scene_format,
                        gpu.sample_count,
                        &data.data,
                    );
                    probe.set_voxel_filter(gpu.voxel_filter);
                    gpu.surface_probe = Some(probe);
                }
                if gpu.surface_probe.is_none() {
                    let mut terrain = terrain::TerrainGpu::new_with_samples(
                        &gpu.device,
                        gpu.scene_format,
                        &data.data,
                        gpu.sample_count,
                    );
                    terrain.set_voxel_filter(gpu.voxel_filter);
                    terrain.upload(&gpu.queue, &data);
                    gpu.terrain = Some(terrain);
                }
            }
            gpu.volume_owner = self.world.topology_base_cells().is_some();
            if let Some(prepared) = self.hybrid_initial.take() {
                gpu.init_hybrid(prepared)?;
                if let Some(stream) = &mut self.hybrid_stream {
                    stream.published(gpu.hybrid.as_ref().unwrap().center());
                }
            }
            if let Some(cover) = self.page_initial.take() {
                gpu.init_pages(&cover);
            }
            let size = window.inner_size();
            let config = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: gpu.format,
                width: size.width.max(1),
                height: size.height.max(1),
                present_mode: if self.uncapped {
                    wgpu::PresentMode::AutoNoVsync
                } else {
                    wgpu::PresentMode::Fifo
                },
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: vec![],
            };
            surface.configure(&gpu.device, &config);
            self.depth = Some(gpu.texture(config.width, config.height, true));
            self.multisample = gpu.multisample_texture(config.width, config.height);
            if self.visual_menu_enabled {
                self.settings_ui = Some(settings_ui::SettingsUi::new(
                    &window,
                    &gpu.device,
                    gpu.format,
                    self.visual_settings.clone(),
                ));
            }
            if self.visual_menu_enabled
                || self.visual_settings.atmosphere
                || self.visual_settings.render_scale != 1.
                || gpu.scene_format != gpu.format
            {
                self.atmosphere = Some(atmosphere::Atmosphere::new_with_scene_format(
                    &gpu.device,
                    gpu.format,
                    config.width,
                    config.height,
                    self.visual_settings.render_scale,
                    gpu.sample_count,
                    gpu.scene_format,
                ));
            }
            self.config = Some(config);
            self.gpu = Some(gpu);
            self.surface = Some(surface);
            self.window = Some(window);
            self.mesh = None;
            self.last_frame = Instant::now();
            self.route_started = self.last_frame;
            Ok(())
        })();
        if let Err(error) = setup {
            self.error = Some(error.to_string());
            event_loop.exit();
        }
    }
    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if !self.benchmark_input_lock && self.grabbed && self.pending_teleport.is_none() {
            if let DeviceEvent::MouseMotion { delta } = event {
                self.camera.yaw -= delta.0 as f32 * 0.002;
                self.camera.pitch = (self.camera.pitch - delta.1 as f32 * 0.002).clamp(-1.55, 1.55);
            }
        }
    }
    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.visual_input(&event) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                if self.benchmark_input_lock {
                    self.error = Some("benchmark interrupted by window close".into());
                }
                event_loop.exit();
            }
            WindowEvent::Resized(size) => self.resize(size.width, size.height),
            WindowEvent::Occluded(value) => self.minimized = value,
            WindowEvent::Focused(false) => {
                self.keys.clear();
                self.grab(false);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                if self.benchmark_input_lock {
                    return;
                }
                if !self.grabbed {
                    self.grab(true);
                } else if button == MouseButton::Left || button == MouseButton::Right {
                    self.edit(button == MouseButton::Right);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(key) = event.physical_key {
                    if self.benchmark_input_lock {
                        if key == KeyCode::Escape && event.state == ElementState::Pressed {
                            self.error = Some("benchmark interrupted by Escape".into());
                            event_loop.exit();
                        }
                        return;
                    }
                    if event.state == ElementState::Pressed {
                        self.keys.insert(key);
                        if key == KeyCode::KeyF && !event.repeat {
                            self.visual_settings.walking = !self.visual_settings.walking;
                            self.walking.reset();
                            if let Some(ui) = &mut self.settings_ui {
                                ui.set_walking(self.visual_settings.walking);
                            }
                        }
                        if key == KeyCode::KeyT && !event.repeat && self.hybrid_stream.is_some() {
                            let home = [
                                self.initial_camera.position.x,
                                self.initial_camera.position.z,
                            ];
                            let target = if (self.camera.position.x - home[0]).abs() > 100. {
                                home
                            } else {
                                [home[0] + 512., home[1]]
                            };
                            if let Err(error) = self.begin_teleport(target) {
                                self.error = Some(error.to_string());
                            }
                        }
                        if key == KeyCode::Escape {
                            self.grab(false);
                        }
                        if key == KeyCode::F12 && !event.repeat {
                            let path = PathBuf::from(format!(
                                "frontier-capture-{:03}.png",
                                self.capture_number
                            ));
                            self.capture_number += 1;
                            if let (Some(gpu), Some(config)) = (&self.gpu, &self.config) {
                                if let Err(error) =
                                    gpu.capture(&self.camera, &path, config.width, config.height)
                                {
                                    eprintln!("capture failed: {error}");
                                }
                            }
                        }
                    } else {
                        self.keys.remove(&key);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.render() {
                    self.error = Some(error.to_string());
                    event_loop.exit();
                }
                if self.adaptive_route_started
                    && (self.frame_limit.is_some_and(|limit| {
                        self.total_frames.saturating_sub(self.route_frame_origin) >= limit
                    }) || self
                        .seconds
                        .is_some_and(|limit| self.route_started.elapsed().as_secs_f64() >= limit))
                {
                    event_loop.exit();
                }
            }
            _ => (),
        }
    }
    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

fn capture_pose(
    camera: &mut Camera,
    world: &World,
    route: frontier_runtime::routes::Route,
    speed: f64,
    time: Option<f64>,
    hybrid: bool,
) {
    if let Some(time) = time {
        let pose = route.pose_at(
            time,
            camera.position.to_array(),
            camera.yaw as f64,
            camera.pitch as f64,
            speed,
        );
        camera.position = DVec3::from_array(pose.position);
        camera.yaw = pose.yaw as f32;
        camera.pitch = pose.pitch as f32;
        camera.position.y = camera.position.y.max(
            world.height_m(camera.position.x, camera.position.z) + if hybrid { 1.7 } else { 10. },
        );
    }
}

/// Runs the interactive fixture or captures it without a window using `--capture PATH`.
/// Camera movement never rebuilds geometry; edits rebuild on one bounded background worker.
pub fn run() -> Result<()> {
    appearance_method::initialize_application();
    let Some(options::Options {
        atmosphere: atmosphere_option,
        visual,
        render_scale,
        capture,
        capture_on_exit,
        seconds,
        width,
        height,
        frame_limit,
        scene,
        camera_preset,
        seed,
        start_xz,
        params,
        route,
        uncapped,
        warmup,
        report_path,
        route_step,
        route_speed,
        route_realtime,
        terrain_static,
        view_levels,
        page_spacing,
        msaa,
        capture_route_time,
        capture_coverage_shift,
        voxel_filter,
        renderer,
        surface_layout,
        teleport_test,
        edit_test,
        eye,
        target: explicit_target,
        fov_y,
        capture_linear,
        capture_far,
        pixel_error,
        capture_unresolved,
        adaptive_wait_quality,
        adaptive_cache_mib,
        adaptive_workers,
        adaptive_refinements,
        adaptive_direct_native,
        adaptive_build_mib,
        adaptive_disk_cache,
        adaptive_disk_mib,
        benchmark_input_lock,
        native_bins,
    }) = options::parse()?
    else {
        return Ok(());
    };
    let visual_menu_enabled =
        scene == "adaptive" && !benchmark_input_lock && !adaptive_wait_quality;
    let visual_disk_cache = adaptive_disk_cache.clone();
    let mut visual_settings = settings::VisualSettings {
        pixel_error,
        view_distance: capture_far.unwrap_or(20000.),
        render_scale,
        msaa,
        workers: adaptive_workers,
        refinements: adaptive_refinements,
        cache_mib: adaptive_cache_mib,
        voxel_filter,
        atmosphere: atmosphere_option.unwrap_or(
            params.recipe != frontier_world::FbmRecipe::Legacy
                && !capture_linear
                && !benchmark_input_lock
                && !adaptive_wait_quality,
        ),
        ..visual
    };
    if capture_linear {
        visual_settings.atmosphere = false;
        visual_settings.render_scale = 1.;
    }
    if visual_settings.beauty
        && params.recipe != frontier_world::FbmRecipe::Legacy
        && !capture_linear
    {
        appearance_method::initialize_meadow_application();
    }
    let world = if scene == "hybrid-topology"
        || scene == "adaptive-topology"
        || scene == "pages-topology"
    {
        World::fbm_topology(seed, params)?
    } else if scene == "fixture" {
        World::new(seed)
    } else {
        World::fbm(seed, params)?
    };
    let world = world.with_vegetation_enabled(visual_settings.vegetation);
    if world.vegetation_enabled() {
        frontier_world::vegetation::prewarm_models();
    }
    let fbm = match scene.as_str() {
        "fbm" => true,
        "fixture" | "fbm-native" | "hybrid" | "hybrid-static" | "hybrid-topology" | "adaptive"
        | "adaptive-topology" | "pages" | "pages-topology" => false,
        _ => return Err("--scene requires fixture|fbm|fbm-native|hybrid|hybrid-static".into()),
    };
    let mut adaptive_view = None;
    let mut terrain_data = None;
    let mut terrain_streamer = None;
    let mut terrain_prepare_ms = 0.;
    let mut hybrid_stream = None;
    let mut hybrid_initial = None;
    let mut camera = Camera::fixture();
    let [start_x, start_z] = start_xz;
    let mesh = if scene.starts_with("adaptive") {
        camera.position = DVec3::new(start_x, world.height_m(start_x, start_z) + 1.7, start_z);
        camera.yaw = 0.;
        camera.pitch = -0.08;
        if scene == "adaptive-topology" {
            let base = world.topology_base_cells().unwrap() as f64 * VOXEL_SIZE;
            let pose = match camera_preset.as_str() {
                "cave" => Some((
                    DVec3::new(-3., base - 2., -6.8),
                    DVec3::new(-3., base - 2., 1.),
                )),
                "bridge" => Some((
                    DVec3::new(4., base - 2., -2.8),
                    DVec3::new(4., base + 1., -1.5),
                )),
                "topology" => Some((
                    DVec3::new(12., base + 10., -15.),
                    DVec3::new(0., base - 1., -1.),
                )),
                _ => None,
            };
            if let Some((eye, target)) = pose {
                let direction = (target - eye).normalize();
                camera.position = eye;
                camera.yaw = direction.x.atan2(direction.z) as f32;
                camera.pitch = direction.y.asin() as f32;
            }
        }
        if camera_preset == "alpine" {
            let target = DVec3::new(0., 650., 3300.);
            let direction = (target - camera.position).normalize();
            camera.yaw = direction.x.atan2(direction.z) as f32;
            camera.pitch = direction.y.asin() as f32;
        }
        if camera_preset == "aerial" {
            camera.position.y += 300.;
            camera.pitch = -0.3;
        }
        capture_pose(
            &mut camera,
            &world,
            route,
            route_speed.unwrap_or(3.),
            capture_route_time,
            true,
        );
        camera.set_explicit_pose(eye, explicit_target);
        // Final capture clip and projection are applied before first worker demand.
        camera.far = capture_far.unwrap_or(20000.);
        camera.fov_y = fov_y;
        adaptive_view = Some(adaptive_view::AdaptiveView::new(
            world.clone(),
            pixel_error,
            adaptive_cache_mib * 1024 * 1024,
            native_bins,
            adaptive_workers,
            adaptive_refinements,
            adaptive_direct_native,
            adaptive_build_mib * 1024 * 1024,
            adaptive_disk_cache.map(|path| adaptive_stream::DiskCacheConfig {
                path,
                budget_bytes: (adaptive_disk_mib * 1024 * 1024) as u64,
                max_entry_bytes: 64 * 1024 * 1024,
            }),
        )?);
        if scene == "adaptive-topology" {
            volume::build(&world)?
        } else {
            mesh::Mesh {
                vertices: vec![],
                indices: vec![],
                origin: Cell { x: 0, y: 0, z: 0 },
                exposed_native_faces: 0,
            }
        }
    } else if fbm {
        let source = world.fbm_terrain().unwrap();
        let (position, target) = match camera_preset.as_str() {
            "valley" => (
                DVec3::new(
                    start_x,
                    source.height_m(start_x, start_z - 20.) + 8.,
                    start_z - 20.,
                ),
                DVec3::new(
                    start_x + 100.,
                    source.height_m(start_x + 100., start_z + 400.) + 70.,
                    start_z + 400.,
                ),
            ),
            "aerial" => (
                DVec3::new(
                    start_x + 800.,
                    source.height_m(start_x + 800., start_z - 900.) + 600.,
                    start_z - 900.,
                ),
                DVec3::new(
                    start_x,
                    source.height_m(start_x, start_z + 200.),
                    start_z + 200.,
                ),
            ),
            _ => return Err("--camera requires valley|aerial".into()),
        };
        let direction = (target - position).normalize();
        camera = Camera {
            position,
            far: 20000.,
            fov_y: 60.,
            yaw: direction.x.atan2(direction.z) as f32,
            pitch: direction.y.asin() as f32,
        };
        capture_pose(
            &mut camera,
            &world,
            route,
            route_speed.unwrap_or(80.),
            capture_route_time,
            false,
        );
        camera.set_explicit_pose(eye, explicit_target);
        let preparing = Instant::now();
        let mut streamer = terrain::TerrainStreamer::with_levels(source.clone(), view_levels)?;
        streamer.request([camera.position.x, camera.position.z]);
        eprintln!("preparing initial coherent FBM coverage on background worker");
        terrain_data = Some(loop {
            if let Some(update) = streamer.poll() {
                break update;
            }
            if preparing.elapsed().as_secs() >= 120 {
                return Err("initial FBM coverage did not become ready within 120 seconds".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        });
        terrain_prepare_ms = preparing.elapsed().as_secs_f64() * 1000.;
        terrain_streamer = Some(streamer);
        eprintln!("initial FBM coverage ready in {:.1}ms", terrain_prepare_ms);
        mesh::Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
            origin: Cell { x: 0, y: 0, z: 0 },
            exposed_native_faces: 0,
        }
    } else if scene.starts_with("hybrid") || scene.starts_with("pages") {
        let base = world.topology_base_cells().unwrap_or(0) as f64 * VOXEL_SIZE;
        let (position, target) = match camera_preset.as_str() {
            "cave" if scene == "hybrid-topology" || scene == "pages-topology" => (
                DVec3::new(-3., base - 2., -6.8),
                DVec3::new(-3., base - 2., 1.),
            ),
            "bridge" if scene == "hybrid-topology" || scene == "pages-topology" => (
                DVec3::new(4., base - 2., -2.8),
                DVec3::new(4., base + 1., -1.5),
            ),
            "topology" if scene == "hybrid-topology" || scene == "pages-topology" => (
                DVec3::new(12., base + 10., -15.),
                DVec3::new(0., base - 1., -1.),
            ),
            "valley" => {
                let position = DVec3::new(start_x, world.height_m(start_x, start_z) + 1.7, start_z);
                // Startup-only canonical probes choose an open/downhill view;
                // keep the eye at walking height rather than lifting it over terrain.
                let yaw = (0..16)
                    .map(|i| {
                        let yaw = i as f64 * std::f64::consts::TAU / 16.;
                        let obstruction = [4., 16., 64., 256.]
                            .into_iter()
                            .map(|distance| {
                                (world.height_m(
                                    start_x + yaw.sin() * distance,
                                    start_z + yaw.cos() * distance,
                                ) - position.y)
                                    .atan2(distance)
                            })
                            .fold(f64::NEG_INFINITY, f64::max);
                        (yaw, obstruction)
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .unwrap()
                    .0;
                eprintln!(
                    "hybrid walking camera: open-view yaw {:.1} degrees, eye {:.3}m",
                    yaw.to_degrees(),
                    position.y
                );
                (
                    position,
                    position + DVec3::new(yaw.sin() * 40., -2., yaw.cos() * 40.),
                )
            }
            "aerial" => (
                DVec3::new(
                    start_x + 70.,
                    world.height_m(start_x, start_z) + 90.,
                    start_z - 70.,
                ),
                DVec3::new(start_x, world.height_m(start_x, start_z), start_z),
            ),
            _ => return Err("--camera requires valley|aerial".into()),
        };
        let direction = (target - position).normalize();
        camera = Camera {
            position,
            yaw: direction.x.atan2(direction.z) as f32,
            pitch: direction.y.asin() as f32,
            far: (terrain::half_extent(view_levels) * 3.) as f32,
            fov_y: 60.,
        };
        capture_pose(
            &mut camera,
            &world,
            route,
            route_speed.unwrap_or(3.),
            capture_route_time,
            true,
        );
        camera.set_explicit_pose(eye, explicit_target);
        if scene == "hybrid-static" {
            hybrid::build(&world, view_levels)?
        } else {
            let preparing = Instant::now();
            let mut stream = hybrid_gpu::Stream::new(
                if world.topology_base_cells().is_some() {
                    World::fbm(seed, params)?
                } else {
                    world.clone()
                },
                view_levels,
            )?;
            let prepared = loop {
                stream.poll(
                    [
                        camera.position.x + capture_coverage_shift,
                        camera.position.z,
                    ],
                    true,
                )?;
                if let Some(ready) = stream.ready.take() {
                    break ready;
                }
                if preparing.elapsed().as_secs() > 120 {
                    return Err("initial native/proxy cover timed out".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            };
            terrain_prepare_ms = preparing.elapsed().as_secs_f64() * 1000.;
            eprintln!(
                "initial coherent native/proxy/collar ready in {:.1}ms",
                terrain_prepare_ms
            );
            hybrid_initial = Some(prepared);
            hybrid_stream = Some(stream);
            if world.topology_base_cells().is_some() {
                volume::build(&world)?
            } else {
                mesh::Mesh {
                    vertices: Vec::new(),
                    indices: Vec::new(),
                    origin: Cell { x: 0, y: 0, z: 0 },
                    exposed_native_faces: 0,
                }
            }
        }
    } else if scene == "fbm-native" {
        let (mesh, position, target) = native_fbm::build(&world)?;
        let position = DVec3::from_array(position);
        let direction = (DVec3::from_array(target) - position).normalize();
        camera = Camera {
            position,
            far: 20000.,
            fov_y: 60.,
            yaw: direction.x.atan2(direction.z) as f32,
            pitch: direction.y.asin() as f32,
        };
        mesh
    } else {
        let started = Instant::now();
        let mesh = fixture_mesh(&world);
        eprintln!(
            "native fixture: {} faces, {} greedy quads in {:.2}s",
            mesh.exposed_native_faces,
            mesh.indices.len() / 6,
            started.elapsed().as_secs_f64()
        );
        mesh
    };
    if scene == "fixture" {
        let view = match camera_preset.as_str() {
            "cave" => Some((DVec3::new(-3., 3., -7.), DVec3::new(-3., 3., 6.))),
            "bridge" => Some((DVec3::new(4.5, 2.8, -2.5), DVec3::new(4.5, 4.1, -1.5))),
            _ => None,
        };
        if let Some((eye, target)) = view {
            let direction = (target - eye).normalize();
            camera.position = eye;
            camera.yaw = direction.x.atan2(direction.z) as f32;
            camera.pitch = direction.y.asin() as f32;
        }
    }
    let brick_initial = (renderer != "mesh").then(|| {
        let mut snapshot =
            brick_ray::BrickSnapshot::build(|p| world.material(p), ARENA_MIN, ARENA_SIZE);
        if renderer == "brick-directional" {
            snapshot.build_directional_bounds();
        }
        snapshot
    });
    let mesh = if brick_initial.is_some() {
        mesh::Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
            origin: ARENA_MIN,
            exposed_native_faces: 0,
        }
    } else {
        mesh
    };
    camera.far = (terrain::half_extent(view_levels) * 3.).max(20000.) as f32;
    camera.fov_y = fov_y;
    if let Some(far) = capture_far {
        camera.far = far;
    }
    camera.set_explicit_pose(eye, explicit_target);
    let mut page_stream = None;
    let mut page_initial = None;
    if scene.starts_with("pages") {
        camera.far = capture_far.unwrap_or(300. * page_spacing as f32);
        if camera.far as f64 > page_terrain::guaranteed_radius(page_spacing) {
            return Err(format!("page experiment far {}m exceeds guaranteed finite coverage {}m; increase --page-spacing",camera.far,page_terrain::guaranteed_radius(page_spacing)).into());
        }
        let prepared = hybrid_initial
            .as_ref()
            .expect("pages require hybrid native ownership");
        let mut pages =
            page_terrain::Stream::new(world.fbm_terrain().unwrap().clone(), page_spacing);
        pages.request(
            prepared.native.center,
            world.revision(),
            prepared.native.revision,
        );
        let started = Instant::now();
        loop {
            if let Some(cover) = pages.poll() {
                page_initial = Some(cover);
                break;
            }
            if started.elapsed().as_secs() > 120 {
                return Err("initial source pages timed out".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        eprintln!(
            "initial source pages ready in {:.1}ms; spacing={}m, finite guaranteed radius={}m",
            started.elapsed().as_secs_f64() * 1000.,
            page_spacing,
            page_terrain::guaranteed_radius(page_spacing)
        );
        terrain_prepare_ms += started.elapsed().as_secs_f64() * 1000.;
        page_stream = Some(pages);
    }
    if let Some(path) = capture {
        if std::env::var("FRONTIER_CAPTURE_DIAGNOSTICS").as_deref() == Ok("1")
            && (appearance_method::debug_view() != 0
                || !capture_linear
                || msaa != 1
                || visual_settings.atmosphere
                || visual_settings.terrain_shadows)
        {
            return Err("diagnostic capture set requires normal view, --capture-linear --msaa 1, atmosphere/shadows off".into());
        }
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let mut gpu = pollster::block_on(Gpu::new(
            &instance,
            None,
            capture_linear.then_some(wgpu::TextureFormat::Rgba32Float),
            &mesh,
            msaa,
            voxel_filter,
            !capture_linear && params.recipe == frontier_world::FbmRecipe::Alpine,
        ))?;
        gpu.init_summary_contention(&world)?;
        gpu.volume_owner = world.topology_base_cells().is_some();
        if let Some(snapshot) = &brick_initial {
            gpu.install_bricks(snapshot, brick_skip_mode(&renderer))?;
        }
        if let Some(data) = &terrain_data {
            if surface_layout == "quadtree" {
                let probe = surface_compare::SurfaceCompareGpu::new(
                    &gpu.device,
                    gpu.scene_format,
                    gpu.sample_count,
                    &data.data,
                );
                probe.set_voxel_filter(gpu.voxel_filter);
                gpu.surface_probe = Some(probe);
            }
            if gpu.surface_probe.is_none() {
                let mut terrain = terrain::TerrainGpu::new_with_samples(
                    &gpu.device,
                    gpu.scene_format,
                    &data.data,
                    gpu.sample_count,
                );
                terrain.set_voxel_filter(gpu.voxel_filter);
                terrain.upload(&gpu.queue, data);
                gpu.terrain = Some(terrain);
            }
        }
        if let Some(prepared) = hybrid_initial.take() {
            gpu.init_hybrid(prepared)?;
        }
        if let Some(cover) = page_initial.take() {
            gpu.init_pages(&cover);
        }
        gpu.configure_visuals(&world, &visual_settings);
        if let Some(view) = &mut adaptive_view {
            view.settle(&mut gpu, camera, width, height, capture_unresolved)?;
        }
        if gpu
            .pages
            .as_ref()
            .is_some_and(|p| !p.contains(camera.position, camera.far))
        {
            return Err(
                "initial page experiment finite cover cannot cover requested camera/far".into(),
            );
        }
        gpu.settle_vegetation(&world, camera, width, height)?;
        gpu.capture(&camera, &path, width, height)?;
        let metadata_path = path.with_extension("capture.json");
        let mut metadata = std::fs::read_to_string(&metadata_path)?;
        metadata.pop(); // capture writes exactly one JSON object.
        let bounds = if scene == "fixture" {
            ",\"native_min_cell\":[-128,-32,-128],\"native_size_cells\":[256,224,256]"
        } else {
            ""
        };
        let source_version = world
            .fbm_terrain()
            .map_or(frontier_world::SOURCE_VERSION, |source| {
                source.source_version()
            });
        metadata.push_str(&format!(",\"seed\":{seed},\"scene\":{scene:?},\"renderer\":{renderer:?},\"source_version\":{source_version},\"edit_revision\":{}{bounds},\"native_voxel_m\":{VOXEL_SIZE},\"terrain_recipe\":\"{:?}\",\"horizontal_scale_m\":{},\"amplitude_m\":{},\"octaves\":{},\"roughness\":{},\"appearance_version\":{},\"surface_layout\":{surface_layout:?},\"view_levels\":{view_levels}}}", world.revision(), params.recipe, params.horizontal_scale, params.amplitude, params.octaves, params.roughness, world.fbm_terrain().map_or(0, |source| source.appearance_version())));
        metadata.pop();
        metadata.push_str(&format!(
            ",{}}}",
            renderer_appearance_json(&world, adaptive_view.is_some())
        ));
        if let Some(pages) = &gpu.pages {
            metadata.pop();
            metadata.push_str(&format!(
                ",\"page_experiment\":{}}}",
                pages.json(
                    page_stream
                        .as_ref()
                        .map_or(Default::default(), |s| s.stats()),
                    gpu.terrain
                        .as_ref()
                        .map_or(0, |t| t.resident_buffer_bytes())
                )
            ));
        }
        if let Some(view) = &adaptive_view {
            metadata.pop();
            metadata.push_str(&format!(",\"adaptive\":{}}}", view.json()));
        }
        std::fs::write(metadata_path, &metadata)?;
        if std::env::var("FRONTIER_CAPTURE_DIAGNOSTICS").as_deref() == Ok("1") {
            let view = adaptive_view
                .as_ref()
                .ok_or("diagnostic set requires adaptive terrain")?;
            let directory = path.parent().unwrap_or(Path::new("."));
            for (name, mode) in [
                ("lod", 1u32),
                ("normals", 2),
                ("flat", 3),
                ("paint", 4),
                ("blend", 5),
                ("raw-light", 6),
                ("material", 7),
                ("profile", 8),
            ] {
                let adaptive = gpu
                    .adaptive
                    .as_mut()
                    .ok_or("missing adaptive diagnostic renderer")?;
                adaptive.set_debug_view(mode);
                adaptive.prepare(
                    &gpu.device,
                    &gpu.queue,
                    &camera,
                    width,
                    height,
                    &view.owners,
                    view.pixel_error,
                );
                let destination = directory.join(format!("{name}.png"));
                gpu.capture(&camera, &destination, width, height)?;
                let mut diagnostic =
                    metadata.replace("\"debug_view\":0", &format!("\"debug_view\":{mode}"));
                for version in [2, 3, 4, 6] {
                    diagnostic = diagnostic.replace(
                        &format!("\"render_appearance_version\":{version}"),
                        "\"render_appearance_version\":5",
                    );
                }
                diagnostic.pop();
                diagnostic.push_str(",\"diagnostic_geometry_frozen\":true}");
                std::fs::write(destination.with_extension("capture.json"), diagnostic)?;
            }
        }
        return Ok(());
    }
    let now = Instant::now();
    let adaptive_route_started = adaptive_view.is_none();
    let mut viewer = Viewer {
        visual_settings,
        settings_ui: None,
        atmosphere: None,
        visual_menu_enabled,
        visual_started: now,
        display_frame_ms: 0.,
        visual_build_mib: adaptive_build_mib,
        visual_disk_cache,
        visual_disk_mib: adaptive_disk_mib,
        adaptive_view,
        adaptive_route_quality: Default::default(),
        adaptive_route_started,
        adaptive_wait_quality,
        last_preparation_present: None,
        benchmark_input_lock: benchmark_input_lock || adaptive_wait_quality,
        route_frame_origin: 0,
        renderer,
        surface_layout,
        brick_initial,
        gpu_probe: None,
        pending_teleport: None,
        teleport_destinations: if teleport_test {
            [
                [512., 0.],
                [camera.position.x, camera.position.z],
                [512., 0.],
                [512., 0.],
            ]
            .into()
        } else {
            Default::default()
        },
        next_teleport_seconds: 2.,
        teleport_count: 0,
        teleport_last_ms: 0.,
        teleport_start_jobs: 0,
        teleport_start_upload: 0,
        teleport_records: Vec::new(),
        edit_test: edit_test.then(EditTest::default),
        edit_published_count: 0,
        edit_build_ms: 0.,
        edit_input_to_publish_ms: 0.,
        edit_records: Vec::new(),
        mesh: Some(mesh),
        window: None,
        surface: None,
        gpu: None,
        config: None,
        depth: None,
        multisample: None,
        sample_count: msaa,
        voxel_filter,
        initial_camera: Camera {
            position: camera.position,
            far: camera.far,
            fov_y: camera.fov_y,
            yaw: camera.yaw,
            pitch: camera.pitch,
        },
        camera,
        walking: Default::default(),
        frame_trace: Default::default(),
        terrain_data,
        hybrid_initial,
        page_initial,
        page_stream,
        page_pending: None,
        page_spacing,
        hybrid_stream,
        fbm,
        hybrid: scene == "hybrid"
            || scene == "hybrid-topology"
            || scene.starts_with("adaptive")
            || scene.starts_with("pages"),
        route,
        uncapped,
        warmup,
        route_step,
        route_speed: route_speed.unwrap_or(if fbm {
            80.
        } else if scene.starts_with("hybrid")
            || scene.starts_with("adaptive")
            || scene.starts_with("pages")
        {
            3.
        } else {
            2.
        }),
        route_realtime,
        route_started: now,
        terrain_streamer,
        terrain_static,
        terrain_uploads: 0,
        terrain_upload_bytes: 0,
        terrain_upload_ms: 0.,
        terrain_prepare_ms,
        terrain_center_lag_max_m: 0.,
        terrain_center_lag_end_m: 0.,
        keys: HashSet::new(),
        grabbed: false,
        minimized: false,
        last_frame: now,
        fps_start: now,
        frames: 0,
        capture_number: 0,
        error: None,
        width,
        height,
        frame_limit,
        seconds,
        total_frames: 0,
        stats: frontier_runtime::FrameStats::new(10000),
        draw_encode_frames: DrawEncodeFrames::new(),
        world,
        pending_edit: None,
    };
    EventLoop::new()?.run_app(&mut viewer)?;
    let route_elapsed_seconds = if viewer.adaptive_route_started {
        viewer.route_started.elapsed().as_secs_f64()
    } else {
        0.
    };
    if let Some(path) = capture_on_exit {
        if let (Some(gpu), Some(config)) = (&viewer.gpu, &viewer.config) {
            gpu.capture(&viewer.camera, &path, config.width, config.height)?;
            if let Some(adaptive) = &viewer.adaptive_view {
                let metadata_path = path.with_extension("capture.json");
                let mut metadata = std::fs::read_to_string(&metadata_path)?;
                metadata.pop();
                metadata.push_str(&format!(
                    ",\"scene\":{scene:?},\"seed\":{seed},\"adaptive\":{},{}}}",
                    adaptive.json(),
                    renderer_appearance_json(&viewer.world, true),
                ));
                std::fs::write(metadata_path, metadata)?;
                if !adaptive.quality_ready {
                    eprintln!(
                        "interactive diagnostic capture has unresolved adaptive quality: {}",
                        adaptive.json()
                    );
                }
            }
        }
    }
    eprintln!(
        "frame intervals (includes presentation; not isolated GPU timing): {:?}",
        viewer.stats.summary()
    );
    if let Some(path) = report_path {
        let report = frontier_runtime::benchmark::BenchmarkReport {
            scene: scene.clone(),
            seed,
            route: format!("{:?}", route),
            output_width: viewer.config.as_ref().map_or(width, |c| c.width),
            output_height: viewer.config.as_ref().map_or(height, |c| c.height),
            adapter: viewer
                .gpu
                .as_ref()
                .map(|g| g.adapter_info.clone())
                .unwrap_or_default(),
            vsync: !uncapped,
            warmup_frames: warmup,
            frames: viewer.stats.summary(),
            isolated_gpu_ms: viewer
                .gpu_probe
                .as_ref()
                .and_then(|p| p.stats.summary().p50_ms),
            used_memory_bytes: None,
            committed_memory_bytes: None,
        };
        let mut json = report.to_json();
        json.pop();
        json.push_str(&format!(
            ",\"process_memory\":{}}}",
            playability::process_memory_json()
        ));
        json.pop();
        json.push_str(&format!(",\"benchmark_interrupted\":{},\"benchmark_failed\":{},\"benchmark_requested_seconds\":{},\"benchmark_requested_frames\":{}}}",viewer.error.as_deref().is_some_and(|e|e.starts_with("benchmark interrupted")),viewer.error.is_some(),seconds.map_or("null".into(),|v|v.to_string()),frame_limit.map_or("null".into(),|v|v.to_string())));
        if let Some(view) = &viewer.adaptive_view {
            json.pop();
            json.push_str(&format!(
                ",\"adaptive\":{},\"adaptive_wait_quality\":{},\"adaptive_route_quality\":{},\"adaptive_preparation_policy\":\"{}\",\"benchmark_input_lock\":{}}}",
                view.json(),
                viewer.adaptive_wait_quality,
                viewer.adaptive_route_quality.json(),
                if viewer.adaptive_wait_quality {"clear1Hz_poll20Hz_until_refinement_reserve"} else {"normal_interactive_rendering"},
                viewer.benchmark_input_lock
            ));
        }
        json.pop();
        json.push_str(&format!(
            ",\"terrain_recipe\":\"{:?}\",\"appearance_version\":{},{}}}",
            params.recipe,
            viewer
                .world
                .fbm_terrain()
                .map_or(0, |source| source.appearance_version()),
            renderer_appearance_json(&viewer.world, viewer.adaptive_view.is_some()),
        ));
        json.pop();
        json.push_str(&format!(",\"source\":{{\"version\":{},\"native_voxel_m\":{},\"horizontal_scale_m\":{},\"amplitude_m\":{},\"octaves\":{},\"roughness\":{}}},\"representation\":{{\"name\":\"{}\",\"surface_only\":{},\"fixed_center\":{},\"half_extent_m\":{},\"finest_spacing_m\":{},\"levels\":{},\"triangles\":{},\"gpu_page_bytes\":{}}},\"route_height_policy\":\"FBM proxy routes clamp to canonical surface +10m; hybrid routes clamp +1.7m; freefly unconstrained\",\"route_step_seconds\":{},\"camera_start\":{{\"position\":[{},{},{}],\"yaw\":{},\"pitch\":{}}}}}",
            viewer.world.fbm_terrain().map_or(frontier_world::SOURCE_VERSION, |source| source.source_version()),VOXEL_SIZE,params.horizontal_scale,params.amplitude,params.octaves,params.roughness,
            if scene.starts_with("pages") {"bounded_uniform_reusable_gpu_source_pages_with_native_collar"} else if scene.starts_with("adaptive") {"adaptive_pooled_canonical_clusters"} else if fbm {if viewer.surface_layout=="quadtree" {"bounded_adaptive_quadtree_probe"} else if terrain_static {"experimental_static_nested_surface_grids"} else {"experimental_moving_nested_surface_grids"}} else if scene.starts_with("hybrid") && scene!="hybrid-static" {"moving_chunked_native_64m_coherent_collar_gpu_proxy"} else if scene=="hybrid-static" {"fixed_native_64m_cpu_proxy_reference"} else if viewer.renderer!="mesh" {"exact_native_brick_rays"} else {"exact_native_greedy"},scene=="adaptive" || scene=="pages" || fbm || (scene.starts_with("hybrid") && scene!="hybrid-topology"),(fbm && terrain_static) || scene=="hybrid-static" || (scene=="hybrid" && terrain_static),if scene.starts_with("pages") {page_terrain::HALF_EXTENT_M*page_spacing as f64} else if scene.starts_with("adaptive") {viewer.camera.far as f64} else if fbm || scene.starts_with("hybrid") {terrain::half_extent(view_levels)} else if scene=="fbm-native" {4.} else {8.},if fbm {0.5} else {VOXEL_SIZE},if fbm || scene.starts_with("hybrid") {view_levels} else {1},viewer.gpu.as_ref().map_or(0,|g|g.adaptive.as_ref().map_or_else(||g.surface_probe.as_ref().map_or_else(||g.pages.as_ref().map_or_else(||g.terrain.as_ref().map_or(g.index_count,|t|t.index_count)/3,|_|(page_terrain::MAX_PAGES*page_terrain::SIDE*page_terrain::SIDE*2) as u32)+g.hybrid.as_ref().map_or(0,|h|h.triangles())+if g.volume_owner {g.index_count/3}else{0},|p|p.stats().triangles),|_|viewer.adaptive_view.as_ref().map_or(0,|v|v.stats.triangles as u32)+if g.volume_owner {g.index_count/3}else{0})),if scene.starts_with("pages") {page_terrain::GPU_BYTES} else if fbm || scene.starts_with("hybrid") {terrain::page_bytes(view_levels)} else {0},route_step,viewer.initial_camera.position.x,viewer.initial_camera.position.y,viewer.initial_camera.position.z,viewer.initial_camera.yaw,viewer.initial_camera.pitch));
        if let Some(gpu) = &viewer.gpu {
            json.pop();
            json.push_str(&format!(",{}}}", gpu.vegetation_json()));
        }
        if scene == "hybrid" || scene == "hybrid-topology" {
            json.pop();
            json.push_str(&format!(",\"hybrid_scope\":{{\"native_half_extent_m\":32,\"approximate_collar_outer_m\":34,\"moving_native_ownership\":true,\"edits_supported\":{},\"volume_handoff_passed\":false,\"proxy_error_bound_certified\":false}}}}",scene=="hybrid-topology"));
        }
        let streaming = viewer
            .terrain_streamer
            .as_ref()
            .map(|s| s.metrics())
            .or_else(|| viewer.hybrid_stream.as_ref().map(|s| s.terrain.metrics()))
            .unwrap_or_default();
        json.pop();
        json.push_str(&format!(",\"route_clock\":\"{}\",\"route_elapsed_seconds\":{},\"route_speed_mps\":{},\"streaming\":{{\"initial_preparation_ms\":{},\"uploads\":{},\"upload_bytes\":{},\"upload_cpu_ms\":{},\"camera_to_center_max_m\":{},\"camera_to_center_end_m\":{},\"requests\":{},\"publications\":{},\"stale_rejections\":{},\"generated_pages\":{},\"generated_samples\":{},\"cache_hits\":{},\"reused_vertex_samples\":{},\"cancellations\":{},\"resident_bytes\":{},\"inflight_bytes\":{},\"generation_ms\":{}}}}}",
            if route_realtime { "realtime" } else { "frame" },
            route_elapsed_seconds, viewer.route_speed, viewer.terrain_prepare_ms,
            viewer.terrain_uploads, viewer.terrain_upload_bytes, viewer.terrain_upload_ms,
            viewer.terrain_center_lag_max_m, viewer.terrain_center_lag_end_m,
            streaming.requests, streaming.publications, streaming.stale_rejections,
            streaming.generated_pages, streaming.generated_samples, streaming.cache_hits, streaming.reused_samples,
            streaming.cancellations, streaming.resident_bytes, streaming.inflight_bytes,
            streaming.generation_ms));
        json.pop();
        json.push_str(&format!(
            ",\"msaa_samples\":{},\"voxel_filter\":{}}}",
            viewer.sample_count, viewer.voxel_filter
        ));
        // Record the actual presentation settings, including interactive changes.
        // Terrain pass timestamps exclude atmosphere/UI; frame intervals include them.
        json.pop();
        json.push_str(&format!(
            ",\"experiments\":{{\"native_profile_reuse\":{},\"native_summary_reuse\":{}}}}}",
            adaptive_geometry::native_profile_reuse_enabled(),
            adaptive_geometry::native_summary_reuse_enabled()
        ));
        let visual = &viewer.visual_settings;
        json.pop();
        json.push_str(&format!(
            ",\"visual\":{{\"atmosphere\":{},\"render_scale\":{},\"exposure\":{},\"fog_density\":{},\"fog_height_falloff\":{},\"sun_elevation\":{},\"sun_azimuth\":{},\"cloud_coverage\":{},\"cloud_speed\":{},\"overcast\":{},\"terrain_shadows_requested\":{},\"terrain_shadows_active\":{},\"shadow_map_size\":2048,\"shadow_footprint_m\":2048,\"shadow_casters\":\"published_adaptive_terrain_owners_only\",\"menu_enabled\":{},\"gpu_timing_scope\":\"opaque_scene_pass_without_atmosphere_or_ui\",\"settings_may_have_changed\":{}}},\"projection\":{{\"fov_y_degrees\":{},\"near_m\":0.025,\"far_m\":{}}}}}",
            visual.atmosphere, visual.render_scale, visual.exposure,
            visual.fog_density, visual.fog_height_falloff, visual.sun_elevation,
            visual.sun_azimuth, visual.cloud_coverage, visual.cloud_speed,
            visual.overcast, visual.terrain_shadows, visual.terrain_shadows && scene.starts_with("adaptive") && viewer.world.fbm_terrain().is_some_and(|s| s.params().recipe == frontier_world::FbmRecipe::Alpine) && visual.sun_elevation > 0., viewer.visual_menu_enabled, viewer.visual_menu_enabled,
            viewer.camera.fov_y, viewer.camera.far
        ));
        json.pop();
        json.push_str(&format!(",\"beauty\":{}}}", visual.beauty));
        if let Some(pages) = viewer.gpu.as_ref().and_then(|g| g.pages.as_ref()) {
            json.pop();
            json.push_str(&format!(
                ",\"page_experiment\":{}}}",
                pages.json(
                    viewer
                        .page_stream
                        .as_ref()
                        .map_or(Default::default(), |s| s.stats()),
                    viewer
                        .gpu
                        .as_ref()
                        .and_then(|g| g.terrain.as_ref())
                        .map_or(0, |t| t.resident_buffer_bytes())
                )
            ));
        }
        if let (Some(stream), Some(hybrid)) = (
            &viewer.hybrid_stream,
            viewer.gpu.as_ref().and_then(|g| g.hybrid.as_ref()),
        ) {
            let native = stream.native.metrics();
            json.pop();
            json.push_str(&format!(",\"native_streaming\":{{\"chunk_m\":8,\"core_m\":{},\"requests\":{},\"jobs\":{},\"cache_hits\":{},\"reused_chunks\":{},\"cancellations\":{},\"stale_rejections\":{},\"cpu_cache_chunks\":{},\"cpu_cache_bytes\":{},\"pending_chunks\":{},\"gpu_upload_queue\":{},\"gpu_resident_bytes\":{},\"gpu_chunk_uploads\":{},\"gpu_upload_bytes\":{},\"gpu_upload_cpu_ms\":{},\"coherent_publications\":{},\"last_upload_to_publish_ms\":{},\"last_cpu_generation_ms\":{},\"coverage_lag_end_m\":{}}}}}",native_stream::CORE_METERS,native.requests,native.jobs,native.cache_hits,native.reused_chunks,native.cancellations,native.stale_rejections,native.cache_chunks,native.cache_bytes,native.pending_chunks,hybrid.queued(),hybrid.bytes(),hybrid.uploads,hybrid.upload_bytes,hybrid.upload_ms,hybrid.publications,hybrid.last_publish_ms,native.last_job_ms,viewer.terrain_center_lag_end_m));
        }
        if let Some(hybrid) = viewer.gpu.as_ref().and_then(|g| g.hybrid.as_ref()) {
            json.pop();
            json.push_str(&format!(",\"native_gpu_budget\":{{\"cache_target_bytes\":536870912,\"hard_transition_bytes\":1073741824,\"cache_target_chunks\":128,\"active_and_pending_protected\":true,\"peak_resident_bytes\":{},\"oversized_single_chunk_uploads\":{},\"max_frame_chunk_upload_bytes\":{},\"stale_cover_rejections\":{},\"chunk_cache_hits\":{},\"published_epoch\":{},\"published_center\":[{},{}]}}}}",hybrid.peak_bytes,hybrid.oversized_uploads,hybrid.max_frame_upload_bytes,hybrid.stale_rejections,hybrid.cache_hits,hybrid.revision(),hybrid.center()[0],hybrid.center()[1]));
        }
        if let Some(gpu) = &viewer.gpu {
            json.pop();
            json.push_str(&format!(",\"gpu_summary_contention\":{{\"enabled\":{},\"pages\":{},\"logical_buffer_bytes\":{},\"startup_ms\":{},\"dispatches\":{},\"per_frame_readback\":false}}}}", gpu.summary_contention.is_some(), gpu.summary_contention.as_ref().map_or(0, |p| p.loaded_pages()), gpu.summary_contention.as_ref().map_or(0, |p| p.resident_bytes()), gpu.summary_contention_startup_ms, gpu.summary_contention_dispatches.get()));
        }
        if let Some(gpu) = &viewer.gpu {
            json.pop();
            json.push_str(&format!(",\"draw_encode_cpu\":{{\"total_ms\":{},\"calls\":{},\"includes_warmup_capture\":true}}}}", gpu.draw_encode_cpu_ms.get(), gpu.draw_encode_calls.get()));
        }
        json.pop();
        json.push_str(&format!(
            ",\"draw_encode_cpu_measured\":{}}}",
            viewer.draw_encode_frames.json()
        ));
        json.pop();
        let gpu_stats = viewer.gpu_probe.as_ref().map(|p| p.stats.summary());
        let number = |x: Option<f64>| x.map_or("null".into(), |v| v.to_string());
        json.push_str(&format!(",\"renderer_backend\":\"{}\",\"gpu_render_pass\":{{\"samples\":{},\"p50_ms\":{},\"p95_ms\":{},\"p99_ms\":{}}},\"teleports\":{{\"completed\":{},\"last_readiness_ms\":{}}},\"prediction_horizon_seconds\":{},\"brick_gpu_bytes\":{}}}",
            viewer.renderer, gpu_stats.as_ref().map_or(0,|s| s.sample_count), number(gpu_stats.as_ref().and_then(|s|s.p50_ms)), number(gpu_stats.as_ref().and_then(|s|s.p95_ms)), number(gpu_stats.as_ref().and_then(|s|s.p99_ms)), viewer.teleport_count, viewer.teleport_last_ms,
            number(viewer.hybrid_stream.as_ref().map(|s|s.prediction_horizon_seconds())), viewer.gpu.as_ref().and_then(|g|g.brick.as_ref()).map_or(0,|b|b.resident_bytes)));
        json.pop();
        json.push_str(&format!(
            ",\"frame_observations\":{},\"step_smoothing\":{}}}",
            viewer.frame_trace.json(),
            viewer.visual_settings.step_smoothing
        ));
        let full = viewer
            .gpu_probe
            .as_ref()
            .filter(|p| p.full_supported)
            .map(|p| p.full_stats.summary());
        json.pop();
        json.push_str(&format!(",\"gpu_full_render\":{{\"supported\":{},\"samples\":{},\"p50_ms\":{},\"p95_ms\":{},\"p99_ms\":{},\"scope\":\"GPU command span: shadows, opaque terrain and plants, atmosphere, UI; excludes prior uploads, CPU preparation and presentation; asynchronous unpaired timestamps\"}}}}",
            full.is_some(),full.as_ref().map_or(0,|s|s.sample_count),number(full.as_ref().and_then(|s|s.p50_ms)),number(full.as_ref().and_then(|s|s.p95_ms)),number(full.as_ref().and_then(|s|s.p99_ms))));
        if let Some(probe) = &viewer.gpu_probe {
            let color = probe.plant_color_stats.summary();
            let shadow = probe.plant_shadow_stats.summary();
            json.pop();
            json.push_str(&format!(",\"vegetation_gpu_timing\":{{\"supported\":{},\"color_samples\":{},\"color_p50_ms\":{},\"color_p95_ms\":{},\"shadow_samples\":{},\"shadow_p50_ms\":{},\"shadow_p95_ms\":{},\"scope\":\"asynchronous vegetation color and plant-only shadow spans; excludes terrain and CPU generation\"}}}}", probe.vegetation_supported,color.sample_count,number(color.p50_ms),number(color.p95_ms),shadow.sample_count,number(shadow.p50_ms),number(shadow.p95_ms)));
        }
        if let Some(gpu) = &viewer.gpu {
            let m = &gpu.brick_cache_metrics;
            json.pop();
            json.push_str(&format!(",\"brick_directional_cache\":{{\"builds\":{},\"latest_build_ms\":{},\"total_build_ms\":{},\"gpu_bytes\":{},\"uploaded_bytes\":{},\"latest_scratch_bytes\":{},\"peak_scratch_bytes\":{},\"immutable_revision_publication\":true}}}}", m.builds, m.latest_build_ms, m.total_build_ms, m.latest_gpu_bytes, m.uploaded_bytes, m.latest_scratch_bytes, m.peak_scratch_bytes));
        }
        json.pop();
        json.push_str(&format!(",\"topology_owner\":{{\"enabled\":{},\"world_revision\":{},\"exact_at_all_distances\":{},\"full_G3_passed\":false}}}}",viewer.world.topology_base_cells().is_some(),viewer.world.revision(),viewer.world.topology_base_cells().is_some()));
        if let Some(probe) = viewer.gpu.as_ref().and_then(|g| g.surface_probe.as_ref()) {
            let stats = probe.stats();
            json.pop();
            json.push_str(&format!(",\"surface_comparison\":{{\"layout\":\"quadtree\",\"leaves\":{},\"triangles\":{},\"last_selection_ms\":{},\"page_bytes\":{},\"instance_upload_bytes\":{},\"static_source_pages\":true,\"full_CDLOD\":false}}}}",stats.leaves,stats.triangles,stats.selection_ms,stats.page_bytes,stats.instance_upload_bytes));
        }
        json.pop();
        json.push_str(&format!(
            ",\"teleport_readiness\":[{}],\"gpu_timing_dropped\":{}}}",
            viewer.teleport_records.join(","),
            viewer.gpu_probe.as_ref().map_or(0, |p| p.dropped)
        ));
        json.pop();
        json.push_str(&format!(",\"edits\":{{\"test_enabled\":{},\"test_restored\":{},\"published_count\":{},\"last_build_ms\":{},\"last_input_to_publish_ms\":{},\"world_revision\":{},\"records\":[{}]}}}}",
            viewer.edit_test.is_some(),viewer.edit_test.as_ref().is_some_and(|t|t.phase==3),
            viewer.edit_published_count,viewer.edit_build_ms,viewer.edit_input_to_publish_ms,
            viewer.world.revision(),viewer.edit_records.join(",")));
        std::fs::write(&path, json)?;
        eprintln!("benchmark JSON: {}", path.display());
    }
    if let Some(error) = viewer.error {
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn renderer_appearance_metadata_reports_resolved_world_modes() {
        for recipe in [
            frontier_world::FbmRecipe::Legacy,
            frontier_world::FbmRecipe::Alpine,
        ] {
            let world = frontier_world::World::fbm(
                super::SEED,
                frontier_world::FbmParams {
                    recipe,
                    ..Default::default()
                },
            )
            .unwrap();
            let mode = super::adaptive_geometry::appearance_mode(&world);
            let metadata = super::renderer_appearance_json(&world, true);
            assert!(metadata.contains("\"render_appearance_version\":2"));
            assert!(metadata.contains(&format!("\"spatial_profiles\":{}", mode.spatial_profiles)));
            assert!(metadata.contains(&format!("\"profile_probe_cells\":{}", mode.probe_cells)));
            assert!(metadata.contains(&format!(
                "\"profile_footprint\":\"{}\"",
                if mode.half_footprint { "half" } else { "full" }
            )));
            assert!(metadata.contains(&format!(
                "\"near_detail\":{}",
                super::adaptive_stream::near_detail_setting(&world)
            )));
            assert!(!metadata.contains("\"appearance_version\":"));
            assert_eq!(
                super::renderer_appearance_json(&world, false),
                "\"render_appearance_version\":1"
            );
        }
    }

    #[test]
    fn measured_encode_excludes_warmup_missing_draws_and_reset_history() {
        let mut stats = super::DrawEncodeFrames::new();
        stats.pending_ms = Some(99.);
        stats.record_interval(false, 100.); // settling or warmup
        stats.record_interval(true, 100.); // no presented draw to pair
        assert_eq!(stats.encode.summary().sample_count, 0);
        stats.pending_ms = Some(0.5);
        stats.record_interval(true, 10.);
        assert_eq!(stats.encode.summary().p50_ms, Some(0.5));
        assert_eq!(stats.frame.summary().p50_ms, Some(10.));
        stats.pending_ms = Some(3.);
        stats = super::DrawEncodeFrames::new(); // route publication reset
        stats.record_interval(true, 100.);
        assert_eq!(stats.encode.summary().sample_count, 0);
        assert_eq!(stats.frame.summary().sample_count, 0);
    }

    #[test]
    fn measured_encode_keeps_presented_value_when_capture_overwrites_gpu_timer() {
        let last_gpu_draw = std::cell::Cell::new(0.5);
        let mut stats = super::DrawEncodeFrames::new();
        stats.pending_ms = Some(last_gpu_draw.get());
        last_gpu_draw.set(100.); // an independent capture calls Gpu::draw
        stats.record_interval(true, 10.);
        assert_eq!(stats.encode.summary().p95_ms, Some(0.5));
        assert_eq!(stats.frame.summary().p95_ms, Some(10.));
        assert!(stats.json().contains("\"gpu_samples_paired\":false"));
        assert!(stats.json().contains("\"includes_warmup_capture\":false"));
    }

    #[test]
    fn measured_encode_invalid_intervals_do_not_desynchronize_paired_samples() {
        let mut stats = super::DrawEncodeFrames::new();
        for (encode, frame) in [(1., f64::NAN), (f64::INFINITY, 2.), (-1., 2.)] {
            stats.pending_ms = Some(encode);
            stats.record_interval(true, frame);
        }
        for i in 0..10001 {
            stats.pending_ms = Some(i as f64 / 1000.);
            stats.record_interval(true, i as f64);
        }
        let encode = stats.encode.summary();
        let frame = stats.frame.summary();
        assert_eq!(encode.sample_count, 10000);
        assert_eq!(frame.sample_count, encode.sample_count);
        assert_eq!(encode.accepted_frames, 10001);
        assert_eq!(frame.accepted_frames, encode.accepted_frames);
        assert_eq!(encode.p95_ms, frame.p95_ms.map(|v| v / 1000.));
    }
    #[test]
    fn scripted_cube_snapshot_restores_original_materials_including_existing_edits() {
        use super::*;
        for mut world in [
            World::new(SEED),
            World::fbm_topology(SEED, Default::default()).unwrap(),
        ] {
            let (_, cells) = edit_test_addition(&world);
            assert_eq!(cells.len(), 4096);
            assert!(cells.iter().all(|(c, m)| *m == Material::Air
                && if world.topology_base_cells().is_some() {
                    world.volume_owned(*c)
                } else {
                    inside_arena(*c)
                }));
            let a = cells[0].0;
            let b = cells[1].0;
            world.set(a, Material::Wood);
            world.set(b, Material::Soil);
            let original_revision = world.revision();
            let (added, original) = edit_test_addition(&world);
            assert_eq!(world.material(a), Material::Wood);
            assert!(added.revision() > original_revision);
            assert!(original
                .iter()
                .all(|(c, _)| added.material(*c) == Material::Stone));
            // Exercise the same immutable background snapshot contract with a
            // small extraction extent, independently of renderer publication.
            let pending = PendingEdit::start(added.clone(), a, [16; 3]);
            world.set(a, Material::Air);
            let built = pending
                .receiver
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
            assert_eq!(built.1, added.revision());
            assert!(!built.0.indices.is_empty());
            let restored = edit_test_restore(&added, &original);
            assert!(restored.revision() > added.revision());
            for &(cell, material) in &original {
                assert_eq!(restored.material(cell), material);
            }
            assert_eq!(restored.material(a), Material::Wood);
            assert_eq!(restored.material(b), Material::Soil);
            let outside = Cell { x: 63, ..a };
            assert_eq!(restored.material(outside), added.material(outside));
        }
    }
    #[test]
    fn directional_edit_worker_rebuilds_cache_for_immutable_added_and_removed_revisions() {
        use super::*;
        let mut world = World::new(SEED);
        let min = Cell {
            x: 0,
            y: 1600,
            z: 0,
        };
        let target = Cell {
            x: 8,
            y: 1608,
            z: 8,
        };
        for material in [Material::Wood, Material::Air] {
            world.set(target, material);
            let revision = world.revision();
            let pending = PendingEdit::start_backend(world.clone(), min, [17; 3], true, true);
            // Further edits cannot mutate either the canonical worker input or
            // the cache derived from that same revision.
            world.set(target, Material::Stone);
            let (_, got_revision, _, bricks) = pending
                .receiver
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
            assert_eq!(got_revision, revision);
            let bricks = bricks.unwrap();
            assert_eq!(bricks.directional_bounds.len(), bricks.headers.len() * 4);
            let target_brick = (1 * 3 + 1) * 3 + 1;
            assert_eq!(
                bricks.headers[target_brick * 2] == 0,
                material == Material::Air
            );
            for octant in 0..8 {
                assert_eq!(
                    bricks.directional_bounds[target_brick * 8 + octant] == 0,
                    material != Material::Air
                );
            }
        }
    }
    #[test]
    fn asynchronous_edit_snapshot_preserves_revision_and_changes_geometry() {
        use super::*;
        let mut world = World::new(SEED);
        let target = Cell { x: 0, y: 176, z: 0 };
        world.set(target, Material::Stone);
        let before = PendingEdit::start(world.clone(), target, [1; 3])
            .receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(before.1, world.revision());
        assert_eq!(before.0.indices.len(), 36);
        world.set(target, Material::Air);
        let expected_revision = world.revision();
        let pending = PendingEdit::start(world.clone(), target, [1; 3]);
        // A later mutation cannot change the immutable worker snapshot.
        world.set(target, Material::Wood);
        let after = pending
            .receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(after.1, expected_revision);
        assert_ne!(after.1, world.revision()); // publication guard rejects this snapshot
        assert!(after.0.indices.is_empty());
    }
    #[test]
    fn edits_pick_native_cell_and_adjacent_air_and_reject_pending() {
        use super::*;
        let mut world = World::new(SEED);
        // Isolated fixture cell in the air above the mountain.
        let target = Cell { x: 0, y: 176, z: 0 };
        world.set(target, Material::Stone);
        let origin = [-0.1, 11.03125, 0.03125];
        let direction = [1., 0., 0.];
        assert_eq!(
            edit_target(&world, origin, direction, false, false),
            Some(target)
        );
        assert_eq!(
            edit_target(&world, origin, direction, true, false),
            Some(Cell { x: -1, ..target })
        );
        assert_eq!(edit_target(&world, origin, direction, false, true), None);
        assert_eq!(edit_target(&world, origin, direction, true, true), None);
    }
    #[test]
    fn edit_bounds_are_half_open_and_do_not_place_outside_mesh() {
        use super::*;
        assert!(inside_arena(ARENA_MIN));
        assert!(!inside_arena(Cell { x: 128, y: 0, z: 0 }));
        assert!(!inside_arena(Cell {
            x: -129,
            y: 0,
            z: 0
        }));
        assert!(!inside_arena(Cell { x: 0, y: 192, z: 0 }));
        let mut world = World::new(SEED);
        world.set(
            Cell {
                x: -128,
                y: 176,
                z: 0,
            },
            Material::Stone,
        );
        assert_eq!(
            edit_target(&world, [-8.1, 11.03125, 0.03125], [1., 0., 0.], true, false),
            None
        );
        assert_eq!(
            edit_target(
                &world,
                [-8.1, 11.03125, 0.03125],
                [1., 0., 0.],
                false,
                false
            ),
            Some(Cell {
                x: -128,
                y: 176,
                z: 0
            })
        );
    }
    #[test]
    fn wgsl_is_valid() {
        let module = naga::front::wgsl::parse_str(super::SHADER).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }
    #[test]
    fn local_camera_is_stable_at_large_origins() {
        let mut camera = super::Camera::fixture();
        let original = camera.matrix(glam::DVec3::ZERO, 1.);
        let offset = glam::DVec3::new(1e9, -1e9, 1e9);
        camera.position += offset;
        assert!(original.abs_diff_eq(camera.matrix(offset, 1.), 1e-5));
    }
}
