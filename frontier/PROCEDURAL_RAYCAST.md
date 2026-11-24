# Meshless Alpine ray casting experiment

Branch `experiment/procedural-raycast` starts at Frontier `b59dedb`. The
standalone Rust/wgpu binary renders the canonical continuous Alpine height
function directly. The renderer builds no terrain vertices, triangles, or
geometric LODs. The packed hybrid remains the game renderer. The experiment
covers the untouched Alpine heightfield at the default terrain parameters.
The native 6.25 cm voxel source, sparse edits, and NAADF stay in Frontier.

The [coherence follow-up](PROCEDURAL_COHERENCE.md) measures ray packets, sparse
prediction and GPU work compaction against this control.

## Captures and benchmark reports

![The packed hybrid and meshless renderer at the same Alpine horizon](reference/procedural-raycast/horizon-comparison.png)

[Packed horizon at 1080p](reference/procedural-raycast/packed-1080.png) ·
[Meshless horizon at 1080p](reference/procedural-raycast/horizon-1080.png) ·
[Alpine ridgeline screenshot](reference/procedural-raycast/alpine-oblique.png) ·
[Alpine flyby MP4](reference/procedural-raycast/alpine-flyby.mp4) ·
[Raw JSON reports](reference/procedural-raycast/reports)

## Intersection and shading

- Build an 80 m cell grid of upper height bounds on the GPU. Two 4×4 max
  reductions give 320 m and 1,280 m skip regions. Only upper bounds are stored.
- Walk the intersected grid intervals front to back. Where the ray could enter
  terrain, bound ray height minus terrain height with a center, a signed linear
  term along the ray, and a remainder. The signed term cancels shared slopes
  near a grazing incidence. Gradient noise uses its analytic derivative and a
  conservative Hessian bound. Alpine hills and envelopes have separate analytic
  bounds. A bounded residual lets the solver reject an interval before it pays
  for all its octaves. Endpoint signs at small intervals bound the reported
  first surface to the configured screen-space tolerance, subject to numerical
  padding and the finite work budget. Ambiguous results remain visibly flagged.
- The optional gradient atlas stores exact seeded lattice codes for nine bands.
  Outside the atlas, and in other bands, the shader computes the original hash.
  The same function supplies intersections with or without the atlas.
- The optional second pass only revisits ambiguous pixels. Strict repair uses
  smaller certified intervals. Numeric repair also allows a same-sign interval
  at most 5 cm along the ray to pass after isolation, and gives each difficult
  interval a larger budget. This can miss a crossing inside that interval.
  The report counts pixels resolved by the numeric pass. The screenshots use
  numeric repair. Unresolved pixels are shown in magenta and counted separately.
- Shading estimates the projected footprint from depth and grazing angle, and
  attenuates detail normal bands continuously when they become subpixel. Primary
  intersection always targets the full function. Macro normals use local finite
  differences. Geometry mode uses constant color for hit pixels. The prototype
  runs one primary ray per pixel and selective repair, with no temporal history
  accumulation or silhouette supersampling.

Selective port from `experiment/voxel-box-splats@a341e2d` plus the prior
working-tree experiment at `frontier/renderer/src/procedural_terrain.wgsl`.
`alpine_source.wgsl` carries the split-u32 hash, gradient table, scalar and dual
noise, and default Alpine formula. The continuous interval solver, skip grid,
runner and CPU oracle are new. The CPU oracle calls `FbmTerrain` and the native
voxel raycast independently of the shader.

## Laptop results

Ryzen 5 7640U, Radeon 760M, RADV Mesa 25.2.8. Alpine seed 7, eye
`(-43.625, 75, -289.5625)`, looking toward `(0, 490, 3300)`, vertical FOV 60°,
0.025 m near and 6,000 m forward clip. The packed renderer uses MSAA1, packed
quads, and a fixed 512-voxel (32 m) NAADF radius. Vegetation, shadows,
atmosphere and fog are off. Both use comparable camera and terrain settings.
Colors, material filtering and sky treatments differ. The flat diagnostic uses
no terrain shading, so its timing is a lower-cost geometry comparison against
the regular full packed pass.

