# Geography-aware climate and drainage — step 2

## Scope and current status

The headless atlas compiler now supports `--climate` (implies `--atlas`). This is
the coarse climate/drainage stage, not live weather and not production integration.
The geographic controls from step 1 are unchanged. Rainfall now affects erosion,
so the final terrain differs from the uniform-runoff step-1 solve. The saved
climate is authoritative for painting: repainting cannot silently substitute an
unrelated climate preset. Steps 3–5 (constrained refinement, integration and
promotion) remain pending.

## Physical fields and complexity

`atlas_climate.rs` builds one annual/seasonal field with two O(N) upwind DAG
sweeps. The logical lattice approximates the irregular solver positions for
transport; actual physical node positions determine latitude and broad temperature
variation. This discretization is fixed for a saved atlas, not changed by LOD.

- Sea-level temperature varies with latitude (default 35° at z=0, 111.32 km/degree)
  and a small broad anomaly; elevation cools it at 6.2°C/km. The 1000 km example
  spans about 9° of latitude, not every climate on Earth.
- An O(N) eight-neighbor chamfer distance from actual ocean nodes increases
  seasonal temperature amplitude inland. It is an approximate distance, not an
  exact coastline geodesic. With no ocean in the domain the assumed context is
  continental; cropped domains must reuse their parent atlas climate.
- Prevailing transport is 25° toward +x/+z; a 15% secondary season differs by
  155°. Ocean cells replenish vapor. Land can recycle 22% of precipitation but
  has no independent humidity recharge. Boundary humidity is explicit, 0.65 by
  default, representing the unmodeled external atmosphere.
- Lifting and cooling condense moisture. Condensate advects and falls over an
  18 km characteristic distance rather than raining out on one numerical cell.
  Background depletion and ocean recharge use physical length scales. This
  follows the finite-advection/fallout concept, not the full Smith–Barstad model.
- Rainfall and PET are calibrated annual estimates. Turc–Pike n=2 partition
  gives runoff = P - P*PET/hypot(P,PET). Land AET cannot exceed P or PET.
  Applying a catchment relation at nodes is a coarse approximation. Atmospheric
  recycling is not a globally closed coupled ocean/soil water budget.
- Four bounded climate updates during evolution plus one final update. Erosion
  uses accumulated annual runoff / 0.3 m/year in place of geometric area, while
  geometric catchment areas remain independently available and conserved.

No runtime clouds, pressure solver, live fluid, vegetation, glacial process,
channel cross-section excavation or native-voxel weather work is introduced.

## Watersheds, lake outlets and accounting

`atlas_hydrology.rs` operates on the final flood/receiver graph. Ocean nodes are
fixed sea-level sinks. Equal-spill components include their dry saddle nodes,
which makes reservoir consolidation acyclic. A reservoir has one explicit
outlet. Potential spill paths remain available for terrain diagnostics; annual
flow and watershed IDs account for the reservoir rather than multiplying water
among its internal virtual branches.

A permanent spill-level lake is emitted only if annual inflow can meet the
incremental open-water evaporation demand (PET minus the land AET already
accounted for). Otherwise it is marked a closed/seasonal sink with zero outflow
and no fabricated permanent lake surface. This conservative screening does NOT
solve partial nested lake filling. Such a basin may physically support a smaller
permanent pond; resolving that requires a depression hierarchy and finer basin
geometry. No claim of a full Fill–Spill–Merge implementation is made.

The annual ledger checks local runoff = external outlet export + incremental
lake/closed-basin evaporation. Terrain sediment accounting remains separate.
Permanent lakes share a level surface. The PNG clips shore triangles to that
level and never interpolates different water bodies into a sloping water plane.

The painter uses final temperature/moisture and runoff-aware valley wetness.
Its dry-to-green palette is continuous, with the existing footprint-filtered
physical color variation and one solid color per native voxel. There are no
independently assigned biome patches or texture assets.

## Saved data

All arrays follow `nodes.bin` ordering:

- `climate.bin`: 8 little-endian f32/node: mean, cold, warm °C; rain, PET, runoff
  mm/year; moisture suitability; approximate coast distance km.
- `flow.bin`: 2 little-endian f64/node: annual-average discharge m³/s and a
  diagnostic hydraulic-geometry width estimate in meters. Width is not yet an
  excavated channel. Within reservoirs only the outlet carries downstream flow.
- `watershed.bin`: u32/node, marine/domain outlet or closed-basin outlet node ID.
- `lake-id.bin`: u32/node; `u32::MAX` means no reservoir component.
- `lakes.csv`: explicit outlet, spill, permanent flag, annual budget and storage.
- `water.bin`: actual permanent water surface or dry ground height, distinct
  from the virtual flood spill elevations used internally for erosion.
- Existing `nodes.bin`, `triangles.bin`, `ocean.bin`, `paint.bin` remain supported.

## References inspected

- Red Blob Games Mapgen4 source, `assignRainfall`/`assignRivers`:
  https://raw.githubusercontent.com/redblobgames/mapgen4/master/map.ts
  Inspected wind-ordered neighbor transport, ocean evaporation and rainfall-fed
  river flow. Conceptual reference only; no Apache-licensed source copied.
