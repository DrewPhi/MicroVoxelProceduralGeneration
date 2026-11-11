# Guide for developers

## Current checkpoint

The default `./run.sh` preview now uses the adaptive renderer with Regional
terrain. Press **Esc** for live sun, atmosphere and exposure controls; image
quality and loading settings use **Apply quality & loading**. See the
[interactive visual preview](frontier/VISUAL_PREVIEW.md) for the current controls,
material-painting scope and outstanding integrated validation.

Adaptive loading now defaults to eight refinement selections per coherent
transaction (at most four builders), cached GPU draw commands, and unchanged
voxel/reference quality. `--adaptive-refinements 1..32` exposes the batch size;
`--adaptive-disk-cache PATH --adaptive-disk-mib 768` enables the optional,
disposable persistent cache. See [loading measurements](frontier/LOADING_OPTIMIZATION.md)
for cold/restart timings, opt-in experiments and the still-failing motion and
multi-kilometre quality/performance gates.


For rendering and LOD changes, start with the [primary player-view visual check](frontier/PRIMARY_VISUAL_CHECK.md). It fixes the original spiky terrain and player-height camera against a 256-sample exact-voxel reference.

The default scene tests a CPU-canonical seeded FBM source using adaptive
terrain clusters. The explicit `fbm` scene retains experimental moving nested
GPU grids. Background workers prepare coherent coverage; native
voxel identity remains 0.0625 m. All implementation is greenfield (tier G).
The [prototype checkpoints 3–5](frontier/PROTOTYPE_CHECKPOINT.md) record the current
measurements and remaining gates.

| Scene or option | Current scope |
|---|---|
| `fbm` | Moving surface-only nested grids; `--surface-layout clipmap` is the default |
| `fbm-native` | Exact bounded FBM native crop |
| `hybrid` | Moving native terrain chunks, approximate collar and GPU surface proxy |
| `hybrid-static` | Earlier fixed CPU hybrid reference |
| `hybrid-topology` | Moving hybrid plus a fixed 16 × 16 m canonical cave/bridge/edit owner |
| `adaptive` | Candidate projected-error cluster selection with canonical appearance summaries; `--pixel-error` controls requested geometric fidelity |
| `adaptive-topology` | Same candidate with exclusive exact cave/bridge/edit ownership |
| `fixture --renderer mesh` | Editable bounded mountain/tunnel/bridge greedy-mesh reference |
| `fixture --renderer brick` | Exact bounded brick-ray comparison |
| `fixture --renderer brick-skip` | Same traversal with conservative empty-brick distance skips |
| `fbm --surface-layout quadtree` | Static quadtree surface selection probe; automatically freezes coverage |

Topology geometry stays exact and resident at every distance within its fixed
ownership zone. Both ordinary native terrain and every proxy level exclude that
zone. Edits publish their candidate canonical world and replacement volume mesh
together; ordinary terrain chunks remain independent of those edits. This proves
a bounded implementation path, not arbitrary world-wide volume streaming.
Distant exact/proxy perimeter seams in the legacy hybrid scenes remain uncertified. See the
[ownership notes](frontier/OWNERSHIP_CHECKPOINT3.md).

The brick comparison is restricted to `--scene fixture`, with the same materials,
voxel lattice, halo face ownership and local-coordinate depth convention as the
mesh. `brick-skip` is a conservative cache experiment, not the published NAADF
implementation. Use `--msaa 1` for mesh/ray image comparisons. The quadtree probe
uses hard LOD selection and remains a static experiment, not a production CDLOD
implementation or a declared winner. The Alpine adaptive preset adds grounded walking and wood collision. Other
experimental scenes retain free flight. Persistent edits remain unimplemented.
See the [Alpine vegetation beauty checkpoint](frontier/reference/BEAUTY_CHECKPOINT.md)
for near-tree shadows, voxel ambient occlusion, procedural meadow variation and
the current sky/fog profile.

## Adaptive loading controls

See [loading optimization and measurements](frontier/LOADING_OPTIMIZATION.md).
Ordinary interactive launches default to **Development** resources; finite runs
(`--frames`, `--seconds`, captures or reports) preserve **Constrained** defaults
unless explicitly overridden. **Custom** permits manual budgets. These profiles
are not measured FPS guarantees: development targets 1080p at 60 FPS where
possible, with 30 FPS acceptable.

