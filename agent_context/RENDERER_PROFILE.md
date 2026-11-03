# Analytic versus voxel traversal profile

Adapter: llvmpipe (LLVM 20.1.2, 256 bits) (Vulkan, Cpu)
320 × 240 pixels; five fixed cameras; three modes; two independent runs of 62 frames. Values below are medians of two late-frame compute timestamps, not all-frame percentile statistics. Software GPU results are diagnostic, not a prediction for hardware GPUs. Headless compute excludes presentation.

| Camera | Continuous ms | Snapped ms | Exact voxels ms | Exact cell visits/ray |
|---|---:|---:|---:|---:|
| overview | 2.25 | 1.99 | 30.02 | 134.4 |
| near_oblique | 4.03 | 4.11 | 62.62 | 525.7 |
| near_horizontal | 3.42 | 4.85 | 65.82 | 540.4 |
| overhead | 3.39 | 4.20 | 58.17 | 448.3 |
| underside | 2.65 | 2.94 | 26.02 | 143.3 |

Exact mode explicitly calls `trace_voxels`, which classifies and traverses native cells across the ray interval. It does not call the closed-form patch solver. Continuous/snapped call `trace` instead. More traversed cells correlate with higher exact compute time. CPU encode samples remain below 0.08 ms here; coefficient storage is 15,360 B in all modes. No refitting or per-frame coefficient uploads occur.

Next experiment: conservative analytic empty-space rejection and bounded local voxel traversal, retaining the current DDA as the correctness reference. Avoid simply snapping one continuous hit: it cannot account for voxel side faces, changed silhouettes, grazing rays or patch discontinuities. Explicitly handle those cases in the acceleration proof and CPU comparisons.

Reproduce: `python3 tools/benchmark_renderer.py --size 320x240`. Raw captures and JSON: `output/analytic/profile/`. Interactive user-session history is not logged automatically; these are fresh controlled measurements.
