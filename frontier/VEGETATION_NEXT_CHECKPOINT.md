# Vegetation next checkpoint — 2026-11-08

Base: `0a6ca71`, following [VEGETATION_STREAMING_CHECKPOINT.md](VEGETATION_STREAMING_CHECKPOINT.md).
This pass combines exact CPU preparation work with bounded renderer experiments.
It keeps canonical 6.25 cm cells, vegetation source version 3, full plant seeds,
placement, physical size, pigments, terrain source/detail settings and the horizon.
Reuse tier is G: new Frontier code; no historical renderer/runtime is transplanted.
The independent recipe test oracles preserve the pre-change arithmetic from
`world/src/vegetation.rs@0a6ca71`.

**Status: the corrected default lifecycle has been built and timed, workspace
checks pass and the fixed primary output is unchanged. Architectural labs are
measured but fail their promotion gates; before/after recorded-motion review
is complete with the limits below.** The native sparse-DDA lab currently
fails strict parity at a few edge pixels and is not promoted. The tables identify
each measured executable separately. This does not close the laptop 60 FPS gate
or establish that all terrain, loading and shadow cases are resolved.

## Work and evidence map

| Work area | Implementation / current acceptance |
| --- | --- |
| Compact procedural plant structure | `plant_recipe.rs`: grass strokes, Rowan wood/sprays; frozen CPU model oracles pass |
| Bounded GPU generation | `vegetation_generation_lab.*`: grass expansion measured; float-bit parity and dense-grid memory gates fail, not promoted |
| Localized native voxel traversal | `vegetation_lab.*`: sparse native bricks; strict raster parity fails at a few edge pixels, not promoted |
| Distant aggregate appearance | Stride-4/8 solid versus axis-averaged coverage lab; measured approximation; useful distant error reduction, no runtime promotion |
| GPU storage/reuse | `vegetation_arena.rs` / pooled backend; measured, opt-in due reservation/mixed frame results |
| GPU visibility | `vegetation_cull.wgsl`: bounded conservative color clusters; independent direct shadows |
| Repeated instance preparation/uploads | Exact packed-byte reuse; rebase/fade/membership changes invalidate uploads |
| Terrain CPU preparation | Current-view math/ownership preserved; cached immutable accounting/partition/basis work; measured tails remain above target |
| Latency/velocity prediction | Measured CPU completion horizon, bounded swept demand and time-to-need job ordering; GPU upload deadline not certified |
| Stable shadow representation ownership | Default lifecycle, nearest feasible baseline priority, exact overlap accounting within 150k; motion reviewed with 20 FPS/sample limits |
| Near/far shadow composition and caching | Exact default depth caches; combined cascades measured and remain opt-in |
| Explainable validation/telemetry | Canonical-root event inspector, frame/worker/GPU spans, command hashes, frozen parity and primary comparison; movement and recorded samples reviewed |

## Exact work in the current renderer

### Terrain preparation

`adaptive_gpu.rs` now reuses immutable residency accounting and the allocation
behind its depth-sort scratch vector. It filters resident clusters to currently
published owners before sorting. This preserves the original relative depth/key
order and the current-camera occluder insertion order. Visibility, occlusion and
projected error are still evaluated from the current pose. A prepared-view hit
still requires the identical pose, projection, error setting and owner set;
upload/removal/filter changes invalidate that snapshot.

The f32 view basis is constructed once per frame. Each cluster retains its own
f64 world-to-local eye subtraction, f32 dot products and projection multiplication.
This avoids introducing a large global f32 camera origin. Occluded clusters no
longer construct an unused color matrix. The old scalar basis path and repeated
residency/sort work remain available through `FRONTIER_TERRAIN_PREPARE_REUSE=0`.

`adaptive_view.rs` retains scratch storage for residency keys and only rebuilds
that set/runs retention when the published owners or upload target change. A
published partition caches its integer bounds/area facts. Camera/frustum
containment is recomputed every frame; it cannot publish a stale-camera cover.
The shared partition guard also computes the same facts in one pass rather than
repeated scans. These changes do not relax the existing partition contract.

The report adds `adaptive.terrain_prepare_stages_cpu_ms`: shadows,
resident accounting/order, occluder construction, and visibility/error/uniform
upload. These are cumulative CPU wall durations, including preparation before
the measured frame interval. Bundle build time remains separately reported.
They must not be presented as GPU timings or summed with overlapping worker
wall times. Per-frame terrain preparation observations remain the source for
walking percentiles.

### Procedural growth recipes

`world/src/plant_recipe.rs` separates deterministic rule growth from native-cell
expansion. Grass emits ordered radius-zero strokes with rounded endpoints and
packed RGB8/material. Canonical `generate_grass` then materializes those strokes
through the existing native raster, rooted-component filter, pigment rules and
lookup-index construction.

