# Terrain correction implementation and visual review

2026-10-29. Generator baseline **5**. This pass follows
[TERRAIN_CORRECTION_PLAN.md](TERRAIN_CORRECTION_PLAN.md), preserving the native
integer world, both renderers, bounded streaming and the real-data import option.
Generated captures/benchmarks stay under ignored `output/correction/`.

## Implemented changes

1. **River embedding.** Curve sampling and footprint clipping live in
   `src/generation/river.rs`. Routing nodes share downstream water profiles and
   stable reach IDs. Four-segment Hermite curves retain shared endpoints/tangents;
   curves crossing excessive relief fall back to the routing edge. Ordinary
   carving only lowers terrain. Profiles are constrained by nearby ground and
   lakes. Small dry saddles admit bounded breaches; large virtual spill routes
   and profiles needing excessive excavation are rejected explicitly. Static
   lake levels are reconciled by basin ID; adjacent lake elevations are selected
   by ownership, never averaged into a new intermediate plane. Bank materials
   vary coherently among sediment, gravel and local soil instead of universal
   pale outlines. `ProceduralTerrain::river_diagnostics()` reports segments,
   reaches, blocked spill links, straightened segments and unresolved profiles.

2. **Water-aware LOD and allocation.** Indexed centerline/cell intersections
   preserve information missed by four quadrature points, combine coverage for
   segments of the same reach and keep a representative water profile. Resolved
   coverage influences proxy bed/water geometry; smaller coverage filters into
   proxy appearance. Coarse summaries choose a representative wet child instead
   of averaging unrelated water elevations. A priority queue spends splits on
   nearby terrain, the view and projected water features before peripheral
   refinements. Each admitted split includes its local 2:1 balancing closure;
   it does not clone/rebalance the entire layout for each candidate. Resident
   tile/cache ceilings and staged cover replacement remain enforced.

3. **Contour geometry.** Horizontal LOD no longer rounds vertical heights to
   horizontal cell size. Mesh rectangles and ray runs both retain native integer
   Y intervals. Coarse rectangles carry one filtered proxy color rather than a
   native-height color array. Equal-height/color horizontal rectangles merge.
   Junction references pack losslessly into eight bytes when coordinate limits
   permit, retaining a twelve-byte fallback. Slope-light sampling uses a physical
   one-meter minimum instead of a hard-coded native-cell count. Native occupancy,
   native color and collision do not depend on display settings.

4. **Middle-scale relief.** Terrain-guided incision has a regional amplitude
   variation, nested troughs, curvature-conditioned footslope/debris shapes and
   geology-conditioned ledges with varying bedding thickness/dip. Plain and
   drainage masks limit their effect. Periodic ledges avoid a discontinuous
   sawtooth reset. Fixed physical sampling anchors and interpolated cached
   gradients keep relief continuous and resolution-independent. During review a
   continuously changing anchor hash caused noisy detail; a regression test now
   catches that error. These are bounded procedural geomorphology approximations,
   not mass-conserving talus or erosion simulations.

## Reproduction and evidence

Build with a Rust toolchain supported by `native/Cargo.toml`, then run:

```sh
python3 tools/capture_terrain_sweep.py --river-checks --output output/correction/after
cargo test --locked --all-targets
cargo test --locked --manifest-path native/Cargo.toml
cargo test --locked --manifest-path native/Cargo.toml -- --ignored --test-threads=1
native/target/release/microvoxel-game --world procedural --seed 2027 --climate continental --stream-test
```

The original ten cameras retain seeds, climates, FOV, image size, quality and
residency limits. Four additional cameras test actual wet channels after drainage
correction changes the original invalid river site. Manifests include full
commands and binary SHA-256. Original images/binary were retained under
`output/correction/before/`; do not overwrite them with this build.

The prior "effective pixels" value was a globally relaxed planning threshold.
The current visible P95 estimate is the projected cell-size P95 of heuristically
visible, non-native leaves, floored at requested quality. Neither is a certified
silhouette-error bound; those values cannot be compared as the same metric.

## Efficiency and limits

Regional graph/profile work happens once on the existing 16 m scaffold. Queries
visit spatially indexed curves and bounded summary caches. A compact gradient
field avoids repeatedly differentiating the regional surface for every octave.
No fine 10 km heightmap, per-voxel simulation or GPU-resident full world is added.

