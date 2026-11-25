# November 16 coordinated implementation

Baseline: `38fcf40`, branch `frontier-voxel-engine`. Work is staged to avoid
concurrent edits to shared renderer files. Canonical voxels remain 6.25 cm.

| Work | Status |
|---|---|
| Edit publication fairness during continuous following | Implemented: bounded alternation between content and movement |
| Bounded edit differences and constant-time fluid diagnostics | Implemented with branch/revert/eviction and independent counter oracles; capped spatial index accelerates local edit queries |
| Resident procedural editing bounds and smaller-radius save reload | Implemented; resident ownership checks and dormant-state retention |
| #88 CPU physical liquid reference | Native 60-second still-water gate passes; ghost/MacCormack post-impact energy errors 18.11% native / 5.04% fine block promotion |
| #88 GPU pressure component | Isolated GPU-resident PCG; actual Radeon parity passed three manufactured fixtures; not coupled to fluid/runtime |
| #88 pickup, inventory, transactions and persistence | Implemented; actual X11 quick/hold/place test passed, FVWATER3 with V2 import |
| #76/#77/#79 existing implementation acceptance | Audited; #76 per-chunk source shortcut fixed; full performance matrix still open |
| #78 independent cosmetic scheduling | Opt-in worker no longer cancels valid work on every new geometry request; GPU evaluation pending |
| #80/#84 ground-cover representation/statistics | Opt-in nested 16/64 probes, correct cache identity; dense-oracle error measured |
| #81 native preparation and #82 draw preparation experiments | Chunk-local sort negative synthetic result; exact in-place order A/B candidate |
| #83 terrain handoff | Offline stepped-mesh candidate rejected for live integration; poor error/cost tradeoff |
| #85 tree transitions | Opt-in recipe-derived 4/8/16 contour policy with resident replacement |
| #86 AA | Opt-in five-tap spatial contrast experiment; no temporal-history/default change |
| #87 seeded source water-feature/quality acceptance | Feature spacing debt capped at unresolved descriptor diameter; geometry certificates retained |


Primary pre-change capture: `output/nov16-primary-before/`, fixed existing
Legacy source/camera and independent `output/naadf-next-primary/reference256`.
GPU timings must be measured separately from compilers and recordings. Feature
implementation does not close an issue until its correctness, visual and motion
acceptance are demonstrated. Failed candidates stay opt-in or are rejected.


## Playtest

Build from `frontier/` with `cargo build --release --locked`, then:

```sh
./run.sh --preset seeded-world --seed 19 --live-water on --water-save output/my-world.save
```

Left press previews a native selection; release quickly picks up one voxel, hold
up to 900 ms for at most two at the default 0.5 L hand cap. Right click places
one owned selected material. Number keys 1–6 select material. Inventory defaults
to 64 cells. Opening menus/focus loss cancels an active pickup. Water placement
is rejected until physical displacement is supported. CA water remains in-game;
physical reference code is **offline** and must not be described as promoted.

Actual owned-window input check:
`python3 tools/player_edit_check.py --output output/nov16/player-edit-new`.
Its saved inventory must total two cells at action sequence three after one
quick pickup, one charged pickup and one placement. The first capture click
must not edit. The direct final image excludes the window HUD; the saved state
is the action oracle, not a screenshot alone.

Related evidence: [representation experiments](../../NOV16_REPRESENTATION_EXPERIMENTS.md),
[existing optimization audit](EXISTING_OPTIMIZATIONS.md),
[physical numerical contract](PHYSICAL_WATER_M0.md),
[physical acceptance status](PHYSICAL_WATER_M1_STATUS.md).

## Checkpoint decision

All delegated workers and numerical probes have stopped. Physical M1 is not accepted: post-impact convergence fails despite improved pre-impact results. No physical runtime promotion or whole-world fluid claim is made. See [numerical evidence](PHYSICAL_WATER_M1_STATUS.md) and [GPU pressure component](PHYSICAL_PRESSURE_GPU.md). Remaining optimization issues retain their visual/motion/performance gates; disabled experiments are not defaults.

## Final validation

- Integrated `cargo test --workspace --lib --bins`: **734 passed, 0 failed, 44 ignored**. Ignored hardware/long-running probes require explicit invocation.
- Actual Radeon pressure parity separately passed three manufactured SPD systems; canonical seeded water GPU parity also passed.
- Primary pre/post float capture was bit-identical. Actual X11 pickup/hold/place checkpoint test passed.
- Python tooling suite: 40 passed.
- Million-edit spatial query exactly matched full scan with 16 canonical map checks; this is a CPU diagnostic, not a whole-game speed claim.
- Static 1080p control frame p50/p95/p99: 19.52/22.45/23.30 ms; matched 16-probe cover candidate 19.94/21.91/22.56 ms. Motion control p50/p95/p99: 25.87/34.06/50.12 ms. These are the measured cases, not universal frame-rate promises.
