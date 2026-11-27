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

## Integrated Frontier experiment

Launch the experimental full engine from the `frontier/` directory.

```sh
cargo build --release --locked --bin frontier
target/release/frontier --native-lattice --seed random --weather regional \
  --flight-walkup --width 1280 --height 720
```

The experimental launch uses Eroded climate and drainage to generate one
world-aligned, 8 m control lattice. Signed interpolation at cell centers,
whole-cell floors and the two deterministic relief bands define the new
canonical **6.25 cm** terrain. The 8 m interval is part of world generation
and does not vary with viewpoint. The render hierarchy contains only maximum
height bounds for the same columns. It does not contain substitute surfaces.
Legacy Eroded terrain and the experimental source have distinct cache
identities and can differ in elevations, banks and narrow channels.

Gameplay, aircraft contact, vegetation placement, picking, water height and
voxel edits use this source. The terrain fragment writes to the shared
Frontier depth buffer. NAADF retains priority at coincident faces in its
nearby volume and handles volumetric geometry. Edited columns can include
multiple material spans. The distant tracer publishes those sparse spans
with their raised conservative bounds, including cuts and unsupported
placed blocks outside NAADF. Water uses the existing absorption, Fresnel
reflection and sky colors. Terrain shadows still use a sampled lighting
field. Vegetation keeps its existing distance-based rendering choices.

A 2048² patch window covers the 6 km radial horizon. A replacement reuses
the overlapping control samples and gets published with its own matching
views and edit data. Cold cover and a teleport beyond resident coverage
can require synchronous generation. Player edits live in the current
world session, like those in the standard rolling launch. Live water
simulation and other voxel sizes are outside this prototype.

### Reproduce

```sh
mkdir -p output/native-lattice
target/release/frontier --preset eroded --native-lattice \
  --seed 1685576058315138427 --weather regional --day-night on \
  --width 1920 --height 1080 --frames 180 --warmup 30 --uncapped \
  --walk off --eye -6688,1450,6080 --target -5350,900,8360 \
  --route bank-return --route-clock frame --route-speed 80 \
  --route-step 0.0333333 --report output/native-lattice/bank.json \
  --capture-on-exit output/native-lattice/bank.png
```

Remove `--native-lattice` for the current rolling renderer. Change both
dimensions to 1280×720 for the native 720p run. Use
`FRONTIER_NATIVE_LATTICE_COMPUTE=1` to measure the separate full-resolution
visibility prepass and `FRONTIER_NATIVE_LATTICE_BEAMS=1` for conservative
8×8 beam prefixes. Neither flag changes the terrain generator. The full
GPU timestamp includes the visibility prepass. The opaque render-pass
timer starts later. The rolling baseline uses its original Eroded world,
so this is a matched route and composition comparison. Depth correctness
uses the independent native DDA against the new world source.

```sh
cargo test --release --locked -p frontier-world native_lattice
cargo test --release --locked -p frontier-renderer --lib \
  production_depth_matches_native_world_and_sparse_edits -- --ignored --nocapture
```

The GPU audit compares captured production depths with the CPU native
voxel DDA. It checks a banked view, a cut through the terrain, and raised
blocks beyond the local exact column window.

### Integrated measurements

Ryzen 5 7640U, Radeon 760M, RADV Vulkan, Mesa 25.2.8. The
6 km bank-return run measures 149 presented frame intervals after 30
warm-up frames. Atmosphere, a regional weather simulation, vegetation,
shadows and nearby NAADF were active. Full GPU times include compute
visibility where selected. The banked route moves the viewer camera.
The separate aircraft smoke test uses keyboard input.

| Mode | Resolution | Full GPU p50 / p95 / p99 | Presented p50 / p95 / p99 |
|---|---:|---:|---:|
| Current rolling | 1280×720 | 8.1 / 9.3 / 9.6 ms | 9.3 / 11.9 / 18.6 ms |
| Native lattice, separate visibility | 1280×720 | 19.5 / 21.9 / 22.4 ms | 20.9 / 23.8 / 25.3 ms |
| Current rolling | 1920×1080 | 13.2 / 14.5 / 14.8 ms | 14.7 / 16.8 / 18.8 ms |
| Native lattice, separate visibility | 1920×1080 | 37.4 / 45.7 / 46.9 ms | 39.3 / 48.9 / 50.0 ms |
| Native lattice, visibility and shared beam prefixes | 1920×1080 | 37.3 / 41.0 / 41.4 ms | 38.5 / 43.9 / 45.1 ms |

The 840-frame 720p route at 120 m/s took 25.9 / 32.0 / 34.8 ms
(p50 / p95 / p99) per presented frame with separate visibility. The
resident cache published six windows, generating 262,272 new
controls for a typical 1,024 m strip. Its active terrain resources
were about 99 MiB, including the draw cache, lighting data and 16 MiB
of compute visibility output. The rolling baseline used about 39 MiB
in this configuration. A tighter conservative bound on each one-metre
relief region (`FRONTIER_NATIVE_LATTICE_RELIEF_BOUNDS=1`) did not beat
the beam and compute combination.

The viewport audit exposed loss of fractional voxel position when an
8192 m window-relative camera position was stored as one f32. Splitting
it into integer cell and fractional offset now keeps the initial ray
phase. An independent DDA comparison of the compute kernel returned
966 matching sampled hits across unedited terrain, a cut and unsupported
placed blocks. The production fragment and compute-plus-beam captures
also matched the independent DDA in all three cases. The largest measured
depth error in that compute audit was 0.042 mm. A river view matched 239
hits, including 55 water surfaces, with a maximum depth error of 0.40 mm.
The flight
smoke test took off from the runway using keyboard input. A subsequent
touchdown and impact created four sparse edited patches in the active
terrain view.

