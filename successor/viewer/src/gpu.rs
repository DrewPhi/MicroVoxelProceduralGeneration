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
#[path = "arena.rs"]
mod arena;
use arena::{Arena, Handle};

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
    metadata: Handle,
    geometry: Handle,
    colors: Handle,
    vertices: Handle,
    count: u32,
    border_vertices: u32,
}
struct Pool {
    buffer: wgpu::Buffer,
    arena: Arena,
    label: &'static str,
    usage: wgpu::BufferUsages,
}
impl Pool {
    fn new(
        device: &wgpu::Device,
        label: &'static str,
        capacity: u64,
        usage: wgpu::BufferUsages,
    ) -> Self {
        // Migration copies the full buffer. Keep its capacity copy-aligned too,
        // including when a device binding limit supplied an odd upper bound.
        let capacity = capacity / wgpu::COPY_BUFFER_ALIGNMENT * wgpu::COPY_BUFFER_ALIGNMENT;
        assert!(capacity >= wgpu::COPY_BUFFER_ALIGNMENT);
        Self {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: capacity,
                usage: usage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            arena: Arena::new(capacity),
            label,
            usage,
        }
    }
    fn write(&mut self, queue: &wgpu::Queue, data: &[u8], alignment: u64) -> Option<Handle> {
        let h = self.arena.allocate(data.len() as u64, alignment)?;
        if !data.is_empty() {
            queue.write_buffer(&self.buffer, h.offset, data);
        }
        Some(h)
    }
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
    pub timing: crate::gpu_timing::GpuTiming,
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
    transition_in: BTreeSet<MeshKey>,
    shadow_texture: wgpu::Texture,
    frame_buffer: wgpu::Buffer,
    frame_bind: wgpu::BindGroup,
    tile_bind: wgpu::BindGroup,
    tile_layout: wgpu::BindGroupLayout,
    pool_migrations: Vec<(u64, wgpu::Buffer)>,
    ray_migrations: Vec<(u64, usize)>,
    metadata_pool: Pool,
    geometry_pool: Pool,
    color_pool: Pool,
    vertex_pool: Pool,
    submitted: u64,
    completed: u64,
    completion_tx: mpsc::Sender<u64>,
    completion_rx: mpsc::Receiver<u64>,
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
                    required_features: crate::gpu_timing::required_features(adapter.features()),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: std::num::NonZeroU64::new(48),
                    },
                    count: None,
                },
                layout_entry(
                    1,
                    wgpu::ShaderStages::VERTEX,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
                layout_entry(
                    2,
                    wgpu::ShaderStages::VERTEX_FRAGMENT,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
                layout_entry(
                    3,
                    wgpu::ShaderStages::VERTEX,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
            ],
        });
        let storage_limit = device.limits().max_storage_buffer_binding_size as u64;
        let geometry_pool = Pool::new(
            &device,
            "terrain geometry arena",
            (4u64 << 20).min(storage_limit),
            wgpu::BufferUsages::STORAGE,
        );
        let color_pool = Pool::new(
            &device,
            "terrain color arena",
            (4u64 << 20).min(storage_limit),
            wgpu::BufferUsages::STORAGE,
        );
        let vertex_pool = Pool::new(
            &device,
            "terrain border arena",
            (1u64 << 20).min(storage_limit),
            wgpu::BufferUsages::STORAGE,
        );
        let metadata_pool = Pool::new(
            &device,
            "terrain draw metadata arena",
            1u64 << 20,
            wgpu::BufferUsages::UNIFORM,
        );
        let tile_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pooled terrain"),
            layout: &tile_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &metadata_pool.buffer,
                        offset: 0,
                        size: std::num::NonZeroU64::new(48),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: geometry_pool.buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: color_pool.buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: vertex_pool.buffer.as_entire_binding(),
                },
            ],
        });
        let (completion_tx, completion_rx) = mpsc::channel();
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
        let timing = crate::gpu_timing::GpuTiming::new(&device, &queue);
        println!("{}", timing.status());
        Ok(Self {
            timing,
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
            transition_in: BTreeSet::new(),
            shadow_texture,
            frame_buffer,
            frame_bind,
            tile_bind,
            tile_layout,
            pool_migrations: Vec::new(),
            ray_migrations: Vec::new(),
            metadata_pool,
            geometry_pool,
            color_pool,
            vertex_pool,
            submitted: 0,
            completed: 0,
            completion_tx,
            completion_rx,
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
            self.clear_transition();
        }
    }
    pub fn transitioning(&self) -> bool {
        self.transition
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed().as_secs_f32() < 0.16)
    }
    fn clear_transition(&mut self) {
        let mut keys = self.transition_in.clone();
        if let Some((old, _)) = &self.transition {
            keys.extend(old);
        }
        for key in &keys {
            if let Some(tile) = self.tiles.get(key) {
                for part in &tile.parts {
                    self.queue.write_buffer(
                        &self.metadata_pool.buffer,
                        part.metadata.offset + 28,
                        bytemuck::bytes_of(&0f32),
                    );
                }
            }
        }
        self.transition_in.clear();
        self.transition = None;
    }
    pub fn transition_keys(&mut self) -> BTreeSet<MeshKey> {
        if !self.transitioning() {
            self.clear_transition();
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
        if !self.transitioning() {
            self.clear_transition();
        }
        if enabled && !self.settings.ray && !self.transitioning() && !old.is_empty() && old != new {
            // Only changed ownership regions render twice. Unchanged mountains
            // and the player are submitted once, never as two full-frame images.
            self.transition_in = new.difference(old).copied().collect();
            // Only an actual one-level split has this parent reconstruction.
            // Neighbor stitch/revision changes must not collapse to a parent.
            for key in &self.transition_in {
                let morph = u32::from(
                    key.tile
                        .parent()
                        .is_some_and(|p| old.iter().any(|k| k.tile == p)),
                );
                if let Some(tile) = self.tiles.get(key) {
                    for part in &tile.parts {
                        self.queue.write_buffer(
                            &self.metadata_pool.buffer,
                            part.metadata.offset + 44,
                            bytemuck::bytes_of(&morph),
                        );
                    }
                }
            }
            self.transition = Some((
                old.difference(new).copied().collect(),
                std::time::Instant::now(),
            ));
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
        // Cached, unprotected tiles must not consume the replacement working
        // set. Protected cover/transition tiles may exceed this cache target.
        while self.bytes > self.budget / 2 {
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
        self.reap_pools();
        if self.retiring_budget.is_some()
            || self.managed_committed_bytes() > self.budget as u64 * 3 / 4
        {
            self.shrink_pools();
            if self.bytes <= self.budget && self.managed_committed_bytes() <= self.budget as u64 {
                self.retiring_budget = None;
            }
        }
    }
    fn evict(&mut self, key: MeshKey) {
        if let Some(tile) = self.tiles.remove(&key) {
            self.bytes -= tile.bytes;
            for part in tile.parts {
                self.retire_part(part);
            }
            if let Some((p, n)) = tile.ray {
                self.raycaster.release(p, n);
            }
        }
    }
    /// Mesh keys are world-local. Never let a replacement world reuse old buffers.
    pub fn clear_world(&mut self) {
        self.clear_transition();
        let keys: Vec<_> = self.tiles.keys().copied().collect();
        for key in keys {
            self.evict(key);
        }
        self.actor = None;
        while let Some(mesh) = self.actor_meshes.pop_front() {
            for part in mesh.parts {
                self.retire_part(part);
            }
        }
        self.visibility = Default::default();
        self.visibility_limit = 24000.;
        self.culled = 0;
        if self.managed_committed_bytes() <= self.budget as u64 {
            self.retiring_budget = None;
        }
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
        self.reap_pools();
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
                // The ray arena's limit is the capacity of its replacement buffer.
                // Charge both the current allocation and every unfinished migration
                // before granting space for that replacement.
                let current = self.raycaster.allocated_bytes;
                let growth_limit = limit
                    .saturating_sub(self.managed_committed_bytes() as usize)
                    .min(self.device.limits().max_storage_buffer_binding_size as usize);
                let allocation =
                    self.raycaster
                        .upload(&self.device, &self.queue, &brick, growth_limit);
                if self.raycaster.allocated_bytes != current {
                    self.submitted += 1;
                    let serial = self.submitted;
                    self.ray_migrations.push((serial, current));
                    let completed = self.completion_tx.clone();
                    self.queue.on_submitted_work_done(move || {
                        let _ = completed.send(serial);
                    });
                }
                if let Some(allocation) = allocation {
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
        while self.bytes + size > limit / 2 {
            let candidate = self
                .tiles
                .iter()
                .filter(|(key, _)| !protected.contains(key))
                .min_by_key(|(_, tile)| tile.used)
                .map(|(key, _)| *key);
            let Some(candidate) = candidate else {
                if self.bytes + size > limit {
                    return Err("GPU mesh budget reached; reduce LOD tile budget".into());
                }
                break;
            };
            self.evict(candidate);
        }
        let heights = mesh.heights();
        let quads = mesh.quads();
        let faces = mesh.faces();
        let [x, z] = key.tile.origin_in(self.grid);
        let mut parts = Vec::new();
        for ((part, prepared), packed) in mesh.parts.into_iter().zip(prepared).zip(packed) {
            match self.upload_part(
                part,
                Some(prepared),
                Some(packed),
                [x, 0, z],
                [
                    self.grid.extent - x,
                    self.grid.spec.ceiling_cells(),
                    self.grid.extent - z,
                ],
            ) {
                Ok(Some(p)) => parts.push(p),
                Ok(None) => {}
                Err(e) => {
                    for p in parts {
                        self.retire_part(p);
                    }
                    return Err(e);
                }
            }
        }
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
            for part in old.parts {
                self.retire_part(part);
            }
        }
        self.bytes += size + ray_bytes;
        Ok(())
    }
    fn retire_part(&mut self, part: Part) {
        let serial = self.submitted;
        self.metadata_pool.arena.retire(part.metadata, serial);
        self.geometry_pool.arena.retire(part.geometry, serial);
        self.color_pool.arena.retire(part.colors, serial);
        self.vertex_pool.arena.retire(part.vertices, serial);
    }
    fn reap_pools(&mut self) {
        self.device.poll(wgpu::Maintain::Poll);
        while let Ok(done) = self.completion_rx.try_recv() {
            self.completed = self.completed.max(done);
        }
        self.metadata_pool.arena.reclaim(self.completed);
        self.geometry_pool.arena.reclaim(self.completed);
        self.color_pool.arena.reclaim(self.completed);
        self.vertex_pool.arena.reclaim(self.completed);
        self.pool_migrations
            .retain(|(serial, _)| *serial > self.completed);
        self.ray_migrations
            .retain(|(serial, _)| *serial > self.completed);
    }
    /// Current terrain buffers plus old buffers retained until migration completes.
    pub fn terrain_committed_bytes(&self) -> u64 {
        [
            &self.geometry_pool,
            &self.color_pool,
            &self.vertex_pool,
            &self.metadata_pool,
        ]
        .iter()
        .map(|p| p.arena.capacity())
        .sum::<u64>()
            + self
                .pool_migrations
                .iter()
                .map(|(_, b)| b.size())
                .sum::<u64>()
    }
    /// All managed terrain/voxel buffer capacity, including in-flight copies.
    pub fn managed_committed_bytes(&self) -> u64 {
        self.terrain_committed_bytes()
            + self.raycaster.allocated_bytes as u64
            + self
                .ray_migrations
                .iter()
                .map(|(_, bytes)| *bytes as u64)
                .sum::<u64>()
    }
    fn write_pool(&mut self, index: usize, data: &[u8], alignment: u64) -> Result<Handle, String> {
        let mut pools = [
            &mut self.geometry_pool,
            &mut self.color_pool,
            &mut self.vertex_pool,
            &mut self.metadata_pool,
        ];
        if let Some(h) = pools[index].write(&self.queue, data, alignment) {
            return Ok(h);
        }
        let committed = pools.iter().map(|p| p.arena.capacity()).sum::<u64>()
            + self
                .pool_migrations
                .iter()
                .map(|(_, b)| b.size())
                .sum::<u64>()
            + self.raycaster.allocated_bytes as u64
            + self
                .ray_migrations
                .iter()
                .map(|(_, bytes)| *bytes as u64)
                .sum::<u64>();
        let limit = self.retiring_budget.unwrap_or(self.budget).max(self.budget) as u64;
        let available = limit.saturating_sub(committed);
        let active_capacity = pools.iter().map(|p| p.arena.capacity()).sum::<u64>()
            + self.raycaster.allocated_bytes as u64;
        let pool = &mut pools[index];
        let old_capacity = pool.arena.capacity();
        // Keep the old buffer charged until its copy and all preceding draws finish.
        let minimum = (old_capacity + data.len().max(4) as u64 + alignment - 1).next_multiple_of(4);
        let binding_limit = if index == 3 {
            self.device.limits().max_buffer_size.min(u32::MAX as u64)
        } else {
            self.device.limits().max_storage_buffer_binding_size as u64
        };
        // Leave one quarter of the ceiling available for future migrations
        // and actor poses. Steady capacity and temporary migration capacity
        // are different limits; filling both makes compaction impossible.
        let steady_available = (limit * 3 / 4).saturating_sub(active_capacity - old_capacity);
        let capacity = (old_capacity + old_capacity / 2)
            .max(minimum)
            .min(available)
            .min(steady_available)
            .min(binding_limit);
        // Growth ratios and budget ceilings can produce 1/2/3-byte tails.
        // Round down after every ceiling so copies stay aligned without ever
        // exceeding the committed-memory budget or the binding size limit.
        let capacity = capacity / wgpu::COPY_BUFFER_ALIGNMENT * wgpu::COPY_BUFFER_ALIGNMENT;
        if capacity < minimum {
            return Err(format!(
                "GPU mesh budget reached ({} growth); reduce LOD tile budget",
                pool.label
            ));
        }
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(pool.label),
            size: capacity,
            usage: pool.usage | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("terrain arena migration"),
            });
        debug_assert_eq!(old_capacity % wgpu::COPY_BUFFER_ALIGNMENT, 0);
        encoder.copy_buffer_to_buffer(&pool.buffer, 0, &buffer, 0, old_capacity);
        // Submission flushes queued writes to the old buffer before copying it.
        self.queue.submit(Some(encoder.finish()));
        self.submitted += 1;
        let serial = self.submitted;
        let completed = self.completion_tx.clone();
        self.queue.on_submitted_work_done(move || {
            let _ = completed.send(serial);
        });
        let old = std::mem::replace(&mut pool.buffer, buffer);
        pool.arena.grow(capacity);
        self.pool_migrations.push((serial, old));
        self.refresh_tile_bind();
        let pools = [
            &mut self.geometry_pool,
            &mut self.color_pool,
            &mut self.vertex_pool,
            &mut self.metadata_pool,
        ];
        pools[index]
            .write(&self.queue, data, alignment)
            .ok_or_else(|| "GPU mesh budget reached (fragmented terrain arena)".into())
    }
    fn refresh_tile_bind(&mut self) {
        self.tile_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pooled terrain"),
            layout: &self.tile_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.metadata_pool.buffer,
                        offset: 0,
                        size: std::num::NonZeroU64::new(48),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.geometry_pool.buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.color_pool.buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.vertex_pool.buffer.as_entire_binding(),
                },
            ],
        });
    }
    /// Migrate one pool per trim, using only headroom under the retiring ceiling.
    fn shrink_pools(&mut self) {
        if !self.pool_migrations.is_empty() || !self.ray_migrations.is_empty() {
            return;
        }
        let limit = self.retiring_budget.unwrap_or(self.budget).max(self.budget) as u64;
        let available = limit.saturating_sub(self.managed_committed_bytes());
        for index in 0..4 {
            let alignment = [32, 4, 4, 256][index];
            let pools = [
                &self.geometry_pool,
                &self.color_pool,
                &self.vertex_pool,
                &self.metadata_pool,
            ];
            let pool = pools[index];
            if pool.arena.pending_bytes() != 0 {
                continue;
            }
            let packed = pool.arena.packed_size(alignment);
            let capacity = (packed + packed / 8).max(65536).next_multiple_of(256);
            if capacity >= pool.arena.capacity() * 3 / 4 || capacity > available {
                continue;
            }
            let pools = [
                &mut self.geometry_pool,
                &mut self.color_pool,
                &mut self.vertex_pool,
                &mut self.metadata_pool,
            ];
            let pool = &mut *pools[index];
            let mapping = pool.arena.compact(capacity, alignment);
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(pool.label),
                size: capacity,
                usage: pool.usage | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("terrain arena compaction"),
                });
            for (old, new) in &mapping {
                encoder.copy_buffer_to_buffer(
                    &pool.buffer,
                    old.offset,
                    &buffer,
                    new.offset,
                    old.size,
                );
            }
            self.queue.submit(Some(encoder.finish()));
            self.submitted += 1;
            let serial = self.submitted;
            let old = std::mem::replace(&mut pool.buffer, buffer);
            self.pool_migrations.push((serial, old));
            let completed = self.completion_tx.clone();
            self.queue.on_submitted_work_done(move || {
                let _ = completed.send(serial);
            });
            let handles: BTreeMap<_, _> = mapping
                .into_iter()
                .map(|(old, new)| (old.slot, new))
                .collect();
            let remap = |part: &mut Part| {
                let h = match index {
                    0 => &mut part.geometry,
                    1 => &mut part.colors,
                    2 => &mut part.vertices,
                    _ => &mut part.metadata,
                };
                *h = handles[&h.slot];
            };
            for tile in self.tiles.values_mut() {
                for part in &mut tile.parts {
                    remap(part);
                }
            }
            for actor in &mut self.actor_meshes {
                for part in &mut actor.parts {
                    remap(part);
                }
            }
            self.refresh_tile_bind();
            // Geometry/color/border moves change shader element indices. Metadata
            // copies preserve origins, clipping and transition phases.
            for part in self
                .tiles
                .values()
                .flat_map(|t| t.parts.iter())
                .chain(self.actor_meshes.iter().flat_map(|a| a.parts.iter()))
            {
                let indices = [
                    (part.geometry.offset / 32) as u32,
                    (part.colors.offset / 4) as u32,
                    (part.vertices.offset / 4) as u32,
                ];
                self.queue.write_buffer(
                    &self.metadata_pool.buffer,
                    part.metadata.offset + 32,
                    bytemuck::cast_slice(&indices),
                );
            }
            return;
        }
        if let Some(old) = self
            .raycaster
            .shrink(&self.device, &self.queue, available as usize)
        {
            self.submitted += 1;
            let serial = self.submitted;
            self.ray_migrations.push((serial, old));
            let completed = self.completion_tx.clone();
            self.queue.on_submitted_work_done(move || {
                let _ = completed.send(serial);
            });
        }
    }
    pub fn terrain_arena_stats(&self) -> [(u64, u64, u64); 4] {
        [
            &self.geometry_pool,
            &self.color_pool,
            &self.vertex_pool,
            &self.metadata_pool,
        ]
        .map(|p| {
            (
                p.arena.live_bytes(),
                p.arena.pending_bytes(),
                p.arena.capacity(),
            )
        })
    }
    fn upload_part(
        &mut self,
        part: microvoxel::render::terrain::SurfacePart,
        prepared: Option<triangulation::Prepared>,
        packed_colors: Option<Vec<u32>>,
        origin: [i64; 3],
        clip: [i64; 3],
    ) -> Result<Option<Part>, String> {
        if part.surface.quads.is_empty() {
            return Ok(None);
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
                    padding: factor.trailing_zeros()
                        | (u32::from(q.slope_light) << 15)
                        | (u32::from(q.constant_color) << 14),
                }
            })
            .collect();
        let packed_colors =
            packed_colors.unwrap_or_else(|| microvoxel::render::colors::pack(&part.surface.colors));
        self.reap_pools();
        // Every draw uses one shared bind group. These offsets are element indices,
        // leaving shader storage bindings fixed across the entire visible cover.
        let geometry = self.write_pool(
            0,
            bytemuck::cast_slice(&records),
            std::mem::size_of::<GpuQuad>() as u64,
        )?;
        let colors = match self.write_pool(1, bytemuck::cast_slice(&packed_colors), 4) {
            Ok(h) => h,
            Err(e) => {
                self.geometry_pool.arena.retire(geometry, self.submitted);
                return Err(e);
            }
        };
        let vertices = match self.write_pool(2, bytemuck::cast_slice(&references), 4) {
            Ok(h) => h,
            Err(e) => {
                self.geometry_pool.arena.retire(geometry, self.submitted);
                self.color_pool.arena.retire(colors, self.submitted);
                return Err(e);
            }
        };
        let mut meta = [0u32; 12];
        let floats = [
            origin[0] as f32,
            origin[1] as f32,
            origin[2] as f32,
            part.step as f32,
            clip[0] as f32,
            clip[1] as f32,
            clip[2] as f32,
            0.,
        ];
        for (out, v) in meta[..8].iter_mut().zip(floats) {
            *out = v.to_bits();
        }
        meta[8] = (geometry.offset / std::mem::size_of::<GpuQuad>() as u64) as u32;
        meta[9] = (colors.offset / 4) as u32;
        meta[10] = (vertices.offset / 4) as u32;
        let metadata = match self.write_pool(3, bytemuck::cast_slice(&meta), 256) {
            Ok(h) => h,
            Err(e) => {
                self.geometry_pool.arena.retire(geometry, self.submitted);
                self.color_pool.arena.retire(colors, self.submitted);
                self.vertex_pool.arena.retire(vertices, self.submitted);
                return Err(e);
            }
        };
        Ok(Some(Part {
            metadata,
            geometry,
            colors,
            vertices,
            count: regular_count,
            border_vertices,
        }))
    }
    pub fn move_cached_actor(&mut self, root: microvoxel::CellPos) {
        if let Some(actor) = &mut self.actor {
            actor.root = root;
        }
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
                )?
                .into_iter()
                .collect();
            if self.actor_meshes.len() >= 16 {
                if let Some(old) = self.actor_meshes.pop_front() {
                    for p in old.parts {
                        self.retire_part(p);
                    }
                }
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
        let mut local_cover = cover.clone();
        if !overview {
            local_cover.extend(self.transition_keys());
        }
        if overview || !self.settings.antialias {
            return self.render_scene(
                target,
                eye,
                matrix,
                &local_cover,
                selection,
                overview,
                cell_size,
            );
        }
        let current = std::sync::Arc::clone(&self.presentation.image);
        let count = self.render_scene(
            &current,
            eye,
            matrix,
            &local_cover,
            selection,
            false,
            cell_size,
        );
        self.presentation.draw(
            &self.device,
            &self.queue,
            target,
            self.width,
            self.height,
            true,
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
        self.timing.poll(&self.device);
        let timing_token = self.timing.begin(self.frame);
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
        if !overview {
            if let Some((old, start)) = &self.transition {
                let progress = (start.elapsed().as_secs_f32() / 0.16).clamp(0., 1.);
                for key in &visible {
                    let phase = if old.contains(key) {
                        2. + progress
                    } else if self.transition_in.contains(key) {
                        1. + progress
                    } else {
                        continue;
                    };
                    for part in &self.tiles[key].parts {
                        self.queue.write_buffer(
                            &self.metadata_pool.buffer,
                            part.metadata.offset + 28,
                            bytemuck::bytes_of(&phase),
                        );
                    }
                }
            }
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
                let mut meta = [0u32; 12];
                let floats = [
                    position.x as f32,
                    position.y as f32,
                    position.z as f32,
                    1.,
                    32768.,
                    32768.,
                    32768.,
                    0.,
                ];
                for (out, v) in meta[..8].iter_mut().zip(floats) {
                    *out = v.to_bits();
                }
                meta[8] = (part.geometry.offset / std::mem::size_of::<GpuQuad>() as u64) as u32;
                meta[9] = (part.colors.offset / 4) as u32;
                meta[10] = (part.vertices.offset / 4) as u32;
                self.queue.write_buffer(
                    &self.metadata_pool.buffer,
                    part.metadata.offset,
                    bytemuck::cast_slice(&meta),
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
                timestamp_writes: self.timing.pass_writes(timing_token),
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
                    pass.set_bind_group(1, &self.tile_bind, &[part.metadata.offset as u32]);
                    pass.draw(0..6, 0..part.count);
                }
            }
            if let Some((_, mesh)) = actor {
                for part in &mesh.parts {
                    pass.set_bind_group(1, &self.tile_bind, &[part.metadata.offset as u32]);
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
                        pass.set_bind_group(1, &self.tile_bind, &[part.metadata.offset as u32]);
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
        self.timing.resolve(&mut encoder, timing_token);
        self.queue.submit(Some(encoder.finish()));
        self.timing.after_submit(timing_token);
        self.submitted += 1;
        let serial = self.submitted;
        let completed = self.completion_tx.clone();
        self.queue.on_submitted_work_done(move || {
            let _ = completed.send(serial);
        });
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
    fn repeated_growth_aligns_fractional_ratios_and_odd_budget_ceilings() {
        use super::*;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let mut gpu = pollster::block_on(Gpu::new(&instance, None, 16, 16)).unwrap();
        // 260 * 1.5 = 390: a valid old buffer can produce an invalid new
        // copy length even without any odd-sized input payload.
        gpu.geometry_pool = Pool::new(
            &gpu.device,
            "odd growth regression",
            263,
            wgpu::BufferUsages::STORAGE,
        );
        gpu.refresh_tile_bind();
        assert_eq!(gpu.geometry_pool.buffer.size(), 260);
        let other = gpu.managed_committed_bytes() as usize - 260;
        gpu.budget = ((other + 389) * 4).div_ceil(3);
        let first = gpu.write_pool(0, &vec![0xabu8; 260], 4).unwrap();
        gpu.write_pool(0, &[0; 4], 4).unwrap();
        assert_eq!(gpu.geometry_pool.buffer.size(), 388);
        assert!(gpu.managed_committed_bytes() <= gpu.budget as u64);
        gpu.device.poll(wgpu::Maintain::Wait);
        gpu.reap_pools();
        gpu.budget = GPU_BUDGET;
        gpu.write_pool(0, &[0; 124], 4).unwrap();
        gpu.write_pool(0, &[0; 4], 4).unwrap();
        assert_eq!(gpu.geometry_pool.buffer.size(), 580);
        let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("odd growth readback"),
            size: 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&gpu.geometry_pool.buffer, first.offset, &staging, 0, 4);
        gpu.queue.submit(Some(encoder.finish()));
        let (tx, rx) = mpsc::channel();
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                tx.send(result).unwrap();
            });
        gpu.device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap().unwrap();
        assert_eq!(&*staging.slice(..).get_mapped_range(), &[0xab; 4]);
    }
    #[test]
    #[ignore = "needs a Vulkan/Metal/DX12 device; run explicitly"]
    fn lower_budget_compacts_high_offset_live_cover_and_updates_metadata() {
        use super::*;
        use microvoxel::render::lod::TileKey;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let mut gpu = pollster::block_on(Gpu::new(&instance, None, 16, 16)).unwrap();
        let padding = gpu.write_pool(0, &vec![0; 1048576], 32).unwrap();
        let geometry = gpu.write_pool(0, &[0xabu8; 32], 32).unwrap();
        let colors = gpu.write_pool(1, &[0; 4], 4).unwrap();
        let vertices = gpu.write_pool(2, &[0; 4], 4).unwrap();
        let metadata = gpu.write_pool(3, &[0; 48], 256).unwrap();
        let key = MeshKey {
            tile: TileKey {
                level: 0,
                x: 0,
                z: 0,
            },
            neighbors: [0; 4],
            revision: 0,
        };
        gpu.tiles.insert(
            key,
            Tile {
                solid_top: 0,
                ray: None,
                mesh_ready: true,
                parts: vec![Part {
                    geometry,
                    colors,
                    vertices,
                    metadata,
                    count: 1,
                    border_vertices: 0,
                }],
                bytes: 88,
                quads: 1,
                faces: 1,
                heights: [0; 2],
                used: 0,
            },
        );
        gpu.bytes = 88;
        gpu.geometry_pool.arena.retire(padding, gpu.submitted);
        gpu.retiring_budget = Some(gpu.budget);
        gpu.budget = 2 * 1048576;
        for _ in 0..8 {
            gpu.trim(&[key].into());
            gpu.device.poll(wgpu::Maintain::Wait);
        }
        gpu.trim(&[key].into());
        assert!(gpu.retiring_budget.is_none());
        assert!(gpu.managed_committed_bytes() <= gpu.budget as u64);
        let part = &gpu.tiles[&key].parts[0];
        assert_eq!(part.geometry.offset, 0);
        assert_eq!(part.geometry.generation, geometry.generation);
        assert!(gpu.geometry_pool.arena.contains(part.geometry));
        let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("compacted cover readback"),
            size: 36,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(
            &gpu.geometry_pool.buffer,
            part.geometry.offset,
            &staging,
            0,
            32,
        );
        encoder.copy_buffer_to_buffer(
            &gpu.metadata_pool.buffer,
            part.metadata.offset + 32,
            &staging,
            32,
            4,
        );
        gpu.queue.submit(Some(encoder.finish()));
        let (tx, rx) = mpsc::channel();
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                tx.send(result).unwrap();
            });
        gpu.device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap().unwrap();
        let mapped = staging.slice(..).get_mapped_range();
        assert_eq!(&mapped[..32], &[0xab; 32]);
        assert_eq!(&mapped[32..], &[0; 4]);
    }
    #[test]
    #[ignore = "needs a Vulkan/Metal/DX12 device; run explicitly"]
    fn ray_growth_shares_committed_budget_with_mesh_pools() {
        use super::*;
        use microvoxel::render::{lod::TileKey, ray::Brick};
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let mut gpu = pollster::block_on(Gpu::new(&instance, None, 16, 16)).unwrap();
        let key = MeshKey {
            tile: TileKey {
                level: 0,
                x: 0,
                z: 0,
            },
            neighbors: [0; 4],
            revision: 0,
        };
        let brick = Brick {
            words: vec![0; 1024],
            nodes: 0,
            runs: 0,
            bounds: [[0; 3], [1; 3]],
            solid_top: 0,
        };
        let before = gpu.ray_allocated_bytes();
        gpu.upload_ray(key, brick.clone(), &BTreeSet::new())
            .unwrap();
        assert!(gpu.ray_allocated_bytes() > before);
        assert_eq!(
            gpu.ray_migrations.iter().map(|(_, n)| *n).sum::<usize>(),
            before
        );
        assert!(gpu.managed_committed_bytes() <= gpu.budget as u64);
        gpu.device.poll(wgpu::Maintain::Wait);
        gpu.reap_pools();
        assert!(gpu.ray_migrations.is_empty());
        gpu.budget = gpu.managed_committed_bytes() as usize;
        let capacity = gpu.ray_allocated_bytes();
        let large = Brick {
            words: vec![0; capacity / 4],
            ..brick
        };
        assert!(gpu
            .upload_ray(MeshKey { revision: 1, ..key }, large, &[key].into())
            .unwrap_err()
            .contains("GPU mesh budget"));
        assert_eq!(gpu.ray_allocated_bytes(), capacity);
        assert!(gpu.tiles.contains_key(&key));
        assert!(gpu.managed_committed_bytes() <= gpu.budget as u64);
        gpu.retiring_budget = Some(GPU_BUDGET);
        gpu.budget = 2 * 1048576;
        gpu.evict(key);
        for _ in 0..8 {
            gpu.trim(&BTreeSet::new());
            gpu.device.poll(wgpu::Maintain::Wait);
        }
        gpu.trim(&BTreeSet::new());
        assert!(gpu.ray_allocated_bytes() < capacity);
        assert!(gpu.managed_committed_bytes() <= gpu.budget as u64);
        assert!(gpu.retiring_budget.is_none());
    }
    #[test]
    #[ignore = "needs a Vulkan/Metal/DX12 device; run explicitly"]
    fn arena_growth_copies_queued_writes_and_charges_migration_peak() {
        use super::*;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let mut gpu = pollster::block_on(Gpu::new(&instance, None, 16, 16)).unwrap();
        let initial = gpu.terrain_committed_bytes();
        let old_capacity = gpu.geometry_pool.arena.capacity();
        let bytes = vec![0xabu8; old_capacity as usize];
        let first = gpu.write_pool(0, &bytes, 32).unwrap();
        let next = gpu.write_pool(0, &[0xcdu8; 4], 32).unwrap();
        assert!(gpu.geometry_pool.arena.contains(first));
        assert_eq!(next.offset, old_capacity);
        assert_eq!(gpu.pool_migrations.len(), 1);
        assert_eq!(
            gpu.terrain_committed_bytes(),
            initial + gpu.geometry_pool.arena.capacity()
        );
        assert!(
            gpu.terrain_committed_bytes() + gpu.ray_allocated_bytes() as u64 <= gpu.budget as u64
        );
        let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("arena growth readback"),
            size: 8,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&gpu.geometry_pool.buffer, first.offset, &staging, 0, 4);
        encoder.copy_buffer_to_buffer(&gpu.geometry_pool.buffer, next.offset, &staging, 4, 4);
        gpu.queue.submit(Some(encoder.finish()));
        let (tx, rx) = mpsc::channel();
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                tx.send(result).unwrap();
            });
        gpu.device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap().unwrap();
        assert_eq!(
            &*staging.slice(..).get_mapped_range(),
            &[0xab, 0xab, 0xab, 0xab, 0xcd, 0xcd, 0xcd, 0xcd]
        );
        gpu.reap_pools();
        assert!(gpu.pool_migrations.is_empty());
        gpu.budget = (gpu.terrain_committed_bytes() + gpu.ray_allocated_bytes() as u64) as usize;
        let capacity = gpu.geometry_pool.arena.capacity();
        assert!(gpu
            .write_pool(0, &vec![0; capacity as usize], 32)
            .unwrap_err()
            .contains("GPU mesh budget"));
        assert_eq!(gpu.geometry_pool.arena.capacity(), capacity);
    }
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
