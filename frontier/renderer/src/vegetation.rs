//! Bounded, camera-demand vegetation streaming. Canonical placement stays in world;
//! automatically derived display meshes never change gameplay voxel identity.
use frontier_world::{
    vegetation::{self, PlantRecord, Species},
    Cell, Material, World, VOXEL_SIZE,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::Instant,
};
#[path = "shadow_lifecycle.rs"]
mod shadow_lifecycle;
use shadow_lifecycle::ShadowLifecycle;
pub use shadow_lifecycle::{ShadowCoverage, ShadowInspection, ShadowReason, ShadowTransition};

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VegetationVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    /// Static display-only ambient visibility; never modifies canonical pigment.
    pub ao: f32,
    pub transmission: f32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct MeshKey {
    pub species: u8,
    pub variant: u8,
    pub seed: u64,
    /// Global native-grid ground-cover tile; zero for individual trees.
    pub tile: [i64; 2],
    /// Native-cell stride: 1,2,4,8.
    pub lod: u8,
    pub edit_id: u64,
}
#[derive(Clone, Debug, Default)]
pub struct VegetationMesh {
    pub vertices: Vec<VegetationVertex>,
    pub indices: Vec<u32>,
}
#[derive(Clone, Copy, Debug)]
pub struct VegetationInstance {
    pub root_cell: [i64; 3],
    pub quarter_turn: u32,
    pub fade: f32,
}
#[derive(Clone)]
pub struct VegetationBatch {
    pub key: MeshKey,
    pub mesh: Arc<VegetationMesh>,
    pub instances: Vec<VegetationInstance>,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct VegetationStats {
    pub resident_tiles: usize,
    pub resident_meshes: usize,
    pub pending_jobs: usize,
    pub selected_instances: usize,
    pub triangles: usize,
    pub budget_rejected: usize,
    pub missing_tiles: usize,
    pub missing_meshes: usize,
    /// Grass, edelweiss, rowan, in that order.
    pub selected_by_species: [usize; 3],
    pub triangles_by_species: [usize; 3],
    pub rejected_by_species: [usize; 3],
    pub culled_instances: usize,
    pub coverage_fallbacks: usize,
    pub coverage_unresolved: usize,
    /// Offline native reference is incomplete if the global budget rejected a plant.
    pub reference_budget_exceeded: bool,
    pub reference: bool,
    pub mesh_budget_bytes: usize,
    pub gpu_budget_bytes: usize,
    pub shadow_only_instances: usize,
    pub shadow_only_triangles: usize,
    /// Resident ground detail kept while unrelated coverage is being prepared.
    pub retained_ground_detail: usize,
    /// Cumulative worker telemetry; order is placement, tree mesh, ground mesh.
    pub jobs: VegetationJobStats,
    pub retained_tree_detail: usize,
    /// Unique geometry referenced by this frame, excluding Arc metadata.
    pub selected_cpu_geometry_bytes: usize,
    /// Cache plus selected payload retained by the frame after eviction.
    /// Excludes worker models, queued results and external retained frame Arcs.
    pub total_cpu_geometry_bytes: usize,
    pub cpu_cache_geometry_bytes: usize,
    pub selected_cpu_meshes: usize,
    pub selected_mesh_evictions: usize,
    pub selected_mesh_evicted_bytes: usize,
    pub tree_lod_changes: usize,
    pub tree_refinements: usize,
    pub tree_coarsenings: usize,
    /// Selected color trees, strides 1, 2, 4, 8.
    pub tree_lod_histogram: [usize; 4],
    pub shadow_lod_histogram: [usize; 4],
    pub shadow_lod_changes: usize,
    pub explicit_shadow_instances: usize,
    pub explicit_shadow_triangles: usize,
    pub shadow_budget_rejected: usize,
    pub shadow_transition_pairs: usize,
    pub shadow_transition_triangles: usize,
    pub shadow_fading_casters: usize,
    /// The CPU lifecycle exclusively owns tree admission/range in this mode.
    pub shadow_lifecycle_enabled: bool,
    /// Color demand and resident render candidates before budget admission.
    pub demanded_by_species: [usize; 3],
    pub ready_by_species: [usize; 3],
    pub prediction_horizon_ms: f64,
    pub prediction_lead_m: f64,
    pub predicted_tiles: usize,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct VegetationJobStats {
    pub workers: usize,
    pub started: [u64; 3],
    pub completed: [u64; 3],
    pub cancelled: [u64; 3],
    pub cpu_nanos: [u64; 3],
    pub max_job_nanos: [u64; 3],
    pub assembly_nanos: [u64; 3],
    pub mesh_nanos: [u64; 3],
}
#[derive(Default)]
struct JobCounters {
    started: [AtomicU64; 3],
    completed: [AtomicU64; 3],
    cancelled: [AtomicU64; 3],
    cpu_nanos: [AtomicU64; 3],
    max_job_nanos: [AtomicU64; 3],
    assembly_nanos: [AtomicU64; 3],
    mesh_nanos: [AtomicU64; 3],
}
impl JobCounters {
    fn snapshot(&self, workers: usize) -> VegetationJobStats {
        let read =
            |values: &[AtomicU64; 3]| std::array::from_fn(|i| values[i].load(Ordering::Relaxed));
        VegetationJobStats {
            workers,
            started: read(&self.started),
            completed: read(&self.completed),
            cancelled: read(&self.cancelled),
            cpu_nanos: read(&self.cpu_nanos),
            max_job_nanos: read(&self.max_job_nanos),
            assembly_nanos: read(&self.assembly_nanos),
            mesh_nanos: read(&self.mesh_nanos),
        }
    }
}
// RAII also accounts for jobs cancelled midway through canonical generation.
struct JobTimer<'a> {
    counters: &'a JobCounters,
    kind: usize,
    start: Instant,
    completed: bool,
}
impl<'a> JobTimer<'a> {
    fn new(counters: &'a JobCounters, kind: usize) -> Self {
        counters.started[kind].fetch_add(1, Ordering::Relaxed);
        Self {
            counters,
            kind,
            start: Instant::now(),
            completed: false,
        }
    }
}
impl Drop for JobTimer<'_> {
    fn drop(&mut self) {
        let nanos = self.start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        self.counters.cpu_nanos[self.kind].fetch_add(nanos, Ordering::Relaxed);
        self.counters.max_job_nanos[self.kind].fetch_max(nanos, Ordering::Relaxed);
        let terminal = if self.completed {
            &self.counters.completed
        } else {
            &self.counters.cancelled
        };
        terminal[self.kind].fetch_add(1, Ordering::Relaxed);
    }
}
#[derive(Clone)]
pub struct VegetationFrame {
    pub origin: [i64; 3],
    pub batches: Vec<VegetationBatch>,
    /// Independent tree shadow geometry; GPU selection prefers these per root.
    pub shadow_batches: Vec<VegetationBatch>,
    pub shadow_transitions: Vec<ShadowTransition>,
    pub shadow_coverage: Vec<ShadowCoverage>,
    /// Bounded per-tree admission/replacement decisions, independent of eye visibility.
    pub shadow_inspection: Vec<ShadowInspection>,
    pub stats: VegetationStats,
}
#[derive(Clone, Debug)]
pub struct VegetationConfig {
    pub grass_radius_m: f64,
    pub flower_radius_m: f64,
    pub tree_radius_m: f64,
    pub pixel_error: f32,
    pub max_tiles: usize,
    pub max_meshes: usize,
    pub max_instances: usize,
    pub max_triangles: usize,
    /// Screenshot-only native meshes; retains runtime placement, density and fades.
    pub reference: bool,
}
impl Default for VegetationConfig {
    fn default() -> Self {
        Self {
            grass_radius_m: 28.,
            flower_radius_m: 56.,
            tree_radius_m: 220.,
            pixel_error: 1.25,
            max_tiles: 384,
            max_meshes: 512,
            max_instances: 12_000,
            max_triangles: 1_000_000,
            reference: false,
        }
    }
}
impl VegetationConfig {
    fn with_reference_flag(mut self, flag: Option<&str>) -> Self {
        self.reference |= flag == Some("1");
        if self.reference {
            self.max_triangles = self.max_triangles.max(32_000_000);
            self.max_instances = self.max_instances.max(20_000);
            self.max_meshes = self.max_meshes.max(128);
        }
        self
    }
}
fn ground_key(p: &PlantRecord, lod: u8, edit_id: u64) -> MeshKey {
    MeshKey {
        species: species_id(p.species),
        variant: p.variant,
        seed: p.seed,
        tile: [p.root.x.div_euclid(128), p.root.z.div_euclid(128)],
        lod,
        edit_id,
    }
}
fn selected_stride(distance: f64, projection: f64, config: &VegetationConfig) -> u8 {
    let pixel = VOXEL_SIZE * projection / distance.max(1.);
    let mut lod = 1;
    while !config.reference && lod < 8 && pixel * (lod * 2) as f64 <= config.pixel_error as f64 {
        lod *= 2;
    }
    lod
}
fn ground_stride(distance: f64, projection: f64, config: &VegetationConfig) -> u8 {
    selected_stride(distance, projection, config).max(if distance <= 8. {
        1
    } else if distance <= 16. {
        2
    } else {
        4
    })
}
fn hysteretic_stride(target: u8, relaxed: u8, previous: Option<u8>) -> u8 {
    match previous {
        Some(old) if old < target => target.min(old.max(relaxed)),
        _ => target,
    }
}
fn budget_allows(
    config: &VegetationConfig,
    stats: &VegetationStats,
    species: usize,
    triangles: usize,
) -> bool {
    if stats.selected_instances >= config.max_instances
        || triangles > config.max_triangles.saturating_sub(stats.triangles)
    {
        return false;
    }
    if config.reference {
        return true;
    }
    // Trees are considered first for their silhouettes, but cannot consume
    // the 35% reserved for small ground cover. Ground cover may borrow any
    // unused tree allocation while the global budget remains authoritative.
    match species {
        2 => {
            triangles
                <= (config.max_triangles.saturating_mul(65) / 100)
                    .saturating_sub(stats.triangles_by_species[2])
        }
        1 => triangles <= (config.max_triangles / 10).saturating_sub(stats.triangles_by_species[1]),
        _ => true,
    }
}
fn plant_edit_id(p: &PlantRecord, edits: &[Cell], revision: u64) -> u64 {
    let altered = edits.iter().any(|e| {
        (e.x - p.root.x).abs() <= 160
            && (e.z - p.root.z).abs() <= 160
            && e.y >= p.root.y - 1
            && e.y <= p.root.y + 320
    });
    if altered {
        mix(p.root.x as u64 ^ mix(p.root.z as u64) ^ revision).max(1)
    } else {
        0
    }
}
fn tree_refinement_headroom(total: usize, used: usize, remaining_baseline: usize) -> usize {
    (total.saturating_mul(65) / 100)
        .saturating_sub(used)
        .saturating_sub(remaining_baseline)
}

// Retaining valid detail has no admission penalty. Refinement needs 10% spare
// capacity, so a budget fluctuating at the measured mesh cost cannot toggle LOD.
fn tree_lod_admitted(lod: u8, previous: Option<u8>, cost: usize, room: usize) -> bool {
    let margin = if previous.is_some_and(|old| lod < old) {
        cost / 10
    } else {
        0
    };
    cost.saturating_add(margin) <= room
}
// A resident baseline is coverage, not a ready distance replacement. Keep
// finer current geometry until the requested coarser level can replace it.
fn retain_current_mesh(
    candidate: Option<MeshKey>,
    current: Option<MeshKey>,
    target: u8,
) -> Option<MeshKey> {
    if current.is_some_and(|old| old.lod < target) && candidate.is_none_or(|next| next.lod > target)
    {
        current
    } else {
        candidate.or(current)
    }
}
// Match GPU admission: reserve its remaining 50k triangles for ground cover.
const TREE_SHADOW_TRIANGLE_BUDGET: usize = 150_000;
fn resident_coverage_key(
    meshes: &BTreeMap<MeshKey, (Arc<VegetationMesh>, u64)>,
    key: MeshKey,
    previous: Option<u8>,
) -> Option<MeshKey> {
    previous
        .map(|lod| MeshKey { lod, ..key })
        .filter(|k| meshes.contains_key(k))
        .or_else(|| {
            meshes
                .keys()
                .filter(|k| tree_identity(**k) == tree_identity(key))
                .min_by_key(|k| k.lod)
                .copied()
        })
}
fn shadow_stride(distance: f64, previous: Option<u8>, reference: bool) -> u8 {
    if reference {
        return 1;
    }
    let target = if distance <= 16. { 2 } else { 4 };
    let relaxed = if distance * 0.9 <= 16. { 2 } else { 4 };
    hysteretic_stride(target, relaxed, previous).max(2)
}
fn tree_identity(key: MeshKey) -> MeshKey {
    MeshKey { lod: 0, ..key }
}

