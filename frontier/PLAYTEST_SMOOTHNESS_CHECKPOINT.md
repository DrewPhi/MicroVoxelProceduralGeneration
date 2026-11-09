# Development playtest smoothness — 2026-11-08

Implementation checkpoint against `bc28210`, with bounded hardware evidence below.
The current development target is 1920×1080 at 60 FPS where possible; 30 FPS is
acceptable during development. Stable visible detail and responsive walking and
turning take priority over the former small cache defaults. This does not change
the canonical 6.25 cm voxel world or establish the longer-term performance gate.

## Follow-up: camera-turn regression

The initial walking check missed a loaded-tree/ground budget conflict. See
[the reproduced failure and repair](VEGETATION_TURN_FIX.md) before treating this
checkpoint as proof that nearby detail survives arbitrary camera turns.

## Implemented changes

- Development, Constrained and Custom resource controls expose terrain CPU/GPU,
  build reservations, vegetation CPU/GPU, triangle and near-detail budgets.
  Development budgeting reads available RAM, leaves OS/driver headroom and
  charges shared GPU residency to the same working envelope. The reservation
  calculation leaves at least 1 GiB or 30% of currently available memory, and
  limits the envelope to half physical RAM. These are lazy budgets, not proof of
  whole-process peak memory. Frame/memory pressure controls background throughput
  without changing source density, terrain quality or fog.
- Applying resource or quality settings reconfigures existing terrain/vegetation
  streams. Valid cached products and current coherent owners remain reusable.
  Geometry cache resizing preserves its source reservation and pages; a total
  below that retained reservation is rejected. MSAA changes replace color
  pipelines while preserving terrain geometry, profiles and ownership. The
  constructor-based pipeline replacement temporarily allocates ancillary
  resources during settings Apply; it is not a per-frame operation.
- Exact native cliff geometry can use a conforming rectangle triangulation.
  Every original corner is indexed along all three lattice directions, including
  corners belonging to perpendicular faces. Rectangle boundaries split at every
  incident source corner; independent tile/volume boundaries keep unit lattice
  segments. An exact dyadic center forms a triangle fan over each unchanged
  planar rectangle. Negative faces retain outward winding. Canonical positions,
  material colors and profile IDs are retained, with the same native shading
  flag and fragment path. The existing flat-triangle GPU/shadow pipeline is
  reused; the word `proxy` in its storage counter does not make these exact
  triangles an approximate surface.
- Compact packing is accepted only when it uses fewer primitive bytes than the
  unit-face representation and respects the bounded packed-output limit.
  Unrepresentable centers and unhelpful tiny faces retain the unit path.
  `FRONTIER_NATIVE_GEOMETRY=unit` selects the original unit baseline; the default
  representation tag is `conforming-v1`. Derived cache identity distinguishes
  the modes. Packing checks cancellation during preparation and output.
- Vegetation admission reserves tree coverage before refinements, retains
  affordable existing detail through turns and uses configurable CPU/GPU and
  triangle headroom. Ground-cover decisions have bounded inspection records
  containing requested/selected LOD, distance, headroom and refusal reasons.
  Vegetation color visibility receives the actual camera projection. Inspect
  captures and moving views before declaring flower placement or turn stability
  accepted; instrumentation alone does not prove either.
- The visual settings expose a clear-air atmosphere switch and resource controls.
  Reports include selected resource settings, pressure state and native geometry
  mode, alongside existing frame observations and asynchronous GPU timing.

## Correctness and provenance

Legacy reference policy: **G / greenfield**. Existing Frontier source contracts,
unit-face geometry and frozen captures are comparison oracles; no old-engine
scheduler, terrain approximation or runtime was transplanted. No external code
or dependency was copied for the conforming triangulation.