The full scene is playable at 720p. Keep the current renderer as the
default while 1080p exact visibility takes more than the 33.3 ms frame
budget and the active terrain memory remains substantially higher.

## Native detail at every terrain scale

The previous experiment used the 8 m Eroded control lattice plus two
small relief bands. Near a cliff it could show exact voxel steps on a
surface whose larger ledges and crevices were missing. `--native-detail`
selects a new canonical source. The original Eroded generator still
supplies the broad landforms, climate, and drainage. A deterministic
world-space relief field supplies structure between the control points.
It subdivides the 8 m regions down to the native 6.25 cm voxel spacing.
Native column heights include every subdivision. Physics, ray picking,
vegetation placement, and sparse edits use those same heights.

The source chooses from 256 relief tiles using hashes on shared world
edges. Neighbors agree on the edge curve. The interior uses recursive
midpoint displacement and native-cell grain. The finite set of tile
shapes is a design choice and can repeat across a world. The carrier
and relief together define a new canonical world with different heights. A small bed constraint preserves the base
channel where its water surface cuts the carrier.

The renderer stores exact residual samples and conservative maxima in
an 11 MiB signed-data atlas uploaded as an R16Uint texture array.
It checks the ray against a quadratic carrier and the maximum residual
of each candidate region. The hierarchy rejects empty intervals and
descends to integer columns for the visible first hits. The use of edge-matched procedural tiles and
a conservative height hierarchy is informed by [Wang Tiles for Image
and Texture Generation](https://doi.org/10.1145/882262.882265) and
[Maximum Mipmaps](https://pure.mpg.de/rest/items/item_1325622_5/component/file_3590464/content).

Lighting estimates the pixel's world-space footprint from the camera
projection and local slope. It interpolates relief gradients across
sample scales so resolved geometry retains voxel-face lighting and
subpixel roughness blends continuously. At 1× the renderer traces the
pixel center. At `--msaa 4` it traces each raster sample at its actual
subpixel location with the same full native geometry. Four samples
provide a finer estimate of the pixel's area coverage.
The performance runs below use 1× with native display-sized rendering.
Vegetation retains its existing selection and filtering.

### Run and verify

```sh
cargo build --release --locked --bin frontier
target/release/frontier --native-detail --seed random --weather regional \
  --flight-walkup --width 1280 --height 720 --msaa 1
cargo test --release --locked -p frontier-world --lib native_lattice
cargo test --release --locked -p frontier-renderer --lib \
  production_depth_matches_native -- --ignored --nocapture --test-threads 1
```

Use the preceding bank-return benchmark command with `--native-detail`
and `--msaa 1`. A streaming run uses `--route fly --route-speed 120
--frames 840`. The geometry audit compares production output with CPU
voxel DDA in the cut and placed-block fixture, a banked 6 km view and
a stretch of river. The audit checks first hits against the canonical
source for the selected mode. It matched 1,358 sampled hits for the
new source, including 60 water hits. Maximum measured depth error was
1.33 mm, in the 6 km fixture. A runway spawn on seed 7 also completed
with vegetation, weather, and the nearby NAADF volume enabled.

To compare traversal on identical native detail, set
`FRONTIER_NATIVE_DETAIL_SPLIT=0` to visit water and edits in the primary
pass, `FRONTIER_NATIVE_DETAIL_STEP=0` to use flat native DDA inside a
candidate tile, or `FRONTIER_NATIVE_DETAIL_GROUP=16` to use 16×4 workgroups.
These switches change the search or dispatch only. The default runs an
8×8 primary pass for dry terrain, then resolves the pixels whose rays
reached a potentially wet or edited patch. Every unresolved pixel is
handed off to the general exact search. The first pass provides a
conservative starting distance for that search.

### Radeon 760M measurements

All runs use the Ryzen 5 7640U and the AMD Radeon 760M via RADV
Vulkan, Mesa 25.2.8. The bank uses seed 1685576058315138427 and a
6 km view. Presented p50 / p95 / p99 include CPU and presentation.
GPU p95 spans shadows, visibility, opaque rendering, atmosphere and UI.

| Native detail trial | Resolution | Presented samples | Presented p50 / p95 / p99 | GPU p95 |
|---|---:|---:|---:|---:|
| Texture atlas, single pass | 1920×1080 | 149 | 49.9 / 55.3 / 56.1 ms | 52.8 ms |
| Split exact pass, bank | 1280×720 | 149 | 22.8 / 25.0 / 26.5 ms | 22.9 ms |
| Final split exact pass, bank | 1920×1080 | 149 | 44.2 / 48.9 / 50.7 ms | 46.3 ms |
| Final split exact pass, 120 m/s flight | 1280×720 | 809 | 25.3 / 30.0 / 33.6 ms | 27.4 ms |

The final flight published six cache windows. The active reported terrain
memory, including lighting and the compute output, was about 111 MiB.
A flat native DDA reached 113 ms p95 at 1080p. Stepping over more
hierarchy levels per node and grouping rays into wider workgroups
also slowed this route. Keeping the 8×8 group and splitting the dry
search from wet and edited tiles gave the best measured full-scene
result. Native 720p meets the 30 FPS target at p95 on these routes.
Native 1080p exceeds its 33.3 ms budget, so the mode remains experimental.
