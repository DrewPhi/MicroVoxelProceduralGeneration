# Initial native renderer measurements

Latest measurements and source changes are in
[profiling and cleanup](PROFILING_AND_CLEANUP.md). The sections below preserve
earlier measurement history and use different scenes/settings where stated.

Measured in a release build on this machine, using **AMD Radeon 760M integrated
graphics**, Vulkan / RADV PHOENIX. These are observations of one fixed scene, not
minimum hardware requirements or a guarantee of gameplay performance.

Command from the repository root:

```sh
./native/target/release/microvoxel-game --headless --fly-test --frames 60 --screenshot native_preview.png
```

Configuration: seed 42, 10 km region, 6.25 cm native cells, 1280×720, 75° vertical
FOV, default target of 384 LOD tiles, four mesh workers. The scripted flight uses
canonical collision queries and installs replacement LOD meshes before measurement.

| Measurement | Observed |
|---|---:|
| Terrain setup | 2.89 s |
| First detailed stitched cover after setup | 0.55 s |
| Warm synchronized offscreen frame time, 60 frames | 1.57 ms/frame |
| Tiles in final cover / visible | 333 / 105 |
| GPU mesh cache, including retained earlier meshes | 14.87 MiB |
| Active exposed cell faces / merged rectangles | 760,202 / 351,999 |

Each frame includes camera-relative uniforms, canonical picking, frustum culling,
drawing and a GPU completion wait. It excludes window presentation/vsync,
continuous movement and future simulation. Background loading is finished before
the timed frames. The desktop application has a frame-rate display in its title
bar; test moving and rotating there to assess interactive behavior.

Equal-footprint CPU comparison (`--benchmark`), over a 16 m × 16 m patch:

| Render cell edge | Exposed faces | Rectangles | Mesh/atlas payload | Generation + merging |
|---|---:|---:|---:|---:|
| 6.25 cm, exact | 136,539 | 68,906 | 2.62 MiB | 156.46 ms |
| 12.5 cm | 33,709 | 17,154 | 0.65 MiB | 92.71 ms |
| 25 cm | 8,347 | 4,307 | 0.16 MiB | 24.98 ms |
| 50 cm | 2,051 | 1,065 | 0.04 MiB | 3.24 ms |

The per-run profile cache starts empty, but the shared generator cache can be
warm between rows. Coarse profiles require more samples per cell than exact
columns, so timings need not scale directly with face counts.

Flat geometry is a different case: tests merge a 32×32-cell flat surface into
**one rectangle**, retaining all 1,024 independent voxel colors. Its payload is
4,128 bytes: 32 bytes of geometry and 4,096 bytes of colors. Rough terrain cannot
merge nearly as much, which is why distance LOD still makes a useful difference.

Current limits: LOD uses bounded sampled terrain summaries, not exact aggregation
of the entire world. Grid alignment and explicit boundary faces close seams;
changes in approximation can still pop. Occlusion hierarchies, GPU-driven indirect
submission and simulation performance are future work.


## Climate/landform foundation — 2026-10-28

Generator **version 2**, default seed 42 / temperate preset, same Radeon 760M,
1280×720 and default 384-tile target. Generator changes alter terrain and the
fixed scene: these are new observations, not a controlled speedup over version 1.

| Measurement | Observed |
|---|---:|
| Core regional generation (`./build/bench --terrain`, Rust 1.75 -O) | 2.53 s |
| Persistent regional arrays / channel bins | 46.52 MiB |
| 200,000 native cell queries (bounded repeated 32×32-column footprint) | 11.94 ms |
| 4,096 atomic edits | 1.79 ms |
| Native world setup including 12 PNG map exports | 2.97 s |
| Initial stitched cover | 0.74 s |
| Warm synchronized offscreen frame time, after flight, 60 frames | 1.18 ms/frame |
| Tiles final / visible | 333 / 94 |
| Retained GPU mesh cache | 19.53 MiB |
| Active exposed faces / merged rectangles | 837,221 / 477,889 |

The persistent regional estimate excludes allocator overhead, temporary generation
scratch, hot column caches, renderer CPU caches, GPU resources and the executable;
it is not total application RSS. No live weather solver runs. Frame timings are
settled offscreen draw costs, not streaming-game frame rates.

Equal-footprint mesh comparison after the climate foundation (16 m square):

