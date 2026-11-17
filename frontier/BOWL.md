# Bowl interaction checkpoint

Build and launch from `frontier/`:

```sh
cargo build --release --locked -p frontier-tools --bin frontier
./run.sh --preset desert --seed 19
```

The desert starts with empty hands. Press **2** to equip the first-person voxel
bowl from its pocket. Walk to the water's edge and aim at water within 2.2 m.

- Left click scoops real water, up to eight whole native units (1.953125 L).
- Hold right mouse or **R** to tilt and pour; release to stop.
- Hold Q to tip farther and spill. E stows it; 1–4 swap pockets. B is a bowl shortcut.
- Escape releases mouse/opens settings. The first click captures mouse input.

Clean exit saves the bowl, removed pond water, deposited water, moisture and damp
footprints under `output/desert-game/`. `--bowl-save PATH` selects a separate file
for an experiment. Existing files restore automatically and validate seed/source,
physical scale and amplitude. Camera position currently resets to the shore spawn.
Failed/corrupt saves report an error instead of silently resetting water.

## Model and boundaries

`world/src/bowl.rs` owns the finite mass ledger and native-grid droplets. Source
pond water is removed, never implicitly refilled. Numerical velocity/credits
advance whole voxel positions with swept face collision. Sand absorbs deposited
water into an integer moisture store; a small surrounding damp footprint provides
visual feedback. Pouring back into a waterbody or onto an impermeable floor
returns whole water units to the world. This is a small local bowl transport model,
not integration of a worldwide pressure solver. Stationary deposited water has
no lateral pressure flow yet. Internal bowl sloshing is not simulated.

The ledger is now `scooped = held + airborne + soil moisture + deposited + plant transfer`.
[Planting and seedling growth](PLANTING.md) accounts for plant storage and
transpiration. Legacy bowl-only saves migrate with zero plant transfer. Returning and
recollecting water preserves it. The serialized state includes droplets in flight,
source-removal tombstones, moisture, damp footprints, emission phase and counters.
A size/identity/finite-value/duplicate/ledger check guards restore; writes use a
temporary sibling followed by rename. This is a single-player checkpoint file,
not a synchronized multi-writer save service.

Moisture is independent of fertility: watering creates no nutrients or organic
matter. The first visual implementation marks damp surface cells with the existing
Soil color/material palette. Their original desert sand source is unchanged and
the separate moisture record is authoritative; a dedicated wetness appearance
channel remains a later rendering improvement. There is no standalone soil evaporation; the plant update separately counts transpiration.

`renderer/src/bowl_live.rs` handles controls, persistence and a procedural held
voxel view model. Its tiny number of airborne cubes is projected in world space
with canonical occlusion through the UI compositor. Those cubes currently do not
cast shadows or participate in the pressure solver. Stationary water and damp
patches use canonical world edits/NAADF publication. Far-away edited appearances
outside the native region retain the engine's existing limitations.

## Verification

Core tests cover capacity, unreachable/blocked scooping, actual source removal,
pouring/absorption, restart in flight, return-and-rescoop conservation, malformed
ledger rejection and persistent moisture. `cargo test -p frontier-world --lib`
passed 200 tests (6 ignored) before the final emitter-position adjustment; the
focused bowl tests were rerun afterward. `cargo check --workspace --tests --locked`
is the integration compile gate. The release binary is rebuilt for the playtest.

Record an actual input-driven demonstration (no injected water state):

```sh
python3 tools/record_bowl.py --output output/my-bowl-playtest
```

The tool validates its own X11 child window, walks to the pond, scoops, carries,
pours on sand, returns, refills and tips the bowl. It records MP4, key frames,
commands, timestamps, binary hash, mass counters and game report. Video recording
adds overhead; its frame times are not an isolated performance benchmark.

The game restart check caught and fixed an initial integration error: restored
wet terrain edits cannot be passed into the immutable far-surface builder.
The bowl now uses the same immutable far-source/native-edit ownership split as
the live-water modes. Reload is checked in a second actual game process.

Final video: `output/bowl-lip-playtest/bowl.mp4` (actual 1080p input-driven run).
The player scooped 15 native water units, poured 8 and spilled 7; all 15 ended
in saved soil moisture. Both damp patches (58 cells total) survived actual game
restart. The earlier restart failure was corrected afterward without changing
interaction/render code in the recorded video; binary hashes and commands are
recorded in `bowl-checkpoint.json`. The recording's median/p95/p99 frame intervals
are saved there with recording overhead disclosed. This is not a general engine
performance or whole-world hydrodynamics acceptance claim.
