# River, LOD and landform correction plan

Status: implementation pass completed 2026-10-29; see
[TERRAIN_CORRECTION_IMPLEMENTATION.md](TERRAIN_CORRECTION_IMPLEMENTATION.md) for
actual scope, verification and remaining limitations. The proposals below are
retained as design rationale, not a claim that every acceptance target passed.
Based on [the ten-view screenshot review](TERRAIN_SCREENSHOT_REVIEW.md), a fresh
source audit and the primary research/source references below.

## Shared contracts

- Keep default 6.25 cm native cells, configurable world scale and deterministic
  physical-space terrain. All new distances/amplitudes use meters.
- Preserve integer-grid occupancy and logical movement, smooth camera rotation,
  axis-aligned visible geometry and one solid base color per voxel. Finer native
  worlds sample the same landscape; display quality never changes collision.
- Preserve the procedural baseline plus sparse edits, both renderers, balanced
  quadtree, staged ready-patch swaps, nearby streaming priority and bounded caches.
- Material property maps stay empty. Water remains static occupied cells. Live
  fluids/weather, waves, vegetation and a renderer/DAG rewrite are outside scope.
- Retain the import feature without further dataset work. Procedural generation
  changes get a new baseline identity; incompatible old edits must be rejected.

## 0. Lock the regression cases and measure the actual constraints

Reuse all ten cameras in `tools/capture_terrain_sweep.py`, plus a small held-out
route through seed 2027. Save old metrics before changing generation. The river
and lake high/low pairs must keep identical camera, FOV, raster size and budgets.

Add a bounded reference patch for the raised river, a small tributary/confluence,
a low grassy slope and a steep rock face. Record canonical cross-sections,
water/material masks, geometry silhouettes and native occupancy. A Q95 capture
that reached 10-pixel sampling is not native reference geometry.

Before changing selection, record why splits were admitted/rejected: interaction
radius, projected cell size, terrain error, water feature, view priority, required
2:1 balancing, tile limit and GPU residency. Break down visible, nearby-turning,
balance-only and distant/behind-view leaves. Current `achieved_pixels` is a relaxed
selection threshold, not a measured maximum visible error.

## 1. Put rivers into coherent valleys

### Confirmed source-level problems

`generation/terrain.rs` builds independent short `Channel` segments from the
16 m routing grid. Water levels use routing-grid elevations, while the rendered
surroundings use a sampled continuous surface with later detail. Inside a channel,
the bed is assigned directly and can raise terrain; adjacent ground can remain
lower. Selecting the nearest segment by normalized distance can change abruptly
at junctions. Screenshot 04 and the canonical 4.84 m water/bank mismatch are the
first regression case.

### Proposed implementation

1. Retain the global drainage scaffold, but give nodes, reaches, basins and outlets
   stable IDs. Extract connected reaches between source/junction/outlet nodes;
   store downstream relationships, catchment/flow proxies and arc length.
2. Fit constrained, piecewise curves within each reach's existing drainage
   corridor. Preserve junction/outlet positions; reject smoothing that crosses
   ridges, crosses another reach, cuts across a basin divide or creates a loop.
   Add controlled bend/width variation by slope, confinement and discharge proxy;
   steep confined streams and broad lowland rivers should not share one shape.
3. Derive a consistent longitudinal profile from the same physical surface used
   for carving. Solve downstream constraints in graph order, keeping lake outlets
   and confluence levels explicit. Water is non-increasing downstream; allow flat
   lake reaches and discrete drops rather than inventing an uphill link. A locally
   impossible profile triggers bounded corridor re-fitting/breaching or a clear
   diagnostic, not arbitrary infinite excavation or silent filling of dry land.
4. Build bed, wetted channel, bank transition and broader valley/floodplain from
   that shared profile. The ordinary river modifier cuts into the existing field;
   it must not raise a thin strip into an aqueduct. Keep any depositional feature
   explicit and separately bounded. Bank geometry must leave water confined and
   be continuous at junctions, including unequal tributary widths.
5. Compose confluence and lake-inlet/outlet modifiers by shared feature ownership
   and junction constraints, not unrelated nearest-segment water/bed decisions.
   Recheck the modified corridor for unintended barriers or disconnected flow.
   If a correction changes a lake outlet or basin connection, reconcile that
   basin's static level/extent and wetness metadata; do not preserve a stale full
   lake above a newly lowered spillway. Bound regional reconciliation passes and
   report unresolved cases instead of iterating indefinitely.
6. Paint exposed bed/banks, gravel and deposited margins from these same fields;
   this replaces uniform beige outlines with geometry-aware transitions.

Split hydrology/carving helpers out of the already large `terrain.rs`, likely into
`generation/hydrology.rs` and `river.rs`; keep the existing generator adapter.
Reuse the spatial bins with curve/valley bounds and add counters for local candidate
visits. Precompute profiles once per world, not per queried native cell.

**Acceptance:** reproduce and eliminate the raised-wall case; connected source to
outlet paths and shared confluence levels; monotone downstream profiles; carved
beds do not raise the pre-carve field; explicit tests of actual wetted cross-sections
and neighboring banks, not only vertex heights. Include straight/diagonal reaches,
unequal confluences, steep streams, dry channels, lake outlets and adjacent basins.
Verify deterministic results and request-order independence across resolutions.

