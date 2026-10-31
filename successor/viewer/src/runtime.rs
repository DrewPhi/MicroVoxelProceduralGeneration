//! Coordinates background planning, immutable meshes and GPU-resident covers.
//! Local balanced patches swap only when their surfaces and stitches are ready.
use crate::{
    camera::Camera,
    gpu::Gpu,
    player_view::{self, ViewMode},
    streaming::{Payload, Workers},
};
use microvoxel::player::{Player, PlayerInput};
use microvoxel::{
    generation::debug::{self, DebugLayer},
    generation::Terrain,
    queries::{raycast, Trace},
    render::lod::{self, Layout, MeshKey},
    World,
};
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::keyboard::KeyCode;

pub struct Runtime {
    pub native_policy_radius: f64,
    pub max_plan_time: Duration,
    pub gpu: Gpu,
    pub camera: Camera,
    pub player: Player,
    pub flying: bool,
    pub view: ViewMode,
    view_key: Option<(microvoxel::CellPos, u64, u64, u64, u32, u32, ViewMode)>,
    pub world: Arc<World<Terrain>>,
    workers: Workers,
    pub active: Layout,
    pub cover: BTreeSet<MeshKey>,
    target: Option<(Layout, BTreeSet<MeshKey>)>,
    prefetch: BTreeSet<MeshKey>,
    warmed: BTreeSet<MeshKey>,
    pub prefetch_builds: usize,
    pub prefetch_hits: usize,
    pub cache_hits: usize,
    prefetch_enabled: bool,
    prefetch_pause_until: Instant,
    last_motion_eye: [f64; 3],
    velocity: [f64; 3],
    last_plan: Instant,
    last_cell: microvoxel::CellPos,
    last_fov: f64,
    last_height: u32,
    last_visibility: f32,
    pub tile_budget: usize,
    pub settings: crate::settings::Settings,
    pub desired: Layout,
    pub achieved_pixels: f64,
    pub budget_limited: bool,
    pub memory_pressure: bool,
    last_direction: [f64; 3],
    effective_budget: usize,
    planner: crate::lod_planner::Planner,
    planning: bool,
    plan_generation: u64,
    initial_ready: bool,
    still_capture: bool,
    pub completed: usize,
    pub mesh_time: Duration,
    pub visible: usize,
    pub last_error: Option<String>,
    pub debug_layer: Option<DebugLayer>,
}
impl Runtime {
    pub fn new(
        gpu: Gpu,
        world: Arc<World<Terrain>>,
        tile_budget: usize,
        spawn_nonce: u64,
    ) -> microvoxel::Result<Self> {
        let spawn = Self::spawn(&world, spawn_nonce)?;
        Self::with_spawn(gpu, world, tile_budget, spawn)
    }
    pub fn with_spawn(
        mut gpu: Gpu,
        world: Arc<World<Terrain>>,
        tile_budget: usize,
        spawn: (Camera, Player),
    ) -> microvoxel::Result<Self> {
        gpu.grid = lod::Grid::new(world.spec());
        gpu.terrain_lighting(&world);
        let (camera, player) = spawn;
        let height = gpu.height;
        let cell = camera.cell;
        let fov = camera.fov;
        let workers = Workers::new(Arc::clone(&world));
        println!("{} mesh workers; tile budget {tile_budget}", workers.count);
        let active = Layout::root_in(gpu.grid);
        let keys = local_keys(&active, &world).into_values().collect();
        let mut runtime = Self {
            native_policy_radius: 6.,
            max_plan_time: Duration::ZERO,
            gpu,
            camera,
            player,
            flying: false,
            view: ViewMode::First,
            view_key: None,
            world: Arc::clone(&world),
            workers,
            active: active.clone(),
            cover: BTreeSet::new(),
            target: Some((active.clone(), keys)),
            prefetch: BTreeSet::new(),
            warmed: BTreeSet::new(),
            prefetch_builds: 0,
            prefetch_hits: 0,
            cache_hits: 0,
            prefetch_enabled: std::env::var_os("MICROVOXEL_DISABLE_PREFETCH").is_none(),
            prefetch_pause_until: Instant::now(),
            last_motion_eye: world.spec().grid.point(cell),
            velocity: [0.; 3],
            last_plan: Instant::now() - Duration::from_secs(1),
            last_cell: cell,
            last_fov: fov,
            last_height: height,
            last_visibility: 24000.,
            tile_budget,
            settings: crate::settings::Settings::default(),
            desired: active.clone(),
            achieved_pixels: 2.,
            budget_limited: false,
            memory_pressure: false,
            last_direction: [0.; 3],
            effective_budget: tile_budget.min(crate::settings::Settings::default().tile_budget()),
            planner: crate::lod_planner::Planner::new(Arc::clone(&world)),
            planning: false,
            plan_generation: 0,
            initial_ready: false,
            still_capture: false,
            completed: 0,
            mesh_time: Duration::ZERO,
            visible: 0,
            last_error: None,
            debug_layer: None,
        };
        runtime.sync_view();
        Ok(runtime)
    }
    pub(crate) fn spawn(
        world: &World<Terrain>,
        nonce: u64,
    ) -> microvoxel::Result<(Camera, Player)> {
        let mut player = Player::with_scale(
            world.identity().seed,
            microvoxel::CellPos::default(),
            world.spec().grid,
        )?;
        for attempt in 0..128u64 {
            let camera = Camera::spawn(world, nonce.wrapping_add(attempt * 101));
            if attempt < 100
                && world
                    .environment_at(camera.cell)
                    .is_some_and(|e| e.slope > 0.12 || e.soil_depth_m < 0.8)
            {
                continue;
            }
            if player.place_above(world, camera.cell.x, camera.cell.z) {
                return Ok((camera, player));
            }
        }
        Err("Could not find room for the voxel player".into())
    }
    /// Dropping the rest closes old worker/planner queues and their result channels.
    pub fn into_gpu(self) -> Gpu {
        self.gpu
    }
    pub fn respawn(&mut self, nonce: u64) -> microvoxel::Result<()> {
        let (camera, player) = Self::spawn(&self.world, nonce)?;
        self.camera = camera;
        self.player = player;
        self.view_key = None;
        self.sync_view();
        Ok(())
    }
    pub fn clear_controls(&mut self) {
        self.camera.clear();
        self.player.reset_input();
    }
    pub fn toggle_flight(&mut self) {
        self.clear_controls();
        self.flying = !self.flying;
        self.camera.reverse = false;
        self.view_key = None;
        if !self.flying {
            self.sync_view();
        }
        println!(
            "{}",
            if self.flying {
                "Debug flight (F6 returns to player)"
            } else {
                "Walking player"
            }
        );
    }
    pub fn set_view(&mut self, view: ViewMode) {
        self.view = view;
        self.view_key = None;
        self.sync_view();
    }
    pub fn cycle_view(&mut self) {
        self.view = self.view.next();
        self.view_key = None;
        self.sync_view();
        println!("{}", self.view.name());
    }
    fn sync_view(&mut self) {
        if self.flying {
            return;
        }
        let key = (
            self.player.root,
            self.camera.yaw.to_bits(),
            self.camera.pitch.to_bits(),
            self.camera.fov.to_bits(),
            self.gpu.width,
            self.gpu.height,
            self.view,
        );
        if self.view_key != Some(key) {
            player_view::sync(
                &self.world,
                &self.player,
                &mut self.camera,
                self.view,
                self.gpu.width,
                self.gpu.height,
            );
            self.view_key = Some(key);
        }
    }
    /// Commit a local cell transaction without resetting the player, horizon,
    /// or unaffected GPU pages. Old workers own an immutable earlier revision.
    pub fn apply_cell_changes(
        &mut self,
        actions: &[microvoxel::EditAction],
    ) -> Result<microvoxel::world::ChangeSet, String> {
        let mut world = self.world.fork_revision();
        let changes = world.apply_actions(actions).map_err(|e| e.to_string())?;
        if changes.changed_cells.is_empty() {
            return Ok(changes);
        }
        self.prefetch.clear();
        self.warmed.clear();
        self.world = Arc::new(world);
        self.workers.replace_world(Arc::clone(&self.world));
        self.planner.replace_world(Arc::clone(&self.world));
        self.plan_generation += 1;
        self.planning = false;
        // Retain unchanged regional revisions even though world revision rose.
        let keys = local_keys(&self.active, &self.world)
            .into_values()
            .collect();
        self.target = Some((self.active.clone(), keys));
        self.last_fov = -1.;
        Ok(changes)
    }
    pub fn refresh_terrain(&mut self) {
        self.last_fov = -1.;
        self.last_plan = Instant::now() - Duration::from_secs(1);
        self.update(0.);
    }
    pub fn loading(&self) -> (usize, usize) {
        self.target
            .as_ref()
            .map_or((self.cover.len(), self.cover.len()), |(_, keys)| {
                (
                    keys.iter()
                        .filter(|k| self.gpu.ready(k, self.settings.ray))
                        .count(),
                    keys.len(),
                )
            })
    }
    pub fn settled(&self) -> bool {
        !self.planning
            && self.target.is_none()
            && self.active == self.desired
            && self.active.tiles.len() > 1
    }
    pub fn update(&mut self, dt: f64) {
        if self.debug_layer.is_none() {
            if self.flying {
                self.camera.update(&self.world, dt.min(0.25));
            } else {
                let axis = |a, b| {
                    i32::from(self.camera.keys.contains(&a))
                        - i32::from(self.camera.keys.contains(&b))
                };
                let input = PlayerInput {
                    forward: axis(KeyCode::KeyW, KeyCode::KeyS) as f64,
                    strafe: axis(KeyCode::KeyD, KeyCode::KeyA) as f64,
                    yaw: self.camera.yaw,
                    pitch: self.camera.pitch,
                    sprint: self.camera.keys.contains(&KeyCode::ShiftLeft)
                        || self.camera.keys.contains(&KeyCode::ShiftRight),
                    jump: self.camera.keys.contains(&KeyCode::Space),
                    arm_forward: self.view == ViewMode::First,
                };
                if let Err(error) = self.player.advance(&self.world, dt, input) {
                    self.last_error = Some(error);
                }
                self.sync_view();
            }
        }
        if self.last_error.is_some() {
            return;
        }
        let was_moving = self.velocity[0].hypot(self.velocity[2]) > 0.1;
        if dt > 0. {
            let eye = self.camera.eye();
            for axis in [0, 2] {
                let v =
                    ((eye[axis] - self.last_motion_eye[axis]) / dt.max(0.001)).clamp(-100., 100.);
                self.velocity[axis] = self.velocity[axis] * 0.8 + v * 0.2;
            }
            self.last_motion_eye = eye;
        }
        if !was_moving
            && self.velocity[0].hypot(self.velocity[2]) > 0.1
            && !self.still_capture
            && !self.cover.is_empty()
        {
            // An idle refinement batch must not hold near detail behind far jobs.
            // Keep the published cover; cached completed meshes remain reusable.
            self.target = None;
            self.plan_generation += 1;
            self.planning = false;
            self.last_plan = Instant::now() - Duration::from_secs(1);
            self.last_fov = -1.;
        }
        // Camera changes update intent even while a small ready patch is loading.
        let direction = self.camera.direction();
        // Speculation is useful only while the immediate walking neighborhood
        // is already detailed. Pause it when travel outruns required refinement.
        let native_guard = self.native_policy_radius.min(3.);
        let eye = self.camera.eye();
        let near_detail_ready = (0..9).all(|i| {
            let angle = i as f64 * std::f64::consts::TAU / 8.;
            let radius = if i == 8 { 0. } else { native_guard };
            let x = ((eye[0] + angle.cos() * radius) / self.gpu.grid.cell_m()).floor() as i64;
            let z = ((eye[2] + angle.sin() * radius) / self.gpu.grid.cell_m()).floor() as i64;
            self.active
                .at(x, z)
                .is_some_and(|k| k.level == self.gpu.grid.finest)
        });
        if !near_detail_ready {
            self.prefetch.clear();
        }
        // Distant coverage is retained independently of native refinement.
        self.gpu.visibility_limit = 24000.;

        let moved = (!self.settled() && self.target.is_none() && !self.planning)
            || (self.gpu.visibility_limit - self.last_visibility).abs()
                > self.last_visibility.max(1.) * 0.15
            || self.camera.cell != self.last_cell
            || self.camera.fov != self.last_fov
            || self.gpu.height != self.last_height
            || direction
                .iter()
                .zip(self.last_direction)
                .map(|(a, b)| (a - b).abs())
                .sum::<f64>()
                > 0.2;
        if moved && !self.planning && self.last_plan.elapsed() > Duration::from_millis(500) {
            self.plan_generation += 1;
            self.planner.request(crate::lod_planner::Request {
                generation: self.plan_generation,
                // Keep protected detail centered on the player even when its radius
                // is capped. Speed still expands the prefetch neighborhood.
                eye: self.camera.eye(),
                height: self.gpu.height,
                fov: self.camera.fov,
                previous: self.desired.clone(),
                budget: self.effective_budget,
                prefetch_eye: (self.prefetch_enabled
                    && near_detail_ready
                    && !self.still_capture
                    && !self.memory_pressure
                    && self
                        .active
                        .at(self.camera.cell.x, self.camera.cell.z)
                        .is_some_and(|k| k.level + 1 >= self.gpu.grid.finest))
                .then(|| {
                    crate::streaming_policy::prediction(
                        self.camera.eye(),
                        self.velocity,
                        direction,
                        32. * self.gpu.grid.cell_m(),
                        self.native_policy_radius,
                    )
                }),
                quality: lod::Quality {
                    pixels: self.settings.pixels(),
                    native_radius: (self.settings.native_radius
                        + self.velocity[0].hypot(self.velocity[2]) * 0.5)
                        .min(32.),
                    direction,
                    visibility: self.gpu.visibility_limit as f64,
                },
            });
            self.planning = true;
            self.last_plan = Instant::now();
            self.last_cell = self.camera.cell;
            self.last_fov = self.camera.fov;
            self.last_height = self.gpu.height;
            self.last_visibility = self.gpu.visibility_limit;
            self.last_direction = direction;
        }
        while let Ok(result) = self.planner.results.try_recv() {
            if !self.planner.accepts(&result) || result.generation != self.plan_generation {
                continue;
            }
            self.prefetch = result
                .prefetch
                .into_iter()
                .filter(|_| near_detail_ready)
                .map(|mut key| {
                    let [x, z] = key.tile.origin_in(self.gpu.grid);
                    let halo = key.tile.step_in(self.gpu.grid) * 2;
                    key.revision = self.world.region_revision(
                        x - halo,
                        z - halo,
                        key.tile.edge_in(self.gpu.grid) + halo * 2,
                    );
                    key
                })
                .collect();
            self.max_plan_time = self.max_plan_time.max(result.elapsed);
            self.achieved_pixels = result.selection.achieved_pixels;
            self.native_policy_radius = result.selection.native_policy_radius;
            self.budget_limited = result.selection.budget_limited || self.memory_pressure;
            self.desired = result.selection.layout;
            if !self.still_capture {
                let obsolete = self.target.as_ref().is_some_and(|(target, _)| {
                    target.tiles.difference(&self.active.tiles).any(|key| {
                        let [x, z] = key.origin_in(self.gpu.grid);
                        let desired = self.desired.at(x, z).map_or(key.level, |k| k.level);
                        let active = self.active.at(x, z).map_or(key.level, |k| k.level);
                        if key.level > active {
                            desired < key.level
                        } else {
                            desired > key.level
                        }
                    })
                });
                if obsolete {
                    self.target = None;
                }
            }

            self.planning = false;
            if self.still_capture {
                // An offline photograph never presents intermediate frames. Build
                // the final cover directly instead of remeshing hundreds of
                // temporary local layouts intended for interactive movement.
                let keys = local_keys(&self.desired, &self.world)
                    .into_values()
                    .collect();
                self.target = Some((self.desired.clone(), keys));
            }
        }
        // Retain the last complete cover. Only local balanced patches and their
        // dependent neighbor stitches wait on jobs, never the entire desired cover.
        if self.target.is_none() && !self.planning {
            let next = streaming_patch(
                &self.active,
                &self.desired,
                self.camera.eye(),
                self.effective_budget + 64,
                if self.velocity[0].hypot(self.velocity[2]) > 0.1 {
                    2
                } else {
                    12
                },
            );
            if next != self.active {
                let keys: BTreeSet<_> = local_keys(&next, &self.world).into_values().collect();
                self.cache_hits += keys
                    .difference(&self.cover)
                    .filter(|k| self.gpu.ready(k, self.settings.ray))
                    .count();
                self.target = Some((next, keys));
            }
        }
        let mut protected = self.cover.clone();
        if let Some((_, keys)) = &self.target {
            protected.extend(keys);
        }
        protected.extend(self.gpu.transition_keys());
        let speed = self.velocity[0].hypot(self.velocity[2]);
        self.gpu.configure_streaming_cache(
            self.camera.eye(),
            self.native_policy_radius * 1.5 + speed * 0.5 + 32. * self.gpu.grid.cell_m(),
        );
        self.gpu.trim(&protected);
        self.warmed.retain(|k| self.gpu.tiles.contains_key(k));
        let mut wanted = protected.clone();
        wanted.extend(&self.prefetch);
        self.workers.retain(wanted);
        let upload_start = Instant::now();
        for _ in 0..4 {
            let Ok(result) = self.workers.results.try_recv() else {
                break;
            };
            if !self.workers.acknowledge(&result) {
                continue;
            }
            if result.ray != self.settings.ray {
                continue;
            }
            let speculative = !protected.contains(&result.key);
            if speculative && !self.prefetch.contains(&result.key) {
                continue;
            }
            if speculative {
                let Ok(payload) = &result.payload else {
                    continue;
                };
                let bytes = match payload {
                    Payload::Mesh(mesh, prepared, _) => {
                        mesh.bytes() + prepared.iter().map(|p| p.bytes()).sum::<usize>()
                    }
                    Payload::Ray(brick) => brick.bytes(),
                };
                let warm_bytes: usize = self
                    .warmed
                    .iter()
                    .filter(|k| !protected.contains(k))
                    .filter_map(|k| self.gpu.tiles.get(k))
                    .map(|t| t.bytes)
                    .sum();
                if warm_bytes + bytes
                    > crate::streaming_policy::warm_budget(self.settings.memory_mib)
                    || !self.gpu.prefetch_room(bytes, &protected)
                {
                    self.prefetch_pause_until = Instant::now() + Duration::from_millis(500);
                    continue;
                }
            }
            if result
                .payload
                .as_ref()
                .err()
                .is_some_and(|e| e == "mesh cancelled")
            {
                continue;
            }
            self.completed += 1;
            self.mesh_time += result.elapsed;
            match result.payload.and_then(|payload| match payload {
                Payload::Mesh(mesh, prepared, solid_top) => {
                    self.gpu
                        .upload_prepared(result.key, mesh, prepared, &protected)?;
                    self.gpu.tiles.get_mut(&result.key).unwrap().solid_top = solid_top;
                    Ok(())
                }
                Payload::Ray(brick) => self.gpu.upload_ray(result.key, brick, &protected),
            }) {
                Ok(()) => {
                    if speculative {
                        self.prefetch_builds += 1;
                        self.warmed.insert(result.key);
                    }
                }
                Err(message) if message.contains("GPU mesh budget") && speculative => {
                    self.prefetch_pause_until = Instant::now() + Duration::from_millis(500);
                    continue;
                }
                Err(message) if message.contains("GPU mesh budget") => {
                    self.prefetch.clear();
                    self.prefetch_pause_until = Instant::now() + Duration::from_millis(500);
                    if !self.gpu.recover_cache_pressure(&protected) {
                        self.relax_detail();
                    }
                    break;
                }
                Err(message) => {
                    self.last_error = Some(message);
                    return;
                }
            }
            if upload_start.elapsed() > Duration::from_millis(3) {
                break;
            }
        }
        if let Some((_, keys)) = &self.target {
            if !self.gpu.transitioning()
                && keys.iter().all(|k| self.gpu.ready(k, self.settings.ray))
            {
                let (layout, keys) = self.target.take().unwrap();
                self.prefetch_hits += keys
                    .difference(&self.cover)
                    .filter(|k| self.warmed.remove(k))
                    .count();
                self.gpu.begin_transition(
                    &self.cover,
                    &keys,
                    self.settings.transitions && self.initial_ready,
                    crate::streaming_policy::transition_seconds(
                        32. * self.gpu.grid.cell_m(),
                        self.velocity[0].hypot(self.velocity[2]),
                    ),
                );
                self.active = layout;
                self.cover = keys;
                if self.active == self.desired {
                    self.still_capture = false;
                }
                let mut retained = self.cover.clone();
                retained.extend(self.gpu.transition_keys());
                self.gpu.trim(&retained);
            }
        }
        if !self.initial_ready
            && (self.settled()
                || (self
                    .active
                    .at(self.camera.cell.x, self.camera.cell.z)
                    .is_some_and(|k| k.level + 1 >= self.gpu.grid.finest)
                    && !self.cover.is_empty()
                    && self
                        .cover
                        .iter()
                        .all(|k| self.gpu.ready(k, self.settings.ray))))
        {
            // Nearby continuity is sufficient; distant polishing must not keep
            // ready near patches popping until the entire world has settled.
            self.initial_ready = true;
        }
        let ray = self.settings.ray;
        let mut requests: BTreeSet<_> = self
            .cover
            .iter()
            .filter(|k| !self.gpu.ready(k, ray))
            .copied()
            .collect();
        if let Some((_, keys)) = &self.target {
            requests.extend(keys.iter().filter(|k| !self.gpu.ready(k, ray)));
        }
        let eye = self.camera.eye();
        let mut requests: Vec<_> = requests.into_iter().collect();
        requests.sort_by(|a, b| {
            let priority = |k: &MeshKey| {
                let [x, z] = k.tile.origin_in(self.gpu.grid);
                let e = k.tile.edge_in(self.gpu.grid);
                let dx = (x as f64 * self.gpu.grid.cell_m() - eye[0])
                    .max(0.)
                    .max(eye[0] - (x + e) as f64 * self.gpu.grid.cell_m());
                let dz = (z as f64 * self.gpu.grid.cell_m() - eye[2])
                    .max(0.)
                    .max(eye[2] - (z + e) as f64 * self.gpu.grid.cell_m());
                let d = dx.hypot(dz);
                let cx = (x + e / 2) as f64 * self.gpu.grid.cell_m() - eye[0];
                let cz = (z + e / 2) as f64 * self.gpu.grid.cell_m() - eye[2];
                let ahead = cx * direction[0] + cz * direction[2];
                d * (if ahead < 0. && d > self.settings.native_radius * 2. {
                    3.
                } else {
                    1.
                })
            };
            priority(a).total_cmp(&priority(b))
        });
        let radius = self.settings.native_radius * 2.;
        let warm_bytes: usize = self
            .warmed
            .iter()
            .filter(|k| !protected.contains(k))
            .filter_map(|k| self.gpu.tiles.get(k))
            .map(|t| t.bytes)
            .sum();
        let allow_prefetch = near_detail_ready
            && Instant::now() >= self.prefetch_pause_until
            && warm_bytes.saturating_add(65536)
                < crate::streaming_policy::warm_budget(self.settings.memory_mib)
            && self.gpu.prefetch_room(65536, &protected);
        let warm_requests: Vec<_> = if allow_prefetch {
            self.prefetch
                .iter()
                .filter(|k| !self.gpu.ready(k, ray))
                .copied()
                .collect()
        } else {
            Vec::new()
        };
        self.workers.schedule_with_prefetch(
            requests.into_iter().map(|key| {
                let [x, z] = key.tile.origin_in(self.gpu.grid);
                let edge = key.tile.edge_in(self.gpu.grid);
                let dx = (x as f64 * self.gpu.grid.cell_m() - eye[0])
                    .max(0.)
                    .max(eye[0] - (x + edge) as f64 * self.gpu.grid.cell_m());
                let dz = (z as f64 * self.gpu.grid.cell_m() - eye[2])
                    .max(0.)
                    .max(eye[2] - (z + edge) as f64 * self.gpu.grid.cell_m());
                (key, dx.hypot(dz) < radius)
            }),
            ray,
            warm_requests,
        );
    }