| View and renderer | Samples | GPU p50 / p95 / p99, ms | Frame wall p50 / p95 / p99, ms |
| --- | ---: | ---: | ---: |
| 320×180 packed hybrid | 277 | 5.45 / 5.66 / 5.87 | 16.67 / 17.87 / 18.52 |
| 320×180 meshless geometry with numeric repair | 14 | 28.27 / 29.40 / 29.42 | 29.62 / 30.36 / 30.39 |
| 1920×1080 packed hybrid | 397 | 11.42 / 11.71 / 12.28 | 16.66 / 18.31 / 19.34 |
| 1920×1080 meshless geometry with numeric repair | 12 | 540.59 / 542.16 / 543.02 | 545.40 / 546.62 / 547.75 |
| 1920×1080 meshless with filtered shading and numeric repair | 12 | 546.27 / 551.81 / 552.48 | 551.30 / 556.68 / 556.74 |
| 480×270 meshless turn and 190 m approach, filtered | 36 | 71.78 / 80.46 / 84.72 | 73.13 / 81.53 / 86.58 |

GPU time sums timestamp ranges over compute dispatch tiles. Wall time includes
submission and synchronizing timing results, with PNG encoding performed after
its sample. Packed wall samples include presentation with vsync. The short
meshless runs give only a rough upper-tail estimate. At 1080p each packed
benchmark frame submitted about 1.24 million visible quad records. The meshless
renderer submits zero terrain geometry.

- **Work reduction.** At 320×180 the coarse interval pass reached 21.20 ms p95
  with direct hashing and 14.54 ms with cached gradients. Both left 2,021 of
  57,600 pixels unresolved and produced byte-identical ray-distance buffers.
  Strict repair brought that to 157 at 45.99 ms p95.
  Numeric repair with a later increase in its work budget reached zero unresolved
  pixels at 29.40 ms p95. An instrumented earlier numeric run measured 14/49/151
  detailed intervals, 2/4/30 full height evaluations, 91/227/899 individual
  detailed bands bounded, and 20/78/112 hierarchy visits per ray (p50/p95/p99).
  Separate tests of eager and octave-aware evaluation and correlation-free band
  ranges showed that removing correlation increased ambiguous pixels. Better
  bounds and a second pass help with grazing rays, which still dominate cost.
- **Visual geometry.** Comparing captured terrain masks at 1080p, the uppermost
  hit is within two pixels in every column, with a median difference of one
  pixel. The masks disagree on 1,349 of 2,073,600 pixels (0.065%). The two 1080p
  meshless horizon runs had zero unresolved pixels. Numeric repair supplied
  89,486 pixels (4.3%). In 196 stratified CPU rays, both smooth and exact-voxel
  oracles agreed with the terrain/sky classification in every sample. Hit
  distance error relative to those two oracles was 0.57 m p95 for each. The smooth
  oracle uses 0.1 m stepping and bisection. This is a sampled cross-check, with
  a finite chance of missing grazing roots, and gives no formal guarantee about
  per-pixel depth, near-field voxel faces, or silhouette stability.
- **Startup.** On a new process with an already populated driver shader cache,
  the filtered 1080p run produced its first GPU frame in 0.66 s. A fresh process
  with the Mesa disk shader cache disabled took 1.89 s. GPU bounds and atlas
  construction with a host wait took 3.15 ms in the warm-cache run. The fresh packed stream
  reported part coverage at 51 ms and reached the requested quality at 8.06 s
  after streaming started. Its on-disk terrain cache was disabled. Stream
  milestones and meshless process time have different clock origins.
- **Memory.** Meshless uses 273 KiB of bounds and 2.91 MiB of lattice gradients.
  The 1080p RGB/depth storage buffer is 31.64 MiB plus an equal readback buffer.
  Process peak RSS was about 154 MiB. The packed benchmark reported 87 MiB of
  resident adaptive GPU terrain, 124 MiB of pool allocations, and 386 MiB peak
  process RSS. Pool allocations include resident data. These are reported
  allocation sizes and RSS, with distinct readback, renderer and app scopes.
- **Movement.** A 36-frame yaw and 190 m approach gave 80.46 ms GPU p95 at
  480×270. A separate 108-frame 640×360 clip covers a 740 m approach and plays
  back at 24 fps. The clip shows rendered motion at offline playback speed. No
  temporal stability score or native-volume handoff was measured.

