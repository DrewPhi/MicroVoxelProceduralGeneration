# Guide for developers

## Current checkpoint

Branch `frontier-voxel-engine` now has a standalone Rust workspace under
`frontier/`. This is the bounded native-voxel reference checkpoint, not a
completed production renderer. The scene is 16 × 14 × 16 meters at 6.25 cm
native resolution. Terrain, a tunnel, and a bridge share one canonical source.
No old runtime code was transplanted (reuse tier G).

## Build and run

Install Rust 1.99.1 with rustup; the workspace pins it. Linux needs an X11 or
XWayland session, Vulkan/OpenGL drivers and the usual C linker/build tools.

```sh
cd frontier
cargo test --workspace --locked
cargo build --release --locked
./run.sh
./run.sh --capture output/fixture.png --width 1920 --height 1080
./run.sh --frames 300 --width 1920 --height 1080
```

Create the output directory before capture (`mkdir -p output`). `run.sh` runs
an existing release binary; rebuild after source changes. The finite window run
prints adapter information and frame interval percentiles, including presentation
wait. These are not isolated GPU timings or a production performance result.

Click the window to capture the mouse. WASD moves horizontally, mouse looks,
Space/Ctrl moves vertically, Shift accelerates, Esc releases the mouse, F12 saves
a PNG. Aim at screen center: left click removes a cell, right click places stone
within 8 m. One background rebuild runs at a time; wait for it before editing
again. Edits are session-only. This is a free-fly camera, not a colliding player
controller.

## Layout

| Path | Purpose |
|---|---|
| `frontier/world` | Integer voxel source, materials, COW edit snapshots, local coordinates and reference DDA |
| `frontier/renderer` | Exact greedy surface reference mesh, wgpu window/headless capture and camera |
| `frontier/runtime` | Bounded cache/scheduler policy and frame statistics; not a production streaming service |
| `frontier/tools` | Launcher and independent mesh-versus-world ray parity tests |
| `frontier/VALIDATION.md` | Reference cases and proposed laptop quality/performance gates |
| `frontier/ARCHITECTURE.md` | Production design and staged renderer decisions |
| `frontier/AGENTS.md` | Scope, reuse and measurement rules |

See [checkpoint measurements](frontier/CHECKPOINT.md) for tests, hardware results
and explicit limitations.

## What remains

The static reference does not implement reusable displaced-grid clipmaps, distant
LOD, heightfield/volume ownership, streaming, persistent edits, a walking player,
or production geology. Do not claim G0/G1/G2/G3/G4 passed from this fixture.
The next step is the minimal clipmap and ownership smoke experiment against this
native reference, followed by measured renderer comparisons on the laptop.

The initial mesh uses a one-cell halo and never caps solid crop boundaries.
That intentionally leaves a finite cutaway scene: it is not a streaming hole.
World generation stays on the CPU for this checkpoint, so GPU procedural-source
parity is not yet demonstrated. See `frontier/world/README.md` for fixture and
ray boundary semantics.

## Legacy references

Read the [master issue](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/24)
and both Frontier planning files before implementing later stages. Old engines
remain on their remote branches; inspect with `git show` or a separate benchmark
checkout. Do not restore old source trees here. Generated historical artifacts
remain in the [archive release](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/releases/tag/legacy-research-archive-2026-11-04).