**Research application:** [Procedural Riverscapes](https://people.cs.uct.ac.za/~jgain/wp-content/papercite-data/pdf/peytavie2019.pdf)
is especially relevant because it derives river trajectories and bounded carving
modifiers together. Borrow the static profile/cross-section approach, leaving its
animated flow system out. [Génevaux et al.](https://cs.purdue.edu/homes/bbenes/papers/Genevaux13ToG.pdf)
provides the broader drainage/terrain hierarchy reference; this pass need not
replace our existing global network with its upstream-growing generator.

## 2. Preserve visible water features and spend LOD work selectively

### Representation before selection

Extend coarse summaries with compact water-body/reach IDs, occupied intersections,
width/coverage, bed/level ranges and connectivity at cell boundaries. Distinguish
lake-level constants from river profiles. A cell containing multiple unrelated
water bodies must not turn them into one averaged water level.

Use analytic corridor/cell intersection or bounded adaptive sampling along known
features. Existing four-sample quadrature and `wet >= 0.5` are not sufficient for
narrow streams. Geometry and filtered appearance need separate treatment:

- For resolvable channels, refine locally or emit a bounded grid-aligned finer
  water/bank representation that fits the river into the land and preserves joins.
- For subpixel channels, use coverage-aware filtering at the display-cell level;
  accept legitimate loss below image resolution rather than painting every partly
  wet coarse cell completely blue or drawing an off-grid river ribbon.
- Bound local subdivisions. If one feature cannot fit the current budget, report
  the unmet feature requirement and degrade deliberately; do not recursively
  materialize all native voxels. Preserve edits through the existing overlay.

Both mesh and ray backends consume the same display-column/water description.
Native occupancy and picking continue to use exact generator queries.

### Planner allocation

The current selector retries whole layouts with a globally relaxed threshold
(`threshold *= 1.25`) whenever the balanced result exceeds budget. It also uses
horizontal facing rather than a complete 3D view test when assigning peripheral
importance. This can spend detail on less useful regions while degrading a
visible river everywhere.

Keep protected nearby coverage first. Prototype a bounded priority queue of
candidate splits, ranking projected feature/geometric benefit against the actual
incremental tile cost, including the local 2:1 balancing closure. Prioritize the
current 3D view, with a modest turning reserve; retain coarse complete coverage.
Use uncertainty conservatively: sampled bounds can rank importance but must not
certify terrain as invisible. Use valid conservative bounds for hard rejection.

Compute local balancing closures without cloning/rebalancing the entire layout
for every rejected candidate. Keep hysteresis, cancellation and staged resident
patch swaps. Use recorded build/byte estimates where available; do not generate
fine terrain merely to predict its cost. The existing selector remains an A/B
reference until the new one beats it at equal budgets.

Update diagnostics to report active constraints and measured projected cell-size
statistics for visible leaves, alongside requested quality and feature failures.
Do not rename an approximate criterion into a guaranteed pixel-error bound.

**Acceptance:** thin/diagonal stream, confluence, separate adjacent lakes and
river-to-lake tests; native/fine versus coarse masks measuring water continuity,
width, false widening and shoreline displacement. Establish tolerances for
features at least two pixels wide; exempt true subpixel loss explicitly. Rerun
both backends and walking/sprinting/turning: nearby misses must not worsen, and
memory/queue budgets must hold. Compare the new allocator at equal residency and
tile limits, not merely a higher graphics preset.

**Reference:** [Geometry Clipmaps](https://hhoppe.com/geomclipmap.pdf) and the
[GPU Gems implementation discussion](https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry)
are useful for bounded multiresolution caches, separate elevation sampling and
boundary consistency. Keep our voxel/quadtree representation; do not adopt their
sloped triangles or off-grid geometric morphing. The water-identity and allocation
changes above are proposed adaptations for this engine, not claims from that work.

## 3. Diagnose contour bands, then fix geometry or shading at its source

Use screenshot 10's low grassy slope plus a bounded native-reference patch. Add
temporary diagnostic captures for unlit material, face/slope illumination, display
step, geometry/depth and mixed-LOD boundaries. Repeat at fixed native resolution
with two LOD settings and a short camera movement. This distinguishes:

- Intentional native voxel stair edges.
- Coarse horizontal step being imposed on vertical height.
- Step-dependent or discontinuous slope-light sampling.
- Actual stale/mismatched boundary geometry or temporal transitions.

The coarse path currently rounds land heights to multiples of `step`; one scalar
step also scales all axes in the mesh path. Test retaining native integer Y bounds
while keeping coarse X/Z spans. This represents merged native voxel columns, not
fractional voxels, and need not enumerate hidden native cells.

This is a representation change, not just deleting a rounding operation. Separate
native-coordinate rectangle geometry from display-cell color addressing; emit
exposed vertical intervals/rectangles directly. Avoid expanding a tall distant
side into a native-resolution color atlas. Ray runs already use native Y endpoints;
keep both backends consistent. Update boundary junctions, cache identity, clipping,
cavity/arch handling and exact-neighbor exposure tests together.

If lighting causes a band, evaluate filtered slope illumination from stable
physical-space anchors and blend its sampling footprints consistently across LOD.
Replace the hard-coded `step.max(16)` assumption with an explicitly physical
footprint. Preserve crisp face shading when native cells are resolvable and never
blur colors within one voxel. This is correction of existing shading, not a new
weather/lighting pipeline.

**Acceptance:** flat planes, shallow ramps, abrupt cliffs, mixed 2:1 neighbors,
water banks, caves/arches and far-from-origin/fine-grid cases. Shared boundaries
must agree in native coordinates; no sky holes, stretched walls or duplicate
surface slivers. Compare silhouettes/depth and both backends. A short movement
capture is required before declaring temporal bands or joins fixed.

## 4. Broaden mountain and middle-scale landforms

After stabilizing river/LOD comparisons, replace the repeated-looking relief
vocabulary with a small set of deterministic, geology-conditioned feature fields:

1. Derive ridge/valley orientation, curvature, flow, exposed bedrock and soil
   thickness at fixed physical scales. Keep global drainage authoritative; small
   features must not arbitrarily dam it or redirect it across a watershed divide.
2. Add bounded ledges/resistant strata and scarps where sedimentary geology and
   exposure support them. Use slowly varying bedding thickness/dip/warping, not
   globally repeated horizontal stripes. Let resistant ridges differ from softer
   eroded slopes; variation changes geometry as well as paint.
3. Add local footslope aprons, debris/fan shapes and terrace/swale structure where
   curvature, confinement and deposition proxies justify them. Preserve
   open/flat plains rather than roughening every location. Exclude/resolve overlap
   with river corridors and lake surfaces.
4. Couple rock/soil/gravel patches to those generated structures. Reduce repetitive
   ribbing and rotate/vary its wavelength/profile coherently over larger regions.
   Preserve the existing small-scale, material-dependent surface detail.

Expose feature bounds/amplitude allowances to summaries and cache keys from the
start. A static talus/fan shape is a geometric approximation unless actual material
transfer is tracked; do not claim a mass-conserving physical simulation.

Use [Controlled Procedural Patterns](https://onlinelibrary.wiley.com/doi/10.1111/cgf.14992)
for guided hierarchical detail, while respecting its stated limitations for flat
floodplains, cliffs/table mountains and global river generation. Use
[Multi-Scale Erosion](https://h-schott.github.io/p/mserosion/) for the hierarchy of
thermal/incision/deposition effects. Its [MIT code release](https://github.com/H-Schott/MultiScaleErosion)
documents an erosion/thermal/deposition/upsampling sequence and requires OpenGL
compute; importing its runtime is not required. A bounded local erosion prototype
is optional only if the cheaper feature-field approach fails the visual/cost gate,
and must share coarse drainage/boundary conditions rather than independently
eroding unrelated tiles.

**Acceptance:** recapture all ten views and held-out dry/humid examples. Preserve
macro mountain shapes and flat lowlands; show distinguishable ridges, ledges,
debris and shore/valley transitions at middle distances. Compare material-only
and geometry views so color cannot conceal a geometry regression. Preserve
cross-resolution physical fields, seams and river connectivity.

## Efficiency, source use and delivery

- Regional topology/profile work happens once at the existing coarse resolution.
  Local queries visit spatially indexed nearby features. No per-voxel fluid solve,
  global fine heightfield or full-resolution reduction for distant tiles.
- Establish isolated same-machine generation/build/streaming baselines before
  implementation. Proposed review gates: no more than 25% regional-generation
  slowdown, 15% increase in coarse-tile median/p95 build time or 32 MiB additional
  persistent regional/feature data without an explicit measured quality tradeoff.
  These are acceptance targets to evaluate, not performance promises or current
  measurements. Existing GPU, cache, upload and queue ceilings remain hard limits.
- Preserve nearby miss counts and movement frame-time percentiles; profile the
  allocator separately from generation, meshing and GPU submission. Repeat only
  when a change/failure warrants it. Do not accept a beauty improvement that
  simply spends unreported extra memory or starves walking detail.
- Reviewed [RichDEM's Barnes2014 source](https://github.com/r-barnes/richdem/blob/master/include/richdem/depressions/Barnes2014.hpp)
  for priority-queue/pit-queue processing and watershed labeling. It is GPL-3.0:
  use the published algorithm as a reference for an original Rust implementation;
  do not copy its implementation into this MIT project. Keep virtual drainage
  fill separate from physical terrain/lake water. Our current priority-flood
  scaffold does not need replacement merely to fix channel embedding.
- Deliver diagnostics first, river correction second, water representation/planner
  third, contour correction fourth and landform/material refinement last. Keep
  each step reviewable with the fixed screenshot manifest and numeric regression
  cases. Canonical generation changes require a generator-version bump and clear
  save incompatibility; display-only changes do not reinterpret saved cells.

The papers inform the approach; these implementation choices and cost gates are
specific proposals for this Rust voxel engine. No literature timing is treated as
a prediction for this laptop.
