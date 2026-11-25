# NAADF optimization and walking stability

See [traversal and streaming diagnostics](NAADF_TRAVERSAL.md) for the follow-up
GPU heatmaps, sparse long skips, depth study and upload timings.

The default moving NAADF window uses exact compact leaves when every pigment
fits, CPU root distance fields, and a mesh handoff that keeps the published
tile partition while boundary ownership changes. It keeps the same 6.25 cm
voxels, lighting settings and screen-space terrain error target.

## Stable mesh handoff

Keep the committed closed-edge tile keys as the first target for the new
ownership certificate. Raster tiles that are still needed keep their
identity and already prepared payload. Restore retiring tiles from the
bounded cache or rebuild only what is missing. Reject stale worker results
and keep presenting the previous partition until the replacement is coherent.
Normal camera coverage guards and screen-error hysteresis still apply.

The radius sweep's `--watch-xz X,Z` reports the published width of one
fixed world-space tile and the NAADF owner at each change. The full frame
trace also reports the current owner and watched tile width. These counters
test whether a particular mountain region changes level; they do not certify
that every tile or material stays unchanged under arbitrary motion.

## Exact storage and ray experiments

- **Compact leaves:** Two 16-bit leaves share a GPU word. Solid leaves use
  the same palette IDs and empty leaves retain all six skip distances.
  A wide palette uses 32-bit leaves; streaming can rebuild wide before
  publishing a strip whose palette would overflow. Deduplication shares
  identical leaves across incoming strips within a single publication.
- **CPU working set:** Snapshot deduplication indexes fingerprints and
  compares against the voxel array before sharing a leaf. It does not keep a
  second copy of every leaf in the index. After installation the streaming
  worker retains only mixed-root allocation metadata; GPU roots still cover
  the entire cube. The first build still has dense root and scratch allocations.
- **Sparse GPU roots (opt-in, fixed window only):** 4³ homogeneous pages
  skip as a unit; mixed pages retain exact root words. An atlas replaces the
  dense root texture only when it is smaller. Try
  `FRONTIER_NAADF_SPARSE_ROOTS=1 ./run.sh --preset alpine --naadf-radius 64 --naadf-follow off --msaa 1`.
  This version does not stream sparse GPU pages or combine with GPU distances.
- **GPU distances (opt-in):** `--naadf-gpu-distance on` publishes exact roots
  before constructing longer empty-space skips. It fills a bounded cache in
  32³ tiles on the graphics queue. A committed shift invalidates the cache.
  Cold or uncached roots fall back to one-chunk steps. This is slower at large
  radii on the Radeon 760M and stays off by default.
- **Exact ray early-outs:** Limit the draw to the conservative screen bounds
  when the cube is in front of the camera; retain the full viewport if it
  crosses the eye plane. An exact shadow query stops at its first blocker
  without reconstructing a face. Raster receivers skip the voxel shadow
  query if raster shadows already prove zero visibility. A raster-depth
  cutoff for primary rays was not implemented; it would need a separate
  sampled depth path and a comparison of its copy/pass cost.
- **Shadow timings:** `--ray-profile primary|voxel-shadows|mesh-shadows|full`
  on the sweep isolates the cost of each NAADF shadow query family by
  disabling the others. These are ablations, not production quality modes.
  GPU timings are asynchronous and deltas are comparative, not additive.

