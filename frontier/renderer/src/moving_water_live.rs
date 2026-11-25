//! One owned moving-water controller on a worker. Publication and terrain edits
//! stay paired with the current solid source; camera demands never discard mass.
use crate::{Camera, Result};
use frontier_world::{
    discrete_water::{environmental_flow::FlowConfig, StepStats},
    edit::{EditTransaction, Inventory, MATERIALS},
    moving_water::MovingWater,
    procedural_water::TileKey,
    Cell, Material, World,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender},
    time::Instant,
};
const HISTORY_TILES: usize = 4096;
struct View {
    bounds: (Cell, Cell),
    states: Vec<u32>,
    changed: Vec<(Cell, bool)>,
}
fn restore_history(state: &MovingWater, world: &mut World) -> Result<()> {
    state.for_each_block(|block| {
        let changes = {
            let mut changes = Vec::new();
            let mut columns = BTreeMap::new();
            for (i, native) in block.cells.iter().enumerate() {
                let c = Cell {
                    x: block.origin.x + (i % 16) as i64,
                    y: block.origin.y + ((i / 16) % 16) as i64,
                    z: block.origin.z + (i / 256) as i64,
                };
                let column = columns
                    .entry((c.x, c.z))
                    .or_insert_with(|| world.column(c.x, c.z));
                if (column.material(c.y) == Material::Water) != native.water {
                    changes.push((c, native.water));
                }
            }
            changes
        };
        world.set_fluid_occupancy(changes);
    })?;
    Ok(())
}
fn prepare_view(state: &MovingWater, world: &World) -> Result<View> {
    let bounds = state.overlay_bounds();
    let mut states = vec![0; 64usize.pow(3)];
    let mut changed = Vec::new();
    let mut columns = BTreeMap::new();
    for (c, wet) in state.occupancy_in_bounds(Some(bounds))? {
        let i = (c.x - bounds.0.x) as usize
            + 64 * ((c.y - bounds.0.y) as usize + 64 * (c.z - bounds.0.z) as usize);
        states[i] = if wet { 2 } else { 1 };
        let column = columns
            .entry((c.x, c.z))
            .or_insert_with(|| world.column(c.x, c.z));
        if (column.material(c.y) == Material::Water) != wet {
            changed.push((c, wet));
        }
    }
    Ok(View {
        bounds,
        states,
        changed,
    })
}
struct Job {
    state: MovingWater,
    world: World,
    demand: TileKey,
    epoch: u64,
    dt: f64,
    flow: FlowConfig,
}
struct Completed {
    state: MovingWater,
    epoch: u64,
    stats: StepStats,
    view: Option<View>,
    error: Option<String>,
    requested: f64,
    demand: TileKey,
    ms: f64,
}
pub struct MovingWaterLive {
    pub inventory: Inventory,
    pub center: Option<Cell>,
    state: Option<MovingWater>,
    tx: SyncSender<Job>,
    rx: Receiver<Completed>,
    path: Option<PathBuf>,
    static_world: Option<World>,
    dynamic: Option<(Cell, [u32; 3], u64, Vec<u32>)>,
    bounds: Option<(Cell, Cell)>,
    generation: u64,
    epoch: u64,
    clock: Instant,
    debt: f64,
    simtime: f64,
    wall_elapsed: f64,
    started: bool,
    error: Option<String>,
    failed_demand: Option<TileKey>,
    first_success: bool,
    metrics: String,
    camera_pose: Option<([f64; 3], f32, f32)>,
    river_rate: u64,
    requested_rate: Option<u64>,
}
impl MovingWaterLive {
    pub fn new(world: &mut World, path: Option<PathBuf>, river_rate: Option<u64>) -> Result<Self> {
        let (tx, jobs) = mpsc::sync_channel::<Job>(1);
        let (results, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("moving-native-water".into())
            .spawn(move || {
                while let Ok(mut job) = jobs.recv() {
                    let start = Instant::now();
                    let mut error = None;
                    let mut stats = StepStats::default();
                    if let Err(e) = job.state.migrate(&job.world, job.demand) {
                        error = Some(format!("water activation paused: {e}"));
                    } else {
                        match job
                            .state
                            .step_with_environment(&job.world, job.dt, job.flow)
                        {
                            Ok(s) => stats = s,
                            Err(e) => error = Some(format!("water step paused: {e}")),
                        }
                    }
                    let view = match prepare_view(&job.state, &job.world) {
                        Ok(v) => Some(v),
                        Err(e) => {
                            error = Some(format!("water view paused: {e}"));
                            None
                        }
                    };
                    if results
                        .send(Completed {
                            state: job.state,
                            epoch: job.epoch,
                            stats,
                            view,
                            error,
                            requested: job.dt,
                            demand: job.demand,
                            ms: start.elapsed().as_secs_f64() * 1000.,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        let mut live = Self {
            inventory: Inventory::default(),
            center: None,
            state: None,
            tx,
            rx,
            path,
            static_world: None,
            dynamic: None,
            bounds: None,
            generation: 0,
            epoch: 0,
            clock: Instant::now(),
            debt: 0.,
            simtime: 0.,
            wall_elapsed: 0.,
            started: false,
            error: None,
            failed_demand: None,
            first_success: false,
            metrics: "{}".into(),
            camera_pose: None,
            river_rate: river_rate.unwrap_or(128),
            requested_rate: river_rate,
        };
        if let Some(path) = &live.path {
            if path.exists() {
                let bytes = std::fs::read(path)?;
                live.restore(world, &bytes)?;
            }
        }
        Ok(live)
    }
    pub fn start(&mut self, world: &mut World, camera: &mut Camera) -> Result<()> {
        let (_, water, normal) = crate::procedural_water_live::choose_bank(world, camera)?;
        self.camera_pose = Some((
            [
                water[0] + normal[0] * 0.7,
                water[1] + 1.7,
                water[2] + normal[1] * 0.7,
            ],
            (-normal[0]).atan2(-normal[1]) as f32,
            -1.0,
        ));
        self.position_camera(camera);
        let feet = Cell::from_meters([
            camera.position.x,
            camera.position.y - 1.7,
            camera.position.z,
        ]);
        let demand = MovingWater::window_for(feet)?;
        let state = MovingWater::new(world, demand, HISTORY_TILES)?;
        let view = prepare_view(&state, world)?;
        self.center = Some(demand.origin()?);
        self.state = Some(state);
        self.publish(world, view)?;
        eprintln!("moving native water started at {:?}: connected2m detailed window, exact1m dormant halo, {} tile persistent cap; finite regional ownership, no invented supply",demand,HISTORY_TILES);
        Ok(())
    }
    pub fn observe_camera(&mut self, camera: Camera) {
        self.camera_pose = Some((camera.position.to_array(), camera.yaw, camera.pitch));
    }
    pub fn position_camera(&self, camera: &mut Camera) {
        if let Some((eye, yaw, pitch)) = self.camera_pose {
            camera.position = glam::DVec3::from_array(eye);
            camera.yaw = yaw;
            camera.pitch = pitch;
        }
    }
    fn publish(&mut self, world: &mut World, view: View) -> Result<()> {
        let start = Instant::now();
        world.set_fluid_occupancy(view.changed);
        let moved = self.bounds != Some(view.bounds);
        if self
            .dynamic
            .as_ref()
            .is_none_or(|(o, _, _, s)| *o != view.bounds.0 || s != &view.states)
        {
            self.generation += 1;
            self.dynamic = Some((view.bounds.0, [64; 3], self.generation, view.states));
        }
        if moved || self.static_world.is_none() {
            self.static_world = Some(world.without_fluid_in_bounds(view.bounds.0, view.bounds.1));
            self.bounds = Some(view.bounds);
        }
        let state = self.state.as_ref().ok_or("missing moving water state")?;
        let r = state.regional();
        let l = r.ledger();
        let sim = state.simulation();
        self.center = Some(state.origin().origin()?);
        self.metrics=format!("{{\"model\":\"moving\",\"camera_following\":true,\"detailed_cells\":32768,\"tick\":{},\"simtime\":{},\"wall_elapsed\":{},\"wall_lag\":{},\"deferred_seconds\":{},\"detailed_units\":{},\"stored_units\":{},\"initial\":{},\"supplied\":{},\"drained\":{},\"conserved\":{},\"migrations\":{},\"history_tiles\":{},\"history_cap\":{},\"native_payload_bytes\":{},\"pending_relaxations\":{},\"relax_queries\":{},\"relax_moves\":{},\"publish_ms\":{},\"edit_epoch\":{}}}",sim.tick(),self.simtime,self.wall_elapsed,(self.wall_elapsed-self.simtime).max(0.),self.debt,sim.mass(),r.stored_units(),l.initial,l.supplied,l.drained,r.conserved(),state.migrations(),state.tile_count(),HISTORY_TILES,state.native_payload_bytes(),state.pending_relaxations(),state.relax_queries(),state.relax_moves(),start.elapsed().as_secs_f64()*1000.,self.epoch);
        let env = state.environmental_counters();
        self.metrics.pop();
        self.metrics.push_str(&format!(",\"environmental_features\":{},\"upstream_attempted\":{},\"upstream_added\":{},\"upstream_suppressed\":{},\"downstream_attempted\":{},\"downstream_removed\":{},\"downstream_unsatisfied\":{}}}",state.environmental_features(),env.upstream_attempted,env.upstream_added,env.upstream_suppressed,env.downstream_attempted,env.downstream_removed,env.downstream_unsatisfied));
        Ok(())
    }
    fn accept(&mut self, world: &mut World, done: Completed) -> Result<()> {
        if done.epoch != self.epoch {
            return Err("moving water edit epoch mismatch".into());
        }
        if !self.first_success && done.error.is_none() && done.stats.simulated_dt > 0. {
            eprintln!("moving water simulation started");
            self.first_success = true;
        }
        self.state = Some(done.state);
        self.simtime += done.stats.simulated_dt;
        self.debt += if done.stats.simulated_dt == 0. && done.error.is_some() {
            done.requested
        } else {
            done.stats.deferred_dt
        };
        self.failed_demand = done.error.as_ref().map(|_| done.demand);
        self.error = done.error;
        if let Some(view) = done.view {
            self.publish(world, view)?;
        }
        self.metrics.pop();
        self.metrics.push_str(&format!(",\"last_step_ms\":{},\"boundary_transfers\":{},\"max_speed_mps\":{},\"pressure_residual_rms\":{}}}",done.ms,done.stats.boundary_transfers.len(),done.stats.max_speed_mps,done.stats.pressure_residual_rms));
        Ok(())
    }
    fn poll(&mut self, world: &mut World) -> Result<()> {
        match self.rx.try_recv() {
            Ok(done) => self.accept(world, done)?,
            Err(mpsc::TryRecvError::Disconnected) if self.state.is_none() => {
                return Err("moving water worker disconnected".into())
            }
            _ => {}
        }
        Ok(())
    }
    pub fn update(&mut self, world: &mut World, camera: Camera) -> Result<()> {
        let before = self.bounds;
        self.poll(world)?;
        if before != self.bounds {
            return Ok(());
        }
        let feet = Cell::from_meters([
            camera.position.x,
            camera.position.y - 1.7,
            camera.position.z,
        ]);
        let demand = MovingWater::window_for(feet)?;
        if self.error.is_some() {
            if self.failed_demand == Some(demand) {
                self.wait_for_render();
                return Ok(());
            }
            self.error = None;
            self.failed_demand = None;
        }
        let elapsed = self.clock.elapsed().as_secs_f64();
        if elapsed >= 0.04 || self.state.as_ref().is_some_and(|s| s.origin() != demand) {
            if let Some(state) = self.state.take() {
                self.started = true;
                self.wall_elapsed += elapsed;
                let available = elapsed + self.debt;
                let dt = available.min(0.25);
                self.debt = available - dt;
                self.tx.send(Job {
                    state,
                    world: world.clone(),
                    demand,
                    epoch: self.epoch,
                    dt,
                    flow: FlowConfig {
                        upstream_units_per_second: self.river_rate,
                        downstream_units_per_second: self.river_rate,
                    },
                })?;
                self.clock = Instant::now();
            }
        }
        Ok(())
    }
    pub fn wait_for_render(&mut self) {
        let elapsed = self.clock.elapsed().as_secs_f64();
        if self.started {
            self.wall_elapsed += elapsed;
            self.debt += elapsed;
        }
        self.clock = Instant::now();
    }
    pub fn static_world(&self) -> Option<&World> {
        self.static_world.as_ref()
    }
    pub fn dynamic_water(&self) -> Option<(Cell, [u32; 3], u64, &[u32])> {
        self.dynamic
            .as_ref()
            .map(|(o, d, g, s)| (*o, *d, *g, s.as_slice()))
    }
    pub fn diagnostics_json(&self) -> String {
        let mut s = self.metrics.clone();
        s.pop();
        s.push_str(&format!(
            ",\"pending_snapshots\":{},\"river_units_per_second\":{},\"paused_error\":{:?}}}",
            usize::from(self.state.is_none()),
            self.river_rate,
            self.error.as_deref().unwrap_or("")
        ));
        s
    }
    pub fn apply_player_transaction(
        &mut self,
        world: &mut World,
        t: &EditTransaction,
        owns: impl Fn(Cell) -> bool,
    ) -> std::result::Result<(), &'static str> {
        self.poll(world)
            .map_err(|_| "Moving water worker unavailable")?;
        let state = self
            .state
            .as_ref()
            .ok_or("Water step pending; retry edit")?;
        if t.place.is_some()
            && t.expected.iter().any(|&(c, _)| {
                world.material(c) == Material::Water
                    || state.native_state(c).is_some_and(|n| n.water)
            })
        {
            return Err("Placement into water needs supported displacement");
        }
        let mut staged_world = world.clone();
        let mut inventory = self.inventory.clone();
        t.apply(&mut staged_world, &mut inventory, owns)?;
        let mut staged = state.clone();
        for &(c, _) in &t.expected {
            if staged.contains_history(c) {
                staged
                    .set_solid(&staged_world, c, t.place.is_some())
                    .map_err(|_| "Unsupported moving water terrain edit")?;
            }
        }
        *world = staged_world;
        self.inventory = inventory;
        self.state = Some(staged);
        self.static_world = self
            .bounds
            .map(|(a, b)| world.without_fluid_in_bounds(a, b));
        self.epoch += 1;
        self.error = None;
        Ok(())
    }
    pub fn edit(&mut self, world: &mut World, c: Cell, material: Material) -> Result<()> {
        self.poll(world)?;
        let state = self
            .state
            .as_ref()
            .ok_or("Water step pending; retry edit")?;
        let mut staged = state.clone();
        let mut staged_world = world.clone();
        staged_world.set(c, material);
        if staged.contains_history(c) {
            staged.set_solid(
                &staged_world,
                c,
                !matches!(material, Material::Air | Material::Water),
            )?;
        }
        *world = staged_world;
        self.state = Some(staged);
        self.static_world = self
            .bounds
            .map(|(a, b)| world.without_fluid_in_bounds(a, b));
        self.epoch += 1;
        self.error = None;
        Ok(())
    }
    fn checkpoint(&self, world: &World) -> Result<Vec<u8>> {
        let state = self
            .state
            .as_ref()
            .ok_or("in-flight moving water checkpoint")?;
        let mut bytes = crate::grid_checkpoint::header(b"FVMWATR2");
        let edits: Vec<_> = world.terrain_edits().collect();
        put(&mut bytes, edits.len() as u64);
        for (c, m) in edits {
            for v in [c.x, c.y, c.z] {
                put(&mut bytes, v as u64)
            }
            bytes.push(m as u8);
        }
        bytes.extend(self.inventory.encode());
        put(&mut bytes, self.river_rate);
        for v in [self.simtime, self.debt, self.wall_elapsed] {
            put(&mut bytes, v.to_bits())
        }
        let (eye, yaw, pitch) = self.camera_pose.ok_or("missing moving camera pose")?;
        for v in eye {
            put(&mut bytes, v.to_bits())
        }
        bytes.extend(yaw.to_bits().to_le_bytes());
        bytes.extend(pitch.to_bits().to_le_bytes());
        bytes.extend(state.snapshot()?);
        Ok(bytes)
    }
    pub fn save(&mut self, world: &mut World) -> Result<()> {
        if self.state.is_none() {
            let done = self.rx.recv()?;
            self.accept(world, done)?;
        }
        let Some(path) = &self.path else {
            return Ok(());
        };
        let bytes = self.checkpoint(world)?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("moving-water.tmp");
        let mut file = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut file, &bytes)?;
        file.sync_all()?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
    fn restore(&mut self, world: &mut World, b: &[u8]) -> Result<()> {
        let b = crate::grid_checkpoint::payload(b)?;
        let legacy = b.get(..8) == Some(b"FVMWATR1");
        if !legacy && b.get(..8) != Some(b"FVMWATR2") {
            return Err("expected moving water save; other water models remain separate".into());
        }
        let mut p = 8;
        let n = read(b, &mut p)?;
        if n > 1_048_576 {
            return Err("moving terrain edit count exceeded".into());
        }
        let mut edits = Vec::new();
        for _ in 0..n {
            let c = Cell {
                x: read(b, &mut p)? as i64,
                y: read(b, &mut p)? as i64,
                z: read(b, &mut p)? as i64,
            };
            let material = *MATERIALS
                .get(*b.get(p).ok_or("truncated moving material")? as usize)
                .ok_or("invalid moving material")?;
            p += 1;
            edits.push((c, material));
        }
        let inventory = Inventory::decode(b.get(p..p + 104).ok_or("truncated moving inventory")?)
            .ok_or("invalid moving inventory")?;
        p += 104;
        let saved_rate = if legacy { 128 } else { read(b, &mut p)? };
        if saved_rate > 1_000_000 {
            return Err("saved river rate exceeds limit".into());
        }
        self.river_rate = self.requested_rate.unwrap_or(saved_rate);
        let mut values = [0.; 6];
        for v in &mut values {
            *v = f64::from_bits(read(b, &mut p)?);
            if !v.is_finite() {
                return Err("nonfinite moving checkpoint".into());
            }
        }
        if values[..3].iter().any(|&v| v < 0.) {
            return Err("negative moving elapsed time".into());
        }
        let yaw = f32::from_bits(read32(b, &mut p)?);
        let pitch = f32::from_bits(read32(b, &mut p)?);
        if !yaw.is_finite() || !pitch.is_finite() {
            return Err("invalid moving camera".into());
        }
        let mut staged_world = world.clone();
        staged_world.set_batch(edits);
        let state =
            MovingWater::restore(&staged_world, b.get(p..).ok_or("truncated moving state")?)?;
        state.validate_world(&staged_world)?;
        restore_history(&state, &mut staged_world)?;
        let view = prepare_view(&state, &staged_world)?;
        *world = staged_world;
        self.inventory = inventory;
        self.simtime = values[0];
        self.debt = values[1];
        self.wall_elapsed = values[2];
        self.started = self.simtime > 0.;
        self.camera_pose = Some(([values[3], values[4], values[5]], yaw, pitch));
        self.state = Some(state);
        self.publish(world, view)?;
        Ok(())
    }
}
fn put(b: &mut Vec<u8>, v: u64) {
    b.extend(v.to_le_bytes())
}
fn read(b: &[u8], p: &mut usize) -> Result<u64> {
    let end = p.checked_add(8).ok_or("moving offset overflow")?;
    let v = u64::from_le_bytes(b.get(*p..end).ok_or("truncated moving save")?.try_into()?);
    *p = end;
    Ok(v)
}
fn read32(b: &[u8], p: &mut usize) -> Result<u32> {
    let end = p.checked_add(4).ok_or("moving offset overflow")?;
    let v = u32::from_le_bytes(b.get(*p..end).ok_or("truncated moving pose")?.try_into()?);
    *p = end;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::{FbmParams, FbmRecipe};
    fn source() -> World {
        World::fbm(
            19,
            FbmParams {
                recipe: FbmRecipe::SeededWorld,
                ..Default::default()
            },
        )
        .unwrap()
    }
    fn started() -> (World, MovingWaterLive, Camera) {
        let mut world = source();
        let mut live = MovingWaterLive::new(&mut world, None, Some(0)).unwrap();
        let mut camera = Camera::fixture();
        camera.position.x = 0.;
        camera.position.z = 0.;
        live.start(&mut world, &mut camera).unwrap();
        (world, live, camera)
    }
    #[test]
    fn generated_moving_window_uses_native_terrain_and_reuses_identical_publication() {
        let (mut world, mut live, _) = started();
        assert_eq!(world.terrain_edits().count(), 0);
        let state = live.state.as_ref().unwrap();
        assert_eq!(state.simulation().dimensions(), [32; 3]);
        assert_eq!(state.tile_count(), 64);
        assert!(state.regional().conserved());
        let view = prepare_view(state, &world).unwrap();
        assert!(view.changed.is_empty());
        let generation = live.generation;
        let revision = world.revision();
        live.publish(&mut world, view).unwrap();
        assert_eq!(live.generation, generation);
        assert_eq!(world.revision(), revision);
    }
    #[test]
    fn moving_job_clean_save_restores_exact_state_inventory_actual_pose_and_rate() {
        let (mut world, mut live, mut camera) = started();
        let cell = Cell {
            x: 1_000_000,
            y: 16000,
            z: 1_000_000,
        };
        world.set(cell, Material::Stone);
        let pickup = EditTransaction::pickup(&live.inventory, vec![(cell, Material::Stone)]);
        live.apply_player_transaction(&mut world, &pickup, |_| true)
            .unwrap();
        camera.position.x += 4.;
        camera.yaw += 0.15;
        live.observe_camera(camera);
        let mut demand = live.state.as_ref().unwrap().origin();
        demand.0[0] += 4;
        live.river_rate = 0;
        live.tx
            .send(Job {
                state: live.state.take().unwrap(),
                world: world.clone(),
                demand,
                epoch: live.epoch,
                dt: 0.002,
                flow: FlowConfig {
                    upstream_units_per_second: 0,
                    downstream_units_per_second: 0,
                },
            })
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "frontier-moving-worker-{}.save",
            std::process::id()
        ));
        live.path = Some(path.clone());
        live.save(&mut world).unwrap();
        assert!(live.error.is_none(), "{:?}", live.error);
        assert!(live.state.as_ref().unwrap().migrations() > 0);
        let before = live.state.as_ref().unwrap().snapshot().unwrap();
        let mut other_world = source();
        let restored = MovingWaterLive::new(&mut other_world, Some(path.clone()), None).unwrap();
        assert_eq!(restored.inventory, live.inventory);
        assert_eq!(restored.camera_pose, live.camera_pose);
        assert_eq!(restored.river_rate, 0);
        assert_eq!(restored.state.as_ref().unwrap().snapshot().unwrap(), before);
        assert_eq!(other_world.material_without_fluid(cell), Material::Air);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn reload_restores_dry_dormant_history_outside_current_dynamic_bounds() {
        let (mut world, mut live, _) = started();
        let origin = live.state.as_ref().unwrap().origin();
        let cell = live
            .state
            .as_ref()
            .unwrap()
            .occupancy()
            .unwrap()
            .into_iter()
            .find(|&(c, wet)| wet && world.material_without_fluid(c) == Material::Water)
            .unwrap()
            .0;
        // A pre-existing authoritative dry tombstone must survive reactivation
        // and reload without borrowing/regenerating its original source water.
        world.set_fluid_occupancy([(cell, false)]);
        let mut state = MovingWater::new(&world, origin, HISTORY_TILES).unwrap();
        let mut away = origin;
        away.0[0] += 8;
        state.migrate(&world, away).unwrap();
        let view = prepare_view(&state, &world).unwrap();
        assert!(cell.x < view.bounds.0.x);
        live.state = Some(state);
        live.publish(&mut world, view).unwrap();
        let bytes = live.checkpoint(&world).unwrap();
        let mut other_world = source();
        let mut other = MovingWaterLive::new(&mut other_world, None, None).unwrap();
        other.restore(&mut other_world, &bytes).unwrap();
        assert_eq!(other_world.material(cell), Material::Air);
        assert_eq!(other.static_world().unwrap().material(cell), Material::Air);
        assert!(other.state.as_ref().unwrap().regional().conserved());
    }
}