    fn relax_detail(&mut self) {
        // Preserve playable coverage on exhaustion; quality can be
        // raised again through settings after increasing residency.
        self.prefetch.clear();
        self.memory_pressure = true;
        self.plan_generation += 1;
        self.planning = false;
        self.budget_limited = true;
        let lower = (self.effective_budget * 3 / 4).max(64);
        self.target = None;
        if lower == self.effective_budget {
            self.desired = self.active.clone();
            self.last_cell = self.camera.cell;
            self.last_fov = self.camera.fov;
        } else {
            self.effective_budget = lower;
            self.last_fov = -1.;
        }
        self.workers.retain(self.cover.clone());
    }

    pub fn prepare_still_capture(&mut self) {
        self.prefetch.clear();
        self.still_capture = true;
        self.target = None;
        self.workers.retain(self.cover.clone());
    }
    pub fn apply_settings(&mut self, settings: crate::settings::Settings) {
        let renderer_changed = self.settings.ray != settings.ray;
        let terrain_changed = renderer_changed
            || self.settings.quality != settings.quality
            || self.settings.native_radius != settings.native_radius
            || self.settings.memory_mib != settings.memory_mib;
        self.settings = settings;
        if terrain_changed {
            self.prefetch.clear();
            self.warmed.clear();
        }
        if renderer_changed {
            self.initial_ready = false;
        }
        self.gpu.detail_settings(&self.settings);
        self.effective_budget = self.tile_budget.min(self.settings.tile_budget());
        if terrain_changed {
            self.memory_pressure = false;
            self.plan_generation += 1;
            self.planning = false;
            self.target = None;
            self.desired = Layout::root_in(self.gpu.grid); // mark intent dirty before any wait-for-ready caller
            self.workers.retain(self.cover.clone());
            self.last_fov = -1.;
            self.last_plan = Instant::now() - Duration::from_secs(1);
        }
    }
    pub fn pending_jobs(&self) -> usize {
        self.workers.outstanding() + usize::from(self.planning)
    }
    pub fn set_environment_map(&mut self, layer: Option<DebugLayer>) -> Result<(), String> {
        if let Some(layer) = layer {
            let image = debug::image_from_world(&self.world, layer, 256)?;
            self.gpu.set_environment_map(&image);
            println!("Map {layer}: {}", layer.legend());
        }
        self.debug_layer = layer;
        self.clear_controls();
        Ok(())
    }
    pub fn cycle_environment_map(&mut self) -> Result<(), String> {
        let next = self
            .debug_layer
            .map_or(Some(DebugLayer::ALL[0]), |current| {
                DebugLayer::ALL
                    .iter()
                    .position(|&v| v == current)
                    .and_then(|i| DebugLayer::ALL.get(i + 1).copied())
            });
        self.set_environment_map(next)
    }
    pub fn draw(&mut self, view: &wgpu::TextureView) {
        if self.debug_layer.is_some() {
            self.gpu.render_environment_map(view);
            self.visible = 0;
            return;
        }
        let selection = match raycast(&self.world, self.camera.eye(), self.camera.direction(), 12.)
        {
            Trace::Hit(hit)
                if self
                    .active
                    .at(hit.cell.x, hit.cell.z)
                    .is_some_and(|tile| tile.level == self.gpu.grid.finest) =>
            {
                Some([hit.cell.x, hit.cell.y, hit.cell.z])
            }
            _ => None,
        };
        let anchor = self
            .player
            .root
            .offset([0, (1.5 / self.gpu.grid.cell_m()).round() as i64, 0])
            .unwrap_or(self.player.root);
        let boom_distance = (self.camera.cell.x - anchor.x).pow(2)
            + (self.camera.cell.y - anchor.y).pow(2)
            + (self.camera.cell.z - anchor.z).pow(2);
        let arm = !self.flying && (self.view == ViewMode::First || boom_distance < 100);
        if let Err(error) = self.gpu.set_actor(
            self.player.pose,
            arm,
            self.player.root,
            self.player.frame(),
            &self.player.materials,
        ) {
            if error.contains("GPU mesh budget") {
                let mut required = self.cover.clone();
                required.extend(self.gpu.transition_keys());
                if let Some((_, keys)) = &self.target {
                    required.extend(keys);
                }
                self.prefetch.clear();
                if !self.gpu.recover_cache_pressure(&required) {
                    self.relax_detail();
                }
                self.gpu.move_cached_actor(self.player.root);
            } else {
                self.last_error = Some(error);
                self.gpu.hide_actor();
            }
        }
        self.visible = self.gpu.render(view, &self.camera, &self.cover, selection);
    }
    pub fn stats(&self, fps: f64) -> String {
        if let Some(layer) = self.debug_layer {
            return format!(
                "MicroVoxel | {fps:.0} FPS | Map {layer} | {} | M next",
                layer.legend()
            );
        }
        let (ready, total) = self.loading();
        let level = self.cover.iter().map(|k| k.tile.level).max().unwrap_or(0);
        let spacing =
            100. * (1i64 << (self.gpu.grid.finest - level)) as f64 * self.gpu.grid.cell_m();
        let mode = if self.flying {
            "Debug flight"
        } else {
            self.view.name()
        };
        let movement = if self.flying { self.camera.speed } else { 3. };
        let world_name = if self.world.baseline().survey().is_some() {
            "Volcan Mountain"
        } else {
            "Procedural"
        };
        format!("MicroVoxel — {world_name} | {} | {mode} | {fps:.0} FPS | Speed {:.1} m/s | FOV {:.0}° | {spacing:.2} cm | {} / {} tiles | {:.1} MiB | Loaded {ready}/{total}{}",if self.settings.ray {"Ray"}else{"Mesh"},movement,self.camera.fov.to_degrees(),self.visible,self.cover.len(),self.gpu.bytes as f64/(1024.*1024.),if self.last_error.is_some(){" | ERROR: see terminal"}else{""})
    }
    /// Actual resident support at player-eye X/Z; policy radius alone is not proof.
    pub fn report_capture_detail(&self) {
        let grid = self.gpu.grid;
        let mut native = 0;
        for i in 0..10 {
            let (dx, dz) = if i == 0 {
                (0., 0.)
            } else if i == 9 {
                (self.camera.yaw.cos() * 5., self.camera.yaw.sin() * 5.)
            } else {
                let a = (i - 1) as f64 * std::f64::consts::TAU / 8.;
                (a.cos() * 6., a.sin() * 6.)
            };
            let x = self.camera.cell.x + (dx / grid.cell_m()).round() as i64;
            let z = self.camera.cell.z + (dz / grid.cell_m()).round() as i64;
            let tile = self.active.at(x, z);
            let step = tile.map(|t| t.step_in(grid));
            let loaded = tile.is_some_and(|t| {
                self.cover
                    .iter()
                    .any(|k| k.tile == t && self.gpu.ready(k, self.settings.ray))
            });
            if step == Some(1) && loaded {
                native += 1;
            }
            println!("Capture detail sample {i}: offset ({dx:.2},{dz:.2}) m; display step {step:?} voxels; resident {loaded}");
        }
        println!("Capture native support: {native}/10 samples (center, 8 around 6 m, forward 5 m); voxel {:.6} m; requested radius {:.1} m; planning radius {:.2} m",
            grid.cell_m(), self.settings.native_radius, self.native_policy_radius);
        if native < 10 {
            println!("Capture detail limitation: nearby samples include coarse or missing support; settled cover does not establish native coverage.");
        }
    }

