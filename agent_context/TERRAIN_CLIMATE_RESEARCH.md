# Terrain and climate implementation references

Implemented 2026-10-28. Generator version 2; material schema 2.

## Scope and causal pipeline

Seeded, warped regional controls blend plains, hills, plateaus, basins and mountain
belts independently of climate. A separate geological field supplies granite,
sandstone, limestone or basalt and generation-only erosion coefficients. These
are landscape controls, not a plate-tectonic or geological-age simulation.

On the 626 × 626 grid (16 m spacing), estimate mean/seasonal temperatures with
altitude cooling; sweep moisture from the upwind boundary; condense precipitation
on rising terrain; estimate potential evaporation and moisture availability.
Rainfall-weighted runoff feeds the existing priority-flood drainage and sediment
transport. Refresh climate every eight erosion passes and once at completion.
A static lake screen compares external runoff plus direct basin rainfall with
potential evaporation; only sufficiently supplied basins fill. Final wetness
combines moisture, convergent drainage, soil retention and slope.

Native columns refine that baseline with soil-conditioned noise down to the
microvoxel scale, channel profiles and lake shores. Climate, slope, soil and
geology choose materials and stored appearance. Biome names are diagnostic
suitability labels derived from conditions: they never choose terrain recipes.
Woodland suitability generates no trees. All voxel faces retain one base color;
lighting remains a separate operation.

## Literature and open source

| Reference | Relevant method | Use here / limits |
|---|---|---|
| [Cordonnier et al., *Large Scale Terrain Generation from Tectonic Uplift and Fluvial Erosion*, 2016](https://perso.liris.cnrs.fr/apeytavi/website/publication/hal-01262376/) | Uplift and fluvial erosion organize large-scale terrain | Independent regional uplift controls and runoff-dependent erosion; not their tectonic model or solver |
| [Génevaux et al., *Terrain Generation Using Procedural Models Based on Hydrology*, 2013](https://www.cs.purdue.edu/cgvlab/www/resources/papers/Genevaux-ACM_Trans_Graph-2013-Terrain_Generation_Using_Procedural_Models_Based_on_Hydrology.pdf) | Hydrology organizes coherent terrain features | Drainage networks, valleys and local channel refinement, retaining the existing regular-grid approach |
| [Barnes et al., *Priority-Flood: An Optimal Depression-Filling and Watershed-Labeling Algorithm*, 2014](https://rbarnes.org/sci/2014_depressions.pdf) | Outlet-connected drainage across depressions | Existing heap-based priority flood retained. Fill is virtual; terrain basins and lake beds remain intact |
| [RichDEM Barnes2014 implementation](https://github.com/r-barnes/richdem/blob/master/include/richdem/depressions/Barnes2014.hpp) | Priority-Flood implementation and queue variants | Reference for future flat-area/FIFO optimization; no source copied or new dependency |
| [Red Blob Games Mapgen4 article](https://www.redblobgames.com/maps/mapgen4/) and [source `map.ts`](https://github.com/redblobgames/mapgen4/blob/main/map.ts) | Upwind moisture transport, orographic loss, rain shadows | Original regular-grid implementation; signed sweep order is a dependency DAG, avoiding sorting. Mapgen4 uses a different mesh. Apache-2.0 reference, no code copied |
| [fastlem, Rust landscape evolution](https://github.com/TadaTeruki/fastlem) | Spatial erodibility and uplift-driven landscape evolution | Reference for separating rock resistance from landscape shape. MPL-2.0 reference, not vendored, linked or copied |
| [Royal Meteorological Society: Rainfall and relief](https://www.metlink.org/resource/in-depth-rainfall-and-relief/) | Orographic ascent and rain shadows | Design/behavior check: a ridge receives more rain on its upwind side; reversing wind reverses this pattern |
| [Ewel & Whitmore, *The Ecological Life Zones of Puerto Rico and the U.S. Virgin Islands*, 1973](https://people.clas.ufl.edu/jackewel/files/ewelwhitmore-eco-life-zones-pr-1973.pdf) | Holdridge biotemperature, precipitation and potential evaporation | Seasonal positive-temperature sampling and 58.93 × biotemperature evaporation proxy, with a game-specific minimum. Not a full Holdridge classification |

All new runtime algorithms are original dependency-free Rust. No external
implementation was incorporated, so no additional runtime license obligations
were introduced. The native renderer uses its existing PNG dependency for export.

## Efficiency and correctness choices

- O(N) steady moisture sweep; two upwind neighbors, no sort or pressure solver.
- Four climate evaluations in a fixed 24-pass regional erosion budget. No climate
  work per frame, per buried voxel or per rendered face.
- Queries interpolate four regional samples. Fine columns use bounded sharded
  caches; channels are spatially binned. Fine height queries reuse the eroded
  regional baseline rather than reevaluating all large-scale noise octaves.
- Temperature, seasonal range, precipitation, humidity and wind are immutable
  baseline estimates. `World::environment_at(CellPos)` ignores Y and returns the
  regional surface environment; it does not describe a live atmospheric parcel.
  Near-surface paint additionally accounts for fine channel/height refinement.
- Seed, preset, normalized wind direction and incoming moisture enter generator
  identity. Saves reject a mismatched baseline instead of silently moving edits.
- Regional grids, debug images and display LOD are separate from canonical voxel
  storage. Debug views cannot recolor, change or regenerate the world.
- Runtime numerical material properties remain empty. Erosion coefficients belong
  exclusively to generation, pending an actual physical simulation design.

## Interpretation and remaining limits

Rainfall is a calibrated annual-equivalent estimate, not a measured climate
prediction. Moisture recharge is prescribed; it is not evaporated from a conserved
water reservoir. Ground wetness is a terrain suitability proxy, not groundwater
physics. Lakes use a binary full/dry annual screen, not partially filled
reservoirs; their balances are not coupled back into routing. Drainage area is **rainfall/runoff-weighted** area rather than literal
upstream geometric area. Seasonal min/max are sinusoidal baseline estimates.

The five presets are alternative regional boundary climates; a single 10 km map
is not expected to contain Earth's entire latitude range. Altitude and rain
shadows create local differences inside each preset. Temperature lapse, storm
transport, erosion and biome thresholds remain tuning parameters.

No live atmosphere, cloud renderer, day/year clock, Coriolis force, moon, waves,
vegetation, seed transport, radar or physical terrain response is implemented.
A future weather solver should sample these baseline fields and maintain its own
time-dependent state, rather than modifying generation or tying it to LOD.

Validation includes rain-shadow reversal, altitude cooling, dry-boundary behavior,
preset ordering, dry-basin behavior, bounded moisture, drainage and sediment conservation, landscape
diversity, canonical color consistency and save incompatibility. See
[performance measurements](PERFORMANCE.md) for actual runtime observations.
