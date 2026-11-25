//! Retained slots, canceled reservations, expansion and subsequent wrapping
//! must agree with independently built canonical snapshots.
use super::*;
use crate::naadf::{leaf, startup};

#[test]
fn expanding_reserved_window_keeps_voxels_and_then_follows_player() {
    for wide in [false, true] {
        let world = World::fbm(
            7,
            frontier_world::FbmParams {
                recipe: frontier_world::FbmRecipe::Alpine,
                amplitude: 8.,
                horizontal_scale: 50.,
                octaves: 3,
                ..Default::default()
            },
        )
        .unwrap()
        .with_vegetation();
        let center = [-41., world.height_m(-41., -250.) + 1.7, -250.];
        let build = |region: Region| {
            data::build_region(
                &world,
                cell(region.lo),
                region.dims(),
                128 * 1048576,
                &AtomicU32::new(0),
                None,
                false,
                &|| false,
            )
            .unwrap()
        };
        let initial = Region {
            lo: center.map(|v| v.floor() as i64 - 2),
            hi: center.map(|v| v.floor() as i64 + 2),
        };
        let p = leaf::Prepared::new(build(initial), wide);
        let mut voxels = p.snapshot.voxels.clone();
        let encoding = p.encoding;
        let mut state = State::with_storage(
            p.snapshot,
            [16 * 1048576, 32 * 1048576, 4 * 1048576],
            encoding,
            24,
        );
        let mut covered = initial.dims().iter().product::<usize>();
        for step in 0..6 {
            let next = if step < 3 {
                // Stationary first, then grow while moving across negative coordinates.
                startup::next_region(
                    state.region,
                    12,
                    std::array::from_fn(|a| center[a] + if a == 0 { step as f64 } else { 0. }),
                )
            } else {
                Region {
                    lo: state.region.lo.map(|v| v - 5),
                    hi: state.region.hi.map(|v| v - 5),
                }
            };
            let previous = state.region;
            let old_roots = state.chunks.clone();
            let canceled = state
                .prepare(&world, next, 128 * 1048576, &|| false)
                .unwrap();
            state.finish(canceled, false);
            assert_eq!(state.region, previous);
            assert_eq!(state.chunks, old_roots);
            let c = state
                .prepare(&world, next, 128 * 1048576, &|| false)
                .unwrap();
            for r in &c.roots {
                // Existing voxels are never fed back into terrain generation.
                assert!((0..3)
                    .any(|a| r.region.hi[a] <= previous.lo[a] || r.region.lo[a] >= previous.hi[a]));
            }
            if step == 0 {
                covered += c.roots.iter().map(|r| r.nodes.len()).sum::<usize>();
                assert_eq!(covered, next.dims().iter().product::<usize>());
            }
            for w in &c.writes {
                if w.kind == 1 {
                    voxels.resize(voxels.len().max(w.word + w.data.len()), 0);
                    voxels[w.word..w.word + w.data.len()].copy_from_slice(&w.data);
                }
            }
            state.finish(c, true);
            // Physical slots retained by the overlap must remain byte-identical.
            let overlap = next.intersection(previous);
            for z in overlap.lo[2]..overlap.hi[2] {
                for x in overlap.lo[0]..overlap.hi[0] {
                    for y in overlap.lo[1]..overlap.hi[1] {
                        let i = state.index([x, y, z]);
                        assert_eq!(state.chunks[i], old_roots[i]);
                    }
                }
            }
            let reference = build(next);
            let dims = next.dims().map(|v| v * 16);
            // Visit every root and sample all voxel quadrants, including edges.
            for z in (0..dims[2]).step_by(7) {
                for x in (0..dims[0]).step_by(7) {
                    for y in (0..dims[1]).step_by(7) {
                        let p = [
                            next.lo[0] + (x / 16) as i64,
                            next.lo[1] + (y / 16) as i64,
                            next.lo[2] + (z / 16) as i64,
                        ];
                        let mut word = state.chunks[state.index(p)];
                        if word & CHILD != 0 {
                            word = state.blocks[(word & MASK) as usize
                                + (x % 16) / 4
                                + 4 * ((y % 16) / 4 + 4 * ((z % 16) / 4))];
                            if word & CHILD != 0 {
                                word = encoding.load(
                                    &voxels,
                                    (word & MASK) as usize + x % 4 + 4 * (y % 4 + 4 * (z % 4)),
                                );
                            }
                        }
                        let value = |word: u32, palette: &[Pigment]| {
                            (word & FULL != 0).then(|| key(&palette[(word & MASK) as usize]))
                        };
                        assert_eq!(
                            value(word, &state.palette),
                            value(reference.material([x, y, z]), &reference.palette),
                            "step {step}, at {p:?}, wide={wide}"
                        );
                    }
                }
            }
        }
        let oversized = Region {
            lo: state.region.lo,
            hi: state.region.hi.map(|v| v + 1),
        };
        assert!(state
            .prepare(&world, oversized, 128 * 1048576, &|| false)
            .is_err());
    }
}
