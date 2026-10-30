# World Scale

Implemented 2026-10-29. Independent of voxel centimeters and render pixel targets.

Use **Esc → World → Procedural World → World Scale → Apply World**. Enter a
whole number 1–100: one game meter represents N source-terrain meters. `1` keeps
10 km × 10 km at full size; `10` compresses the same terrain into 1 km × 1 km;
`100` into 100 m × 100 m. Elevations, rivers and sparse geological features scale
uniformly with horizontal distances. Player dimensions, camera FOV, movement
speed and native voxel centimeters stay fixed. Sub-voxel features can disappear
when heavily compressed; native resolution remains authoritative.

CLI: `--world procedural --world-scale 10`. Existing launcher starts the rebuilt
release game. Selection is session-local, as with existing seed/voxel-size fields.
Imported survey worlds keep their measured scale and ignore these fields.

## Implementation

- `ProceduralTerrain::at_world_scale` shares immutable regional data, rescales and
  reindexes sparse geological descriptors, and starts empty bounded column caches.
  `with_world_scale` validates before generating a new source baseline. Source
  extent is retained separately so repeated scale changes do not compound integer
  rounding. Whole-meter playable bounds round upward by less than one game meter
  for ratios that do not divide the source extent exactly.
- Source coordinates multiply by N; ground/water/bedrock elevations divide by N.
  Native field sampling keeps the same source-detail definition across grid sizes.
  Display sampling maps its footprint to source units before filtering.
- Native material/climate suitability is computed before compression. Public
  environmental lengths divide by N and catchment areas by N²; temperatures and
  moisture stay unchanged. Source climate diagnostic maps retain natural units.
- Water footprints, river widths, geological bounds and sampled LOD errors all
  transform into game coordinates. Fine-material coverage filtering uses source
  footprints. Collision, native runs and both renderers use the transformed data.
- Procedural camera spawn positions map into the new domain; the player keeps its
  physical grid and size. Isometric camera framing follows the domain extent and
  overview sampling targets approximately 16 source meters.
- World-menu requests retain scale and reject malformed/out-of-range values before
  starting the background loader. Apply replaces the session and respawns. The
  existing menu/source replacement path resets GPU/world caches.
- Scale contributes to generator identity, preventing incompatible edit loads.
  Scale 1 preserves the existing generator-5 baseline identity and terrain. Generic
  save format and material schema do not change.

## Verification

- 93 core tests passed, including three new scale tests: physical-field congruence,
  climate/material preservation, grid independence, native height queries,
  summaries, sparse feature bounds, water footprints, scale round-tripping and
  save compatibility.
- 17 native CPU tests passed, including scale validation and actual World-tab
  text editing/Apply behavior.
- All five GPU tests passed. The live source-switch test now applies a 1:10,
  5 cm procedural world, verifies the 1 km domain, player clearance, restored menu
  value, empty stale GPU state, and switching back to a survey.
- Actual release launches at scale 10 and 100 with 6.25 cm voxels spawned and
  rendered successfully. Inspected the menu and both captures.
- Release rebuilt; `git diff --check` passed.

Evidence (ignored local artifacts): [menu](../output/world_scale/menu.png),
[scale 10](../output/world_scale/scale_10.png),
[scale 100](../output/world_scale/scale_100.png), test/build logs in
`output/world_scale/`. These are functional captures, not an isolated performance
benchmark. No new planet, atmosphere, fluids or simulation properties are added.
