//! Optional exact NAADF window. Leading strips are generated and uploaded
//! asynchronously; the existing renderer covers the exterior and handoff.
mod arena;
mod column_cache;
mod data;
mod diagnostics;
mod dynamic_water;
#[cfg(test)]
mod dynamic_water_test;
pub(crate) mod fixture;
pub(crate) mod gpu_stages;
mod leaf;
#[cfg(test)]
mod local_exit_test;
pub(crate) mod ownership;
mod prepare_profile;
mod root_distance;
mod scatter;
mod scheduler;
mod sparse_roots;
mod startup;
pub(super) mod stream;
#[cfg(test)]
mod tests;
mod traversal;
pub(crate) use traversal::configure_player;
#[cfg(test)]
mod traversal_boundary_test;
mod upload;
use super::{Camera, Gpu};
use bytemuck::{Pod, Zeroable};
use data::Snapshot;
use frontier_world::{voxel_size, World};
use glam::DVec3;
use std::{
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Instant,
};
use wgpu::util::DeviceExt;
const SHADER: &str = concat!(
    "const NAADF_GROUP:u32=0u;const NAADF_BASE:u32=0u;\n",
    include_str!("trace.wgsl"),
    include_str!("../water.wgsl"),
    include_str!("../sunlight.wgsl"),
    include_str!("../ground_light.wgsl"),
    include_str!("../terrain_paint.wgsl"),
    include_str!("ray.wgsl")
);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Request {
    radius_voxels: u32,
    budget: usize,
    revision: u64,
    vegetation: bool,
    serial: u64,
    samples: u32,
    follow: bool,
    gpu_distance: bool,
    sparse_roots: bool,
    wide_leaves: bool,
}

impl Request {
    fn radius_roots(self) -> u32 {
        self.radius_voxels.div_ceil(stream::ROOT_EDGE_VOXELS)
    }

    fn radius_m(self) -> f64 {
        f64::from(self.radius_voxels) * voxel_size()
    }

    fn rounded_radius_m(self) -> f64 {
        f64::from(self.radius_roots()) * stream::root_size_m()
    }
}

// The user ceiling can tighten the estimate, never bypass live RAM headroom.
fn build_budget_limit(requested: usize, available: usize) -> usize {
    requested.min(available)
}

fn memory_headroom(memory: crate::settings::MemorySnapshot) -> Option<usize> {
    memory.measured.then(|| {
        memory
            .available_mib
            .saturating_sub((memory.total_mib / 10).max(1024))
            .saturating_mul(1048576)
    })
}

enum Job {
    Build(u64, Request, World, [f64; 3]),
    Install(u64, Snapshot, [usize; 3], leaf::Encoding, usize),
    Shift(u64, World, stream::Region, usize),
    Refresh(u64, World, Vec<frontier_world::Cell>, usize),
    Finish(stream::Change, bool),
    Reset,
}
enum Reply {
    Built(leaf::Prepared),
    Shifted(stream::Change),
}
struct Worker {
    tx: Option<mpsc::Sender<Job>>,
    rx: mpsc::Receiver<(u64, Result<Reply, String>)>,
    epoch: Arc<AtomicU64>,
    stage: Arc<AtomicU32>,
    handle: Option<thread::JoinHandle<()>>,
}
impl Worker {
    fn new() -> Self {
        let (tx, input) = mpsc::channel::<Job>();
        let (output, rx) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(0));
        let stage = Arc::new(AtomicU32::new(0));
        let e = epoch.clone();
        let s = stage.clone();
        let handle = thread::Builder::new()
            .name("naadf-build".into())
            .spawn(move || {
                let mut state: Option<stream::State> = None;
                while let Ok(job) = input.recv() {
                    match job {
                        Job::Build(g, r, w, eye) => {
                            if e.load(Ordering::Relaxed) != g {
                                continue;
                            }
                            state = None;
                            let cpu_root_fallback = r.gpu_distance && root_distance::cpu_fallback();
                            let result = data::build_with_root_mode(
                                &w,
                                eye,
                                r.radius_roots(),
                                r.budget,
                                &s,
                                r.gpu_distance && !cpu_root_fallback,
                                || e.load(Ordering::Relaxed) != g,
                            )
                            .map(|mut s| {
                                s.gpu_distance = r.gpu_distance;
                                s.cpu_root_fallback = cpu_root_fallback;
                                let mut prepared = leaf::Prepared::new(s, r.wide_leaves);
                                if r.sparse_roots {
                                    prepared.make_sparse_roots(|| e.load(Ordering::Relaxed) != g);
                                }
                                Reply::Built(prepared)
                            });
                            if e.load(Ordering::Relaxed) == g && output.send((g, result)).is_err() {
                                break;
                            }
                        }
                        Job::Install(g, s, c, encoding, storage_edge) => {
                            if e.load(Ordering::Relaxed) == g {
                                state =
                                    Some(stream::State::with_storage(s, c, encoding, storage_edge));
                            }
                        }
                        Job::Shift(g, w, r, budget) => {
                            if e.load(Ordering::Relaxed) != g {
                                continue;
                            }
                            let result = state
                                .as_mut()
                                .ok_or_else(|| "NAADF streaming state unavailable".to_string())
                                .and_then(|s| {
                                    s.prepare(&w, r, budget, &|| e.load(Ordering::Relaxed) != g)
                                });
                            if e.load(Ordering::Relaxed) != g {
                                if let (Some(s), Ok(c)) = (&mut state, result) {
                                    s.finish(c, false);
                                }
                                continue;
                            }
                            if output.send((g, result.map(Reply::Shifted))).is_err() {
                                break;
                            }
                        }
                        Job::Refresh(g, w, cells, budget) => {
                            if e.load(Ordering::Relaxed) != g {
                                continue;
                            }
                            let result = state
                                .as_mut()
                                .ok_or_else(|| "NAADF streaming state unavailable".to_string())
                                .and_then(|s| {
                                    s.prepare_refresh(&w, &cells, budget, &|| {
                                        e.load(Ordering::Relaxed) != g
                                    })
                                });
                            if e.load(Ordering::Relaxed) != g {
                                if let (Some(s), Ok(c)) = (&mut state, result) {
                                    s.finish(c, false);
                                }
                                continue;
                            }
                            if output.send((g, result.map(Reply::Shifted))).is_err() {
                                break;
                            }
                        }
                        Job::Finish(c, commit) => {
                            if let Some(s) = &mut state {
                                s.finish(c, commit);
                            }
                        }
                        Job::Reset => state = None,
                    }
                }
            })
            .expect("NAADF worker");
        Self {
            tx: Some(tx),
            rx,
            epoch,
            stage,
            handle: Some(handle),
        }
    }
    fn send(&self, job: Job) {
        let _ = self.tx.as_ref().unwrap().send(job);
    }
    fn cancel(&self) {
        self.epoch.fetch_add(1, Ordering::Relaxed);
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        self.tx.take();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    matrix: [[f32; 4]; 4],
    inverse: [[f32; 4]; 4],
    eye: [f32; 4],
    extent: [u32; 4],
    viewport: [f32; 4],
    sun: [f32; 4],
    sky: [f32; 4],
    far_matrix: [[f32; 4]; 4],
    shadow: [f32; 4],
    forward: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    projection: [f32; 4],
    ring: [u32; 4],
    cache: [u32; 4],
    edit_min: [i32; 4],
    edit_max: [i32; 4],
    water: [f32; 4],
    paint_origin: [u32; 4],
}
struct Volume {
    occupied: Option<stream::Region>,
    water_bounds: Option<stream::Region>,
    may_have_local_edits: bool,
    resources: Bindings,
    dynamic_water_key: Option<(frontier_world::Cell, [u32; 3], u64)>,
    dynamic_water_binding: u64,
    ownership: std::sync::Arc<ownership::Ownership>,
    pipeline: wgpu::RenderPipeline,
    dry_pipeline: Option<wgpu::RenderPipeline>,
    static_water_pipeline: Option<wgpu::RenderPipeline>,
    height: Option<traversal::Height>,
    diagnostics: diagnostics::Diagnostics,
    beam: traversal::Beam,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    _chunks: wgpu::Texture,
    root_distance: Option<root_distance::Cache>,
    cpu_root_fallback: bool,
    edited_root_proofs: bool,
    dirty_roots: Option<[[i64; 3]; 2]>,
    root_texture_dims: [u32; 3],
    sparse_roots: bool,
    blocks: wgpu::Buffer,
    voxels: wgpu::Buffer,
    palette: wgpu::Buffer,
    scatter: Option<scatter::Scatter>,
    region: stream::Region,
    bounds: [[f64; 3]; 2],
    ring: [u32; 3],
    capacities: [usize; 3],
    arena_resizes: u64,
    arena_copy_bytes: u64,
    n: u32,
    bytes: usize,
    wide_bytes: usize,
    leaf_encoding: leaf::Encoding,
    leaf_bytes: usize,
    samples: u32,
    revision: u64,
    stats: [usize; 5],
    build_ms: f64,
    cpu_profile: Option<prepare_profile::Metrics>,
}
impl Volume {
    fn static_shadow_ceiling(&self) -> f32 {
        // Relative to the published volume origin in native voxels. Keep a
        // cell of margin for floating-point ray origins near the ceiling.
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *ENABLED
            .get_or_init(|| std::env::var_os("FRONTIER_NAADF_SUN_TOP").is_none_or(|v| v != "0"))
        {
            self.occupied
                .map_or(0., |r| ((r.hi[1] - self.region.lo[1]) * 16) as f32 + 1.)
        } else {
            0.
        }
    }

    fn edit_bounds(&self) -> [[i32; 4]; 2] {
        self.dirty_roots
            .map_or([[i32::MIN; 4], [i32::MAX; 4]], |b| {
                std::array::from_fn(|k| {
                    std::array::from_fn(|a| {
                        if a < 3 {
                            b[k][a]
                                .saturating_sub(self.region.lo[a])
                                .clamp(i32::MIN as i64, i32::MAX as i64)
                                as i32
                        } else {
                            0
                        }
                    })
                })
            })
    }
    fn traversal_flags(&self) -> u32 {
        u32::from(self.sparse_roots)
            | (u32::from(self.leaf_encoding.is_compact()) << 1)
            | (u32::from(self.cpu_root_fallback) << 2)
            | (u32::from(self.edited_root_proofs) << 3)
    }

    fn has_dynamic_water(&self) -> bool {
        self.dynamic_water_key
            .is_some_and(|(_, size, _)| !size.contains(&0))
    }

