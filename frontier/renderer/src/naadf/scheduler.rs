//! Idle-turn priority shared by growth, content refresh and camera following.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Update {
    Refresh,
    Grow,
    Follow,
}

pub(super) fn next_update(
    idle: bool,
    content_pending: bool,
    growth_pending: bool,
    follow: bool,
    refreshed_last: bool,
) -> Option<Update> {
    if !idle {
        None
    } else if content_pending && !refreshed_last {
        Some(Update::Refresh)
    } else if growth_pending {
        Some(Update::Grow)
    } else if follow {
        Some(Update::Follow)
    } else if content_pending {
        Some(Update::Refresh)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naadf::{data, leaf::Encoding, stream};
    use frontier_world::{Cell, FbmParams, Material, World};
    use std::sync::atomic::AtomicU32;

    #[test]
    fn consecutive_follow_shifts_give_pending_edit_the_next_idle_turn() {
        let mut world = World::fbm(
            19,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        let eye = [0., world.height_m(0., 0.) + 1., 0.];
        let mut source = world.clone();
        let first =
            data::build(&source, eye, 4, 128 * 1048576, &AtomicU32::new(0), || false).unwrap();
        let mut state = stream::State::new_with_encoding(
            first,
            [16 * 1048576, 32 * 1048576, 1048576],
            Encoding::Wide32,
        );
        let edited = Cell::from_meters(eye);
        // Continuous demand already produced one shift; a second is underway
        // when an edit arrives. Retained roots still belong to `source`.
        for step in 0..2 {
            assert_eq!(
                next_update(true, false, false, true, false),
                Some(Update::Follow)
            );
            let mut next = state.region;
            next.lo[0] += 1;
            next.hi[0] += 1;
            let change = state
                .prepare(&source, next, 128 * 1048576, &|| false)
                .unwrap();
            if step == 1 {
                world.set(edited, Material::Water);
                assert_ne!(source.revision(), world.revision());
                // Neither a refresh nor another following job may overlap the
                // current upload/ownership handoff.
                assert_eq!(next_update(false, true, false, true, false), None);
            }
            state.finish(change, true);
        }
        // The camera still wants another shift at this exact publication turn.
        // Previously following immediately won here on every moving frame.
        assert_eq!(
            next_update(true, true, false, true, false),
            Some(Update::Refresh)
        );
        let refreshed = world.clone();
        let cells = source.changed_edit_cells(&refreshed);
        assert_eq!(cells, vec![edited]);
        let change = state
            .prepare_refresh(&refreshed, &cells, 128 * 1048576, &|| false)
            .unwrap();
        assert_eq!(
            change.roots.len(),
            1,
            "the pending native root must actually be rebuilt"
        );
        let region = state.region;
        world.set(edited, Material::Stone);
        assert_ne!(refreshed.revision(), world.revision());
        assert_eq!(next_update(false, true, false, true, false), None);
        state.finish(change, true);
        source = refreshed;
        assert_eq!(
            state.region, region,
            "refresh must preserve the current window"
        );
        assert_ne!(source.revision(), world.revision());
        assert_eq!(
            next_update(true, true, false, true, true),
            Some(Update::Follow)
        );
        // A newer edit arrived during refresh. Following still receives one
        // turn with the coherent published snapshot, then the edit gets a turn.
        let mut next = state.region;
        next.lo[0] += 1;
        next.hi[0] += 1;
        let change = state
            .prepare(&source, next, 128 * 1048576, &|| false)
            .unwrap();
        state.finish(change, true);
        assert_eq!(state.region, next);
        assert_eq!(
            next_update(true, true, false, true, false),
            Some(Update::Refresh)
        );
        let cells = source.changed_edit_cells(&world);
        assert_eq!(cells, vec![edited]);
        let change = state
            .prepare_refresh(&world, &cells, 128 * 1048576, &|| false)
            .unwrap();
        assert_eq!(change.roots.len(), 1);
        state.finish(change, true);
    }

    #[test]
    fn endless_source_changes_and_movement_share_idle_turns() {
        for growing in [false, true] {
            let mut refreshed_last = false;
            let movement = if growing {
                Update::Grow
            } else {
                Update::Follow
            };
            for turn in 0..32 {
                // Fluid changes faster than a refresh can finish, while the
                // camera continues requesting strips (or startup still grows).
                let expected = if turn % 2 == 0 {
                    Update::Refresh
                } else {
                    movement
                };
                assert_eq!(
                    next_update(false, true, growing, !growing, refreshed_last),
                    None
                );
                let update = next_update(true, true, growing, !growing, refreshed_last).unwrap();
                assert_eq!(update, expected);
                refreshed_last = update == Update::Refresh;
            }
        }
        // A previous refresh never delays content if no growth/strip is needed.
        assert_eq!(
            next_update(true, true, false, false, true),
            Some(Update::Refresh)
        );
    }

    #[test]
    fn content_priority_preserves_growth_and_publication_gates() {
        for content in [false, true] {
            for growth in [false, true] {
                // Busy work, stale epochs, raster restore, errors and missing
                // active coverage all feed the same idle gate in the caller.
                assert_eq!(next_update(false, content, growth, true, false), None);
            }
        }
        assert_eq!(
            next_update(true, true, true, true, false),
            Some(Update::Refresh)
        );
        assert_eq!(
            next_update(true, false, true, true, false),
            Some(Update::Grow)
        );
        assert_eq!(
            next_update(true, false, true, false, false),
            Some(Update::Grow)
        );
        assert_eq!(next_update(true, false, false, false, false), None);
    }
}
