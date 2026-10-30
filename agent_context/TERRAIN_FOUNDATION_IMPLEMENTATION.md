# Terrain foundation implementation

Implemented 2026-10-29, following all seven stages of
[the update plan](TERRAIN_FOUNDATION_UPDATE_PLAN.md). This is the first working
implementation, with measured limits below; it is not a claim of physically
accurate geology or interactive millimeter worlds.

## What changed

1. **Quality benchmarks.** `native/src/diagnostics/quality.rs` compares ordinary
   renders, geometry silhouettes and water masks at 6.25 cm versus 1 m display
   spacing. Three seeds, three climates, three terrain sites and two refinement
   recipes give 54 cases; seed 2027 supplies 18 held-out cases. Reports include
   silhouette discrepancy, foreground/water loss, top-down height RMSE, timings,
   memory and cross-grid field equality. A small public SRTM elevation crop in
   `assets/reference/` supplies descriptive elevation/slope comparisons.
2. **Physical scale.** `src/spec.rs` defines validated rational `GridScale` and
   `WorldSpec`. Canonical terrain fields use meters; occupancy, edits and logical
   movement use integer cells. LOD depth/extent, generation, painting, queries,
   collision, cameras, player size and saves now receive world scale. Physical
   page coordinates use 64 m regardless of native resolution; active render
   residency still follows adaptive tiles. GPU coordinates
   subtract integer camera origins before conversion to floats. Water heights
   and boundary references no longer depend on 16-bit native coordinates.
3. **Multiscale terrain.** `generation/geomorph.rs` adds nested drainage/slope-
   oriented ravines, rock-dependent relief, talus-shaped aprons and anisotropic
   fractures. Existing regional drainage, erosion and sediment remain upstream.
   Canonical physical samples remain identical when changing the grid; render
   sampling filters detail according to its footprint. `--terrain-method noise`
   isolates the alternative fine-refinement recipe, not an entire old release.
4. **Feature-aware LOD.** `generation/summary.rs` samples the final surface and
   tracks elevation range, residual error, orientation/appearance changes,
   channels, water, geological boundaries and sparse void features. Cached child
   summaries improve parents without generating every fine cell. Edits invalidate
   affected summaries. The planner has a bounded warm cache and avoids replacing
   every motion request before a completed plan can be applied.
5. **Coherent materials.** Physical-space multiscale coverage replaces independent
   per-cell material lotteries. Geological tones, strata and fine variation stay
   deterministic; coarse colors sample the same structured coverage. Each voxel
   retains one solid base color on every face. Material properties remain empty.
6. **Scaling and performance.** Exact constant/palette/raw color encoding reduces
   uploaded mesh appearance data without changing colors. Thin bank closures
   use constant-color rectangles instead of enormous unit-cell atlases. Exact
   native junction coordinates fix water-bank holes and triangulation failures.
   Player clearance uses occupied column intervals with differential tests against
   cell queries. Pose/column caches remain bounded. Nearby policy considers real
   screen pixels, player position, travel and tile budgets; loaded coverage follows
   that policy rather than demanding native detail at every altitude/resolution.
7. **Sparse 3D geology.** `generation/features.rs` indexes geology-conditioned cave
   voids and solid rock arches in spatial bins. Native queries, column runs,
   collision, edits and both renderers share the descriptors. Feature regions
   cannot become falsely certified solid terrain occluders. Most terrain retains
   the cheap height-column path.

## Reproduction

From the repository root, with Rust 1.85+ and working graphics drivers:

```sh
cargo test --locked --all-targets
cargo test --locked --manifest-path native/Cargo.toml
cargo build --locked --release --manifest-path native/Cargo.toml
./tools/check_foundation.sh output/foundation
cargo run --release --bin scale_probe
./tools/capture_cover.sh
./native/target/release/microvoxel-game --voxel-size 1/100
```

Run GPU benchmarks serially on an idle machine. GPU regression tests are ignored
by default; run the native test binary with `gpu::tests:: --ignored` on a working
adapter. `./tools/build.sh` also builds the dependency-free core/tools with rustc
1.75+, including `build/scale_probe`.

## Measurements and captures

Final verification passed 81 core tests and 17 native tests, including all four
adapter-dependent GPU regressions. Both crates pass formatting checks, and the
dependency-free tools build with the system Rust 1.75 compiler.

AMD Radeon 760M, Vulkan/RADV, release build. Movement checks cover walking,
sprinting, turning and live backend changes over 33.94 m / 600 ticks per case.
Some CPU tests overlapped early startup, so absolute startup timings are indicative.
Frame measurements are synchronized diagnostic draws, not a universal FPS promise.

