# Procedural terrain screenshot sweep — 2026-10-29

Ten final views inspected individually, across seeds 42, 137, 909 and held-out
2027, with temperate, arid, tropical and continental climates. This is a targeted
visual audit, not a statistical survey or an independent realism benchmark.
The engine, painting, world defaults and real-data import feature were not changed.

## Evidence and reproduction

Run `python3 tools/capture_terrain_sweep.py` after building the native release.
The maintained scene list fixes all cameras, seeds and settings. Captures use the
mesh backend, 1280×720, 6.25 cm authoritative voxels, 65° vertical FOV, native
radius 6 m, 256 MiB terrain budget, 2,048 tile limit and haze 0.65. Ordinary current
presentation is enabled; HUD is hidden. Ground views use actual player placement.
Only requested detail changes inside each matched river/lake pair. All captures
wait for the stitched cover to settle. No real imports are involved.

- [Ten-view contact sheet](../output/terrain_sweep/contact_sheet.jpg)
- [River comparison](../output/terrain_sweep/river_comparison.png)
- [Lake comparison](../output/terrain_sweep/lake_comparison.png)
- [Complete commands/cameras](../output/terrain_sweep/manifest.json)
- [Requested versus achieved sampling](../output/terrain_sweep/capture_metrics.csv)

All large artifacts remain ignored under `output/terrain_sweep/`. Logs accompany
every PNG. The lower-detail comparisons were initially Q35, but those reached
nearly the same budget-limited sampling as Q95. They were recaptured at Q0 to
create a meaningful coarse comparison; the final manifest records Q0.

## What each view shows

| View | Seed / climate | Observations |
| --- | --- | --- |
| [01 Valley](../output/terrain_sweep/01_valley.png) | 42 / temperate | Strong mountain/plain/lake composition and readable distant ridge silhouettes. Broad green foreground is visually uniform; mountain relief often has similarly ribbed slopes. |
| [02 River, high](../output/terrain_sweep/02_river_high.png) | 137 / arid | The river branches connect visibly, but form elevated canal-like walls, angular straight reaches and abrupt junctions. These are visible at the higher detail setting. |
| [03 River, coarse](../output/terrain_sweep/03_river_low.png) | 137 / arid | Main blue channel survives, while narrower stretches become irregular, constricted or partly replaced by exposed bank/bed colors. Bank walls and terrain become more terraced. |
| [04 River bank](../output/terrain_sweep/04_river_bank.png) | 137 / arid | Player-height view confirms a tall, continuous wall with water along its crest. This is the clearest water/terrain integration failure in the sweep. |
| [05 Lake, high](../output/terrain_sweep/05_lake_high.png) | 42 / temperate | A coherent mountain basin and uninterrupted water surface. No obvious open sky cracks at this shoreline. Slopes enter water very abruptly; shore shelves and deposited margins are sparse. |
| [06 Lake, coarse](../output/terrain_sweep/06_lake_low.png) | 42 / temperate | Large water-body silhouette remains stable. Small shore details change, and blockier hill contours are visible. Large lakes are more robust than narrow river features in this pair. |
| [07 Humid hills](../output/terrain_sweep/07_tropical_hills.png) | 909 / tropical | Broad green coverage and coherent exposed-rock patches work. Pale river-bank corridors remain recognizable where their blue water becomes intermittent. Patch borders can look cut out; hills lack much intermediate-scale organization. |
| [08 Dry cliffs](../output/terrain_sweep/08_arid_cliffs.png) | 137 / arid | Solid, readable dry mountain silhouettes and restrained color. Close slopes are rounded and fairly uniform, with repeated-looking ribs/blobs instead of many distinct ledges, scree fans or structural rock forms. |
| [09 Continental mountains](../output/terrain_sweep/09_continental.png) | 2027 / continental | The held-out seed produces substantial connected relief rather than a broken outlier. Similar steep ribbed mountain vocabulary recurs; broad brown/gray geological divisions can dominate shape. |
| [10 Ground plain](../output/terrain_sweep/10_plain_ground.png) | 42 / temperate | Solid voxel colors and open, navigable-looking land work without blanket outlines. Dark contour-like bands and step-size changes are conspicuous from player height. This still does not prove whether every band is native stair shading, coarse height quantization or a join defect. |

## Highest-priority finding: rivers can sit above the surrounding ground

This is not solely a rendering problem. A canonical physical-field probe of seed
137/arid finds river bed **74.82 m**, water **76.01 m** at X/Z (1248, 3680).
The nearby dry bank at (1230, 3670), about 20.6 m away, is **71.17 m**. The water
therefore stands roughly **4.84 m above nearby dry terrain**. View 04 shows the
corresponding wall. Isolated natural levees are possible, but the long artificial
walls and angular network here do not read as a naturally incised river.

