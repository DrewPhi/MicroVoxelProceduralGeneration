# Guide for developers

## Current checkpoint

The default scene now tests a CPU-canonical seeded FBM source on an experimental
4.096 km square landscape. Seven independently moving nested GPU grids share
cached canonical samples and match boundary heights, normals and filtered colors.
Geometry is reused during camera motion; a bounded background worker prepares
coherent replacements. This remains a **surface prototype**, not a native-voxel
handoff or a completed production renderer.

`--scene fbm-native` displays an exact 8×8 m, 6.25 cm voxel crop from the same
FBM source. `--scene hybrid` shows a fixed 64×64 m exact native interior, an
approximately 2 m collar, and a static outer proxy. It is diagnostic only: no
moving native ownership, caves, edits, or certified error bound. `--scene
fixture` preserves the original editable 16×14×16 m mountain/tunnel/bridge
reference. All implementation is greenfield (tier G).

## Build and run

Install Rust 1.99.1 with rustup; the workspace pins it. Linux needs an X11 or
XWayland session, Vulkan/OpenGL drivers and the usual C linker/build tools.

```sh
cd frontier
cargo test --workspace --locked
cargo build --release --locked
./run.sh --scene fbm --seed 7 --camera valley
./run.sh --scene fbm --seed 7 --camera aerial --view-levels 11
./run.sh --scene fbm-native
./run.sh --scene hybrid --seed 7
./run.sh --scene hybrid --seed 7 --camera aerial
./run.sh --scene fixture
./run.sh --capture output/fixture.png --width 1920 --height 1080
./run.sh --seconds 15 --route stop-resume --route-clock realtime --warmup 30 --uncapped --report output/run.json --width 1920 --height 1080
```

Create the output directory before capture (`mkdir -p output`). `run.sh` runs
an existing release binary; rebuild after source changes. The finite window run
prints adapter information and frame interval percentiles, including presentation
wait. These are not isolated GPU timings or a production performance result.

Click the window to capture the mouse. WASD moves horizontally, mouse looks,
Space/Ctrl moves vertically, Shift accelerates, Esc releases the mouse, F12 saves
a PNG. Aim at screen center: left click removes a cell, right click places stone
within 8 m in the fixture. One background rebuild runs at a time; wait for it before editing
again. Edits work only in `--scene fixture` and are session-only. This is a free-fly camera, not a colliding player
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

See [moving coverage measurements](frontier/STREAMING_CHECKPOINT.md),
[FBM checkpoint measurements](frontier/FBM_CHECKPOINT.md) and the earlier
[native reference checkpoint](frontier/CHECKPOINT.md) for tests, hardware results
and explicit limitations.
The [handoff checkpoint](frontier/HANDOFF_CHECKPOINT.md) documents the partial
hybrid prototype and the 7/9/11-level view-distance experiment. Its initial
hybrid-ground report is provisional pending a retake. The screenshots show clear
native/proxy aliasing and shading differences; no invisible-transition claim is
made.

## FBM controls and repeatable captures

`--seed`, `--scale` (horizontal base wavelength in meters), `--amplitude` (vertical
relief in meters), `--octaves` and `--roughness` configure the source. These change
the landscape, not the voxel size. `--camera valley|aerial` selects a starting view.
FBM defaults: seed 7, scale 1800 m, amplitude 1000 m, 8 octaves, roughness 0.5.
The source combines warped gradient noise and ridged FBM; it is not an erosion,
hydrology or tectonic simulation. Material coloring is currently grass and rock.

WASD speed is 35 m/s (Shift 80 m/s) in the broad FBM scene, 3 m/s (Shift 15 m/s)
in the native references. Free flight has no collision. The broad scene does not
use infinite resident terrain: its outer grid covers ±2048 m around its published
center, with 0.5 m finest to 32 m outer spacing. Levels recenter independently;
generation can lag rapid travel. Going outside loaded coverage or below ground
is possible. Both FBM scenes
disable editing until the correct rendering ownership/update path exists.

Run `./tools/smoke_streaming.sh` for real-time stop–resume and turn–return routes,
end-of-route PNGs, and streaming metrics. `--route-speed` sets benchmark speed
in m/s (default 80 for FBM); 240 m/s currently fails streaming and is a stress
test, not a supported travel profile. `--terrain-static` freezes initial
coverage for comparison; `--seconds` limits wall-clock run duration and
`--capture-on-exit` captures the final displayed scene.

Run `./tools/smoke_fbm.sh` after building for a seed/camera PNG sweep and a
360-frame stop–resume report. `--route static|fly|stop-resume|turn-return` uses
absolute simulated time `frame_index * --route-step` (default 1/60 s), independent
of achieved FPS. Broad FBM benchmark routes stay at least 10 m above the canonical
surface; interactive flight does not. These are repeatable synthetic workloads,
not measurements of streaming at actual real-time travel speeds unless
`--route-clock realtime` is selected.

JSON reports include source parameters, native size, representation, camera,
route, adapter, warmup and frame percentiles. GPU timestamps and total RSS/VRAM
are null until measured. `--capture` and `--report` are intentionally separate
modes. Warmup frames are excluded from frame statistics. Keep raw artifacts under
ignored `frontier/output/`.

## What remains

The surface prototype does not yet implement toroidal GPU strip uploads,
error-driven LOD selection, seamless exact-near voxel handoff, hybrid cave/edit
ownership, persistent edits, walking collision, or production geography. The
simple footprint filter has no certified silhouette bound. Do not claim G0–G4
passed from these tests. Near-surface smoothing in the broad proxy is a known
prototype limitation, not the final voxel appearance contract.

The native reference uses a complete halo and never caps solid crop boundaries;
its cutaway edges are intentional. World generation stays CPU-canonical and
samples are uploaded to the GPU, avoiding a second noise implementation.
See `frontier/world/README.md` for source and boundary semantics.

## Legacy references

Read the [master issue](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/24)
and both Frontier planning files before implementing later stages. Old engines
remain on their remote branches; inspect with `git show` or a separate benchmark
checkout. Do not restore old source trees here. Generated historical artifacts
remain in the [archive release](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/releases/tag/legacy-research-archive-2026-11-04).
