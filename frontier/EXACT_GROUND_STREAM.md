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

The view can be explored in the preview. A fast turn still reveals a chain
of page dependencies, and the working cache currently only grows until
its cap or a spatial recenter. The next experiment should fetch several
certified search intervals ahead of the visible ray in a single batch,
use a bounded recyclable cache, and decouple work from presentation.
The measured 1080p visibility pass also needs work before full-resolution
integration.

## Reproduce the short flight and the 6 km test

```sh
bench=target/release/exact_ground_stream
"$bench" --far 185 --width 640 --height 360 --page-side 32 \
  --eye -6688,510,6080 --target -6548,475,6220 --frames 96 --bank 12 \
  --rounds 160 --audit 12 --capture-motion --prefetch-siblings --output output/exact-flight
"$bench" --far 6000 --width 640 --height 360 --page-side 16 --frames 12 \
  --speed 0 --rounds 512 --audit 10 --stackless --output output/exact-far
```

The reports include first-hit range, unresolved pixels, failed queries,
camera ground clearance, native page residency, and measured wall and GPU
timings. Diagnostic captures identify unresolved rays with a checker pattern.

The implementation draws on [ray-guided streaming](https://www-sop.inria.fr/reves/Basilic/2009/CNLE09/CNLE09.pdf)
and [hierarchical heightfield ray traversal](https://diglib.eg.org/items/2e7fb9c6-3bff-46e4-821e-80218187e456).
All geometry in this experiment is the same native ground at every distance.
The hierarchy is used for conservative skipping.
