# Engine redesign: audited implementation plan

2026-10-29. **Planning only; successor not implemented.**

This refines the [proposed master plan](Frontier_Voxel_World_Engine_Master_Plan_2026-10-29.md) against the current local working tree. See [research decisions](ENGINE_REDESIGN_RESEARCH.md) for primary literature, inspected source, and limitations. These documents propose the next implementation; they do not override current behavior merely by existing.

## Decision

Build an isolated successor rendering/streaming pipeline and selectively strengthen the existing world core. Preserve the generator, canonical cell semantics, player, survey import, and regression fixtures. Replace the parts that couple tiny voxel tiles to work scheduling, GPU resources, and frequent scene replacement. Do not rebuild everything or make DAG storage mandatory.

The old implementation is a useful prototype with real architectural limits, not a wholly incorrect voxel engine. Other projects demonstrate better solutions to particular problems; none establishes that a single architecture wins our procedural, editable, laptop workload. Benefits below are hypotheses until measured on matching scenes.

The first deliverable is a small end-to-end vertical slice: **the same landscape and edited cave/tower, exact native cells, pooled raster rendering, persistent far terrain, and stable movement**. This must work before advanced compression, lighting, weather, or a new geology pipeline.

## 1. Audit corrections and remaining defects

Audit base: Git HEAD `6ddabe85c9a8b8b502049279bfcb2a69fe5b67a3` **plus substantial tracked and untracked local changes**. HEAD alone does not reproduce this audit. No new runtime measurements were made for this planning pass. Prior measurements below come from repository reports.

| Area | What is actually present | Decision |
|---|---|---|
| Scale | `src/spec.rs` has rational `GridScale`, configurable voxel size, and 64 m ownership pages. `grid.rs` still exposes default-grid convenience conversions. | Preserve scale implementation, remove ambiguous conversions from successor APIs, extend ownership to 3D. Do not reimplement scale from zero. |
| World Scale | `ProceduralTerrain::at_world_scale` compresses source landscape distances/heights independently of voxel size. | Preserve this feature and naming. The master plan incorrectly uses `WorldScale` for physical voxel edge. |
| Coordinates | Signed i64 cells and Euclidean chunk division already exist. Source and renderer still have finite domain/local packing assumptions. | Keep types; audit boundary arithmetic and introduce explicit source-domain and render-local coordinates. |
| Storage/edits | `storage.rs` has sparse/dense/uniform override bricks; `World::apply` validates batches and records neighbor invalidation. Air excavation is explicit. | Keep semantics; improve snapshots, bulk filling, revision granularity, and measured hot storage paths. |
| Saves | `persistence.rs` has versioned atomic edit snapshots with grid/source identity. | Retain reader/fixtures. Add journal and crash recovery; do not describe existing snapshots as a transaction journal. |
| Generation | `Generator` already separates exact queries, column/run hints, approximate display samples, summaries, and sparse volumetric features. | Adapt it, rather than discard it. Add bounded bulk sampling and a coherent visual hierarchy. |
| LOD | `render/lod.rs::Grid` computes extent/depth from `WorldSpec`; default constants are not the whole implementation. Still 32 display cells per tile and primarily horizontal organization. | Replace successor render layout, preserve old quadtree as benchmark competitor. Fixed-cell render granularity remains a scaling problem. |
| Meshing | Native `FaceCollector` uses ordered per-face maps; coarse path already emits constant-color rectangles and retains native Y heights. | Replace native collector after comparison. Do not claim all distant rendering expands native faces. |
| Appearance | `render/colors.rs` already losslessly packs constant, palette, and raw color tiles. | Reuse codec and parity tests. Solve repeated payload, allocation, and expensive construction before inventing a new color compressor. |
| GPU | `native/gpu.rs` allocates buffers/bindings per part and draws parts individually. | Rewrite resource ownership around shared arenas and compact draw records. |
| Streaming | Cancellation, local patches, priorities, and background planning exist. Runtime still shifts visibility target between 24,000 and 1,024 m according to near readiness. | Retain useful scheduling ideas/tests, replace residency lifecycle and remove near-readiness-driven horizon contraction in successor. |
| Transitions | Whole-image old/new presentation transitions remain. | Replace with local region transitions and shared parent/fine reconstruction. |
| Ray backend | Existing column/runs and SVO/DAG studies are useful comparators. | Keep experimental; add a simple 3D brick traversal candidate before adding more DAG machinery. |

