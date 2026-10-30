# Engine overhaul implementation and acceptance

The active application is the two-crate `successor/` workspace. The root launcher
and build script select it. The earlier `src/`/`native/` implementation remains a
legacy reference, and the pre-overhaul local binary/source manifest are preserved
in ignored `output/redesign/baseline/`. No baseline artifacts were overwritten.

## Implemented

- Shared immutable procedural source, COW override bricks, immutable query
  snapshots, atomic Set/Air/reset transactions, and bounded bulk/run queries.
- Versioned checksummed write-ahead journal, checkpoint loading, identity checks,
  interrupted-tail recovery, and tests for immutable readers and transaction failure.
- Native bitmask face collection with reusable scratch and lossless cell colors.
- GPU suballocation with generational handles, stable indices, nonblocking growth,
  completion-based retirement and bounded asynchronous compaction. Both terrain
  and ray buffer migration commitments count against the permitted ceiling.
- Stable world-anchored nested refinement bands, screen-size quality policy,
  hysteresis and a finite balanced cover retained while replacements build.
  These are hierarchical patches, not GPU toroidal clipmap rings.
- Connected filtered coarse surfaces, parent reconstruction, worker-local bounded
  visual-node caches, common edge samples/colors and conforming 2:1 triangles.
  Native-to-proxy and volumetric replacement boundaries have explicit joins.
- Local dither transitions, with parent morph only on actual one-level splits.
  One final AA target replaces the former pair of full-frame transition images.
- Edited/volumetric footprint replacement geometry includes subtraction, so a
  cave opening cannot expose an unchanged base heightfield. Regional revisions
  preserve unaffected cached tiles; epoch-scoped worker/planner results reject
  stale sources even when regional keys/revision numbers happen to match.
- Near-first bounded queues, physical page batching, cooperative cancellation,
  retained distant coverage, GPU timestamp instrumentation, strict diagnostic
  configuration and managed buffer accounting.
- Menu/world/player integration, survey compatibility and a procedural default.
  Local downloaded surveys no longer silently replace the starting world.
- Standalone CPU/GPU brick-DDA experiment with exact cell/material/color comparison.
  Existing selectable column-ray backend remains available. DAG storage is optional.

Finite-edge balancing needed an additional correction during acceptance: clipped
parent sides could fall outside the old quarter-point probes, and merging then
created a two-level gap. Neighbor queries now use clipped spans and merges walk
all touching leaves. Regression coverage includes the reproduced corner and six
non-power-of-two extents. The true 128 MiB route also exposed actor-pose allocation starvation. Steady
terrain pool growth now reserves one quarter of its ceiling for migration and
new poses; pressure reduces terrain refinement and temporarily retains a cached
pose instead of making it a fatal world error. GPU growth also required final four-byte alignment after
ratio/budget clamps; repeated odd-capacity migration now has a readback regression.

## Verification and measurements

All measurements here are actual runs on Radeon 760M / RADV PHOENIX Vulkan.
Machine/power/thermal conditions were not controlled for a formal performance study.
Raw logs and images are local under ignored `output/redesign/`.

- Final workspace run: **167 tests passed**. Expensive/device tests are explicit.
- GPU tests passed for buffer copies, shared budgets, growth alignment, compaction,
  high water coordinates, native surfaces, water joins and backend switching.
- Additional GPU fixtures passed native/coarse and smooth 2:1 joins at kilometer
  coordinates and multiple camera rotations. Menu survey/procedural replacement
  passed without stale resident tiles.
- Live edit exercise: 6,202 exact changes, 485 unrelated resident tiles preserved,
  505 current regional cover keys after settling, camera/player unchanged. The
  fixture uses 25 cm cells for bounded test cost; this is not a default-scale
  edit-latency result. Native cave geometry and tower top have CPU assertions.
- Brick-DDA experiment: 110,622 identical CPU/GPU rays across cave, edited bridge
  and high-entropy-color fixtures, **zero exact mismatches**. At 256×144 the kernel
  median was about 1.06 ms; upload, oracle, readback and host synchronization are
  reported separately. This does not establish superiority over the game renderer.
- Isolated native collector fixtures showed 2.3–4.2× lower collection time.
  Repeated coarse-cache builds improved 1.91× in an isolated 40-build test; cold
  adjacent tiles reused only about 2.8% of source evaluations. Neither is a claim
  of an overall game speedup.