    pub fn report(&self) {
        println!(
            "Resident detail reused before generation: {} tiles",
            self.cache_hits
        );
        println!("Predictive detail: {} speculative meshes built; {} reused at publication; {} ready tracked",self.prefetch_builds,self.prefetch_hits,self.warmed.len());
        println!("Streaming state: eye {:?} m; active {} / desired {} tiles; target {:?}; jobs {}; effective tile budget {}; pressure {}; error {:?}",
            self.camera.eye(),self.active.tiles.len(),self.desired.tiles.len(),
            self.target.as_ref().map(|(_,keys)|keys.len()),self.pending_jobs(),self.effective_budget,self.memory_pressure,self.last_error);
        let pools = self.gpu.terrain_arena_stats();
        println!(
            "Terrain arenas: {:.2} MiB live, {:.2} MiB pending GPU completion, {:.2} MiB reserved",
            pools.iter().map(|p| p.0).sum::<u64>() as f64 / 1048576.,
            pools.iter().map(|p| p.1).sum::<u64>() as f64 / 1048576.,
            pools.iter().map(|p| p.2).sum::<u64>() as f64 / 1048576.
        );
        println!(
            "Managed terrain commitment including ray buffers and in-flight migrations: {:.2} MiB",
            self.gpu.managed_committed_bytes() as f64 / 1048576.
        );
        if let Some(sample) = self.gpu.timing.latest() {
            println!("GPU terrain pass timestamp: {:.3} ms at frame {} (excludes presentation/readback); timing buffers {} B, dropped {}, mapping failures {}",sample.pass_ms,sample.frame,self.gpu.timing.allocated_buffer_bytes(),self.gpu.timing.dropped_samples(),self.gpu.timing.map_failures());
        }

        if self.settings.ray {
            println!("Ray terrain: {} tiles; {:.2} MiB resident voxel data; {:.2} MiB committed arena; {} build jobs, summed worker time {:.2}s; {} culled tiles",self.cover.len(),self.gpu.bytes as f64/1048576.,self.gpu.ray_allocated_bytes() as f64/1048576.,self.completed,self.mesh_time.as_secs_f64(),self.gpu.culled);
            return;
        }

        let faces: usize = self
            .cover
            .iter()
            .filter_map(|k| self.gpu.tiles.get(k))
            .map(|t| t.faces)
            .sum();
        let quads: usize = self
            .cover
            .iter()
            .filter_map(|k| self.gpu.tiles.get(k))
            .map(|t| t.quads)
            .sum();
        println!("Active surface: {faces} cell faces -> {quads} merged rectangles ({:.1}x fewer); GPU cache {:.2} MiB; {} mesh jobs, summed worker time {:.2}s",faces as f64/quads.max(1) as f64,self.gpu.bytes as f64/(1024.*1024.),self.completed,self.mesh_time.as_secs_f64());
    }
}

