# Shared native vegetation at a 6 km horizon

`--module-flora` is an opt-in vegetation source for seeded Eroded terrain
and 6.25 cm voxels. It selects the native-detail terrain renderer. Terrain,
grass, and woody vegetation all use the same radial view distance.

Each 16 m world-space site has a deterministic chance of growing a woody
plant. Habitat comes from smoothly varying grove density and the exact
native terrain material and elevation. A site chooses one of 16 seed-grown
conifer, deciduous, sapling, and shrub archetypes and a quarter turn. The
archetypes are composed of the engine's woody strokes and foliage clusters.
Their occupied cells are shared by every instance. Each site keeps its
archetype at every distance.

The GPU makes root and visibility decisions from the existing terrain
controls. A projected rectangle schedules rays for a plant. Each ray checks
a shared atlas of native 4³-cell bricks, skipping empty 16³-cell blocks and
bricks. Solid terrain hits shorten the rays where applicable. The GPU
retains the shared atlas and recomputes a compact list of visible sites as
the camera moves.

Grass blades and occasional flowers are deterministic native columns in
the five-cell ground band. The terrain relief atlas includes exact cover
heights at leaves and conservative combined relief-and-cover extrema at
coarser nodes. Those nodes skip empty intervals. The projected footprint
filters grass color and normals while intersections and edits stay at
native cells. Shorter 4×4 screen beams, interval leaps, and 16×2 compute
groups are scheduling and empty-space skipping choices.

Picking and wood collision use the same world-cell source. Edited columns
carry their revised soil and ground cover. Sparse world-cell overrides
hide the edited voxel in a single plant instance without modifying the
shared archetype. A GPU test compares 3,185 first-hit rays in each of two
edit states against the independent CPU world DDA. Its fixture includes
grass and woody hits.

This experiment uses one sample per pixel for the performance measurements.
Existing multisampling and postprocessing can be used separately. Dedicated
foliage shadows, temporal foliage filtering, and cross-session edit
persistence remain outside this launch mode. [Finite renderer and source
research](EXACT_VEGETATION.md)

## Radeon 760M results

Ryzen 5 7640U, Radeon 760M Vulkan with Mesa RADV 25.2.8. All runs used
release builds, a 6 km radial horizon, native output resolution, and one
visibility sample per pixel. The test series used fixed seeds and camera
poses. Weather simulation, NAADF, and shadow rendering were disabled.
The atmospheric run additionally enabled the atmosphere and the engine's
edge filter.

| 6 km scene | Resolution | GPU full-render p95 | Presented frame p95 |
| --- | --- | ---: | ---: |
| Riverside bank, 80 m/s | 1280×720 | 15.08 ms | 17.54 ms |
| Aerial transit, 2.15 km along the route | 1280×720 | 19.05 ms | 21.71 ms |
| Steep wooded valley | 1280×720 | 29.84 ms | 32.60 ms |
| Steep valley with atmosphere and edge filtering | 1280×720 | 30.02 ms | 32.70 ms |
| Steep wooded valley | 1920×1080 | 67.10 ms | 69.27 ms |

The 720p runs collected 74–124 presented intervals after warmup. The steep
valley reached 35.39 ms p95 while a test build competed for the CPU.
During the quiet transit, the terrain window published twice, including
startup, and the worst frame interval was 51.35 ms. The plant pass itself
took 1.5–3.2 ms GPU p95 at 720p. Terrain and ground-cover traversal remain
the main cost in steep views.

The woody atlas uses 1.02 MiB, with 18.32 MiB total for woody placement,
visibility buffers, and unedited storage. Adding exact grass bounds and
coverage increases the terrain relief texture by 10.67 MiB. The final GPU
edit test matched coverage and distance to within 0.02 mm in its fixture
and observed 985 changed pixels after cuts. A separate 4× MSAA smoke
capture reached the same horizon.

## Launch

Run from `frontier/`.

```sh
cargo build --release --locked --bin frontier

./target/release/frontier --preset eroded --seed random --module-flora \
  --capture-far 6000 --width 1280 --height 720 --msaa 1 --naadf off
```

To reproduce the steep valley benchmark:

```sh
./target/release/frontier --preset eroded --module-flora \
  --seed 4788417784819070747 \
  --eye -6388.69,1429.56,519.53 --target -6886,376.25,2859 \
  --capture-far 6000 --width 1280 --height 720 --msaa 1 \
  --naadf off --weather off --terrain-shadows off --atmosphere off \
  --render-scale 1 --benchmark-input-lock --uncapped --warmup 25 \
  --frames 140 --route static --report /tmp/module-flora-valley.json

cargo test --release --locked -p frontier-world --lib shared_models_and_grass
cargo test --release --locked -p frontier-renderer --lib \
  modular_flora_depth_and_cut_cells_match_world_raycast -- --ignored
```
