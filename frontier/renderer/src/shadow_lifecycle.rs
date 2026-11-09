//! Bounded ownership handoff for immutable tree shadow representations.
//! Depth values are never blended. The shader partitions native world cells
//! between old/new geometry with complementary coverage masks.
use super::{MeshKey, VegetationBatch};
use std::collections::BTreeMap;
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct ShadowTransition {
    pub root_cell: [i64; 3],
    pub from: MeshKey,
    pub to: MeshKey,
    pub progress: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowReason {
    Admitted,
    Retained,
    ReplacementPending,
    Transition,
    TransitionBudget,
    BudgetRejected,
    Entering,
    Leaving,
}
impl ShadowReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Retained => "retained",
            Self::ReplacementPending => "replacement_pending",
            Self::Transition => "transition",
            Self::TransitionBudget => "transition_budget",
            Self::BudgetRejected => "budget_rejected",
            Self::Entering => "entering",
            Self::Leaving => "leaving",
        }
    }
}
#[derive(Clone, Debug)]
pub struct ShadowInspection {
    pub root_cell: [i64; 3],
    pub selected: Option<MeshKey>,
    pub requested: MeshKey,
    pub reason: ShadowReason,
    pub triangles: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct ShadowCoverage {
    pub root_cell: [i64; 3],
    pub key: MeshKey,
    pub coverage: f32,
}
struct Resident {
    batch: VegetationBatch,
    transition: Option<(VegetationBatch, Instant)>,
    visibility: f32,
    last: Instant,
    leaving: bool,
}
#[derive(Default)]
pub(super) struct ShadowLifecycle {
    residents: BTreeMap<[i64; 3], Resident>,
}
impl ShadowLifecycle {
    /// Geometry still owned during replacement/exit must fit alongside color,
    /// even after the camera or resource profile changes the desired casters.
    pub(super) fn geometry_triangles(&self) -> usize {
        self.residents
            .values()
            .map(|resident| {
                cost(&resident.batch) + resident.transition.as_ref().map_or(0, |(old, _)| cost(old))
            })
            .sum()
    }