Press **Esc** to adjust the profile, minimum native terrain radius, near plant
detail distance, CPU/GPU caches, build memory, triangles, workers and uploads.
**Apply quality & loading** preserves valid terrain/source caches and current
GPU owners; MSAA changes rebuild compatible pipelines and attachments.
Development initially requests 4 m native terrain and 12 m near plant detail;
Constrained uses zero for both, preserving the earlier selection policy.
Native terrain remains canonical 6.25 cm; larger radii can be costly.

Development grows lazy working budgets from measured available RAM, accounting
for CPU and shared GPU memory together. It leaves at least 1 GiB or 30% of
available RAM as headroom and recommends at most half of total RAM. The startup
snapshot stays fixed until **Refresh available RAM**; refreshing during play
includes current game residency. A separate check at most once per second
pauses new terrain builds/uploads and plant job submissions under memory
pressure, then resumes with hysteresis. Existing owners remain drawable and
bounded plant jobs may finish. This is not a whole-process memory cap or driver
memory reclamation. Frame pacing adjusts throughput without automatically
changing geometry quality, vegetation density, view distance or fog.

From `frontier/`:

```sh
./run.sh --preset alpine --resource-profile development --native-radius 4 --vegetation-near 12
./run.sh --resource-profile constrained --frames 120
```

Manual flags include `--terrain-cache-mib`, `--build-memory-mib`,
`--terrain-gpu-mib`, `--plant-cpu-mib`, `--plant-gpu-mib`, `--triangle-budget`
and `--upload-mib`; they select Custom while retaining other profile values.
Existing `--adaptive-cache-mib`/`--adaptive-build-mib` aliases remain supported.
`--adaptive-workers 1..16` permits at most four simultaneous builders;
`--adaptive-refinements 1..32` controls selections per coherent replacement.

Persistent derived geometry is opt-in: add
`--adaptive-disk-cache output/terrain-cache --adaptive-disk-mib 2048`.
It is disposable, versioned render data, not a save. A restart can reuse it;
changed source identity or corrupt entries fall back to generation. First-time
exploration still requires generation. See [cache details](frontier/DERIVED_DISK_CACHE.md).

## Build and run

The `frontier/reference` crate is an independent CPU exact-voxel image oracle.
It uses canonical materials but no renderer meshes or shared ray traversal.
See [reference usage](frontier/reference/README.md) and the
[measured checkpoint](frontier/REFERENCE_CHECKPOINT.md). From `frontier/`, run
`python3 tools/reference_gate.py --output output/reference-gate` to build, render
64/256-sample references, capture the same GPU camera and compare them. This
analysis command needs NumPy, Pillow and a working graphics driver; it is not
an interactive-performance benchmark. Generated raw buffers remain ignored.

`.github/workflows/frontier-ci.yml` gates this branch on formatting, locked Rust
tests (including CPU WGSL validation), the release build and CPU reference checks.
Hardware performance and visual acceptance remain separate measured gates.

Install Rust 1.99.1 with rustup; the workspace pins it. Linux needs an X11 or
XWayland session, Vulkan/OpenGL drivers and the usual C linker/build tools.

```sh
cd frontier
cargo test --workspace --locked
cargo build --release --locked
./run.sh --scene fbm --seed 7 --camera valley
./run.sh --scene fbm --seed 7 --camera aerial --view-levels 11
./run.sh --scene fbm-native
./run.sh --scene hybrid --seed 7
./run.sh --scene hybrid --seed 7 --camera aerial --msaa 4 --voxel-filter off
./run.sh --scene hybrid-static --seed 7
./run.sh --scene hybrid-topology --camera topology
./run.sh --scene hybrid-topology --camera cave
./run.sh --scene hybrid-topology --camera bridge
./run.sh --scene hybrid --teleport-test --seconds 30
./run.sh --scene fixture --renderer mesh --msaa 1
./run.sh --scene fixture --renderer brick --msaa 1
./run.sh --scene fixture --renderer brick-skip --msaa 1
./run.sh --scene fbm --surface-layout quadtree --camera aerial
./run.sh --scene fixture --capture output/fixture.png --width 1920 --height 1080
./run.sh --seconds 15 --route stop-resume --route-clock realtime --warmup 30 --uncapped --report output/run.json --width 1920 --height 1080
```

