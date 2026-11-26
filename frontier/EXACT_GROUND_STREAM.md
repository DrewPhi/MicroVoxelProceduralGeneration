# Exact ground streaming experiment

`exact_ground_stream` is a standalone flyable preview and benchmark for opaque
ground from Frontier's current procedural source. Every first hit is a native
6.25 cm column top or side. A quadtree records conservative ceilings, and
screen rays request exact pages where a first hit remains possible. Packed
pages retain integer height and ground material. Changing the page size
changes scheduling, memory, and traversal, while the surface is the same.

The preview uses keyboard free flight, diagnostic materials, and lighting
filtered from neighboring exact hits. It presents complete frames with a
status in the window title. When input reveals unready ground, the previous
complete frame stays on the screen while the requests finish. The preview
covers ground only. A game integration will also need vegetation, water,
voxel edits, caves, suitable visibility antialiasing, and a cache that can
recycle pages over a long route.

```sh
cargo build --release --locked -p frontier-renderer --bin exact_ground_stream
target/release/exact_ground_stream --interactive --far 185 --width 640 --height 360 \
  --page-side 32 --prefetch-siblings --stackless
```

WASD moves the camera, the arrow keys look, Space and Ctrl climb and descend,
Shift moves faster, Home returns to the original pose, and Esc closes the
preview. `--seed` accepts an integer. The supplied command uses seed
1685576058315138427 and a 185 m diagnostic view limit. The height and material
source is Eroded. The main game continues to use its standard launcher.

## What changed

- The first attempt used the existing full terrain bounds. On 400 sampled 1 m
  boxes near the Eroded test area their ceilings were about 167 m above their
  center samples. Mirroring the bedrock and province functions in conservative
  interval arithmetic reduced that to 14 m. The ceiling can ignore river
  cutting and secondary incision for opaque ground, since both only lower it.
- Persistent workers keep each thread's procedural climate, drainage, and
  erosion caches warm. Height page maxima propagate into the parent
  hierarchy, which can tighten bounds after generation.
- Pages pack exact integer heights with 8-, 16- or 32-bit values and retain
  the true maximum of each 4×4 native block. The GPU uses those maxima to
  skip work before intersecting native faces.
- Speculatively loading sibling pages reduces some round trips. Its memory
  cost can exceed the savings for a full horizon.
- The two GPU traversal modes use per-ray ancestor indices or parent links.
  Both use the same exact geometry and conservative rejection rules.
- `--interval-bounds` clips each noise query to the gradient cells it
  actually covers. `--drainage-bounds` also encloses the primary riverbed at
  small nodes. These are rejection bounds for the original terrain.
- `--certificates` retains only an exact maximum when all requesting rays
  pass over a native page. `--refine-roofs` tries tighter procedural
  certificates before generating that page.
- `--lookahead N` lets one ray per 8×8 group request up to N more nodes
  after its first unresolved node. `--resume` carries the proven empty
  prefix and its entry face into later passes for the same pose. As the
  number of unresolved pixels falls, lookahead follows a remaining ray
  in each group. Every reported first hit still needs a complete prefix.
- `--beams` shares conservative empty-prefix tests across an 8×8 group.
  `--block-packing` optionally encodes each exact 4×4 block from its own
  maximum, preserving the original integer heights and materials.
- `--sparse-blocks` requests just the intersected 4×4 blocks. Each page
  has a table with pointers to the exact data and conservative bounds for
  blocks a ray passed over. Roof refinement can certify a block before
  running native generation.
- `--delta-bounds` publishes appended hierarchy records and patches only
  changed ceilings or links. Large patches fall back to a full upload.
- `--vertical 8` experiments with sharing an exact horizontal traversal
  across eight pixel rows in a level camera. The pixel rows retain separate
  first intersections and radial range tests. A pitched or banked camera
  changes the horizontal azimuth within a screen column, so this mode
  currently requires a level headless camera.

## Radeon 760M results

Ryzen 5 7640U, Radeon 760M, RADV Vulkan Mesa 25.2.8. Headless GPU
timestamps cover first-hit tracing. Wall time includes submitting,
reading requests, and generating all pages needed at the sampled pose.
Stationary warm metrics exclude the start frame. Motion sampling fixes the
camera step at 1/60 s, with elapsed wall time measured independently.