/// Bound an atomic update by actual rebuilt neighbor identities, not only splits.
/// One indivisible balanced closure may exceed the cap; never split that closure.
fn streaming_patch(
    active: &Layout,
    desired: &Layout,
    eye: [f64; 3],
    budget: usize,
    steps: usize,
) -> Layout {
    if active == desired {
        return active.clone();
    }
    let original: BTreeSet<_> = if steps > 2 {
        active.mesh_keys(0).into_values().collect()
    } else {
        BTreeSet::new()
    };
    let mut next = active.clone();
    for _ in 0..steps {
        let Some(candidate) = lod::next_patch_budget(&next, desired, eye, budget) else {
            break;
        };
        let changed = if steps > 2 {
            candidate
                .mesh_keys(0)
                .into_values()
                .filter(|k| !original.contains(k))
                .take(33)
                .count()
        } else {
            0
        };
        if next != *active && changed > 32 {
            break;
        }
        next = candidate;
    }
    next
}

fn local_keys(
    layout: &Layout,
    world: &World<Terrain>,
) -> std::collections::BTreeMap<lod::TileKey, MeshKey> {
    let mut keys = layout.mesh_keys(0);
    for key in keys.values_mut() {
        let [x, z] = key.tile.origin_in(layout.grid);
        let halo = key.tile.step_in(layout.grid) * 2;
        key.revision =
            world.region_revision(x - halo, z - halo, key.tile.edge_in(layout.grid) + halo * 2);
    }
    keys
}

