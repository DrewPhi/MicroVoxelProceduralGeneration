# Seeded world playtest

## Watershed exploration preset

Watershed is a separate world recipe. It pairs 6.25 cm canonical voxels and
static water with the smooth surface renderer for distant terrain. To start
above a new world, run the following from `frontier/`.

```sh
./run.sh --preset watershed --seed random --camera aerial --walk off --msaa 1
```

Use the printed integer seed to revisit the same geography. Omit the camera
and walk flags to start on the ground. `F` switches between walking and free
flight. Watershed free flight moves at 35 m/s, or 80 m/s with Shift.
`FRONTIER_DISTANT_TERRAIN=voxy-seed` selects the stepped distance
renderer for an A/B comparison. The `seeded-world` preset uses its original
source and procedural river/save identities.

Watershed varies ridge profiles, highland texture, and foothill relief. Its
drainage graph uses jittered global nodes and accumulates a climate-derived
runoff estimate for eight upstream hops. Shared world-space endpoints and
their accumulation halo make reach descriptors independent of region load
order. The accumulated runoff varies channel width and depth. Deep incisions
get broad valley shoulders, and sink lakes can stretch toward their main
inlet. The water surface is bounded by local land and bed heights where the
graph's proposed grade crosses a low saddle. This also bounds shoreline and
water heights used by the renderer's source certificates.

Woodland, wetland, and steppe extend the climate classes. Supported rowan
trees favor woodland and wet grass banks. The preset uses static inland
water and a bounded contributing area. Continent-scale spill routing,
long-term discharge and hydraulic erosion remain open. Generation changes
have a separate source ID from `SeededWorld`. Renderer caches include the
recipe and distance representation.

For a source survey, build the `seeded_world_gallery` tool and run it with
`--recipe watershed`. The chart script described below accepts the generated
directory. Dense local surveys are needed to show narrow channels. Sparse
100 km samples can quantify broad coverage and miss individual tributaries.

