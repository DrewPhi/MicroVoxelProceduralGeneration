# Moving native terrain and transition-quality checkpoint

Later travel improvements and volume/renderer experiments are recorded in
[Prototype checkpoints 3–5](PROTOTYPE_CHECKPOINT.md). Measurements below describe
the earlier checkpoint.

Implemented on `frontier-voxel-engine`, greenfield (tier G). This supersedes
the fixed hybrid experiment in [HANDOFF_CHECKPOINT.md](HANDOFF_CHECKPOINT.md).
The moving heightfield prototype is playable; the invisible-transition and
arbitrary-volume ownership gates are **not** passed.

## What changed

- `--scene hybrid`: 64 × 64 m of exact 6.25 cm columns follows the camera in
  8 × 8 m chunks. A one-chunk move reuses 56/64 chunks. Source identity stays
  fixed while demand epochs change; stop/resume and return retain valid work.
- One native worker generates entering chunks, cancellation occurs between
  chunks, and another worker prepares the collar. Old coherent coverage remains
  drawable until native chunks, collar and GPU terrain pages are ready together.
  Obsolete camera selections cannot publish. No render-thread terrain generation.
- CPU cache: 128 chunks / 512 MiB. GPU cache: 128 chunks / 512 MiB target;
  active and incoming coverage are protected, with a 1 GiB native transition
  admission limit. These are payload budgets, **not total application RAM/VRAM**:
  pinned CPU references, pages, staging, render targets and driver allocations
  are additional. Native chunk writes are limited to 8 MiB/frame; collar and
  changed proxy pages upload separately at publication.
- Chunks have worker-computed bounds for frustum rejection. Partial GPU uploads
  never draw. A bounded recycler takes large CPU destruction off the render thread.
- The collar uses actual filtered GPU page endpoints and canonical native
  column heights/materials. Shared parent normals now use the actual parent
  triangle interpolation. Morph weights follow continuous camera motion rather
  than jumping with page-center snaps.
- `--msaa 1|4` provides edge antialiasing in window and headless modes.
  `--voxel-filter on|off` experiments with footprint-filtered native lighting
  and projected staircase appearance on the proxy. It is **off by default**:
  captures found bias and bright grazing-angle slivers. MSAA is the recommended
  antialiasing option for now; it does not soften native face normals.
- `--scene hybrid-static` preserves the earlier fixed CPU reference.

## Tests and image evidence

64 workspace tests pass, one existing performance-only test is ignored. Release
build and formatting checks pass. Tests include exact chunk halo face ownership,
canonical DDA depth/material parity, cached return reuse, obsolete demand
rejection, strict partial-upload budgets, and complete-before-publication rules.

Five collar centers (including negative coordinates and 65.536 km) give maximum
sampled outer depth discrepancy 8.72 micrometers; outer endpoint normals/colors
match the actual GPU page data. Common vertices have zero morph-height change
at tested center snaps. These finite tests do not certify all silhouettes or
arbitrary 3D topology. The planar appearance model matches a discrete-column
oracle, but that does not establish correctness on rough curved terrain.

Raw PNGs and JSON are in ignored `output/moving-native-checkpoint/`:
`filter-{on,off}-msaa{1,4}.png`, `reference-off-4k.png`, `aerial.png`, and
`moved-24m.png`. The latter is a cold coherent snapshot at the route position,
not evidence that streaming kept up; use route JSON for motion evidence.

The 3840 × 2160, 4× MSAA, filter-off reference is downsampled in linear RGB by
`tools/compare_capture.py`. In the native-heavy bottom corners of the seed-7
ground view, linear RGB RMSE decreases from **0.04195 to 0.01524** with 4× MSAA
(about 64%). Enabling the experimental appearance filter at 4× raises it to
0.02267. Whole-frame filter-on/off errors also include an intentionally different
proxy lighting model, so they cannot be called aliasing-only errors. This is a
sampling comparison against the same geometry, not a fine/coarse silhouette test.

Visual inspection still finds resolvable native steps ending at the smooth
collar, despite improved boundary alignment. Aerial views also expose a thin
boundary line. Smooth distant facets and occasional shading slivers remain.
Do not claim checkpoint 2's final invisible-transition quality target is done.
The next geometry experiment must preserve visible steps or move the handoff
according to projected error; blanket smoothing is not an acceptable substitute.

## Reproduction and performance

Hardware: Ryzen 5 7640U / Radeon 760M, RADV PHOENIX, Mesa 25.2.8, Vulkan.
Seed 7 / source version 1; scale 1800 m, amplitude 1000 m, 8 octaves, roughness
0.5. Seven levels, proxy half extent 2048 m; no fog or temporal AA.

```sh
cd frontier
cargo build --release --locked
./run.sh --scene hybrid --msaa 4 --voxel-filter off
bash tools/smoke_hybrid.sh
./run.sh --scene hybrid --capture output/view.png --msaa 4
./run.sh --scene hybrid --route fly --route-speed 3 --capture-route-time 8 \
  --capture output/moved.png --msaa 4
```

WASD moves at 3 m/s, Shift at 15 m/s. Mouse looks; Space/Ctrl moves vertically.
This remains free flight, with a 1.7 m starting eye height, **not walking collision**.
Benchmark routes clamp above terrain; interactive flight can go underground.
`--capture-route-time` selects a deterministic initial route pose, including
waiting for coherent cold coverage. Default scene is still the broad `fbm` proxy.

Final recommended profile: 1920 × 1080, 4× MSAA, appearance filter off,
uncapped, first 30 frames excluded. Durations 20 / 15 / 8 seconds respectively.

| Route | Speed m/s | p50 / p95 / p99 ms | Frames >16.67 ms | Max / end coverage-center lag m | Coherent publications |
|---|---:|---:|---:|---:|---:|
| walk-off | 3 | 6.30 / 11.71 / 14.96 | 14/2899 | 6.20 / 5.92 | 7 |
| sprint-off | 15 | 3.89 / 8.29 / 12.60 | 6/3227 | 29.96 / 2.71 | 21 |
| return-off | 15 | 7.73 / 11.84 / 20.00 | 13/931 | 29.96 / 0.00 | 6 |

Walking remains well inside native coverage. Sprint/return reached almost 30 m
of transient center lag in the early travel segment, close to the 32 m native
half extent; prediction/latency work is still needed before calling sprint
coverage robust. Return reaches the original center using the cached chunks.
The return run also has a ~20 ms p99: 60 FPS at every instant is not established.

Initial coherent CPU preparation was 1.39–1.40 s, separate from frame intervals
and initial GPU setup. Native chunk writes remained <=8 MiB/frame with zero
budget violations. Native GPU payload peaked around 512 MiB; the reports retain
exact CPU cache/GPU payload counts, triangle counts and pending uploads.


Frame intervals include presentation and are not isolated GPU timings. Short
runs do not establish sustained thermal performance or all-seed worst cases.
The broad issues remain open: cave/edit ownership, certified visual error,
aircraft/teleport behavior, adaptive native extent, and renderer comparisons.