#[cfg(test)]
#[path = "runtime/travel_cache_test.rs"]
mod travel_cache_test;

#[cfg(test)]
#[path = "runtime/stop_resume_test.rs"]
mod stop_resume_test;

#[cfg(test)]
mod tests {
    use super::*;
    use microvoxel::{
        materials::{Appearance, MaterialId, Voxel},
        CellPos, Edit, EditAction,
    };

    #[test]
    fn idle_batches_bound_real_rebuilds_and_keep_balanced_cover() {
        let mut active = Layout::root();
        let desired = lod::select([5000., 100., 5000.], 720, 1.1, &active, 2048);
        let mut bounded = 0;
        for _ in 0..30 {
            let first = lod::next_patch_budget(&active, &desired, [5000., 100., 5000.], 2112);
            let next = streaming_patch(&active, &desired, [5000., 100., 5000.], 2112, 12);
            if next == active {
                break;
            }
            let original: BTreeSet<_> = active.mesh_keys(0).into_values().collect();
            let jobs = next
                .mesh_keys(0)
                .into_values()
                .filter(|k| !original.contains(k))
                .count();
            assert!(
                jobs <= 32 || first.as_ref() == Some(&next),
                "oversized multi-step idle batch: {jobs}"
            );
            if jobs <= 32 {
                bounded += 1;
            }
            for &tile in &next.tiles {
                assert!(next
                    .neighbors(tile)
                    .iter()
                    .all(|level| level.abs_diff(tile.level) <= 1));
            }
            assert!(next.at(80000, 80000).is_some());
            active = next;
        }
        assert!(bounded > 5);
    }

