//! Explicit prescribed-head ports on a fixed detailed domain. Quotas must be
//! reserved by the caller before stepping and reconciled with accepted reports.
use super::{Cell, DiscreteWater, VOXEL_SIZE};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpenBoundary {
    pub cell: Cell,
    pub axis: usize,
    pub positive: bool,
    /// Global stage, quantized to the solver's first-dry-cell-center pressure
    /// convention. See module README for the explicit half-cell distinction.
    pub head_m: f64,
    pub velocity_mps: [f32; 3],
    pub import_quota: u64,
    pub export_quota: u64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundaryTransfer {
    pub boundary_id: u64,
    pub cell: Cell,
    pub into_detail: bool,
    pub units: u64,
    pub momentum_mps: [f32; 3],
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundaryGhost {
    pub wet: bool,
    /// Same cell-coordinate pressure units as NativeCellState::pressure.
    pub pressure: f32,
    pub velocity_mps: [f32; 3],
}
impl BoundaryGhost {
    pub(super) fn validate(self) -> Result<(), String> {
        if !self.pressure.is_finite()
            || self.pressure < 0.
            || self.velocity_mps.iter().any(|v| !v.is_finite())
            || (!self.wet && (self.pressure != 0. || self.velocity_mps.iter().any(|&v| v != 0.)))
        {
            return Err("invalid exact boundary ghost".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub(super) struct BoundaryState {
    pub(super) id: u64,
    pub(super) ghost: Option<BoundaryGhost>,
    pub(super) condition: OpenBoundary,
    pub(super) index: usize,
    /// Outward normal velocity in cells/second and displacement in cells.
    pub(super) velocity: f32,
    pub(super) credit: f32,
}
impl BoundaryState {
    pub(super) fn pressure(&self) -> f32 {
        if let Some(g) = self.ghost {
            return g.pressure;
        }
        let exterior_y = self.condition.cell.y as f64
            + if self.condition.axis == 1 {
                if self.condition.positive {
                    1.
                } else {
                    -1.
                }
            } else {
                0.
            };
        // The interior binary free surface fixes p=0 at the first dry
        // cell CENTER. Quantize the supplied stage to that identical lattice
        // convention; using continuum p=g*(head-y) creates a half-cell seam.
        let first_dry_y = (self.condition.head_m / VOXEL_SIZE - 0.5).ceil();
        (9.81 / VOXEL_SIZE * (first_dry_y - exterior_y).max(0.)) as f32
    }
    pub(super) fn exterior_wet(&self) -> bool {
        self.ghost.map_or_else(|| self.pressure() > 0., |g| g.wet)
    }
    pub(super) fn external_velocity(&self) -> [f32; 3] {
        self.ghost
            .map_or(self.condition.velocity_mps, |g| g.velocity_mps)
    }
    pub(super) fn open(&self) -> bool {
        self.condition.import_quota > 0 || self.condition.export_quota > 0
    }
    pub(super) fn sign(&self) -> f32 {
        if self.condition.positive {
            1.
        } else {
            -1.
        }
    }
    pub(super) fn accepts(&self, velocity: f32) -> bool {
        if velocity < 0. {
            self.condition.import_quota > 0 && self.exterior_wet()
        } else {
            self.condition.export_quota > 0 && self.ghost.is_none_or(|g| !g.wet)
        }
    }
}
impl DiscreteWater {
    /// Configure a physical face leading outside this fixed domain. Supplying
    /// the same ID updates head/quotas while retaining its numerical face state.
    pub fn set_boundary(&mut self, id: u64, condition: OpenBoundary) -> Result<(), String> {
        let index = self
            .index(condition.cell)
            .ok_or("boundary cell outside detailed domain")?;
        if condition.axis >= 3
            || self
                .neighbor(index, condition.axis, condition.positive)
                .is_some()
            || self.flags[index] == 2
            || self.sources.contains_key(&index)
            || self.outlets.contains(&index)
        {
            return Err("open boundary must face outside a nonsolid domain cell".into());
        }
        if !condition.head_m.is_finite()
            || condition.head_m.abs() > 1e9
            || condition.velocity_mps.iter().any(|v| !v.is_finite())
        {
            return Err("invalid exterior water boundary state".into());
        }
        if self.boundaries.iter().any(|b| {
            b.id != id
                && b.index == index
                && b.condition.axis == condition.axis
                && b.condition.positive == condition.positive
        }) {
            return Err("duplicate physical boundary face".into());
        }
        let state = BoundaryState {
            id,
            ghost: None,
            condition,
            index,
            velocity: 0.,
            credit: 0.,
        };
        match self.boundaries.binary_search_by_key(&id, |b| b.id) {
            Ok(i) => {
                let old = &mut self.boundaries[i];
                if old.index != index
                    || old.condition.axis != condition.axis
                    || old.condition.positive != condition.positive
                {
                    return Err("boundary ID already names another face".into());
                }
                old.condition = condition;
            }
            Err(i) => {
                if self.boundaries.len() >= 65536 {
                    return Err("open boundary budget exhausted".into());
                }
                self.boundaries.insert(i, state);
            }
        }
        Ok(())
    }
    pub fn set_boundary_ghost(
        &mut self,
        id: u64,
        ghost: Option<BoundaryGhost>,
    ) -> Result<(), String> {
        if let Some(g) = ghost {
            g.validate()?;
        }
        let b = self
            .boundaries
            .iter_mut()
            .find(|b| b.id == id)
            .ok_or("unknown exact boundary ghost port")?;
        b.ghost = ghost;
        Ok(())
    }
    pub fn boundary_ghost(&self, id: u64) -> Option<BoundaryGhost> {
        self.boundaries
            .iter()
            .find(|b| b.id == id)
            .and_then(|b| b.ghost)
    }
    pub fn boundaries(&self) -> impl Iterator<Item = (u64, OpenBoundary)> + '_ {
        self.boundaries.iter().map(|b| (b.id, b.condition))
    }
    pub fn has_internal_endpoints(&self) -> bool {
        !self.sources.is_empty() || !self.outlets.is_empty()
    }
    pub fn boundary(&self, id: u64) -> Option<OpenBoundary> {
        self.boundaries
            .binary_search_by_key(&id, |b| b.id)
            .ok()
            .map(|i| self.boundaries[i].condition)
    }
    pub fn remove_boundary(&mut self, id: u64) -> bool {
        if let Ok(i) = self.boundaries.binary_search_by_key(&id, |b| b.id) {
            self.boundaries.remove(i);
            true
        } else {
            false
        }
    }
    pub fn is_boundary_cell(&self, c: Cell) -> bool {
        self.index(c)
            .is_some_and(|i| self.boundaries.iter().any(|b| b.index == i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c(x: i64, y: i64, z: i64) -> Cell {
        Cell { x, y, z }
    }
    fn lake() -> DiscreteWater {
        let mut w = DiscreteWater::new(c(0, 0, 0), [8, 8, 5]).unwrap();
        for x in 0..8 {
            for y in 0..8 {
                for z in 0..5 {
                    if x == 7 || y == 0 || z == 0 || z == 4 {
                        w.set_solid(c(x, y, z), true).unwrap();
                    } else if y < 4 {
                        w.set_water(c(x, y, z), true).unwrap();
                    }
                }
            }
        }
        for y in 1..4 {
            for z in 1..4 {
                w.set_boundary(
                    (y * 10 + z) as u64,
                    OpenBoundary {
                        cell: c(0, y, z),
                        axis: 0,
                        positive: false,
                        head_m: 4. * VOXEL_SIZE,
                        velocity_mps: [0.; 3],
                        import_quota: 10000,
                        export_quota: 10000,
                    },
                )
                .unwrap();
            }
        }
        w
    }
    fn check(w: &DiscreteWater) {
        let l = w.ledger();
        assert_eq!(
            l.added + l.source_added + l.boundary_imported,
            w.mass() as u64 + l.removed + l.outlet_removed + l.boundary_exported
        );
    }
    #[test]
    fn prescribed_head_lake_seam_stays_still() {
        let mut w = lake();
        let initial: Vec<_> = w.occupied_cells().collect();
        let mut transfers = 0;
        let mut last_speed = 0.;
        for _ in 0..120 {
            let s = w.step(0.025);
            transfers += s.boundary_transfers.len();
            last_speed = s.max_speed_mps;
            check(&w);
        }
        assert_eq!(initial, w.occupied_cells().collect::<Vec<_>>());
        assert_eq!(transfers, 0);
        assert!(last_speed < 0.01, "lake seam speed {last_speed}");
    }
    #[test]
    fn accepted_exchange_conserves_and_respects_finite_quotas() {
        let mut w = DiscreteWater::new(c(0, 0, 0), [8, 5, 3]).unwrap();
        for x in 0..8 {
            for z in 0..3 {
                w.set_solid(c(x, 0, z), true).unwrap();
            }
        }
        let boundary = OpenBoundary {
            cell: c(0, 3, 1),
            axis: 0,
            positive: false,
            head_m: 5. * VOXEL_SIZE,
            velocity_mps: [2., 0., 0.],
            import_quota: 7,
            export_quota: 0,
        };
        w.set_boundary(9, boundary).unwrap();
        assert!(w.set_source(boundary.cell, true).is_err());
        assert!(w.set_boundary(10, boundary).is_err());
        let mut accepted = 0;
        for _ in 0..200 {
            let s = w.step(0.025);
            for t in s.boundary_transfers {
                assert!(t.into_detail);
                assert_eq!(t.boundary_id, 9);
                accepted += t.units;
            }
            check(&w);
        }
        assert_eq!(accepted, 7);
        assert_eq!(w.mass(), 7);
        assert_eq!(w.boundary(9).unwrap().import_quota, 0);
        assert_eq!(w.ledger().source_added, 0);
        let mut r = DiscreteWater::restore(&w.snapshot()).unwrap();
        for _ in 0..10 {
            w.step(0.025);
            r.step(0.025);
            assert_eq!(w.snapshot(), r.snapshot());
        }
    }
    #[test]
    fn export_pressure_port_reports_only_real_cells() {
        let mut w = DiscreteWater::new(c(0, 0, 0), [3, 4, 3]).unwrap();
        let at = c(1, 0, 1);
        w.set_water(at, true).unwrap();
        w.set_boundary(
            1,
            OpenBoundary {
                cell: at,
                axis: 1,
                positive: false,
                head_m: -1.,
                velocity_mps: [0.; 3],
                import_quota: 0,
                export_quota: 1,
            },
        )
        .unwrap();
        let mut transfers = 0;
        for _ in 0..80 {
            let s = w.step(0.025);
            for t in s.boundary_transfers {
                assert!(!t.into_detail);
                transfers += t.units;
            }
            check(&w);
        }
        assert_eq!(transfers, 1);
        assert_eq!(w.mass(), 0);
        assert_eq!(w.ledger().boundary_exported, 1);
    }
}

#[cfg(test)]
mod exact_ghost_tests {
    use super::*;
    #[test]
    fn exact_ghost_cannot_duplicate_or_overwrite_a_whole_cell_even_with_excess_quota() {
        let at = Cell { x: 0, y: 0, z: 0 };
        let mut sim = DiscreteWater::new(at, [2, 1, 1]).unwrap();
        sim.set_boundary(
            1,
            OpenBoundary {
                cell: at,
                axis: 0,
                positive: false,
                head_m: 100.,
                velocity_mps: [10., 0., 0.],
                import_quota: 10,
                export_quota: 0,
            },
        )
        .unwrap();
        sim.set_boundary_ghost(
            1,
            Some(BoundaryGhost {
                wet: false,
                pressure: 0.,
                velocity_mps: [0.; 3],
            }),
        )
        .unwrap();
        for _ in 0..20 {
            assert!(sim.step(0.025).boundary_transfers.is_empty());
        }
        assert_eq!(sim.mass(), 0);
        sim.set_boundary_ghost(
            1,
            Some(BoundaryGhost {
                wet: true,
                pressure: 100.,
                velocity_mps: [2., 0., 0.],
            }),
        )
        .unwrap();
        let mut imported = 0;
        for _ in 0..100 {
            imported += sim
                .step(0.025)
                .boundary_transfers
                .iter()
                .filter(|t| t.into_detail)
                .map(|t| t.units)
                .sum::<u64>();
        }
        assert_eq!(imported, 1);
        assert_eq!(sim.mass(), 1);
        assert!(!sim.boundary_ghost(1).unwrap().wet);
        let mut one = DiscreteWater::new(at, [1; 3]).unwrap();
        one.set_water(at, true).unwrap();
        one.set_boundary(
            1,
            OpenBoundary {
                cell: at,
                axis: 1,
                positive: false,
                head_m: -1.,
                velocity_mps: [0.; 3],
                import_quota: 0,
                export_quota: 10,
            },
        )
        .unwrap();
        one.set_boundary_ghost(
            1,
            Some(BoundaryGhost {
                wet: true,
                pressure: 0.,
                velocity_mps: [0.; 3],
            }),
        )
        .unwrap();
        for _ in 0..30 {
            assert!(one.step(0.025).boundary_transfers.is_empty());
        }
        assert_eq!(one.mass(), 1);
        one.set_boundary_ghost(
            1,
            Some(BoundaryGhost {
                wet: false,
                pressure: 0.,
                velocity_mps: [0.; 3],
            }),
        )
        .unwrap();
        let mut exported = 0;
        for _ in 0..80 {
            exported += one
                .step(0.025)
                .boundary_transfers
                .iter()
                .filter(|t| !t.into_detail)
                .map(|t| t.units)
                .sum::<u64>();
        }
        assert_eq!(exported, 1);
        assert_eq!(one.mass(), 0);
        assert!(one.boundary_ghost(1).unwrap().wet);
        assert_eq!(
            DiscreteWater::restore(&one.snapshot()).unwrap().snapshot(),
            one.snapshot()
        );
    }
}