Create the output directory before capture (`mkdir -p output`). `run.sh` runs
an existing release binary; rebuild after source changes. The finite window run
prints adapter information and frame interval percentiles, including presentation
wait. When the adapter supports timestamp queries, JSON reports additionally
record isolated render-pass GPU percentiles automatically; unsupported or absent
samples remain null. Frame intervals and GPU render-pass timings measure different
parts of a frame. Neither alone establishes production performance acceptance.

Click the window to capture the mouse. WASD moves horizontally, mouse looks,
Space/Ctrl moves vertically, Shift accelerates, Esc opens settings and releases the mouse, F12 saves
a PNG. Aim at screen center: left click removes a cell, right click places stone
within 8 m in the fixture or inside the `hybrid-topology` ownership zone. One
background rebuild runs at a time; wait for it before editing again. Edits are
session-only. This is a free-fly camera, not a colliding player controller.

Press **T** in `hybrid` or `hybrid-topology` to prepare a destination away from the
starting position, then return on the next press. The current camera remains
presented until coherent destination coverage reaches GPU publication. A pending
teleport displays a preparation state. `--teleport-test` exercises scripted away
and return destinations in these moving hybrid scenes; readiness timings are
reported separately from frame intervals. This bounded test is not a claim that
arbitrary cold destinations are instantly ready.

## Layout

| Path | Purpose |
|---|---|
| `frontier/world` | Integer voxel source, materials, COW edit snapshots, local coordinates and reference DDA |
| `frontier/renderer` | Exact greedy surface reference mesh, wgpu window/headless capture and camera |
| `frontier/runtime` | Bounded cache/scheduler policy and frame statistics; not a production streaming service |
| `frontier/tools` | Launcher and independent mesh-versus-world ray parity tests |
| `frontier/VALIDATION.md` | Reference cases and proposed laptop quality/performance gates |
| `frontier/ARCHITECTURE.md` | Production design and staged renderer decisions |
| `frontier/AGENTS.md` | Scope, reuse and measurement rules |

See [moving coverage measurements](frontier/STREAMING_CHECKPOINT.md),
[FBM checkpoint measurements](frontier/FBM_CHECKPOINT.md) and the earlier
[native reference checkpoint](frontier/CHECKPOINT.md) for tests, hardware results
and explicit limitations.
The [moving native checkpoint](frontier/MOVING_NATIVE_CHECKPOINT.md) records
moving native/proxy measurements and remaining limits. The earlier
[handoff checkpoint](frontier/HANDOFF_CHECKPOINT.md) documents the fixed hybrid
reference and the 7/9/11-level view-distance experiment. These earlier checkpoints do not establish an invisible transition or arbitrary
volume ownership; the new bounded topology experiment is described separately.

## FBM controls and repeatable captures

`--seed`, `--scale` (horizontal base wavelength in meters), `--amplitude` (vertical
relief in meters), `--octaves` and `--roughness` configure the source. These change
the landscape, not the voxel size. `--camera valley|aerial` selects a starting view.
FBM defaults: seed 7, scale 1800 m, amplitude 1000 m, 8 octaves, roughness 0.5.
The source combines warped gradient noise and ridged FBM; it is not an erosion,
hydrology or tectonic simulation. Regional painting uses landform slope, elevation and continuous moisture for grass, soil and exposed rock; see the [painting checkpoint](frontier/REGIONAL_PAINTING.md).

WASD speed is 35 m/s (Shift 80 m/s) in the broad FBM scene, 3 m/s (Shift 15 m/s)
in the hybrid and native references. Free flight has no collision. The broad scene does not
use infinite resident terrain: its outer grid covers ±2048 m around its published
center, with 0.5 m finest to 32 m outer spacing. Levels recenter independently;
generation can lag rapid travel. Going outside loaded coverage or below ground
is possible. `--view-levels 7..11` extends the outer half extent from 2.048 km
to 32.768 km without changing the finest spacing. The broad FBM scenes disable editing; `hybrid-topology` permits it only inside
its explicit volume owner. Topology cameras are selected with `--camera
topology|cave|bridge` in that scene.

`--msaa 1|4` selects raster sample coverage (default 1), including headless
captures. `--voxel-filter on|off` toggles the shared proxy/native appearance
filter for comparison (default off; experimental); it does not change canonical voxel identity
or establish a geometric error bound. The fixed `hybrid-static` reference does
not use the moving GPU-page appearance path.

