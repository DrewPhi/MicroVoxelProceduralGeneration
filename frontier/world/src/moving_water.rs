//! Connected camera-window ownership controller. Dormant native tiles keep exact
//! binary/numerical state; their regional volumes are indexes, never refill rules.
use crate::{
    discrete_water::{
        coupled::CoupledWater,
        dormant_relax::{self, RelaxEvent, RelaxOutcome, RelaxPending, RelaxPlan},
        environmental_flow::{canonical_key, EnvironmentalFlow, FlowConfig, FlowCounters},
        BoundaryGhost, DiscreteWater, NativeBlock, NativeCellState, NativeFaceState, OpenBoundary,
        StepStats,
    },
    procedural_water::{ProceduralWater, ReachKey, TileKey},
    voxel_size, Cell, World,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

#[derive(Clone)]
pub struct MovingWater {
    coupled: CoupledWater,
    registry: ProceduralWater,
    blocks: BTreeMap<TileKey, Arc<NativeBlock>>,
    origin: TileKey,
    migrations: u64,
    pending: VecDeque<(RelaxEvent, RelaxPending)>,
    pending_set: BTreeSet<RelaxEvent>,
    relax_queries: usize,
    relax_moves: usize,
    environment: EnvironmentalFlow,
    environment_config: FlowConfig,
    environment_subns: f64,
    environment_cache: Option<Arc<EnvironmentCache>>,
}
#[derive(Clone)]
struct EnvironmentCandidate {
    cell: Cell,
    pressure: f32,
}
#[derive(Clone, Default)]
struct ChannelContinuation {
    upstream: Vec<EnvironmentCandidate>,
    downstream: Vec<EnvironmentCandidate>,
}
#[derive(Clone, Default)]
struct EnvironmentCache {
    channels: BTreeMap<ReachKey, ChannelContinuation>,
}
fn checked_window_origin(origin: TileKey) -> Result<Cell, String> {
    let o = origin.origin()?;
    for v in [o.x, o.y, o.z] {
        v.checked_sub(17).ok_or("water halo coordinate underflow")?;
        v.checked_add(49).ok_or("water halo coordinate overflow")?;
    }
    Ok(o)
}
fn active_keys(origin: TileKey) -> Vec<TileKey> {
    let mut out = Vec::new();
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                out.push(TileKey([origin.0[0] + x, origin.0[1] + y, origin.0[2] + z]));
            }
        }
    }
    out
}
fn next_cell(c: Cell, axis: usize, positive: bool) -> Cell {
    let mut p = [c.x, c.y, c.z];
    p[axis] += if positive { 1 } else { -1 };
    Cell {
        x: p[0],
        y: p[1],
        z: p[2],
    }
}
fn block_mass(block: &NativeBlock) -> u64 {
    block.cells.iter().filter(|c| c.water).count() as u64
}
impl MovingWater {
    /// A 1m hysteresis band inside the 2m connected simulation window.
    pub fn window_for(feet: Cell) -> Result<TileKey, String> {
        let mut a = [0; 3];
        for (i, v) in [feet.x, feet.y, feet.z].into_iter().enumerate() {
            a[i] = v
                .checked_sub(8)
                .ok_or("water window coordinate overflow")?
                .div_euclid(16);
        }
        let key = TileKey(a);
        checked_window_origin(key)?;
        Ok(key)
    }
    pub fn new(world: &World, origin: TileKey, max_tiles: usize) -> Result<Self, String> {
        checked_window_origin(origin)?;
        let mut registry = ProceduralWater::new(world, max_tiles)?;
        registry.regional_mut().add_reservoir(0, 32768, 0, 0)?;
        let mut blocks = BTreeMap::new();
        for key in active_keys(origin) {
            Self::materialize(world, &mut registry, &mut blocks, key)?;
        }
        let incoming: Vec<_> = active_keys(origin)
            .iter()
            .map(|k| blocks[k].as_ref().clone())
            .collect();
        let sim = DiscreteWater::from_native_blocks(origin.origin()?, [32; 3], &incoming, 0)?;
        let mut token = registry.regional_mut().take_region(1, 0, 0)?;
        let mut serial = 1;
        for key in active_keys(origin) {
            let id = registry.tile(key).unwrap().reservoir;
            let units = block_mass(&blocks[&key]);
            token = registry
                .regional_mut()
                .exchange_owned(serial, token, id, units, true)?;
            serial += 1;
        }
        let coupled = CoupledWater::new(sim, registry.regional().clone(), token, serial)?;
        let mut state = Self {
            coupled,
            registry,
            blocks,
            origin,
            migrations: 0,
            pending: VecDeque::new(),
            pending_set: BTreeSet::new(),
            relax_queries: 0,
            relax_moves: 0,
            environment: EnvironmentalFlow::new(4096)?,
            environment_config: FlowConfig::default(),
            environment_subns: 0.,
            environment_cache: None,
        };
        state.configure_ports(world, true)?;
        state.validate()?;
        Ok(state)
    }
    fn materialize(
        world: &World,
        registry: &mut ProceduralWater,
        blocks: &mut BTreeMap<TileKey, Arc<NativeBlock>>,
        key: TileKey,
    ) -> Result<(), String> {
        if blocks.contains_key(&key) {
            return Ok(());
        }
        let tile = registry.register(world, key)?.clone();
        if tile.native {
            return Err("native tile lost its exact dormant block".into());
        }
        let units = registry
            .regional()
            .reservoir(tile.reservoir)
            .ok_or("missing pristine reservoir")?
            .volume;
        let wet: BTreeSet<_> = tile.geometry.occupied_cells(units)?.into_iter().collect();
        let o = key.origin()?;
        let head = if units == 0 {
            tile.geometry.stage_volume.local_head_m(0)?
        } else {
            tile.geometry.initial_head_m()?
        };
        let first_dry = (head / voxel_size() - 0.5).ceil();
        let mut cells = Vec::with_capacity(4096);
        for z in 0..16 {
            for y in 0..16 {
                for x in 0..16 {
                    let c = Cell {
                        x: o.x + x,
                        y: o.y + y,
                        z: o.z + z,
                    };
                    let water = wet.contains(&c);
                    cells.push(NativeCellState {
                        water,
                        solid: tile.geometry.solid_at(c).ok_or("tile geometry bounds")?,
                        pressure: if water {
                            (9.81 / voxel_size() * (first_dry - c.y as f64).max(0.)) as f32
                        } else {
                            0.
                        },
                        ..Default::default()
                    });
                }
            }
        }
        let block = NativeBlock {
            origin: o,
            dimensions: [16; 3],
            tick: 0,
            cells,
            boundaries: Vec::new(),
        };
        block.validate()?;
        registry.mark_native(key)?;
        blocks.insert(key, Arc::new(block));
        Ok(())
    }
    pub fn origin(&self) -> TileKey {
        self.origin
    }
    pub fn simulation(&self) -> &DiscreteWater {
        self.coupled.simulation()
    }
    pub fn regional(&self) -> &crate::discrete_water::regional::RegionalWater {
        self.coupled.regional()
    }
    pub fn migrations(&self) -> u64 {
        self.migrations
    }
    pub fn pending_relaxations(&self) -> usize {
        self.pending.len()
    }
    pub fn relax_queries(&self) -> usize {
        self.relax_queries
    }
    pub fn relax_moves(&self) -> usize {
        self.relax_moves
    }
    pub fn tile_count(&self) -> usize {
        self.blocks.len()
    }
    pub fn active_bounds(&self) -> (Cell, Cell) {
        let a = self.origin.origin().expect("validated water origin");
        (
            a,
            Cell {
                x: a.x + 32,
                y: a.y + 32,
                z: a.z + 32,
            },
        )
    }
    pub fn overlay_bounds(&self) -> (Cell, Cell) {
        let (a, b) = self.active_bounds();
        (
            Cell {
                x: a.x - 16,
                y: a.y - 16,
                z: a.z - 16,
            },
            Cell {
                x: b.x + 16,
                y: b.y + 16,
                z: b.z + 16,
            },
        )
    }
    fn is_active_key(&self, key: TileKey) -> bool {
        (0..3).all(|a| key.0[a] >= self.origin.0[a] && key.0[a] < self.origin.0[a] + 2)
    }
    fn inactive_cell(&self, c: Cell) -> Result<NativeCellState, String> {
        if self.simulation().contains(c) {
            return Err("ghost cell is inside connected active window".into());
        }
        self.blocks
            .get(&TileKey::containing(c))
            .and_then(|b| b.cell(c))
            .copied()
            .ok_or("exact ghost tile not materialized".into())
    }
    fn configure_ports(&mut self, world: &World, rebind: bool) -> Result<(), String> {
        // A connected one-tile dormant ring surrounds the detailed cube.
        // Edge/corner blocks let a vacancy reach a real surface around ACTIVE
        // cells, rather than stopping at an unknown diagonal gap.
        for z in -1..=2 {
            for y in -1..=2 {
                for x in -1..=2 {
                    Self::materialize(
                        world,
                        &mut self.registry,
                        &mut self.blocks,
                        TileKey([
                            self.origin.0[0] + x,
                            self.origin.0[1] + y,
                            self.origin.0[2] + z,
                        ]),
                    )?;
                }
            }
        }
        // New finite reservoirs may have been registered while preparing the
        // window; append-only graph metadata is synchronized before rebinding.
        if self.registry.regional().snapshot() != self.coupled.regional().snapshot() {
            let sim = self.coupled.simulation().clone();
            let token = self.coupled.ownership();
            let graph = self.registry.regional().clone();
            let serial = graph
                .last_transfer_id()
                .checked_add(1)
                .ok_or("transfer serial overflow")?;
            if sim.boundaries().next().is_some() {
                return Err("unbound ports required when adding halo reservoirs".into());
            }
            self.coupled = CoupledWater::new(sim, graph, token, serial)?;
        }
        let old: Vec<_> = self.simulation().boundaries().map(|(id, _)| id).collect();
        let mut keep = BTreeSet::new();
        let (o, _) = self.active_bounds();
        for z in 0..32 {
            for y in 0..32 {
                for x in 0..32 {
                    let c = Cell {
                        x: o.x + x,
                        y: o.y + y,
                        z: o.z + z,
                    };
                    if self.simulation().solid_at(c) {
                        continue;
                    }
                    for axis in 0..3 {
                        for positive in [false, true] {
                            if [x, y, z][axis] != if positive { 31 } else { 0 } {
                                continue;
                            }
                            let outside = next_cell(c, axis, positive);
                            let ghost = self.inactive_cell(outside)?;
                            if ghost.solid {
                                continue;
                            }
                            let tile = self
                                .registry
                                .tile(TileKey::containing(outside))
                                .ok_or("missing ghost registry")?;
                            let id = (x + 32 * (y + 32 * z)) as u64 * 6
                                + (axis * 2 + usize::from(positive)) as u64
                                + 1;
                            keep.insert(id);
                            let existed = self.simulation().boundary(id).is_some();
                            self.coupled.set_port(
                                id,
                                tile.reservoir,
                                OpenBoundary {
                                    cell: c,
                                    axis,
                                    positive,
                                    head_m: 0.,
                                    velocity_mps: ghost.momentum_mps,
                                    import_quota: 0,
                                    export_quota: 0,
                                },
                            )?;
                            if rebind || !existed {
                                let lower = if positive { c } else { outside };
                                let face = if self.simulation().contains(lower) {
                                    self.simulation()
                                        .positive_face_state(lower, axis)
                                        .ok_or("missing active interface face")?
                                } else {
                                    self.blocks[&TileKey::containing(lower)]
                                        .cell(lower)
                                        .ok_or("missing dormant interface face")?
                                        .positive_faces[axis]
                                };
                                self.coupled.set_port_face(id, face)?;
                            }
                            self.coupled.set_port_ghost(
                                id,
                                Some(BoundaryGhost {
                                    wet: ghost.water,
                                    pressure: ghost.pressure,
                                    velocity_mps: ghost.momentum_mps,
                                }),
                            )?;
                            self.coupled.set_port_limits(
                                id,
                                u64::from(ghost.water),
                                u64::from(!ghost.water),
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
    fn refresh_ghosts(&mut self) -> Result<(), String> {
        let mut queue_room = 65536usize.saturating_sub(self.pending.len());
        let ports: Vec<_> = self.simulation().boundaries().collect();
        for (id, p) in ports {
            let ghost = self.inactive_cell(next_cell(p.cell, p.axis, p.positive))?;
            if ghost.solid {
                return Err("stale port overlaps a dormant solid edit".into());
            }
            self.coupled.set_port_ghost(
                id,
                Some(BoundaryGhost {
                    wet: ghost.water,
                    pressure: ghost.pressure,
                    velocity_mps: ghost.momentum_mps,
                }),
            )?;
            let event = if ghost.water {
                RelaxEvent::ImportVacancy(next_cell(p.cell, p.axis, p.positive))
            } else {
                RelaxEvent::ExportFilled(next_cell(p.cell, p.axis, p.positive))
            };
            let allowed = if self.pending_set.contains(&event) {
                true
            } else if queue_room > 0 {
                queue_room -= 1;
                true
            } else {
                false
            };
            self.coupled.set_port_limits(
                id,
                u64::from(ghost.water && allowed),
                u64::from(!ghost.water && allowed),
            )?;
        }
        Ok(())
    }
    fn sync_external_faces(&mut self) -> Result<Vec<(Cell, usize, NativeFaceState)>, String> {
        let mut faces = Vec::new();
        for boundary in self.simulation().export_boundaries() {
            let (lower, axis, face) = boundary.positive_face()?;
            if !self.simulation().contains(lower) {
                let block = self
                    .blocks
                    .get_mut(&TileKey::containing(lower))
                    .ok_or("missing exterior face owner")?;
                Arc::make_mut(block)
                    .cell_mut(lower)
                    .ok_or("missing exterior face cell")?
                    .positive_faces[axis] = face;
            }
            faces.push((lower, axis, face));
        }
        Ok(faces)
    }
    fn cache_active(&mut self) -> Result<Vec<(Cell, usize, NativeFaceState)>, String> {
        let faces = self.sync_external_faces()?;
        for key in active_keys(self.origin) {
            let mut block = self.simulation().export_block(key.origin()?, [16; 3])?;
            block.boundaries.clear();
            for &(lower, axis, face) in &faces {
                if let Some(cell) = block.cell_mut(lower) {
                    cell.positive_faces[axis] = face;
                }
            }
            self.blocks.insert(key, Arc::new(block));
        }
        Ok(faces)
    }
    fn reconcile_external(&mut self, stats: &StepStats) -> Result<(), String> {
        let mut touched = BTreeSet::new();
        let mut events = Vec::new();
        for transfer in &stats.boundary_transfers {
            if transfer.units != 1 {
                return Err("nonbinary boundary report".into());
            }
            let port = self
                .simulation()
                .boundary(transfer.boundary_id)
                .ok_or("missing accepted boundary")?;
            let outside = next_cell(port.cell, port.axis, port.positive);
            let key = TileKey::containing(outside);
            let block = Arc::make_mut(
                self.blocks
                    .get_mut(&key)
                    .ok_or("accepted ghost has no exact block")?,
            );
            let old = *block.cell(outside).ok_or("outside cell bounds")?;
            if old.solid || old.water != transfer.into_detail {
                return Err("dormant transfer occupancy mismatch".into());
            }
            block.set_water_state(
                outside,
                !transfer.into_detail,
                if transfer.into_detail {
                    [0.; 3]
                } else {
                    transfer.momentum_mps
                },
            )?;
            touched.insert(key);
            events.push(if transfer.into_detail {
                RelaxEvent::ImportVacancy(outside)
            } else {
                RelaxEvent::ExportFilled(outside)
            });
        }
        for event in events {
            self.enqueue(event, RelaxPending::UnknownBoundary)?;
        }
        let ports: Vec<_> = self.simulation().boundaries().collect();
        for (id, port) in ports {
            let outside = next_cell(port.cell, port.axis, port.positive);
            let key = TileKey::containing(outside);
            if let Some(ghost) = self.simulation().boundary_ghost(id) {
                let cell = Arc::make_mut(self.blocks.get_mut(&key).ok_or("ghost block missing")?)
                    .cell_mut(outside)
                    .ok_or("ghost cell missing")?;
                if cell.water != ghost.wet {
                    return Err("accepted ghost state disagrees with exact cell".into());
                }
                cell.pressure = ghost.pressure;
                cell.momentum_mps = if cell.water {
                    ghost.velocity_mps
                } else {
                    [0.; 3]
                };
            }
        }
        self.sync_external_faces()?;
        for key in touched {
            let id = self.registry.tile(key).unwrap().reservoir;
            let volume = self.coupled.regional().reservoir(id).unwrap().volume;
            if volume != block_mass(&self.blocks[&key]) {
                return Err("dormant exact count differs from regional volume".into());
            }
        }
        *self.registry.regional_mut() = self.coupled.regional().clone();
        Ok(())
    }
    fn enqueue(&mut self, event: RelaxEvent, reason: RelaxPending) -> Result<(), String> {
        if self.pending_set.insert(event) {
            if self.pending.len() >= 65536 {
                return Err("dormant relaxation queue capacity exceeded".into());
            }
            self.pending.push_back((event, reason));
        }
        Ok(())
    }
    fn apply_relaxation(&mut self, plan: RelaxPlan) -> Result<(), String> {
        if plan.potential_delta_cells > 0 {
            return Err("dormant routing would raise gravitational potential".into());
        }
        let mut original = BTreeMap::new();
        for m in &plan.moves {
            for c in [m.from, m.to] {
                original.entry(c).or_insert(self.inactive_cell(c)?);
            }
        }
        let mut graph = self.coupled.regional().clone();
        let serial = graph
            .last_transfer_id()
            .checked_add(1)
            .ok_or("routing serial overflow")?;
        let mut transfers = Vec::new();
        let mut potential = 0i128;
        let mut touched = BTreeSet::new();
        for m in &plan.moves {
            let from = self.inactive_cell(m.from)?;
            let to = self.inactive_cell(m.to)?;
            let distance = (m.from.x as i128 - m.to.x as i128).abs()
                + (m.from.y as i128 - m.to.y as i128).abs()
                + (m.from.z as i128 - m.to.z as i128).abs();
            if !from.water
                || from.solid
                || to.water
                || to.solid
                || distance != 1
                || from.momentum_mps != m.momentum_mps
            {
                return Err("stale or nonlocal dormant relaxation move".into());
            }
            potential += m.to.y as i128 - m.from.y as i128;
            let a = TileKey::containing(m.from);
            let b = TileKey::containing(m.to);
            if a != b {
                transfers.push((
                    self.registry
                        .tile(a)
                        .ok_or("routing source missing")?
                        .reservoir,
                    self.registry
                        .tile(b)
                        .ok_or("routing target missing")?
                        .reservoir,
                    1,
                ));
            }
            Arc::make_mut(self.blocks.get_mut(&a).unwrap())
                .set_water_state(m.from, false, [0.; 3])?;
            let destination = Arc::make_mut(self.blocks.get_mut(&b).unwrap());
            destination.set_water_state(m.to, true, m.momentum_mps)?;
            let old = original[&m.to];
            destination.cell_mut(m.to).unwrap().pressure = if old.water {
                old.pressure
            } else {
                (9.81 / voxel_size() * ((plan.equilibrium_head_y - m.to.y as i128).max(0) as f64))
                    as f32
            };
            touched.insert(a);
            touched.insert(b);
        }
        if potential != plan.potential_delta_cells || potential > 0 {
            return Err("dormant routing potential accounting mismatch".into());
        }
        if !transfers.is_empty() {
            graph.transfer_batch(serial, &transfers)?;
        }
        for key in touched {
            let id = self.registry.tile(key).unwrap().reservoir;
            if graph.reservoir(id).unwrap().volume != block_mass(&self.blocks[&key]) {
                return Err("routed native/regional counts disagree".into());
            }
        }
        self.coupled.adopt_regional_after_transfers(graph)?;
        *self.registry.regional_mut() = self.coupled.regional().clone();
        self.relax_moves += plan.moves.len();
        Ok(())
    }
    fn process_relaxations(&mut self, budget: &mut usize) -> Result<(), String> {
        let attempts = self.pending.len();
        let (min, max) = self.overlay_bounds();
        for _ in 0..attempts {
            if *budget == 0 {
                break;
            }
            let (event, _) = self.pending.pop_front().ok_or("relax queue underflow")?;
            self.pending_set.remove(&event);
            if self.simulation().contains(event.cell()) {
                continue;
            } // Refined simulation now owns this work.
            let start = event.cell();
            if start.x < min.x
                || start.y < min.y
                || start.z < min.z
                || start.x >= max.x
                || start.y >= max.y
                || start.z >= max.z
            {
                self.enqueue(event, RelaxPending::UnknownBoundary)?;
                continue;
            }
            let outcome = dormant_relax::plan(
                event,
                |c| {
                    if self.simulation().contains(c)
                        || c.x < min.x
                        || c.y < min.y
                        || c.z < min.z
                        || c.x >= max.x
                        || c.y >= max.y
                        || c.z >= max.z
                    {
                        return None;
                    }
                    self.blocks
                        .get(&TileKey::containing(c))
                        .and_then(|b| b.cell(c))
                        .copied()
                },
                budget,
            );
            match outcome {
                RelaxOutcome::Ready(plan) => self.apply_relaxation(plan)?,
                RelaxOutcome::Pending(RelaxPending::StaleEvent) => {}
                RelaxOutcome::Pending(RelaxPending::InvalidState) => {
                    return Err("invalid dormant relaxation state".into())
                }
                RelaxOutcome::Pending(reason) => self.enqueue(event, reason)?,
            }
        }
        Ok(())
    }
    /// Every accepted exterior move changes its actual native cell in this same
    /// transaction. No frozen cell can be consumed twice in one call.
    pub fn step(&mut self, dt: f64) -> Result<StepStats, String> {
        let mut staged = self.clone();
        let stats = staged.step_inner(dt)?;
        *self = staged;
        Ok(stats)
    }
    fn step_inner(&mut self, dt: f64) -> Result<StepStats, String> {
        let mut budget = 8192;
        self.relax_moves = 0;
        self.process_relaxations(&mut budget)?;
        self.refresh_ghosts()?;
        let stats = self.coupled.step(dt)?;
        self.reconcile_external(&stats)?;
        self.process_relaxations(&mut budget)?;
        self.relax_queries = 8192 - budget;
        self.refresh_ghosts()?;
        if !self.regional().conserved()
            || self.coupled.ownership().units != self.simulation().mass() as u64
        {
            return Err("moving water conservation failed".into());
        }
        Ok(stats)
    }
    pub fn environmental_flow(&self) -> &EnvironmentalFlow {
        &self.environment
    }
    pub fn environmental_features(&self) -> usize {
        self.environment.features().count()
    }
    pub fn environmental_counters(&self) -> FlowCounters {
        self.environment
            .features()
            .fold(FlowCounters::default(), |mut total, (_, s)| {
                total.upstream_attempted = total
                    .upstream_attempted
                    .saturating_add(s.counters.upstream_attempted);
                total.upstream_added = total
                    .upstream_added
                    .saturating_add(s.counters.upstream_added);
                total.upstream_suppressed = total
                    .upstream_suppressed
                    .saturating_add(s.counters.upstream_suppressed);
                total.downstream_attempted = total
                    .downstream_attempted
                    .saturating_add(s.counters.downstream_attempted);
                total.downstream_removed = total
                    .downstream_removed
                    .saturating_add(s.counters.downstream_removed);
                total.downstream_unsatisfied = total
                    .downstream_unsatisfied
                    .saturating_add(s.counters.downstream_unsatisfied);
                total
            })
    }
    fn prepare_environment(&mut self, world: &World) -> Result<(), String> {
        if self.environment_cache.is_some() {
            return Ok(());
        }
        let (min, max) = self.overlay_bounds();
        let terrain = world
            .fbm_terrain()
            .ok_or("environmental flow requires generated rivers")?;
        let reaches = terrain.water_reaches(
            [min.x as f64 * voxel_size(), min.z as f64 * voxel_size()],
            [max.x as f64 * voxel_size(), max.z as f64 * voxel_size()],
        )?;
        let mut cache = EnvironmentCache::default();
        let mut columns = BTreeMap::new();
        for z in min.z..max.z {
            for y in min.y..max.y {
                for x in min.x..max.x {
                    let c = Cell { x, y, z };
                    let coordinates = [x, y, z];
                    let lower = [min.x, min.y, min.z];
                    let upper = [max.x, max.y, max.z];
                    if !(0..3).any(|a| coordinates[a] == lower[a] || coordinates[a] == upper[a] - 1)
                    {
                        continue;
                    }
                    if self.inactive_cell(c)?.solid {
                        continue;
                    }
                    let px = (x as f64 + 0.5) * voxel_size();
                    let pz = (z as f64 + 0.5) * voxel_size();
                    let mut best = None;
                    for r in &reaches {
                        if (y as f64 + 0.5) * voxel_size()
                            > r.upstream.stage_m.max(r.downstream.stage_m) + voxel_size()
                        {
                            continue;
                        }
                        let dx = r.downstream.x_m - r.upstream.x_m;
                        let dz = r.downstream.z_m - r.upstream.z_m;
                        let dy = r.downstream.stage_m - r.upstream.stage_m;
                        let length2 = dx * dx + dz * dz;
                        if length2 <= 0. {
                            continue;
                        }
                        let u = (((px - r.upstream.x_m) * dx + (pz - r.upstream.z_m) * dz)
                            / length2)
                            .clamp(0., 1.);
                        let radius =
                            r.upstream_radius_m + (r.downstream_radius_m - r.upstream_radius_m) * u;
                        if radius <= 0. {
                            continue;
                        }
                        let distance = (px - r.upstream.x_m - u * dx).powi(2)
                            + (pz - r.upstream.z_m - u * dz).powi(2);
                        let score = distance / (radius * radius);
                        if score > 1. {
                            continue;
                        }
                        let tangent = [dx, dy, dz];
                        let mut face = None;
                        for axis in 0..3 {
                            for positive in [false, true] {
                                if coordinates[axis]
                                    != if positive {
                                        upper[axis] - 1
                                    } else {
                                        lower[axis]
                                    }
                                {
                                    continue;
                                }
                                let outward = tangent[axis] * if positive { 1. } else { -1. };
                                if outward.abs() <= 1e-12 {
                                    continue;
                                }
                                if face.is_none_or(|(old, _, _, _): (f64, usize, bool, bool)| {
                                    outward.abs() > old
                                }) {
                                    face = Some((outward.abs(), axis, positive, outward < 0.));
                                }
                            }
                        }
                        let Some((_, axis, positive, upstream)) = face else {
                            continue;
                        };
                        if best.as_ref().is_none_or(
                            |(old, key, _, _, _, _, _): &(
                                f64,
                                ReachKey,
                                usize,
                                bool,
                                bool,
                                f64,
                                bool,
                            )| {
                                score < *old || (score == *old && r.key < *key)
                            },
                        ) {
                            best = Some((
                                score,
                                r.key,
                                axis,
                                positive,
                                upstream,
                                r.upstream.stage_m + u * dy,
                                r.lake,
                            ));
                        }
                    }
                    let Some((_, key, axis, positive, upstream, head, lake)) = best else {
                        continue;
                    };
                    if lake {
                        continue;
                    }
                    let canonical = columns
                        .entry((x, z))
                        .or_insert_with(|| world.column(x, z))
                        .material_without_fluid(y);
                    if canonical != crate::Material::Water {
                        continue;
                    }
                    let outside = next_cell(c, axis, positive);
                    if columns
                        .entry((outside.x, outside.z))
                        .or_insert_with(|| world.column(outside.x, outside.z))
                        .material_without_fluid(outside.y)
                        != crate::Material::Water
                    {
                        continue;
                    }
                    let candidate = EnvironmentCandidate {
                        cell: c,
                        pressure: (9.81 / voxel_size()
                            * ((head / voxel_size() - 0.5).ceil() - y as f64).max(0.))
                            as f32,
                    };
                    let channel = cache.channels.entry(canonical_key(key)).or_default();
                    if upstream {
                        channel.upstream.push(candidate);
                    } else {
                        channel.downstream.push(candidate);
                    }
                }
            }
        }
        for channel in cache.channels.values_mut() {
            channel.upstream.sort_unstable_by_key(|c| c.cell);
            channel.downstream.sort_unstable_by_key(|c| c.cell);
        }
        self.environment_cache = Some(Arc::new(cache));
        Ok(())
    }
    fn apply_environment(
        &mut self,
        world: &World,
        simulated_dt: f64,
        config: FlowConfig,
    ) -> Result<(), String> {
        self.environment_config = config;
        if simulated_dt <= 0.
            || (config.upstream_units_per_second == 0 && config.downstream_units_per_second == 0)
        {
            return Ok(());
        }
        self.prepare_environment(world)?;
        let cache = self.environment_cache.as_ref().unwrap().clone();
        let active: Vec<_> = cache.channels.keys().copied().collect();
        for &key in &active {
            let channel = &cache.channels[&key];
            self.environment.configure(
                key,
                false,
                FlowConfig {
                    upstream_units_per_second: if channel.upstream.is_empty() {
                        0
                    } else {
                        config.upstream_units_per_second
                    },
                    downstream_units_per_second: if channel.downstream.is_empty() {
                        0
                    } else {
                        config.downstream_units_per_second
                    },
                },
            )?;
        }
        let ns = simulated_dt * 1_000_000_000. + self.environment_subns;
        let elapsed = ns.floor() as u64;
        self.environment_subns = ns - elapsed as f64;
        let budgets = self.environment.advance(&active, elapsed)?;
        let mut changes = BTreeMap::<u32, (u64, u64)>::new();
        let mut supplied = 0u64;
        let mut drained = 0u64;
        let mut suppressed = 0u64;
        for budget in budgets {
            let channel = &cache.channels[&budget.key];
            let mut accepted_in = 0;
            let mut accepted_out = 0;
            let counters = self.environment.feature(budget.key).unwrap().counters;
            for (upstream, candidates, attempts, prior) in [
                (
                    true,
                    &channel.upstream,
                    budget.upstream_attempts,
                    counters.upstream_attempted,
                ),
                (
                    false,
                    &channel.downstream,
                    budget.downstream_attempts,
                    counters.downstream_attempted,
                ),
            ] {
                // Maintained river outlets may bootstrap at most16 whole units
                // ahead of actual upstream additions. A blocked inlet therefore
                // cannot silently drain the visible river while routing catches up.
                let attempts = if !upstream
                    && config.upstream_units_per_second > 0
                    && config.downstream_units_per_second > 0
                {
                    attempts.min(
                        (counters.upstream_added as u128 + accepted_in as u128 + 16)
                            .saturating_sub(counters.downstream_removed as u128)
                            .min(u64::MAX as u128) as u64,
                    )
                } else {
                    attempts
                };
                let mut accepted = 0;
                let count = candidates.len();
                for offset in 0..count {
                    if accepted >= attempts {
                        break;
                    }
                    let candidate = &candidates[(offset + (prior % count as u64) as usize) % count];
                    let c = candidate.cell;
                    let old = self.inactive_cell(c)?;
                    if old.solid || old.water == upstream {
                        continue;
                    }
                    let event = if upstream {
                        RelaxEvent::ExportFilled(c)
                    } else {
                        RelaxEvent::ImportVacancy(c)
                    };
                    if !self.pending_set.contains(&event) && self.pending.len() >= 65536 {
                        continue;
                    }
                    let key = TileKey::containing(c);
                    let block = Arc::make_mut(
                        self.blocks
                            .get_mut(&key)
                            .ok_or("environmental cell lacks exact history")?,
                    );
                    block.set_water_state(c, upstream, [0.; 3])?;
                    if upstream {
                        block.cell_mut(c).unwrap().pressure = candidate.pressure;
                    }
                    let id = self
                        .registry
                        .tile(key)
                        .ok_or("environmental cell lacks reservoir")?
                        .reservoir;
                    let delta = changes.entry(id).or_default();
                    if upstream {
                        delta.0 += 1;
                    } else {
                        delta.1 += 1;
                    }
                    self.enqueue(event, RelaxPending::UnknownBoundary)?;
                    accepted += 1;
                }
                if upstream {
                    accepted_in = accepted;
                } else {
                    accepted_out = accepted;
                }
            }
            supplied = supplied
                .checked_add(accepted_in)
                .ok_or("environmental supply overflow")?;
            drained = drained
                .checked_add(accepted_out)
                .ok_or("environmental drain overflow")?;
            suppressed = suppressed
                .checked_add(budget.upstream_attempts - accepted_in)
                .ok_or("environmental suppression overflow")?;
            self.environment.settle(budget, accepted_in, accepted_out)?;
        }
        if !changes.is_empty() || suppressed != 0 {
            let mut graph = self.regional().clone();
            let id = graph
                .last_transfer_id()
                .checked_add(1)
                .ok_or("environmental serial overflow")?;
            let entries: Vec<_> = changes
                .iter()
                .map(|(&id, &(added, removed))| (id, added, removed))
                .collect();
            graph.external_exchange(id, &entries, suppressed)?;
            self.coupled
                .adopt_regional_after_external_flow(graph, supplied, drained, suppressed)?;
            *self.registry.regional_mut() = self.regional().clone();
            for (&id, _) in &changes {
                let key = self
                    .registry
                    .tiles()
                    .find_map(|(&k, t)| (t.reservoir == id).then_some(k))
                    .ok_or("unknown changed environmental reservoir")?;
                if self.regional().reservoir(id).unwrap().volume != block_mass(&self.blocks[&key]) {
                    return Err("environmental exact native volume mismatch".into());
                }
            }
        }
        Ok(())
    }
    /// Optional explicit environment at the continuing river's exterior halo.
    /// The finite reference step(dt) remains unchanged. This entire update is
    /// staged: native cells, global ledger, clocks and pending work commit once.
    /// Rates are explicit game-boundary parameters, not inferred hydrology.
    /// With both directions enabled, each channel's drain leads accepted supply
    /// by at most16 whole units; downstream-only configurations may drain freely.
    /// Geometry candidates cache until migration or an explicit native solid edit.
    pub fn step_with_environment(
        &mut self,
        world: &World,
        dt: f64,
        config: FlowConfig,
    ) -> Result<StepStats, String> {
        if config.upstream_units_per_second > 1_000_000_000
            || config.downstream_units_per_second > 1_000_000_000
        {
            return Err("environmental game rate exceeds supported range".into());
        }
        let mut staged = self.clone();
        let stats = staged.step_inner(dt)?;
        staged.apply_environment(world, stats.simulated_dt, config)?;
        if !staged.regional().conserved() {
            return Err("environmental global conservation failed".into());
        }
        *self = staged;
        Ok(stats)
    }
    pub fn migrate(&mut self, world: &World, origin: TileKey) -> Result<(), String> {
        checked_window_origin(origin)?;
        if origin == self.origin {
            return Ok(());
        }
        let mut staged = self.clone();
        staged.migrate_inner(world, origin)?;
        staged.validate()?;
        *self = staged;
        Ok(())
    }
    fn migrate_inner(&mut self, world: &World, origin: TileKey) -> Result<(), String> {
        let faces = self.cache_active()?;
        let old_origin = self.origin;
        let old_sim = self.simulation().clone();
        let mut graph = self.coupled.regional().clone();
        let mut token = self.coupled.ownership();
        let mut serial = graph
            .last_transfer_id()
            .checked_add(1)
            .ok_or("migration serial overflow")?;
        for key in active_keys(old_origin) {
            let id = self.registry.tile(key).unwrap().reservoir;
            graph
                .exchange_owned(serial, token, id, block_mass(&self.blocks[&key]), false)
                .map(|t| token = t)?;
            serial = serial.checked_add(1).ok_or("migration serial overflow")?;
        }
        *self.registry.regional_mut() = graph;
        for key in active_keys(origin) {
            Self::materialize(world, &mut self.registry, &mut self.blocks, key)?;
        }
        let mut graph = self.registry.regional().clone();
        for key in active_keys(origin) {
            let id = self.registry.tile(key).unwrap().reservoir;
            let units = block_mass(&self.blocks[&key]);
            if graph.reservoir(id).unwrap().volume != units {
                return Err("reactivation would regenerate or lose dormant water".into());
            }
            token = graph.exchange_owned(serial, token, id, units, true)?;
            serial = serial.checked_add(1).ok_or("migration serial overflow")?;
        }
        let incoming: Vec<_> = active_keys(origin)
            .iter()
            .map(|k| self.blocks[k].as_ref().clone())
            .collect();
        let mut sim = old_sim.migrate(origin.origin()?, [32; 3], &incoming)?;
        // migrate() keeps old overlapping cells, whose dense fringe fields were
        // not authoritative while a separate open-boundary face owned them.
        for (lower, axis, face) in faces {
            if sim.contains(lower) {
                sim.set_positive_face_state(lower, axis, face)?;
            }
        }
        self.origin = origin;
        self.environment_cache = None;
        *self.registry.regional_mut() = graph.clone();
        self.coupled = CoupledWater::new(sim, graph, token, serial)?;
        self.configure_ports(world, true)?;
        self.migrations = self
            .migrations
            .checked_add(1)
            .ok_or("water migration counter overflow")?;
        Ok(())
    }
    /// Native-role reservoirs use explicit spatial capacity 4096. Dry digging
    /// never edits a stale coarse stage table or fabricates displaced water.
    pub fn set_solid(&mut self, world: &World, c: Cell, solid: bool) -> Result<(), String> {
        let mut next = self.clone();
        let key = TileKey::containing(c);
        if !next.blocks.contains_key(&key) {
            return Err("edit outside materialized water history".into());
        }
        if next.simulation().contains(c) {
            if solid {
                let ids: Vec<_> = next
                    .simulation()
                    .boundaries()
                    .filter(|(_, p)| p.cell == c)
                    .map(|(id, _)| id)
                    .collect();
                for id in ids {
                    next.coupled.remove_port(id);
                }
            }
            next.coupled.set_solid(c, solid)?;
        } else {
            let cell = Arc::make_mut(next.blocks.get_mut(&key).unwrap())
                .cell_mut(c)
                .ok_or("dormant edit bounds")?;
            if solid && cell.water {
                return Err("solid placement would displace dormant water".into());
            }
            let was_solid = cell.solid;
            cell.solid = solid;
            cell.pressure = 0.;
            cell.momentum_mps = [0.; 3];
            cell.positive_faces = [NativeFaceState::default(); 3];
            if was_solid && !solid {
                next.enqueue(RelaxEvent::ImportVacancy(c), RelaxPending::UnknownBoundary)?;
            }
        }
        next.configure_ports(world, false)?;
        next.environment_cache = None;
        next.validate()?;
        *self = next;
        Ok(())
    }
    pub fn native_state(&self, c: Cell) -> Option<NativeCellState> {
        if self.simulation().contains(c) {
            self.simulation()
                .export_block(c, [1; 3])
                .ok()?
                .cells
                .first()
                .copied()
        } else {
            self.blocks.get(&TileKey::containing(c))?.cell(c).copied()
        }
    }
    pub fn native_payload_bytes(&self) -> usize {
        self.blocks.len() * 4096 * std::mem::size_of::<NativeCellState>()
    }
    pub fn validate_world(&self, world: &World) -> Result<(), String> {
        for (&key, saved) in &self.blocks {
            let active;
            let block = if self.is_active_key(key) {
                active = self.simulation().export_block(key.origin()?, [16; 3])?;
                &active
            } else {
                saved.as_ref()
            };
            let o = block.origin;
            for z in 0..16 {
                for x in 0..16 {
                    let column = world.column(o.x + x, o.z + z);
                    for y in 0..16 {
                        let c = Cell {
                            x: o.x + x,
                            y: o.y + y,
                            z: o.z + z,
                        };
                        let material = column.canonical_material_without_fluid(c.y);
                        if block.solid_at(c)
                            != Some(!matches!(
                                material,
                                crate::Material::Air | crate::Material::Water
                            ))
                        {
                            return Err(
                                "saved moving native solids disagree with canonical terrain".into(),
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub fn contains_history(&self, c: Cell) -> bool {
        self.blocks.contains_key(&TileKey::containing(c))
    }
    pub fn for_each_block(&self, mut visit: impl FnMut(&NativeBlock)) -> Result<(), String> {
        for (&key, saved) in &self.blocks {
            if self.is_active_key(key) {
                let active = self.simulation().export_block(key.origin()?, [16; 3])?;
                visit(&active)
            } else {
                visit(saved)
            }
        }
        Ok(())
    }
    pub fn occupancy(&self) -> Result<Vec<(Cell, bool)>, String> {
        self.occupancy_in_bounds(None)
    }
    pub fn occupancy_in_bounds(
        &self,
        bounds: Option<(Cell, Cell)>,
    ) -> Result<Vec<(Cell, bool)>, String> {
        let mut out = Vec::new();
        for (&key, saved) in &self.blocks {
            if let Some((a, b)) = bounds {
                let o = key.origin()?;
                if o.x < a.x || o.y < a.y || o.z < a.z || o.x >= b.x || o.y >= b.y || o.z >= b.z {
                    continue;
                }
            }
            let active;
            let block = if self.is_active_key(key) {
                active = self.simulation().export_block(key.origin()?, [16; 3])?;
                &active
            } else {
                saved.as_ref()
            };
            for (i, cell) in block.cells.iter().enumerate() {
                out.push((
                    Cell {
                        x: block.origin.x + (i % 16) as i64,
                        y: block.origin.y + ((i / 16) % 16) as i64,
                        z: block.origin.z + (i / 256) as i64,
                    },
                    cell.water,
                ));
            }
        }
        Ok(out)
    }
    pub fn validate(&self) -> Result<(), String> {
        if !self.regional().conserved()
            || self.coupled.ownership().units != self.simulation().mass() as u64
        {
            return Err("connected ownership/mass mismatch".into());
        }
        let mut active_mass = 0;
        for (&key, block) in &self.blocks {
            let tile = self
                .registry
                .tile(key)
                .ok_or("unregistered exact native block")?;
            if !tile.native || block.origin != key.origin()? || block.dimensions != [16; 3] {
                return Err("native tile identity/role mismatch".into());
            }
            let r = self
                .regional()
                .reservoir(tile.reservoir)
                .ok_or("missing exact spatial reservoir")?;
            if r.capacity != 4096 {
                return Err("native spatial capacity mismatch".into());
            }
            if self.is_active_key(key) {
                if r.volume != 0 {
                    return Err("active tile still owns coarse water".into());
                }
                active_mass += self
                    .simulation()
                    .export_block(key.origin()?, [16; 3])?
                    .cells
                    .iter()
                    .filter(|c| c.water)
                    .count() as u64;
            } else if r.volume != block_mass(block) {
                return Err("dormant native volume mismatch".into());
            }
        }
        if active_mass != self.simulation().mass() as u64
            || active_keys(self.origin)
                .iter()
                .any(|k| !self.blocks.contains_key(k))
        {
            return Err("incomplete connected active tile coverage".into());
        }
        Ok(())
    }
    pub fn snapshot(&self) -> Result<Vec<u8>, String> {
        let mut staged = self.clone();
        staged.cache_active()?;
        *staged.registry.regional_mut() = staged.coupled.regional().clone();
        let registry = staged.registry.snapshot();
        let coupled = staged.coupled.snapshot();
        let mut b = b"DWMOVE03".to_vec();
        for v in staged.origin.0 {
            put(&mut b, v as u64)
        }
        for v in [
            staged.migrations,
            registry.len() as u64,
            coupled.len() as u64,
            staged.blocks.len() as u64,
        ] {
            put(&mut b, v)
        }
        b.extend(registry);
        b.extend(coupled);
        for (&key, block) in &staged.blocks {
            for v in key.0 {
                put(&mut b, v as u64)
            }
            let bytes = block.snapshot()?;
            put(&mut b, bytes.len() as u64);
            b.extend(bytes);
        }
        put(&mut b, staged.pending.len() as u64);
        for &(event, reason) in &staged.pending {
            put(
                &mut b,
                u64::from(matches!(event, RelaxEvent::ExportFilled(_))),
            );
            let c = event.cell();
            for v in [c.x, c.y, c.z] {
                put(&mut b, v as u64)
            }
            put(&mut b, reason as u64);
        }
        put(&mut b, staged.environment_config.upstream_units_per_second);
        put(
            &mut b,
            staged.environment_config.downstream_units_per_second,
        );
        put(&mut b, staged.environment_subns.to_bits());
        let environment = staged.environment.snapshot();
        put(&mut b, environment.len() as u64);
        b.extend(environment);
        let sum = checksum(&b);
        put(&mut b, sum);
        Ok(b)
    }
    pub fn restore(world: &World, b: &[u8]) -> Result<Self, String> {
        let legacy = b.get(..8) == Some(b"DWMOVE01");
        let environmental = b.get(..8) == Some(b"DWMOVE03");
        if b.len() < 72 || (!legacy && !environmental && b.get(..8) != Some(b"DWMOVE02")) {
            return Err("invalid moving water header".into());
        }
        let end = b.len() - 8;
        if checksum(&b[..end]) != u64::from_le_bytes(b[end..].try_into().unwrap()) {
            return Err("moving water checksum mismatch".into());
        }
        let mut r = Reader { b: &b[..end], p: 8 };
        let origin = TileKey([r.get()? as i64, r.get()? as i64, r.get()? as i64]);
        checked_window_origin(origin)?;
        let migrations = r.get()?;
        let a = r.get()?;
        let c = r.get()?;
        let n = r.get()?;
        if n > 4096 {
            return Err("moving native history exceeds budget".into());
        }
        let registry = ProceduralWater::restore(world, r.bytes(a)?)?;
        let coupled = CoupledWater::restore(r.bytes(c)?)?;
        if registry.regional().snapshot() != coupled.regional().snapshot() {
            return Err("moving registry/coupled ledger disagreement".into());
        }
        let mut blocks = BTreeMap::new();
        for _ in 0..n {
            let key = TileKey([r.get()? as i64, r.get()? as i64, r.get()? as i64]);
            let len = r.get()?;
            let block = NativeBlock::restore(r.bytes(len)?)?;
            if blocks.insert(key, Arc::new(block)).is_some() {
                return Err("duplicate dormant native tile".into());
            }
        }
        let mut pending = VecDeque::new();
        let mut pending_set = BTreeSet::new();
        if !legacy {
            let n = r.get()?;
            if n > 65536 {
                return Err("moving pending queue exceeds budget".into());
            }
            for _ in 0..n {
                let kind = r.get()?;
                let c = Cell {
                    x: r.get()? as i64,
                    y: r.get()? as i64,
                    z: r.get()? as i64,
                };
                let event = match kind {
                    0 => RelaxEvent::ImportVacancy(c),
                    1 => RelaxEvent::ExportFilled(c),
                    _ => return Err("invalid pending event".into()),
                };
                let reason = match r.get()? {
                    0 => RelaxPending::BudgetExhausted,
                    1 => RelaxPending::UnknownBoundary,
                    2 => RelaxPending::NoSurfaceDonor,
                    3 => RelaxPending::NoLowerDestination,
                    4 => RelaxPending::StaleEvent,
                    5 => RelaxPending::InvalidState,
                    _ => return Err("invalid pending reason".into()),
                };
                if !pending_set.insert(event) {
                    return Err("duplicate pending relaxation".into());
                }
                pending.push_back((event, reason));
            }
        }
        let (environment_config, environment_subns, environment) = if environmental {
            let config = FlowConfig {
                upstream_units_per_second: r.get()?,
                downstream_units_per_second: r.get()?,
            };
            let subns = f64::from_bits(r.get()?);
            let n = r.get()?;
            let flow = EnvironmentalFlow::restore(r.bytes(n)?)?;
            if config.upstream_units_per_second > 1_000_000_000
                || config.downstream_units_per_second > 1_000_000_000
                || !subns.is_finite()
                || !(0. ..1.).contains(&subns)
                || flow.pending_budgets().next().is_some()
            {
                return Err("invalid moving environmental clock/policy".into());
            }
            (config, subns, flow)
        } else {
            (FlowConfig::default(), 0., EnvironmentalFlow::new(4096)?)
        };
        if r.p != r.b.len() {
            return Err("moving water trailing bytes".into());
        }
        let next = Self {
            coupled,
            registry,
            blocks,
            origin,
            migrations,
            pending,
            pending_set,
            relax_queries: 0,
            relax_moves: 0,
            environment,
            environment_config,
            environment_subns,
            environment_cache: None,
        };
        if next.simulation().origin() != origin.origin()?
            || next.simulation().dimensions() != [32; 3]
        {
            return Err("moving active window bounds mismatch".into());
        }
        next.validate()?;
        Ok(next)
    }
}
fn put(b: &mut Vec<u8>, v: u64) {
    b.extend(v.to_le_bytes())
}
fn checksum(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf29ce484222325, |h, &v| {
        (h ^ v as u64).wrapping_mul(0x100000001b3)
    })
}
struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}
impl<'a> Reader<'a> {
    fn get(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self, n: u64) -> Result<&'a [u8], String> {
        let len = usize::try_from(n).map_err(|_| "moving size overflow")?;
        let end = self.p.checked_add(len).ok_or("moving offset overflow")?;
        let v = self.b.get(self.p..end).ok_or("truncated moving state")?;
        self.p = end;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FbmParams, FbmRecipe, Material};
    const Y: i64 = 16000;
    fn empty_world() -> World {
        World::fbm(
            19,
            FbmParams {
                recipe: FbmRecipe::SeededWorld,
                amplitude: 0.,
                octaves: 1,
                ..Default::default()
            },
        )
        .unwrap()
    }
    fn lake() -> World {
        let mut w = empty_world();
        let mut edits = Vec::new();
        for z in -32..128 {
            for x in -32..128 {
                edits.push((Cell { x, y: Y - 1, z }, Material::Stone));
                for y in 0..4 {
                    edits.push((Cell { x, y: Y + y, z }, Material::Water));
                }
            }
        }
        w.set_batch(edits);
        w
    }
    fn origin() -> TileKey {
        TileKey([0, 1000, 0])
    }
    #[test]
    fn connected_lake_has_open_native_seams_and_exact_finite_accounting() {
        let w = lake();
        let mut s = MovingWater::new(&w, origin(), 256).unwrap();
        assert_eq!(s.tile_count(), 64);
        assert_eq!(s.simulation().dimensions(), [32; 3]);
        assert!(s.simulation().boundaries().count() > 0);
        let mass = s.simulation().mass();
        let stats = s.step(0.01).unwrap();
        assert!(
            stats.boundary_transfers.is_empty(),
            "still lake moved across seam: {}",
            stats.boundary_transfers.len()
        );
        assert_eq!(s.simulation().mass(), mass);
        s.validate().unwrap();
        assert_eq!(s.regional().ledger().supplied, 0);
    }
    #[test]
    fn away_back_keeps_native_momentum_pressure_face_credits_and_dormant_mass() {
        let w = lake();
        let mut s = MovingWater::new(&w, origin(), 256).unwrap();
        let c = Cell {
            x: 15,
            y: Y + 2,
            z: 7,
        };
        let key = TileKey::containing(c);
        let block = Arc::make_mut(s.blocks.get_mut(&key).unwrap());
        let cell = block.cell_mut(c).unwrap();
        cell.momentum_mps = [1.25, 2., -0.5];
        cell.pressure = 3.25;
        cell.positive_faces[0] = NativeFaceState {
            velocity_cells_per_second: 1.125,
            credit_cells: 0.375,
        };
        let expected = *cell;
        let incoming: Vec<_> = active_keys(s.origin)
            .iter()
            .map(|k| s.blocks[k].as_ref().clone())
            .collect();
        let sim =
            DiscreteWater::from_native_blocks(s.origin.origin().unwrap(), [32; 3], &incoming, 17)
                .unwrap();
        s.coupled = CoupledWater::new(
            sim,
            s.regional().clone(),
            s.coupled.ownership(),
            s.regional().last_transfer_id() + 1,
        )
        .unwrap();
        s.configure_ports(&w, true).unwrap();
        s.migrate(&w, TileKey([4, 1000, 0])).unwrap();
        assert_eq!(*s.blocks[&key].cell(c).unwrap(), expected);
        let initial_after_away = s.regional().ledger().initial;
        let saved = s.snapshot().unwrap();
        let mut restored = MovingWater::restore(&w, &saved).unwrap();
        assert_eq!(restored.snapshot().unwrap(), saved);
        restored.migrate(&w, origin()).unwrap();
        let active = restored
            .simulation()
            .export_block(key.origin().unwrap(), [16; 3])
            .unwrap();
        assert_eq!(*active.cell(c).unwrap(), expected);
        assert_eq!(restored.regional().ledger().initial, initial_after_away);
        assert_eq!(restored.regional().ledger().supplied, 0);
        restored.validate().unwrap();
    }
    #[test]
    fn ghost_import_consumes_the_actual_external_cell_once_and_restarts() {
        let mut w = empty_world();
        let outside = Cell {
            x: 32,
            y: Y + 8,
            z: 5,
        };
        w.set(outside, Material::Water);
        let mut s = MovingWater::new(&w, origin(), 256).unwrap();
        let key = TileKey::containing(outside);
        let ghost = Arc::make_mut(s.blocks.get_mut(&key).unwrap())
            .cell_mut(outside)
            .unwrap();
        ghost.momentum_mps = [-10., 0., 0.];
        ghost.pressure = 100.;
        s.refresh_ghosts().unwrap();
        let (id, _) = s
            .simulation()
            .boundaries()
            .find(|(_, p)| next_cell(p.cell, p.axis, p.positive) == outside)
            .unwrap();
        s.coupled
            .set_port_face(
                id,
                NativeFaceState {
                    velocity_cells_per_second: -160.,
                    credit_cells: -0.99,
                },
            )
            .unwrap();
        let stats = s.step(0.002).unwrap();
        assert_eq!(
            stats
                .boundary_transfers
                .iter()
                .filter(|t| t.into_detail)
                .count(),
            1
        );
        assert_eq!(s.blocks[&key].water_at(outside), Some(false));
        assert_eq!(
            s.regional().stored_units() + s.regional().detailed_units(),
            1
        );
        s.validate().unwrap();
        let snapshot = s.snapshot().unwrap();
        let mut b = MovingWater::restore(&w, &snapshot).unwrap();
        s.step(0.002).unwrap();
        b.step(0.002).unwrap();
        assert_eq!(s.snapshot().unwrap(), b.snapshot().unwrap());
        assert_eq!(s.regional().ledger().supplied, 0);
    }
    #[test]
    fn import_fringe_refills_by_real_downward_neighbor_move_and_cross_tile_ledger() {
        let mut w = empty_world();
        let hole = Cell {
            x: 32,
            y: Y + 15,
            z: 5,
        };
        let donor = Cell { y: Y + 16, ..hole };
        w.set_batch([(hole, Material::Water), (donor, Material::Water)]);
        let mut s = MovingWater::new(&w, origin(), 128).unwrap();
        {
            let cell = Arc::make_mut(s.blocks.get_mut(&TileKey::containing(hole)).unwrap())
                .cell_mut(hole)
                .unwrap();
            cell.pressure = 100.;
            cell.momentum_mps = [-10., 0., 0.];
        }
        let momentum = [0.5, -0.75, 0.25];
        Arc::make_mut(s.blocks.get_mut(&TileKey::containing(donor)).unwrap())
            .cell_mut(donor)
            .unwrap()
            .momentum_mps = momentum;
        s.refresh_ghosts().unwrap();
        let (id, _) = s
            .simulation()
            .boundaries()
            .find(|(_, p)| next_cell(p.cell, p.axis, p.positive) == hole)
            .unwrap();
        s.coupled
            .set_port_face(
                id,
                NativeFaceState {
                    velocity_cells_per_second: -160.,
                    credit_cells: -0.99,
                },
            )
            .unwrap();
        let stats = s.step(0.002).unwrap();
        assert_eq!(
            stats
                .boundary_transfers
                .iter()
                .filter(|t| t.into_detail)
                .count(),
            1
        );
        assert_eq!(s.relax_moves(), 1);
        assert_eq!(s.pending_relaxations(), 0);
        assert_eq!(
            s.blocks[&TileKey::containing(hole)].water_at(hole),
            Some(true)
        );
        assert_eq!(
            s.blocks[&TileKey::containing(hole)].momentum_at(hole),
            Some(momentum)
        );
        assert_eq!(
            s.blocks[&TileKey::containing(donor)].water_at(donor),
            Some(false)
        );
        assert_eq!(
            s.regional().stored_units() + s.regional().detailed_units(),
            2
        );
        assert_eq!(s.regional().ledger().supplied, 0);
        s.validate().unwrap();
    }
    #[test]
    fn exhausted_history_rejects_migration_without_half_returning_water() {
        let w = lake();
        let mut s = MovingWater::new(&w, origin(), 64).unwrap();
        let before = s.snapshot().unwrap();
        assert!(s.migrate(&w, TileKey([1, 1000, 0])).is_err());
        assert_eq!(s.snapshot().unwrap(), before);
        s.validate().unwrap();
    }
    #[test]
    fn air_window_exposes_all_6144_faces_instead_of_inventing_walls() {
        let w = empty_world();
        let s = MovingWater::new(&w, origin(), 64).unwrap();
        assert_eq!(s.simulation().boundaries().count(), 6144);
        s.validate().unwrap();
    }
    #[test]
    fn dormant_dry_dig_gets_real_surface_water_without_camera_migration() {
        let mut w = lake();
        let mut water = MovingWater::new(&w, origin(), 128).unwrap();
        let at = Cell {
            x: 32,
            y: Y - 1,
            z: 8,
        };
        assert!(!water.simulation().contains(at));
        let mass = water.regional().stored_units() + water.regional().detailed_units();
        let ledger = water.regional().ledger();
        w.set(at, Material::Air);
        water.set_solid(&w, at, false).unwrap();
        assert!(water.pending_set.contains(&RelaxEvent::ImportVacancy(at)));
        water.step(0.001).unwrap();
        assert!(water.native_state(at).unwrap().water);
        assert_eq!(water.origin(), origin());
        assert_eq!(
            water.regional().stored_units() + water.regional().detailed_units(),
            mass
        );
        assert_eq!(water.regional().ledger(), ledger);
        assert!(water.regional().conserved());
    }
    #[test]
    fn finite_reference_and_closed_lake_do_not_gain_environmental_water() {
        let w = lake();
        let mut water = MovingWater::new(&w, origin(), 128).unwrap();
        let before = water.regional().ledger();
        for _ in 0..2 {
            water
                .step_with_environment(
                    &w,
                    0.025,
                    FlowConfig {
                        upstream_units_per_second: 128,
                        downstream_units_per_second: 128,
                    },
                )
                .unwrap();
        }
        assert_eq!(water.regional().ledger().supplied, before.supplied);
        assert_eq!(water.regional().ledger().drained, before.drained);
        assert_eq!(water.environmental_counters(), FlowCounters::default());
        assert_eq!(water.environmental_features(), 0);
        let bytes = water.snapshot().unwrap();
        assert_eq!(
            MovingWater::restore(&w, &bytes)
                .unwrap()
                .snapshot()
                .unwrap(),
            bytes
        );
    }
    #[test]
    fn generated_exterior_environment_is_counted_restartable_and_migration_never_refills() {
        let w = World::fbm(
            19,
            FbmParams {
                recipe: FbmRecipe::SeededWorld,
                ..Default::default()
            },
        )
        .unwrap();
        let terrain = w.fbm_terrain().unwrap();
        let r = terrain
            .water_reaches([-9600., -9600.], [0., 0.])
            .unwrap()
            .into_iter()
            .find(|r| !r.lake && r.upstream.stage_m > 0.)
            .expect("generated river fixture");
        let x = (((r.upstream.x_m + r.downstream.x_m) * 0.5) / voxel_size()).floor() as i64;
        let z = (((r.upstream.z_m + r.downstream.z_m) * 0.5) / voxel_size()).floor() as i64;
        let col = terrain.column_cells(x, z);
        assert!(col.visible_top > col.ground_top);
        let origin = MovingWater::window_for(Cell {
            x,
            y: col.visible_top - 1,
            z,
        })
        .unwrap();
        let mut water = MovingWater::new(&w, origin, 128).unwrap();
        water.prepare_environment(&w).unwrap();
        let (&key, channel) = water
            .environment_cache
            .as_ref()
            .unwrap()
            .channels
            .iter()
            .find(|(_, c)| !c.upstream.is_empty() && !c.downstream.is_empty())
            .expect("river traverses both exterior halo sides");
        let inlet = channel.upstream[0].cell;
        let outlet = channel.downstream[0].cell;
        let (a, b) = water.overlay_bounds();
        assert!(!water.simulation().contains(inlet));
        assert!(!water.simulation().contains(outlet));
        assert!([
            inlet.x == a.x,
            inlet.x == b.x - 1,
            inlet.y == a.y,
            inlet.y == b.y - 1,
            inlet.z == a.z,
            inlet.z == b.z - 1
        ]
        .into_iter()
        .any(|v| v));
        let mut blocked = water.clone();
        for _ in 0..40 {
            blocked
                .apply_environment(
                    &w,
                    0.125,
                    FlowConfig {
                        upstream_units_per_second: 128,
                        downstream_units_per_second: 128,
                    },
                )
                .unwrap();
        }
        assert_eq!(blocked.environmental_counters().upstream_added, 0);
        for (_, feature) in blocked.environment.features() {
            assert!(
                feature.counters.downstream_removed <= 16,
                "blocked maintained inlet drained its river"
            );
        }
        assert!(blocked.regional().conserved());
        blocked.validate().unwrap();
        let tile = TileKey::containing(inlet);
        let id = water.registry.tile(tile).unwrap().reservoir;
        Arc::make_mut(water.blocks.get_mut(&tile).unwrap())
            .set_water_state(inlet, false, [0.; 3])
            .unwrap();
        let mut graph = water.regional().clone();
        let serial = graph.last_transfer_id() + 1;
        graph.external_exchange(serial, &[(id, 0, 1)], 0).unwrap();
        water
            .coupled
            .adopt_regional_after_external_flow(graph, 0, 1, 0)
            .unwrap();
        *water.registry.regional_mut() = water.regional().clone();
        let config = FlowConfig {
            upstream_units_per_second: 8,
            downstream_units_per_second: 8,
        };
        water.apply_environment(&w, 0.125, config).unwrap();
        assert!(water.native_state(inlet).unwrap().water);
        assert!(!water.native_state(outlet).unwrap().water);
        let counts = water.environment.feature(key).unwrap().counters;
        assert_eq!(counts.upstream_added, 1);
        assert_eq!(counts.downstream_removed, 1);
        assert!(water.regional().conserved());
        water.validate().unwrap();
        let bytes = water.snapshot().unwrap();
        let mut restarted = MovingWater::restore(&w, &bytes).unwrap();
        assert_eq!(bytes, restarted.snapshot().unwrap());
        for _ in 0..3 {
            water.apply_environment(&w, 0.125, config).unwrap();
            restarted.apply_environment(&w, 0.125, config).unwrap();
            assert_eq!(water.snapshot().unwrap(), restarted.snapshot().unwrap());
        }
        let clock = water.environment.snapshot();
        let next = TileKey([origin.0[0] + 1, origin.0[1], origin.0[2]]);
        water.migrate(&w, next).unwrap();
        water.migrate(&w, origin).unwrap();
        assert_eq!(
            water.environment.snapshot(),
            clock,
            "migration emitted or reset environmental water"
        );
        assert!(water.regional().conserved());
        let active_ns = water.environment.feature(key).unwrap().active_ns;
        let stats = water.step_with_environment(&w, 0.001, config).unwrap();
        assert_eq!(
            water.environment.feature(key).unwrap().active_ns - active_ns,
            (stats.simulated_dt * 1_000_000_000.).floor() as u64
        );
    }
    #[test]
    fn moving_window_rejects_unaddressable_halo_before_materialization() {
        let w = empty_world();
        assert!(MovingWater::new(&w, TileKey([i64::MIN / 16, 0, 0]), 128).is_err());
        assert!(MovingWater::new(&w, TileKey([i64::MAX, 0, 0]), 128).is_err());
        assert!(MovingWater::window_for(Cell {
            x: i64::MAX,
            y: 0,
            z: 0
        })
        .is_err());
        assert!(MovingWater::window_for(Cell {
            x: i64::MIN,
            y: 0,
            z: 0
        })
        .is_err());
    }
}
