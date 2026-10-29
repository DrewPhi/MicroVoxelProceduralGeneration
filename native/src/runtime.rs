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
    generation::ProceduralTerrain,
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
    pub gpu: Gpu,
    pub camera: Camera,
    pub player: Player,
    pub flying: bool,
    pub view: ViewMode,
    view_key: Option<(microvoxel::CellPos, u64, u64, u64, u32, u32, ViewMode)>,
    pub world: Arc<World<ProceduralTerrain>>,
    workers: Workers,
    pub active: Layout,
    pub cover: BTreeSet<MeshKey>,
    target: Option<(Layout, BTreeSet<MeshKey>)>,
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
        mut gpu: Gpu,
        world: Arc<World<ProceduralTerrain>>,
        tile_budget: usize,
        spawn_nonce: u64,
    ) -> microvoxel::Result<Self> {
        gpu.terrain_lighting(&world);
        let (camera, player) = Self::spawn(&world, spawn_nonce)?;
        let height = gpu.height;
        let cell = camera.cell;
        let fov = camera.fov;
        let workers = Workers::new(Arc::clone(&world));
        println!(
            "{} mesh workers; 128 MiB GPU mesh budget; tile budget {tile_budget}",
            workers.count
        );
        let active = Layout::root();
        let keys = active.mesh_keys(world.revision()).into_values().collect();
        let mut runtime = Self {
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
            last_motion_eye: cell.meters(),
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
    fn spawn(world: &World<ProceduralTerrain>, nonce: u64) -> microvoxel::Result<(Camera, Player)> {
        let mut player = Player::new(world.identity().seed, microvoxel::CellPos::default())?;
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
        if dt > 0. {
            let eye = self.camera.eye();
            for axis in [0, 2] {
                let v =
                    ((eye[axis] - self.last_motion_eye[axis]) / dt.max(0.001)).clamp(-100., 100.);
                self.velocity[axis] = self.velocity[axis] * 0.8 + v * 0.2;
            }
            self.last_motion_eye = eye;
        }
        // Camera changes update intent even while a small ready patch is loading.
        let direction = self.camera.direction();
        if !self.still_capture && dt > 0. {
            let eye = self.camera.eye();
            let nearby_ready = (0..8).all(|i| {
                let a = i as f64 * std::f64::consts::TAU / 8.;
                let x = (eye[0] * 16. + a.cos() * self.settings.native_radius * 12.) as i64;
                let z = (eye[2] * 16. + a.sin() * self.settings.native_radius * 12.) as i64;
                self.active
                    .at(x, z)
                    .is_none_or(|key| key.level == lod::FINEST)
            });
            let goal = if nearby_ready { 24000. } else { 1024. };
            let speed = if nearby_ready { 0.3 } else { 2. };
            self.gpu.visibility_limit +=
                (goal - self.gpu.visibility_limit) * (dt as f32 * speed).min(1.);
        }

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
        if moved && self.last_plan.elapsed() > Duration::from_millis(200) {
            self.plan_generation += 1;
            self.planner.request(crate::lod_planner::Request {
                generation: self.plan_generation,
                eye: std::array::from_fn(|i| self.camera.eye()[i] + self.velocity[i] * 0.5),
                height: self.gpu.height,
                fov: self.camera.fov,
                previous: self.desired.clone(),
                budget: self.effective_budget,
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
            if result.generation != self.plan_generation {
                continue;
            }
            self.achieved_pixels = result.selection.achieved_pixels;
            self.budget_limited = result.selection.budget_limited || self.memory_pressure;
            self.desired = result.selection.layout;
            if !self.still_capture {
                let obsolete = self.target.as_ref().is_some_and(|(target, _)| {
                    target.tiles.difference(&self.active.tiles).any(|key| {
                        let [x, z] = key.origin();
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
                let keys = self
                    .desired
                    .mesh_keys(self.world.revision())
                    .into_values()
                    .collect();
                self.target = Some((self.desired.clone(), keys));
            }
        }
        // Retain the last complete cover. Only local balanced patches and their
        // dependent neighbor stitches wait on jobs, never the entire desired cover.
        if self.target.is_none() && !self.planning {
            let mut next = self.active.clone();
            for _ in 0..12 {
                if let Some(patch) = lod::next_patch_budget(
                    &next,
                    &self.desired,
                    self.camera.eye(),
                    self.effective_budget + 64,
                ) {
                    next = patch;
                } else {
                    break;
                }
            }
            if next != self.active {
                let keys = next
                    .mesh_keys(self.world.revision())
                    .into_values()
                    .collect();
                self.target = Some((next, keys));
            }
        }
        let mut protected = self.cover.clone();
        if let Some((_, keys)) = &self.target {
            protected.extend(keys);
        }
        protected.extend(self.gpu.transition_keys());
        self.gpu.trim(&protected);
        self.workers.retain(protected.clone());
        let upload_start = Instant::now();
        for _ in 0..4 {
            let Ok(result) = self.workers.results.try_recv() else {
                break;
            };
            self.workers.acknowledge(result.key, result.ray);
            if result.ray != self.settings.ray {
                continue;
            }
            if !protected.contains(&result.key) {
                continue;
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
                Ok(()) => {}
                Err(message) if message.contains("GPU mesh budget") => {
                    // Preserve playable coverage on exhaustion; quality can be
                    // raised again through settings after increasing residency.
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
            if keys.iter().all(|k| self.gpu.ready(k, self.settings.ray)) {
                let (layout, keys) = self.target.take().unwrap();
                self.gpu.begin_transition(
                    &self.cover,
                    &keys,
                    self.settings.transitions && self.initial_ready,
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
        if self.settled() {
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
                let [x, z] = k.tile.origin();
                let e = k.tile.edge();
                let dx = (x as f64 / 16. - eye[0])
                    .max(0.)
                    .max(eye[0] - (x + e) as f64 / 16.);
                let dz = (z as f64 / 16. - eye[2])
                    .max(0.)
                    .max(eye[2] - (z + e) as f64 / 16.);
                let d = dx.hypot(dz);
                let cx = (x + e / 2) as f64 / 16. - eye[0];
                let cz = (z + e / 2) as f64 / 16. - eye[2];
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
        self.workers.schedule(
            requests.into_iter().map(|key| {
                let [x, z] = key.tile.origin();
                let edge = key.tile.edge();
                let dx = (x as f64 / 16. - eye[0])
                    .max(0.)
                    .max(eye[0] - (x + edge) as f64 / 16.);
                let dz = (z as f64 / 16. - eye[2])
                    .max(0.)
                    .max(eye[2] - (z + edge) as f64 / 16.);
                (key, dx.hypot(dz) < radius)
            }),
            ray,
        );
    }

    pub fn prepare_still_capture(&mut self) {
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
            self.desired = Layout::root(); // mark intent dirty before any wait-for-ready caller
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
                    .is_some_and(|tile| tile.level == lod::FINEST) =>
            {
                Some([hit.cell.x, hit.cell.y, hit.cell.z])
            }
            _ => None,
        };
        let anchor = self
            .player
            .root
            .offset([0, 24, 0])
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
            self.last_error = Some(error);
            self.gpu.hide_actor();
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
        let spacing = 100. * (1i64 << (lod::FINEST - level)) as f64 / 16.;
        let mode = if self.flying {
            "Debug flight"
        } else {
            self.view.name()
        };
        let movement = if self.flying { self.camera.speed } else { 3. };
        format!("MicroVoxel | {} | {mode} | {fps:.0} FPS | Speed {:.1} m/s | FOV {:.0}° | {spacing:.2} cm | {} / {} tiles | {:.1} MiB | Loaded {ready}/{total}{}",if self.settings.ray {"Ray"}else{"Mesh"},movement,self.camera.fov.to_degrees(),self.visible,self.cover.len(),self.gpu.bytes as f64/(1024.*1024.),if self.last_error.is_some(){" | ERROR: see terminal"}else{""})
    }
    pub fn report(&self) {
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
