//! Fixed procedural near-bank patch with finite coarse neighbors. This is a
//! staged integration, not camera-following or complete regional hydrology.
use crate::{Camera, Result};
use frontier_world::{
    discrete_water::{coupled::CoupledWater, DiscreteWater, OpenBoundary, StepStats},
    edit::{EditTransaction, Inventory, MATERIALS},
    procedural_water::{ProceduralWater, TileGeometry, TileKey},
    voxel_size, Cell, Material, World,
};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender},
    time::Instant,
};

struct State {
    coupled: CoupledWater,
    registry: ProceduralWater,
    key: TileKey,
}
impl State {
    fn new(w: &World, key: TileKey) -> Result<Self> {
        let mut registry = ProceduralWater::new(w, 64)?;
        let tile = registry.register(w, key)?.clone();
        let o = key.origin()?;
        let mut sim = DiscreteWater::new(o, [16; 3])?;
        for z in 0..16 {
            for y in 0..16 {
                for x in 0..16 {
                    let c = Cell {
                        x: o.x + x,
                        y: o.y + y,
                        z: o.z + z,
                    };
                    sim.set_solid(c, tile.geometry.solid_at(c).unwrap())?;
                    if tile.geometry.initially_wet(c) == Some(true) {
                        sim.set_water(c, true)?;
                    }
                }
            }
        }
        let head = (tile.geometry.initial_head_m()? * 1000.).round() as i64;
        registry.regional_mut().add_reservoir(0, 4096, 0, head)?;
        let token = registry
            .regional_mut()
            .take_region(tile.reservoir as u64, 0, 0)?;
        let token = registry.regional_mut().exchange_owned(
            1,
            token,
            tile.reservoir,
            sim.mass() as u64,
            true,
        )?;
        for axis in 0..3 {
            for positive in [false, true] {
                let mut k = key.0;
                k[axis] += if positive { 1 } else { -1 };
                registry.register(w, TileKey(k))?;
            }
        }
        let coupled = CoupledWater::new(sim, registry.regional().clone(), token, 2)?;
        let mut state = Self {
            coupled,
            registry,
            key,
        };
        state.rebuild_ports(w)?;
        Ok(state)
    }
    fn neighbor(c: Cell, axis: usize, positive: bool) -> Cell {
        let mut p = [c.x, c.y, c.z];
        p[axis] += if positive { 1 } else { -1 };
        Cell {
            x: p[0],
            y: p[1],
            z: p[2],
        }
    }
    fn head(geometry: &TileGeometry, units: u64) -> Result<f64> {
        if units == 0 {
            return Ok(geometry.stage_volume.local_head_m(0)?);
        }
        if geometry.initial_units() == geometry.stage_volume.capacity()
            && geometry.initial_units() > 0
        {
            Ok(geometry.initial_head_m()?
                + (units as f64 - geometry.initial_units() as f64) / 256. * voxel_size())
        } else {
            Ok(geometry.stage_volume.local_head_m(units)?)
        }
    }
    fn rebuild_ports(&mut self, w: &World) -> Result<()> {
        let o = self.key.origin()?;
        let old: Vec<_> = self
            .coupled
            .simulation()
            .boundaries()
            .map(|(id, _)| id)
            .collect();
        let mut keep = BTreeSet::new();
        for z in 0..16 {
            for y in 0..16 {
                for x in 0..16 {
                    let c = Cell {
                        x: o.x + x,
                        y: o.y + y,
                        z: o.z + z,
                    };
                    if self.coupled.simulation().solid_at(c) {
                        continue;
                    }
                    for axis in 0..3 {
                        for positive in [false, true] {
                            let v = [x, y, z][axis];
                            if v != if positive { 15 } else { 0 } {
                                continue;
                            }
                            let outside = Self::neighbor(c, axis, positive);
                            if fluid_solid(w.material_without_fluid(outside)) {
                                continue;
                            }
                            let tile = self
                                .registry
                                .tile(TileKey::containing(outside))
                                .ok_or("unregistered coarse neighbor")?;
                            let units = self
                                .coupled
                                .regional()
                                .reservoir(tile.reservoir)
                                .ok_or("missing coarse reservoir")?
                                .volume;
                            let id = (x + 16 * (y + 16 * z)) as u64 * 6
                                + (axis * 2 + usize::from(positive)) as u64
                                + 1;
                            keep.insert(id);
                            self.coupled.set_port(
                                id,
                                tile.reservoir,
                                OpenBoundary {
                                    cell: c,
                                    axis,
                                    positive,
                                    head_m: Self::head(&tile.geometry, units)?,
                                    velocity_mps: [0.; 3],
                                    import_quota: 0,
                                    export_quota: 0,
                                },
                            )?;
                        }
                    }
                }
            }
        }
        for id in old {
            if !keep.contains(&id) {
                self.coupled.remove_port(id);
            }
        }
        Ok(())
    }
    fn validate_solids(&self, w: &World) -> Result<()> {
        for (&key, tile) in self.registry.tiles() {
            let o = key.origin()?;
            for z in 0..16 {
                for x in 0..16 {
                    let column = w.column(o.x + x, o.z + z);
                    for y in 0..16 {
                        let c = Cell {
                            x: o.x + x,
                            y: o.y + y,
                            z: o.z + z,
                        };
                        let saved = if key == self.key {
                            self.coupled.simulation().solid_at(c)
                        } else {
                            tile.geometry.solid_at(c).ok_or("tile geometry bounds")?
                        };
                        if saved != fluid_solid(column.canonical_material_without_fluid(c.y)) {
                            return Err(
                                "saved procedural solids disagree with canonical terrain".into()
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn advance(&mut self, dt: f64) -> Result<StepStats> {
        let ports: Vec<_> = self.coupled.simulation().boundaries().collect();
        for (id, mut port) in ports {
            let outside = Self::neighbor(port.cell, port.axis, port.positive);
            let tile = self
                .registry
                .tile(TileKey::containing(outside))
                .ok_or("missing port geometry")?;
            let r = self
                .coupled
                .regional()
                .reservoir(tile.reservoir)
                .ok_or("missing port reservoir")?;
            port.head_m = Self::head(&tile.geometry, r.volume)?;
            self.coupled.set_port(id, tile.reservoir, port)?;
        }
        let stats = self.coupled.step(dt)?;
        *self.registry.regional_mut() = self.coupled.regional().clone();
        Ok(stats)
    }
    fn occupancy(&self) -> Result<Vec<(Cell, bool)>> {
        let mut out = Vec::new();
        for (&key, tile) in self.registry.tiles() {
            let wet: BTreeSet<_> = if key == self.key {
                self.coupled.simulation().occupied_cells().collect()
            } else {
                let units = self
                    .coupled
                    .regional()
                    .reservoir(tile.reservoir)
                    .ok_or("missing tile reservoir")?
                    .volume;
                tile.geometry.occupied_cells(units)?.into_iter().collect()
            };
            let o = key.origin()?;
            for z in 0..16 {
                for y in 0..16 {
                    for x in 0..16 {
                        let c = Cell {
                            x: o.x + x,
                            y: o.y + y,
                            z: o.z + z,
                        };
                        out.push((c, wet.contains(&c)));
                    }
                }
            }
        }
        Ok(out)
    }
    fn checkpoint(&self) -> Vec<u8> {
        let a = self.coupled.snapshot();
        let mut registry = self.registry.clone();
        *registry.regional_mut() = self.coupled.regional().clone();
        let b = registry.snapshot();
        let mut out = b"DWPFIX01".to_vec();
        for v in self.key.0 {
            out.extend(v.to_le_bytes())
        }
        out.extend((a.len() as u64).to_le_bytes());
        out.extend(a);
        out.extend(b);
        out
    }
    fn restore(w: &World, b: &[u8]) -> Result<Self> {
        if b.len() < 40 || b.get(..8) != Some(b"DWPFIX01") {
            return Err("invalid fixed procedural state".into());
        }
        let mut p = 8;
        let key = TileKey([
            read(b, &mut p)? as i64,
            read(b, &mut p)? as i64,
            read(b, &mut p)? as i64,
        ]);
        let n = usize::try_from(read(b, &mut p)?)?;
        let end = p.checked_add(n).ok_or("procedural length overflow")?;
        let coupled = CoupledWater::restore(b.get(p..end).ok_or("truncated coupled state")?)?;
        let registry = ProceduralWater::restore(w, b.get(end..).ok_or("truncated registry")?)?;
        if coupled.simulation().origin() != key.origin()?
            || coupled.simulation().dimensions() != [16; 3]
            || registry.regional().snapshot() != coupled.regional().snapshot()
        {
            return Err("procedural state/registry mismatch".into());
        }
        Ok(Self {
            coupled,
            registry,
            key,
        })
    }
}
struct Completed {
    state: State,
    epoch: u64,
    stats: StepStats,
    ms: f64,
}
pub struct ProceduralWaterLive {
    pub inventory: Inventory,
    state: Option<State>,
    tx: SyncSender<(State, u64, f64)>,
    rx: Receiver<std::result::Result<Completed, (State, u64, f64, String)>>,
    pub center: Option<Cell>,
    path: Option<PathBuf>,
    static_world: Option<World>,
    clock: Instant,
    debt: f64,
    epoch: u64,
    generation: u64,
    simtime: f64,
    wall_elapsed: f64,
    started: bool,
    worker_error: Option<String>,
    last_occupancy: Vec<(Cell, bool)>,
    metrics: String,
    dynamic: Option<(Cell, [u32; 3], u64, Vec<u32>)>,
    camera_pose: Option<([f64; 3], f32, f32)>,
}
impl ProceduralWaterLive {
    pub fn new(w: &mut World, path: Option<PathBuf>) -> Result<Self> {
        let (tx, commands) = mpsc::sync_channel::<(State, u64, f64)>(1);
        let (results, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("procedural-water".into())
            .spawn(move || {
                while let Ok((mut state, epoch, dt)) = commands.recv() {
                    let start = Instant::now();
                    let result = match state.advance(dt) {
                        Ok(stats) => Ok(Completed {
                            state,
                            epoch,
                            stats,
                            ms: start.elapsed().as_secs_f64() * 1000.,
                        }),
                        Err(error) => Err((state, epoch, dt, error.to_string())),
                    };
                    if results.send(result).is_err() {
                        break;
                    }
                }
            })?;
        let mut v = Self {
            inventory: Inventory::default(),
            state: None,
            tx,
            rx,
            center: None,
            path,
            static_world: None,
            clock: Instant::now(),
            debt: 0.,
            epoch: 0,
            generation: 0,
            simtime: 0.,
            wall_elapsed: 0.,
            started: false,
            worker_error: None,
            last_occupancy: Vec::new(),
            metrics: "{}".into(),
            dynamic: None,
            camera_pose: None,
        };
        if let Some(p) = &v.path {
            if p.exists() {
                let b = std::fs::read(p)?;
                v.restore(w, &b)?;
            }
        }
        Ok(v)
    }
    pub fn position_camera(&self, camera: &mut Camera) {
        if let Some((eye, yaw, pitch)) = self.camera_pose {
            camera.position = glam::DVec3::from_array(eye);
            camera.yaw = yaw;
            camera.pitch = pitch;
        }
    }
    pub fn start(&mut self, w: &mut World, camera: &mut Camera) -> Result<()> {
        let (key, water, normal) = choose_bank(w, camera)?;
        self.state = Some(State::new(w, key)?);
        self.center = Some(key.origin()?);
        self.static_world = Some(w.without_fluid_snapshot());
        self.camera_pose = Some((
            [
                water[0] + normal[0] * 1.3,
                water[1] + 1.7,
                water[2] + normal[1] * 1.3,
            ],
            (-normal[0]).atan2(-normal[1]) as f32,
            -0.86,
        ));
        self.position_camera(camera);
        self.publish(w)?;
        eprintln!("procedural fixed water patch at {:?}: actual seeded bank, 1m detailed tile + six finite coarse neighbors; open head boundaries, no fixture/source injection; camera-following not enabled",key);
        Ok(())
    }
    fn publish(&mut self, w: &mut World) -> Result<()> {
        let publish_start = Instant::now();
        let state = self.state.as_ref().ok_or("missing procedural state")?;
        let occupancy = state.occupancy()?;
        let same_layout = self.last_occupancy.len() == occupancy.len()
            && self
                .last_occupancy
                .iter()
                .zip(&occupancy)
                .all(|(a, b)| a.0 == b.0);
        if occupancy != self.last_occupancy {
            let changed = if same_layout {
                occupancy
                    .iter()
                    .zip(&self.last_occupancy)
                    .filter_map(|(&next, &old)| (next != old).then_some(next))
                    .collect::<Vec<_>>()
            } else {
                occupancy.clone()
            };
            w.set_fluid_occupancy(changed);
            self.generation += 1;
            let o = state.key.origin()?;
            let origin = Cell {
                x: o.x - 16,
                y: o.y - 16,
                z: o.z - 16,
            };
            let mut states = vec![0; 48usize.pow(3)];
            for &(c, wet) in &occupancy {
                let i = (c.x - origin.x) as usize
                    + 48 * ((c.y - origin.y) as usize + 48 * (c.z - origin.z) as usize);
                states[i] = if wet { 2 } else { 1 };
            }
            self.dynamic = Some((origin, [48; 3], self.generation, states));
            self.last_occupancy = occupancy;
        }
        let r = state.coupled.regional();
        let l = r.ledger();
        let s = state.coupled.simulation();
        self.metrics=format!("{{\"model\":\"procedural\",\"fixed_patch\":true,\"camera_following\":false,\"tick\":{},\"simtime\":{},\"wall_elapsed\":{},\"wall_lag\":{},\"detailed_units\":{},\"stored_units\":{},\"initial\":{},\"supplied\":{},\"drained\":{},\"conserved\":{},\"registered_tiles\":{},\"open_faces\":{},\"edit_epoch\":{}}}",s.tick(),self.simtime,self.wall_elapsed,(self.wall_elapsed-self.simtime).max(0.),s.mass(),r.stored_units(),l.initial,l.supplied,l.drained,r.conserved(),state.registry.tiles().count(),s.boundaries().count(),self.epoch);
        self.metrics.pop();
        self.metrics.push_str(&format!(
            ",\"publish_ms\":{}}}",
            publish_start.elapsed().as_secs_f64() * 1000.
        ));
        Ok(())
    }
    fn accept(
        &mut self,
        w: &mut World,
        result: std::result::Result<Completed, (State, u64, f64, String)>,
    ) -> Result<()> {
        let done = match result {
            Ok(done) => done,
            Err((state, epoch, dt, error)) => {
                // The failed staged transaction retains its exact owned state.
                // Stop dispatching, preserve requested time and permit saving.
                if epoch != self.epoch {
                    return Err("procedural water failure epoch mismatch".into());
                }
                self.state = Some(state);
                self.debt += dt;
                self.worker_error = Some(error.clone());
                self.publish(w)?;
                eprintln!("procedural water paused, state retained: {error}");
                return Ok(());
            }
        };
        if done.epoch != self.epoch {
            return Err("procedural water epoch mismatch".into());
        }
        self.simtime += done.stats.simulated_dt;
        self.debt += done.stats.deferred_dt;
        self.state = Some(done.state);
        self.publish(w)?;
        self.metrics.pop();
        self.metrics.push_str(&format!(",\"last_step_ms\":{},\"boundary_transfers\":{},\"max_speed_mps\":{},\"pressure_residual_rms\":{}}}",done.ms,done.stats.boundary_transfers.len(),done.stats.max_speed_mps,done.stats.pressure_residual_rms));
        Ok(())
    }
    fn poll(&mut self, w: &mut World) -> Result<()> {
        match self.rx.try_recv() {
            Ok(done) => self.accept(w, done)?,
            Err(mpsc::TryRecvError::Disconnected) if self.state.is_none() => {
                return Err("procedural water worker disconnected".into())
            }
            _ => {}
        }
        Ok(())
    }
    pub fn update(&mut self, w: &mut World) -> Result<()> {
        self.poll(w)?;
        if self.worker_error.is_some() {
            self.wait_for_render();
            return Ok(());
        }
        let elapsed = self.clock.elapsed().as_secs_f64();
        if elapsed >= 0.1 {
            if let Some(state) = self.state.take() {
                if !self.started {
                    eprintln!("procedural water simulation started");
                }
                self.started = true;
                self.wall_elapsed += elapsed;
                // Preserve elapsed time rather than dropping every frame's
                // scheduling overshoot. Worker and solver budgets remain bounded.
                let available = elapsed + self.debt;
                let dt = available.min(0.25);
                self.debt = available - dt;
                self.tx.send((state, self.epoch, dt))?;
                self.clock = Instant::now();
            }
        }
        Ok(())
    }
    pub fn wait_for_render(&mut self) {
        if self.started {
            self.wall_elapsed += self.clock.elapsed().as_secs_f64();
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
            ",\"pending_snapshots\":{},\"paused_error\":{:?}}}",
            usize::from(self.state.is_none()),
            self.worker_error.as_deref().unwrap_or("")
        ));
        s
    }
    pub fn apply_player_transaction(
        &mut self,
        w: &mut World,
        t: &EditTransaction,
        owns: impl Fn(Cell) -> bool,
    ) -> std::result::Result<(), &'static str> {
        self.poll(w)
            .map_err(|_| "Procedural water worker unavailable")?;
        let state = self
            .state
            .as_mut()
            .ok_or("Water step pending; retry edit")?;
        for &(c, _) in &t.expected {
            if TileKey::containing(c) != state.key
                && state.registry.tile(TileKey::containing(c)).is_some()
            {
                return Err("Coarse water neighbor needs detailed activation before editing");
            }
        }
        if t.place.is_some()
            && t.expected
                .iter()
                .any(|&(c, _)| w.material(c) == Material::Water)
        {
            return Err("Placement into water needs supported displacement");
        }
        // Stage solver edits before committing terrain/inventory, including port
        // closure. A stale inventory or residency check leaves this copy unused.
        let mut staged = state.coupled.clone();
        for &(c, _) in &t.expected {
            if staged.simulation().contains(c) {
                if t.place.is_some() {
                    let ids: Vec<_> = staged
                        .simulation()
                        .boundaries()
                        .filter(|(_, p)| p.cell == c)
                        .map(|(id, _)| id)
                        .collect();
                    for id in ids {
                        staged.remove_port(id);
                    }
                }
                staged
                    .set_solid(c, t.place.is_some())
                    .map_err(|_| "Unsupported hydraulic boundary edit")?;
            }
        }
        t.apply(w, &mut self.inventory, owns)?;
        state.coupled = staged;
        state
            .rebuild_ports(w)
            .map_err(|_| "Hydraulic port rebuild failed")?;
        self.static_world = Some(w.without_fluid_snapshot());
        self.epoch += 1;
        Ok(())
    }
    pub fn edit(&mut self, w: &mut World, c: Cell, m: Material) -> Result<()> {
        self.poll(w)?;
        let state = self
            .state
            .as_mut()
            .ok_or("Water step pending; retry edit")?;
        if state.registry.tile(TileKey::containing(c)).is_some()
            && TileKey::containing(c) != state.key
        {
            return Err("Coarse neighbor edit needs detailed activation".into());
        }
        let mut staged = state.coupled.clone();
        if staged.simulation().contains(c) {
            if fluid_solid(m) {
                let ids: Vec<_> = state
                    .coupled
                    .simulation()
                    .boundaries()
                    .filter(|(_, p)| p.cell == c)
                    .map(|(id, _)| id)
                    .collect();
                for id in ids {
                    staged.remove_port(id);
                }
            }
            staged.set_solid(c, fluid_solid(m))?;
        }
        w.set(c, m);
        state.coupled = staged;
        state.rebuild_ports(w)?;
        self.static_world = Some(w.without_fluid_snapshot());
        self.epoch += 1;
        Ok(())
    }
    pub fn save(&mut self, w: &mut World) -> Result<()> {
        if self.state.is_none() {
            let done = self.rx.recv()?;
            self.accept(w, done)?;
        }
        let Some(path) = &self.path else {
            return Ok(());
        };
        let state = self.state.as_ref().ok_or("missing procedural save state")?;
        let mut b = crate::grid_checkpoint::header(b"FVPWATR1");
        let edits: Vec<_> = w.terrain_edits().collect();
        b.extend((edits.len() as u64).to_le_bytes());
        for (c, m) in edits {
            for v in [c.x, c.y, c.z] {
                b.extend(v.to_le_bytes())
            }
            b.push(m as u8)
        }
        b.extend(self.inventory.encode());
        for v in [self.simtime, self.debt, self.wall_elapsed] {
            b.extend(v.to_bits().to_le_bytes())
        }
        let (eye, yaw, pitch) = self.camera_pose.ok_or("missing procedural camera")?;
        for v in eye {
            b.extend(v.to_bits().to_le_bytes())
        }
        b.extend(yaw.to_bits().to_le_bytes());
        b.extend(pitch.to_bits().to_le_bytes());
        b.extend(state.checkpoint());
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("procedural-save.tmp");
        let mut file = std::fs::File::create(&temp)?;
        std::io::Write::write_all(&mut file, &b)?;
        file.sync_all()?;
        std::fs::rename(temp, path)?;
        Ok(())
    }
    fn restore(&mut self, w: &mut World, b: &[u8]) -> Result<()> {
        let b = crate::grid_checkpoint::payload(b)?;
        if b.get(..8) != Some(b"FVPWATR1") {
            return Err("expected procedural water save; CA/discrete saves remain separate".into());
        }
        let mut p = 8;
        let n = read(b, &mut p)?;
        if n > 1_048_576 {
            return Err("procedural terrain edit limit".into());
        }
        let mut edits = Vec::new();
        for _ in 0..n {
            let c = Cell {
                x: read(b, &mut p)? as i64,
                y: read(b, &mut p)? as i64,
                z: read(b, &mut p)? as i64,
            };
            let m = *MATERIALS
                .get(*b.get(p).ok_or("truncated material")? as usize)
                .ok_or("invalid material")?;
            p += 1;
            edits.push((c, m));
        }
        self.inventory = Inventory::decode(b.get(p..p + 104).ok_or("truncated inventory")?)
            .ok_or("invalid inventory")?;
        p += 104;
        let mut numbers = [0.; 6];
        for v in &mut numbers {
            *v = f64::from_bits(read(b, &mut p)?);
            if !v.is_finite() {
                return Err("nonfinite procedural save".into());
            }
        }
        if numbers[..3].iter().any(|&v| v < 0.) {
            return Err("negative procedural elapsed time".into());
        }
        let yaw = f32::from_bits(read32(b, &mut p)?);
        let pitch = f32::from_bits(read32(b, &mut p)?);
        if !yaw.is_finite() || !pitch.is_finite() {
            return Err("nonfinite procedural camera".into());
        }
        let state = State::restore(w, b.get(p..).ok_or("truncated procedural state")?)?;
        let mut staged_world = w.clone();
        staged_world.set_batch(edits);
        state.validate_solids(&staged_world)?;
        *w = staged_world;
        self.simtime = numbers[0];
        self.debt = numbers[1];
        self.wall_elapsed = numbers[2];
        self.started = self.simtime > 0.;
        self.camera_pose = Some(([numbers[3], numbers[4], numbers[5]], yaw, pitch));
        self.center = Some(state.key.origin()?);
        self.static_world = Some(w.without_fluid_snapshot());
        self.state = Some(state);
        self.publish(w)?;
        Ok(())
    }
}
pub(crate) fn choose_bank(w: &World, camera: &Camera) -> Result<(TileKey, [f64; 3], [f64; 2])> {
    let t = w
        .fbm_terrain()
        .ok_or("procedural water requires SeededWorld")?;
    let radius = 4800. * t.params().horizontal_scale / 1800.;
    let center = [camera.position.x, camera.position.z];
    let mut reaches = t.water_reaches(center.map(|v| v - radius), center.map(|v| v + radius))?;
    reaches.retain(|r| !r.lake && r.upstream.stage_m > 0.);
    reaches.sort_by(|a, b| {
        let distance = |r: &frontier_world::procedural_water::WaterReach| {
            let x = (r.upstream.x_m + r.downstream.x_m) * 0.5 - center[0];
            let z = (r.upstream.z_m + r.downstream.z_m) * 0.5 - center[1];
            x * x + z * z
        };
        distance(a).total_cmp(&distance(b))
    });
    let mut best = None;
    for r in reaches.iter().take(12) {
        let dx = r.downstream.x_m - r.upstream.x_m;
        let dz = r.downstream.z_m - r.upstream.z_m;
        let length = dx.hypot(dz);
        if length == 0. {
            continue;
        }
        let normal = [-dz / length, dx / length];
        let u = (((center[0] - r.upstream.x_m) * dx + (center[1] - r.upstream.z_m) * dz)
            / (length * length))
            .clamp(0.1, 0.9);
        let middle = [r.upstream.x_m + u * dx, r.upstream.z_m + u * dz];
        let width = r.upstream_radius_m + u * (r.downstream_radius_m - r.upstream_radius_m);
        for sign in [-1., 1.] {
            for i in 8..=40 {
                let distance = width * i as f64 / 40.;
                let x = middle[0] + normal[0] * sign * distance;
                let z = middle[1] + normal[1] * sign * distance;
                let c = Cell::from_meters([x, 0., z]);
                let column = t.column_cells(c.x, c.z);
                let depth = column.visible_top - column.ground_top;
                if !(1..=16).contains(&depth) {
                    continue;
                }
                let cost = (x - center[0]).hypot(z - center[1]) + ((depth - 6).abs() as f64) * 0.05;
                let key = TileKey::containing(Cell {
                    x: c.x,
                    y: column.visible_top - 1,
                    z: c.z,
                });
                if best.as_ref().is_none_or(|(old, _, _, _)| cost < *old) {
                    best = Some((
                        cost,
                        key,
                        [x, column.visible_top as f64 * voxel_size(), z],
                        [normal[0] * sign, normal[1] * sign],
                    ));
                }
            }
        }
    }
    let (_,key,water,normal)=best.ok_or("no shallow generated river bank found within bounded hydrology query; choose --start-xz near a river")?;
    Ok((key, water, normal))
}
fn fluid_solid(material: Material) -> bool {
    !matches!(material, Material::Air | Material::Water)
}
fn read(b: &[u8], p: &mut usize) -> Result<u64> {
    let end = p.checked_add(8).ok_or("save length overflow")?;
    let v = u64::from_le_bytes(
        b.get(*p..end)
            .ok_or("truncated procedural save")?
            .try_into()?,
    );
    *p = end;
    Ok(v)
}
fn read32(b: &[u8], p: &mut usize) -> Result<u32> {
    let end = p.checked_add(4).ok_or("save length overflow")?;
    let v = u32::from_le_bytes(
        b.get(*p..end)
            .ok_or("truncated procedural save")?
            .try_into()?,
    );
    *p = end;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::{FbmParams, FbmRecipe};
    fn source_world() -> World {
        World::fbm(
            19,
            FbmParams {
                recipe: FbmRecipe::SeededWorld,
                ..Default::default()
            },
        )
        .unwrap()
    }
    fn started() -> (World, ProceduralWaterLive) {
        let mut world = source_world();
        let mut live = ProceduralWaterLive::new(&mut world, None).unwrap();
        let mut camera = Camera::fixture();
        camera.position.x = 0.;
        camera.position.z = 0.;
        live.start(&mut world, &mut camera).unwrap();
        (world, live)
    }
    #[test]
    fn generated_patch_has_no_fixture_or_implicit_sources_and_restart_is_exact() {
        let (world, live) = started();
        assert_eq!(world.terrain_edits().count(), 0);
        let state = live.state.as_ref().unwrap();
        assert_eq!(state.registry.tiles().count(), 7);
        assert!(state.coupled.simulation().boundaries().count() > 0);
        assert!(!state.coupled.simulation().has_internal_endpoints());
        assert_eq!(state.coupled.regional().ledger().supplied, 0);
        assert_eq!(
            state.coupled.simulation().mass() as u64,
            state.coupled.ownership().units
        );
        assert!(state.coupled.regional().conserved());
        let before = state.checkpoint();
        let mut other = State::restore(&world, &before).unwrap();
        assert_eq!(other.checkpoint(), before);
        other.advance(0.01).unwrap();
        assert!(other.coupled.regional().conserved());
        assert_eq!(other.coupled.regional().ledger().supplied, 0);
        let occupancy = other.occupancy().unwrap();
        let wet = occupancy.iter().filter(|(_, wet)| *wet).count() as u64;
        assert_eq!(
            wet,
            other.coupled.regional().stored_units() + other.coupled.regional().detailed_units()
        );
    }
    #[test]
    fn failed_worker_transaction_retains_exact_owned_state_and_saveability() {
        let (mut world, mut live) = started();
        let state = live.state.take().unwrap();
        let before = state.checkpoint();
        live.accept(
            &mut world,
            Err((
                state,
                live.epoch,
                0.1,
                "injected transaction failure".into(),
            )),
        )
        .unwrap();
        assert_eq!(live.state.as_ref().unwrap().checkpoint(), before);
        assert_eq!(live.debt, 0.1);
        assert!(live.worker_error.is_some());
        live.save(&mut world).unwrap();
        assert!(live.state.is_some());
        let generation = live.generation;
        let revision = world.revision();
        live.publish(&mut world).unwrap();
        assert_eq!(generation, live.generation);
        assert_eq!(revision, world.revision());
    }
    #[test]
    fn clean_save_waits_for_worker_and_restores_inventory_without_water_regeneration() {
        let (mut world, mut live) = started();
        let c = Cell {
            x: 1_000_000,
            y: 16000,
            z: 1_000_000,
        };
        world.set(c, Material::Stone);
        let pickup = EditTransaction::pickup(&live.inventory, vec![(c, Material::Stone)]);
        live.apply_player_transaction(&mut world, &pickup, |_| true)
            .unwrap();
        live.camera_pose = Some(([1., 2., 3.], 0., 0.));
        let path = std::env::temp_dir().join(format!(
            "frontier-procedural-water-{}.save",
            std::process::id()
        ));
        live.path = Some(path.clone());
        live.tx
            .send((live.state.take().unwrap(), live.epoch, 0.01))
            .unwrap();
        live.save(&mut world).unwrap();
        let before = live.state.as_ref().unwrap().checkpoint();
        let mut other_world = source_world();
        let restored = ProceduralWaterLive::new(&mut other_world, Some(path.clone())).unwrap();
        assert_eq!(restored.inventory, live.inventory);
        assert_eq!(restored.state.as_ref().unwrap().checkpoint(), before);
        assert_eq!(other_world.material_without_fluid(c), Material::Air);
        std::fs::remove_file(path).unwrap();
    }
}
