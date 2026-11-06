# Issue 75: Phase A experiment contract

Status: bounded GPU experiments measured; see [results](reference/issue75/README.md).
The second iteration tests [indexed adaptive GPU records](reference/issue75/ITERATION_2.md)
and [parallel GPU appearance preparation](reference/issue75/GPU_PROFILES.md).
These preserve useful architectural ideas for further integration instead of
rejecting their families based on the first uniform-grid or serial-kernel test.
They remain opt-in experiments, not completion of #75 or promotion under #74.
The [integrated appearance experiment](reference/issue75/INTEGRATION.md) now
consumes GPU-computed profiles in real draws. Lossless source packing reduced
transfer/storage costs, but repeated loading and motion runs did not establish
an end-to-end win. Keep this path optional while investigating shared source
reuse and batched preparation; do not promote it on kernel timings alone.
The uniform-page candidate fails the primary visual gate; no replacement is
promoted and the broader architecture decision remains open. Reuse tier G for
the experiment tools. Existing adaptive,
clipmap/hybrid and native-reference implementations remain comparisons. Reusable
GPU terrain grids already exist in this repository; the page experiment must
measure its specific changes rather than claim that GPU grids are new.

## Ordered gates

1. Run the frozen [Legacy player-height 300 m check](PRIMARY_VISUAL_CHECK.md)
   first. Preserve seed 7, 6.25 cm canonical voxels, source parameters, exact
   eye/target, 1920×1080, 60° FOV, near .025 m, far 300 m, materials and lighting.
   Compare the existing adaptive path and hybrid clipmap against the new pages
   path and the independent native-column reference. Record MSAA1 for depth.
2. Inspect side-by-side images, ridges, native/proxy boundaries and openings;
   color agreement alone never establishes geometric or visual acceptance.
   Only after the small gate passes should a separately labelled matched 1 km
   experiment run. Regenerate a reference with sufficient extent; never reuse
   the 300 m reference as 1 km ground truth.
3. Add a matched Regional source comparison, retaining Legacy as the primary
   regression target. Freeze each source's camera and source recipe separately.
4. Measure identical frame/realtime routes at 3, 15 and 80 m/s, recording route
   clock, duration, start pose, cache state and warmup. End-frame captures alone
   cannot certify temporal quality. Preserve reference trajectory frames for
   inspection of holes, shimmer, popping and ownership transitions.
5. Run bounded fixture cave/bridge comparisons for mesh, brick and brick-skip;
   add directional skip only when its actual selector exists. A surface page
   result does not certify volume topology. Teleport optimization is lower
   priority than these visual and motion gates.

The initial pages design is a fixed 24×24 tile square, 32 display cells per tile
at 1 m spacing: a 768 m square with at least 344 m conservative radius after a 32 m snap and native-owner guard.
At 1 m spacing this covers the 300 m clip at every snapped pose. For other
spacing, the minimum conservative radius is 344 times the spacing; record edge coverage
explicitly and do not attempt the 1 km gate prematurely.
The four canonical samples per display height form an empirical proxy, not a
certified geometric error bound. Pages do not change canonical voxel identity.

## Existing commands and integration boundary

From `frontier/`, these observed tools already expose the following interfaces:

```sh
python3 tools/primary_visual_check.py --scene adaptive --dry-run --output output/phasea/legacy-adaptive
python3 tools/primary_visual_check.py --scene hybrid --dry-run --output output/phasea/legacy-clipmap
python3 tools/adaptive_loading_check.py --dry-run --workers 1 --distances 300 --output output/phasea/loading
```

`adaptive_motion_check.py` runs hardware experiments, with `--speeds 3,15,80`,
`--route turn-return`, `--far 300`, `--seconds`, `--msaa 1`, `--output` and
optional `--settled`. It currently hardcodes `--scene adaptive` and Legacy;
therefore it is not a page-versus-adaptive or Regional runner. Do not relabel
its results. Inspect/extend the command construction when page integration
lands. Do not use `--settled` to hide cold preparation time.

The page integration contract is `--scene pages|pages-topology`, reusing
`--terrain-recipe`, explicit `--eye`, `--target`, `--seed`, `--route` and camera
flags. Confirm parsing and generated commands before hardware execution.
Current bounded volume parser accepts `--scene fixture --renderer
mesh|brick|brick-skip|brick-directional`; directional mode is explicitly selectable. Single-sample
captures write `.depth.f32` and `.capture.json`. MSAA4 can lack depth; absence
must remain unavailable rather than count as parity.

## Explicit comparison manifest

`tools/phasea_report.py` performs analysis only, never builds or launches a GPU:

```sh
python3 tools/phasea_report.py output/phasea/manifest.json --output output/phasea/report.json
python3 -m unittest discover -s tools -p test_phasea_report.py
```

Manifest shape: `{"runs": [...]}`. Each run has an `id`, `rgb`, `depth`, optional
`report` benchmark JSON, `match`, and `measurements`. Paths resolve relative to
the manifest. Candidate runs name a baseline run through `compare_to`.

`match` requires `source` (the actual `terrain_recipe` string), `seed`,
`world_version`, `camera` (`eye`, `target`, `fov_y_degrees`, `near`), `far_m`,
`width`, `height`, `voxel_m`, `materials`, `lighting`, `sampling`, and `route`.
Use explicit versioned descriptions for appearance and trajectory metadata.
Equality of caller metadata alone is insufficient: source parameters, camera,
projection, sampling, filter and dimensions must also agree in capture-produced
sidecars. Source fields may come from the explicit benchmark `report` when
not duplicated in the sidecar; conflicts fail. Two missing fields never match.
The independent reference has a different metadata format; inspect it through
`primary_visual_check.py`/`landscape_pair.py`, rather than inventing provenance
fields to force this native-resolution parity tool to accept it.

