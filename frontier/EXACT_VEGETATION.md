# Editable vegetation without geometry LODs

`--exact-vegetation` is a finite rolling-terrain experiment. It uses the
existing seeded plant records and models. Trees, bushes, grass and flowers
contribute the original occupied native cells wherever the canonical world
generates them. Primary visibility traverses a hierarchy of bounds and
resolves the first occupied cell in a 4×4×4 brick. Camera distance never
selects a substitute plant. Terrain and vegetation use the same explicit
`--capture-far` horizon, with a 256 m resource guard on this trial. Dense
biomes can hit the configured plant budgets at a shorter horizon.

The unedited brick data is immutable. Each current world revision supplies
a sorted sparse overlay. A cut cell is omitted immediately on the GPU, even
if the deleted cell belonged to a shared or overlapping plant. The ordinary
world picker and collision still query canonical occupancy. Stone placements
are drawn by the existing terrain edit owner. The regular eight-meter edit
reach applies. Edits survive rebuilds during the session. This trial has no
save option for edits across sessions.

The shader uses hardware depth with the scene projection and a simple
axis-normal lighting model. Dedicated vegetation shadows and temporal
coverage filtering are future experiments. At one sample per pixel, foliage
can still alias as its projected coverage shrinks. The plant renderer does
not turn that aliasing into a distance-dependent change of geometry.

## Measurements

Ryzen 5 7640U, Radeon 760M hardware Vulkan adapter, Mesa RADV 25.2.8.
Release builds used Eroded seed 6401777571733596131 and the fixed 62 m
woodland camera below. Both renderers used the same native-detail terrain,
one sample per pixel and identical lighting switches. NAADF, weather,
atmosphere and dedicated shadows were disabled. Each run requested 180
frames, with 30 warmup frames, 109 collected GPU samples and 149 presented
intervals. Ranges summarize the two measured runs. Frame intervals include
presentation and CPU work.

| Configuration | Vegetation GPU p95 | Full render GPU p50 / p95 / p99 | Frame interval p95 |
| --- | ---: | ---: | ---: |
| Stock, 1080p | 0.88–0.93 ms | 10.48–10.84 / 10.59–11.08 / 10.64–11.19 ms | 14.01–14.62 ms |
| Exact, 1080p | 17.55–19.08 ms | 26.47–28.49 / 26.85–29.18 / 26.94–29.41 ms | 30.04–32.36 ms |
| Exact, 720p | 8.54 ms | 12.79 / 12.87 / 12.89 ms | 15.50 ms |

The 62 m scene expanded to 7,205,715 occupied cells in 310,376 bricks.
Palette indexing preserved all stored pigments and reduced resident plant
bindings from 43.8 MiB to 30.1 MiB, a 31% reduction. Scene preparation
still took 2.1–2.2 seconds. A 110 m riverside view of the same seed contained
17,652,136 cells and used 73.0 MiB after palette compression. It took 4.85
seconds to prepare. CPU placement and materialization are repeated when
the camera leaves the current coverage margin. At 960×540 a 6 m/s,
240-frame fly-through peaked at 2.36 seconds between frames, with a
2.34-second synchronous vegetation prepare. Its p95 of 11.29 ms hides
that single hitch. These costs rule out scaling the resident flattened
volume directly to multi-kilometer views.

Near-ordered and fixed depth-first traversal produced identical 960×540
woodland images. The near-ordered version took 19.66 ms versus 20.15 ms
for the fixed walk in the initial 1080p vegetation-pass comparison.
Clipping against same-frame terrain first hits gave a small preliminary
improvement. With clipping and palette indexing toggled together, the
matched 960×540 runs agreed at every pixel in both Depth32Float and PNG
readbacks. The rolled synthetic GPU fixture also matched the independent
f64 world picker for 713 pixels before and after removing the foreground
leaves, for both traversal modes.

## Research and design choices