#[derive(Clone, Copy)]
struct CullView {
    eye: glam::DVec3,
    forward: glam::DVec3,
    right: glam::DVec3,
    up: glam::DVec3,
    tan_y: f64,
    tan_x: f64,
}
impl CullView {
    fn new(eye: [f64; 3], forward: [f64; 3], fov_y: f32, aspect: f32) -> Option<Self> {
        let forward = glam::DVec3::from_array(forward).try_normalize()?;
        let world_up = if forward.y.abs() > 0.999 {
            glam::DVec3::Z
        } else {
            glam::DVec3::Y
        };
        let right = forward.cross(world_up).normalize();
        let up = right.cross(forward).normalize();
        let tan_y = (fov_y as f64 * 0.5).tan();
        let tan_x = tan_y * aspect as f64;
        if !tan_x.is_finite() || !tan_y.is_finite() || tan_x <= 0. || tan_y <= 0. {
            return None;
        }
        Some(Self {
            eye: glam::DVec3::from_array(eye),
            forward,
            right,
            up,
            tan_y,
            tan_x,
        })
    }
    fn intersects_sphere(&self, center: glam::DVec3, radius: f64) -> bool {
        let delta = center - self.eye;
        let z = delta.dot(self.forward);
        z + radius >= 0.025
            && delta.dot(self.right).abs() - z * self.tan_x
                <= radius * (1. + self.tan_x * self.tan_x).sqrt()
            && delta.dot(self.up).abs() - z * self.tan_y
                <= radius * (1. + self.tan_y * self.tan_y).sqrt()
    }
    fn plant_visible(&self, p: &PlantRecord) -> bool {
        // Conservative source bounds avoid generating unique trees on the render
        // thread. Detailed meshes and their exact boxes belong to the worker.
        let (radius, height) = if p.species == Species::Rowan {
            (
                vegetation::MAX_PLANT_RADIUS_CELLS as f64,
                vegetation::MAX_PLANT_HEIGHT_CELLS as f64,
            )
        } else {
            (8., 12.)
        };
        let center = glam::DVec3::new(
            p.root.x as f64,
            p.root.y as f64 + height * 0.5,
            p.root.z as f64,
        ) * VOXEL_SIZE;
        let bound = (2. * radius * radius + height * height * 0.25).sqrt() * VOXEL_SIZE;
        self.intersects_sphere(center, bound + 7. * VOXEL_SIZE * 3f64.sqrt())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Tile {
    species: u8,
    x: i64,
    z: i64,
}
enum Job {
    Tile(Tile, World, u64),
    Mesh(MeshKey, PlantRecord, World, u64),
    Ground(MeshKey, PlantRecord, World, u64),
}
enum Done {
    Tile(Tile, u64, Vec<PlantRecord>),
    Mesh(MeshKey, u64, Arc<VegetationMesh>),
}
fn species(s: u8) -> Species {
    match s {
        0 => Species::Grass,
        1 => Species::Edelweiss,
        _ => Species::Rowan,
    }
}
fn species_id(s: Species) -> u8 {
    match s {
        Species::Grass => 0,
        Species::Edelweiss => 1,
        Species::Rowan => 2,
    }
}
fn mix(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}
const TILE_CELLS: i64 = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
struct StableFrameKey {
    camera: [u64; 3],
    viewport_height: u32,
    fov_y: u32,
    world_revision: u64,
    config: [u64; 9],
    view: Option<[u64; 14]>,
}
impl StableFrameKey {
    fn new(
        camera: [f64; 3],
        viewport_height: u32,
        fov_y: f32,
        world_revision: u64,
        config: &VegetationConfig,
        view: Option<CullView>,
    ) -> Self {
        Self {
            camera: camera.map(f64::to_bits),
            viewport_height,
            fov_y: fov_y.to_bits(),
            world_revision,
            config: [
                config.grass_radius_m.to_bits(),
                config.flower_radius_m.to_bits(),
                config.tree_radius_m.to_bits(),
                u64::from(config.pixel_error.to_bits()),
                config.max_tiles as u64,
                config.max_meshes as u64,
                config.max_instances as u64,
                config.max_triangles as u64,
                u64::from(config.reference),
            ],
            view: view.map(|v| {
                [
                    v.eye.x,
                    v.eye.y,
                    v.eye.z,
                    v.forward.x,
                    v.forward.y,
                    v.forward.z,
                    v.right.x,
                    v.right.y,
                    v.right.z,
                    v.up.x,
                    v.up.y,
                    v.up.z,
                    v.tan_x,
                    v.tan_y,
                ]
                .map(f64::to_bits)
            }),
        }
    }
}

type GroundGroups = BTreeMap<(u8, i64, i64), Vec<PlantRecord>>;
#[derive(Default)]
struct StreamingPrediction {
    previous: Option<([f64; 3], Instant)>,
    velocity: [f64; 2],
    latencies: VecDeque<f64>,
}
impl StreamingPrediction {
    fn observe_latency(&mut self, start: Instant, now: Instant) {
        self.latencies
            .push_back(now.saturating_duration_since(start).as_secs_f64());
        if self.latencies.len() > 64 {
            self.latencies.pop_front();
        }
    }
    fn horizon(&self) -> f64 {
        let mut sorted: Vec<_> = self.latencies.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        // Includes queue, canonical work and result publication, plus one
        // conservative scheduling margin. Upload is budgeted separately by GPU.
        sorted
            .get((sorted.len().saturating_sub(1) * 95).div_ceil(100))
            .copied()
            .unwrap_or(0.25)
            + 0.1
    }
    fn update(&mut self, camera: [f64; 3], now: Instant) -> [f64; 2] {
        if let Some((previous, time)) = self.previous {
            let dt = now.saturating_duration_since(time).as_secs_f64();
            let delta = [camera[0] - previous[0], camera[2] - previous[2]];
            if dt > 0.00001 && dt < 2. && delta[0].hypot(delta[1]) < 256. {
                self.velocity = delta.map(|d| d / dt);
            } else {
                self.velocity = [0.; 2];
            }
        }
        self.previous = Some((camera, now));
        self.velocity.map(|v| v * self.horizon())
    }
    fn time_to_need(&self, delta: [f64; 2], radius: f64) -> f64 {
        let speed = self.velocity[0].hypot(self.velocity[1]);
        if speed < 0.01 {
            return f64::INFINITY;
        }
        let along = (delta[0] * self.velocity[0] + delta[1] * self.velocity[1]) / speed;
        let across2 = (delta[0] * delta[0] + delta[1] * delta[1] - along * along).max(0.);
        if across2 > radius * radius {
            return f64::INFINITY;
        }
        ((along - (radius * radius - across2).sqrt()) / speed).max(0.)
    }
}
pub struct VegetationStream {
    world: World,
    edits: Vec<Cell>,
    pub config: VegetationConfig,
    tx: Option<SyncSender<Job>>,
    rx: Receiver<(u64, Done)>,
    workers: Vec<thread::JoinHandle<()>>,
    job_counters: Arc<JobCounters>,
    epoch: Arc<AtomicU64>,
    content_epoch: u64,
    tiles: BTreeMap<Tile, (Vec<PlantRecord>, u64)>,
    meshes: BTreeMap<MeshKey, (Arc<VegetationMesh>, u64)>,
    mesh_costs: BTreeMap<MeshKey, (usize, u64)>,
    pending_tiles: BTreeSet<Tile>,
    pending_meshes: BTreeSet<MeshKey>,
    tile_started: BTreeMap<Tile, Instant>,
    mesh_started: BTreeMap<MeshKey, Instant>,
    prediction: StreamingPrediction,
    prediction_enabled: bool,
    tick: u64,
    center: Option<[i64; 2]>,
    stable_frame: Option<(StableFrameKey, VegetationFrame)>,
    previous_lods: BTreeMap<MeshKey, u8>,
    shadow_lods: BTreeMap<MeshKey, u8>,
    shadow_lifecycle: ShadowLifecycle,
    shadow_transitions_enabled: bool,
    grouped_ground: Option<(Vec<Tile>, GroundGroups)>,
    #[cfg(test)]
    regroup_builds: usize,
}
impl VegetationStream {
    pub fn new(world: World, config: VegetationConfig) -> Self {
        // Read once when the stream is created, never inside frame selection.
        let reference_flag = std::env::var("FRONTIER_VEGETATION_REFERENCE").ok();
        let config = config.with_reference_flag(reference_flag.as_deref());
        // Leave CPU capacity for terrain and the render thread; memory remains
        // bounded by queue depth plus at most four active canonical models.
        let default_workers =
            thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(2).clamp(1, 4));
        let worker_count = std::env::var("FRONTIER_VEGETATION_WORKERS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| (1..=4).contains(n))
            .unwrap_or(default_workers);
        let (tx, jobs) = mpsc::sync_channel::<Job>(worker_count * 2);
        let jobs = Arc::new(Mutex::new(jobs));
        let (results, rx) = mpsc::sync_channel(worker_count * 2);
        let job_counters = Arc::new(JobCounters::default());
        let epoch = Arc::new(AtomicU64::new(1));
        let workers = (0..worker_count)
            .map(|index| {
                let cancel = epoch.clone();
                let jobs = jobs.clone();
                let results = results.clone();
                let counters = job_counters.clone();
                thread::Builder::new()
                    .name(format!("vegetation-stream-{index}"))
                    .spawn(move || {
                        loop {
                            // Only dequeue is serialized. No model/mesh generation owns
                            // the receiver lock, so independent jobs run concurrently.
                            let job = { jobs.lock().expect("vegetation queue").recv() };
                            let Ok(job) = job else { break };
                            let kind = match &job {
                                Job::Tile(..) => 0,
                                Job::Mesh(..) => 1,
                                Job::Ground(..) => 2,
                            };
                            let mut timing = JobTimer::new(&counters, kind);
                            let generation = match &job {
                                Job::Tile(_, _, e)
                                | Job::Mesh(_, _, _, e)
                                | Job::Ground(_, _, _, e) => *e,
                            };
                            if generation != cancel.load(Ordering::Relaxed) {
                                continue;
                            }
                            let done = match job {
                                Job::Tile(t, w, _) => {
                                    let min = [t.x * TILE_CELLS, t.z * TILE_CELLS];
                                    let max = [min[0] + TILE_CELLS, min[1] + TILE_CELLS];
                                    Done::Tile(
                                        t,
                                        w.revision(),
                                        w.vegetation_records_species(min, max, species(t.species)),
                                    )
                                }
                                Job::Mesh(key, p, w, _) => {
                                    let assembly_start = Instant::now();
                                    let Some(model) = vegetation::model_for_cancellable(&p, || {
                                        generation != cancel.load(Ordering::Relaxed)
                                    }) else {
                                        continue;
                                    };
                                    let edited: BTreeSet<Cell> = if key.edit_id == 0 {
                                        BTreeSet::new()
                                    } else {
                                        w.edit_cells().collect()
                                    };
                                    let cells = model
                                        .voxels
                                        .iter()
                                        .filter(|v| !edited.contains(&p.world_cell(v.cell)))
                                        .map(|v| (v.cell, v.material as u8, v.color));
                                    counters.assembly_nanos[kind].fetch_add(
                                        assembly_start.elapsed().as_nanos() as u64,
                                        Ordering::Relaxed,
                                    );
                                    let mesh_start = Instant::now();
                                    let mut mesh =
                                        mesh_plant_cells(p.species, cells, key.lod, || {
                                            generation != cancel.load(Ordering::Relaxed)
                                        });
                                    // Bound resident payload by content size, not Vec's
                                    // geometric growth slack left during face extraction.
                                    mesh.vertices.shrink_to_fit();
                                    mesh.indices.shrink_to_fit();
                                    counters.mesh_nanos[kind].fetch_add(
                                        mesh_start.elapsed().as_nanos() as u64,
                                        Ordering::Relaxed,
                                    );
                                    Done::Mesh(key, w.revision(), Arc::new(mesh))
                                }
                                Job::Ground(key, p, w, _) => {
                                    let assembly_start = Instant::now();
                                    // Include neighboring roots, then assign every native
                                    // cell to exactly one tile. Source query is independent
                                    // of which placement neighbors are currently resident.
                                    let records = w.vegetation_records_species(
                                        [p.root.x - 8, p.root.z - 8],
                                        [p.root.x + 136, p.root.z + 136],
                                        p.species,
                                    );
                                    let edited: BTreeSet<Cell> = w.edit_cells().collect();
                                    let Some(cells) =
                                        ground_cells_cached(&records, p.root, &edited, || {
                                            generation != cancel.load(Ordering::Relaxed)
                                        })
                                    else {
                                        continue;
                                    };
                                    counters.assembly_nanos[kind].fetch_add(
                                        assembly_start.elapsed().as_nanos() as u64,
                                        Ordering::Relaxed,
                                    );
                                    let mesh_start = Instant::now();
                                    let mut mesh = mesh_ground_cells(
                                        cells.iter().map(|(c, (m, rgb))| (*c, *m, *rgb)),
                                        key.lod,
                                        || generation != cancel.load(Ordering::Relaxed),
                                    );
                                    // Bound resident payload by content size, not Vec's
                                    // geometric growth slack left during face extraction.
                                    mesh.vertices.shrink_to_fit();
                                    mesh.indices.shrink_to_fit();
                                    counters.mesh_nanos[kind].fetch_add(
                                        mesh_start.elapsed().as_nanos() as u64,
                                        Ordering::Relaxed,
                                    );
                                    Done::Mesh(key, w.revision(), Arc::new(mesh))
                                }
                            };
                            if generation != cancel.load(Ordering::Relaxed) {
                                continue;
                            }
                            // Never block cancellation behind a stalled renderer/full result queue.
                            timing.completed = true;
                            // This timer measures CPU generation, excluding backpressure.
                            drop(timing);
                            let mut output = (generation, done);
                            loop {
                                match results.try_send(output) {
                                    Ok(()) => break,
                                    Err(mpsc::TrySendError::Full(v)) => {
                                        output = v;
                                        if generation != cancel.load(Ordering::Relaxed) {
                                            break;
                                        }
                                        thread::park_timeout(std::time::Duration::from_millis(1));
                                    }
                                    Err(_) => return,
                                }
                            }
                        }
                    })
                    .expect("vegetation worker")
            })
            .collect();
        Self {
            edits: world.edit_cells().collect(),
            world,
            config,
            tx: Some(tx),
            rx,
            workers,
            job_counters,
            epoch,
            content_epoch: 1,
            tiles: BTreeMap::new(),
            meshes: BTreeMap::new(),
            mesh_costs: BTreeMap::new(),
            pending_tiles: BTreeSet::new(),
            pending_meshes: BTreeSet::new(),
            tile_started: BTreeMap::new(),
            mesh_started: BTreeMap::new(),
            prediction: StreamingPrediction::default(),
            prediction_enabled: std::env::var("FRONTIER_VEGETATION_PREDICTION").as_deref()
                != Ok("off"),
            tick: 0,
            center: None,
            stable_frame: None,
            previous_lods: BTreeMap::new(),
            shadow_lods: BTreeMap::new(),
            shadow_lifecycle: ShadowLifecycle::default(),
            shadow_transitions_enabled: !matches!(
                std::env::var("FRONTIER_SHADOW_TRANSITIONS").as_deref(),
                Ok("off" | "0")
            ),
            grouped_ground: None,
            #[cfg(test)]
            regroup_builds: 0,
        }
    }
    /// Replace the immutable snapshot after edits; no stale plant mesh survives.
    pub fn set_world(&mut self, world: World) {
        self.stable_frame = None;
        self.previous_lods.clear();
        self.shadow_lods.clear();
        self.shadow_lifecycle.clear();
        self.grouped_ground = None;
        self.edits = world.edit_cells().collect();
        self.world = world;
        self.content_epoch = self.epoch.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        self.tiles.clear();
        // Unedited individual models are independent of terrain/root height.
        // Ground tiles also depend on source placement/height, so invalidate
        // them even when a replacement source happens to share its revision.
        self.meshes.retain(|k, _| k.species == 2 && k.edit_id == 0);
        self.mesh_costs
            .retain(|k, _| k.species == 2 && k.edit_id == 0);
        self.pending_tiles.clear();
        self.pending_meshes.clear();
        self.center = None;
        self.tile_started.clear();
        self.mesh_started.clear();
        self.prediction = StreamingPrediction::default();
    }
    pub fn update(
        &mut self,
        camera: [f64; 3],
        viewport_height: u32,
        fov_y: f32,
        world_revision: u64,
    ) -> VegetationFrame {
        self.update_inner(camera, viewport_height, fov_y, world_revision, None)
    }
    /// Frustum culling affects display selection only; all-heading placement
    /// demand remains warm for camera turns. FOV is vertical radians.
    pub fn update_view(
        &mut self,
        camera: [f64; 3],
        viewport_height: u32,
        fov_y: f32,
        world_revision: u64,
        forward: [f64; 3],
        aspect: f32,
    ) -> VegetationFrame {
        self.update_inner(
            camera,
            viewport_height,
            fov_y,
            world_revision,
            CullView::new(camera, forward, fov_y, aspect),
        )
    }
    fn update_inner(
        &mut self,
        camera: [f64; 3],
        viewport_height: u32,
        fov_y: f32,
        world_revision: u64,
        view: Option<CullView>,
    ) -> VegetationFrame {
        let now = Instant::now();
        let mut lead = self.prediction.update(camera, now);
        if !self.prediction_enabled {
            lead = [0.; 2];
            self.prediction.velocity = [0.; 2];
        }
        let key = StableFrameKey::new(
            camera,
            viewport_height,
            fov_y,
            world_revision,
            &self.config,
            view,
        );
        // Release retained frame Arcs before changed demand can evict geometry.
        if self
            .stable_frame
            .as_ref()
            .is_some_and(|(old, _)| old != &key)
        {
            self.stable_frame = None;
        }
        let c = Cell::from_meters(camera);
        let center = [c.x.div_euclid(TILE_CELLS), c.z.div_euclid(TILE_CELLS)];
        if self.center != Some(center) {
            self.center = Some(center);
            self.epoch.fetch_add(1, Ordering::Relaxed);
            self.pending_tiles.clear();
            self.pending_meshes.clear();
            self.tile_started.clear();
            self.mesh_started.clear();
        }
        let mut drained = false;
        while let Ok((generation, done)) = self.rx.try_recv() {
            // Camera epochs can reuse content. A new immutable source snapshot
            // cannot consume completions from the preceding snapshot, even if
            // their externally supplied revision numbers happen to match.
            if generation < self.content_epoch {
                continue;
            }
            let current_generation = generation == self.epoch.load(Ordering::Relaxed);
            drained = true;
            self.stable_frame = None;
            match done {
                Done::Tile(t, r, p) => {
                    if t.species != 2 {
                        self.grouped_ground = None;
                    }
                    if current_generation {
                        self.pending_tiles.remove(&t);
                        if let Some(start) = self.tile_started.remove(&t) {
                            self.prediction.observe_latency(start, now);
                        }
                    }
                    if r == self.world.revision() {
                        self.tiles.insert(t, (p, self.tick.wrapping_add(1)));
                    }
                }
                Done::Mesh(k, r, m) => {
                    if current_generation {
                        self.pending_meshes.remove(&k);
                        if let Some(start) = self.mesh_started.remove(&k) {
                            self.prediction.observe_latency(start, now);
                        }
                    }
                    if r == self.world.revision() {
                        self.mesh_costs
                            .insert(k, (m.indices.len() / 3, self.tick.wrapping_add(1)));
                        self.meshes.insert(k, (m, self.tick.wrapping_add(1)));
                    }
                }
            }
        }
        if !drained && self.pending_tiles.is_empty() && self.pending_meshes.is_empty() {
            if let Some((old, frame)) = &self.stable_frame {
                if old == &key {
                    let mut cached = frame.clone();
                    cached.stats.jobs = self.job_counters.snapshot(self.workers.len());
                    cached.stats.tree_lod_changes = 0;
                    cached.stats.selected_mesh_evictions = 0;
                    cached.stats.selected_mesh_evicted_bytes = 0;
                    cached.stats.shadow_lod_changes = 0;
                    cached.stats.tree_refinements = 0;
                    cached.stats.tree_coarsenings = 0;
                    cached.stats.prediction_horizon_ms = self.prediction.horizon() * 1000.;
                    cached.stats.prediction_lead_m = lead[0].hypot(lead[1]);
                    return cached;
                }
            }
        }
        // LRU age is a selection-build epoch. Idle cache hits neither age
        // demanded geometry nor make unused content artificially recent.
        self.tick = self.tick.wrapping_add(1);
        #[cfg(test)]
        {
            self.regroup_builds += 1;
        }
        let mut stats = VegetationStats {
            shadow_lifecycle_enabled: self.shadow_transitions_enabled,
            prediction_horizon_ms: self.prediction.horizon() * 1000.,
            prediction_lead_m: lead[0].hypot(lead[1]),
            reference: self.config.reference,
            jobs: self.job_counters.snapshot(self.workers.len()),
            mesh_budget_bytes: if self.config.reference {
                768 * 1024 * 1024
            } else {
                160 * 1024 * 1024
            },
            gpu_budget_bytes: if self.config.reference {
                1024 * 1024 * 1024
            } else {
                256 * 1024 * 1024
            },
            ..Default::default()
        };
        let mut batches: BTreeMap<MeshKey, VegetationBatch> = BTreeMap::new();
        // A revision without its new immutable snapshot must never resurrect edits.
        if world_revision != self.world.revision() {
            stats.pending_jobs = 1;
            return VegetationFrame {
                origin: [c.x, c.y, c.z],
                batches: Vec::new(),
                shadow_batches: Vec::new(),
                shadow_transitions: Vec::new(),
                shadow_coverage: Vec::new(),
                shadow_inspection: Vec::new(),
                stats,
            };
        }
        let radii = [
            self.config.grass_radius_m,
            self.config.flower_radius_m,
            self.config.tree_radius_m,
        ];
        let mut demand = Vec::new();
        for s in 0..3u8 {
            let radius = radii[s as usize].clamp(0., 512.);
            // Predict placement only. Mesh generation still follows final
            // budget admission below, preventing speculative variant churn.
            // Bound enumeration by the existing tile budget even at aircraft speed.
            let bounded_lead = lead.map(|v| {
                v.clamp(
                    -(self.config.max_tiles as f64).sqrt() * 32.,
                    (self.config.max_tiles as f64).sqrt() * 32.,
                )
            });
            let n = ((radius + bounded_lead[0].hypot(bounded_lead[1])) / 32.).ceil() as i64 + 1;
            for dz in -n..=n {
                for dx in -n..=n {
                    let t = Tile {
                        species: s,
                        x: center[0] + dx,
                        z: center[1] + dz,
                    };
                    let x = (t.x as f64 + 0.5) * 32.;
                    let z = (t.z as f64 + 0.5) * 32.;
                    let d = ((x - camera[0]).powi(2) + (z - camera[2]).powi(2)).sqrt();
                    let delta = [x - camera[0], z - camera[2]];
                    let lead2 = bounded_lead[0].powi(2) + bounded_lead[1].powi(2);
                    let along = if lead2 > 0. {
                        ((delta[0] * bounded_lead[0] + delta[1] * bounded_lead[1]) / lead2)
                            .clamp(0., 1.)
                    } else {
                        0.
                    };
                    let swept = (delta[0] - bounded_lead[0] * along)
                        .hypot(delta[1] - bounded_lead[1] * along);
                    if swept <= radius + 23. {
                        let deadline = if d <= radius + 23. {
                            0.
                        } else {
                            self.prediction
                                .time_to_need([x - camera[0], z - camera[2]], radius + 23.)
                        };
                        demand.push((deadline, d, t));
                    }
                }
            }
        }
        demand.sort_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then(a.1.total_cmp(&b.1))
                .then(a.2.cmp(&b.2))
        });
        demand.truncate(self.config.max_tiles);
        stats.predicted_tiles = demand
            .iter()
            .filter(|(deadline, _, _)| *deadline > 0.)
            .count();
        let epoch = self.epoch.load(Ordering::Relaxed);
        let mut submitted = 0;
        // (coverage/refinement, fair class, distance, key, source, ground).
        let mut mesh_requests = Vec::new();
        let mut plants = Vec::new();
        let mut offscreen = Vec::new();
        let mut group_key: Vec<_> = demand
            .iter()
            .map(|(_, _, t)| *t)
            .filter(|t| t.species != 2 && self.tiles.contains_key(t))
            .collect();
        group_key.sort_unstable();
        let previous = self
            .grouped_ground
            .take()
            .filter(|(key, _)| key == &group_key);
        let ground_reused = previous.is_some();
        let mut ground = previous.map_or_else(BTreeMap::new, |(_, groups)| groups);
        for (_, _, t) in demand {
            if let Some((records, used)) = self.tiles.get_mut(&t) {
                *used = self.tick;
                for p in records.iter() {
                    if p.species != Species::Rowan {
                        if ground_reused {
                            continue;
                        }
                        for x in (p.root.x - 8).div_euclid(128)..=(p.root.x + 8).div_euclid(128) {
                            for z in (p.root.z - 8).div_euclid(128)..=(p.root.z + 8).div_euclid(128)
                            {
                                ground.entry((t.species, x, z)).or_default().push(*p);
                            }
                        }
                        continue;
                    }
                    let dx = (p.root.x - c.x) as f64 * VOXEL_SIZE;
                    let dz = (p.root.z - c.z) as f64 * VOXEL_SIZE;
                    let distance = (dx * dx + dz * dz).sqrt();
                    if distance < radii[t.species as usize] {
                        if view.as_ref().is_none_or(|v| v.plant_visible(p)) {
                            plants.push((distance, *p));
                        } else {
                            stats.culled_instances += 1;
                            if distance < 96. {
                                offscreen.push((distance, *p));
                            }
                        }
                    }
                }
            } else {
                stats.missing_tiles += 1;
                if submitted < self.workers.len()
                    && !self.pending_tiles.contains(&t)
                    && self
                        .tx
                        .as_ref()
                        .unwrap()
                        .try_send(Job::Tile(t, self.world.clone(), epoch))
                        .is_ok()
                {
                    self.pending_tiles.insert(t);
                    self.tile_started.insert(t, now);
                    submitted += 1;
                }
            }
        }
        for (&(s, x, z), records) in &ground {
            let dx = (x as f64 + 0.5) * 8. - camera[0];
            let dz = (z as f64 + 0.5) * 8. - camera[2];
            let distance = (dx * dx + dz * dz).sqrt();
            if distance > radii[s as usize] + 5.7 {
                continue;
            }
            if view
                .as_ref()
                .is_some_and(|v| !records.iter().any(|p| v.plant_visible(p)))
            {
                continue;
            }
            let p = PlantRecord {
                species: species(s),
                variant: u8::MAX,
                seed: mix(self.world.seed()
                    ^ mix(x as u64)
                    ^ (z as u64).rotate_left(31)
                    ^ s as u64),
                root: Cell {
                    x: x * 128,
                    y: 0,
                    z: z * 128,
                },
                rotation: 0,
            };
            plants.push((distance, p));
        }
        self.grouped_ground = Some((group_key, ground));
        // Preserve tall silhouettes before spending the remaining geometry
        // budget on small repeated ground cover. Within each class, nearest wins.
        plants.sort_by(|a, b| {
            let priority = |s| match s {
                Species::Rowan => 0,
                Species::Edelweiss => 1,
                Species::Grass => 2,
            };
            priority(a.1.species)
                .cmp(&priority(b.1.species))
                .then(a.0.total_cmp(&b.0))
                .then(a.1.root.cmp(&b.1.root))
        });
        let edits = &self.edits;
        let projection = viewport_height as f64 / (2. * (fov_y as f64 * 0.5).tan().max(0.01));
        let mut remaining_ground_coarse = 0usize;
        let mut ground_reservations = BTreeMap::new();
        let mut ground_coverage_ready = true;
        if !self.config.reference {
            for (_, p) in plants.iter().filter(|(_, p)| p.species != Species::Rowan) {
                let key = ground_key(
                    p,
                    8,
                    if edits.is_empty() {
                        0
                    } else {
                        self.world.revision().max(1)
                    },
                );
                let current = resident_coverage_key(
                    &self.meshes,
                    key,
                    self.previous_lods.get(&tree_identity(key)).copied(),
                );
                if let Some(current) = current {
                    let (mesh, used) = self.meshes.get_mut(&current).unwrap();
                    *used = self.tick;
                    let cost = mesh.indices.len() / 3;
                    ground_reservations.insert(tree_identity(key), cost);
                    remaining_ground_coarse += cost;
                } else {
                    ground_coverage_ready = false;
                }
            }
        }
        // Reserve cheap geometry for every visible tree before any near tree
        // spends the refinement budget. Baseline templates have priority over
        // fine requests, and their actual costs replace speculative estimates.
        let mut remaining_tree_coarse = 0usize;
        let mut tree_reservations = BTreeMap::new();
        let mut tree_current_costs = Vec::new();
        let tree_budget = self.config.max_triangles.saturating_mul(65) / 100;
        let mut tree_coverage_ready = true;
        for (_, p) in plants
            .iter()
            .filter(|(_, p)| !self.config.reference && p.species == Species::Rowan)
        {
            let key = MeshKey {
                species: 2,
                variant: p.variant,
                seed: p.seed,
                tile: [0; 2],
                lod: 8,
                edit_id: plant_edit_id(p, edits, self.world.revision()),
            };
            let current = resident_coverage_key(
                &self.meshes,
                key,
                self.previous_lods.get(&tree_identity(key)).copied(),
            );
            if let Some(current) = current {
                let (mesh, used) = self.meshes.get_mut(&current).unwrap();
                *used = self.tick;
                let cost = mesh.indices.len() / 3;
                // Only detail previously displayed by this stream owns
                // a retention reservation. A new candidate's finest cached
                // mesh must compete for residual room after retained owners.
                if self.previous_lods.get(&tree_identity(key)) == Some(&current.lod) {
                    tree_current_costs.push((tree_identity(key), cost));
                }
                // First preserve the cheapest already resident representation
                // for every feasible tree. Four direct lookups avoid scanning
                // the cache or requiring historical LOD8 meshes to be rebuilt.
                let baseline = [1, 2, 4, 8]
                    .into_iter()
                    .filter_map(|lod| {
                        self.meshes
                            .get(&MeshKey { lod, ..key })
                            .map(|(m, _)| m.indices.len() / 3)
                    })
                    .min()
                    .unwrap();
                let room = tree_budget.saturating_sub(remaining_tree_coarse);
                let reserved = if baseline <= room { baseline } else { 0 };
                tree_reservations.insert(tree_identity(key), reserved);
                remaining_tree_coarse = remaining_tree_coarse.saturating_add(reserved);
            } else {
                tree_coverage_ready = false;
            }
        }
        // Spend only the residual allocation retaining current detail, nearest
        // first. Near refinement cannot consume a farther tree's cheap cover.
        for (identity, current) in tree_current_costs {
            let baseline = tree_reservations[&identity];
            let extra = current.saturating_sub(baseline);
            if extra <= tree_budget.saturating_sub(remaining_tree_coarse) {
                tree_reservations.insert(identity, current);
                remaining_tree_coarse = remaining_tree_coarse.saturating_add(extra);
            }
        }
        // Shadow quality is independent of color refinement: a native color
        // mesh must never multiply a tree's shadow-budget cost during a turn.
        let mut shadow_plants: Vec<_> = plants
            .iter()
            .filter(|(d, p)| p.species == Species::Rowan && *d < 96.)
            .map(|(d, p)| (*d, *p, false))
            .chain(offscreen.into_iter().map(|(d, p)| (d, p, true)))
            .collect();
        // Heading changes only color visibility, never caster membership.
        // Retained casters share the GPU's 90m entry / 96m exit band.
        shadow_plants.retain(|(distance, p, _)| {
            *distance < 90.
                || self.shadow_lods.contains_key(&MeshKey {
                    species: 2,
                    variant: p.variant,
                    seed: p.seed,
                    tile: [0; 2],
                    lod: 0,
                    edit_id: plant_edit_id(p, edits, self.world.revision()),
                })
        });
        shadow_plants.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.root.cmp(&b.1.root)));
        shadow_plants.truncate(self.config.max_instances.min(256));
        for (distance, p) in plants {
            let s = species_id(p.species);
            let radius = radii[s as usize];
            let fade = ((radius + if s == 2 { 0. } else { 5.7 } - distance)
                / (radius * 0.2).max(1.))
            .clamp(0., 1.) as f32;
            if fade <= 0. {
                continue;
            }
            stats.demanded_by_species[s as usize] += 1;
            let mut lod = selected_stride(distance, projection, &self.config);
            // More conservative tree contour than ground cover.
            if s == 2 {
                lod = lod.min(4);
            }
            let edit_id = if s == 2 {
                plant_edit_id(&p, edits, self.world.revision())
            } else if edits.is_empty() {
                0
            } else {
                self.world.revision().max(1)
            };
            let mut key = MeshKey {
                species: s,
                variant: p.variant,
                seed: p.seed,
                tile: if s == 2 {
                    [0; 2]
                } else {
                    [p.root.x.div_euclid(128), p.root.z.div_euclid(128)]
                },
                lod,
                edit_id,
            };
            let previous = self.previous_lods.get(&MeshKey { lod: 0, ..key }).copied();
            if s == 2 && !self.config.reference {
                key.lod = hysteretic_stride(
                    key.lod,
                    selected_stride(distance * 0.9, projection, &self.config).min(4),
                    previous,
                );
            }
            let coarse_key = MeshKey { lod: 8, ..key };
            let ground_headroom = if s != 2 && !self.config.reference {
                remaining_ground_coarse = remaining_ground_coarse.saturating_sub(
                    ground_reservations
                        .get(&tree_identity(key))
                        .copied()
                        .unwrap_or(0),
                );
                self.config
                    .max_triangles
                    .saturating_sub(stats.triangles)
                    .saturating_sub(remaining_ground_coarse)
                    .min(if s == 1 {
                        (self.config.max_triangles / 10)
                            .saturating_sub(stats.triangles_by_species[1])
                    } else {
                        usize::MAX
                    })
            } else {
                usize::MAX
            };
            let tree_headroom = if s == 2 && !self.config.reference {
                remaining_tree_coarse = remaining_tree_coarse.saturating_sub(
                    tree_reservations
                        .get(&tree_identity(key))
                        .copied()
                        .unwrap_or(0),
                );
                tree_refinement_headroom(
                    self.config.max_triangles,
                    stats.triangles_by_species[2],
                    remaining_tree_coarse,
                )
            } else {
                usize::MAX
            };
            // If fine geometry costs too much, automatically request the next
            // coarser stride. Native desired LOD still drives the quality target.
            let ground_target = if self.config.reference {
                1
            } else {
                hysteretic_stride(
                    ground_stride(distance, projection, &self.config),
                    ground_stride(distance * 0.9, projection, &self.config),
                    previous,
                )
            };
            let mut request_key = key;
            if s == 2 && !self.config.reference {
                if !tree_coverage_ready {
                    request_key = resident_coverage_key(&self.meshes, coarse_key, previous)
                        .unwrap_or(coarse_key);
                } else {
                    while request_key.lod < 8 {
                        match self.mesh_costs.get_mut(&request_key) {
                            Some((cost, used))
                                if !tree_lod_admitted(
                                    request_key.lod,
                                    previous,
                                    *cost,
                                    tree_headroom,
                                ) =>
                            {
                                *used = self.tick;
                                request_key.lod *= 2
                            }
                            _ => break,
                        }
                    }
                }
            }
            if s != 2 && !self.config.reference {
                request_key =
                    resident_coverage_key(&self.meshes, coarse_key, previous).unwrap_or(coarse_key);
                if ground_coverage_ready && request_key.lod < ground_target {
                    request_key.lod = ground_target;
                }
                if ground_coverage_ready {
                    // Refine one automatically derived level at a time, and only
                    // if its measured/estimated cost leaves every later tile's
                    // actual baseline intact. Rejected native work is not queued.
                    while request_key.lod > ground_target {
                        let next = MeshKey {
                            lod: request_key.lod / 2,
                            ..key
                        };
                        if let Some((cost, used)) = self.mesh_costs.get_mut(&next) {
                            *used = self.tick;
                            if *cost > ground_headroom {
                                break;
                            }
                        }
                        if let Some((mesh, _)) = self.meshes.get(&next) {
                            if mesh.indices.len() / 3 <= ground_headroom {
                                request_key = next;
                            } else {
                                break;
                            }
                        } else {
                            let current = self
                                .meshes
                                .get(&request_key)
                                .map_or(usize::MAX, |(m, _)| m.indices.len() / 3);
                            if current.saturating_mul(4) <= ground_headroom {
                                request_key = next;
                            }
                            break;
                        }
                    }
                }
            }
            let headroom = if s == 2 {
                tree_headroom
            } else {
                ground_headroom
            };
            if !self.meshes.contains_key(&request_key)
                && self
                    .mesh_costs
                    .get(&request_key)
                    .is_none_or(|(cost, _)| *cost <= headroom)
            {
                stats.missing_meshes += 1;
                let coverage = resident_coverage_key(&self.meshes, key, previous).is_none();
                mesh_requests.push((
                    u8::from(!coverage),
                    if s == 2 { 0 } else { 1 },
                    distance,
                    request_key,
                    p,
                    s != 2,
                ));
            }
            let render_key = if self.config.reference {
                // A missing native mesh stays pending; never substitute an LOD
                // proxy in the reference target.
                key
            } else if s == 2 {
                let candidate = self
                    .meshes
                    .iter()
                    .filter(|(k, (mesh, _))| {
                        k.species == 2
                            && k.variant == key.variant
                            && k.seed == key.seed
                            && k.tile == key.tile
                            && k.edit_id == key.edit_id
                            && k.lod >= key.lod
                            && tree_lod_admitted(
                                k.lod,
                                previous,
                                mesh.indices.len() / 3,
                                tree_headroom,
                            )
                    })
                    .min_by_key(|(k, _)| k.lod)
                    .map(|(k, _)| *k);
                let current = previous.and_then(|lod| {
                    let old = MeshKey { lod, ..key };
                    self.meshes
                        .get(&old)
                        .filter(|(m, _)| m.indices.len() / 3 <= tree_headroom)
                        .map(|_| old)
                });
                retain_current_mesh(candidate, current, key.lod).unwrap_or(coarse_key)
            } else {
                let candidate = self
                    .meshes
                    .iter()
                    .filter(|(k, (m, _))| {
                        k.species == key.species
                            && k.seed == key.seed
                            && k.tile == key.tile
                            && k.edit_id == key.edit_id
                            // Generation priority is not display quality. A new
                            // neighbor may need a coarse baseline, but that must
                            // not hide affordable resident detail on this tile.
                            && k.lod >= ground_target
                            && m.indices.len() / 3 <= ground_headroom
                    })
                    .min_by_key(|(k, _)| k.lod)
                    .map(|(k, _)| *k);
                let current = previous.and_then(|lod| {
                    let old = MeshKey { lod, ..key };
                    self.meshes
                        .get(&old)
                        .filter(|(m, _)| m.indices.len() / 3 <= ground_headroom)
                        .map(|_| old)
                });
                retain_current_mesh(candidate, current, ground_target).unwrap_or(coarse_key)
            };
            if let Some((mesh, used)) = self.meshes.get_mut(&render_key) {
                stats.ready_by_species[s as usize] += 1;
                let tris = mesh.indices.len() / 3;
                if !budget_allows(&self.config, &stats, s as usize, tris) {
                    if s == 2 {
                        stats.coverage_unresolved += 1;
                    }
                    stats.budget_rejected += 1;
                    stats.reference_budget_exceeded |= self.config.reference;
                    stats.rejected_by_species[s as usize] += 1;
                    continue;
                }
                *used = self.tick;
                if s != 2
                    && (render_key.lod < request_key.lod
                        || (!ground_coverage_ready
                            && previous == Some(render_key.lod)
                            && render_key.lod < 8))
                {
                    stats.retained_ground_detail += 1;
                }
                if s == 2 && previous == Some(render_key.lod) && render_key.lod < request_key.lod {
                    stats.retained_tree_detail += 1;
                }
                if s == 2 && render_key.lod > key.lod {
                    stats.coverage_fallbacks += 1;
                }
                stats.selected_instances += 1;
                stats.triangles += tris;
                stats.selected_by_species[s as usize] += 1;
                stats.triangles_by_species[s as usize] += tris;
                batches
                    .entry(render_key)
                    .or_insert_with(|| VegetationBatch {
                        key: render_key,
                        mesh: mesh.clone(),
                        instances: Vec::new(),
                    })
                    .instances
                    .push(VegetationInstance {
                        root_cell: [p.root.x, p.root.y, p.root.z],
                        quarter_turn: p.rotation as u32,
                        fade,
                    });
            }
        }
        // Reserve current caster costs before admitting refinements. Both
        // heading directions share this history; color LOD never enters it.
        let mut shadow_remaining = 0usize;
        let mut shadow_coverage_ready = true;
        let mut shadow_reservations = BTreeMap::new();
        for (_, p, _) in &shadow_plants {
            let identity = MeshKey {
                species: 2,
                variant: p.variant,
                seed: p.seed,
                tile: [0; 2],
                lod: 0,
                edit_id: plant_edit_id(p, edits, self.world.revision()),
            };
            let current = self
                .shadow_lods
                .get(&identity)
                .copied()
                .map(|lod| MeshKey { lod, ..identity })
                .filter(|key| self.meshes.contains_key(key))
                .or_else(|| {
                    self.meshes
                        .contains_key(&MeshKey { lod: 8, ..identity })
                        .then_some(MeshKey { lod: 8, ..identity })
                });
            shadow_coverage_ready &= current.is_some();
            let cost = current
                .and_then(|key| self.meshes.get(&key))
                .map_or(0, |(m, _)| m.indices.len() / 3);
            // Candidates are nearest first. Reserve feasible coverage before
            // refinement, without allowing unadmitted far baselines to consume
            // the nearest tree's allocation. The lifecycle separately charges
            // real old/new overlap and defers a handoff when no room remains.
            let reserved = if cost <= TREE_SHADOW_TRIANGLE_BUDGET.saturating_sub(shadow_remaining) {
                cost
            } else {
                0
            };
            shadow_reservations.insert(identity, reserved);
            shadow_remaining = shadow_remaining.saturating_add(reserved);
        }
        let mut shadow_batches = Vec::new();
        let mut shadow_inspection = Vec::new();
        let mut shadow_used = 0usize;
        for (distance, p, offscreen) in shadow_plants {
            let identity = MeshKey {
                species: 2,
                variant: p.variant,
                seed: p.seed,
                tile: [0; 2],
                lod: 0,
                edit_id: plant_edit_id(&p, edits, self.world.revision()),
            };
            let previous = self.shadow_lods.get(&identity).copied();
            let target = shadow_stride(distance, previous, self.config.reference);
            shadow_remaining = shadow_remaining.saturating_sub(shadow_reservations[&identity]);
            let room = TREE_SHADOW_TRIANGLE_BUDGET
                .saturating_sub(shadow_used)
                .saturating_sub(shadow_remaining);
            let mut request = MeshKey {
                lod: if shadow_coverage_ready {
                    target
                } else {
                    previous
                        .filter(|&lod| self.meshes.contains_key(&MeshKey { lod, ..identity }))
                        .unwrap_or(8)
                },
                ..identity
            };
            while request.lod < 8
                && self
                    .mesh_costs
                    .get(&request)
                    .is_some_and(|(cost, _)| !tree_lod_admitted(request.lod, previous, *cost, room))
            {
                request.lod *= 2;
            }
            let request_affordable = self
                .mesh_costs
                .get(&request)
                .is_none_or(|(cost, _)| tree_lod_admitted(request.lod, previous, *cost, room));
            if !self.meshes.contains_key(&request) && request_affordable {
                stats.missing_meshes += 1;
                mesh_requests.push((
                    u8::from(shadow_reservations[&identity] > 0),
                    2,
                    distance,
                    request,
                    p,
                    false,
                ));
            }
            if !request_affordable {
                stats.shadow_budget_rejected += 1;
            }
            let affordable = |k: &MeshKey, m: &VegetationMesh| {
                tree_lod_admitted(k.lod, previous, m.indices.len() / 3, room)
            };
            let candidate = self
                .meshes
                .iter()
                .filter(|(k, (m, _))| {
                    tree_identity(**k) == identity
                        && k.lod
                            >= if shadow_coverage_ready {
                                target
                            } else {
                                previous.unwrap_or(8)
                            }
                        && (self.config.reference || k.lod >= 2)
                        && affordable(k, m)
                })
                .min_by_key(|(k, _)| k.lod)
                .map(|(k, _)| *k);
            let current = previous.and_then(|lod| {
                let key = MeshKey { lod, ..identity };
                self.meshes
                    .get(&key)
                    .filter(|(m, _)| affordable(&key, m))
                    .map(|_| key)
            });
            let selected = retain_current_mesh(candidate, current, target);
            shadow_inspection.push(ShadowInspection {
                root_cell: [p.root.x, p.root.y, p.root.z],
                selected,
                requested: request,
                reason: if !request_affordable {
                    ShadowReason::BudgetRejected
                } else if selected.is_none_or(|k| k != request)
                    && !self.meshes.contains_key(&request)
                {
                    ShadowReason::ReplacementPending
                } else if previous.is_some_and(|lod| selected.is_some_and(|k| k.lod == lod)) {
                    ShadowReason::Retained
                } else {
                    ShadowReason::Admitted
                },
                triangles: selected
                    .and_then(|k| self.meshes.get(&k))
                    .map_or(0, |(m, _)| m.indices.len() / 3),
            });
            if let Some(selected) = selected {
                let (mesh, used) = self.meshes.get_mut(&selected).unwrap();
                *used = self.tick;
                let triangles = mesh.indices.len() / 3;
                shadow_used += triangles;
                stats.explicit_shadow_instances += 1;
                stats.explicit_shadow_triangles += triangles;
                stats.shadow_lod_histogram[selected.lod.ilog2() as usize] += 1;
                stats.shadow_lod_changes +=
                    usize::from(previous.is_some_and(|old| old != selected.lod));
                if offscreen {
                    stats.shadow_only_instances += 1;
                    stats.shadow_only_triangles += triangles;
                }
                shadow_batches.push(VegetationBatch {
                    key: selected,
                    mesh: mesh.clone(),
                    instances: vec![VegetationInstance {
                        root_cell: [p.root.x, p.root.y, p.root.z],
                        quarter_turn: p.rotation as u32,
                        fade: 1.,
                    }],
                });
            }
        }
        let (shadow_batches, shadow_transitions, shadow_coverage) =
            if self.shadow_transitions_enabled {
                self.shadow_lifecycle.apply(
                    shadow_batches,
                    TREE_SHADOW_TRIANGLE_BUDGET,
                    Instant::now(),
                    &mut shadow_inspection,
                )
            } else {
                (shadow_batches, Vec::new(), Vec::new())
            };
        stats.shadow_transition_pairs = shadow_transitions.len();
        let shadow_total: usize = shadow_batches
            .iter()
            .map(|b| b.mesh.indices.len() / 3 * b.instances.len())
            .sum();
        stats.shadow_transition_triangles = shadow_transitions
            .iter()
            .filter_map(|t| {
                shadow_batches
                    .iter()
                    .find(|b| b.key == t.from && b.instances[0].root_cell == t.root_cell)
            })
            .map(|b| b.mesh.indices.len() / 3)
            .sum();
        stats.shadow_fading_casters = shadow_coverage
            .iter()
            .map(|c| c.root_cell)
            .collect::<BTreeSet<_>>()
            .len();
        stats.explicit_shadow_triangles = shadow_total;
        if self.shadow_transitions_enabled {
            let primary: BTreeMap<_, _> = shadow_batches
                .iter()
                .map(|b| (tree_identity(b.key), b.key.lod))
                .collect();
            stats.explicit_shadow_instances = primary.len();
            stats.shadow_lod_changes = primary
                .iter()
                .filter(|entry| {
                    self.shadow_lods
                        .get(entry.0)
                        .is_some_and(|old| old != entry.1)
                })
                .count();
            stats.shadow_lod_histogram = [0; 4];
            for lod in primary.values() {
                stats.shadow_lod_histogram[lod.ilog2() as usize] += 1;
            }
        }
        // Every outgoing mesh remains an admitted, cache-protected owner until
        // its complementary native-cell coverage has fully transferred.
        for batch in &shadow_batches {
            if let Some((_, used)) = self.meshes.get_mut(&batch.key) {
                *used = self.tick;
            }
        }
        mesh_requests.sort_by(|a, b| {
            let deadline = |p: &PlantRecord| {
                self.prediction.time_to_need(
                    [
                        p.root.x as f64 * VOXEL_SIZE - camera[0],
                        p.root.z as f64 * VOXEL_SIZE - camera[2],
                    ],
                    8.,
                )
            };
            a.0.cmp(&b.0)
                .then(((a.1 + self.tick as u8 % 3) % 3).cmp(&((b.1 + self.tick as u8 % 3) % 3)))
                .then(deadline(&a.4).total_cmp(&deadline(&b.4)))
                .then(a.2.total_cmp(&b.2))
                .then(a.3.cmp(&b.3))
        });
        let mut requested = BTreeSet::new();
        for (_, _, _, key, p, ground) in mesh_requests {
            if submitted >= self.workers.len() * 2 {
                break;
            }
            if !requested.insert(key) || self.pending_meshes.contains(&key) {
                continue;
            }
            let job = if ground {
                Job::Ground(key, p, self.world.clone(), epoch)
            } else {
                Job::Mesh(key, p, self.world.clone(), epoch)
            };
            if self.tx.as_ref().unwrap().try_send(job).is_err() {
                break;
            }
            self.pending_meshes.insert(key);
            self.mesh_started.insert(key, now);
            submitted += 1;
        }
        evict(&mut self.tiles, self.config.max_tiles, |v| v.1);
        evict(&mut self.mesh_costs, 2048, |v| v.1);
        // Equal-age baseline/selected meshes used to evict in key order, which
        // can evict a just-selected fine mesh before an unused cached variant.
        // Prefer unused geometry; hard limits still win if all entries are live.
        let selected: BTreeSet<_> = batches
            .keys()
            .chain(shadow_batches.iter().map(|b| &b.key))
            .copied()
            .collect();
        stats.selected_cpu_meshes = selected.len();
        let selected_geometry: BTreeMap<_, _> = batches
            .values()
            .chain(shadow_batches.iter())
            .map(|b| (b.key, &b.mesh))
            .collect();
        stats.selected_cpu_geometry_bytes = selected_geometry.values().map(|m| mesh_bytes(m)).sum();
        let retained_outside_cache: usize = selected_geometry
            .iter()
            .filter(|(key, _)| !self.meshes.contains_key(key))
            .map(|(_, m)| mesh_bytes(m))
            .sum();
        let mut bytes: usize = self.meshes.values().map(|(m, _)| mesh_bytes(m)).sum();
        while bytes > stats.mesh_budget_bytes || self.meshes.len() > self.config.max_meshes {
            let Some(key) = self
                .meshes
                .iter()
                .min_by_key(|(k, v)| (selected.contains(k), v.1))
                .map(|(k, _)| *k)
            else {
                break;
            };
            let removed_bytes = mesh_bytes(&self.meshes.remove(&key).unwrap().0);
            bytes -= removed_bytes;
            if selected.contains(&key) {
                stats.selected_mesh_evictions += 1;
                stats.selected_mesh_evicted_bytes += removed_bytes;
            }
        }
        stats.cpu_cache_geometry_bytes = bytes;
        stats.total_cpu_geometry_bytes =
            bytes + stats.selected_mesh_evicted_bytes + retained_outside_cache;
        stats.resident_tiles = self.tiles.len();
        stats.resident_meshes = self.meshes.len();
        stats.pending_jobs = stats.missing_tiles + stats.missing_meshes;
        for batch in batches.values().filter(|b| b.key.species == 2) {
            let count = batch.instances.len();
            stats.tree_lod_histogram[batch.key.lod.ilog2() as usize] += count;
            if let Some(&previous) = self.previous_lods.get(&tree_identity(batch.key)) {
                if batch.key.lod != previous {
                    stats.tree_lod_changes += count;
                    if batch.key.lod < previous {
                        stats.tree_refinements += count;
                    } else {
                        stats.tree_coarsenings += count;
                    }
                }
            }
        }
        let frame = VegetationFrame {
            origin: [c.x, c.y, c.z],
            batches: batches.into_values().collect(),
            shadow_batches,
            shadow_transitions,
            shadow_coverage,
            shadow_inspection,
            stats,
        };
        let mut color_lods: BTreeMap<_, _> = frame
            .batches
            .iter()
            .take(self.config.max_meshes)
            .map(|b| (tree_identity(b.key), b.key.lod))
            .collect();
        // Preserve a hidden tree's color history without overwriting it with
        // the independent shadow LOD while ownership transfers on a turn.
        for b in &frame.shadow_batches {
            let identity = tree_identity(b.key);
            if let Some(&lod) = self.previous_lods.get(&identity) {
                color_lods.entry(identity).or_insert(lod);
            }
        }
        self.previous_lods = color_lods;
        self.shadow_lods = frame
            .shadow_batches
            .iter()
            .map(|b| (tree_identity(b.key), b.key.lod))
            .collect();
        if vegetation_trace() {
            let mut ground_lods = [0usize; 4];
            for batch in &frame.batches {
                if batch.key.species != 2 {
                    ground_lods[batch.key.lod.ilog2() as usize] += 1;
                }
            }
            eprintln!("vegetation_selection tick={} camera={camera:?} ground_ready={} ground_lods={ground_lods:?} retained={} pending={} triangles={} cache_bytes={}",
                self.tick, ground_coverage_ready, stats.retained_ground_detail,
                stats.pending_jobs, stats.triangles, bytes);
        }
        if frame.stats.pending_jobs == 0
            && frame.shadow_transitions.is_empty()
            && frame.shadow_coverage.is_empty()
            && self.pending_tiles.is_empty()
            && self.pending_meshes.is_empty()
            && frame
                .batches
                .iter()
                .chain(&frame.shadow_batches)
                .all(|b| self.meshes.contains_key(&b.key))
        {
            self.stable_frame = Some((key, frame.clone()));
        }
        frame
    }
}
fn vegetation_trace() -> bool {
    static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("FRONTIER_VEGETATION_TRACE").as_deref() == Ok("1"))
}
impl Drop for VegetationStream {
    fn drop(&mut self) {
        self.epoch.fetch_add(1, Ordering::Relaxed);
        self.tx.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
#[cfg(test)]
mod streaming_stability_tests {
    use super::*;
    #[test]
    fn newly_visible_far_detail_cannot_reserve_away_the_nearest_color_tree() {
        // Exercise both retained far detail and newly visible cached detail.
        // Their combined cost exceeds the unchanged 65% tree allocation.
        for (retained_far, fine_costs) in [
            (false, [300, 250, 250]),
            (true, [300, 250, 250]),
            (true, [350, 300, 250]),
        ] {
            let mut stream = VegetationStream::new(
                World::new(7),
                VegetationConfig {
                    grass_radius_m: 0.,
                    flower_radius_m: 0.,
                    tree_radius_m: 100.,
                    max_triangles: 1_000,
                    ..Default::default()
                },
            );
            for species in 0..3 {
                for x in -5..=5 {
                    for z in -5..=5 {
                        stream.tiles.insert(Tile { species, x, z }, (vec![], 0));
                    }
                }
            }
            for (i, z) in [16, 640, 1120].into_iter().enumerate() {
                let tree = PlantRecord {
                    species: Species::Rowan,
                    variant: 0,
                    seed: i as u64 + 1,
                    root: Cell { x: 0, y: 0, z },
                    rotation: 0,
                };
                stream
                    .tiles
                    .get_mut(&Tile {
                        species: 2,
                        x: 0,
                        z: 0,
                    })
                    .unwrap()
                    .0
                    .push(tree);
                let identity = MeshKey {
                    species: 2,
                    variant: 0,
                    seed: tree.seed,
                    tile: [0; 2],
                    lod: 0,
                    edit_id: 0,
                };
                for (lod, cost) in [(2, fine_costs[i]), (4, 2_000), (8, 30)] {
                    let key = MeshKey { lod, ..identity };
                    stream.meshes.insert(
                        key,
                        (
                            Arc::new(VegetationMesh {
                                vertices: vec![],
                                indices: vec![0; cost * 3],
                            }),
                            0,
                        ),
                    );
                    stream.mesh_costs.insert(key, (cost, 0));
                }
                // Exclude generation from this admission/representation test.
                for lod in [1] {
                    stream
                        .mesh_costs
                        .insert(MeshKey { lod, ..identity }, (2_000, 0));
                }
                if i == 0 || retained_far {
                    stream.previous_lods.insert(identity, 2);
                }
            }
            let frame = stream.update([0.; 3], 1080, 1., 0);
            let lods: BTreeMap<_, _> = frame
                .batches
                .iter()
                .map(|b| (b.key.seed, b.key.lod))
                .collect();
            if fine_costs[0] == 300 {
                assert_eq!(lods, BTreeMap::from([(1, 2), (2, 2), (3, 8)]));
                assert_eq!(frame.stats.triangles_by_species[2], 580);
            } else {
                assert_eq!(
                    lods.len(),
                    3,
                    "near detail must preserve every feasible far baseline"
                );
                assert_eq!(lods[&1], 2, "near resident detail remains protected");
                assert_eq!(frame.stats.coverage_unresolved, 0);
            }
            assert!(frame.stats.triangles_by_species[2] <= 650);
            assert_eq!(
                frame.stats.pending_jobs, 0,
                "unaffordable variants are not queued"
            );
        }
    }
    #[test]
    fn shadow_baselines_keep_nearest_coverage_without_a_fixed_overlap_reservation() {
        for count in [3, 4] {
            let mut stream = VegetationStream::new(
                World::new(7),
                VegetationConfig {
                    grass_radius_m: 0.,
                    flower_radius_m: 0.,
                    tree_radius_m: 100.,
                    ..Default::default()
                },
            );
            stream.shadow_transitions_enabled = true;
            for species in 0..3 {
                for x in -5..=5 {
                    for z in -5..=5 {
                        stream.tiles.insert(Tile { species, x, z }, (vec![], 0));
                    }
                }
            }
            for (i, z) in [16, 640, 1120, 1280].into_iter().take(count).enumerate() {
                let tree = PlantRecord {
                    species: Species::Rowan,
                    variant: 0,
                    seed: i as u64 + 1,
                    root: Cell { x: 0, y: 0, z },
                    rotation: 0,
                };
                stream
                    .tiles
                    .get_mut(&Tile {
                        species: 2,
                        x: 0,
                        z: 0,
                    })
                    .unwrap()
                    .0
                    .push(tree);
                let identity = MeshKey {
                    species: 2,
                    variant: 0,
                    seed: tree.seed,
                    tile: [0; 2],
                    lod: 0,
                    edit_id: 0,
                };
                let baseline = MeshKey { lod: 8, ..identity };
                stream.meshes.insert(
                    baseline,
                    (
                        Arc::new(VegetationMesh {
                            vertices: vec![],
                            indices: vec![0; 49_000 * 3],
                        }),
                        0,
                    ),
                );
                stream.mesh_costs.insert(baseline, (49_000, 0));
                // Known unaffordable refinements isolate feasible baseline
                // admission from asynchronous generation and color refinement.
                for lod in [1, 2, 4] {
                    stream
                        .mesh_costs
                        .insert(MeshKey { lod, ..identity }, (2_000_000, 0));
                }
                stream.shadow_lods.insert(identity, 8);
                stream.previous_lods.insert(identity, 8);
            }
            let frame = stream.update([0.; 3], 1080, 1., 0);
            let admitted: BTreeSet<_> = frame.shadow_batches.iter().map(|b| b.key.seed).collect();
            assert_eq!(admitted,BTreeSet::from([1,2,3]),"all feasible baselines survive; excess far coverage cannot reserve against nearer roots");
            assert_eq!(frame.stats.explicit_shadow_triangles, 147_000);
            assert_eq!(
                frame.stats.pending_jobs, 0,
                "rejected refinements do not queue"
            );
        }
    }
    #[test]
    fn prediction_tracks_measured_latency_velocity_turns_and_stops() {
        use std::time::Duration;
        let now = Instant::now();
        let mut prediction = StreamingPrediction::default();
        for ms in 1..=64 {
            prediction.observe_latency(now, now + Duration::from_millis(ms));
        }
        assert!((prediction.horizon() - 0.161).abs() < 1e-9);
        assert_eq!(prediction.update([0.; 3], now), [0.; 2]);
        let lead = prediction.update([10., 0., 0.], now + Duration::from_secs(1));
        assert!((lead[0] - 1.61).abs() < 1e-9);
        assert!((prediction.time_to_need([20., 0.], 5.) - 1.5).abs() < 1e-9);
        assert!(prediction.time_to_need([0., 20.], 5.).is_infinite());
        let lead = prediction.update([10., 0., 10.], now + Duration::from_secs(2));
        assert_eq!(lead[0], 0.);
        assert!(lead[1] > 0.);
        assert_eq!(
            prediction.update([10., 0., 10.], now + Duration::from_secs(3)),
            [0.; 2]
        );
        assert_eq!(
            prediction.update([1e6, 0., 0.], now + Duration::from_secs(4)),
            [0.; 2]
        );
        assert_eq!(prediction.latencies.len(), 64);
    }
    #[test]
    fn budget_boundary_has_separate_refine_and_retain_thresholds() {
        assert!(!tree_lod_admitted(1, Some(4), 1_000, 1_099));
        assert!(tree_lod_admitted(1, Some(4), 1_000, 1_100));
        for room in 1_000..1_100 {
            assert!(tree_lod_admitted(1, Some(1), 1_000, room));
        }
        assert!(!tree_lod_admitted(1, Some(1), 1_000, 999));
    }
    #[test]
    fn nearby_refinement_preserves_another_resident_tree_across_reorders() {
        let world = World::new(7);
        let mut stream = VegetationStream::new(
            world,
            VegetationConfig {
                grass_radius_m: 0.,
                flower_radius_m: 0.,
                tree_radius_m: 32.,
                max_triangles: 2_000,
                ..Default::default()
            },
        );
        // Supply all placement tiles to keep this a scheduler/selection test.
        for species in 0..3 {
            for x in -3..=3 {
                for z in -3..=3 {
                    stream.tiles.insert(Tile { species, x, z }, (Vec::new(), 0));
                }
            }
        }
        let trees: Vec<_> = [32, 64]
            .into_iter()
            .enumerate()
            .map(|(i, x)| PlantRecord {
                species: Species::Rowan,
                variant: 0,
                seed: i as u64 + 1,
                root: Cell { x, y: 0, z: 32 },
                rotation: 0,
            })
            .collect();
        stream
            .tiles
            .get_mut(&Tile {
                species: 2,
                x: 0,
                z: 0,
            })
            .unwrap()
            .0 = trees.clone();
        for tree in &trees {
            for (lod, cost) in [(1, 1_000), (2, 500), (4, 200), (8, 100)] {
                let key = MeshKey {
                    species: 2,
                    variant: 0,
                    seed: tree.seed,
                    tile: [0; 2],
                    lod,
                    edit_id: 0,
                };
                stream.meshes.insert(
                    key,
                    (
                        Arc::new(VegetationMesh {
                            vertices: vec![],
                            indices: vec![0; cost * 3],
                        }),
                        0,
                    ),
                );
                stream.mesh_costs.insert(key, (cost, 0));
            }
        }
        let identity = MeshKey {
            species: 2,
            variant: 0,
            seed: 2,
            tile: [0; 2],
            lod: 0,
            edit_id: 0,
        };
        stream.previous_lods.insert(identity, 1);
        for camera in [[0., 0., 0.], [5., 0., 0.], [0., 0., 0.], [5., 0., 0.]] {
            let frame = stream.update(camera, 1080, 1., 0);
            let retained = frame.batches.iter().find(|b| b.key.seed == 2).unwrap();
            assert_eq!(
                retained.key.lod, 1,
                "new detail must not steal current tree allocation"
            );
            assert_eq!(frame.stats.selected_by_species[2], 2);
            assert!(frame.stats.triangles_by_species[2] <= 1_300);
        }
    }
    #[test]
    fn heading_transfer_keeps_tree_mesh_and_shadow_identity() {
        let mut stream = VegetationStream::new(
            World::new(7),
            VegetationConfig {
                grass_radius_m: 0.,
                flower_radius_m: 0.,
                tree_radius_m: 32.,
                max_triangles: 2_000,
                ..Default::default()
            },
        );
        for species in 0..3 {
            for x in -3..=3 {
                for z in -3..=3 {
                    stream.tiles.insert(Tile { species, x, z }, (Vec::new(), 0));
                }
            }
        }
        let tree = PlantRecord {
            species: Species::Rowan,
            variant: 0,
            seed: 123,
            root: Cell { x: 0, y: 0, z: 320 },
            rotation: 0,
        };
        stream
            .tiles
            .get_mut(&Tile {
                species: 2,
                x: 0,
                z: 0,
            })
            .unwrap()
            .0
            .push(tree);
        for (lod, cost) in [(1, 1_000), (2, 500), (4, 200), (8, 100)] {
            let key = MeshKey {
                species: 2,
                variant: 0,
                seed: tree.seed,
                tile: [0; 2],
                lod,
                edit_id: 0,
            };
            stream.meshes.insert(
                key,
                (
                    Arc::new(VegetationMesh {
                        vertices: vec![],
                        indices: vec![0; cost * 3],
                    }),
                    0,
                ),
            );
            stream.mesh_costs.insert(key, (cost, 0));
        }
        let native_key = MeshKey {
            species: 2,
            variant: 0,
            seed: tree.seed,
            tile: [0; 2],
            lod: 1,
            edit_id: 0,
        };
        let native = stream.meshes.remove(&native_key).unwrap();
        stream.pending_meshes.insert(native_key); // Delay native completion deterministically.
        let initial = stream.update_view([0.; 3], 1080, 1., 0, [0., 0., 1.], 1.);
        assert_eq!(initial.batches[0].key.lod, 2);
        let shadow_key = initial.shadow_batches[0].key;
        assert_eq!(shadow_key.lod, 4);
        stream.meshes.insert(native_key, native);
        stream.pending_meshes.remove(&native_key);
        let refined = stream.update_view([0.; 3], 1080, 1., 0, [0., 0., 1.], 1.);
        assert_eq!(refined.batches[0].key, native_key);
        assert_eq!(refined.shadow_batches[0].key, shadow_key);
        assert_eq!(refined.stats.shadow_lod_changes, 0);
        for _ in 0..3 {
            let back = stream.update_view([0.; 3], 1080, 1., 0, [0., 0., -1.], 1.);
            assert!(back.batches.is_empty());
            assert_eq!(back.shadow_batches.len(), 1);
            assert_eq!(back.shadow_batches[0].key, shadow_key);
            assert_eq!(back.stats.shadow_lod_changes, 0);
            let front = stream.update_view([0.; 3], 1080, 1., 0, [0., 0., 1.], 1.);
            assert_eq!(front.batches[0].key, native_key);
            assert_eq!(front.shadow_batches[0].key, shadow_key);
            assert_eq!(front.stats.shadow_lod_changes, 0);
        }
    }
    #[test]
    fn heading_preserves_more_than_eight_forty_meter_casters() {
        let mut stream = VegetationStream::new(
            World::new(7),
            VegetationConfig {
                grass_radius_m: 0.,
                flower_radius_m: 0.,
                tree_radius_m: 100.,
                max_triangles: 100_000,
                ..Default::default()
            },
        );
        for species in 0..3 {
            for x in -4..=4 {
                for z in -4..=4 {
                    stream.tiles.insert(Tile { species, x, z }, (Vec::new(), 0));
                }
            }
        }
        for i in 0..12 {
            let tree = PlantRecord {
                species: Species::Rowan,
                variant: 0,
                seed: i + 1,
                root: Cell {
                    x: i as i64 * 8,
                    y: 0,
                    z: 640,
                },
                rotation: 0,
            };
            stream
                .tiles
                .get_mut(&Tile {
                    species: 2,
                    x: 0,
                    z: 1,
                })
                .unwrap()
                .0
                .push(tree);
            for (lod, cost) in [(1, 1_000), (2, 500), (4, 200), (8, 100)] {
                let key = MeshKey {
                    species: 2,
                    variant: 0,
                    seed: tree.seed,
                    tile: [0; 2],
                    lod,
                    edit_id: 0,
                };
                stream.meshes.insert(
                    key,
                    (
                        Arc::new(VegetationMesh {
                            vertices: vec![],
                            indices: vec![0; cost * 3],
                        }),
                        0,
                    ),
                );
                stream.mesh_costs.insert(key, (cost, 0));
            }
        }
        let front = stream.update_view([0.; 3], 1080, 1., 0, [0., 0., 1.], 1.);
        let expected: BTreeSet<_> = front.shadow_batches.iter().map(|b| b.key).collect();
        assert_eq!(expected.len(), 12);
        for forward in [[0., 0., -1.], [1., 0., 0.], [0., 0., 1.], [0., 0., -1.]] {
            let frame = stream.update_view([0.; 3], 1080, 1., 0, forward, 1.);
            assert_eq!(
                frame
                    .shadow_batches
                    .iter()
                    .map(|b| b.key)
                    .collect::<BTreeSet<_>>(),
                expected
            );
            assert_eq!(frame.stats.shadow_lod_changes, 0);
            assert!(frame.stats.explicit_shadow_triangles <= 200_000);
        }
    }
    #[test]
    fn coarsening_waits_for_replacement_without_a_baseline_flash() {
        let mut stream = VegetationStream::new(
            World::new(7),
            VegetationConfig {
                grass_radius_m: 0.,
                flower_radius_m: 0.,
                tree_radius_m: 32.,
                max_triangles: 2_000,
                ..Default::default()
            },
        );
        // This regression isolates request/readiness selection. Temporal
        // ownership handoff is covered independently by shadow_lifecycle tests.
        stream.shadow_transitions_enabled = false;
        for species in 0..3 {
            for x in -3..=3 {
                for z in -3..=3 {
                    stream.tiles.insert(Tile { species, x, z }, (Vec::new(), 0));
                }
            }
        }
        let tree = PlantRecord {
            species: Species::Rowan,
            variant: 0,
            seed: 123,
            root: Cell { x: 0, y: 0, z: 320 },
            rotation: 0,
        };
        stream
            .tiles
            .get_mut(&Tile {
                species: 2,
                x: 0,
                z: 0,
            })
            .unwrap()
            .0
            .push(tree);
        let identity = MeshKey {
            species: 2,
            variant: 0,
            seed: tree.seed,
            tile: [0; 2],
            lod: 0,
            edit_id: 0,
        };
        let mesh = |cost| {
            Arc::new(VegetationMesh {
                vertices: vec![],
                indices: vec![0; cost * 3],
            })
        };
        for (lod, cost) in [(2, 500), (8, 100)] {
            let key = MeshKey { lod, ..identity };
            stream.meshes.insert(key, (mesh(cost), 0));
            stream.mesh_costs.insert(key, (cost, 0));
        }
        let replacement = MeshKey { lod: 4, ..identity };
        stream.mesh_costs.insert(replacement, (200, 0));
        stream.pending_meshes.insert(replacement);
        stream.previous_lods.insert(identity, 2);
        stream.shadow_lods.insert(identity, 2);
        for _ in 0..3 {
            let pending = stream.update([0.; 3], 32, 1., 0);
            assert_eq!(pending.batches[0].key.lod, 2);
            assert_eq!(pending.shadow_batches[0].key.lod, 2);
            assert_eq!(pending.stats.tree_lod_changes, 0);
            assert_eq!(pending.stats.shadow_lod_changes, 0);
        }
        stream.meshes.insert(replacement, (mesh(200), 0));
        stream.pending_meshes.remove(&replacement);
        let ready = stream.update([0.; 3], 32, 1., 0);
        assert_eq!(ready.batches[0].key.lod, 4);
        assert_eq!(ready.shadow_batches[0].key.lod, 4);
        assert_eq!(ready.stats.tree_lod_changes, 1);
        assert_eq!(ready.stats.shadow_lod_changes, 1);
        let stable = stream.update([0.; 3], 32, 1., 0);
        assert_eq!(stable.stats.tree_lod_changes, 0);
        assert_eq!(stable.stats.shadow_lod_changes, 0);
    }
    #[test]
    fn static_cache_pressure_keeps_current_coverage_without_regeneration() {
        let mut stream = VegetationStream::new(
            World::new(7),
            VegetationConfig {
                grass_radius_m: 2.,
                flower_radius_m: 0.,
                tree_radius_m: 32.,
                max_triangles: 2_000,
                max_meshes: 3,
                ..Default::default()
            },
        );
        for species in 0..3 {
            for x in -3..=3 {
                for z in -3..=3 {
                    stream.tiles.insert(Tile { species, x, z }, (Vec::new(), 0));
                }
            }
        }
        let tree = PlantRecord {
            species: Species::Rowan,
            variant: 0,
            seed: 123,
            root: Cell { x: 0, y: 0, z: 320 },
            rotation: 0,
        };
        let grass = PlantRecord {
            species: Species::Grass,
            variant: 0,
            seed: 456,
            root: Cell { x: 64, y: 0, z: 64 },
            rotation: 0,
        };
        stream
            .tiles
            .get_mut(&Tile {
                species: 2,
                x: 0,
                z: 0,
            })
            .unwrap()
            .0
            .push(tree);
        stream
            .tiles
            .get_mut(&Tile {
                species: 0,
                x: 0,
                z: 0,
            })
            .unwrap()
            .0
            .push(grass);
        let tree_identity = MeshKey {
            species: 2,
            variant: 0,
            seed: tree.seed,
            tile: [0; 2],
            lod: 0,
            edit_id: 0,
        };
        let grass_identity = MeshKey {
            species: 0,
            variant: u8::MAX,
            seed: mix(7),
            tile: [0; 2],
            lod: 0,
            edit_id: 0,
        };
        for (key, cost) in [
            (
                MeshKey {
                    lod: 2,
                    ..tree_identity
                },
                500,
            ),
            (
                MeshKey {
                    lod: 4,
                    ..tree_identity
                },
                200,
            ),
            (
                MeshKey {
                    lod: 8,
                    ..tree_identity
                },
                100,
            ),
            (
                MeshKey {
                    lod: 2,
                    ..grass_identity
                },
                200,
            ),
        ] {
            stream.meshes.insert(
                key,
                (
                    Arc::new(VegetationMesh {
                        vertices: vec![],
                        indices: vec![0; cost * 3],
                    }),
                    0,
                ),
            );
            stream.mesh_costs.insert(key, (cost, 0));
        }
        stream.mesh_costs.insert(
            MeshKey {
                lod: 1,
                ..tree_identity
            },
            (2_000, 0),
        );
        stream.mesh_costs.insert(
            MeshKey {
                lod: 1,
                ..grass_identity
            },
            (5_000, 0),
        );
        stream.previous_lods.insert(tree_identity, 2);
        stream.previous_lods.insert(grass_identity, 2);
        stream.shadow_lods.insert(tree_identity, 4);
        for _ in 0..12 {
            let frame = stream.update([0.; 3], 1080, 1., 0);
            assert_eq!(frame.stats.selected_by_species, [1, 0, 1]);
            assert_eq!(frame.stats.explicit_shadow_instances, 1);
            assert_eq!(
                frame.stats.pending_jobs, 0,
                "resident fine coverage must not require evicted baseline8"
            );
            assert_eq!(frame.stats.selected_mesh_evictions, 0);
            assert!(stream.pending_meshes.is_empty());
            assert_eq!(stream.meshes.len(), 3);
            assert_eq!(
                stream.job_counters.snapshot(stream.workers.len()).started,
                [0; 3]
            );
        }
        assert!(!stream.meshes.contains_key(&MeshKey {
            lod: 8,
            ..tree_identity
        }));
    }
    #[test]
    fn source_change_rejects_old_content_but_camera_result_preserves_new_pending() {
        let world = World::new(7);
        let mut stream = VegetationStream::new(
            world.clone(),
            VegetationConfig {
                max_tiles: 0,
                ..Default::default()
            },
        );
        let tile = Tile {
            species: 2,
            x: 0,
            z: 0,
        };
        let obsolete = stream.epoch.load(Ordering::Relaxed);
        stream.set_world(World::new(8)); // Same revision, different canonical source.
        let (send, receive) = mpsc::sync_channel(2);
        stream.rx = receive;
        send.send((obsolete, Done::Tile(tile, 0, Vec::new())))
            .unwrap();
        stream.update([0.; 3], 1080, 1., 0);
        assert!(!stream.tiles.contains_key(&tile));
        // Permit one resident tile so this assertion checks content validity,
        // rather than immediately evicting the valid completion at max_tiles=0.
        stream.config.max_tiles = 1;
        let camera_epoch = stream.epoch.fetch_add(1, Ordering::Relaxed);
        stream.pending_tiles.insert(tile); // New request with identical identity.
        send.send((camera_epoch, Done::Tile(tile, 0, Vec::new())))
            .unwrap();
        stream.update([0.; 3], 1080, 1., 0);
        assert!(
            stream.tiles.contains_key(&tile),
            "old camera content remains reusable"
        );
        assert!(
            stream.pending_tiles.contains(&tile),
            "old completion cannot clear newer work"
        );
    }
    #[test]
    fn pool_completes_each_bounded_job_once_and_discards_obsolete_generation() {
        let world = World::new(7);
        let stream = VegetationStream::new(world.clone(), VegetationConfig::default());
        let generation = stream.epoch.load(Ordering::Relaxed);
        let count = stream.workers.len() * 2;
        for x in 0..count {
            stream
                .tx
                .as_ref()
                .unwrap()
                .send(Job::Tile(
                    Tile {
                        species: 0,
                        x: x as i64,
                        z: 0,
                    },
                    world.clone(),
                    generation,
                ))
                .unwrap();
        }
        let mut seen = BTreeSet::new();
        for _ in 0..count {
            let (epoch, done) = stream
                .rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            assert_eq!(epoch, generation);
            let Done::Tile(tile, revision, _) = done else {
                panic!("expected placement")
            };
            assert_eq!(revision, world.revision());
            assert!(seen.insert(tile));
        }
        // No racing camera epoch is needed to exercise cancellation: this job
        // is unambiguously obsolete before any worker can start it.
        stream.epoch.fetch_add(1, Ordering::Relaxed);
        stream
            .tx
            .as_ref()
            .unwrap()
            .send(Job::Tile(
                Tile {
                    species: 0,
                    x: 999,
                    z: 0,
                },
                world,
                generation,
            ))
            .unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let jobs = stream.job_counters.snapshot(stream.workers.len());
            if jobs.cancelled[0] == 1 {
                assert_eq!(jobs.completed[0], count as u64);
                assert_eq!(jobs.started[0], count as u64 + 1);
                assert!(stream.rx.try_recv().is_err());
                break;
            }
            assert!(
                Instant::now() < deadline,
                "worker did not cancel obsolete work"
            );
            thread::yield_now();
        }
    }
}

type GroundCells = BTreeMap<Cell, (u8, [f32; 3])>;
#[cfg(test)]
mod ground_assembly_cache_tests {
    use super::*;
    fn key(seed: u64) -> GroundCacheKey {
        GroundCacheKey {
            records: vec![PlantRecord {
                species: Species::Grass,
                seed,
                root: Cell::default(),
                variant: 2,
                rotation: 0,
            }],
            origin: Cell::default(),
            edits: BTreeSet::new(),
        }
    }
    #[test]
    fn native_assembly_reuse_has_exact_identity_and_a_hard_cache_budget() {
        let mut cache = GroundCache {
            entries: Default::default(),
            bytes: 0,
            hits: 0,
            misses: 0,
        };
        let cells = Arc::new(BTreeMap::from([(Cell::default(), (1, [0.3, 0.4, 0.5]))]));
        cache.insert(key(7), cells.clone(), 4096);
        assert!(Arc::ptr_eq(&cells, &cache.get(&key(7)).unwrap()));
        let mut edited = key(7);
        edited.edits.insert(Cell::default());
        assert!(cache.get(&edited).is_none());
        let mut moved = key(7);
        moved.origin.x = -128;
        assert!(cache.get(&moved).is_none());
        assert!(cache.get(&key(8)).is_none());
        for seed in 8..100 {
            cache.insert(key(seed), cells.clone(), 4096);
            assert!(cache.bytes <= 4096);
        }
        assert!(cache.get(&key(7)).is_none());
        assert_eq!(cells[&Cell::default()].1, [0.3, 0.4, 0.5]); // eviction doesn't invalidate a live job
    }
    #[test]
    fn cached_native_assembly_equals_source_and_applies_edits() {
        let record = key(0x913ab).records[0];
        let empty = BTreeSet::new();
        let origin = Cell::default();
        let reference = ground_cells(&[record], origin, &empty, || false).unwrap();
        assert!(!reference.is_empty());
        let first = ground_cells_cached(&[record], origin, &empty, || false).unwrap();
        let warm = ground_cells_cached(&[record], origin, &empty, || false).unwrap();
        assert!(Arc::ptr_eq(&first, &warm));
        assert_eq!(*first, reference);
        let cell = *reference.first_key_value().unwrap().0;
        let edits = BTreeSet::from([cell]);
        let edited = ground_cells_cached(&[record], origin, &edits, || false).unwrap();
        assert!(!edited.contains_key(&cell));
        assert_eq!(
            *edited,
            ground_cells(&[record], origin, &edits, || false).unwrap()
        );
        assert!(ground_cells_cached(&[record], origin, &empty, || true).is_none());
    }
}
#[derive(Clone, PartialEq, Eq)]
struct GroundCacheKey {
    records: Vec<PlantRecord>,
    origin: Cell,
    edits: BTreeSet<Cell>,
}
struct GroundCache {
    entries: std::collections::VecDeque<(GroundCacheKey, Arc<GroundCells>, usize)>,
    bytes: usize,
    hits: u64,
    misses: u64,
}
impl GroundCache {
    fn get(&mut self, key: &GroundCacheKey) -> Option<Arc<GroundCells>> {
        let Some(index) = self.entries.iter().position(|e| &e.0 == key) else {
            self.misses += 1;
            return None;
        };
        self.hits += 1;
        let entry = self.entries.remove(index).unwrap();
        let cells = entry.1.clone();
        self.entries.push_back(entry);
        Some(cells)
    }
    fn insert(&mut self, key: GroundCacheKey, cells: Arc<GroundCells>, budget: usize) {
        // Deliberately conservative accounting for tree-node allocations and
        // record/key storage. Outstanding worker Arcs are separately live data.
        let bytes = cells.len().saturating_mul(128)
            + key.records.capacity() * std::mem::size_of::<PlantRecord>()
            + key.edits.len() * 128
            + std::mem::size_of::<GroundCacheKey>();
        if bytes > budget || self.entries.iter().any(|e| e.0 == key) {
            return;
        }
        while self.bytes.saturating_add(bytes) > budget || self.entries.len() >= 256 {
            let Some((_, _, old)) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= old;
        }
        self.bytes += bytes;
        self.entries.push_back((key, cells, bytes));
    }
}
fn ground_cache() -> &'static std::sync::Mutex<GroundCache> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<GroundCache>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        std::sync::Mutex::new(GroundCache {
            entries: Default::default(),
            bytes: 0,
            hits: 0,
            misses: 0,
        })
    })
}
pub(crate) fn ground_cache_json() -> String {
    let cache = ground_cache().lock().unwrap();
    format!("{{\"hits\":{},\"misses\":{},\"accounted_bytes\":{},\"budget_bytes\":67108864,\"entries\":{},\"scope\":\"process-shared native ground assembly; conservative accounting excludes worker-held Arcs\"}}",
        cache.hits, cache.misses, cache.bytes, cache.entries.len())
}
fn ground_cells_cached(
    records: &[PlantRecord],
    origin: Cell,
    edited: &BTreeSet<Cell>,
    cancel: impl Fn() -> bool,
) -> Option<Arc<GroundCells>> {
    if cancel() {
        return None;
    }
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let enabled = *ENABLED
        .get_or_init(|| std::env::var("FRONTIER_VEGETATION_GROUND_CACHE").as_deref() != Ok("off"));
    if !enabled {
        return ground_cells(records, origin, edited, cancel).map(Arc::new);
    }
    // The records encode exact plant seeds/shapes; the complete local edit set
    // encodes overrides. No camera, LOD, world pointer or lossy hash is identity.
    let key = GroundCacheKey {
        records: records.to_vec(),
        origin,
        edits: edited
            .iter()
            .copied()
            .filter(|c| {
                c.x >= origin.x && c.x < origin.x + 128 && c.z >= origin.z && c.z < origin.z + 128
            })
            .collect(),
    };
    if let Some(cells) = ground_cache().lock().unwrap().get(&key) {
        return Some(cells);
    }
    let cells = Arc::new(ground_cells(records, origin, edited, &cancel)?);
    if cancel() {
        return None;
    }
    ground_cache()
        .lock()
        .unwrap()
        .insert(key, cells.clone(), 64 * 1024 * 1024);
    Some(cells)
}
/// Assemble a complete globally addressed tile. Never pass a camera-selected
/// subset here: identical content keys must always produce identical voxels.
fn ground_cells(
    records: &[PlantRecord],
    origin: Cell,
    edited: &BTreeSet<Cell>,
    cancel: impl Fn() -> bool,
) -> Option<GroundCells> {
    let mut cells = BTreeMap::new();
    for record in records {
        let model = vegetation::model_for_cancellable(record, &cancel)?;
        for v in &model.voxels {
            let cell = record.world_cell(v.cell);
            if cell.x >= origin.x
                && cell.x < origin.x + 128
                && cell.z >= origin.z
                && cell.z < origin.z + 128
                && !edited.contains(&cell)
            {
                cells
                    .entry(Cell {
                        x: cell.x - origin.x,
                        y: cell.y - origin.y,
                        z: cell.z - origin.z,
                    })
                    .or_insert((v.material as u8, v.color));
            }
        }
    }
    if cancel() {
        None
    } else {
        Some(cells)
    }
}
fn mesh_bytes(mesh: &VegetationMesh) -> usize {
    mesh.vertices.capacity() * std::mem::size_of::<VegetationVertex>() + mesh.indices.capacity() * 4
}
fn evict<K: Ord + Copy, V>(map: &mut BTreeMap<K, V>, limit: usize, age: impl Fn(&V) -> u64) {
    while map.len() > limit {
        if let Some(key) = map.iter().min_by_key(|(_, v)| age(v)).map(|(k, _)| *k) {
            map.remove(&key);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Paint {
    material: u8,
    color: [u32; 3],
    ao: u8,
}
// Exterior face samples distinguish crevices from convex edges. All samples
// are in aggregate-grid coordinates, so this stays bounded at every LOD. Coarse
// occupancy exaggerates canopy density; reduce its strength with stride.
fn face_visibility(
    blocks: &BTreeMap<[i64; 3], Paint>,
    c: [i64; 3],
    d: usize,
    sign: i64,
    step: i64,
) -> u8 {
    let mut exterior = c;
    exterior[d] += sign;
    let u = (d + 1) % 3;
    let v = (d + 2) % 3;
    let mut contact = 0.;
    for (a, b) in [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (-1, -1),
        (1, -1),
        (-1, 1),
    ] {
        let mut q = exterior;
        q[u] += a;
        q[v] += b;
        if blocks.contains_key(&q) {
            contact += 1.;
        }
    }
    let mut canopy = 0.;
    if d != 1 || sign > 0 {
        for r in [2, 4, 8, 16] {
            let mut q = c;
            q[d] += sign * (r / 2).max(1);
            q[1] += r;
            if blocks.contains_key(&q) {
                canopy += 1.;
            }
        }
    }
    let strength = 1. / (step as f32).sqrt();
    let visibility = 1. - strength * (0.22 * contact / 8. + 0.12 * canopy / 4.);
    // Sixteen visibility levels keep greedy merging useful without moving color.
    ((visibility * 15.).round().clamp(0., 15.) as u8) * 17
}
/// Coarse foliage coverage must not replace the rooted woody scaffold. Keep a
/// separately derived structural mesh and trim only the automatic downward
/// expansion of the lowest foliage row. Native geometry is unchanged. The two
/// display meshes may overlap inside the crown; this bounded proxy trades exact
/// internal occlusion for retaining the external trunk and crown support.
fn mesh_plant_cells(
    species: Species,
    cells: impl Iterator<Item = (Cell, u8, [f32; 3])>,
    stride: u8,
    cancel: impl Fn() -> bool,
) -> VegetationMesh {
    if species != Species::Rowan || stride < 4 {
        return mesh_cells(cells, stride, cancel);
    }
    let mut wood = Vec::new();
    let mut foliage = Vec::new();
    let mut foliage_bottom = i64::MAX;
    for (i, cell) in cells.enumerate() {
        if i % 1024 == 0 && cancel() {
            return VegetationMesh::default();
        }
        if cell.1 == Material::Wood as u8 {
            wood.push(cell);
        } else {
            foliage_bottom = foliage_bottom.min(cell.0.y);
            foliage.push(cell);
        }
    }
    // One finer structural level preserves the bole without making every far
    // tree pay near-native branch costs (stride8 crown -> stride4 wood).
    let mut structural = mesh_cells(wood.into_iter(), stride / 2, &cancel);
    if cancel() {
        return VegetationMesh::default();
    }
    let mut crown = mesh_cells(foliage.into_iter(), stride, &cancel);
    if cancel() {
        return VegetationMesh::default();
    }
    if foliage_bottom != i64::MAX {
        let floor = foliage_bottom as f32 * VOXEL_SIZE as f32;
        for vertex in &mut crown.vertices {
            vertex.position[1] = vertex.position[1].max(floor);
        }
    }
    let base = structural.vertices.len() as u32;
    structural.vertices.extend(crown.vertices);
    structural
        .indices
        .extend(crown.indices.into_iter().map(|i| i + base));
    structural
}
/// Palette-aware aggregation anchored on the canonical local grid, followed by
/// exposed-face extraction and coplanar greedy merging. Thin occupied features
/// retain aggregate coverage; majority material and mean color retain palette.
fn mesh_cells(
    cells: impl Iterator<Item = (Cell, u8, [f32; 3])>,
    stride: u8,
    cancel: impl Fn() -> bool,
) -> VegetationMesh {
    mesh_cells_scaled(cells, stride, stride, cancel)
}
fn mesh_ground_cells(
    cells: impl Iterator<Item = (Cell, u8, [f32; 3])>,
    stride: u8,
    cancel: impl Fn() -> bool,
) -> VegetationMesh {
    // Horizontal coverage may coarsen under pressure, but a low carpet must
    // not turn into half-meter-high cubes or bury its roots by half a meter.
    mesh_cells_scaled(cells, stride, stride.min(2), cancel)
}
fn mesh_cells_scaled(
    cells: impl Iterator<Item = (Cell, u8, [f32; 3])>,
    stride: u8,
    vertical_stride: u8,
    cancel: impl Fn() -> bool,
) -> VegetationMesh {
    static REFERENCE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *REFERENCE
        .get_or_init(|| std::env::var("FRONTIER_VEGETATION_MESHER").as_deref() == Ok("reference"))
    {
        mesh_cells_scaled_reference(cells, stride, vertical_stride, cancel)
    } else {
        crate::vegetation_mesher::mesh_cells_scaled(cells, stride, vertical_stride, cancel)
    }
}

pub(crate) fn mesh_cells_scaled_reference(
    cells: impl Iterator<Item = (Cell, u8, [f32; 3])>,
    stride: u8,
    vertical_stride: u8,
    cancel: impl Fn() -> bool,
) -> VegetationMesh {
    let step = i64::from(stride.max(1));
    let steps = [step, i64::from(vertical_stride.max(1)), step];
    let mut groups: BTreeMap<[i64; 3], BTreeMap<u8, (usize, [f64; 3])>> = BTreeMap::new();
    for (i, (c, mat, color)) in cells.enumerate() {
        if i % 1024 == 0 && cancel() {
            return VegetationMesh::default();
        }
        let key = [
            c.x.div_euclid(step),
            c.y.div_euclid(steps[1]),
            c.z.div_euclid(step),
        ];
        let entry = groups
            .entry(key)
            .or_default()
            .entry(mat)
            .or_insert((0, [0.; 3]));
        entry.0 += 1;
        for j in 0..3 {
            entry.1[j] += color[j] as f64;
        }
    }
    let blocks: BTreeMap<[i64; 3], Paint> = groups
        .into_iter()
        .map(|(c, palette)| {
            let material = *palette
                .iter()
                .max_by_key(|(m, (n, _))| (*n, std::cmp::Reverse(**m)))
                .unwrap()
                .0;
            let count: usize = palette.values().map(|p| p.0).sum();
            let mut color = [0.; 3];
            for (_, sum) in palette.values() {
                for j in 0..3 {
                    color[j] += sum[j];
                }
            }
            let color = color.map(|v| ((v / count as f64) as f32).to_bits());
            (
                c,
                Paint {
                    material,
                    ao: 255,
                    color,
                },
            )
        })
        .collect();
    let mut planes: BTreeMap<(usize, i64, i64), BTreeMap<[i64; 2], Paint>> = BTreeMap::new();
    for (i, (c, paint)) in blocks.iter().enumerate() {
        if i % 1024 == 0 && cancel() {
            return VegetationMesh::default();
        }
        for d in 0..3 {
            for sign in [-1, 1] {
                let mut neighbor = *c;
                neighbor[d] += sign;
                if !blocks.contains_key(&neighbor) {
                    let mut face_paint = *paint;
                    face_paint.ao = face_visibility(&blocks, *c, d, sign, step);
                    planes
                        .entry((d, sign, c[d] + i64::from(sign > 0)))
                        .or_default()
                        .insert([c[(d + 1) % 3], c[(d + 2) % 3]], face_paint);
                }
            }
        }
    }
    let mut mesh = VegetationMesh::default();
    for ((d, sign, plane), mut faces) in planes {
        if cancel() {
            return VegetationMesh::default();
        }
        while let Some((&start, &paint)) = faces.first_key_value() {
            let mut w = 1;
            while faces.get(&[start[0] + w, start[1]]) == Some(&paint) {
                w += 1;
            }
            let mut h = 1;
            'height: loop {
                for x in 0..w {
                    if faces.get(&[start[0] + x, start[1] + h]) != Some(&paint) {
                        break 'height;
                    }
                }
                h += 1;
            }
            for y in 0..h {
                for x in 0..w {
                    faces.remove(&[start[0] + x, start[1] + y]);
                }
            }
            let base = mesh.vertices.len() as u32;
            let u = (d + 1) % 3;
            let v = (d + 2) % 3;
            let mut normal = [0.; 3];
            normal[d] = sign as f32;
            for [a, b] in [[0, 0], [w, 0], [w, h], [0, h]] {
                let mut p = [0.; 3];
                p[d] = (plane * steps[d]) as f32 * VOXEL_SIZE as f32;
                p[u] = ((start[0] + a) * steps[u]) as f32 * VOXEL_SIZE as f32;
                p[v] = ((start[1] + b) * steps[v]) as f32 * VOXEL_SIZE as f32;
                mesh.vertices.push(VegetationVertex {
                    position: p,
                    normal,
                    color: paint.color.map(f32::from_bits),
                    ao: paint.ao as f32 / 255.,
                    transmission: if paint.material == Material::Leaf as u8 {
                        1.
                    } else if paint.material == Material::Flower as u8 {
                        0.35
                    } else {
                        0.
                    },
                });
            }
            let indices = if sign > 0 {
                [0, 1, 2, 0, 2, 3]
            } else {
                [0, 2, 1, 0, 3, 2]
            };
            mesh.indices.extend(indices.map(|i| base + i));
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distance_hysteresis_preserves_detail_until_outside_exit_band() {
        let c = VegetationConfig::default();
        assert_eq!(
            hysteretic_stride(
                ground_stride(8.1, 100., &c),
                ground_stride(8.1 * 0.9, 100., &c),
                Some(1)
            ),
            1
        );
        assert_eq!(
            hysteretic_stride(
                ground_stride(9., 100., &c),
                ground_stride(9. * 0.9, 100., &c),
                Some(1)
            ),
            2
        );
        assert_eq!(
            hysteretic_stride(1, 1, Some(4)),
            1,
            "approach refines immediately"
        );
        assert_eq!(
            hysteretic_stride(4, 2, None),
            4,
            "new demand has no invented history"
        );
    }
    #[test]
    fn offline_reference_is_opt_in_native_budget_without_species_caps() {
        let default = VegetationConfig::default();
        assert!(!default.reference);
        for flag in [None, Some("0"), Some("true"), Some("")] {
            let config = default.clone().with_reference_flag(flag);
            assert!(!config.reference);
            assert_eq!(config.max_triangles, 1_000_000);
        }
        let reference = default.with_reference_flag(Some("1"));
        assert!(reference.reference);
        assert_eq!(reference.max_triangles, 32_000_000);
        assert_eq!(reference.max_instances, 20_000);
        assert!(budget_allows(
            &reference,
            &VegetationStats::default(),
            2,
            30_000_000
        ));
        assert!(budget_allows(
            &reference,
            &VegetationStats::default(),
            1,
            30_000_000
        ));
        assert!(!budget_allows(
            &reference,
            &VegetationStats::default(),
            0,
            32_000_001
        ));
        let full = VegetationStats {
            triangles: 32_000_000,
            ..Default::default()
        };
        assert!(!budget_allows(&reference, &full, 2, 1));
    }
    #[test]
    fn tree_reservation_keeps_far_coverage_while_refining_near_trees() {
        let total = 1_000_000;
        let count = 101;
        let baseline = 1_000;
        let native = 50_000;
        let mut remaining = count * baseline;
        let mut spent = 0;
        let mut refined = 0;
        let mut covered = 0;
        for _ in 0..count {
            remaining -= baseline;
            let room = tree_refinement_headroom(total, spent, remaining);
            let selected = if native <= room {
                refined += 1;
                native
            } else {
                baseline
            };
            assert!(selected <= room, "fine trees must not steal later coverage");
            spent += selected;
            covered += 1;
        }
        assert_eq!(covered, count);
        assert!(refined > 0 && refined < count);
        assert!(spent <= 650_000);
        assert_eq!(tree_refinement_headroom(total, 650_000, 1_000), 0);
    }
    #[test]
    fn frustum_rejects_behind_and_side_spheres_but_keeps_front() {
        let view = CullView::new([0.; 3], [0., 0., 1.], std::f32::consts::FRAC_PI_2, 1.).unwrap();
        assert!(view.intersects_sphere(glam::DVec3::new(0., 0., 10.), 1.));
        assert!(!view.intersects_sphere(glam::DVec3::new(0., 0., -10.), 1.));
        assert!(!view.intersects_sphere(glam::DVec3::new(20., 0., 10.), 1.));
        assert!(!view.intersects_sphere(glam::DVec3::new(0., 20., 10.), 1.));
        assert!(!view.intersects_sphere(glam::DVec3::new(-20., 0., 10.), 1.));
    }
    #[test]
    fn frustum_conservatively_retains_bounds_crossing_planes() {
        let view = CullView::new([0.; 3], [0., 0., 1.], std::f32::consts::FRAC_PI_2, 1.).unwrap();
        // Sphere centers lie outside, but their occupied bounds intersect.
        assert!(view.intersects_sphere(glam::DVec3::new(11., 0., 10.), 1.));
        assert!(view.intersects_sphere(glam::DVec3::new(0., 11., 10.), 1.));
        assert!(view.intersects_sphere(glam::DVec3::new(0., 0., -0.5), 1.));
        let wide = CullView::new([0.; 3], [0., 0., 1.], std::f32::consts::FRAC_PI_2, 2.).unwrap();
        assert!(wide.intersects_sphere(glam::DVec3::new(19., 0., 10.), 0.1));
        assert!(CullView::new([0.; 3], [0.; 3], 1., 1.).is_none());
    }
    #[test]
    fn ground_tiles_preserve_seeded_rotated_native_cells_and_edits() {
        let origin = Cell {
            x: -128,
            y: 0,
            z: 256,
        };
        let records = [
            PlantRecord {
                species: Species::Grass,
                variant: 2,
                seed: 17,
                root: Cell {
                    x: -90,
                    y: 64,
                    z: 290,
                },
                rotation: 3,
            },
            PlantRecord {
                species: Species::Grass,
                variant: 2,
                seed: 18,
                root: Cell {
                    x: -70,
                    y: 61,
                    z: 312,
                },
                rotation: 1,
            },
        ];
        let mut expected = BTreeMap::new();
        for p in &records {
            for v in &vegetation::model_for(p).voxels {
                let c = p.world_cell(v.cell);
                expected
                    .entry(Cell {
                        x: c.x - origin.x,
                        y: c.y,
                        z: c.z - origin.z,
                    })
                    .or_insert((v.material as u8, v.color));
            }
        }
        assert_eq!(
            ground_cells(&records, origin, &BTreeSet::new(), || false).unwrap(),
            expected
        );
        let edited = records[0].root;
        expected.remove(&Cell {
            x: edited.x - origin.x,
            y: edited.y,
            z: edited.z - origin.z,
        });
        assert_eq!(
            ground_cells(&records, origin, &BTreeSet::from([edited]), || false).unwrap(),
            expected
        );
        // A neighboring root may cross the tile edge, but each native voxel
        // belongs to one tile, preventing duplicate coplanar seeded colors.
        let boundary = PlantRecord {
            root: Cell {
                x: 0,
                y: 10,
                z: 290,
            },
            ..records[0]
        };
        let left = ground_cells(&[boundary], origin, &BTreeSet::new(), || false).unwrap();
        let right_origin = Cell { x: 0, ..origin };
        let right = ground_cells(&[boundary], right_origin, &BTreeSet::new(), || false).unwrap();
        assert!(!left.is_empty() && !right.is_empty());
        assert_eq!(
            left.len() + right.len(),
            vegetation::model_for(&boundary).voxels.len()
        );
        assert!(ground_cells(&records, origin, &BTreeSet::new(), || true).is_none());
    }
    #[test]
    fn rowans_reserve_ground_budget_and_ground_borrows_unused_tree_budget() {
        let config = VegetationConfig::default();
        let mut stats = VegetationStats::default();
        assert!(budget_allows(&config, &stats, 2, 650_000));
        stats.triangles = 650_000;
        stats.triangles_by_species[2] = 650_000;
        assert!(!budget_allows(&config, &stats, 2, 1));
        assert!(budget_allows(&config, &stats, 0, 250_000));
        stats.triangles += 250_000;
        stats.triangles_by_species[0] = 250_000;
        assert!(budget_allows(&config, &stats, 1, 100_000));
        assert!(!budget_allows(&config, &stats, 1, 100_001));
        let flower_full = VegetationStats {
            triangles: 100_000,
            triangles_by_species: [0, 100_000, 0],
            ..Default::default()
        };
        assert!(!budget_allows(&config, &flower_full, 1, 1));
        assert!(budget_allows(&config, &flower_full, 0, 900_000));
        assert!(budget_allows(
            &config,
            &VegetationStats::default(),
            0,
            1_000_000
        ));
    }
    fn cell(x: i64, y: i64, z: i64) -> (Cell, u8, [f32; 3]) {
        (Cell { x, y, z }, 1, [0.2, 0.4, 0.1])
    }
    #[test]
    fn cuboid_merges_to_six_exposed_quads() {
        let mesh = mesh_cells(
            (0..4).flat_map(|x| (0..3).flat_map(move |y| (0..2).map(move |z| cell(x, y, z)))),
            1,
            || false,
        );
        assert_eq!(mesh.indices.len(), 36);
        assert_eq!(mesh.vertices.len(), 24);
        assert!(mesh.vertices.iter().all(|v| v.ao == 1.));
    }
    #[test]
    fn static_visibility_shades_crevices_without_changing_pigment() {
        let single = mesh_cells([cell(0, 0, 0)].into_iter(), 1, || false);
        assert!(single.vertices.iter().all(|v| v.ao == 1.));
        let cells = [cell(0, 0, 0), cell(1, 1, 0), cell(1, 0, 1)];
        let crevice = mesh_cells(cells.into_iter(), 1, || false);
        assert!(crevice.vertices.iter().any(|v| v.ao < 1.));
        assert!(crevice.vertices.iter().all(|v| v.ao >= 0.65 && v.ao <= 1.));
        assert!(crevice.vertices.iter().all(|v| v.color == [0.2, 0.4, 0.1]));
        // Filling a coarse block cannot amplify static occlusion strength.
        let blocks = BTreeMap::from([
            (
                [0, 0, 0],
                Paint {
                    material: 1,
                    color: [0; 3],
                    ao: 255,
                },
            ),
            (
                [1, 1, 0],
                Paint {
                    material: 1,
                    color: [0; 3],
                    ao: 255,
                },
            ),
        ]);
        assert!(
            face_visibility(&blocks, [0, 0, 0], 0, 1, 8)
                >= face_visibility(&blocks, [0, 0, 0], 0, 1, 1)
        );
        assert_eq!(std::mem::size_of::<VegetationVertex>(), 44);
    }
    #[test]
    fn face_winding_is_outward() {
        let m = mesh_cells([cell(0, 0, 0)].into_iter(), 1, || false);
        for t in m.indices.chunks_exact(3) {
            let a = m.vertices[t[0] as usize];
            let b = m.vertices[t[1] as usize];
            let c = m.vertices[t[2] as usize];
            let u = glam::Vec3::from(b.position) - glam::Vec3::from(a.position);
            let v = glam::Vec3::from(c.position) - glam::Vec3::from(a.position);
            assert!(u.cross(v).dot(glam::Vec3::from(a.normal)) > 0.);
        }
    }
    #[test]
    fn coarse_negative_grid_and_thin_coverage() {
        let m = mesh_cells([cell(-1, 0, 0), cell(0, 0, 0)].into_iter(), 4, || false);
        assert_eq!(m.indices.len(), 36);
        assert!(m.vertices.iter().any(|v| v.position[0] == -0.25));
        assert!(m.vertices.iter().any(|v| v.position[0] == 0.25));
    }
    #[test]
    fn palette_boundaries_do_not_merge() {
        let mut b = cell(1, 0, 0);
        b.1 = 2;
        let m = mesh_cells([cell(0, 0, 0), b].into_iter(), 1, || false);
        assert_eq!(m.indices.len(), 60);
    }
    #[test]
    fn cancellation_emits_no_partial_mesh() {
        assert!(mesh_cells([cell(0, 0, 0)].into_iter(), 1, || true)
            .indices
            .is_empty());
    }
    #[test]
    fn deterministic_eviction_is_bounded() {
        let mut cache = BTreeMap::from([(0, ("a", 2)), (1, ("b", 1)), (2, ("c", 3))]);
        evict(&mut cache, 2, |v| v.1);
        assert_eq!(cache.keys().copied().collect::<Vec<_>>(), vec![0, 2]);
    }
    #[test]
    fn coarse_tree_keeps_separate_wood_and_native_crown_floor() {
        let bark = [0.4, 0.2, 0.1];
        let leaf = [0.1, 0.5, 0.1];
        let mut cells = vec![(Cell::default(), Material::Wood as u8, bark)];
        for y in 1..14 {
            cells.push((Cell { x: 0, y, z: 0 }, Material::Wood as u8, bark));
        }
        for x in -2..3 {
            cells.push((Cell { x, y: 13, z: 0 }, Material::Leaf as u8, leaf));
        }
        let coarse = mesh_plant_cells(Species::Rowan, cells.clone().into_iter(), 8, || false);
        assert!(coarse
            .vertices
            .iter()
            .any(|v| v.color == bark && v.position[1] < 0.5));
        let crown = coarse
            .vertices
            .iter()
            .filter(|v| v.color == leaf)
            .collect::<Vec<_>>();
        assert!(!crown.is_empty());
        assert_eq!(
            crown
                .iter()
                .map(|v| v.position[1])
                .fold(f32::INFINITY, f32::min),
            13. / 16.
        );
        assert!(coarse
            .indices
            .iter()
            .all(|&i| (i as usize) < coarse.vertices.len()));
        assert!(
            mesh_plant_cells(Species::Rowan, cells.clone().into_iter(), 8, || true)
                .vertices
                .is_empty()
        );
        for stride in [1, 2] {
            let baseline = mesh_cells(cells.clone().into_iter(), stride, || false);
            let actual =
                mesh_plant_cells(Species::Rowan, cells.clone().into_iter(), stride, || false);
            assert_eq!(actual.indices, baseline.indices);
            assert_eq!(
                bytemuck::cast_slice::<VegetationVertex, u8>(&actual.vertices),
                bytemuck::cast_slice::<VegetationVertex, u8>(&baseline.vertices)
            );
        }
    }
    #[test]
    fn structural_tree_template_costs_remain_bounded() {
        let mut costs = Vec::new();
        for variant in 0..vegetation::variant_count(Species::Rowan) {
            let p = PlantRecord {
                species: Species::Rowan,
                variant,
                seed: 0,
                root: Cell::default(),
                rotation: 0,
            };
            let model = vegetation::model_for(&p);
            let cells = || {
                model
                    .voxels
                    .iter()
                    .map(|v| (v.cell, v.material as u8, v.color))
            };
            let coarse = mesh_plant_cells(Species::Rowan, cells(), 8, || false);
            let native = mesh_plant_cells(Species::Rowan, cells(), 1, || false);
            costs.push((variant, coarse.indices.len() / 3, native.indices.len() / 3));
            assert!(coarse.indices.len() < native.indices.len());
        }
        eprintln!("structural tree costs (variant, stride8, native): {costs:?}");
        assert!(costs.iter().map(|v| v.1).max().unwrap() * 64 <= 450_000);
    }
    #[test]
    fn coarse_ground_keeps_low_native_carpet_height() {
        let cells = [cell(1, 101, 1), cell(6, 102, 6), cell(2, 101, 6)];
        let mesh = mesh_ground_cells(cells.into_iter(), 8, || false);
        assert!(mesh
            .vertices
            .iter()
            .all(|v| v.position[1] >= 100. * VOXEL_SIZE as f32
                && v.position[1] <= 104. * VOXEL_SIZE as f32));
        assert!(!mesh.indices.is_empty());
        assert!(mesh_ground_cells(cells.into_iter(), 8, || true)
            .indices
            .is_empty());
    }
    #[test]
    fn coarse_palette_keeps_mean_coverage() {
        let a = (Cell { x: 0, y: 0, z: 0 }, 1, [1., 0., 0.]);
        let b = (Cell { x: 1, y: 0, z: 0 }, 2, [0., 0., 1.]);
        let mesh = mesh_cells([a, b].into_iter(), 2, || false);
        assert_eq!(mesh.indices.len(), 36);
        assert!(mesh.vertices.iter().all(|v| v.color == [0.5, 0., 0.5]));
    }
    #[test]
    fn settled_static_frames_reuse_mesh_arcs_and_invalidate_on_inputs() {
        let world = World::new(37).with_vegetation();
        let config = VegetationConfig {
            grass_radius_m: 2.,
            flower_radius_m: 0.,
            tree_radius_m: 0.,
            max_tiles: 16,
            ..Default::default()
        };
        let camera = [20., world.height_m(20., 0.) + 1.7, 0.];
        let mut stream = VegetationStream::new(world.clone(), config);
        let mut settled = None;
        for _ in 0..2000 {
            let frame = stream.update(camera, 900, 1., world.revision());
            if frame.stats.pending_jobs == 0 {
                settled = Some(frame);
                break;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }
        let first = settled.expect("small canonical grass demand settles");
        assert!(
            !first.batches.is_empty(),
            "fixture must exercise real mesh Arc reuse"
        );
        let builds = stream.regroup_builds;
        let tick = stream.tick;
        let second = stream.update(camera, 900, 1., world.revision());
        assert_eq!(
            stream.regroup_builds, builds,
            "settled hit must skip record grouping and jobs"
        );
        assert_eq!(stream.tick, tick, "idle reuse preserves relative LRU ages");
        assert_eq!(first.batches.len(), second.batches.len());
        for (a, b) in first.batches.iter().zip(&second.batches) {
            assert_eq!(a.key, b.key);
            assert!(Arc::ptr_eq(&a.mesh, &b.mesh));
            let instances = |b: &VegetationBatch| {
                b.instances
                    .iter()
                    .map(|i| (i.root_cell, i.quarter_turn, i.fade.to_bits()))
                    .collect::<Vec<_>>()
            };
            assert_eq!(instances(a), instances(b));
        }
        let moved = [camera[0] + 0.125, camera[1], camera[2]];
        stream.update(moved, 900, 1., world.revision());
        assert_eq!(stream.regroup_builds, builds + 1);
        stream.config.pixel_error += 0.125;
        stream.update(moved, 900, 1., world.revision());
        assert_eq!(stream.regroup_builds, builds + 2);
        stream.update_view(moved, 900, 1., world.revision(), [0., 0., 1.], 16. / 9.);
        assert_eq!(stream.regroup_builds, builds + 3);
        stream.set_world(World::new(38).with_vegetation());
        assert!(
            stream.stable_frame.is_none(),
            "new snapshot releases retained frame before eviction"
        );
        stream.update(moved, 900, 1., 0);
        assert_eq!(stream.regroup_builds, builds + 4);
    }
    #[test]
    fn missing_neighbor_coverage_does_not_demote_resident_ground_detail() {
        // Exercise the real selector with cheap, deterministic cached geometry.
        // A new tile's missing baseline must not reset every existing tile to 8.
        let world = World::new(7);
        let mut stream = VegetationStream::new(
            world.clone(),
            VegetationConfig {
                grass_radius_m: 20.,
                flower_radius_m: 0.,
                tree_radius_m: 0.,
                ..Default::default()
            },
        );
        let tile = Tile {
            species: 0,
            x: 0,
            z: 0,
        };
        let plant = PlantRecord {
            species: Species::Grass,
            variant: 0,
            seed: 123,
            root: Cell { x: 64, y: 0, z: 64 },
            rotation: 0,
        };
        stream.tiles.insert(tile, (vec![plant], 0));
        let batch = PlantRecord {
            variant: u8::MAX,
            seed: mix(world.seed() ^ mix(0) ^ 0u64.rotate_left(31)),
            root: Cell { x: 0, y: 0, z: 0 },
            ..plant
        };
        let mesh = Arc::new(mesh_ground_cells(
            [(plant.root, Material::Grass as u8, [0.2, 0.5, 0.1])].into_iter(),
            1,
            || false,
        ));
        for lod in [1, 2, 4, 8] {
            let key = ground_key(&batch, lod, 0);
            stream.mesh_costs.insert(key, (mesh.indices.len() / 3, 0));
            stream.meshes.insert(key, (mesh.clone(), 0));
        }
        let camera = [4., 1.7, 4.];
        let before = stream.update(camera, 720, 1., world.revision());
        assert_eq!(
            before
                .batches
                .iter()
                .find(|b| b.key.tile == [0, 0])
                .unwrap()
                .key
                .lod,
            1
        );
        // Deliver the new placement through the real completion/invalidation
        // path, as opposed to mutating a cached source behind its revision key.
        let mut records = stream.tiles[&tile].0.clone();
        records.push(PlantRecord {
            root: Cell {
                x: 192,
                y: 0,
                z: 64,
            },
            seed: 456,
            ..plant
        });
        let (done, receive) = mpsc::sync_channel(1);
        stream.rx = receive;
        done.send((
            stream.epoch.load(Ordering::Relaxed),
            Done::Tile(tile, world.revision(), records),
        ))
        .unwrap();
        let during = stream.update(camera, 720, 1., world.revision());
        assert_eq!(
            during
                .batches
                .iter()
                .find(|b| b.key.tile == [0, 0])
                .unwrap()
                .key
                .lod,
            1,
            "unrelated missing coverage must not flash an already resident tile coarse"
        );
        assert!(during.stats.retained_ground_detail > 0);
        let farther = stream.update([25., 1.7, 4.], 720, 1., world.revision());
        assert_eq!(
            farther
                .batches
                .iter()
                .find(|b| b.key.tile == [0, 0])
                .unwrap()
                .key
                .lod,
            4,
            "retention must still respect distance-based quality"
        );
        stream.config.max_triangles = mesh.indices.len() / 3 - 1;
        let constrained = stream.update(camera, 720, 1., world.revision());
        assert!(
            constrained.stats.triangles <= stream.config.max_triangles,
            "retention must not bypass the hard geometry budget"
        );
    }
    #[test]
    fn static_frame_key_uses_exact_pose_projection_and_config_bits() {
        let config = VegetationConfig::default();
        let key = |camera, height, fov, revision, config: &VegetationConfig, view| {
            StableFrameKey::new(camera, height, fov, revision, config, view)
        };
        let baseline = key([0.; 3], 900, 1., 0, &config, None);
        assert_ne!(baseline, key([-0., 0., 0.], 900, 1., 0, &config, None));
        assert_ne!(baseline, key([0.; 3], 901, 1., 0, &config, None));
        assert_ne!(baseline, key([0.; 3], 900, 1.1, 0, &config, None));
        assert_ne!(baseline, key([0.; 3], 900, 1., 1, &config, None));
        let mut changed = config.clone();
        changed.max_meshes += 1;
        assert_ne!(baseline, key([0.; 3], 900, 1., 0, &changed, None));
        let view = CullView::new([0.; 3], [0., 0., 1.], 1., 16. / 9.);
        assert_ne!(baseline, key([0.; 3], 900, 1., 0, &config, view));
    }
    #[test]
    fn stream_settles_with_bounded_tiles_and_rejects_stale_revision() {
        // Disabled vegetation isolates scheduling/cancellation from model work.
        let world = World::new(7);
        let config = VegetationConfig {
            max_tiles: 8,
            ..Default::default()
        };
        let mut stream = VegetationStream::new(world, config);
        let mut settled = false;
        for _ in 0..1000 {
            let frame = stream.update([-0.1, 20., -0.1], 1080, 1., 0);
            assert!(frame.stats.resident_tiles <= 8);
            assert!(frame.batches.is_empty());
            if frame.stats.pending_jobs == 0 {
                settled = true;
                break;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(settled, "bounded empty-tile work should settle");
        let frame = stream.update([5000., 20., 5000.], 1080, 1., 1);
        assert!(frame.batches.is_empty());
        assert_ne!(frame.stats.pending_jobs, 0);
    }
}