The experiment removes terrain meshing and arrives at a closely matching distant
silhouette with low persistent terrain storage. It is roughly 47× slower at
1080p on this laptop. Flat shading saves only about 10 ms p95. Repeated bound
construction and octave work in grazing ray intervals remain the benchmark
bottleneck. The results support keeping the packed hybrid in Frontier. Any new
meshless performance attempt needs to amortize most visibility work across
neighboring pixels or frames to be competitive on this hardware.

## Reproduce

Run from the `frontier/` workspace. If needed, set Vulkan to the laptop’s Radeon
ICD and use your active X11 session for the windowed control. Start with idle
hardware. Generated outputs are git-ignored.

```bash
cargo build --release --locked --bin frontier --bin procedural_raycast
cargo test --release --locked -p frontier-renderer --bin procedural_raycast
out=output/procedural-raycast
mkdir -p "$out"
ray=(target/release/procedural_raycast --gradient-cache --no-repair --numerical-fallback)
"${ray[@]}" --width 1920 --height 1080 --tile 240 --frames 12 --warmup 3 \
  --oracle 196 --output "$out/horizon-1080"
MESA_SHADER_CACHE_DISABLE=true "${ray[@]}" --width 1920 --height 1080 --tile 240 \
  --frames 1 --warmup 0 --output "$out/cold-1080"
"${ray[@]}" --width 1920 --height 1080 --tile 240 --frames 12 --warmup 3 \
  --shading geometry --output "$out/geometry-1080"
"${ray[@]}" --width 320 --height 180 --tile 160 --frames 14 --warmup 3 \
  --shading geometry --oracle 196 --output "$out/final-small"
"${ray[@]}" --width 1920 --height 1080 --tile 240 --frames 2 --warmup 0 \
  --eye -1156,995,2460 --target 900,650,3900 --haze .000075 --output "$out/alpine-oblique"
"${ray[@]}" --width 480 --height 270 --tile 160 --frames 36 --warmup 2 \
  --turn-deg 17 --approach-m 190 --output "$out/alpine-motion"
"${ray[@]}" --width 640 --height 360 --tile 160 --frames 108 --warmup 2 \
  --turn-deg 20 --approach-m 740 --haze .000045 --capture-motion --output "$out/flyby"
ffmpeg -framerate 24 -i "$out/flyby/frames/%04d.png" -c:v libx264 -crf 16 \
  -frames:v 108 -pix_fmt yuv420p -movflags +faststart "$out/alpine-flyby.mp4"

packed=(target/release/frontier --preset alpine --walk off --seed 7
  --voxel-size 0.0625 --msaa 1 --vegetation off --terrain-shadows off --beauty off
  --atmosphere off --fog-density 0 --streaming-haze 0 --no-adaptive-disk-cache
  --capture-far 6000 --naadf-radius-voxels 512 --naadf-follow off
  --eye -43.625,75,-289.5625 --target 0,490,3300 --fov-y 60)
"${packed[@]}" --width 1920 --height 1080 --pixel-error 7.5 --adaptive-wait-quality \
  --benchmark-input-lock --warmup 25 --seconds 7 --report "$out/packed-1080.report.json" \
  --capture-on-exit "$out/packed-1080.png"
"${packed[@]}" --width 320 --height 180 --pixel-error 1.25 --adaptive-wait-quality \
  --benchmark-input-lock --warmup 25 --seconds 5 --report "$out/packed-320.report.json"
"${packed[@]}" --width 1920 --height 1080 --pixel-error 7.5 --capture-linear \
  --capture "$out/packed-depth.png"
```

With NumPy, compare geometry masks from `packed-depth.coverage.f32` and
`horizon-1080/ray-distance.f32` read as little-endian float32 and reshaped to
1080 rows of 1920 columns. Packed hits have coverage greater than zero. Meshless
hits have nonnegative distance. The first hit in a column gives the skyline.
The meshless runner also writes `report.json`, with optional per-ray work
percentiles via `--instrument`. Replay the small geometry pass without the
gradient cache, with `--no-repair` alone, or with strict repair (omit both
`--no-repair` and `--numerical-fallback`) to measure each component.

## Cached heightfield research

The original interval resolver spends most of its time evaluating and bounding
noise near intersections. The [coherence results](PROCEDURAL_COHERENCE.md)
show that removing coarse traversal alone cannot offset that cost. This follow-up
uses resident height samples for intersections and bounds only for rejection.

