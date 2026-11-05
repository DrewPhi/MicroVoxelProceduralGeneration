# Archived analytic terrain experiments

Development moved to the
[`frontier-voxel-engine` branch](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/tree/frontier-voxel-engine).
This branch preserves the analytic renderer, fitting experiments, negative results,
and Bouchet job tooling as research references. Generated previews, fitted models,
datasets, and submission receipts are preserved in the
[legacy research archive](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/releases/tag/legacy-research-archive-2026-11-04).
Extract `analytic-experiments-artifacts.tar.gz` into this branch's checkout to
restore its original `output/` directory. Build caches and executables are omitted.
See [the archive record](agent_context/ARCHIVE_STATUS.md).

Rust terrain authoring and a native GPU analytic renderer, with Python fitting
experiments and cluster tooling. Default world: **10 km × 10 km**, **6.25 cm voxels**,
1024 m vertical extent. Seeded 3D noise creates a continuous density field;
voxel occupancy samples its sign at cell centers. Caves and surface overhangs
are supported by the authoring field. The viewer renders fitted or initialized
quadratic patches with continuous and exact-voxel modes, ground walking,
selectable sampling, and conservative air-hierarchy traversal.

Generated terrain datasets and compiled executables are local artifacts and are
not committed. A fresh clone needs to build the binaries and generate or obtain
a model using the workflows below; `explore-1km` expects the local initialized
world and `./analytic` binary.

## Run

Launch the locally built executable `./fbm`, or build with Rust 1.88+:

```sh
cargo run --release
```

Adjust parameters and click **Preview**. Nothing regenerates automatically.
Changes leave the previous preview visible and marked as stale. Another Preview
cancels obsolete work; Cancel retains the previous image. Results are saved in
separate directories beneath `output/fbm/`.

- **Overview:** sampled base-height map of the complete world. Click it to select
  a location. This diagnostic excludes 3D rock displacement and caves; it is not
  a full-world native-voxel rendering.
- **Local voxels:** exact native occupancy in a cropped region, with orbit
  controls and directional face shading. The exposed vertical cut sides are the
  inspection box, not giant cliffs in the actual terrain. Underground material
  is gray stone; green is restricted to a thin outer-surface skin. This is a CPU voxel
  traversal reference, not our proposed analytic renderer.
- **Isometric:** parallel-ray local voxel view at a fixed 35.26° elevation. Orbit
  yaw rotates the view; 45° diagonals give equal-axis isometric projection.
  North/West/East/South buttons above local previews move by one inspection
  width and generate the next preview. North is −Z, East is +X. Movement clamps
  at the world boundary; another click cancels obsolete generation.
  A cached world locator marks the displayed chunk in red. The full overview
  also shows the selected footprint; a crosshair helps locate subpixel boxes.
- **Cutaway:** vertical X/Y occupancy slice at the selected Z coordinate.
- **Save/Load settings:** JSON recipes include all terrain and inspection values.
- **Export 16 m voxel fixture:** writes a small native-resolution 3D occupancy
  dataset near the selected location for fitting experiments. At 6.25 cm this is
  256³ voxels, about 2 MiB of bits. The maximum is 32 million cells per fixture.

The complete world is defined by the small recipe, not a huge dense voxel file.
World coordinates and noise scales are meters. Voxel size changes sampling,
not the underlying terrain field. No patch-local randomness or flattened patch
boundaries are used. This is an fBM authoring baseline, not simulated geology.

## Reproducible headless previews

```sh
cargo run --release -- --headless output/overview --width 512
cargo run --release -- --view local --headless output/local --width 320
cargo run --release -- --view isometric --headless output/isometric --width 512
cargo run --release -- --view slice --headless output/slice --width 512
cargo run --release -- --params output/local/recipe.json --headless output/recheck
cargo run --release -- --export-voxels output/fixture.fbmvox
cargo test --release
```

For custom terrain, save a recipe in the GUI or edit its JSON. Put `--params`
before command-line overrides such as `--view` and `--width`.

## Layout and data

