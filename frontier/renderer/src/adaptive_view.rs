//! Presentation keeps previous complete ownership until a current replacement
//! is entirely resident. CPU readiness alone never switches the displayed cover.
use crate::adaptive_geometry::{deferred_cover, GeometryTile};
use crate::{
    adaptive_geometry::TileKey,
    adaptive_gpu::{AdaptiveGpu, DrawStats},
    adaptive_stream::{SelectionFrame, StreamSettings, Worker},
    Camera, Gpu,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Instant,
};

#[derive(Default)]
struct CapacityRefusals(HashMap<TileKey, (u64, u64)>);
impl CapacityRefusals {
    fn blocked(&self, key: &TileKey, resources: (u64, u64)) -> bool {
        self.0.get(key) == Some(&resources)
    }
    fn park<T>(&mut self, key: TileKey, resources: (u64, u64), ready: &mut Option<T>) {
        self.0.insert(key, resources);
        // Release the completed-product slot so other keys can be polled and
        // promoted. This key is eligible after residency or budget changes.
        ready.take();
    }
}

struct CoverState {
    worker: deferred_cover::Worker,
    tiles: HashMap<TileKey, Arc<GeometryTile>>,
    tickets: HashMap<TileKey, deferred_cover::Ticket>,
    refused: HashSet<TileKey>,
    capacity_refused: CapacityRefusals,
    ready: Option<GeometryTile>,
    owner: u64,
    stale_results: u64,
    promotions: u64,
    cpu_ms: f64,
    upload_ms: f64,
    upload_bytes: u64,
    allocation_refusals: u64,
    appearance_debt: usize,
    first_geometry_ms: Option<f64>,
    first_appearance_ms: Option<f64>,
}
const PREPARE_SAMPLE_CAP: usize = 4096;
const PREPARE_NAMES: [&str; 7] = [
    "shadows",
    "resident_accounting",
    "order_excluding_accounting",
    "occluder_build",
    "visibility_error_uniforms",
    "upload_enqueue",
    "bundle_prepare",
];

