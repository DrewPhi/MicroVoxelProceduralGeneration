# Agent context

Start with [the developer guide](../guide_for_devs.md) for the current layout,
commands and implemented behavior. This folder consolidates project planning and
handoff notes; it is not part of the runtime.

- [Foundation plan](FOUNDATION_PLAN.md): architectural direction and future
  capability probes. Later stages describe proposed work, not completed systems.
- [Performance](PERFORMANCE.md): initial native measurements, reproduction command
  and the limits of those measurements.
- [Profiling and cleanup](PROFILING_AND_CLEANUP.md): latest paired renderer and
  movement timings, retained optimizations, memory sampling and reproduction.

- [Voxel template review](VOXEL_TEMPLATE_REVIEW.md): source-level observations
  and candidate techniques from Jacco Bikker's renderer; no engine replacement.
- [LOD research and plan](LOD_RESEARCH_AND_PLAN.md): current detail/range audit,
  MishMash, Cubiquity, VoxelCore and literature; controls, joins, filtering
  and streaming references. [Implementation record](LOD_IMPLEMENTATION.md) describes
  completed work and remaining limits.

- [Ray renderer and streaming](RAY_RENDERER_AND_STREAMING.md): selectable voxel
  ray casting, responsive queues, culling, source review and comparison captures.
- [Octree and DAG study](OCTREE_DAG_STUDY.md): opt-in GPU comparison, exact subtree
  sharing and immutable/editable hybrid probe; measured benefits and limits.
- [Terrain presentation](TERRAIN_PRESENTATION.md): lake-plane seams, filtered slope
  shading, continuous material coverage and the two screenshot review rounds.
- [Terrain/climate research](TERRAIN_CLIMATE_RESEARCH.md): implemented static climate,
  diverse landforms, generation feedback, references and approximation limits.
- [Climate and weather plan](CLIMATE_WEATHER_PLAN.md): static climate baseline now
  implemented; live atmosphere, clouds and physical response remain deferred.

- [Player update checklist](PLAYER_UPDATE_PLAN.md): character/animation,
  walking controller, exact voxel collision and F5 cameras implemented.
- [Player research](PLAYER_RESEARCH.md): literature, Rust source review and
  grid-aligned animation tradeoffs.

## Constraints to preserve

- Base voxels are 1/16 meter. Authoritative occupancy and logical spatial state
  use integer cells; smooth camera rotation is allowed.
- One canonical base color per voxel on all faces, including water. Lighting can
  shade faces. Color variation must not introduce detail below voxel size.
- LOD is disposable display data. Keep axis-aligned grid geometry, stitched
  boundaries, and collision/picking independent of visual resolution.
- Target basic laptops: keep work queues/caches bounded, skip inactive or buried
  work, and measure changes instead of promising performance.
- Keep numerical material properties empty until a concrete system needs them.
  Future digging/building, water, life and electrical/mechanical systems motivated
  extension points; those systems are not implemented yet.
- Use Rust without a game engine. The native app is the active renderer; obsolete
  browser, image-to-OBJ and archived prototype sources were deliberately removed.

Likely next work: further visual tuning, terrain/noise and biome painting, then
native lighting, sky and clouds. Keep these systems separate from canonical world
storage. Record meaningful new measurements here with their hardware and scope.

The source and tests describe current behavior; update these notes when that
behavior changes. Do not commit generated worlds, build output or local credentials.
