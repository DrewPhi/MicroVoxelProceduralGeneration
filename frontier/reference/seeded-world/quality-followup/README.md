# Source-v8 rendering quality follow-up

The canonical source is unchanged by this bound comparison: seed 19, source 8,
scale 1800 m, amplitude 1000 m, eight octaves, roughness 0.5, native cells
0.0625 m. Each 512-cell tile uses 64-cell fan-grid spacing. These are three
actual offenders from the grounded turn-return route, not selected source
samples promoted into certificates.

| Tile minimum (native X,Z) | Previous affine error (m) | Tightened affine error (m) |
|---|---:|---:|
| -17920, 3072 | 51.220974054 | 4.520060546 |
| -17408, 2560 | 41.152281552 | 4.418246126 |
| -14848, 4096 | 15.855113938 | 4.357041635 |

[Machine-readable results](weighted-bounds.json) also preserve visible-height
ranges and identical native center tops. The previous model independently
bounded each normalized weight, losing the shared denominator. The new model
intersects that enclosure with the extrema of a positive interval-weighted
mean. For fixed target endpoints the derivative in a weight has the sign of
`target - mean`; sorting targets therefore leaves only n+1 endpoint-threshold
assignments. Outward interval arithmetic encloses those assignments. Canonical
roundoff reserves remain in place. Native height queries construct the proxy
vertices only; they do not certify unsampled source extrema.

The Rust [probe](weighted_bound_probe.rs) runs independently of renderer/GPU
state. To reproduce the current model from `frontier/`:

```sh
source ../../.toolchains/env.sh
rustc --edition=2021 --crate-name frontier_world --crate-type rlib world/src/lib.rs -C opt-level=2 -o /tmp/libfrontier_world_probe.rlib
rustc --edition=2021 reference/seeded-world/quality-followup/weighted_bound_probe.rs --extern frontier_world=/tmp/libfrontier_world_probe.rlib -O -o /tmp/weighted-bound-probe
/tmp/weighted-bound-probe
```

To reproduce the previous bound model while keeping source 8 and the newer
conservative distance/width proofs, copy `world/src/` into a temporary directory
and remove only the three statements beginning `let normalized =
positive_weighted_mean_range`, `ground.lo = ground.lo.max(normalized.lo)` and
`ground.hi = ground.hi.min(normalized.hi)` from its `affine_bounds.rs`. Compile
that copy's `lib.rs` with the commands above. The unused helper may remain.
Do not alter the canonical geography, source parameters or proxy vertices.

Detailed maps now integrate the independent fourth water run, including the
exposed bed/soil/rock spans. Old three-band Hierarchy/Visibility experiments
retain their explicit Baseline fallback. Separate descriptor-derived river/lake
spacing debt requests finer display sampling where the core can intersect;
CPU selection and GPU current-draw diagnostics assess the same debt. It never
replaces the vertical geometry certificate. The codec representation is 14.
Ocean shoreline area filtering and full pixel-footprint water coverage remain
separate acceptance gates.

The first hardware Legacy regression used the fixed 1920x1080 primary scene
with the 300 m clip, exact source/camera and float32 capture. Before/after
buffers were bit-identical (maximum absolute difference 0, RMSE 0). The paired
image was inspected; the pre-existing smoothing/faceting remains. Artifacts:
`output/live-water/primary-{before,after}/` and the independent native reference
`output/naadf-next-primary/reference256/`.

The initial source-v8 Map aerial reached requested 5 px quality in 80.77 s at
6 km and 1920x1080 on Radeon 760M. A first turn-return motion run retained
coverage for all 183 measured frames but had zero quality-ready frames. Its
50.46/54.79/57.68 ms p50/p95/p99 overlapped test compilation and is not an
isolated runtime comparison. That run precedes the weighted-mean tightening
and the descriptor spacing debt. Final quiet captures are required before
claiming a motion-quality or performance improvement.

## Final quiet GPU check

[Curated machine-readable results](quiet-rendering.json) record the final
source-8/representation-14 build on Ryzen 5 7640U / Radeon 760M. Compilers were
paused throughout these captures. Both use 1920x1080, MSAA1, 64 m NAADF, 6 km
clip, 5 px requested error, Map appearance, no vegetation and no disk cache.

The matched aerial reached quality in **27.35 s**, with **4.99938 px** maximum
visible error, **zero unresolved clusters**, 552,128 triangles and 4,949 builds.
This is a successful settled view. The before view used the same source and
camera but different cache/CPU contention, so the 80.77→27.35 s difference is
not an isolated speedup measurement. The actual before/after river crop at
`output/live-water/river-v8-before-after.png` was inspected: the after view
keeps the river more consistently narrow through the diagonal foreground and
retains the existing point-map surface pattern. These are real captures, not
an approved artistic target or a pixel-footprint filtering proof.

The quiet grounded turn-return route measured **584 frames** at
**21.218 / 24.008 / 25.847 ms** p50/p95/p99 (about 47 FPS median). Coverage was
ready for all 584 frames; the camera left NAADF on zero frames. Peak process
RSS was **809.5 MB**. Whole-render GPU timestamps were **19.727 / 21.680 /
22.190 ms** p50/p95/p99, measured asynchronously rather than paired per frame.

**Zero moving frames met requested distant quality.** Nineteen frames had an
unbounded conservative projection; the end view retained 146 unresolved
clusters and 162.66 px maximum error. Some far offenders carry grid-spacing
river debt much larger than the actual river width. A later per-descriptor
cap at actual water diameter should avoid refining subpixel cores while
preserving the explicit sampling debt. Neither that future change nor ocean
shoreline/water-area filtering is implemented here. Multi-seed motion, cold
teleport, long-coordinate routes and all-significant-distance ownership remain
broader acceptance work. This checkpoint does not close those gates.

Reproduce the final captures from `frontier/` with a hardware-capable process:

```sh
target/release/frontier --preset seeded-world --seed 19 --no-adaptive-disk-cache --walk off --eye '-2000,650,-1500' --target '-900,595,-693' --width 1920 --height 1080 --msaa 1 --naadf-radius 64 --capture-far 6000 --pixel-error 5 --capture-unresolved --capture output/live-water/seed19-map-v8-after.png
target/release/frontier --preset seeded-world --seed 19 --no-adaptive-disk-cache --start-xz '-896,256' --width 1920 --height 1080 --msaa 1 --naadf-radius 64 --capture-far 6000 --pixel-error 5 --route turn-return --route-clock realtime --route-speed 3 --seconds 15 --warmup 120 --adaptive-wait-quality --report output/live-water/walk19-map-v8-after.json --capture-on-exit output/live-water/walk19-map-v8-after.png
```
