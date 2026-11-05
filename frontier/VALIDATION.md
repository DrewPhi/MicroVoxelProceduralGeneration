# First checkpoint validation contract

This is a greenfield implementation (reuse tier G). No legacy runtime, scheduler,
renderer or generator is copied. The first bounded native voxel fixture is a
correctness reference, not the production terrain renderer or a passed G1/G3 gate.

## Required reference cases

- Half-open integer cells at 6.25 cm, negative coordinates and 100 km offsets.
- Canonical materials and immutable snapshots with explicit edit precedence.
- Ray entry faces, tied crossings, zero/invalid rays and independent box checks.
- Cave air, bridge voids and edits must come from the same world query as drawing.
- Camera demand changes preserve valid cache entries, but obsolete selections and
  edited content cannot publish. Test stop/resume and away/return behavior.
- Capture a fixed camera through the same graphics pipeline as the viewer.

## Performance gate, before production comparisons

Target hardware is the user's laptop, detected rather than assumed. Record CPU,
GPU, driver, RAM, output/internal resolution, scene seed/version, route and settings.
Use 1920x1080 as the baseline; disclose any lower resolution separately.

Provisional acceptance thresholds for the later production profile:

- Whole-frame p95 <= 16.67 ms and p99 <= 20 ms, missed 16.67 ms deadlines <= 5%,
  measured on an uncapped route after warmup; additionally verify paced 60 Hz play.
- Report CPU and GPU separately; unavailable timings are null, never zero.
- Reserve runtime/gameplay headroom; do not allocate the full budget to terrain.
- Zero incorrect hit/ownership results in deterministic reference fixtures.
- Report silhouette error against a native oracle at 0.5, 1 and 2 pixel settings;
  no guarantee is claimed from projected cell size or empirical samples alone.
- Show raw and filtered stationary/grazing/movement captures. Visible transition
  popping or loss of resolvable voxel identity fails regardless of FPS.
- Walk-stop-wait-resume, turn-return, warm/cold revisit, edits and teleport readiness
  must report latency, frame tails, residency and wasted work.

These are proposed measurable gates, not achieved performance. The foundation
fixture alone cannot establish horizon quality, streaming, or production LOD.
Software Vulkan rendering validates functionality only, not laptop GPU speed.

## Algorithm references

- Amanatides and Woo, voxel traversal (1987):
  https://www.eecs.yorku.ca/~amana/research/grid.pdf
- Pinned wgpu API used by the first viewer:
  https://docs.rs/wgpu/24.0.5/wgpu/
- Later reusable geometry baseline, not implemented by the reference voxel mesh:
  https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry
