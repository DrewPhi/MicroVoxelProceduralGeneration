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

## Stop/wait/resume follow-up (October 30)

A reported loss of nearby refinement after waiting prompted an asynchronous
GPU regression at the real application's 2,048-tile setting (the older travel
benchmark capped tiles at 512). The replay warms seed 42, switches to seed
8957990822362167096, immediately sprints for 10 s, stands for 20 s, and resumes
sprinting for 20 s. It polls the GPU without a per-frame blocking wait.

The full persistent visual stall was not reproduced in these short routes.
However, logs exposed idle atomic targets requiring 130–174 changed meshes;
resumed travel retained that unfinished idle target until its work completed.
That is a priority barrier even when the nearby samples happen to stay detailed.

Idle batches now count changed tile/neighbor identities and stop at 32 rebuilt
meshes, except when a single indivisible balanced closure requires more. Restarting
movement drops the unfinished idle target and supersedes its planner request;
published coverage and reusable completed meshes stay available. Initial loading
and explicit still captures retain their own coverage policy. The existing
small traveling batch avoids the extra idle-bound calculation.

A CPU regression verifies bounded multi-step idle batches, balanced neighbors,
and retained coverage. The final GPU replay verifies that the idle target is
replaced on restart and reports zero native-detail misses across the nine nearby
probes during both idle and resumed travel (1,200 frames each). Cold-start
refinement still missed 333/600 frames after the world switch; that is a separate
initial loading limitation, not a claimed improvement. Final local logs are
`output/redesign/stop-resume-{unit-final,final,build}.log`.

Reproduce with:

```sh
cargo test --release --manifest-path successor/Cargo.toml -p microvoxel-successor \
  stop_wait_resume_streaming -- --ignored --nocapture
```

`MICROVOXEL_STOP_SEED` overrides the replay seed. Streaming reports also include
camera coordinates, effective tile budget, pressure state and pending work to
help diagnose a remaining location-specific case. The local launcher is rebuilt;
this follow-up has not been pushed to GitHub.

## Shared-corner seam follow-up (October 30)

Screenshots showed foreground contour bands and a large mountain boundary. A
confirmed shared-corner defect was found independently: side-only mesh identities
could choose different height/color filter levels at one four-tile junction when
only the diagonal tile was coarse. A curved, mixed-water CPU fixture reproduces
that old disagreement and verifies one common value after the fix.

Layout keys now include the coarsest incident level at each corner; changing a
diagonal dependency invalidates the appropriate cached mesh. Selection, balance
and safe merges include diagonal neighbors, preventing a two-level corner gap.
Corner queries are cached while constructing mesh identities, and finite-edge
queries use the actual clipped world boundary. Isolated diagnostic keys explicitly
omit corner context. Native cells and persistent save identities are unchanged.

The terrain shader also combines camera-relative cell coordinates before the
single meter conversion. This avoids different arithmetic paths for a shared
vertex at non-binary cell sizes such as 2 cm. GPU fixtures include 2 cm voxels
at eight-kilometer coordinates and mixed coarse/fine diagonal corners.

Checks passed: 69 core unit tests, 12 rendering integration tests, three seam GPU
fixtures and eight allocation/renderer GPU regressions. Before/after captures of
the logged world/position were inspected under `output/seams/`. The exact large
mountain cut in the clipboard images was not reproduced because their camera
orientation was unavailable; it is not claimed fixed. Foreground curved lines
also include real quantized surface risers emphasized by lighting, which remain.
F12 now saves seed, scale, camera, FOV and display settings in a `.txt` beside its
PNG so a remaining seam can be reproduced exactly. These changes remain local.

## Predictive streaming, cache retention and fades (October 30)

The viewer now forecasts a second camera position in physical meters, following
travel rather than just view direction. At rest it warms a small look-ahead
margin. The forecast shares the planner's terrain-error cache. Up to 48 nearby
native/step-2/step-4 candidates are admitted, with at most 8 MiB (1/32 of the
configured terrain budget, clamped to 1–8 MiB) retained as optional warm meshes.
Native interiors use a canonical key rather than speculating on border variants.
Native keys omit corner filtering dependencies that their geometry does not use.

Real jobs precede speculative jobs across all page batches. Any urgent job stops
new speculative work and cancels in-progress speculative meshes at normal worker
cancellation checkpoints. A nine-point, up-to-3-meter native coverage guard also
pauses prediction when walking detail falls behind. World switches and local
edits invalidate forecast work through the existing epochs/revision footprints.
No finest-detail world is generated in advance and this is an in-memory cache,
not an unbounded disk cache.

Eviction favors recent nearby native and near-native terrain over distant cached
tiles, within the existing managed-memory ceiling. Optional warm work reserves
allocation headroom and backs off on pressure; required uploads first evict
optional data and attempt nonblocking compaction. Pool growth now reserves space
to migrate the largest active pool, not merely a fixed quarter of the budget.
The 2 cm/max-radius replay exposed the old policy repeatedly coarsening even
with unused arena space; the revised allocation policy retains the full 2048-tile
budget in that replay.

Ready patches use the existing complementary ordered fade and parent morph with
25–160 ms durations based on tile edge and travel speed. Sprinting no longer
turns transitions off. Near-ready startup can enable fades without waiting for
all distant refinement. Required geometry still publishes only as complete,
balanced patches; fades do not change logical voxel positions or collision.