    fn pipeline_for_view(&self, submerged: bool) -> &wgpu::RenderPipeline {
        if !submerged && !self.has_dynamic_water() {
            if self.water_bounds.is_none() {
                if let Some(dry) = &self.dry_pipeline {
                    return dry;
                }
            }
            if let Some(wet) = &self.static_water_pipeline {
                return wet;
            }
        }
        &self.pipeline
    }
    fn encode_traversal(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        viewport: [u32; 2],
        primary: bool,
    ) {
        if let Some(height) = &self.height {
            height.encode(
                &gpu.queue,
                encoder,
                self.ownership.id,
                self.n,
                self.region.dims(),
                self.sparse_roots,
                self.ring,
                gpu.naadf_gpu_stages.as_ref(),
            );
        }
        if primary
            && (self.occupied.is_some()
                || self.has_dynamic_water()
                || gpu.camera_submerged.get()
                || self.diagnostics.enabled
                || capture_layer() != CaptureLayer::Scene)
        {
            self.beam.encode(
                &gpu.device,
                encoder,
                &self.uniform,
                &self.resources.height,
                &self.diagnostics.buffer,
                self.ownership.id,
                viewport,
                gpu.naadf_gpu_stages.as_ref(),
            );
        }
    }

    fn root_bytes(&self) -> usize {
        self.root_texture_dims
            .map(|v| v as usize)
            .iter()
            .product::<usize>()
            * 4
    }
}
struct Pending {
    volume: Volume,
    snapshot: Snapshot,
    phase: u8,
    offset: usize,
    upload_ms: f64,
}
impl Pending {
    fn new(gpu: &Gpu, prepared: leaf::Prepared) -> Result<Self, String> {
        Self::with_traversal(gpu, prepared, traversal::options())
    }

    fn reserved(gpu: &Gpu, prepared: leaf::Prepared, edge: u32) -> Result<Self, String> {
        Self::with_storage(gpu, prepared, traversal::options(), edge)
    }

    fn with_traversal(
        gpu: &Gpu,
        prepared: leaf::Prepared,
        options: traversal::Options,
    ) -> Result<Self, String> {
        let edge = prepared.snapshot.chunks_per_edge;
        Self::with_storage(gpu, prepared, options, edge)
    }

    fn with_storage(
        gpu: &Gpu,
        prepared: leaf::Prepared,
        options: traversal::Options,
        edge: u32,
    ) -> Result<Self, String> {
        let leaf::Prepared {
            occupied,
            snapshot,
            encoding,
            wide_bytes,
            sparse_root_dims,
        } = prepared;
        let limits = gpu.device.limits();
        let n = edge;
        let growing = n != snapshot.chunks_per_edge;
        if n == 0 || n > 512 || n < snapshot.chunks_per_edge || n > limits.max_texture_dimension_3d
        {
            return Err("NAADF region exceeds supported/device storage extent".into());
        }
        for (name, bytes) in [
            ("blocks", snapshot.blocks.len().max(1) * 4),
            ("voxels", snapshot.voxels.len().max(1) * 4),
            ("palette", snapshot.palette.len() * 16),
        ] {
            if bytes > limits.max_storage_buffer_binding_size as usize
                || bytes > limits.max_buffer_size as usize
            {
                return Err(format!(
                    "NAADF {name} needs {} MiB; device binding limit is {} MiB",
                    bytes.div_ceil(1048576),
                    limits.max_storage_buffer_binding_size / 1048576
                ));
            }
        }
        let follow = growing || gpu.visual_settings.as_ref().is_some_and(|s| s.naadf_follow);
        let max = limits
            .max_buffer_size
            .min(limits.max_storage_buffer_binding_size as u64) as usize;
        let grow = |used: usize, extra: usize| {
            if follow {
                (used + used / 2)
                    .max(used + extra)
                    .min(max & !255)
                    .max(used)
            } else {
                used
            }
        };
        let ordinary_capacities = [
            grow(snapshot.blocks.len() * 4, 2 * 1048576),
            grow(snapshot.voxels.len() * 4, 4 * 1048576)
                .min((1usize << 30).saturating_mul(encoding.bits() as usize / 8)),
            grow(snapshot.palette.len() * 16, 2 * 1048576),
        ];
        let root_texture_dims = sparse_root_dims.unwrap_or([n; 3]);
        let [root_width, root_height, root_layers] = root_texture_dims;
        let root_bytes = root_width as usize * root_height as usize * root_layers as usize * 4;
        let headroom = memory_headroom(crate::settings::MemorySnapshot::detect());
        let requested_budget = gpu
            .visual_settings
            .as_ref()
            .filter(|s| !s.naadf_auto_memory)
            .map_or(usize::MAX, |s| s.naadf_budget_mib.saturating_mul(1048576));
        let mut capacities = ordinary_capacities;
        if follow && n >= 32 {
            // A high start can build an almost empty volume. Reserve room
            // for the first descent into terrain when measured RAM permits.
            for (i, floor_mib) in [(0, 8), (1, 24)] {
                capacities[i] = capacities[i].max((floor_mib * 1048576).min(max & !255));
            }
            if !headroom.is_some_and(|available| {
                root_bytes + capacities.iter().sum::<usize>() + 64 * 1048576 <= available
            }) {
                capacities = ordinary_capacities;
            }
        }
        if growing {
            capacities = startup::arena_reservation(
                [
                    snapshot.blocks.len() * 4,
                    snapshot.voxels.len() * 4,
                    snapshot.palette.len() * 16,
                ],
                capacities,
                [
                    max,
                    max.min((1usize << 30).saturating_mul(encoding.bits() as usize / 8)),
                    max,
                ],
                snapshot.chunks_per_edge as usize,
                n as usize,
                headroom
                    .unwrap_or(0)
                    .saturating_sub(root_bytes)
                    .saturating_sub(upload::scatter_allowance().unwrap_or(0))
                    .saturating_div(2)
                    .min(
                        requested_budget
                            .saturating_sub(root_bytes)
                            .saturating_sub(upload::scatter_allowance().unwrap_or(0)),
                    ),
            );
        }
        // Snapshot CPU data is still live at this point. Check the actual
        // arena capacities, including follow-mode slack, before allocating.
        let upload_bytes = root_bytes
            + capacities.iter().sum::<usize>()
            + upload::scatter_allowance().unwrap_or(0);

        if growing && upload_bytes > requested_budget {
            return Err(
                "NAADF reserved roots and initial arenas exceed the selected memory budget".into(),
            );
        }
        if let Some(headroom) = headroom {
            if upload_bytes > headroom {
                return Err(format!(
                    "NAADF upload needs {} MiB; {} MiB available with system headroom. Try a smaller radius.",
                    upload_bytes.div_ceil(1048576),
                    headroom / 1048576,
                ));
            }
        }
        let chunks = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("NAADF exact chunk words"),
            size: wgpu::Extent3d {
                width: root_width,
                height: root_height,
                depth_or_array_layers: root_layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R32Uint,
            usage: wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::TEXTURE_BINDING
                | if snapshot.gpu_distance && root_distance::direct_publication() {
                    wgpu::TextureUsages::STORAGE_BINDING
                } else {
                    wgpu::TextureUsages::empty()
                },
            view_formats: &[],
        });
        let buffer = |bytes: usize, label: &'static str| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes.max(4) as u64,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let blocks = buffer(capacities[0], "NAADF block words");
        let voxels = buffer(capacities[1], "NAADF voxel words");
        let palette = buffer(capacities[2], "NAADF exact plant pigments");
        let uniform = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("NAADF camera"),
                contents: bytemuck::bytes_of(&Uniform::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let layout = layout(&gpu.device);
        let view = chunks.create_view(&Default::default());
        let build_distance = !growing
            && snapshot.gpu_distance
            && headroom.is_none_or(|bytes| upload_bytes + root_distance::bytes(n) <= bytes);
        let (root_distance, distance_view) = if build_distance {
            let (mut cache, view) =
                root_distance::Cache::new(&gpu.device, n, snapshot.region(), &view);
            cache.stages = gpu.naadf_gpu_stages.clone();
            (Some(cache), view)
        } else {
            (None, root_distance::empty(&gpu.device))
        };
        let (height, height_view) = if options.height_enabled() || options.beam {
            let (height, view) = traversal::Height::new(&gpu.device, n, &view, options);
            (Some(height), view)
        } else {
            (None, traversal::empty_height(&gpu.device))
        };
        let beam = traversal::Beam::new(&gpu.device, options);
        let (pipeline, dry_pipeline, static_water_pipeline) = make_pipelines(
            gpu,
            &layout,
            beam.layout(),
            snapshot.water_bounds.is_some(),
            options,
            [
                1 + u32::from(encoding.is_compact()),
                1 + u32::from(sparse_root_dims.is_some()),
                u32::from(root_distance.is_none()),
            ],
        );
        let diagnostics = diagnostics::Diagnostics::new(&gpu.device, options.diagnostics);
        let resources = Bindings {
            uniform: uniform.clone(),
            chunks: view,
            distance: distance_view,
            height: height_view,
            diagnostics: diagnostics.buffer.clone(),
            blocks: blocks.clone(),
            voxels: voxels.clone(),
            palette: palette.clone(),
            dynamic_water: dynamic_water::empty(&gpu.device),
            ground_light: crate::terrain_light::buffer(&gpu.device),
        };
        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("NAADF snapshot"),
            layout: &layout,
            entries: &resources.entries(0),
        });
        let scatter = upload::scatter_allowance().map(|allowance| {
            let mut scatter =
                scatter::Scatter::new(&gpu.device, [&blocks, &voxels, &palette], allowance);
            scatter.stages = gpu.naadf_gpu_stages.clone();
            scatter
        });
        let volume = Volume {
            occupied,
            may_have_local_edits: snapshot.region_has_edits,
            water_bounds: snapshot.water_bounds,
            resources,
            dynamic_water_key: None,
            dynamic_water_binding: 0,
            ownership: snapshot.ownership.clone(),
            pipeline,
            dry_pipeline,
            static_water_pipeline,
            height,
            diagnostics,
            beam,
            bind,
            uniform,
            _chunks: chunks,
            root_distance,
            cpu_root_fallback: snapshot.cpu_root_fallback,
            edited_root_proofs: false,
            dirty_roots: None,
            root_texture_dims,
            sparse_roots: sparse_root_dims.is_some(),
            blocks,
            voxels,
            palette,
            scatter,
            region: snapshot.region(),
            bounds: snapshot.bounds(),
            ring: [0; 3],
            capacities,
            arena_resizes: 0,
            arena_copy_bytes: 0,
            n,
            bytes: snapshot.bytes(),
            wide_bytes,
            leaf_encoding: encoding,
            leaf_bytes: snapshot.voxels.len().max(1) * 4,
            samples: gpu.sample_count,
            revision: snapshot.source_revision,
            stats: [
                snapshot.mixed_chunks,
                snapshot.unique_blocks,
                snapshot.sampled_columns,
                snapshot.plant_cells,
                snapshot.palette.len(),
            ],
            build_ms: snapshot.build_ms,
            cpu_profile: snapshot.cpu_profile.clone(),
        };
        Ok(Self {
            volume,
            snapshot,
            phase: 0,
            offset: 0,
            upload_ms: 0.,
        })
    }
    fn advance(&mut self, queue: &wgpu::Queue) -> bool {
        if self.phase >= 4 {
            return true;
        }
        let start = Instant::now();
        let allowance = 8 * 1048576;
        if self.phase == 0 {
            let [width, height, depth] = if self.volume.sparse_roots {
                self.volume.root_texture_dims
            } else {
                [self.snapshot.chunks_per_edge; 3]
            };
            let plane = width as usize * height as usize * 4;
            let layers = (allowance / plane).max(1).min(depth as usize - self.offset);
            let bytes = bytemuck::cast_slice::<_, u8>(&self.snapshot.chunks);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.volume._chunks,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: self.offset as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes[self.offset * plane..(self.offset + layers) * plane],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: layers as u32,
                },
            );
            self.offset += layers;
            if self.offset == depth as usize {
                self.phase = 1;
                self.offset = 0;
            }
        } else {
            let (buffer, bytes) = match self.phase {
                1 => (
                    &self.volume.blocks,
                    bytemuck::cast_slice::<_, u8>(&self.snapshot.blocks),
                ),
                2 => (
                    &self.volume.voxels,
                    bytemuck::cast_slice::<_, u8>(&self.snapshot.voxels),
                ),
                _ => (
                    &self.volume.palette,
                    bytemuck::cast_slice::<_, u8>(&self.snapshot.palette),
                ),
            };
            let count = allowance.min(bytes.len() - self.offset);
            if count > 0 {
                queue.write_buffer(
                    buffer,
                    self.offset as u64,
                    &bytes[self.offset..self.offset + count],
                );
            }
            self.offset += count;
            if self.offset == bytes.len() {
                self.phase += 1;
                self.offset = 0;
            }
        }
        self.upload_ms += start.elapsed().as_secs_f64() * 1000.;
        self.phase >= 4
    }
}
#[derive(Default)]
struct ShiftTiming {
    start: Option<Instant>,
    ready: Option<Instant>,
    upload: Option<Instant>,
    raster: Option<Instant>,
}
impl ShiftTiming {
    fn sample(&self, published: Instant) -> Option<[f64; 4]> {
        let start = self.start?;
        let ready = self.ready?;
        let ms = |a: Instant, b: Instant| b.duration_since(a).as_secs_f64() * 1000.;
        Some([
            ms(start, ready),
            ms(ready, self.upload?),
            ms(ready, self.raster?),
            ms(start, published),
        ])
    }
}
fn fixed_center() -> Result<Option<[f64; 3]>, &'static str> {
    static CENTER: std::sync::OnceLock<Result<Option<[f64; 3]>, &'static str>> =
        std::sync::OnceLock::new();
    *CENTER.get_or_init(|| {
        let Some(raw) = std::env::var_os("FRONTIER_NAADF_FIXED_CENTER") else {
            return Ok(None);
        };
        let error = "FRONTIER_NAADF_FIXED_CENTER requires three finite metres as x,y,z";
        let mut fields = raw.to_str().ok_or(error)?.split(',');
        let mut center = [0.; 3];
        for value in &mut center {
            *value = fields
                .next()
                .ok_or(error)?
                .trim()
                .parse::<f64>()
                .map_err(|_| error)?;
            if !value.is_finite() || value.abs() > 1e12 {
                return Err(error);
            }
        }
        if fields.next().is_some() {
            return Err(error);
        }
        Ok(Some(center))
    })
}

