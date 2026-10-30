# Terrain foundation update plan

Status: first implementation completed, 2026-10-29. See the
[implementation and measurements](TERRAIN_FOUNDATION_IMPLEMENTATION.md) for actual
behavior, commands and remaining limits. The proposal below preserves the original
audit after commit `6ddabe8`; its findings describe that baseline. Default native
resolution remains 1/16 m.

## Objective and boundaries

Generate convincing connected landscapes, preserve their features through LOD,
and make grid resolution explicit without making work proportional to the number
of possible native cells. Keep the existing Rust core, procedural baseline plus
sparse edits, balanced quadtree, native app and both selectable renderers.

Separate three contracts:

1. Physical landscape: seed, recipe/version and coordinates in meters determine
   terrain, drainage and material fields independently of the camera and grid.
2. Authoritative grid: a world has one fixed native cell size. Occupancy, edits,
   collision and logical motion are discrete on that grid.
3. Display: pixel size, view, feature error, residency and work budgets determine
   temporary display cells and geometry. Output resolution never changes the world.

Continuous functions are a construction/query tool, not fractional authoritative
voxels. Every displayed voxel keeps one solid base RGB on all faces; lighting may
shade it. Numerical material properties stay empty. Live weather, water dynamics,
waves, vegetation/life, player editing UI and a mutable DAG remain outside this update.

## Audit findings and qualifications

- `src/grid.rs` fixes 16 cells/m; generation, painting, player/physics, LOD extent
  and GPU packing contain further scale assumptions. The isolated character
  preview's alternate sampling size is not configurable world resolution.
- `native/src/lod_planner.rs` estimates height/error from nine regional environment
  samples, missing later local detail and channel carving. Neither sampled extrema
  nor a fixed margin constitute guaranteed bounds.
- `terrain_surface` combines existing regional erosion/sediment with local noise
  and channels. `surface_detail` uses nine wavelengths, 32 m through 0.125 m.
- Native surface coverage uses a hash of coordinates multiplied by 16. Environmental
  proportions can be appropriate while their fine spatial arrangement is incoherent.
- Mesh color payload is per exposed display-cell face even when rectangles merge.
  Ray attributes also remain a substantial cost. Compression needs measurements.
- Procedural generation currently supplies height columns; general queries and
  edited native runs can represent cavities. Coarse representations are approximations.
- Finer resolution also expands procedural player poses and cell-by-cell collision
  sweeps. Terrain streaming is not the only obstacle to centimeter/millimeter worlds.
- Latest paired profiles support mesh as default and ray as a selectable experiment.
  The latest 1440p cover measured 12.58 synchronized ms/frame with 3733 tiles and
  effective 4.77 px sampling. Older 33–45 ms figures are not the current baseline;
  settings and achieved quality must accompany every comparison.

## Phase 0 — Establish a terrain-quality benchmark

Extend `native/src/diagnostics/` before tuning generation. Create a versioned
manifest of deterministic physical viewpoints, walking routes and small patches.
Begin with three fixed seeds across temperate, arid and humid presets; select
representative mountains, plains/hills, a dry region and a river/confluence/lake
from generated diagnostics rather than assuming each seed contains every feature.
Keep a separate set of seeds for final validation.

Capture overview, near-ground and elevated views; geometry-only masks/depth,
material/feature maps and ordinary images. Record generator/spec version, physical
area, resolution, render size, settings, achieved detail and hardware. Keep baseline
recipes available for comparisons even when generator versions change.

Measure generation latency, startup, build/upload p50/p95/p99, cancellation,
streaming detail lag, update/submission/GPU-wait time, resident payload, capacity,
CPU caches and sampled RSS. Use optional hardware GPU timestamps where available;
do not label CPU-synchronized waiting as isolated GPU execution time.

For LOD, compare identical-camera geometry/feature masks with a high-detail
reference over bounded patches. Report silhouette displacement in pixels,
depth error and visible river/cliff retention separately. A high-detail reference
is not ground truth geology, and silhouettes alone miss interior feature loss.
Use small licensed real DEM/reference crops for slope/drainage/landform comparisons;
record units, resolution and license. Visual resemblance is not physical validation.

Gate: a reproducible baseline and complete artifact manifest, with no seed-42-only
acceptance. Reference renders and large datasets stay ignored or externally sourced.

## Phase 1 — Define world scale and physical sampling contracts

Introduce a validated `WorldSpec`/`GridScale` with exact positive rational meters
per cell, physical extent, vertical limits and checked conversion helpers. Preserve
signed integer `CellPos` and 32³ storage bricks; distinguish storage brick size from
physical streaming page size. Avoid a process-global mutable resolution.

Make `sample_surface_m`/material evaluation explicitly independent of native cell
size. Separate canonical field evaluation from display filtering by physical
footprint. Lower-frequency terrain and drainage must not change when selecting a
finer grid. Fine features use stable physical anchors, not new seeds per resolution.
Document the occupancy/height quantization rule; coarse and fine voxelizations
will differ at boundaries even though the underlying physical field is identical.

