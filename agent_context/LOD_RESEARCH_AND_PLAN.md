# LOD quality research and implementation plan

Research and source audit: 2026-10-28. The sections below preserve the initial
audit and proposal. Implementation subsequently proceeded at the user's request;
see [the implementation record](LOD_IMPLEMENTATION.md) for completed work and
remaining limitations. Weather, river generation and fluid simulation stay deferred.

## What the current renderer actually does

- `src/render/lod.rs` builds a complete quadtree cover of the finite 10 km world.
  Leaves are 32 by 32 display cells; native cells are 1/16 meter. Neighbors are
  balanced to at most a 2:1 spacing ratio. Neighbor levels participate in mesh IDs.
- Selection starts with a projected-cell-size threshold of 4 pixels, with existing
  split/merge hysteresis. It repeatedly raises that threshold if the layout exceeds
  the requested tile budget (384 by default). Tiles within 4 m horizontally are
  forced to split to native detail. This is a soft layout target: the final retry
  can return more tiles, particularly because of the protected near region.
- Selection measures horizontal distance to tile bounds, without terrain height or
  view direction. Drawing uses frustum culling, but refinement spends the tile
  allowance behind the player too. Drawing culling and refinement priority are
  separate problems; frustum culling already exists.
- `src/render/terrain.rs` obtains exact column runs at native resolution. Each coarse
  column instead samples five native surface columns, chooses the median ground,
  quantizes heights, and uses majority water coverage. Each level samples
  independently. This can erase narrow ridges/channels and change shoreline and
  material placement between levels. It is not a filtered hierarchy.
- The coarse display color is one appearance sample, rather than a footprint
  average. Greedy meshing already preserves individual native colors through a
  color atlas; fewer quads alone do not explain coarse color loss.
- Boundary side faces compare neighboring profiles and subdivide at the finer
  spacing. This already attempts stitching. Different coarse/fine estimates can
  create an artificial ledge even when a join has no hole. The reported glitches
  need separate tests for actual gaps, overlapping faces, and ugly but closed joins;
  this audit has not established a single defect causing all of them.
- `native/src/runtime.rs` retains the old complete cover until every mesh in its
  replacement is resident. This protects coverage but delays nearby upgrades while
  unrelated jobs finish. Planning waits for the previous target, with a 300 ms
  movement planning cadence. Uploads are bounded to two results / approximately
  3 ms per frame. The GPU mesh cache is capped at 128 MiB.
- The shader uses directional lighting plus a large uniform ambient term, and
  exponential fog with density 0.00011 per meter: approximately 10% fog at 1 km,
  42% at 5 km and 67% at 10 km. Fog affects distant contrast, but is not the main
  explanation for loss of geometry detail a few meters away. The camera far plane
  is 24 km; the world is not simply clipped at the apparent detail distance.

At 720 pixels high and 75-degree vertical FOV, one native cell projects to about
one pixel at 29 m. Doubling output height doubles that distance. Rendering native
cells across kilometers spends work on subpixel geometry. The goal is to retain
the *appearance and silhouette* of those cells through good display summaries.
Protecting a 32 m native radius alone takes roughly 800 two-meter tiles, before
transition rings, so a quality slider cannot deliver that inside a 384-tile target.

## Primary references and what transfers

### MGMishMash: directly relevant mesh-based microvoxels