fn stream_lead_seconds() -> f64 {
    static LEAD: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *LEAD.get_or_init(|| {
        std::env::var("FRONTIER_NAADF_LEAD_SECONDS")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|s| s.is_finite())
            .unwrap_or(0.)
            .clamp(0., 8.)
    })
}
pub struct Experiment {
    restoring_raster: bool,
    // A completed inner snapshot stays visible while the next radius builds.
    expanding: bool,
    incremental_startup: bool,
    growth_pending: bool,
    startup_started: Option<Instant>,
    startup_publications: Vec<(f64, f64)>,
    startup_generated_roots: u64,
    wide_leaves: bool,
    effective_budget_mib: usize,
    worker: Worker,
    request: Option<Request>,
    epoch: u64,
    active: Option<Volume>,
    source_world: Option<World>,
    refresh_world: Option<World>,
    last_update_was_refresh: bool,
    pending: Option<Pending>,
    shift: Option<upload::PendingShift>,
    stream_upload: upload::UploadStats,
    shifting: bool,
    shifts: u64,
    strip_ms: f64,
    strip_columns: usize,
    strip_shared_leaves: usize,
    proof_refresh_ms: f64,
    proofs_refreshed: u64,
    root_occupancy_bytes: usize,
    shift_timing: ShiftTiming,
    shift_samples: std::collections::VecDeque<[f64; 4]>,
    prepare_samples: std::collections::VecDeque<(f64, prepare_profile::Metrics)>,
    last_eye: Option<(Instant, DVec3)>,
    eye_velocity: DVec3,
    camera_margin_m: f64,
    min_camera_margin_m: Option<f64>,
    stream_active_frames: u64,
    stream_outside_frames: u64,
    sky_frames: u64,
    split_pixels: std::cell::Cell<[u64; 2]>,
    sky_clearance: crate::sky_clearance::Airspace,
    pub status: String,
    pub stage: u32,
    pub upload_ms: f64,
    pub refused: u64,
    published_epoch: u64,
    pub error: Option<String>,
}
impl Default for Experiment {
    fn default() -> Self {
        Self {
            restoring_raster: false,
            expanding: false,
            incremental_startup: false,
            growth_pending: false,
            startup_started: None,
            startup_publications: Vec::new(),
            startup_generated_roots: 0,
            wide_leaves: false,
            effective_budget_mib: 0,
            worker: Worker::new(),
            request: None,
            epoch: 0,
            active: None,
            source_world: None,
            refresh_world: None,
            last_update_was_refresh: false,
            pending: None,
            shift: None,
            stream_upload: upload::UploadStats::default(),
            shifting: false,
            shifts: 0,
            strip_ms: 0.,
            strip_columns: 0,
            strip_shared_leaves: 0,
            proof_refresh_ms: 0.,
            proofs_refreshed: 0,
            root_occupancy_bytes: 0,
            shift_timing: ShiftTiming::default(),
            shift_samples: Default::default(),
            prepare_samples: Default::default(),
            last_eye: None,
            eye_velocity: DVec3::ZERO,
            camera_margin_m: 0.,
            min_camera_margin_m: None,
            stream_active_frames: 0,
            stream_outside_frames: 0,
            sky_frames: 0,
            split_pixels: Default::default(),
            sky_clearance: Default::default(),
            status: "Off".into(),
            stage: 0,
            upload_ms: 0.,
            refused: 0,
            published_epoch: 0,
            error: None,
        }
    }
}
impl Experiment {
    pub fn poll(
        &mut self,
        gpu: &Gpu,
        world: &World,
        camera: Camera,
        serial: u64,
        pressure: bool,
        raster_restored: bool,
    ) {
        let Some(s) = &gpu.visual_settings else {
            return;
        };
        let now = Instant::now();
        if let Some((previous, eye)) = self.last_eye.replace((now, camera.position)) {
            let dt = now.duration_since(previous).as_secs_f64();
            let displacement = camera.position - eye;
            if dt > 0.5 || displacement.length() > 8. {
                self.eye_velocity = DVec3::ZERO;
            } else if dt >= 0.0001 {
                self.eye_velocity = self
                    .eye_velocity
                    .lerp(displacement / dt, 1. - (-dt / 0.25).exp());
            }
        }
        for volume in self
            .active
            .iter_mut()
            .chain(self.pending.iter_mut().map(|p| &mut p.volume))
        {
            if volume.samples != gpu.sample_count {
                (
                    volume.pipeline,
                    volume.dry_pipeline,
                    volume.static_water_pipeline,
                ) = make_pipelines(
                    gpu,
                    &volume.pipeline.get_bind_group_layout(0),
                    volume.beam.layout(),
                    volume.water_bounds.is_some(),
                    traversal::options(),
                    [
                        1 + u32::from(volume.leaf_encoding.is_compact()),
                        1 + u32::from(volume.sparse_roots),
                        u32::from(volume.root_distance.is_none()),
                    ],
                );
                volume.samples = gpu.sample_count;
            }
        }
        if !s.naadf_enabled {
            self.wide_leaves = false;
            if self.request.is_some() {
                self.worker.cancel();
                self.worker.send(Job::Reset);
            }
            self.shift = None;
            self.shifting = false;
            self.request = None;
            self.expanding = false;
            self.growth_pending = false;
            self.pending = None;
            self.restoring_raster = self.active.is_some();
            if raster_restored {
                self.active = None;
                self.restoring_raster = false;
            }
            self.status = if self.active.is_some() {
                "Restoring raster coverage before disabling"
            } else {
                "Off"
            }
            .into();
            return;
        }
        if gpu.volume_owner || gpu.adaptive.is_none() {
            self.status = "NAADF experiment currently requires the adaptive terrain scene".into();
            self.error = Some(self.status.clone());
            return;
        }
        let target_roots = match stream::radius_roots(s.naadf_radius_voxels) {
            Ok(roots) => roots,
            Err(error) => {
                self.status = error.clone();
                self.error = Some(error);
                return;
            }
        };
        let mut r = Request {
            radius_voxels: s.naadf_radius_voxels,
            budget: if s.naadf_auto_memory {
                usize::MAX
            } else {
                s.naadf_budget_mib.saturating_mul(1048576)
            },
            revision: world.revision(),
            vegetation: world.vegetation_enabled(),
            serial,
            samples: 0,
            follow: s.naadf_follow,
            gpu_distance: s.naadf_gpu_distance,
            sparse_roots: !s.naadf_follow
                && !s.naadf_gpu_distance
                && std::env::var_os("FRONTIER_NAADF_SPARSE_ROOTS").is_some_and(|v| v == "1"),
            wide_leaves: self.wide_leaves
                || std::env::var_os("FRONTIER_NAADF_WIDE_LEAVES").is_some_and(|v| v == "1"),
        };
        // Content revisions use atomic local root replacement on the ordinary
        // following path. Whole-volume skip-proof experiments retain rebuilds.
        let local_edits = r.follow
            && !r.gpu_distance
            && !r.sparse_roots
            && self
                .source_world
                .as_ref()
                .is_none_or(|old| old.shares_source_lineage(world))
            && root_distance::occupancy::requested().is_none();
        if local_edits {
            if let Some(old) = self.request {
                r.revision = old.revision;
            }
        }
        if self.request != Some(r) {
            let eye = camera.position.to_array();
            let build_center = if r.follow {
                eye
            } else {
                match fixed_center() {
                    Ok(value) => value.unwrap_or(eye),
                    Err(error) => {
                        self.status = error.into();
                        self.error = Some(error.into());
                        return;
                    }
                }
            };
            self.restoring_raster = self.active.is_some();
            let source_changed = self
                .request
                .is_some_and(|old| old.vegetation != r.vegetation);
            self.worker.cancel();
            self.epoch = self.worker.epoch.load(Ordering::Relaxed);
            self.request = Some(r);
            self.source_world = Some(world.clone());
            self.refresh_world = None;
            self.last_update_was_refresh = false;
            self.pending = None;
            self.shift = None;
            self.shifting = false;
            if self
                .active
                .as_ref()
                .is_some_and(|a| a.revision != world.revision() || source_changed)
            {
                self.active = None;
            }
            self.worker.stage.store(0, Ordering::Relaxed);
            self.startup_started = Some(Instant::now());
            self.startup_publications.clear();
            self.startup_generated_roots = 0;
            self.expanding = true;
            // Height tiles use the published residency dimensions and physical
            // ring separately. Shell growth and edit publications invalidate
            // their reduction through the new ownership ID.
            self.incremental_startup =
                startup::enabled() && self.active.is_none() && !r.gpu_distance && !r.sparse_roots;
            self.growth_pending = false;
            self.status = "Building nearby exact voxels".into();
            self.error = None;
            let headroom = memory_headroom(crate::settings::MemorySnapshot::detect())
                .unwrap_or_else(|| s.naadf_budget_mib.saturating_mul(1048576));
            let effective_budget = build_budget_limit(r.budget, headroom);
            self.effective_budget_mib = effective_budget / 1048576;
            let mut job = r;
            job.budget = effective_budget;
            let first_roots = startup::first_radius(target_roots, self.incremental_startup);
            job.radius_voxels = first_roots * stream::ROOT_EDGE_VOXELS;
            self.worker
                .send(Job::Build(self.epoch, job, world.clone(), build_center));
        }
        if pressure {
            if self.active.is_some() && self.published_epoch == self.epoch {
                if self.expanding || self.growth_pending {
                    self.worker.cancel();
                    self.epoch = self.worker.epoch.load(Ordering::Relaxed);
                    self.published_epoch = self.epoch;
                    self.worker.send(Job::Reset);
                    self.pending = None;
                    self.shift = None;
                    self.shifting = false;
                    self.refresh_world = None;
                    self.expanding = false;
                    self.growth_pending = false;
                    self.status = "NAADF expansion paused: low RAM; rebuild to retry".into();
                    self.error = Some(self.status.clone());
                    return;
                }
                if self.shifting {
                    self.worker.cancel();
                    self.epoch = self.worker.epoch.load(Ordering::Relaxed);
                    self.published_epoch = self.epoch;
                    self.shifting = false;
                    self.refresh_world = None;
                    if let Some(p) = self.shift.take() {
                        self.worker.send(Job::Finish(p.change, false));
                    }
                }
                while let Ok((_, result)) = self.worker.rx.try_recv() {
                    if let Ok(Reply::Shifted(c)) = result {
                        self.worker.send(Job::Finish(c, false));
                    }
                }
                self.status = "NAADF streaming paused: low available RAM".into();
            } else {
                self.worker.cancel();
                self.expanding = false;
                self.growth_pending = false;
                self.pending = None;
                self.status =
                    "NAADF build cancelled: low available RAM; rebuild here to retry".into();
                self.error = Some(self.status.clone());
            }
            return;
        }
        self.stage = self.worker.stage.load(Ordering::Relaxed);
        while let Ok((epoch, result)) = self.worker.rx.try_recv() {
            if epoch != self.epoch {
                // A stale prepared strip may own reservations in the current
                // arena after pressure cancellation. Full rebuilds replace it.
                if self.active.is_some()
                    && self.published_epoch == self.epoch
                    && !self.restoring_raster
                {
                    if let Ok(Reply::Shifted(c)) = result {
                        self.worker.send(Job::Finish(c, false));
                    }
                }
                continue;
            }
            match result {
                Ok(Reply::Built(snapshot)) => match if self.incremental_startup {
                    Pending::reserved(gpu, snapshot, target_roots * 2)
                } else {
                    Pending::new(gpu, snapshot)
                } {
                    Ok(p) => {
                        // Growth usually contains the old owner. If the player
                        // travelled outside it, restore outgoing raster first.
                        self.restoring_raster = self
                            .active
                            .as_ref()
                            .is_some_and(|old| !startup::contains(p.volume.bounds, old.bounds));
                        self.pending = Some(p);
                        self.status = "Uploading next exact region".into();
                    }
                    Err(e) => {
                        self.error = Some(e.clone());
                        self.status = e;
                        self.refused += 1;
                        self.expanding = false;
                        self.growth_pending = false;
                    }
                },
                Ok(Reply::Shifted(c)) => {
                    self.shift_timing.ready = Some(Instant::now());
                    if let Some(v) = &mut self.active {
                        if let Err(error) =
                            v.ensure_arenas(gpu, &c, self.effective_budget_mib * 1048576)
                        {
                            self.worker.send(Job::Finish(c, false));
                            self.error = Some(error.clone());
                            self.status = error;
                            self.refused += 1;
                            self.shifting = false;
                            self.refresh_world = None;
                            self.expanding = false;
                            self.growth_pending = false;
                            continue;
                        }
                    }
                    self.shift = Some(upload::PendingShift::new(gpu, c));
                    self.status =
                        "Uploading leading strips; restoring outgoing mesh coverage".into();
                }
                Err(e) if e == leaf::WIDEN && !self.wide_leaves => {
                    // A leading strip introduced a pigment the live format
                    // cannot address. Rebuild off-thread with wide leaves;
                    // the active volume remains available during replacement.
                    self.wide_leaves = true;
                    self.status = "Preparing 32-bit NAADF leaf fallback".into();
                    self.shifting = false;
                    return;
                }
                Err(e) => {
                    self.error = Some(e.clone());
                    self.status = e;
                    self.refused += 1;
                    self.shifting = false;
                    self.refresh_world = None;
                    self.expanding = false;
                    self.growth_pending = false;
                }
            }
        }
        if let Some(p) = &mut self.pending {
            if p.advance(&gpu.queue) && (!self.restoring_raster || raster_restored) {
                let p = self.pending.take().unwrap();
                self.upload_ms = p.upload_ms;
                let resident = p.volume.resident_radius();
                self.startup_generated_roots += p.snapshot.chunks.len() as u64;
                self.expanding = false;
                self.growth_pending = p.volume.region.dims()[0] < target_roots as usize * 2;
                if let Some(started) = self.startup_started {
                    let elapsed = started.elapsed().as_secs_f64();
                    self.startup_publications.push((resident, elapsed));
                    startup::log_residency(resident, r.rounded_radius_m(), elapsed);
                }
                if r.follow || self.growth_pending {
                    self.worker.send(Job::Install(
                        self.epoch,
                        p.snapshot,
                        if self.incremental_startup {
                            p.volume.arena_limits(gpu)
                        } else {
                            p.volume.capacities
                        },
                        p.volume.leaf_encoding,
                        p.volume.n as usize,
                    ));
                }
                self.active = Some(p.volume);
                self.restoring_raster = false;
                self.published_epoch = self.epoch;
                self.status = if self.growth_pending {
                    format!(
                        "{} m visible · expanding toward {} m",
                        self.active.as_ref().unwrap().resident_radius(),
                        r.rounded_radius_m()
                    )
                } else if r.follow {
                    "Ready · following player".into()
                } else {
                    "Ready · fixed region".into()
                };
            }
        }
        // An empty published volume owns no visible geometry. Keep it parked
        // while the complete requested cube and a flight approach margin are
        // provably above everything at the destination. On descent or edits
        // the ordinary jump/strip scheduler resumes in this same poll.
        static PARK_EMPTY_SKY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if s.flight_horizon
            && r.follow
            && !r.gpu_distance
            && !self.shifting
            && !self.expanding
            && !self.growth_pending
            && !self.restoring_raster
            && self.ready_for_water_source(world)
            && self.active.as_ref().is_some_and(|v| {
                v.occupied.is_none()
                    && !v.may_have_local_edits
                    && v.dynamic_water_key.is_none()
                    && !v.diagnostics.enabled
            })
            && capture_layer() == CaptureLayer::Scene
            && *PARK_EMPTY_SKY.get_or_init(|| {
                std::env::var_os("FRONTIER_FLIGHT_PARK_SKY").is_none_or(|v| v != "0")
            })
        {
            let approach = r.rounded_radius_m() + 128.;
            if self
                .sky_clearance
                .clear_above(world, camera.position, approach, approach)
            {
                let v = self.active.as_ref().unwrap();
                self.camera_margin_m = (0..3)
                    .map(|a| {
                        (camera.position[a] - v.bounds[0][a])
                            .min(v.bounds[1][a] - camera.position[a])
                    })
                    .fold(f64::INFINITY, f64::min);
                self.sky_frames += 1;
                if self.status != "Ready · clear air" {
                    self.status = "Ready · clear air".into();
                }
                return;
            }
        }
        // A destination jump supersedes the in-flight shift before any
        // queued strip can publish for the old camera demand.
        if r.follow
            && !self.restoring_raster
            && self.published_epoch == self.epoch
            && self.active.as_ref().is_some_and(|v| {
                (0..3).any(|a| {
                    let x = camera.position[a];
                    x < v.bounds[0][a] - r.rounded_radius_m() * 0.5
                        || x > v.bounds[1][a] + r.rounded_radius_m() * 0.5
                })
            })
        {
            self.worker.cancel();
            self.request = None;
            self.shift = None;
            self.shifting = false;
            self.refresh_world = None;
            self.status = "Preparing NAADF at new location".into();
            return;
        }
        if let (Some(p), Some(v)) = (&mut self.shift, &mut self.active) {
            let (uploaded, frame_upload) = p.advance(gpu, v);
            self.stream_upload.record(frame_upload);
            if uploaded {
                self.shift_timing.upload.get_or_insert_with(Instant::now);
            }
            if raster_restored {
                self.shift_timing.raster.get_or_insert_with(Instant::now);
            }
            if uploaded && raster_restored {
                if let Some(sample) = self.shift_timing.sample(Instant::now()) {
                    if self.shift_samples.len() == 64 {
                        self.shift_samples.pop_front();
                    }
                    self.shift_samples.push_back(sample);
                }
                let p = self.shift.take().unwrap();
                let (c, copies) = p.publish(gpu, v);
                self.stream_upload.publish_copies += copies;
                self.shifts += 1;
                self.strip_ms = c.build_ms;
                self.strip_columns = c.generated_columns;
                self.strip_shared_leaves = c.shared_leaves;
                self.proof_refresh_ms += c.root_proof_refresh_ms;
                self.proofs_refreshed += c.root_proofs_refreshed as u64;
                self.root_occupancy_bytes = c.root_occupancy_bytes;
                if let Some(profile) = &c.cpu_profile {
                    if self.prepare_samples.len() == 64 {
                        self.prepare_samples.pop_front();
                    }
                    self.prepare_samples
                        .push_back((c.build_ms, profile.clone()));
                }
                if self.expanding {
                    self.startup_generated_roots +=
                        c.roots.iter().map(|r| r.nodes.len() as u64).sum::<u64>();
                }
                if self.refresh_world.is_some() {
                    for root in &c.roots {
                        let b = v
                            .dirty_roots
                            .get_or_insert([root.region.lo, root.region.hi.map(|x| x - 1)]);
                        for a in 0..3 {
                            b[0][a] = b[0][a].min(root.region.lo[a]);
                            b[1][a] = b[1][a].max(root.region.hi[a] - 1);
                        }
                    }
                }
                self.worker.send(Job::Finish(c, true));
                if let Some(updated) = self.refresh_world.take() {
                    v.revision = updated.revision();
                    v.edited_root_proofs = true;
                    self.source_world = Some(updated);
                }
                self.shifting = false;
                if self.expanding {
                    let resident = v.resident_radius();
                    self.expanding = false;
                    self.growth_pending = v.region.dims()[0] < target_roots as usize * 2;
                    if let Some(started) = self.startup_started {
                        let elapsed = started.elapsed().as_secs_f64();
                        self.startup_publications.push((resident, elapsed));
                        startup::log_residency(resident, r.rounded_radius_m(), elapsed);
                    }
                }
                if !r.follow && !self.growth_pending {
                    self.worker.send(Job::Reset);
                }
                self.stage = 100;
                self.worker.stage.store(100, Ordering::Relaxed);
                self.status = if self.growth_pending {
                    format!(
                        "{} m visible · expanding toward {} m",
                        v.resident_radius(),
                        r.rounded_radius_m()
                    )
                } else if r.follow {
                    "Ready · following player".into()
                } else {
                    "Ready · fixed region".into()
                };
            }
        }
        // Select only after publication releases the current job. Alternate
        // pending content with demanded movement/growth so continuous edits and
        // continuous travel each receive a bounded share of idle turns.
        let following_target = if r.follow && !self.growth_pending {
            self.active.as_ref().and_then(|v| {
                stream::target(
                    v.region,
                    camera.position.to_array(),
                    self.eye_velocity.to_array(),
                    stream_lead_seconds(),
                )
            })
        } else {
            None
        };
        let update = scheduler::next_update(
            !self.shifting
                && !self.expanding
                && !self.restoring_raster
                && self.active.is_some()
                && self.published_epoch == self.epoch
                && self.error.is_none(),
            local_edits
                && self
                    .source_world
                    .as_ref()
                    .is_some_and(|old| old.revision() != world.revision()),
            self.growth_pending,
            following_target.is_some(),
            self.last_update_was_refresh,
        );
        if update == Some(scheduler::Update::Refresh) {
            self.last_update_was_refresh = true;
            let old = self.source_world.as_ref().unwrap();
            let cells = old.changed_edit_cells(world);
            self.refresh_world = Some(world.clone());
            self.shifting = true;
            self.shift_timing = ShiftTiming {
                start: Some(Instant::now()),
                ..Default::default()
            };
            self.worker.send(Job::Refresh(
                self.epoch,
                world.clone(),
                cells,
                self.effective_budget_mib * 1048576,
            ));
            self.status = "Updating changed voxel roots".into();
        }
        if update == Some(scheduler::Update::Grow) {
            self.last_update_was_refresh = false;
            if let Some(v) = &self.active {
                let region = v.region;
                let center = if r.follow {
                    camera.position.to_array()
                } else {
                    std::array::from_fn(|a| (v.bounds[0][a] + v.bounds[1][a]) * 0.5)
                };
                let next = startup::next_region(region, target_roots, center);
                self.expanding = true;
                self.growth_pending = false;
                self.shifting = true;
                self.shift_timing = ShiftTiming {
                    start: Some(Instant::now()),
                    ..Default::default()
                };
                self.worker.stage.store(0, Ordering::Relaxed);
                self.status = format!(
                    "Expanding exact region: {} → {} m",
                    v.resident_radius(),
                    next.dims()[0] as f64 * stream::root_size_m() * 0.5
                );
                self.refresh_world = None;
                self.worker.send(Job::Shift(
                    self.epoch,
                    self.source_world.as_ref().unwrap_or(world).clone(),
                    next,
                    self.effective_budget_mib * 1048576,
                ));
            }
        }
        if !self.shifting && !self.restoring_raster && self.published_epoch == self.epoch {
            if let Some(v) = &mut self.active {
                if let Some(cache) = &mut v.root_distance {
                    cache.advance(&gpu.device, &gpu.queue, v.n, v.ring, camera);
                }
            }
        }
        if let Some(v) = &self.active {
            let eye = camera.position.to_array();
            self.camera_margin_m = (0..3)
                .map(|a| (eye[a] - v.bounds[0][a]).min(v.bounds[1][a] - eye[a]))
                .fold(f64::INFINITY, f64::min);
            if r.follow && self.published_epoch == self.epoch {
                self.min_camera_margin_m = Some(
                    self.min_camera_margin_m
                        .map_or(self.camera_margin_m, |m| m.min(self.camera_margin_m)),
                );
                self.stream_active_frames += 1;
                self.stream_outside_frames += u64::from(self.camera_margin_m < voxel_size());
            }
            if update == Some(scheduler::Update::Follow) {
                if let Some(next) = following_target {
                    self.last_update_was_refresh = false;
                    self.shift_timing = ShiftTiming {
                        start: Some(Instant::now()),
                        ..Default::default()
                    };
                    self.shifting = true;
                    self.status = "Building leading strips".into();
                    self.refresh_world = None;
                    self.worker.send(Job::Shift(
                        self.epoch,
                        self.source_world.as_ref().unwrap_or(world).clone(),
                        next,
                        self.effective_budget_mib * 1048576,
                    ));
                } else {
                    self.status = "Ready · following player".into();
                }
            }
        }
    }

