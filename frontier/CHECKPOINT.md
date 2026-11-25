# Native reference checkpoint — 2026-11-04

Status: runnable foundation and bounded edit sandbox. No production architecture
or performance gate is marked complete. Reuse tier G throughout; new code, no
legacy source imports. Sol medium implemented world/reference geometry and viewer;
Luna high implemented runtime policy contracts; root reviewed integration and tests.

## Delivered

- Isolated pinned Rust workspace and launch script.
- Deterministic integer source, 6.25 cm cells, tunnel, bridge, COW session edits.
- Exact surface-only greedy mesh with per-material colors and axis face lighting.
- Free-fly camera, headless/window PNG capture, finite window runs, frame statistics.
- One background edit remesh at a time, matching-revision-only upload, old mesh
  retained until replacement. One entire small arena is rebuilt per edit; this
  is a reference mechanism, not the production chunk update implementation.
- Bounded cache/scheduler policy tests separate content validity from view demand.
  They are not yet the viewer's streaming implementation.

## Verification

`cargo test --workspace --locked --offline`: **24 passed**, plus doc tests.
`cargo fmt --all -- --check`: passed. Release build passed.
Tests include 500 reference ray/box comparisons, mesh/voxel ray parity through
cave and edited bridge, large offsets, shader validation, async snapshot edits,
stale completion invalidation, bounded caches and frame statistics.

Hardware capture and 300-frame window run both completed successfully:

- CPU: AMD Ryzen 5 7640U; reported host RAM 15,654,044 KiB.
- GPU: AMD Radeon 760M Graphics (RADV PHOENIX), Vulkan.
- Driver: Mesa 25.2.8-0ubuntu0.24.04.2.
- Scene: source version 1, seed 7; static camera [11,9,-15] looking at [-1,3,1].
- Output/internal resolution: 1920x1080, native rendering, single sample, no TAA.
- Native arena: 16x14x16 m; 181,362 exposed native faces, 41,203 greedy quads,
  164,812 vertices, 82,406 triangles. No LOD pages or streaming jobs.
- Startup mesh generation: 0.72 s window run; 0.97 s capture run.
- 299 recorded presentation-inclusive frame intervals (FIFO/vsync):
  p50 16.666339 ms, p95 17.711174 ms, p99 19.944514 ms.
- 149 intervals exceeded 16.6667 ms; this strict count includes vsync jitter.
- Isolated GPU timing, RSS/VRAM peak and uncapped throughput: **not measured**.

This does **not** pass the production gate: no moving route, no warmup-separated
uncapped GPU measurements, no distant renderer or streaming, and p95 exceeds the
provisional 16.67 ms threshold. Software llvmpipe capture was separately checked
only for functionality. Frame statistics are not an FPS extrapolation to a large
world. Interactive edit selection and worker behavior have tests, but a complete
scripted human-input edit-and-flight session has not been measured.

Local ignored artifacts:
- `output/native-fixture-hardware.png` (inspected; cave and bridge visible).
- `output/hardware-capture.log` and `output/hardware-window.log`.
- `output/native-fixture.png` (software capture).

The finite crop is deliberately uncapped where its neighbor is solid. It can look
like a cutaway; that is not evidence of a hole-free hybrid renderer. Visible fine
stair-step aliasing remains; no anti-aliasing or distant filtered representation
has been implemented. No claim of final terrain beauty is made from this fixture.

## Next checkpoint

Use this exact native reference to evaluate minimal GPU clipmaps plus explicit
heightfield/volume ownership on the same mountain, cave, bridge and edit fixtures.
Prove hit depth, missing AND false geometry, subpixel transitions and laptop
budgets before adding production geology or committing to a renderer. Add GPU
timestamps and deterministic motion routes, then compare adaptive CDLOD and
volume traversal according to the existing gates. Persistence and walking
collision remain separate later work; current edits disappear on exit.
