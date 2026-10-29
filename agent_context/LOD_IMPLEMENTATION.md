# LOD implementation — 2026-10-28

Implements the four-stage direction in [the research plan](LOD_RESEARCH_AND_PLAN.md).
No simulation, material-property or canonical voxel-resolution changes. The world
remains 10 km square at 1/16 m, with integer logical positions and cell-aligned
render vertices. These changes are independent of future fluid/weather work.

## Implemented

1. **Controls and measurement.** Esc opens a native, dependency-free overlay with
   detail, protected native radius, terrain cache and haze sliders plus numeric
   input; laptop/balanced/high presets; ambient, shadow, antialias and fade toggles.
   Changes debounce, movement input clears and local preferences persist in ignored
   `native/settings.conf`. Cosmetic changes do not rebuild terrain. Requested and
   effective projected-cell size, memory, tiles, work queue and frame percentiles
   are visible. Maximum detail requests 1 px; budget pressure remains explicit.
2. **Stable samples and joins.** Display-only procedural sampling drops unresolvable
   noise octaves. Four-area samples combine heights, wet coverage and linear-light
   colors; above 32 m they form a lazily generated four-child hierarchy with a
   shared 65,536-entry limit. Worker profile caches stay bounded. Native material
   colors and edited appearances remain exact. Existing 2:1 boundary profiles are
   verified with adversarial cliff/water/corner tests. Greedy T-junctions are removed
   by collecting axis-line corner positions across mesh/stitch parts, then
   triangulating only affected rectangles. Convex-perimeter ear removal uses linked
   neighbors and row/column counts, including long thin cliff faces. Shared tile edges include the finer
   grid's vertices. Other rectangles keep the instanced two-triangle path.
3. **Selection and streaming.** A latest-intent background planner owns terrain-error
   estimates and quadtree selection; requests/results are bounded and versioned.
   Detail favors the view, retains a near turning buffer and uses sampled height
   bounds/curvature. Ready local patches include all changed neighbor mesh keys;
   the old complete cover remains until they are resident. Workers cancel obsolete
   jobs at row boundaries and prepare triangulation off the rendering thread.
   Uploads have a per-frame time allowance. Cache reduction retires unused meshes
   and coarsens under the previous ceiling until the new target fits. Exhaustion
   preserves a valid last cover rather than turning terrain into holes.
4. **Presentation.** A 160 ms dithered replacement composites two complete opaque
   views, including sky. It has one previous cover at a time and no double draw
   when stationary. Ambient visibility is stored separately from RGB in the existing
   face atlas's alpha byte, with no extra geometry. A small static terrain horizon
   map approximates sun shadows. Directional sky and haze share colors. Optional
   bounded edge filtering reduces spatial aliasing without sub-voxel world textures.

## Costs and limitations

- The pixel number is a projected display-cell target, not a mathematical bound
  on silhouette error. Terrain bounds/curvature use regional samples, so a narrow
  unsampled ridge or cave can still be lost. Coarse summaries are not exact sums
  of every native voxel; that would defeat direct coarse generation.
- The terrain budget counts mesh/atlas/triangle-reference/uniform buffers. It is
  not total GPU memory. Two presentation targets, depth, 256 KiB horizon texture,
  bounded UI/actor resources and driver allocations also consume memory.
- Lowering a budget may temporarily retain the old ceiling while coarsening. It
  does not destroy the active cover just to hit a new number immediately.
- Fades can double scene rendering temporarily. Antialiasing is a contrast-aware
  spatial filter, not full FXAA, MSAA or TAA. It does not eliminate temporal shimmer.
- Ambient visibility is per display-cell face; no smooth gradients within native
  cells. Sun shadows are coarse, static terrain-only approximations, not cascaded
  voxel shadow maps. Trees/actors/caves and edits are not correctly shadow casters.
- Occlusion/meshlet culling and a compressed DAG renderer remain optional future
  profiling experiments. Frustum culling, hidden-face removal and greedy merging
  already exist; this update preserves them.
- One driver crash occurred with an interpolated screen-coordinate sky input on
  RADV. Computing it from fragment pixel position avoids that path. GPU initialization,
  opaque fade phases and readback are covered by the opt-in GPU regression test.

## References used before difficult changes

- [MGMishMash's meshing-based microvoxel development post](https://www.reddit.com/r/VoxelGameDev/comments/1vrqluv/improving_rendering_distance_in_my_micro_voxel/):
  direct coarse generation and temporary, rather than permanently overlapping, LOD fades.
- [Geometry clipmaps, Losasso/Hoppe](https://hhoppe.com/proj/geomclipmap/) and
  [GPU implementation, Asirvatham/Hoppe](https://hhoppe.com/proj/gpugcm/):
  incremental cached levels and bounded terrain work; smooth vertex morphing is not used.
- [Mikola Lysenko's greedy-meshing/T-junction discussion](https://0fps.net/2012/07/07/meshing-minecraft-part-2/):
  why surface reduction and conformity are separate questions. T-junctions are not
  assumed to break every GPU; actual screenshots motivated the local fix here.
- [Lysenko's voxel ambient-occlusion article](https://0fps.net/2013/07/03/ambient-occlusion-for-minecraft-like-worlds/):
  view-independent neighboring-voxel lighting. Our implementation stores one
  visibility value per face, rather than interpolated per-vertex AO.
- [Lottes, FXAA white paper](https://developer.download.nvidia.com/assets/gamedev/files/sdk/11/FXAA_WhitePaper.pdf):
  contrast-based filtering/early-out. The shader is an original small filter, not
  copied FXAA code and not a claim of equivalent quality.
- Cubiquity and VoxelCore source observations, exact links and inspected revisions
  are retained in the research document. No third-party project code was vendored.

## Cleanup and reproduction

Removed 38 ignored legacy PNG/PGM/OBJ files from the repository root (205.4 MiB).
Intentional assets and useful character/climate/LOD reference captures remain.
New renderer captures default to ignored `output/`; build caches stay ignored.
No additional dependencies or external font/image assets were added.

Run `--lod-test` for same-camera low/balanced/high captures, a 64 MiB cache shrink,
and 180 paced moving-camera frames. See [performance notes](PERFORMANCE.md) for
recorded hardware and measurements. Core tests verify canonical geometry/color,
local patch convergence and cliff/water coverage; native tests validate settings,
scaled UI input and all WGSL modules. The opt-in GPU test checks surface readback,
all opaque transition phases and preservation under allocation failure.
