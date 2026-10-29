# Sparse octree and DAG study

Measured 2026-10-29 on AMD Radeon 760M, RADV PHOENIX/Vulkan. This is an
opt-in experiment, not a replacement for the selectable column ray renderer.
The editable procedural world, saves, collision and default renderer are unchanged.

## Reproduce and inspect

From the repository root, with a working graphics device:

```sh
cargo build --locked --release --manifest-path native/Cargo.toml
./native/target/release/microvoxel-game --ray-study
```

Use the default seed 42 and climate, without an edit save, to reproduce these sites.
The command exits after writing `output/ray_study/report.txt` and `metrics.csv`.
Generated reports remain ignored. Study code is in `native/src/diagnostics/ray_study.rs`,
`ray_study_gpu.rs` and `ray_study.wgsl`; topology/storage code is in
`src/render/octree.rs`.

## What was implemented

1. A fixed-cover whole-frame profile separates sky/setup, ray geometry/depth and
   full shading. This uses the existing renderer, not the experimental kernel.
2. A compact binary sparse voxel octree (SVO) with empty/solid leaves and masked,
   packed child lists. Construction classifies vertical intervals rather than
   expanding buried terrain into a dense volume.
3. Exact bottom-up subtree interning into an immutable DAG. Geometry is independent
   of attributes. A shared-pool probe measures internal reuse across 16 bricks;
   it does not substitute whole-brick fingerprints for subtree equality.
4. A hybrid lifecycle prototype: edited regions use ordinary column snapshots;
   idle regions may freeze into a DAG. Old `Arc` snapshots remain unchanged.

The compute benchmark sends the same 4,096 mixed rays through columns, SVO and
DAG, both with and without native color lookup. Each variant runs 35 times,
discards five warmups and reports median/p95 of 30 hardware timestamp samples.
Every alternate result is checked for identical hit/cell/color against the column
reference. A subset also checks the CPU tracer. Core tests exhaustively compare
occupancy in a small cave-containing volume and check snapshot isolation.

The alternative representations keep the existing run/color attributes, with an
additional 8 KiB direct column table replacing BVH-based attribute lookup. Their
total memory figures include this overhead. Build timings are additional
conversion from an already-built column snapshot, not direct-generation timings.

## Results

Hardware GPU times below include instrumented counters and output writes for
4,096 rays, not full-screen rendering. Memory is uploaded representation payload,
excluding allocator capacity, build temporaries, rays/results and driver storage.
Both timing variants bind complete attribute buffers; disabling color lookup does
not remove those allocations. Attribute-enabled results:

| Scene | Structure | Median ms | p95 ms | Geometry KiB | Attributes KiB | Total KiB |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Flat | Columns | 0.094660 | 0.097145 | 63.97 | 108.00 | 171.97 |
| Flat | SVO | 0.087807 | 0.090211 | 14.71 | 116.00 | 130.71 |
| Flat | DAG | 0.087847 | 0.089410 | 0.30 | 116.00 | 116.30 |
| Terrain A (`grass`) | Columns | 0.113496 | 0.114618 | 63.97 | 108.00 | 171.97 |
| Terrain A | SVO | 0.169322 | 0.171887 | 7.65 | 116.00 | 123.65 |
| Terrain A | DAG | 0.168841 | 0.170324 | 2.32 | 116.00 | 118.32 |
| Terrain B (`cliff`) | Columns | 0.100070 | 0.102555 | 63.97 | 36.14 | 100.11 |
| Terrain B | SVO | 0.185953 | 0.188358 | 14.56 | 44.14 | 58.70 |
| Terrain B | DAG | 0.186314 | 0.189199 | 4.29 | 44.14 | 48.43 |
| Carved cave | Columns | 0.353712 | 0.358321 | 63.97 | 136.00 | 199.97 |
| Carved cave | SVO | 0.101994 | 0.104398 | 0.38 | 144.00 | 144.38 |
| Carved cave | DAG | 0.101833 | 0.103917 | 0.25 | 144.00 | 144.25 |
| Synthetic checkerboard | Columns | 0.445166 | 0.450416 | 63.97 | 2304.00 | 2367.97 |
| Synthetic checkerboard | SVO | 0.166556 | 0.173369 | 475.45 | 2312.00 | 2787.45 |
| Synthetic checkerboard | DAG | 0.164152 | 0.167799 | 0.21 | 2312.00 | 2312.21 |