The source explains a plausible mechanism. In `generation/terrain.rs`,
`build_channels` derives levels from the 16 m routing grid. `terrain_surface` then
sets the center of a channel to its calculated bed, even when that bed is above
the smoothly sampled surrounding surface. Outside the channel it retains lower
ground via `base.min(target)`. A channel can consequently raise a narrow strip
while leaving lower land beside it. `Channel::sample` joins short raster-routing
segments; choosing one nearest segment can also expose abrupt changes at bends
or confluences. This diagnosis identifies concrete code paths; it is not a proof
that they explain every visible discontinuity.

**Next change:** derive one consistent river longitudinal profile, bed, bank and
surrounding valley from the same terrain scaffold. Incise a corridor with a
graduated cross-section; avoid unintentionally elevating the bed above adjoining
low ground. Fit longer connected reaches within the drainage corridor, keep
downstream levels coherent, and match junction elevations/widths. Preserve the
integer voxel grid at the final occupancy/render stage. None of this requires
live water physics.

## LOD loses water features and often reaches a budget ceiling

The two effects need separate treatment:

1. **Representation:** `render/terrain.rs` retains coarse water only when sampled
   wet coverage is at least 0.5. Sparse samples can miss a narrow channel; even
   sampled water can disappear below that threshold. Wet-weighted level averaging
   also does not distinguish separate water bodies. Feature-aware planner metadata
   cannot preserve a river if the coarse representation itself removes it.
2. **Budget:** river Q95 requested **1.04 px**, but achieved **9.66 px**; lake Q95
   achieved **12.08 px**. Both coarse captures reached their **16 px** targets.
   The other eight captures missed their finer requested targets under the fixed
   settings. The high river
   capture used only about 96 MiB of a 256 MiB terrain budget, so increasing memory
   alone would not address its planner/tile constraint. The reported pixel value
   describes cell sampling, not a certified silhouette or river-error bound.

**Next change:** carry explicit channel/lake identity and intersection/coverage
through coarse summaries. Refine the local river corridor when it still occupies
visible pixels, and filter coverage when it becomes subpixel. Avoid simply turning
every partly wet coarse cell into a full blue square, which would widen rivers.
Audit where the tile/balance/priority budgets go before increasing global detail.
Use matched water masks and bounded native-reference patches to measure continuity,
width and shoreline error; the current high captures are not native ground truth.

## Additional improvements, after river geometry and water LOD

- **Reduce artificial terraces while keeping voxels.** Examine view 10 with
  occupancy/LOD-step/lighting diagnostics. Test coarse horizontal sampling with
  finer vertical quantization on the native integer grid, instead of requiring
  every land-height step to equal the coarse horizontal cell edge. Check joins
  and face shading separately; do not erase intentional native voxel steps.
- **Give transitions physical structure.** Broad grass/rock patches are coherent,
  but their borders and narrow beige water corridors sometimes look painted on.
  Tie banks, soil thickness, exposed rock and deposited margins to the corrected
  cross-sections. Keep one solid base color per voxel.
- **Vary middle-scale landforms.** Preserve flat plains, but add occasional swales,
  terraces and deposited fans where terrain/flow justify them. Broaden mountain
  shapes through geology-dependent ledges, resistant ridges and footslope debris.
  Adding more tiny noise alone will not supply those structures.
- **Improve lake margins selectively.** The captured lake is already a strength.
  Shelves, inlets and depositional shore zones could connect it more naturally to
  the land. Its probed maximum-depth candidate is about 334 m; that depth alone
  does not make it invalid, and a screenshot cannot validate its water balance.

## What to preserve

- Large landform composition: mountains, lower hills, open plains and basins have
  distinct scales and recognizable silhouettes across the sampled worlds.
- Coherent large material regions and clearly different greener/drier appearances.
  Seed and climate both vary here, so these images do not isolate climate effects.
- Continuous lake surfaces and useful broad shoreline retention at coarse detail.
- Solid voxel colors without universal black borders. The grid can remain the
  foundation while river geometry and coarse proxies improve.

Recommended order: **river embedding/profile → water-preserving coarse summaries
and priorities → contour/join diagnosis → bank/material transitions → broader
middle-scale terrain variety**. Keep weather, waves, live water dynamics and
vegetation outside this pass. Static screenshots cannot evaluate temporal popping,
streaming while walking, or demonstrate a universal absence of seams.
