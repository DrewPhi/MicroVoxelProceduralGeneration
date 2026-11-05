# Literature audit — screen-space-adaptive exact microvoxels (2026-11-05)

**Branch:** `frontier-voxel-engine`. **User choice:** do **not** intentionally exaggerate the voxel aesthetic. Preserve the appearance of the same 6.25 cm axis-aligned voxel world as it naturally becomes subpixel with distance. Several-mile visible mountains, rapid aircraft flight, exact gameplay voxels, caves and player edits, smooth transitions, modest hardware. This is an **architecture research and issue-planning document**, not a claim of verified state-of-the-art performance.

## Executive conclusion

**Keep the canonical voxel world, independent procedural FBM/geology, and the useful GPU landscape clipmap prototype. REPLACE the fixed 64×64 m exact-native mesh square and abrupt 2 m native/heightfield collar as proposed production quality controls.** Build a hierarchical, sparse, view-selected *voxel-derived* display representation. Use a GPU-displaced heightfield only where the geometry is provably heightfield-safe; use sparse multiresolution 3D coverage/surface clusters for meaningful voxel steps, caves, cliffs, arches, edits and buildings. Select the necessary geometry by measured/projected **representation error, pixel coverage, occlusion and topology**, not by a fixed circle, fixed chunk radius, or naive voxel projected width alone. Reuse valid generated content across viewpoints, and stream by deadlines and quality deficits. Test against independently rendered, supersampled **canonical voxel geometry**.

The proposal is **competitive with modern methods in design**, but no surveyed paper implements and proves this complete combination for our workload/hardware. Do not label this engine the fastest or state-of-the-art before the matched A/B gates below.

## Avoid a tempting incorrect shortcut

"Draw only voxels larger than one pixel" is WRONG as a literal rule. Individually subpixel voxels can:
- collectively alter the mountain silhouette, mean pixel color, roughness, ambient occlusion and shadow;
- form thin ropes/bridges/fences visible through coverage even if each width is <1 pixel;
- separate cave openings and exposed surfaces;
- cause moiré and temporal shimmer if retained as unresolved high-frequency geometry.

Render work should be **roughly proportional to perceptible image complexity**, while *filtered scene coverage and depth remain physically representative*. Do not cull all subpixel features or replace every mixed voxel block by a solid macrovoxel. Small but significant features need a conservative proxy/coverage model or subpixel shading and temporal filtering, and shadow/reflection ray requirements are separate from the primary camera's visibility.

For a front-facing native voxel width `w = 0.0625 m` at distance `d`, projected width is approximately `f_px * w / d`, subject to camera FoV and orientation. This is **not** a sufficient geometric-error or silhouette guarantee. A geometric error vector projects differently over a grazing surface and at depth discontinuities. Compute conservative screen-space bounds or evaluate exact reference images.

## Screen-adaptive representation

1. **Canonical world, no view dependencies:** globally addressed exact occupancy/material/edits; queries and gameplay exact at 6.25 cm regardless of draw distance. Distinct generator/source and appearance versions.
2. **Multiresolution voxel-derived content keyed only by world region/LOD/revision:** each node stores occupancy classification (empty/full/mixed), conservative visible-surface or contour bounds, geometry-error estimates, material coverage/filtered shading descriptors, and *volume/topology* warnings. Keep exact voxel surface clusters only in demanded regions. Build/update coarse feature anchors and conservative bounding data without enumerating every native cell of an untouched 10 km volume. Derived visual data is disposable; edit overrides are persistent.
3. **Landscape path:** reusable GPU grids or CDLOD patches for predominantly single-valued geological terrain. Use page-resident FBM samples, octave filtering, parent-child residuals, and silhouette/ridge/channel anchors. No per-camera rebuild of large CPU meshes. Heightfield eligibility cannot silently cover caves/edits or remove geometric steps that still matter on screen.
4. **3D path:** sparse voxel surface/brick clusters with balanced multiresolution geometry, or a ray-based first-hit backend where verified faster. Native voxels appear wherever they are resolvable, wherever gameplay demands exact interaction, and wherever approximation error otherwise exceeds tolerance—**not** within a global 64 m square. Preserve caves/bridges/overhangs/edited structures at all visually meaningful distances.
5. **Selection and scheduling:** recursively test conservative projected error and on-screen coverage; frustum and safe occlusion culling; hysteresis/motion prediction; retain visible parent coverage while children load. Resident data shared across viewpoints; no old camera job may publish obsolete draw selections. Prioritize by time-until-needed, screen error, generation latency, upload bytes, and view-predicted visibility.
6. **Transitions and filtering:** geometric residual morph where parent/child topology and anchors agree; topology-aware ownership at heightfield/volume interfaces; optional world-space dither+depth-safe coverage/TAA for discrete replacements with an explicit overlap cost; stable filtered normal/albedo/roughness/occlusion when microfaces become subpixel. Don't blur a geometric seam or hide unloaded terrain behind fog.
7. **Edits:** invalidate only overlapping bricks, dependent parents/proxy pages and lighting summaries; correct shared boundary face ownership across chunks; stage all atomic world/revision-dependent visibility state together. Don't regenerate large native squares for small modifications.