The improved vertical representation has a measurable cost: smooth slopes now
have more distinct height planes/rectangles than the rounded old proxies. Compact
references and constant-color rectangles reduce overhead but do not make this
free. The plan's coarse-build slowdown gate is **not met** on all sampled patches;
this pass must not be described as an unconditional performance optimization.
See the saved isolated before/after CPU measurements and capture logs.

Remaining limitations:

- The 16 m routing graph remains the drainage scaffold. Bounded saddle breaches
  are not an arbitrary watershed rerouter; rejected routes can terminate streams.
  Diagnostics expose those cases. Lake water balance is still a static full/dry
  approximation, not a reservoir solve or fluid simulation.
- Footprint coverage is an approximation. A coarse water cell can widen a stream,
  and very thin tributaries can still disappear or fragment under tight budgets.
  The allocator increases priority; it cannot promise every visible feature fits.
- Sampled summaries have residual allowances, not rigorous extrema. They only
  rank refinement and are not trusted to prove terrain invisible.
- Dark, resolvable native voxel stair edges remain. Removing horizontal-step Y
  rounding corrects exaggerated distant terraces, not every visible contour.
  Temporal smoothness and boundary quality need continued movement inspection.
- Mountain ribbing, abrupt material distributions and simple shorelines remain
  artistic/geological limitations. This is an incremental improvement, not a
  finished Earth-like river/landform system.

## References and source use