| Backend / preset | Sampled nearby-detail misses | Frame p50 / p95 / p99 | Terrain payload |
| --- | --- | --- | --- |
| Mesh / 0 | 0 | 4.03 / 6.12 / 7.46 ms | 45.77 MiB |
| Ray / 0 | 0 | 13.87 / 20.22 / 21.72 ms | 82.99 MiB |
| Mesh / 1 | 0 | 4.20 / 6.44 / 8.12 ms | 47.88 MiB |
| Ray / 1 | 0 | 14.05 / 19.82 / 22.58 ms | 110.93 MiB |

Initial coverage took about 6.3–6.8 s; peak process RSS was approximately 407 MiB
across the sequential cases. Ray arena capacities were 96/128 MiB. Mesh remains
the default. These are different terrain/routes from earlier version-3 profiles.

The 54-case suite and 18 held-out cases all reported zero canonical height-field
discrepancy across grids. The new recipe's median mean silhouette discrepancy was
0.83 pixels (held-out 0.67); worst individual column discrepancy was 10 pixels
(held-out 4), comparing deliberately coarse 1 m patches with native patches.
Some cases lose roughly 10% of visible water pixels at that forced coarse spacing.
These tests do not establish a strict error bound or prove increased realism.

The bounded 0.5 m square scale probe queried 4 / 64 / 2,500 / 250,000 columns at
25 cm / 6.25 cm / 1 cm / 1 mm. Times were approximately 0.009 / 0.084 / 3.05 /
323 ms; the finest case retained only the configured 32,768 cached columns.
Regional arrays occupied 48,781,952 bytes and were shared between scales; that
figure is not total process memory. A million identical packed surface colors
compress from 4 MB to 20 bytes; constructing such data still costs CPU work.

Actual captures live under ignored `output/foundation/`: paired `valley_mesh.png`
and `valley_ray.png`, `arch.png`, player views, quality masks and CSV reports.
The refreshed tracked [cover](../assets/cover.png) is a 2560×1440 mesh capture:
3,400 tiles, achieved 9.31 px sampling, 181.72 MiB terrain, 21.39 ms averaged over
60 synchronized frames, 24.44 s initial readiness. The tile budget prevented its
requested maximum quality from reaching a one-pixel target.

## Compatibility and remaining limits

- Default native cells remain 1/16 m. Core scale accepts 1 mm–1 m rational edges;
  interactive character recipes currently support integer 4–128 cells/m. A 1 cm
  player passes actual walking/jumping/collision, but 301 diagnostic ticks/checks
  took 13.28 s CPU time. A 1 mm field probe is not a playable 1 mm character.
- Generator identity is version 4, material schema 2, save format 2. Grid and
  physical dimensions participate in compatibility. Format-1 headers can be read
  as default-grid metadata only when their generator/material baseline matches;
  old generator-3 edits are rejected rather than silently reinterpreted.
- Summary bounds are explicitly sampled with a residual allowance, not certified
  analytic extrema. Narrow features and silhouettes can still be lost under
  constrained budgets. A screen-pixel target is not a silhouette-error guarantee.
- Amplification is a fast procedural approximation. Talus is not mass-conserving,
  channel detail is not a global fine-grid hydrology solve, and millimeter fracture
  terms do not demonstrate realistic millimeter geology. Coverage proportions and
  coarse filtering are approximate. The 64-point DEM crop is a modest reference.
- Sparse caves/arches are localized analytic features, not connected global cave
  systems. Coarse feature appearance remains approximate. Shadows still use the
  regional surface and do not accurately shadow cave interiors or arches.
- Per-part draw submission and some per-cell construction remain. This pass chose
  measured exact color compression rather than speculative indirect drawing or a
  GPU material rewrite. Fine edits and exact character poses still have real costs.
  The fixed physical-page API does not yet replace tile-based job/residency
  ownership with a full page manager.
- Weather, water dynamics, waves, vegetation/life, general rigid bodies, player
  editing UI and production DAG storage remain deferred.

## Research used

[Terrain Amplification using Multi-Scale Erosion](https://h-schott.github.io/p/mserosion/)
and its [MIT source](https://github.com/H-Schott/MultiScaleErosion) informed the
coarse-to-fine direction; this implementation is not a port of its erosion solver.
[Real-time Terrain Enhancement with Controlled Procedural Patterns](https://onlinelibrary.wiley.com/doi/10.1111/cgf.14992)
informed slope/flow-guided bounded procedural refinement. Earlier visibility and
streaming references remain in [the LOD research](LOD_RESEARCH_AND_PLAN.md) and
[renderer implementation](RAY_RENDERER_AND_STREAMING.md). Reference dataset
provenance is recorded in [assets/reference](../assets/reference/README.md).
