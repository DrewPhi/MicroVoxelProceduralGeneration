# Alpine playability pass — 2026-11-08

Baseline: `5a919a4`, after the confirmed vegetation flash fix. This pass keeps
Alpine terrain source 4, appearance 2, vegetation source 3 and native 6.25 cm
voxels. No new terrain recipe, painting, fog curtain, reduced draw distance or
prebuilt vegetation assets. It is greenfield work on Frontier, not an old-engine
port. Full-resolution remains the normal Alpine setting.

## Implemented

1. **Reuse detailed appearance before filtering.** A bounded shared node cache
   keys global position, effective filter/footprint, surface policy and the
   owning immutable world. A separate namespace avoids patch-profile collisions.
   A hit replays actual sampled height extrema, preserving the height envelope.
   Existing per-tile node IDs are also checked before recomputing a map. Point,
   radius-2 box and footprint tests compare uncached/cold/warm float bits and
   exact sampled extrema. Warm jobs query fewer native columns.
2. **Protect nearby generation.** Alpine requested-quality failures within 32 m
   form their own coherent refinement transaction, so a slow distant child
   cannot delay that transaction. Distant refinement resumes after these quality deficits
   are resolved; existing complete coverage remains visible throughout. One fair
   vegetation slot may refine affordable ground cover within 12 m once its own
   baseline is available, without waiting for all distant coverage.
3. **Bounded cache/streaming tuning.** Alpine now reserves 640 MiB total CPU
   terrain cache: 128 MiB source and 512 MiB retained geometry. The source includes
   up to 16 MiB memoized profiles and 4 MiB bounds within that reservation. This
   is a disclosed memory trade, not a free speedup. Other sources keep their
   previous defaults; explicit overrides remain available. Terrain uploads default
   to 2 MiB per frame, with the existing disclosed single-cluster quantum.
   Persistent derived caching defaults on for Alpine, bounded to 2,048 MiB at
   `output/cache/alpine-terrain`. It is disposable data; edits/world authority
   remain independent. `--no-adaptive-disk-cache` disables it for cold tests.
4. **Stable vegetation selection.** Existing fine geometry survives unrelated
   pending baselines. Coarsening has a 10% distance hysteresis; approaching still
   refines immediately when affordable. Hard instance/triangle/memory caps remain.
   Ground placement grouping is reused while resident placement inputs remain
   unchanged; asynchronous placement completion and world changes invalidate it.
   Each plant still uses its own full seed and growth rules.
5. **Render-only step easing.** Upward voxel steps use a 55 ms exponential camera
   response, bounded to the controller's three-cell step height. Collision and
   gravity continue from the separately retained physical eye. Falls, jumps,
   external camera moves and flight reset/bypass easing. Unit tests compare the
   physical path with an uneased controller and verify frame-rate-independent
   presentation. Esc exposes the checkbox; `--step-smoothing off` disables it.
6. **Motion and complete rendering evidence.** The report includes a bounded
   last-3,600-frame observation ring with frame/preparation/terrain/vegetation
   time, physical and visible eye, yaw, pending counts and native clusters.
   Supported devices also report asynchronous full-render GPU spans, including
   shadows, opaque geometry, atmosphere and UI. CPU, earlier uploads and
   presentation are outside that GPU span. Unsupported devices report no full
   span; no blocking readback is added. The HUD distinguishes full-render from
   opaque-only GPU timing.

## Additional measured causes of stalls

On this X11 backend, winit's `inner_size()` requests window-server geometry.
The renderer called it twice each frame; the overlay and standard GUI convenience
adapter made additional calls. Resize/occlusion events now maintain dimensions,
and the GUI consumes its public RawInput with those cached dimensions. Startup
queries remain, while normal per-frame queries are removed. Minimized/occluded
windows skip presentation. This was the largest demonstrated frame-tail fix in
this pass; menu, input and DPI behavior still need broader platform testing.

Camera projection/light constants are calculated once per view rather than per
terrain cluster. Bit-equivalence tests compare the cached camera visibility/error
queries with their scalar equivalents. An identical prepared view also reuses
shadow-camera uniforms; shadow passes still render each frame so newly generated
plant casters are included. The local matrix operations and floating-point ordering remain
unchanged. Vegetation grouping reuse avoids repeated allocation while walking
through unchanged placement tiles.

The convex-solid occlusion test was also repeatedly checking up to 64 boxes
against thousands of mostly visible terrain clusters. A cheap angular rectangle
containment check now rejects impossible *occlusion tests*. It never establishes
occlusion itself: all surviving candidates still use the original same-box,
eight-corner segment proof. Near-plane/uncertain projections fall back to that
proof. A 12,000-case randomized comparison agrees with the original predicate;
existing dense oracle tests remain. Related background on culling overhead and
conservatism: [Wimmer and Bittner, GPU Gems 2 chapter 6](https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-6-hardware-occlusion-queries-made-useful).
This implementation is a CPU proof broad phase, not that chapter's hardware-query
algorithm, and never reuses stale-camera occlusion results.

## Reproduce

```sh
./run.sh --preset alpine
./run.sh --preset alpine --no-adaptive-disk-cache
./run.sh --preset alpine --step-smoothing off
./run.sh --preset alpine --route walk-stop-resume --route-clock realtime --route-speed 3 --seconds 22 --report output/walk.json
./run.sh --preset alpine --route walk-turn-return --route-clock realtime --route-speed 3 --seconds 12 --report output/turn.json
python3 tools/record_walk.py --output output/my-walking-check
```

The `walk-*` routes use real collision/step presentation. Older `fly`,
`stop-resume` and `turn-return` routes retain their flight semantics and should
not be reported as player-height walking evidence. The recording helper uses
actual X11 walking, stop/resume and a gradual mouse turn, targets only its child
PID/window and captures 20 FPS video. ffmpeg/XTest are required. Recording and
selection tracing perturb performance; use separate runs for timings.

Controls: Esc menu, WASD, Space jump, F flight. Render scale is independently
adjustable; reducing it changes internal image resolution, not canonical voxels,
terrain/material definitions or the visible-distance setting. It is an explicit
quality/performance choice, not part of a claim of identical full-resolution
output. `FRONTIER_SOURCE_CACHE_MIB=32` selects the smaller source allocation;
`FRONTIER_MAP_NODE_REUSE=off` provides the uncached map-building diagnostic.

## Measurement scope

Actual laptop: Ryzen 5 7640U / Radeon 760M integrated graphics, Vulkan RADV
Mesa 25.2.8. Dense featured view eye `[-43.625,31.0625,-289.5625]`, target
`[-43.625,31.3625,-259.5625]`, 60° FOV, 6 km clip, 4×MSAA, detailed maps,
soft-ground radius-2 filtering / 2× anisotropy. 720p uses 3-pixel terrain error;
1080p uses 4.5 pixels for the same angular bound. Timing runs are sequential,
uncapped, input locked and separate from builds/recording. Settled runs begin
after requested quality plus the internal reserve, followed by 60 warmup frames.

The 32 versus 128 MiB source-cache experiment used equal 640 MiB total budgets
and grounded 22-second cold walking, before the final window/culling changes.
128 MiB produced 499 versus 442 resident clusters, about 368k versus 509k page
evictions and 37.59 versus 42.62 s summed appearance worker time. First native
terrain appeared around 5.24 versus 5.56 s. This is better generation progress,
not an equal-quality FPS win: frame p95 was slightly worse as more content was
prepared. Worker sums overlap and include canonical source queries.

The per-frame window-query change reduced the dense static 720p frame p95 from
39.95 to 19.07 ms and 1080p p95 from 61.77 to 27.00 ms in the measured short
runs. Median/burst rates did not uniformly improve. These measurements precede
the final occlusion broad phase; final results and evidence follow below.

## Final evidence

Release build and workspace tests pass: **465 passed, 11 intentionally ignored,
zero failed**. The fixed Legacy primary regression is byte-identical to frozen
`5a919a4` for PNG, linear RGB, depth and coverage. This verifies that the
non-Alpine reference did not change; it does not establish identical asynchronous
Alpine refinement schedules.

| Run | Frame p50 / p95 / p99, ms | Full-render GPU p50, ms | End requested quality |
| --- | --- | --- | --- |
| 1280×720, dense settled view | 17.30 / 20.31 / 28.57 | 16.12 | ready |
| 1920×1080, dense settled view | 25.16 / 28.28 / 30.72 | 23.21 | ready |
| 1280×720, grounded walk/stop/resume | 21.16 / 32.45 / 37.47 | 17.41 | ready |
| 1280×720, grounded turn/return | 13.21 / 24.56 / 30.42 | 11.44 | still refining |
| 1280×720 output, 0.75 render scale, grounded walk | 14.69 / 24.30 / 28.18 | 12.40 | ready |

The last row renders internally at **960×540**. It is an optional quality trade,
not a lossless performance improvement. All runs retained coherent coverage.
The turn changed demand and ended before the new heading reached requested
quality. Neither the full-resolution runs nor the scaled walking run pass the
16.67 ms whole-frame tail target: this checkpoint improves playability, but
**does not close the laptop 60 FPS production gate**. Native 1080p in this dense
view is already GPU-limited; walking also has CPU terrain-preparation spikes.
Remaining work is measured batching/visibility/preparation optimization and
GPU plant/shadow cost, rather than hiding loading with opaque fog.

The actual X11 input smoke passed walking, jump and flight displacement, and
Esc open/close input delivery. This is a local X11 check, not cross-platform
DPI or menu-state certification. The new recording contains normal startup,
walking, stopping/resuming and a delivered 1.28-radian mouse turn. I inspected
spaced frames and the largest adjacent-frame differences: no isolated coarse
vegetation reset was identified in that sample. Loading/refinement remains
visible, and a 20 FPS recording cannot certify every presented frame.

Retained artifacts:

- [Before/after scene](reference/gallery/playability/before-after.png).
- [Final full-resolution scene](reference/gallery/playability/alpine-after.png).
- [Walking/turning contact sheet](reference/gallery/playability/walking-contact.png).
- [Compact measurements, cache/memory counts and primary hashes](reference/gallery/playability/measurements.json).

Raw reports/logs and `recording-release/walking.mp4` remain locally under
`output/playability/` (ignored). The executable SHA-256 is in the compact
measurements. The default persistent cache is distinct from the explicit reused
`output/cozy-scene/map-cache` used for this measurement set; the first ordinary
launch may therefore still have a cold cache.