The drainage-first approach draws on
[Génevaux et al., Terrain Generation Using Procedural Models Based on Hydrology](https://www.cs.purdue.edu/cgvlab/www/publications/Genevaux13ToG/).
Frontier uses its own finite, globally addressed graph and its existing
column, water, and affine-bound infrastructure.

### Flight baseline

The Radeon 760M / Ryzen 5 7640U flew these 12-second routes at 80 m/s, 1080p
MSAA1, with a 6 km view, 32 m NAADF radius, 5 px detail target, vegetation,
terrain shadows and no streaming haze. Disk cache was disabled. The water
artifact comparison used the same camera route and the surface path.

| Scene | Seed | Frame p50 / p95 / p99, ms | Coverage frames | Resident terrain clusters | Terrain triangles |
| --- | ---: | ---: | ---: | ---: | ---: |
| High country | 997 | 16.68 / 17.18 / 17.52 | 710/710 | 862 | 56,840 |
| Mountain valley | 19 | 16.66 / 17.25 / 17.93 | 710/710 | 490 | 34,840 |
| Woodland inlet | 40961 | 16.69 / 17.25 / 17.69 | 709/709 | 352 | 27,856 |
| Low river | 997 | 16.67 / 17.19 / 17.70 | 710/710 | 559 | 23,744 |

Final pending selection memory was zero in each run. Peak builder reservation
was about 85 MiB. Vsync was active. The full GPU span p95 was 10.5–12.4 ms.
Zero measured frames met the detail target. Narrow banks still show aliasing
and visible refinement at flight speed. The high-country capture exposed
oversized blue water columns in both distance renderers. The source survey
traced one to a 380 m difference between the proposed stage and the local
bed. The same flight after correcting the canonical water height removed the
large columns. A 16-sample offline valley comparison also shows the effect of
widening the valley shoulders before that fix.

Reproduce the high-country route from `frontier/` after a release build.
Use a fresh report and capture path for each run.

```sh
./run.sh --preset watershed --seed 997 --walk off --width 1920 --height 1080 \
  --msaa 1 --capture-far 6000 --naadf-radius 32 --pixel-error 5 \
  --streaming-haze 0 --cloud-speed 0 --benchmark-input-lock --warmup 10 \
  --no-adaptive-disk-cache --route fly --route-speed 80 --route-clock realtime \
  --seconds 12 --eye -3610,1420,707 --target -2050,800,3100 \
  --report output/high-country.json --capture-on-exit output/high-country.png
```

The mountain valley uses eye `-1050,1280,-550`, target `1300,700,1200`.
The woodland inlet uses eye `-3300,250,800`, target `-900,30,3300`.
The low river uses eye `-800,420,-350`, target `100,175,1000`.
The latest captures, source surveys and footage are in the ignored
`output/watershed-exploration/` directory. The source-v8 regression survey
matched all 66,049 sampled rows, and the fixed Legacy reference matched
the previous PNG, float color, depth and coverage byte for byte.

Five final 100 km surveys of seeds 7, 19, 83, 997 and 40961 each contained all
nine biome classes. Sampling 193×193 sites for canonical columns and climate
took 326–332 ms per survey. Positive water stages in those samples had depths
of at most 7 m. The bounded per-thread drainage cache retains up to 20 regions
for Watershed; this lets the wide survey reuse regions across scanlines. The
world suite passed 200 tests, including the exact low-saddle regression and
held-out bank and region-edge certificate checks.

The separately recorded keyboard-and-mouse flight turned 73° and moved 361 m
from its start. Its frame trace shows the ordinary and Shift-boost speed bands.
Recording timings are separate from the benchmark table.

## Original seeded-world preset

This is the first implementation checkpoint for [issue #87](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/87). The original design brief and acceptance criteria are in [agent_context/seeded_world_issue.md](agent_context/seeded_world_issue.md). The issue remains open for its broader acceptance gates.

From this directory:

```sh
./run.sh --preset seeded-world --seed 19
./run.sh --preset seeded-world --seed random
```

The resolved random seed prints to the terminal. Reuse that integer to reproduce the world. The new preset disables vegetation, enables walking, and retains the NAADF/adaptive renderer and its quality controls. It starts with a 64 m native radius, 6 km view distance and requested 5 px geometric error; these are adjustable preview settings, not a world boundary. **F** toggles flight; **Esc** opens settings. Existing default, Alpine and Legacy launches are preserved.

There is no prebuilt square to walk off: generation uses global coordinates and the existing moving terrain/NAADF streaming. A preview's extent controls only its picture. This is a procedurally addressed flat world, not a finite simulated planet. Integer storage is large; the viewer and conservative source proofs still have practical coordinate/precision limits. Source tests include queries a million meters from the origin, not a claim that every renderer path has been tested across all i64 addresses.

## What generates the landscape

The source uses seeded FBM for continental elevation, broad mountain provinces, ridged ranges, foothills and fine relief. Mountains fade into lowlands through a continuous province field. `--scale` changes horizontal wavelengths (default 1800); `--amplitude` changes relief (default 1000). These are world-generation parameters, not voxel or LOD settings. Native cells remain 6.25 cm.

A globally anchored 512 m climate lattice caches heat/moisture and a bounded upwind rain-shadow estimate. Altitude cools the surface. Those continuous fields drive diagnostic Ocean, Coast, Desert, Plains, Tundra and Mountains classes. Mountains are landforms as well as a diagnostic class. Sand, grass, soil, stone and snow follow those conditions; coherent moisture-belt patches soften the sand/grass transition. No vegetation is spawned in this preset.

Hydrology uses globally addressed 2.4 km drainage nodes, downhill local D8 routing, shared stages, bounded FBM-meandering polyline reaches and level sink lakes. Coasts/oceans fill to sea level. Channels carve the bed and protect its central water corridor from fine noise. Cache boundaries do not define rivers, and graph construction does not depend on neighboring chunks being loaded.

This is **bounded local drainage**, not continent-wide watershed accumulation or hydraulic erosion. Rivers have a currently fixed width scale and schematic runoff, and some terminate in closed lakes. Terrain below sea level may form inland seas without proving ocean connectivity. Long-term discharge, realistic catchment size, channel width/depth tuning and erosion are future work. The geometry/materials now have a coherent place to add those improvements.

## Canonical water and rendering

`TerrainColumn` stores separate exclusive ground and visible tops. Water occupies actual native cells between them; the seabed remains soil/sand/rock. CPU queries, near voxel extraction, the NAADF hierarchy, distant profiles and the offline oracle share this definition. Uniform water chunks use a direct uniform-node proof instead of assembling 4,096 identical cells.

By default, water is opaque, static and nonblocking. Enable the experimental [live-water mode](LIVE_WATER.md) explicitly for local conserved flow and saved fluid state. Swimming, waves, refraction and a transparent underwater view remain unimplemented. Walk on land or fly over water; entering opaque water is not a finished gameplay experience. Generated water supplies an initial condition, not a perpetual source.

Detailed appearance maps now retain separate ground and water run boundaries, including native side-face integrals across exposed bed, soil, rock and water. The seeded source can use the selected Map appearance path, with point/native/footprint filtering sharing the same canonical run definition. The older Hierarchy and Visibility experiments still fall back to the run-aware Baseline path for this source. Map filtering remains bounded quadrature; it does not certify water-area coverage or preserve every narrow waterway between probes. Distant selection now carries a separate river/lake display-spacing debt from conservative descriptor core supports and minimum endpoint widths. Resolvable features request geometry spacing at most half the local minimum radius; subpixel supports may remain coarse. This debt participates in both streaming selection and current-draw quality assessment, without replacing the vertical geometry certificate. Filtered water-area coverage and ocean shoreline preservation remain separate quality gates.

Local terrain bounds use outward-rounded affine/interval evaluation of this source, including channel/lake influence. Definitely wet lake regions bound the visible water surface rather than the hidden bed. Unsupported legacy omitted-band experiments reject this source explicitly. Derived cache representation version 14 invalidates incompatible older payloads.

## Inspection and reproduction

```sh
cargo run --release --locked -p frontier-tools --bin seeded_world_gallery -- --output output/seeded-world/cpu
python3 tools/seeded_world_gallery.py output/seeded-world/cpu
```

The gallery exports canonical ground/visible heights, climate, material colors and biome labels for seeds 7, 19 and 83 at 10 km and 100 km spans. These are sparse CPU sample maps and true-scale isometric surface plots, **not full-native voxel screenshots**. Narrow waterways can fall between samples. The Rust tool accepts repeated `--seed` and `--span`, plus `--resolution` and `--center`; accompanying metadata records the actual source/settings and query timing. Query timings are not game FPS.

The source files are [world/src/seeded_world.rs](world/src/seeded_world.rs), [world/src/terrain.rs](world/src/terrain.rs) and [world/src/affine_bounds.rs](world/src/affine_bounds.rs). Source and appearance versions are separate. Tests cover negative/cache boundaries, source recreation, seed changes, distant coordinates, river/lake stages, wet coastal endpoints, material runs, codecs, edits and native bounds. The explicit GPU water test compares actual source water, exposed bed and air to canonical rays in compact and wide storage.

New screenshots, measurements and the Legacy regression comparison are kept under ignored `output/seeded-world/`. This checkpoint is for inspecting the new geography; it does not close the renderer's motion/quality gates or claim state-of-the-art terrain realism.

## Initial measurement

On the Ryzen 5 7640U / Radeon 760M laptop, a fresh 1920×1080 MSAA1 seed-19
walk-turn-return run used source 7, appearance 2, full lighting, no vegetation,
64 m NAADF, 6 km far clip and requested 5 px error. Native expansion reached
64 m in 4.88 s. The route began after settled reserve, then recorded 838 frames:
p50 **16.90 ms**, p95 **21.59 ms**, p99 **24.53 ms** (about 59 FPS by median).
Peak process RSS was 0.80 GiB, separate from GPU allocations.

Coverage remained ready for all measured frames and the camera stayed inside
NAADF. However, distant adaptive quality was ready for **zero** moving frames;
conservative error diagnostics still reported unresolved terrain. This does not
pass the combined image-quality/motion gate, and it is not a renderer speedup
comparison with the vegetation-filled Alpine scene. The 12 km / 3 px coast
capture also timed out before full quality and is retained as an explicit
unresolved diagnostic. Improving far feature preservation and the four-run
appearance representation remains phase B of #87.


## Inspected captures

The local `output/seeded-world/valley19.png` aerial and `walk19.png` end-of-walk frame show the current real-time result. The distant water has visibly coarse patches and the drainage still reads too straight; those are remaining quality work, not an approved visual target. `cpu/seeded_world_19_100000m.png` shows the 100 km source/biome survey. `native-reference/color.png` is an independent 640×360, four-sample CPU raycast of a 256 m native crop, clipped at 96 m, viewing a river bank. It verifies source appearance separately from adaptive geometry; its simpler regional lighting is not pixel-matched to the beauty renderer.

Rebuild that bounded native reference from this directory after a workspace release build:

```sh
mkdir -p output/seeded-world/native-reference
target/release/landscape --terrain-recipe seeded-world --seed 19 --center 0,0 --extent 256 --native-extent 256 --sample-stride 1 --eye '-20,501.2,20' --target '-55,493,0' --width 640 --height 360 --spp 4 --far 96 --threads 4 --lighting regional --sun-elevation 28 --sun-azimuth 22 --overcast .08 --output output/seeded-world/native-reference --cache output/seeded-world/native-reference/columns.bin
```


## Live-water and quality follow-up

The experimental [live voxel-water mode](LIVE_WATER.md) now adds conservative integer cell transfers and persistent terrain/fluid edits. Use its explicit launch flags to enable it. Source version 8 adds stronger bounded river bends and shared climate-derived width variation. It changes terrain identity; source-7 saves are rejected rather than silently interpreted against changed ground. Water-aware detailed appearance maps replace the prior seeded-only baseline fallback. Refer to the follow-up evidence for current results; the measurement above remains the original source-7 checkpoint, not a measurement of the new fluid system.
