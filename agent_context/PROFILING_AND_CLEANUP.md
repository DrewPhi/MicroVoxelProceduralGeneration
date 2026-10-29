# Profiling and cleanup, 2026-10-29

Measured on AMD Radeon 760M / RADV PHOENIX Vulkan. This pass preserves generation,
occupancy, colors, player collision and renderer selection. No new dependencies.

## Retained changes

- Terrain uniforms retain fixed grid origins. The vertex shader subtracts the
  shared camera position, avoiding per-frame writes to every visible mesh part.
  Moving actors still update their origins. Uniform size remains 32 bytes;
  conforming geometry and alternate-resolution previews use the same path.
- Ray stack entries retain already-tested node entry distances. Missed children
  are never queued; a nearer hit rejects later nodes without another bounds test.
  Ordering and hit/color semantics remain unchanged. Stack size increases from
  128 to 256 bytes per ray; measured performance justified retaining it.
- Coarse ray generation skips four neighboring profiles that cannot affect its
  colors. Native side colors still inspect neighbors. Combined timings below do
  not isolate this change's individual contribution.
- Worker shutdown cancels in-flight terrain builds at existing cancellation checks.
- Streaming diagnostics separate CPU update, draw submission and GPU waiting,
  save every tick to CSV and sample process RSS. The benchmark and HUD now include
  long stalls rather than silently dropping them.
- Offline native benchmarks and representation experiments are grouped under
  `native/src/diagnostics/`. Maintained paths and commands are in the developer guide.

The finite world spans 160,000 grid cells, which fit exactly in GPU `f32` values.
Power-of-two cell sizes preserve grid origins; integer CPU state is unchanged.
Future worlds beyond the exact integer range of GPU floats need region-relative
origins. This does not promise arbitrary-distance floating-point precision.

## Matched 1080p captures

Seed 42, quality 75, native radius 6 m, 256 MiB terrain budget, 768-tile ceiling,
normal effects, no HUD. Each value averages 60 synchronized offscreen frames;
run 2 repeats the before/after pair. Covers and visible tile counts match.

| Renderer / scene | Before 1 | After 1 | Before 2 | After 2 |
| --- | ---: | ---: | ---: | ---: |
| Mesh / valley | 5.90 ms | 5.10 ms | 4.83 ms | 4.11 ms |
| Mesh / ground | 6.73 ms | 4.97 ms | 6.66 ms | 5.93 ms |
| Ray / valley | 15.24 ms | 14.13 ms | 15.17 ms | 14.05 ms |
| Ray / ground | 18.94 ms | 17.63 ms | 19.00 ms | 17.70 ms |

Measured frame-time reductions: mesh 11–26%; ray about 7%. Mesh timing varies
substantially between runs. These include CPU work and GPU synchronization, not
isolated GPU shader time, and do not establish universal speedups. All four run-1
before/after PNGs are pixel-identical, including the actor in ground captures.
Fixed-cover resident payload is unchanged: mesh 15.17/16.86 MiB, ray 84.51/96.65 MiB.

## Movement

Each case runs 240 walking, 240 sprinting and 120 stationary turning ticks at
60 fixed ticks/s, paced when possible. Resolution 960×540; tile ceiling 512.
Laptop: quality 45/radius 4 m/96 MiB. Balanced: quality 75/radius 6 m/256 MiB.
The route travels 37.10 m. Measured work excludes diagnostic collision/detail
checks, CSV/RSS collection and pacing sleeps.

| Case | Before p50 / p95 / p99 ms | After p50 / p95 / p99 ms |
| --- | --- | --- |
| Mesh / laptop | 5.91 / 9.89 / 14.55 | 5.15 / 9.31 / 13.89 |
| Mesh / balanced | 6.60 / 11.73 / 16.12 | 5.51 / 10.82 / 16.32 |
| Ray / laptop | 13.94 / 22.63 / 25.68 | 13.87 / 21.97 / 25.25 |
| Ray / balanced | 13.96 / 23.81 / 31.07 | 13.75 / 24.15 / 27.04 |

Every case before/after: zero native-detail misses at nine samples within 3 m;
no collision errors; both live backend switches settle. Sampling does not prove
complete native coverage at every position or speed. Ray movement improves little,
and balanced-ray p95 does not improve. This is not consistent 60 FPS during streaming.
Generation, poses, changing covers and transition passes remain possible costs.

Before, typical CPU update medians were 0.65–0.95 ms; mesh CPU submission medians
were 3.74–4.32 ms, while ray GPU-wait medians were about 11.75 ms. GPU wait is
synchronization time, not isolated hardware GPU execution time. This motivated
targeting submission and traversal separately rather than replacing world storage.

Sampled process RSS reached about 397 MiB before and 405 MiB after across each
four-case process. RSS includes earlier cases, caches and some driver mappings,
does not include all GPU allocations, and misses short transient peaks. Memory is
broadly unchanged. Ray arena commitment remains 96/128 MiB for the two cases.

## Reproduce and validate

```sh
cargo build --locked --release --manifest-path native/Cargo.toml
./tools/profile.sh output/performance
./tools/capture_cover.sh
```

The profile script runs movement and fixed captures serially. Optional
`MICROVOXEL_GAME=/absolute/path/to/baseline` selects another executable;
`MICROVOXEL_FRAMES=120` increases still samples. Use an otherwise idle GPU.
Reports/CSVs/PNGs remain ignored under `output/`. Recorded local runs are in
`output/performance_{before,after}` and `output/performance_{before,after}_repeat`.

The cover is an actual 2560×1440 native mesh frame, quality 100, native radius
12 m, 512 MiB budget and 4096-tile ceiling. Achieved sampling was 4.77 px with
3733 tiles, 102.70 MiB terrain payload. Maximum requested quality still respects
the finite planning budget. These settings differ from the performance comparison.

## References

- [PBRT 4e BVH chapter](https://www.pbr-book.org/4ed/Primitives_and_Intersection_Acceleration/Bounding_Volume_Hierarchies)
  and [source](https://github.com/mmp/pbrt-v4/blob/master/src/pbrt/cpu/aggregates.cpp):
  near-first traversal and closest-hit pruning.
- [Embree BVH traversal](https://github.com/RenderKit/embree/blob/master/kernels/bvh/bvh_traverser1.h):
  child-hit filtering and node/distance stack entries. Independently implemented
  in WGSL; no Embree code imported.
- [wgpu 24.0.5 Queue source](https://github.com/gfx-rs/wgpu/blob/v24.0.5/wgpu/src/api/queue.rs):
  reviewed staging/submit behavior of small buffer writes. Static terrain uniforms
  avoid those writes rather than changing their API.

SVO/DAG results remain in [the representation study](OCTREE_DAG_STUDY.md).
Production ray casting still uses columns; mesh remains the default renderer.

Validation: 75 core tests, 13 native CPU/shader tests and all three explicit GPU
regressions pass. Both Cargo formatting checks, shell syntax checks, Markdown
local-link checks and the dependency-free Rust 1.75 build pass. Generated reports,
settings, launchers and intermediate screenshots are excluded from the commit.
