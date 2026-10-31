# Player-scale valley implementation

## Inspection region and launch

The launcher starts the atlas at world scale 10 and now places the player at
source(975761.75,475600.52), looking upstream. The local refinement experiment
covers source X 960000..980000,Z 465000..485000: exactly 2×2 km in game at scale 10.
At other scales it remains the same physical landscape, compressed uniformly.
The full geographic atlas remains available around it. Rivers/painting/LOD
improvements apply throughout; the new erosion field is deliberately bounded
to this inspection region before adopting a whole-world refinement scheduler.

## Delivered changes

1. River geometry: global quadratic curves,17-sample compiled downhill profiles,
shared junction levels propagated through receiver graph, fixed lake levels,
bounded incision, source-space width>=10 m/depth>=3 m. At scale 10 these start at
1 m wide/0.3 m deep, independent of native voxel size. Width grows with discharge.
Per-query clipping remains a final bank safety guard. Arbitrary triangle-center
tributaries removed. Adjacent patches share channel geometry, profiles and the
160 m protection halo. Profile storage and indexes are bounded packed arrays.

2. Local refinement: immutable 513² field over the canonical valley. A bounded
smoothing pass reduces construction-edge slope changes; slope-dependent seed
relief evolves at 257² then 513² through multiple-direction runoff/incision,
material transport/deposition and steep-slope relaxation. Exact river/lake
constraints are applied both at compilation and query time. Outer boundaries
taper back to the atlas. This is our bounded CPU prototype, not a reproduction
of the full Schott 2024 method or a globally conserved catchment simulation.
It does not yet add another hierarchy of permanent water-bearing tributaries.

3. Efficient runtime: field is built before publication/collision use and cached
as a checksummed versioned ~2 MiB file under output/atlas-refinement. Camera/LOD
never reruns erosion. Geometry-only queries skip paint derivatives. A bounded
16384-column cache retains canonical columns and surface paint; voxel-size
changes get independent column caches but share immutable geographic data.
Local displacement summaries avoid a global error floor near the player.

4. Painting: coherent gravel/soil banks, damp cover, rock exposure and sediment
response use refined geometry. Material IDs derive from the same coverage.
Surface albedo no longer falls back to flat material colors merely because
coarse/fine classifications differ. Buried materials retain distinct colors.
Small physical ground relief is now visible at 6.25 cm voxels. Each native voxel
still has one color on all faces; directional lighting is separate.

5. LOD: centered river footprint queries apply at dry coarse nodes as well as wet
ones. Fractional water coverage contributes color without moving terrain to a
remote water level. Native occupancy stays exact. Still captures now report
actual finest resident support at 10 nearby positions, separate from requested
native radius and scene-wide screen-space error.

## Validation and caveats

River tests cover actual 3D face-connected wet intervals, actual sampled downstream
water monotonicity, shared triangle edges, confluences, lake levels and bank
support. The final curve pass checked 58 valley reaches,209,427 native wet steps,
and 57 meaningfully curved reaches; no dry-bank profile excess was measured.
Atlas arrays/indexes/profile payload is 190.9 MiB before refinement/render caches.

Native collision, immutable edits/reload, scale/grid agreement and query-order
checks remain required. Tests are reproducible with:

    cargo test --locked --release --workspace --manifest-path successor/Cargo.toml
    cargo test --locked --release --manifest-path successor/Cargo.toml -p microvoxel --lib generation::atlas -- --include-ignored --nocapture
    cargo test --locked --release --manifest-path successor/Cargo.toml -p microvoxel --test atlas_runtime -- --ignored --nocapture

Full atlas fixtures are local generated artifacts; ignored checks never download
them. Geometry/configuration identity is bumped to prevent old edits silently
reinterpreting a changed baseline. Raw geographic atlas binaries remain reusable.

Screenshots/logs and native support reports are in output/valley-pass. These use
software Vulkan in this session, so frame times cannot establish laptop GPU
performance. Inspect first-person captures after settling, plus the optional
higher-budget comparison. Exact 6.25 cm stairs remain inherent to native occupancy;
coarse facets, color transitions and river appearance need separate visual review.

## Research basis

- https://h-schott.github.io/p/mserosion/ and https://github.com/H-Schott/MultiScaleErosion
  motivate staged amplification at successive scales. Our bounded solver has
  different operators and constraints; paper timings do not apply to it.
- https://perso.liris.cnrs.fr/eric.galin/Articles/2013-river-networks.pdf
  motivates shared hydrological construction before local surface detail.
- https://github.com/redblobgames/mapgen4/blob/main/geometry.ts
  informed separation of river topology,width control and projected coverage.

No third-party source was copied into these implementations. Full catchment-scale
mass accounting, global adaptive refinement and live water physics remain outside
this bounded valley prototype.

## Final verification and visual review

The release workspace suite passed (220 tests; optional fixtures excluded).
The atlas suite including saved fixtures passed all 32 tests; both saved-atlas
runtime tests passed, including 3,600 walk/stop/resume/jump physics ticks with
no occupied-body intersections and scale/edit persistence checks.
`git diff --check` passed. The launcher was rebuilt from the final source.

The final first-person, rear and downstream captures use the v8 refinement
cache and verify native resident support at all ten nearby diagnostic points.
The exact 2×2 km overview uses center (97000, 47500) at world scale 10.
Inspection confirms curved connected rivers, clearer banks and more local
relief, but the visual target is not fully met: distant facets, broad gray
material regions and conspicuous river terraces remain. The higher-budget
comparison only improves visible P95 error from about 4.15 to 3.92 pixels;
planner memory-pressure fallback prevents the requested tile count from being
fully used. Increasing the slider alone does not resolve those artifacts.

The immutable field reloads in 2–3 ms; total atlas source load is about 2.9 s,
with 194.11 MiB reported payload. These are local measurements, not hardware
GPU frame-rate claims. See output/valley-pass/README.md and manifest.json for
capture settings and achieved detail.