The background construction and packed leaves follow the methods in
[Ulschmid et al., sections 3.1–3.5](https://onlinelibrary.wiley.com/doi/full/10.1111/cgf.70413).
The sparse atlas and hybrid handoff are local experiments; no external code
was copied (reuse tier G).

## Reproduction

From `frontier/` using the built release launcher:

```sh
python3 tools/naadf_radius_sweep.py --radii 96 --route walk-stop-resume --seconds 35 --watch-xz 1100,3100 --output output/naadf-walk --run
python3 tools/naadf_radius_sweep.py --radii 32 --route walk-turn-return --seconds 8 --output output/naadf-return --run
python3 tools/naadf_radius_sweep.py --radii 32 --route walk-stop-resume --seconds 32 --eye=-900,114.6875,1000 --target=-900,119.6875,1100 --output output/naadf-climb --run
python3 tools/naadf_radius_sweep.py --radii 64 --root-format sparse --output output/naadf-sparse --run
```

The sweep holds 1080p, 6 km clip, 5 px screen error, 1× MSAA, ordinary
Alpine lighting and vegetation, and seed 7 unless an option explicitly
changes one of them. Frame-time percentiles include presentation. GPU
allocation and process RSS are separate measurements and should not be added.

## Results on the Radeon 760M

| Alpine walking run | Frame p50 / p95 / p99 | NAADF GPU allocation | Peak process RSS |
| --- | --- | ---: | ---: |
| 96 m, previous build | 31.68 / 44.78 / 64.18 ms | 318.40 MiB | 2,010.83 MiB |
| 96 m, current defaults | 26.26 / 39.07 / 43.95 ms | 188.22 MiB | 1,793.53 MiB |
| 32 m, previous build | 17.42 / 35.86 / 44.20 ms | 37.44 MiB | 1,996.04 MiB |
| 32 m, first integrated defaults | 26.47 / 43.71 / 50.45 ms | 35.08 MiB | 2,179.44 MiB |
| 96 m, experimental GPU distances | 77.13 / 97.69 / 106.78 ms | 196.47 MiB | 2,052.88 MiB |

The 96 m current build moved 81.24 m with six cube shifts and zero tracked
frames outside the exact volume. The tile containing (1100, 3100) m stayed
1024 native cells wide (64 m) across the walk. In the 32 m matched route,
holding mesh quality increased visible cluster count from 1,683 to 3,448 at
the end of the run. The stability change does not improve every frame-time
case; the earlier renderer often displayed a coarser cover.

The short turn/return and the foothill climb (15.2 m ascent) each kept exact
coverage. Starting far above the foothill could previously exhaust the arena
and stop the benchmark; the guarded follow-mode reserve let it complete and
continue streaming, though the fast vertical drop briefly outran the cube.

The cold/warm GPU distance prototype matched canonical geometry before and
after a signed wrap, but it spent about 64 ms in the NAADF pass at 96 m versus
11 ms with CPU distances. The cache is bounded to a central region; uncached
space loses the longer CPU skips. The timing is a reason to keep the
prototype off, not a reason to drop exact voxel coverage.

Four explicit Radeon GPU oracle modes (compact, forced-wide, cold/warm
distance cache with ring wrap, and sparse roots) matched canonical coverage,
hit depth, material and shadow samples. The renderer suite passed 435 tests.
The fixed legacy PNG, linear color and depth remained byte-identical in the
integrated handoff/ray regression check.

[Walking frame](reference/gallery/naadf-optimization/walk-96m.png) ·
[Climb frame](reference/gallery/naadf-optimization/uphill-32m.png) ·
[Raw measurement summary](reference/gallery/naadf-optimization/measurements.json).

The 256 m guarded capacity run published at default memory settings. Its
35-second walk completed at 33.81 / 44.92 / 50.30 ms with 1,596 MiB of NAADF
GPU allocation and 4,091 MiB peak RSS. A previous successful 256 m run used
2.41 GiB of GPU allocation and 6.47 GiB peak RSS; it used a longer route and
the old manual budget, so use this as memory context rather than a matched
FPS comparison. Cold build is still slow (166 seconds). One strip published,
with another in flight at the end of the 81 m run. The camera remained at
least 206.8 m inside the cube; this does not establish indefinite tracking at
256 m. The 96 m route is the stronger playtest checkpoint.

The 128 m run followed the same 81 m route at 25.44 / 37.83 / 41.36 ms,
published five shifts, and stayed inside exact coverage. It took 43.84 s to
build, allocated 343 MiB of NAADF GPU storage and peaked at 2,084 MiB RSS.

The fixed 64 m shadow ablation used the same camera and settings:

| Enabled NAADF shadow queries | NAADF GPU pass p50 | Full GPU frame p50 |
| --- | ---: | ---: |
| Neither | 7.31 ms | 19.12 ms |
| On exact voxel receivers only | 8.14 ms | 19.50 ms |
| On mesh receivers only | 7.38 ms | 19.27 ms |
| Both | 8.41 ms | 20.07 ms |

This view spends more time on primary traversal and shading than on the
two NAADF shadow query families. The mesh query runs in raster passes and
does not appear directly in the NAADF timestamp. These four runs cannot be
subtracted as if they were timestamps from the same frame.

At the same fixed 64 m view, sparse roots reduced the actual root texture
from 8 MiB to 0.5625 MiB and total NAADF allocation from 55.86 to 48.42 MiB.
The NAADF GPU pass rose from 8.41 to 21.42 ms. Uniform pages lose the dense
root's longer skip hints, and mixed pages add a lookup. This atlas is a
memory experiment, not the default. Forced wide leaves used 95.12 MiB total
versus 55.86 MiB packed. The wide, packed, and sparse captures produced
byte-identical 1080p linear RGB at this camera.
