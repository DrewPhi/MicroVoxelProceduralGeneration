//! GPU resources and drawing. World generation never touches this module.
use crate::camera::Camera;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec4};
use microvoxel::render::{
    lod::{Grid, MeshKey},
    terrain::TerrainMesh,
    triangulation,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::Path,
    sync::mpsc,
};
use wgpu::util::DeviceExt;

pub const GPU_BUDGET: usize = 128 * 1024 * 1024;
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    vp: [f32; 16],
    selection: [f32; 4],
    viewport: [f32; 4],
    inverse: [f32; 16],
    eye: [f32; 4],
    effects: [f32; 4],
    grid_eye: [i32; 4],
    eye_fraction: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuQuad {
    origin: [i32; 3],
    face: u32,
    size: [u32; 2],
    color_offset: u32,
    padding: u32,
}
struct Part {
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
    count: u32,
    border_vertices: u32,
}
pub struct Tile {
    pub solid_top: i64,
    ray: Option<(u32, u32)>,
    mesh_ready: bool,
    parts: Vec<Part>,
    pub bytes: usize,
    pub quads: usize,
    pub faces: usize,
    heights: [i64; 2],
    used: u64,
}
struct ActorMesh {
    pose: microvoxel::character::Pose,
    arm: bool,
    parts: Vec<Part>,
}
struct ActorInstance {
    pose: microvoxel::character::Pose,
    arm: bool,
    root: microvoxel::CellPos,
}
pub struct Gpu {
    pub quality_mask: bool,
    pub quality_water: bool,
    pub grid: Grid,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
    pub adapter_name: String,
    pipeline: wgpu::RenderPipeline,
    conforming: wgpu::RenderPipeline,
    map_renderer: crate::maps::MapRenderer,
    actor: Option<ActorInstance>,
    actor_meshes: std::collections::VecDeque<ActorMesh>,
    hud: wgpu::RenderPipeline,
    pub hud_visible: bool,
    sky: wgpu::RenderPipeline,
    presentation: crate::presentation::Presentation,
    pub menu_renderer: crate::menu::Renderer,
    settings: crate::settings::Settings,
    budget: usize,
    retiring_budget: Option<usize>,
    transition: Option<(BTreeSet<MeshKey>, std::time::Instant)>,
    shadow_texture: wgpu::Texture,
    frame_buffer: wgpu::Buffer,
    frame_bind: wgpu::BindGroup,
    tile_layout: wgpu::BindGroupLayout,
    depth: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub tiles: BTreeMap<MeshKey, Tile>,
    pub bytes: usize,
    frame: u64,
    raycaster: crate::raycaster::Raycaster,
    visibility: crate::visibility::Visibility,
    pub visibility_limit: f32,
    pub culled: usize,
    study_skip_ray: bool,
}
fn layout_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BufferBindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
impl Gpu {
    pub async fn new(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        width: u32,
        height: u32,
    ) -> microvoxel::Result<Self> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: surface,
            })
            .await
            .ok_or("No compatible GPU adapter. Check your graphics driver.")?;
        let info = adapter.get_info();
        let adapter_name = format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type);
        println!("GPU: {adapter_name}");
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("MicroVoxel GPU"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                },
                None,
            )
            .await?;
        let format = surface
            .map(|s| {
                let caps = s.get_capabilities(&adapter);
                caps.formats
                    .iter()
                    .copied()
                    .find(|f| f.is_srgb())
                    .unwrap_or(caps.formats[0])
            })
            .unwrap_or(wgpu::TextureFormat::Rgba8UnormSrgb);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voxel surfaces"),
            source: wgpu::ShaderSource::Wgsl(include_str!("terrain.wgsl").into()),
        });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera"),
            entries: &[
                layout_entry(
                    0,
                    wgpu::ShaderStages::VERTEX_FRAGMENT,
                    wgpu::BufferBindingType::Uniform,
                ),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let tile_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("merged voxel faces"),
            entries: &[
                layout_entry(
                    0,
                    wgpu::ShaderStages::VERTEX,
                    wgpu::BufferBindingType::Uniform,
                ),
                layout_entry(
                    1,
                    wgpu::ShaderStages::VERTEX,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
                layout_entry(
                    2,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
                layout_entry(
                    3,
                    wgpu::ShaderStages::VERTEX,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain layout"),
            bind_group_layouts: &[&frame_layout, &tile_layout],
            push_constant_ranges: &[],
        });
        let hud_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("HUD layout"),
            bind_group_layouts: &[&frame_layout],
            push_constant_ranges: &[],
        });
        let sky_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sky.wgsl").into()),
        });
        let make_pipeline = |label, layout: &wgpu::PipelineLayout, vs, fs, terrain: bool| {
            let shader = if vs == "vs_sky" { &sky_shader } else { &shader };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: if terrain {
                        Some(wgpu::Face::Back)
                    } else {
                        None
                    },
                    ..Default::default()
                },
                depth_stencil: if terrain {
                    Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: true,
                        depth_compare: wgpu::CompareFunction::Less,
                        stencil: Default::default(),
                        bias: Default::default(),
                    })
                } else {
                    Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: false,
                        depth_compare: wgpu::CompareFunction::Always,
                        stencil: Default::default(),
                        bias: Default::default(),
                    })
                },
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        let pipeline = make_pipeline(
            "voxel terrain",
            &pipeline_layout,
            "vs_main",
            "fs_main",
            true,
        );
        let conforming = make_pipeline(
            "conforming tile borders",
            &pipeline_layout,
            "vs_conforming",
            "fs_main",
            true,
        );
        let hud = make_pipeline("crosshair", &hud_layout, "vs_hud", "fs_hud", false);
        let frame_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera uniform"),
            contents: bytemuck::bytes_of(&FrameUniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let shadow_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("static terrain horizon visibility"),
            size: wgpu::Extent3d {
                width: 256,
                height: 256,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            shadow_texture.as_image_copy(),
            bytemuck::cast_slice(&vec![1f32; 256 * 256]),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256 * 4),
                rows_per_image: Some(256),
            },
            wgpu::Extent3d {
                width: 256,
                height: 256,
                depth_or_array_layers: 1,
            },
        );
        let shadow_view = shadow_texture.create_view(&Default::default());
        let frame_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera"),
            layout: &frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
            ],
        });
        let sky = make_pipeline("sky", &hud_layout, "vs_sky", "fs_sky", false);
        let presentation = crate::presentation::Presentation::new(&device, format, width, height);
        let menu_renderer = crate::menu::Renderer::new(&device, format);
        let depth = depth(&device, width, height);
        let raycaster = crate::raycaster::Raycaster::new(&device, format, &frame_layout);
        let map_renderer = crate::maps::MapRenderer::new(&device, format, &frame_layout);
        Ok(Self {
            device,
            queue,
            format,
            adapter_name,
            pipeline,
            conforming,
            map_renderer,
            actor: None,
            actor_meshes: std::collections::VecDeque::new(),
            hud,
            hud_visible: true,
            sky,
            presentation,
            menu_renderer,
            settings: crate::settings::Settings::default(),
            budget: GPU_BUDGET,
            retiring_budget: None,
            transition: None,
            shadow_texture,
            frame_buffer,
            frame_bind,
            tile_layout,
            depth,
            raycaster,
            visibility: Default::default(),
            visibility_limit: 24000.,
            culled: 0,
            quality_mask: false,
            quality_water: false,
            grid: Grid::default(),
            study_skip_ray: false,
            width,
            height,
            tiles: BTreeMap::new(),
            bytes: 0,
            frame: 0,
        })
    }
    pub fn detail_settings(&mut self, settings: &crate::settings::Settings) {
        self.settings = *settings;
        let requested = settings.memory_mib * 1024 * 1024;
        if requested < self.budget {
            self.retiring_budget =
                Some(self.retiring_budget.unwrap_or(self.budget).max(self.budget));
        }
        self.budget = requested;
        if !settings.transitions {
            self.transition = None;
        }
    }
    pub fn transitioning(&self) -> bool {
        self.transition
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed().as_secs_f32() < 0.16)
    }
    pub fn transition_keys(&mut self) -> BTreeSet<MeshKey> {
        if !self.transitioning() {
            self.transition = None;
        }
        self.transition
            .as_ref()
            .map_or_else(BTreeSet::new, |(keys, _)| keys.clone())
    }
    pub fn begin_transition(
        &mut self,
        old: &BTreeSet<MeshKey>,
        new: &BTreeSet<MeshKey>,
        enabled: bool,
    ) {
        if enabled && !self.transitioning() && old.len() > 32 && old != new {
            // Keep only one previous cover; temporary residency remains protected.
            self.transition = Some((old.clone(), std::time::Instant::now()));
        }
    }
    /// Static, coarse terrain-only sun visibility. It costs 256 KiB and one
    /// bilinear lookup per face sample; actors/caves do not cast into this map.
    pub fn terrain_lighting<G: microvoxel::generation::Generator>(
        &mut self,
        world: &microvoxel::World<G>,
    ) {
        const N: usize = 256;
        let mut heights = vec![0f32; N * N];
        for z in 0..N {
            for x in 0..N {
                let p = microvoxel::CellPos::new(
                    ((x as f64 + 0.5) * self.grid.extent as f64 / N as f64) as i64,
                    0,
                    ((z as f64 + 0.5) * self.grid.extent as f64 / N as f64) as i64,
                );
                heights[z * N + x] = world.environment_at(p).map_or_else(
                    || {
                        world
                            .surface_column(p.x, p.z)
                            .map_or(0., |c| c.ground as f32 * self.grid.cell_m() as f32)
                    },
                    |e| e.elevation_m,
                );
            }
        }
        let mut visibility = vec![1f32; N * N];
        let cell = self.grid.spec.extent_m as f32 / N as f32;
        for z in 0..N {
            for x in 0..N {
                let start = heights[z * N + x];
                let mut excess = 0f32;
                for i in 1..=32 {
                    let distance = i as f32 * cell;
                    let sx = (x as f32 - 0.4 / 0.5315073 * i as f32).round() as i32;
                    let sz = (z as f32 - 0.35 / 0.5315073 * i as f32).round() as i32;
                    if sx < 0 || sz < 0 || sx >= N as i32 || sz >= N as i32 {
                        break;
                    }
                    excess = excess.max(
                        heights[sz as usize * N + sx as usize]
                            - start
                            - distance * 0.85 / 0.5315073,
                    );
                }
                visibility[z * N + x] = (1. - excess / 12.).clamp(0.2, 1.);
            }
        }
        self.queue.write_texture(
            self.shadow_texture.as_image_copy(),
            bytemuck::cast_slice(&visibility),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(N as u32 * 4),
                rows_per_image: Some(N as u32),
            },
            wgpu::Extent3d {
                width: N as u32,
                height: N as u32,
                depth_or_array_layers: 1,
            },
        );
    }
    pub fn set_environment_map(&mut self, image: &microvoxel::generation::debug::DebugImage) {
        self.map_renderer.upload(&self.device, &self.queue, image);
    }
    pub fn render_environment_map(&mut self, view: &wgpu::TextureView) {
        let mut frame = FrameUniform::zeroed();
        frame.viewport = [self.width as f32, self.height as f32, 0., 0.];
        self.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame));
        self.map_renderer
            .draw(&self.device, &self.queue, view, &self.frame_bind);
    }
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.depth = depth(&self.device, self.width, self.height);
        self.presentation = crate::presentation::Presentation::new(
            &self.device,
            self.format,
            self.width,
            self.height,
        );
    }
    pub fn trim(&mut self, protected: &BTreeSet<MeshKey>) {
        while self.bytes > self.budget {
            let candidate = self
                .tiles
                .iter()
                .filter(|(key, _)| !protected.contains(key))
                .min_by_key(|(_, tile)| tile.used)
                .map(|(key, _)| *key);
            let Some(key) = candidate else {
                break;
            };
            self.evict(key);
        }
        if self.bytes <= self.budget {
            self.retiring_budget = None;
        }
    }
    fn evict(&mut self, key: MeshKey) {
        if let Some(tile) = self.tiles.remove(&key) {
            self.bytes -= tile.bytes;
            if let Some((p, n)) = tile.ray {
                self.raycaster.release(p, n);
            }
        }
    }
    /// Mesh keys are world-local. Never let a replacement world reuse old buffers.
    pub fn clear_world(&mut self) {
        self.transition = None;
        let keys: Vec<_> = self.tiles.keys().copied().collect();
        for key in keys {
            self.evict(key);
        }
        self.actor = None;
        self.actor_meshes.clear();
        self.visibility = Default::default();
        self.visibility_limit = 24000.;
        self.culled = 0;
        self.retiring_budget = None;
    }
    pub fn ready(&self, key: &MeshKey, ray: bool) -> bool {
        self.tiles
            .get(key)
            .is_some_and(|t| if ray { t.ray.is_some() } else { t.mesh_ready })
    }
    pub fn upload_ray(
        &mut self,
        key: MeshKey,
        brick: microvoxel::render::ray::Brick,
        protected: &BTreeSet<MeshKey>,
    ) -> Result<(), String> {
        let bytes = brick.bytes().max(32);
        let limit = self.retiring_budget.unwrap_or(self.budget).max(self.budget);
        // Reserve arena space for coarser replacement parents. Without this,
        // an entirely protected full cover could prevent its own retirement.
        let coarsening = protected.iter().any(|k| k.tile.parent() == Some(key.tile));
        let arena_limit = limit.min(128 * 1048576);
        let arena_target = if coarsening {
            arena_limit
        } else {
            arena_limit * 7 / 8
        };
        loop {
            if self.bytes + bytes <= limit && self.raycaster.used_bytes() + bytes <= arena_target {
                if let Some(allocation) =
                    self.raycaster
                        .upload(&self.device, &self.queue, &brick, limit)
                {
                    let tile = self.tiles.entry(key).or_insert(Tile {
                        parts: vec![],
                        solid_top: brick.solid_top,
                        ray: None,
                        mesh_ready: false,
                        bytes: 0,
                        quads: 0,
                        faces: 0,
                        heights: [brick.bounds[0][1] as i64, brick.bounds[1][1] as i64],
                        used: self.frame,
                    });
                    if let Some((p, n)) = tile.ray.replace(allocation) {
                        self.raycaster.release(p, n);
                        tile.bytes -= n as usize * 4;
                        self.bytes -= n as usize * 4;
                    }
                    tile.heights = [brick.bounds[0][1] as i64, brick.bounds[1][1] as i64];
                    tile.bytes += bytes;
                    self.bytes += bytes;
                    tile.solid_top = brick.solid_top;
                    return Ok(());
                }
            }
            let victim = self
                .tiles
                .iter()
                .filter(|(k, _)| !protected.contains(k))
                .min_by_key(|(_, t)| t.used)
                .map(|(k, _)| *k);
            if let Some(victim) = victim {
                self.evict(victim);
            } else {
                return Err("GPU mesh budget reached (voxel arena); reduce LOD tile budget".into());
            }
        }
    }
    /// Used only by --ray-study. Normal rendering never changes this mode.
    pub fn study_ray_stage(&mut self, stage: u8) {
        self.study_skip_ray = stage == 2;
        if stage < 2 {
            self.raycaster
                .study_geometry_only(&self.device, self.format, stage == 1);
        }
    }
    pub fn ray_allocated_bytes(&self) -> usize {
        self.raycaster.allocated_bytes
    }
    pub fn upload(
        &mut self,
        key: MeshKey,
        mesh: TerrainMesh,
        protected: &BTreeSet<MeshKey>,
    ) -> Result<(), String> {
        let prepared = mesh.prepare(key);
        self.upload_prepared(key, mesh, prepared, protected)
    }
    pub fn upload_prepared(
        &mut self,
        key: MeshKey,
        mesh: TerrainMesh,
        prepared: Vec<triangulation::Prepared>,
        protected: &BTreeSet<MeshKey>,
    ) -> Result<(), String> {
        assert_eq!(mesh.parts.len(), prepared.len());
        let packed: Vec<_> = mesh
            .parts
            .iter()
            .map(|p| microvoxel::render::colors::pack(&p.surface.colors))
            .collect();
        let color_bytes: usize = packed.iter().map(|p| p.len() * 4).sum();
        let raw_colors: usize = mesh.parts.iter().map(|p| p.surface.colors.len() * 4).sum();
        let border_bytes: usize = prepared.iter().map(|p| p.bytes()).sum();
        let size = mesh.bytes() - raw_colors
            + color_bytes
            + border_bytes
            + mesh
                .parts
                .iter()
                .filter(|p| !p.surface.quads.is_empty())
                .count()
                * 32;
        // Lowering a budget retains its previous ceiling only while coarsening
        // the protected cover. Otherwise a smaller parent could never be loaded.
        let limit = self.retiring_budget.unwrap_or(self.budget).max(self.budget);
        while self.bytes + size > limit {
            let candidate = self
                .tiles
                .iter()
                .filter(|(key, _)| !protected.contains(key))
                .min_by_key(|(_, tile)| tile.used)
                .map(|(key, _)| *key);
            let Some(candidate) = candidate else {
                return Err("GPU mesh budget reached; reduce LOD tile budget".into());
            };
            self.evict(candidate);
        }
        let heights = mesh.heights();
        let quads = mesh.quads();
        let faces = mesh.faces();
        let [x, z] = key.tile.origin_in(self.grid);
        let parts = mesh
            .parts
            .into_iter()
            .zip(prepared)
            .zip(packed)
            .filter_map(|((part, prepared), packed)| {
                self.upload_part(
                    part,
                    Some(prepared),
                    Some(packed),
                    [x, 0, z],
                    [
                        self.grid.extent - x,
                        self.grid.spec.ceiling_cells(),
                        self.grid.extent - z,
                    ],
                )
            })
            .collect();
        let previous_ray = self.tiles.get(&key).and_then(|t| t.ray);
        let ray_bytes = previous_ray.map_or(0, |(_, n)| n as usize * 4);
        if let Some(old) = self.tiles.insert(
            key,
            Tile {
                solid_top: 0,
                ray: previous_ray,
                mesh_ready: true,
                parts,
                bytes: size + ray_bytes,
                quads,
                faces,
                heights,
                used: self.frame,
            },
        ) {
            self.bytes -= old.bytes;
        }
        self.bytes += size + ray_bytes;
        Ok(())
    }
    fn upload_part(
        &self,
        part: microvoxel::render::terrain::SurfacePart,
        prepared: Option<triangulation::Prepared>,
        packed_colors: Option<Vec<u32>>,
        origin: [i64; 3],
        clip: [i64; 3],
    ) -> Option<Part> {
        if part.surface.quads.is_empty() {
            return None;
        }
        let prepared =
            prepared.unwrap_or_else(|| triangulation::Prepared::quads_only(&part.surface));
        let regular_count = prepared.regular;
        let references = prepared.vertex_words();
        let border_vertices = prepared.vertices.len() as u32;
        let records: Vec<_> = prepared
            .order
            .into_iter()
            .zip(prepared.factors)
            .map(|(index, factor)| {
                let q = &part.surface.quads[index];
                GpuQuad {
                    origin: q.origin,
                    face: q.face | (q.water_level << 3),
                    size: q.size,
                    color_offset: q.color_offset,
                    // Bits 0..14: triangulation factor; 15: terrain light;
                    // 16..31: native water Y (zero means no water clip).
                    padding: factor.trailing_zeros()
                        | (u32::from(q.slope_light) << 15)
                        | (u32::from(q.constant_color) << 14),
                }
            })
            .collect();
        let vertices = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("conforming border vertex references"),
                contents: bytemuck::cast_slice(&references),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("tile origin and grid step"),
                // Terrain origins are immutable grid coordinates. Camera motion
                // now changes one shared frame uniform, not every tile buffer.
                contents: bytemuck::cast_slice(&[
                    origin[0] as f32,
                    origin[1] as f32,
                    origin[2] as f32,
                    part.step as f32,
                    clip[0] as f32,
                    clip[1] as f32,
                    clip[2] as f32,
                    0.,
                ]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let geometry = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("merged rectangles"),
                contents: bytemuck::cast_slice(&records),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let packed_colors =
            packed_colors.unwrap_or_else(|| microvoxel::render::colors::pack(&part.surface.colors));
        let colors = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("exact cell colors"),
                contents: bytemuck::cast_slice(&packed_colors),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tile"),
            layout: &self.tile_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: geometry.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: colors.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: vertices.as_entire_binding(),
                },
            ],
        });
        Some(Part {
            uniform,
            bind,
            count: regular_count,
            border_vertices,
        })
    }
    pub fn hide_actor(&mut self) {
        self.actor = None;
    }
    pub fn set_actor(
        &mut self,
        pose: microvoxel::character::Pose,
        arm: bool,
        root: microvoxel::CellPos,
        frame: &microvoxel::character::VoxelFrame,
        materials: &microvoxel::materials::MaterialRegistry,
    ) -> Result<(), String> {
        if !self
            .actor_meshes
            .iter()
            .any(|m| m.pose == pose && m.arm == arm)
        {
            let surface = if arm {
                frame.right_arm(pose.heading).surface(materials, [0; 3])?
            } else {
                frame.surface(materials, [0; 3])?
            };
            let parts = self
                .upload_part(
                    microvoxel::render::terrain::SurfacePart { step: 1, surface },
                    None,
                    None,
                    [0; 3],
                    [32768; 3],
                )
                .into_iter()
                .collect();
            if self.actor_meshes.len() >= 16 {
                self.actor_meshes.pop_front();
            }
            self.actor_meshes.push_back(ActorMesh { pose, arm, parts });
        }
        self.actor = Some(ActorInstance { pose, arm, root });
        Ok(())
    }
    pub fn render(
        &mut self,
        target: &wgpu::TextureView,
        camera: &Camera,
        cover: &BTreeSet<MeshKey>,
        selection: Option<[i64; 3]>,
    ) -> usize {
        self.render_view(
            target,
            camera.eye(),
            camera.matrix(self.width, self.height),
            cover,
            selection,
            false,
        )
    }
    /// An alternate presentation of the same voxel meshes; world data is unchanged.
    pub fn render_view(
        &mut self,
        target: &wgpu::TextureView,
        eye: [f64; 3],
        matrix: Mat4,
        cover: &BTreeSet<MeshKey>,
        selection: Option<[i64; 3]>,
        overview: bool,
    ) -> usize {
        self.render_at_scale(
            target,
            eye,
            matrix,
            cover,
            selection,
            overview,
            self.grid.cell_m(),
        )
    }
    /// Alternate asset resolution is for isolated previews. It never changes
    /// the canonical world's cell size or applies a rotating mesh transform.
    pub fn render_at_scale(
        &mut self,
        target: &wgpu::TextureView,
        eye: [f64; 3],
        matrix: Mat4,
        cover: &BTreeSet<MeshKey>,
        selection: Option<[i64; 3]>,
        overview: bool,
        cell_size: f64,
    ) -> usize {
        if overview || (!self.transitioning() && !self.settings.antialias) {
            return self.render_scene(target, eye, matrix, cover, selection, overview, cell_size);
        }
        let previous = self
            .transition
            .as_ref()
            .filter(|(_, t)| t.elapsed().as_secs_f32() < 0.16)
            .map(|(keys, t)| (keys.clone(), t.elapsed().as_secs_f32() / 0.16));
        let current = std::sync::Arc::clone(&self.presentation.images[1]);
        let old_image = std::sync::Arc::clone(&self.presentation.images[0]);
        if let Some((old, _)) = &previous {
            self.render_scene(&old_image, eye, matrix, old, selection, false, cell_size);
        }
        let count = self.render_scene(&current, eye, matrix, cover, selection, false, cell_size);
        self.presentation.draw(
            &self.device,
            &self.queue,
            target,
            self.width,
            self.height,
            previous.map_or(1., |(_, progress)| progress),
            self.settings.antialias,
        );
        count
    }
    fn render_scene(
        &mut self,
        target: &wgpu::TextureView,
        eye: [f64; 3],
        matrix: Mat4,
        cover: &BTreeSet<MeshKey>,
        selection: Option<[i64; 3]>,
        overview: bool,
        cell_size: f64,
    ) -> usize {
        self.frame += 1;
        let selection = selection.map_or([0.; 4], |p| {
            [
                (p[0] as f64 * cell_size - eye[0]) as f32,
                (p[1] as f64 * cell_size - eye[1]) as f32,
                (p[2] as f64 * cell_size - eye[2]) as f32,
                1.,
            ]
        });
        let frame = FrameUniform {
            grid_eye: [
                (eye[0] / cell_size).floor() as i32,
                (eye[1] / cell_size).floor() as i32,
                (eye[2] / cell_size).floor() as i32,
                0,
            ],
            eye_fraction: [
                (eye[0] - (eye[0] / cell_size).floor() * cell_size) as f32,
                (eye[1] - (eye[1] / cell_size).floor() * cell_size) as f32,
                (eye[2] - (eye[2] / cell_size).floor() * cell_size) as f32,
                0.,
            ],
            vp: matrix.to_cols_array(),
            selection,
            viewport: [
                self.width as f32,
                self.height as f32,
                if overview {
                    0.
                } else {
                    0.00011 * self.settings.haze as f32
                },
                cell_size as f32,
            ],
            inverse: matrix.inverse().to_cols_array(),
            eye: [
                eye[0] as f32,
                eye[1] as f32,
                eye[2] as f32,
                self.grid.spec.extent_m as f32,
            ],
            effects: [
                if self.quality_water {
                    -2.
                } else if self.quality_mask {
                    -1.
                } else {
                    f32::from(self.settings.ambient)
                },
                f32::from(self.settings.shadows && !overview),
                matrix.row(1).truncate().length() * 0.5,
                if overview {
                    40000.
                } else {
                    self.visibility_limit
                },
            ],
        };
        self.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame));
        let entries: Vec<_> = cover
            .iter()
            .filter_map(|key| {
                self.tiles.get(key).map(|tile| crate::visibility::Entry {
                    key: *key,
                    low: [
                        key.tile.origin_in(self.grid)[0] as f64 * cell_size,
                        tile.heights[0] as f64 * cell_size,
                        key.tile.origin_in(self.grid)[1] as f64 * cell_size,
                    ],
                    high: [
                        (key.tile.origin_in(self.grid)[0] + key.tile.edge_in(self.grid))
                            .min(self.grid.extent) as f64
                            * cell_size,
                        tile.heights[1] as f64 * cell_size,
                        (key.tile.origin_in(self.grid)[1] + key.tile.edge_in(self.grid))
                            .min(self.grid.extent) as f64
                            * cell_size,
                    ],
                    solid_top: tile.solid_top as f64 * cell_size,
                })
            })
            .collect();
        let candidates = self.visibility.select(
            entries,
            eye,
            matrix,
            if overview {
                f32::INFINITY
            } else {
                self.visibility_limit
            },
            !self.settings.ray && !overview,
        );
        self.culled = cover.len().saturating_sub(candidates.len());
        let mut visible = Vec::new();
        for key in &candidates {
            let Some(tile) = self.tiles.get_mut(key) else {
                continue;
            };
            let [x, z] = key.tile.origin_in(self.grid);
            let e = key.tile.edge_in(self.grid);
            let low = [
                x as f64 * cell_size - eye[0],
                tile.heights[0] as f64 * cell_size - eye[1],
                z as f64 * cell_size - eye[2],
            ];
            let high = [
                (x + e).min(self.grid.extent) as f64 * cell_size - eye[0],
                tile.heights[1] as f64 * cell_size - eye[1],
                (z + e).min(self.grid.extent) as f64 * cell_size - eye[2],
            ];
            if !in_frustum(matrix, low, high) {
                continue;
            }
            tile.used = self.frame;
            visible.push(*key);
        }
        let actor = self.actor.as_ref().and_then(|instance| {
            self.actor_meshes
                .iter()
                .find(|m| m.pose == instance.pose && m.arm == instance.arm)
                .map(|mesh| (instance, mesh))
        });
        if let Some((instance, mesh)) = actor {
            let position = instance.root;
            for part in &mesh.parts {
                self.queue.write_buffer(
                    &part.uniform,
                    0,
                    bytemuck::cast_slice(&[
                        position.x as f32,
                        position.y as f32,
                        position.z as f32,
                        1.,
                        32768.,
                        32768.,
                        32768.,
                        0.,
                    ]),
                );
            }
        }
        if !overview {
            let roots = visible
                .iter()
                .filter_map(|key| {
                    let tile = &self.tiles[key];
                    if !self.settings.ray && tile.mesh_ready {
                        return None;
                    }
                    let (offset, _) = tile.ray?;
                    if tile.heights[1] <= tile.heights[0] {
                        return None;
                    }
                    let [x, z] = key.tile.origin_in(self.grid);
                    Some(crate::raycaster::Root {
                        key: *key,
                        offset,
                        bounds: [
                            [x as i32, tile.heights[0] as i32, z as i32],
                            [
                                (x + key.tile.edge_in(self.grid)).min(self.grid.extent) as i32,
                                tile.heights[1] as i32,
                                (z + key.tile.edge_in(self.grid)).min(self.grid.extent) as i32,
                            ],
                        ],
                    })
                })
                .collect();
            self.raycaster.cover(&self.queue, roots);
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("voxel frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terrain + crosshair"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.435,
                            g: 0.590,
                            b: 0.729,
                            a: 1.,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if !overview {
                pass.set_pipeline(&self.sky);
                pass.set_bind_group(0, &self.frame_bind, &[]);
                pass.draw(0..6, 0..1);
            }
            if !overview && !self.study_skip_ray && !self.raycaster.roots.is_empty() {
                pass.set_pipeline(&self.raycaster.pipeline);
                pass.set_bind_group(0, &self.frame_bind, &[]);
                pass.set_bind_group(1, &self.raycaster.bind, &[]);
                pass.draw(0..3, 0..1);
            }
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.frame_bind, &[]);
            for key in &visible {
                if self.settings.ray && !overview && self.tiles[key].ray.is_some() {
                    continue;
                }
                for part in &self.tiles[key].parts {
                    pass.set_bind_group(1, &part.bind, &[]);
                    pass.draw(0..6, 0..part.count);
                }
            }
            if let Some((_, mesh)) = actor {
                for part in &mesh.parts {
                    pass.set_bind_group(1, &part.bind, &[]);
                    pass.draw(0..6, 0..part.count);
                }
            }
            pass.set_pipeline(&self.conforming);
            for key in &visible {
                if self.settings.ray && !overview && self.tiles[key].ray.is_some() {
                    continue;
                }
                for part in &self.tiles[key].parts {
                    if part.border_vertices > 0 {
                        pass.set_bind_group(1, &part.bind, &[]);
                        pass.draw(0..part.border_vertices, 0..1);
                    }
                }
            }
            if !overview && self.hud_visible {
                pass.set_pipeline(&self.hud);
                pass.set_bind_group(0, &self.frame_bind, &[]);
                pass.draw(0..6, 0..1);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        visible.len()
    }
    pub fn offscreen(&self) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native screenshot"),
            size: wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }
    pub fn screenshot(&self, texture: &wgpu::Texture, path: &Path) -> microvoxel::Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let pixels = self.read_pixels(texture)?;
        let mut encoder = png::Encoder::new(
            std::io::BufWriter::new(File::create(path)?),
            self.width,
            self.height,
        );
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&pixels)?;
        println!("Saved {}", path.display());
        Ok(())
    }
    pub fn read_pixels(&self, texture: &wgpu::Texture) -> microvoxel::Result<Vec<u8>> {
        let padded = (self.width * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PNG readback"),
            size: padded as u64 * self.height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        let (tx, rx) = mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let mapped = buffer.slice(..).get_mapped_range();
        let mut pixels = Vec::with_capacity((self.width * self.height * 4) as usize);
        for row in mapped.chunks_exact(padded as usize) {
            pixels.extend_from_slice(&row[..(self.width * 4) as usize]);
        }
        if self.format == wgpu::TextureFormat::Bgra8UnormSrgb
            || self.format == wgpu::TextureFormat::Bgra8Unorm
        {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        drop(mapped);
        buffer.unmap();
        Ok(pixels)
    }
}
fn depth(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}
pub(crate) fn in_frustum(matrix: Mat4, low: [f64; 3], high: [f64; 3]) -> bool {
    let corners: Vec<_> = (0..8)
        .map(|i| {
            matrix
                * Vec4::new(
                    if i & 1 == 0 { low[0] } else { high[0] } as f32,
                    if i & 2 == 0 { low[1] } else { high[1] } as f32,
                    if i & 4 == 0 { low[2] } else { high[2] } as f32,
                    1.,
                )
        })
        .collect();
    !(corners.iter().all(|p| p.x < -p.w)
        || corners.iter().all(|p| p.x > p.w)
        || corners.iter().all(|p| p.y < -p.w)
        || corners.iter().all(|p| p.y > p.w)
        || corners.iter().all(|p| p.z < 0.)
        || corners.iter().all(|p| p.z > p.w))
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "needs a Vulkan/Metal/DX12 device; run explicitly"]
    fn centimeter_water_above_u16_height_matches_both_renderers_far_from_origin() {
        use super::*;
        use microvoxel::{
            generation::{Column, FlatTerrain, Generator, Run},
            CellPos, GridScale, Voxel, World, WorldSpec,
        };
        use std::{ops::Range, sync::Arc};
        struct Scaled(FlatTerrain);
        impl Generator for Scaled {
            fn spec(&self) -> WorldSpec {
                WorldSpec {
                    grid: GridScale::new(1, 100).unwrap(),
                    ..Default::default()
                }
            }
            fn identity(&self) -> microvoxel::generation::GeneratorIdentity {
                self.0.identity()
            }
            fn voxel(&self, p: CellPos) -> Option<Voxel> {
                self.0.voxel(p)
            }
            fn column_hint(&self, x: i64, z: i64) -> Option<Column> {
                self.0.column_hint(x, z)
            }
            fn runs(&self, x: i64, z: i64, r: Range<i64>) -> Option<Vec<Run>> {
                self.0.runs(x, z, r)
            }
        }
        let world = Arc::new(World::new(Scaled(FlatTerrain {
            height: 80000,
            water: 80007,
            seed: 42,
        })));
        let grid = microvoxel::render::lod::Grid::new(world.spec());
        let key = MeshKey {
            tile: microvoxel::render::lod::TileKey {
                level: grid.finest,
                x: 28125,
                z: 28125,
            },
            neighbors: [grid.finest; 4],
            revision: 0,
        };
        let mut sampler = microvoxel::render::terrain::Sampler::new(world);
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let mut gpu = pollster::block_on(Gpu::new(&instance, None, 128, 128)).unwrap();
        gpu.grid = grid;
        gpu.settings.haze = 0.;
        gpu.settings.shadows = false;
        gpu.settings.antialias = false;
        gpu.hud_visible = false;
        let cover = [key].into();
        gpu.upload(key, sampler.mesh(key).unwrap(), &cover).unwrap();
        let mut camera = Camera::at(CellPos::new(900016, 80030, 900016));
        camera.grid = grid.spec.grid;
        camera.pitch = -1.5;
        let texture = gpu.offscreen();
        let view = texture.create_view(&Default::default());
        gpu.render(&view, &camera, &cover, None);
        let mesh = gpu.read_pixels(&texture).unwrap();
        gpu.settings.ray = true;
        gpu.upload_ray(key, sampler.ray_brick(key, || false).unwrap(), &cover)
            .unwrap();
        gpu.render(&view, &camera, &cover, None);
        let ray = gpu.read_pixels(&texture).unwrap();
        for y in 58..70 {
            for x in 58..70 {
                let i = (y * 128 + x) * 4;
                assert!(
                    mesh[i + 2] > mesh[i + 1] && mesh[i + 1] > mesh[i],
                    "scaled water missing"
                );
                for c in 0..3 {
                    assert!(
                        mesh[i + c].abs_diff(ray[i + c]) <= 3,
                        "scaled renderer mismatch {:?} {:?}",
                        &mesh[i..i + 3],
                        &ray[i..i + 3]
                    );
                }
            }
        }
    }
    #[test]
    fn terrain_and_hud_shaders_validate_without_a_graphics_device() {
        let ray_source = crate::raycaster::shader_source();
        for source in [
            ray_source.as_str(),
            include_str!("diagnostics/ray_study.wgsl"),
            include_str!("terrain.wgsl"),
            include_str!("environment.wgsl"),
            include_str!("presentation.wgsl"),
            include_str!("menu.wgsl"),
            include_str!("sky.wgsl"),
        ] {
            let shader = naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&shader)
            .unwrap();
        }
    }
    #[test]
    #[ignore = "needs a Vulkan/Metal/DX12 device; run explicitly"]
    fn ray_and_mesh_renderers_agree_on_edited_surface_and_switch_live() {
        use super::*;
        use microvoxel::{
            generation::FlatTerrain,
            materials::{Appearance, MaterialId, Voxel},
            render::{lod::TileKey, terrain::Sampler},
            CellPos, Edit, World,
        };
        use std::sync::Arc;
        let mut world = World::new(FlatTerrain {
            height: 16,
            water: 16,
            seed: 42,
        });
        let edits: Vec<_> = (32..64)
            .flat_map(|x| {
                (32..64).map(move |z| {
                    Edit::set(
                        CellPos::new(x, 15, z),
                        Voxel::new(MaterialId::GRASS, Appearance(50000)),
                    )
                })
            })
            .collect();
        world.apply(&edits).unwrap();
        let mut sampler = Sampler::new(Arc::new(world));
        let key = MeshKey {
            tile: TileKey {
                level: 13,
                x: 1,
                z: 1,
            },
            neighbors: [13; 4],
            revision: 1,
        };
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let mut gpu = pollster::block_on(Gpu::new(&instance, None, 128, 128)).unwrap();
        gpu.settings.haze = 0.;
        gpu.settings.shadows = false;
        gpu.hud_visible = false;
        let cover = [key].into();
        gpu.upload(key, sampler.mesh(key).unwrap(), &cover).unwrap();
        let mut camera = Camera::at(CellPos::new(48, 48, 48));
        camera.pitch = -1.5;
        let texture = gpu.offscreen();
        let view = texture.create_view(&Default::default());
        gpu.render(&view, &camera, &cover, None);
        let mesh = gpu.read_pixels(&texture).unwrap();
        gpu.settings.ray = true;
        // Mesh fallback remains visible while the new backend has no brick yet.
        gpu.render(&view, &camera, &cover, None);
        let fallback = gpu.read_pixels(&texture).unwrap();
        assert_eq!(mesh, fallback);
        gpu.upload_ray(key, sampler.ray_brick(key, || false).unwrap(), &cover)
            .unwrap();
        gpu.render(&view, &camera, &cover, None);
        let ray = gpu.read_pixels(&texture).unwrap();
        for y in 54..74 {
            for x in 54..74 {
                let i = (y * 128 + x) * 4;
                for c in 0..3 {
                    assert!(
                        mesh[i + c].abs_diff(ray[i + c]) <= 3,
                        "renderer pixel mismatch at {x},{y}: {:?} {:?}",
                        &mesh[i..i + 3],
                        &ray[i..i + 3]
                    );
                }
            }
        }
        gpu.settings.ray = false;
        gpu.render(&view, &camera, &cover, None);
        assert_eq!(mesh, gpu.read_pixels(&texture).unwrap());
    }
    #[test]
    #[ignore = "needs a Vulkan/Metal/DX12 device; run explicitly"]
    fn gpu_lake_has_no_dark_ledge_between_native_and_coarse_tiles() {
        use super::*;
        use microvoxel::{
            generation::FlatTerrain,
            render::{lod::TileKey, terrain::Sampler},
            CellPos, World,
        };
        use std::sync::Arc;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let mut gpu = pollster::block_on(Gpu::new(&instance, None, 256, 256)).unwrap();
        gpu.settings.haze = 0.;
        gpu.settings.shadows = false;
        gpu.settings.antialias = false;
        gpu.hud_visible = false;
        let world = Arc::new(World::new(FlatTerrain {
            height: 17,
            water: 67,
            seed: 42,
        }));
        let mut sampler = Sampler::new(world);
        let keys = [
            MeshKey {
                tile: TileKey {
                    level: 12,
                    x: 1,
                    z: 1,
                },
                neighbors: [13, 12, 12, 12],
                revision: 0,
            },
            MeshKey {
                tile: TileKey {
                    level: 13,
                    x: 1,
                    z: 2,
                },
                neighbors: [13, 12, 13, 13],
                revision: 0,
            },
            MeshKey {
                tile: TileKey {
                    level: 13,
                    x: 1,
                    z: 3,
                },
                neighbors: [13, 12, 13, 13],
                revision: 0,
            },
        ];
        let cover = keys.into_iter().collect();
        for key in keys {
            gpu.upload(key, sampler.mesh(key).unwrap(), &cover).unwrap();
        }
        let mut camera = Camera::at(CellPos::new(64, 95, 96));
        camera.pitch = -1.5;
        camera.yaw = 0.;
        let texture = gpu.offscreen();
        let view = texture.create_view(&Default::default());
        gpu.render(&view, &camera, &cover, None);
        let pixels = gpu.read_pixels(&texture).unwrap();
        let mut low = [255u8; 3];
        let mut high = [0u8; 3];
        for y in 80..176 {
            for x in 80..176 {
                let p = &pixels[(y * 256 + x) * 4..][..3];
                assert!(p[2] > p[1] && p[1] > p[0], "lake missing: {p:?}");
                for i in 0..3 {
                    low[i] = low[i].min(p[i]);
                    high[i] = high[i].max(p[i]);
                }
            }
        }
        for i in 0..3 {
            assert!(high[i] - low[i] <= 3, "dark LOD ledge: {low:?}..{high:?}");
        }
    }
    #[test]
    #[ignore = "needs a Vulkan/Metal/DX12 device; run explicitly"]
    fn gpu_draws_the_exposed_top_of_a_flat_voxel_surface() {
        use super::*;
        use microvoxel::{
            generation::FlatTerrain,
            render::{lod::TileKey, terrain::Sampler},
            CellPos, World,
        };
        use std::sync::Arc;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let mut gpu = pollster::block_on(Gpu::new(&instance, None, 256, 256)).unwrap();
        let world = Arc::new(World::new(FlatTerrain {
            height: 5721,
            water: 5721,
            seed: 42,
        }));
        let mut sampler = Sampler::new(world);
        let key = MeshKey {
            tile: TileKey {
                level: 10,
                x: 582,
                z: 314,
            },
            neighbors: [10; 4],
            revision: 0,
        };
        gpu.upload(key, sampler.mesh(key).unwrap(), &BTreeSet::new())
            .unwrap();
        let camera = Camera::at(CellPos::new(149056, 5748, 80592));
        let texture = gpu.offscreen();
        let view = texture.create_view(&Default::default());
        gpu.render(&view, &camera, &[key].into(), None);
        let path = std::env::temp_dir().join("microvoxel-gpu-surface-test.png");
        gpu.screenshot(&texture, &path).unwrap();
        let decoder = png::Decoder::new(std::io::BufReader::new(File::open(&path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut bytes = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut bytes).unwrap();
        // Away from the crosshair, the center must be grass, not clear sky.
        let offset = (176 * 256 + 128) * 4;
        let pixel = &bytes[offset..offset + 3];
        assert!(
            pixel[1] > pixel[0] && pixel[1] > pixel[2],
            "surface missing: {pixel:?}"
        );
        let top = (80 * 256 + 128) * 4;
        assert!(
            bytes[top + 2] > bytes[top + 1],
            "sky must be above the ground"
        );
        // Dither whole opaque scenes, including their sky. At every phase the
        // sampled patch remains terrain; mid-transition contains both colors.
        let mut red = sampler.mesh(key).unwrap();
        for part in &mut red.parts {
            for color in &mut part.surface.colors {
                *color = u32::from_le_bytes([230, 40, 20, 255]);
            }
        }
        let new_key = MeshKey { revision: 1, ..key };
        gpu.upload(new_key, red, &[key].into()).unwrap();
        for seconds in [0., 0.08, 0.17] {
            gpu.transition = Some((
                [key].into(),
                std::time::Instant::now() - std::time::Duration::from_secs_f64(seconds),
            ));
            gpu.render(&view, &camera, &[new_key].into(), None);
            let pixels = gpu.read_pixels(&texture).unwrap();
            let mut green = 0;
            let mut red = 0;
            for y in 176..200 {
                for x in 100..150 {
                    let p = &pixels[(y * 256 + x) * 4..][..3];
                    assert!(
                        p[2] < p[0].max(p[1]),
                        "hole or sky during LOD transition: {p:?}"
                    );
                    if p[1] > p[0] {
                        green += 1;
                    } else {
                        red += 1;
                    }
                }
            }
            if seconds == 0. {
                assert!(green > 1100);
            } else if seconds < 0.16 {
                assert!(green > 300 && red > 300);
            } else {
                assert!(red > 1100);
            }
        }
        gpu.budget = gpu.bytes;
        let third = MeshKey { revision: 2, ..key };
        assert!(gpu
            .upload(third, sampler.mesh(key).unwrap(), &[key, new_key].into())
            .unwrap_err()
            .contains("GPU mesh budget"));
        assert!(gpu.tiles.contains_key(&key) && gpu.tiles.contains_key(&new_key));
        let _ = std::fs::remove_file(path);
    }
}