    pub fn clear(&mut self) {
        self.residents.clear();
    }
    pub fn apply(
        &mut self,
        selected: Vec<VegetationBatch>,
        budget: usize,
        now: Instant,
        inspections: &mut Vec<ShadowInspection>,
    ) -> (
        Vec<VegetationBatch>,
        Vec<ShadowTransition>,
        Vec<ShadowCoverage>,
    ) {
        let mut desired: BTreeMap<_, _> = selected
            .into_iter()
            .map(|b| (b.instances[0].root_cell, b))
            .collect();
        // A cache eviction must not turn a requested, still-valid tree into an
        // exit while its replacement is being built. Ownership keeps its Arc.
        for inspection in inspections.iter_mut() {
            if inspection.reason == ShadowReason::ReplacementPending
                && inspection.selected.is_none()
            {
                if let Some(resident) = self.residents.get(&inspection.root_cell) {
                    desired
                        .entry(inspection.root_cell)
                        .or_insert_with(|| resident.batch.clone());
                    inspection.selected = Some(resident.batch.key);
                    inspection.triangles = cost(&resident.batch);
                }
            }
        }
        // Advance the previous ownership state before applying changed demand.
        // Reversal starts at the actual current visibility, so a quick return
        // cannot reset a half-visible tree to fully lit or fully shadowed.
        for (root, resident) in &mut self.residents {
            let step = now.saturating_duration_since(resident.last).as_secs_f32() / 0.3;
            resident.visibility =
                (resident.visibility + if resident.leaving { -step } else { step }).clamp(0., 1.);
            resident.last = now;
            resident.leaving = !desired.contains_key(root);
            if resident.transition.as_ref().is_some_and(|(_, start)| {
                now.saturating_duration_since(*start).as_secs_f32() >= 0.3
            }) {
                resident.transition = None;
            }
        }
        self.residents
            .retain(|_, r| !r.leaving || r.visibility > 0.);
        let mut used: usize = self
            .residents
            .values()
            .map(|r| cost(&r.batch) + r.transition.as_ref().map_or(0, |(b, _)| cost(b)))
            .sum();
        for (root, batch) in desired {
            if let Some(resident) = self.residents.get_mut(&root) {
                if resident.batch.key != batch.key {
                    if resident.transition.is_none() && used.saturating_add(cost(&batch)) <= budget
                    {
                        used += cost(&batch);
                        let old = std::mem::replace(&mut resident.batch, batch);
                        resident.transition = Some((old, now));
                    } else if let Some(i) = inspections.iter_mut().find(|i| i.root_cell == root) {
                        i.reason = ShadowReason::TransitionBudget;
                        i.selected = Some(resident.batch.key);
                        i.triangles = cost(&resident.batch);
                    }
                } else {
                    resident.batch = batch;
                }
            } else if self.residents.len() < 512 && used.saturating_add(cost(&batch)) <= budget {
                used += cost(&batch);
                self.residents.insert(
                    root,
                    Resident {
                        batch,
                        transition: None,
                        visibility: 0.,
                        last: now,
                        leaving: false,
                    },
                );
            } else if let Some(i) = inspections.iter_mut().find(|i| i.root_cell == root) {
                i.reason = ShadowReason::BudgetRejected;
                i.selected = None;
                i.triangles = 0;
            }
        }
        let mut output = Vec::new();
        let mut transitions = Vec::new();
        let mut coverage = Vec::new();
        for (root, resident) in &self.residents {
            if resident.leaving && !inspections.iter().any(|i| i.root_cell == *root) {
                inspections.push(ShadowInspection {
                    root_cell: *root,
                    selected: Some(resident.batch.key),
                    requested: resident.batch.key,
                    reason: ShadowReason::Leaving,
                    triangles: cost(&resident.batch),
                });
            }
            if let Some((old, start)) = &resident.transition {
                transitions.push(ShadowTransition {
                    root_cell: *root,
                    from: old.key,
                    to: resident.batch.key,
                    progress: (now.saturating_duration_since(*start).as_secs_f32() / 0.3).min(1.),
                });
                output.push(old.clone());
                if resident.visibility < 1. || resident.leaving {
                    coverage.push(ShadowCoverage {
                        root_cell: *root,
                        key: old.key,
                        coverage: resident.visibility,
                    });
                }
                if let Some(i) = inspections.iter_mut().find(|i| i.root_cell == *root) {
                    i.reason = ShadowReason::Transition;
                }
            }
            if resident.visibility < 1. || resident.leaving {
                coverage.push(ShadowCoverage {
                    root_cell: *root,
                    key: resident.batch.key,
                    coverage: resident.visibility,
                });
                if let Some(i) = inspections.iter_mut().find(|i| i.root_cell == *root) {
                    if resident.transition.is_none() {
                        i.reason = if resident.leaving {
                            ShadowReason::Leaving
                        } else {
                            ShadowReason::Entering
                        };
                    }
                }
            }
            output.push(resident.batch.clone());
        }
        debug_assert!(output.iter().map(cost).sum::<usize>() <= budget);
        (output, transitions, coverage)
    }
}

fn cost(batch: &VegetationBatch) -> usize {
    batch.mesh.indices.len() / 3 * batch.instances.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vegetation::{VegetationInstance, VegetationMesh};
    use std::{sync::Arc, time::Duration};
    fn batch(lod: u8, triangles: usize) -> VegetationBatch {
        VegetationBatch {
            key: MeshKey {
                species: 2,
                variant: 0,
                seed: 3,
                tile: [0; 2],
                lod,
                edit_id: 0,
            },
            mesh: Arc::new(VegetationMesh {
                vertices: vec![],
                indices: vec![0; triangles * 3],
            }),
            instances: vec![VegetationInstance {
                root_cell: [1, 2, 3],
                quarter_turn: 0,
                fade: 1.,
            }],
        }
    }
    #[test]
    fn replacement_keeps_both_then_converges_and_releases_old() {
        let mut life = ShadowLifecycle::default();
        let now = Instant::now();
        life.apply(vec![batch(4, 10)], 40, now, &mut vec![]);
        let (out, transition, _) = life.apply(vec![batch(2, 20)], 40, now, &mut vec![]);
        assert_eq!(out.len(), 2);
        assert_eq!(transition.len(), 1);
        let (out, transition, _) = life.apply(
            vec![batch(2, 20)],
            40,
            now + Duration::from_secs(1),
            &mut vec![],
        );
        assert_eq!(out.len(), 1);
        assert!(transition.is_empty());
        assert_eq!(out[0].key.lod, 2);
        life.apply(vec![], 40, now + Duration::from_secs(2), &mut vec![]);
        life.apply(vec![], 40, now + Duration::from_secs(3), &mut vec![]);
        assert!(life.residents.is_empty());
    }
    #[test]
    fn overlap_cannot_exceed_budget_and_revision_reset_releases_geometry() {
        let mut life = ShadowLifecycle::default();
        let now = Instant::now();
        life.apply(vec![batch(4, 10)], 25, now, &mut vec![]);
        let (out, transition, _) = life.apply(vec![batch(2, 20)], 25, now, &mut vec![]);
        assert!(transition.is_empty());
        assert_eq!(out[0].key.lod, 4);
        assert!(out.iter().map(cost).sum::<usize>() <= 25);
        life.clear();
        assert!(life.residents.is_empty());
    }
    #[test]
    fn entry_exit_and_reversal_preserve_continuous_bounded_coverage() {
        let mut life = ShadowLifecycle::default();
        let now = Instant::now();
        let (_, _, coverage) = life.apply(vec![batch(4, 10)], 20, now, &mut vec![]);
        assert_eq!(coverage[0].coverage, 0.);
        let (_, _, coverage) = life.apply(
            vec![batch(4, 10)],
            20,
            now + Duration::from_millis(150),
            &mut vec![],
        );
        assert!((coverage[0].coverage - 0.5).abs() < 1e-6);
        let (_, _, coverage) =
            life.apply(vec![], 20, now + Duration::from_millis(150), &mut vec![]);
        assert!((coverage[0].coverage - 0.5).abs() < 1e-6);
        let (_, _, coverage) = life.apply(
            vec![batch(4, 10)],
            20,
            now + Duration::from_millis(225),
            &mut vec![],
        );
        assert!((coverage[0].coverage - 0.25).abs() < 1e-6);
        let (_, _, coverage) = life.apply(
            vec![batch(4, 10)],
            20,
            now + Duration::from_secs(1),
            &mut vec![],
        );
        assert!(coverage.is_empty());
        let (out, _, coverage) = life.apply(vec![], 20, now + Duration::from_secs(1), &mut vec![]);
        assert_eq!(out.len(), 1);
        assert_eq!(coverage[0].coverage, 1.);
        let (out, _, coverage) =
            life.apply(vec![], 20, now + Duration::from_millis(1150), &mut vec![]);
        assert_eq!(out.len(), 1);
        assert!((coverage[0].coverage - 0.5).abs() < 1e-6);
        let (out, _, coverage) = life.apply(vec![], 20, now + Duration::from_secs(2), &mut vec![]);
        assert!(out.is_empty());
        assert!(coverage.is_empty());
    }
    #[test]
    fn exiting_owners_reserve_budget_before_new_entries_and_edits_clear_every_owner() {
        let mut life = ShadowLifecycle::default();
        let now = Instant::now();
        life.apply(vec![batch(4, 10)], 10, now, &mut vec![]);
        life.apply(
            vec![batch(4, 10)],
            10,
            now + Duration::from_secs(1),
            &mut vec![],
        );
        let mut next = batch(4, 10);
        next.instances[0].root_cell = [9, 2, 3];
        let (out, _, _) = life.apply(
            vec![next.clone()],
            10,
            now + Duration::from_secs(1),
            &mut vec![],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].instances[0].root_cell, [1, 2, 3]);
        assert_eq!(out.iter().map(cost).sum::<usize>(), 10);
        let (out, _, coverage) =
            life.apply(vec![next], 10, now + Duration::from_secs(2), &mut vec![]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].instances[0].root_cell, [9, 2, 3]);
        assert_eq!(coverage[0].coverage, 0.);
        life.clear();
        assert!(life.residents.is_empty());
    }
}
