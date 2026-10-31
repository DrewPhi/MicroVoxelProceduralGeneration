# First landscape-evolution visual checkpoint

2026-10-30. Experimental, headless Rust generator; **not the playable world's default**. This completes the first inspectable 100 km prototype, not all replacement-plan acceptance gates or production integration.

## Implementation

- `successor/world/src/generation/landscape_experiment.rs`: independent coarse solver using meter/year units, evolving uplift, downstream-first implicit n=1 stream-power incision, sediment supply/transport/deposition and conservative hillslope transfers.
- `successor/world/src/bin/landscape_lab.rs`: headless compiler and binary field exporter; `--baseline` samples the existing generator6 for comparison. No new Rust dependencies.
- `tools/render_landscape_lab.py`: simple triangle-based diagnostic rendering of the actual exported heights. Gray surfaces, fixed lighting, true-scale isometric projection, no fog, textures, vertical exaggeration or added geometry. This is not a screenshot of the voxel renderer; triangles expose the coarse numerical surface rather than changing the voxel engine.

The spatial controls are a deliberately selected young curved uplift belt and an older upland. Seed affects initial small relief, resistance, uplift variability and node jitter. The seed sweep tests the solver under the same geographic fixture; it does NOT demonstrate different continents or all biome diversity. Wider geographic synthesis is the next planned stage.

Noise occurs in initial conditions and geological coefficients. There is no independent ridge-noise addition or channel-carving pass after evolution.

A bounded jittered quad triangulation supplies irregular routing edges. It is explicitly not a general Delaunay mesh. The regular comparison uses eight-neighbor routing. Both use node control areas from triangle areas. Maximum jitter is bounded to preserve positive triangle winding; a regression test verifies winding and total domain area across several seeds.

References and licensing decisions are in `TERRAIN_GENERATION_REPLACEMENT_PLAN.md`. This implementation uses published numerical ideas, not copied GPL solver code. The prototype uses a heap priority flood (O(N log N) worst case), not the proposed future linear basin-graph algorithm.

## Outputs to inspect

All under ignored `output/landscape-milestone/`:

- `revised42/isometric.png`: principal 3600×2200 PNG, 100 km × 100 km.
- `revised42/range-closeup.png`: 25 km × 25 km crop of the SAME solved heights, centered at (60 km, 55 km).
- `revised42/diagnostics.png`: top-down relief/drainage, elevation, uplift and mobile sediment.
- `comparison.png`: generator6 versus experiment with matching projection, light, extent and nominal sampling. Different models do not imply identical geographic features for the same seed. Generator6 retains its 2048 m ceiling; the experiment is unconstrained by the game's height ceiling.
- `raster42/isometric.png`: regular-grid comparison.
- `seed-sweep.png`: eight seeds at lower numerical resolution, same uplift-layout fixture.
- Each run contains `nodes.bin`, `triangles.bin`, `summary.txt`. Nodes: eight little-endian f64 values [x,z,height,initial_height,uplift,drainage_area,sediment,receiver_index]; triangles: three little-endian u32 indices.

The initial `seed42/` experiment was rejected for overly parallel valleys. It also exposed an overlarge node-jitter bound; that bound was fixed and tested before the revised run. Use `revised42/` for evaluation.

## Measured results

Main run: 513×513 nodes, nominal 195.3125 m spacing, 320 epochs of 10,000 model years, seed42, 100 km domain. These geological time parameters are artistic model controls, not a calibrated geological reconstruction.

- Elevation: 83.443–3255.643 m.
- Approximately 65% of nodes have absolute downstream slope below 0.05. This is a downstream-edge metric, not an exact surface-gradient land-area measurement.
- Solver plus diagnostics: 40.803 seconds in the recorded release run; image rendering is additional. This is world-generation work, not a frame time or streaming latency benchmark.
- Material-volume residual: 51.8125 m³ relative to 4.328×10¹² m³ uplift input, about 1.2×10⁻¹¹. Floating-point roundoff remains. The erosion counter describes fluvial incision; hillslope flux is separately accounted in total volume.
- 11,302 drainage nodes exceed 5 km² contributing area. 145 of their receiver connections remain virtual uphill spill routes. These are not rendered as downhill rivers; full retained-lake/spill/breach handling is still needed.
- Median channel-edge angular departure from the nearest 45° direction: 7.44° irregular versus 0° regular. This confirms reduced exact directional locking, NOT isotropy or absence of grid bias. The underlying quad connectivity still affects direction distribution.
- Regular comparison: 41.191 seconds, maximum height 3500.521 m. These single runs do not establish a speed advantage for either graph.

