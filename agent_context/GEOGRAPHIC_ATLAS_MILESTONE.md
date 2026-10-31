# Seed-generated geography — step 1

## Scope

The headless landscape compiler now supports `--atlas`. This replaces the fixed
100 km mountain fixture with seed-generated geographic controls. The old fixture
remains the default for reproducible erosion comparisons. Production generator 6,
launcher, saves and gameplay have not been changed. Steps 2–5 (geography-aware
climate/hydrology, constrained refinement, runtime integration, promotion) remain.

## Model

`successor/world/src/generation/geographic_atlas.rs` generates physical-space
continental/shelf/basin elevation, sparse curved uplift systems, older uplands,
backland plateaus, adjacent forelands, and spatially coherent rock erodibility.

Systems are hashed from 340 km sectors with jittered centers, 170–520 km lengths,
variable direction/curvature/width and a 64% occurrence probability. The sectors
are only a bounded search index; fields blend across them. Not every sector has
a range. Curves taper at their ends and have compact cross-range support. A
650 km search halo covers the maximum system reach. Feature sampling is tested
for exact equality across overlapping export bounds. Coasts are broad, warped
crustal fields; there is no map-edge island falloff. All these are heuristic
geological controls, not plate mechanics or a reconstruction of Earth.

Uplift and erosion evolve together using the existing solver. Geological age
now follows each generated system rather than a fixed west/east coordinate.
Older systems lose most uplift early. Quiet interiors do not receive mountain
noise. One global sea level is 0 m; negative elevations represent bathymetry.
Only below-sea nodes connected to the domain boundary are ocean sinks. Enclosed
depressions remain spill-routed lakes. Ocean bed and coastline are fixed during
this pass; exported marine sediment is counted, not simulated as deltas.

The compiler exports `ocean.bin` (one u8/node). The preview draws ocean at sea
level and clips mixed shoreline triangles instead of painting the submerged
terrain blue. Existing ground painting is reused; no new continental climate
model or biome guarantees have been added.

## Consistency and limits

Geographic controls depend on seed and physical coordinates, not sample spacing.
The evolved terrain still depends on the chosen numerical domain and mesh.
Select and save one canonical atlas; smaller images must crop its saved data.
Do not claim that rerunning a 100 km solver reproduces a crop of a 1000 km solve.
No centimeter-scale quality or in-game streaming performance is established by
these kilometer-scale numerical previews. No automatic coast migration, marine
erosion, glaciation or live water/weather is present.

## References

- Cordonnier et al. 2016, *Large Scale Terrain Generation from Tectonic Uplift and
  Fluvial Erosion*: https://perso.liris.cnrs.fr/apeytavi/website/publication/hal-01262376/
  The paper uses authored uplift with coupled erosion; our seeded uplift fields
  are our own procedural extension, not a claim that the paper solves geography.
- Schott et al. 2023, *Large-Scale Terrain Authoring through Interactive Erosion
  Simulation*: https://github.com/H-Schott/StreamPowerErosion
  Existing stream-power reference; no code copied in this atlas pass.

## Reproduction

```sh
cargo build --manifest-path successor/Cargo.toml -p microvoxel --release \
  --bin landscape_lab --bin landscape_paint
successor/target/release/landscape_lab --atlas --extent-km 1000 --side 1025 \
  --seed 42 --output output/geographic-atlas/seed42
successor/target/release/landscape_paint output/geographic-atlas/seed42
MPLCONFIGDIR=/tmp/microvoxel-mpl python3 tools/render_landscape_lab.py \
  output/geographic-atlas/seed42 --painted --overview-only
```

## Validation

- Release library suite: 84 passed, 1 existing manual benchmark ignored.
- Eight seeds tested for finite controls, ocean/land, quiet land, uplift and
  seed variation; three seeds tested for export-independent control samples.
- Three evolved atlas tests verify acyclic downhill water routing, drainage-area
  conservation, fixed sea surface/bed and terrain-volume accounting.
- Additional 257²-node / 320-epoch solves: seeds 1 and 99 completed in 5.755 and
  6.153 seconds respectively, with zero uphill spill links. These coarse solves
  are validation samples, not resolution-converged equivalents of seed 42.

## Seed 42 result and inspection

- Literal 1000 × 1000 km atlas, 1025² irregular nodes, 976.5625 m nominal spacing,
  320 epochs. Compiler elapsed 192.137 s; `/usr/bin/time -v` peak RSS 325640 KiB
  (~318 MiB), excluding the separate Python image renderer. This exceeds the
  earlier 256 MiB target at this diagnostic resolution; runtime budgeting is
  still pending. This is not gameplay frame time.
- Heights -542.244 to 5094.121 m; 14.50% of nodes marine. Land height percentiles
  25/50/75/90/99: 126/253/361/762/2540 m. About 9.24% of land nodes have initial
  uplift above 0.4 mm/year. Quiet lowlands remain extensive.
- Zero uphill spill links; conservation residual 835.656 m³ against ~1.922e14 m³
  uplift (~4.35e-12 relative). Saved full arrays checked finite, sea surface
  exactly zero, all receiver water elevations non-increasing.
- Painting took 0.864 s (separate from compiler, loading and PNG rendering).
- Inspected the main colored PNG and lower-resolution seeds 1/99. Seed 42 now
  shows several separated curved ranges, older lower relief and broad plains,
  plus a coastal embayment. Other seeds move both ranges and coastlines. There
  is no single fixed mountain corner or dense repeated mountain-cell pattern.
- Limitations remain visible: coasts are broad/smooth, uplift arcs can read as
  deliberately shaped belts, and land colors remain predominantly temperate.
  No claim of complete geographic realism or worldwide biome diversity.
- Main PNG: `output/geographic-atlas/seed42/painted-isometric.png`.
  `painted-range-closeup.png` is a 100 km crop of the SAME saved atlas near its
  highest peak, with the same coarse geometry, not a finer independent solve.

Step 2 is now documented in `ATLAS_CLIMATE_MILESTONE.md`; the commands above retain the step-1 uniform-runoff comparison.
