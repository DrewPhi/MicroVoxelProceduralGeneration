# Guide for developers

## Build and run

The current native app requires Cargo, Rust **1.85+**, working graphics drivers,
and X11/XWayland on Linux. Run from the repository root:

```sh
cargo run --locked --release --manifest-path successor/Cargo.toml -p microvoxel-successor
./tools/build_launcher.sh
```

Open **MicroVoxel** or **MicroVoxel.desktop** after building the launcher. The
release executable is `successor/target/release/microvoxel-successor`; launcher
logs go to `output/launcher.log`. `tools/build_successor.sh` also builds a separate
MicroVoxel-Successor shortcut. Generated binaries/settings/captures are ignored.

Ordinary launch opens the procedural world. Imported surveys remain available
through **Esc → World** or `--earth PATH`; a downloaded experiment in `output/`
never silently changes the starting world. See [survey attribution](assets/reference/volcan/README.md).

WASD walks; Space jumps; Shift sprints; mouse looks; F5 cycles player views; F6
toggles grid flight. Esc opens graphics/world settings. Seed, voxel centimeters
and World Scale are independent inputs. Apply World starts a new session.

## Active code

| Path | Responsibility |
|---|---|
| `successor/world/src/generation/` | Physical-space terrain, climate, hydrology, geological detail and optional survey adapter |
| `successor/world/src/grid.rs`, `spec.rs` | Integer cells, rational voxel scale and physical ownership pages |
| `successor/world/src/world.rs`, `storage.rs`, `snapshot.rs` | Exact queries, COW overrides, immutable revisions and atomic transactions |
| `successor/world/src/persistence.rs`, `persistence/` | Compatible edit saves and checksummed transaction journal/recovery |
| `successor/world/src/render/bitmask.rs`, `surface.rs` | Native face masks, greedy rectangles and reusable scratch |
| `successor/world/src/render/coherent.rs`, `clipmap.rs`, `lod.rs`, `triangulation.rs` | Cached filtered patches, nested refinement bands, balanced coverage and conforming joins |
| `successor/world/src/render/brick_trace.rs` | Bounded CPU voxel traversal oracle with explicit incomplete/unknown outcomes |
| `successor/world/src/character/`, `physics.rs`, `player.rs`, `queries.rs` | Procedural voxel player, fixed ticks and exact collision/picking |
| `successor/world/src/materials.rs`, `painting.rs` | Material identities, palettes and deterministic cell colors |
| `successor/viewer/src/runtime.rs`, `streaming.rs`, `lod_planner.rs` | Near-first jobs, physical page batching, immutable worker sources and regional revision acceptance |
| `successor/viewer/src/gpu.rs`, `arena.rs`, `gpu_timing.rs` | Shared buffers, generational allocations, deferred retirement and optional GPU timestamps |
| `successor/viewer/src/terrain.wgsl`, `presentation.*` | Native/coarse terrain, local transitions and one optional AA target |
| `successor/viewer/src/menu.rs`, `settings.rs`, `worlds.rs` | Esc controls, preferences, world requests and session replacement |
| `successor/viewer/src/diagnostics/` | Repeatable movement, live edits, quality captures and brick-DDA experiment |
| `tools/benchmark_successor.py`, `capture_terrain_sweep.py` | Paired benchmark manifests and fixed-camera screenshot sweeps |
| `agent_context/` | Research, implementation decisions and recorded measurements |

The root `src/`, `tests/` and `native/` preserve the pre-overhaul implementation
and its historical tools for comparison. They are **legacy**, not the default
application. Keep changes to active engine code in `successor/`. The frozen local
baseline and source hashes are under ignored `output/redesign/baseline/`.

## World contracts

World positions are integer voxel cells. A default cell is 6.25 cm. Generation is
specified in physical space; changing voxel size samples the same underlying
landscape. Unknown data is not Air. Explicit Air overrides carve terrain; reset
removes an override and restores its procedural baseline. A transaction publishes
one consistent revision. Snapshot readers retain earlier immutable state.

Ordinary coarse patches are visual reconstructions and may have fractional
heights. Exact collision, picking and edits use native cells. Each native voxel
has one solid color on every side, with lighting applied separately. Edited or
volumetric footprints replace the base surface, including removals, instead of
rendering added geometry over an unchanged procedural top.

Terrain stays procedural; overrides are sparse COW bricks. Worker jobs are bounded
and grouped into physical pages. Regional dependency revisions preserve unrelated
GPU tiles after edits; worker/planner epochs reject stale results. GPU allocations
retire after submission completion. Growth/compaction counts migration buffers
against the permitted memory ceiling. Lowering a budget can retain the previous
ceiling while the visible cover coarsens; high-offset ray allocations may delay
shrinking until replaced or evicted.

The finite source is **10 km square**, not a whole planet. World Scale N compresses
source coordinates in X/Y/Z while voxel centimeters, player size and movement stay
fixed: 1 is 10 km square, 10 is 1 km square, 100 is 100 m square. It enters save
identity; incompatible source/scale saves are rejected. Procedural generation and
long-term climate are deterministic; live weather and water physics remain future work.

## Checks and captures

```sh
cargo test --locked --workspace --manifest-path successor/Cargo.toml
cargo test --locked -p microvoxel-successor --manifest-path successor/Cargo.toml gpu::tests -- --ignored --test-threads=1
cargo test --locked -p microvoxel-successor --manifest-path successor/Cargo.toml coherent_gpu_tests -- --ignored --test-threads=1
./MicroVoxel --brick-study
MICROVOXEL_EDIT_CAPTURE=1 ./MicroVoxel --world procedural --voxel-size 1/4 --edit-test
```

A repeatable 120-second 1080p route, using the actual default 256 MiB terrain budget:

```sh
MICROVOXEL_STREAM_SIZE=1920x1080 MICROVOXEL_STREAM_TICKS=7200 \
MICROVOXEL_STREAM_RENDERER=mesh MICROVOXEL_STREAM_PRESET=1 \
MICROVOXEL_STREAM_OUTPUT=output/redesign/movement ./MicroVoxel --stream-test
```

Renderer accepts `mesh`, `ray`, or `both`; presets are 0/1/2. Without environment
overrides, the historical short diagnostic runs 600 ticks at 960×540 for both
backends and presets 0/1. `MICROVOXEL_STREAM_CAPTURE=1` saves motion samples; use a
separate run because readback changes timings. Managed buffer capacity excludes
render targets and driver allocations; RSS is a separate sampled process metric.

For photographs use `--world procedural --headless --camera X,Y,Z,YAW,PITCH`
(meters/degrees), `--size 2560x1440 --no-hud --frames 60 --screenshot PATH`.
Add `--camera-ground --view rear` to settle the player at X/Z. Explicit cameras
otherwise start in flight and snap to the voxel grid. `--renderer mesh|ray`,
`--quality 0..100`, `--terrain-memory MiB` and `--haze 0..2` override display settings.
These diagnostics do not write graphics preferences.

[Implementation/results](agent_context/ENGINE_REDESIGN_IMPLEMENTATION.md) ·
[Research](agent_context/ENGINE_REDESIGN_RESEARCH.md) ·
[Benchmark workflow](successor/README.md)
