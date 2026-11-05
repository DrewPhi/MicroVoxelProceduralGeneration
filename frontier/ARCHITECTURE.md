# Frontier Voxel Engine — audited architecture (2026-11-04)

This document governs the **new, isolated** `frontier-voxel-engine` branch and the `frontier/` workspace. It is a research-backed plan, **not a claim of globally optimal performance**. Revisit architectural choices only against matched measurements, the visual requirements below, and documented alternatives.

**Fresh-start checkout (2026-11-04):** inherited engine source, tools, tests, assets,
build manifests and obsolete CI have been removed from this branch at the user's
request. Legacy paths below identify files on the named reference branches, not
files that must remain in this checkout. No Frontier runtime has been implemented
yet. Generated legacy artifacts are preserved in the
[archive release](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/releases/tag/legacy-research-archive-2026-11-04).

## Product contract / invariants

- A deterministic, **editable, 0.0625 m** (6.25 cm) integer-lattice world, including air/material, digging/placing, caves, rock arches, player structures and vegetation.
- **Beautiful, visually stable distant mountains and valleys**, several miles of unobstructed visibility. Do not substitute artificial opaque fog for a working horizon. Real atmospheric scattering/aerial perspective is compatible with this goal and must be independently adjustable.
- Walk/run/turn and aircraft travel at a **configurable, measured game max speed**. Teleports instantly supersede old-camera work. Visible popping, holes, stale tiles, frustum holes, terrain faceting, tiny-ridge disappearance, and tile-chasing are primary defects.
- **No handcrafted LOD files or offline mesh fitting prerequisite.** All caches and display representations derive automatically from a single world definition. Automatic visual LOD is permitted; exact gameplay voxel identity never changes with rendering LOD.
- Target mainstream hardware at 1080p/60 FPS, with a configurable lower-spec profile. Treat this as a performance target requiring measurement, not a proven result. Budget GPU work, CPU time, transient/staging VRAM, cache residency, frame-time tails, and edit latency.
- No topology loss hidden by a heightfield: a carven cave mouth, overhang, bridge, player-built arch or giant tower must have valid distant representation when screen-visible.
- The authoritative game world and deterministic save/physics/raycast semantics must never depend on which renderer is currently selected.

## Most important design correction

Do not commit to a single renderer before the architecture gate. For normal landscape surfaces, **adaptive quadtree-selected, reusable GPU-displaced terrain grids (CDLOD-inspired)** appear stronger than repeatedly CPU-building thousands of coarse meshes. Geometry clipmaps remain a simple competing candidate and may win if terrain-uniform traversal + low CPU overhead are dominant. For topology-rich or heavily edited scenes, an **NAADF-style hierarchical voxel ray renderer or compressed GPU-driven voxel representation** may beat meshed proxies. Implement small, matched prototypes of *both* and freeze the primary renderer only after evidence. No heavyweight full-engine work needs to wait for visual/source-world contracts.

## One canonical world, multiple display paths

`WorldSource(seed, parameters, version)` defines canonical material at any global integer cell `(i,j,k)`; mathematical macroscale surface `H(x,z)` and implicit caves/3D landforms are input generators, **not different worlds**. Persistent sparse edit overrides take precedence, including add and remove. Use CPU integer coordinates and sufficiently precise chunk origins; pass camera-relative/local coordinates to GPU to prevent 10 km jitter. CPU and GPU LOD sampling need compatible, tested positioning/seed/noise semantics; do not silently reinterpret the generated world at each LOD.

`H` composition: geologic province / uplift / ridge skeleton / continuous domain-warped ridged FBM + erosion-and-drainage-constrained terrain + conditioned fine surface detail. Preserve the same river graph/shorelines/ridgeline anchors across resolutions. Direct coarse generation is allowed, but **must be consistent with canonical geometry**; unsampled features require conservatively represented bounds or feature descriptors. Fine-scale detail must be filtered by footprint, not independently regenerated at coarse LOD. Analytical omitted-band bounds may work for pure FBM; complex warped/eroded fields need certified/pessimistic sampled envelopes with a conservative margin.

