# Guide for developers

## Build and run

The native app requires Cargo, Rust **1.85+**, and working graphics drivers.
Linux currently uses X11, including XWayland. Run commands from the repo root:

```sh
cargo run --locked --release --manifest-path native/Cargo.toml
```

For a double-click Linux launch, run `./tools/build_launcher.sh` once, then open
**`MicroVoxel`** in the repository root. It starts the current release build with
the correct working directory; diagnostics go to `output/launcher.log`. The script
also creates `MicroVoxel.desktop` with the project icon. Generated launchers stay
ignored; `tools/launcher.rs` and the build script are the maintained sources.

For reproducible photographs, use `--camera X,Y,Z,YAW,PITCH` (meters/degrees),
`--size 2560x1440` and `--no-hud`. Add `--camera-ground --view rear` to
ignore Y, settle the player at the supplied X/Z and photograph it at ground level.
Without `--camera-ground`, an explicit camera starts in debug flight. Positions
snap to whole voxel cells. These flags do not alter saved graphics
preferences or world data.

The core is a separate, dependency-free crate supporting Rust **1.75+**.
Both Cargo lockfiles are checked in. After building, the game executable is
`native/target/release/microvoxel-game`.

## Layout and where to make changes

| Path | Responsibility |
|---|---|
| `src/generation/landforms.rs`, `noise.rs` | Independent landform blending, geology and noise |
| `src/generation/climate.rs`, `environment.rs` | Steady climate sweep, environment types, presets and biome suitability |
| `src/generation/terrain.rs` | Climate-conditioned erosion, drainage, sediment and fine surface refinement |
| `src/generation/debug.rs` | Twelve environment maps, legends and regional statistics |
| `src/generation/mod.rs` | Canonical terrain adapter and column caching |
| `src/character/` | Procedural character recipe, discrete poses, local voxel volumes and bounded pose cache |
| `src/physics.rs`, `src/player.rs` | Exact voxel collision, camera sweeps and fixed-tick walking controller |
| `src/grid.rs` | Integer coordinates, 6.25 cm units and brick indexing |
| `src/materials.rs`, `src/painting.rs` | Material IDs, palettes and voxel colors |
| `src/world.rs`, `src/storage.rs` | Queries, atomic edits, revisions and compact overrides |
| `src/persistence.rs` | Versioned edit saves and baseline compatibility |
| `src/queries.rs` | Canonical raycasts and collision sweeps |
| `src/meshing.rs` | Bounded general 3D surface extraction |
| `src/render/` | Mesh surfaces, sparse ray bricks, LOD selection and boundary stitching |
| `src/simulation.rs` | Tick, active-region and stable-ID primitives |
| `src/bin/` | Noise images, environment PPM maps and world benchmark |
| `native/src/main.rs`, `camera.rs`, `player_view.rs` | Window/input, three player camera views and grid flight |
| `native/src/runtime.rs`, `streaming.rs`, `lod_planner.rs` | Near-first LOD patches, replaceable bounded build jobs and predictive planning |
| `native/src/settings.rs`, `menu.rs`, `menu.wgsl` | Esc settings, numeric sliders, presets, local preferences and frame-time statistics |
| `native/src/presentation.rs`, `presentation.wgsl`, `sky.wgsl` | Opaque LOD fades, optional edge antialiasing and directional sky |
| `native/src/diagnostics/lod_check.rs` | Same-camera quality captures, memory-shrink and paced streaming checks |
| `native/src/capture.rs` | Validated repeatable still-camera positions and export dimensions |
| `tools/launcher.rs`, `build_launcher.sh` | Native executable entry point and machine-local desktop shortcut |
| `native/src/gpu.rs`, `terrain.wgsl` | GPU buffers, drawing, lighting and selection |
| `native/src/environment.rs`, `maps.rs`, `environment.wgsl` | PNG map export and native diagnostic view |
| `native/src/overview.rs` | Full-region true-scale isometric PNG export |
| `native/src/diagnostics/character_preview.rs` | Standalone model/resolution PNGs and grid-aligned APNG animations |
| `native/src/diagnostics/` | Offline profiles, quality/movement checks and experimental ray representations |
| `tests/` | Core, persistence and rendering contracts |
| `assets/cover.png` | Actual native screenshot used by the README |
| `agent_context/` | Design notes, constraints and measurement history |

Keep terrain changes in generation, color changes in painting, geometry/LOD
changes in render, and presentation changes in the native renderer. New sky and
cloud passes can live alongside the native terrain pipeline.

## How the world works

**Canonical cells.** A voxel is 1/16 meter on a signed integer grid. Air differs
from unknown territory. Player and camera translation advance in whole cells with smooth view
rotation. Third-person camera booms retract against a swept volume. Every face of a voxel shares its base color;
lighting may darken faces. Water occupies complete cells.

