# Native voxel terrain with a bounded procedural source

`polynomial_ground` is a standalone terrain renderer and free-flight preview.
It uses the Eroded world to generate fixed, world-aligned control heights,
including its larger watershed and erosion features. The default generator
uses an 8 m control grid. The 4 m setting defines a second world with
finer features. Neither spacing changes when the camera moves.

Every ray can see the native 6.25 cm top and side faces up to the full
6 km radial horizon. The renderer uses one ray per output pixel. All geometry
comes from a single deterministic column function. The only hierarchy
contains upper bounds used to reject empty ray intervals.

## Source and traversal

For a native column, multiply the four integer control heights by
the bilinear weights at the column centre and round down exactly. Two
integer procedural relief bands add metre and quarter-metre features.
Their combined height is between −4 and +3 voxels. The GPU uses signed
fixed-point interpolation. The CPU reference uses a separate signed
64-bit numerator and walks every native column intersected by a ray.

Along a straight ray through one control tile, the continuous part of
the height function is quadratic. A conservative upper envelope expands
it by half a column of horizontal slope allowance in each axis, the
maximum positive relief, and a numerical guard. Solve that envelope for
the earliest potentially occupied interval. From there, the 2D DDA
tests the real integer columns and returns a native intersection.
Small relief bounds allow skipping the hash for certain empty cells
and for side hits whose entry is already below the minimum height.

The hierarchy stores a maximum of each tile's corner heights plus relief
allowance. The screen-beam pass checks four conservative maxima over
the XZ envelope of each 8×8 pixel tile. A shared empty prefix advances
the independent rays. It changes no heights and returns no interpolated
hits. Traversal starts in newly entered sibling cells and tests two
hierarchy levels per transition by default.

