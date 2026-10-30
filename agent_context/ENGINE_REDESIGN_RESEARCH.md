# Redesign research audit

2026-10-29. Companion to the [implementation plan](ENGINE_REDESIGN_IMPLEMENTATION_PLAN.md). Sources were inspected for decisions, not run as competing engines. No external benchmark numbers are predictions for our hardware. No external source was copied into production code in this pass.

## Findings that change the plan

### Native meshing: an existing Rust reference, with integration costs

Read the C++ [binary mesher implementation](https://github.com/cgerikj/binary-greedy-meshing/blob/master/src/mesher.h), the Rust [port implementation](https://github.com/Inspirateur/binary-greedy-meshing/blob/main/src/lib.rs), and its [manifest](https://github.com/Inspirateur/binary-greedy-meshing/blob/main/Cargo.toml). The inspected C++ uses padded 64-wide inputs with 62-cell interiors, occupancy masks, direction-specific face masks, and compact quad records. The Rust port offers reusable mesher buffers and an accelerated mask path. Its manifest declares MIT, edition 2024, and no runtime dependencies.

**Decision:** use this as the principal bitmask reference, not an unquestioned dependency. Our 32³ storage, exact appearance, unknown cells, halo snapshots and wgpu record format differ. The Rust edition also matters: the old core declares Rust 1.75, the native app 1.85. Set and test a successor toolchain explicitly. Benchmark source sampling and appearance construction alongside meshing.

Also read [`block-mesh-rs/greedy.rs`](https://github.com/bonsairobo/block-mesh-rs/blob/main/src/greedy.rs): useful reusable output/scratch, merge-value abstraction and padded-neighbor semantics. Use it as an independent comparison/reference, not another permanent production renderer. Its transparency behavior is not a universal material model.

### Clipmaps: useful coherence, not universal 3D

[Losasso/Hoppe](https://hhoppe.com/proj/geomclipmap/) describes incrementally updated nested grids. The [GPU Gems implementation chapter](https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry) provides toroidal updates, constant grid topology, filtered levels and local transitions.

**Decision:** choose this for far heightfield terrain. Preserve native blocks near the player, and prototype their transition separately. Do not claim the paper solves caves, independent procedural filtering, or edited holes. Disjoint replacement ownership is our required integration work. PBRT's [sampling discussion](https://www.pbr-book.org/4ed/Sampling_and_Reconstruction/Sampling_Theory) supports filtering before reduction; independently sampling increasingly coarse noise is not sufficient antialiasing.

### MishMash: adopt selected ideas and reject one deliberate tradeoff

Read [MGMishMash's own development post and replies](https://www.reddit.com/r/VoxelGameDev/comments/1vrqluv/improving_rendering_distance_in_my_micro_voxel/). It describes mesh rendering, direct coarse generation including features, independently cached macro edits, temporary transitions, and adaptive fog while detail streams. The finite regional map and author-reported hardware timings are not evidence of an infinite editable microvoxel planet. The 25-level/16-quad meshlet reply is by another participant.

**Decision:** retain direct coarse generation and independent far edit caches. Budget near work first while reserving horizon coverage. Intentionally do not copy adaptive horizon retreat; the proposed experience requires stable distant geography. No public implementation was available to verify its internals in this audit.

### GPU allocation and submission

Read [Project Ascendant's geometry write-up](https://vkguide.dev/docs/ascendant/ascendant_geometry/): shared buffer suballocation and compact draw metadata are useful even without adopting its smoothing or its entire hybrid renderer. **Decision:** shared arenas, bounded uploads, stable handles and deferred reclamation first; indirect draw experiments afterward. Its desktop pool size is not our laptop budget.

Check the actual [wgpu 24.0.5 feature documentation](https://docs.rs/wgpu/24.0.5/wgpu/struct.Features.html), matching the repository dependency. Multi-draw, indirect count and first-instance support have distinct requirements; some multi-draw implementations are emulated. **Decision:** a direct pooled draw fallback, no mandatory mesh shaders/bindless/hardware RT, no silent dependence on latest wgpu features.

Read [VoxelCore's chunk renderer](https://github.com/MihailRis/voxelcore/blob/main/src/graphics/render/ChunksRenderer.cpp). It shows pooled worker inputs, bounded per-frame enqueueing, near-priority work, and queue clearing. It also uses blocking near builds and per-mesh drawing. **Decision:** borrow the scheduling lessons, not the whole renderer; do not introduce blocking terrain builds on our frame thread. Treat MPL-2.0 source reuse separately from an independent Rust implementation and retain applicable notices if code is adapted.

### Cubiquity: selective storage/traversal reference

Read its [README](https://github.com/DavidWilliams81/cubiquity), [`storage.h`](https://github.com/DavidWilliams81/cubiquity/blob/master/src/library/storage.h), and the relevant traversal sections of [`raytracing.cpp`](https://github.com/DavidWilliams81/cubiquity/blob/master/src/library/raytracing.cpp). Source exposes shared/unshared nodes, root history, baking, and localized sub-DAG traversal. The header's material-width prose and actual typedef disagree, a useful warning against copying commentary without checking code. A coarse material-selection helper explicitly approximates intersection.

**Decision:** keep mutable overrides; investigate frozen local DAGs only after an end-to-end benefit appears. Do not import a global 32-level root, material packing, or approximate visual queries into authoritative physics. CC0 makes reference reuse straightforward, but not automatically suitable. A beautiful experimental/path-traced image is not a benchmark against our moving procedural scene.

### Aokana: strong large-scene reference, not the first subsystem

Read the [paper's HTML](https://arxiv.org/html/2505.02017v1), including its framework discussion. It couples SVDAG storage with GPU-driven rendering, streaming and LOD. **Decision:** use its separation of streamed representation and GPU work as a design reference; postpone a dedicated implementation. Its reported comparative speed/memory gains do not establish interactive edit costs or wins over our packed raster on a 760M. Do not infer an editable-world solution from static-scene compression.

### NAADF: broaden the ray experiment beyond octrees/DAGs

Read the [2026 project page](https://www.cg.tuwien.ac.at/research/publications/2026/ulschmid-2026-naadf/), [repository README](https://github.com/cg-tuwien/NAADF), and [`rayTracing.fxh`](https://github.com/cg-tuwien/NAADF/blob/main/NAADF/Content/shaders/render/rayTracing.fxh). The implementation uses nested blocks/chunks and directional empty-space distances; its source also has explicit ray step limits and a separate transformed-entity path. The released application uses a compute-capable MonoGame fork and supports Windows; it is not a portable Rust drop-in. Repository declares MIT.

**Decision:** promote empty-space skipping to a small later traversal experiment, rather than treating this solely as future GI literature. First compare plain brick DDA; then measure acceleration build/invalidation cost as well as traversal. Do not copy step-limit handling as exact picking semantics or adopt its off-grid entity behavior into our grid universe. GI, temporal reservoirs and extended history remain out of scope.

### Sources useful as boundaries, not chosen implementations

- [Transvoxel](https://transvoxel.org/) solves transitions between smooth isosurfaces. It is not an ordinary block-face seam algorithm. Do not add it to the initial native mesher.
- [Voxel Tools smooth terrain documentation](https://voxel-tools.readthedocs.io/en/latest/smooth_terrain/) describes a different smooth/SDF LOD path. Useful constraints and transitions, not evidence that its blocky mode solves our full problem.
- [NULLSEED diary](https://nullseedgame.com/) reports meshlets, visibility and hybrid techniques. No inspected open-source implementation or matched laptop benchmark; use as visual inspiration, not an algorithm/performance proof. Its page is a moving, partly undated diary, so do not treat every entry as a pinned cutoff-date result.
- [Terrain Amplification using Multi-Scale Erosion](https://h-schott.github.io/p/mserosion/) supplies a useful later terrain-quality direction. Keep generation fixed during rendering comparisons. Do not add numerical erosion to the frame loop.

## Research limits and follow-up

The original document is a reference library, not a requirement to implement every entry. This pass prioritizes primary material for the chosen architecture, inspects representative implementation paths, and does not claim exhaustive source audits or reproduction of research results.

The analytical-erosion Inria page timed out; the Eurographics GPU-editing page returned 403. A guessed GPU-SVDAG repository branch also failed. Neither was used as verified implementation evidence here. A search surfaced a 2026 clipmap development diary but direct access failed; it is not a basis for our decisions. Proprietary Enshrouded notes and extra geographic datasets were not needed for this foundation choice. Full SVO/DAG/HashDAG reproduction remains an experiment-stage task, not completed research validation.

Before incorporating external code, pin an exact commit and record file hashes, license and modifications in a third-party manifest. URLs above identify inspected branch paths; they are not immutable source snapshots. Check commits against the intended cutoff. Preserve attribution for adapted MIT code. Current research establishes algorithm candidates and risks; it does not establish their performance in our engine.