| Method | Relevant evidence | Decision for Frontier |
| --- | --- | --- |
| Sparse native bricks and a hierarchy of conservative bounds | Laine and Karras use occupancy masks and compact nodes to skip empty space. Occupancy is the useful part here. | The experiment stores native occupancy and a pigment per occupied cell, with a single inline pigment for uniformly colored bricks. Bounds only reject rays. [Efficient Sparse Voxel Octrees](https://research.nvidia.com/sites/default/files/publications/laine2010i3d_paper.pdf) |
| Terrain-guided instancing with local plant acceleration | Dietrich, Colditz, Deussen and Slusallek use terrain-height intervals plus plant extents for a top-level 2D tree. Each leaf maps a reusable local plant scene onto the terrain, and object rays traverse the individual models. | The strongest candidate for long-range woody vegetation. Use integer transforms, native voxel modules and biome-dependent deterministic placement. Both rendering and picking must expand the same modules. Build the search hierarchy from empty-space bounds and keep one occupancy field. [Terrain Guided Multi-Level Instancing](https://jahrbib.sulb.uni-saarland.de/bitstream/20.500.11880/25910/1/MLI_RT2006.pdf) |
| Shared exact occupancy with sparse edits | HashDAG preserves shared unmodified voxel subtrees while editing affected paths. | Keep reused plant modules immutable. A world-cell tombstone can suppress one occurrence. Shared source data stays intact. Consider hash-consing once we know how much geometry the new source repeats. [Interactively Modifying Compressed Sparse Voxel Representations](https://doi.org/10.1111/cgf.13916) |
| Large-scale hardware foliage rendering | NVIDIA's HPG 2026 work uses instances, merged geometry LODs and opacity micromaps. AMD's HPG work reuses static local geometry under animated cages. These results use hardware ray tracing. | Reuse of small modules is transferable. RTX-style geometry substitutions and hardware-dependent throughput are outside this experiment's contract. [NVIDIA HPG presentation](https://highperformancegraphics.org/2026/_assets/slides/Dietger_slides.pdf), [AMD HPG research](https://gpuopen.com/learn/ray-tracing-massive-amounts-animated-geometry/) |
| Filtering subpixel coverage | Temporally distributed blue-noise samples improve the error pattern and convergence of stochastic rendering. | Trace the same occupancy field at jittered pixel positions. Accumulate with depth and edit-revision validation. Test silhouettes and rapid camera motion against supersampled native references. [Scalar Spatiotemporal Blue Noise Masks](https://research.nvidia.com/publication/2021-12_scalar-spatiotemporal-blue-noise-masks) |

The long-range design needs two kinds of queries. A terrain-aligned hierarchy
should store bounds for possible canopy, then descend into sparse root pages
and reusable branch/leaf modules. The world seed can choose maturity, branching
sites, module combinations, quarter turns and color independently. Define
these modules in the canonical generator so the runtime has one set of native
cell addresses to edit and render. A ground-cover query should restrict its
work to conservative vegetation-height intervals above the terrain and
materialize deterministic native blades only there. This avoids enumerating
every meadow cell within six kilometers. Keep the exact occupancy contract
identical for CPU picking, physics and GPU intersections. Test canopy bounds
with low grazing rays and compare pixel coverage with an independent
supersampled native reference. Pages and caches can change residency, while
their native cell addresses, silhouettes and edit keys remain stable.

## Reproduce

From `frontier/`, with an available GPU adapter:

```bash
cargo build --release --locked --bin frontier

common=(--preset eroded --native-detail --seed 6401777571733596131
        --eye -128,82.45,-128 --target -149,85.67,-149
        --capture-far 62 --vegetation on --naadf off --terrain-shadows off
        --weather off --atmosphere off --render-scale 1 --msaa 1
        --plant-cpu-mib 384 --plant-gpu-mib 512)

./target/release/frontier "${common[@]}" --exact-vegetation \
  --width 960 --height 540 --capture /tmp/woodland-exact.png

./target/release/frontier "${common[@]}" --exact-vegetation \
  --width 1920 --height 1080 --benchmark-input-lock --route static \
  --uncapped --warmup 30 --frames 180 --report /tmp/exact-vegetation.json

cargo test --release --locked -p frontier-renderer --lib negative_cells_address
cargo test --release --locked -p frontier-renderer --lib \
  gpu_first_hit_follows_canonical_voxels_and_live_edits -- --ignored
```

The renderer reports plant and occupied-cell counts, allocated buffer bytes and
scene build time in capture metadata and benchmark JSON. For the direct GPU
comparison, remove `--exact-vegetation` to measure stock vegetation, or set
`FRONTIER_EXACT_VEGETATION_TRAVERSAL=threaded` to walk the same bounds without
the near-ordered traversal's deferred stack.

For ablations, `FRONTIER_EXACT_VEGETATION_DEPTH=0` disables reuse of native
terrain's center-sample hit buffer. Water, unresolved rays and fading terrain
never shorten the vegetation ray. `FRONTIER_EXACT_VEGETATION_PALETTE=0` stores
the same packed RGB values directly instead of using 16-bit palette indices.