| Render cell edge | Exposed faces | Rectangles | Mesh/atlas payload | Generation + merging |
|---|---:|---:|---:|---:|
| 6.25 cm, exact | 153,525 | 91,737 | 3.39 MiB | 124.00 ms |
| 12.5 cm | 37,862 | 23,408 | 0.86 MiB | 80.39 ms |
| 25 cm | 9,409 | 5,998 | 0.22 MiB | 23.04 ms |
| 50 cm | 2,337 | 1,521 | 0.06 MiB | 2.77 ms |

Preset checks, same seed and wind (25 degrees). Generation 2.4–2.8 s in separate
core runs, regional arrays/bins 45.65–47.20 MiB. Precipitation is annual-equivalent
model output, not a real-world forecast:

| Preset | Mean rain, mm/year | Mean moisture index | Example diagnostic coverage |
|---|---:|---:|---|
| Temperate | 947 | 0.42 | 23.9% grassland, 9.7% woodland suitability |
| Continental | 396 | 0.26 | 33.3% desert, 20.6% arid scrub |
| Arid | 123 | 0.07 | 71.9% desert, 0.6% wetland |
| Subpolar | 307 | 0.56 | 73.1% tundra, 13.2% alpine |
| Tropical | 1,403 | 0.38 | 22.4% grassland, 6.8% woodland suitability |

Warm tropical regions also have higher potential evaporation; tropical does not
mean uniformly wet forest. Woodland is suitability, with no trees generated.
Landform proportions remain independent of the preset: 20.5% plains, 13.0% hills,
25.1% plateau, 11.0% basins, 30.4% mountains; gentle ground (slope < 0.15) is
40.7–42.0%. Distributions are sampled on a deterministic 128×128 grid.

Reproduce maps without graphics through `./build/environment 42 temperate
output/environment 25` or export PNGs with the native `--debug-maps` option.
Use M in the native window for the twelve diagnostic views. Detailed references
and modeling limits are in [TERRAIN_CLIMATE_RESEARCH.md](TERRAIN_CLIMATE_RESEARCH.md).


## Procedural character asset preview — 2026-10-28

Release build, seed 42, same Radeon 760M. One idle character, approximately 1.75 m
high, generated from the same meter-scale recipe at three cell sizes. This is an
asset/export measurement, not animated-entity throughput in a running world.

| Cells/m | Occupied voxels | Dense frame bytes | Exposed faces | Merged rectangles | Observed generation |
|---|---:|---:|---:|---:|---:|
| 16 (current world scale) | 1,304 | 48,000 | 1,092 | 87 | ~0.06 ms |
| 32 | 10,032 | 384,000 | 4,388 | 101 | ~0.75 ms |
| 64 | 81,836 | 3,072,000 | 17,844 | 300 | ~4.59 ms |

Frame size includes air inside the bounded local volume. The pose cache retains
at most 16 frames and 16 MiB per resolution in the preview: native-scale frames
occupy 768,000 bytes at the 16-entry limit, while 64 cells/m retains five frames
under the same byte budget. External Arc owners and mesh scratch are not included.

The complete three-resolution static and four-clip APNG export took about 1.08 s,
including GPU readbacks and PNG encoding. Export rebuilds/uploads inspection
meshes; persistent per-entity GPU pose caching remains future integration work.
Fine resolutions here do not alter the world's canonical 16 cells/m resolution.
Reproduce with `--character-preview output/character`; source/design references
are in [PLAYER_RESEARCH.md](PLAYER_RESEARCH.md).


## Walking player and cameras — 2026-10-28

Seed 42 / temperate, Radeon 760M, release build, 1280×720. Player spawn now
prefers gentle soil-covered terrain, so this is a different scene from earlier
flight and overview measurements.

```sh
./native/target/release/microvoxel-game --player-test --frames 30 --screenshot output/player_game.png
```

The generated-world smoke test walks for two simulation seconds, settles,
jumps and lands, checking exact character occupancy against terrain throughout.
It then loads/captures all three camera views. One observed run used about
65 ms CPU wall time for the movement phase (roughly 301 fixed ticks plus explicit
full-body validation, streaming work and camera updates). This is not a pure
collision microbenchmark. The jump rose about 0.81 m and ended grounded.

A settled first-person view measured 1.53 ms per synchronized offscreen frame
(30 frames). Retained terrain GPU mesh data was 4.69 MiB; character GPU meshes
are separate and bounded to 16 native-resolution pose/visibility entries.
As before, this excludes window presentation/vsync and continuously changing LOD.

Collision skips interior character cells using six leading masks. Pose changes
check conservative envelopes of changed occupancy; unchanged feet do not block
an arm-only change. CPU pose/body banks and GPU mesh caches are bounded at 16
entries. The character is not written into terrain edits, and no runtime material
properties, rigid-body engine or water simulation were added.


