# Issue #74: GPU methods and landscape references

Research audit, 2026-11-06. Scope: read-only external implementation study and
proposed bounded experiments; no renderer promotion, external code transplant,
Rust change, build, or GPU run. Reuse tier **R** for all sources below; a later
implementation is greenfield unless separately justified under `../AGENTS.md`.
`../ARCHITECTURE.md` and `../PRIMARY_VISUAL_CHECK.md` remain authoritative.

**Recommendation:** first determine whether submission or GPU execution limits
the current raster path. Compare batching against its existing render bundles
on unchanged geometry. CUDA rasterizers are useful algorithm references but
do not establish a Radeon 760M win. Establish reference-quality still images
before transition polish; defer permanent backend decisions to matched gates.

## CuRast: dense opaque tiny triangles

- Primary sources: [paper, DOI 10.1111/cgf.70538](https://doi.org/10.1111/cgf.70538),
  [author publication page](https://www.cg.tuwien.ac.at/research/publications/2026/SCHUETZ-2026-CURAST/),
  [repository](https://github.com/m-schuetz/CuRast). Inspected repository tree at
  `55c9a26b2563ee32c5b84480fe112a4d7e38d37b`, especially
  [triangles_visbuffer.cu](https://github.com/m-schuetz/CuRast/blob/55c9a26b2563ee32c5b84480fe112a4d7e38d37b/src/kernels/triangles_visbuffer.cu).
- Algorithm: small triangles handled per thread; larger/near-plane cases enter
  queues, then warp and tiled stages. Code packs depth/primitive identity into
  64-bit values and uses atomic minimum for visibility. This can avoid wasting
  hardware rasterization work on extremely small dense primitives.
- Limitations: paper studies dense opaque triangle datasets on NVIDIA RTX
  hardware; README explicitly warns about many small meshes and low-density
  scenes. Local edits are possible input changes, but this is not a canonical
  voxel generator, persistent edit hierarchy, collision system, or streaming
  solution. Queue buffers, visibility targets and resolve bandwidth must count
  toward memory. Source materials and lighting still need correct resolution.
- Portability: CUDA/cooperative groups/warp assumptions and 64-bit atomics make
  a WGSL translation substantial research. Test actual optional wgpu shader and
  atomic support before planning it. NVIDIA timings predict no integrated-GPU
  gain; larger primitives and queue overflow are essential counterexamples.
- License: [LICENSE.md](https://github.com/m-schuetz/CuRast/blob/55c9a26b2563ee32c5b84480fe112a4d7e38d37b/LICENSE.md)
  defaults project code to AGPL v3 with file/section exceptions and separate
  third-party library licenses. No code copied here.
- Minimal experiment: only after submission work, freeze one opaque terrain
  batch and compare hardware raster against an independently written tiny-
  triangle visibility probe with large-triangle fallback. Include near-plane
  clipping, dense distant geometry and large near faces. Win only if total
  visibility+resolve+queue cost improves by at least 10% on the laptop without
  coverage/depth/material regression; stop on absent capabilities, unbounded
  queues, or a two-day probe with no whole-frame prospect. This is a proposed
  screen, not an acceptance result.

## SVRaster: adaptive radiance-field voxels

- Primary sources: [CVPR 2025 paper](https://openaccess.thecvf.com/content/CVPR2025/html/Sun_Sparse_Voxels_Rasterization_Real-time_High-fidelity_Radiance_Field_Rendering_CVPR_2025_paper.html),
  [repository](https://github.com/NVlabs/svraster). Inspected tree
  `3503131b5558f5e5eaaab01881129a5f3bd79ad3`,
  [preprocess.cu](https://github.com/NVlabs/svraster/blob/3503131b5558f5e5eaaab01881129a5f3bd79ad3/cuda/src/preprocess.cu)
  and [forward.cu](https://github.com/NVlabs/svraster/blob/3503131b5558f5e5eaaab01881129a5f3bd79ad3/cuda/src/forward.cu).
- Algorithm: adaptive sparse voxels, projected tile bounds, ray-direction-aware
  ordering, tiled shared-memory loading and front-to-back integration. Forward
  code evaluates trilinear density samples along voxel intersections and
  accumulates transmittance/color. The trained representation supports novel
  view synthesis; its voxel grid is not exact binary game occupancy.
- Potential lesson: spatial work grouping and coherent loading. Density
  interpolation, opacity termination and learned appearance cannot replace
  exact 6.25 cm material/air, cave openings, collision, or deterministic edits.
  Image-reconstruction PSNR does not certify silhouette or edit fidelity.
  Tile/quadrant duplication and sorting scratch can amplify memory. CUDA,
  PyTorch, CUB and shared-memory assumptions require independent wgpu work.
- License: [NVIDIA LICENSE](https://github.com/NVlabs/svraster/blob/3503131b5558f5e5eaaab01881129a5f3bd79ad3/LICENSE)
  limits the Work and derivatives to research/evaluation, with incorporated
  [Inria conditions](https://github.com/NVlabs/svraster/blob/3503131b5558f5e5eaaab01881129a5f3bd79ad3/LICENSE_inria.md).
  This is not a permissively licensed production renderer dependency.
- Minimal experiment: compare Morton/spatial ordering versus current ordering
  for the **same exact** brick/cluster data and opaque first-hit semantics;
  cap scratch before execution. Retain only if total ordering+render cost falls
  at least 10%, memory fits, and oracle depth/material is unchanged. Stop if
  sorting/duplication costs erase traversal savings. Do not build a training
  pipeline for this question.

## AMD RDNA guide and current submission path

- [AMD primary guide](https://gpuopen.com/learn/rdna-performance-guide/) supports
  reducing state changes, measuring memory/occupancy, and evaluating indirect
  argument compaction and vertex pulling. Its DX12 ExecuteIndirect/root-
  signature advice is conceptual guidance for wgpu, not an API prescription.
  Workgroup shape, register pressure, LDS and bandwidth need measurement;
  maximize useful work rather than treating occupancy alone as the objective.
  Guide/code examples are references, not an imported library or license grant.
- Actual local baseline: `renderer/src/adaptive_gpu.rs` already pools immutable
  allocations, instances native quads/proxy triangles and caches render bundles.
  Its `encode_draws!` loop still changes pipeline, dynamic uniform offset,
  vertex slice, sometimes profile binding/index slice, then issues per-cluster
  or per-native-bin draws. A multi-draw call cannot vary those bindings for each
  command. Group compatible pipeline/page/profile state; move cluster metadata
  into indexed storage or instance records before expecting broad batching.
- `renderer/Cargo.toml` pins **wgpu 24.0.5**. `renderer/src/lib.rs` currently
  requests only supported `TIMESTAMP_QUERY`, with default limits. Adapter
  support alone does not enable optional device features. The pinned
  [Features documentation](https://docs.rs/wgpu/24.0.5/wgpu/struct.Features.html)
  specifies `MULTI_DRAW_INDIRECT`, `MULTI_DRAW_INDIRECT_COUNT`, and
  `INDIRECT_FIRST_INSTANCE` for their respective uses; count-buffer draws are
  native-only. Multi-draw is native on Vulkan/DX12 and can be emulated elsewhere.
  Nonzero indirect first-instance requires its feature. Basic indirect draws
  also require checking downlevel indirect execution support, buffer `INDIRECT`
  usage, command alignment/stride and buffer bounds. GPU-generated arguments
  additionally need storage usage and valid pass ordering. Query/request limits
  for storage binding sizes/counts, buffer sizes and compute dispatch; do not
  assume the current 512 MiB aggregate arena allowance is a valid single binding.
- Portable first probe: preserve direct/bundle drawing; group compatible native
  and proxy instances, explicitly preserving cluster origins, profile lookup,
  ownership and visible-bin masks. Then optionally compare indirect forms on
  supported devices. GPU culling comes later: near-plane/camera-cut uncertainty
  must remain visible; prior-frame occlusion cannot establish current coverage.
- Minimal A/B: same warmed 300 m scene, then bounded 1 km **only after** the
  small case; direct/bundle/batched submission with identical selected owners.
  Measure CPU encoding, GPU timestamps, draw/state counts, upload bytes and
  p95/p99 whole-frame cost. Win: at least 10% whole-frame p95 improvement or
  0.5 ms CPU submission saving with no frame-tail, memory or image regression.
  Stop if submission is below 5% of total cost or GPU traffic consumes savings.
  Shared CPU/GPU bandwidth and power on the laptop require cold preparation as
  well as settled checks; a discrete CUDA card cannot substitute for this test.

## Geometry clipmaps and CDLOD: bounded terrain-only candidates

- [Losasso/Hoppe project](https://hhoppe.com/proj/geomclipmap/) and
  [GPU Gems 2 implementation chapter](https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry):
  nested grids, incremental toroidal updates, reusable topology and filtered
  elevation levels reduce camera-driven mesh rebuilding. Memory scales with
  level count times fixed grid area plus page/update storage. Movement still
  needs newly exposed data; teleport requires a cold destination strategy.
  Historical dataset/FPS numbers are not current laptop evidence. Publication
  and downloadable sample code are distinct artifacts; sample-code licensing
  was not audited, so implement concepts independently.
- [CDLOD repository](https://github.com/fstrugar/CDLOD), inspected at
  `ea8195b335c1d542627a6af048e44326d75cb1ef`, especially
  [CDLODQuadTree.cpp](https://github.com/fstrugar/CDLOD/blob/ea8195b335c1d542627a6af048e44326d75cb1ef/source/BasicCDLOD/CDLODQuadTree.cpp):
  min/max-height quadtree bounds, 3D distance selection, partial child coverage
  and morph ranges select reusable terrain grids. DirectX9 code is not a wgpu
  port. Distance rings do not prove screen-space geometric error. Root
  [LICENSE](https://github.com/fstrugar/CDLOD/blob/ea8195b335c1d542627a6af048e44326d75cb1ef/LICENSE)
  is MIT; inspected source header instead names zlib/readme. Resolve per-file
  provenance before any selective port; this audit copies no code.
- Both are display structures for single-valued terrain, not 3D occupancy.
  Cave/bridge/tower/edit volumes need explicit ownership and conservative
  representation. Height edits invalidate levels and normals; digging must not
  resurrect a cave cap. Keep canonical lighting/material and physics separate
  from displayed height resolution. Shared anchors and morphs help continuity,
  but cannot hide a large geometric mismatch against native stepped voxels.
- Minimal experiment: same precomputed canonical heightfield-safe pages, small
  clipmap versus adaptive grid; separately measure page preparation so neither
  gets free source work. Fixed player-height view first, then short translation
  across a ring boundary. Win: reference-quality silhouettes/materials with at
  least 10% combined selection+update+draw improvement within equal memory.
  Stop immediately on visible cave/tower ownership loss; defer 4/20 km and
  transition polish until bounded still quality and costs pass. This does not
  reopen architecture gates or require a second permanent backend.

## Community references: checked versus unverified

- [PlanetSmith devlog LzivS_KzffY](https://www.youtube.com/watch?v=LzivS_KzffY):
  video fetch unavailable and exact-ID search yielded no useful result in this
  audit. **Unverified content**: no inferred algorithm, hardware, FPS or license.
- [Distant moiré thread](https://www.reddit.com/r/VoxelGameDev/comments/1vtnemb/has_anyone_dealt_with_distant_voxels_moire_effect/):
  accessible author report describes projected LOD coarseness plus depth-masked
  normal blur. Useful counterexample: anti-aliasing/shading stability is separate
  from geometric error. Blur is not adopted as a coverage fix.
- [Microvoxel view-distance thread](https://www.reddit.com/r/VoxelGameDev/comments/1vrqluv/improving_rendering_distance_in_my_micro_voxel/):
  accessible author report uses direct coarse generation, independently cached
  macro edits and adaptive fog while loading. Reported M1 Pro FPS is unpaired;
  fog-driven temporary range reduction conflicts with the fixed no-haze gate.
- [Chunkee update](https://www.reddit.com/r/VoxelGameDev/comments/1qicz42/update_5_chunkee_lods_are_back/):
  accessible report describes stable world groups and retaining a parent until
  all eight children are ready. Valuable publication invariant; reported chunk
  reduction is not measured frame speedup. Linked implementation/license was
  not audited here. All community performance claims remain unverified.

## Common execution contract for later experiments

Freeze binary/commit, seed/source version, camera/FOV/route, 1080p output/internal
resolution, MSAA, far range and quality/cache/worker settings. Start with the
approved Legacy seed-7 300 m oracle and retain the Regional visual target as a
separate comparison. Log first coverage, requested quality and reserve separately;
never count fast coarse fallback as strict-quality success. Run randomized A/B
order, at least three warm repeats, and separately report cold runs, GPU/driver,
CPU/GPU p50/p95/p99, resident/transient memory and edits/invalidations. Proposed
10% screens above must exceed observed run variation; otherwise record
inconclusive. Reject missing topology/material, stale publication or changed
canonical occupancy regardless of timing. These were research proposals at initial publication. Follow the measured
[issue74 evidence](../reference/issue74/README.md) for executed experiments and
keep/reject decisions; untested proposals remain unproven.