Eight-seed robustness sweep: 1,7,19,42,99,123,777,2026 at 257×257 nodes. Peak heights 2138–2907 m; finite outputs and relative volume residuals below 4×10⁻¹² in recorded runs. Unresolved large-flow spill links vary (33–404). At seed42, 257-node peak is 2700 m versus 3256 m at 513 nodes: the numerical model is not resolution-converged. Numerical landscape resolution must be a fixed saved-world parameter, not a renderer LOD setting. Future multiscale refinement must preserve its parent rather than rerunning this whole solve at every resolution.

## Visual inspection and remaining gates

Inspected the full gray overview, same-world close crop, old/new comparison, diagnostic map and eight-seed contact sheet.

The experiment produces connected ridge/valley structure and a clearer contrast between the young range, gentler old upland and lowlands. The previous model's grainy, independently textured mountain belt is substantially reduced. The first iteration's near-identical parallel tributaries improved after broadening/varying uplift and providing more varied initial drainage conditions.

Remaining weaknesses: the chosen geographic fixture still favors one obvious spine; some tributaries remain too parallel; the surface is visibly faceted at 195 m sampling; many lowlands are very smooth; actual lake water levels and complete spill treatment are unfinished. Open domain boundaries influence drainage. There is no climate-driven rainfall, glacial erosion, local terrain amplification, biome painting or calibrated DEM comparison in this prototype.

Do not promote this generator as production-complete or resolution-independent based on these images. The visual checkpoint is ready for user inspection. Before broader integration, resolve basin handling, improve directional bias, establish time-step/spatial convergence and parent-child constraints, and evaluate real bare-earth reference terrain. Peaks exceed the current game's ceiling, so integrating these heights also requires the planned vertical-range audit.

## Verification

`cargo test --manifest-path successor/Cargo.toml -p microvoxel --lib --release`: 77 passed, 1 existing manual benchmark ignored. Four new tests cover acyclic routing/area conservation including a closed bowl, repeatability/material conservation, an unchanged flat surface, and positive triangulation/domain area. Python renderer syntax check and repository diff whitespace check passed.

## Reproduce

From repository root, with the installed Rust toolchain available:

```sh
cargo run --manifest-path successor/Cargo.toml -p microvoxel --bin landscape_lab --release -- --seed 42 --side 513 --output output/landscape-milestone/revised42
MPLCONFIGDIR=/tmp/microvoxel-mpl python3 tools/render_landscape_lab.py output/landscape-milestone/revised42
MPLCONFIGDIR=/tmp/microvoxel-mpl python3 tools/render_landscape_lab.py output/landscape-milestone/revised42 --crop 60000 55000 25000
```

Add `--raster` to compare routing geometry; `--baseline` to sample generator6. Use distinct output directories. Python requires NumPy, Pillow and Matplotlib; these are development/export tools, not game runtime dependencies.

No launcher replacement, world-save changes, GitHub push or unrelated local modifications were included in this checkpoint.

## Focused follow-up: drainage continuity and directional bias

Implemented 2026-10-30 after user review. Frozen before data/images are in
`output/landscape-focused-pass/before/`; corrected after data/images in `after/`.
The comparison is `output/landscape-focused-pass/before-after-100km.png`.
This remains an experimental headless generator; no playable-world replacement.

Changes:

- Added `landscape_mesh.rs`: wider bounded point jitter and deterministic Lawson
  edge flips replace quad-imposed routing connectivity with a locally Delaunay
  triangulation. Initial concave quadrilaterals select a valid diagonal. Floating
  point circle tests use a scale-relative tolerance; this is not a general exact
  computational-geometry library. Independently tested positive winding, total
  area, and the opposite-angle Delaunay condition across multiple seeds.
- Exact priority-flood spill heights replace artificial epsilon slopes. The
  acyclic flood tree resolves equal-height lake routing without tilting water.
  Exported `water.bin` contains one little-endian f64 water-or-ground spill
  surface elevation per node, in the same order as `nodes.bin`.
- Snapshot submerged nodes before each erosion pass. Lake beds are protected
  from fluvial incision; incoming sediment settles within available lake depth
  before excess travels onward. A regression checks that erosion of dry ground
  does not turn its old elevation into a fictitious lake surface.
- Hillslope diffusion substeps now respect actual edge lengths/control areas;
  the stability coefficient is precomputed once per mesh.

Static water assumes wet, overflowing conditions. This does not establish a
rainfall/evaporation water budget, dry-basin behavior, live fluids or river widths.
Beds may rise along submerged lake traversals; their water surface must not.
The grayscale overview continues to display ground geometry, while diagnostic
drainage includes these water connections and marks submerged nodes in cyan.