| Case | Cold complete view | Warm or moving wall p50 / p95 |
| --- | ---: | ---: |
| Legacy spiky reference, 640×360, 300 m | 0.37 s | 7.54 / 8.12 ms |
| Eroded, 640×360, 185 m, walking-speed camera and gentle turn | 0.90 s | 8.48 / 44.44 ms |
| Eroded, 640×360, 185 m, 80 m/s and bank | 1.31 s | 76.95 / 118.32 ms |
| Eroded fixed view, 320×180, 6 km clip, 32-cell pages | 14.30 s | warm p95 approximately 5.8 ms |
| Eroded fixed view, 640×360, 6 km clip, 16-cell pages | 29.51 s | 10.98 / 16.44 ms |
| Eroded fixed view, 1080p, 185 m, parent-link traversal | 3.27 s | 27.20 / 49.47 ms |

The 6 km run at 640×360 used 71.04 MiB of packed column data and 4.46 MiB
of active hierarchy records. The farthest visible first hit at that pose
was 2.52 km. Adding sibling prefetch hit the 96 MiB cache capacity and
left 301 pixels unresolved. The walking and banked short-range routes
completed 96 frames each with zero unresolved pixels and remained above
the terrain. An open high-altitude view reached terrain 4.65 km away
and exhausted the hierarchy capacity with 25,878 unresolved pixels at
320×180.

The CPU audits used an independent flat native-column traversal and
agreed on terrain versus sky for the tested complete views. Splitting
the ray origin into integer cells and a fractional part reduced the
walking audit's maximum depth error from 111 mm to 0.013 mm. The fixed
1080p audit still had an 87 mm grazing depth outlier. Page compression,
the Eroded ceiling and both shaders have dedicated tests.

## Open 6 km horizon

The open view looks down from 1,100 m above Eroded ground on the same
seed and exercises long visible intersections. The complete packed
320×180 runs all returned the same depth buffer,
with a farthest first hit of 5,999.61 m and 5,718 pixels hitting beyond 3 km.
The independent native traversal agreed on hit coverage for all 64 audit
rays, with a maximum depth error of 0.73 mm.

| 320×180, identical open camera | Cold complete view | Warm GPU p95 | Native payload |
| --- | ---: | ---: | ---: |
| Interval, certificates, beams, lookahead | 94.88 s | 7.36 ms | 83.07 MiB |
| Same plus saved empty prefixes | 85.29 s | 7.47 ms | 83.07 MiB |
| Watershed bounds, saved prefixes, more lookahead | 56.97 s | 6.59 ms | 83.91 MiB |
| Watershed bounds, refined roofs, adaptive lookahead and block packing | 48.38 s | 6.60 ms | 72.44 MiB |
| Requested 4×4 blocks, adaptive lookahead | 19.44 s | 15.15 ms | at least 56.12 MiB of table entries |

The warm samples contain 5, 9, 11, 9 and 5 frames respectively. GPU timings
cover visibility. They exclude lighting, clouds, water, vegetation and the
rest of a game frame. The lossless block-packed run used another 12.18 MiB
of active hierarchy records. Capacity is reserved separately from resident
data. The saved-prefix runs match the restarted rays byte for byte.

At 640×360, the first watershed configuration exhausted the 256 MiB
native payload cache with 13,955 pixels pending. The packed configuration
at 384 MiB capacity completed the same camera in 117.85 s. Its exact
view reached 5,999.86 m, with 22,861 pixels hitting farther than 3 km.
Five stationary visibility samples took 17.59 ms GPU p95 and 19.01 ms
wall p95. The first moving and banked sample took another 64.09 s to
complete. The next sample exhausted the cache with 36,101 pending rays.
These results leave startup, moving-view residency and 1080p visibility
outside the playability target.

Sparse 16-cell tables completed the same 640×360 view in 67.96 s with
71.35 MiB of active block data and 48.44 MiB of active hierarchy records.
The complete image is pixel-identical to the packed configuration.
All 64 native audit rays agreed on coverage, with a maximum depth error
of 1.63 mm. Across eight warm passes GPU p50 was 17.45 ms,
with 33.11 ms p95. The 4.19-million-node cap left 558 rays unresolved.
Increasing the reserved capacity to 8.39 million nodes let the view finish
with 4.23 million active nodes.

The 1920×1080 run used sparse blocks, 32-cell tables with a 384 MiB
payload budget, and the same open camera. It exceeded the 130 s probe
limit. The last complete report batch was at pass 320 with 514,100
pixels still pending. The run produced no complete 1080p frame.
Its partial pass timings are unsuitable for comparison with complete
visibility frames.

