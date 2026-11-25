# NAADF traversal and streaming diagnostics

`tools/naadf_traversal.py` renders a headless primary-ray diagnostic with
exact per-pixel root, block, and leaf visits. Misses are included. Each
iteration visits a root (or a sparse page and potentially its mixed root
word); a block or leaf visit is counted only when the ray descends to that
level. These are logical visits, not memory transactions or GPU timings.
The normal rendering pipeline compiles with counters disabled. The raw RGB
float attachment stores integer counts; use the generated heatmap rather
than the diagnostic PNG, whose display range clips the raw counts.

From `frontier/` with the release binary built, using NumPy and Pillow:

```sh
python3 tools/naadf_traversal.py --radius 64 --depth --output output/traversal-meadow
python3 tools/naadf_traversal.py --radius 128 --target=-39,76,0 --output output/traversal-mountain
python3 tools/naadf_traversal.py --radius 64 --sparse --output output/traversal-sparse
```

With `--depth`, the tool captures raster-only and voxel-only depth at
the same pose, with the same NAADF ownership mask. It counts raster hits
that could conservatively terminate a primary ray before its voxel hit or
the cube exit. This estimates the opportunity; it does not pay for a depth
copy, add a live GPU pass, or time the instrumented shader.

The opt-in fixed sparse atlas now encodes conservative six-direction skip
extents across uniform empty pages. Mixed pages stop expansion and keep
their existing exact root words. The renderer uses the same leaf geometry,
pigments, and shadows. `FRONTIER_NAADF_SPARSE_SHORT=1` restores the old
one-page skip for comparisons. Sparse roots still do not stream.

The walking benchmark records the last 64 published shifts as
`stream_stage_ms`: request to worker reply, reply to final upload submission,
reply to raster restoration, and request to publication. Upload and raster
restoration can overlap. These are wall-clock intervals, not GPU completion
timings. `FRONTIER_NAADF_LEAD_SECONDS=4` tests early requests using recent
camera velocity. Lead is capped below half a shift per axis; reversals,
teleports and the existing raster handoff still use the same coverage
rules. Lead is off by default.

## Diagnostic findings

Alpine seed 7, 6.25 cm voxels, Radeon 760M. These counters use 640×360 so
captures are quick; the timing sweeps below use 1920×1080 with full lighting.

| View | Radius | Root mean / p95 | Block mean / p95 | Leaf mean / p95 |
| --- | ---: | ---: | ---: | ---: |
| Meadow, dense | 64 m | 11.70 / 33 | 7.17 / 28 | 3.20 / 13 |
| Meadow, sparse short | 64 m | 23.80 / 39 | 7.17 / 28 | 3.20 / 13 |
| Meadow, sparse long | 64 m | 10.26 / 32 | 7.17 / 28 | 3.20 / 13 |
| Toward the mountains | 128 m | 13.38 / 37 | 6.12 / 29 | 2.58 / 13 |
| Uphill | 64 m | 14.42 / 31 | 8.72 / 22 | 4.54 / 12 |

The meadow's top third averages 4.88 root visits with no block visits.
The middle third averages 17.63 root visits and contains the hottest
grazing band. Both sparse versions had identical block and leaf counts to
dense at every pixel in that view. Neither of the meadow or uphill
captures found a conservative raster-depth cutoff among 230,400 primary
rays per pose, so the live renderer keeps its existing draw order.

[Meadow heatmap](reference/gallery/naadf-traversal/meadow.png) ·
[Mountain heatmap](reference/gallery/naadf-traversal/mountain.png) ·
[Uphill heatmap](reference/gallery/naadf-traversal/uphill.png) ·
[Measurements](reference/gallery/naadf-traversal/measurements.json).

## Matched timing runs

At a fixed 64 m radius, the longer-skip sparse atlas used **0.56 MiB**
for roots versus **8.00 MiB** dense, and the final linear RGB captures were
byte-identical. Median NAADF GPU time was **8.54 ms sparse vs. 6.82 ms
dense**. Frame p50/p95/p99 was 20.21/23.07/23.60 ms sparse and
18.23/21.10/22.19 ms dense. The short-skip atlas from the previous
checkpoint was much slower, but its timing was a separate run; the
matched evidence here for long vs. short is the per-pixel visit count.
Dense stays the default.

A 35-second 128 m stop/resume walk traveled about 81 m per run. Both
variants published five shifts with no frames outside exact coverage and
held the watched mountain tile width at 1,024 native cells.

| Lead | Frame p50 / p95 / p99 | Minimum exact margin | Shift stage medians: worker / upload / raster / total |
| --- | --- | ---: | --- |
| Off | 25.07 / 37.78 / 42.36 ms | 110.23 m | 1.169 / 1.579 / 0.063 / 2.781 s |
| 4 s | 25.22 / 38.00 / 42.83 ms | 116.36 m | 1.167 / 1.627 / 0.056 / 2.808 s |

Lead bought about six meters of minimum coverage without a frame-time or
shift-latency improvement in this test. Upload submission was the largest
median stage; prioritize it before making prediction the default.

```sh
python3 tools/naadf_radius_sweep.py --radii 128 --route walk-stop-resume --seconds 35 --watch-xz 1100,3100 --output output/lead-control --run
FRONTIER_NAADF_LEAD_SECONDS=4 python3 tools/naadf_radius_sweep.py --radii 128 --route walk-stop-resume --seconds 35 --watch-xz 1100,3100 --output output/lead-test --run
```

The fixed Legacy PNG, linear RGB, and depth are unchanged. The diagnostics
and experiments reuse Frontier's exact occupancy and AADF expansion; no
external code was copied (reuse tier G).

Validation: 437 renderer tests passed. Four separate Radeon GPU oracles
passed, including the sparse atlas and a wrapped moving window.

## Seven exact-acceleration experiments

See [matched experiments, correctness gates and results](NAADF_SEVEN_EXPERIMENTS.md)
for node reuse, upload batching/scatter, streamed skip quality, height bounds,
persistent/ray-guided caches and conservative beams. All are selectable experiments;
a passing correctness test is not a performance endorsement.