## Look-following refinement — 2026-10-28

The separate head/torso look controller and aimed first-person forearm passed
native walking/jumping and look-up/down checks on the same Radeon 760M. One
settled first-person run measured 1.24 ms/frame over 10 synchronized offscreen
frames; the movement phase with full occupancy validation took 56.26 ms. These
are scene/export checks, not steady moving gameplay frame-rate guarantees.

The refined recipe uses true 3D limb transforms and explicit forearm ownership.
At native resolution the neutral standing model has 1,316 cells, 1,100 exposed
faces and 122 merged rectangles. Its cached frame occupies 58,336 bytes including
ownership indices; the earlier asset table predates this refinement. Look poses
have larger local bounds where required by raised arms. All CPU/GPU pose banks
remain bounded, and first-person geometry is still sampled onto the world grid.

## LOD controls, conformity and streaming — 2026-10-28

Release build, seed 42, temperate climate, Radeon 760M (RADV PHOENIX), 1280×720.
The deterministic `--lod-test` runs quality changes sequentially at the same
camera, shrinks the terrain cache, then flies 60 m while rotating over 180 paced
60 Hz frames. Static measurements synchronize the GPU after each frame; these
are rendering costs, not a promise of equivalent FPS in every interactive scene.
No live weather/fluid solver runs. Native radius is 6 m in each comparison; the
named menu presets also change radius/effects and are not identical to this matrix.

| Quality / cache target | Requested / effective projected cell size | Tiles / native | Frame median / p95 / p99, ms | Retained mesh cache, MiB |
|---|---:|---:|---:|---:|
| 25 / 128 MiB | 9.44 / 9.44 px | 408 / 52 | 2.88 / 3.72 / 3.74 | 66.08 |
| 75 / 128 MiB (balanced defaults) | 1.94 / 4.73 px | 979 / 148 | 5.42 / 7.08 / 7.30 | 66.59 |
| 100 / 256 MiB | 1.00 / 3.05 px | 1870 / 228 | 9.74 / 13.34 / 13.47 | 134.80 |

Retained cache includes inactive reusable meshes from preceding cases; it is not
active-view payload alone. Effective quality is deliberately shown when the tile
working target prevents the requested pixel size. Larger memory settings permit
larger planning targets, up to the configurable `--tiles` limit (default 8192).
Maximum quality is a screen-sampling request, not a certified terrain-error bound.

- Regional world generation: 2.75 s; initial complete target cover: another 5.96 s.
- Moving work (CPU update + synchronized GPU draw), median/p95/p99:
  **8.52 / 11.13 / 12.63 ms**. 111 of 180 frames included a LOD fade.
- Moving CPU update: **0.46 / 2.39 / 3.67 ms**; synchronized draw:
  **7.77 / 10.05 / 11.71 ms**. These exclude the test's pacing sleep.
- Final settled draw after travel: 5.35 ms/frame over 60 synchronized frames;
  423 visible tiles, 107.00 MiB retained mesh cache.
- Shrinking from the higher cache setting to 64 MiB ended at **63.96 MiB**, with
  a complete resident cover and a reported 9.31 px effective target.
- The entire multi-quality/memory/movement check took 39.28 s; peak process RSS
  was 258,412 KiB (252.36 MiB). RSS is not total GPU/driver memory.

The pipeline preserves merged interior rectangles, splits actual T-junctions and
uses shared finer-grid vertices at tile borders. In the fixed ground region
(x 0–1279, y 400–719), a simple bright-pixel diagnostic counted 21 seam specks before
conforming triangulation and zero afterward in all three quality captures. This
is supporting visual evidence, not proof for every seed/view; geometric coverage
and triangulation tests supply separate invariants.

Main-thread spikes were reduced by background selection/triangulation and by
rejecting illegal merges before cloning and rebalancing an entire candidate
layout. Stationary quality costs are intentionally higher than the old renderer's
coarser, less complete presentation; no blanket speedup is claimed.

Reproduction:

```sh
cargo run --locked --release --manifest-path native/Cargo.toml -- \
  --lod-test --frames 60 --screenshot output/lod/final.png
cargo run --locked --release --manifest-path native/Cargo.toml -- \
  --player-test --frames 5 --screenshot output/lod/player_check.png
```

The player check passed all camera views and look directions, 301 walking/jumping
fixed ticks, an approximately 0.81 m jump and a grounded landing. The real-window
check passed Esc, editable numbers, slider input, effect toggling, saved presets,
Resume, F5 and F12. It restored balanced settings and captured the menu.
The opt-in GPU test passed opaque old/mid/new transition coverage and preservation
of active meshes when an upload cannot fit. Full core/native suites and the
system-Rust dependency-free build passed; generated captures/logs stay ignored.