Saved prefixes reduced the summed GPU time across 438 startup passes
from 2.76 s to 0.11 s. CPU bounds and column generation remained the
bottleneck. The 48.38 s run still generated 174,044 native pages while
retaining 58,369. Sharper bounds eliminated many unnecessary pages in
this pose. Reducing the number of evaluated native columns is the larger
remaining opportunity.

The 320×180 sparse run generated 40.9 million native columns, down from
178.2 million in the packed run. The first two moving and banked samples at
80 m/s took 16.61 s and 15.32 s. The next exceeded the 144 MiB page budget.
Sparse tables reduced generation and storage. More lookups slowed warm
visibility, and the streaming cost remained too high for flight.

With narrower block roofs enabled, the matched incremental-upload test
finished at 17.50 s and 17.33 s. Full hierarchy uploads took 20.60 s and
20.49 s in the same two order-swapped pairs. All four runs needed 662
passes and produced bit-identical depths, including when compared with
the independently scheduled packed run. The incremental mode's warm
GPU p95 was 14.30–14.62 ms across six warm frames in each run.

On a separate level 640×360 camera with a 185 m clip, the 8-row traversal
and independent rays agreed on coverage at all 230,400 pixels, with a
maximum depth difference of 0.061 mm. Across 19 warm samples their median
GPU times were 4.73 ms and 5.34 ms respectively. This limited fixture does
not establish a benefit for arbitrary camera orientations.

The preview can fly through the same source. The cache currently grows
until its cap or a spatial recenter. The measurements point to GPU-side
native column generation and a rolling cache that recycles data and
preserves proofs during flight. Either approach needs to keep exact
occupancy parity with the canonical terrain.

## Reproduce the short flight and the 6 km test

```sh
bench=target/release/exact_ground_stream
"$bench" --far 185 --width 640 --height 360 --page-side 32 \
  --eye -6688,510,6080 --target -6548,475,6220 --frames 96 --bank 12 \
  --rounds 160 --audit 12 --capture-motion --prefetch-siblings --output output/exact-flight
"$bench" --far 6000 --width 640 --height 360 --page-side 16 --frames 12 \
  --speed 0 --rounds 512 --audit 10 --stackless --output output/exact-far
"$bench" --eye -6688,1450,6080 --target -5350,900,8360 \
  --far 6000 --width 320 --height 180 --page-side 32 --cache-mib 96 \
  --node-limit 2097152 --stackless --drainage-bounds --refine-roofs \
  --block-packing --beams --lookahead 12 --resume --rounds 512 \
  --frames 10 --speed 0 --audit 8 --output output/exact-open-6km
"$bench" --eye -6688,1450,6080 --target -5350,900,8360 \
  --far 6000 --width 640 --height 360 --page-side 32 --cache-mib 384 \
  --node-limit 4194304 --stackless --drainage-bounds --refine-roofs \
  --block-packing --beams --lookahead 8 --resume --rounds 512 \
  --frames 11 --stationary-frames 5 --speed 80 --bank 4 \
  --capture-motion --output output/exact-open-banked
"$bench" --eye -6688,1450,6080 --target -5350,900,8360 \
  --far 6000 --width 640 --height 360 --page-side 16 --cache-mib 192 \
  --node-limit 8388608 --stackless --drainage-bounds --sparse-blocks \
  --refine-roofs --beams --lookahead 48 --resume --rounds 1800 \
  --frames 9 --speed 0 --audit 8 --output output/exact-open-sparse
"$bench" --eye -6688,1450,6080 --target -5350,900,8360 \
  --far 6000 --width 320 --height 180 --page-side 32 --cache-mib 96 \
  --node-limit 2097152 --stackless --drainage-bounds --sparse-blocks \
  --refine-roofs --delta-bounds --beams --lookahead 48 --resume \
  --rounds 1800 --frames 7 --speed 0 --audit 8 --output output/exact-open-delta
```

The reports include first-hit range, unresolved pixels, failed queries,
camera ground clearance, native page residency, and measured wall and GPU
timings. Diagnostic captures identify unresolved rays with a checker pattern.

The implementation draws on [ray-guided streaming](https://www-sop.inria.fr/reves/Basilic/2009/CNLE09/CNLE09.pdf)
and [hierarchical heightfield ray traversal](https://diglib.eg.org/items/2e7fb9c6-3bff-46e4-821e-80218187e456).
The shared empty prefix also follows the beam optimization described in
[Efficient Sparse Voxel Octrees](https://research.nvidia.com/sites/default/files/publications/laine2010i3d_paper.pdf).
All geometry in this experiment is the same native ground at every distance.
The hierarchy is used for conservative skipping.
