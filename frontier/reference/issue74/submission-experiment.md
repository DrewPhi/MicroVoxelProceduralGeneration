# Issue 74: adjacent vertex-page binding experiment

2026-11-06. Greenfield, bounded M2 experiment. Three runs per condition at 1 km
pass the provisional whole-frame p95 keep screen using the median run result:
20.210 ms control versus 17.161 ms with adjacent vertex-page binds, a 15.1%
reduction. Recommend retaining this bounded submission change with
`FRONTIER_COALESCE_VERTEX_PAGES=0` as the slice-path fallback after required
tests pass. These are small sequential samples, not a broad hardware or motion
claim. The 300 m pair does not pass the keep screen, and the 1 km result still
fails the 16.67 ms whole-frame p95 target. No culling, geometry approximation,
lighting or motion behavior is promoted by these measurements.

## Why this experiment

Pinned wgpu 24.0.5 render bundles are CPU command streams replayed on every
execution. In `wgpu-core-24.0.5/src/command/bundle.rs`, `execute` at line 963
iterates `self.base.commands` around line 977 and calls HAL operations for
dynamic bind groups, vertex buffers and draws. This is not a cached native
secondary command buffer. Different per-owner uniform offsets and vertex
allocation slices therefore remain real replay commands.

The source inspected in this session is
`/tmp/voxel-cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-core-24.0.5/src/command/bundle.rs`.
At line 1611, `wgpu_render_bundle_set_pipeline` already discards repeated
pipeline IDs. The corresponding direct-pass early-out is in the same pinned
package's `src/command/render.rs`, `render_pass_set_pipeline` at line 2864.
Adding another adjacent pipeline cache would not remove settled bundle replay
commands. These observations are pinned implementation evidence, not a claim
about all wgpu versions or backends.

The new path binds the entire existing vertex page across adjacent native/proxy
owners on that page. It shifts instance ranges by the allocation's record offset
instead of rebinding a different slice. It preserves the original owner and draw
order, per-draw dynamic uniform offsets, profile tables, local origins, material
IDs and native-bin ranges. Generic indexed meshes retain their original slice
path and reset the adjacent-page cache. Unsupported addresses fall back to slices.
No shader, GPU feature, geometry or visibility changes are required.

Only the enabled experiment rounds vertex reservations to 64 bytes. Released
ranges and resident-memory accounting include the padding. At most 63 bytes per
cluster is added logically, less than 1 MiB at 16,384 clusters; crossing a page
boundary could allocate another 8 MiB page, still subject to the existing 512 MiB
pool cap. Address checks retain page, record, count and u32 instance bounds.

## Frozen fixture and provenance

The eight MSAA4 runs below use one executable, SHA256
`12f7c42220ce0e7a869b2c658b3f0611544395ce8db9483c7b6ccb55f5f8cab8`.
The recorded checkout was dirty; its observed commit does not identify the
executable's build revision. Each run's `runs.json`, report, capture sidecar,
process log and time log remain under
[`output/issue74/vertex-pages`](../../output/issue74/vertex-pages/).
Commands and allowlisted environment values are recorded there.

The adapter is AMD Radeon 760M integrated graphics, RADV PHOENIX, Vulkan,
Mesa 25.2.8-0ubuntu0.24.04.2. The source is Legacy seed 7, source version 1,
canonical 0.0625 m cells, horizontal scale 1800 m, amplitude 1000 m,
eight octaves, roughness 0.5. The representation is pooled canonical native quads
and proxy triangles, with exactly the same selected owners in each distance pair.

The full output is 1920×1080, render scale 1, 4× MSAA, voxel filtering on,
atmosphere off, exposure 1. Eye is `(0,94.8875,0)`, vertical FOV 60°,
near clip 0.025 m, far clip 300 or 1000 m. The actual captured target is
`(-1.508074092271272e-7,94.83756238296628,-0.9987523555755615)`;
use the recorded capture sidecar rather than assuming input direction survives
the renderer's f32 yaw conversion exactly. The route is stationary with input
locked, vsync off, 60 warmup frames, four preparation workers, eight refinements
per transaction and initial quality/reserve required before measurement. Each new
run requests eight measured seconds. CPU cache/build/GPU pool budgets remain
512 MiB, with the actual source partition 32 MiB.

## Matched timing scope

`draw_encode_cpu_measured` samples the preceding presented frame's `Gpu::draw`
wall duration alongside the next accepted frame interval. Warmup, preparation
and capture-only draw calls do not enter this histogram. The paired frame
histogram has the same sample population. The scope includes queue writes,
render-pass begin/end and direct encoding or bundle replay; it excludes
`adaptive.prepare`, command finish/submit, presentation, atmosphere and UI.
It is not exclusive bundle replay time or total CPU frame work.

GPU timestamps measure the terrain pass only. They are an asynchronous subset
with their own sample counts, not paired whole-frame GPU measurements. Do not
subtract their percentiles from CPU/frame percentiles to derive overlap or
exclusive costs. Historical `draw_encode_cpu` aggregate fields remain available
but include warmup/capture and are not used for this comparison.

