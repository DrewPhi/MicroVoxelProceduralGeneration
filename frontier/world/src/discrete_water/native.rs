//! Exact native state transfer for fixed aligned detailed tiles. These methods
//! do not allocate regional water ownership or infer a hydraulic boundary.
use super::{BoundaryGhost, DiscreteWater, Ledger, OpenBoundary, Source};
use crate::Cell;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NativeFaceState {
    pub velocity_cells_per_second: f32,
    pub credit_cells: f32,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceClock {
    pub units_per_second: f64,
    pub phase: f64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NativeCellState {
    pub water: bool,
    pub solid: bool,
    pub momentum_mps: [f32; 3],
    pub pressure: f32,
    /// Positive-axis faces are owned by this lower global cell even when the
    /// next cell lives in a different dormant tile.
    pub positive_faces: [NativeFaceState; 3],
    pub source: Option<SourceClock>,
    pub outlet: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeBoundaryState {
    pub id: u64,
    pub condition: OpenBoundary,
    /// Outward normal state; convert explicitly when this becomes internal.
    pub normal: NativeFaceState,
    pub ghost: Option<BoundaryGhost>,
}
impl NativeBoundaryState {
    pub fn positive_face(self) -> Result<(Cell, usize, NativeFaceState), String> {
        if self.condition.axis >= 3 {
            return Err("invalid native boundary axis".into());
        }
        let mut lower = self.condition.cell;
        if !self.condition.positive {
            let v = match self.condition.axis {
                0 => &mut lower.x,
                1 => &mut lower.y,
                2 => &mut lower.z,
                _ => return Err("invalid boundary axis".into()),
            };
            *v = v
                .checked_sub(1)
                .ok_or("boundary face coordinate overflow")?;
        }
        let sign = if self.condition.positive { 1. } else { -1. };
        Ok((
            lower,
            self.condition.axis,
            NativeFaceState {
                velocity_cells_per_second: sign * self.normal.velocity_cells_per_second,
                credit_cells: sign * self.normal.credit_cells,
            },
        ))
    }
    pub fn from_positive_face(id: u64, condition: OpenBoundary, face: NativeFaceState) -> Self {
        let sign = if condition.positive { 1. } else { -1. };
        Self {
            id,
            condition,
            ghost: None,
            normal: NativeFaceState {
                velocity_cells_per_second: sign * face.velocity_cells_per_second,
                credit_cells: sign * face.credit_cells,
            },
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct NativeBlock {
    pub origin: Cell,
    pub dimensions: [usize; 3],
    /// Last export tick, not a command to advance sleeping water.
    pub tick: u64,
    /// x-fast, then y, then z; there are no duplicate cell coordinates.
    pub cells: Vec<NativeCellState>,
    /// Explicit exterior ports are separate from native positive-axis fields.
    /// A manager must rebind them against current global regional ownership.
    pub boundaries: Vec<NativeBoundaryState>,
}
impl NativeBlock {
    pub fn cell(&self, c: Cell) -> Option<&NativeCellState> {
        self.index(c).and_then(|i| self.cells.get(i))
    }
    pub fn cell_mut(&mut self, c: Cell) -> Option<&mut NativeCellState> {
        self.index(c).and_then(|i| self.cells.get_mut(i))
    }
    pub fn water_at(&self, c: Cell) -> Option<bool> {
        self.cell(c).map(|v| v.water)
    }
    pub fn solid_at(&self, c: Cell) -> Option<bool> {
        self.cell(c).map(|v| v.solid)
    }
    pub fn momentum_at(&self, c: Cell) -> Option<[f32; 3]> {
        self.cell(c).map(|v| v.momentum_mps)
    }
    pub fn pressure_at(&self, c: Cell) -> Option<f32> {
        self.cell(c).map(|v| v.pressure)
    }
    pub fn set_water_state(
        &mut self,
        c: Cell,
        water: bool,
        momentum_mps: [f32; 3],
    ) -> Result<(), String> {
        let cell = self.cell_mut(c).ok_or("native water edit outside block")?;
        if cell.solid
            || momentum_mps.iter().any(|v| !v.is_finite())
            || (!water && momentum_mps.iter().any(|&v| v != 0.))
        {
            return Err("invalid native water occupancy/momentum edit".into());
        }
        cell.water = water;
        cell.momentum_mps = momentum_mps;
        cell.pressure = 0.;
        Ok(())
    }
    fn index(&self, c: Cell) -> Option<usize> {
        let delta = [
            c.x as i128 - self.origin.x as i128,
            c.y as i128 - self.origin.y as i128,
            c.z as i128 - self.origin.z as i128,
        ];
        if self.dimensions.contains(&0)
            || (0..3).any(|a| delta[a] < 0 || delta[a] >= self.dimensions[a] as i128)
        {
            return None;
        }
        let yz =
            (delta[1] as usize).checked_add(self.dimensions[1].checked_mul(delta[2] as usize)?)?;
        (delta[0] as usize).checked_add(self.dimensions[0].checked_mul(yz)?)
    }
    fn to_sim(&self) -> Result<DiscreteWater, String> {
        let mut sim = DiscreteWater::new(self.origin, self.dimensions)?;
        if self.cells.len() != sim.flags.len() {
            return Err("native block cell count mismatch".into());
        }
        sim.tick = self.tick;
        for (i, &cell) in self.cells.iter().enumerate() {
            sim.install_native(i, cell)?;
        }
        sim.ledger = Ledger {
            added: sim.mass as u64,
            ..Ledger::default()
        };
        for &boundary in &self.boundaries {
            if sim.boundary(boundary.id).is_some() {
                return Err("duplicate native boundary ID".into());
            }
            sim.set_boundary_state(boundary)?;
        }
        sim.rebuild_active_face_cache();
        Ok(sim)
    }
    pub fn validate(&self) -> Result<(), String> {
        self.to_sim().map(|_| ())
    }
    /// Complete dormant state, including numerical bits and endpoint clocks.
    /// This envelope contains no independent regional ownership ledger.
    pub fn snapshot(&self) -> Result<Vec<u8>, String> {
        let sim = self.to_sim()?;
        let mut bytes = b"DWNAT001".to_vec();
        bytes.extend(sim.snapshot());
        Ok(bytes)
    }
    pub fn restore(bytes: &[u8]) -> Result<Self, String> {
        if bytes.get(..8) != Some(b"DWNAT001") {
            return Err("unsupported native block checkpoint".into());
        }
        let sim = DiscreteWater::restore(&bytes[8..])?;
        sim.export_block(sim.origin, sim.dims)
    }
}
impl DiscreteWater {
    fn native_cell(&self, i: usize) -> NativeCellState {
        NativeCellState {
            water: self.flags[i] == 1,
            solid: self.flags[i] == 2,
            momentum_mps: self.momentum[i],
            pressure: self.pressure[i],
            positive_faces: std::array::from_fn(|a| NativeFaceState {
                velocity_cells_per_second: self.velocity[i][a],
                credit_cells: self.credit[i][a],
            }),
            source: self.sources.get(&i).map(|s| SourceClock {
                units_per_second: s.rate,
                phase: s.phase,
            }),
            outlet: self.outlets.contains(&i),
        }
    }
    fn install_native(&mut self, i: usize, cell: NativeCellState) -> Result<(), String> {
        if (cell.water && cell.solid)
            || (cell.solid && (cell.source.is_some() || cell.outlet))
            || (cell.source.is_some() && cell.outlet)
        {
            return Err("native occupancy/endpoints overlap".into());
        }
        if cell.momentum_mps.iter().any(|v| !v.is_finite())
            || !cell.pressure.is_finite()
            || cell.pressure < 0.
            || cell
                .positive_faces
                .iter()
                .any(|f| !f.velocity_cells_per_second.is_finite() || !f.credit_cells.is_finite())
        {
            return Err("invalid native numerical fields".into());
        }
        if !cell.water && cell.momentum_mps.iter().any(|&v| v != 0.) {
            return Err("dry native cell carries fluid momentum".into());
        }
        if let Some(source) = cell.source {
            if !source.units_per_second.is_finite()
                || source.units_per_second <= 0.
                || source.units_per_second > 1_000_000.
                || !source.phase.is_finite()
                || !(0. ..1.).contains(&source.phase)
            {
                return Err("invalid native source clock".into());
            }
            self.sources.insert(
                i,
                Source {
                    rate: source.units_per_second,
                    phase: source.phase,
                },
            );
        }
        if cell.outlet {
            self.outlets.insert(i);
        }
        self.flags[i] = if cell.solid { 2 } else { u8::from(cell.water) };
        if cell.water {
            self.mass += 1;
        }
        self.momentum[i] = cell.momentum_mps;
        self.pressure[i] = cell.pressure;
        for a in 0..3 {
            self.velocity[i][a] = cell.positive_faces[a].velocity_cells_per_second;
            self.credit[i][a] = cell.positive_faces[a].credit_cells;
        }
        Ok(())
    }
    pub fn export_boundaries(&self) -> Vec<NativeBoundaryState> {
        self.boundaries
            .iter()
            .map(|b| NativeBoundaryState {
                id: b.id,
                condition: b.condition,
                ghost: b.ghost,
                normal: NativeFaceState {
                    velocity_cells_per_second: b.velocity,
                    credit_cells: b.credit,
                },
            })
            .collect()
    }
    pub fn set_boundary_state(&mut self, state: NativeBoundaryState) -> Result<(), String> {
        if !state.normal.velocity_cells_per_second.is_finite()
            || !state.normal.credit_cells.is_finite()
        {
            return Err("invalid native boundary face fields".into());
        }
        if let Some(ghost) = state.ghost {
            ghost.validate()?;
        }
        self.set_boundary(state.id, state.condition)?;
        let b = self
            .boundaries
            .iter_mut()
            .find(|b| b.id == state.id)
            .unwrap();
        b.velocity = state.normal.velocity_cells_per_second;
        b.credit = state.normal.credit_cells;
        self.set_boundary_ghost(state.id, state.ghost)?;
        Ok(())
    }
    pub fn positive_face_state(&self, lower: Cell, axis: usize) -> Option<NativeFaceState> {
        if axis >= 3 {
            return None;
        }
        self.index(lower).map(|i| NativeFaceState {
            velocity_cells_per_second: self.velocity[i][axis],
            credit_cells: self.credit[i][axis],
        })
    }
    pub fn set_positive_face_state(
        &mut self,
        lower: Cell,
        axis: usize,
        state: NativeFaceState,
    ) -> Result<(), String> {
        let i = self
            .index(lower)
            .ok_or("native face owner outside detailed domain")?;
        if axis >= 3
            || !state.velocity_cells_per_second.is_finite()
            || !state.credit_cells.is_finite()
        {
            return Err("invalid positive native face state".into());
        }
        self.velocity[i][axis] = state.velocity_cells_per_second;
        self.credit[i][axis] = state.credit_cells;
        if let Some(j) = self.neighbor(i, axis, true) {
            self.active_faces.push((i, axis, j));
        }
        Ok(())
    }
    pub fn export_block(
        &self,
        origin: Cell,
        dimensions: [usize; 3],
    ) -> Result<NativeBlock, String> {
        let probe = Self::new(origin, dimensions)?;
        let mut cells = Vec::with_capacity(probe.flags.len());
        for i in 0..probe.flags.len() {
            let c = probe.cell(i);
            let old = self
                .index(c)
                .ok_or("native export extends outside detailed domain")?;
            cells.push(self.native_cell(old));
        }
        let boundaries = self
            .export_boundaries()
            .into_iter()
            .filter(|b| probe.contains(b.condition.cell))
            .collect();
        Ok(NativeBlock {
            origin,
            dimensions,
            tick: self.tick,
            cells,
            boundaries,
        })
    }
    pub(super) fn rebuild_active_face_cache(&mut self) {
        self.active_faces.clear();
        for i in 0..self.flags.len() {
            for a in 0..3 {
                if self.velocity[i][a] != 0. || self.credit[i][a] != 0. {
                    if let Some(j) = self.neighbor(i, a, true) {
                        self.active_faces.push((i, a, j));
                    }
                }
            }
        }
    }
    /// Assemble an initial connected cuboid from complete, nonoverlapping
    /// native blocks. No exterior ports are automatically rebound.
    pub fn from_native_blocks(
        origin: Cell,
        dimensions: [usize; 3],
        blocks: &[NativeBlock],
        tick: u64,
    ) -> Result<Self, String> {
        let mut next = Self::new(origin, dimensions)?;
        next.tick = tick;
        let supplied = covered_cells(&next, blocks)?;
        for (i, cell) in supplied.into_iter().enumerate() {
            next.install_native(i, *cell.ok_or("missing native assembly cell coverage")?)?;
        }
        next.ledger = Ledger {
            added: next.mass as u64,
            ..Ledger::default()
        };
        next.rebuild_active_face_cache();
        Ok(next)
    }
    /// Reassemble a connected detailed cuboid. Existing overlapping cells win;
    /// incoming blocks cover every newly entered cell exactly once. Pure native
    /// state is retained bit-for-bit, including cross-tile positive face fields.
    /// All ports are deliberately unbound: the caller must atomically restore
    /// valid exterior ports and convert old exterior faces that became internal.
    /// Regional ownership is unchanged by this operation and must be reconciled
    /// by the manager in the same staged transaction.
    pub fn migrate(
        &self,
        origin: Cell,
        dimensions: [usize; 3],
        incoming: &[NativeBlock],
    ) -> Result<Self, String> {
        let mut next = Self::new(origin, dimensions)?;
        next.tick = self.tick;
        let supplied = covered_cells(&next, incoming)?;
        let mut entered = 0u64;
        for i in 0..next.flags.len() {
            let c = next.cell(i);
            let cell = if let Some(old) = self.index(c) {
                self.native_cell(old)
            } else {
                *supplied[i].ok_or("missing native migration cell coverage")?
            };
            if !self.contains(c) && cell.water {
                entered += 1;
            }
            next.install_native(i, cell)?;
        }
        let departed = self.occupied_cells().filter(|&c| !next.contains(c)).count() as u64;
        next.ledger = self.ledger;
        next.ledger.migration_imported = next
            .ledger
            .migration_imported
            .checked_add(entered)
            .ok_or("migration ledger overflow")?;
        next.ledger.migration_exported = next
            .ledger
            .migration_exported
            .checked_add(departed)
            .ok_or("migration ledger overflow")?;
        next.rebuild_active_face_cache();
        Ok(next)
    }
}

fn covered_cells<'a>(
    target: &DiscreteWater,
    blocks: &'a [NativeBlock],
) -> Result<Vec<Option<&'a NativeCellState>>, String> {
    if blocks.len() > 4096
        || blocks
            .iter()
            .try_fold(0usize, |n, b| n.checked_add(b.cells.len()))
            .is_none_or(|n| n > super::MAX_CELLS)
    {
        return Err("native migration input budget exceeded".into());
    }
    // Reject duplicate coverage even outside the destination rather than let a
    // caller mistake ignored input blocks for distinct dormant ownership.
    for (i, a) in blocks.iter().enumerate() {
        for b in &blocks[..i] {
            let ao = [a.origin.x, a.origin.y, a.origin.z];
            let bo = [b.origin.x, b.origin.y, b.origin.z];
            if (0..3).all(|axis| {
                (ao[axis] as i128) < bo[axis] as i128 + b.dimensions[axis] as i128
                    && (bo[axis] as i128) < ao[axis] as i128 + a.dimensions[axis] as i128
            }) {
                return Err("duplicate native migration block coverage".into());
            }
        }
    }
    let mut supplied = vec![None; target.flags.len()];
    for block in blocks {
        block.validate()?;
        let probe = DiscreteWater::new(block.origin, block.dimensions)?;
        for (bi, cell) in block.cells.iter().enumerate() {
            if let Some(i) = target.index(probe.cell(bi)) {
                if supplied[i].replace(cell).is_some() {
                    return Err("duplicate native migration cell coverage".into());
                }
            }
        }
    }
    Ok(supplied)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c(x: i64, y: i64, z: i64) -> Cell {
        Cell { x, y, z }
    }
    fn block(x: i64) -> NativeBlock {
        let cells = (0..4096)
            .map(|i| {
                let solid = i % 13 == 0;
                let water = !solid && i % 11 == 0;
                let source = if !solid && i % 97 == 0 {
                    Some(SourceClock {
                        units_per_second: 7.25,
                        phase: 0.123456789,
                    })
                } else {
                    None
                };
                NativeCellState {
                    solid,
                    water,
                    momentum_mps: if water {
                        [i as f32 * 0.0001, -0.125, 0.25]
                    } else {
                        [0.; 3]
                    },
                    pressure: (i % 17) as f32 * 0.25,
                    positive_faces: std::array::from_fn(|a| NativeFaceState {
                        velocity_cells_per_second: (i % 23) as f32 * 0.01 + a as f32,
                        credit_cells: (i % 7) as f32 * 0.03125 - 0.125,
                    }),
                    source,
                    outlet: !solid && source.is_none() && i % 101 == 0,
                }
            })
            .collect();
        NativeBlock {
            origin: c(x, 0, 0),
            dimensions: [16; 3],
            tick: 77,
            cells,
            boundaries: Vec::new(),
        }
    }
    fn check(w: &DiscreteWater) {
        let l = w.ledger();
        assert_eq!(
            l.added + l.source_added + l.boundary_imported + l.migration_imported,
            w.mass() as u64
                + l.removed
                + l.outlet_removed
                + l.boundary_exported
                + l.migration_exported
        );
    }
    #[test]
    fn connected_window_a_b_a_preserves_exact_dormant_native_state() {
        let (a, b, cc) = (block(0), block(16), block(32));
        let total = a
            .cells
            .iter()
            .chain(&b.cells)
            .chain(&cc.cells)
            .filter(|v| v.water)
            .count();
        let sim = DiscreteWater::from_native_blocks(
            c(0, 0, 0),
            [32, 16, 16],
            &[a.clone(), b.clone()],
            77,
        )
        .unwrap();
        let asleep_a = sim.export_block(c(0, 0, 0), [16; 3]).unwrap();
        let a_bytes = asleep_a.snapshot().unwrap();
        let moved = sim
            .migrate(c(16, 0, 0), [32, 16, 16], &[cc.clone()])
            .unwrap();
        check(&moved);
        assert_eq!(
            moved
                .export_block(c(16, 0, 0), [16; 3])
                .unwrap()
                .snapshot()
                .unwrap(),
            b.snapshot().unwrap()
        );
        assert_eq!(
            moved.mass() + asleep_a.cells.iter().filter(|v| v.water).count(),
            total
        );
        let asleep_c = moved.export_block(c(32, 0, 0), [16; 3]).unwrap();
        assert_eq!(asleep_c.snapshot().unwrap(), cc.snapshot().unwrap());
        let restored_a = NativeBlock::restore(&a_bytes).unwrap();
        let back = moved
            .migrate(c(0, 0, 0), [32, 16, 16], &[restored_a])
            .unwrap();
        check(&back);
        assert_eq!(
            back.export_block(c(0, 0, 0), [16; 3])
                .unwrap()
                .snapshot()
                .unwrap(),
            a_bytes
        );
        assert_eq!(
            back.export_block(c(16, 0, 0), [16; 3])
                .unwrap()
                .snapshot()
                .unwrap(),
            b.snapshot().unwrap()
        );
        assert_eq!(
            back.mass() + asleep_c.cells.iter().filter(|v| v.water).count(),
            total
        );
        let mut restart = DiscreteWater::restore(&back.snapshot()).unwrap();
        let mut continued = back;
        for _ in 0..3 {
            continued.step(0.002);
            restart.step(0.002);
            assert_eq!(continued.snapshot(), restart.snapshot());
            check(&continued);
        }
    }
    #[test]
    fn invalid_duplicate_or_missing_migration_coverage_rejects_atomically() {
        let a = block(0);
        let sim = DiscreteWater::from_native_blocks(c(0, 0, 0), [16; 3], &[a.clone()], 77).unwrap();
        let before = sim.snapshot();
        assert!(sim.migrate(c(16, 0, 0), [16; 3], &[]).is_err());
        let b = block(16);
        assert!(sim
            .migrate(c(16, 0, 0), [16; 3], &[b.clone(), b.clone()])
            .is_err());
        let mut invalid = b.clone();
        invalid.cells[1].solid = true;
        invalid.cells[1].water = true;
        assert!(sim.migrate(c(16, 0, 0), [16; 3], &[invalid]).is_err());
        let mut invalid = b;
        invalid.cells[1].pressure = f32::NAN;
        assert!(invalid.snapshot().is_err());
        assert_eq!(sim.snapshot(), before);
    }
    #[test]
    fn explicit_boundary_and_canonical_positive_face_state_roundtrip() {
        let mut sim = DiscreteWater::new(c(0, 0, 0), [2, 1, 1]).unwrap();
        let condition = OpenBoundary {
            cell: c(0, 0, 0),
            axis: 0,
            positive: false,
            head_m: 1.,
            velocity_mps: [0.; 3],
            import_quota: 1,
            export_quota: 0,
        };
        let state = NativeBoundaryState {
            id: 7,
            condition,
            normal: NativeFaceState {
                velocity_cells_per_second: -3.125,
                credit_cells: -0.375,
            },
            ghost: Some(BoundaryGhost {
                wet: true,
                pressure: 12.5,
                velocity_mps: [0.125, 0., 0.],
            }),
        };
        sim.set_boundary_state(state).unwrap();
        let block = sim.export_block(c(0, 0, 0), [2, 1, 1]).unwrap();
        assert_eq!(
            NativeBlock::restore(&block.snapshot().unwrap()).unwrap(),
            block
        );
        let (lower, axis, face) = state.positive_face().unwrap();
        assert_eq!(lower, c(-1, 0, 0));
        assert_eq!(axis, 0);
        assert_eq!(face.credit_cells, 0.375);
        let mut converted = NativeBoundaryState::from_positive_face(7, condition, face);
        converted.ghost = state.ghost;
        assert_eq!(converted, state);
        let mut incoming = NativeBlock {
            origin: c(-1, 0, 0),
            dimensions: [1; 3],
            tick: 0,
            cells: vec![NativeCellState::default()],
            boundaries: Vec::new(),
        };
        incoming.cells[0].positive_faces[0] = face;
        let migrated = sim.migrate(c(-1, 0, 0), [3, 1, 1], &[incoming]).unwrap();
        assert_eq!(migrated.boundaries().count(), 0);
        assert_eq!(migrated.positive_face_state(lower, axis), Some(face));
    }
}
