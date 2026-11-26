# Seeded world playtest

## Eroded terrain

From `frontier/`, `./run.sh` starts a walk in a fresh Eroded world with
regional weather and native vegetation. To start airborne, run:

```sh
./run.sh --flight-view chase
```

Add `--flight-parked` for a runway start, or use `./run.sh --flight-walkup`
to start beside the parked aircraft. The launch prints the resolved seed.
Pass that integer with `--seed` to load the same world. Eroded has its
own world recipe and cache identity, with the standard 6.25 cm voxel grid.
An unspecified airborne heading is selected from the terrain visible at
the aircraft's starting height. The automatic altitude can rise above a
nearby ridge along that heading.

The source adds climate-shaped ridges and coastal relief, fractured rock
beds, flood terraces and nested drainage to the original height function.
River links choose a low path within a bounded corridor. Rainfall and local
grade drive smaller gullies. Low-gradient perennial reaches have a stronger
tendency to bend across their floodplains. Excavation cost limits the bends,
and the endpoints and downstream stages are shared across region boundaries.
This is a stable world-generation rule inspired by alluvial channel migration.
[Bend Theory of River Meanders](https://www.cambridge.org/core/journals/journal-of-fluid-mechanics/article/abs/bend-theory-of-river-meanders-part-1-linear-development/EAE7F1CD6803CC4F4F61148CB4FE96B3)

A 1.58 km prairie reach measured about 6 m of centerline departure from its
chord in the initial fit and 193 m after the migration pass, using 10 m
canonical samples. The reach and valley comparison images are under
`output/terrain-polish/river-after/`.

Rock exposure and incision inform the material
rules, alongside temperature, moisture and proximity to floodplains.
The analytical grade of the parent ridges exposes steep rock and moves the
snowline with aspect. The grade calculation reproduces the sampled height
values bit for bit.
Woodland placement follows this cover, with native trees nearby and shared
crown geometry farther out. Beyond individual trees, the display samples
irregular overlapping crowns from the habitat field and blends to their
mean coverage at large pixel footprints. That avoids the grid visible
with the first distant canopy filter. The bank benchmark put both filters
within 0.07 ms GPU p95 of each other. Results are under
`output/terrain-polish/canopy-screening/`.
Erosion and soils are procedural approximations of those processes.

Eroded also grows native mixed forests, understory and biome-specific ground
plants. The close renderer shows editable voxel branches and foliage. Filtered
crown views carry individual trees into the distance. See the procedural
vegetation implementation and comparison results. [Vegetation](VEGETATION.md)

The meshless terrain pass uses a 1024-cell middle cache at 3 m and a 16 m
outer cache. Its hierarchy bounds the sampled display surface, while integer
columns and NAADF provide exact geometry and edits in their resident regions.
The terrain, plants and NAADF share a GPU horizon field for direct sunlight and
diffuse sky exposure. Fair-weather clouds use seeded moisture fields to set
the base and lifting depth of each column. Billows and wind shear shape the
tops. The sky and ground shadows sample the same world-space density texture.
Simulated weather supplies condensate and rainwater when enabled.

The renderer first draws nearby voxels and vegetation. It reduces their
depth to a conservative farthest depth per 4 × 4 tile, taking the farthest
sample under MSAA. Terrain rays can stop past that bound. Visible rays use
their usual intersections and all ownership tests remain in effect. Use
`--terrain-depth-occlusion off` to measure the earlier draw path. The related
research is [Hierarchical Z-Buffer Visibility](https://doi.org/10.1145/166117.166147).

### Compare quality and speed

The test camera list, raw reports and actual captures are under
`output/terrain-polish/`. The middle cache, near refinement and outer
refinement are independently selectable.

```sh
python3 tools/terrain_polish_check.py \
  --views output/terrain-polish/views.json \
  --before output/terrain-polish/before \
  --output output/terrain-polish/replay
```

- `--terrain-mid-cells 512 --terrain-mid-strips off` selects the smaller 3 m window for comparison.
- `--terrain-far-detail on` increases the outer density to 8 m at a 6 km horizon. It costs about 40 MiB more terrain cache and increases cold generation time.
- `--terrain-detail on --terrain-near-spacing 0.75 --terrain-near-cells 1024` enables the larger fine near band.
- `--edge-aa 0.7` enables optional post-composite edge filtering. Its default strength is zero.
- `--terrain-gray` displays the actual relief with a common material. In the image harness, gray captures also disable vegetation and atmosphere.

The native comparison traces 6.25 cm columns independently over cropped
regions. First-hit distance errors use rays whose two hits lie in the native
region interior. Single-sample native color retains the true voxel-face
pattern. Shaded renderer screenshots include the footprint-filtered result.

| Native crop | Median hit distance | 512-cell middle cache p95 error | 1024-cell middle cache p95 error |
| --- | ---: | ---: | ---: |
| Seed 19 mountainside | 875 m | 2.81 m | 0.64 m |
| Seed 70231 riverbank | 757 m | 0.99 m | 0.74 m |

Errors are absolute distance along the camera ray. On the mountainside,
the 1024-cell cache also reduced missing hits from 51 to 26 out of 78,091
native terrain pixels in the crop. Beyond the middle band, the 8 m outer
cache reduced p95 ray-distance error from 9.93 to 8.16 m on the 4.4 km crest
crop. Its cold outer build took about 1.7 s, versus roughly 0.6 s at 16 m.
The optional 0.75 m band reduced p95 from 0.75 to 0.71 m on the native
shoulder crop at 314 m. The standard preset uses the wider 3 m middle cache.

Order-swapped 1080p walking runs on the Radeon 760M with vegetation and
NAADF measured GPU p95 of 18.94–19.97 ms with foreground-depth reuse
disabled and 14.88–15.34 ms with it enabled. Both MSAA modes produced
identical fixed-camera PNGs with NAADF and matched plant rendering. Dense
vegetation can settle to different plant detail across separate captures.
The final parity checks include foliage-disabled pairs and pairs with a
smaller plant working set. The standard Legacy 300 m check produced
identical color, coverage and depth buffers. Raw results are under
`output/terrain-polish/final-occlusion/` and
`output/terrain-polish/release-parity-fixed/`.
The fair-weather cloud refinement measured 9.38–9.43 ms GPU p95 on the
wooded bank with 0.32 and 0.48 coverage, compared with 9.32–9.44 ms for the
earlier cloud shapes. Each configuration was measured twice at each
coverage, after 30 warmup frames.

The full preset comparison uses two order-swapped runs per route on the Ryzen
5 7640U and Radeon 760M via RADV Vulkan. Each run used 1920 × 1080, a 60°
vertical FOV, one sample and a 6 km horizon. The report contains 470 presented
frames per run with 24 frames of warmup. The frozen Landforms capture and the
Eroded build use their own canonical terrain, materials and vegetation.

| Route | Landforms GPU p95 | Eroded GPU p95 | Landforms frame p95 | Eroded frame p95 |
| --- | ---: | ---: | ---: | ---: |
| Alpine flight, terrain and atmosphere | 11.26–11.92 ms | 12.46–13.24 ms | 17.44–17.45 ms | 17.28–17.36 ms |
| Woodland bank, vegetation and regional weather | 9.26–9.30 ms | 9.17–9.24 ms | 17.19–17.20 ms | 17.14–17.23 ms |
| Woodland walk, vegetation and NAADF | 12.91–13.09 ms | 14.10–14.89 ms | 17.07–17.10 ms | 17.58–17.93 ms |

The terrain allocation in those runs was 34.3–34.8 MiB for Eroded and
22.3 MiB for Landforms. The matched walk locked the same start and heading
in both presets. Eroded selected 2,092–2,367 vegetation instances compared
with 335 in Landforms. Both recorded zero unresolved vegetation coverage
at the end. Walk frame p99 reached 27.61 ms while vegetation was populating.
The benchmark summaries, per-frame observations, and exact commands are in
`output/terrain-polish/release-walk-timing/` and `output/terrain-polish/final-timing/`.
The latter also includes the earlier walking runs with each preset's
chosen heading.

The combined default with revised regional clouds completed two cold 1080p
routes on the Radeon 760M at 1× sampling. Each route used 509 measured
frames and the interactive 1.5-million-triangle vegetation budget. The storm
route resumed a regional checkpoint and ended in 24.4 mm/h local rain.

| Route | Full GPU p50 / p95 / p99 (ms) | Complete frame p50 / p95 / p99 (ms) |
| --- | ---: | ---: |
| Dense forest walk and turn, seed 6401777571733596131 | 17.40 / 18.64 / 19.69 | 18.50 / 23.64 / 31.13 |
| Low bank through rain, seed 3905432349249063754 | 17.75 / 21.03 / 24.80 | 19.53 / 24.66 / 40.54 |

Both stress routes exceeded the 16.67 ms frame budget. Two fresh launches
resolved distinct seeds, and a walk-up launch produced a 520 × 24 m dirt
runway. Local reports and captures are under `output/main-default/`.

Indexing every vegetation shadow record and skipping the shadow metadata
for color-only draws were tested in separate 540-frame A/B runs. Both
raised CPU preparation and frame p99. Fixed-camera captures matched byte
for byte and the original packing path was retained. Reports are in
`output/terrain-polish/vegetation-index-check/` and
`output/terrain-polish/shadow-prefix-check/`.

On three camera/seed pairs, increasing cold-cache generation from four to
up to six workers reduced the combined outer and middle readiness time
from 1.05–1.13 s to 0.82–0.90 s. Peak process memory in those isolated runs
rose by about 2 MiB. Recentered caches use four workers and reuse their
interior samples. Raw runs are in `output/terrain-polish/cold-timing/`.

The drainage design draws on spatial terrain profiles and finite runoff
connections. The cloud design uses a broad density envelope with
smaller-scale shape and lighting. Frontier uses its own implementations.
[Terrain Generation Using Procedural Models Based on Hydrology](https://doi.org/10.1145/2461912.2461996) ·
[Real-time Terrain Enhancement with Controlled Procedural Patterns](https://doi.org/10.1111/cgf.14992) ·
[Nubis Cloudscapes](https://www.guerrilla-games.com/read/nubis-authoring-real-time-volumetric-cloudscapes-with-the-decima-engine)

## Landscapes flight and walking preset

From `frontier/`, launch the new recipe at a randomly selected seed.

```sh
./run.sh --preset landscapes --seed random --flight-view chase --weather regional --msaa 1
```

Add `--flight-parked` for a ground start. Omit `--flight-view chase` to walk,
or add `--vegetation off` to inspect the landforms before cover. Earlier recipes
retain their world identities and remain available.

Landscapes layers branching ridges and cuts onto the Watershed source. Dry
provinces develop rock benches and ephemeral washes while lowlands retain
wider, quieter relief.
The shared runoff graph and the near and distant renderers use these same
canonical columns. An expanded descriptor halo carries river shoulders across
region lines. The affine bound treats any descriptor missing from an
intersected region as optional.

The rolling cache uses a 16 m outer grid for the preset's 6 km clip and a 22 m
grid for the extended view up to 9 km. Continuous normals remove lighting seams
between outer grid cells. World-space bedding and vegetation cover filter by
footprint. Near foliage uses native plant generation and the distant shade
summarizes the same grove probability. The latter carries forest colour across
kilometres. Geometry at that distance is a sampled heightfield with
subpixel water coverage. A narrow channel can still fall between height samples.
The native column and NAADF rendering paths still own exact voxel faces.
Subpixel surface lighting blends toward the underlying slope in unedited
terrain. This reduces the hard shading change where the moving exact cache
meets the sampled terrain. Ordinary fast flight can use the sampled surface
for a frame while the exact-column worker catches up.

The atmosphere projects soft shadows from the cloud field using sun direction.
It uses the sky's world-space local and regional cloud volumes during weather
simulation. Coarse altitude samples approximate cloud density for surface
lighting. Terrain haze follows scene depth and uses the same horizon gradient
as the sky.

### Repeat the comparison

Preserve a release binary before changing the recipe or renderer. Build the new
release binary and run this from `frontier/` on the target machine.

```sh
python3 tools/terrain_flight_check.py --baseline /path/to/frontier-before --motion
```

The tool captures the same views across alpine, desert, plains, coast, and a
held-out seed. It repeats the views with and without vegetation and records the
adapter, GPU and presentation percentiles, process memory, and cache
publication metrics. Motion covers approach, bank, and stop-and-resume walking.
Capture and benchmarking overhead are tracked separately from the GPU span.

### First laptop measurements

These are 1920×1080, MSAA 1, 6 km, 60° vertical FOV runs on the Ryzen 5
7640U / Radeon 760M with Mesa RADV. The comparison uses the previous
SeededWorld recipe at the same camera and lighting controls. GPU timestamps
span the rendered frame from shadows through composition and UI. Displayed
frame intervals include preparation, surface acquisition and presentation.

| View | GPU p95 (ms) | Frame p95 (ms) |
| --- | ---: | ---: |
| Five stationary Landscapes views with vegetation | 8.8–9.2 | 17.4–17.7 |
| 90 m/s mountain approach, previous SeededWorld | 9.5 | 17.5 |
| 90 m/s mountain approach, Landscapes | 9.0 | 17.4 |
| 72 m/s coastal bank, regional weather | 9.1 | 17.5 |
| Walking with vegetation | 13.4 | 17.6 |
| Held-out ridge, dawn / dense overcast | 8.9 / 8.8 | 17.4 / 17.3 |

An earlier frozen-baseline run took 54.0 ms at frame p95 during the same
approach and spent that time in blocking preparation. Baseline runs varied,
so the table uses the final matched run and records the stall separately.
The final approach recorded 464 GPU samples, 17.9 ms frame p99, and zero
exact-cache fallback updates. An earlier updated run used the fallback on
two updates. The sampled windows took roughly 0.4–0.6 s combined on cold
creation; steady flight reused existing samples. The terrain cache allocates
about 22.3 MiB on the GPU before NAADF, vegetation and atmospheric resources.

The approach footage runs from 505 m to a close landing view. Its near
segment advances more slowly to show the exact-column and NAADF handoffs
separately. The fixed-camera filter A/B keeps the central depth identical.
The last native segment reaches an independently verified voxel hit.
The five-seed gallery, ownership probes and footage are under
`output/terrain-checklist/final/`. The final motion and lighting timings are
under `output/terrain-checklist/measured/`. The fixed player-height Legacy replay in
`output/terrain-checklist/primary/` matched the frozen executable's PNG,
linear color, depth and coverage byte for byte.

## Watershed exploration preset

Watershed is a separate world recipe. It pairs a canonical voxel grid and
static water with stepped distance LODs. To start
above a new world, run the following from `frontier/`.

```sh
./run.sh --preset watershed --seed random --camera aerial --walk off --msaa 1
```

Use the printed integer seed to revisit the same geography. Omit the camera
and walk flags to start on the ground. `F` switches between walking and free
flight. Watershed free flight moves at 35 m/s, or 80 m/s with Shift.
`FRONTIER_DISTANT_TERRAIN=surface` selects smooth distance LODs for an A/B
comparison. The `seeded-world` preset uses its original
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

Woodland, wetland, and steppe extend the climate classes. Steppe carries
grass with bare soil in the driest patches. Low ground close to inland
channels supports grass even in dry climates, and supported rowan trees
favor woodland and wet grass banks. Appearance version 2 refreshes cached
ground cover. The preset uses static inland
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

### Choose a voxel size

`--voxel-size METERS` sets the edge length of a native cube for the whole
launch. The default is 0.0625 m. The size can be a decimal and can be smaller
or larger than the default.

```sh
./run.sh --preset watershed --seed 997 --voxel-size 0.03 --naadf-radius-voxels 800
./run.sh --preset watershed --seed 997 --voxel-size 1 --naadf-radius-voxels 1024
```

World coordinates, fBm scales, camera dimensions and view distance are in
meters. `--naadf-radius-voxels` sets the exact cube's half-extent in native
voxels. NAADF rounds up to a whole 16-voxel root. The default 1,024-voxel
radius spans 64 m at 6.25 cm and 1,024 m at 1 m. The address limit is 4,096
voxels per side of the center, subject to the native build and GPU memory
budgets. Existing commands can still use `--naadf-radius METERS`.

Terrain is quantized on the selected grid. On grids of 25 cm and larger,
grass and flowers are decorative 6.25 cm voxel meshes placed on exposed
grass blocks. Six shared templates are batched and culled through the plant
renderer, with distance limits of 28 m for grass and 56 m for flowers.
The native grid still determines tree geometry. Use the same size when
reopening a water save. Saves written before this setting use 0.0625 m.
The derived terrain cache includes the grid size. The offline `landscape`
reference accepts the same flag.

### Grass cover and stepped LODs

In a 193×193 canonical survey of the 10 km square around seed 19's origin,
grass rose from 7.0% to 33.7%. Bare soil fell from 35.4% to 11.5%, and sand
still covered 36.2%. Seed 997 went from 31.7% to 37.6% grass. Ground and water
heights matched at all sampled coordinates across seeds 19, 997 and 40961.

The static capture set is in `output/watershed-greening/`. The before and
after pairs use the same camera, seed, sun and stepped renderer, so they show
the cover change. The 1920×1080 shots use one sample, a 6 km clip, 64 m NAADF
and no streaming haze. The desert-bank comparison's frame p95 was 26.5 ms
before and 27.0 ms after in separate 17-second runs. These still captures
record full coverage and unresolved detail. The fixed Legacy reference color,
depth and coverage matched byte for byte, and 201 world tests passed.

### Initial flight baseline with smooth distance LODs

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
FRONTIER_DISTANT_TERRAIN=surface ./run.sh --preset watershed --seed 997 --walk off --width 1920 --height 1080 \
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

## Landforms geometry study

`--preset landforms` adds canonical joint shelves and talus to the Landscapes
recipe. The changes feed voxel occupancy and the rolling heightfield through
the same terrain source. In a surveyed 200 m mountain patch at seed 19, the
new recipe changed 52,019 of 63,001 sampled column heights. The median
absolute height change was 0.125 m and the largest was 1.4375 m.

`--terrain-detail on` enables the experimental larger exact-column cache and
a sampled layer spaced four native cells apart (0.25 m at the default voxel
size). The camera footprint and resident coverage govern the exact search
range and the blend into the 1.5 m parent samples. NAADF continues to handle
nearby volume. The sampled layer smooths and loses some subcell detail, and
its maxima bound the interpolated surface. The diagnostic mode
`--terrain-gray` uses a neutral pigment for color-independent comparison.

`--formation-site X,Z` builds an optional seeded test site with a voxel arch,
grotto, and overhang. These use real air and solid cells for picking and
collision. An exact local mesh provides render coverage at all distances at
the site and casts shadows onto the surrounding ground. The site is
explicitly placed, and the world generator still needs biome-aware formation
placement, more varied shape profiles, and underground networks.

From the Frontier directory, after a release build of `frontier` and `landscape`:

```sh
python3 tools/terrain_geometry_study.py close
python3 tools/terrain_geometry_study.py riverbank
python3 tools/terrain_geometry_study.py source
python3 tools/terrain_geometry_study.py gallery
python3 tools/terrain_geometry_study.py timing --output output/geometry-study/timing-final
./run.sh --preset landforms --seed 83 --terrain-detail on --formation-site -4200,1800 \
  --eye -4186,23.3,1786 --target -4186,23.5,1810 --msaa 1
```

The close and riverbank harnesses compare the same Landforms world with the
projected detail mode on and off. The independent CPU reference casts four
native rays per pixel. Depth percentiles use shared, fully covered interior
pixels and exclude the far-plane margin. They measure geometry agreement
within the chosen crops. The riverbank also compares two shading paths, so
water appearance differs in the independent reference.

| Setup | Median absolute depth error, rolling | Projected detail |
| --- | ---: | ---: |
| Cliff, 20 m aim distance | 3.8 cm | 2.1 cm |
| Cliff, 60 m aim distance | 5.8 cm | 3.0 cm |
| Cliff, 150 m aim distance | 6.3 cm | 3.8 cm |
| Riverbank | 3.0 cm | 1.1 cm |

On the Radeon 760M (RADV Vulkan), the 1080p seed-19 route flew at 80 m/s
with a 6 km horizon and vegetation disabled. Two order-swapped runs per mode
measured GPU p95 of 12.18–12.59 ms with the standard cache and 21.49–21.53 ms
with projected detail. Frame interval p95 was 17.61–17.70 ms and 24.45–26.64 ms,
respectively. Each run recorded 295 frame intervals and 220–295 GPU
timestamps. Rolling GPU allocations rose from 22.3 to 41.6 MiB. The measured
GPU span includes atmosphere and UI. It excludes cache uploads, CPU
preparation, and presentation. The wider window continued to fall back to
the sampled surface during flight. Projected detail remains opt-in. Partial
strip publication in the exact-column cache and tighter grazing-ray bounds
are the performance work for this geometry path.

The local captures are under `output/geometry-study`. The canonical relief
sheet keeps the camera and renderer fixed while swapping the terrain source.
The cliff approach keeps the source and path fixed while swapping the
renderer mode. The arch walk uses the normal walking controller and takes
the player from Z 1786 to Z 1804.4 through the arch at Z 1800. The 1080p
legacy hybrid regression capture matches its pre-study color, depth, and
coverage buffers byte for byte. The Landforms preset also produced a
hardware capture with a one-meter voxel grid.

### Projected detail at the middle distance

`--terrain-mid-detail on` extends the 1.5 m rolling cache from 512 to
1024 cells per axis in Landscapes and Landforms. The same world-space
samples feed a smooth blend into the outer cache. The blend uses projected
sample size and resident coverage. Large height residuals hold fine relief
longer within the blend. The acceleration bounds include the overlapping
parent cells when a ray may intersect either surface.

The randomly selected spawn below exposes a hill roughly 340–400 m away.
The original renderer draws that hill from 16 m samples. The extended
cache captures changes along the crest and smaller variations on its face.
The sample spacings are still fixed in memory, and the 1.5 m level can
still omit detail visible at 1080p. The blend varies continuously between
available samples.

```sh
./run.sh --preset landforms --seed 12611535567061277817 --terrain-detail on \
  --terrain-mid-detail on --naadf off --vegetation on --walk on --msaa 1
```

On the Radeon 760M at 1080p with NAADF and vegetation disabled, the
stationary spawn comparison measured 12.78–12.81 ms GPU p95 for the
original cache and 12.82–12.86 ms for the extended cache. Rolling GPU
allocation rose from 41.6 to 51.6 MiB. Building and publishing the
first 1.5 m window took about 66 ms originally and 270 ms at the larger
size. The separate seed-19 flight at 80 m/s measured 20.85–20.97 ms GPU
p95 originally and 23.20–23.38 ms with the extended cache. Each test used
two order-reversed runs per mode and a 6 km horizon. The full-size
screenshots, aligned crop, and raw reports are in
`output/geometry-study/mid-projected`. The legacy visual regression
matched its pre-change color, depth, and coverage byte for byte.

### Terrain cache sampling and refinement study

The optional cache study keeps the canonical Landforms source fixed. It
compares the original cache with denser or wider world-aligned caches,
partial strip uploads, error-bounded coarse intersections, and selective
generation of the finest samples. The sampled layers still fade
continuously where their coverage overlaps.

- `--terrain-mid-spacing` changes the pitch of the middle cache with
  `--terrain-mid-detail on`. The 0.75 m and 1.5 m cases span the same
  physical window, making their direct comparison useful.
- `--terrain-mid-strips on` writes newly entered strips and boundary
  samples into a wrapping GPU texture. CPU staging still builds a
  logical window and its bounds. Coarser mips are rebuilt when their
  addressing is incompatible with a strip shift.
- `--terrain-mid-error` enables optional coarse first-hit queries.
  The hierarchy bounds the difference between two cached bilinear
  surfaces. A ray accepts a coarse hit only when the bounding surfaces
  bracket the first hit within the configured fraction of projected depth
  scale. Ambiguous hits descend. The bounds cover the reconstructed cache.
  Features between canonical samples require a separate bound.
- `--terrain-mid-tile-error` measures representative fine-grid
  residuals within fixed world tiles. Tiles with larger residuals get
  finer heights and mixed tile edges feather into their shared parent
  lattice. The thresholds are heuristic height errors in meters. The
  mode saves procedural queries while keeping a dense GPU texture.
- `--terrain-mid-boundary-ascent on` measures an alternative traversal
  that ascends at parent cell boundaries. It was slower in the initial
  flight screen.

The gray geometry reference covers a bounded 256 m patch, with four
native rays per pixel. The reported depth percentiles include only
matched, fully covered interior samples within the hill's depth range.
GPU depth encoding quantizes long-distance measurements. The side-by-side
images also show silhouette differences which the interior statistic omits.

```sh
study=output/geometry-study/footprint-cache
mkdir -p "$study"
target/release/landscape --terrain-recipe landforms --seed 12611535567061277817 \
  --gray --spp 4 --sample-stride 1 --threads 6 --extent 256 --center 1580,-1395 \
  --far 6000 --output "$study/native-hill" --width 1920 --height 1080 --fov-y 60 \
  --crop 0,370,444,154 --eye 1536,189.7625,-1024 \
  --target 1536.7069546580315,189.78324158005415,-1024.7069546580315
python3 tools/terrain_geometry_study.py cache-images --output "$study"
python3 tools/terrain_geometry_study.py cache-fly --output "$study" --repetitions 2
python3 tools/terrain_geometry_study.py cache-walk --output "$study" --repetitions 2
```

The table uses the same 1080p, 6 km, seed-19 flight on the Radeon 760M
at 80 m/s with vegetation and NAADF disabled. Geometry errors are from
the independent seed-12611535567061277817 hill crop and use the fully
covered matched pixels for each configuration.

| Cache setup | GPU p95 in flight | Complete frame p95 | Rolling GPU allocation | Hill depth error median / p95 |
| --- | ---: | ---: | ---: | ---: |
| Baseline rolling cache | 11.59–11.97 ms | 17.54–17.68 ms | 22.3 MiB | 0.66 / 4.08 m |
| 3 m middle, standard exact columns nearby | 12.42–12.46 ms | 17.79–17.88 ms | 22.3 MiB | 0.26 / 1.17 m |
| 1.5 m middle, standard exact columns nearby | 15.09–15.14 ms | 17.92–18.13 ms | 32.3 MiB | 0.21 / 0.79 m |
| 1.5 m middle, expanded near cache | 24.43–24.70 ms | 27.26–27.44 ms | 51.6 MiB | 0.21 / 0.79 m |
| 0.75 m middle, expanded near cache and strip uploads, screening run | 28.16 ms | 31.29 ms | 91.6 MiB | 0.19 / 0.70 m |

The 3 m option uses a wider sample spacing to extend its reach at the
baseline allocation. The first middle window was ready in 72–75 ms
compared with about 62 ms for the baseline in the flight runs. The
1.5 m option took 250–265 ms. At the sampled Alpine ridge, the same
three modes measured median depth errors of 1.04 m (baseline), 0.98 m
(3 m), and 0.89 m (1.5 m) on matched reference pixels. Other near
terrain in the full GPU view can occlude the bounded reference crop.

The two standard-near options both held about 9 ms GPU p95 on the
grounded walk. The strongest 0.75 m selective-height setup reduced
initial procedural queries from 4.20 million to 2.85 million in the
screening test. It still allocated 91.6 MiB and took about 28 ms GPU
p95 in flight. The selective 1.5 m setup showed the same pattern.
The 30 cm threshold reduced initial queries from 1.05 million to
0.76 million and held flight GPU p95 at 15.15 ms while hill error p95
rose to 1.11 m. Strip uploads preserved the image and reduced the
screened 1.5 m moving upload maximum from 5.12 ms to 0.92 ms, with a
small cost from wrapped texture addressing. Both acceleration trials
spent more GPU time than the unmodified extended-cache traversal.

For a low-cost improvement at the tested spawn:

```sh
./run.sh --preset landforms --seed 12611535567061277817 --msaa 1 \
  --terrain-detail off --terrain-mid-detail on --terrain-mid-spacing 3 --naadf off
```

### Native patches from the chase cameras

The chase-camera study in `output/geometry-study/native-flight-reference`
compares the 3 m middle-cache setting with four independent 512 m
footprints of canonical 6.25 cm voxels. It uses the saved seed-70231 valley
and seed-19 Alpine cameras. The original gray renders include all scene
visibility, while the reference renders cover the finite footprints.
The common-lighting sheet derives normals from first-hit positions using
the same filter on the shared interior pixels. It illustrates relief;
the original sheets also retain silhouette and coverage information.

In the examined regions, the rolling cache softened the riverbanks at
roughly 760 m and replaced texture-like physical relief on the 845 m
Alpine face with broad facets. Nearly all of those hits had faded to the
outer samples. The exact 335 m shoulder kept its rounded form with
sharper small features. At the 4.4 km crest, the broad form in the
shared footprint agreed visually. The terrain source itself still
showed broad smooth regions around the existing features.

For terrain-only rolling captures, `--capture-terrain-distance` with
`--capture-linear`, `--naadf off`, and `--vegetation off` writes camera-ray
distance in meters to the three float readback channels at covered pixels.
This avoids the numerical quantization of the perspective depth buffer
at kilometers. Use the accompanying coverage file to mask misses.
The capture metadata identifies this encoding as
`rolling_ray_distance_metres`.