The final default-preset route and cover/comparison results are recorded below. Older 10-second numbers used a 256 MiB diagnostic
override despite the preset name; that override has been removed, and reports now
state the actual budget. Those earlier runs must not be labeled 128 MiB results.

## Decisions and remaining limits

Promote pooled raster as the default. Keep column ray rendering experimental and
retain brick DDA as a bounded diagnostic candidate. Do not adopt a DAG, indirect
multi-draw, HZB, or directional empty-space accelerator without a measured benefit.
Sampled terrain bounds are not sufficient proof for occlusion; unsafe solid-prism
metadata is disabled for generated meshes, retaining conservative frustum culling.

Coarse surfaces are visual reconstructions; collision and native voxels stay
exact. Projected visible-leaf P95 is an estimate of cell size, not a pixel-weighted
image error, rigorous silhouette bound or guarantee that every river survives.
Very small coarse holes may be lost. The replacement path is footprint based;
fully independent 3D streaming partitions for tall structures remain future work.

Budget lowering may retain the previous ceiling while coarsening. Compaction waits
for headroom rather than hiding migration memory. High-offset live ray bricks can
prevent shrinking until replaced/evicted. Counters cover managed terrain/ray
buffers, not all render targets, staging, driver memory or process RSS.

The economical acceptance pass is shorter than the original research plan's
five-trial, ten-minute, multi-resolution campaign. It does not certify older
laptops, universal crack-free motion, a 1-pixel silhouette bound, millimeter-world
performance, or a statistically proven 20% whole-engine gain. The harness and
long-route controls remain available for further profiling. Weather, water
physics and new terrain generation work are outside this renderer overhaul.

## Economical acceptance pass

The strict 128 MiB pressure route completed 7,200 ticks/407.91 m at 1080p with
zero sampled cover gaps and no crashes. However, 6,493/7,200 frames deliberately
used less than native detail within 3 m. This failed the desired default-quality
behavior despite p95 synchronized work of 7.48 ms. This exposed an inactive-cache eviction defect: cached tiles were
allowed to fill the working set before the hard byte limit. The final fix limits
unprotected cached payload to half the ceiling, preserving protected visible
cover even when it exceeds that cache target. Do not use these earlier pressure
numbers as final behavior. The default is now
**256 MiB**, the original redesign plan's evaluation budget. The 96 MiB low preset
also trades native coverage for residency.

One paired four-camera screenshot smoke pass completed (eight captures total):
valley, river, lake and held-out seed. Source, camera, voxel scale, resolution and
requested settings match; achieved detail can differ. Images were inspected at
1280×720. Connected terrain reduces many old contour discontinuities and bright
gaps; some close cliff contours and material/lighting joins remain visible. River
geometry still needs its separately deferred generation work. This is visual
acceptance evidence, not a formal five-trial performance win.

### Final promoted travel result

After cache eviction was corrected, walking and turning retained native detail.
Sprinting still had 1,346 missed frames because the 160 ms patch fade gated ready
refinement. Fades are now deferred above 4 m/s; ordinary walking retains them.

Final run: 1080p, default 256 MiB preset, 512-tile diagnostic cap, 7,200 ticks,
407.91 m / 120.76 s. Native-detail misses across nine points within 3 m:
**walk 0/2,880, sprint 30/2,880, turn 0/1,440**; longest consecutive miss 0.150 s.
All misses requested native detail, so these are streaming lag, not a hidden
quality-policy reduction. All 7,200 samples retained published GPU coverage and
the fixed horizon. This remains sampled coverage, not a proof of every raster pixel.

Synchronized update+draw/submit+GPU-wait work: p50 **6.09 ms**, p95 **8.98 ms**,
p99 **10.87 ms**, maximum **32.96 ms**. Isolated terrain-pass GPU timestamps:
p50 **2.53 ms**, p95 **3.54 ms**, p99 **4.08 ms**, 225 samples. Sampled peak managed
terrain/ray capacity **237.56 MiB**, process RSS **251.75 MiB**; those counters
measure different things and must not be added as total system memory.

Raw final data: `output/redesign/acceptance/final-travel/mesh_1.{txt,csv}`.
Eight allocation/migration GPU regressions passed after the final cache changes.
The last travel change only defers fades during sprinting and is exercised by the
full route. This is a bounded route, not a benchmark of every world or preset.

The cover is a direct 2560×1440 native mesh capture of seed 42, with its camera,
requested settings and binary hash in `assets/cover.json`. No AI image generation
or image retouching was used. `MicroVoxel` launches the tested new workspace.
