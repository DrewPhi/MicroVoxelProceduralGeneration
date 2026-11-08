//! Bounded, camera-demand vegetation streaming. Canonical placement stays in world;
//! automatically derived display meshes never change gameplay voxel identity.
use frontier_world::{
    vegetation::{self, PlantRecord, Species},
    Cell, Material, World, VOXEL_SIZE,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
};

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VegetationVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    /// Static display-only ambient visibility; never modifies canonical pigment.
    pub ao: f32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct MeshKey {
    pub species: u8,
    pub variant: u8,
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
}
pub struct VegetationFrame {
    pub origin: [i64; 3],
    pub batches: Vec<VegetationBatch>,
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
            max_meshes: 128,
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
fn ground_density_fade(distance: f64, radius: f64, rank: f64) -> f32 {
    let t = ((distance / radius.max(0.001) - 0.25) / 0.75).clamp(0., 1.);
    let density = 1. - 0.65 * t * t;
    // The upper end of the interval is density+0.1, so density=1 gives
    // complete native coverage for every stable rank, including rank=1.
    ((density + 0.1 - rank) / 0.1).clamp(0., 1.) as f32
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
        // Canonical models are prewarmed at scene startup. The bounding sphere
        // encloses the full rotated box, including automatic coarse-cell expansion.
        let model = vegetation::model_for(p);
        let min = glam::DVec3::new(model.min.x as f64, model.min.y as f64, model.min.z as f64);
        let max = glam::DVec3::new(model.max.x as f64, model.max.y as f64, model.max.z as f64);
        let pivot = glam::DVec3::new(0.5, 0., 0.5);
        let center = (min + max) * 0.5 - pivot;
        let center = match p.rotation % 4 {
            1 => glam::DVec3::new(-center.z, center.y, center.x),
            2 => glam::DVec3::new(-center.x, center.y, -center.z),
            3 => glam::DVec3::new(center.z, center.y, -center.x),
            _ => center,
        };
        let root = glam::DVec3::new(p.root.x as f64, p.root.y as f64, p.root.z as f64);
        let radius = ((max - min) * 0.5).length() * VOXEL_SIZE + 7. * VOXEL_SIZE * 3f64.sqrt();
        self.intersects_sphere((root + pivot + center) * VOXEL_SIZE, radius)
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

pub struct VegetationStream {
    world: World,
    edits: Vec<Cell>,
    pub config: VegetationConfig,
    tx: Option<SyncSender<Job>>,
    rx: Receiver<Done>,
    worker: Option<thread::JoinHandle<()>>,
    epoch: Arc<AtomicU64>,
    tiles: BTreeMap<Tile, (Vec<PlantRecord>, u64)>,
    meshes: BTreeMap<MeshKey, (Arc<VegetationMesh>, u64)>,
    pending_tiles: BTreeSet<Tile>,
    pending_meshes: BTreeSet<MeshKey>,
    tick: u64,
    center: Option<[i64; 2]>,
}
impl VegetationStream {
    pub fn new(world: World, config: VegetationConfig) -> Self {
        // Read once when the stream is created, never inside frame selection.
        let reference_flag = std::env::var("FRONTIER_VEGETATION_REFERENCE").ok();
        let config = config.with_reference_flag(reference_flag.as_deref());
        let (tx, jobs) = mpsc::sync_channel::<Job>(4);
        let (results, rx) = mpsc::sync_channel(8);
        let epoch = Arc::new(AtomicU64::new(1));
        let cancel = epoch.clone();
        let worker = thread::Builder::new()
            .name("vegetation-stream".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let generation = match &job {
                        Job::Tile(_, _, e) | Job::Mesh(_, _, _, e) => *e,
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
                            let model = vegetation::model_for(&p);
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
                            let mesh = mesh_plant_cells(p.species, cells, key.lod, || {
                                generation != cancel.load(Ordering::Relaxed)
                            });
                            Done::Mesh(key, w.revision(), Arc::new(mesh))
                        }
                    };
                    if generation != cancel.load(Ordering::Relaxed) {
                        continue;
                    }
                    // Never block cancellation behind a stalled renderer/full result queue.
                    let mut output = done;
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
            .expect("vegetation worker");
        Self {
            edits: world.edit_cells().collect(),
            world,
            config,
            tx: Some(tx),
            rx,
            worker: Some(worker),
            epoch,
            tiles: BTreeMap::new(),
            meshes: BTreeMap::new(),
            pending_tiles: BTreeSet::new(),
            pending_meshes: BTreeSet::new(),
            tick: 0,
            center: None,
        }
    }
    /// Replace the immutable snapshot after edits; no stale plant mesh survives.
    pub fn set_world(&mut self, world: World) {
        self.edits = world.edit_cells().collect();
        self.world = world;
        self.epoch.fetch_add(1, Ordering::Relaxed);
        self.tiles.clear();
        self.meshes.retain(|k, _| k.edit_id == 0);
        self.pending_tiles.clear();
        self.pending_meshes.clear();
        self.center = None;
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
        self.tick = self.tick.wrapping_add(1);
        let c = Cell::from_meters(camera);
        let center = [c.x.div_euclid(TILE_CELLS), c.z.div_euclid(TILE_CELLS)];
        if self.center != Some(center) {
            self.center = Some(center);
            self.epoch.fetch_add(1, Ordering::Relaxed);
            self.pending_tiles.clear();
            self.pending_meshes.clear();
        }
        while let Ok(done) = self.rx.try_recv() {
            match done {
                Done::Tile(t, r, p) => {
                    self.pending_tiles.remove(&t);
                    if r == self.world.revision() {
                        self.tiles.insert(t, (p, self.tick));
                    }
                }
                Done::Mesh(k, r, m) => {
                    self.pending_meshes.remove(&k);
                    if r == self.world.revision() {
                        self.meshes.insert(k, (m, self.tick));
                    }
                }
            }
        }
        let mut stats = VegetationStats::default();
        let mut batches: BTreeMap<MeshKey, VegetationBatch> = BTreeMap::new();
        // A revision without its new immutable snapshot must never resurrect edits.
        if world_revision != self.world.revision() {
            stats.pending_jobs = 1;
            return VegetationFrame {
                origin: [c.x, c.y, c.z],
                batches: Vec::new(),
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
            let n = (radius / 32.).ceil() as i64 + 1;
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
                    if d <= radius + 23. {
                        demand.push((d, t));
                    }
                }
            }
        }
        demand.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        demand.truncate(self.config.max_tiles);
        let epoch = self.epoch.load(Ordering::Relaxed);
        let mut submitted = 0;
        let mut plants = Vec::new();
        for (_, t) in demand {
            if let Some((records, used)) = self.tiles.get_mut(&t) {
                *used = self.tick;
                for p in records.iter() {
                    let dx = (p.root.x - c.x) as f64 * VOXEL_SIZE;
                    let dz = (p.root.z - c.z) as f64 * VOXEL_SIZE;
                    let distance = (dx * dx + dz * dz).sqrt();
                    if distance < radii[t.species as usize] {
                        if view.as_ref().is_none_or(|v| v.plant_visible(p)) {
                            plants.push((distance, *p));
                        } else {
                            stats.culled_instances += 1;
                        }
                    }
                }
            } else {
                stats.missing_tiles += 1;
                if submitted < 1
                    && !self.pending_tiles.contains(&t)
                    && self
                        .tx
                        .as_ref()
                        .unwrap()
                        .try_send(Job::Tile(t, self.world.clone(), epoch))
                        .is_ok()
                {
                    self.pending_tiles.insert(t);
                    submitted += 1;
                }
            }
        }
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
        // Reserve cheap geometry for every visible tree before any near tree
        // spends the refinement budget. Baseline templates have priority over
        // fine requests, and their actual costs replace speculative estimates.
        let mut remaining_tree_coarse = 0usize;
        let mut tree_coverage_ready = true;
        for (_, p) in plants
            .iter()
            .filter(|(_, p)| !self.config.reference && p.species == Species::Rowan)
        {
            let key = MeshKey {
                species: 2,
                variant: p.variant,
                lod: 8,
                edit_id: plant_edit_id(p, edits, self.world.revision()),
            };
            if let Some((mesh, _)) = self.meshes.get(&key) {
                remaining_tree_coarse =
                    remaining_tree_coarse.saturating_add(mesh.indices.len() / 3);
            } else {
                tree_coverage_ready = false;
                stats.missing_meshes += 1;
                if submitted < 2
                    && !self.pending_meshes.contains(&key)
                    && self
                        .tx
                        .as_ref()
                        .unwrap()
                        .try_send(Job::Mesh(key, *p, self.world.clone(), epoch))
                        .is_ok()
                {
                    self.pending_meshes.insert(key);
                    submitted += 1;
                }
            }
        }
        let projection = viewport_height as f64 / (2. * (fov_y as f64 * 0.5).tan().max(0.01));
        for (distance, p) in plants {
            let s = species_id(p.species);
            let radius = radii[s as usize];
            let mut fade = ((radius - distance) / (radius * 0.2).max(1.)).clamp(0., 1.) as f32;
            // Stable density thinning for ground cover only. Tall tree silhouettes
            // remain present until the outer range fade, with no distance lottery.
            let rank = mix(p.seed) as f64 / u64::MAX as f64;
            if s != 2 {
                fade *= ground_density_fade(distance, radius, rank);
            }
            if fade <= 0. {
                continue;
            }
            let pixel = VOXEL_SIZE * projection / distance.max(1.);
            let mut lod = 1;
            while !self.config.reference
                && lod < 8
                && pixel * (lod * 2) as f64 <= self.config.pixel_error as f64
            {
                lod *= 2;
            }
            // More conservative tree contour than ground cover.
            if s == 2 {
                lod = lod.min(4);
            }
            let edit_id = plant_edit_id(&p, edits, self.world.revision());
            let key = MeshKey {
                species: s,
                variant: p.variant,
                lod,
                edit_id,
            };
            let coarse_key = MeshKey { lod: 8, ..key };
            let tree_headroom = if s == 2 && !self.config.reference {
                if let Some((mesh, _)) = self.meshes.get(&coarse_key) {
                    remaining_tree_coarse =
                        remaining_tree_coarse.saturating_sub(mesh.indices.len() / 3);
                }
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
            let mut request_key = key;
            if s == 2 && !self.config.reference {
                if !tree_coverage_ready {
                    request_key = coarse_key;
                } else {
                    while request_key.lod < 8 {
                        match self.meshes.get(&request_key) {
                            Some((mesh, _)) if mesh.indices.len() / 3 > tree_headroom => {
                                request_key.lod *= 2
                            }
                            _ => break,
                        }
                    }
                }
            }
            if !self.meshes.contains_key(&request_key) {
                stats.missing_meshes += 1;
                if submitted < 2
                    && !self.pending_meshes.contains(&request_key)
                    && self
                        .tx
                        .as_ref()
                        .unwrap()
                        .try_send(Job::Mesh(request_key, p, self.world.clone(), epoch))
                        .is_ok()
                {
                    self.pending_meshes.insert(request_key);
                    submitted += 1;
                }
            }
            let render_key = if self.config.reference {
                // A missing native mesh stays pending; never substitute an LOD
                // proxy in the reference target.
                key
            } else if s == 2 {
                self.meshes
                    .iter()
                    .filter(|(k, (mesh, _))| {
                        k.species == 2
                            && k.variant == key.variant
                            && k.edit_id == key.edit_id
                            && k.lod >= key.lod
                            && mesh.indices.len() / 3 <= tree_headroom
                    })
                    .min_by_key(|(k, _)| k.lod)
                    .map(|(k, _)| *k)
                    .unwrap_or(coarse_key)
            } else if self.meshes.contains_key(&key) {
                key
            } else {
                self.meshes
                    .keys()
                    .filter(|k| {
                        k.species == key.species
                            && k.variant == key.variant
                            && k.edit_id == key.edit_id
                    })
                    .min_by_key(|k| (k.lod.abs_diff(key.lod), k.lod))
                    .copied()
                    .unwrap_or(key)
            };
            if let Some((mesh, used)) = self.meshes.get_mut(&render_key) {
                *used = self.tick;
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
        evict(&mut self.tiles, self.config.max_tiles, |v| v.1);
        evict(&mut self.meshes, self.config.max_meshes, |v| v.1);
        stats.resident_tiles = self.tiles.len();
        stats.resident_meshes = self.meshes.len();
        stats.pending_jobs = stats.missing_tiles + stats.missing_meshes;
        VegetationFrame {
            origin: [c.x, c.y, c.z],
            batches: batches.into_values().collect(),
            stats,
        }
    }
}
impl Drop for VegetationStream {
    fn drop(&mut self) {
        self.epoch.fetch_add(1, Ordering::Relaxed);
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
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
    let step = i64::from(stride.max(1));
    let mut groups: BTreeMap<[i64; 3], BTreeMap<u8, (usize, [f64; 3])>> = BTreeMap::new();
    for (i, (c, mat, color)) in cells.enumerate() {
        if i % 1024 == 0 && cancel() {
            return VegetationMesh::default();
        }
        let key = [
            c.x.div_euclid(step),
            c.y.div_euclid(step),
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
                p[d] = (plane * step) as f32 * VOXEL_SIZE as f32;
                p[u] = ((start[0] + a) * step) as f32 * VOXEL_SIZE as f32;
                p[v] = ((start[1] + b) * step) as f32 * VOXEL_SIZE as f32;
                mesh.vertices.push(VegetationVertex {
                    position: p,
                    normal,
                    color: paint.color.map(f32::from_bits),
                    ao: paint.ao as f32 / 255.,
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
    fn near_ground_cover_has_complete_native_coverage() {
        for rank in [0., 0.5, 0.95, 0.999999, 1.] {
            for distance in [0., 1., 7.] {
                assert_eq!(ground_density_fade(distance, 28., rank), 1.);
            }
        }
        assert!(ground_density_fade(28., 28., 0.4) > 0.);
        assert!(ground_density_fade(28., 28., 0.4) < 1.);
        assert_eq!(ground_density_fade(28., 28., 0.5), 0.);
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
        assert_eq!(std::mem::size_of::<VegetationVertex>(), 40);
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
    fn coarse_palette_keeps_mean_coverage() {
        let a = (Cell { x: 0, y: 0, z: 0 }, 1, [1., 0., 0.]);
        let b = (Cell { x: 1, y: 0, z: 0 }, 2, [0., 0., 1.]);
        let mesh = mesh_cells([a, b].into_iter(), 2, || false);
        assert_eq!(mesh.indices.len(), 36);
        assert!(mesh.vertices.iter().all(|v| v.color == [0.5, 0., 0.5]));
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
