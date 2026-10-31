# Terrain generation replacement plan

Research and source audit: 2026-10-30. Status: proposed; no generator changes made by this plan.

## Decision

Replace the procedural landscape model, retaining the voxel engine. Use a finite, cached geographic atlas with coupled uplift, drainage and erosion; refine its catchments into local terrain; derive material coverage from the same landscape state. Noise supplies variability and small residual detail rather than prescribing the mountain and river network independently.

This is a proposed architecture, not a claim that a paper supplies a ready-made seamless, centimeter-resolution world generator. The highest-risk work is hydrologically consistent refinement and bounded streaming latency. Prototype those before broad integration.

## What the current code actually does

Inspected `successor/world/src/generation/{provinces,landforms,terrain,climate,geomorph}.rs`, generation identity and world spec.

- `provinces.rs` uses jittered sites at a single 72 km pitch. Velocity-derived convergence modulates smooth distance-to-boundary belts. This is a tectonic-inspired spatial pattern, not geological evolution. Similar spacing and belt cross sections explain the repeated network in the huge overview.
- `landforms.rs` assigns mountain height from that belt plus an independent five-octave ridged field beginning at 850 m. Plains, hills, plateaus and basins are separate weighted height formulas. Watersheds do not create these ridges.
- `Region::generate` samples those finished shapes, performs 24 sediment/relaxation iterations, then an additional valley-incision pass. Climate, channels and subsequent detail operate on related but different representations.
- Regional spacing grows with requested extent to keep approximately 783 squared samples. Changing extent changes erosion, routing and climate discretization, not just the visible crop. The temporary 1000 km export changed sampling further; it is not a production-supported million-square-kilometer world.
- Eight-neighbor routing introduces preferred directions. `geomorph.rs` adds slope-oriented cosine troughs and heuristic debris/ledges; those are not a connected tributary network or a conserved sediment model.
- Climate has prescribed boundary moisture and a dominant/reverse sweep. It lacks geographic ocean sources and continental context.
- The world ceiling is capped at 2048 m, and generation clips below it. Raising mountains without addressing this would flatten summits.

These explain a structural quality ceiling. A heightfield itself is not the problem for ordinary landscapes; disconnected rules defining the same relief are. Caves remain a separate sparse volumetric layer.

## Research choices and their limits

