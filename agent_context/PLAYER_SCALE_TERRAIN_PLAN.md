# Player-scale terrain diagnosis and implementation plan

## Evidence (seed42, world scale10, 6.25cm voxels)

The 1025² atlas covers1000km: nominal node spacing976.5625m source,
97.65625m game. Runtime refinement uses cubic triangles plus up to four straight
local tributary segments. Added height grain is 4m/0.08m,0.5m/0.018m,
0.0625m/0.004m wavelength/amplitude in source units. At scale10 the largest
amplitude is8mm. Meaningful intermediate landforms are largely absent.

Measured all212,998 valid global receiver channels (nonself, discharge>=0.02,
nonincreasing water level): game width percentiles50/90/99 are
0.102588/0.390686/2.276964m; nominal depth percentiles are
0.01105/0.024649/0.070977m.97.9934% have nominal depth<0.0625m.
These are formula values, not measurements of final wet occupancy. Floors of
water and bed heights can coincide or differ intermittently along a shallow bed.
Uniform compression makes these already shallow profiles sub-voxel.

Shared height controls ensure C0 continuity, not matching cross-edge slopes.
Straight receiver links follow triangulation edges; local branches use fixed
barycentric positions. Both expose construction geometry. Separate fine/coarse
renders are needed to attribute visible facets between generator and renderer.

Painting uses interpolated atlas-node gradients rather than final local geometry.
Runtime material classification is a few coarse thresholds, whereas albedo blends
coverage. Local banks/cliffs therefore lack consistent material response.

The coherent renderer's visual_node decides wetness at one sample. Footprint
coverage is consulted only if already wet and display step>=32m. A narrow river
between dry vertices can disappear even though an indexed footprint exists.

The player screenshot waited for cover completion (jobs0), but native radius was
6m and visibleP95 error3.81px against requested1.04px. Settled does not mean exact
terrain everywhere in frame. GPU software rendering timings do not characterize
the user's laptop.

## Ordered implementation

1. Establish fixed player-eye routes and paired references. Six scenes: river
   confluence, foothills, steep ridge, dry valley, lakeshore, plain. Same source
   coordinates at scales5/10/20; native size fixed. Capture only after nearby
   target detail is loaded, report actual spacing/error and outstanding jobs.
   Compare a bounded exact reference against production LOD; gray and color.
   Measure height/slope continuity and native wet connectivity before optimizing.

2. Correct river representation first. Keep compiled discharge/network identity;
   distinguish permanent visible channels from dry drainage lines. Design width,
   depth and bank/floodplain sections in game meters using a versioned terrain
   style policy. Preserve current uniform scale semantics; any artistic feature
   exaggeration must be explicit and save-keyed. Trial creek widths1–3m and depths
   0.25–0.75m, larger rivers according to drainage rank (design ranges, not universal
   hydraulic laws). Reconstruct downstream-monotone bed/water profiles and connected
   confluences; add explicit still-water steps where needed. Verify actual integer
   occupied water cells, not just float profiles. Stop fading whole tributaries at
   arbitrary triangle edges. Build shared river curves and bank constraints.

3. Replace triangle-local embellishment with canonical catchment refinement.
   Retain geographic atlas/climate. Prototype one valley plus neighboring units
   with shared divides, outlet levels, river crossings and runoff budgets. Refine
   through bounded resolutions, comparing constrained analytic surfaces with staged
   stream-power/thermal/deposition amplification. Select by first-person quality,
   deterministic seams, monotone drainage and p95 build cost. No independent erosion
   per renderer tile. Reject incompatible children; fix parent constraints.
   Add shoulders, gullies, floodplains and deposits before centimeter grain.

4. Feed painting final local slope/curvature, river distance, deposition and soil.
   Coherent material patches must agree with occupied material IDs and preserve
   one solid color per voxel. Fine roughness comes from material/landform fields;
   deliberate plains remain smooth. Do not scatter independent noise everywhere.

5. Preserve channels in every LOD. Query vector coverage even when coarse sample
   vertices are dry; use matching ground/water geometry and consistent confluence
   identity. Integrate projected coverage for subpixel water, never floating ribbons.
   Compare native wet masks and coarse coverage, silhouette errors and seam profiles.
   Quantization terraces are partly inherent to exact grid occupancy; isolate them
   from coarse faceting and aliasing before changing reconstruction or appearance.

## Efficiency and acceptance

Keep approximately160MiB atlas; separate bounded refinement budget (initial target
64–128MiB, measure before committing). Store2D physical fields and sparse vector
features, voxelize on demand. Example257² patch with8f32 fields is~2MiB before
halos, graph/index/cache overhead. Canonical patch IDs and parent/config hashes;
immutable publication and cached finished results. Prepare collision neighborhood
before entry; visual loading may not change authoritative ground beneath player.
Near-first cancellable workers, deduplicated jobs, no long solve in voxel queries.
Optional compute acceleration only after profiling; integratedGPU bandwidth is
shared with rendering. Record cold/warm latency,p95/p99 frame time, queue age,
resident/peak memory and stop/resume coverage on hardware.

First deliverable: one2×2km playable valley and adjacent refinement boundaries,
connected visible river, interesting100–500m walk, varied coherent materials,
player-eye captures after settling. Then validate other scenes/seeds before broad
promotion. Passing unit tests is insufficient visual acceptance. No engine rewrite
or global millimeter erosion is required for this work.

## Primary references reviewed

- Schott et al2024 project and MIT reference implementation: staged erosion,
  thermal transport,deposition alternating with upsampling. Reference pipeline
  requires adaptation for canonical catchment boundaries and runtime budgets.
  https://h-schott.github.io/p/mserosion/
  https://github.com/H-Schott/MultiScaleErosion
- Genevaux et al2013: hierarchical hydrology graph and continuous terrain features.
  https://perso.liris.cnrs.fr/eric.galin/Articles/2013-river-networks.pdf
- Red Blob mapgen4 Apache2: inspected geometry.ts river flow threshold,width control,
  explicit incoming/outgoing confluence geometry. Useful topology/render separation;
  stylized map geometry is not a voxel-world collision solution.
  https://github.com/redblobgames/mapgen4/blob/main/geometry.ts
  https://www.redblobgames.com/maps/mapgen4/

No runtime code changed in this diagnosis pass. References are design evidence,
not proof of speed or quality in this engine. Pin revisions and preserve notices
before adapting third-party code.
