# Guide for developers

## Build and run

The native app requires Cargo, Rust **1.85+**, and working graphics drivers.
Linux currently uses X11, including XWayland. Run commands from the repo root:

```sh
cargo run --locked --release --manifest-path native/Cargo.toml
```

The core is a separate, dependency-free crate supporting Rust **1.75+**.
Both Cargo lockfiles are checked in. After building, the game executable is
`native/target/release/microvoxel-game`.

## Layout and where to make changes

| Path | Responsibility |
|---|---|
| `src/generation/terrain.rs` | Noise, macro terrain, erosion, drainage and sediment |
| `src/generation/mod.rs` | Canonical terrain adapter and column caching |
| `src/grid.rs` | Integer coordinates, 6.25 cm units and brick indexing |
| `src/materials.rs`, `src/painting.rs` | Material IDs, palettes and voxel colors |
| `src/world.rs`, `src/storage.rs` | Queries, atomic edits, revisions and compact overrides |
| `src/persistence.rs` | Versioned edit saves and baseline compatibility |
| `src/queries.rs` | Canonical raycasts and collision sweeps |
| `src/meshing.rs` | Bounded general 3D surface extraction |
| `src/render/` | Color-preserving face merging, LOD selection and boundary stitching |
| `src/simulation.rs` | Tick, active-region and stable-ID primitives |
| `src/bin/` | Noise-image diagnostic and world benchmark |
| `native/src/main.rs`, `camera.rs` | Window, input and grid flight |
| `native/src/runtime.rs`, `streaming.rs` | LOD cover replacement and bounded worker jobs |
| `native/src/gpu.rs`, `terrain.wgsl` | GPU buffers, drawing, lighting and selection |
| `tests/` | Core, persistence and rendering contracts |
| `assets/cover.png` | Actual native screenshot used by the README |
| `agent_context/` | Design notes, constraints and measurement history |

Keep terrain changes in generation, color changes in painting, geometry/LOD
changes in render, and presentation changes in the native renderer. New sky and
cloud passes can live alongside the native terrain pipeline.

## How the world works

**Canonical cells.** A voxel is 1/16 meter on a signed integer grid. Air differs
from unknown territory. Camera translation advances in whole cells with smooth
rotation and one voxel of clearance. Every face of a voxel shares its base color;
lighting may darken faces. Water occupies complete cells.

**Storage and edits.** Untouched terrain is procedural. A stored voxel is four
bytes: material ID and appearance. Only overrides persist, in adaptive sparse,
dense or uniform 32³-cell bricks. `World::apply` validates transactions before
committing and reports local invalidation. Restoring the baseline removes an
override. Save/load checks generator and material compatibility.

**Rendering.** Vertical runs skip buried terrain; exposed coplanar faces merge
into rectangles with a separate, integer-indexed color atlas. Meshes stay in GPU
buffers. A balanced quadtree chooses power-of-two grid spacings, with explicit
coarse/fine connecting faces. Complete replacement covers install atomically.
Workers, queues and caches are bounded; the GPU mesh-buffer budget is 128 MiB.

**Current limits.** Generation supplies a finite 10 km region. Terrain extraction
covers 0–2048 m vertically. Coarse LOD uses five-column sampled summaries, so small
features can disappear and transitions can pop. Rendering never changes canonical
occupancy. There is no browser renderer, player editing UI, character physics,
cloud/sky system or organism simulation. Numerical material properties remain
empty; simulation types are extension primitives.

## Controls and options

Click to capture the mouse; **WASD** moves, **Space/C** moves vertically,
**Shift** boosts and **Escape** releases. Wheel or **+/−** adjusts speed;
**1/2/3** selects 4/20/100 m/s. **[ / ]** adjusts FOV, **R** selects another dry
location, **F11** toggles fullscreen and **F12** saves a PNG. The title bar shows
speed, FOV, frame rate, tile loading and mesh memory.

Options include `--seed 42`, `--tiles 384`, `--save edits.mvw` and
`--screenshot image.png`. Use `--help` for details. Windowed starts are random;
offscreen checks use a fixed scene. The tile count is a layout target.

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
cargo run --locked --release --manifest-path native/Cargo.toml -- --headless --fly-test --frames 60 --screenshot native_preview.png
# World queries and edits:
cargo run --locked --release --bin bench -- --terrain
```

Without Cargo, `./tools/build.sh` builds the core diagnostics with `rustc`.
For example, `./build/noise 0.0625 8 42 /tmp/patch 9316 5036` exports sampled
height/water/bedrock/material PGM images. These are diagnostics, not world saves.

Keep build caches, generated images, mesh exports and saves out of commits.
Add intentional presentation assets under `assets/`. See
[measurement notes](agent_context/PERFORMANCE.md) for recorded hardware and limits.