The latest [terrain correction report](TERRAIN_CORRECTION_IMPLEMENTATION.md) records approximately 24–48% slower coarse builds and 2.4–8.8× payload growth in sampled patches after preserving native vertical detail. That is a concrete reason to change distant representation, not merely tune a slider. These are patch measurements, not whole-engine speed ratios. Its movement checks also show improvements already achieved; do not erase that evidence by calling every existing subsystem slow.

## 2. Contracts to settle before implementation

1. **Canonical world:** signed integer cells, fixed rational `VoxelSize`, explicit `Known(Air)`, `Known(Solid)`, and `Unknown/OutsideSource`. Unknown must never silently become empty. Exact collision/picking never consume visual LOD.
2. **Independent landscape compression:** retain menu label **World Scale**, internal name `LandscapeScale`. Source coordinates = game meters × scale; source heights / scale. Seed, both scales, generator version, material schema, and source identity belong in saves/cache keys. Player size and speed remain in game meters.
3. **Visual geometry:** propose accepting the master plan's fractional far proxies/morph vertices and optional visual camera interpolation. Native visible blocks retain exact occupancy and one base color per cell. This is an explicit relaxation of the current axis-aligned-display rule, not a consequence of using triangles. Keep snapped camera mode. If strict axis-aligned display remains required, use stepped proxies/local coverage transitions and accept more geometry/pop; do not promise smooth morphs under that constraint.
4. **Domain:** support safe sparse addressing, but initially retain the current finite 10 km source. Do not claim an Earth-sized planet or infinite watersheds. Synthetic sources exercise negative/large coordinates without pretending the current regional solver supplies them.
5. **Materials:** stable IDs and appearance keys; numerical physical properties stay unfilled. Simulation-specific state will be optional sparse channels, not extra mandatory bytes on every generated voxel.
6. **Determinism:** native queries must match across worker order and render settings on the supported build/platform. Cross-platform bitwise floating-point generation is a separate test/contract, not assumed from a seed. GPU appearance reconstruction cannot silently change native RGB.

## 3. What to keep, rewrite, retire, and defer

| Action | Components | Benefit / constraint |
|---|---|---|
| **Keep and wrap** | `generation/` landforms, climate, rivers, geomorphology, painting, survey source; `spec.rs`; material IDs | Same world for before/after comparisons; avoids mixing terrain changes with renderer changes. Existing river limitations remain separately visible. |
| **Keep semantics, strengthen implementation** | `grid.rs`, `world.rs`, `storage.rs`, `persistence.rs`, `queries.rs` | Preserve digging/building correctness while adding snapshots, journals, bulk access, and large-coordinate checks. |
| **Reuse behind adapters** | Player/character/physics, F5 modes, world menu/launcher, diagnostics, color packing | Preserve working user features without importing renderer assumptions into the new core. |
| **Rewrite in successor** | Native face extraction, GPU part allocation/submission, render layout/planner, streaming lifecycle, full-scene presentation replacement | Lower avoidable CPU/GPU overhead, fewer redundant builds, stable coverage, controlled memory. |
| **Keep as experiments/oracles** | Existing mesh and ray renderer, SVO/DAG studies, exact source exports | Prevent an attractive rewrite from silently becoming slower or less correct. |
| **Retire only after promotion** | Old runtime/planner/presentation dependency chain and obsolete collector production path | One production architecture rather than indefinitely maintaining parallel engine stacks. |
| **Defer** | Mandatory DAG, all eight brick formats, GPU procedural canonical generation, mesh shaders, hardware RT, GI, weather, fluids, organism rules, terrain regeneration | Avoid combining unproven systems before the bottlenecks and semantics are understood. |

No deletion in this planning pass. Do not delete ignored baseline binaries/captures before archiving reproducible results. Remove superseded experiments only after their evidence and source revision are retained.

## 4. Target architecture and concrete algorithms

### 4.1 Small workspace, explicit interfaces

Start under `successor/` as an independent Cargo workspace with two crates: `world` (query/storage/source adapter) and `viewer` (wgpu, render preparation, streaming, benchmark entry points). Use modules for `coords`, `source`, `edits`, `snapshot`, `journal`, `summary`, `meshing`, `arena`, `residency`, `clipmap`, `visibility`, and `diagnostics`. Split more crates only when dependency or compilation evidence warrants it.

