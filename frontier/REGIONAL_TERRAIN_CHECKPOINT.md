# Regional terrain pass — issue 58

The launcher now defaults to `--terrain-recipe regional`, `--voxel-filter on`
(user preference), and 4×MSAA. Native cells remain 6.25cm. This is a small
source-generation experiment, not completed geology, hydrology, or biome work.

## Implemented

A deterministic low-frequency province field masks gentle ridges into some
regions while leaving others as rolling plains/foothills. A restrained 100m
warp at 4500m wavelength bends the composition. At default scale/amplitude:
90m smooth macro influence at2500m,320m masked ridge influence at1400m,
12m conditioned foothills at300m,1.5m conditioned detail at40m. Smooth ridge
absolute values remove derivative cusps. Regional octave gain is
`0.8 * roughness` (default0.4); `octaves` controls fine detail bands. Macro/ridge
bands use three octaves. All wavelengths/displacements scale with `scale/1800`;
all height influences scale with `amplitude/1000`. These are noise band weights,
not guaranteed peak heights or measured Earth geology.

`regional-unwarped` is the otherwise identical control. `legacy` retains the
original generator exactly, with frozen bit-level fixtures. Source versions:
legacy1, regional2; report JSON also records recipe name (warped/unwarped share
version2 but differ in recipe). CPU f64 source is sampled at canonical column
centers and floored once to native cells. The GPU still receives those source
samples; no independent LOD-dependent landscape exists. Runtime caches belong
to a fixed source instance. There is no persistent-world save migration here;
any future save key must include recipe, source version, seed and parameters.

Library `FbmParams::default()` deliberately remains Legacy for existing fixtures;
the interactive CLI explicitly chooses Regional. The historical prototype and
LOD sweep scripts explicitly select Legacy for reproducibility. No old engine,
optimizer or scheduler was transplanted. Two inexpensive agents implemented the
world recipe/tests and CPU gallery; root integrated and checked actual captures.

## Evidence

CPU gallery: three recipes, seeds7/19/83,2km and8km squares,257² samples each.
Height quantization is canonical6.25cm; slopes/curvature use fixed±2m derivatives.
Shared scales in plots; isometric surfaces display65² samples without vertical
exaggeration. These are diagnostic heightmap plots, not game screenshots.

Seed7 over8km: legacy elevation -274..773.44m versus regional -40.94..256.63m.
Slope p50/p95 drops from49.91°/69.18° to about5.2°/20.2°. Other held-out seeds
113,997,0x6d28,0xdead_beef pass tests retaining >150m regional relief while
reducing p50/p95/p99 slopes below half legacy. Lower slope alone is not realism.
Default source costs19 noise evaluations (unwarped17) versus legacy11; the CPU
gallery measured roughly twice the evaluation time. This trades generation
work for calmer geometry; it is not a noise-evaluation speedup.

Clean sequential GPU comparison: Radeon760M/Ryzen5 7640U, RADV PHOENIX Vulkan,
Mesa25.2.8,1920×1080,4×MSAA,voxel-filter on,seed7,15m/s stop/resume route,
15seconds,60warmup frames,uncapped,seven view levels. No concurrent tests/builds.
Both start at origin and choose their source-dependent open-view yaw/eye height;
these are different scenes, not renderer speedup measurements.

| Recipe | frame p50 / p95 / p99 ms | >16.67ms frames | resident native GPU payload | cold CPU cover prep |
|---|---:|---:|---:|---:|
| Legacy | 3.98 / 7.94 / 10.81 | 8/3172 | 535.1MB | 285ms |
| Regional | 3.53 / 5.58 / 8.19 | 1/3936 | 135.0MB | 682ms |

Both had zero cancelled native builds; end native-center lag5.25m/4.80m.
Smoother terrain emits fewer voxel side faces, explaining lower mesh payload
and rendering cost despite slower source queries. Payload is not total RAM/VRAM;
short runs do not prove all-seed performance or sustained60fps.

89 workspace tests pass, one existing expensive benchmark ignored. Formatting,
Python syntax, release build and actual GPU capture/report paths pass.

## Visual judgment / remaining work

Inspected ground and aerial views, a mountain-region view, a held-out seed, and
shared-scale CPU galleries. Plains and foothills are much less spiky, with
regional relief rather than ridges everywhere. Warp subtly bends features; it
is not a dramatic change at this strength. The origin is intentionally quiet.
Mountains are restrained/rounded, not dramatic alpine peaks. Basic grass-only
appearance in these elevations makes the result look plainer than its heightmap;
materials, exposure and biome diversity still need work. Existing LOD boundaries
remain visible. No rivers, erosion, sediment, caves or overhangs were added.
This leaves issue58 open for further comparison, not a claim of physical realism.

## Try it

```sh
./run.sh --scene hybrid
./run.sh --scene hybrid --terrain-recipe legacy
./run.sh --scene fbm --camera aerial --view-levels 9
./run.sh --scene fbm --camera aerial --view-levels 9 --start-xz -1156,3469
./run.sh --scene hybrid --start-xz -750,-2500
./run.sh --scene hybrid --terrain-recipe regional-unwarped
# Appearance can still be disabled explicitly:
./run.sh --scene hybrid --voxel-filter off
# Diagnostic galleries (Pillow/NumPy/Matplotlib are offline tooling only):
cargo run --release --locked -p frontier-tools --bin terrain_gallery
MPLCONFIGDIR=/tmp/terrain-mpl python3 tools/terrain_gallery.py
```

`--start-xz X,Z` sets physical starting coordinates for fbm/hybrid; seed and
recipe are unchanged. Source parameters remain configurable with `--scale`,
`--amplitude`, `--octaves`, `--roughness`. Release launcher rebuilt. Artifacts:
ignored `output/regional-terrain/*.png`, `*-sprint-clean.json`, and `cpu/`
(CSVs,metrics,method metadata,six comparison PNGs). Earlier `*-sprint.json`
runs overlapped background tests and are not the performance evidence above.

Guidance: [issue58](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/58)
and [Red Blob Games' noise terrain tutorial](https://www.redblobgames.com/maps/terrain-from-noise/)
on octave amplitudes, redistribution and regional variation. Independent
implementation using existing Frontier gradient noise; no external code copied.