Related research includes [maximum mipmaps and bilinear ray intersection](https://pure.mpg.de/rest/items/item_1325622_5/component/file_3590464/content),
[boundary-aware heightfield traversal](https://www.cs.cit.tum.de/fileadmin/w00cfj/cg/Research/Publications/2009/GPU_Ray-Casting/EG09AreasTerrain.pdf),
and [2D DDA through exact column geometry](https://diglib.eg.org/server/api/core/bitstreams/37905fb9-2e91-45a2-9238-152be9d9fecc/content).
The implementation is independent. Its fixed generator and integer leaf
test retain the same voxel geometry at every visible distance.

## Loading and scope

At launch, a 16.384 km square provides coverage in all directions.
An asynchronous worker reuses overlapping world-aligned controls, builds
new ones and recomputes the bounds. It prepares a complete GPU snapshot;
the preview publishes the new bindings only when the replacement covers
the full viewing sphere. During the move there can be two CPU and GPU
snapshots. The steady GPU lattice takes 53.36 MiB at 8 m and 213.40 MiB
at 4 m, excluding resolution-dependent outputs.

The preview shades native faces at close range and uses continuous
interpolation of slope-lit control colours for distant shading. Terrain
height and occupancy are independent of that lighting filter. The
preview demonstrates opaque ground and static lighting. Frontier's
edit ownership, caves, volumetric geometry, vegetation, water, weather
and aircraft simulation are outside this standalone test. The Eroded
control generator carries over broader drainage landforms, while small
channel and bedrock geometry differs from the original source.

## Original Eroded source experiment

`native_column_gpu` evaluates the original Eroded ground height in WGSL
using FP64 and 64-bit integer hash operations. The CPU exports its primary
reach and secondary gully candidate bins. On the Radeon 760M, 921,600
full-height queries took 151.90 ms GPU p50 and 153.48 ms p95. The
descriptor snapshot took 93.81 ms to prepare and occupied 1,545,424 bytes.
The native height agreed with the CPU for every query in that batch.
The shader evaluates heights and performs no ray intersection in this
test. The GPU's Vulkan adapter reports native FP64, int64 and ray queries.

With the original exact streaming renderer, the 720p open view completed
in 159.55 s with no pending pixels. After warm-up, stationary visibility
alone took about 48 ms. The fixed-generator experiment exercises a
different source with the same seed, field of view, starting pose and
6 km radius.

## Reproduce

```sh
cargo build --release --locked -p frontier-renderer --bin polynomial_ground --bin native_column_gpu
target/release/native_column_gpu

target/release/polynomial_ground --interactive --width 1920 --height 1080
target/release/polynomial_ground --width 1920 --height 1080 --frames 840 \
  --roll-every 512 --output output/native-flight-1080
target/release/polynomial_ground --width 1280 --height 720 --frames 840 \
  --roll-every 512 --output output/native-flight-720
```

WASD and Space/Ctrl move the preview camera, the arrows look, Shift
accelerates, Home returns to the initial pose, F12 captures a PNG and
Esc closes the preview. Set `--seed INTEGER` to explore another world.
Use `--present-benchmark` for the scripted route in a real window. The
headless benchmark includes visibility and shading and writes frame times,
failed-ray counts, the independent audit, and first/last depths
and images. The simulation follows an 80 m/s route with banking and a
26° heading change at a 30 Hz simulation step. The sample clock is
independent of measured frame time.

Use `--walk-leaves` to run a native DDA for each intersected candidate tile
without the quadratic skip. `--no-beams`, `--full-relief-eval`,
`--ascend-parent`, `--level-step` and `--group-width` reproduce the
individual traversal ablations. `--control-spacing 4` selects the
denser generator.

## Radeon 760M results

Ryzen 5 7640U, Radeon 760M, RADV Vulkan and Mesa 25.2.8. The long route
starts 1,100 m above the terrain and traverses about 2.24 km, with an
80 m/s ground speed, 26° of yaw change and an intervening bank. The
two headless runs traced and shaded each pixel of 840 frames. Timing
distributions use the 832 frames after warm-up.

| Long route, 8 m source | GPU p50 / p95 / p99 | Frame wall p50 / p95 / p99 | First complete view |
| --- | ---: | ---: | ---: |
| 1920×1080 | 24.88 / 29.47 / 29.59 ms | 25.90 / 30.71 / 31.18 ms | 1.52 s |
| 1280×720 | 12.13 / 14.49 / 14.61 ms | 13.03 / 15.77 / 16.18 ms | 1.34 s |

Both routes completed three asynchronous window updates. The viewport
hit ground approximately 6,000 m away, with zero failed rays. The 128
independent native-column checks on each route agreed on coverage.
Maximum differences were 0.92 mm at 1080p and 0.77 mm at 720p.
A 640×360 full native walk across candidate tiles produced bit-identical
depths and colours to the analytic skip across the full frame.

The hardware-backed desktop runs also presented all 840 frames at each
native output resolution, including all three coverage updates. Average
FPS is based on the total wall time of the 832 frames after warm-up.

| Presented route, 8 m source | Average FPS | Frame wall p50 / p95 / p99 | Frames within 33.33 ms |
| --- | ---: | ---: | ---: |
| 1920×1080 | 35.15 | 28.18 / 33.55 / 34.17 ms | 779 / 832 |
| 1280×720 | 69.34 | 14.21 / 17.49 / 18.96 ms | 832 / 832 |

The alternate seed-7 headless route completed at 720p with 15.29 ms wall
p95, no failed rays and visible terrain 5,999.97 m away. The 4 m source's
720p route ran in 17.96 ms wall p95 through one coverage update.

## Generator comparison

At the matched initial 720p pose, the 8 m generator and original Eroded
source disagreed on terrain coverage at 720 of 921,600 pixels. The p95
skyline displacement was 2 px. At 4 m that fell to 380 pixels and 1 px.
For terrain shared by both images, the p95 depth difference fell
from 2.58 m to 1.46 m. The 4 m lattice retains finer features at a cost
of four times the residency and roughly four seconds of initial build.
The 8 m generator is the default for this preview.