Rowan emits its woody strokes and accepted ellipsoidal foliage sprays after the
existing seeded space-colonization/competition rules. These are growth structures,
not a run-length encoding of already generated cells. Materialization preserves
wood precedence, spray order, native pigment arithmetic and rooted connectivity.
Blossom selection still examines exposed expanded cells; its topology-dependent
CPU phase is retained and included in materialization time. This is not a claim
of full GPU tree growth.

The frozen CPU oracle compares complete bounds, ordered native cells/materials,
f32 colors and lookups: 120 grass seeds at all eight variants (960 models), and
120 Rowan seeds spanning all eight variants. Targeted recipe and existing world
vegetation checks passed before the integrated run. Source version remains 3.

### Placement prediction and upload reuse

Vegetation placement demand now sweeps toward measured horizontal velocity using
a rolling p95 of the last 64 queue-to-CPU-frame-ready completion observations,
plus 100 ms. Existing `max_tiles` and job/cache budgets bound the footprint.
Time-to-need orders jobs only after final representation/budget admission.
Stopping, turning and discontinuous movement update/reset the prediction; camera
changes still affect demand rather than canonical identity. The horizon does
**not** include measured GPU upload completion, because this pass does not add
an asynchronous upload fence. `FRONTIER_VEGETATION_PREDICTION=off` is the matched
comparison control.

Packed instance uploads are skipped only when their complete bytes and length
match. Origin rebases, canonical root, rotation, fade, shadow prefix and transition
metadata force a new upload. This preserves precision and ordering rather than
using a lossy instance signature.

## Storage and GPU visibility experiment

Compact pulled geometry remains the default. `FRONTIER_VEGETATION_GPU=pooled`
selects the experimental shared storage/instance arenas. On the comparison
adapter, reservation is 247.5 MiB geometry, 8 MiB instances and 0.5 MiB bounded
cluster/indirect tables, within the unchanged 256 MiB GPU allocation cap; geometry
reservation also respects the device's maximum buffer size. Reports distinguish
reserved and used bytes from live-template accounting.

The coalescing allocator aligns suballocations, retains inactive templates until
pressure and does not relocate live geometry. Reuse relies on ordered queue
writes after the preceding frame is submitted. Fragmentation/exhaustion returns
an explicit resource error and clears draw lists, instead of drawing stale-origin
content. It is not a sparse-residency or virtual-memory implementation.

Pooled color work is divided into clusters of at most eight instances, bounded
by 8,192 commands. A compute pass rejects a cluster only when all eight enclosing
AABB corners lie outside the same homogeneous clip plane, with an uncertainty
margin. Uncertain boundary clusters remain visible. It writes ordinary indirect
draw arguments; there is no visibility readback in gameplay. Shadow admission
and draws remain independent of color frustum rejection.

Color drawing falls back to the direct path when indirect execution is unsupported,
`FRONTIER_VEGETATION_CULL=off`, or the command bound is exceeded. This experiment
still encodes individual indirect draws; it is not a claim that one GPU command
replaces every vegetation draw call. The extended frozen GPU parity test covers
the actual pooled compute/indirect path and checks an offscreen command separately.
The frozen hardware test now passes across all four backends; measured pooled
performance and its memory trade are reported below. This does not establish
that the pooled path is preferable for all routes.

## Shadow stability experiments

Default exact depth caches reuse the far terrain and near plant maps when their
complete dependencies match. The far cache includes sorted published owner keys,
residency revision and world-snapped light projection. The near cache includes
actual sampling-uniform bits, camera eye/paint origin, sorted mesh identities and
the complete packed caster-instance prefix, including fade/transition state.
The cached depth lives with its owning texture. `FRONTIER_SHADOW_CACHE=off`
disables these caches for comparison.

The 300 ms tree-shadow entry, exit and LOD-replacement lifecycle is enabled by
default after GPU ownership fixes. Set
`FRONTIER_SHADOW_TRANSITIONS=off` or `0` to disable it; `1` explicitly enables it. Entry/exit reversal starts from current
coverage. Replacement geometry uses complementary canonical-cell masks; depth
values are never blended. Old geometry Arcs survive until the handoff completes.
The bounded lifecycle retains at most 512 roots, keeps all overlap within the
150k tree-shadow triangle budget and reserves existing exits before admitting
new entries. The complete 150k budget is available for primary coverage: there
is no fixed 15k overlap reserve. Feasible coarse/prior reservations are capped in
nearest-first order, so over-budget distant baselines cannot reject the closest
tree. Actual old + new + exit geometry must still fit 150k; a replacement/admission
is deferred when that overlap cannot fit. Regression coverage keeps all three
49k baselines, or the nearest three of four 49k baselines, without requesting
unselected refinements. The initial timing batch predates this default promotion:
`new-*` had lifecycle disabled, while `shadow-transitions-walk` enabled the earlier
lifecycle. Corrected default measurements are labeled `final-*` below. Recorded-motion
review and its sampling limits are retained with the evidence.