**Storage and edits.** Untouched terrain is procedural. A stored voxel is four
bytes: material ID and appearance. Only overrides persist, in adaptive sparse,
dense or uniform 32³-cell bricks. `World::apply` validates transactions before
committing and reports local invalidation. Restoring the baseline removes an
override. Save/load checks generator and material compatibility.

**Rendering.** Vertical runs skip buried terrain; exposed coplanar faces merge
into rectangles with a separate, integer-indexed color atlas. Meshes stay in GPU
buffers. A balanced quadtree chooses power-of-two grid spacings, with explicit
coarse/fine connecting faces. Local balanced patches install only when their new
meshes and neighbor stitches are resident. A separate latest-intent planner handles
view-aware projected cell size and sampled terrain-curvature importance; mesh
workers perform generation, ambient visibility and conforming triangulation.
Greedy rectangles without T-junctions keep their two triangles. Boundary polygons
include shared finer-grid junctions, with all vertices on integer world cells.

Coarse columns use band-limited procedural samples and linear-light footprint
colors. From 32–128 m, summaries can combine cached children; larger footprints
use direct bounded quadrature to avoid generating fine data over the whole region.
Native columns, edited appearance and collision remain exact. These proxies are
sampled approximations, not exact reductions of all buried voxels.

Workers, queues, caches and uploads are bounded. The default terrain mesh-buffer
budget is 128 MiB, configurable in the menu. When lowering it, the old ceiling
remains temporarily available to load coarser replacements; inactive meshes are
retired until the new budget fits. The displayed count excludes color/depth
presentation targets, the small shadow map, UI buffer, actor cache and driver
allocations. A short LOD transition draws two complete views and composites them
with a screen-space dither; steady views draw one. No transparent terrain holes.

**Environment.** Generation blends plains, hills, plateaus, basins and mountains,
then uses altitude, seasonal temperature, prevailing wind, rain shadows, potential
evaporation and runoff to condition erosion and sediment. Moisture, wetness, soil
and geology determine materials and biome suitability. `World::environment_at`
returns immutable regional surface estimates; Y is ignored. Fine painting also
accounts for local channel refinement. No trees or live weather are simulated.

**Compatibility.** Generator version 3 and material schema 2 intentionally reject
older edit saves. Seed and climate configuration form part of baseline identity.

**Current limits.** Generation supplies a finite 10 km region. Terrain extraction
covers 0–2048 m vertically. Coarse summaries can still miss narrow features.
Maximum detail requests approximately pixel-sized cells; output resolution, FOV,
planning tile limits and residency determine achieved quality. The reported pixel
value is a projected-cell sampling target, not a certified silhouette-error bound.
Ambient shading is constant per display-cell face. Sun shadows approximate static
terrain through a 256-square regional height map; caves, actors and edits do not
cast accurate sun shadows yet. The optional edge filter is inspired by FXAA's
contrast test, not a complete FXAA implementation or temporal antialiasing.
Rendering never changes canonical
occupancy. There is no browser renderer, player editing UI,
live fluid/weather system or organism simulation. Numerical material properties remain
empty; simulation types are extension primitives.

## Controls and options

Click to capture the mouse; **WASD** walks, **Space** jumps, **Shift** sprints
and **Escape** opens display settings and releases the mouse. Escape or Resume
returns to play. The menu has detail, native-radius, terrain-cache and haze sliders
with clickable numeric fields (Delete clears an entry; Enter commits), three
presets and ambient/shadow/AA/LOD-fade toggles. Changes debounce for 200 ms; held
movement keys clear while the menu is open. Preferences live in ignored
`native/settings.conf`; headless runs use deterministic defaults.  **F5** cycles first/rear/front camera views. **F6** toggles
debug flight and returns to the player; flight uses **Space/C** vertically.
Wheel or **+/−** adjusts flight speed; **1/2/3** selects 4/20/100 m/s. **[ / ]** adjusts FOV, **R** selects another dry
location, **M** cycles environment maps and returns to 3D, **F11** toggles fullscreen and **F12** saves a PNG. The title bar shows
speed, FOV, frame rate, tile loading and mesh memory.

Options include `--seed 42`, `--quality 0..100`, `--native-radius 2..32`,
`--terrain-memory 64..1024` (MiB), `--haze 0..2`, `--no-effects`, `--save edits.mvw`
and `--screenshot output/image.png`. `--tiles 64..8192` is an additional planning
limit (default 8192); the memory setting also selects a smaller working tile target.
The protected native neighborhood can exceed that soft target. The menu shows
requested/effective pixel size, budget pressure, native/total tiles, queued work
and recent frame-time percentiles. Use `--help` for details. Windowed starts are
random; offscreen checks use a fixed scene.

## Validate and measure

GitHub Actions runs formatting and both test suites on pushes and pull requests.
The hardware-dependent GPU test is opt-in.

