# Selectable voxel ray casting and responsive streaming — 2026-10-28

Implements the approved reference review, streaming, visibility, alternative
renderer prototype, sharing investigation, comparison captures and cleanup scope.
DAG compression, a separate instancing renderer, gameplay digging, weather and
fluid simulation remain deferred. Numerical material properties remain empty.

## Reference-code review

Read Cubiquity's actual source through the GitHub connector:

- [`raytracing.cpp`](https://github.com/DavidWilliams81/cubiquity/blob/master/src/library/raytracing.cpp),
  blob `337359206106e39d2d7a5377f0436efad3696f16`: ESVO-style subdivision,
  integer node positions, empty-space skipping, entry/exit spans, footprint-based
  stopping and bounded traversal stack. Also reviewed its direction-dependent
  approximate material selection. Our native hits retain actual sampled colors.
- [`storage.h`](https://github.com/DavidWilliams81/cubiquity/blob/master/src/library/storage.h),
  blob `97fbc28a8b258bf969bface13c2dc73361e835fe`: material leaves, eight-child
  nodes, immutable/shared versus unshared nodes. No DAG storage was adopted.
- [`extraction.cpp`](https://github.com/DavidWilliams81/cubiquity/blob/master/src/library/extraction.cpp),
  blob `e221a23f3fac275adc55ce457b3661d11727493d`: separate glyph extraction.
- [`instancing_demo.cpp`](https://github.com/DavidWilliams81/cubiquity/blob/master/src/application/commands/view/instancing_demo.cpp),
  blob `969859cf984b6faed73fa70e359ae5881d87deb7`: currently calls `extractGlyphs`;
  the `findVisibleOctreeNodes` call is commented out. Do not present that demo as
  evidence of a finished visibility-driven open-world implementation.

Cubiquity is CC0. This pass uses original Rust/WGSL implementation code, with no
vendored C++ or translated chunks. References informed the algorithms and tests.

Other primary references consulted or cross-checked:

- [MGMishMash's author post](https://www.reddit.com/r/VoxelGameDev/comments/1vrqluv/improving_rendering_distance_in_my_micro_voxel/):
  direct macro generation, independently cached coarse edits, temporary fades,
  high-detail-first loading and loaded-band-aware adaptive visibility.
- [Laine/Karras, Efficient Sparse Voxel Octrees](https://research.nvidia.com/publication/2010-02_efficient-sparse-voxel-octrees):
  hierarchical voxel traversal and filtering. Our prototype uses a BVH of sparse
  column bricks, not a port of its contour-enhanced octree representation.
- [Geometry Clipmaps](https://hhoppe.com/proj/geomclipmap/): incremental,
  viewer-centered cached work and predictable residency. Block geometry is retained.
- [PBRT BVHs](https://pbr-book.org/4ed/Primitives_and_Intersection_Acceleration/Bounding_Volume_Hierarchies):
  compact bounded hierarchy and nearest-hit traversal. Our equal-count split is
  inexpensive for regular column grids; it is not an SAH-optimized hierarchy.
- [Intel Masked Software Occlusion Culling](https://www.intel.com/content/www/us/en/developer/articles/technical/masked-software-occlusion-culling.html)
  and [`MaskedOcclusionCulling.h`](https://github.com/GameTechDev/MaskedOcclusionCulling/blob/master/MaskedOcclusionCulling.h),
  inspected blob `4ace5258870bb2f62741afaa9e800e0823c98ea2`: conservative depth/coverage,
  current-frame query/raster interleaving and front-to-back ordering. Our small
  solid-prism coverage grid is original and considerably simpler than Intel's
  SIMD triangle implementation. No Intel code is included.

## Shared streaming and coarse generation

`native/src/streaming.rs` replaces the bounded FIFO with a latest-priority queue.
Each frame replaces queued requests, preserving only active/completed work until
acknowledged. Workers cancel obsolete requests at row boundaries. Worker zero
reserves capacity for urgent nearby requests and yields distant work when urgent
work appears. Queues/results remain bounded; shutdown wakes sleeping workers.

`src/render/lod.rs` now permits nearby splitting before unrelated coarsening when
there is residency headroom. `runtime.rs` forms up to 12 balanced local operations
per patch rather than four. Small working-layout headroom is 64 leaves; the GPU
byte budget still applies. Superseded incompatible patches are discarded. A fade
no longer blocks building the next patch; at most one old complete cover remains
for a 160 ms opaque transition. Work, uploads and overlap residency remain bounded.

Planning predicts half a second of horizontal camera movement and expands the
protected radius to include both the current and predicted positions (cap 32 m).
The native grid, player ticks, collision and picking are unchanged.

Coarse appearances formerly called native `generated()` through appearance/coverage
queries. `Generator::display_sample` now returns a band-limited column and its
already computed environment together. Summaries reuse those inputs for palette
and coverage integration. Coarse water also avoids native appearance calls.
A panic-based regression generator verifies coarse land and water can build both
render representations without requesting a native voxel or native appearance.

The shared summary pyramid is bounded in work as well as entries: 32–128 m
footprints may combine children; larger footprints use direct four-area quadrature
rather than recursively evaluating the entire 10 km region at 32 m. This changes
only display sampling, not the procedural baseline or generator version.

Edited columns still use sparse canonical overrides. Coarse summaries cache the
result independently of native render resources. Cache invalidation now queries
edited chunk revisions over each footprint, retaining unrelated summaries,
profiles and illumination. No dense world or native-render copy is required to
retain edits. Coarse sampling can still miss a very narrow unsampled feature;
this pass does not claim exact integration of every edit into every LOD footprint.
GPU mesh/resource keys retain the existing world revision contract; no new live
edit input or concurrent mutable-world protocol was introduced.

## Visibility

`native/src/visibility.rs` builds/refits a cached binary hierarchy over resident
terrain bounds. Traversal rejects entire branches outside the current frustum or
visibility distance, then processes remaining branches front to back.

The mesh path uses a conservative 64×36 software coverage/depth grid. Occluders
are base prisms proved solid across all sampled display columns, respecting
cavities and the native-height water plane. Only coverage cells wholly inside a
projected occluder hull receive depth; stored depth is the furthest prism corner.
A target is rejected only when its complete projected rectangle is behind covered
cells. Near-plane crossings skip occlusion testing. No previous-camera depth is
reused, so turning cannot leave stale occlusion holes. This is deliberately
conservative, and its efficacy depends on the view and available solid prisms.

The ray path skips empty hierarchy branches and visits nearer children first;
a nearest hit bounds later traversal and naturally hides farther voxel regions.

When native detail near the camera lags, visibility smoothly approaches 1024 m;
it expands toward 24 km when nearby detail is ready. Planning updates when this
limit changes materially and skips refinement beyond it. Both shaders blend fully
into their matching background over the final quarter of that range, and whole
regions beyond it are excluded from drawing. Static haze remains independently
adjustable. Still captures retain the full range for repeatable comparisons.

## Ray-casting prototype

- `src/render/ray.rs`: immutable 32×32 column bricks, vertical occupied runs,
  native colors for exposed cells, compact binary hierarchy, relocation metadata,
  cancellation and a CPU reference ray tracer. Cavities and edited appearances
  are retained at native resolution. Deep uniform interiors remain runs.
- `native/src/raycaster.rs`: one on-demand GPU arena, recycled/coalesced free
  ranges, a small top-level BVH rebuilt on cover changes, and the fullscreen pass.
- `native/src/ray.wgsl`: iterative nearest-first hierarchy traversal, slab tests,
  direct run intersections, exact grid face normals, native cell colors, existing
  selection/shadows/haze and fragment depth. No terrain triangles are extracted.

The hierarchy is a BVH over sparse voxel columns, not a full 3D SVO/DAG. It is a
practical first backend for the existing column/run world. Dense cave networks or
highly fragmented vertical runs may favor a richer per-brick hierarchy later.
A 32-entry stack covers the maximum top-level-plus-brick binary depth at the
8192-tile ceiling. The top-level reservation is 1 MiB and includes transient
balanced-patch headroom; a capacity regression covers 8256 leaves. Per-brick expanded colors are capped at 262,144. Actor/player
rendering remains the same voxel-derived raster path with correct shared depth.

The arena grows on demand up to the smaller of the terrain budget and 128 MiB,
matching the current storage-buffer binding limit. Ordinary allocations reserve
1/8 of that capacity for retiring coarser parents. Pressure triggers normal LOD
coarsening instead of overwriting protected data. Committed arena capacity is
reported separately from resident payload bytes. Growth briefly retains the old
buffer during a GPU copy, and fragmentation can still cause coarsening.

The Esc renderer button and `--renderer mesh|ray` select the backend. Preferences
persist; choosing a quality preset preserves the selected backend. Cached old
resources provide fallback while the other backend streams, and are evicted under
the shared budget. Switching back to meshes can temporarily retain ray fallback.
An existing no-AA shortcut incorrectly treated ordinary views as overviews; this
was corrected so AA-off retains the proper backend, sky and haze.

## Immutable sharing investigation

`cargo run --release --bin sharing` (or `./build/sharing`) measures 16 adjacent
native bricks for flat and procedural terrain. X/Z-normalized geometry has one
pattern in the flat case versus 16 in this terrain sample. Complete payloads have
16 distinct patterns in both cases because colors/coordinates differ. Each sample
occupies about 2.69 MiB unshared. Results: `output/ray_sharing.txt`.

This supports keeping geometry and appearance separable for future sharing. It
is not a measurement of all possible internal-subtree sharing, repeated authored
assets, or a DAG compression ratio. No interning or DAG was enabled on the basis
of this small probe. Existing immutable profile/summary/pose sharing remains.

## Validation and captures

Core: 72 tests passed, including caves/painted native ray hits, sparse interior
storage, cancellation, relocation, local edited-summary invalidation, coarse-only
sampling, near-first patches, canonical values and coverage/joins.
Native: 12 CPU/shader tests passed; three opt-in GPU tests passed on the Radeon
760M, including mesh/ray edited-surface agreement and fallback/live switching.
The new tests caught and fixed flat-tile ray bounds and an overly loose face-normal
tie test near native cell edges. Formatting and dependency-free Rust 1.75 build
passed. No dependencies or lockfiles were added.

`--stream-test` walks four seconds, sprints four seconds and turns for two seconds,
checking collision and nine active-cover sample locations within three meters of
the camera. It compares laptop (96 MiB) and balanced (256 MiB) configurations,
then switches backends. Reports are saved to `output/streaming/`; see performance
notes for actual values and limits. This is a short deterministic route, not a
promise of zero lag at every speed, location, radius or quality setting.

`tools/capture_renderers.sh` saves matching 1920×1080 valley and rear-third-person
views for each backend under ignored `screenshots/renderer_comparison/`. Seed 42,
quality 75, radius 6 m, 256 MiB budget, tile ceiling 768, same camera/FOV/haze and
20 synchronized frames. Both settle on the same tile layouts/effective 11.55 px
target in these captures. Logs sit beside the PNGs. Ray casting is currently slower
and uses more resident data here; it is an experiment, not an established speedup.