- `src/terrain.rs`: parameters, global density and canonical voxel occupancy.
- `src/preview.rs`: diagnostic rendering, PNG output and native occupancy export.
- `src/main.rs`: native GUI, cancellable background jobs, recipe IO and CLI.

`FBMVOX01` files contain an 8-byte magic, a little-endian u32 JSON header length,
the UTF-8 header and packed occupancy bits (least-significant bit first). The
header records the complete recipe, integer origin and dimensions. X varies
fastest, then Z, then Y. Outside the finite world is air. These are volume
occupancies, not height columns, so tunnels and disconnected solids survive.
Successful exports replace the destination only after the data is complete.

Noise: [FastNoiseLite](https://github.com/Auburn/FastNoiseLite), OpenSimplex2,
with f64 input coordinates and normalized additive octaves. GUI:
[egui/eframe](https://github.com/emilk/egui), using its OpenGL backend to display
completed images. Runtime dependencies are pinned in `Cargo.lock`.

## Analytic camera demo

The independent native renderer loads the fitted **continuous** min-of-implicit-quadrics scene. It uses GPU compute: ordered patch-grid traversal followed by closed-form quadratic intersections and active-branch checks. No terrain meshes or geometric LOD. Continuous mode does not step through voxels; the selectable exact-voxel mode uses analytic empty-space skipping and local cell checks. Fitted patch continuity remains a separate fitting task.

```sh
cargo build --release --bin analytic
./target/release/analytic
```

Default input: `output/fit-occupancy/terrain.json` (the fitted **32 m cube**, not the full 10 km authoring world). Use `--scene PATH` to load another compatible fit. The local `./analytic` executable also launches the demo from this directory.

Click the window to capture the mouse. **WASD** flies, **Space / left Ctrl** moves vertically, **Shift** boosts speed, and the **mouse wheel** changes speed. **Esc** releases the mouse; **Home** resets the camera. **1 / 2 / 3** selects lighting / patch traversal heatmap / quadratic test heatmap. **F12** saves `output/analytic/camera.png`.

The title displays frame rate, GPU compute time when supported, CPU submission time and sparse traversal counters. Diagnostic readbacks are asynchronous every 30 frames. Compute time includes traversal, intersections and shading; these are not individually timed.

```sh
./analytic --headless --size 640x360 --frames 60 --verify \
  --capture output/analytic/preview.png
```

Headless mode saves a PNG and a matching JSON report with adapter, memory, timings and GPU-hit comparison against an independent double-precision CPU reference (one ray every 32 pixels; 2 mm distance tolerance). `--camera x,y,z,yaw,pitch` uses scene-local meters and degrees. Headless synchronized frame time includes waiting/readback effects and is not an interactive FPS prediction. Software adapters do not represent laptop GPU performance.

### Comparing voxel rendering

Press **Esc** for the clickable rendering-options menu; selecting a mode keeps the menu open for comparisons. Choose **Resume** or press Esc again to fly. **4** selects snapped-hit mode and **5** selects accelerated exact voxels directly. **6** selects the unaccelerated reference for comparison. The original **1/2/3** shading/diagnostic shortcuts remain.

- **Continuous:** analytic intersection and smooth quadric normals.
- **Snapped hit approximate:** analytic intersection; quantize the hit's material sample to a cell center and choose an axis-aligned shading normal. This is a visual approximation: hit depth and silhouette remain continuous, and it does not construct voxel side faces.
- **Exact voxels:** solid iff every fitted quadratic is positive at the voxel center. Closed-form intersections with conservative quadratic envelopes skip empty intervals, stopping at patch boundaries. Remaining candidate cells get exact occupancy checks and axis-aligned faces. This is now the accelerated path.

Both voxel modes use the fitted model's voxel size (currently **6.25 cm**) and a consistent per-cell material on every face, with face lighting. No refitting is required. The finite fitted scene is treated as empty outside its bounds in exact mode, exposing cut faces. Starting inside a solid voxel gives a zero-distance hit; fly into air to inspect the surface.

Headless testing accepts `--mode snapped` or `--mode voxels`; `--verify` checks voxel hits against the double-precision occupancy reference in exact mode. Reports include the rendering mode and voxel size. Traversal counters mean **patch visits** in continuous/snapped mode and **cell visits** in exact mode. Compare steady-state GPU timings at the same camera and viewport; short runs include pipeline warmup.

To repeat a five-camera performance sweep on your own GPU:

```sh
python3 tools/benchmark_renderer.py --size 640x360
```

This compares continuous, snapped, accelerated voxels and reference voxels at identical cameras, performs two runs of 62 frames per combination, and saves captures plus `output/analytic/profile/summary.json`. GPU measurements use the last periodic timestamp (after startup); they are repeated samples, not frame-time percentiles. The test is headless and excludes swapchain presentation. No new terrain fitting is involved.

### Analytic voxel acceleration

For each quadratic Q, a precomputed margin M bounds how much its value can change between a ray point and that point's voxel center. Where `Q(ray(t)) + M < 0`, that quadratic proves the cell is air. The next possible occupied interval begins at a closed-form quadratic root. The ray jumps there, clipped to the current patch, then tests candidate cells. No numerical root search, distance-field assumption, terrain mesh or geometric LOD is involved. Residual cell traversal remains for uncertain intervals, including grazing views. Non-grid-aligned patches conservatively use the reference path.

Margins are computed once from analytic gradient bounds and use existing coefficient-buffer padding, adding no GPU storage. Boundary times are computed directly from integer coordinates to avoid cumulative drift. `--mode voxel-reference` retains the full traversal baseline. Reproduce checks with:

```sh
python3 tools/benchmark_renderer.py --modes voxels voxel-reference --verify
python3 tools/check_analytic_renderer.py
```

The conservative-bound approach follows the general implicit-surface rejection principle described by [Knoll et al., Interactive Ray Tracing of Arbitrary Implicits with SIMD Interval Arithmetic](https://www.sci.utah.edu/~knolla/rtia-rt07.pdf). Our specialization uses quadratic roots and a voxel-center gradient bound rather than their general interval subdivision method.

### Seam-free quadratic reconstruction experiment

`tools/quadratic_spline.py` implements the direct construction in Rössl et al., *Visualization of Volume Data with Quadratic Super Splines* (VIS 2003), sections 2–3. Each cube uses a shared 27-cell-center stencil to generate 65 Bernstein–Bézier controls, then exports 24 spatially restricted total-degree-two polynomials. Neighboring cubes agree on their shared face; there is no seam-loss optimization. Cube faces are C1 under this construction; arbitrary internal tetrahedral faces need not be C1.

```sh
cargo build --release --bin fbm_points --bin analytic
python3 tools/quadratic_spline.py        # mathematical self-tests
python3 tools/test_spline_boundary.py    # same two original seam patches
./analytic --scene output/fit-spline-boundary/1m/terrain.json
```

The last command opens only the targeted **8 × 16 × 8 m** region. Standard continuous, snapped and exact voxel modes work with the new format. The regular `./analytic` default is still the original 32 m model.

Results: `output/fit-spline-boundary/index.html`, `metrics.json`, `boundary-comparison.png`, and `8m/`, `4m/`, `2m/`, `1m/` models. The test reuses the ignored original native density fixture and calls the Rust authoring field for sparse samples and halo values; it needs no GPU fitter. The published reconstruction is a quasi-interpolant, not voxel-exact fitting. It does not claim the same compression or expressivity as five unconstrained quadrics per old patch.

The viewer specializes its shader per representation, locates each tetrahedron in constant time, clips ray intervals using barycentric planes, and solves only quadratics. No terrain triangles or ray-marched root search are used. For exact voxels, the sampled cell center chooses its own piece; analytic skips stop at piece boundaries and use a gradient bound covering **all** pieces in the cube. This is necessary because the ray point and its cell center can belong to different tetrahedra. Model fields: `representation: quadratic-super-spline-24`, `solid: Q>0`, 24 coefficients per cube in the partition order defined by `quadratic_spline.py`. Old models remain supported.

### One-kilometer authoring world (before GPU fitting)

```sh
cargo run --release --bin fbm_world
python3 tools/preview_world.py
```

This samples the existing seed-42 3D fBM around the tested area and saves a **1 km × 1 km** region definition in `output/world-1km/world.json`, a 1024² top-down PNG, native-column height samples, and a vertical cave cross-section. The region bounds in `world.json` are authoritative; the accompanying old-format recipe retains local inspection settings and does not specify this larger crop by itself. Surface colors are a height/shading diagnostic, not biome painting. The full 3D field remains defined by the recipe; no dense native volume, quadratic fit, GPU training implementation or cluster job is created by these commands.

### GPU fitting on Bouchet

The GPU fitting package is documented in [tools/gpu_fit/README.md](tools/gpu_fit/README.md).
It uses the compact min-of-five quadratic representation with hard shared-face
constraints, not the separate tetrahedral spline experiment. It samples the Rust
3D field sparsely, trains with PyTorch/CUDA, validates independent native centers,
and exports shared parameters, baked coefficients, checkpoints and a preview.
Large-world validation is sampled rather than exhaustive. Submission records for
the authorized 1km run are saved locally in `output/bouchet-world-1km/submission.json`.

### Explore the initialized 1 km world

Run `./explore-1km` from this directory. It opens the downloaded Bouchet
initialization covering 1000 × 424 × 1000 meters, with 8 m patches and five
quadrics per patch. This is the retained step-zero base-height approximation,
not a successful fit of the full cave/overhang FBM target.

Click to capture the mouse; WASD flies, Space/Ctrl moves vertically, Shift boosts,
and the wheel adjusts speed (starts at 30 m/s). Esc opens rendering options;
1 selects continuous terrain and 5 selects exact 6.25 cm voxels.

The viewer loads `output/world-1km-initialized/terrain.json` and its little-endian
packed coefficient sidecar directly, without expanding it into nested JSON.
The 198,750,000 coefficient bytes occupy two GPU storage pages; this does not
change terrain resolution or introduce LOD. Existing small JSON scenes still work.
Captured overview and close voxel view are in the same output directory.

Ground walking: press **G**, or choose **TOGGLE WALK FLY** in the Esc menu.
`./explore-1km --walk` starts grounded. Eye height is 1.7 m; WASD moves
horizontally at 4.3 m/s, Shift runs, and the wheel adjusts speed (walking capped
at 10 m/s before sprint). G returns to free flight. Ground height is solved
analytically in the current vertical patch column; exact voxel mode samples the
voxel column center and quantizes its surface height. Steps are limited to 15 cm
horizontal increments, rejecting large/cliff height changes and world edges.
This is a top-surface walking preview, without jumping, gravity, a body collider,
or entering caves. It does not add any per-pixel rendering work.

Anti-aliasing is on by default. **T** or the Esc-menu **ANTIALIASING** row toggles
it for comparison. It uses one jittered primary ray per pixel, depth-validated
camera reprojection, and bounded temporal history in linear color. Resize,
render-mode switches, toggling AA and camera teleports reset history. Exact voxel
mode also filters subpixel face lighting toward the analytic surface normal;
this approximates distant appearance without changing occupancy or intersections.
Nearby voxel faces retain their individual shading. Fixed voxel-coordinate color
variation gives grass and stone subtle texture, faded when below pixel size.

Headless `--no-aa` disables filtering; `--verify` automatically disables jitter
for exact CPU/GPU comparisons. `--camera-step x,y,z` moves the camera by local
meters each captured frame, useful for temporal regression captures. This test
motion is not ground-following. New comparisons are in
`output/world-1km-initialized/{aa-voxels,unfiltered-voxels,aa-moving}.png`.

Temporal filtering references: [AMD's temporal reconstruction guide](https://gpuopen.com/manuals/fsr_sdk/techniques/super-resolution-temporal/)
(camera jitter, reprojection and history reset); [PBRT sampling theory](https://www.pbr-book.org/4ed/Sampling_and_Reconstruction/Sampling_Theory).
This is a small custom filter, not an FSR integration. Remaining limitations:
possible softening/trails at difficult disocclusions, and approximate subpixel
voxel lighting. Broad 8 m facets in the initialized model are geometry, not aliasing.

The Esc menu now has two live strength sliders (click or drag, 0–100):
- TEMPORAL STRENGTH: previous-frame contribution, default 50; zero disables temporal jitter/accumulation. History contribution is capped at 95%.
- FACE LIGHTING SMOOTHING: distant voxel-face lighting blend, default 0 to preserve the voxel look. Independent of temporal strength, but the master AA switch must be on.
Slider changes discard old history immediately. Settings apply for the current session.

Esc → SPATIAL SAMPLES selects 1–4 rays per pixel with fixed sample positions.
Each ray intersects and shades the actual terrain; colors are averaged in linear
space. Start at 2, try 4 for quality; cost grows with ray count. Temporal strength
now defaults to zero (face smoothing remains zero), so stationary samples do not
move. Keep the master ANTIALIASING switch on to use spatial sampling. Defaults to
1 sample to preserve performance; settings are session-only. Headless captures
accept `--samples 1|2|3|4`; `--verify` forces one unjittered center ray. Diagnostics
report average traversal work per ray, not total per pixel.

Esc now includes a TRAVERSAL panel: GRID BASELINE (default) and AIR HIERARCHY.
The hierarchy proves air patches using conservative bounds on the rendered f32
quadratics, with voxel-center and numerical margins; parent nodes are air only
when all children are air. Rays jump across proven-air regions and retain exact
quadratic/voxel intersection tests elsewhere. No LOD, surface replacement or
mesh is introduced. The current world adds about 3.8 MB of hierarchy data.
Reference-voxel mode intentionally keeps its original traversal.

HARDWARE UNAVAILABLE is disabled: this adapter exposes experimental ray-query
flags, but wgpu 24's public BLAS API only accepts triangle geometry, not the
procedural AABBs required here. No hardware traversal backend has been implemented.
The general approach is supported by [Vulkan procedural ray traversal](https://docs.vulkan.org/spec/latest/chapters/raytraversal.html);
see the installed wgpu 24 `api/blas.rs` (`BlasGeometries`) for the API limitation.

Headless `--traversal grid|hierarchy` permits reproducible comparisons. In one
960x540 long-view test, exact-voxel GPU time fell from 7.56 ms to 5.26 ms; close
views were slightly slower (1.90 vs 2.05 ms), so both options remain available.
Raw results: `output/world-1km-initialized/traversal-benchmark.json`. Settings
are session-only. CPU/GPU checks and full-image comparisons passed for sampled
views; performance improvements are scene-dependent.

### Experimental quadratic envelope traversal (#20)

Choose **Esc → Traversal → Quadratic Envelopes**, or launch with:

```sh
./explore-1km --walk --mode voxels --traversal envelopes
```

Parent quadratics conservatively bound the solid/air field (including native
voxel-center margins). Closed-form ray inequalities skip certified-negative
prefixes even inside some mixed terrain regions. Source surfaces, voxel size and
appearance remain the same. Grid and Air Hierarchy stay selectable.

This is a research comparison, currently **slower than Air Hierarchy** in tested
views: about 6.84 vs 5.41 ms for a long 960x540 voxel view at 1 sample, and 27.29
vs 21.68 ms at 4 samples. It reduces source-quadratic tests but adds parent-bound
work. GPU envelope memory is ~5.9 MB; construction adds ~0.2 s at scene load.
The window title and JSON captures now distinguish source tests from envelope
bound tests. Reference-voxel mode remains unaccelerated; continuous experimental
spline scenes retain their previous traversal.

The mathematical construction, float32 limitations, testing and results are in
[agent_context/QUADRATIC_ENVELOPES.md](agent_context/QUADRATIC_ENVELOPES.md).
Reproduce with `tools/check_quadratic_envelopes.py` and
`tools/benchmark_envelopes.py` after building `--bin analytic`. Raw generated
reports stay in ignored `output/envelopes/`.
