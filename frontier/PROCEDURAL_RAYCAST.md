# Meshless Alpine ray casting experiment

Branch `experiment/procedural-raycast` starts at Frontier `b59dedb`. The
standalone Rust/wgpu binary renders the canonical continuous Alpine height
function directly. The renderer builds no terrain vertices, triangles, or
geometric LODs. The packed hybrid remains the game renderer. The experiment
covers the untouched Alpine heightfield at the default terrain parameters.
The native 6.25 cm voxel source, sparse edits, and NAADF stay in Frontier.

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