Audit hard-coded 16 conversions, 160000 extent, 32768 height, quadtree depth,
local-coordinate widths, 16-bit water heights, GPU floats and material hashes.
Move world-dependent values into the spec; use wider checked fields or region-local
packing where required. Internal power-of-two LOD steps do not require native
meters-per-cell to be a power of two.

Include resolution/spec and recipe identity in saves/cache keys. Read legacy saves
as their original 1/16 m grid where the baseline matches. Changed generation must
version/reject incompatible baselines clearly. Changing a saved world's resolution
requires an explicit future conversion policy; never reinterpret edited cell indices.
Runtime resolution switching and automatic cross-resolution edit migration are not
part of this plan.

Gate: existing 1/16 m behavior remains unchanged during this refactor. At common
physical coordinates, 25 cm, 6.25 cm and 1 cm test worlds agree on unquantized fields;
quantized heights obey the stated cell-size tolerance. Use tiny 1 mm patches for
conversion/packing tests, not a promise of interactive 10 km millimeter terrain.
Player dimensions, gravity, speed and step height must retain physical units.

## Phase 2 — Build multiscale, drainage-guided landforms

Retain regional climate, routing and sediment as the large-scale scaffold. Add
explicit stable feature descriptions: channel/connectivity graph, watershed/ridge
information, local flow/slope orientation, bedrock resistance and deposited material.
Produce deterministic detail over fixed physical patches, with bounded caches,
consistent boundary conditions and cancellation. Global drainage cannot be made
consistent merely by adding a small halo to unrelated local erosion simulations.

Prototype two detail approaches on the same patches:

- Multiscale erosion-inspired amplification: thermal relaxation, stream-power
  incision and deposition at progressively smaller physical scales.
- Controlled directional procedural patterns: cheaper ravines/ridges aligned to
  smoothed slope and existing drainage. This is amplification, not a replacement
  for connected watershed generation or a universal pattern for flat floodplains.

Select/combine based on measured quality and latency. Implement a mountain slope
and river-valley slice first: nested gullies, ridge shape, exposed rock and talus/
alluvial deposits. Then test hills/plains to ensure the mountain recipe does not
roughen everything. Keep soil and rock responses distinct. Where mass is tracked,
account for erosion, deposition and export; elsewhere label the process as a
geometric approximation rather than claiming conservation.

Fine structure should follow materials: fracture/weathering fields for rock,
smoother deposited soils, localized gravel and small channels. Only evaluate
features supported by the requested footprint, without changing canonical fields.
Do not promise scientifically meaningful geology at arbitrary millimeter scales.

Gate: channel continuity/downstream connectivity, stable confluences, preserved
watershed boundaries, repeatability under different request orders, matching patch
seams and bounded cost. Compare to existing noise using the full benchmark set.
Every new feature exposes bounds/metadata for Phase 3 instead of being invisible
to the planner.

## Phase 3 — Feature-aware hierarchy and screen-space LOD

Add a generator summary interface and a bounded physical-space summary hierarchy.
Track height range, a refinement-error bound/estimate, slope/normal variation,
water/channel coverage, cliff/geology boundaries and appearance variation. Include
the final channel/detail fields and edit effects, not only regional environment.

Distinguish conservative bounds from sampled estimates in the types. Bound known
detail amplitudes analytically where possible; use feature intersection queries
and adaptive samples elsewhere. A parent combines child bounds and reconstruction
error, not just averages. Coarse summaries must be buildable directly from fields
without first materializing all children/native voxels. Keep topological feature
constraints separate from statistical area averages so narrow rivers survive.

Drive refinement by projected geometric error and feature importance, with
projected cell size as a floor/limit, temporal hysteresis, nearby interaction needs
and existing memory/work budgets. Keep balance, axis-aligned geometry and staged
ready-patch swaps. Use conservative uncertainty for culling; sampled summaries
must never certify hidden terrain as solid. Edits locally invalidate summaries.

Gate: at equal measured budgets, improve silhouette/depth/feature metrics across
the manifest without breaking seams, collision independence or streaming. Establish
numeric thresholds from Phase 0 and report cases where budgets prevent meeting them;
do not advertise sampled error as a certified pixel guarantee.

## Phase 4 — Coherent physical material fields

Replace independent surface lottery choices with deterministic spatial fields
conditioned on exposure, wetness, slope, soil, geology, drainage and deposition.
Use physically anchored large patches, meter-scale structures and restrained
small-scale variation. Geological layers/fractures must agree with rock geometry.
Do not require fixed hand-authored biome borders to achieve coherent coverage.

Make canonical material-at-position and filtered display coverage derive from the
same fields. Update coarse summaries when this recipe changes; the old probability
mixture is not automatically the correct filter for new coherent patches. Each
native cell still gets exactly one material/base color. All faces agree, including
water. Keep appearance and presentation lighting separate.

