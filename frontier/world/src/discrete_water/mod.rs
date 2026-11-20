//! Bounded, whole native voxel fluid. Occupancy is binary and authoritative.
//! Pressure/momentum guide quantized transfers; this is not a converged continuum
//! solver. Region boundaries are impermeable. Sources emit at a fixed whole-cell rate;
//! outlets remove whole cells. Neither represents an infinite mass reservoir in
//! the occupancy ledger. No renderer state is authoritative.
mod boundary;
mod native;
pub use native::{NativeBlock, NativeBoundaryState, NativeCellState, NativeFaceState, SourceClock};
pub mod coupled;
pub mod dormant_relax;
pub mod environmental_flow;
pub mod regional;
use boundary::BoundaryState;
pub use boundary::{BoundaryGhost, BoundaryTransfer, OpenBoundary};

use crate::{Cell, voxel_size};
use std::collections::{BTreeMap, BTreeSet};

const MAX_CELLS: usize = 2_000_000;
const MAX_SUBSTEPS: usize = 32;
const PRESSURE_SWEEPS: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ledger {
    pub added: u64,
    pub removed: u64,
    pub source_added: u64,
    pub outlet_removed: u64,
    pub source_suppressed: u64,
    pub boundary_imported: u64,
    pub boundary_exported: u64,
    pub migration_imported: u64,
    pub migration_exported: u64,
}
#[derive(Clone, Debug, Default)]
pub struct StepStats {
    pub mass: usize,
    pub moved: usize,
    pub candidates: usize,
    pub rejected: usize,
    pub substeps: usize,
    pub simulated_dt: f64,
    /// Time the bounded work budget did not simulate; caller may retain it.
    pub deferred_dt: f64,
    pub pressure_residual_rms: f64,
    pub pressure_sweeps: usize,
    /// Sum over substeps of weighted face kinetic guidance energy (unit mass).
    pub projection_energy_before: f64,
    pub projection_energy_after: f64,
    pub projection_reductions: usize,
    pub boundary_transfers: Vec<BoundaryTransfer>,
    pub boundary_blocked: usize,
    pub divergence_rms: f64,
    pub max_speed_mps: f64,
    pub source_added: u64,
    pub outlet_removed: u64,
    pub source_suppressed: u64,
}
#[derive(Clone, Copy)]
struct Source {
    rate: f64,
    phase: f64,
}
#[derive(Clone)]
pub struct DiscreteWater {
    origin: Cell,
    dims: [usize; 3],
    // One byte state, cell momentum/pressure, implicit positive-axis face fields.
    // 41 bytes/cell excluding Vec headers; no per-cell adjacency allocations.
    flags: Vec<u8>,
    momentum: Vec<[f32; 3]>,
    pressure: Vec<f32>,
    velocity: Vec<[f32; 3]>,
    credit: Vec<[f32; 3]>,
    sources: BTreeMap<usize, Source>,
    outlets: BTreeSet<usize>,
    tick: u64,
    mass: usize,
    ledger: Ledger,
    boundaries: Vec<BoundaryState>,
    // Derived topology cache only; restart rebuilds from persisted face fields.
    active_faces: Vec<(usize, usize, usize)>,
}
#[derive(Clone, Copy)]
struct Move {
    from: usize,
    to: usize,
    axis: usize,
    face: usize,
    sign: f32,
    score: f32,
}
impl DiscreteWater {
    pub fn new(origin: Cell, dimensions: [usize; 3]) -> Result<Self, String> {
        let n = dimensions
            .iter()
            .try_fold(1usize, |a, &b| if b == 0 { None } else { a.checked_mul(b) })
            .filter(|&n| n <= MAX_CELLS)
            .ok_or("invalid or excessive water region")?;
        for (o, d) in [origin.x, origin.y, origin.z].into_iter().zip(dimensions) {
            o.checked_add(d as i64)
                .ok_or("water region coordinate overflow")?;
        }
        Ok(Self {
            origin,
            dims: dimensions,
            flags: vec![0; n],
            momentum: vec![[0.; 3]; n],
            pressure: vec![0.; n],
            velocity: vec![[0.; 3]; n],
            credit: vec![[0.; 3]; n],
            sources: BTreeMap::new(),
            outlets: BTreeSet::new(),
            tick: 0,
            mass: 0,
            ledger: Ledger::default(),
            boundaries: Vec::new(),
            active_faces: Vec::new(),
        })
    }
    pub fn origin(&self) -> Cell {
        self.origin
    }
    pub fn dimensions(&self) -> [usize; 3] {
        self.dims
    }
    pub fn tick(&self) -> u64 {
        self.tick
    }
    pub fn mass(&self) -> usize {
        self.mass
    }
    pub fn ledger(&self) -> Ledger {
        self.ledger
    }
    pub fn contains(&self, c: Cell) -> bool {
        self.index(c).is_some()
    }
    pub fn water_at(&self, c: Cell) -> bool {
        self.index(c).is_some_and(|i| self.flags[i] == 1)
    }
    pub fn solid_at(&self, c: Cell) -> bool {
        self.index(c).is_some_and(|i| self.flags[i] == 2)
    }
    pub fn is_source(&self, c: Cell) -> bool {
        self.index(c).is_some_and(|i| self.sources.contains_key(&i))
    }
    pub fn is_outlet(&self, c: Cell) -> bool {
        self.index(c).is_some_and(|i| self.outlets.contains(&i))
    }
    pub fn occupied_cells(&self) -> impl Iterator<Item = Cell> + '_ {
        self.flags
            .iter()
            .enumerate()
            .filter(|(_, v)| **v == 1)
            .map(|(i, _)| self.cell(i))
    }
    fn index(&self, c: Cell) -> Option<usize> {
        let d = [
            c.x as i128 - self.origin.x as i128,
            c.y as i128 - self.origin.y as i128,
            c.z as i128 - self.origin.z as i128,
        ];
        if (0..3).any(|a| d[a] < 0 || d[a] >= self.dims[a] as i128) {
            return None;
        }
        Some(d[0] as usize + self.dims[0] * (d[1] as usize + self.dims[1] * d[2] as usize))
    }
    fn cell(&self, i: usize) -> Cell {
        Cell {
            x: self.origin.x + (i % self.dims[0]) as i64,
            y: self.origin.y + ((i / self.dims[0]) % self.dims[1]) as i64,
            z: self.origin.z + (i / (self.dims[0] * self.dims[1])) as i64,
        }
    }
    fn neighbor(&self, i: usize, a: usize, positive: bool) -> Option<usize> {
        let stride = [1, self.dims[0], self.dims[0] * self.dims[1]][a];
        let p = (i / stride) % self.dims[a];
        if positive {
            (p + 1 < self.dims[a]).then_some(i + stride)
        } else {
            (p > 0).then(|| i - stride)
        }
    }
    pub fn set_solid(&mut self, c: Cell, solid: bool) -> Result<(), String> {
        let i = self.index(c).ok_or("cell outside water region")?;
        if solid
            && (self.flags[i] == 1
                || self.sources.contains_key(&i)
                || self.outlets.contains(&i)
                || self.boundaries.iter().any(|b| b.index == i))
        {
            return Err("solid edit would displace water/source/outlet; edit rejected".into());
        }
        if self.flags[i] == 2 || solid {
            self.flags[i] = if solid { 2 } else { 0 };
            self.reset_cell(i);
        }
        Ok(())
    }
    pub fn set_water(&mut self, c: Cell, water: bool) -> Result<(), String> {
        let i = self.index(c).ok_or("cell outside water region")?;
        if self.flags[i] == 2 {
            return Err("water edit overlaps solid".into());
        }
        if water != (self.flags[i] == 1) {
            self.flags[i] = u8::from(water);
            self.reset_cell(i);
            if water {
                self.mass += 1;
                self.ledger.added += 1;
            } else {
                self.mass -= 1;
                self.ledger.removed += 1;
            }
        }
        Ok(())
    }
    fn reset_cell(&mut self, i: usize) {
        self.momentum[i] = [0.; 3];
        self.pressure[i] = 0.;
        self.velocity[i] = [0.; 3];
        self.credit[i] = [0.; 3];
        for a in 0..3 {
            if let Some(j) = self.neighbor(i, a, false) {
                self.velocity[j][a] = 0.;
                self.credit[j][a] = 0.;
            }
        }
    }
    pub fn set_source(&mut self, c: Cell, enabled: bool) -> Result<(), String> {
        let i = self.index(c).ok_or("source outside water region")?;
        if enabled
            && (self.flags[i] == 2
                || self.outlets.contains(&i)
                || self.boundaries.iter().any(|b| b.index == i))
        {
            return Err("source overlaps solid/outlet".into());
        }
        if enabled {
            self.sources.entry(i).or_insert(Source {
                rate: 40.,
                phase: 0.,
            });
        } else {
            self.sources.remove(&i);
        }
        Ok(())
    }
    pub fn set_outlet(&mut self, c: Cell, enabled: bool) -> Result<(), String> {
        let i = self.index(c).ok_or("outlet outside water region")?;
        if enabled
            && (self.flags[i] == 2
                || self.sources.contains_key(&i)
                || self.boundaries.iter().any(|b| b.index == i))
        {
            return Err("outlet overlaps solid/source".into());
        }
        if enabled {
            self.outlets.insert(i);
        } else {
            self.outlets.remove(&i);
        }
        Ok(())
    }
    /// Configure emission in whole native cells/second. Blocked attempts are
    /// suppressed, never banked for a later burst. Fractional phase is a clock,
    /// not fractional fluid mass.
    pub fn set_source_rate(&mut self, c: Cell, units_per_second: f64) -> Result<(), String> {
        if !units_per_second.is_finite() || units_per_second <= 0. || units_per_second > 1_000_000.
        {
            return Err("invalid water source rate".into());
        }
        self.set_source(c, true)?;
        let i = self.index(c).unwrap();
        self.sources.get_mut(&i).unwrap().rate = units_per_second;
        Ok(())
    }
    fn endpoints(&mut self, dt: f64, stats: &mut StepStats) {
        for &i in &self.outlets {
            if self.flags[i] == 1 {
                self.flags[i] = 0;
                self.momentum[i] = [0.; 3];
                self.pressure[i] = 0.;
                self.mass -= 1;
                self.ledger.outlet_removed += 1;
                stats.outlet_removed += 1;
            }
        }
        for (&i, source) in &mut self.sources {
            source.phase += dt * source.rate;
            let attempts = (source.phase + 1e-10).floor() as u64;
            source.phase = (source.phase - attempts as f64).max(0.);
            let added = if attempts > 0 && self.flags[i] == 0 {
                self.flags[i] = 1;
                self.momentum[i] = [0.; 3];
                self.mass += 1;
                self.ledger.source_added += 1;
                stats.source_added += 1;
                1
            } else {
                0
            };
            self.ledger.source_suppressed += attempts - added;
            stats.source_suppressed += attempts - added;
        }
    }
    pub fn step(&mut self, dt: f64) -> StepStats {
        let mut stats = StepStats::default();
        if !dt.is_finite() || dt <= 0. {
            stats.mass = self.mass;
            return stats;
        }
        let mut remaining = dt;
        while remaining > 1e-9 && stats.substeps < MAX_SUBSTEPS && self.tick < u64::MAX {
            let vmax = self
                .momentum
                .iter()
                .flat_map(|v| v.iter())
                .fold(0f64, |m, &v| m.max((v as f64).abs()));
            // CFL including gravity; no velocity clamp. Excess time is explicit.
            let exterior_speed = self
                .boundaries
                .iter()
                .filter(|b| b.open())
                .flat_map(|b| b.external_velocity())
                .fold(0f64, |m, v| m.max((v as f64).abs()));
            let exterior_pressure = self
                .boundaries
                .iter()
                .filter(|b| b.open())
                .map(|b| b.pressure() as f64)
                .fold(0f64, f64::max);
            let h = remaining.min(0.025).min(
                0.65 / (vmax.max(exterior_speed) / voxel_size()
                    + ((9.81 / voxel_size() + exterior_pressure) * 0.65).sqrt()),
            );
            self.endpoints(h, &mut stats);
            self.substep(h, &mut stats);
            self.endpoints(0., &mut stats);
            remaining -= h;
            stats.simulated_dt += h;
            stats.substeps += 1;
            self.tick += 1;
        }
        stats.deferred_dt = remaining.max(0.);
        stats.mass = self.mass;
        stats
    }
    fn substep(&mut self, dt: f64, stats: &mut StepStats) {
        self.substep_with_gravity(dt, stats, 9.81);
    }
    fn substep_with_gravity(&mut self, dt: f64, stats: &mut StepStats, gravity: f32) {
        let n = self.flags.len();
        let h = dt as f32;
        let wet: Vec<usize> = (0..n).filter(|&i| self.flags[i] == 1).collect();
        let mut div = vec![0f32; n];
        let mut boundary_degree = vec![0u8; n];
        for b in &mut self.boundaries {
            if !b.open() {
                // Accounting backpressure temporarily withholds a reserved
                // whole unit. Keep partial displacement (not mass) so fair
                // quota scheduling cannot starve every inlet at small dt.
                // No velocity/credit is advanced while this face is closed.
                b.velocity = 0.;
                continue;
            }
            if self.flags[b.index] == 2 || (self.flags[b.index] != 1 && !b.exterior_wet()) {
                b.velocity = 0.;
                b.credit = 0.;
                continue;
            }
            boundary_degree[b.index] += 1;
            let wet_inside = self.flags[b.index] == 1;
            let wet_outside = b.exterior_wet();
            let count = u8::from(wet_inside) as f32 + u8::from(wet_outside) as f32;
            let exterior = if wet_outside {
                b.external_velocity()[b.condition.axis]
            } else {
                0.
            };
            let interior = if wet_inside {
                self.momentum[b.index][b.condition.axis]
            } else {
                0.
            };
            let mut v = b.sign() * (interior + exterior) / count / voxel_size() as f32;
            if b.condition.axis == 1 {
                v -= b.sign() * gravity / voxel_size() as f32 * h;
            }
            // Prescribed exterior pressure is external forcing. The pressure
            // solve below uses zero exterior pressure for its unknown gradient.
            v -= h * b.pressure();
            b.velocity = v;
            div[b.index] += v;
        }
        let stencils: Vec<_> = wet
            .iter()
            .map(|&i| {
                let mut neighbors = [usize::MAX; 6];
                let mut degree = boundary_degree[i];
                for a in 0..3 {
                    for (side, positive) in [false, true].into_iter().enumerate() {
                        if let Some(j) = self.neighbor(i, a, positive) {
                            if self.flags[j] != 2 {
                                neighbors[a * 2 + side] = j;
                                degree += 1;
                            }
                        }
                    }
                }
                (neighbors, degree)
            })
            .collect();
        let mut face_bits = vec![0u8; n];
        let mut active_faces = Vec::with_capacity(wet.len() * 4);
        for (wi, &i) in wet.iter().enumerate() {
            for (side, &j) in stencils[wi].0.iter().enumerate() {
                if j == usize::MAX {
                    continue;
                }
                let axis = side / 2;
                let positive = side % 2 == 1;
                let face = if positive { i } else { j };
                if face_bits[face] & (1 << axis) == 0 {
                    face_bits[face] |= 1 << axis;
                    active_faces.push((face, axis, if positive { j } else { i }));
                }
            }
        }
        active_faces.sort_unstable_by_key(|&(face, axis, _)| face * 3 + axis);
        for &(face, axis, _) in &self.active_faces {
            if face_bits[face] & (1 << axis) == 0 {
                self.velocity[face][axis] = 0.;
                self.credit[face][axis] = 0.;
            }
        }
        self.active_faces = active_faces;
        for &(i, a, j) in &self.active_faces {
            let count = u8::from(self.flags[i] == 1) as f32 + u8::from(self.flags[j] == 1) as f32;
            let mut v = (self.momentum[i][a] + self.momentum[j][a]) / count / voxel_size() as f32;
            if a == 1 {
                v -= gravity / voxel_size() as f32 * h;
            }
            self.velocity[i][a] = v;
            div[i] += v;
            div[j] -= v;
        }
        for i in 0..n {
            if self.flags[i] != 1 {
                self.pressure[i] = 0.;
            }
        }
        for sweep in 0..PRESSURE_SWEEPS {
            let mut largest_update = 0f32;
            // Alternating traversal removes persistent index preference.
            for k in 0..wet.len() {
                let wi = if (sweep as u64 + self.tick) % 2 == 0 {
                    k
                } else {
                    wet.len() - 1 - k
                };
                let i = wet[wi];
                let (neighbors, degree) = stencils[wi];
                let degree = degree as f32;
                let mut sum = 0.;
                for j in neighbors {
                    if j != usize::MAX {
                        sum += self.pressure[j];
                    }
                }
                if degree > 0. {
                    let p = ((sum - div[i] / h) / degree).max(0.);
                    let next = (self.pressure[i] + 1.3 * (p - self.pressure[i])).max(0.);
                    largest_update = largest_update.max((next - self.pressure[i]).abs());
                    self.pressure[i] = next;
                }
            }
            stats.pressure_sweeps += 1;
            if largest_update < 1e-4 {
                break;
            }
        }
        let mut residual = 0f64;
        for (wi, &i) in wet.iter().enumerate() {
            let mut lap = -(boundary_degree[i] as f32) * self.pressure[i];
            for j in stencils[wi].0 {
                if j != usize::MAX {
                    lap += self.pressure[j] - self.pressure[i];
                }
            }
            // Complementarity residual: nonnegative pressure allows expansion.
            let r = lap - div[i] / h;
            let r = if self.pressure[i] == 0. { r.max(0.) } else { r };
            residual += (r as f64).powi(2);
        }
        stats.pressure_residual_rms = stats
            .pressure_residual_rms
            .max((residual / wet.len().max(1) as f64).sqrt());
        // A bounded/inexact Poisson solve can point too far along a pressure
        // gradient after topology changes. Minimize weighted face energy along
        // that correction, instead of clipping speeds or adding arbitrary drag.
        // Weight=(adjacent wet cells)/2 gives the averaging/reconstruction pair
        // a kinetic-energy bound, including zero normal velocity at solid faces.
        let mut dot = 0f64;
        let mut norm = 0f64;
        let mut energy_before = 0f64;
        for &i in &wet {
            for a in 0..3 {
                for positive in [false, true] {
                    let Some(j) = self.neighbor(i, a, positive) else {
                        continue;
                    };
                    if self.flags[j] == 2 {
                        continue;
                    }
                    let face = if positive { i } else { j };
                    let correction = h
                        * (self.pressure[if positive { j } else { i }]
                            - self.pressure[if positive { i } else { j }]);
                    let v = self.velocity[face][a] as f64;
                    let g = correction as f64;
                    // Each wet incidence contributes half the face weight.
                    dot += 0.5 * v * g;
                    norm += 0.5 * g * g;
                    energy_before += 0.25 * (v * voxel_size()).powi(2);
                }
            }
        }
        for b in &self.boundaries {
            if boundary_degree[b.index] == 0 || !b.open() {
                continue;
            }
            let weight = 0.5
                * (u8::from(self.flags[b.index] == 1) as f64 + u8::from(b.exterior_wet()) as f64);
            let v = b.velocity as f64;
            let g = -(h * self.pressure[b.index]) as f64;
            dot += weight * v * g;
            norm += weight * g * g;
            energy_before += 0.5 * weight * (v * voxel_size()).powi(2);
        }
        let pressure_scale = if norm > 0. {
            (dot / norm).clamp(0., 1.)
        } else {
            1.
        };
        if pressure_scale < 0.999999 {
            stats.projection_reductions += 1;
        }
        stats.projection_energy_before += energy_before;
        let mut energy_after = 0f64;
        div.fill(0.);
        let mut candidates = Vec::new();
        for &(i, a, j) in &self.active_faces {
            let v = (self.velocity[i][a] as f64
                - pressure_scale * (h * (self.pressure[j] - self.pressure[i])) as f64)
                as f32;
            let weight =
                0.5 * (u8::from(self.flags[i] == 1) as f64 + u8::from(self.flags[j] == 1) as f64);
            energy_after += 0.5 * weight * (v as f64 * voxel_size()).powi(2);
            self.velocity[i][a] = v;
            stats.max_speed_mps = stats.max_speed_mps.max(v.abs() as f64 * voxel_size());
            div[i] += v;
            div[j] -= v;
            let c = &mut self.credit[i][a];
            *c += v * h;
            let (from, to, sign) = if *c >= 1. {
                (i, j, 1.)
            } else if *c <= -1. {
                (j, i, -1.)
            } else {
                continue;
            };
            if self.flags[from] == 1 {
                candidates.push(Move {
                    from,
                    to,
                    axis: a,
                    face: i,
                    sign,
                    score: c.abs(),
                });
            }
        }
        let mut boundary_normal = if self.boundaries.is_empty() {
            Vec::new()
        } else {
            vec![[0f32; 3]; n]
        };
        for (bi, b) in self.boundaries.iter_mut().enumerate() {
            if !b.open() || boundary_degree[b.index] == 0 {
                continue;
            }
            let proposed =
                (b.velocity as f64 + pressure_scale * (h * self.pressure[b.index]) as f64) as f32;
            let v = if b.accepts(proposed) {
                proposed
            } else {
                stats.boundary_blocked += 1;
                0.
            };
            b.velocity = v;
            boundary_normal[b.index][b.condition.axis] += b.sign() * v;
            div[b.index] += v;
            let weight = 0.5
                * (u8::from(self.flags[b.index] == 1) as f64 + u8::from(b.exterior_wet()) as f64);
            energy_after += 0.5 * weight * (v as f64 * voxel_size()).powi(2);
            stats.max_speed_mps = stats.max_speed_mps.max(v.abs() as f64 * voxel_size());
            b.credit += v * h;
            let (from, to, sign) =
                if b.credit >= 1. && self.flags[b.index] == 1 && b.condition.export_quota > 0 {
                    (b.index, n + bi * 2 + 1, 1.)
                } else if b.credit <= -1. && b.exterior_wet() && b.condition.import_quota > 0 {
                    (n + bi * 2, b.index, -1.)
                } else {
                    continue;
                };
            candidates.push(Move {
                from,
                to,
                axis: 0,
                face: n + bi,
                sign,
                score: b.credit.abs(),
            });
        }
        stats.projection_energy_after += energy_after;
        stats.divergence_rms = stats.divergence_rms.max(
            (wet.iter().map(|&i| (div[i] as f64).powi(2)).sum::<f64>() / wet.len().max(1) as f64)
                .sqrt(),
        );
        // Reconstruct projected cell momentum, then carry that vector with its
        // whole water unit through the simultaneous assignment below.
        for &i in &wet {
            for a in 0..3 {
                let mut sum = 0.;

                if let Some(j) = self.neighbor(i, a, true) {
                    if self.flags[j] != 2 {
                        sum += self.velocity[i][a];
                    }
                }
                if let Some(j) = self.neighbor(i, a, false) {
                    if self.flags[j] != 2 {
                        sum += self.velocity[j][a];
                    }
                }
                if !boundary_normal.is_empty() {
                    sum += boundary_normal[i][a];
                }
                // Both normal faces always participate: a missing/solid face
                // has zero velocity. Renormalizing over open faces doubles the
                // reconstructed wall-normal speed and breaks the energy bound.
                self.momentum[i][a] = 0.5 * sum * voxel_size() as f32;
            }
        }
        let mut assignment_flags = self.flags.clone();
        for _ in &self.boundaries {
            assignment_flags.extend([1, 0]);
        }
        let moves = resolve(&assignment_flags, &mut candidates, self.tick);
        stats.candidates += candidates.len();
        stats.rejected += candidates.len() - moves.len();
        stats.moved += moves.len();
        let mut accepted = vec![false; (n + self.boundaries.len()) * 3];
        for m in &moves {
            accepted[m.face * 3 + m.axis] = true;
        }
        let transported: Vec<_> = moves
            .iter()
            .map(|m| {
                if m.from < n {
                    self.momentum[m.from]
                } else {
                    {
                        let b = &self.boundaries[(m.from - n) / 2];
                        let mut v = b.external_velocity();
                        v[b.condition.axis] = b.sign() * b.velocity * voxel_size() as f32;
                        v
                    }
                }
            })
            .collect();
        for m in &moves {
            if m.from < n {
                self.flags[m.from] = 0;
                self.momentum[m.from] = [0.; 3];
                self.pressure[m.from] = 0.;
            }
        }
        for (m, momentum) in moves.iter().zip(transported) {
            if m.to < n {
                self.flags[m.to] = 1;
                self.momentum[m.to] = momentum;
            }
            if m.face < n {
                self.credit[m.face][m.axis] -= m.sign;
            } else {
                let b = &mut self.boundaries[m.face - n];
                b.credit -= m.sign;
                let into_detail = m.from >= n;
                if into_detail {
                    b.condition.import_quota -= 1;
                    self.mass += 1;
                    self.ledger.boundary_imported += 1;
                } else {
                    b.condition.export_quota -= 1;
                    self.mass -= 1;
                    self.ledger.boundary_exported += 1;
                }
                if let Some(g) = &mut b.ghost {
                    g.wet = !into_detail;
                    g.pressure = 0.;
                    g.velocity_mps = if into_detail { [0.; 3] } else { momentum };
                }
                stats.boundary_transfers.push(BoundaryTransfer {
                    boundary_id: b.id,
                    cell: b.condition.cell,
                    into_detail,
                    units: 1,
                    momentum_mps: momentum,
                });
            }
        }
        for m in &candidates {
            if !accepted[m.face * 3 + m.axis] {
                if m.face < n {
                    self.credit[m.face][m.axis] = 0.;
                } else {
                    self.boundaries[m.face - n].credit = 0.;
                }
            }
        }
        debug_assert_eq!(self.flags.iter().filter(|&&f| f == 1).count(), self.mass);
    }
    /// Complete bit-exact deterministic restart, including transported momentum.
    pub fn snapshot(&self) -> Vec<u8> {
        let mut b = b"DWPROD04".to_vec();
        for v in [
            self.origin.x as u64,
            self.origin.y as u64,
            self.origin.z as u64,
            self.dims[0] as u64,
            self.dims[1] as u64,
            self.dims[2] as u64,
            self.tick,
            self.mass as u64,
            self.ledger.added,
            self.ledger.removed,
            self.ledger.source_added,
            self.ledger.outlet_removed,
            self.ledger.source_suppressed,
            self.ledger.boundary_imported,
            self.ledger.boundary_exported,
            self.ledger.migration_imported,
            self.ledger.migration_exported,
            self.boundaries.len() as u64,
        ] {
            b.extend(v.to_le_bytes());
        }
        for i in 0..self.flags.len() {
            b.push(self.flags[i]);
            b.push(
                u8::from(self.sources.contains_key(&i)) | u8::from(self.outlets.contains(&i)) << 1,
            );
            for v in self.momentum[i]
                .into_iter()
                .chain([self.pressure[i]])
                .chain(self.velocity[i])
                .chain(self.credit[i])
            {
                b.extend(v.to_bits().to_le_bytes());
            }
        }
        for source in self.sources.values() {
            b.extend(source.rate.to_le_bytes());
            b.extend(source.phase.to_le_bytes());
        }
        for state in &self.boundaries {
            let c = state.condition;
            b.extend(state.id.to_le_bytes());
            b.extend((state.index as u64).to_le_bytes());
            b.push(c.axis as u8);
            b.push(u8::from(c.positive));
            b.extend(c.head_m.to_le_bytes());
            for v in c.velocity_mps {
                b.extend(v.to_bits().to_le_bytes());
            }
            b.extend(c.import_quota.to_le_bytes());
            b.extend(c.export_quota.to_le_bytes());
            b.extend(state.velocity.to_bits().to_le_bytes());
            b.extend(state.credit.to_bits().to_le_bytes());
            b.push(u8::from(state.ghost.is_some()));
            let ghost = state.ghost.unwrap_or(BoundaryGhost {
                wet: false,
                pressure: 0.,
                velocity_mps: [0.; 3],
            });
            b.push(u8::from(ghost.wet));
            b.extend(ghost.pressure.to_bits().to_le_bytes());
            for v in ghost.velocity_mps {
                b.extend(v.to_bits().to_le_bytes());
            }
        }
        let hash = checksum(&b);
        b.extend(hash.to_le_bytes());
        b
    }
    pub fn restore(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 120
            || (&bytes[..8] != b"DWPROD02"
                && &bytes[..8] != b"DWPROD03"
                && &bytes[..8] != b"DWPROD04")
        {
            return Err("unsupported/truncated water snapshot".into());
        }
        let fourth = &bytes[..8] == b"DWPROD04";
        let modern = &bytes[..8] == b"DWPROD03" || fourth;
        if modern && bytes.len() < if fourth { 160 } else { 144 } {
            return Err("truncated open water snapshot".into());
        }
        let header = if fourth {
            152
        } else if modern {
            136
        } else {
            112
        };
        let end = bytes.len() - 8;
        if checksum(&bytes[..end]) != u64::from_le_bytes(bytes[end..].try_into().unwrap()) {
            return Err("water snapshot checksum mismatch".into());
        }
        let mut pos = 8;
        let mut read64 = || {
            let v = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            v
        };
        let origin = Cell {
            x: read64() as i64,
            y: read64() as i64,
            z: read64() as i64,
        };
        let dims = [read64(), read64(), read64()];
        let mut w = Self::new(
            origin,
            dims.map(|d| usize::try_from(d).unwrap_or(usize::MAX)),
        )?;
        w.tick = read64();
        w.mass = usize::try_from(read64()).map_err(|_| "invalid water mass")?;
        w.ledger = Ledger {
            added: read64(),
            removed: read64(),
            source_added: read64(),
            outlet_removed: read64(),
            source_suppressed: read64(),
            boundary_imported: if modern { read64() } else { 0 },
            boundary_exported: if modern { read64() } else { 0 },
            migration_imported: if fourth { read64() } else { 0 },
            migration_exported: if fourth { read64() } else { 0 },
        };
        let boundary_count = if modern {
            usize::try_from(read64()).map_err(|_| "invalid boundary count")?
        } else {
            0
        };
        if boundary_count > 65536 {
            return Err("water boundary count exceeds budget".into());
        }
        if end < header + w.flags.len() * 42 {
            return Err("invalid water snapshot length".into());
        }
        for i in 0..w.flags.len() {
            w.flags[i] = bytes[pos];
            let endpoints = bytes[pos + 1];
            pos += 2;
            if w.flags[i] > 2 || endpoints > 2 || (endpoints != 0 && w.flags[i] == 2) {
                return Err("invalid water cell flags".into());
            }
            if endpoints == 1 {
                w.sources.insert(
                    i,
                    Source {
                        rate: 40.,
                        phase: 0.,
                    },
                );
            } else if endpoints == 2 {
                w.outlets.insert(i);
            }
            let mut fields = [0f32; 10];
            for v in &mut fields {
                *v = f32::from_bits(u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()));
                pos += 4;
                if !v.is_finite() {
                    return Err("nonfinite water numerical state".into());
                }
            }
            w.momentum[i].copy_from_slice(&fields[..3]);
            w.pressure[i] = fields[3];
            w.velocity[i].copy_from_slice(&fields[4..7]);
            w.credit[i].copy_from_slice(&fields[7..10]);
        }
        if end
            != header
                + w.flags.len() * 42
                + w.sources.len() * 16
                + boundary_count * if fourth { 80 } else { 62 }
        {
            return Err("invalid water source state length".into());
        }
        for source in w.sources.values_mut() {
            source.rate = f64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            source.phase = f64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            if !source.rate.is_finite()
                || source.rate <= 0.
                || source.rate > 1_000_000.
                || !source.phase.is_finite()
                || !(0. ..1.).contains(&source.phase)
            {
                return Err("invalid water source clock".into());
            }
        }
        for _ in 0..boundary_count {
            let id = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let index = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let axis = bytes[pos] as usize;
            let positive = bytes[pos + 1];
            pos += 2;
            let head_m = f64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let mut velocity_mps = [0f32; 3];
            for v in &mut velocity_mps {
                *v = f32::from_bits(u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()));
                pos += 4;
            }
            let import_quota = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let export_quota = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let velocity =
                f32::from_bits(u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()));
            pos += 4;
            let credit =
                f32::from_bits(u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()));
            pos += 4;
            if index >= w.flags.len() as u64
                || positive > 1
                || !velocity.is_finite()
                || !credit.is_finite()
                || w.boundary(id).is_some()
            {
                return Err("invalid/duplicate open water state".into());
            }
            w.set_boundary(
                id,
                OpenBoundary {
                    cell: w.cell(index as usize),
                    axis,
                    positive: positive == 1,
                    head_m,
                    velocity_mps,
                    import_quota,
                    export_quota,
                },
            )?;
            let b = w.boundaries.iter_mut().find(|b| b.id == id).unwrap();
            b.velocity = velocity;
            b.credit = credit;
            if fourth {
                let present = bytes[pos];
                let wet = bytes[pos + 1];
                pos += 2;
                let pressure =
                    f32::from_bits(u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()));
                pos += 4;
                let mut velocity_mps = [0f32; 3];
                for v in &mut velocity_mps {
                    *v =
                        f32::from_bits(u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()));
                    pos += 4;
                }
                if present > 1 || wet > 1 {
                    return Err("invalid exact ghost flags".into());
                }
                let ghost = BoundaryGhost {
                    wet: wet == 1,
                    pressure,
                    velocity_mps,
                };
                if present == 0
                    && ghost
                        != (BoundaryGhost {
                            wet: false,
                            pressure: 0.,
                            velocity_mps: [0.; 3],
                        })
                {
                    return Err("disabled exact ghost has state".into());
                }
                w.set_boundary_ghost(id, if present == 1 { Some(ghost) } else { None })?;
            }
        }
        if w.flags.iter().filter(|&&f| f == 1).count() != w.mass
            || w.ledger.added as u128
                + w.ledger.source_added as u128
                + w.ledger.boundary_imported as u128
                + w.ledger.migration_imported as u128
                != w.mass as u128
                    + w.ledger.removed as u128
                    + w.ledger.outlet_removed as u128
                    + w.ledger.boundary_exported as u128
                    + w.ledger.migration_exported as u128
        {
            return Err("invalid water ledger".into());
        }
        w.rebuild_active_face_cache();
        Ok(w)
    }
}
fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}
/// Injective matching plus dependency traversal. Empty-ended chains and cycles
/// publish atomically. Rotating total-order ties are deterministic across saves.
fn resolve(flags: &[u8], candidates: &mut [Move], tick: u64) -> Vec<Move> {
    let faces = flags.len() * 3;
    let offset = (tick.wrapping_mul(0x9e3779b97f4a7c15) % faces as u64) as usize;
    candidates.sort_unstable_by(|a, b| {
        b.score.total_cmp(&a.score).then_with(|| {
            ((a.face * 3 + a.axis + faces - offset) % faces)
                .cmp(&((b.face * 3 + b.axis + faces - offset) % faces))
        })
    });
    let mut next = vec![usize::MAX; flags.len()];
    let mut claimed = vec![false; flags.len()];
    for (index, &m) in candidates.iter().enumerate() {
        if next[m.from] == usize::MAX && !claimed[m.to] {
            next[m.from] = index;
            claimed[m.to] = true;
        }
    }
    let mut state = vec![0u8; flags.len()];
    for root in 0..flags.len() {
        if next[root] == usize::MAX || state[root] != 0 {
            continue;
        }
        let mut path = Vec::new();
        let mut at = root;
        let accept = loop {
            if flags[at] != 1 {
                break true;
            }
            match state[at] {
                1 | 2 => break true,
                3 => break false,
                _ => {}
            }
            if next[at] != usize::MAX {
                let m = candidates[next[at]];
                state[at] = 1;
                path.push(at);
                at = m.to;
            } else {
                break false;
            }
        };
        for i in path {
            state[i] = if accept { 2 } else { 3 };
        }
    }
    next.into_iter()
        .enumerate()
        .filter_map(|(i, m)| {
            if state[i] == 2 {
                Some(candidates[m])
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c(x: i64, y: i64, z: i64) -> Cell {
        Cell { x, y, z }
    }
    fn tank() -> DiscreteWater {
        let mut w = DiscreteWater::new(c(-4, -2, -3), [12, 18, 8]).unwrap();
        for z in -3..5 {
            for y in -2..16 {
                for x in -4..8 {
                    if x == -4 || x == 7 || y == -2 || z == -3 || z == 4 {
                        w.set_solid(c(x, y, z), true).unwrap();
                    }
                }
            }
        }
        w
    }
    fn check(w: &DiscreteWater) {
        let l = w.ledger();
        assert_eq!(
            w.mass() as u64
                + l.removed
                + l.outlet_removed
                + l.boundary_exported
                + l.migration_exported,
            l.added + l.source_added + l.boundary_imported + l.migration_imported
        );
        assert_eq!(w.occupied_cells().count(), w.mass());
    }
    #[test]
    fn still_water() {
        let mut w = tank();
        for z in -2..4 {
            for y in -1..3 {
                for x in -3..7 {
                    w.set_water(c(x, y, z), true).unwrap();
                }
            }
        }
        let initial: Vec<_> = w.occupied_cells().collect();
        for _ in 0..80 {
            w.step(0.025);
            check(&w);
        }
        assert_eq!(initial, w.occupied_cells().collect::<Vec<_>>());
    }
    #[test]
    fn waterfall_sources_outlets_and_restart() {
        let mut w = tank();
        w.set_source(c(0, 13, 0), true).unwrap();
        for x in -3..7 {
            for z in -2..4 {
                w.set_outlet(c(x, -1, z), true).unwrap();
            }
        }
        for _ in 0..120 {
            w.step(0.025);
            check(&w);
        }
        assert!(w.ledger().source_added > 20, "{:?}", w.ledger());
        assert!(w.ledger().outlet_removed > 0, "{:?}", w.ledger());
        let mut r = DiscreteWater::restore(&w.snapshot()).unwrap();
        for _ in 0..15 {
            w.step(0.025);
            r.step(0.025);
            assert_eq!(w.snapshot(), r.snapshot());
        }
    }
    #[test]
    fn sustained_waterfall_sixty_seconds_and_dam_remain_finite() {
        let mut waterfall = tank();
        waterfall.set_source(c(0, 13, 0), true).unwrap();
        for x in -3..7 {
            for z in -2..4 {
                waterfall.set_outlet(c(x, -1, z), true).unwrap();
            }
        }
        let mut max_residual = 0f64;
        let mut max_speed = 0f64;
        let mut total_moved = 0;
        for _ in 0..2400 {
            let stats = waterfall.step(0.025);
            check(&waterfall);
            assert!(stats.pressure_residual_rms.is_finite());
            assert!(stats.max_speed_mps.is_finite());
            assert!(waterfall.momentum.iter().flatten().all(|v| v.is_finite()));
            max_residual = max_residual.max(stats.pressure_residual_rms);
            max_speed = max_speed.max(stats.max_speed_mps);
            total_moved += stats.moved;
        }
        assert!(waterfall.ledger().source_added > 500);
        assert!(waterfall.ledger().outlet_removed > 500);
        assert!(total_moved > 1000);
        println!("60s waterfall: ledger={:?}, mass={}, moved={}, max_speed_mps={}, max_pressure_residual={}",waterfall.ledger(),waterfall.mass(),total_moved,max_speed,max_residual);
        let mut dam = tank();
        for x in -3..1 {
            for y in -1..10 {
                for z in -2..4 {
                    dam.set_water(c(x, y, z), true).unwrap();
                }
            }
        }
        let before: Vec<_> = dam.occupied_cells().collect();
        for _ in 0..1200 {
            let stats = dam.step(0.025);
            check(&dam);
            assert!(stats.pressure_residual_rms.is_finite());
            assert!(dam.momentum.iter().flatten().all(|v| v.is_finite()));
        }
        assert_ne!(before, dam.occupied_cells().collect::<Vec<_>>());
    }
    #[test]
    fn exhausted_tick_returns_time_without_mutation() {
        let mut w = DiscreteWater::new(Cell { x: 0, y: 0, z: 0 }, [2; 3]).unwrap();
        w.tick = u64::MAX;
        let mut restored = DiscreteWater::restore(&w.snapshot()).unwrap();
        let before = restored.snapshot();
        let stats = restored.step(0.1);
        assert_eq!(stats.simulated_dt, 0.);
        assert_eq!(stats.deferred_dt, 0.1);
        assert_eq!(restored.snapshot(), before);
    }
    #[test]
    fn edits_are_exact_or_rejected() {
        let mut w = tank();
        let at = c(0, 3, 0);
        w.set_water(at, true).unwrap();
        let before = w.snapshot();
        assert!(w.set_solid(at, true).is_err());
        assert_eq!(w.snapshot(), before);
        w.set_water(at, false).unwrap();
        w.set_solid(at, true).unwrap();
        assert!(w.set_water(at, true).is_err());
        check(&w);
    }
    #[test]
    fn closed_v2_save_migrates_without_changing_numerical_state() {
        let mut w = tank();
        w.set_water(c(0, 8, 0), true).unwrap();
        w.set_source(c(1, 12, 0), true).unwrap();
        w.step(0.017);
        let modern = w.snapshot();
        let mut legacy = modern[..112].to_vec();
        legacy[..8].copy_from_slice(b"DWPROD02");
        legacy.extend_from_slice(&modern[152..modern.len() - 8]);
        legacy.extend(checksum(&legacy).to_le_bytes());
        let r = DiscreteWater::restore(&legacy).unwrap();
        assert_eq!(r.snapshot(), modern);
    }
    #[test]
    fn open_v3_save_migrates_boundary_fields_without_ghosts() {
        let at = Cell { x: 0, y: 0, z: 0 };
        let mut w = DiscreteWater::new(at, [2, 1, 1]).unwrap();
        w.set_boundary(
            7,
            OpenBoundary {
                cell: at,
                axis: 0,
                positive: false,
                head_m: 0.2,
                velocity_mps: [0.25, 0., 0.],
                import_quota: 2,
                export_quota: 0,
            },
        )
        .unwrap();
        w.step(0.017);
        let modern = w.snapshot();
        let mut legacy = modern[..128].to_vec();
        legacy[..8].copy_from_slice(b"DWPROD03");
        legacy.extend_from_slice(&modern[144..152]);
        let boundaries_start = 152 + w.flags.len() * 42 + w.sources.len() * 16;
        legacy.extend_from_slice(&modern[152..boundaries_start]);
        for b in modern[boundaries_start..modern.len() - 8].chunks_exact(80) {
            legacy.extend_from_slice(&b[..62]);
        }
        legacy.extend(checksum(&legacy).to_le_bytes());
        let r = DiscreteWater::restore(&legacy).unwrap();
        assert_eq!(r.snapshot(), modern);
    }
    #[test]
    fn restart_rejects_corruption() {
        let w = tank();
        let mut bytes = w.snapshot();
        bytes[150] ^= 1;
        assert!(DiscreteWater::restore(&bytes).is_err());
        for n in 0..112 {
            assert!(DiscreteWater::restore(&bytes[..n]).is_err());
        }
    }
    #[test]
    fn adaptive_budget_has_no_hidden_speed_limit() {
        let mut w = tank();
        w.set_water(c(0, 10, 0), true).unwrap();
        let i = w.index(c(0, 10, 0)).unwrap();
        w.momentum[i] = [0., -20., 0.];
        let stats = w.step(10.);
        assert_eq!(stats.substeps, MAX_SUBSTEPS);
        assert!(stats.deferred_dt > 0.);
        assert!(stats.max_speed_mps > 2.);
        check(&w);
    }
    #[test]
    fn inexact_pressure_and_atomic_transport_do_not_create_kinetic_energy() {
        let mut w = tank();
        for z in -2..4 {
            for y in -1..9 {
                for x in -3..5 {
                    if (x + y + z) % 5 != 0 {
                        w.set_water(c(x, y, z), true).unwrap();
                    }
                }
            }
        }
        for i in 0..w.flags.len() {
            if w.flags[i] == 1 {
                w.momentum[i] = [
                    (((i * 17) % 31) as f32 - 15.) / 10.,
                    (((i * 7) % 23) as f32 - 11.) / 10.,
                    (((i * 11) % 19) as f32 - 9.) / 10.,
                ];
                // Deliberately hostile warm-start from an obsolete topology.
                w.pressure[i] = if i % 3 == 0 { 1e7 } else { 0. };
            }
        }
        let energy = |w: &DiscreteWater| {
            w.momentum
                .iter()
                .flatten()
                .map(|&v| 0.5 * (v as f64).powi(2))
                .sum::<f64>()
        };
        let mut previous = energy(&w);
        let mut reductions = 0;
        for _ in 0..160 {
            let mut stats = StepStats::default();
            w.substep_with_gravity(0.002, &mut stats, 0.);
            w.tick += 1;
            check(&w);
            assert!(
                stats.projection_energy_after
                    <= stats.projection_energy_before * (1. + 1e-6) + 1e-9,
                "pressure added energy: {:?}",
                stats
            );
            let next = energy(&w);
            assert!(
                next <= previous * (1. + 1e-6) + 1e-9,
                "transport/reconstruction added energy: {previous} -> {next}"
            );
            previous = next;
            reductions += stats.projection_reductions;
        }
        assert!(
            reductions > 0,
            "test must exercise inexact projection correction"
        );
    }
    #[test]
    fn source_clock_is_partition_independent_and_never_banks_blockage() {
        fn blocked_source() -> DiscreteWater {
            let mut w = DiscreteWater::new(c(0, 0, 0), [3, 3, 3]).unwrap();
            for z in 0..3 {
                for y in 0..3 {
                    for x in 0..3 {
                        if c(x, y, z) != c(1, 1, 1) {
                            w.set_solid(c(x, y, z), true).unwrap();
                        }
                    }
                }
            }
            w.set_source_rate(c(1, 1, 1), 7.).unwrap();
            w
        }
        let mut a = blocked_source();
        let mut b = blocked_source();
        for _ in 0..100 {
            a.step(0.01);
        }
        for _ in 0..1000 {
            b.step(0.001);
        }
        assert_eq!(a.ledger(), b.ledger());
        assert_eq!(a.ledger().source_added, 1);
        assert_eq!(a.ledger().source_suppressed, 6);
        let mut r = DiscreteWater::restore(&a.snapshot()).unwrap();
        a.set_water(c(1, 1, 1), false).unwrap();
        r.set_water(c(1, 1, 1), false).unwrap();
        a.step(0.001);
        r.step(0.001);
        assert_eq!(a.mass(), 0, "blocked source accumulated burst");
        assert_eq!(a.snapshot(), r.snapshot());
    }
    #[test]
    fn freefall_accelerates_past_two_meters_per_second() {
        let mut w = DiscreteWater::new(c(0, 0, 0), [3, 100, 3]).unwrap();
        w.set_water(c(1, 90, 1), true).unwrap();
        let mut max_speed = 0f64;
        for _ in 0..24 {
            max_speed = max_speed.max(w.step(0.025).max_speed_mps);
            check(&w);
        }
        assert!(max_speed > 4., "freefall slowed to {max_speed}");
        assert!(w.occupied_cells().next().unwrap().y < 80);
    }
    #[test]
    fn simultaneous_chains_cycles_blockage() {
        let m = |from, to, face| Move {
            from,
            to,
            face,
            axis: 0,
            sign: 1.,
            score: 1.,
        };
        assert_eq!(
            resolve(&[1, 1, 0], &mut [m(0, 1, 0), m(1, 2, 1)], 0).len(),
            2
        );
        assert_eq!(resolve(&[1, 1], &mut [m(0, 1, 0), m(1, 0, 1)], 0).len(), 2);
        assert!(resolve(&[1, 1, 1], &mut [m(0, 1, 0), m(1, 2, 1)], 0).is_empty());
    }
}
