# Full initialized world rendering

`./explore-1km` launches the step-zero Bouchet export downloaded into
`output/world-1km-initialized`. Full extent: 1000x424x1000 m; grid 125x53x125,
5 quadrics per 8 m cube. The FBM cave/overhang fit did not improve; this is
explicitly an initialization rendering test accepted by the user.

Binary export is loaded directly into packed f32 coefficients and borrowed for
GPU upload. Two storage pages bypass the default 128 MiB single-binding limit;
shader quad_at selects the correct page. No mesh or LOD added. CPU reference
supports both binary and old nested JSON. Seven analytic unit tests pass.

Radeon 760M capture at 960x540, 32 synchronized frames: overview continuous
GPU compute 5.80 ms; close voxel view 3.45 ms. These are two fixed viewpoints,
not a comprehensive performance claim. Exact voxel screenshot shows distant
subpixel stair-step aliasing; initial approximation exposes 8 m faceting.

Ground walking added: G / Esc toggle / --walk. Analytic vertical-only surface
query, 1.7 m eye height, yaw-relative WASD, 4.3 m/s, step/cliff rejection. Not a
full body collider or cave navigator. Eight unit tests pass, including sloped
plane grounding/motion and pitch independence.

Ground-view probes (760M,960x540,62 frames each): continuous looking down25deg
1.09ms GPU /2.51 average patches, level1.75ms/7.79, up25deg4.70ms/22.34.
Exact voxels level2.45ms. Traversal counts correlate strongly with total GPU
compute cost; timestamps still combine traversal, intersections and shading,
so these do not isolate traversal from quadratic-test cost. Raw reports:
output/world-1km-initialized/walk-timings.json.

## Appearance / aliasing pass
- AA default on; T / Esc ANTIALIASING toggles. One primary ray/pixel, 16-phase
  Halton jitter; reproject into previous camera, depth-reject history, bilinear
  taps, bounded 85% history contribution; sky direction reprojection included.
- Two RGBA32F history textures hold linear color + ray distance, ping-ponged.
  Reset on resize, mode changes, AA switches and >5m frame translation.
  Menu composited after history and gamma, so UI is not accumulated.
- Projected footprint (including grazing angle) filters subpixel voxel face
  lighting toward analytic normals. Geometry/occupancy unchanged; approximation
  in distant shading only. Voxel color hash stable, ±8% fine grain plus ±4%
  2m-scale tone; fine variation fades when subpixel. Earthier palette and warm
  sun/cool ambient, no extra shadow rays or textures.
- --verify disables jitter; --no-aa comparison; --camera-step meters/frame
  permits motion regression captures. Nine unit tests passed. 80 CPU/GPU voxel
  rays, zero mismatches. Stationary and translating-camera captures inspected.
- 760M960x540 closeview: AA2.65ms vs off2.50ms in one 62-frame comparison;
  not a broad benchmark. Raw JSON next to PNG comparisons.
- Does not claim all aliasing/ghosting eliminated; near resolved voxel steps
  remain, initial terrain's broad facets remain, and temporal disocclusions may
  still soften/trail. Custom small filter, not FSR. References in README.

AA sliders added to Esc menu: temporal_strength default0.5, face_smoothing
 default0; click/drag with numeric 0-100 readouts. Independent controls, master
AA gates both. Temporal zero disables jitter; lighting may still be enabled.
Changes invalidate history. Verified clicks (72 temporal,25 smoothing) in private
Xvfb GPU session, screenshots sliders-default.png and sliders-adjusted.png.
Rebuilt ./analytic. Settings session-only.

Spatial sampling: 1–4 discrete menu slider; fixed within-pixel patterns, independent
trace and shade per sample, linear color averaging. Temporal default changed to0.
MasterAA gates all filters. Nine tests pass; 1/2/3/4GPUcaptures produced.
Stationary4sample PNG identical after1 and32frames.960x540fixedview approximate
GPUcosts3/6/8.6/11.4ms respectively. ComparisonPNGspatial-1..4 under world output.

## Selectable traversal
`src/analytic/traversal.rs`: conservative padded polynomial upper bounds,
all-air mip hierarchy (8 levels,3,806,996B for1km scene;307244/828125air patches).
WGSL air_jump skips largest proven-air box, recomputes exact grid-boundary cell
and entry normal. Continuous and accelerated-voxel modes supported; reference
voxels unchanged. Esc right panel grid / hierarchy; hardware grey unavailable.
Defaultgrid; headless --traversal. Changing method invalidates temporal history.

Actual760M exposes wgpu experimentalAS+rayquery flags. Blocker is wgpu24 API:
BlasGeometries only TriangleGeometries, no procedural AABBs. No hardware backend
implemented; don't imply grey menu option means hardware lacking RT support.

62-frame960x540 longview: continuous7.02->4.39ms, voxels7.56->5.26ms.
Closeview continuous1.39->1.42ms, voxels1.90->2.05ms. Four imagepairs: exactvoxel
RGBidentical; continuouslongviewmax1leveloutof255. CPUreference80rays each across
multiplecamera/pitch cases passed0mismatches. Tests cover conservative upper
bounds and odd-sized hierarchy parents. GUI selection inspected viaXWarpPointer.