Terrain A starts at X=4250 m, Z=6250 m; terrain B at X=1250 m, Z=8750 m.
Each native brick is only 2×2 m horizontally. The names identify selected sites,
not independently verified biome/slope categories. Flat extends to Y=30,000
cells; the cave and checkerboard extend to 128 cells. Checkerboard deliberately
maximizes repetition and is not representative generated terrain.

For the two real terrain bricks, DAG total storage is **31% and 52% smaller than
columns**. However, most of that saving already comes from replacing the column
BVH with an SVO: DAG adds only **4.3% and 17.5% savings over SVO total payload**.
The terrain kernels are **1.49× and 1.86× slower than columns**. DAG versus SVO
times differ by less than 0.4%; these measurements show no meaningful DAG speedup.
The carved-cave DAG is 3.47× faster than columns, but that is mostly the octree
traversal benefit; SVO performs similarly. This does not establish a universal
terrain or cave speedup.

Across 16 adjacent native terrain bricks: 5,399 SVO internal nodes become 1,433
nodes with independent per-brick DAGs, or 801 nodes in one shared pool. Thus
cross-brick sharing removes another 44% of already-deduplicated internal nodes.
Attributes still occupy 1,900,544 bytes. Node counts are not total memory savings;
this probe does not deduplicate color or count a full-world resident cache.

The existing full renderer at 1280×720, 618 fixed valley tiles, AA/fades off,
no actor, measured these synchronized CPU+GPU medians:

- Sky/setup only: 0.502 ms.
- Geometry/depth: 7.257 and 7.249 ms.
- Full shading: 8.604 and 7.500 ms in the enclosing passes.

Traversal dominates this scene, but variation between the two full passes prevents
a precise shading-cost subtraction. These are not isolated hardware GPU timings.
Logical word-load counters in the compute CSV count source-level references;
they do not measure DRAM traffic, bandwidth or cache misses.

The edit probe preserves an old immutable snapshot through 16 real sparse edits,
ordinary column rebuilds and re-freezing. Total times were 63.659 ms when freezing
only at the end and 58.591 ms when freezing after every edit; final idle freeze
was 0.145 ms in the former. These single, ordered samples include snapshot rebuilds
and show timing noise; they do not demonstrate cheaper edits or a concurrency,
compaction or persistence solution for a production mutable DAG.

Validation completed: 75 core tests, 12 native CPU/shader tests and all three
explicit native GPU regression tests passed. The complete study also passed its
hit/cell/color comparisons for all five scenes and all six variants per scene.
The dependency-free build also passes with the system Rust 1.75 compiler.

## Recommendation and limits

Keep columns as the live terrain renderer for now. The study supports a future
per-region choice between columns and octrees, especially for volumetric content,
rather than a universal DAG migration. Before integrating, compare uninstrumented
full-frame kernels on larger representative landscapes, caves and repeated assets.
The compact topology is promising for memory, but attribute packing is the larger
remaining target. Deduplication alone does not reduce the number of ray traversal
steps. This prototype also retains source columns and build structures on the CPU;
GPU payload savings are not a demonstrated reduction in overall process memory.
The ray set includes exterior views, misses and rays inside cave air; it does not
establish behavior for cameras embedded inside solid terrain or broad workloads.

Shared immutable geometry plus ordinary editable regions is feasible without
building a fully mutable DAG now. Production integration would still need bounded
pool ownership, reclamation, revision-aware invalidation and GPU residency updates.
Those mechanisms are deliberately outside this measurement experiment.

During validation, a native cave-ceiling color issue was fixed in both CPU and GPU
column tracing: run-bottom cells below the side-color interval now use their own
stored native color. This reuses an existing header word with no storage increase.

## Primary references

- Kämpe, Sintorn and Assarsson, [High Resolution Sparse Voxel DAGs (2013)](https://research.chalmers.se/en/publication/182658):
  repeated-subtree sharing; motivates topology interning, not an assumed speedup.
- Laine and Karras, [Efficient Sparse Voxel Octrees (2010)](https://research.nvidia.com/publication/2010-02_efficient-sparse-voxel-octrees):
  compact sparse topology and hierarchical traversal. Our first prototype uses an
  explicit small traversal stack rather than reproducing their entire renderer.
- [GPU-SVDAG-Editing](https://github.com/mathijs727/GPU-SVDAG-Editing):
  reference for the much broader engineering involved in dynamic DAG editing.
- Cubiquity source review and pinned revisions are recorded in
  [the renderer notes](RAY_RENDERER_AND_STREAMING.md). This study uses a custom
  Rust/WGSL implementation and does not import another engine or add dependencies.