    pub fn ready(&self) -> bool {
        self.active.is_some() && self.published_epoch == self.epoch && self.error.is_none()
    }
    /// Water advances only against the exact static source already published.
    /// A local edit keeps the previous revision visible until all replacement
    /// roots publish; camera-only streaming does not change source identity.
    pub(crate) fn ready_for_water_source(&self, world: &World) -> bool {
        self.ready()
            && !self.restoring_raster
            && self.refresh_world.is_none()
            && self
                .active
                .as_ref()
                .is_some_and(|a| a.revision == world.revision())
            && self.source_world.as_ref().is_some_and(|source| {
                source.shares_source_lineage(world) && source.revision() == world.revision()
            })
    }
    /// Full requested-region gate for captures and locked benchmarks.
    /// Normal interactive readiness still permits progressive startup.
    pub(crate) fn ready_for_capture(&self) -> bool {
        self.ready()
            && !self.shifting
            && !self.expanding
            && !self.growth_pending
            && self.request.is_some_and(|r| {
                self.active
                    .as_ref()
                    .is_some_and(|volume| volume.region.dims()[0] == r.radius_roots() as usize * 2)
            })
            && (!root_distance::settle_capture()
                || self
                    .active
                    .as_ref()
                    .and_then(|v| v.root_distance.as_ref())
                    .is_none_or(|cache| {
                        let (ready, total) = cache.progress();
                        ready == total
                    }))
    }
    /// Whether a complete native voxel remains resident through the current
    /// ownership handoff. Callers still validate hits against the live world.
    pub fn owns_native_edit_cell(&self, cell: frontier_world::Cell) -> bool {
        let lo = [cell.x, cell.y, cell.z];
        let Some(hi) = cell
            .x
            .checked_add(1)
            .zip(cell.y.checked_add(1))
            .zip(cell.z.checked_add(1))
            .map(|((x, y), z)| [x, y, z])
        else {
            return false;
        };
        self.owns_native_edit_bounds(lo, hi)
    }