Initially use the old core as a read-only source adapter and fixture oracle. Pin its exact working-tree snapshot before doing this; a path dependency on a changing parent is not a stable baseline. Do not copy `native/runtime.rs` wholesale. Preserve the existing game and launcher until promotion.

Core interfaces, expressed as contracts rather than fixed Rust signatures:

- `WorldRead`: exact cell and bounded region/run queries with explicit unknown results.
- `WorldSnapshot`: immutable source identity + revisioned override references; every worker sees one consistent transaction revision.
- `WorldWrite`: validated batch, explicit set-air versus reset-to-baseline operations, revision/change bounds. Large imports are bounded batches with documented transaction boundaries.
- `VisualSource`: directly sample coarse physical footprints and feature metadata. It is never an alternative authoritative world.
- `RenderBuildRequest`: world ID, source version, region key, target representation, dependent edit revisions, priority, cancellation token.
- `RenderResult`: the same identity/dependencies, exact/estimated bounds classification, geometry/appearance payload, byte accounting. Reject stale results at publication, including on world switches.

### 4.2 Three different granularities

**Storage:** retain 32³ cells initially. Only edits or explicitly cached queried data allocate storage bricks. An absent override brick means baseline, not air. Start with sparse/uniform/dense+presence-mask formats. Add palette compression only if storage profiles justify it. Preserve explicit presence masks for holes carved to Air.

**Ownership/scheduling:** extend existing 64 game-meter pages to signed 3D page keys, using exact rational conversion with i128 intermediate arithmetic. A page indexes sparse work; it is never a dense 64 m cube at native resolution. Store spatial summary nodes only where needed. Bound page metadata and job bytes, not just job count.

**Rendering:** variable physical-footprint pages and bounded batches of surface bricks. Visible native surface bricks can share an upload and GPU arena; no one-to-one brick/draw/bind-group requirement. Far terrain is sampled directly at its footprint. Native cell edge does not set the far ring spacing.

Full native detail still has an irreducible surface/volume cost. Separating pages removes overhead multiplication; it does not make a dense millimeter-scale cave free. Resolution tests must report that distinction.

### 4.3 Bulk source access and native meshing

First provide a fast source bulk path. Reuse column/runs and native column caches to fill intersecting surface bricks; skip proven uniform air/solid interiors. For arbitrary 3D sources use a bounded brick query. No independent expensive terrain evaluation for every y in a solid column.

Use 32³ interiors with a one-cell halo from the **same snapshot**. Neighbor revisions are dependencies. Pack occupancy rows in u64; construct six face masks with shifts and AND-NOT. Merge with linear sweeps, reusable per-worker scratch, and trailing-zero scans. Unknown halo cells remain explicitly unresolved; never mesh a false world boundary as final terrain.

Compare two output strategies on identical cells: material-compatible greedy rectangles, and geometry merging with exact per-cell appearance pages. The former can lose compression on painted surfaces; the latter spends color bandwidth. Preserve exact appearance in the first implementation using the existing lossless codec. Do not assume published eight-byte quad encodings cover our coordinates, flags, water, and appearance offsets; use a checked compact record plus page header and measure actual bytes.

Keep a reference exposed-face enumerator for small fixtures. Compare oriented cell-face ownership and material/RGB, not only screenshots. Halo tests include a tunnel and edits across all six brick faces, corners, and negative coordinates. Water stays opaque blue geometry for now; transparency rules are a separate later decision.

### 4.4 GPU arenas and submission

Suballocate a small bounded set of geometry, appearance, and draw-metadata buffers. Addresses are arena-local offsets with generation-tagged handles. Stage/coalesce uploads; track alignment, fragmentation, temporary allocations, and deferred frees. Publish new descriptors only after payload upload ordering is valid; reclaim old ranges only when all referencing submissions are complete. World switches invalidate handles without freeing in-flight memory early.

Start with CPU visibility and direct draws over pooled buffers, then compare compact visible-quad batches or indirect commands. Require neither bindless textures nor buffer device address. Capability-detect wgpu 24's indirect/count/first-instance features individually; direct draws remain a real tested fallback. Pool sizes must respect binding limits and shared-memory GPUs, not copy a desktop's fixed 400 MB allocation.

Optional GPU HZB culling comes **after** frustum/region pruning and correct residency. Occluders must be actual opaque surfaces for the current view. An inflated bounding box or approximate hill cannot safely occlude a cave entrance. Do not hard-cull from sampled terrain bounds or stale previous-view depth; uncertain/newly exposed regions render. Compare culling on/off and camera reversals; keep it disabled if overhead exceeds savings.