## Research and open-source: what each truly proves

### GPU-driven and voxel-specific primary literature

- **NAADF, Ulschmid et al., Eurographics 2026** — https://www.cg.tuwien.ac.at/research/publications/2026/ulschmid-2026-naadf/ ; working source https://github.com/cg-tuwien/NAADF . Nested directional-axis skip distances, editable world, ray tracing without geometric LOD, multi-frame temporal accumulation. **Credible alternate primary-ray engine**, especially for caves. Its developer demo does not prove success at 6.25 cm over multi-mile landscapes on our Radeon 760M. Benchmark, don't port wholesale (MonoGame / compute fork).
- **Aokana, Fang et al., 2025** — https://doi.org/10.1145/3728299 ; https://arxiv.org/abs/2505.02017 . Shallow SVDAGs, LOD, streaming, Hi-Z and visibility buffers for huge voxel worlds. **The published version explicitly lacks runtime voxel modification**. Do not treat as a finished survival-world editing solution.
- **SVRaster, Sun et al., CVPR 2025** — https://openaccess.thecvf.com/content/CVPR2025/html/Sun_Sparse_Voxels_Rasterization_Real-time_High-fidelity_Radiance_Field_Rendering_CVPR_2025_paper.html ; https://github.com/NVlabs/svraster . Adaptive sparse-voxel screen rasterization and ordered visibility demonstrate a useful *approach to rendering*; it is a trained radiance-field / novel-view-synthesis system in CUDA, **not** proof of exact binary voxel occupancy or cheap persistent edits. Optional bounded study only.
- **Six Ways to Draw Vangers with WebGPU, Malyshau, Aug 2026** — https://arxiv.org/abs/2608.17390 . On equal source and implementation, greedy triangulation was fastest for tested editable multilayer terrain but required substantial mutable geometry RAM; alternate ray approaches can miss side walls or coverage. Our exact voxel/reference-image A/B methodology should be comparable, but its scene is not our arbitrary 3D world.
- **Editing Compact Voxel Representations on the GPU, Molenaar & Eisemann 2024** — https://doi.org/10.2312/pg.20241310 . A research route for local edits to compressed GPU SVDAGs; supports decision to store persistent edits independently of display meshes, but compression/indirection overhead must be profiled.
- **CuRast, Schütz et al., Aug 2026** — https://doi.org/10.1111/cgf.70538 ; https://github.com/m-schuetz/CuRast . CUDA software rasterization of very dense opaque triangle fields; notably reported advantages on RTX 5090. Authors say it performs poorly on many small meshes and is not yet game-suitable. **Do NOT select as mandatory Radeon760M / wgpu runtime**; inspect its visibility-buffer / micropolygon principles later as optional research.

### Established terrain and volumetric geometry