References: [Geometry Clipmaps](https://hhoppe.com/proj/geomclipmap/) for incremental
cached terrain coverage, and [Ascendant geometry management](https://vkguide.dev/docs/ascendant/ascendant_geometry/)
for pooled geometry allocation tradeoffs. No external source code was copied.

Validation includes real GPU first-traversal/reversal/return replays, worker
priority tests, an eviction-order fixture, and short/long fade coverage captures.
The normal 6.25 cm replay requires zero missing native coverage at nine probes
within 3 m. The optional 2 cm/max-radius case reports full-detail coverage as a
stress metric rather than promising it: both preload-disabled and initial preload
runs fell behind, despite complete resident terrain coverage. Initial preload
built 1073 meshes and reused 334, but was slightly worse than the disabled control
(230 vs 197 return frames missing full native coverage out of 600). This prompted
the coverage guard; cache-hit counts alone are not a speedup benchmark.

Reproduce (normal first; add MICROVOXEL_CACHE_CM=1 for the stress measurement):

```sh
cargo test --release --manifest-path successor/Cargo.toml -p microvoxel-successor \
  predictive_detail_and_backtracking -- --ignored --nocapture
```

`MICROVOXEL_DISABLE_PREFETCH=1` supplies a control. Logs are ignored local artifacts
in `output/streaming-upgrade-*.log`. These follow-ups have not been pushed.

Final guarded normal-resolution replay: zero native misses across all 1680 frames;
400 resident mesh reuses, including 220 warm meshes reused at publication out of
456 speculative builds. Final managed terrain commitment was 171.71 MiB under
the 256 MiB setting, with no pressure/coarsening. This run was a coverage test,
not an isolated frame-time benchmark (builds also ran on the host). Checks: 70
world unit tests, 12 rendering integrations, 39 non-GPU viewer tests, 10 GPU
allocation/cache/fade tests and three seam fixtures passed.

Guarded 2 cm/max-radius stress replay: native misses [0,419,156,0] for
[240,600,600,240] hold/outbound/return/hold frames. The disabled control had
[0,419,197,0]. Thus this single comparison improved return coverage (74.0% vs
67.2% fully native within 3 m) while first-traversal coverage was unchanged
(30.2%); it does not establish a general frame-rate gain. 249 warm meshes were
reused of 488 built, final managed commitment 150.68 MiB, no pressure and no
missing cover. Max density still visibly refines during travel. The stress test
asserts coverage/residency/errors, but intentionally reports native coverage
instead of imposing the normal-resolution zero-miss guarantee. Local MicroVoxel
launcher rebuilt successfully without warnings.

## Geographic hierarchy / generator 6 (October 30)

The 100 km experiment showed that 4.2 km uplift noise repeated independent
mountain blobs throughout the map. Generation now begins with a bounded analytic
geographic skeleton (`generation/provinces.rs`): jittered 72 km sites carry
velocities; relative convergence drives uplift on curved Voronoi-like boundaries.
This is tectonic-inspired shaping, not a plate-history simulation. Nine local
sites are evaluated on the stack, with smoothly gated pair contributions to
avoid nearest-rank discontinuities at junctions. There is no global plate raster
and no per-voxel tectonic work.

Mountain cores, foothills and broad 22–38 km lowland/plateau/basin fields guide
landform weights. Fine ridges modulate connected uplift instead of choosing where
mountains exist. Geology varies over broad provinces. Catchments are then routed
on this relief; an implicit downstream-first stream-power-inspired incision pass
lowers valley profiles and accounts removed sediment as exported material.
Existing conservative sediment transport, slope relaxation, lake screening and
bounded channel carving remain. D8 directional bias and static lake-level
approximations remain limitations, particularly in a coarse 128 m overview.

Climate uses dominant and weaker reverse-season wind sweeps (80/20), with broad
moisture-source modulation, elevation lapse rates and orographic condensation.
Temperate storm transport calibration increased from 300000 to 600000: the old
100 km extension dried almost half the map into desert. This is a prescribed
annual climate model, not live weather. Surface coverage adds a continuous
cold/moist-upland seasonal snow-persistence estimate alongside permanent snow;
there is no snowpack or live snowfall simulation. Voxel painting remains one
solid base color per cell with lighting allowed to shade faces.

Source extents 1–100 km are now supported explicitly via `--extent-km`; default
interactive extent remains 10 km. Dense regional spacing increases by powers of
two above 12.5 km to bound climate/drainage arrays to <=783² samples (16 m at
10 km, 128 m at 100 km). Macro fields remain in source meters, independent of
voxel resolution. Enlarging the regional solve changes its boundary conditions
and sampling; this is not a guarantee that 10 km and 100 km independently solved
crops are identical. Debug maps/statistics use the actual extent. Isometric
exports use the current connected distant-terrain mesher with extent-scaled
sampling and visibility cutoff. No temporary modified-source build is needed.

Generator identity bumped 5 -> 6. Old procedural edit saves are rejected rather
than silently replayed on a different landscape. No edit-save migration was
attempted. The legacy root generator is retained only as an old comparison.

Abstract references inspected (no third-party source copied):

* Tectonic ridge composition: `mountain_ridges/ridges.json`,
  `shifteddetail.json`, `weathering.json` under
  https://github.com/Apollounknowndev/tectonic/tree/rewrite-squared/src/common/main/resources/resourcepacks/tectonic/data/tectonic/worldgen/density_function
  — broad fields, separate shifted detail and weathering.
* WorldEngine's staged geography/environment pipeline:
  https://github.com/Mindwerks/worldengine/blob/master/worldengine/generation.py
  and `simulations/precipitation.py`. Its normalization/temperature-modulated
  precipitation is a reference, not an implementation port; we retain a physical
  meter-space wind sweep instead of image-relative noise/quantile thresholds.
* Schott et al., Terrain Amplification using Multi-Scale Erosion (2024):
  https://h-schott.github.io/p/mserosion/ — coarse relief guides hydrologically
  consistent detail. The new incision pass is a bounded approximation, not a
  reproduction of that paper's full multiscale solver.
* Lithosphere/JJThunder are visual references only; their published listings are
  all-rights-reserved, and their code/assets were not copied.

Inspection follow-up: geographic bands exposed the old D8-grid directional bias
more clearly. At >=32 m display spacing, narrow wet river samples now mix water
with surrounding footprint-filtered land colors by analytic river coverage,
rather than painting the entire display cell solid blue or exposed-bank white.
This changes only display colors. Size-aware volumetric significance keeps
sub-cell caves/arches from switching enormous distant tiles to a different mesh
representation. Canonical features and edited footprints remain exact; custom
generators default conservatively to their existing volumetric behavior.
Feature placement now covers the actual source extent rather than only its
initial 10 km corner. Source extent also enters generator definition identity,
including when world-scale rounding produces equal playable extents.

The geographic tests now evaluate diversity at 100 km instead of requiring all
landforms in every 10 km crop. Multi-seed analytic tests check connected mountain
regions, quiet interiors and continuity across plate-search boundaries. The
regional integration checks native-grid independence, populated distant fields,
useful relief and <256 MiB regional memory at 100 km. An incision fixture checks
mass export/nonnegative layers and downhill profiles. Snow remains a static
seasonal-persistence approximation; the snow/rock balance is not a validated
snowpack forecast.

Additional numerical reference: Braun & Willett (2013), implicit stream-power
incision, https://doi.org/10.1016/j.geomorph.2012.10.008 and
https://fastscapelib.readthedocs.io/en/latest/api_cpp/eroder.html . Our linear
implicit update uses bounded incision and prescribed effective coefficients,
not calibrated geological durations or a full landscape-evolution solver.

Final seed-42 capture: `output/overview-100km-v6/isometric-100km-seed42.png`,
4096×3072, true-scale heights, 128 m regional/display sampling, world scale 1:1.
73 world unit tests, 20 terrain/environment/render/scale integration tests and
three seam GPU fixtures passed. Sparse cave/arch capability remains verified
at native spacing while sub-cell distant features use the ordinary surface path.
Launcher rebuilt. Export measured 5.97 s region/maps and 11.89 s overview,
179.84 MiB regional fields/bins and 49.02 MiB mesh payload; these are single-run
export timings, not interactive FPS claims. Previous comparison PNG retained.
Inspection: connected long belt and broad quiet regions are a substantial
structural improvement; biome color contrast is still subtle, woodland
suitability sparse, and D8 rivers/lake matching still visibly artificial. No
claim of a complete tectonic/climate/hydrology simulation or photorealism.

## 1000 km² close overview (October 30)

Added `--overview-area-km2` for a same-world isometric crop around the highest
point in a 256² physical-space sampling sweep. It preserves the original 100 km
regional climate/drainage solution; it does not regenerate a smaller region or
compress the mountains. Bounds align to display cells and boundary tiles discard
outside display-cell quads while retaining their original height/color records.
PNG sidecar records seed, extent, origin, area, display spacing and sampled peak.
The exporter now honors the CLI terrain-memory setting. This crop uses 32 m
display samples and needs `--terrain-memory 512`; the initial 256 MiB attempt
correctly stopped on allocation pressure. Interactive preferences are unchanged.

Reproduce:

```sh
./MicroVoxel --world procedural --seed 42 --extent-km 100 --isometric \
  --overview-area-km2 1000 --terrain-memory 512 \
  --screenshot output/overview-1000km2/isometric-seed42.png
```

The requested 1000 km² is about 31.6 km on each side, compared with the earlier
100×100 km = 10000 km² image. This is a view-scale comparison, not a terrain-scale
change. Peak elevations are around 1.8 km above world zero, not tens of kilometers.

Capture completed and visually inspected: `output/overview-1000km2/isometric-seed42.png`,
4096×3072, 31616 m square (999.571456 km²), 32 m display cells. Crop origin
[0,68384] meters. Denser peak search found 1838.1 m near [2929.69,86914.06],
slightly above the earlier coarse-statistics maximum of 1816.2 m; terrain was
not rescaled. The connected range is clearer at this view scale; parallel D8
tributaries, angular lake edges and fine ridge repetition remain visible.