[Procedural Riverscapes (2019)](https://people.cs.uct.ac.za/~jgain/wp-content/papercite-data/pdf/peytavie2019.pdf)
and [Terrain Generation Using Procedural Models Based on Hydrology (2013)](https://cs.purdue.edu/homes/bbenes/papers/Genevaux13ToG.pdf)
inform connected drainage/profile/carving organization.
[Geometry Clipmaps](https://hhoppe.com/geomclipmap.pdf) informs bounded
multiresolution representation and consistent boundaries; the engine retains its
axis-aligned quadtree proxies.
[Controlled Procedural Patterns](https://onlinelibrary.wiley.com/doi/10.1111/cgf.14992)
and [Multi-Scale Erosion](https://h-schott.github.io/p/mserosion/) inform guided
hierarchical relief. The latter's [MIT code](https://github.com/H-Schott/MultiScaleErosion)
was reviewed as a reference; its compute runtime was not imported.
[RichDEM Barnes2014 source](https://github.com/r-barnes/richdem/blob/master/include/richdem/depressions/Barnes2014.hpp)
was reviewed for the virtual-fill/physical-water distinction. No GPL source was
copied into this MIT repository. Implementations here are original adaptations;
none of the papers' timing or accuracy results are claimed for this engine.

## Final capture review and measured results

Inspected all ten replacement views, the two additional wet-river high/low pairs,
a matching ray capture and sampled movement frames from both backends.
[Before/after gallery](../output/correction/gallery.html),
[ten-view overview](../output/correction/after/contact_sheet.jpg),
[mesh movement samples](../output/correction/after/motion/mesh_contact.jpg) and
[ray movement samples](../output/correction/after/motion/ray_contact.jpg).
The GIFs are sparse route samples, not smooth real-time recordings.

| Views | Assessment |
| --- | --- |
| 01 valley, 10 plain | Clear improvement in distant hill contours and reduced large LOD terraces. The broad landscape remains recognizable. Dark, resolved foreground stair faces remain. |
| 02–04 original river | The raised water-topped aqueduct is gone. The original bank is dry; short incised channel remnants elsewhere in the view remain straight and can end abruptly. This is corrected embedding, not a completed river network. |
| 05–06 lake | Main water extent remains stable between high/low quality. Shoreline/silhouette steps are reduced; steep shore walls and some surface/lighting bands remain. |
| 07 humid hills | Lowland/rock coverage remains coherent. Channels are more clearly embedded, but angular paths and mottled material contours remain conspicuous. |
| 08–09 dry/continental mountains | Crests and local relief are more detailed while broad mountains remain intact. Ledge/debris variety is subtle at these distances; repetitive ribbing is still a weakness. |
| 11–12 additional dry river | A connected tributary/junction survives the large quality change. Width and banks become blockier at low quality. Long straight segments still look artificial. |
| 13–14 humid inlet | River/lake connection and nearby wet features survive both presets. Banks are less uniformly pale, but the system still creates overly simple shorelines and isolated wet pockets. |
| Matching ray view | Same broad terrain/water structures; local detail differs with budget/backend representation. Mean RGB difference is approximately one 8-bit value/channel; this is an observation, not a pixel-identity requirement. |

**Verification:** 90 core tests, 16 native CPU tests and all 5 explicitly run GPU
tests passed. Checks cover continuity across physical anchors, identical terrain
across world grids, distinct lake planes, bounded/inert edits, native-height
coarse boundaries, large/fine coordinates, preserved color atlases, compact/wide
junction references, mesh/ray water agreement and live source/backend switching.
`git diff --check` passed.

The held-out seed-2027 quality suite completed 18 noise/geomorphic cases over
three climates. For the nine geomorphic cases (6.25 cm reference versus 1 m proxy,
32 m patches, 640×360), mean silhouette edge distance was 0–0.878 L1 pixels,
maximum 0–6 pixels, lost foreground at most 1.79%, and lost water pixels at most
1.253%. Sampled physical-field differences across 25 cm/6.25 cm/1 cm/1 mm grids
were zero. These are small-patch measurements, **not** river connectivity,
shoreline-width guarantees or full-world error bounds. CSVs and raw masks are in
`output/correction/quality/`.

On the Radeon 760M, the final paced held-out movement run covered about 37.1 m
per case, with 240 walking, 240 sprinting and 120 turning ticks, at 960×540.
All four low/balanced mesh/ray cases had **zero** native-detail misses at the
nine sampled points within 3 m and no player collision failures. Work p95 was
12.3–12.8 ms for mesh and 15.6–18.4 ms for ray. Initial coverage took 6.3–7.5 s.
This is synchronized work timing, not isolated GPU execution time or a universal
FPS claim. A separate screenshot-readback run saved seven movement frames/case;
do not use its timings for the performance comparison.

The first low-ray test had 75 sprint and 12 turning misses (maximum 1.45 s).
Large atomic patch batches made ready nearby data wait for other jobs. Moving
updates now batch two local changes rather than twelve; stationary refinement
retains twelve. Repeated tests gave zero misses. Original failing diagnostics are
retained under `output/correction/streaming_initial/`. Still captures use the
same final terrain/shaders and bypass this interactive batching path; their
recorded binary hash precedes this movement-only scheduling adjustment.

A sequential release CPU probe sampled nine cold tiles at each of four levels
for each of three seeds. Regional generation remained approximately 2.4–2.9 s;
regional retained data increased about 4.5–5.3 MiB, remaining approximately
51–52 MiB. Coarse median mesh+preparation builds increased approximately
24–48%; sampled coarse payloads increased about 2.4–8.8× because native Y keeps
many planes the old rounded surface merged. Native-tile median timings ranged
from about 14% faster to 15% slower across those patches. These are a small,
scene-dependent sample, not an optimized cross-machine benchmark. The resident
GPU budgets still cap caches; better geometry can force fewer resident tiles.

Thus regional-cost/memory targets passed, but the proposed **15% coarse-build
slowdown gate failed**. The visual improvement is real and the measured movement
route works, but further compact geometry/visibility work is warranted before
raising default detail. This tradeoff is intentionally exposed rather than
reported as a general speedup. Probe source, old baseline rlib/binary and
before/after CSVs remain in `output/correction/` for reproduction.

River diagnostics for seeds 42/137/909 respectively report 6,898/3,314/7,764
accepted curve segments and 206/61/145 unresolved profiles. Blocked spill-link
counts include small routing cells, not just significant rivers. Those unresolved
cases explain some abrupt endings and are concrete remaining hydrology work.

The release game used by the existing `MicroVoxel` launcher is rebuilt. Select a
procedural world in the World menu to inspect these changes; the existing local
Earth-import option remains available. No live weather, fluid physics or material
properties were added. Generator-4 edit saves remain incompatible with baseline 5.