`FRONTIER_SHADOW_CASCADES=1` enables another independent experiment: terrain can
cast into the existing near plant map, and plants can cast into the existing far
terrain map. It creates no extra depth textures, but allocates a 4 MiB near-terrain
transform buffer rather than the default 256-byte placeholder. Near receiver-edge
fading and the existing far receiver policy remain explicit; depth is not blended.
Far foliage caster admission remains 90 m entry / 96 m retention, so this does
not establish 220 m tree-shadow coverage or global shadow coverage. Both exact
map caches are disabled in combined-cascade mode to avoid retaining changed
plant depths. Missing/off-map terrain casters and fixed volume owners remain
outside this bounded presentation experiment.

Per-frame `ShadowInspection` includes selected and requested mesh identities.
The bounded retained `shadow_inspector` JSON ring records canonical roots,
selected from/to LOD, reasons and actual GPU caster membership changes, with
monotonic timestamps. A requested-key change can trigger an event, but requested
LOD is not serialized into that retained JSON. The ring holds 4,096 events and
reports drops. It cannot infer which shadow texels were visible; recordings and
timestamps supply the visual context.

## Controlled architectural labs

`vegetation_lab` uses one frozen native seeded Rowan, a matched rasterized native
face baseline and local 4³ sparse-brick traversal. Brick masks prove empty space;
ranked values retain native cells/colors. The traversal has a finite step bound.
Coarse stride-4/8 solid occupancy and stride-4/8 axis-averaged projected-coverage
aggregates are separate display approximations. The aggregate is an empirical
axis average, not a direction-conditioned opacity function or certified
appearance-error bound.

The 256×256 lab compares distances of 2, 8 and 32 tree heights under the same
simplified Lambert lighting. Each representation has two warmups and seven timed
samples. Its `reference16` is a finite 4×4-sample native estimate, not ground truth.
The native brick/raster check reports all-pixel edge/coverage differences and
separately checks nonedge RGB/depth tolerances. The current hardware experiment
still fails strict native/raster parity at a few edge pixels after the traversal
precision correction. The diagnostic edge classifier is incomplete, tolerances
remain unchanged and no ray backend is promoted. The normal lab command can
finish all distances/generation and writes the failed parity result; the strict
ignored test still fails. The measured lab decisions below retain these failures.
Coarse methods report linear RGB RMSE and coverage MAE against that estimate. A favorable still or isolated kernel
time cannot establish moving foliage quality, full-scene performance or streaming
readiness.

The same lab's `procedural_generation` object runs bounded compute expansion of
16 grass recipes. GPU input consists of growth strokes and seeds, not CPU native
models. Ordered stroke ownership reconstructs exact occupancy/base pigment;
paired u32 arithmetic implements the existing 64-bit hash. Final f32 pigment
field/sRGB arithmetic is checked against the canonical CPU color, with bit
mismatch counts, maximum absolute error and nonfinite counts. A mismatch must be
reported, not silently promoted as source parity. CPU growth and materialization,
GPU raster/color spans, upload bytes, native bytes, bounded-grid bytes and
submit/readback wall time are separate. Dense grids/readback are headless-only;
`promoted` remains false. Eight CPU Rowan recipes also report structural bytes
and rule/materialization timings, including the exposed-cell blossom phase.
Descriptor bytes exclude Vec headers/capacity; native struct bytes exclude the
lookup index. These are not total process-memory claims.

## Research and provenance

