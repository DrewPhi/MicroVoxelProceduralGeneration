# 1 km authoring region awaiting user inspection

User explicitly wants to inspect the unfitted world preview FIRST, then will authorize GPU implementation and Bouchet submission. Do not submit a cluster job or start that implementation as part of preview generation.

Authoritative definition: output/world-1km/world.json. Same original seed-42 3D FBM and parameters (no terrain/noise changes). X 5168..6168 m, Z 4272..5272 m; Y 168..592 m (424 m). Native voxel edge .0625m. Surface samples 270.5625..563.6875m: ~293m relief. Bottom chosen >96m below lowest sampled surface; top >24m above highest. Below region planned solid; above air. This boundary convention is metadata for future rendering, not an edit to the old viewer or a materialized floor.

World is recipe + spatial bounds, not a huge resident native voxel volume. Native dimensions XYZ [16000,6784,16000] = 1,736,704,000,000 possible cells. Keep sparse querying. Proposed patch edge8m/n5 gives XYZ [125,53,125] =828125 patches, 4,140,625 quadrics, expanded padded48B coefficient payload198,750,000B (~189.5MiB). Future fitter/viewer will need larger-scene handling (existing arbitrary quad count cap1m and device binding size limits), and must avoid a dense global constraint nullspace. This is planning context, not an assertion that global fixed-label constraints will retain tested fidelity.

src/bin/fbm_world.rs builds preview with the existing Terrain field. Terrain::density_with_base_height caches the exact same base height for repeated column queries; original density behavior preserved, tested. FastNoiseLite outputs bounded[-1,1], normalized FBM too; highest occupied native voxel is found by exact downward scan from base+overhang+2native rows. X/Z sampled columns are snapped native centers. Top-down samples 1024² columns (~0.977m spacing); no claim all native columns were enumerated or global extrema certified. Full3D caves/overhangs still defined by recipe.

Outputs:
- top-down.png: readable elevation tint/hillshade diagnostic; not biome/material definitions.
- overview.png: same image with title, range and100m scale bar.
- surface-heights.f32: 1024² little-endian native top-face heights, row+Z/column+X,4MiB.
- vertical-slice.png: exact occupancy samples at native centers on center Z4772m, region's Y range; diagnostic grid1024x512.
- recipe.json: original format source noise recipe. Its inspection span32m DOES NOT define the 1km fitting region; use world.json bounds instead.
- world.json: recipe parameters, bounds, grid/voxel metadata and authoring source hashes.

Render colors were deliberately changed only in the preview to show macro shape: 8m Gaussian smoothing for broad hillshade and subtle fine hillshade, height-based tint. tools/preview_world.py reproduces it; this does NOT smooth or change the density field. Built + generated in ~2.5sec; tests7authoring +2world passed. Both top-down and cross-section visually inspected. Default analytic model unchanged.

Reproduce: cargo run --release --bin fbm_world ; python3 tools/preview_world.py . Host rust env as other notes. GUI script fbm unchanged. No SSH or cluster action in this stage.