| Priority for Frontier | Method | Algorithm, costs and published evidence |
| --- | --- | --- |
| 1 | Dick, Krüger and Westermann, 2009 | Mirror horizontal direction signs to positive texel traversal, snap the crossed coordinate to the boundary, and carry the current mip level. Descend at potential hits and ascend only across a parent boundary. A descending ray may advance to a parent's height before descending. Piecewise-constant leaves provide exact roofs and step walls for that sampled grid. Bound overhead is about one third of a scalar height texture. The authors report roughly 5× over their own unnested texel walk. Their raster comparison uses tiled terrain and matching screen-error targets. Dense Vorarlberg favored the ray path with roughly 128 MB of geometry residency. Rasterization was faster and smaller on the less dense Utah data. [Paper, sections 3 and 4](https://www.cs.cit.tum.de/fileadmin/w00cfj/cg/Research/Publications/2009/GPU_Ray-Casting/EG09AreasTerrain.pdf) |
| 2 | Tevs, Ihrke and Seidel, 2008 | Max-reduce each four-texel group. At a bilinear leaf, four corner samples bound the patch and substitution of the ray gives a quadratic. Use the first root inside the cell's ray interval, even if both endpoint signs agree. The paper packs corner samples into RGBA to trade texture footprint for fewer fetches. Successive reductions are straightforward to update locally. Their 8800 Ultra took 2.13 ms at 2048² and 7.52 ms at 4096² for mip construction. [Paper, section 3 and Table 1](https://pure.mpg.de/rest/items/item_1325622_5/component/file_3590464/content) |
| 3 | Vertical coherence | Maintain the angle of the highest visible terrain in a vertical plane. A constant-height cell contributes a monotone slope interval, so one traversal can resolve an entire upright screen column, including exact side hits. The 1997 method projects pencils of vertical planes onto arbitrary camera planes. The projected lines converge under pitch and uncovered pixels use interpolation. Kakkar's current compute WGSL walks increasing adaptive depth steps once per column, emits newly visible spans, and samples repeated heights and colors. It provides a shared projection reference with approximate visibility rather than an exact texel walk. The older fragment example in the README walks independently per pixel. One invocation per column reduces parallelism on integrated GPUs. [Lee and Shin](https://doi.org/10.1002/(SICI)1099-1778(199703)8:2%3C97::AID-VIS164%3E3.0.CO;2-A) · [Kakkar shader](https://github.com/rachitkakkar/gpu-voxel-terrain/blob/9f72df83b22fc6e2f206504dde24fb35f156d361/src/voxels.wgsl) |
| 4 | Nilles and Müller, 2025 | Project into a 2D DDA and intersect vertical intervals in each cell. Per-layer min/max merge creates coarse boxes. Fine half-precision stacks carry material boundaries, and coarser levels merge excess vertical layers. The optional stackless traversal ascends after 8 misses (16 at the finest grid). Their CUDA code uses packed half pairs and specializes ray and material modes. Counter-versus-stack speed ordering reversed between their older GPUs and RTX 4080. Their 3.5 ms low preset used the RTX 4080 at 1080p on a 256 m square with 4096² columns at 6.25 cm. [Paper, sections 4 and 5](https://diglib.eg.org/server/api/core/bitstreams/37905fb9-2e91-45a2-9238-152be9d9fecc/content) · [MIT source at 81863c6](https://github.com/Clocktown/CUDA-3D-Hydraulic-Erosion-Simulation-with-Layered-Stacks/blob/81863c697e2a1b3a8948d5edad58e0304182ec30/core/geo/device/raymarch.cu) |
| 5 | Feldmann and Hinrichs, 2012 | Cache nested active tile regions in a texture array, publish coarse tiles first, and use a tile-map lookup for the finest resident level. Traverse projected cells via 2D exits. Screen-projected face area and resident coverage determine termination. Optional local linear/bicubic reconstruction changes the hit model. The GTX 470 reached 52.3 FPS at 1080p on City 2, 42 FPS with local linear reconstruction. A bounded world-aligned cache is a good follow-up once the finite renderer reports acceptable cost and quality. [Paper, sections 3–4](https://studyres.com/doc/22424173/gpu-based-single-pass-ray-casting-of-large-heightfields) |
| 6 | RMIP, 2023 | Four overlapping power-of-two rectangular min/max reads bound an arbitrary rectangle. Separate horizontal and vertical scales tighten slender projected footprints. The dense representation costs N²(1+log₂N)² ranges, with the paper compressing storage to mipmap order at the expense of some tighter queries. Its displacement inverse, ray-curve splits and reported 5× mean gain on RTX 3080 concern triangle-based shell displacement. Frontier could independently test anisotropic bounds for shallow planar rays. [Paper, sections 4–6](https://www.iliyan.com/publications/RMIP/RMIP_SigAsia2023.pdf) |
| 7 | Robust cone steps, 2024 | Advance within a conservative cone, with an exact cell-boundary step to avoid stagnation. Ordinary bilinear interpolation of apertures can create an unsafe cone. The authors use a neighborhood minimum correction and compute relaxed apertures by testing limiting points that can admit a second crossing. Updates can affect cones centered beyond the edited tile. Benchmark preprocessing and invalidation along with traversal before adopting. [Paper, sections 3–6](https://diglib.eg.org/server/api/core/bitstreams/c6853f7d-4962-412f-927e-462fe29031d1/content) |
| 8 | Moinet and Neyret, 2025 | Partial octave sums plus bounded omitted amplitudes form nested enclosures. Sub-stepping and early-stop-plus-one choose fewer bands and tighter local Lipschitz bounds. Their second-order envelope switches from quadratic to linear when the global slope cap is reached. They report gains up to 12× over classical sphere tracing in opaque cases. This is a candidate for selective residuals after caching, with independent residual error and first-hit tests. [Paper, sections 4–6 and 9](https://diglib.eg.org/bitstream/handle/10.1111/cgf70072/cgf70072.pdf) |

The 2026 Vangers preprint provides a useful integrated-GPU counterexample.
At default settings on the Radeon 780M its 128-step texture ray march averaged
4.83 ms, its coarse 3D occupancy acceleration averaged 8.32 ms, and its fitted
mesh averaged 4.07 ms. The simple march can miss thin features, and the fitted
mesh incurred a roughly 2.5 s one-time fit in the worst 780M scene. Extra lookup
work, memory and cache locality can erase benefits even when the hierarchy
is conservative. [Malyshau, sections 3 and 5](https://arxiv.org/html/2608.17390v1)

A fresh search through 2026 also surfaced QuadStack's compressed layered
ray cast, hierarchical terrain horizon visibility and 2026 NAADF temporal
history. QuadStack is relevant to later layered edits. Precomputed region
horizons have potential in static terrain and incur edit maintenance.
NAADF uses temporal accumulation of quantized voxel hits for shading, a separate
contract from proving current-frame first visibility. [QuadStack](https://cs.purdue.edu/homes/bbenes/papers/Graciano20TVCG.pdf) · [Hierarchical terrain visibility](https://research.cs.queensu.ca/home/jstewart/papers/egwr97.html) · [NAADF, 2026](https://onlinelibrary.wiley.com/doi/full/10.1111/cgf.70413)

The implementation below is independent Rust/WGSL. Source-reading provenance
is listed for the CUDA and WGSL comparisons. All traversal modes use the same
sampled geometry within an A/B. The existing affine source bounds and native
reference still describe the canonical world. Coarse sample maxima certify
only the selected reconstruction, so native fidelity is checked separately.

### Experimental implementation

`cached_heightfield` is a standalone compute renderer. Its finite control
generates CPU f64 samples from seed 7, then uploads scalar height textures
and conservative bounds. The default benchmark footprint is 16,384 m square,
with 16, 8 and 4 m sample pitches. The separate native fixture covers 32 m
at 6.25 cm. Rays use forward depth and an actual 6 km projection clip, with
the established camera and a 60° vertical FOV for the stationary full-range
comparison. The core traversals are selectable as follows.

- `--modes max` restarts at the maximum mip after an empty segment.
- `--modes dick` retains a mip and ascends on parent crossings. A small
  conservative guard precedes the descending-ray jump at a parent maximum.
- `--modes dda` counts misses before ascending and shares the same mip
  and leaf intersection functions. It adapts the Nilles and Müller
  stackless policy to one vertically solid interval per column.
- `--modes vertical --level` holds the horizontal direction fixed over a
  column or row band. It rejects a block using the lowest unfilled row,
  shares horizontal intersections and writes each newly visible row once.
  A bilinear leaf shares its height polynomial across rows. Pitched and
  banked cameras use independent DDA, since a screen column then projects
  to several azimuths. This keeps arbitrary aircraft orientations exact
  for the cached sampled surface.
- `--bilinear` analytically solves the first zero of the ray-minus-patch
  quadratic. Opposite signs at the two endpoints are sufficient but not
  necessary for intersection. Stepped mode intersects the top plane or a
  vertical entry wall. Max/min and two-mip strides are selectable experiments.

The GPU cache `--stream` snaps its world origin in 64-sample increments.
Existing height samples and mip levels through level 6 stay at toroidal
physical addresses. Strip jobs generate only newly visible interior samples
and the top/right halo. The small upper mips are rebuilt in logical order,
which keeps their alignment correct after arbitrary origin shifts. Reads
use the new origin only after all writes and reductions have completed.
This first publisher waits synchronously and keeps overlapping data resident.
A move that displaces the entire window requires a cold fill.

`--nested` adds a 512² moving grid at one sixteenth the outer pitch. A
world-space camera-centered smoothstep blends fine and parent samples
before reconstruction. The inner trace uses the union of fine maxima and
covering parent bounds wherever a blend can occur. The fade reaches the
parent heightfield inside the inner window edges. Both world-aligned grids
then describe the same patch at the ownership boundary, so recentering a
window need only change sampled addresses. The native fixture remains a
separate integer-column test.

`--history` tries a reprojected previous hit, tests the candidate inside
current cached terrain, and uses the verified point only as a far limit.
The ordinary traversal still proves visibility from the near plane. A view
cut invalidates the candidate before texture reads. History storage and
copy time are included in GPU results. This is a visibility experiment
separate from temporal antialiasing.

### Benchmark results

Measurements use the Ryzen 5 7640U and **Radeon 760M** selected as an
integrated Vulkan device, with Mesa RADV 25.2.8. The finite tests run
two trials in opposite method order. The main stills contain 40 timed
frames per method after separate warmups. Resolution is the actual
visibility and shading dispatch size.

The 8 m cached DDA took **2.72 ms p95 for visibility**
and **3.43 ms shaded GPU p95** at 1080p. The rolling
two-resolution path took **5.13 ms** for the stationary
view and **5.62 ms** over the banked flight route. The
nested GPU span includes any strip updates during the sample. These are
standalone passes with terrain shading and depth. The packed hybrid
control covers a broader frame with different shading and ownership.

| Resolution | Restart max visibility | Dick visibility | Miss-counter DDA visibility | DDA plus shading |
| --- | ---: | ---: | ---: | ---: |
| 320×180 | 0.99 | 0.37 | 0.33 | 0.36 |
| 640×360 | 3.11 | 0.78 | 0.60 | 0.68 |
| 1920×1080 | 9.96 | 4.53 | 2.72 | 3.43 |

Values above are GPU p95 in milliseconds for the same pitched bilinear
grid. The earlier measured references are 11.54 ms for the packed
hybrid repeat, 550.33 ms for direct fBm, and 523.94 ms for coherent fBm.
The raw reports contain p50/p95/p99, separate complete submit-and-wait
wall time, sample counts, cold setup and memory. The packed span includes
its frame passes and excludes presentation. The cached path uses a
simpler shading model and approximate far terrain. [Prior controls](PROCEDURAL_COHERENCE.md#stationary-benchmark)

On the instrumented pitched still, restart traversal fetched 165.6M
bounds, boundary ascent fetched 43.4M,
and miss-counter DDA fetched 45.4M. The DDA is faster despite
slightly more reads than boundary ascent. The branch and arithmetic
sequence matter alongside request count. Native warp occupancy was
outside the measurement scope. The cold generator evaluates fBm for
new texture samples only. Intersection kernels perform zero procedural
height evaluations.

The max/min experiment took **2.90 ms** versus **3.04 ms**
for max-only stepped traversal, with twice the bound storage.
Using two mip levels per ascent took **3.45 ms** for bilinear DDA.
The combined timings favor scalar max textures and one-level changes
for these views. A flat full-height solid already admits an inexpensive
max-only rejection, so extra volume lower bounds have limited value.

### Vertical coherence and camera movement

| Level camera | Stepped independent / vertical | Bilinear independent / vertical |
| --- | ---: | ---: |
| 320×180 | 0.26 / 0.94 ms | 0.34 / 1.23 ms |
| 640×360 | 0.55 / 0.49 ms | 1.12 / 0.89 ms |
| 1920×1080 | 3.16 / 0.54 ms | 6.36 / 0.65 ms |

Band widths trade locality against parallelism. Full-column invocations
reduce texture requests at the expense of fewer active groups and serial
output. The corrected grid traversal tests the other axis against the
same edge times used by the DDA and skips zero-length intervals. This
removed a 90.8 m disagreement at one grazing pixel in the early stepped
test. The initial Dick height jump also needed its conservative guard
to preserve narrow native intersections. Final comparisons below use
the corrected traversal.

| 1080p route | Finite shaded GPU p95 | Nested shaded + update GPU p95 |
| --- | ---: | ---: |
| Grazing ridge | 3.46 ms | — |
| Walk | 5.84 ms | 7.00 ms |
| Rapid turn | 3.31 ms | — |
| High flight | 3.43 ms | — |
| Low flight | 4.19 ms | 7.00 ms |
| Banking flight | 3.42 ms | 5.62 ms |
| Fixed 33.8° bank | 3.27 ms | 4.87 ms |
| Revisit | 3.79 ms | 6.04 ms |

Published shaded captures use analytic terrain normals and a
footprint-filtered trail tint. They use a single visibility sample
per pixel and clear distance shading for comparisons. The motion
capture uses 960×540 per panel and offline playback at 24 FPS.
Actual 1080p route metrics above use 72 or 96 timed frames. The
nested still and bank measured 6.02 ms and
7.33 ms p95 for complete headless frame
submission and readback wait. Presentation and capture encoding have
separate scopes.

### Cache behavior and temporal test

The rolling 2048² plus 512² configuration holds **39.7 MiB**
of logical height and max textures at 8 m and 0.5 m, including corner
halos. The 1080p depth and color outputs add 15.8 and 7.9 MiB
respectively. Capture staging, optional work counters and driver
allocations are separate. The raw process RSS includes CPU oracle work.
The two-tier still first completed at **121 ms**
from a fresh process with a populated driver shader cache. The run with
`MESA_SHADER_CACHE_DISABLE=true` first completed at **257 ms**.

One outer 64-column shift regenerated 131,072 of 4,194,304 height
samples (3.125%) plus the halo, with a **0.67 ms**
median GPU update on the revisit route. A full 18 km relocation
regenerated the entire texture in **18.0 ms**
median GPU time. The nested teleport reported 3.81 / 5.42 / 21.45
ms GPU p50/p95/p99, so p95 alone hides the cold transition. Overlapping
data survive a return trip. An evicted strip is produced again when
revisited. This synchronous cache validates geometry and scheduling
cost, with asynchronous publication still needed for teleport tails.

The temporal tests alternate off/on/on/off across four distinct trials
per route, with counters enabled on both paths. Current-surface
validation limits the tested interval without skipping the first hit.

| Route | History off, GPU p95 | History on, GPU p95 |
| --- | ---: | ---: |
| Turn | 5.46, 5.51 ms | 6.54, 6.67 ms |
| Bank | 5.66, 5.81 ms | 6.89, 6.92 ms |
| Teleport | 5.84, 5.84 ms | 6.74, 6.87 ms |

History candidates had no coverage disagreements in the matched last
frames. Reprojection, sampling validation and an extra 15.8 MiB depth
history copy made this technique slower than the inexpensive cached walk.
The tested path is off by default. RMIP, cones and wider residual
search remain research options, with sampling fidelity ahead of further
traversal acceleration in the measured workload.

### What the depth checks establish

The exact native GPU fixture matched the independent 6.25 cm
pixel-center reference on **all 1,079,525 visible terrain pixels**
at 1080p with zero terrain/sky disagreements, 3.8 µm
radial depth error at p95, and zero sampled face disagreements.
The scene is a 32 m Alpine window with a 10 m clip. Colors use the
two implementations' shading. The native reference emitted coverage
and radial depth independently from GPU data.

The pitched 6 km comparison uses the prior continuous GPU tracer as a
full-frame visual control and the canonical f64 voxel DDA for targeted
and stratified checks. The error map marks missed terrain in red, extra
terrain in blue, and common hits differing by more than 5 m in amber.
Full-resolution exact native terrain is used only
in the bounded fixture. Far samples have approximation error.

| Far pitch | Logical terrain | Shaded GPU p95 | Missing / extra hits | Depth p95 | Errors above 5 m |
| --- | ---: | ---: | ---: | ---: | ---: |
| 16 m | 9.3 MiB | 3.60 ms | 285 / 7 | 1.14 m | 2429 |
| 8 m | 37.3 MiB | 3.43 ms | 114 / 2 | 0.66 m | 417 |
| 4 m | 149.4 MiB | 3.94 ms | 45 / 1 | 0.58 m | 160 |
| 8 m + 0.5 m | 39.7 MiB | 5.13 ms | 115 / 2 | 0.66 m | 418 |

The 8 m visual skyline differs by at most one pixel against the older
direct run, but that hides large errors at internal occlusion edges.
At [378, 467] the cached hit is 3890.15 m
and the native voxel first hit is 3152.11 m.
The independent sampled oracle matches the cached surface there. This
is a representation error at a grazing first crossing, rather than
a missing GPU dispatch or disagreement over the cached geometry.

A forced cache recenter at the same camera produced zero coverage
changes and 0.25 mm depth p95
(3.94 cm max). Both
cache boundaries describe the same parent surface before entering the
outer domain. These tests cover the named poses and captured trajectories.
Revisiting the original pose had 0 coverage
changes and a 0.00 mm maximum depth difference.
Long exploration, shading antialiasing and edits need separate validation.

The native-center GPU audit found 82
and 382 integer-height
disagreements in two 262,144-column windows, each at most one voxel.
Native occupancy still needs canonical CPU heights or another bit-exact
generator. GPU f32 generation is suitable for the approximate layers.

### Recommendation

Use the stackless cached DDA as the next meshless experimental base.
Retain vertical profiles as an additional path for compatible cameras.
The main performance risk now lies in cold updates and near-ground
traversal. The main quality risk lies in first crossings lost by
sampling, which smooth fades cannot repair. Prioritize asynchronously
published outer coverage, canonical integer-column ownership in a near
window, and source-aware sampling at grazing ridges. Test edit revisions
and NAADF volume ownership together before considering production use.
Elevation-only edits can update integer heights and their ancestors.
An excavation or overhang needs ownership that excludes the affected
coarse heightfield cells and delegates volume visibility to NAADF.
Cache generation and volume ownership should publish the same revision.

[Matched horizon still](reference/cached-heightfield/horizon-comparison.png) ·
[Native fixture](reference/cached-heightfield/native-comparison.png) ·
[Fixed 33.8° bank](reference/cached-heightfield/banked-comparison.png) ·
[Silhouette and depth errors](reference/cached-heightfield/horizon-error-map.png) ·
[Walking and banked flight](reference/cached-heightfield/motion.mp4) ·
[Visual metrics](reference/cached-heightfield/visual-validation.json) ·
[Raw reports](reference/cached-heightfield/reports)

### Reproduction

Run from `frontier/` with its pinned toolchain. The hardware runs selected
Mesa RADV on the Radeon 760M. Use the Radeon Vulkan ICD on machines with
several adapters. The benchmark refuses a software Vulkan adapter by default.

```sh
cargo build --release --locked --bin cached_heightfield --bin landscape
cargo test --release --locked -p frontier-renderer --bin cached_heightfield
bash tools/cached_heightfield_bench.sh output/cached-heightfield
python3 tools/cached_heightfield_compare.py output/cached-heightfield --video
MESA_SHADER_CACHE_DISABLE=true target/release/cached_heightfield --nested \
  --cells 2048 --width 1920 --height 1080 --modes dda --shaded \
  --frames 1 --warmup 0 --trials 1 --output output/cached-heightfield/nested-cold-shaders
```

The script supports independently running `stationary`, `profile`, `routes`,
`streams` and `reference`. The optional full-screen direct baseline comparison
reads the existing procedural-coherence output. If it is absent, regenerate
the earlier experiment using [its commands](PROCEDURAL_COHERENCE.md#reproduce).
The offline native reference uses one ray at the exact pixel center to match
the GPU visibility samples. The previous offline jittered mode remains the
default. Python comparisons need NumPy and Pillow, plus FFmpeg for the video.

The new tests check dirty-strip coverage, analytic root isolation and WGSL
validation. The checked-in reports include all final sampled configurations
and earlier diagnostic findings. The experimental binary, prior procedural
ray caster and production Frontier renderer remain separate entry points.