    pub(crate) fn owns_native_edit_bounds(&self, lo: [i64; 3], hi: [i64; 3]) -> bool {
        if !self.ready() || self.restoring_raster {
            return false;
        }
        let Some(active) = &self.active else {
            return false;
        };
        if active.sparse_roots {
            return false;
        }
        let bounds = [
            lo.map(|x| x as f64 * voxel_size()),
            hi.map(|x| x as f64 * voxel_size()),
        ];
        (0..3).all(|axis| bounds[1][axis] > bounds[0][axis])
            && active.ownership.contains(bounds)
            && self
                .shift
                .as_ref()
                .is_none_or(|shift| shift.change.retiring.contains(bounds))
    }
    pub(crate) fn full_bounds(&self) -> Option<[[f64; 3]; 2]> {
        self.active.as_ref().map(|a| a.bounds)
    }
    pub fn bounds_for(&self, camera: Camera) -> Option<[[f64; 3]; 2]> {
        let a = self.active.as_ref()?;
        let _ = camera;
        Some([
            a.bounds[0].map(|v| v + voxel_size()),
            a.bounds[1].map(|v| v - voxel_size()),
        ])
    }
    /// An unmodified volume containing only air owns no visible terrain or
    /// plants. Publishing a new air window must not cancel far refinement.
    pub(crate) fn empty_unedited_volume(&self) -> bool {
        self.active.as_ref().is_some_and(|a| {
            a.occupied.is_none() && !a.may_have_local_edits && !a.has_dynamic_water()
        })
    }
    pub(crate) fn desired_owner(&self) -> Option<std::sync::Arc<ownership::Ownership>> {
        if self.restoring_raster || self.empty_unedited_volume() {
            None
        } else if let Some(s) = &self.shift {
            Some(s.change.retiring.clone())
        } else {
            self.active.as_ref().map(|a| a.ownership.clone())
        }
    }

