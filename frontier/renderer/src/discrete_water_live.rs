//! Bounded whole-voxel demonstration. A single owned simulation moves to the
//! worker and back; gameplay never speculates edits against an in-flight state.
use crate::{live_water::LiveWater, Camera, Result};
use frontier_world::{
    discrete_water::{DiscreteWater, StepStats},
    edit::{EditTransaction, Inventory, MATERIALS},
    voxel_size, Cell, Material, World,
};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender},
    time::Instant,
};

pub enum WaterDriver {
    Ca(LiveWater),
    Discrete(DiscreteWaterLive),
    Procedural(crate::procedural_water_live::ProceduralWaterLive),
    Moving(crate::moving_water_live::MovingWaterLive),
}
impl WaterDriver {
    pub fn new(
        world: &mut World,
        path: Option<PathBuf>,
        radius: u32,
        history: usize,
        discrete: bool,
        procedural: bool,
        moving: bool,
        river_rate: Option<u64>,
    ) -> Result<Self> {
        Ok(if moving {
            Self::Moving(crate::moving_water_live::MovingWaterLive::new(
                world, path, river_rate,
            )?)
        } else if procedural {
            Self::Procedural(crate::procedural_water_live::ProceduralWaterLive::new(
                world, path,
            )?)
        } else if discrete {
            Self::Discrete(DiscreteWaterLive::new(world, path)?)
        } else {
            Self::Ca(LiveWater::new(world, path, radius, history)?)
        })
    }
    pub fn inventory(&self) -> &Inventory {
        match self {
            Self::Moving(v) => &v.inventory,
            Self::Procedural(v) => &v.inventory,
            Self::Ca(v) => &v.inventory,
            Self::Discrete(v) => &v.inventory,
        }
    }
    pub fn inventory_mut(&mut self) -> &mut Inventory {
        match self {
            Self::Moving(v) => &mut v.inventory,
            Self::Procedural(v) => &mut v.inventory,
            Self::Ca(v) => &mut v.inventory,
            Self::Discrete(v) => &mut v.inventory,
        }
    }
    pub fn demo_center(&self) -> Option<Cell> {
        match self {
            Self::Moving(v) => v.center,
            Self::Procedural(v) => v.center,
            Self::Ca(v) => v.demo_center,
            Self::Discrete(v) => v.center,
        }
    }
    pub fn position_demo_camera(&self, camera: &mut Camera) {
        if let Self::Moving(v) = self {
            v.position_camera(camera);
            return;
        }
        if let Self::Procedural(v) = self {
            v.position_camera(camera);
            return;
        }
        let Some(c) = self.demo_center() else { return };
        let discrete = matches!(self, Self::Discrete(_));
        camera.position = glam::DVec3::new(
            c.x as f64 * voxel_size() + if discrete { 0.9 } else { 0. },
            c.y as f64 * voxel_size() + if discrete { 1.9 } else { 2.2 },
            c.z as f64 * voxel_size() + if discrete { 2.2 } else { -2. },
        );
        camera.yaw = if discrete { -2.84 } else { 0. };
        camera.pitch = if discrete { -0.42 } else { -0.7 };
    }
    pub fn demo(&mut self, w: &mut World, c: &mut Camera) -> Result<()> {
        match self {
            Self::Moving(v) => v.start(w, c),
            Self::Procedural(v) => v.start(w, c),
            Self::Ca(v) => v.demo(w, c),
            Self::Discrete(v) => v.demo(w, c),
        }
    }
    pub fn observe_camera(&mut self, camera: Camera) {
        if let Self::Moving(v) = self {
            v.observe_camera(camera);
        }
    }
    pub fn wait_for_render(&mut self) {
        match self {
            Self::Moving(v) => v.wait_for_render(),
            Self::Procedural(v) => v.wait_for_render(),
            Self::Ca(v) => v.wait_for_render(),
            Self::Discrete(v) => {
                if v.started {
                    v.wall_elapsed += v.clock.elapsed().as_secs_f64();
                }
                v.clock = Instant::now();
            }
        }
    }
    pub fn update(&mut self, w: &mut World, c: Camera) -> Result<()> {
        match self {
            Self::Moving(v) => v.update(w, c),
            Self::Procedural(v) => v.update(w),
            Self::Ca(v) => v.update(w, c),
            Self::Discrete(v) => v.update(w),
        }
    }
    pub fn edit(&mut self, w: &mut World, c: Cell, m: Material) {
        match self {
            Self::Moving(v) => {
                if let Err(e) = v.edit(w, c, m) {
                    eprintln!("{e}")
                }
            }
            Self::Procedural(v) => {
                if let Err(e) = v.edit(w, c, m) {
                    eprintln!("{e}")
                }
            }
            Self::Ca(v) => v.edit(w, c, m),
            Self::Discrete(v) => {
                if let Err(e) = v.edit(w, c, m) {
                    eprintln!("{e}")
                }
            }
        }
    }
    pub fn apply_player_transaction(
        &mut self,
        w: &mut World,
        t: &EditTransaction,
        owns: impl Fn(Cell) -> bool,
    ) -> std::result::Result<(), &'static str> {
        match self {
            Self::Moving(v) => v.apply_player_transaction(w, t, owns),
            Self::Procedural(v) => v.apply_player_transaction(w, t, owns),
            Self::Ca(v) => v.apply_player_transaction(w, t, owns),
            Self::Discrete(v) => v.apply_player_transaction(w, t, owns),
        }
    }
    pub fn open_dam(&mut self, w: &mut World) {
        match self {
            Self::Moving(_) => eprintln!("Moving water follows the player; no artificial dam"),
            Self::Procedural(_) => eprintln!("Procedural patch uses existing river geometry; dig dry terrain to change local flow"),
            Self::Ca(v) => v.open_dam(w),
            Self::Discrete(_) => eprintln!(
                "Discrete waterfall supply/outlet are continuous; dig basin walls to redirect flow"
            ),
        }
    }
    pub fn add_water(&mut self, w: &mut World, c: Cell) -> Result<()> {
        match self {
            Self::Moving(_) => {
                Err("Moving finite water has no unaccounted creative additions".into())
            }
            Self::Procedural(_) => {
                Err("Procedural finite water has no unaccounted creative additions".into())
            }
            Self::Ca(v) => v.add_water(w, c),
            Self::Discrete(v) => {
                v.poll(w)?;
                let sim = v.sim.as_mut().ok_or("Water step pending; retry edit")?;
                sim.set_water(c, true)?;
                v.publish(w);
                Ok(())
            }
        }
    }
    pub fn save(&mut self, w: &mut World) -> Result<()> {
        match self {
            Self::Moving(v) => v.save(w),
            Self::Procedural(v) => v.save(w),
            Self::Ca(v) => v.save(w),
            Self::Discrete(v) => v.save(w),
        }
    }
    pub fn diagnostics_json(&self) -> String {
        match self {
            Self::Moving(v) => v.diagnostics_json(),
            Self::Procedural(v) => v.diagnostics_json(),
            Self::Ca(v) if !v.simulate => r#"{"model":"static","tick":0}"#.into(),
            Self::Ca(v) => {
                let d = v.fluid.diagnostics();
                format!("{{\"model\":\"ca\",\"tick\":{},\"tick_seconds\":0.1,\"last_step_ms\":{},\"loaded_cells\":{},\"stored_cells\":{},\"initial_mass\":{},\"additions\":{},\"removals\":{},\"resident_mass\":{},\"dormant_mass\":{},\"displaced_mass\":{},\"conserved\":{},\"activation_error\":{:?}}}",d.tick,v.last_step_ms,d.loaded_cells,d.stored_cells,d.initial_mass,d.additions,d.removals,d.resident_mass,d.dormant_mass,d.displaced_mass,d.conserved(),v.activation_error.as_deref().unwrap_or(""))
            }
            Self::Discrete(v) => v.metrics.replace(
                "\"pending_snapshots\":0",
                &format!("\"pending_snapshots\":{}", usize::from(v.sim.is_none())),
            ),
        }
    }
    pub fn static_world(&self) -> Option<&World> {
        match self {
            Self::Moving(v) => v.static_world(),
            Self::Procedural(v) => v.static_world(),
            Self::Discrete(v) => v.static_world.as_ref(),
            Self::Ca(_) => None,
        }
    }
    pub fn dynamic_water(&self) -> Option<(Cell, [u32; 3], u64, &[u32])> {
        match self {
            Self::Moving(v) => v.dynamic_water(),
            Self::Procedural(v) => v.dynamic_water(),
            Self::Discrete(v) => v
                .dynamic
                .as_ref()
                .map(|(o, d, g, s)| (*o, *d, *g, s.as_slice())),
            Self::Ca(_) => None,
        }
    }
}
struct Completed {
    stats: StepStats,
    sim: DiscreteWater,
    epoch: u64,
    ms: f64,
    dt: f64,
    deferred: f64,
}
pub struct DiscreteWaterLive {
    sim: Option<DiscreteWater>,
    tx: SyncSender<(DiscreteWater, u64, f64)>,
    rx: Receiver<Completed>,
    inventory: Inventory,
    path: Option<PathBuf>,
    center: Option<Cell>,
    wet: BTreeSet<Cell>,
    static_world: Option<World>,
    debt: f64,
    clock: Instant,
    epoch: u64,
    step_ms: f64,
    step_stats: StepStats,
    generation: u64,
    simtime: f64,
    wall_elapsed: f64,
    started: bool,
    metrics: String,
    dynamic: Option<(Cell, [u32; 3], u64, Vec<u32>)>,
}
fn solid(w: &World, c: Cell) -> bool {
    !matches!(w.material_without_fluid(c), Material::Air | Material::Water)
}
impl DiscreteWaterLive {
    fn new(w: &mut World, path: Option<PathBuf>) -> Result<Self> {
        let (tx, commands) = mpsc::sync_channel::<(DiscreteWater, u64, f64)>(1);
        let (results, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("whole-voxel-water".into())
            .spawn(move || {
                while let Ok((mut sim, epoch, dt)) = commands.recv() {
                    let start = Instant::now();
                    let stats = sim.step(dt);
                    if results
                        .send(Completed {
                            sim,
                            epoch,
                            ms: start.elapsed().as_secs_f64() * 1000.,
                            dt: stats.simulated_dt,
                            deferred: stats.deferred_dt,
                            stats,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        let mut v = Self {
            sim: None,
            tx,
            rx,
            inventory: Inventory::default(),
            path,
            center: None,
            wet: BTreeSet::new(),
            static_world: None,
            debt: 0.,
            clock: Instant::now(),
            epoch: 0,
            step_ms: 0.,
            step_stats: StepStats::default(),
            generation: 0,
            simtime: 0.,
            wall_elapsed: 0.,
            started: false,
            metrics: "{}".into(),
            dynamic: None,
        };
        if let Some(p) = &v.path {
            if p.exists() {
                let b = std::fs::read(p)?;
                v.restore(w, &b)?;
            }
        }
        Ok(v)
    }
    fn publish(&mut self, w: &mut World) {
        let Some(sim) = &self.sim else { return };
        let publish_start = Instant::now();
        let wet: BTreeSet<_> = sim.occupied_cells().collect();
        w.set_fluid_occupancy(
            self.wet
                .union(&wet)
                .map(|&c| (c, wet.contains(&c)))
                .collect::<Vec<_>>(),
        );
        self.wet = wet;
        self.generation += 1;
        let origin = sim.origin();
        let d = sim.dimensions();
        let mut states = vec![1; d.iter().product()];
        for c in &self.wet {
            let i = (c.x - origin.x) as usize
                + d[0] * ((c.y - origin.y) as usize + d[1] * (c.z - origin.z) as usize);
            states[i] = 2;
        }
        self.dynamic = Some((origin, d.map(|v| v as u32), self.generation, states));
        let l = sim.ledger();
        let additions = l.added + l.source_added;
        let removals = l.removed + l.outlet_removed;
        let conserved = additions.checked_sub(removals) == Some(sim.mass() as u64);
        self.metrics=format!("{{\"model\":\"discrete\",\"bounded_demo\":true,\"tick\":{},\"simtime\":{},\"wall_elapsed\":{},\"wall_lag\":{},\"additions\":{},\"removals\":{},\"resident_mass\":{},\"source_added\":{},\"source_suppressed\":{},\"outlet_removed\":{},\"conserved\":{},\"pending_snapshots\":0,\"last_step_ms\":{},\"edit_epoch\":{}}}",sim.tick(),self.simtime,self.wall_elapsed,(self.wall_elapsed-self.simtime).max(0.),additions,removals,sim.mass(),l.source_added,l.source_suppressed,l.outlet_removed,conserved,self.step_ms,self.epoch);
        let publish_ms = publish_start.elapsed().as_secs_f64() * 1000.;
        // Solver diagnostics are observational and need not persist in saves.
        let d = &self.step_stats;
        self.metrics.pop();
        self.metrics.push_str(&format!(",\"substeps\":{},\"max_speed_mps\":{},\"pressure_sweeps\":{},\"pressure_residual_rms\":{},\"divergence_rms\":{},\"projection_energy_before\":{},\"projection_energy_after\":{},\"projection_reductions\":{},\"deferred_seconds\":{},\"publish_ms\":{}}}",d.substeps,d.max_speed_mps,d.pressure_sweeps,d.pressure_residual_rms,d.divergence_rms,d.projection_energy_before,d.projection_energy_after,d.projection_reductions,self.debt,publish_ms));
    }
    fn accept(&mut self, w: &mut World, c: Completed) -> Result<()> {
        // Edits are admitted only with local ownership, so obsolete epochs are
        // an invariant violation rather than permission to discard source mass.
        if c.epoch != self.epoch {
            return Err("discrete water epoch mismatch".into());
        }
        self.sim = Some(c.sim);
        self.step_ms = c.ms;
        self.step_stats = c.stats;
        self.simtime += c.dt;
        self.debt += c.deferred;
        self.publish(w);
        Ok(())
    }
    fn poll(&mut self, w: &mut World) -> Result<()> {
        match self.rx.try_recv() {
            Ok(c) => self.accept(w, c)?,
            Err(mpsc::TryRecvError::Disconnected) if self.sim.is_none() => {
                return Err("Discrete water worker disconnected".into())
            }
            _ => {}
        }
        Ok(())
    }
    fn update(&mut self, w: &mut World) -> Result<()> {
        self.poll(w)?;
        let elapsed = self.clock.elapsed().as_secs_f64();
        if elapsed >= 0.1 {
            if let Some(sim) = self.sim.take() {
                if sim.tick() == 0 {
                    eprintln!("discrete water simulation started");
                }
                self.started = true;
                self.wall_elapsed += elapsed;
                // Preserve elapsed time rather than dropping every frame's
                // scheduling overshoot. Worker and solver budgets remain bounded.
                let available = elapsed + self.debt;
                let dt = available.min(0.25);
                self.debt = available - dt;
                self.tx.send((sim, self.epoch, dt))?;
                self.clock = Instant::now();
            }
        }
        Ok(())
    }
    fn edit(&mut self, w: &mut World, c: Cell, m: Material) -> Result<()> {
        self.poll(w)?;
        let s = self.sim.as_mut().ok_or("Water step pending; retry edit")?;
        if s.contains(c) {
            s.set_solid(c, m.is_blocking())?;
        }
        w.set(c, m);
        self.static_world = Some(w.without_fluid_snapshot());
        self.epoch += 1;
        self.publish(w);
        Ok(())
    }
    fn apply_player_transaction(
        &mut self,
        w: &mut World,
        t: &EditTransaction,
        owns: impl Fn(Cell) -> bool,
    ) -> std::result::Result<(), &'static str> {
        self.poll(w).map_err(|_| "Water worker unavailable")?;
        let s = self.sim.as_mut().ok_or("Water step pending; retry edit")?;
        if t.place.is_some()
            && t.expected.iter().any(|&(c, _)| {
                s.water_at(c)
                    || s.is_source(c)
                    || s.is_outlet(c)
                    || w.material(c) == Material::Water
            })
        {
            return Err("Placement into water needs supported displacement");
        }
        let changed = t.apply(w, &mut self.inventory, owns)?;
        for c in changed {
            if s.contains(c) {
                s.set_solid(c, solid(w, c))
                    .expect("validated dry bounded edit");
            }
        }
        self.static_world = Some(w.without_fluid_snapshot());
        self.epoch += 1;
        Ok(())
    }
    fn demo(&mut self, w: &mut World, camera: &mut Camera) -> Result<()> {
        let ground =
            (w.ground_height_m(camera.position.x, camera.position.z) / voxel_size()).ceil() as i64;
        let o = Cell {
            x: (camera.position.x / voxel_size()).floor() as i64 - 24,
            y: ground + 2,
            z: (camera.position.z / voxel_size()).floor() as i64 - 24,
        };
        let mut s = DiscreteWater::new(o, [48, 40, 48])?;
        let mut edits = Vec::new();
        for x in 0..48i64 {
            for z in 0..48i64 {
                for y in 0..40i64 {
                    let c = Cell {
                        x: o.x + x,
                        y: o.y + y,
                        z: o.z + z,
                    };
                    // Narrow descending chute, with side/back walls directing
                    // the 4-cell-wide stream toward the open downstream lip.
                    let deck = (19..29).contains(&x)
                        && z < 16
                        && y == if z < 8 {
                            24
                        } else if z < 12 {
                            23
                        } else {
                            22
                        };
                    let chute_sides =
                        (x == 21 || x == 26) && (1..16).contains(&z) && (23..30).contains(&y);
                    let chute_back = z == 1 && (21..=26).contains(&x) && (25..30).contains(&y);
                    let support = (20..28).contains(&x) && z < 2 && y < 24;
                    let wall = y == 0
                        || ((x == 0 || x == 47 || z == 0 || z == 47) && y < 9)
                        || deck
                        || chute_sides
                        || chute_back
                        || support;
                    s.set_solid(c, wall)?;
                    edits.push((c, if wall { Material::Stone } else { Material::Air }));
                }
            }
        }
        for x in 22..26 {
            for z in 3..7 {
                s.set_source(
                    Cell {
                        x: o.x + x,
                        y: o.y + 25,
                        z: o.z + z,
                    },
                    true,
                )?;
            }
        }
        for x in 18..30 {
            for z in 44..47 {
                s.set_outlet(
                    Cell {
                        x: o.x + x,
                        y: o.y + 4,
                        z: o.z + z,
                    },
                    true,
                )?;
            }
        }
        // A four-cell-deep pool retains its lower layers below the spillway.
        for x in 1..47 {
            for z in 1..47 {
                for y in 1..=4 {
                    let c = Cell {
                        x: o.x + x,
                        y: o.y + y,
                        z: o.z + z,
                    };
                    if !s.solid_at(c) {
                        s.set_water(c, true)?;
                    }
                }
            }
        }
        // Counted starting water makes the directed chute visible immediately.
        for x in 22..26 {
            for z in 2..16 {
                let floor = if z < 8 {
                    24
                } else if z < 12 {
                    23
                } else {
                    22
                };
                for y in floor + 1..=floor + 2 {
                    let c = Cell {
                        x: o.x + x,
                        y: o.y + y,
                        z: o.z + z,
                    };
                    if !s.solid_at(c) {
                        s.set_water(c, true)?;
                    }
                }
            }
        }
        w.set_batch(edits);
        self.static_world = Some(w.without_fluid_snapshot());
        self.center = Some(Cell {
            x: o.x + 24,
            y: o.y,
            z: o.z + 24,
        });
        self.sim = Some(s);
        self.publish(w);
        let c = self.center.unwrap();
        camera.position = glam::DVec3::new(
            c.x as f64 * voxel_size() + 0.9,
            c.y as f64 * voxel_size() + 1.9,
            c.z as f64 * voxel_size() + 2.2,
        );
        camera.yaw = -2.84;
        camera.pitch = -0.42;
        eprintln!("Discrete whole-voxel supplied waterfall/basin/outlet demo: bounded 3 x 2.5 x 3 m domain; dig walls and place collected terrain; simulation worker at 10 Hz");
        Ok(())
    }
    fn identity(w: &World) -> Vec<u64> {
        let t = w.fbm_terrain().expect("FBM water");
        let p = t.params();
        vec![
            w.seed(),
            p.recipe as u64,
            t.source_version() as u64,
            p.horizontal_scale.to_bits(),
            p.amplitude.to_bits(),
            p.octaves as u64,
            p.roughness.to_bits(),
            w.vegetation_enabled() as u64,
        ]
    }
    fn checkpoint(&self, w: &World) -> Result<Vec<u8>> {
        let s = self
            .sim
            .as_ref()
            .ok_or("Cannot save in-flight simulation")?;
        let mut b = crate::grid_checkpoint::header(b"FVDWATR1");
        for v in Self::identity(w) {
            b.extend(v.to_le_bytes());
        }
        let edits: Vec<_> = w.terrain_edits().collect();
        b.extend((edits.len() as u64).to_le_bytes());
        for (c, m) in edits {
            for v in [c.x, c.y, c.z] {
                b.extend(v.to_le_bytes());
            }
            b.push(m as u8);
        }
        let c = self.center.ok_or("Missing discrete demo center")?;
        for v in [c.x, c.y, c.z] {
            b.extend(v.to_le_bytes());
        }
        b.extend(self.inventory.encode());
        b.extend(self.simtime.to_bits().to_le_bytes());
        b.extend(self.debt.to_bits().to_le_bytes());
        b.extend(self.wall_elapsed.to_bits().to_le_bytes());
        b.extend(s.snapshot());
        Ok(b)
    }
    fn restore(&mut self, w: &mut World, b: &[u8]) -> Result<()> {
        let b = crate::grid_checkpoint::payload(b)?;
        if b.get(..8) != Some(b"FVDWATR1") {
            return Err("Expected discrete water save; CA water saves remain separate".into());
        }
        fn read(b: &[u8], p: &mut usize) -> Result<u64> {
            let v = u64::from_le_bytes(
                b.get(*p..*p + 8)
                    .ok_or("Truncated discrete save")?
                    .try_into()?,
            );
            *p += 8;
            Ok(v)
        }
        let mut p = 8;
        for expected in Self::identity(w) {
            if read(b, &mut p)? != expected {
                return Err("Discrete save world source mismatch".into());
            }
        }
        let n = read(b, &mut p)?;
        if n > 1_048_576 {
            return Err("Discrete terrain edit limit exceeded".into());
        }
        let mut edits = Vec::new();
        for _ in 0..n {
            let c = Cell {
                x: read(b, &mut p)? as i64,
                y: read(b, &mut p)? as i64,
                z: read(b, &mut p)? as i64,
            };
            let m = *MATERIALS
                .get(*b.get(p).ok_or("Truncated material")? as usize)
                .ok_or("Invalid material")?;
            p += 1;
            edits.push((c, m));
        }
        let center = Cell {
            x: read(b, &mut p)? as i64,
            y: read(b, &mut p)? as i64,
            z: read(b, &mut p)? as i64,
        };
        let inventory = Inventory::decode(b.get(p..p + 104).ok_or("Truncated inventory")?)
            .ok_or("Invalid inventory")?;
        p += 104;
        let simtime = f64::from_bits(read(b, &mut p)?);
        if !simtime.is_finite() || simtime < 0. {
            return Err("Invalid simulated time".into());
        }
        let debt = f64::from_bits(read(b, &mut p)?);
        if !debt.is_finite() || debt < 0. {
            return Err("Invalid deferred time".into());
        }
        self.debt = debt;
        let wall_elapsed = f64::from_bits(read(b, &mut p)?);
        if !wall_elapsed.is_finite() || wall_elapsed < 0. {
            return Err("Invalid wall elapsed time".into());
        }
        self.wall_elapsed = wall_elapsed;
        self.started = wall_elapsed > 0. || simtime > 0.;
        let sim = DiscreteWater::restore(&b[p..])?;
        w.restore_terrain_edits(edits);
        let o = sim.origin();
        let d = sim.dimensions();
        for z in 0..d[2] {
            for y in 0..d[1] {
                for x in 0..d[0] {
                    let c = Cell {
                        x: o.x + x as i64,
                        y: o.y + y as i64,
                        z: o.z + z as i64,
                    };
                    if sim.solid_at(c) != solid(w, c) {
                        return Err("Saved water solids disagree with canonical terrain".into());
                    }
                }
            }
        }
        self.simtime = simtime;
        self.static_world = Some(w.without_fluid_snapshot());
        self.inventory = inventory;
        self.center = Some(center);
        self.sim = Some(sim);
        self.publish(w);
        Ok(())
    }
    fn save(&mut self, w: &mut World) -> Result<()> {
        if self.sim.is_none() {
            let done = self.rx.recv()?;
            self.accept(w, done)?;
        }
        if self.path.is_none() {
            return Ok(());
        }
        let b = self.checkpoint(w)?;
        let p = self.path.as_ref().unwrap();
        if let Some(parent) = p.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = p.with_extension("discrete-save.tmp");
        let mut file = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut file, &b)?;
        file.sync_all()?;
        std::fs::rename(tmp, p)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tiny() -> (World, DiscreteWaterLive, Cell) {
        let mut w = World::fbm(19, Default::default()).unwrap();
        let c = Cell {
            x: 0,
            y: 16000,
            z: 0,
        };
        for z in 0..4 {
            for y in 0..4 {
                for x in 0..4 {
                    w.set(Cell { x, y: c.y + y, z }, Material::Air);
                }
            }
        }
        let mut v = DiscreteWaterLive::new(&mut w, None).unwrap();
        v.sim = Some(DiscreteWater::new(c, [4, 4, 4]).unwrap());
        v.center = Some(c);
        v.static_world = Some(w.without_fluid_snapshot());
        v.publish(&mut w);
        (w, v, c)
    }
    #[test]
    fn checkpoint_restores_inventory_source_and_next_simulation_exactly() {
        let (mut w, mut v, c) = tiny();
        v.edit(&mut w, c, Material::Stone).unwrap();
        let transaction = EditTransaction::pickup(&v.inventory, vec![(c, Material::Stone)]);
        v.apply_player_transaction(&mut w, &transaction, |_| true)
            .unwrap();
        let source = Cell { y: c.y + 3, ..c };
        v.sim.as_mut().unwrap().set_source(source, true).unwrap();
        v.sim.as_mut().unwrap().step(0.1);
        v.simtime = 0.1;
        v.debt = 0.007;
        v.publish(&mut w);
        let saved = v.checkpoint(&w).unwrap();
        let mut other = World::fbm(19, Default::default()).unwrap();
        let mut restored = DiscreteWaterLive::new(&mut other, None).unwrap();
        restored.restore(&mut other, &saved).unwrap();
        assert_eq!(v.inventory, restored.inventory);
        assert_eq!(v.wet, restored.wet);
        assert_eq!(v.simtime, restored.simtime);
        assert_eq!(v.debt, restored.debt);
        v.sim.as_mut().unwrap().step(0.1);
        restored.sim.as_mut().unwrap().step(0.1);
        assert_eq!(
            v.sim.as_ref().unwrap().snapshot(),
            restored.sim.as_ref().unwrap().snapshot()
        );
        assert_eq!(other.material_without_fluid(c), Material::Air);
    }
    #[test]
    fn worker_receives_accepted_solid_edits_and_inflight_edits_do_not_mutate() {
        let (mut w, mut v, c) = tiny();
        v.edit(&mut w, c, Material::Stone).unwrap();
        let pickup = EditTransaction::pickup(&v.inventory, vec![(c, Material::Stone)]);
        let held = v.sim.take().unwrap();
        let before = w.revision();
        assert_eq!(
            v.apply_player_transaction(&mut w, &pickup, |_| true),
            Err("Water step pending; retry edit")
        );
        assert_eq!(before, w.revision());
        assert_eq!(v.inventory.total(), 0);
        v.sim = Some(held);
        v.apply_player_transaction(&mut w, &pickup, |_| true)
            .unwrap();
        v.tx.send((v.sim.take().unwrap(), v.epoch, 0.1)).unwrap();
        let done = v.rx.recv().unwrap();
        assert!(!done.sim.solid_at(c));
        v.accept(&mut w, done).unwrap();
        assert_eq!(v.inventory.total(), 1);
        assert_eq!(w.material_without_fluid(c), Material::Air);
    }
    #[test]
    fn quick_hold_place_and_wet_rejection_preserve_inventory_and_mass() {
        let (mut w, mut v, c) = tiny();
        let hand = frontier_world::edit::HandSettings::default();
        assert_eq!(hand.count_at(0.06), 1);
        assert_eq!(hand.count_at(1.1), 2);
        let cells = [c, Cell { x: c.x + 1, ..c }, Cell { x: c.x + 2, ..c }];
        for cell in cells {
            v.edit(&mut w, cell, Material::Stone).unwrap();
        }
        let quick = EditTransaction::pickup(&v.inventory, vec![(cells[0], Material::Stone)]);
        v.apply_player_transaction(&mut w, &quick, |_| true)
            .unwrap();
        assert_eq!(v.inventory.total(), 1);
        let held = EditTransaction::pickup(
            &v.inventory,
            cells[1..].iter().map(|&c| (c, Material::Stone)).collect(),
        );
        v.apply_player_transaction(&mut w, &held, |_| true).unwrap();
        assert_eq!(v.inventory.total(), 3);
        let place = EditTransaction::placement(&v.inventory, cells[0], Material::Air);
        v.apply_player_transaction(&mut w, &place, |_| true)
            .unwrap();
        assert_eq!(v.inventory.total(), 2);
        assert_eq!(v.inventory.sequence, 3);
        assert!(v.sim.as_ref().unwrap().solid_at(cells[0]));
        assert_eq!(v.sim.as_ref().unwrap().mass(), 0);
        let wet = Cell {
            x: c.x + 3,
            y: c.y + 2,
            z: c.z + 3,
        };
        v.sim.as_mut().unwrap().set_water(wet, true).unwrap();
        v.publish(&mut w);
        let before = v.sim.as_ref().unwrap().snapshot();
        let inventory = v.inventory.clone();
        let revision = w.revision();
        let blocked = EditTransaction::placement(&v.inventory, wet, Material::Water);
        assert_eq!(
            v.apply_player_transaction(&mut w, &blocked, |_| true),
            Err("Placement into water needs supported displacement")
        );
        assert_eq!(v.inventory, inventory);
        assert_eq!(w.revision(), revision);
        assert_eq!(v.sim.as_ref().unwrap().snapshot(), before);
        assert_eq!(v.sim.as_ref().unwrap().mass(), 1);
    }
    #[test]
    fn disk_save_finishes_pending_worker_and_reload_is_identical() {
        let (mut w, mut v, c) = tiny();
        let source = Cell {
            x: c.x + 1,
            y: c.y + 3,
            z: c.z + 1,
        };
        v.sim.as_mut().unwrap().set_source(source, true).unwrap();
        let mut expected = DiscreteWater::restore(&v.sim.as_ref().unwrap().snapshot()).unwrap();
        expected.step(0.1);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "frontier-water-edit-{}-{unique}",
            std::process::id()
        ));
        let path = dir.join("world.save");
        v.path = Some(path.clone());
        v.tx.send((v.sim.take().unwrap(), v.epoch, 0.1)).unwrap();
        assert!(v.sim.is_none());
        v.save(&mut w).unwrap();
        assert_eq!(v.sim.as_ref().unwrap().snapshot(), expected.snapshot());
        assert!(!path.with_extension("discrete-save.tmp").exists());
        let saved = std::fs::read(&path).unwrap();
        let mut other = World::fbm(19, Default::default()).unwrap();
        let restored = DiscreteWaterLive::new(&mut other, Some(path)).unwrap();
        assert_eq!(restored.checkpoint(&other).unwrap(), saved);
        assert_eq!(
            v.sim.as_ref().unwrap().snapshot(),
            restored.sim.as_ref().unwrap().snapshot()
        );
        assert_eq!(v.inventory, restored.inventory);
        assert_eq!(v.wet, restored.wet);
        assert_eq!(
            w.terrain_edits().collect::<Vec<_>>(),
            other.terrain_edits().collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
