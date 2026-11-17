# Discrete whole-voxel water lab

Historical precursor. The current game integration is documented in
[the water integration checkpoint](../../research/nov16/water-integration/README.md).
This lab remains an independent comparison, not the production solver.

Standalone, greenfield CPU experiment; **not integrated into the game**. Each
authoritative cell is either empty or one full water cube. There are no water
fractions, particle rendering, smoothed surfaces, or changes to the engine.
The offline preview draws the actual recorded states as equal-size cubes.

The experimental solver uses staggered face velocities, a free-surface pressure
projection, accumulated signed face displacement, and simultaneous injective
cell moves including occupied chains and cycles. A bounded maximum-weight
assignment provides a small-fixture reference for the move resolver. It is not
a continuum-fluid reference or a validated Navier–Stokes discretization.
Quantization, a speed clamp, credit saturation, and omitted velocity advection
and viscosity limit physical fidelity. Velocity energy is a proxy, not Joules;
mass conservation does not establish energy or momentum conservation.
Face velocities guide occupancy moves; transported voxel momentum is not
implemented. This is one preliminary candidate, not completion of the water
simulation plan or a claim of physical acceptance.

In the recorded 240-step run, dam release triggers 23,420 speed clamps and
227,598 credit saturations; communicating vessels trigger 25,147 clamps and
500,831 saturations. Those large intervention counts and remaining pressure
residuals expose substantial numerical limitations. The vessel level gap
shrinks from 12 to about 1.38 cells, but six seconds does not demonstrate
settled equilibrium. Consult `data/diagnostics.json` for the recorded run.

Run from the repository root (`MicroVoxelProceduralGeneration/`):

```sh
rustc --edition=2021 -O frontier/experiments/discrete_water/main.rs -o /tmp/discrete-water-lab
/tmp/discrete-water-lab frontier/output/discrete-water-lab/data 240
python frontier/experiments/discrete_water/preview.py
```

The simulator uses 0.0625 m cells and a 0.025 s fixed timestep. The 240-step run
records six simulation seconds for falling water, a released dam, a resting
pool, communicating vessels, and flow around an obstacle. The binary accepts
the output directory and step count as positional arguments. Run core tests:

```sh
rustc --edition=2021 --test frontier/experiments/discrete_water/core.rs -o /tmp/discrete-water-tests
/tmp/discrete-water-tests
```

Preview requirements: Python 3, Pillow, DejaVu fonts; optional `ffmpeg` for MP4.
All generated files stay under `frontier/output/discrete-water-lab/`:

- `data/*.jsonl`: recorded binary simulation states and metrics.
- `{scene}.mp4`: 30 FPS playback at one simulation second per video second.
- `{scene}_comparison.png`: six evenly spaced recorded states, with timestamps.
- `{scene}_final.png`: final state at 1280 × 800.
- `{scene}/frame_*.png`: individually rendered samples and playback manifest.
- `{scene}_preview.json`: source, playback settings, and water-cell counts.

For only the requested comparisons:

```sh
python frontier/experiments/discrete_water/preview.py \
  frontier/output/discrete-water-lab/data/dam.jsonl \
  frontier/output/discrete-water-lab/data/vessels.jsonl
```

`--speed 2` plays two simulation seconds per video second; the default is `1`.
Sample intervals come from JSON timestamps, independent of simulation CPU
runtime. The video repeats sampled states; it does not invent intermediate
positions. Frame labels show simulation time and exact occupied-water count.
The final sampled state has a brief terminal hold (about two video frames).
CPU step timings measure this experimental solver, not engine frame rate.

The renderer uses a fixed orthographic isometric camera, a dark background,
three blue face shades, and outlined planar cube faces. Front exterior walls
and the ceiling are cut away only in the preview; their solid collision cells
remain in the simulation. Internal barriers remain visible. Cubes are opaque
so hidden cells are occluded naturally; there is no liquid mesh or smoothing.
JSON indexing is `x + nx * (y + ny * z)`, with `y` up. `water` and `solids`
contain sparse occupied indices. Frames include dimensions, `cell_size_m`,
`frame`, `time` in seconds, and solver metrics.

Reuse tier: **G (greenfield)**. No inherited engine code was transplanted.
This bounded model and its software images establish no real-time performance
claim, renderer promotion, or production fluid acceptance gate.