```sh
cargo fmt --check
cargo fmt --manifest-path native/Cargo.toml --check
cargo test --locked --all-targets
cargo test --locked --manifest-path native/Cargo.toml
# Explicit GPU readback test; requires a graphics device:
cargo test --locked --manifest-path native/Cargo.toml gpu_draws -- --ignored
# Same-footprint exact/LOD mesh comparison:
cargo run --locked --release --manifest-path native/Cargo.toml -- --benchmark
# Offscreen GPU, flight and LOD replacement check:
cargo run --locked --release --manifest-path native/Cargo.toml -- --headless --fly-test --frames 60 --screenshot output/native_preview.png
# Quality PNGs and paced moving-camera/memory stress check:
cargo run --locked --release --manifest-path native/Cargo.toml -- --lod-test --screenshot output/lod/final.png
# World queries and edits:
cargo run --locked --release --bin bench -- --terrain
```

Without Cargo, `./tools/build.sh` builds the core diagnostics with `rustc`.
For example, `./build/noise 0.0625 8 42 /tmp/patch 9316 5036` exports sampled
height/water/bedrock/material PGM images. These are diagnostics, not world saves.

Keep build caches, generated images, mesh exports and saves out of commits.
Add intentional presentation assets under `assets/`. See
[measurement notes](agent_context/PERFORMANCE.md) for recorded hardware and limits.

Climate presets: `temperate` (default), `continental`, `arid`, `subpolar`,
`tropical`. These choose a regional climate; terrain shapes are generated
independently. `--wind` is the direction towards which air moves in degrees
(+X = 0, +Z = 90); `--moisture` overrides incoming moisture in 0..1.

```sh
./native/target/release/microvoxel-game --isometric --screenshot output/isometric_10km.png
./native/target/release/microvoxel-game --climate tropical --wind 225
./native/target/release/microvoxel-game --climate temperate --map rainfall
./native/target/release/microvoxel-game --debug-maps output/environment
# Dependency-free alternative exports the same 12 layers as PPM:
cargo run --release --bin environment -- 42 temperate output/environment 25
# An optional seventh noise argument selects the climate preset:
./build/noise 0.0625 8 42 /tmp/patch 9316 5036 arid
```

`--isometric` exports a 3200×2400 full-region overview with uniform 16 m display
cells, canonical materials, no perspective, no fog and no height exaggeration.
It accepts the same seed/climate options and exits after saving the PNG.

Maps cover the full 10 km region with +X right, +Z down. Window titles and export
`legend.txt` explain each layer; `summary.txt` reports distributions. They are
coarse environmental diagnostics, not voxel-resolution exports. M releases mouse
capture; cycle through the twelve maps to return to flight.

[Research and implementation notes](agent_context/TERRAIN_CLIMATE_RESEARCH.md)
explain sources, approximations and efficiency choices. The
[climate/weather plan](agent_context/CLIMATE_WEATHER_PLAN.md) describes deferred
live weather and physical response; the static climate baseline is implemented.


## Character asset milestone

```sh
./native/target/release/microvoxel-game --character-preview output/character --seed 42
# Optional GIF/MP4 copies for viewers without APNG support (requires ffmpeg):
./tools/encode_character_previews.sh output/character
```

This skips terrain generation and exports model comparisons plus idle, walking,
jumping and waving animated PNGs. Columns compare 6.25 cm, 3.125 cm and 1.5625 cm
cells at the same physical height. Whole-cell occupancy is generated after posing;
no rigid voxel mesh is rotated off-grid. This asset experiment does not change
the world's 6.25 cm grid. Detailed export statistics are in `README.txt`.

The character uses ordinary `Voxel` values with palette IDs registered into a
caller-supplied material registry; the world's default registry and save identity
are unchanged by character previews. The recipe has a single body proportion
set for now. Material properties remain empty.

Walking and F5 views are now integrated. The game starts with a walking player;
`--view rear` or `--view front` chooses another initial camera, and `--fly` starts
debug flight. F6 returns to the same player rather than dropping it from the sky.

The controller runs at 60 fixed ticks/s with gravity, a single-press jump,
3 m/s walking, 5.5 m/s sprinting and up to four cells (25 cm) of autostep.
Exact collision uses the active pose's occupied cells. Leading-face masks skip
buried cells during unit-step sweeps; unknown and water cells block movement.
Pose changes check a conservative swept row envelope of changed occupancy and
may be held near obstructions. Physics and picking do not use LOD geometry.

Walking phase advances with accepted distance. Jump height comes from controller
gravity, never the character preview's decorative root offset. Character values
stay outside terrain edits/saves. First person hides the body and renders only
actual arm voxels; third person renders the complete active pose. CPU pose/body
and GPU character mesh caches each keep at most 16 entries at native resolution.