    #[test]
    fn local_edit_refreshes_neighbor_halo_and_preserves_distant_keys() {
        let baseline = World::new(Terrain::new(5));
        let grid = lod::Grid::new(baseline.spec());
        let tiles = [0, 1, 5].map(|x| lod::TileKey {
            level: grid.finest,
            x,
            z: 0,
        });
        let layout = Layout {
            grid,
            tiles: tiles.into(),
        };
        let old = local_keys(&layout, &baseline);
        let mut edited = baseline.fork_revision();
        let pos = CellPos::new(32, 10, 10);
        let value = if baseline.voxel_at(pos).known() == Some(Voxel::AIR) {
            Voxel::new(MaterialId::BASALT, Appearance(0x39e7))
        } else {
            Voxel::AIR
        };
        edited
            .apply_actions(&[EditAction::Set(Edit::set(pos, value))])
            .unwrap();
        let new = local_keys(&layout, &edited);
        // The edit lies in tile 1. Tile 0's two-cell sampling halo crosses
        // that boundary and must refresh even though its core is untouched.
        for tile in &tiles[..2] {
            assert_ne!(old[tile], new[tile]);
            assert_eq!(new[tile].revision, edited.revision());
        }
        assert_eq!(old[&tiles[2]], new[&tiles[2]]);
        assert_eq!(baseline.revision(), 0);
        assert_ne!(baseline.voxel_at(pos).known(), Some(value));
    }

    #[test]
    fn appearance_only_edit_refreshes_local_surface_revision() {
        let mut world = World::new(Terrain::new(5));
        let grid = lod::Grid::new(world.spec());
        let tile = lod::TileKey {
            level: grid.finest,
            x: 1,
            z: 1,
        };
        let layout = Layout {
            grid,
            tiles: [tile].into(),
        };
        let pos = CellPos::new(40, 10, 40);
        let first = Voxel::new(MaterialId::BASALT, Appearance(0x39e7));
        world
            .apply_actions(&[EditAction::Set(Edit::set(pos, first))])
            .unwrap();
        let before = local_keys(&layout, &world);
        let mut next = world.fork_revision();
        let second = Voxel::new(first.material, Appearance(0x1234));
        next.apply_actions(&[EditAction::Set(Edit {
            pos,
            value: second,
            expected: Some(first),
        })])
        .unwrap();
        assert_ne!(before[&tile], local_keys(&layout, &next)[&tile]);
        assert_eq!(world.voxel_at(pos).known(), Some(first));
        assert_eq!(next.voxel_at(pos).known(), Some(second));
    }
}