## Terrain presentation review (2026-10-28)

The two 1440p maximum-settings review rounds, actual per-view logs, source
references, fixed-plane lake seam regression and measured costs are documented
in [terrain presentation](TERRAIN_PRESENTATION.md). Final cache footprints were
224.81 MiB (valley), 447.84 MiB (lake), 165.47 MiB (ground); synchronized five-frame
means 45.29/37.12/33.39 ms respectively on the Radeon 760M. These short measurements
and the changed default FOV do not establish an FPS improvement over earlier runs.


## Selectable ray casting and revised streaming (2026-10-28)

See [renderer/streaming notes](RAY_RENDERER_AND_STREAMING.md) for the reference
review, sparse column BVH format, conservative visibility algorithm and limitations.
No terrain triangles are extracted by the ray backend; the player remains rasterized.

Final `--stream-test` on the Radeon 760M, Vulkan, 960×540, 512-tile ceiling.
Four seconds walking, four sprinting, two turning; each route traveled 37.10 m.
Collision remained valid. All four cases recorded zero native-detail misses at
the nine tested locations within 3 m of the camera, with up to 17 outstanding
queued/active/completed/planning jobs. All four subsequent live backend switches
settled. These short routes do not establish universal streaming deadlines.

| Backend / configuration | Initial cover | Work ms p50 / p95 / p99 | Resident data | Committed ray arena |
| --- | ---: | ---: | ---: | ---: |
| Mesh / laptop, 96 MiB | 0.95 s | 6.21 / 11.40 / 15.86 | 39.92 MiB | 0.50 MiB |
| Ray / laptop, 96 MiB | 1.17 s | 14.11 / 23.39 / 25.89 | 83.37 MiB | 96 MiB |
| Mesh / balanced, 256 MiB | 1.36 s | 6.92 / 13.01 / 17.13 | 51.78 MiB | 0.50 MiB |
| Ray / balanced, 256 MiB | 1.56 s | 13.75 / 24.43 / 31.45 | 111.48 MiB | 128 MiB |

Work time includes CPU update, drawing and GPU synchronization; pacing sleeps
are excluded. Laptop configuration uses quality 45, native radius 4 m; balanced
uses quality 75, radius 6 m. The ray arena grows in blocks, so committed capacity
can exceed live payload size. CPU summaries, color/build temporaries, render
targets, depth and driver allocations are additional memory. Arena growth can
briefly hold both buffers; ordinary allocations leave 1/8 capacity for retirement.
Reports: `output/streaming_check.log` and `output/streaming/{mesh,ray}_{0,1}.txt`.

The matched screenshot script uses different fixed settings: 1920×1080, quality
75, native radius 6 m, budget 256 MiB, tile ceiling 768. Both backends selected
the same 597-tile valley and 669-tile ground covers, effective target 11.55 px.
Twenty synchronized offscreen frames averaged 5.00/5.84 ms for mesh valley/ground
and 15.37/19.30 ms for ray valley/ground. Resident data was 15.17/16.86 MiB for
meshes versus 84.51/96.65 MiB for rays (128 MiB committed arena). These are
scene-specific prototype comparisons, not an established ray-casting speedup.

The whole-brick sharing probe found one normalized geometric pattern among 16
flat bricks and 16 patterns among 16 procedural bricks. Complete payloads were
unique in both samples. It does not measure internal DAG-subtree reuse or future
repeated assets, and no DAG or interning was enabled.

The recorded runs above preceded the final top-level-capacity guard: its reserved
prefix increased from 0.5 to 1 MiB to include transient patch headroom beyond 8192
leaves. Initial mesh-mode arena commitment is now 1 MiB. The comparison captures
remain the same scene/layout; no rendering or scheduling policy changed in that
capacity adjustment.

## Native octree/DAG experiment (2026-10-29)

`--ray-study` now compares current columns, compact SVOs and deduplicated DAGs
using hardware-timestamped, validated ray batches on the same Radeon 760M.
For two native terrain bricks, DAG payloads were 31–52% smaller than columns,
but compute traversal with color lookup was 1.49–1.86× slower. DAG and SVO times
were effectively the same. A carved-cave test favored octree traversal by about
3.47×. These are small, instrumented kernel tests, not full-world speedups.
The production renderer/storage remain unchanged; detailed tables, frame profile,
subtree sharing, edit snapshot experiment and caveats are in
[the study](OCTREE_DAG_STUDY.md).