Gate: patch continuity across pages/LODs, stable physical boundaries across tested
resolutions, matching native/coarse coverage and exact edited appearance. Compare
with material-only views so lighting cannot conceal a painting regression.

## Phase 5 — Resolution-scalable streaming, appearance and submission

Group residency/work ownership into fixed physical pages; benchmark candidate
page sizes before choosing defaults. A page can own implicit terrain plus optional
local render subdivisions and sparse edited 32³ bricks. Fine native resolution
must not force allocation of all storage bricks or hundreds of thousands of jobs
inside an unchanged meter-radius disk.

Choose display detail from projected size, visual error and explicitly local
interaction requirements. Native collision/edit queries do not force native
rendering throughout a fixed 2–6 m radius. Keep a native-radius override as an
expensive diagnostic option, with achievable-detail/budget reporting.

Compare current dense color payload against exact constant/palette/block-compressed
tiles and procedural reconstruction plus sparse overrides. Include both canonical
RGB and existing per-display-cell lighting metadata in correctness/accounting.
Choose the simplest measured win; GPU procedural evaluation is not automatically
faster or bit-identical to CPU floating-point code. Validate CPU/GPU parity before
using it for canonical appearance, and never store only edits while silently losing
nonprocedural material/color data.

Separately profile draw submission, pooled geometry/attribute buffers, grouped bind
groups and indirect draws. Retain a portable fallback; no assumption that GPU-driven
submission or a new renderer is required. Preserve the mesh/ray scene comparisons.

Add player/collision scaling probes: pose occupancy build cost, animation cache
size and sweep cost at each resolution. If needed, use hierarchical occupied
blocks/row intervals and exact integer-grid sweeps that skip empty/uniform spans,
rather than enumerating every interior body voxel and every traversed cell.
Preserve visible character dimensions and collision semantics. Millimeter walking
support is gated on these results, not just terrain rendering performance.

Gate: at fixed physical view/interaction settings, measure work, memory and queue
growth from 6.25 cm to 1 cm; 1 mm remains a bounded experiment until it passes.
Memory limits must include caches, transient builds/uploads and actor data, not
only resident terrain. No silent loss of nearby coverage to meet a timing target.

## Phase 6 — Sparse volumetric geological features

Add deterministic spatially indexed cave/arch/overhang descriptors in selected
geological regions. Compose them with the height-column baseline through bounded
run/occupancy queries. Untouched regions keep the fast column path. Features own
physical bounds and summary metadata; absent queries allocate nothing.

Use existing exact cavity tests as a starting point. Ensure both backends, coarse
LOD, visibility solid bounds, lighting approximations and edits understand the
features. A valid cave at native resolution is insufficient if LOD fills its mouth
or an occlusion proxy wrongly treats its interior as solid.

Gate: matching canonical queries and collision, visible entrances/overhangs across
LOD, seam-free region boundaries, editable features and bounded generation costs.
Prefer a limestone cave and one exposed arch as capability tests before broad
procedural cave networks.

## Delivery and acceptance

Deliver phases as individually reviewable changes with updated developer guide,
research notes, benchmark manifests and before/after images. Keep generated worlds,
temporary datasets and obsolete captures out of version control; retain chosen
evidence and references. Review output on the basic-laptop configuration throughout.

Start with Phase 0 + the behavior-preserving scale contract, then one mountain/
river vertical slice with summary metadata and coherent materials. This proves the
new approach before rebuilding every terrain type. Complete the broader hierarchy,
material and scaling phases based on those measurements; sparse 3D features follow.
Keep default native size at 6.25 cm throughout. Do not promise fixed FPS, constant
memory with arbitrarily fine voxels, or physically complete simulation.

## Primary research to evaluate

- [Schott et al., Terrain Amplification using Multi-Scale Erosion (2024)](https://h-schott.github.io/p/mserosion/)
  and [authors' source release](https://github.com/H-Schott/MultiScaleErosion).
  Candidate for terrain amplification using scale-dependent erosion/deposition.
  Benchmark its scheduling/data needs before choosing an online tile implementation.
- [Grenier et al., Real-time Terrain Enhancement with Controlled Procedural Patterns](https://onlinelibrary.wiley.com/doi/10.1111/cgf.14992).
  Candidate for inexpensive slope/flow-guided detail. The paper explicitly notes
  limitations on flat sedimentary terrain and global river-network generation.
  Reference timings are not predictions of this Rust engine's performance.
- [Existing LOD research](LOD_RESEARCH_AND_PLAN.md),
  [latest profiles](PROFILING_AND_CLEANUP.md) and
  [octree/DAG results](OCTREE_DAG_STUDY.md) remain the baseline for renderer choices.
  Review source licenses and preserve attribution before copying any reference code.