    pub(crate) fn retirement(&self) -> Option<ownership::Retirement> {
        if self.empty_unedited_volume() {
            return None;
        }
        Some(ownership::Retirement {
            old: self.active.as_ref()?.bounds,
            kept: self.shift.as_ref()?.change.retiring.bounds,
        })
    }
    pub(crate) fn restore_owner_id(&self) -> Option<u64> {
        if self.restoring_raster || self.request.is_none() || self.empty_unedited_volume() {
            Some(0)
        } else {
            self.shift.as_ref().map(|s| s.change.retiring.id)
        }
    }
    pub fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        far: &'a wgpu::BindGroup,
        camera: Camera,
        width: u32,
        height: u32,
        submerged: bool,
    ) {
        if let Some(a) = &self.active {
            let bounds = if !a.has_dynamic_water()
                && !submerged
                && !a.diagnostics.enabled
                && capture_layer() == CaptureLayer::Scene
            {
                let Some(occupied) = a.occupied else { return };
                occupied.bounds()
            } else {
                a.bounds
            };
            let Some([x, y, w, h]) = ray_scissor(camera, bounds, width, height) else {
                return;
            };
            pass.set_bind_group(0, &a.bind, &[]);
            pass.set_bind_group(1, far, &[]);
            a.beam.bind(pass);
            let outer = [x, y, w, h];
            if let (Some(water), Some(dry)) = (a.water_bounds, &a.dry_pipeline) {
                if !submerged
                    && !a.has_dynamic_water()
                    && !a.diagnostics.enabled
                    && capture_layer() == CaptureLayer::Scene
                {
                    // AO reads adjacent native cells. Include their reach in
                    // the wet footprint even when the primary hit is on land.
                    let [lo, hi] = water.bounds();
                    let pad = 3. * voxel_size();
                    let wet = ray_scissor(
                        camera,
                        [lo.map(|v| v - pad), hi.map(|v| v + pad)],
                        width,
                        height,
                    );
                    let (outer_parts, wet_part) = partition_scissor(outer, wet);
                    let [dry_total, wet_total] = self.split_pixels.get();
                    let wet_area = wet_part.map_or(0, |[_, _, w, h]| u64::from(w) * u64::from(h));
                    let area = u64::from(w) * u64::from(h);
                    self.split_pixels.set([
                        dry_total.saturating_add(area - wet_area),
                        wet_total.saturating_add(wet_area),
                    ]);
                    pass.set_pipeline(dry);
                    for [dx, dy, dw, dh] in outer_parts {
                        if dw > 0 && dh > 0 {
                            pass.set_scissor_rect(dx, dy, dw, dh);
                            pass.draw(0..3, 0..1);
                        }
                    }
                    if let Some([wx, wy, ww, wh]) = wet_part {
                        pass.set_pipeline(a.pipeline_for_view(false));
                        pass.set_scissor_rect(wx, wy, ww, wh);
                        pass.draw(0..3, 0..1);
                    }
                    pass.set_scissor_rect(0, 0, width, height);
                    return;
                }
            }
            pass.set_scissor_rect(x, y, w, h);
            pass.set_pipeline(a.pipeline_for_view(submerged));
            pass.draw(0..3, 0..1);
            pass.set_scissor_rect(0, 0, width, height);
        }
    }
    pub(crate) fn has_traversal_compute(&self) -> bool {
        self.active.as_ref().is_some_and(|a| a.height.is_some())
    }

    pub(crate) fn encode_traversal(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        width: u32,
        height: u32,
        primary: bool,
    ) {
        let Some(a) = &self.active else {
            return;
        };
        a.encode_traversal(gpu, encoder, [width, height], primary);
    }

    pub(crate) fn prepare_ground_light(&self, queue: &wgpu::Queue, data: &[u8]) {
        if let Some(active) = &self.active {
            queue.write_buffer(&active.resources.ground_light, 0, data);
        }
    }

    pub fn prepare(&self, gpu: &Gpu, camera: Camera, width: u32, height: u32) {
        let Some(a) = &self.active else {
            return;
        };
        let origin = DVec3::from_array(a.bounds[0]);
        let matrix = camera.matrix(origin, width as f32 / height as f32);
        let s = gpu.visual_settings.clone().unwrap_or_default();
        let sun = if s.beauty || gpu.regional_appearance {
            crate::atmosphere::sun_direction(s.sun_elevation, s.sun_azimuth)
        } else {
            glam::Vec3::new(-0.5, 0.8, -0.3).normalize()
        };
        let aspect = width as f32 / height as f32;
        let basis = ray_basis(camera, aspect);
        let eye = (camera.position - origin).as_vec3();
        // Some terrain recipes have no far shadow caster pass. An unrendered
        // depth texture would incorrectly shade the entire native window.
        let far_shadow = gpu
            .adaptive
            .as_ref()
            .filter(|a| s.terrain_shadows && !s.visibility_benchmark && a.shadows_active());
        a.beam.set_view(eye, basis);
        gpu.queue.write_buffer(
            &a.uniform,
            0,
            bytemuck::bytes_of(&Uniform {
                matrix: matrix.to_cols_array_2d(),
                inverse: matrix.inverse().to_cols_array_2d(),
                eye: eye
                    .extend(if gpu.camera_submerged.get() { 1. } else { 0. })
                    .to_array(),
                extent: [
                    a.region.dims()[0] as u32 * 16,
                    a.region.dims()[1] as u32 * 16,
                    a.region.dims()[2] as u32 * 16,
                    a.n,
                ],
                viewport: [width as f32, height as f32, a.samples as f32, 0.],
                sun: [sun.x, sun.y, sun.z, if s.beauty { 1. } else { 0. }],
                sky: [
                    s.overcast,
                    a.static_shadow_ceiling(),
                    // Standalone fixture, legacy terrain, or regional daylight.
                    if gpu.regional_appearance {
                        2.
                    } else if gpu.adaptive.is_some() {
                        1.
                    } else {
                        0.
                    },
                    if s.visibility_benchmark { 1. } else { 0. },
                ],
                far_matrix: far_shadow
                    .map_or(glam::Mat4::IDENTITY, |a| a.far_shadow_projection(&camera))
                    .to_cols_array_2d(),
                forward: basis[0],
                right: basis[1],
                up: basis[2],
                projection: basis[3],
                edit_min: a.edit_bounds()[0],
                edit_max: a.edit_bounds()[1],
                ring: [a.ring[0], a.ring[1], a.ring[2], a.traversal_flags()],
                cache: a
                    .root_distance
                    .as_ref()
                    .map_or([0; 4], |c| c.uniforms(aspect)),
                shadow: [
                    f32::from(u8::from(far_shadow.is_some())),
                    f32::from(u8::from(ray_profile().0)),
                    f32::from(u8::from(ray_profile().1)),
                    0.,
                ],
                // The root lattice uses sixteen native cells per side. Keep the
                // low cell address bits even for negative or very distant regions.
                paint_origin: [
                    (a.region.lo[0] as u32).wrapping_mul(stream::ROOT_EDGE_VOXELS),
                    (a.region.lo[1] as u32).wrapping_mul(stream::ROOT_EDGE_VOXELS),
                    (a.region.lo[2] as u32).wrapping_mul(stream::ROOT_EDGE_VOXELS),
                    crate::appearance_method::voxel_grain()
                        .unwrap_or(-1.)
                        .to_bits(),
                ],
                water: [
                    origin.x.rem_euclid(128.) as f32,
                    origin.z.rem_euclid(128.) as f32,
                    gpu.water_time.get(),
                    0.,
                ],
            }),
        );
    }
    pub(crate) fn shadow_bindings(&self) -> Option<(u64, Bindings)> {
        self.active.as_ref().map(|a| {
            let id = if a.dynamic_water_binding != 0 {
                a.dynamic_water_binding
            } else {
                a.ownership.id
            };
            (id, a.resources.clone())
        })
    }
    pub fn gpu_bytes(&self) -> usize {
        let allocated = |a: &Volume| {
            a.root_bytes()
                + a.resources.dynamic_water.size() as usize
                + a.capacities.iter().sum::<usize>()
                + a.root_distance
                    .as_ref()
                    .map_or(0, |_| root_distance::bytes(a.n))
                + a.scatter.as_ref().map_or(0, scatter::Scatter::gpu_bytes)
                + a.height.as_ref().map_or(0, traversal::Height::bytes)
                + a.beam.bytes()
                + a.diagnostics.bytes()
        };
        self.active.as_ref().map_or(0, allocated)
            + self.pending.as_ref().map_or(0, |p| allocated(&p.volume))
    }
    pub(crate) fn traversal_diagnostics(&self, gpu: &Gpu) -> Result<Option<String>, String> {
        match &self.active {
            Some(a) => a.diagnostics.read(&gpu.device, &gpu.queue),
            None => Ok(None),
        }
    }

    pub fn json(&self) -> String {
        let a = self.active.as_ref();
        let bounds = a.map(|a| a.bounds);
        let region = bounds.map_or("null".into(), |b| format!("{:?}", b));
        let (distance_tiles_ready, distance_tiles_total) = a
            .and_then(|v| v.root_distance.as_ref())
            .map_or((0, 0), root_distance::Cache::progress);
        let distance_counters = a.and_then(|v| v.root_distance.as_ref()).map_or(
            root_distance::Counters::default(),
            root_distance::Cache::counters,
        );
        let native_voxel_m = voxel_size();
        let mut json = format!("{{\"status\":{:?},\"stage\":{},\"requested_radius_m\":{},\"resident_radius_m\":{},\"world_bounds_m\":{},\"native_voxel_m\":{native_voxel_m},\"requested_build_budget_mib\":{},\"gpu_payload_bytes\":{},\"build_ms\":{},\"upload_cpu_ms\":{},\"mixed_chunks\":{},\"unique_blocks\":{},\"sampled_columns\":{},\"plant_cells\":{},\"palette_entries\":{},\"color_channel_bits\":10,\"refused\":{},\"fixed_snapshot\":{},\"stream_shifts\":{},\"stream_build_ms\":{},\"stream_sampled_columns\":{},\"stream_shared_leaves_last_shift\":{},\"camera_margin_m\":{},\"min_camera_margin_m\":{},\"stream_active_frames\":{},\"stream_outside_frames\":{},\"initial_snapshot_payload_bytes\":{},\"initial_snapshot_wide_leaf_payload_bytes\":{},\"initial_leaf_bytes\":{},\"gpu_leaf_bits\":{},\"lighting\":\"primary_visibility_lambert_without_GI\",\"includes_distant_and_boundary_raster_overhead\":true,\"interior_meshes_omitted\":true,\"supports_direct_voxel_shadows\":true,\"effective_build_budget_mib\":{},\"auto_memory\":{},\"gpu_root_distance_requested\":{},\"gpu_root_distance_tiles_ready\":{},\"gpu_root_distance_tiles_total\":{},\"sparse_roots_requested\":{},\"sparse_roots_active\":{},\"gpu_root_texture_bytes\":{}}}",
            self.status,self.stage,self.request.map_or(0., Request::radius_m),a.map_or(0.,Volume::resident_radius),region,
            self.request.filter(|r|r.budget!=usize::MAX).map_or(0,|r|r.budget/1048576),self.gpu_bytes(),a.map_or(0.,|a|a.build_ms),self.upload_ms,
            a.map_or(0,|a|a.stats[0]),a.map_or(0,|a|a.stats[1]),a.map_or(0,|a|a.stats[2]),a.map_or(0,|a|a.stats[3]),a.map_or(0,|a|a.stats[4]),self.refused,!self.request.is_some_and(|r|r.follow),self.shifts,self.strip_ms,self.strip_columns,self.strip_shared_leaves,self.camera_margin_m,self.min_camera_margin_m.map_or("null".into(),|v|v.to_string()),self.stream_active_frames,self.stream_outside_frames,a.map_or(0,|a|a.bytes),a.map_or(0,|a|a.wide_bytes),a.map_or(0,|a|a.leaf_bytes),a.map_or(0,|a|a.leaf_encoding.bits()),self.effective_budget_mib,self.request.is_some_and(|r|r.budget==usize::MAX),self.request.is_some_and(|r|r.gpu_distance),distance_tiles_ready,distance_tiles_total,self.request.is_some_and(|r|r.sparse_roots),a.is_some_and(|a|a.sparse_roots),a.map_or(0,Volume::root_bytes));
        json.pop();
        use std::fmt::Write;
        write!(
            json,
            ",\"requested_radius_voxels\":{},\"resident_radius_voxels\":{}",
            self.request.map_or(0, |r| r.radius_voxels),
            a.map_or(0, |v| v.region.dims()[0] as u32 / 2
                * stream::ROOT_EDGE_VOXELS)
        )
        .unwrap();
        write!(
            json,
            ",\"static_water_possible\":{},\"dry_pipeline_available\":{},\"static_water_pipeline_available\":{}",
            a.is_some_and(|v| v.water_bounds.is_some()),
            a.is_some_and(|v| v.dry_pipeline.is_some()),
            a.is_some_and(|v| v.static_water_pipeline.is_some())
        )
        .unwrap();
        write!(
            json,
            ",\"split_dry_wet_pixels\":{:?}",
            self.split_pixels.get()
        )
        .unwrap();
        write!(json, ",\"proven_sky_frames\":{}", self.sky_frames).unwrap();
        write!(json, ",\"published_world_revision\":{},\"content_update_pending\":{},\"edited_root_proofs_invalidated\":{}", a.map_or(0,|v|v.revision),self.refresh_world.is_some(),a.is_some_and(|v|v.edited_root_proofs)).unwrap();
        write!(
            json,
            ",\"startup_expanding\":{},\"startup_publications_radius_seconds\":{:?}",
            self.expanding || self.growth_pending,
            self.startup_publications
                .iter()
                .map(|&(r, t)| [r, t])
                .collect::<Vec<_>>()
        )
        .unwrap();
        write!(json, ",\"incremental_startup\":{},\"storage_radius_m\":{},\"arena_resizes\":{},\"arena_copy_bytes\":{}",
            self.incremental_startup, a.map_or(0., |v| v.n as f64 * stream::root_size_m() * 0.5), a.map_or(0, |v| v.arena_resizes), a.map_or(0, |v| v.arena_copy_bytes)).unwrap();
        write!(
            json,
            ",\"startup_generated_roots\":{}",
            self.startup_generated_roots
        )
        .unwrap();
        write!(json, ",\"cpu_root_refresh\":{:?},\"cpu_root_refresh_ms\":{},\"cpu_root_proofs_refreshed\":{},\"cpu_root_occupancy_bytes\":{}",
            root_distance::occupancy::requested().unwrap_or("off"), self.proof_refresh_ms, self.proofs_refreshed, self.root_occupancy_bytes).unwrap();
        let options = traversal::options();
        write!(json, ",\"specialize_format\":{}", options.specialize_format).unwrap();
        write!(
            json,
            ",\"native_column_cache\":{},\"fast_leaf_hash\":{}",
            column_cache::json(),
            prepare_profile::fast_hash()
        )
        .unwrap();
        write!(
            json,
            ",\"shadow_coarse_after_m\":{}",
            options.shadow_coarse_after_m
        )
        .unwrap();
        write!(
            json,
            r#","cpu_profile_enabled":{},"stack_block_bounds":{},"column_extrema":{}"#,
            prepare_profile::enabled(),
            prepare_profile::stack_blocks(),
            prepare_profile::column_extrema()
        )
        .unwrap();
        if prepare_profile::enabled() {
            let initial = a
                .and_then(|v| v.cpu_profile.as_ref())
                .map_or("null".into(), prepare_profile::Metrics::json);
            let samples: Vec<_> = self
                .prepare_samples
                .iter()
                .map(|(wall, profile)| {
                    format!(
                        r#"{{"prepare_wall_ms":{wall},"profile":{}}}"#,
                        profile.json()
                    )
                })
                .collect();
            write!(
                json,
                r#","initial_cpu_prepare_profile":{},"stream_cpu_prepare_samples":[{}]"#,
                initial,
                samples.join(",")
            )
            .unwrap();
        }
        write!(json,
            ",\"traversal_instrumented\":{},\"local_exit_repair\":{},\"node_cache\":{},\"node_cache_mode\":{},\"height_tile_roots\":{},\"height_mips\":{},\"height_probe_level\":{},\"height_gate\":{},\"height_levels\":{},\"height_bounds_bytes\":{},\"beam_prefix\":{},\"beam_prefix_bytes\":{}",
            options.diagnostics,
            options.local_exit_repair,
            options.cache != 0,
            options.cache,
            if options.height_enabled() { options.tile() } else { 0 },
            options.mips_enabled(),
            options.height_probe.map_or("null".into(), |level| level.to_string()),
            options.height_gate,
            a.and_then(|a| a.height.as_ref()).map_or(0, traversal::Height::levels),
            a.and_then(|a| a.height.as_ref()).map_or(0, traversal::Height::bytes),
            options.beam,
            a.map_or(0, |a| a.beam.bytes()),
        ).unwrap();
        write!(json,
            ",\"gpu_root_distance_schedule\":{:?},\"gpu_root_distance_cpu_fallback\":{},\"gpu_root_distance_tiles_retained\":{},\"gpu_root_distance_tiles_invalidated\":{},\"gpu_root_distance_tiles_submitted\":{},\"gpu_root_distance_ray_ranked_tiles\":{},\"gpu_root_distance_rank_cpu_ns\":{},\"gpu_root_distance_feedback_dispatches\":{},\"gpu_root_distance_feedback_reads\":{},\"gpu_root_distance_feedback_stale\":{},\"gpu_root_distance_feedback_prioritized_tiles\":{},\"gpu_root_distance_feedback_cpu_ns\":{}",
            root_distance::Mode::requested().label(),
            a.is_some_and(|v| v.cpu_root_fallback),
            distance_counters.retained,
            distance_counters.invalidated,
            distance_counters.submitted,
            distance_counters.ray_ranked,
            distance_counters.rank_cpu_ns,
            distance_counters.feedback_dispatches,
            distance_counters.feedback_reads,
            distance_counters.feedback_stale,
            distance_counters.feedback_prioritized,
            distance_counters.feedback_cpu_ns,
        ).unwrap();
        write!(json,
            ",\"gpu_root_distance_direct\":{},\"gpu_root_distance_repair\":{},\"gpu_root_distance_direct_publications\":{},\"gpu_root_distance_settle_capture\":{}",
            a.and_then(|v| v.root_distance.as_ref()).is_some_and(root_distance::Cache::is_direct),
            a.and_then(|v| v.root_distance.as_ref()).is_some_and(root_distance::Cache::repairs_roots),
            distance_counters.direct_publications,
            root_distance::settle_capture(),
        ).unwrap();
        let fixed_override = self
            .request
            .filter(|r| !r.follow)
            .and_then(|_| fixed_center().ok().flatten());
        write!(
            json,
            ",\"fixed_center_override_m\":{}",
            fixed_override.map_or("null".into(), |c| format!("{c:?}")),
        )
        .unwrap();
        write!(json, r#","stream_upload":{}"#, self.stream_upload.json()).unwrap();
        write!(
            json,
            r#","scatter_gpu_staging_bytes":{},"scatter_cpu_pack_bytes":{}"#,
            a.and_then(|v| v.scatter.as_ref())
                .map_or(0, scatter::Scatter::gpu_bytes),
            a.and_then(|v| v.scatter.as_ref())
                .map_or(0, scatter::Scatter::cpu_bytes),
        )
        .unwrap();
        write!(
            json,
            ",\"stream_lead_s\":{},\"stream_stage_ms\":{:?}}}",
            stream_lead_seconds(),
            self.shift_samples
        )
        .unwrap();
        json
    }
}

