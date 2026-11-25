# Early FBM renderer checkpoint

Historical fixed-center baseline at `9550ed1`. See
[STREAMING_CHECKPOINT.md](STREAMING_CHECKPOINT.md) for the subsequent moving-level
implementation and current performance/limitations.

User priority: move a lightweight procedural source forward so rendering is
measured on varied terrain now. This is partial work on FV05/FV09/FV12/FV02
(#29/#33/#36/#26), not completion of their production acceptance criteria.

## Implemented

- Seeded CPU-f64 gradient noise, low-frequency warp and mixed ridged FBM, with
  validated scale/amplitude/octaves/roughness. Source coordinates remain meters;
  canonical column tops sample native centers and quantize to 6.25 cm.
- `World::fbm` uses the same source as render pages, with authoritative material
  queries and edit semantics. Existing cave/bridge fixture remains independent.
- Default surface prototype: 4.096x4.096 km fixed-center nested grids, seven levels,
  0.5–32 m spacing, immutable geometry, CPU-sampled GPU height/normal/color pages.
- Boundary vertices collapse onto parent anchors; height, normal and canonical
  footprint-averaged material color morph toward the same parent interpolation.
- `fbm-native`: exact 8x8 m native surface cutaway from the same field; cached
  column samples avoid reevaluating FBM for every voxel. Explicit allocation and
  relief limits reject excessive requests without changing terrain.
- Deterministic camera routes, frame warmup exclusion, uncapped option and JSON
  source/representation/camera metadata. Reproducible smoke sweep script.

Greenfield (G); no legacy source imported. Sol medium implemented source and
renderer and independently reviewed topology; Luna high implemented route/report
helpers and smoke script; root reviewed, integrated and tested on actual hardware.
Primary algorithm references are in world/README.md and ARCHITECTURE.md.

## Verification and artifacts

40 workspace tests pass, including native mesh/source parity, seed/parameter and
negative coordinate tests, WGSL validation, full-perimeter height/normal/color
agreement, winding, covered area and edge multiplicity. The edge check found and
prevented a corner T-junction which total area alone did not catch. Release build,
rustfmt check and git diff whitespace checks pass.

Hardware: Ryzen 5 7640U / Radeon 760M (RADV PHOENIX), Vulkan,
Mesa25.2.8-0ubuntu0.24.04.2. Source version1, seed7, scale1800m, amplitude1000m,
8octaves, roughness0.5, 1920x1080 output/internal rendering, single sample,
no TAA, no haze. Surface:116487 vertices,180224 triangles,9318960 bytes of source
pages (not total GPU memory). One immutable scene generated before interaction.

Short isolated stop-resume run after compilation ended:360 frames, first30
excluded,329 measured intervals; simulated route advances1/60 second per frame
and remains at least10m above source. Uncapped/presentation-inclusive frame times:

| Metric | ms |
|---|---:|
| p50 | 1.373726 |
| p95 | 2.866185 |
| p99 | 3.487152 |

Zero measured intervals exceeded16.6667ms. This is a short synthetic fixed-scene
workload, not sustained thermal, real-time travel/streaming, or isolated GPU timing.
GPU timestamps, total peak VRAM/RSS, page-generation latency and silhouette error
remain unmeasured. Do not extrapolate these times to the final game.

Inspected local ignored captures under `output/fbm-checkpoint/`:
- `fbm-seed7-valley.png` (1920x1080)
- `fbm-seed7-aerial.png` (1920x1080)
- `fbm-seed42-aerial.png` (1920x1080)
- `fixture-seed7.png` (1920x1080)
- `fbm-native-seed7.png` (1280x720)
- `stop-resume-isolated.json` and matching log contain the reported hardware run.

Native FBM crop:36428 exposed faces,36154 triangles,3396900 sampled halo cells;
y=[84.938,97.375)m for seed7. Cutaway boundaries intentionally have no fake caps.

## Visual assessment and next gates

Both seeds produce coherent varied ridges/valleys suitable for renderer stress.
This is deliberately simple FBM, not geological realism: shape and material
variety still need later work. Captures show noticeable coarse faceting, shading
variation with spacing, and smooth nearby proxy geometry. The exact crop retains
voxel steps and shows why matching the two paths matters. No claim of invisible
LOD transitions or voxel-faithful broad rendering is made.

The rings do NOT recenter or stream. Their center stays at origin and cameras can
leave the finite area. There is no screen-space error selector or certified error
bound, no exact-near handoff, no cave ownership in this heightfield scene, and no
shadow/AA/weather stack. Both FBM comparison scenes disable editing; fixture edits
still work. The displayed smooth proxy must not become the final visual contract.

Next: moving bounded height-page coverage with measured update latency and
motion-safe publication; exact near voxel/heightfield ownership and transition
smoke tests against the canonical reference; then matched CDLOD/clipmap and
volume-renderer decisions. Keep the new FBM seeds in those tests. G0–G4 and the
full laptop quality/performance acceptance remain open.