**Heightfield-safe classification:** represent a region as one top surface only if it is valid for that region (single-valued surface, and no exposed 3D/edited content that the heightfield would incorrectly fill). The entire world remains voxel-authoritative; the heightfield is just an acceleration/display approximation.

## Rendering paths and ownership

### A. Landscape surface (default bulk renderer)
- A bounded pool of immutable/reusable grid topology; instance quadtree-selected patches with GPU displacement from cached multiresolution height pages or efficient deterministic generators. No CPU mesh rebuilding for ordinary camera movement.
- Screen-space **geometric error** (not merely projected cell width) and silhouette/ridge/shoreline visibility drive refinement; use per-region height/error extrema. Hysteresis controls oscillation.
- Parent–child shared anchor samples and **residual-based geometric morphing**; crack-safe 2:1 transitions and coherent normal/material filtering.
- A conservative root/coarse world always available at the destination (including after teleport); refine in-place with no empty terrain gaps.
- Geometry clipmaps are an A/B alternative to adaptive CDLOD selection—not additional required complexity.

### B. Volumetric and editable features
- Sparse canonical voxel bricks with uniform/empty/runs/dense layouts and copy-on-write edits plus versioned persistent journal; edit changes are local, no quadratic refitting.
- Exact near geometry through greedy/bitmask mesh extraction or GPU ray traversal. Which is faster is workload/device dependent; preserve both in comparison tests.
- 3D structures/caves/overhangs at a distance get **automatic** conservative multi-scale proxies or hierarchical ray tracing. Large visible openings/arches cannot simply disappear into a heightfield.
- For shared areas, the renderer needs **explicit authority/ownership**, not overlapping independently drawn surfaces. A volume footprint removes corresponding heightfield triangles/visibility at all relevant LODs; regenerated covers must be hole-aware and never resurrect a removed cave top.
- Edits invalidate affected canonical chunks, height/volume display pages, collision, materials and shadows by **revision key**, with interleaved old/new data hidden until a coherent replacement can publish.

### C. Composition / visual correctness
- Same global coordinate/frame depth semantics for both renderers.
- Heightfield-to-near-native transitions need a tested block-geometry policy: the smooth proxy is *not* automatically identical to stepped native 6.25 cm voxels. Transition only when projected error is below the pixel target, with depth-safe local morph/dither and motion-aware temporal reconstruction. Reconstruct geometry ownership consistently through cave openings and edits.
- Frame-consistent shading: terrain PBR layers, macro color/rock masks, triplanar texture or virtual texture pages, slope/wetness, distant texture mipmaps, atmospheric depth, water, foliage impostors. Do not let texture/high-frequency normal maps shimmer.
- Object trees/vehicles/actors may use standard instanced mesh/impostor systems; never force arbitrary entities into native voxels unless destructive edit/gameplay semantics demand it.

## Streaming and cache architecture (replace legacy control logic)

Single camera-intent controller; every CPU, page-generation, GPU upload, and publication job carries a **view generation, world revision, and dependency key**. A new teleport/view epoch cancels queued old-camera tasks, stops cooperative CPU work quickly, and prevents outdated outputs from publishing. Already submitted GPU work may finish but must not block new priorities or consume unbounded budget.

- Predict camera frustum/ground footprint over `T_horizon >= measured p95 generation+upload+publish latency + margin`; derive lead from actual velocity/acceleration/maximum aircraft speed, **no hardcoded 12 m cap**.
- Prioritize root coverage, visible silhouette/error, time-to-need/deadline, then near detail and speculative prefetch. Keep dedicated budgets for near urgent work and horizon. No fixed 500 ms response delay; event-driven replanning plus rate-limited expensive work.
- Use reserved physical pages / GPU arenas, lock-free or short-lock job queues where justified, epoch-tagged upload completion, deferred reclamation, and bounded memory pressure. Never stall the render thread on synchronous terrain generation or disposal.
- At teleport, publish root/coarse destination coverage immediately from deterministic low-cost data (within an explicit measured budget); discard old intent and progressively refine with zero holes.
- Maintain predictable average **and p95/p99 frame-time** while moving. Scale quality gracefully when constrained, but preserve topology and horizon.