#[derive(Clone)]
pub(crate) struct Bindings {
    pub uniform: wgpu::Buffer,
    pub chunks: wgpu::TextureView,
    pub distance: wgpu::TextureView,
    pub height: wgpu::TextureView,
    pub diagnostics: wgpu::Buffer,
    pub blocks: wgpu::Buffer,
    pub voxels: wgpu::Buffer,
    pub palette: wgpu::Buffer,
    pub dynamic_water: wgpu::Buffer,
    pub ground_light: wgpu::Buffer,
}
pub(crate) fn binding_layout(first: u32) -> Vec<wgpu::BindGroupLayoutEntry> {
    (0..10)
        .map(|i| wgpu::BindGroupLayoutEntry {
            binding: first + i,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: match i {
                0 | 9 => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                1 | 5 => wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                },
                6 => wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                7 => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                _ => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            },
            count: None,
        })
        .collect()
}
fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("NAADF exact hierarchy"),
        entries: &binding_layout(0),
    })
}
impl Bindings {
    pub fn entries(&self, first: u32) -> Vec<wgpu::BindGroupEntry<'_>> {
        vec![
            wgpu::BindGroupEntry {
                binding: first,
                resource: self.uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: first + 1,
                resource: wgpu::BindingResource::TextureView(&self.chunks),
            },
            wgpu::BindGroupEntry {
                binding: first + 2,
                resource: self.blocks.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: first + 3,
                resource: self.voxels.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: first + 4,
                resource: self.palette.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: first + 5,
                resource: wgpu::BindingResource::TextureView(&self.distance),
            },
            wgpu::BindGroupEntry {
                binding: first + 6,
                resource: wgpu::BindingResource::TextureView(&self.height),
            },
            wgpu::BindGroupEntry {
                binding: first + 7,
                resource: self.diagnostics.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: first + 8,
                resource: self.dynamic_water.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: first + 9,
                resource: self.ground_light.as_entire_binding(),
            },
        ]
    }
    pub fn empty(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("inactive NAADF"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R32Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let buffer = |bytes: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("inactive NAADF words"),
                contents: bytes,
                usage,
            })
        };
        Self {
            uniform: buffer(
                bytemuck::bytes_of(&Uniform::zeroed()),
                wgpu::BufferUsages::UNIFORM,
            ),
            chunks: texture.create_view(&Default::default()),
            distance: root_distance::empty(device),
            height: traversal::empty_height(device),
            diagnostics: diagnostics::Diagnostics::new(device, false).buffer,
            blocks: buffer(&[0; 4], wgpu::BufferUsages::STORAGE),
            voxels: buffer(&[0; 4], wgpu::BufferUsages::STORAGE),
            palette: buffer(&[0; 16], wgpu::BufferUsages::STORAGE),
            dynamic_water: dynamic_water::empty(device),
            ground_light: crate::terrain_light::buffer(device),
        }
    }
}