Run `./tools/smoke_streaming.sh` for real-time stop–resume and turn–return routes,
end-of-route PNGs, and streaming metrics. `--route-speed` sets benchmark speed
in m/s (default 80 for FBM, 3 for moving hybrid). High-speed stress runs require
their own coverage and frame-tail measurements; no maximum supported travel
speed is established by the prototype. `--terrain-static` freezes initial
coverage for comparison; `--seconds` limits wall-clock run duration and
`--capture-on-exit` captures the final displayed scene.

Run `./tools/smoke_fbm.sh` after building for a seed/camera PNG sweep and a
360-frame stop–resume report. `--route static|fly|stop-resume|turn-return` uses
absolute simulated time `frame_index * --route-step` (default 1/60 s), independent
of achieved FPS. Broad FBM benchmark routes stay at least 10 m above the canonical
surface; hybrid routes clamp to at least surface +1.7 m. The hybrid starts with
its eye about 1.7 m above the canonical surface. Interactive free flight has no
ground clamp or collision. These are repeatable synthetic workloads,
not measurements of streaming at actual real-time travel speeds unless
`--route-clock realtime` is selected.

For a deterministic headless route pose, use `--capture-route-time SECONDS`
with a selected route. The pose is applied before initial native/proxy readiness
work, so capture does not depend on wall-clock travel speed:

```sh
./run.sh --scene hybrid --seed 7 --route fly --route-speed 3 \
  --capture-route-time 4 --capture output/hybrid-route-4s.png \
  --msaa 4 --voxel-filter off --width 1920 --height 1080
```

JSON reports include source parameters, native size, representation, camera,
route, adapter, warmup and frame percentiles. GPU render-pass timestamps are
collected automatically when supported; total process RSS/VRAM remain unmeasured.
Backend, brick residency, teleport readiness and prediction metadata accompany
the corresponding experiments. `--capture` and `--report` are intentionally separate
modes. Warmup frames are excluded from frame statistics. Keep raw artifacts under
ignored `frontier/output/`. Moving hybrid reports additionally include native
CPU cache/queue counts, GPU residency and cache reuse, chunk upload bytes/time,
coherent publications, generation time and coverage lag. Native vertex/index
writes are capped at 8 MiB per frame; collar and proxy-page uploads are separate.
The GPU cache targets 128 chunks / 512 MiB, protects active and incoming covers
during replacement, and rejects transitions above a 1 GiB mesh admission limit.
These mesh figures are not total process RSS or total GPU memory.

## What remains

The new selectable path is documented in
[adaptive rendering](frontier/ADAPTIVE_RENDERING.md). It implements conservative
screen-space selection and shared-edge joins without the old fixed native
square. Keep its quality/readiness and memory diagnostics alongside FPS: a fast
coarse fallback is not a successful quality benchmark. Use
`--adaptive-wait-quality` for settled-performance runs and omit it to measure
cold streaming. Existing scenes remain available as independent comparisons.

The legacy `fbm`/`hybrid` surface prototypes do not yet implement toroidal GPU strip uploads,
a production error-driven LOD selector, seamless exact-near voxel handoff,
world-wide cave/edit ownership, persistent edits, walking collision, or production
geography. The static quadtree and fixed topology zone are bounded probes of
those requirements. The
simple footprint filter has no certified silhouette bound. Do not claim G0–G4
passed from these tests. Near-surface smoothing in the broad proxy is a known
prototype limitation, not the final voxel appearance contract.

The native reference uses a complete halo and never caps solid crop boundaries;
its cutaway edges are intentional. World generation stays CPU-canonical and
samples are uploaded to the GPU, avoiding a second noise implementation.
See `frontier/world/README.md` for source and boundary semantics.

## Legacy references

Read the [master issue](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/24)
and both Frontier planning files before implementing later stages. Old engines
remain on their remote branches; inspect with `git show` or a separate benchmark
checkout. Do not restore old source trees here. Generated historical artifacts
remain in the [archive release](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/releases/tag/legacy-research-archive-2026-11-04).

Latest transition work: [four-pass LOD experiment](frontier/LOD_POLISH_CHECKPOINT.md).
4×MSAA and `--voxel-filter on` are now defaults (filter enabled by user preference).
The CLI defaults to the regional recipe; `--terrain-recipe legacy` preserves the
original world. See [regional terrain results](frontier/REGIONAL_TERRAIN_CHECKPOINT.md)
and use `--start-xz X,Z` to inspect another physical region.

