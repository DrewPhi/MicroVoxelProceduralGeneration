# Basic lighting and painting pass

Two Sol6.1 medium agents implemented lighting and source painting; root integrated
coordinate-based color, reviewed captures, toned down the initial camouflage-like
patch coverage, and checked performance. This is a basic shadowless beauty pass,
not weather, physically complete illumination, or a solution to LOD transitions.

## Changes

- Shared warm directional sunlight plus cool hemisphere ambient and muted ground
  bounce. Mesh, brick rays, surface probe, native terrain, collar and proxy use the
  same RGB lighting function (shader equality regression). Voxel face normals stay
  sharp. No extra shadow rays or screen-space pass.
- Less saturated grass, earth, stone and wood palette. The palette update also
  affects legacy fixtures, while legacy material IDs and terrain geometry stay
  unchanged. Regional painting uses seeded220m exposure/altitude and55m soil
  fields to select coherent patches from existing grass/soil/stone materials.
  Painting costs two noise evaluations at surface queries, no slope stencil.
- Hybrid/clipmap terrain receives gentle4m/32m color variation computed on theGPU.
  Native cell addresses derive from integer chunk origins plus an inward-biased
  fragment position, so all axis faces identify the same solid voxel. Nearby
  grain is only ±2.75% and fades between12–40m; no stored per-voxel color, added
  triangles, texture images or black borders. Coherent color is evaluated at
  voxel centers, not at arbitrary subvoxel texture coordinates. Distant smooth
  proxies are still filtered approximations, not exact voxel color/silhouettes.
- The cave/bridge owner receives the same tint, avoiding a new rectangular color
  discontinuity. The generic fixture remains untinted to preserve mesh/ray
  comparisons. The experimental quadtree receives the shared lighting/palette
  but not the new clipmap tint; its appearance comparisons must account for this.
- Appearance source version1 (regional) and renderer appearance version1 recorded
  separately from unchanged geometric source versions1/2. Canonical occupancy
  stays fixed; regional surface material IDs can change. No save migration exists.

## Validation and tradeoffs

91 workspace tests pass, one pre-existing expensive test ignored; final volume
shader change also passes its three focused validation tests. Tests include
canonical material/column/color agreement, deterministic coherent regions,
negative column coordinates, legacy bit fixtures, shared shader lighting and WGSL
validation. Root reviewed origin arithmetic and an agent independently audited
uniform layouts/voxel identity. Rust formatting and release build pass.

Captures inspected: same1920×1080 ground, aerial, mountain-region and cave/bridge
views before/after; first palette rejected as too patchy, then revised. Camera,
seed7, geometry parameters and4×MSAA/filter-on settings match. Generated files
are ignored under `output/beauty-pass/`: `before-*.png`, first `after-*.png`, final
`final-*.png`, plus logs/timing reports and a frozen before binary.

Radeon760M/Ryzen5 7640U, RADV PHOENIX Vulkan/Mesa25.2.8; hybrid seed7,
1080p,4×MSAA,filter on,3m/s real-time stop/resume12s,60warmup,uncapped:

| | Frame p50/p95/p99 ms | GPU render-pass p95 ms | >16.67ms | Native GPU payload |
|---|---:|---:|---:|---:|
| Before | 4.04 / 5.55 / 5.80 | 3.49 | 0/2754 | 71.43MB |
| Final | 4.34 / 5.86 / 6.18 | 3.69 | 0/2565 | 71.45MB |

Small extra shading cost in this short route, not a universal performance bound.
GPU timestamps are sampled and excludeCPU/upload/presentation. Source material
boundaries may split some greedy quads; GPU grain itself never splits meshes.
One additional16-byte uniform origin per native draw and terrain uniform is used;
this does not allocate per-cell color storage.

Limitations: no cast shadows/AO, so interiors receive ambient light even where
physically occluded. Existing seam/ownership and coarse color interpolation limits
remain. Near the collar, filtering intentionally varies grain strength across the
handoff; the tiny bias used to identify a solid cell can shift its coarse tint
sample by one cell relative to the proxy. Full material-transition/mipmap accuracy
is future work. Do not describe the pattern fields as geology or biomes.

## Try

```sh
./run.sh --scene hybrid
./run.sh --scene fbm --camera aerial --start-xz -1156,3469
./run.sh --scene hybrid-topology --camera topology
./run.sh --scene hybrid --width 1920 --height 1080 --capture output/beauty.png
```

Defaults remain regional terrain, voxel filtering on and4×MSAA. No new lighting
menu is introduced. This work is independent Frontier code, no copied external
shader/texture assets or legacy runtime changes.