`measurements` records `cpu_model`, `gpu_model`, `binary_sha256`, `command`,
`p50_ms`, `p95_ms`, `p99_ms`, `cold_ready_ms`, `resident_bytes`, `pending_bytes`,
and `page_or_mesh_count`. Declare units and collection scopes in the experiment
record; resident GPU, logical payload and CPU memory are different measures.
Keep unavailable values null. Include original reports and command provenance;
the tool does not authenticate caller-supplied performance measurements.

The report reuses existing PNG byte and linear-light comparison helpers. Depth
is little-endian row-major float32 normalized [0,1], with 1 denoting background.
It reports coverage disagreement independently from error on common hits; no
common hits yields null error, not a false zero-error claim. Missing files,
invalid depths, provenance conflicts and unavailable measurements yield an
incomplete report and exit status 1. RGB-only evidence may still be retained
with an explicit unverified label. `quality_pass` is always null: these metrics
cannot approve the visual target, frame stability, readiness or performance.

Actual appearance metadata is required: `materials` is an object with
`appearance_version` and `render_appearance_version`; `sampling` contains
`samples`, `voxel_filter`, `color_encoding` and `render_scale`; `lighting`
contains actual `atmosphere`, `exposure`, `fog_density`, `fog_height_falloff`,
`sun_elevation`, `sun_azimuth`, `cloud_coverage`, `cloud_speed` and `overcast`.
These must equal observed sidecar/benchmark fields, not arbitrary shared labels.
Runs lacking these actual fields remain incomplete. Postprocessed captures need
additional provenance and currently remain incomplete; benchmarks reporting
changed visual settings are rejected.


The page experiment retains the existing hybrid coordinator, its full nested
`TerrainStreamer`, its GPU topology and canonical sample buffers, and its
native/collar caches. It replaces the outer draw only. The retained sampler still
runs during movement and supplies the collar appearance binding; it is additional
CPU work, not an optimized collar-only implementation. `page_experiment.gpu_bytes`
counts the additional page grid buffers only (30,142,608 bytes). It is **not** total
GPU residency. At seven levels, retained `TerrainGpu` buffers add 25,617,600 bytes
before native meshes, volume meshes, render targets, pipelines, transient uploads
and allocator overhead. Existing `streaming` and `native_streaming` report their
own scopes. A future collar-only source interface is needed before attributing
memory or generation savings to the page path.

The page CPU cache holds at most 1,152 pages. Its reported hard payload bound
also reserves two additional 576-page snapshots for generation/publication;
vector/hash/Arc metadata, canonical source state and the retained hybrid caches
are additional. Upload staging is bounded by one full 576-page replacement,
but uploads are currently issued together at publication rather than spread
through a per-frame byte budget. Cold/teleport publication can therefore have
substantial CPU or staging cost; no low tail-latency claim follows from bounded
memory. The old coherent cover remains active until both page and native covers
are ready, and the viewer refuses presentation if the camera outruns that finite
cover.

Only `--page-spacing 1` uses the same one-meter filtered endpoint samples as the
existing collar. Larger spacing is a separately labelled, empirically coarse
experiment; it can create a collar seam and is not quality accepted. Its far limit
must not be used as a geometric error bound. Page shading uses the existing
Legacy shared terrain lighting even when Regional source/materials are selected;
a Regional comparison with the adaptive lighting pipeline is not a matched
lighting experiment. The experiment neither implements a selected CDLOD
quadtree nor represents resolvable native vertical faces with its triangles.

## Existing selected-grid candidate and remaining comparison

`--scene fbm --surface-layout quadtree` already selects instances of a reusable
16×16 GPU grid (`renderer/src/surface_compare.rs`). It shares the FBM clipmap's
immutable `TerrainData` height/color/normal pages. CPU selection uses distance
versus patch width, balances neighbors to 2:1 and collapses shared edges. It is
a useful bounded selection/draw-cost candidate; it has no certified geometric
error selection, morphing, hysteresis, native handoff or volume ownership. The
parser freezes its source pages, so motion does not exercise source streaming.

Shared source pages preserve the clipmap approximation, not exact native voxel
geometry: filtered heights/colors average four canonical source queries, with
additional cached normal and parent sampling. Startup still builds all levels;
11 levels contain 14,644,080 bytes of samples before topology and instance
buffers. Camera draws reselect patch descriptors and upload instances only when
selection changes. Reported selection time and page bytes do not establish lower
canonical source cost or total memory usage.

This candidate is **not currently matched in appearance**. Its shader retains
older projected-face weighting and lacks the coordinate tint used by the current
clipmap/native shaders. Disabling voxel filtering still leaves that tint
mismatch. Existing tests validate WGSL, nonoverlapping complete square coverage,
2:1 balance, capacity and matching shared-edge positions/heights/colors/normals;
the source fixture uses one octave. The shared native/proxy lighting test does
not include this shader. Those tests do not certify full-source image quality,
temporal behavior or hardware performance.

The next selected-grid follow-up must share actual appearance code and add
adaptive geometric-error selection with measured quality, followed by matched
source/camera/lighting comparisons. Finish the bounded Phase A evidence first;
a negative result for the current uniform page experiment does not reject all
GPU-displaced terrain grids, establish an adaptive/native winner, or complete
the architecture comparison.
