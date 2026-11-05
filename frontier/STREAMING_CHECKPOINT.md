# Moving FBM surface checkpoint

Partial progress on #34/#36/#41; no G1–G4 production gate is closed. Greenfield
implementation, with Sol medium implementing and independently testing the code,
Luna auditing the ownership mathematics, and root running hardware validation.

## What changed

Seven levels now move independently on globally anchored grids. Four immutable
index variants per level accommodate the offset inner hole. A single background
worker coalesces camera requests, cancels distant obsolete selections at row
boundaries, and publishes coherent level sets. Useful content survives changes
in camera demand; the source is immutable for the lifetime of this worker.

Completed overlapping vertex samples are reused exactly; only entering strips
are evaluated. Unchanged levels retain their pages. Changed levels upload into
existing GPU buffers, without rebuilding pipelines or topology. The GPU still
receives a whole changed level, not toroidal strip uploads. CPU filtered samples
use a bounded 600,000-entry FIFO cache, plus bounded level/snapshot buffers.

The independently moving windows and transition regions follow the geometry
clipmap literature, adapted to the canonical CPU voxel source:
[Asirvatham and Hoppe, GPU Gems 2](https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry).
No external or legacy implementation was copied.

An initial shared-center attempt failed the real-time test despite fast frames:
terrain fell over 300 m behind. Independent levels alone still wasted overlapping
sample work. Direct sample reuse removed that cost. This is why generation lag
is recorded separately from frame time.

## Reproduction and measurements

Build release, then run `./frontier/tools/smoke_streaming.sh`. Reports, logs and
end-of-route PNGs are in ignored `frontier/output/streaming-checkpoint/`.
`--route-clock realtime` advances at wall-clock speed; the existing frame-based
mode remains for deterministic image comparisons. `--seconds`, `--route-speed`,
`--capture-on-exit` and `--terrain-static` support repeatable comparisons.

Hardware: Ryzen 5 7640U / Radeon 760M, RADV PHOENIX Vulkan, Mesa 25.2.8.
1920×1080 internal/output resolution, seed 7, source version 1, 6.25 cm canonical
voxels, scale 1800 m, amplitude 1000 m, 8 octaves, roughness 0.5. Display is a
surface proxy: seven levels at 0.5–32 m spacing, 180,224 submitted triangles,
9,318,960 bytes of GPU sample pages, seven terrain draws. No haze or temporal AA.

Final 15-second uncapped real-time routes at 80 m/s, first 30 frames excluded:

| Route | p50 ms | p95 ms | p99 ms | Max camera/center distance | End distance | Updates |
|---|---:|---:|---:|---:|---:|---:|
| Stop–resume | 1.559 | 3.047 | 4.577 | 14.88 m | 4.54 m | 788 |
| Turn–return | 1.482 | 2.528 | 6.034 | 22.69 m | 0 m | 141 |

Both had zero measured intervals over 16.67 ms and zero cancelled builds. The
finest square extends 32 m from its center; camera distance measures horizontal
Euclidean distance, not a proof of visible surface error. The stop–resume run
reused 91.3 million vertex samples, uploaded 2.53 GB cumulatively, and ended with
an estimated 86.3 MB CPU sample/page cache. Cold preparation was 317–597 ms across
these runs. Estimates exclude allocator overhead, topology, driver allocations
and total process memory; they are not RSS/VRAM measurements. Upload CPU time is
queue submission overhead, not GPU transfer timing.

These are short, presentation-inclusive frame measurements, not isolated GPU
timings, thermal endurance results, or final voxel-engine performance. The
turn–return route finishes moving at five seconds; its remainder tests settling.

Additional 10-second real-time straight-flight tests:

| Speed | p95 / p99 ms | Max / end center distance | Updates / cancellations | Result |
|---|---:|---:|---:|---|
| 35 m/s | 3.028 / 4.715 | 6.77 / 1.27 m | 387 / 0 | Ordinary travel keeps up |
| 240 m/s | 2.183 / 5.552 | 1950 / 1950 m | 42 / 77 | Streaming fails despite fast frames |

The interactive Shift speed is therefore 80 m/s for this checkpoint (ordinary
speed 35 m/s). `--route-speed 240` preserves the failing stress case. Faster
flight needs predictive/coarse coverage and better scheduling rather than simply
relaxing cancellation and calling old distant detail acceptable. No G4 claim.

## Correctness and remaining gates

All 45 workspace tests pass; the release build, rustfmt and whitespace checks
pass. The inspected normal-speed, stop–resume and turn–return PNGs show complete
terrain without obvious open tile edges. They still show coarse facets, simple
green/rock coloring and smooth nearby surfaces. Stills do not establish invisible
LOD transitions during motion.

Regression coverage includes all four inner-hole offsets at every level,
negative coordinates, upward winding, total area, paired internal edges, exact
boundary height/color/normal agreement, row cancellation, teleport supersession,
and byte-for-byte equality of incremental versus fresh samples across collapsed
edges. Validity of cached content is independent of the camera epoch.

This checkpoint remains a smooth, heightfield-only renderer. It does not provide
nearby exact voxel ownership, caves or edited volume coverage, a screen-space
error bound, invisible temporal LOD changes, or a production teleport contract.
There is no collision controller. Fast travel can still outrun generation; old
coherent coverage stays visible while replacement work runs. Partial GPU strip
uploads, predictive demand and conservative source/error summaries remain useful
next optimizations, to be evaluated against measurements.

The next correctness checkpoint is the fixed-boundary native/heightfield ownership
fixture in [HANDOFF_PLAN.md](HANDOFF_PLAN.md). Do not conceal that boundary with
blur or imply that this broad smooth proxy already satisfies voxel appearance.