## Architecture gates / competing renderers

**Gate 0**: a deterministic benchmark scene suite: mountains with knife-edge silhouettes, river valleys, complex caves, bridges/arches, tall edited structures, dense forests, and empty or uniform terrain. Capture fixed/orbit/walk/sprint/aircraft straight + sharp turn + teleport + edit-under-flight. Same images, camera motion, hardware, time budgets; CPU/GPU timestamps and frame-time tails.

**Gate 1 (terrain renderer)**: prototype adaptive reusable-grid GPU displacement with morph, and a minimal nested clipmap option; compare against current `main/successor` on matched physical area + quality. Instrument CPU tile work, GPU cost, bytes in flight, silhouette pixel errors, transition ghosting/pops, and generated height-page work. Choose CDLOD vs clipmap based on measured quality/work, not aesthetics.

**Gate 2 (volume renderer)**: compare existing native bitmask/greedy mesh approach, compact bricks, and one NAADF-inspired ray traversal probe on cave/edit-heavy scenes and long-range surfaces; benchmark full-screen traversal, memory and edit/update overhead. Aokana is a GPU-driven SVDAG+LOD reference but not automatically editable; do not adopt DAG compression merely because a static-scene paper has strong results.

**Gate 3 (hybrid ownership)**: two overlapping render paths must show **no phantom terrain** over a cave, edit, or bridge at any LOD and no depth seams. If ownership/transition costs dominate, reconsider a more unified 3D renderer before committing gameplay.

**Gate 4 (production flight)**: complete horizon at speed, camera cut response, empty/far loading rejection, bounded page work and transitions; visual evaluation with temporal as well as still metrics. Require no significant silhouette changes above chosen screen-space error targets for resolvable geometry; such a bound must be demonstrated, not assumed from vertex cell size alone.

## Measurements and target acceptance (subject to profiling)

- Hardware tiers: mainstream GPU at 1080p/60; secondary Radeon 760M budget-focused profile. Record actual GPU model, driver, VRAM, game resolution, seed, camera trajectory, visible distance and settings.
- Per-frame: p50/p95/p99 CPU time, isolated GPU timestamps, draw calls, triangle counts, page generation/upload, outstanding/stale jobs, cancellations, memory resident/committed including transient/inflight and process RSS.
- Quality: coverage gaps must be zero in fixture views; exact nearby voxel identity/edited structures/collision; contour/tunnel/coastline preservation; screenshot and temporal recording; screen-space silhouette deviation in pixels with a clear oracle and error measurement.
- Motion: benchmark walking, fast turn, the **actual game aircraft's maximum speed**, and teleport across a full world. No unsupported promise of perfect invisibility at every speed.
- A source-world revision should never change merely because its display quality changes. Caches are disposable; game saves remain recoverable and versioned.
- No arbitrary opaque visibility cutoff. Real volumetric atmosphere is allowed as a visual model but may not conceal unavailable geometry.

## Reuse of older branches — mandatory policy for every issue

**Reference tiers:**
- **R / read-only:** inspect old code to understand behavior or reproduce a bug, cite exact path/commit and preserve a test fixture; do **not** transplant algorithms.
- **P / selective port:** port a small, well-isolated algorithm/data contract only after a documented suitability review, exact-voxel or parity tests, performance profiling where relevant, and dependency/license checks. Always update it for the new architecture.
- **B / baseline only:** use the old branch only for matched benchmarks and regression images.
- **G / greenfield:** deliberately avoid relying on the old architecture; design to new interface/acceptance criteria. Old code may still serve as a *negative* example.