Current lighting and deterministic terrain color: [beautification notes](frontier/BEAUTY_CHECKPOINT.md).

Procedural vegetation lives in `frontier/world/src/vegetation.rs`; its canonical
records, voxel models and collision queries are independent of display LOD.
`renderer/src/vegetation.rs` streams placement and cached face meshes,
`vegetation_gpu.rs` batches instances, and `vegetation_scene.rs` integrates them.
`walking.rs` provides the small axis-swept walking camera. See
[vegetation controls, budgets and validation](frontier/VEGETATION.md).

The [cozy meadow checkpoint](frontier/COZY_SCENE.md) describes the current Alpine procedural vegetation, cache budgets, HDR scene target and bounded shadowed-air/cloud passes. Plant geometry is derived from each canonical record seed; caches retain generated products rather than choosing prebuilt assets.

[Playability and motion checks](frontier/PLAYABILITY_CHECKPOINT.md) describe the bounded appearance-node cache, nearby refinement priority, event-driven window sizing, physical-versus-render camera state, full-render GPU query span and frame observation ring. Use `tools/record_walk.py` for window-scoped walking/turn captures; use unrecorded `walk-*` routes for timings.

[Vegetation streaming and GPU representation](frontier/VEGETATION_STREAMING_CHECKPOINT.md): `vegetation_mesher.rs` retains exact reference parity while avoiding nested ordered-map extraction; `vegetation_gpu_parity.rs` is an opt-in hardware test of frozen plant content. `FRONTIER_VEGETATION_GPU` selects compact/compact-indexed/expanded for controlled experiments; compact is the measured default.

## Optional NAADF radius experiment

Esc → **NAADF experiment** exposes an enable switch, 1–256 m cube radius, build
budget, status and GPU payload. Apply changes, or use **Rebuild NAADF here** to
recenter. Start with `./frontier/run.sh --preset alpine --naadf-radius 64`.
The exact voxel window follows the player by default and reuses overlapping
strips. Distant terrain retains the existing backend. See the [streaming design
and measurements](frontier/NAADF_STREAMING.md) and the [mesh handoff, packing,
and ray profiling results](frontier/NAADF_OPTIMIZATION.md).

NAADF optimization experiments and their reproducible configurations are documented
in [the seven-experiment report](frontier/NAADF_SEVEN_EXPERIMENTS.md). Experimental
flags are opt-in; use matched full-frame and streaming measurements before promotion.

The [NAADF optimization follow-up](frontier/NAADF_OPTIMIZATION_FOLLOWUP.md) records
why individual acceleration experiments help or lose, with canonical GPU tests,
matched camera/motion measurements and compiler evidence. `FRONTIER_NAADF_GPU_STAGES=1`
adds opt-in asynchronous compute-stage timings to reports/capture metadata;
`FRONTIER_NAADF_DIAGNOSTICS=1` adds traversal counters for diagnostic captures.
Use ordinary uninstrumented runs for FPS comparisons. The default CPU-proof
traversal remains the recommended path; experimental switches are documented.

Default launch (2026-11-11): adaptive scenes enable the NAADF/raster hybrid at
64 m radius with following enabled, CPU distance proofs, and single-sample
rendering unless MSAA is explicitly specified. Scatter uploads default on;
`FRONTIER_NAADF_SCATTER_UPLOAD=0` restores individual writes for comparisons.
`--naadf off` restores the raster path. Reference/fixture scenes retain their
existing backend; historical experiment configurations explicitly pin scatter
off where required. Ignored historical captures, videos, derived caches and
debug builds were removed locally; source and checked-in references are retained.

The [post-Teardown optimization checkpoint](frontier/NAADF_NEXT_CHECKPOINT.md)
records shader specialization, bounded exact column caching, lighting and brick/RT
experiments, matched 1080p walking gates, and the shared-instance vegetation
cache-eviction fix. New performance modes remain opt-in; the ownership fix is
active. The hardware lab is a standalone workspace, not a game dependency upgrade.

Distant procedural tree coverage and the rejected ground-cover experiment are
documented in [frontier/DISTANT_VEGETATION.md](frontier/DISTANT_VEGETATION.md).