### 4.5 Coherent terrain hierarchy

Use nested viewer-centered regular grids for distant heightfield terrain, with cached ring samples and incremental strip updates. Evaluate an anchored, footprint-filtered version of the existing physical terrain field. Cache parent reconstruction at fine nodes and store the fine residual relative to that parent. Fade residuals in local transition bands; shared boundary samples and triangulation ownership must agree.

For procedural terrain, cheap direct evaluation is an approximation to filtering the canonical surface, not an exact average of every native voxel. Quantify the discrepancy with dense reference patches. Retain water reach/basin IDs and coverage separately; averaging distinct lake levels is forbidden. Reuse existing river metadata. Appearance filtering operates in linear color space; native appearance remains discrete.

Within the exact near zone use native block surfaces. Joining those to a smooth far heightfield is a separate prototype gate: construct a shared boundary contour and local transition strip from both representations. Move that boundary where native steps are small on screen. Where topology differs, switch an entire locally owned replacement region with depth-correct coverage instead of attempting an invalid vertex morph. No whole-frame image crossfade.

Initial quality policy: refine above 1.5 estimated pixels; coarsen below 0.75, with roughly 0.5 s dwell, all tunable trial values. Use nearest conservative depth for perspective projection and handle near-plane intersections separately. Screen-space displacement, feature coverage, and slope/silhouette variation matter alongside projected cell size. Interaction requirements request exact query data independently of render refinement. Do not force a fixed meter-radius of native rendering at every voxel size.

No method can preserve every distant one-cell feature under finite memory. Keep a user-visible quality budget and report when estimates exceed it; never label heuristics as certified error bounds.

### 4.6 Arbitrary 3D and edits: replacement ownership, not additive decoration

An extra mesh drawn over unchanged heightfield terrain cannot excavate that terrain. Implement this correctly in the first vertical slice:

1. Spatially mark regions containing non-heightfield source features or edits. Query exact baseline **plus edits** to build their replacement geometry; do not render only positive additions.
2. Assign disjoint render ownership masks to base heightfield and replacement regions. Suppress the original proxy inside a replacement's owned footprint/volume, including cave mouth surfaces. Keep all geometry on the boundary consistent.
3. For the first finite-world version, replacement ownership may include a whole bounded surface footprint with multiple vertical intervals. Partition tall/complex content into 3D regions so underground or airborne structures do not force one enormous vertical chunk.
4. Build distant replacement proxies from runs/3D summaries and retained edit deltas. Propagate removals as well as additions. Keep the old coherent replacement until the new replacement and its mask are ready; swap atomically.
5. An empty replacement still owns its region: fully deleting terrain must not reveal the procedural parent again. If a region becomes heightfield-compatible, switch ownership back only with a matching-revision base proxy ready.

Test an excavated arch, open tunnel, deep shaft, floating bridge, and far tower. Exact native geometry is non-negotiable. At coarse levels tiny holes may disappear by explicit projected coverage policy, but large resolvable openings must not be filled by the base heightfield. No claim of universal topology preservation from a scalar min/max summary.

### 4.7 Streaming and edits

Lifecycle: `Absent -> Requested -> Building -> CpuReady -> Uploading -> GpuReady -> Visible -> Retiring -> Reclaimable`, plus cancellation/failure paths. Keys include world generation and dependency revisions. Valid results may be cached even if no longer top priority, only within memory limits.

Maintain an independently reserved, cheap distant cover of the finite source. Near requests receive first claim on remaining compute, followed by visible refinement, predicted travel, and background work. Far cover refresh has a minimum service reservation so it cannot starve. Plan by expected image benefit divided by observed build/upload cost, with aging and hysteresis. Bound simultaneous output bytes, scratch, queue length, and upload bytes per frame.

When moving quickly, retain valid coarse parents, lower refinement targets, cancel obsolete work, and defer transitions. Do not shorten the horizon because native pages are late. Teleports/world switches need a coarse initial view before normal streaming; report this as loading, not a frame with missing mountains. Collision queries retain an independent exact path even when visible detail lags.

Edits publish new canonical state atomically and invalidate only affected bricks, halo neighbors, filtered-support ancestors, and visibility metadata. World revisions alone are safe but unnecessarily global; use regional dependency versions for acceptance. Persistence appends checksummed/versioned transactions and checkpoints; distinguish queued, committed-in-memory, and durably-saved status. Recovery ignores an incomplete final record and rejects identity/schema mismatch. Snapshot readers must not observe half a batch.