1. [Cordonnier et al. 2016, Large Scale Terrain Generation from Tectonic Uplift and Fluvial Erosion](https://perso.liris.cnrs.fr/apeytavi/website/publication/hal-01262376/): main architectural reference. Coupled uplift and stream-power erosion on a stream graph can produce related drainage and mountain structure. It still needs suitable uplift inputs; it does not solve procedural continental history for us.
2. [Schott et al. 2023 reference code](https://github.com/H-Schott/StreamPowerErosion): MIT-licensed practical reference. Inspected `code/spe_shader.cpp` and `data/shaders/spe_shader.glsl`: separate uplift, elevation and stream buffers, ping-pong compute passes, weighted neighbor flow and slope-dependent erosion. Its shader fixes border elevations to zero and propagates flow iteratively. Do not copy these boundary assumptions or its parameter normalization into world tiles. Its GPU pass is not the downstream-first implicit CPU solver proposed below.
3. [Fastscapelib implicit stream-power solver](https://fastscapelib.readthedocs.io/en/latest/api_cpp/eroder.html): reference for a downstream-first solve, starting with slope exponent n=1. Stability does not imply arbitrary time steps preserve drainage evolution. The library is GPL-3.0; use published equations for our own implementation, not unreviewed source copying into this MIT repository.
4. [Cordonnier et al. 2019, basin-graph flow routing](https://esurf.copernicus.org/articles/7/549/2019/): connect depressions explicitly and choose physical spill/breach treatment. A virtual routing connection does not itself create a valid downhill river bed.
5. [Schott et al. 2024, Terrain Amplification using Multi-Scale Erosion](https://h-schott.github.io/p/mserosion/) and [MIT source](https://github.com/H-Schott/MultiScaleErosion): primary refinement reference. Inspected erosion, thermal and deposition shaders. These use separate fields and staged operations. Their demo normalization and boundary treatments are not a drop-in physical or tiled simulation; our implementation must explicitly budget transported material and preserve parent hydrology.
6. [Grenier et al., controlled procedural patterns](https://onlinelibrary.wiley.com/doi/full/10.1111/cgf.14992): candidate for fast slope/flow-conditioned local relief. Useful below the solved network, not a replacement for that network.
7. [Rune Skovbo Johansen, Fast and Gorgeous Erosion Filter, 2026](https://blog.runevision.com/2026/03/fast-and-gorgeous-erosion-filter.html): useful recent implementation discussion of derivative-driven, point-evaluable branching detail and peak/valley artifacts. Benchmark as an alternative local amplifier. Do not stack it with another equivalent erosion filter. Verify the exact code's license before porting; a public shader is not automatically MIT.
8. [Barnes et al. 2021, Fill–Spill–Merge](https://esurf.copernicus.org/articles/9/105/2021/index.html): reference for retained depression hierarchies and runoff-limited lakes. Static generation can use this reasoning without adding live water physics.
9. [Argudo et al. 2025, Terrain descriptors](https://onlinelibrary.wiley.com/doi/10.1111/cgf.70080): guide for evaluating relief, drainage and landscape structure rather than optimizing one attractive screenshot.

Read source references are design evidence, not measured performance on our laptop. Retain attribution/license notices for any MIT code adapted; pin revisions when implementing. Full plate simulation, learned terrain synthesis and detailed droplet erosion are not initial dependencies: they add cost and difficult control/consistency requirements before addressing our immediate problem.

## Proposed model and contracts

### 1. Geographic atlas independent of the camera

Start with a fixed finite atlas approximately 1024 km across. Atlas bounds and physical discretization belong to the saved world definition. A 10 km game region and 100/1000 km previews are crops of that same atlas. Expanding the actual atlas boundary is a new world definition; do not promise infinite extension with identical global drainage.

Prototype approximately 2 km irregular nodes (roughly 262,000 over the full atlas), using deterministic jittered sites and triangulation. This avoids restricting major drainage to eight compass directions, although it does not guarantee realistic drainage. Compare against a regular-grid reference on a small domain before committing to the extra graph cost. Keep a regular sampled field for cheap runtime queries.

Generate a few broad landmasses and basins, spatially coherent rock resistance, old uplands, and selected elongated uplift systems with variable width, strength and history. Use sparse curved uplift fields rather than raising every Voronoi boundary. These are explicit procedural geological controls, not a claimed physical plate simulation. Related foreland basins and quiet interiors should follow the same systems.

Keep sea level, outlet definitions, geological age/epochs and physical units explicit. Small regions need not contain all climates or landforms. Gameplay world-scale compression remains a separate coordinate transform applied after landscape generation, preserving its existing documented meaning.

### 2. Coupled landscape evolution

Iterate bounded epochs of uplift, routing, runoff accumulation, implicit fluvial incision, hillslope transport and sediment deposition. Rebuild routing as relief changes. Start with n=1 stream power for a straightforward implicit solve; test nonlinear variants only if they improve measured shape quality.

Track bedrock height, loose sediment thickness, erodibility, drainage area/discharge, receiver topology, basin identity and outlets. Conserved material has an explicit export ledger. Uplift is a source, erosion transfers/removes material, deposition must have sediment supply. Do not use uniform noise as an additional independent mountain generator.

Handle retained lakes and closed arid basins deliberately. Distinguish drainage enforcement used by the erosion algorithm from actual water surfaces. Final channels must descend in the final terrain, not merely in a temporary filled routing surface.

### 3. Refine catchments, then evaluate small detail

Use atlas constraints to refine selected areas, tentatively from kilometer scales through 256/64/16 m. These spacings are prototype parameters, not promises of cost or realism. Preserve major outlets, river connectivity, water elevations and divides while allowing new tributaries between them.

Canonical work units are hydrological regions with stable IDs, not render tiles. Neighboring work units inherit shared boundary records: heights, river crossing positions, discharge and sediment flux. Use parent constraints plus halos for local operators. A halo alone cannot fix globally coupled drainage; solve a larger parent catchment when local refinement cannot satisfy its constraints. Reject contradictory child results rather than hiding them with a seam blend.

At meter/centimeter scales, choose one bounded deterministic local amplifier. Feed it final slope, drainage proximity, lithology, sediment and exposure. Protect channels and lake shores from residual detail. Use material-specific fracture/grain scales rather than generating endlessly smaller mountain-like noise. Centimeter occupancy is still sampled on demand; do not run erosion per voxel.

Parent summaries record unresolved relief bounds and water/material coverage for LOD. Canonical terrain must not depend on render quality or cache state. Fine patches have one deterministic completion criterion and are published atomically; collision cannot silently switch to a different finished terrain just because a visual detail job completes. Prepare the canonical collision neighborhood before entry, or explicitly gate traversal while loading.

### 4. Climate and surface composition from the final landscape

Compute coarse seasonal climate from ocean/moisture sources, prescribed latitude/climate context, elevation and winds. Use a small fixed number of climate/erosion coupling stages rather than an uncontrolled feedback loop. Finish climate again after relief stabilizes.

Derive soil, moisture, bare rock, sediment, snow likelihood and vegetation suitability from shared fields. Biomes are useful classifications of those fields, not independent height generators. Keep solid per-voxel colors. This phase adds no live storms, vegetation physics or fluid simulation.

Warm-climate river erosion alone will not reproduce all Alpine terrain: U-shaped glacial valleys, cirques and glacial deposits require an additional process model. Defer that model until the river/hillslope baseline works, while retaining cold-climate/history fields needed to add it cleanly.

### 5. Runtime and persistence

Compile the coarse atlas at world creation, cache it on disk, then stream bounded refinement patches. Use packed arrays, compact adjacency and spatial indexes rather than one object per sample or a dense 64 m bin table over the entire atlas. Reuse worker buffers. Keep long solves out of synchronous voxel queries and renderer jobs.

Cache keys include generator version, seed, atlas bounds, physical configuration, geological fields, patch ID, parent hash and refinement level. Native voxel size and render quality do not redefine the physical landscape. Voxel edits remain overlays; new generator versions must not reinterpret old edits against changed base terrain.

A single 1024 squared f32 field is about 4 MiB; a 513 squared field about 1 MiB. These are illustrative payload sizes, not total memory budgets: graphs, solver scratch, halos, cached children and GPU copies also count. Initial targets: coarse atlas under 256 MiB resident and refinement cache under 128 MiB, separately measured from renderer budgets. Profile and revise openly if fidelity requires more.

CPU Rust reference first. Consider wgpu compute only after profiling expensive regular passes; integrated graphics shares bandwidth and frame time with rendering. Preserve deterministic reference outputs and define numerical tolerance before accepting a GPU path.

## Implementation milestones and acceptance gates

1. **Freeze evidence and isolate the replacement.** Capture generator6 baselines, seeds 1/42/99 plus at least five fixed additional seeds, gray shaded relief, color, flow and slope maps. Add an experimental generator selector using existing generator interfaces. Preserve current playable worlds while prototyping. No third permanent engine.
2. **Make one convincing 100 km landscape in gray.** Implement explicit uplift/rock fields and coupled routing/erosion on a single bounded domain. Compare irregular graph and raster routing, flat plains, an uplift belt and a closed basin. Gate: connected branching watersheds, varied ridge spacing, broad quiet terrain, downhill channels, no eight-direction or province lattice dominating the image. Inspect actual renders before building more systems.
3. **Build the fixed large atlas.** Generate variable geographic regions and coarse climate context. Render nested 1000/100/10 km crops of the same saved atlas. Gate: identical overlap samples, no export-dependent terrain, no repeated mountain-cell pattern, memory/time recorded. Adjust vertical representation before allowing heights beyond 2048 m; audit storage, GPU bounds, ray traversal and collision rather than just removing a clamp.
4. **Prove refinement and caching.** Build one parent catchment and adjacent children. Compare multiscale erosion with a constrained analytic amplifier. Gate: exact shared boundary records, conserved cross-boundary flux, matching final terrain regardless of request order, bounded residual error, no new uphill rivers. Select one approach based on visual quality and p95 build cost. This is a go/no-go milestone, not routine glue work.
5. **Complete rivers, sediment and appearance.** Establish channel width/depth/profile from runoff and relief, floodplain deposits, retained lakes and coherent coverage. Reuse useful material rules while removing duplicated terrain-shaping heuristics. Gate: water remains inside its final banks at native detail, coarse coverage preserves major rivers, dry basins stay dry, no material confetti or fake height stripes needed to imply relief.
6. **Integrate and test travel.** Adapt the source behind existing world/render interfaces; feed feature/error summaries to LOD. Test cold/warm cache walking, running, stop/resume, teleport, cache eviction and cancellation. Target no regression in nearby refinement latency or p95/p99 frame time against the existing engine on the same laptop. Atlas build time and local cache misses are separate metrics. Do not claim faster generation before measurements.
7. **Promote and remove superseded code.** After visual and performance gates pass, switch default, rebuild launcher, document generator/save incompatibility and retain explicit old-save support or clear rejection. Remove the old production province/belt height model, independent ridge backbone, extra post-hoc incision and competing relief functions. Keep a small reproducible comparison fixture, not two indefinitely maintained terrain systems.

## Evaluation protocol

Use the same seed, atlas, coordinates, lighting, field of view and physical scale for comparisons. Show gray relief before colors; annotate every overview's area, sample spacing and any vertical exaggeration (default none). A 1000 km image cannot establish centimeter quality.

Capture at continental, range, valley and walking scales across wet/dry lowlands, old hills, steep mountains and sedimentary terrain. Compare bare-earth DEM crops of analogous landscapes, not vegetation-contaminated surface scans; record dataset source, resolution and license when selecting fixtures.

Measure elevation/slope distributions, local relief by window size, drainage orientation bias, channel slope/area relationships, river longitudinal profiles, basin connectivity, lake spill consistency and sediment accounting. Compare broad descriptor ranges, not a single universal 'Earth score'. Visual review remains necessary.

Record atlas build duration, patch p50/p95/p99 latency, resident and peak scratch memory, cache hit rate, cold travel native-detail coverage and frame-time percentiles. Include different voxel sizes to verify physical terrain identity. Run rotation tests for solver directional bias, child request-order tests, and coarse-versus-fine silhouette/water comparisons.

The first deliverable should be an honest side-by-side gray 100 km render and diagnostics from milestone 2. If it still looks synthetic, improve or reject the core model there instead of piling on painting, weather or extra noise.