References: [Lawson edge-flip description](https://www.cise.ufl.edu/~ungor/delaunay/delaunay/node5.html)
and [Barnes et al., Priority-Flood](https://rbarnes.org/sci/2014_depressions.pdf).
No reference implementation code was copied. Existing uplift, erodibility,
initial physical fields, epoch count and duration were unchanged.

Matched 100 km seed42 results (513² nodes, 320 × 10,000-year epochs):

| Measurement | Before | After |
| --- | ---: | ---: |
| Peak elevation | 3255.643 m | 3168.594 m |
| Median departure from nearest 45° routing direction | 7.44° | 10.83° |
| Eightfold angular concentration, abs(mean(exp(8i angle))) | 0.2990 | 0.0278 |
| Generation + diagnostics | 40.803 s | 49.033 s |

Angular measurements use edges with contributing area >5 km². They indicate
reduced lattice locking, not a universal realism score or proof of isotropy.
The before run had 145 unresolved uphill bed/spill links at that threshold.
Afterward all receiver links over ALL nodes have non-increasing water-or-ground
surface elevation; 23 large-flow links cross uphill bed under water. The 24 wet
nodes in this particular after scene form very shallow depressions (maximum
depth 0.0334 m), not dramatic new lakes. A deep closed-bowl test separately checks
flat lake planes and downstream connectivity. Do not advertise a large-lake
simulation based on these tiny pools.

Material-volume residual is 39.5864 m³ / 4.328×10¹² m³ uplift, approximately
9.1×10⁻¹². Additional 257² runs for seeds1 and99 have no uphill water links,
finite outputs, nonnegative water depths and relative volume errors below 2×10⁻¹².
Release world-library tests: 79 passed, 1 existing ignored benchmark. Python
syntax and diff whitespace checks pass.

Both overview projection bounds match exactly: x ±70710.6781 m, projected
vertical range [-146.9694, 81508.0714] m. Same lighting, output size and true
vertical scale. No colors or shading tweaks were used to improve the after view.
Inspected before/after, after close-up and diagnostic maps. The change is subtler
than the initial rewrite: similar overall range, less regimented tributaries,
and explicit continuous drainage through depressions. Remaining broad spine
shape follows the intentionally fixed uplift fixture. Proceed to a first painting
pass rather than expanding this focused task into another geography rewrite.

Reproduce after with the existing commands but output to
`output/landscape-focused-pass/after`; retain the frozen before files for comparison.

## First ground-cover / biome-painting pass

2026-10-30: implemented in separate `generation/landscape_paint.rs`, with a
`landscape_paint` headless binary. Repaints the saved landscape without rerunning
erosion. Experimental only; the playable generator and launcher are unchanged.

Quick reference review:

- [Veloren column painter source](https://docs.veloren.net/src/veloren_world/column.rs.html):
  interpolated environmental fields and blends among grass, rock, soil and snow
  palettes are useful design patterns. No Veloren source was copied.
- [Red Blob Games terrain/climate discussion](https://www.redblobgames.com/maps/terrain-from-noise/):
  explains why elevation-only biome thresholds form bands and why moisture,
  temperature and other environmental inputs matter. Used as conceptual guidance,
  not as a terrain-generation replacement or an implementation copied into Rust.

The painter reuses our existing steady climate sweeps on the logical sampling
lattice; node jitter is approximated for this coarse climate pass. Triangle-based
physical slopes, retained sediment, drainage and final elevations come directly
from the new landscape. Climate affects appearance here, not a new coupled erosion
solve. The model has prescribed boundary moisture, not simulated oceans/weather.

Continuous normalized coverage combines vegetated ground, exposed rock, deposited
soil, alpine snow and wet ground. Grass tint varies continuously with moisture and
temperature; rock tone has broad coherent variation. Slope, soil accumulation,
exposure and warm-season temperature modify coverage. No discrete biome label
boundaries or independent random material picks are used in this pass.

Physical-space color variation is filtered against sampling footprint. Small
patterns disappear in coarse exports instead of aliasing. `voxel_rgb` provides
one deterministic solid RGB for each voxel, independent of face orientation; its
small per-cell grain is separate from lighting. Standing water must be at least
one configured voxel deep to occupy/color that voxel. The ground overview does
not draw broad blue rivers from drainage area: channel width/bank geometry still
needs a later pass. There are no simulated trees or forest canopies in the image.

Coarse colors are blended and averaged in linear RGB; the Python exporter applies
face illumination and converts to sRGB. This remains a triangulated coarse
diagnostic, not a native-voxel screenshot. Fine RGB behavior is shown separately
with flat color swatches at 6.25 cm, one solid pixel per voxel enlarged by nearest
neighbor. Swatch environmental contexts are frozen representatives, not new terrain.

Outputs under `output/landscape-focused-pass/after/`:

- `painted-isometric.png`: 100 km × 100 km, same geometry/camera as gray view.
- `painted-range-closeup.png`: same 25 km crop as prior inspection.
- `painted-diagnostics.png`: cover, mean temperature, moisture and snow fields.
- `paint-swatches.png`: native-scale color-detail illustration, 8 m per panel.
- `paint.bin`: ten little-endian f32 values/node: linear RGB, temperature,
  moisture, then vegetation/rock/sediment/snow/wet-ground weights.
- `paint.txt`: configuration and timing. Coverage statistics are node means,
  not area-weighted biome percentages.

Inspected initial and revised overview/close-up images and native swatches. Initial
temperate palette was too uniformly tan; shifted grass tint toward green while
preserving drier areas. Snow remains localized to the highest, colder terrain;
not every peak is white. This is one temperate geographic fixture, not a complete
world-biome diversity demonstration. Current coarse mesh facets remain visible.

Painting plus binary export and swatch generation took ~0.16 s in a release run
for 263,169 nodes (excluding build, input loading and PNG rendering). No new Rust
dependencies. Repainting produces byte-identical colors and preserves hashes of
nodes, triangles and water. All exported coverage sums and RGB bounds validated.
Release library tests: 81 passed, 1 existing ignored benchmark, including smooth
coverage transitions, footprint filtering and repeatable native colors.

Reproduce:

```sh
cargo run --manifest-path successor/Cargo.toml -p microvoxel --bin landscape_paint --release -- output/landscape-focused-pass/after
MPLCONFIGDIR=/tmp/microvoxel-mpl python3 tools/render_landscape_lab.py output/landscape-focused-pass/after --painted
MPLCONFIGDIR=/tmp/microvoxel-mpl python3 tools/render_landscape_lab.py output/landscape-focused-pass/after --painted --crop 60000 55000 25000
```

An optional second positional argument selects an existing climate preset
(`temperate`, `continental`, `arid`, `subpolar`, `tropical`). It repaints the same
saved terrain, not a newly evolved landscape, and replaces that directory's paint
files. Gray geometry files are retained.