### 4.8 Ray and compression experiments

Candidate order: pooled raster; sparse brick DDA with coarse occupancy skipping; existing column ray; local SVO; optional frozen SVDAG. Add NAADF-style directional empty-space skipping only after plain brick traversal works. Rendering architecture must not depend on deduplication.

Ray traversal needs distinct hit/miss/incomplete results: step-budget exhaustion is not a proven miss. Handle zero direction components, grazing faces, negative positions, local-origin precision, and edited empty-space acceleration. An added solid can invalidate a cached empty distance; invalidate affected cache support before tracing or fall back to conservative traversal.

Only build frozen DAG blocks if total-memory or full-frame traversal results justify them. Keep hot edits mutable, cap rebuild work, background-compact and swap snapshots. Include topology, material/appearance bytes, hash tables, scratch, upload, and temporarily duplicated snapshots. A topology-only compression ratio is not total world memory savings. Test high-entropy colors, not only repeated shapes.

## 5. Implementation sequence and promotion gates

Each stage is a separately reviewable change with a fallback, raw measurements, and a short adopt/iterate/reject note. No estimated speedup is treated as achieved.

| Stage | Work and deliverable | Exit gate |
|---|---|---|
| **0 — Freeze and measure** | Snapshot current tracked/untracked source, config, lockfiles and compiler; hash baseline binary and scene manifests. Add deterministic routes, exact-cell exports, GPU timestamp support where available, build/upload timing and total allocation counters. | Baseline can be reproduced independently. Existing game remains launchable. Record failures as well as passes. |
| **1 — Core/adapters** | Two-crate successor; source adapter; typed scales/domains; immutable edit snapshots; bulk sampling; explicit Air/reset semantics; checkpoint reader and transaction journal. | Exact voxel/material/appearance parity, unknown-domain behavior, negative/boundary/overflow tests; replay and interrupted-save recovery. No render changes needed to pass. |
| **2 — Native raster** | Bitmask mesher, reused scratch, lossless appearance, pooled GPU buffers, direct fallback submission. Bring in player camera only through query interfaces. | Exact surface ownership across halos; no stale uploads; native open terrain, cave, and edited tower render. Build+upload cost compared with old collector including source query time. |
| **3 — Coverage and coherence** | Separate far cover, nested terrain cache, coherent parent reconstruction, local transitions, 3D replacement ownership, revisioned far edits, bounded streaming. | Fog-free moving videos show no holes/retreating horizon; cave removal never reappears as base terrain; no repeated stationary LOD oscillation. This is the main rewrite milestone. |
| **4 — Targeted optimization** | Profile staging, CPU submission, page sizes, appearance duplication, source cache contention. Compare indirect batching and conservative visibility optimizations. | Promote only changes with repeatable end-to-end benefit. Correctness unchanged; tiny gains do not justify a much more complex backend. |
| **5 — Volumetric comparison** | Brick DDA, then SVO/DAG or directional skipping as justified. Same source, edits, colors, projection, lighting, and quality. | Full-frame and edit-latency evidence identifies a winning workload, or retain raster and archive the negative result. No obligation to promote ray/DAG. |
| **6 — Game integration** | Adapt menu, seed/voxel cm/World Scale, survey selection, F5/player, capture tools; generic changing-cell stress workload. Preserve old save import or clear incompatibility handling. | End-to-end tests and user walk-through pass; same launch experience; changing cells do not trigger global rebuilds. |
| **7 — Promotion/cleanup** | Make successor default, preserve legacy baseline in an archive/tag/snapshot; remove obsolete production paths and duplicate docs, update developer guide/cover only with new captures. | Performance/visual acceptance below passes; launcher points to tested build. Commit/push only within then-current authorization. |

After this, revisit landform/river improvements as a separate generator version and benchmark cohort. Infinite regional hydrology, future physics, and appearance work must not block the renderer milestone.

## 6. Benchmark specification

Fixtures: existing 10+4 terrain cameras and seeds 42/137/909/2027; flat colored plane; sharp ridge and shoreline; edited cave/tunnel spanning page boundaries; far tower/bridge; repeated cell clusters; high-entropy irregular occupancy/color; generic local cell updates. Include real import only as compatibility, not as the primary beauty target.

