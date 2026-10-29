# Terrain presentation — 2026-10-28

This pass targets the bare terrain's readability before adding weather, clouds,
fluid simulation or more lighting systems. Preserve integer canonical occupancy,
1/16 m cells, one solid base RGB per native voxel and axis-aligned display faces.

## Changes and diagnosis

- **Lake ledges:** coarse profiles previously rounded water to their own LOD
  spacing. A lake at a native height not divisible by that spacing acquired
  different planes and exposed dark vertical walls between levels. Water now has
  an enclosing coarse profile rounded upwards and a native-height clip plane.
  The vertex shader clamps water geometry to that integer Y. Flat neighboring
  cells consequently share exactly the same plane; artificial walls above it
  collapse. Water faces are collected separately by plane/spacing, then appended
  to the existing draw parts, without adding a draw call per plane. Native water
  uses its exact run end. Underwater terrain slope/AO never colors the lake top.
  Genuine changes in river height and approximate coarse shorelines still exist;
  this is a rendering fix, not a hydrology or fluid update.
- **Grassy contour bands:** quantized terrain makes stair risers. Lighting each
  riser as an independent axis-aligned face exaggerates their repetitive contrast.
  Terrain workers now cache a broad slope's sun illumination, using differences
  of unquantized summary heights at a footprint at least one meter wide. Atlas
  alpha carries six bits of that illumination plus two bits of near ambient
  visibility; RGB remains untouched. The existing 32-byte GPU quad packs an
  illumination-mode bit and optional native water plane into unused padding.
  Actors retain the original alpha interpretation. All faces of a distant
  display cell approach the same slope illumination. The blend uses projected
  native voxel size (0.8..3 px) with the actual projection scale; nearby resolvable
  voxels retain distinct face shading. This is a filtered lighting approximation,
  not smoothed or off-grid world geometry, MSAA or TAA.
- **Coverage transitions:** environment surface coverage changes continuously
  with soil depth, slope, moisture, seasonal warmth and wetness. Diagnostic biome
  labels retain their existing thresholds. A native cell deterministically picks
  one solid material from normalized coverage weights. Distant summaries integrate
  the expected coverage in linear light instead of aliasing that cell selection.
  Channel gravel/silt and edited columns bypass procedural coverage blending.
  The four-sample footprint and bounded coarse hierarchy remain in place.
- **Rock bands:** sandstone/limestone tone now has weak, interpolated, gently
  warped 12 m strata instead of independently colored sharp 2 m bands.
- **Scale/depth:** default vertical FOV is 65 degrees (about 97 degrees horizontal
  at 16:9), still adjustable with brackets. Existing static distance haze leaves
  the first 60 m clear and applies 1.65 times the previous density afterwards.
  Sky and haze colors still agree. No new atmospheric simulation or cloud system.

The native surface material recipe changed, so generator version is **3**.
Existing incompatible edit saves are rejected by the existing identity check;
no automatic migration or deletion of saves is performed.

## Bounded work and limits

The extra per-worker illumination cache has at most 16,384 entries, and shares
existing bounded area summaries. Cache misses run on mesh workers. No extra GPU
texture, render pass or GPU bytes per quad/atlas cell are required. CPU quad
metadata grows slightly. Several colors are evaluated for transitional coverage
while making meshes; simulation ticks do not visit those cells every frame.

No claim of perfect antialiasing: geometry and cell-color edges remain
single-sampled, followed by the existing modest edge filter. Fine shimmer,
coarse silhouette stair steps, sampled narrow-feature loss and abrupt geological
region changes can remain. Slope illumination can soften the distant voxel look;
it is deliberate, and never changes collision, edits, material values or picking.
The fixed sun is shared with the existing shader. Changing sun direction later
must invalidate/recompute this display-only illumination cache.

## Exactly two visual review rounds

Each round contains the same valley, alpine lake and ground-level views, all
2560×1440, seed 42, quality 100, native radius 32 m, 1024 MiB terrain cache,
8192-tile ceiling, haze 0.7 and no crosshair. Saved under ignored
`screenshots/terrain_review/round_1/` and `round_2/`, alongside actual run logs.
The earlier `screenshots/max_graphics_*.png` remain available as before images.

