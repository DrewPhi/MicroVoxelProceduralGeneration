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

The **MicroVoxel launcher** opens the new saved geographic atlas, seed 42, at
World Scale 10. Direct CLI execution retains the legacy procedural default; use
`--world atlas --world-scale 10` for the new source or `--world procedural` for
comparison. Imported surveys remain available
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
shrinking until replaced or evicted. Terrain draws bind aligned subranges of the
shared buffers, so total pool capacity can exceed an adapter's per-storage-binding
limit. Bindings are cached and refreshed on growth/compaction. The console memory
ledger distinguishes live/pending/free pool capacity and in-flight migrations;
see [the LOD budget investigation](agent_context/LOD_BUDGET_ISSUE_3.md).

The new atlas is **1000 km square** in source coordinates, not a whole planet.
World Scale 10 makes it 100 km square in game; scale 1 retains its original size.
Changing scale compresses X/Y/Z together; it does not change voxel/player sizes.
The default spawn now enters the player-scale valley near game X=97576, Z=47560
at scale10. Its2×2km refinement region is source X960000..980000, Z465000..485000.
Rivers use shared compiled profiles; the local erosion field caches under
`output/atlas-refinement/`. Details: [valley implementation](agent_context/PLAYER_VALLEY_IMPLEMENTATION.md).

Seed 42 reuses the validated local atlas. Other seeds compile once (several
minutes), then load from `output/atlas-cache/`. The launcher builds the compiler
helper. The menu keeps generation work off the interactive thread.

`generation/atlas/` loads immutable physical terrain/climate/drainage arrays,
reconstructs smooth shared triangle edges and river profiles, and supplies the
existing rendering, collision and edit interfaces. A bounded in-memory patch
cache is disposable; canonical atlas files persist. Analytic refinement does
not run an erosion solve per voxel or renderer request.

The **legacy** source defaults to **10 km square**. Its CLI accepts
`--extent-km 1..100`. World Scale N compresses source coordinates in X/Y/Z while
voxel centimeters, player size and movement stay fixed: with the default source,
1 is 10 km square, 10 is 1 km square, 100 is 100 m square. Source extent, grid,
generator version and scale enter save compatibility; incompatible saves are rejected.

The legacy generator 6 starts with tectonic-inspired 72 km province sites and connected
convergent mountain belts. Broad lowland/hill/plateau fields receive local ridge
and geological detail; catchment routing, bounded valley incision and sediment
transport shape drainage. Two seasonal wind sweeps supply static climate and
continuous material/snow suitability. Native cells retain solid colors. These
are approximate geographic/climate models; live weather and water physics remain
future work. Regional arrays stay bounded (16 m samples at 10 km, 128 m at 100 km),
so independently solved extents have different sampling and boundary conditions.

Reproduce the full-scale overview (true heights, no vertical exaggeration):

```sh
./MicroVoxel --world procedural --seed 42 --extent-km 100 --isometric \
  --screenshot output/overview-100km-v6/isometric-100km-seed42.png \
  --debug-maps output/overview-100km-v6/maps
```

To inspect about 1000 km² around the highest sampled peak of that same world,
add `--overview-area-km2 1000 --terrain-memory 512`. Crop edges snap to display
cells (about 31.616 km square); terrain scale, climate and drainage stay unchanged.
The `.txt` beside the PNG records crop bounds and sampled peak elevation.

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