- [Real-Time GPU Tree Generation — official sample](https://github.com/Bloodwyn/gptree)
  motivates separating procedural structure/expansion. Its sample requires the
  Work Graph Playground with mesh nodes. This pass copies no sample code and uses
  ordinary portable compute instead of claiming those hardware capabilities.
- [AMD procedural grass rendering](https://gpuopen.com/learn/mesh_shaders/mesh_shaders-procedural_grass_rendering/)
  studies procedural geometry and distant aggregate coverage. Its smooth blades
  and density/width trades are not substituted for canonical native blocks here.
- [Efficient Sparse Voxel Octrees — NVIDIA Research](https://research.nvidia.com/publication/2010-02_efficient-sparse-voxel-octrees)
  supplies relevant sparse-storage/ray-traversal context. This lab implements local
  fixed bricks and empty-brick skips, not that paper's full octree/contour renderer.
- [Epic virtual shadow maps](https://dev.epicgames.com/documentation/en-us/unreal-engine/virtual-shadow-maps-in-unreal-engine)
  motivates explicit cache invalidation and independent shadow workload. These
  bounded fixed maps are not an implementation of virtual shadow maps.
- [wgpu indirect drawing](https://docs.rs/wgpu/24.0.5/wgpu/struct.RenderPass.html#method.draw_indirect)
  and [queue buffer writes](https://docs.rs/wgpu/24.0.5/wgpu/struct.Queue.html#method.write_buffer)
  define the portable command/submission facilities used here. Published renderer
  timings from other engines do not transfer to this laptop or scene.

## Reproduction

Run from `frontier/`. Build and test before timing; do not overlap builds, separate
GPU experiments or recording with benchmark intervals. Use the frozen executable
and explicit cache path for each pair, and record its SHA-256, adapter/driver,
command/environment, world/source versions and cache state. The Alpine preset
uses a derived terrain disk cache; vegetation/model caches are process-local.
A process-cold vegetation run with a warm terrain disk cache must be described as
such. Retain current-camera readiness, selected geometry and missing content
alongside time measurements.

```sh
cargo build --release --locked -p frontier-tools --bin frontier --bin vegetation_lab
cargo test --workspace
cargo test -p frontier-renderer frozen_native_gpu_parity --lib -- --ignored --nocapture
python3 tools/primary_visual_check.py --output output/vegetation-next/primary-check
./target/release/vegetation_lab output/vegetation-next/local-lab
```

The local lab writes `report.json` and method/distance PNGs in its
chosen directory. Its readback and synchronization are part of the lab, not the
interactive renderer. The primary check remains the fixed Legacy player-height
terrain reference; an Alpine aerial still does not replace it.

The following is the fixed dense grounded 720p route template. Repeat it in a
fresh process for each switch and give every report a distinct path. These
commands are reproduction templates, not evidence that the run already passed.

```sh
./target/release/frontier --preset alpine --seed 7 --width 1280 --height 720 \
  --pixel-error 3 --msaa 4 --capture-far 6000 --voxel-filter on \
  --adaptive-disk-cache output/cozy-scene/map-cache --adaptive-disk-mib 1024 \
  --eye '-43.625,31.0625,-289.5625' --target '-43.625,31.3625,-259.5625' \
  --route walk-stop-resume --route-speed 3 --route-clock realtime \
  --seconds 16 --adaptive-wait-quality --warmup 60 --uncapped --benchmark-input-lock \
  --report output/vegetation-next/walk-template.json
```

For static comparison use `--route static`; for the 1080p angular-bound match use
`--width 1920 --height 1080 --pixel-error 4.5`. Keep all other source, camera,
lighting, MSAA, terrain-detail and internal-resolution settings matched. Native
output should retain render scale 1; reduced internal rendering is a separate,
explicit quality trade. Actual startup/walk/stop/resume/turn recording is separate:

```sh
python3 tools/record_walk.py --output output/vegetation-next/recording
```

Diagnostic/experiment controls (prefix the same route template):

| Control | Purpose/current default |
| --- | --- |
| `FRONTIER_TERRAIN_PREPARE_REUSE=0` | Disable reusable terrain accounting/sort/basis preparation; default enabled |
| `FRONTIER_PREPARED_VIEW_CACHE=0` | Disable identical-pose prepared-view cache |
| `FRONTIER_RENDER_BUNDLES=0` | Disable existing terrain command bundles |
| `FRONTIER_VEGETATION_PREDICTION=off` | Disable measured placement prediction; default enabled |
| `FRONTIER_VEGETATION_GPU=compact` | Existing compact pulled baseline/default |
| `FRONTIER_VEGETATION_GPU=pooled` | Opt-in shared arenas and supported indirect color culling |
| `FRONTIER_VEGETATION_CULL=off` | Direct color path within the pooled allocation experiment |
| `FRONTIER_SHADOW_CACHE=off` | Disable exact near/far depth caches; default enabled |
| `FRONTIER_SHADOW_TRANSITIONS=off` or `0` | Disable the promoted default lifecycle; `1` explicitly enables it |
| `FRONTIER_SHADOW_CASCADES=1` | Opt-in shared near/far terrain/plant depth composition |

## Initial controlled results

The initial diagnostic timing executable is SHA-256
`9500ad409fd33c9931d5c53bad9e45ec2f18c05e1fac6036abde9746fe0c7847`.
The corrected production/default timing executable is SHA-256
`7dd044637c686b1877f686323a9346816b1de3b6b1875becdc2e3df1b2dcc4a5`;
its runs are `final-720`, `final-walk` and `final-1080`.
Per-run `*.command.json` files under ignored `output/vegetation-next/` preserve
the complete command, environment overrides and executable hash. The frozen
baseline executable and `baseline-720.json`, `baseline-1080.json`,
`baseline-walk.json`, `primary-baseline.*` are retained there too. These sequential
runs use the Radeon 760M / RADV Vulkan / Mesa 25.2.8 adapter, native output,
4×MSAA, the fixed dense camera above, and the explicit
`output/cozy-scene/map-cache` derived terrain cache. The 1080p error target is
4.5 versus 3 at 720p, preserving the angular bound. Static runs last 6 seconds;
the grounded 3 m/s walk/stop/resume runs last 16 seconds and start after the
terrain-quality wait. These are settled-start route timings, not cold-start
walking latency tests. Terrain disk-cache warmth differs during startup; do not
attribute startup duration differences to these optimizations.

The following whole-frame values come directly from each report's measured
frame summary; CPU/driver preparation, GPU work and presentation contribute to
these intervals. Each row is one sequential run, not repeated-trial statistics.

| Matched route/output | Baseline p50 / p95 / p99, ms | Initial diagnostic candidate, ms | Corrected default (`final-*`), ms |
| --- | --- | --- | --- |
| Dense static, 1280×720 | 13.83 / 17.83 / 19.27 | 13.25 / 17.01 / 17.93 | 13.01 / 17.06 / 18.36 |
| Dense static, 1920×1080 | 21.64 / 26.02 / 27.65 | 20.83 / 25.65 / 26.95 | 20.76 / 25.27 / 26.22 |
| Grounded 3 m/s walk/stop/resume, 1280×720 | 17.54 / 30.31 / 34.86 | 18.35 / 29.86 / 34.35 | 18.30 / 28.66 / 33.71 |

All baseline, initial candidate and corrected default comparisons have 100%
measured terrain coverage and quality readiness, zero unbounded errors and zero
unresolved visible clusters. Walking median worsens while its tails improve.
Static frame percentiles improve in these runs. This is not a uniform speedup or
a passing 60 FPS result. Asynchronous representation history prevents a frozen-content reading:
mean walking plant triangles are 952,813 baseline versus 953,519 corrected;
endpoint totals are 948,286 versus 952,554. Endpoint tree LOD histograms differ
(`[0,14,15,31]` versus `[0,18,8,34]`). There are zero selected mesh evictions.
Corrected missing-count p95 is `[0,1,0]` for grass/flowers/trees, compared with
baseline `[0,1,2]`; no missing-candidate distance was logged, so this cannot certify
zero near misses.

The corrected 720p static run finishes with 980,386 plant-color and 199,978
shadow triangles; 1080p finishes with 981,560 / 195,954. Both have zero pending
vegetation, all candidate species ready and zero measured color/shadow LOD or
caster-membership changes. The corrected walk ends with three pending plant
requests; terrain quality remains ready. Explicit tree-shadow allocation is
149,872 triangles static and 148,732 at the walking endpoint, within 150k.

The final color-admission correction uses two passes within the unchanged 650k
tree-color allocation: first reserve the cheapest feasible resident coverage for
all trees; then retain previously displayed detail in nearest-first order using
remaining room. A newly visible tree's finest cached mesh cannot take retention
priority from existing displayed detail. This fixes the observed retention
regression without promising that every LOD change disappears: the corrected
walking observations still record 58 color LOD changes, including 15
coarsenings. The entire source/budget contract remains unchanged.

### CPU diagnosis after startup exclusion

For CPU preparation analysis, take the last `frames.sample_count` observations
from each bounded `frame_observations` list. This removes 61 startup/warmup
observations in these reports. `cpu-comparison-analysis.json` records the method
and initial extracted quantiles; the corrected column applies the same method
to `final-*.json`. Its observation-derived frame quantiles can differ
slightly from the official frame summary because the observations and accepted
frame interval are recorded at neighboring points in the frame loop; use the
official summary for whole-frame claims.

| Observation CPU stage | Baseline p50 / p95 / p99, ms | Initial diagnostic candidate, ms | Corrected default, ms |
| --- | --- | --- | --- |
| Static 720p terrain preparation | 3.58 / 5.13 / 5.48 | 1.31 / 2.16 / 2.55 | 1.28 / 2.03 / 2.30 |
| Walking terrain preparation | 10.01 / 22.41 / 27.24 | 10.47 / 21.51 / 27.30 | 10.32 / 20.72 / 24.38 |
| Walking plant preparation | 1.20 / 3.02 / 4.90 | 1.32 / 3.15 / 5.27 | 1.40 / 3.39 / 5.30 |

Static exact-pose/depth reuse removes substantial CPU work. The corrected
whole-GPU median is still 12.36 ms against a 13.01 ms frame median. Plant color
is 2.93 ms; caching reduces the plant-only shadow median from approximately
0.21 ms baseline to 0.0014 ms corrected. These are asynchronous GPU samples,
not paired frame costs.
The remaining GPU/driver work limits the frame gain from lower static CPU time.

Walking still has roughly 3,900 visible terrain clusters requiring current-view
visibility/error/occlusion and uniform work. Immutable scans and repeated basis
construction were only part of that cost; moving poses generally cannot reuse
the complete prepared view. The candidate's cumulative preparation counters
average 0.68 ms accounting/order, 2.30 ms occluder construction, 3.06 ms
visibility/error/uniform upload and 1.66 ms shadow preparation per cache miss.
Those averages include startup and are **not** measured-route stage percentiles.
Measured walking draw-encoding p95 is 9.03 → 8.03 ms, while the whole-GPU median
is 14.40 → 14.24 ms. Percentiles and asynchronous spans must not be added. The
walking terrain CPU tail alone remains above the 16.67 ms full-frame target.

### Diagnostic and experimental runs

All values below use the same initial candidate executable. Parenthesized
controls are the sole requested environment overrides; asynchronous streaming
can still produce different admitted representations/history.

| Run | Frame p50 / p95 / p99, ms | Relevant observation |
| --- | --- | --- |
| 720p static, compact baseline | 13.25 / 17.01 / 17.93 | Color GPU median 2.93 ms; live GPU template bytes 30.48 MB |
| 720p static, pooled + culling | 12.79 / 16.80 / 17.98 | Color GPU median 2.64 ms; 256 MiB pool/cull reservation |
| 720p static, pooled direct (`CULL=off`) | 13.35 / 17.02 / 18.09 | Color GPU median 2.93 ms; same pool reservation and selected triangle count as pooled+culling |
| 1080p static, pooled + culling | 20.92 / 24.76 / 26.42 | Color GPU median 4.89 ms versus compact 5.19 ms; 256 MiB reservation |
| Walk, pooled + culling | 18.55 / 29.55 / 32.67 | Color GPU median 2.19 ms versus compact 2.37 ms; final pending vegetation 0 |
| Walk, terrain prepare reuse off | 19.74 / 29.59 / 34.02 | Terrain CPU p50/p95 11.92 / 22.16 ms, versus enabled 10.47 / 21.51 ms |
| Walk, prediction off | 18.96 / 30.65 / 35.76 | Missing-count p95 `[0,1,1]`; enabled prediction has the same p95 and slightly fewer maximum flower misses |
| Walk, lifecycle explicitly enabled | 18.39 / 30.53 / 35.74 | Final pending vegetation 1; historical diagnostic lifecycle run, preceding final ownership/color fixes |
| 720p static, shadow cache off | 13.49 / 17.57 / 18.50 | Plant-only shadow GPU median 0.224 ms versus cache-enabled 0.0014 ms |
| 720p static, cascades + lifecycle enabled | 14.77 / 18.19 / 19.34 | Extra depth composition increases cost; cascades remain opt-in |

The pooled color kernel improves in this static view, especially compared with
the same pooled allocation with culling disabled. However, it reserves 256 MiB
versus approximately 30.5 MB live compact payload in this settled scene. Used
payload, reserved GPU buffers and whole-process RSS are separate quantities.
Pooled storage remains opt-in: its additional walking/1080p runs improve some
tails and color-kernel times, while walking/1080p frame medians are slightly worse
than compact. All pooled runs have 100% measured terrain quality readiness;
walking representation history still differs (951,042 final color triangles and
`[0,11,19,30]` tree LOD counts). The reserve-versus-payload memory trade is explicit. The prediction comparison is one history-dependent
pair; it does not prove every near deadline is met. The reuse-off comparison
supports a median CPU benefit, with mixed whole-frame tails rather than a
universal improvement.

### Process-cold 6 m/s turn/return: readiness failure

The separate `cold-turn` run enables the lifecycle, removes the terrain-quality
wait, uses zero warmup and starts the 18-second 6 m/s walk/turn/return route with
process-cold vegetation and the disclosed derived terrain cache. Its frame
p50/p95/p99 is **8.93 / 12.40 / 15.79 ms**, but this cannot be presented as a
quality-matched speedup or a 60 FPS success: terrain quality readiness is
**0 of 1,924 measured frames**. Coverage readiness is 1,924/1,924, which only
establishes coherent coverage. There are 183 unbounded-error observations, a
maximum finite visible error of approximately 136,533 px and a peak of 88
unresolved visible clusters. At exit, 65 terrain requests remain pending and 31
visible clusters remain unresolved. The ending view has only 279 visible terrain
clusters, compared with approximately 3,900 in the dense comparison.

Vegetation also begins incomplete in the demand counters: grass/flower/tree missing-count p95
is `[3,20,8]`, with maxima `[23,44,22]`. It eventually reaches zero pending plant
jobs, and the last 100 presented observations have zero pending vegetation;
final selected color geometry is 902,286 triangles. This distinguishes plant
convergence from terrain quality, and says nothing about whether every initially
missing candidate was near the camera. This is a readiness measurement, not a
claim that the cold terrain quality bound passed.
The cold route exposes an unfinished readiness/terrain-refinement case, rather
than demonstrating that ordinary startup can maintain the settled quality bound.

## Measured architectural labs and decisions

`output/vegetation-next/lab/report.json` contains the complete frozen local lab
result, per-pixel parity diagnostics and generation subexperiment. It uses one
seed-123 variant-3 Rowan at 256×256, simplified matched Lambert lighting,
two warmups and seven timed samples per representation/distance. These are
single-model kernel results, not gameplay FPS, full-scene render budgets or
streaming measurements. The 16-sample native reference is a finite estimate.

### Sparse native bricks and aggregate coverage

Storage below counts representation input only, excluding output/readback
targets, common resources and canonical CPU data. The raster implementation in
this lab is **expanded native faces**, not the actual compact pulled gameplay
backend. Its 7,366,200 bytes imply an estimated 1,620,564-byte compact quad payload
at 44 bytes versus 200 bytes per expanded quad. Thus native brick storage is
16.5× smaller than the expanded lab mesh, but only about **3.63× smaller than
that estimated compact gameplay payload**. This is not a measured whole-game
VRAM reduction.

| Local representation | Input bytes | GPU median at 2 tree heights, ms | At 8 heights, ms | At 32 heights, ms |
| --- | ---: | ---: | ---: | ---: |
| Expanded native raster | 7,366,200 | 0.286 | 0.282 | 0.239 |
| Native 4³ sparse bricks | 446,768 | 0.218 | 0.175 | 0.123 |
| Stride-4 solid cells | 25,888 | 0.140 | 0.066 | 0.065 |
| Stride-4 coverage aggregate | 25,888 | 0.254 | 0.084 | 0.080 |
| Stride-8 solid cells | 5,520 | 0.245 | 0.060 | 0.055 |
| Stride-8 coverage aggregate | 5,520 | 0.317 | 0.073 | 0.058 |

Each entry below is linear RGB RMSE / coverage MAE against the finite 16-sample
native reference. Lower is better; the camera and lighting are matched.

| Distance in tree heights | Native raster, 1 sample | Solid4 | Coverage4 | Solid8 | Coverage8 |
| --- | --- | --- | --- | --- | --- |
| 2 | 0.02238 / 0.002872 | 0.11038 / 0.04265 | 0.06644 / 0.02360 | 0.16338 / 0.09519 | 0.08481 / 0.04284 |
| 8 | 0.01064 / 0.000587 | 0.02408 / 0.002297 | 0.01317 / 0.001187 | 0.03787 / 0.005404 | 0.01811 / 0.002264 |
| 32 | 0.003578 / 0.0001078 | 0.005394 / 0.0001574 | 0.002587 / 0.00007991 | 0.009099 / 0.0003538 | 0.003675 / 0.0001310 |

Coverage aggregates improve same-stride solid-cell RGB error by approximately
40–60% and coverage error by 45–63% here. At 32 heights, Coverage4 also reduces
RGB/coverage error about 28%/26% versus one-sample native raster. This supports
further distant-footprint filtering experiments. It does not justify using the
same aggregates nearby: at 2 heights, Coverage4 has about 3× the raster RGB error
and 8× its coverage error, and Coverage8 is slower than the expanded raster.
No universal distance/appearance bound or motion quality has been demonstrated.

Native sparse traversal is also **not promoted**. Strict native/raster parity
fails at 4, 2 and 1 checked pixels at 2, 8 and 32 heights respectively; the report
sets `native_raster_parity_passed:false` and `promoted:false`. The near failures
include a coverage disagreement and RGB/depth errors up to approximately
0.783 / 0.002435, above the unchanged 0.0002 / 0.00002 tolerances. A small failure
count is not proof that the discrepancies are harmless. The native first-hit
edge diagnostic cannot fully classify occluded raster foreground edges or fixed
subpixel raster precision. These failures are retained as rejection evidence rather than hidden by wider
tolerances, an edge exclusion that removes them, or an implementation switch in
the game.

### Growth structures and bounded GPU grass expansion

The exact CPU recipe oracles remain the correctness basis. These timings are
individual finite-fixture observations; recipe bytes exclude headers/capacity,
and native struct bytes exclude lookup indexes.

| CPU structural fixture | Recipe payload | Native voxel structs | Ratio | Rule phase, ms | CPU materialization, ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| 16 grass recipes, 628 native cells | 8,000 B | 25,120 B | 3.14× smaller | 0.0594 | 0.2268 |
| 8 Rowan recipes, 148,976 native cells | 211,714 B | 5,959,040 B | 28.15× smaller | 59.54 | 56.77 |

Rowan contains 10,991 woody strokes and 2,410 foliage sprays. Its materialization
time includes native expansion, exposed-cell blossom selection, connectivity,
color and model bookkeeping. **28.15× describes payload compression, not a growth
speedup.** Rowan GPU expansion remains false; its exact CPU generator remains
authoritative.

The grass GPU experiment uploads 16,144 bytes of expanded stroke/seed/parameter
records, then uses 278,784 bounded grid cells / 6,690,816 bytes. Its occupancy
and packed base-pigment mismatch counts are both zero. However, final canonical
float color parity fails in **1,329 RGB components**, with maximum absolute error
**1.49×10⁻⁷** and no nonfinite components. GPU raster and color spans are 0.00565
and 0.24194 ms respectively; initialization is 22.48 ms and submit/readback wall
time is 3.56 ms. Readback and initialization are not gameplay-frame timings.

The dense GPU grid is about **266× larger than the 25,120-byte native structs**
for this small fixture, despite the compact input recipe. Its color span alone
is comparable to the whole CPU materialization fixture, before GPU upload and
synchronization. This prototype establishes exact native occupancy/base pigment
from procedural structures, but **does not establish a CPU/GPU time or memory
win and does not pass canonical float-color parity**. It remains headless with
`promoted:false`. Sparse allocation, grouped expansion or a separately approved
float-error contract would need their own matched tests before runtime adoption.

### Correctness evidence and outstanding acceptance

The final integrated workspace run reports **512 tests passed, 15 opt-in tests
ignored**. The separately executed frozen native GPU parity test passes again on the
Radeon 760M: expanded, compact pulled, compact indexed and pooled compute/indirect
paths produce exact matching **RGBA8 and Depth32** output with **1,080 covered
pixels**. The test additionally exercises the offscreen indirect command. Its
small frozen target is not a broad forest/motion test, and its reported live
payload bytes are not the pooled reservation. Canonical plant source remains 3.
The fixed Legacy primary capture is byte-identical to the frozen baseline in
PNG, Depth32 float output and linear float RGB; `primary-regression.json`
records matching SHA-256 values. This confirms this fixed terrain scene did not
change, not that every vegetation/shadow scene is visually accepted. The final
before/after startup/walk/turn recordings, their movement manifests and the
sampled visual inspection are retained below.

### Recorded movement verification

The valid corrected recording manifests use the final executable hash above.
`motion-corrected` includes ordinary startup and verifies **22.27 m** maximum
movement with a **1.28-radian** heading range. `motion-settled-final` explicitly
waits 25 seconds before inputs/recording and verifies **22.31 m / 1.28 radians**.
The last observations immediately before actual movement have zero pending
vegetation in both. Call the latter **vegetation-preloaded / delayed**, not fully
terrain-settled: terrain quality is false throughout both recorded traces,
including the 25-second delayed case. These recordings therefore exercise
vegetation startup/preloading, walking, stopping and turning over terrain that
is still refining; they cannot certify a settled full-engine quality experience.

The earlier `motion-settled` attempt used a benchmark input-locking flag and did
not establish actual motion. It is disqualified. `tools/record_walk.py` now rejects
`--adaptive-wait-quality` and `--benchmark-input-lock`, hashes the executable and
checks measured displacement/heading before accepting a recording. Use
`--settle-seconds 25` for an interactive delay. Merely injecting input events or
producing a video file is not sufficient evidence that walking occurred.

Visual review inspected the before/after contact sheet, corrected and delayed
walking frames, the largest recorded transient pairs, final Alpine/seed-31
stills and the lab montage. The nearby canopy retains detailed structure through
the turn where the baseline showed a coarse block. The largest gray transient
pairs correspond to deliberate mouse movement and loading/refinement; no
unexplained screen-wide spike was identified in the inspected samples.

Distant/startup representation changes remain. Coarsening counters are **not
lower**: normal corrected recording 55 versus baseline 36, delayed recording 45.
The correction prioritizes displayed close detail; it does not eliminate all
LOD changes. Recording adds overhead, and 20 FPS capture can miss a single
presented-frame defect. This sampled review does not establish globally invisible
transitions or fully ready terrain; both corrected motion traces failed the
terrain-quality readiness condition described above.

### Retained evidence and decisions

- [Measurements, commands, hashes and memory](reference/gallery/vegetation-next/measurements.json).
- [Movement and sampled visual inspection](reference/gallery/vegetation-next/motion-inspection.json).
- [Canonical-root shadow events](reference/gallery/vegetation-next/shadow-events.json).
- [Primary byte-equality hashes](reference/gallery/vegetation-next/primary-regression.json).
- [Frozen pooled/compact GPU parity](reference/gallery/vegetation-next/gpu-parity.txt).
- [Complete local lab report, including failed checks](reference/gallery/vegetation-next/lab-report.json).
- [Final Alpine scene](reference/gallery/vegetation-next/alpine-final.png),
  [before/after scene](reference/gallery/vegetation-next/scene-comparison.png),
  [walking contact sheet](reference/gallery/vegetation-next/motion-contact.png) and
  [local lab comparison](reference/gallery/vegetation-next/lab-comparison.png).

| Checkpoint evidence / decision | Status |
| --- | --- |
| Corrected lifecycle-default rebuild and workspace checks | Completed: `final-*` hash above; 512 passed / 15 ignored |
| Pooled 1080p/walking and cold turn/readiness results | Recorded above; cold terrain quality fails the bound |
| Fixed Legacy primary visual comparison | PNG, depth and float RGB byte-identical; hashes in `primary-regression.json` |
| Startup/walk/stop/resume/heading-change recording and inspector correlation | Verified movement/delay and sampled visual review recorded above |
| Sparse-brick/raster/aggregate lab quality, timing and memory interpretation | Measured above; strict parity fails, no runtime promotion |
| Recipe compression, CPU phases and GPU grass expansion parity interpretation | Measured above; CPU recipes exact, GPU grass remains headless |
| Final command/hash/memory manifest and promotion decision | Retained in `measurements.json`; compact CPU-generated path remains default, pooled/cascades opt-in, ray/GPU expansion unpromoted |

Whole-frame presentation intervals, asynchronous GPU spans, local kernel
timings, worker wall durations and content readiness answer different questions.
Static preparation/cache work decreases in the corrected runs; native 1080p and
walking tails still miss the complete laptop 60 FPS contract. This checkpoint
records the measured changes and rejected experiments. The production gate
remains open; failed native-ray/color-parity experiments and cold terrain
readiness remain explicit limitations. This checkpoint does not claim that every loading or transition
issue is resolved.
