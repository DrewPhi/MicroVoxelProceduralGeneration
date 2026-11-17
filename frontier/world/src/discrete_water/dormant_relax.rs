//! Event-driven quasistatic native-water routing outside the detailed solver.
//! The planner never mutates cells, allocates world tiles, or creates water.
//! Its pressure datum is guidance, not a converged fluid projection.
use super::NativeCellState;
use crate::Cell;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum RelaxEvent {
    ImportVacancy(Cell),
    ExportFilled(Cell),
}
impl RelaxEvent {
    pub fn cell(self) -> Cell {
        match self {
            Self::ImportVacancy(c) | Self::ExportFilled(c) => c,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RelaxPending {
    BudgetExhausted = 0,
    UnknownBoundary = 1,
    NoSurfaceDonor = 2,
    NoLowerDestination = 3,
    StaleEvent = 4,
    InvalidState = 5,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RelaxMove {
    pub from: Cell,
    pub to: Cell,
    pub momentum_mps: [f32; 3],
}
#[derive(Clone, Debug, PartialEq)]
pub struct RelaxPlan {
    /// Ordered so a sequential applier always encounters an empty destination.
    /// The same list is also a legal simultaneous injective assignment.
    pub moves: Vec<RelaxMove>,
    pub potential_delta_cells: i128,
    /// First-dry-cell y datum for estimating pressure in newly filled cells.
    /// Do not advect pressure with momentum or overwrite untouched wet fields.
    pub equilibrium_head_y: i128,
    pub visited: usize,
}
#[derive(Clone, Debug, PartialEq)]
pub enum RelaxOutcome {
    Ready(RelaxPlan),
    Pending(RelaxPending),
}

struct Search<'a, F> {
    query: F,
    budget: &'a mut usize,
    memo: BTreeMap<Cell, Option<NativeCellState>>,
    unknown: bool,
    exhausted: bool,
    invalid: bool,
}
impl<F: FnMut(Cell) -> Option<NativeCellState>> Search<'_, F> {
    fn cell(&mut self, c: Cell) -> Option<NativeCellState> {
        if let Some(v) = self.memo.get(&c) {
            return *v;
        }
        if *self.budget == 0 {
            self.exhausted = true;
            return None;
        }
        *self.budget -= 1;
        let mut state = (self.query)(c);
        if state.is_none() {
            self.unknown = true;
        }
        if state.is_some_and(|s| {
            s.water && s.solid
                || s.momentum_mps.iter().any(|v| !v.is_finite())
                || !s.pressure.is_finite()
                || s.pressure < 0.
                || (!s.water && s.momentum_mps.iter().any(|&v| v != 0.))
        }) {
            self.invalid = true;
            state = None;
        }
        self.memo.insert(c, state);
        state
    }
    fn pending(&self, fallback: RelaxPending) -> RelaxOutcome {
        RelaxOutcome::Pending(if self.invalid {
            RelaxPending::InvalidState
        } else if self.exhausted {
            RelaxPending::BudgetExhausted
        } else if self.unknown {
            RelaxPending::UnknownBoundary
        } else {
            fallback
        })
    }
    fn momentum(&self, c: Cell) -> [f32; 3] {
        self.memo[&c].unwrap().momentum_mps
    }
}
fn adjacent(c: Cell) -> [Option<Cell>; 6] {
    [
        c.x.checked_sub(1).map(|x| Cell { x, ..c }),
        c.x.checked_add(1).map(|x| Cell { x, ..c }),
        c.y.checked_sub(1).map(|y| Cell { y, ..c }),
        c.y.checked_add(1).map(|y| Cell { y, ..c }),
        c.z.checked_sub(1).map(|z| Cell { z, ..c }),
        c.z.checked_add(1).map(|z| Cell { z, ..c }),
    ]
}
fn import_path<F: FnMut(Cell) -> Option<NativeCellState>>(
    search: &Search<'_, F>,
    parents: &BTreeMap<Cell, Cell>,
    start: Cell,
    donor: Cell,
    visited: usize,
) -> RelaxPlan {
    let mut path = vec![donor];
    while *path.last().unwrap() != start {
        path.push(parents[path.last().unwrap()]);
    }
    let moves = path
        .windows(2)
        .rev()
        .map(|p| RelaxMove {
            from: p[0],
            to: p[1],
            momentum_mps: search.momentum(p[0]),
        })
        .collect();
    RelaxPlan {
        moves,
        potential_delta_cells: start.y as i128 - donor.y as i128,
        equilibrium_head_y: donor.y as i128 + 1,
        visited,
    }
}
fn export_path<F: FnMut(Cell) -> Option<NativeCellState>>(
    search: &Search<'_, F>,
    parents: &BTreeMap<Cell, Cell>,
    start: Cell,
    last: Cell,
    destination: Cell,
    visited: usize,
) -> RelaxPlan {
    let mut path = vec![last];
    while *path.last().unwrap() != start {
        path.push(parents[path.last().unwrap()]);
    }
    let mut moves = vec![RelaxMove {
        from: last,
        to: destination,
        momentum_mps: search.momentum(last),
    }];
    moves.extend(path.windows(2).map(|p| RelaxMove {
        from: p[1],
        to: p[0],
        momentum_mps: search.momentum(p[1]),
    }));
    RelaxPlan {
        moves,
        potential_delta_cells: destination.y as i128 - start.y as i128,
        equilibrium_head_y: start.y as i128 + 1,
        visited,
    }
}
/// Query must return None for the active detailed cuboid, unknown/unmaterialized
/// cells and other forbidden ownership. Every distinct queried cell (including
/// surface probes and unknown cells) consumes one unit of the SHARED budget.
/// Known cells are memoized per call. Unknowns never become air or water.
///
/// Breadth-first paths minimize hop count. At equal completed search depth,
/// imports prefer higher surface donors; exports prefer lower dry destinations;
/// coordinates break ties. A valid candidate already established before budget
/// exhaustion may be returned without exploring a larger/unknown region.
/// No units move until the caller applies the returned plan atomically together
/// with regional transfers. Retain Pending events rather than fabricating supply.
pub fn plan(
    event: RelaxEvent,
    query: impl FnMut(Cell) -> Option<NativeCellState>,
    budget: &mut usize,
) -> RelaxOutcome {
    let initial_budget = *budget;
    let start = event.cell();
    let importing = matches!(event, RelaxEvent::ImportVacancy(_));
    let mut search = Search {
        query,
        budget,
        memo: BTreeMap::new(),
        unknown: false,
        exhausted: false,
        invalid: false,
    };
    let Some(cell) = search.cell(start) else {
        return search.pending(RelaxPending::StaleEvent);
    };
    if cell.solid || cell.water == importing {
        return RelaxOutcome::Pending(RelaxPending::StaleEvent);
    }
    let mut parents = BTreeMap::<Cell, Cell>::new();
    parents.insert(start, start);
    let mut frontier = vec![start];
    while !frontier.is_empty() {
        if importing {
            frontier.sort_unstable_by(|a, b| b.y.cmp(&a.y).then(a.cmp(b)));
            for &at in &frontier {
                if at == start || at.y < start.y {
                    continue;
                }
                let above = at.y.checked_add(1).map(|y| Cell { y, ..at });
                if let Some(above) = above {
                    if search.cell(above).is_some_and(|s| !s.water && !s.solid) {
                        if search.invalid {
                            return search.pending(RelaxPending::InvalidState);
                        }
                        return RelaxOutcome::Ready(import_path(
                            &search,
                            &parents,
                            start,
                            at,
                            initial_budget - *search.budget,
                        ));
                    }
                } else {
                    search.unknown = true;
                }
            }
        } else {
            frontier.sort_unstable();
            let mut candidates = Vec::new();
            for &at in &frontier {
                for neighbor in adjacent(at) {
                    let Some(next) = neighbor else {
                        search.unknown = true;
                        continue;
                    };
                    if next.y > start.y {
                        continue;
                    }
                    if search.cell(next).is_some_and(|s| !s.water && !s.solid) {
                        candidates.push((next, at));
                    }
                }
            }
            candidates
                .sort_unstable_by(|(a, af), (b, bf)| a.y.cmp(&b.y).then(a.cmp(b)).then(af.cmp(bf)));
            if let Some(&(destination, last)) = candidates.first() {
                if search.invalid {
                    return search.pending(RelaxPending::InvalidState);
                }
                return RelaxOutcome::Ready(export_path(
                    &search,
                    &parents,
                    start,
                    last,
                    destination,
                    initial_budget - *search.budget,
                ));
            }
        }
        if search.exhausted || search.invalid {
            return search.pending(RelaxPending::BudgetExhausted);
        }
        let mut next_frontier = BTreeSet::new();
        for &at in &frontier {
            for neighbor in adjacent(at) {
                let Some(next) = neighbor else {
                    search.unknown = true;
                    continue;
                };
                if parents.contains_key(&next) {
                    continue;
                }
                if search.cell(next).is_some_and(|s| s.water && !s.solid) {
                    parents.insert(next, at);
                    next_frontier.insert(next);
                }
            }
        }
        if search.exhausted || search.invalid {
            return search.pending(RelaxPending::BudgetExhausted);
        }
        frontier = next_frontier.into_iter().collect();
    }
    search.pending(if importing {
        RelaxPending::NoSurfaceDonor
    } else {
        RelaxPending::NoLowerDestination
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c(x: i64, y: i64, z: i64) -> Cell {
        Cell { x, y, z }
    }
    fn wet(v: f32) -> NativeCellState {
        NativeCellState {
            water: true,
            momentum_mps: [v, -0.5 * v, 0.25 * v],
            ..Default::default()
        }
    }
    fn solid() -> NativeCellState {
        NativeCellState {
            solid: true,
            ..Default::default()
        }
    }
    fn outcome(event: RelaxEvent, cells: &BTreeMap<Cell, NativeCellState>) -> RelaxPlan {
        let mut budget = 256;
        match plan(
            event,
            |c| Some(cells.get(&c).copied().unwrap_or_else(solid)),
            &mut budget,
        ) {
            RelaxOutcome::Ready(p) => p,
            other => panic!("expected legal chain, got {other:?}"),
        }
    }
    fn metrics(cells: &BTreeMap<Cell, NativeCellState>) -> (usize, i128, f64) {
        let wet: Vec<_> = cells.iter().filter(|(_, s)| s.water).collect();
        (
            wet.len(),
            wet.iter().map(|(c, _)| c.y as i128).sum(),
            wet.iter()
                .flat_map(|(_, s)| s.momentum_mps)
                .map(|v| 0.5 * (v as f64).powi(2))
                .sum(),
        )
    }
    fn apply(cells: &mut BTreeMap<Cell, NativeCellState>, plan: &RelaxPlan) {
        let mut sources = BTreeSet::new();
        let mut targets = BTreeSet::new();
        for m in &plan.moves {
            assert!(sources.insert(m.from));
            assert!(targets.insert(m.to));
            assert_eq!(
                (m.from.x as i128 - m.to.x as i128).abs()
                    + (m.from.y as i128 - m.to.y as i128).abs()
                    + (m.from.z as i128 - m.to.z as i128).abs(),
                1
            );
            let from = *cells.get(&m.from).unwrap();
            let to = *cells.get(&m.to).unwrap();
            assert!(from.water && !from.solid);
            assert!(!to.water && !to.solid);
            assert_eq!(from.momentum_mps, m.momentum_mps);
            let src = cells.get_mut(&m.from).unwrap();
            src.water = false;
            src.momentum_mps = [0.; 3];
            src.pressure = 0.;
            let dst = cells.get_mut(&m.to).unwrap();
            dst.water = true;
            dst.momentum_mps = m.momentum_mps;
        }
    }
    #[test]
    fn import_uses_known_surface_chain_without_creating_mass_or_kinetic_energy() {
        let vacancy = c(0, 0, 0);
        let near = c(1, 0, 0);
        let donor = c(1, 1, 0);
        let air = c(1, 2, 0);
        let mut cells = BTreeMap::from([
            (vacancy, NativeCellState::default()),
            (near, wet(1.)),
            (donor, wet(2.)),
            (air, NativeCellState::default()),
        ]);
        let before = metrics(&cells);
        let p = outcome(RelaxEvent::ImportVacancy(vacancy), &cells);
        assert_eq!(
            p.moves.iter().map(|m| (m.from, m.to)).collect::<Vec<_>>(),
            vec![(near, vacancy), (donor, near)]
        );
        assert_eq!(p.equilibrium_head_y, 2);
        apply(&mut cells, &p);
        let after = metrics(&cells);
        assert_eq!(before.0, after.0);
        assert_eq!(before.2, after.2);
        assert_eq!(after.1 - before.1, p.potential_delta_cells);
        assert!(p.potential_delta_cells <= 0);
        assert!(cells[&vacancy].water);
        assert!(!cells[&donor].water);
    }
    #[test]
    fn export_moves_destination_first_and_never_raises_potential() {
        let filled = c(0, 1, 0);
        let next = c(1, 1, 0);
        let destination = c(1, 0, 0);
        let mut cells = BTreeMap::from([
            (filled, wet(3.)),
            (next, wet(1.)),
            (destination, NativeCellState::default()),
        ]);
        let before = metrics(&cells);
        let p = outcome(RelaxEvent::ExportFilled(filled), &cells);
        assert_eq!(
            p.moves.iter().map(|m| (m.from, m.to)).collect::<Vec<_>>(),
            vec![(next, destination), (filled, next)]
        );
        apply(&mut cells, &p);
        let after = metrics(&cells);
        assert_eq!(before.0, after.0);
        assert_eq!(before.2, after.2);
        assert_eq!(after.1 - before.1, p.potential_delta_cells);
        assert_eq!(p.potential_delta_cells, -1);
        assert!(!cells[&filled].water);
    }
    #[test]
    fn equal_depth_ties_prefer_high_donor_and_low_destination_deterministically() {
        let start = c(0, 0, 0);
        let high = c(0, 1, 0);
        let side = c(1, 0, 0);
        let cells = BTreeMap::from([
            (start, NativeCellState::default()),
            (high, wet(1.)),
            (c(0, 2, 0), NativeCellState::default()),
            (side, wet(1.)),
            (c(1, 1, 0), NativeCellState::default()),
        ]);
        let a = outcome(RelaxEvent::ImportVacancy(start), &cells);
        let b = outcome(RelaxEvent::ImportVacancy(start), &cells);
        assert_eq!(a, b);
        assert_eq!(a.moves[0].from, high);
        let cells = BTreeMap::from([
            (high, wet(1.)),
            (start, NativeCellState::default()),
            (c(-1, 1, 0), NativeCellState::default()),
        ]);
        let p = outcome(RelaxEvent::ExportFilled(high), &cells);
        assert_eq!(p.moves[0].to, start);
    }
    #[test]
    fn unknown_or_active_cells_are_not_surface_air_and_solids_are_not_crossed() {
        let start = c(0, 0, 0);
        let blocked = c(1, 0, 0);
        let donor = c(2, 0, 0);
        let cells = BTreeMap::from([
            (start, NativeCellState::default()),
            (donor, wet(1.)),
            (c(2, 1, 0), NativeCellState::default()),
        ]);
        let mut budget = 64;
        let result = plan(
            RelaxEvent::ImportVacancy(start),
            |at| {
                if at == blocked {
                    None
                } else {
                    Some(cells.get(&at).copied().unwrap_or_else(solid))
                }
            },
            &mut budget,
        );
        assert_eq!(result, RelaxOutcome::Pending(RelaxPending::UnknownBoundary));
        let mut budget = 64;
        let result = plan(
            RelaxEvent::ImportVacancy(start),
            |at| Some(cells.get(&at).copied().unwrap_or_else(solid)),
            &mut budget,
        );
        assert_eq!(result, RelaxOutcome::Pending(RelaxPending::NoSurfaceDonor));
    }
    #[test]
    fn submerged_component_cannot_invent_a_donor_bubble() {
        let start = c(0, 0, 0);
        let mut cells = BTreeMap::new();
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    cells.insert(c(x, y, z), wet(1.));
                }
            }
        }
        cells.insert(start, NativeCellState::default());
        let mut budget = 256;
        let result = plan(
            RelaxEvent::ImportVacancy(start),
            |at| Some(cells.get(&at).copied().unwrap_or_else(solid)),
            &mut budget,
        );
        assert_eq!(result, RelaxOutcome::Pending(RelaxPending::NoSurfaceDonor));
    }
    #[test]
    fn query_work_is_shared_bounded_and_memoized() {
        let mut budget = 5;
        let mut queried = BTreeSet::new();
        let result = plan(
            RelaxEvent::ImportVacancy(c(0, 0, 0)),
            |at| {
                assert!(queried.insert(at), "duplicate expensive cell query");
                Some(if at == c(0, 0, 0) {
                    NativeCellState::default()
                } else {
                    wet(1.)
                })
            },
            &mut budget,
        );
        assert_eq!(result, RelaxOutcome::Pending(RelaxPending::BudgetExhausted));
        assert_eq!(budget, 0);
        assert_eq!(queried.len(), 5);
        assert_eq!(
            plan(
                RelaxEvent::ImportVacancy(c(0, 0, 0)),
                |_| panic!("query after shared budget exhausted"),
                &mut budget
            ),
            RelaxOutcome::Pending(RelaxPending::BudgetExhausted)
        );
    }
    #[test]
    fn stale_or_invalid_events_reject_without_a_move() {
        let mut budget = 16;
        assert_eq!(
            plan(
                RelaxEvent::ImportVacancy(c(0, 0, 0)),
                |_| Some(wet(1.)),
                &mut budget
            ),
            RelaxOutcome::Pending(RelaxPending::StaleEvent)
        );
        let mut budget = 16;
        let mut invalid = wet(1.);
        invalid.solid = true;
        assert_eq!(
            plan(
                RelaxEvent::ExportFilled(c(0, 0, 0)),
                |_| Some(invalid),
                &mut budget
            ),
            RelaxOutcome::Pending(RelaxPending::InvalidState)
        );
    }
}