[Author's render-distance development post](https://www.reddit.com/r/VoxelGameDev/comments/1vrqluv/improving_rendering_distance_in_my_micro_voxel/)

The author describes direct coarse-resolution generation, separately cached coarse
edits, temporary dithered LOD transitions, loaded-band-aware fog, and deterministic
IDs connecting distant props to their detailed counterparts. These fit our
procedural-baseline/edits architecture. We already generate coarse terrain directly;
the improvements are fidelity, scheduling and transitions rather than adopting that
idea for the first time. Distant props must remain render summaries, especially if
future ecology needs exact entities.

The reported 10–15 km at 45–50 FPS on M1 Pro and savings from temporary transitions
are the author's measurements, not predictions for our laptop. The comment about
25 LOD levels and 16-quad meshlets is by another participant, not MGMishMash.

### Cubiquity: compressed hierarchies and alternative rendering

[Repository and README](https://github.com/DavidWilliams81/cubiquity)
and [instancing demo source](https://github.com/DavidWilliams81/cubiquity/blob/master/src/application/commands/view/instancing_demo.cpp).
Source inspection included commit `cf5125c264c6bb58871b8276c475a6ad586eb243`.

Useful references: Sparse Voxel DAG compression, runtime edits with material IDs,
hierarchical traversal and experimental instancing. CC0 project code. Its CPU
pathtracer and experimental instancing demo are different workloads from a
streaming procedural game. In the inspected instancing update path the visibility
calculator call is commented out and cached glyph extraction is used; the README's
per-frame visibility description should not be treated as a current benchmark.

Do not replace our world storage with a DAG on appearance alone. A separate
compressed render hierarchy or compressed repeated static assets could be tested
later without replacing canonical mutable storage.

### VoxelCore: practical chunk scheduling and graphics controls

[Chunk renderer](https://github.com/MihailRis/voxelcore/blob/main/src/graphics/render/ChunksRenderer.cpp)
and [settings](https://github.com/MihailRis/voxelcore/blob/main/src/settings.hpp).
Source inspection included commit `9d0e7934080ee57214465fc4fcc07f1db35c293e`.

Useful references: pooled padded neighbor volumes, cancellable mesh workers,
distance-prioritized work, separate loading speed/distance controls, and independent
fog, shadow and SSAO settings. Its synchronous important-chunk path is not something
to copy into our frame loop without measuring stalls. The inspected code is not
evidence of a general solution to microvoxel LOD seams. Main project license is
MPL-2.0; this pass copies no implementation code.

### Literature

- [Losasso and Hoppe, Geometry Clipmaps (2004)](https://hhoppe.com/proj/geomclipmap/)
  and [Asirvatham and Hoppe, GPU implementation (2005)](https://hhoppe.com/proj/gpugcm/):
  nested grids, filtered terrain pyramids and incremental viewer-centered updates.
  Borrow predictable residency and incremental work. Their smooth triangle surfaces
  and vertex morphing are not our block geometry contract.
- [Lengyel, Transvoxel / dissertation (2010)](https://transvoxel.org/): explicit local
  transitions at 2:1 boundaries. This targets Marching Cubes surfaces, so its tables
  are not a drop-in fix for axis-aligned block faces. Borrow the principle of a
  precisely defined transition neighborhood and verify every boundary/corner case.
- [Laine and Karras, Efficient Sparse Voxel Octrees (2010)](https://research.nvidia.com/publication/2010-02_efficient-sparse-voxel-octrees):
  compact hierarchy, ray traversal, shading storage and filtering. A reference for
  later rendering experiments, not proof that replacing our rasterizer with ray
  traversal improves laptop performance. Contour-enhanced geometry is outside our
  current whole-cell geometry constraint.

## Recommended implementation order

### 1. Measurement and Esc menu

- [ ] Esc releases capture and opens a small native settings overlay; resuming
      recaptures intentionally. Clear held movement keys while the menu is open.
- [ ] Main control: **Terrain detail**, slider plus numeric input. Higher means a
      smaller target projected error and slower detail decay with distance.
- [ ] Separate **Full-detail radius (meters)**, **Terrain memory budget (MiB)** and
      **Haze** controls, with useful laptop presets. Validate compatible ranges and
      preserve active coverage if a new setting cannot fit. A full view-distance
      control can be separate; it should not silently stand in for detail quality.
- [ ] Show requested versus achieved quality, resident bytes, native/total tile
      counts, pending jobs and frame-time percentiles. Show budget pressure instead
      of quietly undoing the requested quality by raising the error threshold.
- [ ] Debounce slider-driven rebuilds, cancel obsolete requests, save settings.
      Capture same-position low/medium/high screenshots and repeat a walking route.

### 2. Correct joins and stable terrain summaries

- [ ] Add adversarial coverage tests: cliffs, diagonal ridges, four-tile corners,
      all 2:1 neighbor arrangements, water/land intersections, world edges and edits.
      Compare exposed boundary coverage, not merely mesh counts or screenshots.
- [ ] Define shared edge/corner ownership and a single transition sampling contract.
      Keep boundary vertices at integer cell coordinates and avoid overlapping skins.
- [ ] Replace unrelated five-sample estimates with bounded, deterministic hierarchical
      summaries: representative ground plus min/max height, material/water coverage
      and conservative feature/error indicators. Preserve peaks and shoreline shape
      according to screen error rather than choosing a maximum height everywhere.
- [ ] Compute summaries lazily and cache under explicit byte limits; do not create a
      full native-resolution pyramid for the 10 km world. Coarse generation must
      continue to skip fine-scale work, using regional data and bounded samples.
- [ ] Filter distant color in linear light using footprint coverage. Preserve native
      per-voxel colors and the visible proportions of rock, soil and vegetation.
      These are display summaries, never material changes to the canonical world.

### 3. Spend detail where it matters; stream without global stalls

- [ ] Refine according to projected terrain error, view direction, height, silhouette
      and shoreline importance. Retain a near safety region and a behind-camera
      buffer so turning does not expose unprepared terrain.
- [ ] Keep the existing cheap complete horizon cover. Replace ready local patches
      only when their children and affected neighboring stitches are ready; do not
      discard the global atomic swap until local coverage invariants are tested.
- [ ] Give jobs generation IDs, cancel stale work and prioritize visible nearby
      upgrades plus likely movement. Bound workers, CPU caches, uploads and residency.
      Account for old/new overlap memory during both swaps and transitions.

### 4. Transitions and visual depth

- [ ] Add brief dithered replacements only during a swap. Depth/coverage handling
      must keep terrain opaque with no holes. Bound concurrent transitions and
      double-draw cost. This reduces popping; it does not repair incorrect seams.
- [ ] Add inexpensive contact/ambient shading and bounded directional sun shadows,
      testing their GPU and meshing costs separately. Keep the same base color on
      all voxel faces and apply shading as lighting.
- [ ] Improve distance haze and sky contrast independently of detail settings; add
      optional antialiasing for subpixel shimmer. These are display sampling effects,
      not finer-than-voxel textures or changes to logical positions.
- [ ] Evaluate occlusion or meshlet culling only after profiling shows hidden geometry
      dominates. Frustum culling and hidden-face removal already exist.

## Acceptance criteria

Unchanged seed, canonical occupancy, colors, edits and collision at every setting.
Grid-aligned surfaces and closed joins during movement, edits and quality changes.
Same-camera captures that preserve recognizable ridges, shorelines and color regions
across LODs. Repeatable stationary and walking measurements reporting median/p95/p99
frame time, generation/queue latency, peak CPU/GPU bytes, and temporary overlap cost.
No new speedup or supported quality range is established by this research-only pass.
