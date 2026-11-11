# Exact NAADF acceleration experiments

These experiments preserve the 6.25 cm canonical world and the current
terrain, vegetation, materials, lighting and hybrid ownership. They investigate
performance, not a new visual target. Experimental switches are independent;
their defaults and measured acceptance are documented below when tested.

## Fair comparison contract

- Freeze the original executable and record its SHA-256. Compare the integrated
  executable with every experiment disabled before judging candidates.
- Use Alpine seed 7, 1080p, MSAA1, pixel error 5, 6 km far clip and full lighting.
  Verify that certified interior raster meshes remain omitted. Boundary and
  distant raster costs are part of the full renderer, not discarded overhead.
- Measure fixed views separately from 128 m stop/resume and turn/return routes.
  Keep exact radius, source revision, camera and sampling matched. Reverse repeat
  order to expose thermal effects. Do not run builds or other benchmarks alongside
  GPU timing runs.
- Include acceleration construction, transfers, feedback readbacks and publication
  in end-to-end frame/streaming measurements. A fast ray pass with expensive
  preparation is not automatically a win. GPU pass timestamps exclude some work;
  their scope must be reported.
- Compare frozen linear RGB and depth; use canonical CPU/GPU ray and shadow
  oracles, including wrapped windows. A ray-count heatmap is diagnostic only.
- Test each method alone, then promising combinations. Remove superseded work;
  do not judge an experimental representation while unnecessarily building and
  rendering a second representation of the same interior.
- Separate correctness failures, incomplete integration, memory savings, and
  measured speed changes. A losing first implementation does not disprove the
  underlying method.

## Experiments

1. **Per-ray hierarchy reuse:** retain root/block words until crossing their
   containing cell boundary. Compile out the state when disabled; check register
   pressure indirectly through measured throughput.
2. **Upload batching:** account for bytes, queue writes, unused allowances and
   byte/call-limit frames. Merge adjacent disjoint arena writes and share budget
   with staged root uploads. Root mapping is still published atomically only
   after all new data and fallback coverage are ready.
3. **Fresh versus streamed acceleration:** compare identical occupancy at the
   same final region after actual strip updates versus a fresh full build.
   Independent strip bounds may safely but unnecessarily shorten empty skips.
4. **Occupied-height bounds:** reduce *all* occupied/mixed canonical roots,
   including vegetation and edits. Skip only certified air above these bounds,
   then resume exact traversal. Tile sizes trade bound tightness against work.
5. **Persistent distance cache:** use world-aligned tiles and retain proofs only
   where their world-space support is unchanged. Reset and retention comparisons
   use the same layout. A world revision rebuilds the enclosing volume.
6. **Ray-guided cache work:** prioritize optional skip-cache improvements using
   view demand/feedback while preserving exact fallback geometry everywhere.
   Include the probing, scheduling and asynchronous readback costs.
7. **Conservative beams:** prove an empty prefix for all rays in an 8×8 screen
   tile once, then run exact per-pixel intersections. This is not reduced image
   resolution or interpolation of hits.

## References and provenance

Greenfield adaptations (reuse tier G); no external source blocks copied.