Routes: cold launch then immediate walk; stationary warm view; 120 s walk/sprint/reversal; full turn; repeated crossing of LOD boundaries; flight toward mountain; underground-to-surface; near/far edits; memory pressure and world switch. Match speed, viewpoint, resolution, FOV, colors, lighting, and error policy. Disable fog and camera interpolation for the geometry comparison; evaluate interpolation separately so it cannot hide LOD defects.

Start at 1080p on the actual Radeon 760M, 6.25 cm, World Scale 1 and 10, 256 MiB managed terrain budget. Repeat at 96 MiB pressure, then 3.125 cm and 1 cm on bounded test routes. 1 mm is a tiny-fixture correctness/stress test initially. Low-end support needs a named second physical adapter; a 760M run does not establish older-laptop compatibility.

Record at least five release trials after a specified warmup; report median plus trial spread and frame p50/p95/p99/max. Separate cold and warm. Record exact toolchain, source snapshot/hash, GPU/driver, API, scene version, command, camera track and settings. Timestamp GPU passes where supported; CPU synchronized frame time is labeled separately. Time screenshot/readback diagnostics separately from ordinary movement.

Metrics: query/generation/meshing/scratch, queued and canceled work, upload MB and stalls, submission cost/draw calls, GPU frame/pass time, time to first coherent view, native-detail lag, edit-to-visible and durable-save latency, CPU RSS/peak, all engine GPU allocations including targets/staging/transitions, fragmentation, and pinned-reader bytes. Driver-managed allocation overhead is reported separately where observable, not silently equated to our allocator counters.

**Initial proposed engineering gates (not current results):**

- Zero wrong exact cells, native face/color mismatches, crashes, use-after-free, stale world uploads, or cracks in correctness fixtures.
- No horizon contraction due to streaming; no uncovered replacement regions. Repeated stationary replans produce zero LOD swaps after settling.
- A 10-minute bounded-memory stress run reaches a stable high-water mark; all budget exceptions are explicit. Transition reserve is part of the budget, not hidden extra memory.
- At default voxel size, target 1080p p95 <=16.7 ms and p99 <=33.3 ms on the measured 760M route. These are goals; if unmet, report actual limiting stage and choose an honest lower preset. Cold regional generation is measured separately and must not regress >10% without justification.
- Promotion should improve median build+upload or streaming p95 latency by at least 20% on the bottleneck fixtures, with no >10% frame-p95 regression on other representative scenes at comparable quality. Geometry quality can justify an explicit exception, never an unreported one. Reject differences inside trial noise.
- Target isolated edit-to-visible p95 <=100 ms warm; cross-page batch p95 <=250 ms for a declared 1,000-cell batch. Measure and report large-batch backpressure rather than promise unlimited edits.
- Compare silhouettes/depth/material coverage with high-quality references, plus motion-compensated videos. Target <=1 pixel P95 silhouette displacement in selected far-LOD fixtures; treat sampled metrics as test results, not universal bounds. Do not use raw frame differences that confuse camera motion with LOD popping.

## 7. Expected benefits and honest limits

| Change | Expected improvement over current implementation | What it cannot promise |
|---|---|---|
| Bulk source + bitmask meshing | Less repeated procedural work and ordered-map overhead near the player | Constant cost regardless of exposed surface complexity |
| Arenas + batching | Fewer driver objects, allocations, binds and submissions; smoother updates | Higher FPS if terrain evaluation is the actual bottleneck |
| Physical render pages + screen-size policy | Avoid tiny-cell job/draw explosion; spend work on visible information | Native millimeter detail over the whole landscape for free |
| Coherent far grids + local transitions | Lower distant geometry payload and more stable silhouettes/contours | Automatic perfect near/far topology or guaranteed invisible transitions |
| Independent horizon residency | Distant mountains remain while nearby detail catches up | Infinite visible geography or zero cold-start time |
| Full 3D replacement ownership | Digging, caves and buildings remain correct across render paths | A heightfield alone representing arbitrary structures |
| Versioned snapshots + invalidation | Local rebuilds, consistent queries, safe asynchronous updates | Unlimited edits or organisms at fixed frame cost |
| Optional ray/compression experiments | Evidence-based path for complex/repeated volumes | DAGs universally outperforming meshes or columns |

The rewrite should earn its place through a better moving, edited world, not a more impressive data-structure name. Keep the current terrain while proving that result.