The geometric requirement is a triangulation that preserves all constrained
boundary segments, described in [Shewchuk's Triangle documentation](https://www.cs.cmu.edu/~quake/triangle.help.html).
The independent voxel account [Meshing in Voxel Engines](https://blackflux.wordpress.com/2014/02/23/meshing-in-voxel-engines-part-1/)
describes the T-junction failure motivating this check. This implementation uses
integer lattice line keys and convex rectangle fans, not Triangle's Delaunay
algorithm. Closed-shell segment incidence, outward area, original unit-face
coverage, materials/profiles, independently partitioned tile boundaries,
cancellation, tiny-face fallback and malformed/overflow input have targeted CPU
tests in `renderer/src/native_conforming.rs`. Tests are correctness evidence,
not a GPU speed measurement or proof of temporal visual acceptance.

The canonical world, caves/overhang ownership, old Legacy spiky source, source
pigments and horizon remain requirements. No heightfield replacement, reduced
native resolution or opaque fog curtain is part of this geometry change. Compact
triangles currently lack the native-quad direction bins; report total triangles
and exact coverage as well as representation-specific counters.

## Reproduce

Build the candidate separately using the project toolchain. From `frontier/`:

```sh
python3 tools/playtest_smoothness.py
python3 tools/playtest_smoothness.py --run
python3 tools/playtest_smoothness.py --only mountain-still --only grazing-still --run
```

The default command prints a plan and performs no GPU work. `--binary`,
`--output`, repeatable `--only`, `--seconds` and `--timeout` select a bounded run.
`--only flat-native` selects an additional small compact-path A/B fixture;
`--include-flat-native` includes it in the default suite.
The default output is `output/playtest-smoothness/validation-after`; existing
command evidence requires a fresh output directory or explicit `--overwrite`.
Each run saves its exact command, binary SHA-256, explicit mode override, log,
return code and wall time. It does not build or regenerate a reference.

`primary-after` reproduces the fixed Legacy hybrid capture in
[PRIMARY_VISUAL_CHECK.md](PRIMARY_VISUAL_CHECK.md) and the preserved
`output/playtest-smoothness/baseline.py`. Inspect that capture first. The other
fixed stills compare the same candidate binary in conforming and unit modes:
Legacy adaptive at the primary camera (4.5-pixel error), the recorded Alpine mountain camera and
an almost-horizontal grazing view. They preserve identical parameters within
each A/B pair and save float color/depth artifacts at 1080p/MSAA1.
All adaptive pairs explicitly use Custom reservations: 1,024 MiB terrain CPU
cache, 1,536 MiB terrain GPU and 768 MiB build memory. These demanding extra
checks are not a comparison against the Constrained defaults; both topology
modes receive the same larger reservations.

The mountain motion cases use grounded 6 m/s stop/resume and turn/return routes,
12 seconds by default, after quality readiness. Their 1-pixel error request is
stricter than the original 4.5-pixel mountain baseline. They test stronger demand,
not a matched before/after quality speedup. Real-time routes share the
wall-clock trajectory, but frame overshoot can change final image positions;
use fixed stills for pixel comparisons. Per-mode persistent disk directories
prevent representation contamination but do not establish cold-cache startup
cost. These coordinates have not yet been shown to reproduce the user's
reported 3 FPS mountain approach. Ordinary startup and broader routes still need
separate evidence.

## Initial capture evidence

Checks confirm that the frozen Legacy hybrid `primary-after` PNG, depth and linear
RGB artifacts are byte-identical to their preserved before artifacts. This is
fixed-frame regression evidence; it does not certify adaptive geometry or motion.

The additional Legacy primary-camera adaptive capture at 1-pixel error failed
to settle within its 120-second limit with the 512 MiB GPU reservation and
reported `gpu_resource_limited=true`. Preserve that failed run and its log. It
is an observed capacity/readiness failure at those settings, not evidence of a
geometry correctness failure or a completed quality pass. A subsequent primary-adaptive 1-pixel attempt with 1,024 MiB CPU cache and
1,536 MiB GPU failed after about 48 seconds with unresolved CPU cache capacity:
about 1.039 GB selected geometry plus 32 MiB source approached the 1 GiB total.
GPU residency was not the limiting resource in that attempt. Both failed runs
remain in ignored output as capacity evidence. The extra primary-adaptive
harness now requests 4.5-pixel error; the official frozen hybrid regression is
unchanged. This is a separately disclosed validation setting, not a retroactive
quality pass for the failed 1-pixel attempts.

Checks confirm that both Alpine mountain-still and grazing-still topology pairs
achieved requested quality, with byte-identical PNGs and bit-identical depth.
The mountain pair reported 4,054,946 triangles and 300,378,752 GPU resident bytes
in both modes. Those native tiles all chose the unit fallback, so this establishes
matched fallback-path image parity and does not demonstrate compact-path GPU
coverage or a speedup.

The optional `flat-native` pair is intended to exercise actual compact GPU
geometry: Legacy recipe, amplitude 1 m, two octaves, 4.5-pixel error, 64 m clip,
eye `(0,2,0)`, target `(0,0,10)`, 1080p/MSAA1, with beauty, plants, atmosphere
and terrain shadows off. It deliberately changes the source fixture and is not
a replacement for the mountain or frozen primary comparison. Confirm that
compact records are actually resident before interpreting its A/B results.

## Recorded hardware runs — intermediate candidate

The commands and reports below used release binary SHA-256
`39fa1bf686816e922b0c6d75943fe01a37924ede706e236ea81dd1d780f6c4c5`.
This is intermediate revision evidence: the subsequent altitude-selection change
was not in that binary. Final motion results and the final binary identity remain
pending. The frozen `bc28210` meadow binary SHA-256 is
`7dd044637c686b1877f686323a9346816b1de3b6b1875becdc2e3df1b2dcc4a5`.

Hardware: AMD Ryzen 5 7640U, integrated Radeon 760M, Vulkan RADV PHOENIX,
Mesa 25.2.8; physical RAM about 14.93 GiB. All three candidate runs below used
1920×1080, MSAA4, 4.5-pixel terrain error, 6,000 m clip, uncapped presentation,
60 discarded warmup frames and settled-start readiness. They reused the same
persistent disk-cache directory, so preparation timings are not cold-cache
benchmarks. Exact commands and hashes are in the adjacent `.command.json` files.

| Run | Measured frames | Frame p50 / p95 / p99 (ms) | Plant triangles at end | Terrain quality frames |
| --- | ---: | ---: | ---: | ---: |
| `bc28210` meadow before | 431 | 22.54 / 37.51 / 43.87 | 990,772 | 431 / 431 |
| Meadow Constrained | 456 | 21.10 / 36.54 / 43.88 | 999,990 | 456 / 456 |
| Meadow Development | 432 | 21.81 / 39.50 / 49.39 | 2,260,836 | 432 / 432 |
| Mountain Constrained, stationary | 407 | 16.83 / 19.16 / 20.30 | 654,110 | 407 / 407 |

Reports: [meadow before](output/playtest-smoothness/meadow-before.json),
[meadow Constrained](output/playtest-smoothness/meadow-final-constrained.json),
[meadow Development](output/playtest-smoothness/meadow-final-development.json),
[mountain stationary](output/playtest-smoothness/mountain-final.json).
The meadow runs follow the same 12-second real-time grounded stop/resume route
at 3 m/s from `(-43.625,31.0625,-289.5625)`. The mountain run is an eight-second
stationary view at `(-650,587.825,2350)`; it does not reproduce a mountain
approach or establish turn stability. Every measured candidate frame retained
terrain coverage and requested terrain quality, with zero unresolved clusters.
All three ended with zero pending/missing plant meshes, zero unresolved plant
coverage and zero selected-mesh evictions. These are measured-route terrain
counters and final plant counters, not a claim that every plant decision was
visually accepted or that every frame met 30/60 FPS. In particular, meadow p95
and p99 intervals still exceed 33.3 ms. Development displayed over twice the
plant triangles, with slightly higher median and worse tail intervals than
Constrained; this is additional detail rather than a demonstrated speedup.

End-of-run terrain CPU / GPU resident memory was approximately 639 / 233 MiB
for meadow Constrained, 708 / 232 MiB for meadow Development and 546 / 161 MiB
for the mountain. Whole-process peak RSS was approximately 1,349, 1,436 and
875 MiB respectively; shared GPU allocations and process RSS must not simply
be summed. Full GPU span p50/p95/p99 was 19.05/23.00/24.41 ms, 20.60/24.96/26.50 ms
and 15.81/16.58/16.91 ms respectively. These asynchronous spans include shadows,
terrain, plants, atmosphere and UI, and exclude CPU preparation, earlier uploads
and presentation. Actual RAM-dependent budgets are retained in each report.

An earlier Development run,
[meadow-after-development.json](output/playtest-smoothness/meadow-after-development.json),
failed its 120-second settled-start gate before the route began. Terrain had
already converged, but ground-mesh jobs reached 6,523 and four templates kept
reappearing as pending: selected plant CPU geometry exceeded its cache budget.
CPU-aware plant admission and separate shadow/lifecycle reservations now prevent
that observed churn; the final Development run completed with zero selected
mesh evictions. Keep the failed artifact as evidence, not a successful timing.

The actual Esc menu/Apply exercise at 1280×720 is recorded in
[menu/report.json](output/playtest-smoothness/menu/report.json) and
[menu/game.log](output/playtest-smoothness/menu/game.log).
[Before](output/playtest-smoothness/menu/menu.png) shows MSAA4 and 227 MiB terrain
cache; [after Apply](output/playtest-smoothness/menu/applied.png) visibly selects
MSAA Off, shows 710 MiB cache, zero pending work and quality ready. The report
confirms `msaa_samples=1`, final terrain quality/refinement ready and no benchmark
failure; the log contains no GPU validation error. Cache growth across Apply
and the cache-identity CPU regression support retained streaming state. This
menu run includes ordinary coarse startup, so its whole-run quality fraction
is not the 100% measured-route result in the table above.

## Final build and playtest acceptance

Release binary SHA-256:
`c802fbd289ec074d44b6bc324ef30b2c965766baec68d915d570edc13a6a3178`.
The offline workspace suite passes **533 tests**, with 15 opt-in tests ignored;
all release tool binaries build. The explicit hardware
`frozen_native_gpu_parity` test also passes: expanded, compact, indexed compact
and pooled backends have identical color/depth on 1,080 covered frozen pixels.
This is a bounded backend oracle, not a whole-scene parity claim.

The final build adds altitude-aware plant demand. Ground-patch vertical bounds
come from canonical placement records; tree bounds use conservative source
height. Both combine their existing horizontal distance with vertical separation.
No new per-frame terrain noise or detailed model generation is needed. Placement
prefetch stays warm in XZ, but flying far above the surface no longer requests
native grass or unnecessary nearby-tree meshes. A regression checks no ground
mesh requests at 500 m altitude and restored demand after descending.

A fresh final-build 1080p/MSAA4 Development walking run measured **22.20 / 40.82 /
46.06 ms p50/p95/p99** over 424 frames. It ended with 2,142,850 plant triangles,
zero pending plant jobs and zero selected-mesh evictions. Terrain coverage held
for 424/424 frames; requested quality held for **415/424**, with maximum visible
bound 4.5603 px against the 4.5 px request and up to two transient unresolved
clusters. This is deliberately not rounded up to a universal quality pass.
Peak process RSS was approximately 1,541 MiB. RAM-dependent budgets were 824 MiB
terrain CPU, 824 MiB build, 549 MiB terrain GPU, 219 MiB plant CPU and 329 MiB
plant GPU. Preparation took about 48.7 seconds to reach refinement readiness;
this shared-cache, normal-desktop run is not a cold-start speedup benchmark.
The separately measured intermediate results above remain valid for that binary.

Actual grounded recordings at 1920×1080 cover normal startup and a second run
with a 25-second preload delay. Both use 4.5 px, Development, unchanged Alpine
source/painting and default MSAA4. Actual displacement was 22.36 / 22.41 m;
yaw changed 1.28 radians. Both ended with zero plant jobs and no unresolved plant
coverage; no sampled frame reported selected-mesh eviction. Eight timestamped
frames per recording were inspected; image-difference statistics locate the largest
changes during the turn. Grass and
flowers remain present across the sampled turn; no fullscreen triangle spike was
seen. Ordinary startup and distant refinement remain visible. **Terrain quality
readiness stayed false in both recordings**, including the delayed recording;
25 seconds is a preload delay, not proof of full terrain convergence. Video is
20 FPS and can miss subframe artifacts; its perturbed timings are not used for
performance claims.

- [Normal-start walking video](output/playtest-smoothness/walk-startup/walking.mp4)
- [Preloaded walking video](output/playtest-smoothness/walk-settled/walking.mp4)
- [Curated motion frames](reference/gallery/playtest-smoothness/walk-settled.png)
- [Final frame](reference/gallery/playtest-smoothness/meadow.png)
- [Retained measurements and exact invocations](reference/gallery/playtest-smoothness/measurements.json)

A separate 80 m/s, 20-second mountain flight traversed about 1.6 km. Old/new
p50/p95/p99 were 9.41/12.53/13.89 and 8.57/11.27/12.57 ms. The final Development
profile differs from the old fixed budgets, and **neither run achieved requested
terrain quality during that fast cold traversal**. These numbers are not an
equal-quality speedup or proof of readiness at flight speed. They do show avoided
work: old/new tree mesh jobs were 367/0; both ended with no visible plant
triangles at that altitude. The user's original 3 FPS mountain case has not been
reproduced, so its exact cause is still unconfirmed.

## Final geometry comparison

The final frozen Legacy primary PNG, depth and linear RGB are byte-identical
to the before files. The additional adaptive primary at 4.5 px also has identical
color, depth and coverage between modes. The Alpine mountain/grazing comparisons
above used the earlier candidate's unchanged geometry code and unit fallbacks.

The flat fixture needs an explicit **8 m native radius**: without that, its small
error permits entirely coarse surfaces and does not exercise native packing.
With it, four exact native clusters are resident in both modes. Visible triangles
are 11,560 compact versus 75,624 unit; total GPU resident payload is 9,279,872
versus 11,188,608 bytes. Coverage is identical. Color differs at 98/2,073,600
pixels (maximum channel difference 4/255); depth differs by at most
1.78814e-7 (three float32 ULPs at these depths), consistent with changed planar
interpolation. CPU lattice/coverage tests establish the geometric identity;
this is not a bit-identical raster claim. The fixture proves useful compact
packing, not a measured mountain FPS gain.

## Remaining limits

This is ready for another playtest, not a completed production architecture gate.
Median walking is about 45 FPS at the stated settings; tail intervals still miss
30 FPS. Coarse startup and some distant detail/shadow changes remain visible.
The minimum native radius and near-plant settings request detail within available
budgets; they cannot make uncached geometry instant or exceed physical memory.
Current flora draw bounds and source geometry are tested, but broad travel across
many seeds, very large custom radii and every possible camera are not certified.

Runtime low-memory handling has budget and worker pause/resume tests. No real
system-memory-exhaustion experiment was performed. The guard pauses new terrain
builds/uploads and new plant jobs; bounded in-flight plant jobs may drain. It
preserves visible content rather than guaranteeing immediate driver-memory
reclamation or a strict whole-process cap. Cave/bridge/edit ownership regressions
pass in the CPU suite; no new secondary cave/bridge GPU gallery was needed for
this unchanged source/ownership logic. Further work should target remaining CPU
terrain preparation, background streaming and distant transitions against the
existing reference targets.
