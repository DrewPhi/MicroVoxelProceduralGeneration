# Initial native renderer measurements

Measured in a release build on this machine, using **AMD Radeon 760M integrated
graphics**, Vulkan / RADV PHOENIX. These are observations of one fixed scene, not
minimum hardware requirements or a guarantee of gameplay performance.

Command from the repository root:

```sh
./native/target/release/microvoxel-game --headless --fly-test --frames 60 --screenshot native_preview.png
```

Configuration: seed 42, 10 km region, 6.25 cm native cells, 1280×720, 75° vertical
FOV, default target of 384 LOD tiles, four mesh workers. The scripted flight uses
canonical collision queries and installs replacement LOD meshes before measurement.

| Measurement | Observed |
|---|---:|
| Terrain setup | 2.89 s |
| First detailed stitched cover after setup | 0.55 s |
| Warm synchronized offscreen frame time, 60 frames | 1.57 ms/frame |
| Tiles in final cover / visible | 333 / 105 |
| GPU mesh cache, including retained earlier meshes | 14.87 MiB |
| Active exposed cell faces / merged rectangles | 760,202 / 351,999 |

Each frame includes camera-relative uniforms, canonical picking, frustum culling,
drawing and a GPU completion wait. It excludes window presentation/vsync,
continuous movement and future simulation. Background loading is finished before
the timed frames. The desktop application has a frame-rate display in its title
bar; test moving and rotating there to assess interactive behavior.

Equal-footprint CPU comparison (`--benchmark`), over a 16 m × 16 m patch:

| Render cell edge | Exposed faces | Rectangles | Mesh/atlas payload | Generation + merging |
|---|---:|---:|---:|---:|
| 6.25 cm, exact | 136,539 | 68,906 | 2.62 MiB | 156.46 ms |
| 12.5 cm | 33,709 | 17,154 | 0.65 MiB | 92.71 ms |
| 25 cm | 8,347 | 4,307 | 0.16 MiB | 24.98 ms |
| 50 cm | 2,051 | 1,065 | 0.04 MiB | 3.24 ms |

The per-run profile cache starts empty, but the shared generator cache can be
warm between rows. Coarse profiles require more samples per cell than exact
columns, so timings need not scale directly with face counts.

Flat geometry is a different case: tests merge a 32×32-cell flat surface into
**one rectangle**, retaining all 1,024 independent voxel colors. Its payload is
4,128 bytes: 32 bytes of geometry and 4,096 bytes of colors. Rough terrain cannot
merge nearly as much, which is why distance LOD still makes a useful difference.

Current limits: LOD uses bounded sampled terrain summaries, not exact aggregation
of the entire world. Grid alignment and explicit boundary faces close seams;
changes in approximation can still pop. Occlusion hierarchies, GPU-driven indirect
submission and simulation performance are future work.