**`main` is the older active engine**, especially `successor/world/`, `successor/viewer/`, `agent_context/ENGINE_REDESIGN_IMPLEMENTATION.md`, and `src/render/`. **`DrewsAnalyticRendering` is an optional reference** for dramatic FBM and renderer research, not a dependency. Do not modify either original branch. `frontier/` is a new implementation workspace; no wholesale source-directory copy.

**Potential selective ports**: integer grid, voxel query semantics, COW edit bricks, journal/crash recovery, bitmask face mesher, GPU allocator/alignment tests, deterministic generator fixtures, climate/drainage *data* if consistent. **Never blindly port**: old 500 ms planner, 2-patch-per-motion scheduler, limited-lookahead predictor, sequential CPU coarse-mesh regeneration, 160 ms patch fades, coarse summary approximation as a silhouette proof, stale view publication, or globally coupled quadratic fitting.

Every Frontier issue MUST include **Legacy reference policy** specifying named files or branch, tier (R/P/B/G), allowed tests, forbidden transplants, plus tests for the new contract. Read and respect existing `main` issue numbering; Frontier work belongs in its own [FV] issue family. Any dependency or performance gate must be mentioned in pull requests.

## Research / implementations to inspect

- Geometry clipmaps (Losasso & Hoppe 2004): https://hhoppe.com/proj/geomclipmap/ ; GPU Gems 2: https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry
- CDLOD (Strugar 2010): https://github.com/fstrugar/CDLOD (DirectX9 reference—not directly reusable wgpu code)
- Terrain3D GPU clipmap reference: https://github.com/TokisanGames/Terrain3D
- Voxel Tools (editable smooth terrain, Transvoxel, streaming caveats): https://voxel-tools.readthedocs.io/en/latest/smooth_terrain/ ; https://voxel-tools.readthedocs.io/en/latest/performance/
- NAADF (Eurographics 2026, dynamic voxel ray tracing): https://github.com/cg-tuwien/NAADF ; https://onlinelibrary.wiley.com/doi/full/10.1111/cgf.70413
- Aokana (2025 GPU-driven DAG/LOD): https://arxiv.org/abs/2505.02017
- Matched editable multi-layer heightfield comparison (2026): https://arxiv.org/abs/2608.17390
- PBR geometry/LOD alternative and limitations: https://dev.epicgames.com/documentation/en-us/unreal-engine/nanite-virtualized-geometry-in-unreal-engine
- FastNoise2, ridges and domain warps: https://github.com/Auburn/FastNoise2
- Physically-based analytical erosion: https://doi.org/10.1111/cgf.15033 ; coherent features: https://doi.org/10.1145/2461912.2461996 ; controlled GPU procedural detail: https://doi.org/10.1111/cgf.14992
- Foliage, shadow page invalidation inspiration: https://dev.epicgames.com/documentation/en-us/unreal-engine/virtual-shadow-maps-in-unreal-engine
- Related old projects: `main/agent_context/ENGINE_REDESIGN_RESEARCH.md`, `main/agent_context/ENGINE_REDESIGN_IMPLEMENTATION.md`, `main/agent_context/OCTREE_DAG_STUDY.md`; analytic `DrewsAnalyticRendering/agent_context/FITTING_AUDIT.md`.

## Migration strategy

Create `frontier/world`, `frontier/renderer`, `frontier/runtime`, `frontier/tools` as isolated crates/workspace. Current `main` continues running unchanged; its engine files remain available on that remote branch for inspection, comparative benchmarks in a separate checkout, and *selective* vetted ports. Keep legacy source trees out of this fresh working copy. Never overwrite world saves or mutate the stable player experience before an explicit promotion decision. Progress in vertical slices: canonical world → one coherent beautiful mountain visible at distance → motion-aware streaming → exact local voxels + far cave → editing + flight → appearance/performance.