- **Geometry Clipmaps (Losasso & Hoppe 2004)** — https://hhoppe.com/proj/geomclipmap/ ; **CDLOD (Strugar)** — https://github.com/fstrugar/CDLOD . Mature hierarchical terrain geometry, morphing, page reuse; only *heightfield* bulk terrain, not exact non-heightfield microvoxel transitions.
- **Godot Voxel Tools** — https://github.com/Zylann/godot_voxel ; https://voxel-tools.readthedocs.io/en/latest/smooth_terrain/ . Practical 3D voxel LOD and edited source details; SDF/Transvoxel representation differs from strict blocky voxels. Its docs explicitly discuss edited voxels in far normalmap sources, missing/slow streamed data, and LOD fades.
- **Transvoxel (Lengyel)** — https://transvoxel.org/ . Sound stitching for *marching-cubes isosurfaces* at 2:1 voxel LOD boundaries; **not** a drop-in guarantee for discrete block surfaces or independent smooth heightfield to native cube transitions.
- **Crack-free large terrain tile map, Oh & Lee 2025** — https://doi.org/10.4218/etrij.2024-0496 . Useful scalable neighbor adjacency / LOD seam constraints, not a general 3D voxel bridge solution.
- **Filtering After Shading with Stochastic Texture Filtering, Pharr et al. (I3D 2024)** — https://research.nvidia.com/publication/2024-05_filtering-after-shading-stochastic-texture-filtering . Evidence for filtering material/lighting correctly when high-frequency texture/voxel attributes become subpixel; implementation is optional and requires temporal/noise management.

### Direct r/VoxelGameDev reports (developer observations, not controlled research)

- **MGMishMash, Aug 2026:** https://www.reddit.com/r/VoxelGameDev/comments/1vrqluv/improving_rendering_distance_in_my_micro_voxel/ . Reports a meshed microvoxel world, 10–15 km viewing, independent coarse macro chunks, local edit propagation, deterministic vegetation IDs, and transient dither fades. Reported 45–50 FPS on an M1 Pro is developer-specific, not a performance promise. **Reject its adaptive-fog retreat as our primary horizon failure fallback**; prefer complete directly generable root geography.
- **Subpixel/moiré problem at exactly 1/16-unit voxels, May 2026:** https://www.reddit.com/r/VoxelGameDev/comments/1tgp2v1/how_to_combat_extreme_moire_pattern_when/ . Author reports blending toward broad surface normals plus 4x MSAA reduced moiré; other advice emphasizes LOD/AA. Community evidence for **filtered microface shading**, not proof the proposed method is universally correct.
- **Distant voxel moiré, Aug 2026:** https://www.reddit.com/r/VoxelGameDev/comments/1vtnemb/has_anyone_dealt_with_distant_voxels_moire_effect/ . Poster reports projected-error-based more aggressive LOD reduced artifacts; added depth-masked normal filtering. Note shader blur is only an optional aesthetic/AA control; not a solution for wrong geometry.
- **Raycast LOD popping and world-space dithering, July 2026:** https://www.reddit.com/r/VoxelGameDev/comments/1uwhlnq/does_anyone_know_what_to_do_about_this_or_what/ . A developer explains explicit depth/stencil world-space stochastic transitions across voxel LODs and optional TAA. Reported visual success is anecdotal, and persistent multi-LOD overlap can cost substantial GPU time.
- **Chunkee stable spatial clipmaps, Jan 2026:** https://www.reddit.com/r/VoxelGameDev/comments/1qicz42/update_5_chunkee_lods_are_back/ . World-aligned stable macro-LOD identities simplify coalescing/reuse and reduce cracks; distinguishes source region identity from transient camera demand.
- **PaleFernDev voxel engine, Oct 2026:** https://www.reddit.com/r/VoxelGameDev/comments/1ww3tvc/working_on_a_voxel_engine_in_unity_finally_got/ . Reports palette-compressed chunks, independent same-size simplified mid band, farther coarser LOD rings, and explicit edit stamping into distant regions even with overhangs. Useful example of preserving volumetric features far away, although the author deliberately omits small caves from some display bands.
- **Microvoxel trees & grass, July 2026:** https://www.reddit.com/r/VoxelGameDev/comments/1uo7tel/microvoxel_trees_and_grass/ . Near-only DDA can be viable for high-frequency detail, but the author notes the transition to coarse regular cubes is obvious. Beware simply replacing the current native square with a different hard cutoff.
- **Editing boundary chunks, Sept 2024:** https://www.reddit.com/r/VoxelGameDev/comments/1fd3yge/problem_with_chunk_borders/ . Practical dirty-neighbor face tests for edits at chunk borders. Local neighborhood invalidation does not imply recomputing an entire world.