- Ulschmid et al., [NAADF, §§3.1–3.5](https://onlinelibrary.wiley.com/doi/full/10.1111/cgf.70413):
  shallow hierarchy, conservative cuboid proofs, asynchronous improvement and
  dependency-aware invalidation.
- Tevs et al., [Maximum Mipmaps](https://pure.mpg.de/rest/items/item_1325622_5/component/file_3590464/content),
  [author implementation](https://github.com/cgart/i3d08.research): inspiration for
  coarse upper bounds; our bounds include arbitrary occupied voxels, not just
  the terrain heightfield. Both flat tiles and a maximum-mipmap variant are
  implemented. The latter reduces the same base bounds rather than rescanning
  canonical roots at every level.
- Laine and Karras, [Efficient Sparse Voxel Octrees, technical report](https://users.aalto.fi/~laines9/publications/laine2010tr1_paper.pdf):
  conservative beam acceleration; our beam proof uses occupied-height tiles.
- Crassin et al., [GigaVoxels](https://www.icare3d.org/research-cat/publications/gigavoxels-ray-guided-streaming-for-efficient-and-detailed-voxel-rendering.html):
  rendering feedback guides work; here only optional acceleration is prioritized,
  so missing feedback does not remove exact geometry.

## Reproduction tools

`tools/naadf_experiments.py` runs a JSON mapping of names to explicit
`FRONTIER_NAADF_*` environment values. It clears inherited NAADF flags, records
the binary hash and effective environment, and runs candidates sequentially.
`--mode timing` uses the complete radius-sweep renderer and records streaming
telemetry. `--mode capture` checks frozen linear color/depth against the first
variant. Plans are the default; add `--run` to execute.

Existing baseline and prior measurements remain in `NAADF_TRAVERSAL.md`.

## Correctness discoveries

The targeted two-root corner fixture exposed an important distinction: changing
the preceding empty cuboid can change face selection at a simultaneous grid-plane
crossing. On the Radeon driver the accelerated path could also round back into
the previous cell and terminate for lack of numerical progress before finding
the hit. Hit-only tie handling was therefore insufficient. Height/beam variants
now mark such stalls and replay the original traversal, as well as replaying
ambiguous hit boundaries. Ordinary completed sky misses do not require a replay.
The frozen first experimental binary is not accepted for height/beam performance
conclusions. The boundary regression checks primary hits and shadow visibility.

## Streaming proof diagnostic

The real CPU strip-update code was compared against full fresh builds of the
same final 16 m-radius volume (Legacy seed 7, amplitude 8 m, scale 36 m, three
octaves). For each of three shifts, 6,144 cardinal rays reached identical first
occupied/mixed roots with fresh bounds, streamed bounds and unaccelerated root
steps. Streamed root visits were 14.3%, 21.2% and 17.2% higher than fresh visits.

This establishes a loss of skip reach, not a measured full-frame GPU slowdown.
The diagnostic stops at the first mixed/solid root; it does not count subsequent
block/leaf work. Strip builds were 18–23 ms versus 85–86 ms for fresh construction
in this small fixture, with warmed canonical-source caches possible. Rebuilding
the world on every shift is therefore not the proposed remedy.

## Experimental controls

All controls below are opt-in unless a measured promotion is explicitly recorded.

| Environment variable | Values / purpose |
| --- | --- |
| `FRONTIER_NAADF_CACHE_NODES` | `1`: coordinate-key cache; `2`: compact scalar-key cache |
| `FRONTIER_NAADF_BATCH_UPLOAD` | `1`: combine adjacent arena writes and share frame budget |
| `FRONTIER_NAADF_SCATTER_UPLOAD` | `1`: pack disjoint ranges into reusable staging and scatter on GPU |
| `FRONTIER_NAADF_UPLOAD_MIB` | 1–64, default 8; payload budget per streaming frame |
| `FRONTIER_NAADF_UPLOAD_CALLS` | 1–512, default 64; streaming queue-call budget |
| `FRONTIER_NAADF_HEIGHT_TILE_ROOTS` | 1, 4 or 8; enable flat occupied-height bounds |
| `FRONTIER_NAADF_HEIGHT_MIPS` | `1`: maximum pyramid; default base tile 8 roots |
| `FRONTIER_NAADF_HEIGHT_GATE` | `1`: bounds only on empty roots, and only accept a longer skip than the existing proof |
| `FRONTIER_NAADF_BEAM` | `1`: 8×8 screen-tile empty prefixes, sharing base height data |
| `FRONTIER_NAADF_DISTANCE_SCHEDULE` | `reset`, `retain`, `rays`, `feedback`; requires `--naadf-gpu-distance on` |
| `FRONTIER_NAADF_DISTANCE_CPU_FALLBACK` | `1`: retain CPU proofs; GPU replacement must contain them |
| `FRONTIER_NAADF_FIXED_CENTER` | `x,y,z` meters; diagnostic initial center with follow off only |

`rays` ranks tiles using 25 deterministic camera rays. `feedback` adds sampled
GPU root traversal and bounded asynchronous readback; it may underestimate demand
after mixed roots. Neither controls exact geometry residency or hit correctness.
Cache reset/retain/ranking comparisons use the same world-aligned tile layout.
The CPU-fallback case includes both CPU construction and GPU improvement work.

## Initial integrated measurements

Radeon 760M / RADV, 1920×1080, Alpine seed 7, 6.25 cm native voxels, full
lighting, atmosphere on, MSAA1, pixel error 5. These are first-pass measurements,
not a general ranking across scenes. The seven-method initial binary is frozen
by SHA-256 in the measurement records. Both original and integrated controls
have zero certified-interior raster clusters retained.

At a fixed 64 m radius, the original hints were not FPS wins:

| Candidate | Frame p50 ms | NAADF span p50 ms |
| --- | ---: | ---: |
| Integrated control | 18.10 | 7.00 |
| Coordinate-key node cache | 18.70 | 7.53 |
| Flat 4 m height tiles | 21.27 | 9.82 |
| Flat 8 m height tiles | 21.02 | 9.53 |
| Height mip hierarchy | 23.78 | 12.48 |
| Shared beam prefix | 19.65 | 7.92 |
| Coordinate cache + mip hierarchy + beam | 24.56 | 13.10 |

The span includes optional height/beam compute and render-pass setup, consistently
for control and candidates. It is not directly comparable to the earlier narrower
NAADF timestamp in `NAADF_TRAVERSAL.md`. Full-frame timing remains authoritative.
All ten fixed meadow captures, including additional combinations, were byte-equal
in linear RGB and depth. The Legacy PNG, linear RGB and depth were also unchanged.

This motivated two controlled refinements rather than rejecting the techniques:
compact cache keys reduce live state; gated height tests avoid mixed roots,
reuse rejected fine-tile bounds, and cannot replace a longer existing root skip
with a shorter height skip. Those have separate switches and measurements.

The 128 m, 35-second stop/resume upload test revealed fragmentation:

| Upload policy | Frame p50 / p95 / p99 ms | Median upload submission ms | Median total publication ms |
| --- | --- | ---: | ---: |
| Original 64-call policy | 25.23 / 37.00 / 41.79 | 1,609.6 | 2,828.4 |
| Contiguous batching, same limits | 25.71 / 37.87 / 41.48 | 1,517.2 | 2,895.0 |
| Contiguous batching, 256-call allowance | 25.71 / 37.92 / 41.58 | 375.7 | 1,800.4 |

Each published five shifts and reported zero frames outside exact coverage.
The control hit the call limit on 278 upload frames but the byte limit only once.
Both runs uploaded the same 20,971,520 root bytes and 64,598,160 arena bytes.
Contiguous batching removed only 29 of 17,933 arena writes: the holes are real.
The larger call allowance is explicitly a resource-policy change. Scatter upload
is the subsequent algorithmic experiment: one packed transfer includes descriptors
inside the same byte allowance and avoids the individual destination writes.

At 64 m on the turn/return route, retention preserved 96 tile instances and
reduced submitted tile builds from 146 (reset) to 96. Ray ranking submitted 95;
GPU feedback submitted 94, with 24 probes, 23 accepted readbacks and 19 measured
priority selections. All variants retained exact coverage. However, the CPU
control's frame p50 was 16.11 ms versus 19.60 / 20.36 / 20.54 / 20.75 ms for
reset / retain / rays / feedback. Reducing maintenance did not overcome the extra
cache lookup path. Differences between these four GPU-cache policies are not
statistically established from this single run.

The turn/return route moves and turns in its first five seconds, then settles;
the 35-second median must not be represented as continuous walking. During that
first interval, frame p50 was 19.25 ms for CPU control versus 22.91 / 25.64 /
26.04 / 26.09 ms for the four GPU-cache policies. CPU fallback proofs were
enabled in every GPU-cache case. Separate cache build/feedback submissions are
included in frame pacing and startup/stream intervals, but not in the isolated
ray-pass timestamp. These results concern this integration, not the general
value of persistent or ray-guided acceleration.

## Reproduce the final variants

Build the release launcher with the repository's normal `./run.sh` workflow.
Configurations are checked in under `reference/naadf-seven/`; output directories
must be new. Run these sequentially on an otherwise idle GPU:

```sh
python3 tools/naadf_experiments.py --binary target/release/frontier --config reference/naadf-seven/refined.json --output output/naadf-refined --mode timing --radius 64 --seconds 10 --run
python3 tools/naadf_experiments.py --binary target/release/frontier --config reference/naadf-seven/scatter.json --output output/naadf-scatter --mode timing --radius 128 --seconds 35 --route walk-stop-resume --repeat 2 --run
python3 tools/naadf_experiments.py --binary target/release/frontier --config reference/naadf-seven/refined.json --output output/naadf-refined-images --mode capture --radius 64 --run
```

For the initial height/cache/beam controls use `initial.json`, for contiguous
upload controls use `uploads.json`, and for the distance cache policies use
`distance.json`. Keep all policies on the same executable when comparing them.
The harness records the actual expanded CLI, environment and binary hash.

Scatter is opt-in and does not increase the allowance: descriptors and payload
share 8 MiB per streaming frame. It adds one reusable 8 MiB GPU staging allocation
and an 8 MiB CPU packing reserve per active volume at that allowance. New data
remains unreachable until queue-ordered publication; cancellation does not expose
partially scattered contents. Existing arena holes are not copied over. These
allocations, packing time, actual queue calls and dispatch counts are reported.

The final release suite passed 448 tests (21 explicitly ignored hardware or
long-running tests). Separate Radeon hardware runs passed the scatter sentinel,
default/scatter shifted canonical-volume oracles, the corner regression, and all
nine traversal configurations. Earlier dedicated hardware tests also validated
cache retention across multiple wraps and the feedback/fallback path. Hardware
oracles cover hit/depth/material and shadow equivalence, not just plausible PNGs.
No quality or memory knob was silently relaxed to obtain a timing improvement.

Packing CPU time covers packet construction only. Driver upload staging is not
part of explicit GPU-buffer counters; whole-process RSS and full-frame timings
remain necessary alongside those counters. GPU cache and scatter submissions
precede the render command buffer and are excluded from its isolated timestamp.

## Final scatter result (reversed-order repeat)

Frozen final binary SHA-256:
`974421264c502a8eb5bcd7b8a9880dc9e7d9a6d42cb543c2556ffddbd0e7595d`.
Same 128 m radius, 1080p, 35-second stop/resume route and 8 MiB allowance:

| Order | Policy | Frame p50 / p95 / p99 ms | Median upload submission ms | Median total publication ms |
| --- | --- | --- | ---: | ---: |
| 1 | Control | 23.78 / 37.70 / 42.59 | 1,441.7 | 2,571.2 |
| 2 | Scatter | 25.66 / 37.88 / 42.20 | 55.4 | 1,388.7 |
| 3 | Scatter | 25.83 / 39.23 / 43.35 | 63.5 | 1,524.6 |
| 4 | Control | 25.64 / 38.92 / 43.23 | 1,618.2 | 3,015.2 |

Both pairs reduce upload submission latency by about 96%, and total publication
latency by 46–49%. All four runs published five shifts with zero frames outside
exact coverage. Worker preparation still took roughly 1.1–1.4 seconds; scatter
does not eliminate that remaining bottleneck. These are stage medians, not
components that necessarily sum to the median total.

Each control made 17,933 arena writes over 304 active upload frames. Scatter
made 14 packed writes/dispatches over 14 frames, containing the same 64,598,160
arena bytes plus 287,056 descriptor bytes. Both also transferred 20,971,520 root
bytes. Measured peak transfer was exactly 8 MiB in all runs. Descriptor accounting
and unchanged certified-interior mesh omission are checked in the saved summary.

This is a strong streaming improvement, **not a steady-frame speedup**. The first
control was faster than the other runs; the reverse pair had similar frame
medians/tails. Two repeats cannot establish a small FPS effect. Scatter remains
opt-in for a wider interactive playtest, rather than silently changing defaults:

```sh
FRONTIER_NAADF_SCATTER_UPLOAD=1 ./run.sh --preset alpine --naadf-radius 128 --naadf-follow on --msaa 1 --resource-profile development
```

The compact reproducible results, including GPU/CPU memory, cameras, frame tails,
stream counters and interior/boundary mesh residency, are saved in
[`reference/naadf-seven/measurements.json`](reference/naadf-seven/measurements.json).
Raw per-frame logs and full capture buffers remain under `output/naadf-seven/`.

Hardware: AMD Ryzen 5 7640U (6 cores / 12 threads), integrated Radeon 760M,
Vulkan RADV, Mesa 25.2.8. Selected hardware correctness checks can be repeated
sequentially after the CPU suite:

```sh
cargo test --release -p frontier-renderer
cargo test --release -p frontier-renderer naadf_traversal_variants_match_canonical_rays -- --ignored --nocapture
cargo test --release -p frontier-renderer accelerated_primary_replays_ambiguous_corner_face -- --ignored --nocapture
cargo test --release -p frontier-renderer scatter_gpu_preserves_holes_and_reuses_staging_in_queue_order -- --ignored --nocapture
FRONTIER_NAADF_SCATTER_UPLOAD=1 cargo test --release -p frontier-renderer naadf_gpu_distance_matches_canonical_rays -- --ignored --nocapture
cargo test --release -p frontier-renderer retained_gpu_tiles_remain_conservative_across_wraps -- --ignored --nocapture
```

## Refined traversal result and decision

Final 64 m fixed-view runs, same final executable and sequential measurement:

| Candidate | Frame p50 / p95 / p99 ms | NAADF span p50 ms |
| --- | --- | ---: |
| Control | 19.70 / 23.03 / 23.82 | 7.94 |
| Compact scalar-key node cache | 19.89 / 22.87 / 23.63 | 8.19 |
| Gated flat height bounds | 21.65 / 24.92 / 25.38 | 9.72 |
| Gated height mip hierarchy | 23.14 / 26.30 / 26.77 | 11.09 |

The compact cache is close to control, not a demonstrated improvement. Restricting
height queries avoids some redundant work but still loses on this scene. Compare
within this table; the earlier batch's control was faster, so cross-batch absolute
numbers cannot establish a refinement win. Larger or emptier scenes could change
these tradeoffs, and are not certified by these measurements.

**Decision:** retain the CPU proof/default traversal path. Recommend scatter as
an optional streaming playtest. Keep height bounds, mip bounds, beam prefixes,
node caches, persistent GPU proofs and feedback selectable for future experiments,
not enabled merely because their individual operation counts look promising.
No visual quality reduction or redundant interior raster work accompanies them.

Next worthwhile investigations are (a) worker preparation after the upload
bottleneck is removed, and (b) improving fragmented streamed proofs without paying
an additional lookup at every empty root. The diagnostic proves those proofs can
be weaker; it does not yet prove a faster full-frame replacement. Neither is
silently claimed implemented by this experiment checkpoint.

### Final image checks and an unmatched-capture finding

Compact cache, gated mip bounds and scatter produced byte-identical linear RGB
and depth to the final control. One gated-flat capture initially differed at
41 depth pixels: its distant vegetation contained 646,152 tree triangles versus
648,156 in control. Every changed depth ray hit over 101 m beyond its NAADF exit.
Completed vegetation jobs did not guarantee the same retained per-tree detail.
A repeat with matching geometry was byte-identical in color and depth; the
original unmatched result is preserved and explicitly marked invalid in the
measurement summary. The harness now rejects unequal vegetation geometry before
judging a renderer's pixel errors. This is a remaining capture reproducibility
limitation, not a traversal fix or an excuse to ignore image mismatches.

The original executable and final integrated control also match in linear RGB
and depth. The final fixed Legacy reference PNG, linear RGB and depth match the
original byte for byte. All seven experiments are implemented and tested; only
the measured streaming improvement is recommended for the next optional playtest.

## Measured follow-up

The [optimization follow-up](NAADF_OPTIMIZATION_FOLLOWUP.md) adds actual compiler
register statistics, query/skip counters, direct publication with durable proof
provenance, guarded local grid exits, CPU stage profiling, exact classification
reuse, and asynchronous GPU compute-stage timings. It records positive and
negative results across 64/128/256 m workloads with unchanged visual controls.