| Run | CPU/frame samples | Encode p50 / p95 / p99 ms | Frame p50 / p95 / p99 ms | GPU samples | Terrain GPU p95 ms |
| --- | ---: | --- | --- | ---: | ---: |
| [300 control](../../output/issue74/vertex-pages/control300/static.json) | 952 | 0.524 / 0.874 / 1.190 | 7.584 / 9.882 / 10.335 | 712 | 7.041 |
| [300 page binds](../../output/issue74/vertex-pages/pages300/static.json) | 952 | 0.497 / 0.885 / 1.240 | 7.605 / 9.784 / 10.701 | 712 | 7.058 |
| [1000 control A](../../output/issue74/vertex-pages/control1000-a/static.json) | 440 | 5.937 / 9.128 / 10.520 | 15.443 / 20.210 / 22.773 | 425 | 15.131 |
| [1000 page binds](../../output/issue74/vertex-pages/pages1000/static.json) | 492 | 4.927 / 6.530 / 7.846 | 13.957 / 16.948 / 20.561 | 371 | 14.022 |
| [1000 control B](../../output/issue74/vertex-pages/control1000-b/static.json) | 473 | 5.463 / 7.530 / 9.001 | 14.576 / 18.292 / 20.451 | 354 | 14.624 |
| [1000 page binds B](../../output/issue74/vertex-pages/pages1000-b/static.json) | 462 | 5.550 / 8.311 / 9.514 | 14.793 / 19.419 / 21.661 | 440 | 14.703 |
| [1000 control C](../../output/issue74/vertex-pages/control1000-c/static.json) | 417 | 4.964 / 8.211 / 10.417 | 16.452 / 20.619 / 22.955 | 314 | 16.316 |
| [1000 page binds C](../../output/issue74/vertex-pages/pages1000-c/static.json) | 482 | 5.017 / 6.580 / 7.596 | 14.162 / 17.161 / 19.173 | 360 | 13.772 |

At 300 m, 542 of 1,234 eligible adjacent owner binds disappear, while 1,339
draw calls and 2,818,749 triangles remain unchanged. Encode p50 falls only
0.026 ms, frame p95 falls about 1%, and both encode/frame p99 rise. This pair
does not meet either the 0.5 ms encode saving or 10% whole-frame p95 screen.

At 1 km, 3,036 of 10,721 eligible owner binds disappear; 10,826 draws and
5,257,933 triangles remain unchanged. The median of the three encode p50 values
is 5.463 ms control versus 5.017 ms candidate, a 0.446 ms reduction; this does
not independently meet the 0.5 ms CPU screen. The median frame p95 is
20.210 ms control versus 17.161 ms candidate (15.1% lower), passing the
alternative 10% whole-frame p95 screen. Median frame p99 improves from
22.773 ms to 20.561 ms (9.7%). These are medians of per-run percentiles,
not percentiles of a pooled frame distribution.

The runs alternate control/candidate three times, without randomization. There
is substantial variation: control frame p95 spans 18.292–20.619 ms and candidate
16.948–19.419 ms; candidate B is slower than control B. The candidate's
consistent images, reduced command count and median timing direction support a
bounded keep decision, with modest confidence. This is not an isolated estimate
of vertex-binding cost: GPU terrain timings also vary, and encode timing includes
the other work stated above. Repeat across independent sessions and motion
before extending the performance claim.

An [earlier 1 km control](../../output/issue74/distance-current/1000/static.json)
recorded encode p50 4.772 ms and frame p95/p99 17.116/18.909 ms over 217 frames.
It used a different executable (`b40bded5…`) and shorter requested measurement,
so it is context for run variation, not a pooled causal control. Its encode p50
is lower than this experiment's candidate median. A universal 0.5 ms encode
saving is not established. The 1 km candidate median frame p95 of 17.161 ms
remains above the 16.67 ms whole-frame target.

## Equality, memory and remaining gate

Decoded RGB bytes and `static.linear_rgb.f32` files are identical within each
distance comparison, including all six 1 km trials. The decoded-RGB SHA256 is
`158e5d03a5af1de7011e129a2bfe2452058437b03122868a92cb3a7543f61ce0`
at 300 m and
`417901b76613a62e53698e64655e5a7cbe9188cd2eb5ec56f3979c9bf485308b`
at 1 km. All compared runs retain the same visible owner counts, triangle totals and
native-bin work. RGB equality alone does not certify depth equality or motion.

GPU allocated memory is unchanged: 142,606,336 bytes at 300 m and
369,098,752 bytes at 1 km. Resident allocation bytes grow by 992 bytes from
padding at each distance; profile and proxy payload allocations remain unchanged.
This run does not establish memory behavior for a different packing/streaming
history or a capacity-bound scene.

The 4× MSAA captures do not expose depth. A separate matched 300 m, 1× MSAA
[control](../../output/issue74/vertex-pages/control-depth/static.json) and
[candidate](../../output/issue74/vertex-pages/pages-depth/static.json) pair
supplies the [color/coverage/depth parity report](vertex-depth-parity.json):
zero different RGB pixels, zero coverage disagreements, 967,122 common-hit
pixels, and zero maximum/p99 normalized depth error. This is a separate
bounded correctness pair, not additional samples for the MSAA4 timing table.
The CPU address/allocation tests exercise record-byte equivalence, native-bin
ranges, generic fallback, padding/release accounting and invalid addresses;
they complement GPU comparisons rather than replacing them.

Decision: retain the bounded adjacent vertex-page submission path after required
tests pass, with default-on behavior and explicit environment `0` fallback.
The reason is the 1 km median whole-frame p95 improvement above the provisional
10% screen, with improved median p99, identical compared color/depth, and no
allocated GPU-memory increase. Keep this claim limited to the tested 1 km
stationary fixture on this adapter. The 300 m pair establishes parity and a small
encode change, not a meaningful speedup. Further randomized sessions, different
packing/capacity histories and motion remain unmeasured. Other experimental flags
stay independent; these results do not justify GPU-driven culling, a renderer
rewrite, 4–20 km acceptance, or a completed laptop-60-FPS gate.