Round 1 confirmed the dark lake lines disappeared, distant contour bands were
strongly reduced and material boundaries became less abrupt. Ground-level fine
LODs still emphasized risers. The single adjustment before round 2 extends
filtered lighting to fine terrain too, moves its blending range earlier and
retains near AO in the same byte. The ground capture also settles the actual
player and aligns its existing look/body controller before rendering.
No third visual-tuning round is authorized by this request.

Reproduce the final views with the current release binary:

```sh
./MicroVoxel --headless --seed 42 --camera 4250,200,6250,125,-5.14 --size 2560x1440 --quality 100 --native-radius 32 --terrain-memory 1024 --tiles 8192 --no-hud --frames 5 --screenshot output/valley.png
./MicroVoxel --headless --seed 42 --camera 1250,567.81,8750,22.5,-6 --size 2560x1440 --quality 100 --native-radius 32 --terrain-memory 1024 --tiles 8192 --no-hud --frames 5 --screenshot output/alpine_lake.png
./MicroVoxel --headless --seed 42 --camera 4250,200,6250,125,-2 --camera-ground --view rear --size 2560x1440 --quality 100 --native-radius 32 --terrain-memory 1024 --tiles 8192 --no-hud --frames 5 --screenshot output/ground.png
```

`--camera-ground` ignores the supplied Y, places the existing player at X/Z,
settles it for 60 fixed ticks, aligns its look and uses the selected F5 view.

## Validation and references

The full core suite passed (67 tests). After the second adjustment, the five
terrain regression tests and ten integration rendering tests passed again.
Native CPU/shader validation: eight passed; both opt-in GPU tests passed on the
AMD Radeon 760M, including a new native/coarse lake boundary readback test.
Coverage tests check normalization and continuity across biome thresholds;
geometry tests cover native lake height at five LOD levels, cliff/water joins,
exact native RGB/edits and unchanged canonical occupancy. The dependency-free
system Rust 1.75 build also passed.

References consulted before the rendering changes:

- [PBRT sampling theory](https://www.pbr-book.org/4ed/Sampling_and_Reconstruction/Sampling_Theory):
  filter unresolved signals rather than expecting a later image blur to remove
  aliasing already introduced by sampling.
- [GPU Gems 2 atmospheric scattering](https://developer.nvidia.com/gpugems/gpugems2/part-ii-shading-lighting-and-shadows/chapter-16-accurate-atmospheric-scattering):
  distance-dependent attenuation/in-scattering as outdoor depth cues. Our haze
  remains a cheap static approximation, not that chapter's full model.
- [0 FPS voxel ambient occlusion](https://0fps.net/2013/07/03/ambient-occlusion-for-minecraft-like-worlds/):
  keep occupancy-based local shading separate from canonical colors and meshing.
- [Red Blob Games terrain from noise](https://www.redblobgames.com/maps/terrain-from-noise/):
  moisture/elevation-conditioned terrain appearance. Coverage integration here is
  an original extension of this project's existing climate fields.

## Final review and measurements

Round 2 was inspected at all three viewpoints. The lake's dark angular ledges
remain absent, and the elevated valley's fine repetitive contour bands are now
strongly suppressed as well. The ground view retains clearly visible nearby
voxel risers, with progressive filtering further away. Some shoreline stair
steps, thin bright channel banks and small material patches remain; no further
visual adjustments were made after this second inspection.

AMD Radeon 760M, Vulkan, 2560×1440, five synchronized offscreen frames per view:

| Final view | Cover preparation | Cached terrain | Effective target | Mean draw time |
| --- | ---: | ---: | ---: | ---: |
| Valley | 24.11 s | 224.81 MiB | 3.05 px | 45.29 ms |
| Alpine lake | 23.13 s | 447.84 MiB | 3.81 px | 37.12 ms |
| Ground/player | 17.33 s | 165.47 MiB | 3.81 px | 33.39 ms |

These are maximum-control screenshots, not one-native-voxel-per-pixel renders
throughout the world. Tile limits still raise the requested 1 px target. Short
offscreen timings include synchronization and are not interactive FPS guarantees.
The changed FOV selects a different tile distribution from the original 75-degree
captures, so those are not controlled before/after performance comparisons.