pub struct AdaptiveView {
    desired_naadf_owner: u64,
    pub published_naadf_owner: u64,
    pub naadf_owned_tiles: usize,
    pub watched_tile_cells: Option<u32>,
    watch_xz: Option<[i64; 2]>,
    upload_budget: u64,
    background_paused: bool,
    stream_settings: StreamSettings,
    owner_surface_bounds: Vec<(TileKey, [[f64; 3]; 2])>,
    worker: Worker,
    pending: Option<SelectionFrame>,
    deferred: Option<SelectionFrame>,
    epoch: u64,
    published_pose: Option<(Camera, u32, u32)>,
    candidate_preparation: Option<crate::adaptive_stream::PreparationCycle>,
    pub owners: HashSet<TileKey>,
    owner_guard: crate::adaptive_stream::PartitionGuard,
    retain_keys: HashSet<TileKey>,
    retain_dirty: bool,
    pub stats: DrawStats,
    pub quality_ready: bool,
    geometry_quality_ready: bool,
    pub refinement_ready: bool,
    cpu_quality_ready: bool,
    cpu_refinement_ready: bool,
    cpu_max_visible_error_px: Option<f64>,
    pub coverage_ready: bool,
    pub pending_count: usize,
    pub unresolved: bool,
    pub native_clusters: usize,
    pub upload_bytes: u64,
    upload_cpu_ms: f64,
    publication_cpu_ms: f64,
    gpu_prepare_cpu_ms: f64,
    gpu_prepare_stage_ms: [f64; 4],
    prepare_detail_ms: [f64; 7],
    prepare_samples: [VecDeque<f64>; 7],
    prepare_order_reuses: u64,
    cover: Option<CoverState>,
    pub max_upload_frame_bytes: u64,
    pub published_epoch: u64,
    pub pixel_error: f64,
    coarse_y_pitch_divisor: u32,
    visible_vertical_bound: bool,
    topology_supported: bool,
    native_bins: bool,
    worker_count: usize,
    refinements_per_transaction: usize,
    direct_native_urgent: bool,
    build_memory: crate::build_memory::BuildMemoryStats,
    planning: crate::adaptive_stream::PlanningStats,
    pub cache_bytes: usize,
    pub cache_hits: u64,
    pub builds: u64,
    total_build_ms: f64,
    max_build_ms: f64,
    generated_columns: u64,
    source_cache_hits: u64,
    source_cache_bytes: usize,
    latest_cache: crate::adaptive_geometry::CacheStats,
    surface_source_revision: u64,
    displayed_world_revision: u64,
    preparation_started: Instant,
    first_coverage_ms: Option<f64>,
    first_quality_ms: Option<f64>,
    first_refinement_ms: Option<f64>,
    empirical_error_m: f64,
    uncertified_clusters: usize,
    selected_bytes: usize,
    sampled_columns: usize,
    cpu_cache_budget_bytes: usize,
    pending_arc_bytes: usize,
    deferred_arc_bytes: usize,
    gpu_pending_clusters: usize,
    gpu_resource_limited: bool,
    /// Configured admission budget; physical pool allocation is reported separately.
    gpu_pool_budget_bytes: u64,
    max_visible_error_px: Option<f64>,
    visible_unresolved_count: usize,
    visible_uncertified_count: usize,
    global_bound_clusters: usize,
    local_certified_clusters: usize,
    affine_bound_clusters: usize,
    frame_attachment_bytes: u64,
}
impl AdaptiveView {
    /// Suppress new builds and GPU uploads under measured memory pressure.
    /// Existing owners stay drawable; this does not reclaim driver allocations.
    pub(crate) fn set_naadf_owner(
        &mut self,
        owner: Option<std::sync::Arc<crate::naadf::ownership::Ownership>>,
    ) {
        let id = owner.as_ref().map_or(0, |o| o.id);
        if id == self.desired_naadf_owner {
            return;
        }
        self.desired_naadf_owner = id;
        // A new owner starts a new coverage-preparation cycle. Snapshot build
        // time must not consume the raster handoff's independent settle window.
        self.preparation_started = Instant::now();
        self.first_coverage_ms = None;
        self.first_quality_ms = None;
        self.first_refinement_ms = None;
        if let Some(cover) = &mut self.cover {
            for ticket in cover.tickets.values() {
                ticket.cancel();
            }
            cover.ready = None;
            cover.first_geometry_ms = None;
            cover.first_appearance_ms = None;
        }
        self.worker.set_naadf_owner(owner);
        self.pending = None;
        self.deferred = None;
        self.candidate_preparation = None;
        self.quality_ready = false;
        self.refinement_ready = false;
        self.cpu_quality_ready = false;
        self.cpu_refinement_ready = false;
    }
    pub fn set_background_paused(&mut self, paused: bool) {
        self.background_paused = paused;
        self.worker.set_background_paused(paused);
    }
    pub fn set_upload_budget(&mut self, mib: usize) {
        self.upload_budget = mib.clamp(1, 64) as u64 * 1024 * 1024;
    }
    /// Changes scheduling and quality demand while retaining canonical caches,
    /// partially uploaded payloads and the complete currently displayed owners.
    pub fn reconfigure(&mut self, settings: StreamSettings) -> Result<(), String> {
        self.worker.reconfigure(settings)?;
        let quality_changed = self.stream_settings.pixel_error != settings.pixel_error
            || self.stream_settings.native_radius_m != settings.native_radius_m;
        self.stream_settings = settings;
        self.pixel_error = settings.pixel_error;
        self.cpu_cache_budget_bytes = settings.cpu_cache_bytes;
        self.worker_count = settings.worker_count;
        self.refinements_per_transaction = settings.refinements_per_transaction;
        self.direct_native_urgent = settings.direct_native_urgent;
        if quality_changed {
            self.quality_ready = false;
            self.refinement_ready = false;
            self.cpu_quality_ready = false;
            self.cpu_refinement_ready = false;
            self.candidate_preparation = None;
        }
        Ok(())
    }
    fn update_appearance(
        &mut self,
        adaptive: &mut AdaptiveGpu,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: &Camera,
        geometry_upload_idle: bool,
    ) -> Result<(), String> {
        let Some(cover) = &mut self.cover else {
            return Ok(());
        };
        let coherent =
            cover.owner == self.published_naadf_owner && cover.owner == self.desired_naadf_owner;
        // Cover has its own bounded worker/queue. Pending refinement must not
        // repeatedly cancel valid cosmetic work: on a walking camera geometry
        // may never drain completely. Require coherent coverage for admission,
        // and reserve upload bandwidth for geometry at promotion below.
        let cover_admitted = coherent && !self.background_paused && self.coverage_ready;
        cover
            .tiles
            .retain(|key, _| self.owners.contains(key) && adaptive.appearance_pending(key));
        cover.refused.retain(|key| cover.tiles.contains_key(key));
        cover
            .capacity_refused
            .0
            .retain(|key, _| cover.tiles.contains_key(key));
        for (key, ticket) in &cover.tickets {
            if !coherent || self.background_paused || !cover.tiles.contains_key(key) {
                ticket.cancel();
            }
        }
        if cover.ready.is_none() {
            if let Some(completion) = cover.worker.poll() {
                cover.tickets.remove(&completion.key);
                cover.cpu_ms += completion.cpu_ms;
                if !coherent
                    || completion.key.source_revision != self.surface_source_revision
                    || !self.owners.contains(&completion.key)
                    || !adaptive.appearance_pending(&completion.key)
                {
                    cover.stale_results += 1;
                } else {
                    match completion.result {
                        Ok(deferred_cover::Product::Tile(tile)) => cover.ready = Some(tile),
                        Ok(deferred_cover::Product::Patch(_)) => {
                            return Err("appearance worker returned unbound profiles".into())
                        }
                        Err(error) if error.contains("cancelled") => {}
                        Err(error) if error.contains("exceeds payload cap") => {
                            cover.refused.insert(completion.key);
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
        }
        if let Some(tile) = cover.ready.as_ref() {
            if !coherent
                || tile.key.source_revision != self.surface_source_revision
                || !self.owners.contains(&tile.key)
                || !adaptive.appearance_pending(&tile.key)
            {
                cover.stale_results += 1;
                cover.ready = None;
            } else if cover_admitted && geometry_upload_idle {
                let started = Instant::now();
                let mut result = adaptive.promote_appearance(device, queue, tile);
                if result
                    .as_ref()
                    .is_err_and(|e| crate::adaptive_gpu::resource_capacity_error(e))
                    && adaptive.evict_warm()
                {
                    result = adaptive.promote_appearance(device, queue, tile);
                }
                match result {
                    Ok(()) => {
                        cover.upload_bytes += tile.bytes as u64;
                        cover.promotions += 1;
                        cover.tiles.remove(&tile.key);
                        cover.ready = None;
                    }
                    Err(error) if crate::adaptive_gpu::resource_capacity_error(&error) => {
                        cover.allocation_refusals += 1;
                        cover.capacity_refused.park(
                            tile.key,
                            (adaptive.residency_revision, adaptive.resource_budget_mib()),
                            &mut cover.ready,
                        );
                    }
                    Err(error) => return Err(error),
                }
                cover.upload_ms += started.elapsed().as_secs_f64() * 1000.;
            }
        }
        // The single cosmetic worker cannot occupy a mandatory geometry slot.
        // Obsolete owner/source products still cancel; ordinary camera demand
        // retains useful work rather than starting over at every refinement.
        if cover_admitted && cover.ready.is_none() {
            if let Some((&key, tile)) = cover
                .tiles
                .iter()
                .filter(|(key, _)| {
                    !cover.tickets.contains_key(key)
                        && !cover.refused.contains(key)
                        && !cover.capacity_refused.blocked(
                            key,
                            (adaptive.residency_revision, adaptive.resource_budget_mib()),
                        )
                })
                .min_by(|(_, a), (_, b)| {
                    let distance = |tile: &GeometryTile| {
                        let center = glam::DVec3::new(
                            (tile.bounds[0][0] + tile.bounds[1][0]) * 0.5,
                            (tile.bounds[0][1] + tile.bounds[1][1]) * 0.5,
                            (tile.bounds[0][2] + tile.bounds[1][2]) * 0.5,
                        );
                        center.distance_squared(camera.position)
                    };
                    distance(a)
                        .total_cmp(&distance(b))
                        .then_with(|| a.key.cmp(&b.key))
                })
            {
                match cover.worker.submit_tile(tile.clone()) {
                    Ok(ticket) => {
                        cover.tickets.insert(key, ticket);
                    }
                    Err(()) if tile.bytes > cover.worker.max_payload_bytes() => {
                        cover.refused.insert(key);
                    }
                    Err(()) => {}
                }
            }
        }
        cover.appearance_debt = adaptive.appearance_debt(&self.owners);
        if cover.appearance_debt == 0 && self.coverage_ready && cover.first_appearance_ms.is_none()
        {
            cover.first_appearance_ms =
                Some(self.preparation_started.elapsed().as_secs_f64() * 1000.);
        }
        Ok(())
    }
    pub fn stream_settings(&self) -> StreamSettings {
        self.stream_settings
    }
    /// Call only when the governor's integer work allowance changes. Equality
    /// is checked here too, so unchanged throughput never restarts planning.
    pub fn set_throughput(&mut self, jobs: usize, refinements: usize) -> Result<(), String> {
        let settings = StreamSettings {
            jobs_per_update: jobs.clamp(1, 16),
            refinements_per_transaction: refinements.clamp(1, 32),
            ..self.stream_settings
        };
        if settings == self.stream_settings {
            return Ok(());
        }
        self.reconfigure(settings)
    }
    pub fn new(
        world: frontier_world::World,
        pixel_error: f64,
        cpu_cache_budget_bytes: usize,
        native_bins: bool,
        worker_count: usize,
        refinements_per_transaction: usize,
        direct_native_urgent: bool,
        build_budget_bytes: usize,
        disk_cache: Option<crate::adaptive_stream::DiskCacheConfig>,
    ) -> Result<Self, String> {
        let stream_settings = StreamSettings {
            pixel_error,
            cpu_cache_bytes: cpu_cache_budget_bytes,
            worker_count,
            refinements_per_transaction,
            direct_native_urgent,
            build_budget_bytes,
            ..Default::default()
        };
        let topology_supported = world.topology_base_cells().is_some();
        let coarse_y_pitch_divisor =
            crate::adaptive_geometry::distant_voxels::vertical_pitch_divisor(&world);
        let visible_vertical_bound = crate::adaptive_stream::visible_vertical_bound_for_alpine(
            crate::adaptive_stream::is_alpine(&world),
        );
        let revision = world.revision();
        let cover = if deferred_cover::enabled() {
            Some(CoverState {
                worker: deferred_cover::Worker::new(world.clone(), 16 * 1024 * 1024)?,
                tiles: HashMap::new(),
                tickets: HashMap::new(),
                refused: HashSet::new(),
                capacity_refused: CapacityRefusals::default(),
                ready: None,
                owner: 0,
                stale_results: 0,
                promotions: 0,
                cpu_ms: 0.,
                upload_ms: 0.,
                upload_bytes: 0,
                allocation_refusals: 0,
                appearance_debt: 0,
                first_geometry_ms: None,
                first_appearance_ms: None,
            })
        } else {
            None
        };
        Ok(Self {
            desired_naadf_owner: 0,
            published_naadf_owner: 0,
            naadf_owned_tiles: 0,
            watched_tile_cells: None,
            watch_xz: std::env::var("FRONTIER_TERRAIN_WATCH_XZ")
                .ok()
                .and_then(|s| {
                    let (x, z) = s.split_once(',')?;
                    Some([x.parse::<f64>().ok()?, z.parse::<f64>().ok()?])
                })
                .filter(|v| v.iter().all(|x| x.is_finite()))
                .map(|v| v.map(|x| (x / crate::VOXEL_SIZE).floor() as i64)),

            upload_budget: 2 * 1024 * 1024,
            background_paused: false,
            stream_settings,
            owner_surface_bounds: Vec::new(),
            worker: Worker::new_with_cache(world, stream_settings, disk_cache)?,
            pending: None,
            deferred: None,
            epoch: 0,
            published_pose: None,
            candidate_preparation: None,
            owners: HashSet::new(),
            owner_guard: Default::default(),
            retain_keys: HashSet::new(),
            retain_dirty: true,
            stats: DrawStats::default(),
            quality_ready: false,
            geometry_quality_ready: false,
            refinement_ready: false,
            cpu_quality_ready: false,
            cpu_refinement_ready: false,
            cpu_max_visible_error_px: None,
            coverage_ready: false,
            pending_count: 0,
            unresolved: false,
            native_clusters: 0,
            upload_bytes: 0,
            upload_cpu_ms: 0.,
            publication_cpu_ms: 0.,
            gpu_prepare_cpu_ms: 0.,
            gpu_prepare_stage_ms: [0.; 4],
            prepare_detail_ms: [0.; 7],
            prepare_samples: std::array::from_fn(|_| VecDeque::new()),
            prepare_order_reuses: 0,
            cover,
            max_upload_frame_bytes: 0,
            published_epoch: 0,
            pixel_error,
            coarse_y_pitch_divisor,
            visible_vertical_bound,
            topology_supported,
            native_bins,
            worker_count,
            direct_native_urgent,
            refinements_per_transaction,
            build_memory: crate::build_memory::BuildMemoryStats::default(),
            planning: Default::default(),
            cache_bytes: 0,
            cache_hits: 0,
            builds: 0,
            total_build_ms: 0.,
            max_build_ms: 0.,
            generated_columns: 0,
            source_cache_hits: 0,
            source_cache_bytes: 0,
            latest_cache: Default::default(),
            surface_source_revision: revision,
            displayed_world_revision: revision,
            preparation_started: Instant::now(),
            first_coverage_ms: None,
            first_quality_ms: None,
            first_refinement_ms: None,
            empirical_error_m: 0.,
            uncertified_clusters: 0,
            selected_bytes: 0,
            sampled_columns: 0,
            cpu_cache_budget_bytes,
            pending_arc_bytes: 0,
            deferred_arc_bytes: 0,
            gpu_pending_clusters: 0,
            gpu_resource_limited: false,
            gpu_pool_budget_bytes: 512 * 1024 * 1024,
            max_visible_error_px: None,
            visible_unresolved_count: 0,
            visible_uncertified_count: 0,
            global_bound_clusters: 0,
            local_certified_clusters: 0,
            affine_bound_clusters: 0,
            frame_attachment_bytes: 0,
        })
    }
    pub fn poll(
        &mut self,
        gpu: &mut Gpu,
        camera: Camera,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        if gpu.adaptive.is_none() {
            gpu.adaptive = Some(AdaptiveGpu::new(
                &gpu.device,
                gpu.scene_format,
                gpu.sample_count,
            ));
        }
        let color_bytes = if gpu.scene_format == wgpu::TextureFormat::Rgba32Float {
            16u64
        } else if gpu.scene_format == wgpu::TextureFormat::Rgba16Float {
            8u64
        } else {
            4u64
        };
        let samples = gpu.sample_count as u64;
        self.frame_attachment_bytes = width as u64
            * height as u64
            * (color_bytes
                + samples * 4
                + if samples > 1 {
                    samples * color_bytes
                } else {
                    0
                });
        let epoch = self.worker.request(camera, width, height);
        if epoch != self.epoch {
            self.epoch = epoch;
            // Canonical content and partial GPU uploads survive camera epochs.
            // Current coverage is revalidated before any ownership publication.
            self.quality_ready = false;
            self.refinement_ready = false;
            self.cpu_quality_ready = false;
            self.cpu_refinement_ready = false;
            self.cpu_max_visible_error_px = None;
            self.coverage_ready = false;
            self.unresolved = false;
        }
        if let Some(result) = self.worker.poll() {
            let frame = result?;
            if frame.epoch == epoch && frame.ownership_id == self.desired_naadf_owner {
                self.pending_count = frame.pending;
                self.cpu_quality_ready = frame.quality_ready;
                self.cpu_refinement_ready = frame.refinement_ready;
                self.cpu_max_visible_error_px = frame.max_visible_error_px;
                self.unresolved = frame.limit_reached;
                self.latest_cache = frame.cache;
                self.cache_bytes = frame.cache.resident_bytes;
                self.cache_hits = frame.cache.hits;
                self.builds = frame.cache.builds;
                self.total_build_ms = frame.cache.total_build_ms;
                self.max_build_ms = frame.cache.max_build_ms;
                self.generated_columns = frame.cache.generated_columns;
                self.build_memory = frame.build_memory;
                self.planning = frame.planning;
                self.source_cache_hits = frame.cache.source_cache_hits;
                self.source_cache_bytes = frame.cache.source_cache_bytes;
                if frame.coverage_ready {
                    let pending_covers = self.pending.as_ref().is_some_and(|pending| {
                        publication_keys(
                            pending,
                            self.surface_source_revision,
                            self.topology_supported,
                            &camera,
                            width,
                            height,
                        )
                        .is_some()
                    });
                    let incoming_covers = publication_keys(
                        &frame,
                        self.surface_source_revision,
                        self.topology_supported,
                        &camera,
                        width,
                        height,
                    )
                    .is_some();
                    self.retain_dirty = true;
                    queue_upload_target(
                        &mut self.pending,
                        &mut self.deferred,
                        frame,
                        !pending_covers && incoming_covers,
                    );
                } // never publish partial coverage
            }
        }
        let adaptive = gpu.adaptive.as_mut().unwrap();
        adaptive.set_resource_budget_mib(
            gpu.visual_settings
                .as_ref()
                .map_or(512, |settings| settings.terrain_gpu_mib as u64),
        );
        self.gpu_pool_budget_bytes = adaptive.resource_budget_mib().saturating_mul(1024 * 1024);
        adaptive.set_visual_lighting(
            &gpu.visual_settings.clone().unwrap_or_default(),
            gpu.regional_appearance,
            gpu.alpine_appearance,
            gpu.terrain_shadow_appearance,
            gpu.terrain_amplitude,
        );
        adaptive.set_voxel_filter(gpu.voxel_filter);
        adaptive.set_native_bins(self.native_bins);
        if self.retain_dirty {
            self.retain_keys.clear();
            self.retain_keys.extend(self.owners.iter().copied());
            if let Some(frame) = &self.pending {
                self.retain_keys.extend(frame.tiles.iter().map(|t| t.key));
            }
            adaptive.retain(&self.retain_keys, !self.topology_supported);
            self.retain_dirty = false;
        }
        let mut uploaded = 0;
        self.gpu_resource_limited = false;
        if let (false, Some(frame)) = (self.background_paused, &self.pending) {
            for tile in &frame.tiles {
                if adaptive.contains(&tile.key) {
                    continue;
                }
                let bytes = (tile.mesh.vertices.len() * std::mem::size_of::<crate::mesh::Vertex>()
                    + tile.mesh.indices.len() * 4
                    + tile.face_profiles.len() * 80
                    + tile.face_profile_indices.len() * 4
                    + tile.native_quads.len() * 32
                    + tile.proxy_triangles.len() * 64
                    + tile
                        .gpu_profiles
                        .as_ref()
                        .map_or(0, |profiles| profiles.payload_bytes()))
                    as u64;
                // One bounded cluster is the upload quantum. A cluster larger
                // than the frame budget is disclosed by max_upload_frame_bytes.
                if uploaded > 0 && uploaded + bytes > self.upload_budget {
                    break;
                }
                let upload_started = Instant::now();
                let mut upload_result = adaptive.upload(&gpu.device, &gpu.queue, tile);
                if upload_result
                    .as_ref()
                    .is_err_and(|e| crate::adaptive_gpu::resource_capacity_error(e))
                    && adaptive.evict_warm()
                {
                    upload_result = adaptive.upload(&gpu.device, &gpu.queue, tile);
                }
                self.upload_cpu_ms += upload_started.elapsed().as_secs_f64() * 1000.;
                if let Err(error) = upload_result {
                    if crate::adaptive_gpu::resource_capacity_error(&error) {
                        self.gpu_resource_limited = true;
                        self.unresolved = true;
                        break;
                    }
                    return Err(error);
                }
                uploaded += bytes;
                if uploaded >= self.upload_budget {
                    break;
                }
            }
            let publication_started = Instant::now();
            if frame
                .tiles
                .iter()
                .all(|t| adaptive.contains(&t.key) || t.is_empty())
            {
                if let Some(keys) = publication_keys(
                    frame,
                    self.surface_source_revision,
                    self.topology_supported,
                    &camera,
                    width,
                    height,
                ) {
                    self.published_naadf_owner = frame.ownership_id;
                    self.naadf_owned_tiles = frame.tiles.iter().filter(|t| t.naadf_owned).count();
                    // Owned tiles have no raster payload, including shadow payload.
                    let raster_keys = frame
                        .tiles
                        .iter()
                        .filter(|t| !t.is_empty())
                        .map(|t| t.key)
                        .collect();
                    adaptive.retain(&raster_keys, !self.topology_supported);
                    self.owner_guard = crate::adaptive_stream::PartitionGuard::new(
                        keys.iter().copied(),
                        self.topology_supported,
                    );
                    self.owner_surface_bounds = frame
                        .tiles
                        .iter()
                        .filter(|tile| !tile.naadf_owned)
                        .map(|tile| (tile.key, tile.conservative_surface_bounds))
                        .collect();
                    self.watched_tile_cells = self.watch_xz.and_then(|point| {
                        keys.iter()
                            .find(|k| {
                                (0..2).all(|i| {
                                    point[i] >= k.min[i]
                                        && point[i]
                                            < k.min[i].saturating_add(i64::from(k.width_cells))
                                })
                            })
                            .map(|k| k.width_cells)
                    });
                    if let Some(cover) = &mut self.cover {
                        cover.owner = frame.ownership_id;
                        cover.tiles = frame
                            .tiles
                            .iter()
                            .filter(|tile| tile.cover_input.is_some())
                            .map(|tile| (tile.key, tile.clone()))
                            .collect();
                        if cover.first_geometry_ms.is_none() {
                            cover.first_geometry_ms =
                                Some(self.preparation_started.elapsed().as_secs_f64() * 1000.);
                        }
                    }
                    self.owners = keys;
                    self.retain_dirty = true;
                    if let Some(transaction) = frame.publication_transaction {
                        self.worker.record_transaction_publication(transaction);
                    }
                    if self.first_coverage_ms.is_none() {
                        self.first_coverage_ms =
                            Some(self.preparation_started.elapsed().as_secs_f64() * 1000.);
                    }
                    self.empirical_error_m = frame
                        .tiles
                        .iter()
                        .map(|t| t.empirical_error_m)
                        .fold(0., f64::max);
                    self.uncertified_clusters = frame
                        .tiles
                        .iter()
                        .filter(|t| t.certified_error_m.is_none())
                        .count();
                    self.selected_bytes = frame.tiles.iter().map(|t| t.bytes).sum();
                    self.sampled_columns = frame.tiles.iter().map(|t| t.sampled_columns).sum();
                    self.native_clusters = frame.tiles.iter().filter(|t| t.exact).count();
                    self.coverage_ready = true;
                    self.quality_ready = frame.quality_ready;
                    self.max_visible_error_px = frame.max_visible_error_px;
                    self.visible_unresolved_count = frame.visible_unresolved_count;
                    self.visible_uncertified_count = frame.visible_uncertified_count;
                    self.global_bound_clusters = frame.global_bound_clusters;
                    self.local_certified_clusters = frame.local_certified_clusters;
                    self.candidate_preparation = frame.preparation;
                    self.published_epoch = epoch;
                    self.published_pose = Some((camera, width, height));
                }
                self.pending = None;
                self.retain_dirty = true;
            }
            self.publication_cpu_ms += publication_started.elapsed().as_secs_f64() * 1000.;
        }
        if self.pending.is_none() {
            self.pending = self.deferred.take();
            self.retain_dirty |= self.pending.is_some();
        }
        self.deferred_arc_bytes = self
            .deferred
            .as_ref()
            .map_or(0, |f| f.tiles.iter().map(|t| t.bytes).sum());
        self.gpu_pending_clusters = self.pending.as_ref().map_or(0, |f| {
            f.tiles
                .iter()
                .filter(|t| !t.is_empty() && !adaptive.contains(&t.key))
                .count()
        });
        self.pending_arc_bytes = self
            .pending
            .as_ref()
            .map_or(0, |f| f.tiles.iter().map(|t| t.bytes).sum());
        self.upload_bytes += uploaded;
        self.max_upload_frame_bytes = self.max_upload_frame_bytes.max(uploaded);
        if !self.owners.is_empty() {
            self.coverage_ready = self.owner_guard.covers(&camera, width, height);
        } else {
            self.coverage_ready =
                self.coverage_ready && self.published_pose == Some((camera, width, height));
        }
        self.update_appearance(adaptive, &gpu.device, &gpu.queue, &camera, uploaded == 0)?;
        let gpu_prepare_started = Instant::now();
        self.stats = adaptive.prepare(
            &gpu.device,
            &gpu.queue,
            &camera,
            width,
            height,
            &self.owners,
            self.pixel_error,
        );
        self.gpu_prepare_cpu_ms += gpu_prepare_started.elapsed().as_secs_f64() * 1000.;
        for (total, value) in self.gpu_prepare_stage_ms.iter_mut().zip([
            self.stats.prepare_shadow_ms,
            self.stats.prepare_order_ms,
            self.stats.prepare_occlusion_ms,
            self.stats.prepare_visibility_uniform_ms + self.stats.prepare_upload_ms,
        ]) {
            *total += value;
        }
        self.prepare_order_reuses += u64::from(self.stats.prepare_order_reused);
        let stages = [
            self.stats.prepare_shadow_ms,
            self.stats.prepare_resident_ms,
            (self.stats.prepare_order_ms - self.stats.prepare_resident_ms).max(0.),
            self.stats.prepare_occlusion_ms,
            self.stats.prepare_visibility_uniform_ms,
            self.stats.prepare_upload_ms,
            self.stats.prepare_bundle_ms,
        ];
        for (index, value) in stages.into_iter().enumerate() {
            self.prepare_detail_ms[index] += value;
            let samples = &mut self.prepare_samples[index];
            if samples.len() == PREPARE_SAMPLE_CAP {
                samples.pop_front();
            }
            samples.push_back(value);
        }
        let native_pending = self.owner_surface_bounds.iter().any(|(key, bounds)| {
            key.step_cells > 1
                && crate::adaptive_stream::native_band(
                    *bounds,
                    camera.position,
                    self.stream_settings.native_radius_m,
                )
        });
        self.geometry_quality_ready =
            self.coverage_ready && self.stats.current_quality_ready && !native_pending;
        self.quality_ready =
            self.geometry_quality_ready && adaptive.appearance_debt(&self.owners) == 0;
        if self.quality_ready && self.first_quality_ms.is_none() {
            self.first_quality_ms = Some(self.preparation_started.elapsed().as_secs_f64() * 1000.);
        }
        // A safety reserve for settled-start benchmarks; requested visual quality
        // remains the independent threshold above.
        self.refinement_ready = self.quality_ready
            && self.stats.current_max_error_px.is_some_and(|error| {
                error <= self.pixel_error * crate::adaptive_stream::REFINEMENT_RESERVE
            });
        if self.refinement_ready {
            if let Some(cycle) = self.candidate_preparation.take() {
                self.worker.record_preparation(cycle);
            }
        }
        if self.refinement_ready && self.first_refinement_ms.is_none() {
            self.first_refinement_ms =
                Some(self.preparation_started.elapsed().as_secs_f64() * 1000.);
        }
        self.max_visible_error_px = self.stats.current_max_error_px;
        self.visible_unresolved_count = self.stats.current_unresolved_clusters;
        self.visible_uncertified_count = self.stats.current_uncertified_clusters;
        self.global_bound_clusters = self.stats.current_global_bound_clusters;
        self.local_certified_clusters = self.stats.current_local_certified_clusters;
        self.affine_bound_clusters = self.stats.current_affine_bound_clusters;
        Ok(())
    }
    pub fn settle(
        &mut self,
        gpu: &mut Gpu,
        camera: Camera,
        width: u32,
        height: u32,
        allow_unresolved: bool,
    ) -> Result<(), String> {
        let start = Instant::now();
        let mut last = Instant::now();
        loop {
            self.poll(gpu, camera, width, height)?;
            // Headless preparation has no render submissions to flush staged
            // queue writes. Submit and retire them as uploads progress.
            gpu.queue.submit(std::iter::empty());
            gpu.device.poll(wgpu::Maintain::Poll);
            if self.quality_ready && self.coverage_ready && self.published_epoch == self.epoch {
                return Ok(());
            }
            if self.preparation_refused() {
                eprintln!("adaptive preparation stopped: {}", self.json());
                if allow_unresolved && self.coverage_ready {
                    eprintln!("writing explicit unresolved-quality diagnostic capture");
                    return Ok(());
                }
                return Err(format!(
                    "adaptive quality unresolved at pixel-error {}; capture refused",
                    self.pixel_error
                ));
            }
            if start.elapsed().as_secs() >= 120 {
                eprintln!("adaptive preparation timeout: {}", self.json());
                if allow_unresolved && self.coverage_ready {
                    eprintln!("writing explicit unresolved-quality diagnostic capture");
                    return Ok(());
                }
                return Err(format!(
                    "adaptive capture not settled after 120s: coverage={} pending={} unresolved={}",
                    self.coverage_ready, self.pending_count, self.unresolved
                ));
            }
            if last.elapsed().as_secs() >= 10 {
                eprintln!(
                    "adaptive settling: coverage={} pending={} resident={} elapsed={:.1}s",
                    self.coverage_ready,
                    self.pending_count,
                    self.stats.resident_clusters,
                    start.elapsed().as_secs_f64()
                );
                last = Instant::now();
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    /// Outside terrain is unchanged by edits strictly confined to the excluded
    /// exact owner. Advance presentation revision atomically with its mesh swap.
    pub fn publish_volume_revision(&mut self, world: &frontier_world::World) {
        assert!(self.topology_supported && world.edit_cells().all(|c| world.volume_owned(c)));
        self.displayed_world_revision = world.revision();
    }
    pub fn preparation_refused(&self) -> bool {
        self.unresolved
            && self.pending_count == 0
            && self.pending.is_none()
            && self.deferred.is_none()
    }
    pub fn preparation_elapsed_seconds(&self) -> f64 {
        self.preparation_started.elapsed().as_secs_f64()
    }
    pub fn json(&self) -> String {
        let mut json=format!("{{\"pixel_error\":{},\"coverage_ready\":{},\"quality_ready\":{},\"pending_clusters\":{},\"unresolved\":{},\"published_epoch\":{},\"requested_epoch\":{},\"native_clusters\":{},\"resident_clusters\":{},\"visible_clusters\":{},\"triangles\":{},\"gpu_resident_bytes\":{},\"gpu_allocated_bytes\":{},\"upload_bytes\":{},\"max_upload_frame_bytes\":{},\"cpu_cache_bytes\":{},\"cache_hits\":{},\"builds\":{},\"appearance_self_occlusion_exact\":false,\"topology_supported\":{}}}",self.pixel_error,self.coverage_ready,self.quality_ready,self.pending_count,self.unresolved,self.published_epoch,self.epoch,self.native_clusters,self.stats.resident_clusters,self.stats.visible_clusters,self.stats.triangles,self.stats.resident_bytes,self.stats.allocated_bytes,self.upload_bytes,self.max_upload_frame_bytes,self.cache_bytes,self.cache_hits,self.builds,self.topology_supported);
        json.pop();
        json.push_str(&format!(
            ",\"naadf_owned_tiles\":{},\"naadf_ownership_id\":{},\"naadf_handoff_reused_tiles\":{},\"naadf_handoff_changed_tiles\":{}}}",
            self.naadf_owned_tiles, self.published_naadf_owner,
            self.planning.naadf_handoff_reused_tiles, self.planning.naadf_handoff_changed_tiles
        ));
        json.pop();
        json.push_str(&format!(
            ",\"background_paused\":{}}}",
            self.background_paused
        ));
        json.pop();
        json.push_str(&format!(
            ",\"upload_budget_bytes_per_frame\":{}}}",
            self.upload_budget
        ));
        json.pop();
        json.push_str(&format!(",\"elapsed_preparation_ms\":{},\"first_coverage_ms\":{},\"empirical_geometry_error_m\":{},\"uncertified_clusters\":{},\"selected_cpu_bytes\":{},\"selected_sampled_columns\":{}}}",self.preparation_started.elapsed().as_secs_f64()*1000.,self.first_coverage_ms.map_or("null".into(),|v|v.to_string()),self.empirical_error_m,self.uncertified_clusters,self.selected_bytes,self.sampled_columns));
        json.pop();
        json.push_str(&format!(",\"total_build_ms\":{},\"max_build_ms\":{},\"surface_source_revision\":{},\"displayed_world_revision\":{},\"base_source_reused_outside_edited_owner\":{}}}",self.total_build_ms,self.max_build_ms,self.surface_source_revision,self.displayed_world_revision,self.topology_supported));
        json.pop();
        json.push_str(&format!(
            ",\"generated_columns\":{},\"source_cache_hits\":{},\"source_cache_bytes\":{},\"source_budget_bytes\":{}}}",
            self.generated_columns, self.source_cache_hits, self.source_cache_bytes, self.latest_cache.source_budget_bytes
        ));
        json.pop();
        json.push_str(&format!(",\"cpu_cache_budget_bytes\":{},\"pending_selection_arc_bytes\":{},\"cpu_selection_bytes_overlap_cache\":true,\"gpu_pool_budget_bytes\":{}}}",self.cpu_cache_budget_bytes,self.pending_arc_bytes,self.gpu_pool_budget_bytes));
        json.pop();
        json.push_str(&format!(",\"max_visible_error_px\":{},\"visible_unresolved_count\":{},\"visible_global_bound_clusters\":{},\"visible_local_certified_clusters\":{},\"gpu_pool_allocated_bytes\":{},\"gpu_camera_uniform_bytes\":{},\"frame_attachment_logical_bytes\":{}}}",self.max_visible_error_px.map_or("null".into(),|v|v.to_string()),self.visible_unresolved_count,self.global_bound_clusters,self.local_certified_clusters,self.stats.allocated_bytes,crate::adaptive_gpu::CAMERA_UNIFORM_BYTES,self.frame_attachment_bytes));
        json.pop();
        json.push_str(&format!(",\"visible_uncertified_count\":{},\"native_quad_record_bytes\":32,\"resident_native_quad_bytes\":{},\"resident_profile_bytes\":{},\"visible_native_quads\":{}}}",self.visible_uncertified_count,self.stats.resident_native_quad_bytes,self.stats.resident_profile_bytes,self.stats.visible_native_quads));
        json.pop();
        json.push_str(&format!(",\"deferred_selection_arc_bytes\":{},\"gpu_pending_clusters\":{},\"selection_arc_bytes_may_overlap_each_other\":true}}",self.deferred_arc_bytes,self.gpu_pending_clusters));
        json.pop();
        json.push_str(&format!(",\"gpu_occluded_clusters\":{},\"gpu_occluded_triangles\":{},\"gpu_occluder_boxes\":{},\"gpu_occlusion_uses_current_camera\":true}}",self.stats.occluded_clusters,self.stats.occluded_triangles,self.stats.occluder_boxes));
        json.pop();
        json.push_str(
            ",\"quality_assessment\":\"current_gpu_draw_candidates_after_current_occlusion\"",
        );
        json.push('}');
        json.pop();
        json.push_str(&format!(
            ",\"visible_affine_bound_clusters\":{}}}",
            self.affine_bound_clusters
        ));
        json.pop();
        json.push_str(&format!(
            ",\"elapsed_since_stream_start_ms\":{},\"first_quality_ms\":{}}}",
            self.preparation_started.elapsed().as_secs_f64() * 1000.,
            self.first_quality_ms
                .map_or("null".into(), |v| v.to_string())
        ));
        json.pop();
        json.push_str(&format!(
            ",\"refinement_ready\":{},\"refinement_reserve\":{},\"first_refinement_ms\":{}}}",
            self.refinement_ready,
            crate::adaptive_stream::REFINEMENT_RESERVE,
            self.first_refinement_ms
                .map_or("null".into(), |v| v.to_string())
        ));
        json.pop();
        json.push_str(&format!(
            ",\"cpu_quality_ready\":{},\"cpu_refinement_ready\":{},\"cpu_max_visible_error_px\":{}}}",
            self.cpu_quality_ready, self.cpu_refinement_ready,
            self.cpu_max_visible_error_px.map_or("null".into(), |v| v.to_string())
        ));
        json.pop();
        let offenders = self.stats.current_offenders.iter().flatten().map(|offender| {
            let key = offender.key;
            format!("{{\"key\":{{\"min\":{:?},\"width_cells\":{},\"step_cells\":{},\"edge_steps\":{:?},\"source_revision\":{}}},\"canonical_bounds\":{:?},\"proxy_bounds\":{:?},\"cached_vertical_error_m\":{},\"water_feature_spacing_debt_m\":{},\"projected_error_px\":{},\"current_bounds_visible\":true,\"current_occlusion_rejected\":false}}",
                key.min, key.width_cells, key.step_cells, key.edge_steps, key.source_revision,
                offender.canonical_bounds, offender.proxy_bounds,
                offender.cached_vertical_error_m.map_or("null".into(), |v| v.to_string()),
                offender.water_feature_error_m,
                offender.projected_error_px.map_or("null".into(), |v| v.to_string()))
        }).collect::<Vec<_>>().join(",");
        json.push_str(&format!(",\"current_gpu_offenders\":[{}]}}", offenders));
        json.pop();
        json.push_str(&format!(
            ",\"coarse_batch_groups\":{},\"coarse_batch_candidates\":{}}}",
            self.stats.coarse_batch_groups, self.stats.coarse_batch_candidates
        ));
        json.pop();
        json.push_str(&format!(",\"native_bins_enabled\":{},\"native_bin_tests\":{},\"native_bin_skipped_quads\":{},\"draw_calls\":{},\"no_voxel_shadow_clusters\":{}}}",self.native_bins,self.stats.native_bin_tests,self.stats.native_bin_skipped_quads,self.stats.draw_calls,self.stats.no_voxel_shadow_clusters));
        json.pop();
        json.push_str(&format!(",\"vertex_pages_coalescing_enabled\":{},\"vertex_page_binds_eligible\":{},\"vertex_page_binds_avoided\":{}}}", self.stats.vertex_pages_coalescing_enabled, self.stats.vertex_page_binds_eligible, self.stats.vertex_page_binds_avoided));
        json.pop();
        json.push_str(&format!(",\"source_cache_peak_bytes\":{},\"source_cache_pages\":{},\"source_cache_columns\":{},\"source_cache_promotions\":{},\"source_cache_page_evictions\":{},\"affine_quad_bounds\":{},\"affine_noise_cells\":{},\"affine_full_range_fallbacks\":{},\"affine_unavailable_quads\":{},\"affine_ms\":{}}}",self.latest_cache.source_cache_peak_bytes,self.latest_cache.source_cache_pages,self.latest_cache.source_cache_columns,self.latest_cache.source_cache_promotions,self.latest_cache.source_cache_page_evictions,self.latest_cache.affine_quad_bounds,self.latest_cache.affine_noise_cells,self.latest_cache.affine_full_range_fallbacks,self.latest_cache.affine_unavailable_quads,self.latest_cache.affine_ms));
        json.pop();
        json.push_str(&format!(",\"preparation_cpu_stage_ms\":{{\"appearance\":{},\"certificate\":{},\"native_mesh\":{},\"finalize\":{}}},\"preparation_stage_note\":\"CPU stage sums; canonical source query costs are included in caller stages, not exclusive source timings\"}}", self.latest_cache.appearance_ms, self.latest_cache.certificate_ms, self.latest_cache.native_mesh_ms, self.latest_cache.finalize_ms));
        json.pop();
        json.push_str(&format!(",\"preparation_workers\":{}}}", self.worker_count));
        json.pop();
        let disk = self.latest_cache.disk;
        json.push_str(&format!(
            ",\"terrain_bound_pages\":{{\"batches\":{},\"evictions\":{}}}",
            self.latest_cache.bound_page_batches, self.latest_cache.bound_page_evictions
        ));
        json.push_str(&format!(",\"disk_cache\":{{\"hits\":{},\"misses\":{},\"invalid\":{},\"writes\":{},\"errors\":{},\"evictions\":{},\"read_bytes\":{},\"written_bytes\":{},\"read_ms\":{},\"write_ms\":{},\"resident_bytes\":{}}}}}", disk.hits, disk.misses, disk.invalid, disk.writes, disk.errors, disk.evictions, disk.read_bytes, disk.written_bytes, disk.read_ms, disk.write_ms, disk.resident_bytes));
        json.pop();
        json.push_str(&format!(",\"disk_writeback\":{{\"queued_entries\":{},\"peak_queued_entries\":{},\"queued_payload_bytes\":{},\"peak_queued_payload_bytes\":{},\"skipped_writes\":{},\"encode_buffer_cap_bytes\":{},\"encode_buffer_reserved_bytes\":{},\"peak_encode_buffer_reserved_bytes\":{}}}}}", disk.writeback_queued_entries, disk.writeback_peak_queued_entries, disk.writeback_queued_payload_bytes, disk.writeback_peak_queued_payload_bytes, disk.writeback_skipped_writes, disk.writeback_encode_buffer_cap_bytes, disk.writeback_encode_buffer_reserved_bytes, disk.writeback_peak_encode_buffer_reserved_bytes));
        let prediction = self.worker.prediction_stats();
        json.pop();
        json.push_str(&format!(
            ",\"quality_cycle_latency\":{{\"samples\":{},\"p95_ms\":{}}}}}",
            prediction.quality_cycle_samples,
            prediction
                .quality_cycle_p95_ms
                .map_or("null".into(), |v| v.to_string())
        ));
        json.pop();
        json.push_str(&format!(",\"prediction\":{{\"horizon_ms\":{},\"horizon_from_publication\":{},\"publication_samples\":{},\"generation_samples\":{},\"active\":{},\"transaction_latency_ms\":{},\"forecast_transactions\":{},\"debt_prediction_enabled\":{}}},\"live_tile_payload_bytes\":{},\"peak_live_tile_payload_bytes\":{}}}", prediction.horizon_ms.map_or("null".into(), |v| v.to_string()), prediction.horizon_from_publication, prediction.publication_samples, prediction.generation_samples, prediction.active, prediction.transaction_latency_ms.map_or("null".into(), |v| v.to_string()), prediction.forecast_transactions, prediction.debt_prediction_enabled, self.latest_cache.live_tile_payload_bytes, self.latest_cache.peak_live_tile_payload_bytes));
        json.pop();
        json.push_str(&format!(
            ",\"affine_cache_hits\":{},\"affine_cache_misses\":{},\"affine_cache_bytes\":{}}}",
            self.latest_cache.affine_cache_hits,
            self.latest_cache.affine_cache_misses,
            self.latest_cache.affine_cache_bytes
        ));
        json.pop();
        json.push_str(&format!(
            ",\"coarse_certification\":{{\"source_ms\":{},\"roof_ms\":{},\"dominated_patches\":{},\"page_lock_wait_ms\":{},\"page_evaluation_ms\":{}}}}}",
            self.latest_cache.coarse_source_bound_ms,
            self.latest_cache.coarse_roof_bound_ms,
            self.latest_cache.coarse_roofs_skipped,
            self.latest_cache.bound_page_lock_ms,
            self.latest_cache.bound_page_evaluation_ms
        ));
        json.pop();
        json.push_str(&format!(
            ",\"coarse_interior_cache\":{{\"hits\":{},\"reserved_bytes\":{}}},\"coarse_y_pitch_divisor\":{}}}",
            self.latest_cache.coarse_interior_hits,
            self.latest_cache.coarse_interior_cache_bytes,
            self.coarse_y_pitch_divisor
        ));
        json.pop();
        json.push_str(&format!(
            ",\"resident_proxy_triangle_bytes\":{}}}",
            self.stats.resident_proxy_triangle_bytes
        ));
        json.pop();
        json.push_str(&format!(
            ",\"indexed_proxy\":{{\"mode\":{},\"enabled\":{},\"derived_clusters\":{},\"resident_clusters\":{},\"position_bytes\":{},\"position_reserved_bytes\":{},\"record_padding_bytes\":{},\"pack_ms\":{},\"pack_attempts\":{},\"pack_accepted\":{},\"peak_staging_estimate_bytes\":{},\"normal_roundoff_possible\":{},\"extra_bind_group_binds\":0,\"source_generation_savings_claimed\":false,\"cpu_tile_payload_unchanged\":true}}}}",
            self.stats.indexed_proxy_mode,
            self.stats.indexed_proxy_enabled,
            self.stats.resident_derived_proxy_clusters,
            self.stats.resident_indexed_proxy_clusters,
            self.stats.resident_proxy_position_bytes,
            self.stats.resident_proxy_position_reserved_bytes,
            self.stats.resident_indexed_proxy_record_padding_bytes,
            self.stats.indexed_proxy_pack_ms,
            self.stats.indexed_proxy_pack_attempts,
            self.stats.indexed_proxy_pack_accepted,
            self.stats.indexed_proxy_peak_staging_estimate_bytes,
            self.stats.resident_derived_proxy_clusters > 0,
        ));
        json.pop();
        let preparation = self.latest_cache.gpu_profiles;
        let profiles = self.stats.gpu_profiles;
        json.push_str(&format!(
            ",\"gpu_profile_preparation\":{{\"enabled\":{},\"eligible_tiles\":{},\"deferred_tiles\":{},\"deferred_jobs\":{},\"fallback_tiles\":{},\"snapshot_generated_columns\":{},\"snapshot_ms\":{}}},\"gpu_profiles\":{{\"enabled\":{},\"source_cache_hits\":{},\"source_uploads\":{},\"source_upload_bytes\":{},\"job_upload_bytes\":{},\"result_copy_bytes\":{},\"cpu_profile_upload_bytes_avoided\":{},\"dispatches\":{},\"profiles\":{},\"submits\":{},\"enqueue_ms\":{},\"initialization_ms\":{},\"resident_gpu_bytes\":{},\"source_cache_cpu_bytes\":{},\"gpu_ms\":null,\"readback_bytes\":0}}}}",
            preparation.enabled, preparation.eligible_tiles, preparation.deferred_tiles,
            preparation.deferred_jobs, preparation.fallback_tiles,
            preparation.snapshot_generated_columns, preparation.snapshot_ms,
            profiles.enabled, profiles.source_cache_hits, profiles.source_uploads,
            profiles.source_upload_bytes, profiles.job_upload_bytes, profiles.result_copy_bytes,
            profiles.cpu_profile_upload_bytes_avoided, profiles.dispatches, profiles.profiles,
            profiles.submits, profiles.enqueue_ms, profiles.initialization_ms, profiles.resident_gpu_bytes,
            profiles.source_cache_cpu_bytes,
        ));
        json.pop();
        json.push_str(&format!(
            ",\"gpu_resource_limited\":{}}}",
            self.gpu_resource_limited
        ));
        json.pop();
        json.push_str(&format!(
            ",\"gpu_cluster_capacity\":{},\"gpu_available_cluster_slots\":{}}}",
            self.stats.cluster_capacity, self.stats.available_cluster_slots
        ));
        json.pop();
        json.push_str(&format!(
            ",\"direct_native_urgent\":{},\"visible_vertical_error_bound\":{}}}",
            self.direct_native_urgent, self.visible_vertical_bound
        ));
        json.pop();
        json.push_str(&format!(
            ",\"appearance_cache\":{{\"hits\":{},\"misses\":{},\"bytes\":{}}}}}",
            self.latest_cache.appearance_cache_hits,
            self.latest_cache.appearance_cache_misses,
            self.latest_cache.appearance_cache_bytes
        ));
        json.pop();
        json.push_str(&format!(",\"render_bundles\":{{\"enabled\":{},\"active\":{},\"builds\":{},\"reuses\":{},\"build_ms\":{}}}}}", self.stats.bundle_enabled, self.stats.bundle_active, self.stats.bundle_builds, self.stats.bundle_reuses, self.stats.bundle_build_ms));
        let c = self.latest_cache;
        json.pop();
        json.push_str(&format!(",\"cold_source_residency\":{{\"enabled_shards\":{},\"reserved_bytes\":{},\"resident_bytes\":{},\"entries\":{},\"retained_raw_bytes\":{}}}}}", c.source_cold_enabled_shards, c.source_cold_budget_bytes, c.source_cold_resident_bytes, c.source_cold_entries, c.source_cold_retained_raw_bytes));
        json.pop();
        json.push_str(&format!(",\"cold_source_cache\":{{\"hits\":{},\"encoded_bytes_cumulative\":{},\"raw_bytes_cumulative\":{},\"encode_ms\":{},\"decode_ms\":{}}}}}", c.source_cold_hits, c.source_cold_encoded_bytes, c.source_cold_raw_bytes, c.source_cold_encode_ms, c.source_cold_decode_ms));
        json.pop();
        json.push_str(&format!(",\"exclusive_cpu_profile\":{{\"enabled\":{},\"source_lock_ms\":{},\"source_lookup_ms\":{},\"source_generate_ms\":{},\"appearance_ms\":{},\"certificate_ms\":{},\"native_mesh_ms\":{},\"proxy_ms\":{},\"cache_snapshot_ms\":{},\"cache_admission_ms\":{},\"cache_eviction_ms\":{},\"lru_metadata_bytes\":{}}}}}", c.profile_enabled, c.source_lock_ms, c.source_lookup_ms, c.source_generate_ms, c.appearance_exclusive_ms, c.certificate_exclusive_ms, c.native_mesh_exclusive_ms, c.proxy_exclusive_ms, c.cache_snapshot_ms, c.cache_admission_ms, c.cache_eviction_ms, c.lru_metadata_bytes));
        json.pop();
        json.push_str(&format!(",\"presentation_cpu_ms\":{{\"upload_enqueue\":{},\"publication_check\":{},\"gpu_visibility_uniforms_bundles\":{}}}}}", self.upload_cpu_ms, self.publication_cpu_ms, self.gpu_prepare_cpu_ms));
        json.pop();
        json.push_str(&format!(",\"terrain_prepare_stages_cpu_ms\":{{\"reuse_enabled\":{},\"shadows\":{},\"resident_accounting_and_order\":{},\"occluder_build\":{},\"visibility_error_uniforms_upload\":{}}}}}",
            self.stats.prepare_reuse_enabled, self.gpu_prepare_stage_ms[0], self.gpu_prepare_stage_ms[1], self.gpu_prepare_stage_ms[2], self.gpu_prepare_stage_ms[3]));
        json.pop();
        json.push_str(",\"terrain_prepare_detail_cpu_ms\":{\"scope\":\"all_prepare_calls_including_startup_and_warmup\",\"sample_cap_per_stage\":4096,\"incremental_order_reuses\":");
        json.push_str(&self.prepare_order_reuses.to_string());
        for index in 0..PREPARE_NAMES.len() {
            let mut samples: Vec<_> = self.prepare_samples[index].iter().copied().collect();
            samples.sort_by(f64::total_cmp);
            let percentile = |p: usize| {
                samples
                    .get((samples.len() * p).div_ceil(100).saturating_sub(1))
                    .copied()
                    .unwrap_or(0.)
            };
            json.push_str(&format!(
                ",\"{}\":{{\"total\":{},\"samples\":{},\"p50\":{},\"p95\":{},\"p99\":{}}}",
                PREPARE_NAMES[index],
                self.prepare_detail_ms[index],
                samples.len(),
                percentile(50),
                percentile(95),
                percentile(99)
            ));
        }
        json.push_str("}}");
        json.pop();
        let (
            enabled,
            debt,
            queued,
            active,
            outstanding,
            done,
            cancelled,
            discarded,
            refusals,
            promotions,
            stale,
            appearance_cpu,
            appearance_upload,
            appearance_bytes,
            allocation_refusals,
            first_geometry,
            first_appearance,
            retained_inputs,
            ready_bytes,
        ) = self.cover.as_ref().map_or(
            (
                false, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0., 0., 0, 0, None, None, 0, 0,
            ),
            |cover| {
                let stats = cover.worker.stats();
                (
                    true,
                    cover.appearance_debt,
                    stats.queued,
                    stats.active,
                    stats.outstanding,
                    stats.completed,
                    stats.cancelled,
                    stats.discarded_completions,
                    stats.payload_refusals,
                    cover.promotions,
                    cover.stale_results,
                    cover.cpu_ms,
                    cover.upload_ms,
                    cover.upload_bytes,
                    cover.allocation_refusals,
                    cover.first_geometry_ms,
                    cover.first_appearance_ms,
                    cover.tiles.values().map(|tile| tile.bytes).sum::<usize>(),
                    cover.ready.as_ref().map_or(0, |tile| tile.bytes),
                )
            },
        );
        let optional_ms =
            |value: Option<f64>| value.map_or_else(|| "null".into(), |v| v.to_string());
        json.push_str(&format!(",\"deferred_ground_cover\":{{\"enabled\":{},\"geometry_quality_ready\":{},\"appearance_ready\":{},\"appearance_pending_clusters\":{},\"queued\":{},\"active\":{},\"outstanding\":{},\"completed\":{},\"cancelled\":{},\"discarded_completions\":{},\"payload_refusals\":{},\"promotions\":{},\"stale_results\":{},\"compute_cpu_ms\":{},\"upload_enqueue_cpu_ms\":{},\"upload_bytes\":{},\"allocation_refusals\":{},\"first_geometry_coverage_ms\":{},\"first_appearance_ready_ms\":{},\"retained_base_tile_bytes\":{},\"ready_tile_bytes\":{},\"max_product_bytes\":16777216,\"max_outstanding_jobs\":2,\"scratch_and_shared_cover_cache_excluded\":true,\"payload_refused_clusters\":{},\"capacity_refused_clusters\":{}}}}}", enabled, self.geometry_quality_ready, debt == 0, debt, queued, active, outstanding, done, cancelled, discarded, refusals, promotions, stale, appearance_cpu, appearance_upload, appearance_bytes, allocation_refusals,
            optional_ms(first_geometry), optional_ms(first_appearance), retained_inputs, ready_bytes,
            self.cover.as_ref().map_or(0, |cover| cover.refused.len()),
            self.cover.as_ref().map_or(0, |cover| cover.capacity_refused.0.len())));
        json.pop();
        json.push_str(&format!(
            ",\"prepared_view_cache\":{{\"hits\":{},\"misses\":{}}}}}",
            self.stats.prepared_view_hits, self.stats.prepared_view_misses
        ));
        json.pop();
        json.push_str(&format!(
            ",\"refinements_per_transaction\":{}}}",
            self.refinements_per_transaction
        ));
        let planning = self.planning;
        json.pop();
        json.push_str(&format!(",\"turn_cache\":{{\"warm_clusters\":{},\"warm_bytes\":{},\"reuses\":{},\"evictions\":{},\"prefetch_selections\":{}}}", self.stats.warm_clusters, self.stats.warm_bytes, self.stats.warm_reuses, self.stats.warm_evictions, planning.turn_prefetches));
        json.push_str(&format!(",\"planning\":{{\"calls\":{},\"full_calls\":{},\"retained_calls\":{},\"total_ms\":{},\"max_ms\":{},\"prepared_batches_without_plan\":{},\"obsolete_transaction_preemptions\":{},\"obsolete_transaction_pending_jobs\":{}}}}}", planning.calls, planning.full_calls, planning.retained_calls, planning.total_ms, planning.max_ms, planning.prepared_batches_without_plan, planning.obsolete_transaction_preemptions, planning.obsolete_transaction_pending_jobs));
        let memory = self.build_memory;
        json.pop();
        json.push_str(&format!(",\"build_memory_model\":{{\"budget_bytes\":{},\"reserved_bytes\":{},\"peak_reserved_bytes\":{},\"evicted_arc_bytes\":{},\"refused_jobs\":{},\"last_batch_jobs\":{}}}}}", memory.budget_bytes, memory.reserved_bytes, memory.peak_reserved_bytes, memory.evicted_arc_bytes, memory.refused_jobs, memory.last_batch_jobs));
        json
    }
}

/// A current demand may supersede a target only when that target no longer
/// covers it. Otherwise finish the bounded upload before accepting newer detail.
fn queue_upload_target<T>(
    pending: &mut Option<T>,
    deferred: &mut Option<T>,
    incoming: T,
    preempt: bool,
) {
    if pending.is_none() || preempt {
        *pending = Some(incoming);
        *deferred = None;
    } else {
        *deferred = Some(incoming);
    }
}

fn publication_keys(
    frame: &SelectionFrame,
    revision: u64,
    has_volume: bool,
    camera: &Camera,
    width: u32,
    height: u32,
) -> Option<HashSet<TileKey>> {
    if !frame.coverage_ready
        || frame
            .tiles
            .iter()
            .any(|tile| tile.key.source_revision != revision)
    {
        return None;
    }
    let keys: HashSet<_> = frame.tiles.iter().map(|tile| tile.key).collect();
    crate::adaptive_stream::published_partition_covers(&keys, has_volume, camera, width, height)
        .then_some(keys)
}

#[cfg(test)]
mod upload_tests {
    use super::queue_upload_target;

    #[test]
    fn moving_epochs_finish_multi_frame_upload_before_new_detail() {
        let mut pending = None;
        let mut deferred = None;
        let mut resident = std::collections::HashSet::new();
        let target = vec![10u32, 11, 12];
        for epoch in 1..=3 {
            queue_upload_target(
                &mut pending,
                &mut deferred,
                if epoch == 1 {
                    target.clone()
                } else {
                    vec![20 + epoch]
                },
                false,
            );
            // Same retain-and-upload behavior as the presentation loop, with a
            // one-cluster budget and a new camera demand on every frame.
            let active = pending.as_ref().unwrap();
            resident.retain(|key| active.contains(key));
            if let Some(key) = active.iter().find(|key| !resident.contains(*key)) {
                resident.insert(*key);
            }
        }
        assert_eq!(pending, Some(target.clone()));
        assert!(target.iter().all(|key| resident.contains(key)));
        assert_eq!(deferred, Some(vec![23]));
        // A teleport outside the target guard can preempt; partial residency
        // must not force publication of the previous target.
        queue_upload_target(&mut pending, &mut deferred, vec![100], true);
        assert_eq!(pending, Some(vec![100]));
        assert!(deferred.is_none());
    }
}

/// Whole measured-route quality, sampled only by Viewer using its frame-timing gate.
#[derive(Default)]
pub struct RouteQualityStats {
    measured_frames: u64,
    coverage_ready_frames: u64,
    quality_ready_frames: u64,
    unbounded_error_frames: u64,
    max_finite_visible_error_px: Option<f64>,
    peak_unresolved_clusters: usize,
}
impl RouteQualityStats {
    pub fn record(&mut self, view: &AdaptiveView) {
        self.measured_frames += 1;
        self.coverage_ready_frames += u64::from(view.coverage_ready);
        self.quality_ready_frames += u64::from(view.quality_ready);
        self.unbounded_error_frames += u64::from(view.stats.current_max_error_px.is_none());
        if let Some(error) = view.stats.current_max_finite_error_px {
            self.max_finite_visible_error_px = Some(
                self.max_finite_visible_error_px
                    .map_or(error, |max| max.max(error)),
            );
        }
        self.peak_unresolved_clusters = self
            .peak_unresolved_clusters
            .max(view.stats.current_unresolved_clusters);
    }
    pub fn json(&self) -> String {
        format!("{{\"measured_frames\":{},\"coverage_ready_frames\":{},\"quality_ready_frames\":{},\"unbounded_error_frames\":{},\"max_finite_visible_error_px\":{},\"peak_unresolved_clusters\":{}}}",self.measured_frames,self.coverage_ready_frames,self.quality_ready_frames,self.unbounded_error_frames,self.max_finite_visible_error_px.map_or("null".into(),|v|v.to_string()),self.peak_unresolved_clusters)
    }
}

#[cfg(test)]
#[test]
fn appearance_capacity_refusal_releases_completion_and_retries_after_resource_changes() {
    let key = TileKey {
        min: [0, 0],
        width_cells: 128,
        step_cells: 8,
        edge_steps: [8; 4],
        source_revision: 0,
    };
    let other = TileKey {
        min: [128, 0],
        ..key
    };
    let resources = (7, 512);
    let mut refusals = CapacityRefusals::default();
    let mut ready = Some(key);
    refusals.park(key, resources, &mut ready);
    assert!(
        ready.is_none(),
        "refused product must release the polling slot"
    );
    assert!(refusals.blocked(&key, resources));
    assert!(
        !refusals.blocked(&other, resources),
        "another key must make progress"
    );
    assert!(
        !refusals.blocked(&key, (8, 512)),
        "retirement or uploads permit retry"
    );
    assert!(
        !refusals.blocked(&key, (7, 768)),
        "budget changes permit retry"
    );
}