pub(crate) fn mesh_traversal_constants() -> std::collections::HashMap<String, f64> {
    traversal::options().mesh_constants()
}

fn make_pipelines(
    gpu: &Gpu,
    layout: &wgpu::BindGroupLayout,
    beam_layout: &wgpu::BindGroupLayout,
    has_static_water: bool,
    options: traversal::Options,
    format: [u32; 3],
) -> (
    wgpu::RenderPipeline,
    Option<wgpu::RenderPipeline>,
    Option<wgpu::RenderPipeline>,
) {
    let shader = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("NAADF hierarchical exact rays"),
            source: wgpu::ShaderSource::Wgsl(crate::voxel_shader(SHADER)),
        });
    let pl = gpu
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("NAADF primary visibility"),
            bind_group_layouts: &[
                &layout,
                &crate::adaptive_gpu::adaptive_shadow::sampling_layout(&gpu.device),
                beam_layout,
            ],
            push_constant_ranges: &[],
        });
    let mut constants = options.mesh_constants();
    if options.specialize_format {
        constants.insert("traversal_leaf_format".into(), format[0].into());
        constants.insert("traversal_root_format".into(), format[1].into());
        constants.insert("traversal_cpu_roots".into(), format[2].into());
    }
    constants.insert(
        "traversal_diagnostic".into(),
        u8::from(capture_layer() == CaptureLayer::Steps).into(),
    );
    constants.insert("traversal_beam".into(), u8::from(options.beam).into());
    let mut make = |dry: bool, static_water: bool| {
        constants.insert(
            "traversal_static_water".into(),
            u8::from(static_water).into(),
        );
        constants.insert("traversal_dry".into(), u8::from(dry).into());
        gpu.device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("NAADF exact native voxel pass"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &constants,
                        ..Default::default()
                    },
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.scene_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Greater,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: gpu.sample_count,
                    ..Default::default()
                },
                multiview: None,
                cache: None,
            })
    };
    let full = make(false, false);
    let dry = std::env::var_os("FRONTIER_NAADF_DRY_PIPELINE")
        .is_none_or(|v| v != "0")
        .then(|| make(true, false));
    let wet = (has_static_water
        && std::env::var_os("FRONTIER_NAADF_STATIC_WATER_PIPELINE").is_none_or(|v| v != "0"))
    .then(|| make(false, true));
    (full, dry, wet)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureLayer {
    Scene,
    Steps,
    Raster,
    Voxels,
}
pub(crate) fn capture_layer() -> CaptureLayer {
    static LAYER: std::sync::OnceLock<CaptureLayer> = std::sync::OnceLock::new();
    *LAYER.get_or_init(
        || match std::env::var("FRONTIER_NAADF_CAPTURE_LAYER").as_deref() {
            Ok("steps") => CaptureLayer::Steps,
            Ok("raster") => CaptureLayer::Raster,
            Ok("voxels") => CaptureLayer::Voxels,
            _ => CaptureLayer::Scene,
        },
    )
}

pub(crate) fn ray_profile() -> (bool, bool) {
    static PROFILE: std::sync::OnceLock<(bool, bool)> = std::sync::OnceLock::new();
    *PROFILE.get_or_init(
        || match std::env::var("FRONTIER_NAADF_RAY_PROFILE").as_deref() {
            Ok("primary") => (true, true),
            Ok("voxel-shadows") => (false, true),
            Ok("mesh-shadows") => (true, false),
            _ => (false, false),
        },
    )
}

// Return disjoint dry rectangles and their wet complement inside the draw.
fn partition_scissor(outer: [u32; 4], wet: Option<[u32; 4]>) -> ([[u32; 4]; 4], Option<[u32; 4]>) {
    let [x, y, w, h] = outer;
    let Some([a, b, c, d]) = wet else {
        return ([outer, [0; 4], [0; 4], [0; 4]], None);
    };
    let (left, top, right, bottom) = (a.max(x), b.max(y), (a + c).min(x + w), (b + d).min(y + h));
    if right <= left || bottom <= top {
        return ([outer, [0; 4], [0; 4], [0; 4]], None);
    }
    (
        [
            [x, y, w, top - y],
            [x, bottom, w, y + h - bottom],
            [x, top, left - x, bottom - top],
            [right, top, x + w - right, bottom - top],
        ],
        Some([left, top, right - left, bottom - top]),
    )
}

// Project the vertices of the convex box clipped in front of the eye. Rays
// start at 2.5 cm view depth, so clipping at 1 cm leaves numerical slack.
fn ray_scissor(camera: Camera, bounds: [[f64; 3]; 2], w: u32, h: u32) -> Option<[u32; 4]> {
    const CLIP_W: f64 = 0.01;
    let eye = camera.position;
    if (0..3).all(|a| bounds[0][a] <= eye[a] && eye[a] <= bounds[1][a]) {
        return Some([0, 0, w, h]);
    }
    let origin = DVec3::from_array(bounds[0]);
    let matrix = camera.matrix(origin, w as f32 / h as f32).as_dmat4();
    let size = DVec3::from_array(bounds[1]) - origin;
    let corners: [glam::DVec4; 8] = std::array::from_fn(|i| {
        matrix
            * glam::DVec4::new(
                if i & 1 == 0 { 0. } else { size.x },
                if i & 2 == 0 { 0. } else { size.y },
                if i & 4 == 0 { 0. } else { size.z },
                1.,
            )
    });
    if corners.iter().any(|p| !p.is_finite()) {
        return Some([0, 0, w, h]);
    }
    let mut lo = glam::Vec2::splat(f32::INFINITY);
    let mut hi = -lo;
    let mut project = |c: glam::DVec4| {
        let p = glam::DVec2::new(c.x, -c.y) / c.w;
        lo = lo.min(p.as_vec2());
        hi = hi.max(p.as_vec2());
    };
    for (i, &a) in corners.iter().enumerate() {
        if a.w >= CLIP_W {
            project(a);
        }
        for axis in [1, 2, 4] {
            if i & axis != 0 {
                continue;
            }
            let b = corners[i | axis];
            if (a.w < CLIP_W) != (b.w < CLIP_W) {
                project(a.lerp(b, (CLIP_W - a.w) / (b.w - a.w)));
            }
        }
    }
    if lo.x == f32::INFINITY {
        return None;
    }
    let viewport = glam::Vec2::new(w as f32, h as f32);
    let xy = ((lo * 0.5 + 0.5) * viewport - 2.)
        .floor()
        .clamp(glam::Vec2::ZERO, viewport);
    let end = ((hi * 0.5 + 0.5) * viewport + 2.)
        .ceil()
        .clamp(glam::Vec2::ZERO, viewport);
    let span = end - xy;
    (span.x > 0. && span.y > 0.).then_some([xy.x as u32, xy.y as u32, span.x as u32, span.y as u32])
}

#[test]
fn wet_and_dry_draw_rectangles_partition_each_pixel_once() {
    let outer = [2, 3, 9, 7];
    for wet in [
        None,
        Some([0, 0, 2, 3]),
        Some([0, 0, 6, 6]),
        Some([4, 5, 2, 3]),
        Some([2, 3, 9, 7]),
        Some([0, 0, 20, 20]),
    ] {
        let (dry, water) = partition_scissor(outer, wet);
        let inside = |p: [u32; 4], x: u32, y: u32| {
            x >= p[0] && x < p[0] + p[2] && y >= p[1] && y < p[1] + p[3]
        };
        for y in 0..12 {
            for x in 0..14 {
                let wet_pixel = water.is_some_and(|p| inside(p, x, y));
                let dry_count = dry.iter().filter(|&&p| inside(p, x, y)).count();
                assert_eq!(
                    usize::from(wet_pixel) + dry_count,
                    usize::from(inside(outer, x, y))
                );
                assert_eq!(
                    wet_pixel,
                    inside(outer, x, y) && wet.is_some_and(|p| inside(p, x, y))
                );
            }
        }
    }
}

#[test]
fn scissor_covers_visible_box_and_uses_full_view_when_inside() {
    let camera = Camera {
        roll: 0.,
        position: DVec3::ZERO,
        yaw: 0.,
        pitch: 0.,
        fov_y: 60.,
        far: 300.,
    };
    assert_eq!(
        ray_scissor(camera, [[-1.; 3], [1.; 3]], 1920, 1080),
        Some([0, 0, 1920, 1080])
    );
    assert_eq!(
        ray_scissor(camera, [[90., -1., 10.], [92., 1., 12.]], 1920, 1080),
        None
    );
    assert_eq!(
        ray_scissor(camera, [[-4., -2., -6.], [2., 3., -1.]], 1920, 1080),
        None
    );
    let strip = ray_scissor(camera, [[-32., -8., -32.], [32., -1., 32.]], 1920, 1080).unwrap();
    assert!(strip[1] > 540 && strip[1] + strip[3] == 1080);
    let [x, y, w, h] = ray_scissor(camera, [[-1., -1., 10.], [1., 1., 12.]], 1920, 1080).unwrap();
    assert!(x < 960 && x + w > 960 && y < 540 && y + h > 540);
    assert!(w < 250 && h < 250);
}

fn ray_basis(camera: Camera, aspect: f32) -> [[f32; 4]; 4] {
    let forward = camera.direction().normalize();
    let right = forward.cross(camera.view_up()).normalize();
    let up = right.cross(forward);
    let p = camera.projection(aspect);
    [
        forward.extend(0.).to_array(),
        right.extend(1. / p.x_axis.x).to_array(),
        up.extend(1. / p.y_axis.y).to_array(),
        [0.025, camera.far, -p.z_axis.z, p.w_axis.z],
    ]
}