The player is a kinematic controller, not a general rigid-body solver. There is
no pushing, health/fall damage, swimming, inventory or player-position persistence.
Debug flight freezes the player. Coarse terrain LOD can still pop; collision stays
canonical. See the [checklist](agent_context/PLAYER_UPDATE_PLAN.md) and
[research](agent_context/PLAYER_RESEARCH.md) for design details.

```sh
# Real-world movement/collision test plus first/rear/front screenshots:
./native/target/release/microvoxel-game --player-test --frames 30 --screenshot output/player_game.png
```


The head follows camera yaw/pitch independently of the torso. While idle, small
looks turn the head first; large turns and a settled look bring the torso around.
While walking, the body follows walking direction with a limited shoulder
offset; backward walking keeps it facing forward. Look angles are quantized poses
and revoxelized before collision/meshing. Camera rotation stays smooth.

The first-person hand aims in the camera look basis, keeping a consistent
lower-right placement when looking up/down. Forearm visibility uses anatomical cell
ownership, so raising it no longer crops it at shoulder height. The sleeve/shoulder
is hidden in first person to keep it out of the near-camera view. It remains a
subset of the actual collision body, not an off-grid viewmodel. Existing pose
collision checks can still hold an obstructed turn near a wall. Climbing/slope
rules and terrain-adaptive animation remain future work.


Terrain presentation uses native-height water planes across LODs, gradual material
coverage and filtered distant slope illumination. Native voxels retain solid RGB,
exact collision and nearby face shading. Default vertical FOV is 65 degrees;
brackets still adjust it. See [terrain presentation notes](agent_context/TERRAIN_PRESENTATION.md)
for the two review rounds, tests and remaining limits. The new baseline material
recipe uses generator version 3; incompatible older edit saves are rejected.


### Selectable terrain renderer and streaming

Esc includes a renderer button: **Mesh** or **Ray casting**. The prototype traverses
sparse voxel column bricks directly; the player still uses the shared voxel-derived
raster path. Both backends use the same world, edits, materials and collision.
Command line: `--renderer mesh|ray`. Quality presets preserve the renderer choice.

| Files | Purpose |
| --- | --- |
| `src/render/ray.rs` | Immutable sparse column/run bricks and reference CPU traversal |
| `native/src/raycaster.rs`, `ray.wgsl` | Bounded GPU arena and direct voxel ray casting |
| `native/src/visibility.rs` | Hierarchical frustum/distance and conservative occlusion culling |
| `native/src/diagnostics/streaming_check.rs` | Walking, sprinting, turning and live backend comparison |
| `src/bin/sharing.rs` | Whole-brick geometry/payload sharing investigation |

Build queues replace stale work, reserve capacity for nearby detail and predict
movement. Fades permit generation to continue. Adaptive visibility protects nearby
loading and limits distant refinement/drawing when streaming falls behind.

```sh
./MicroVoxel --renderer ray
./native/target/release/microvoxel-game --stream-test
./tools/capture_renderers.sh
cargo run --release --bin sharing
```

The ray prototype is currently slower and larger than meshes in the measured
scenes. Its arena binding is capped at 128 MiB, with retirement headroom; quality
coarsens under pressure. The menu distinguishes resident data from arena capacity.
See [implementation and reference notes](agent_context/RAY_RENDERER_AND_STREAMING.md)
for exact limits, tests and measurements. No DAG or new dependency is required.

The opt-in `--ray-study` command profiles the existing ray renderer and benchmarks
compact SVO/DAG alternatives on identical rays. `src/render/octree.rs` contains
experimental immutable topology and an editable-region lifecycle probe;
`native/src/diagnostics/ray_study{,_gpu}.rs` and `ray_study.wgsl` run the comparison. Reports go
to ignored `output/ray_study/`. This command does not enable octrees or DAGs in
normal play. See [measured results](agent_context/OCTREE_DAG_STUDY.md) before
choosing a storage or renderer change.

For a repeatable movement and still-scene profile, use `./tools/profile.sh`.
It writes complete per-tick CSVs and CPU-update/draw/GPU-wait percentiles to
ignored `output/performance/`. Linux process RSS is sampled separately from
terrain payload and GPU arena capacity. See [the latest measurements](agent_context/PROFILING_AND_CLEANUP.md)
for settings and limitations. The HUD also retains frame stalls longer than a second.

Terrain GPU uniforms store fixed grid origins; camera motion updates one shared
frame uniform. Actor parts update their own origins. Both regular and conforming
vertices subtract the camera in the shader. The current finite grid fits exactly
in GPU floats; larger future coordinates will need region-relative origins.

`./tools/capture_cover.sh` reproduces the README's native 1440p cover. Only the
chosen `assets/cover.png` is tracked; diagnostic images, logs, builds and local
settings remain ignored.
