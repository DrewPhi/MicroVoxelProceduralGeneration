# Frontier Voxel World Engine — Research-Informed Redesign Specification

**Cutoff:** 29 October 2026 (sources and claims must not depend on later work)
**Project:** successor to [DrewPhi/MicroVoxelProceduralGeneration](https://github.com/DrewPhi/MicroVoxelProceduralGeneration)
**Status:** complete agent-oriented engineering design and research/benchmark plan; specific performance improvements are hypotheses to test, not guarantees
**Primary implementation language:** Rust; default portable graphics abstraction: wgpu/WGSL, with feature-gated lower-level Vulkan experiments only if proved necessary.

> **For Codex / implementation agents:** This is the single entry-point specification. Read §§0–3 for invariants and the source-code diagnosis, §§4–8 for architecture and algorithms, §§9–14 for tests and implementation sequence, §15 for the reference library, and §§16–20 for detailed experiment designs, implementation contracts, external examples, and work instructions. **Do not start with lighting, shader beautification, or a universal DAG rewrite.** Record hardware, source commit, inputs and measured evidence for every performance claim.

## 0. The decision

Build **a new foundation in parallel with the existing engine**, retaining the current program as (1) a source of generator code, (2) an oracle for exact voxel queries, edits, and collision, and (3) a benchmark competitor. Do not port the existing LOD quadtree, face collector, GPU buffer-per-tile architecture, or full-image LOD replacement mechanism uncritically.

The new engine is a **strictly discrete voxel universe with a separate, approximating graphics system**. Every physical position is an integer lattice coordinate. A world has one physical voxel edge length; default **1/16 m = 6.25 cm**, fixed at world creation. The smallest living cell and smallest physical object occupy one voxel. There is no subvoxel physical occupancy. Arbitrary materials, organisms, and buildings are patterns of those cells; the engine does not encode their semantic meanings. Physical simulation may use activity scheduling and grouped computational representations, but it must preserve the canonical cell-level world semantics.

Rendering is allowed to use *fractional vertices, geometry morphing, filtered visual aggregates, or a visually interpolated camera*, provided these are derived presentations and never introduce smaller physical matter or fractional canonical occupancy. Near-field rendering must preserve the block-like native voxel aesthetic; far-field detail may disappear and silhouettes may simplify as a function of projected pixel size. No obvious triangles, LOD rings, seams, or rapidly replacing terrain should be visible while moving.

**Prioritize foundations:** world generation, exact editing and persistence, 3D storage, rendering and LOD, streaming, and measured GPU efficiency. Defer global illumination, weather, fancy shaders, physically sophisticated material simulation, liquids, and organism-specific behavior. Begin with flat colors, simple daylight, no obscuring fog, and test geometry in motion.

## 1. Non-negotiable functional contracts

1. **Canonical truth.** `VoxelCoord {x:i64,y:i64,z:i64}` and `MaterialId/CellState` determine occupied matter. Configurable `WorldScale` sets meters per physical voxel **once at world creation**; it does not change dynamically. A generator can answer an exact cell query from seed + configuration + coordinate without pre-expanding its containing volume. Persistent overrides take priority over the procedural baseline.
2. **Universal 3D topology.** Add/remove/change any voxel anywhere, including tunnels, overhangs, floating structures, and trees built out of cells. Terrain-optimized fast paths cannot redefine the world's possible geometry.
3. **Everything sparse unless genuinely dense.** Unchanged procedural world is a function, not an allocated field. Empty, uniform, run-length, palette, bitset, sparse, and dense bricks are implementation representations that can be chosen adaptively.
4. **Render hierarchy is not physics.** Coarse proxy geometry is an image-making device, never a larger physical cell; LOD cannot affect simulation, edits, or collision. Visual geometry at a distance can morph between proxies.
5. **Region streaming is physical-space-oriented.** Render regions, screen-space pages, resident geometry budgets, and job granularity must not all shrink in proportion to base cell length. Near voxel bricks may still use a fixed number of cells; higher-level *render/stream* units must be independent.
6. **Procedural summaries must be cheap.** The generator supports multi-scale height/material summaries and bounded error/feature heuristics without scanning every native cell. Claims of exact error bounds require provably conservative generator bounds; sampled estimates are not certified bounds.
7. **Edits propagate.** Every edit logs a versioned change; derived render data, summaries, visibility and persistence are invalidated at appropriate ancestors, neighboring boundaries, and distant levels, not globally.
8. **Bounds and determinism.** CPU/GPU memory, work queues, per-frame uploads, and scheduling are bounded; identical seed and world config return identical canonical voxels, independent of render LOD or worker completion order.
9. **Quality degradation, not disappearance.** Under budget pressure, degrade projected detail or skip expensive effects; retain conservative distant terrain, valid near gameplay geometry, and coherent previous representations. Never tie the far-plane visibility to whether fine near tiles are ready.
10. **Portable graphics baseline.** The low preset must avoid mandatory mesh shaders, hardware ray tracing, bindless support, CUDA, or enormous GPU memory. Advanced GPU features may improve higher tiers through capability detection.

## 2. Evidence from the old repository (29 Oct 2026)

| File / existing behavior | Observed defect or limit | Redesign action |
|---|---|---|
| `src/grid.rs`: `VOXELS_PER_METER=16`, `CHUNK_EDGE=32`, hardcoded `16.0` in conversion | Physical scale, indices, and other modules embed 6.25 cm assumptions | Central `WorldScale` and typed cell/region conversions, code audit, serialization versioning |
| `src/render/lod.rs`: quadtree leaves have `SIDE=32` display cells, `FINEST=13`, `EXTENT=160_000`, `ROOT=262_144` | A finest 32-cell tile occupies only 2 m × 2 m at 6.25 cm; tile count rises approximately with inverse squared cell size in a fixed-radius region | Separate cell bricks, physically sized scene regions, screen-space adaptation and cached nested clipmaps |
| LOD uses projected *display cell width*, approximate terrain error, and `native_radius` | Sampled hill errors can be much larger than requested projected cell size; detail changes as player travels | Track parent-child displacement, variation and coverage; prefer actual screen-space error estimates, robust silhouette metrics; use hysteresis and temporal residency |
| `src/render/surface.rs`: `BTreeMap` per exposed face plane/cell, up to 262,144 exposed face records; separate `Vec<u32>` color per exposed unit face | Expensive random CPU insert/remove operations and large geometry/color payload | Packed occupancy masks; bitwise hidden-face elimination; linear meshing; palette/material-driven appearance with separate optional exact color patches |
| `src/render/terrain.rs`: profile and run samples; independent coarse sampling | Boundary joins can be watertight but coarse/fine terrain still differs | Shared multiresolution generator with parent/child correspondence and explicit error representation; topology-aware transition handling |
| `native/src/gpu.rs`: part buffers/bind groups, per-visible-part draws | CPU submission grows with mesh/tile/part count | GPU buffer arenas, compact draw packets, batched and where supported multi-draw-indirect; 3D cluster culling |
| `native/src/presentation.wgsl` and `gpu.rs`: 160ms full-frame Bayer image replacement | Visible stippling/frequent fades and temporary double rendering | Local geometry morphing and/or depth-correct stochastic coverage; avoid global image transitions |
| `native/src/runtime.rs`: moves visibility goal between 1024 and 24000 m based on near-detail readiness | Distant landscape can recede when streamers fall behind | Always maintain a cheap far-horizon cover, with independent detail/visibility budgets |
| `native/src/camera.rs` and `player_view.rs`: eye position snapped to canonical cell | Camera can step 6.25 cm; screen shimmer independent of LOD | Keep exact integer physics; permit presentation-only smooth/interpolated view as optional setting |
| `src/storage.rs`: adaptive sparse/dense/uniform edit bricks using `BTreeMap<u16,Voxel>` | Sound semantics; expensive for large active random writes; not compressed frozen geometry | Keep baseline+override architecture, add palette-packed/bitset bricks, bulk operations, optional frozen DAG cache |

**Measured evidence, not predictions:** 16m×16m patch generation and merging in latest 28 Oct report: 50cm 2.77ms, 25cm 23.04ms, 12.5cm 80.39ms, 6.25cm 124.00ms; 6.25cm produced 153,525 exposed native faces, 91,737 merged rectangles, 3.39 MiB mesh/atlas payload. 29 Oct matched 1080p captures on Radeon 760M: after-optimization valley mesh 5.10/4.11ms in two repeats, valley column ray 14.13/14.05ms; these synchronized offscreen frame times **are not interactive FPS guarantees**. Independent instrumented 4,096-ray tests: SVO/DAG were 1.49–1.86× slower than columns on two ordinary terrain samples, but ~3.47× faster on one carved cave. No universal renderer wins all scenes. See [performance](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/agent_context/PERFORMANCE.md), [profiling](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/agent_context/PROFILING_AND_CLEANUP.md), and [SVO/DAG study](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/agent_context/OCTREE_DAG_STUDY.md).

## 3. Overall architecture

```text
  WorldConfig(seed, fixed base-voxel edge, world-generator version)
                              |
     ┌────────────────────────┴─────────────────────────┐
     │                                                  │
  Procedural baseline                           Persistent sparse edits
  - exact point query                           - journals / checkpoints
  - direct coarse fields                        - arbitrary xyz occupancy
  - range/min-max hints                         - changed-region versions
  - geological/climate maps                     - palette/bitmask bricks
     │                                                  │
     └─────────────────────┬────────────────────────────┘
                           │
                     AUTHORITATIVE WORLD
             exact integer material and cell state
                           │
              ┌────────────┴────────────┐
              │                         │
       WORLD-CHANGE API           QUERY / SIMULATION API
       mutation batches          exact occupancy/material
       neighborhood dirtiness    sparse active cells
       immutable snapshots       generic local events
              │                         │
              └────────────┬────────────┘
                           │
                 DERIVED VISUAL HIERARCHY
       occupancy / surface / error / materials / versions
                           │
           ┌───────────────┼────────────────┐
           │               │                │
       Near exact       Mid complex      Far landscape
      voxel geometry   voxel clusters     nested clipmaps
      bitmap meshing    compressed SVO    direct coarse fields
      and GPU batches   or meshlets       silhouette-safe proxy
           └───────────────┼────────────────┘
                           │
           Resident GPU pages + visibility scheduler
                           │
            simple baseline shading / presentation
```

**Separate three hierarchies that are often wrongly conflated:** (a) physical cell-brick addressing for edits/query, (b) coarse 3D occupancy/feature summaries for culling and LOD, (c) render pages in physical or projected footprint units. A 32³ cell storage brick may be 2m across at 6.25cm, but that does not imply a draw call or streaming job every 2m or a dense allocation in distant terrain. Multiple bricks may be packed into one GPU pool or generated directly into proxy geometry.

## 4. Recommended data structures

### 4.1 World address and precision

- CPU uses 64-bit signed integer cell coordinates, Euclidean division for negative coordinates, checked math and extent-free sparse indexing. `WorldScale` uses rational or well-defined integer units (e.g., micrometers per voxel) to avoid accidental floating-point drift; 6.25cm = 62,500 micrometers.
- `CellCoord`, `BrickCoord`, `RegionCoord`, `CellBounds`, `WorldMeters`, and `GpuLocalCoord` are distinct types, never interchangeable raw triples. Configuration is part of world/save identity. All API boundary casts are reviewed for overflow.
- GPU geometry is relative to a local camera or region origin (high/low split if needed); never assume single global `f32` positions remain exact over 10km at 1mm. Keep exact integer cell identities separate from projected vertices.
- Gameplay moves at integer cell positions/ticks; optional **visual-only** camera interpolation does not change voxel occupancy, collision, or physics. A fully snapped-camera mode remains available for aesthetic preference.

### 4.2 Sparse region storage

- On-demand generator is the default fallback. A region not explicitly visited or edited is **not allocated**.
- `BrickPayload` variants for `Empty`, `Uniform(material)`, `Procedural(generator-key)`, `RLE/vertical-runs`, `PalettePacked`, `OccupancyBitset+palette`, `Dense`, and `SparseOverride`; choose based on occupancy, entropy, query/edit patterns, and measured memory cost.
- Start with compact 16³/32³ bricks and benchmark 64³ for relevant hot paths. A brick is a **storage granule only**, not a render LOD tile or fixed physical streaming radius.
- Support bulk edits with sorted locality, copy-on-write immutable snapshots, per-brick dirty ranges, transaction journal and atomic version publication. Do not mutate an actively rendered GPU payload in place while a prior frame is using it.
- Keep occupancy/topology separate from material/appearance bytes. Color need not be duplicated per exposed face if derivable from material, location and a small deterministic variation function. Truly individually painted voxels must still have exact overrides when close.

### 4.3 Compression lifecycle (Cubiquity + SVDAG)

- **Hot, frequently edited:** mutable bitset/palette/array bricks and sparse override maps.
- **Warm, mostly static:** pack runs, compress palettes and optionally build immutable SVO blocks for hierarchical traversal.
- **Cold, static/repeated:** optional hash-consed SVDAG across blocks; deduplicate identical *geometry*, consider separate color/attribute representation; reference-count or epoch-manage shared nodes and reclaim safely.
- Edits temporarily overlay a frozen compressed snapshot and trigger background local rebuild, versioned pointer swap and garbage collection. Avoid rebuilding a globally shared DAG on each single-cell modification.
- Do not adopt Cubiquity's individual `setVoxel` population path for 10 km on-demand generation. Its author explicitly calls bulk construction that way prohibitively slow. Generate procedural surfaces and compact aggregates directly.
- Benchmark *fully loaded* memory and end-to-end edits as well as compressed node size. A DAG that saves topology but leaves 2MB of per-voxel attributes is not a 10× system memory saving.

## 5. Procedural generation: fast at every visual scale

### 5.1 Separate spatial frequencies

Construct a deterministic, compositional macro-to-micro generator:

1. Macro geography: sea level, plateaus, uplift ranges, fault directions, climate zones, basins, drainage graph, regional mountain structure.
2. Mesoscale geomorphology: river valleys, ridges, erosion/deposition, soil depth, biome coverage and lithology.
3. Microstructure: boulders, rock fractures, scree, soil variation, native-cell surface noise and caves/overhangs.
4. Visual summary interface: evaluate the required *frequency band* at each display level, with filtered elevations/colors, min/max/roughness and conservative indicators where mathematically available.

Far-distance generation should omit frequencies well below projected pixel resolution, rather than generate and decimate every 6.25cm cell. Near-distance exact queries run the full deterministic recipe. Edit deltas are applied at every scale.

**Distinguish an exact canonical generator from an approximate LOD image.** It is acceptable for a coarse parent to approximate a yet-unmaterialized million-cell region. It is not acceptable for a physical query to return the parent approximation instead of the true cell value. Where an error summary is sampled, label it an estimate; adopt conservative analytic bounds or explicit feature flags when one needs a strict no-missing-feature guarantee. Tunnel and cave correctness is preserved by the 3D overlay/full geometry path, not a heightfield-only promise.

### 5.2 Geomorphological plausibility and eventual visual beauty

Inspiration: 2024 *Terrain Amplification using Multi-Scale Erosion* (Schott et al.) and *Physically-based analytical erosion for fast terrain generation* (Tzathas et al.). These motivate scale-coupled erosion/drainage and fast physically informed uplift/erosion field generation. They do **not** require a costly water solver during runtime. Favor offline/regional macro solves or fast analytical approximations with deterministic per-region synthesis.

Use real-Earth data (Copernicus DEM GLO-30, ESA WorldCover, MERIT Hydro) for *calibration and visual validation* of macro topography, drainage structure and biome distribution. These have tens-of-meter resolution, not individual microvoxels. Do not claim the data supplies accurate centimeter-level terrain. Respect licenses and provenance. An ML aesthetic critic may be explored much later, only after geometry/LOD and physically plausible terrain generation are correct; it should not become the optimization target for core data structures.

Existing generator/climate code is a starting point rather than thrown away. Preserve scientifically meaningful large-scale structure and reproducible seeds. Resist building shaders to disguise undesirable landform frequency spectra or repetitive terrain motifs.

### 5.3 Arbitrary 3D source model

Define a `WorldSource` that can evaluate a voxel (and bulk/sample volume), plus optional coarse volumetric min/max/occupancy/error summaries. Heightfield terrain is a **specialized source** used for fast outside landscapes; caves, edits, artificial structures, and future cell-life can create fully 3D topology. Top-level renderer must support their union and subtraction with exact semantics, including edited voids below the surface.

## 6. Rendering: choose the winner per scene, do not guess

### 6.1 Recommended initial *production* path: efficient rasterized surfaces

The current 760M data favors mesh over the existing ray path. Optimize that first:

- Generate occupancy masks using packed 64-bit rows and neighboring halo. Compute exposed-face masks through shifts/AND-NOT, skipping interior solid cells in bulk; adapt **Binary Greedy Meshing v2** principles.
- Use fast linear/locality-friendly sweeps and appropriate greedy merging, or compact per-face instance data when merging would cost more than it saves. First benchmark flat, jagged, painted and edited terrain separately.
- Represent faces by small packed GPU records and shared GPU vertex/index structures (vertex pulling or instanced quads). Use a compact material/appearance atlas *only when needed*, not a `u32` color for every exposed cell if color can be computed or palette-indexed.
- Pool terrain vertex/index/instance data in GPU arenas with suballocation, persistent staging and deferred reclamation. Coalesce many small parts and resource updates, avoid bind group/buffer per 2m tile.
- Culling: CPU coarse frustum + hierarchical conservative occlusion; then GPU compute/indirect culling where hardware support/performance warrants. Use meshlet/cluster groupings only when profitable. Keep a simpler batched draw path for older/integrated GPUs; no mandatory mesh shaders.

The published binary greedy meshing project reports tens to hundreds of microseconds for specific 64³ chunks on a Ryzen 3800X. This **is not** an expected number for our complex climate/terrain/world-source pipeline, nor a claim that each 16m region can be meshed in that time.

### 6.2 Parallel research path: 3D hierarchical ray traversal

Build independent identical-scene GPU tests using column/runs, SVO, SVDAG and perhaps brick occupancy masks. Relevant methods: Laine & Karras (2010) compact octrees; Kämpe, Sintorn & Assarsson (2013) DAG sharing; Aokana (2025) shallow DAGs, GPU-driven LOD and screen-tiled rendering. Consider NAADF (2026) a research reference for later ray tracing/lighting, not a reason to force path tracing onto a low-end baseline.

Only promote octree ray traversal where end-to-end *full rendered frames*, material correctness and edit/update cost beat rasterization, e.g., caves and richly volumetric content. Track topology and shading memory independently. Hardware ray tracing/CUDA must be optional; generic compute shaders are not enough evidence of better end-to-end speed.

### 6.3 Hybrid geometry is a policy, not a fragile collection of exceptions

Suggested initial dispatch:

- **Near open terrain and buildings:** compact raster surfaces/bitmask greedy mesh for exact cells.
- **Caves, overhangs, arbitrary complex 3D:** exact compact raster or SVO/SVDAG compute traversal, selected by profiling and occupancy complexity; both must obey identical picking/edit results.
- **Far open terrain:** nested, independently streamed geometry-clipmap-like terrain height/color hierarchy; coarse triangles permitted **only as visual proxies**.
- **Far arbitrary voxel assemblies:** 3D occupancy/surface summary hierarchies, simplified voxel or mesh clusters, instancing when repeated geometry appears. Do not rely on distant heightfields to represent bridges, caves, towers or future cellular structures.
- **Subpixel detail:** prefiltered appearance/coverage or generic clustered splats when appropriate, never expensive fully expanded cells.

The dispatcher must be explicit, observably measurable, and switched at controlled boundaries. It must not classify objects as 'tree' or 'bowl' to render them efficiently; it may classify the data as heightfield-like, dense, sparse, repeated, or dynamic.

## 7. LOD/streaming: the primary visual breakthrough

### 7.1 Replace independent mesh swaps with a coherent hierarchy

For each visual node maintain: parent reference, child references, physical bounds, occupancy/classification summary, filtered material coverage, approximate (or when available provably conservative) displacement/silhouette error, generation revision, GPU residency handle and last-visible frame. Parent and child derive from consistent world-generation levels, not unrelated noise evaluations.

For outdoor heightfields, derive parent samples from the same hierarchical basis as children; use mip/low-pass matched reconstruction, and make residual high-frequency detail fade in gradually. Correct large-scale shoreline, ridge, river and mountain locations across scales. A heightfield cannot represent a general overhang, so volumetric cells must have their own proxy layer and transition policy.

### 7.2 Selection by projected visual error and stability

- Estimate screen error from projected worst/typical displacement and coverage differences, not simply `voxel_width / distance`.
- Refine if predicted error crosses a configurable split threshold; coarsen only below a significantly lower threshold (e.g. start experimentation at a 2:1 ratio). Incorporate minimum dwell time, speed/direction prediction, frustum and a low-cost look-behind ring.
- Keep the parent resident while children generate; allow children to replace only with complete coverage and valid seams, never a hole.
- Use nested sliding windows or ring clipmaps for heightfield terrain to reuse geometry and update only bands as the camera moves. In-between regions use stable clusters; avoid a complete rebuild of the LOD cover every time the camera crosses a voxel.
- If a parent-child pair has huge topology differences, a cheap opaque low-detail representation remains while detail streams in; hide only the smallest local transition with geometry morph or depth-correct stochastic coverage. Do not crossfade full-screen render images each 200ms planning interval.
- Triangles are allowed in visual morphs; they must be subpixel/visually coherent and not show seams, cracks, anomalous shading, giant flat triangles, or mesh wireframes. Near-field exposed faces remain native blocky voxel surfaces.

### 7.3 Coverage and feature preservation policy

We **do not** require preserving every distant silhouette. We do require no surprising motion-dependent changes to large features, no missing materialized edits that should be visually resolvable, and low-frequency continuity in mountains/shorelines. Aggregate tiny geometry into coverage and average color/light response in linear space. Extreme view distance is possible through coarse models, not through explicit 6.25cm voxel visibility.

### 7.4 Never make the horizon disappear because near LOD is late

Maintain independent budgets for (a) global/continental coarse coverage, (b) mid-distance refinement, (c) near exact geometry, and (d) transition/overlap reserve. Design priority queues for on-screen error reduction, future camera direction, nearby edits and time-to-visible. When overloaded, reduce fine detail, not far visibility. Fade in fully new regions only as a last resort on first load, never as a perpetual player-following fog wall.

### 7.5 Robust GPU resource lifecycle

GPU arena pages with generational IDs; no stale mesh can alias a newer revision. Worker tasks are idempotent, revisioned, cancellable and locality-batched. Separate CPU build, compression, transfer and render submission measurements. Retain old proxy until replacement is resident and safe; reclaim after GPU fences; bound overlap memory. Avoid synchronous readbacks and per-frame `queue.write_buffer` for every terrain object. No allocator churn per visible 2m tile.

### 7.6 Temporal stability test

Automated deterministic flight/walk/turn routes with identical world seed and camera path capture video/frames, GPU timing, camera velocity, LOD events, projected error, resource bytes and frame histograms. Include: a still camera; small back-and-forth movement at a level threshold; slow orbit; sprint; rapid yaw; zoom/FOV change; low-memory pressure; distant snow ridgeline; cave entrance; edited tower; waterline. Score per-pixel temporal difference after camera-motion compensation, coarse/fine silhouette IoU and subjective labeled popping. An image-only beauty score is insufficient; measure videos.

## 8. Simulation-friendly interface, without prematurely building a physics engine

Expose only generic APIs initially:

```rust
struct CellCoord { x: i64, y: i64, z: i64 }
struct CellValue { material: MaterialId, state: CellState }
struct CellChange { pos: CellCoord, old: CellValue, new: CellValue }
trait WorldRead {
    fn get_cell(&self, pos: CellCoord) -> CellValue;
    fn query_region(&self, bounds: CellBounds, out: &mut dyn CellSink);
}
trait WorldWrite {
    fn apply_batch(&mut self, changes: &[CellChange]) -> Result<Revision, WorldError>;
}
trait RegionSource {
    fn sample_coarse(&self, physical_bounds: WorldBounds, footprint: Footprint) -> VisualSummary;
}
```

The code above is an **interface sketch, not drop-in compiling Rust**. Physical state, energy, heat, local event rules and physics solvers can be added in a separate crate. Updating one living voxel invalidates only affected sparse blocks and visual ancestors. A seed can be one voxel; no game-specific organism hierarchy is required. The engine does not need to implement organisms or fluids to prove this capability.

No hardcoded `Tree`, `Bowl`, `Seed`, `Pouring` logic in the world/render core. Entities may be generic connected cell structures if needed for future moving matter; using an acceleration structure for a connected group does not create subvoxel physical matter.

## 9. Low-end hardware and capability tiers

- Baseline 6.25cm voxel *semantics* on all tiers. A low-end renderer is allowed a coarser **display** beyond a smaller radius; it does not change physical voxel size.
- Tier 0: portable wgpu graphics, inexpensive raster near-field, direct coarse far-field, simple material colors, no mandatory compute-only trick/mesh shader/hardware RT. Validate actual downlevel feature compatibility.
- Tier 1: compute-assisted culling, GPU indirect draws, palette/material atlas, more pages, temporal AA where inexpensive.
- Tier 2: optional SVO ray path, advanced mesh/compute batching, higher quality/LOD budget. Future lighting only after baseline signoff.
- Experimental: lower physical voxel sizes and advanced ray/path tracing should share one canonical data model, but may trade performance substantially.

Suggested **goals to validate**, not measured performance promises: 1080p 60 FPS on a defined modest iGPU, with a scalable 720p 30 FPS older-GPU target; p99 spikes and streaming latency matter as much as averages. Define exact Intel/AMD hardware, VRAM/shared memory and driver versions before calling something 'potato compatible'. Target an explicit GPU-residency ceiling (configurable ~96–256 MiB for baseline graphics data) and separately account for all CPU caches, temporary geometry, render targets, driver memory and journal. Never confuse one pool's bytes with whole-process memory.

## 10. Research inspirations and what NOT to infer

| Project / paper | Adopt | Important limit |
|---|---|---|
| **Cubiquity** (David Williams; CC0) | Mutable sparse voxel DAG, material ID, efficient exact-volume queries, compressed scenes, voxelization tooling | Experimental visual renderer; author warns bulk on-the-fly per-voxel `setVoxel` generation is prohibitively slow |
| **Binary Greedy Meshing v2** (cgerikj) | Packed 64-bit occupancy/face masks, rapid interior elimination and merging, tiny quad records | Published microbenchmarks do not equal full-world terrain generation or your final GPU FPS; check license before copying code |
| **Aokana** (Fang et al., 2025) | GPU-driven voxel rendering, SVDAG, LOD/streaming, screen-space/tiled traversal | Evaluated research renderer; source/edits/transparent-material support are not a drop-in mutable-world solution |
| **NULLSEED** (developer diary) | Practical 6.25cm ambition, sparse adaptive octree, Vulkan custom engine, far-distance hybrid proxy techniques, view-range design | Published technical notes, not a verified open-source repository; developer performance is not measured on our hardware |
| **Geometry clipmaps** (Losasso–Hoppe 2004; GPU Gems 2, 2005) | Nested viewer-centered grids, incremental updates, low-frequency coherence, smooth geometry morphs | Classic heightfields do not represent caves, buildings, or overhangs |
| **Project Ascendant** (vkguide) | Multiple geometry backends near/far, GPU-aware batching, practical Vulkan design | Smooth near terrain deviates from exact block faces; prototype graphics quality/performance not a guarantee |
| **Enshrouded 1.0** (October 2026) | Visual goal for long-distance edited builds and terrain; new 'brick rendering' update shows importance of preserving distant geometry | Proprietary rendering technique details cannot be invented from patch notes |
| **Laine & Karras SVO** (2010) | Compact 3D occupancy traversal and filtering ideas | Not the ideal default for ordinary open terrain on our measured laptop |
| **Kämpe et al. SVDAG** (2013), **HashDAG/GPU-SVDAG-Editing** (2024) | Shared subtrees, compressed frozen world, methods for mutable compressed geometry | Editing/decompression and attribute overhead must be measured; 2024 GPU implementation uses CUDA and is not a portable drop-in |
| **NAADF** (Ulschmid et al., 2026) | Hierarchical cached ray traversal, temporal stability for voxel worlds, future GI | Path tracing/GI-oriented; not a baseline rationale to replace fast raster on budget hardware |
| **Schott et al. multiscale erosion** (SIGGRAPH 2024) and **Tzathas et al. analytical erosion** (Eurographics 2024) | Morphologically plausible multiscale landforms without expensive per-frame erosion | Ground truth for synthetic terrain is approximate and must be calibrated; not a voxel data structure |

**Research principle:** do not claim a single universally fastest algorithm. Different publications use different hardware, scene composition, lighting, edit rates, quality settings, and memory measures. All architecture decisions require matched end-to-end experiments on our three canonical scenes.

## 11. Prototype bakeoff and falsifiable gates

Create reproducible test fixtures by seed, coordinates, camera track, change journal and known pixel output. The critical three content classes:

- **A: open rugged terrain:** 10km × 10km procedural world, broad mountain range, steep relief, rivers/shorelines. Primarily heightfield-like; 6.25cm canonical grid and direct coarse source.
- **B: arbitrary 3D:** cliffs with caves, arches, underground tunnels, floating formations and a hand-edited tower, with material diversity.
- **C: repetitive/dynamic:** many repeated voxel patterns, densely detailed irregular vegetation-like clusters, batch additions/removals and random single-cell edits; no hardcoded tree logic.

At a minimum benchmark 6.25cm, then 3.125cm and 1cm; run 1mm **architectural correctness/stress tests only** until proven practical. At each setting test 720p, 1080p, 1440p where hardware permits, camera static/walk/sprint/orbit, sunny no fog, cold launch vs warm cache, high/low memory, optional camera interpolation, no-edit vs editing workloads.

For each renderer record: true whole-process RAM/peak, total GPU resident bytes/peak, triangle/quad/visible cell counts, CPU main-frame p50/p95/p99, GPU timestamp time where available, terrain generation p50/p95/p99, worker backlog and cancellations, disk/CPU transfer, GPU upload MB/s and stall time, edit-to-visible latency, LOD switches per second, visibility misses, and image/video temporal changes.

**Representation experiment:**

1. Bitwise greedy raster baseline vs existing `FaceCollector` on identical equal-area surface and exact color features.
2. Native volume bricks (dense/palette/runs), SVO and immutable SVDAG: end-to-end build, compressed bytes, edited bytes, point/range query, CPU memory, full-frame GPU time.
3. Full pipeline with camera-relative shared GPU arenas vs per-part buffers.
4. Terrain exact mesh vs clipmap coarse proxy with dynamic edit overlays.

Promotion gate: a renderer must win in the **relevant workload end to end**, not just in a 4096-ray kernel. If octree traversal loses on general terrain, do not make it the primary renderer. If bitwise meshing drastically reduces generation CPU but GPU submission still dominates, solve drawing and allocation next.

### Proposed acceptance criteria (starting targets; tune by measured hardware)

- Exact canonical queries unchanged under all rendering qualities; automatic random-coordinate and edit replay diff tests pass.
- The camera can walk/sprint/turn near a LOD threshold without visible geometry holes, repeated flickering, sudden mountain movement, or forced far-distance fading; record full video with fog disabled.
- Native surfaces retain voxel block silhouettes nearby; distant terrain can be smoothed/filtered when subpixel without revealing large triangles or lighting discontinuities.
- Bounded memory and upload capacity under fast movement and adverse edits; no runaway backlog, unbounded retries, or resource leaks.
- Low-end native 6.25cm performance passes documented scene-specific p50/p95/p99 gates; no universal '60fps everywhere' claim.
- A sparse update to N cells costs work proportional to locally affected data and necessary ancestor invalidations, not total world area or total existing cell count.
- Full 3D edits reflect correctly in the near renderer and cached distant proxies after a bounded update delay.
- A simple generic cell simulation can create and remove arbitrary cells without code changes to renderer or generator.

## 12. Build stages, deliverables and stop/go decisions

### Stage 0 — freeze and reproduce old engine

- Tag baseline commit; snapshot generator seeds, world semantics, test captures, profiling scripts.
- Add reproducible simple scenes (flat checkerboard, noise cliff, mountain valley, cave, edited structure) and machine-readable ground-truth occupancy / material checks.
- Establish actual hardware tier and profiling protocol. Build automated camera paths and capture videos.
- **Gate:** old engine repeatably benchmarkable; correctness oracle available. **Do not refactor old renderer.**

### Stage 1 — new canonical world and bulk API

- New workspace crates: `voxel_core`, `voxel_world`, `voxel_gen`, `voxel_edits`, `voxel_render_data`, `voxel_gpu`, `voxel_stream`, `voxel_app`, `voxel_bench` (adjust boundaries for build simplicity).
- Define coordinate types, fixed per-world physical scale, world source query, chunk/brick storage adaptive variants, transactions, revisions and serialize/deserialize.
- Port existing procedural generation behind query/summarize APIs without copying current renderer's mesh format.
- **Gate:** exact cell and edit replay tests pass; generation requires no dense whole-world allocation; bulk edits and queries are benchmarked.

### Stage 2 — low-end GPU raster prototype

- Minimal window and camera; intentionally flat colors, no fog. Port same terrain view.
- Benchmark binary bitmask meshing + palette appearance vs existing mesher, pooling GPU buffers, batched draw packets, lightweight CPU visibility.
- **Gate:** measurable improvement over baseline in mesh creation and CPU/GPU submission; exact nearby geometric and edit parity.

### Stage 3 — stable far terrain and hierarchical LOD

- Implement low-pass consistent generator pyramid plus clipmap-style heightfield proxy; view-centered incremental ring shifts, prefetch, hysteresis, parent residency and morphing where feasible. Retain separate 3D proxy path for non-heightfield content.
- Use movement routes with no fog and compare before/after video temporal metrics. **Stop and fix LOD before adding fancy visuals.**
- **Gate:** no conspicuous motion-induced changes on test paths; stable memory and p99 under movement and turning.

### Stage 4 — volumetric complexity research

- Add full 3D raster edits/caves and SVO/DAG bakeoff; choose single primary path and optional complex-geometry accelerated path, based on actual measured winners.
- Full GPU memory accounting, immutable versions and background compaction; optional hardware-capability features.
- **Gate:** no regressions in normal mountains; complexity-specific speedups backed by GPU timestamps, full-frame tests and edit latency.

### Stage 5 — scalable editing / generic simulation contract

- Bulk edits at arbitrary xyz, priority invalidation, controlled GPU residency updates, persistent journals and crash recovery.
- Add an intentionally generic test automaton that changes cells each tick; verify exact semantics and visual updates. Do not build tree growth or fluids yet.
- **Gate:** arbitrary cell changes are correct, localized and smooth under sustained edits.

### Stage 6 — procedural aesthetic quality (geometry first)

- Improve macro/mesoscale geomorphology and microstructure: drainage, ridge continuity, erosion deposition, varied lithology, material patterns and sampled real-Earth comparisons.
- Demonstrate a compelling mountain valley with unadorned lighting and stable walking/flying. Advanced illumination stays deferred.
- **Gate:** geometry is already beautiful/compositional even with simple flat materials and lighting.

### Stage 7 — graphics polish after fundamentals

- Only now consider physically based material rendering, soft shadows, atmospheric scattering, GI, denoising, vegetation appearance, water optical shaders and cinematic camera. Ensure settings cannot mask uncorrected LOD problems.

## 13. Development practices for Codex

- Every pull request must state hypothesis, relevant reference, exact files, validation tests, measured before/after numbers and hardware, and known limitations. No performance assertions from synthetic timings alone.
- Keep isolated experiments in `experiments/`, not branching hot production paths. Feature flags, config version IDs, deterministic seed/track.
- Follow a source audit before reuse; no blind copy of Vulkan/CUDA-only algorithms into WGSL. Check all licenses/attribution and transitive dependencies.
- Every new spatial operation must have a property test using negative coordinates, boundary cases, arbitrary edits and size change.
- Keep layers independently measurable; no work queues with hidden sleeps/automatic retries, no unbounded caches, no per-frame global procedural resampling, and no synchronous GPU readbacks in normal gameplay.
- Favor the simplest proven fast path. If two sophisticated representations lose to one bitset, ship the bitset.

### Initial Codex task queue

1. Read current `src/grid.rs`, `src/world.rs`, `src/storage.rs`, `src/generation`, `src/render/{lod,terrain,surface}.rs`, `native/src/{runtime,gpu,camera,presentation.wgsl}`, and profiling docs; emit an *invariant/assumption map* and current baseline instructions.
2. Generate `ARCHITECTURE.md`, `PERFORMANCE_CONTRACT.md`, `WORLD_SEMANTICS.md`, `RESEARCH_REFERENCES.md`, and `TEST_SCENES.md` in the new workspace, based on this document. Do not implement rendering yet.
3. Implement canonical core types/scale and exact source+overrides with property tests and persistence.
4. Implement independent paired raster meshers with equal-area microbenchmarks and exact-surface comparison. Do not include lighting beyond a flat directional color.
5. Build pooled GPU raster baseline with reproducible native 6.25cm movement benchmark.
6. Start multiscale LOD prototype; demonstrate video comparison without fog; block later work until stable.

## 14. Explicit anti-goals

- Not an engine that simulates every 6.25cm voxel on every physics tick.
- Not a universal DAG, universal SVO, universal per-cell cube renderer, or a CPU mesh builder tied to a fixed number of physical voxels per visual tile.
- Not a photo-realistic lighting project masquerading as a voxel foundation rewrite.
- Not a 1mm-all-the-way-out rendering project.
- Not a terrain-only heightfield world that cannot represent caves, floating matter, or arbitrary edited structures.
- Not fog used to conceal LOD errors.
- Not an ML aesthetics model making performance-engineering decisions before deterministic correctness and frame benchmarks.
- Not a claim that modern hardware makes any voxel resolution or edit rate free.

## 15. Primary sources and code (checked as of 2026-10-29)

### Current engine

- [MicroVoxelProceduralGeneration](https://github.com/DrewPhi/MicroVoxelProceduralGeneration); [performance](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/agent_context/PERFORMANCE.md); [profiling](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/agent_context/PROFILING_AND_CLEANUP.md); [LOD research](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/agent_context/LOD_RESEARCH_AND_PLAN.md); [SVO/DAG study](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/agent_context/OCTREE_DAG_STUDY.md).

### Major rendering/storage sources

- [Cubiquity, open-source CC0](https://github.com/DavidWilliams81/cubiquity).
- [Binary Greedy Meshing v2, source/benchmarks](https://github.com/cgerikj/binary-greedy-meshing).
- [Aokana (Fang, Wang & Wang, 2025)](https://doi.org/10.1145/3728299), [arXiv](https://arxiv.org/abs/2505.02017).
- [NULLSEED developer diaries](https://nullseedgame.com/).
- [Losasso & Hoppe, Geometry Clipmaps (SIGGRAPH 2004)](https://hhoppe.com/proj/geomclipmap/), [GPU Gems 2 (2005)](https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry).
- [Laine & Karras, Efficient Sparse Voxel Octrees (2010)](https://research.nvidia.com/publication/2010-02_efficient-sparse-voxel-octrees).
- [Kämpe et al., High Resolution Sparse Voxel DAGs (2013)](https://doi.org/10.1145/2461912.2462024).
- [HashDAG / GPU-SVDAG editing source](https://github.com/mathijs727/GPU-SVDAG-Editing) and [HashDAG source](https://github.com/Phyronnaz/HashDAG).
- [NAADF, TU Wien (2026)](https://www.cg.tuwien.ac.at/research/publications/2026/ulschmid-2026-naadf/).
- [Project Ascendant, VKGuide](https://vkguide.dev/docs/ascendant/ascendant_geometry/).
- [Enshrouded 1.0, Oct 14, 2026 release notes](https://kg-www.keengames.com/en-US/news/release-trailer-changelog-everything-you-need-to-wait-for-tomorrow).

### Terrain and reference data

- [Terrain Amplification using Multi-Scale Erosion (2024)](https://h-schott.github.io/p/mserosion/).
- [Physically-based analytical erosion for fast terrain generation (2024)](https://www-sop.inria.fr/reves/Basilic/2024/TGSC24/).
- [Copernicus DEM GLO-30](https://dataspace.copernicus.eu/explore-data/data-collections/copernicus-contributing-missions/collections-description/COP-DEM).
- [ESA WorldCover](https://esa-worldcover.org/en/data-access).
- [MERIT Hydro](https://global-hydrodynamics.github.io/MERIT_Hydro/).

---

**Bottom line:** choose a **data-driven hybrid renderer with cheap procedural far fields and exact sparse near fields**. For portability and measured laptop performance, start with fast CPU bitmask meshing + batched GPU rasterization, then experimentally add SVO/DAG traversal where complex volumetric scenes justify it. Build a coherent multiscale LOD/streaming system *before* lighting. Preserve one exact discrete physical universe under every visual shortcut. This is the best-supported engineering hypothesis from the inspected code and research, not a proof of universal optimality.


## 16. Provenance, confidence levels, and how to use the external references

This section makes the research material *operational*. Agents should open the cited source when implementing that subsystem and reproduce its assumptions. A research paper proving a speedup on one scene is **not proof** of our expected performance; a successful commercial game screenshot is **not an algorithm specification**. The date limit for this document is **October 29, 2026**. Do not silently substitute a later paper, repository revision or library API for material known at this cutoff.

**Evidence grades:** **A** = source code and/or reproducible measurement for the directly relevant technique; **B** = published algorithm with detailed method and experiments, but results are hardware/workload dependent; **C** = engineering diary/production documentation whose implementation or benchmarks cannot be independently replicated; **D** = our untested integration hypothesis. Adopt A/B as prototypes and use C as design/visual guidance; measure all D hypotheses.

| Design requirement / flaw | Primary source to read | Concrete code or algorithm to inspect | Evidence and caveat |
|---|---|---|---|
| CPU exposed-face processing is expensive | [Binary Greedy Meshing v2](https://github.com/cgerikj/binary-greedy-meshing/blob/master/README.md), [0fps meshing explanation](https://0fps.net/2012/06/30/meshing-in-a-minecraft-game/) | Occupancy bitmasks, face-mask AND/NOT, greedy compression, vertex pulling, neighbor halo | **A** for implementation and published microbenchmarks; **D** for our speedup |
| Many CPU-driven draw calls | [Project Ascendant rendering](https://vkguide.dev/docs/ascendant/ascendant_geometry/), [Khronos GPU-driven indirect tutorial](https://docs.vulkan.org/tutorial/latest/Advanced_Vulkan_Compute/07_GPU_Driven_Pipelines/04_multi_draw_indirect_mdi.html) | Retained draw lists, large suballocated buffer, indirect draw commands, GPU visibility/count buffers | **A/B** for mechanism; must check portable `wgpu` features |
| Visible distant LOD switches | [Geometry Clipmaps (Losasso/Hoppe)](https://hhoppe.com/proj/geomclipmap/), [GPU Gems 2 implementation](https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry) | Nested regular grids, incremental toroidal updates, low-frequency coarsening, transition-band vertex interpolation | **B** for heightfield terrain; **D** for exact voxel/3D overlay adaptation |
| Far 3D edits and cave geometry | [Godot Voxel Tools source](https://github.com/Zylann/godot_voxel), [smooth terrain/LOD docs](https://voxel-tools.readthedocs.io/en/latest/smooth_terrain/), [Transvoxel](https://transvoxel.org/) | Paged 3D sources, edit persistence, seam construction, coarse-data updates | **A/B**; smooth LOD features do not automatically apply to blocky mode |
| Mutable compressed representation | [Cubiquity](https://github.com/DavidWilliams81/cubiquity), [HashDAG research](https://graphics.tudelft.nl/Publications-new/2020/CBE20/ModifyingCompressedVoxels-main.pdf), [GPU SVDAG editing](https://github.com/mathijs727/GPU-SVDAG-Editing) | Subtree interning, material separation, copy-on-write, GPU hash/edit batch and reclamation | **A/B**; Cubiquity advises against per-cell on-demand generation |
| Efficient complex volumetric visibility | [Laine–Karras paper and implementation notes](https://research.nvidia.com/publication/2010-02_efficient-sparse-voxel-octrees-analysis-extensions-and-implementation), [Google Code archived open-source implementation](https://code.google.com/archive/p/efficient-sparse-voxel-octrees), [Aokana](https://doi.org/10.1145/3728299) | Sparse 3D traversal, shallow/chunked DAGs, separate geometry/color payloads, screen-tiled GPU work | **A/B**; the 2010 reference is legacy CUDA; Aokana lacks drop-in dynamic edit support |
| Efficient ray traversal under complex scene | [NAADF (Eurographics 2026)](https://www.cg.tuwien.ac.at/research/publications/2026/ulschmid-2026-naadf/) | Nested axis-aligned distance-field caches, edit updates, hierarchical ray traversal | **B**; GI/path-tracing workload, not proof against our mesh backend |
| Practical 6.25 cm engineering / huge draw range | [NULLSEED developer log](https://nullseedgame.com/) | Sparse adaptive octrees; meshlet/task-shader path; HZB; distant heightmap + low-poly structure proxies | **C**; developer-reported RTX results; **not verified as public source code** |
| Scene-dependent hybrid architecture | [Project Ascendant](https://vkguide.dev/docs/ascendant/ascendant_geometry/) | Near quantized voxel meshes, far point/raycast draw, retained GPU allocations, region/chunk tradeoff | **A/C**; article details are more reproducible than screenshot claims |
| Distant edited buildings retaining visual detail | [Enshrouded Oct 14, 2026 official notes](https://kg-www.keengames.com/en-US/news/release-trailer-changelog-everything-you-need-to-wait-for-tomorrow) | Developer-described “brick rendering” for distant terrain/buildings, *not* a published data format | **C**; idea and goal only. Do not invent compression/traversal internals |
| Terrain looks artificial despite sufficient voxels | [Schott et al. 2024 erosion](https://h-schott.github.io/p/mserosion/), [Tzathas et al. 2024 analytical erosion](https://www-sop.inria.fr/reves/Basilic/2024/TGSC24/) | Multi-scale erosion/deposition and analytical stream-power geologic structure | **B**; geometry-quality work after renderer stability, but prior to sophisticated lighting |
| Visual quality even with modest geometry | [PBRT 4e sampling theory](https://www.pbr-book.org/4ed/Sampling_and_Reconstruction/Sampling_Theory), [0fps voxel AO](https://0fps.net/2013/07/03/ambient-occlusion-for-minecraft-like-worlds/) | Filter frequencies before downsampling; later AO/shading separate from mesh topology | **B**; illumination effects deferred from the initial project |
| GPU portability | [Current wgpu feature docs](https://docs.rs/wgpu/latest/wgpu/struct.Features.html), [Khronos Vulkan MDI](https://docs.vulkan.org/tutorial/latest/Advanced_Vulkan_Compute/07_GPU_Driven_Pipelines/04_multi_draw_indirect_mdi.html) | Detect actual `MULTI_DRAW_INDIRECT_COUNT`, `INDIRECT_FIRST_INSTANCE`, shader/adapter limits, fallback path | **A** for public API; features vary by adapter/version |

**Open-source and license safety:** Cubiquity's core is **CC0** per its repository. Voxel Tools is MIT; VoxelCore is MPL-2.0; the older SVO proof-of-concept is in the Google Code archive with New BSD license. The `binary-greedy-meshing` project provides public source, but **do not assume public visibility confers permission to copy**; inspect the precise repository revision's licensing and all bundled third-party code. The Aokana paper, NULLSEED development log, commercial Enshrouded and Atomontage showcase are **ideas and evidence**, not blanket permissions to import their code. License reviews belong in PR checklists.

### 16.1 Where to look in the existing code, precisely

The current repo is [DrewPhi/MicroVoxelProceduralGeneration](https://github.com/DrewPhi/MicroVoxelProceduralGeneration). Agents should compare code at a pinned baseline commit, not merely at `main` once changes begin.

- **Integer world:** [`src/grid.rs`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/src/grid.rs), [`src/world.rs`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/src/world.rs), [`src/storage.rs`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/src/storage.rs). Look for hardcoded `16`, 32³ brick indices, persistent override precedence, edits, revision semantics and numeric range assumptions.
- **Generation:** [`src/generation/mod.rs`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/src/generation/mod.rs). Preserve exact generation queries, native voxel identities, material assignment, regions and climate distribution; move renderer-dependent sample policy out of world truth.
- **Old LOD:** [`src/render/lod.rs`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/src/render/lod.rs). Audit `SIDE=32`, `ROOT`, `FINEST`, `EXTENT`, `select_adaptive`, hard minimum native radius, tile budget threshold inflation and `next_patch_budget`.
- **Old mesher:** [`src/render/surface.rs`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/src/render/surface.rs), [`src/render/terrain.rs`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/src/render/terrain.rs). Observe per-face BTreeMap, per-face color atlas, 262,144 face cap, vertical profile loops and neighbor seam dependencies.
- **GPU:** [`native/src/gpu.rs`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/native/src/gpu.rs). Count actual draw calls, bind group changes, buffers, bytes, and rendering work on replace.
- **Streaming:** [`native/src/runtime.rs`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/native/src/runtime.rs), [`native/src/lod_planner.rs`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/native/src/lod_planner.rs), [`native/src/streaming.rs`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/native/src/streaming.rs). Validate planning cadence, cancellations, retained covers, uploads, memory ceilings and visibility-limit coupling.
- **Presentation/movement:** [`native/src/presentation.wgsl`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/native/src/presentation.wgsl), [`native/src/camera.rs`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/native/src/camera.rs), [`native/src/player_view.rs`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/native/src/player_view.rs). Distinguish image fade from geometry morph and camera grid-steps from actual geometry popping.
- **Measured evidence:** [`PERFORMANCE.md`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/agent_context/PERFORMANCE.md), [`PROFILING_AND_CLEANUP.md`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/agent_context/PROFILING_AND_CLEANUP.md), [`OCTREE_DAG_STUDY.md`](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/blob/main/agent_context/OCTREE_DAG_STUDY.md). Preserve conditions, caveats and how to reproduce.

## 17. Specific algorithms to prototype (not merely names of technologies)

### 17.1 Canonical world and spatial coordinates

1. One `WorldId` records base-cell edge as an exact rational scale (the initial case is 1/16 meter). Native occupied coordinates are signed `i64` triples. Use `div_euclid`/`rem_euclid` for negative coordinates and half-open cell/brick bounds.
2. Render coordinates should use **region-relative origins** before conversion to GPU `f32`, so worlds remain numerically stable as global cell coordinates exceed the exact range of 32-bit floats. Keep a `f64` or fixed-point camera for optional *visual-only* smoothing; collision and occupancy remain on integer cells.
3. Canonical query semantics: `edit_overlay.get(coord).unwrap_or(generator.native_cell(coord))`, with a distinct representation for an edit that explicitly removes an otherwise solid procedural voxel. `None` in the overlay means “defer to generator,” while an explicit `Air` means “carved empty.”
4. Every source must support `sample_native_batch(bounds)` and `sample_coarse(bounds, footprint, revision)`; the latter is **not** an authoritative materialization. Arbitrary edited cells may make analytic summary bounds invalid, so edit overlays need conservative occupancy/material/height deltas.
5. Bulk transaction events carry a monotonic revision and dirty AABBs. Maintain event logs and replay tests before introducing replication or concurrency. Do not design runtime physics now, but any future system must be able to issue ordinary world edits.

### 17.2 A fast, correct native raster mesher

Use a physically bounded **brick** of native cells (e.g. initially 32³ with a one-cell neighbor halo), not one 32-cell-wide **LOD tile** whose physical size also sets all streaming and draw granularity. At 6.25 cm a 32³ brick spans 2 meters; many bricks can share a much larger physically-sized render region and a common GPU buffer. The fact that native bricks shrink with voxel resolution is acceptable; **render jobs, draws, clipmap windows, and regional generation must not inherit the same shrinking**.

Algorithm prototype:

```text
for each required native brick plus 1-cell halo:
    material_or_generator -> occupancy_bitplanes + optional palette indices
    for each of 6 face normals:
        visible = occupied AND NOT shifted_neighbor_occupied
        separate material-sensitive boundaries if genuinely visible/necessary
        greedy_pack(visible, material/appearance constraints)
    emit compact quads (or measured alternative) into a shared render arena
    mark generated bounds / revision / dependencies
```

Specific caveats: an occupancy mask suppresses faces between two solid materials; if the material model requires their interface visible (e.g. transparency or cutaway), that must be a different pass. If different colors force each adjacent face to remain separate, the color format affects greedy merging; benchmark palette ID, procedural per-cell color, and per-face atlas formats independently. Never silently change exact near colors to improve a benchmark. Use 64-bit word parallelism and contiguous arrays, not per-face tree maps. Benchmark pathologically irregular geometry against simplistic flat scenes.

On the GPU, pack quad extents and material palette indices into small records; vertex pulling or instance expansion can reconstruct ordinary cube faces. Use a global/segmented GPU arena, suballocator, and per-region offset table. Multiple native bricks are drawn as one or few batches. **Do not unconditionally enable `multi_draw_indirect_count`**: check adapter support first, and keep a robust batched `draw_indirect` or direct draw fallback. Measure the cost of generating draw commands against GPU gains.

### 17.3 A genuinely coherent terrain LOD hierarchy

The key is **one coherent multiscale description**, not five independent samples that happen to use larger steps. To avoid a suddenly displaced hill:

- Define an authoritative large-scale terrain field `H_low` for each parent grid and a finer residual `R`, with `H_fine = upsample(H_parent) + R` in a *procedural description*, then quantize the final canonical height/material/occupancy only when querying native cells. All levels must share the same low-frequency foundation.
- For display, evaluate or cache filtered `H` and material/coverage summaries over each footprint. Avoid aliasing high-frequency noise into coarse samples. Use low-pass filtering of regional fields rather than evaluating a new noise function with arbitrary parameters per level.
- Optionally quantify parent-to-child residual amplitude or conservative min/max elevation. For procedural noise with known maximum amplitude per octave, derive an explicit upper bound; otherwise label the error as sampled/estimated. **Never equate a sampled variance with a certified max silhouette error.**
- A visible terrain vertex inside a defined transition band may morph between the coarse and fine visual surfaces using an interpolation factor depending smoothly on position within the band. The canonical material at the native cell does **not** change.
- Coarse/fine face ownership and crack-free transition geometry must be verified separately from the apparent visual popping. Morphs alone cannot fix holes, hidden features, or arbitrary topological differences.
- For a heightfield-dominated outdoor world, a fixed nested 2D clipmap ring structure is a good starting point. For all other content, provide a separate 3D hierarchical overlay/proxy, so towers, arches, caves and future emergent structures remain possible without converting them to heightfields.

**Edit semantics at a distance:** keep a revisioned per-region dirty tree. An edit to native cell `(x,y,z)` invalidates its local exact brick and each coarse presentation node whose support footprint includes the change. Far summaries should include edited *addition and subtraction*; avoid a full native-volume rewalk by maintaining a sparse edit-derived delta/overlay, then consolidating if edits become dense. When a projected edit is smaller than a pixel, an average or coverage change may be more appropriate than forcing a far mesh subdivision. This may deliberately remove tiny features from distant images, as the user permits.

### 17.4 Streaming/residency as a state machine

Each renderable region and LOD representation should progress through an explicit lifecycle:

```text
ABSENT -> REQUESTED -> BUILDING -> CPU_READY -> UPLOADING
     -> GPU_READY -> VISIBLE -> RETIRING -> RECLAIMABLE
```

Each stage records the world revision and generator version. Out-of-order results with old versions are discarded, not installed. Prior visible parent data persists through replacement, and GPU buffers cannot be reclaimed until all in-flight frames that reference them complete. Allocate a separate transition memory reserve; if it is exhausted, keep the old coarser representation rather than emit holes or thrash allocations.

Priority should be approximately a function of (i) missing near-field exact collision/display requirements, (ii) visible projected **benefit per estimated build/upload byte**, (iii) screen coverage, (iv) camera velocity prediction, (v) recent edit urgency, and (vi) reuse likelihood. Use capped queues, cancellation and per-frame CPU/GPU upload budgets. Far-field parent coverage is a separate always-available stream, not the remainder after near-field cache pressure.

### 17.5 Compression / Cubiquity bakeoff

Implement adapter candidates with identical query semantics:

```text
A) untouched procedural function + sparse explicit override map
B) sparse/palette-packed/dense brick with occupancy mask
C) immutable/frozen SVO for 3D complex geometry
D) hash-consed SVDAG built from frozen region snapshots
E) edited region + immutable DAG/background compaction hybrid
```

For each candidate measure native point queries, arbitrary range traversals, serialized bytes, resident CPU/GPU memory, 1/100/10,000 edits, edited-region invalidation, building from procedural sources, and full-frame traversal. Compression is useful only if all necessary build/update, transfer and rendering costs are included. On the existing two terrain examples, DAG geometry compressed well but traversal was slower than columns; in the carved cave, SVO and DAG were both much faster. Thus the initial hypothesis is **mutable bricks first; DAG as optional cold/static compressed geometry**, not a mandatory canonical root. Cubiquity's mutable DAG and Molenaar/Eisemann's GPU editing are targeted research branches for changing that decision **if** experiments prove superiority.

### 17.6 Make generation cheap at arbitrary scale

Do not allocate a 10 km world in native voxels. Store compact regional landform/climate parameters and solve expensive macro erosion at low or intermediate grid resolutions. The canonical native generator calculates exact cell occupancy when needed; the far generator directly produces low-pass terrain summaries without enumerating all leaf cells. A deterministic material selection function may use hash(seed, native coord), so changing worker order does not alter the physical world.

A practical hierarchy might start with: continental/regional fields (kilometers) → drainages/ridges (hundreds to tens of meters) → erosion and sediment/rock layers (meters) → native material noise/detail (centimeters). **Those are spatial frequencies in procedural rules, not physically existing fractional cells.** Never infer centimeter terrain accuracy from Copernicus 30 m DEM, WorldCover 10 m maps, or other geospatial reference images.

## 18. Benchmark protocol that prevents misleading conclusions

### 18.1 Required baseline environments

Create a machine-readable `benchmark_manifest.json` with: Git SHA, Rust/compiler, driver, operating system, graphics API, adapter IDs and memory, world seed/config, coordinate interval, number of edits, time and camera track, frame resolution, field of view, graphics settings, resident budget, cache-warmness, and benchmark executable. Save raw timestamps and CSV; an average without context is not valid performance evidence.

**Scene suite:** an unobstructed plain; rocky alpine mountain panorama with ridgelines and water; mixed vegetation-shaped voxel clusters; cave/overhang and tall cliff; a densely carved and repainted arbitrary structure; a repeating pattern stress scene; and a genuinely irregular high-entropy material stress scene. The last two prevent a DAG/mesher from winning only because the sample is artificially compressible.

**Tracks:** stationary warm draw; cold start with immediate camera motion; slow forward walk; sprint; spin 360° while stationary; back-and-forth at LOD threshold; fly toward a far mountain; edit a near wall; edit a far tower; mass-edit region while flying; deliberately squeeze the GPU residency budget.

**Metrics:** record CPU update, world query, region generation, mesh building, compression, transfer, driver submission and GPU hardware-timestamp time separately. Also track frame p50/p95/p99/max; time to first coherent view; number and length of visible LOD transitions; screen-space temporal differences on motion-compensated video; parent/child silhouette shifts; dropped or out-of-date jobs; edit-to-visible delay; actual total CPU RSS and all GPU allocations (not just terrain arena). The benchmark runner must be deterministic and **repeat trials**, reporting confidence/variation; do not optimize a single run.

### 18.2 Comparison and promotion rules

- **Meshing decision:** compare old collector vs bitmask mesher on identical *native* occupancy and colors; require exact exposed surfaces, no missing edit faces, and improved end-to-end build + upload times. If a 64³ benchmark looks fast but macro terrain generation remains the bottleneck, optimize generation separately.
- **Renderer decision:** compare packed raster, existing column ray, SVO and DAG on the **same rendered scenes and pixel settings**. A 4,096-ray microkernel is supportive only; it cannot win the production decision without full-frame measurements.
- **LOD decision:** compare no-fog walking **videos**, not screenshots. Image metrics and human review both matter. Fix oscillation, holes and large silhouette shifts before claiming seamlessness.
- **Hardware decision:** a low-end tier requires a specific old GPU and feature fallback. Modern Radeon 760M measurements alone cannot establish “potato” support. Proposed acceptance tiers are targets, not physics limits.
- **Error/quality decision:** geometry approximations may intentionally discard very distant detail. Explicitly control its screen-space footprint; do not mislabel a sampled-only error predictor as mathematically guaranteed.

### 18.3 Stop conditions

If a candidate introduces wrong canonical cell queries, non-deterministic edits, disappearing far mountains, revision races, render holes, unbounded memory, or continuous LOD flutter, reject it regardless of peak frame rate. If a more complex structure gives only a small, noisy performance gain against packed raster on the actual low-end target, prefer packed raster for maintainability and iteration speed.

## 19. Documentation and handoff contract for autonomous implementation agents

Before coding a subsystem, an agent must produce a short design record:

```markdown
# Experiment / PR: <name>
Old-engine defect and reproducible case:
Canonical invariants affected:
Primary external algorithm/source (direct URL; exact commit if copying code):
Other plausible alternatives:
Hypothesis (expected improvement; not claimed result):
Scope and file changes:
Benchmark scene, hardware, settings and success threshold:
Correctness checks (native cells, colors, edits, boundaries, negative coords):
Rollback/feature flag:
Observed result (before/after, p50/p95/p99, variance, actual memory):
Conclusion: promote / iterate / reject
```

**Working rules:**

1. First inspect relevant current files and the named external references. Do not synthesize an alleged implementation from an article headline.
2. Keep the old repo untouched except for adding reproducible benchmarks or oracle exports. Create the successor in a separate workspace/repository or a clearly isolated prototype directory. Pin old baseline and external source commits.
3. Establish tests *before* optimizing. Preserve native occupied cells, exact material identities, carving and placement, seed determinism, and negative/boundary coordinates.
4. Benchmark with a release build, fixed camera track and independent GPU/CPU timing. Never infer total GPU performance from triangle count or one ray microbenchmark.
5. Implement the minimal portable backend first. Experimental Vulkan or hardware-specific paths are optional forks, not requirements for starting the engine.
6. Run replay/serialization, streaming pressure, edited cave, cross-region boundary, and sudden camera reversal tests on every representation change.
7. Avoid per-frame full-world iteration, dense total-world buffers, per-face balanced maps, per-tile GPU buffer churn, and post-processing used to disguise geometry errors.
8. All experimental new optimizations require a clear fallback and a stop/go memo. If the old mesher or a simple brick outperforms the fancy algorithm, keep the simpler path.

### First five Codex prompts, in order

**Prompt A — Audit and fixtures.** “Read this master plan and the pinned original repository. Produce `BASELINE.md`, exact voxel semantics tests, deterministic fixture exports, and camera-path performance scripts. Do not redesign or optimize yet. Record raw baseline results and distinguish simulated vs fully measured timings.”

**Prompt B — Core.** “Implement fixed per-world voxel scale, signed integer grid, deterministic native generator interface, sparse edit overlay with explicit air carving, bulk query and edit APIs, versioned journal, and negative coordinate/overflow property tests. No rendering yet.”

**Prompt C — Native raster bakeoff.** “Implement a CPU bitmask/greedy mesher for 3D brick+halo data and packed shared GPU draw buffers. Compare to old mesher on identical data; document exact surface/color parity, mesh build/upload times, draw calls and peak resident bytes. Use a flat material palette.”

**Prompt D — Coherent far terrain.** “Implement a nested, view-centered, low-pass terrain representation and local transition morphing derived from the exact procedural source, with a separate arbitrary-3D edit overlay. Turn fog off, record stationary/walk/sprint/rotate flights, report LOD transitions and source-level mismatches.”

**Prompt E — Volumetric renderer bakeoff.** “Test SVO and frozen SVDAG against compact raster on caves, arbitrary edits and high-entropy structures; include CPU build, GPU upload, GPU hardware timestamp, palette data, query correctness and edit latencies. Reject candidates that do not help the intended class of scenes.”

## 20. Secondary reference library, arranged in implementation reading order

**Read these when the relevant work begins, not as a pretext to implement every paper.** This list supplements §15 rather than replacing it.

### Geometry, meshing, GPU dispatch

- [0fps — Meshing in a Minecraft Game (2012)](https://0fps.net/2012/06/30/meshing-in-a-minecraft-game/): clear baseline analysis of greedy merging and face counts; begin here before implementing bitwise acceleration.
- [Binary Greedy Meshing v2 — complete public code and benchmarks](https://github.com/cgerikj/binary-greedy-meshing): working implementation of 64-bit occupancy/face masks, greedy packing and vertex pulling; verify license before reuse.
- [VoxelCore — C++ engine source](https://github.com/MihailRis/voxelcore), [its chunk renderer](https://github.com/MihailRis/voxelcore/blob/main/src/graphics/render/ChunksRenderer.cpp): practical chunk scheduling, meshing, visibility, bounded worker cancellation and memory/control considerations; MPL-2.0.
- [Project Ascendant — geometry/rendering write-up](https://vkguide.dev/docs/ascendant/ascendant_geometry/): concrete explanation of hybrid near/far rendering, retained draw lists and shared GPU arenas.
- [Khronos — GPU-driven indirect draw tutorial](https://docs.vulkan.org/tutorial/latest/Advanced_Vulkan_Compute/07_GPU_Driven_Pipelines/04_multi_draw_indirect_mdi.html): compute culling → indirect count/commands → one or a few submissions; native-capability-sensitive.
- [wgpu Features API](https://docs.rs/wgpu/latest/wgpu/struct.Features.html): decide GPU capabilities at startup and implement an actual fallback path. Do not assume Vulkan-only features work across all adapters.

### Terrain LOD, streaming and transitions

- [Losasso/Hoppe — Geometry Clipmaps (2004)](https://hhoppe.com/proj/geomclipmap/): original nested viewer-centered cache and gradual transition rationale.
- [Asirvatham/Hoppe — GPU Geometry Clipmaps (2005)](https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry): exact update and morph concepts, incremental buffer work and GPU terrain synthesis.
- [Godot Voxel Tools source](https://github.com/Zylann/godot_voxel) and [its LOD fading design](https://voxel-tools.readthedocs.io/en/latest/smooth_terrain/): tested volumetric paged-world interfaces, LOD mesh fading and editing constraints; its full feature set depends on smooth vs blocky mode.
- [Eric Lengyel — Transvoxel](https://transvoxel.org/): a reference for transition cells when smooth iso-surface volumes use different resolutions; **not** a direct algorithm for ordinary axis-aligned block-face joins.
- [NULLSEED development diary](https://nullseedgame.com/): 6.25 cm design, meshlet/HZB implementation descriptions, distant heightmap/structure proxies, reported hardware performance; not a verified open-source renderer.
- [Enshrouded 1.0 Oct 2026 notes](https://kg-www.keengames.com/en-US/news/release-trailer-changelog-everything-you-need-to-wait-for-tomorrow): documented visual benefit of distant “brick rendering” and preserving distant player builds; no detailed source algorithm released.

### Sparse volumes, compression and ray traversal

- [Cubiquity README and source](https://github.com/DavidWilliams81/cubiquity): canonical compact mutable SVDAG/material representation and software culling prototype; explicitly **not** a real-time polished terrain renderer.
- [Laine/Karras SVO technical report](https://research.nvidia.com/publication/2010-02_efficient-sparse-voxel-octrees-analysis-extensions-and-implementation) and [archived original CUDA code](https://code.google.com/archive/p/efficient-sparse-voxel-octrees): ray traversal, compact occupancy, contour data and filtering.
- [Kämpe/Sintorn/Assarsson — High Resolution Sparse Voxel DAGs (2013)](https://doi.org/10.1145/2461912.2462024): deduplicating repeated subtrees.
- [HashDAG — Modifying Compressed Voxels](https://graphics.tudelft.nl/Publications-new/2020/CBE20/ModifyingCompressedVoxels-main.pdf): dynamic edits in a shared compressed geometry hierarchy.
- [Molenaar/Eisemann — Editing Compact Voxel Representations on the GPU (2024)](https://diglib.eg.org/items/ffd7acf6-01f7-4e53-87f8-4cded382c944), [GPU implementation](https://github.com/mathijs727/GPU-SVDAG-Editing): real-time GPU editing using dynamic hash tables, but dependency/hardware-specific; treat as optional R&D.
- [Fang/Wang/Wang — Aokana (2025)](https://doi.org/10.1145/3728299), [arXiv preprint](https://arxiv.org/abs/2505.02017): GPU-driven open-world voxel rendering with chunked/shallow DAGs, distinct geometry/color payloads, streaming and LOD. Does not settle arbitrary edit speed.
- [Ulschmid et al. — NAADF (2026)](https://www.cg.tuwien.ac.at/research/publications/2026/ulschmid-2026-naadf/): newer nested AADF cache method for GPU ray tracing, edit updates and temporal artifacts; direct source/paper/video links at the project page.

### Landforms and realistic geometry (not advanced lighting)

- [Schott et al. — Terrain Amplification using Multi-Scale Erosion (SIGGRAPH 2024)](https://h-schott.github.io/p/mserosion/): consistent erosion landmarks across scales, thermal/stream/deposition approximations, examples/video/code linked on project page.
- [Tzathas et al. — Physically-based Analytical Erosion for Fast Terrain Generation (2024)](https://www-sop.inria.fr/reves/Basilic/2024/TGSC24/): physically informed procedural mountain evolution rather than repeated numerical simulation at runtime.
- [ESA Copernicus DEM](https://dataspace.copernicus.eu/explore-data/data-collections/copernicus-contributing-missions/collections-description/COP-DEM), [ESA WorldCover](https://esa-worldcover.org/en/data-access), [MERIT Hydro](https://global-hydrodynamics.github.io/MERIT_Hydro/): real-world regional elevation, land cover and drainage examples; spatial resolution and distribution rights vary. Use only for macro calibration and evaluation, not as microvoxel ground truth.

### Foundational graphics and testing

- [PBRT 4e Sampling Theory](https://www.pbr-book.org/4ed/Sampling_and_Reconstruction/Sampling_Theory): why distant detail must be low-pass filtered *before* downsampling to avoid shimmer/aliasing.
- [PBRT 4e BVH chapter](https://www.pbr-book.org/4ed/Primitives_and_Intersection_Acceleration/Bounding_Volume_Hierarchies) and [Embree source](https://github.com/RenderKit/embree): well-engineered near-first traversal and acceleration ideas if CPU/BVH tests justify them.
- [0fps Voxel Ambient Occlusion](https://0fps.net/2013/07/03/ambient-occlusion-for-minecraft-like-worlds/): useful when the rendering foundation is stable; not an early-stage geometry optimizer.

**Final stop/go principle:** do not copy a complicated state-of-the-art data structure because its screenshots are beautiful. Reproduce a relevant result, preserve canonical semantics, measure full-frame behavior and moving-frame stability on actual target hardware, and promote only the algorithm that wins the specific workload. Hybridization is warranted only where measured improvements outweigh integration complexity.