## Explicit non-goals and rejections

1. No fixed **native 64×64 m square** or hard 2 m collar as a production rendering rule. Can remain as a frozen A/B reference fixture.
2. No artificially large displayed voxels to preserve a Minecraft style at distance; a voxel-style toggle is a **separate future experiment**, not this architecture.
3. No instant cull of every <1px primitive; use coverage/filtered surface and conservative error/occlusion.
4. No forced planet-sized SVO/DAG or a second parallel terrain simulator before comparable evidence.
5. No assumption that a cached smooth heightfield accurately depicts a voxel-cubic surface where steps are visibly resolvable.
6. No reliance on postprocessing blur to cover model holes, false depth, or missing geometry.
7. No mandatory 2026 proprietary GPU features; keep a raster path compatible with Radeon 760M / wgpu.
8. No universal guarantee of zero pops or fastest hardware throughput; success is empirical **image fidelity + real-time workload** under measured routes.

## Decision gates

**G0 image oracle:** deterministic canonical exact voxel geometry on bounded crops; supersampled 4×–16× views for offline comparison of coverage, silhouette, depth, materials and filtered appearance. Include an antialiased reference and no-LOD image. Test 90° FOV/1080p and the actual in-game FoV, motion across representation switching, camera cut, edited cave, river, ridge, tower/bridge/foliage, and long-range silhouette.

**G1 source summaries:** coarse region proves conservative geometric/occupancy/coverage data and persists identifiable major voxel-created features. If exact guarantees impractical, mark uncertified mixed nodes and trigger safe refinement, rather than falsely promise pixel bounds. Rendering complexity does not depend on total native voxels in full world.

**G2 adaptive geometry:** selectable sparse native surface clusters rather than resident 64 m native square; demonstrate lower resident bytes/build/upload and no worse reference-image error in a near + high-altitude mixed view. Compare fast GPU displaced heightfield, voxel mesh hierarchy and candidate NAADF per region; simple renderer wins if adequate.

**G3 transitions/edits:** no false terrain caps, coverage holes or stale edited geometry; residual morph for compatible heightfield; world-stable dither/TAA for necessary discrete transitions; shading mip continuity. Camera movement smooth at 3/15/80 m/s; voxel authority and gameplay exact.

**G4 sustainable aircraft:** at least the current failed 80 m/s test plus max intended airplane speed; publish meaningful detail before perceptually necessary, no >pixel silhouette popping, visible root horizon at all times, bounded p95/p99 frame costs (including generation/upload, not just raster time), truthful VRAM/RSS including inflight. Teleport rejects stale content and displays a coherent destination; if delayed, report readiness separately.

## Dependencies on existing branch issues

- #33 source multiresolution/error summaries; #37 LOD error selector; #38 continuous morph/hand-off; #44 native geometry; #46 distant volumetric features; #47 hybrid authority; #48 incremental edits; #41/#42 scheduler and flight; #52 temporal AA; #55/#56 architecture/acceptance.
- This audit produces **new narrowly scoped issues** for the missing *view-selectable voxel-native hierarchy, explicit exact-voxel screen reference oracle, no-native-square transition contract, and 80 m/s coverage* while retaining old issues as parents. Do not close old gates until new tests pass.
- Legacy reading: `main` and `DrewsAnalyticRendering` only under `frontier/AGENTS.md` tiers (R/P/B/G); no copy of the old scheduler or quadratic fitting into the new renderer.