- Smith & Barstad (2004), *A Linear Theory of Orographic Precipitation*:
  https://journals.ametsoc.org/view/journals/atsc/61/12/1520-0469_2004_061_1377_altoop_2.0.co_2.xml
  Finite condensed-water advection and fallout motivate avoiding instant
  one-cell precipitation. Our two-dimensional sweep is much simpler.
- Rouholahnejad Freund et al. (2020), spatial heterogeneity and Budyko estimates:
  https://hess.copernicus.org/articles/24/1927/2020/hess-24-1927-2020.html
  Annual water/energy partition and cautions about spatial aggregation.
- Barnes et al. (2021), Fill–Spill–Merge:
  https://esurf.copernicus.org/articles/9/105/2021/
  Distinguish potential spill topography from water available to fill it. Full
  nested partially-filled depression dynamics remain deferred as noted above.

## Reproduction

```sh
cargo build --manifest-path successor/Cargo.toml -p microvoxel --release \
  --bin landscape_lab --bin landscape_paint
successor/target/release/landscape_lab --climate --extent-km 1000 --side 1025 \
  --seed 42 --output output/atlas-climate/seed42
successor/target/release/landscape_paint output/atlas-climate/seed42
MPLCONFIGDIR=/tmp/microvoxel-mpl python3 tools/render_landscape_lab.py \
  output/atlas-climate/seed42 --painted --overview-only
MPLCONFIGDIR=/tmp/microvoxel-mpl python3 tools/render_atlas_climate.py \
  output/atlas-climate/seed42
```

Optional compiler controls: `--latitude`, `--wind-deg`, `--boundary-humidity`.
These belong to the atlas definition. No game menu is changed in this pass.

## Validation

90 library tests passed; 1 pre-existing manual benchmark ignored. New tests
cover ocean moisture supply with a dry boundary, no spontaneous rain over dry
land, reversed rain shadows, repeatability, inland seasonality, annual water and
energy bounds, rainfall-weighted catchments, wet/dry lake screening, unchanged
geographic controls, final actual-water downhill flow, sediment conservation,
and invalidation of stale hydrology after rerouting.

## Full seed-42 result and inspection

- 1000 × 1000 km, 1025² irregular nodes, 976.5625 m nominal spacing; same seed,
  initial terrain, uplift and node coordinates as step 1 (array equality checked).
- 320 epochs; 177.360 s compiler time, 395936 KiB (~387 MiB) peak RSS. This is
  offline compilation, not gameplay; Python rendering excluded. Higher memory
  than step 1 remains above the proposed runtime budget and needs integration
  work. The single observed faster solve is not a controlled speedup benchmark.
- Highest peak 4881.565 m; bathymetry minimum -542.244 m. 71 permanent reservoir
  components and 132 water-limited closed/seasonal components. A component count
  is not the count of visibly large lakes.
- Annual runoff 8.85938e10 m³; outlet export 8.45643e10 m³; incremental reservoir
  evaporation 4.02947e9 m³; residual 0.000219 m³/year (~2.5e-15 relative).
  Terrain-volume residual 830.219 m³ against 1.92179e14 m³ uplift (~4.3e-12).
- All saved arrays finite, water at/above ground, marine water exactly zero,
  runoff nonnegative/bounded by precipitation, and zero actual-flow uphill links.
  Virtual spill paths in seasonal basins may cross uphill beds but carry no
  invented outgoing surface flow. `validation.json` records the full checks.
- Land precipitation 10/50/90th percentiles: 36.9/271.9/781.2 mm/year; maximum
  4029.9 mm/year. About 35.4% of land has moisture suitability <0.2 and 12.4% >0.5.
  This is a temperate/subtropical continental example, not an attempt to squeeze
  tropical rainforest and Arctic tundra into every 1000 km square.
- Painting 0.570 s (separate from reading/compiler/rendering).
- Additional 257² / 320-epoch seeds 1 and 99 finished in 5.955/6.712 s; their
  annual water residuals were 0.000106/0.000346 m³/year. Seed-99 colored preview
  inspected: different geography retains coherent wetter mountains and drier
  lowlands, without the same dry-basin arrangement as seed 42.

Inspected `painted-isometric.png` and `climate-and-drainage.png`: seed 42 now has
broad dry interiors, greener windward slopes, limited snow and permanent lakes.
The diagnostic rainfall field explains the color transitions. At this scale,
most rivers are subpixel in the true-scale overview; the diagnostic flow map
uses explicitly exaggerated line widths to expose connectivity. This is not yet
native river geometry. Coasts remain broad/smooth; the new climate does not fix
that geometric limitation. Cropping/refining consistent terrain is step 3.

Outputs in `output/atlas-climate/seed42/`:
- `painted-isometric.png`: main 3600×2200 preview, true vertical scale.
- `climate-and-drainage.png`: six physical-field/flow diagnostic panels.
- `before-after-1000km.png`: complete step-1/step-2 visual comparison (includes
  both climate-driven geometry changes and the updated continuous dry palette).

The two smaller-seed checks and preliminary seed-42 check are diagnostics at
coarser sampling, not substitutes for the saved main atlas or finer crops.
