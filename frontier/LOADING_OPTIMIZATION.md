# Adaptive preparation and streaming optimization

Work for issues #69, #64, #70, #71, #73 and #72. The approved visual target is
[PRIMARY_VISUAL_CHECK.md](PRIMARY_VISUAL_CHECK.md); its source, camera, canonical
voxel size and appearance stay fixed. The comparison uses requested 5 px error
and the existing 0.8 refinement reserve, MSAA4 and a 300 m matched reference clip.
Distance-scaling tests explicitly change the clip and cannot reuse a mismatched
reference as a pixel-error oracle.

## Implementation

- Small jobs use a dense local canonical-column workspace, avoiding repeated
  per-column hash lookups during appearance integration and certification.
- CPU preparation has a configurable worker count, with at most four active
  builds. Spatial source-cache shards share canonical values across workers;
  mesh admission and coherent publication remain ordered by the coordinator.
- CPU stage sums distinguish appearance, certification, native meshing and
  finalization. Source evaluation is nested within those caller stages. These
  sums must not be added to source cost or mistaken for parallel wall time.
- Movement prediction uses observed camera translation/rotation and an exact
  nearest-rank p95 from bounded latency histories. Before a quality publication
  sample exists, generation-batch duration is an explicitly labelled lower-bound
  fallback. Speculative refinement is bounded to one genuinely future-only selection slot
  while current refinement retains the other slots; it does not establish
  successful motion by itself. Identical-pose requests clear stale forecasts
  before epoch deduplication, and forecast occlusion is evaluated at its own pose.
- An optional, disposable disk cache stores fully built geometry with explicit
  versioned source identity, key, checksum and bounded decoding. One process
  owns a cache directory through an OS lock. See [disk cache](DERIVED_DISK_CACHE.md).
- The GPU summary probe evaluates integer exposed-face/material counts from
  CPU-canonical pages. It is an independent experiment, not a second GPU terrain
  definition or an enabled production generation path.

The CPU geometry/source cache budget and builder reservation budget are separate.
Reports include live tile payload retained by external Arc owners as well as
cache residency. Allocator metadata, process overhead, GPU attachments and driver
allocations are not a single enforced process RSS budget.

## Seed terrain pages and turns

Voxy-style coarse tiles request conservative affine source bounds in batches of
up to eight. The cache partitions the world into 8 × 8 bound pages at each sample
pitch. Different tile footprints and seam variants share bounds where their
canonical source regions coincide. Slots are computed on demand, and the source
cache owns the pages for the lifetime of its immutable world snapshot. Pages
reserve up to one eighth of the existing source budget, capped at 16 MiB.
Different pitches have independent certificates.

The November 19 Alpine seed 7 runs used a Ryzen 5 7640U and Radeon 760M at
1920 × 1080, MSAA1, a 950 m clip, 32 m NAADF radius and a 3 px error target. With
turn preloading active in both versions, two cold, 15-second walk-and-turn runs
per mode averaged 15.90 s → 12.83 s summed bound CPU time and 151 → 110 MiB
terrain uploads. Whole-frame p50/p95/p99 averaged 24.9/36.6/47.0 ms with page
caching disabled and 24.6/40.7/49.9 ms enabled. Coarse coverage held throughout.
The 3 px target remained unmet throughout this route.

For the page comparison, run the following from this directory with
`FRONTIER_TERRAIN_BOUND_PAGES` set to 0 and 1. The separate
`FRONTIER_TURN_PRELOAD=0` control disables the turn policy.

```sh
FRONTIER_PROFILE_SOURCE=1 ./run.sh --preset alpine --seed 7 --msaa 1 \
  --width 1920 --height 1080 --capture-far 950 --naadf-radius 32 --pixel-error 3 \
  --plant-cpu-mib 512 --plant-gpu-mib 384 --triangle-budget 2400000 \
  --sun-elevation 13 --sun-azimuth 23 --cloud-coverage 0.12 \
  --eye -43.625,31.0625,-289.5625 --target -43.625,31.04,-288.5625 \
  --route walk-turn-return --route-speed 2.8 --route-clock realtime \
  --benchmark-input-lock --seconds 15 --warmup 10 --no-adaptive-disk-cache \
  --report output/alpine-bound-pages.json
```

## Reproduction

Build with the pinned toolchain:

```sh
cargo build --release --locked --workspace
python3 tools/adaptive_loading_check.py --output output/loading-workers
python3 tools/adaptive_motion_check.py --settled --pixel-error 5 --msaa 4 \
  --workers 2 --speeds 3,15,80 --seconds 12 --output output/loading-motion
python3 tools/adaptive_loading_check.py --workers 2 \
  --distances 300,1000,4000,20000 --output output/loading-distance
```

The sweep runs sequentially. Hardware timings require the actual laptop GPU;
software rendering is unsuitable for performance acceptance. Avoid concurrent
CPU compilation/profiling during runs. First preparation is excluded from settled
frame statistics and is reported separately. A failed convergence or quality gate
remains a failure even if the last image looks good or frame rate is high.

For disk A/B, use a new directory for cold cache, then repeat in a fresh process
with the same path and a different output directory:

```sh
python3 tools/adaptive_loading_check.py --workers 2 \
  --disk-cache output/cache-experiment --output output/disk-cold
python3 tools/adaptive_loading_check.py --workers 2 \
  --disk-cache output/cache-experiment --output output/disk-warm
```

These tools never delete a cache to manufacture a cold result. Disk space defaults
to 2048 MiB when enabled. A 64 MiB entry cap bounds read/decoded buffers; a larger
or invalid entry falls back to generation. Disk encoding and I/O run on one dedicated worker. Indexed reads have priority;
optional writes use a bounded queue so cold exploration does not wait for encoding
and writes. Cache misses bypass the I/O worker. See the disk-cache document for
queue, buffer and shutdown limits.

## Research and scope

The [AMD RDNA performance guide](https://gpuopen.com/learn/rdna-performance-guide/)
and [asynchronous queue discussion](https://gpuopen.com/learn/concurrent-execution-asynchronous-queues/)
advise measuring resource contention and synchronization. This motivates measuring
GPU dispatch, data transfer and readback as well as CPU preparation and drawing;
it is not evidence that a compute port will accelerate this particular workload.
The probe uses the repository's pinned wgpu 24 APIs and CPU shader validation.
No external engine/runtime implementation was copied.

## Measurements

Hardware results and issue acceptance are recorded below after validation.

### Bounded CPU profiling and exact work reuse

The CPU development checkpoint was measured on an AMD Ryzen 5 7640U with
Radeon 760M, using `rustc 1.99.1 (58cf8f9c3 2026-10-13)` and the release
profile. It builds on `ce3e82a4cda00f2e5d8a6e1cdfd7f9a43d1e082c` plus the
loading changes in this working tree. The ignored, reproducible test is:

```sh
cargo test --release --locked -p frontier-renderer profile_primary_cold_stages \
  --lib -- --ignored --nocapture
```

Raw output is retained in `output/issue69-cpu-profile/stages.log`. This is a
single bounded CPU sample with no timing pass threshold. It excludes cache
construction, selection convergence, rendering, GPU upload and disk I/O. These
numbers are tile preparation diagnostics, not complete cold-readiness timings or
a moving-view acceptance result. Hardware comparisons below supersede these
microbenchmarks for end-to-end claims.

The source is the unchanged primary Legacy seed-7 recipe: 1,800 m horizontal
scale, 1,000 m amplitude, eight octaves, roughness 0.5, and 6.25 cm canonical
cells. Each comparison starts with a fresh 128 MiB geometry/source cache,
including a 16 MiB source reservation. The bounded affine cache consumes about
2 MiB of that reservation. The dense and sparse local workspaces run in the
same binary; the test compares their exact face profiles, mesh indices and
certificates. A separate parallel/sparse regression also compares every vertex
attribute and packed native-quad byte.

| Native-cell minimum, 8 m tile | Proxy step | Sparse local workspace | Dense local workspace | Generated columns in either path |
| --- | ---: | ---: | ---: | ---: |
| `[0,-512]` | 8 cells / 0.5 m | 14.630 ms | 7.864 ms | 16,897 |
| `[-256,-1536]` | 16 cells / 1 m | 10.692 ms | 6.543 ms | 16,897 |
| `[512,-3072]` | 32 cells / 2 m | 10.490 ms | 5.731 ms | 16,897 |

The reduction removes repeated local hash lookups without changing samples,
source bands, geometry, filtering or error evidence. A separate full-halo
prefill takes 4.26–4.64 ms in this sample; the following warm dense proxy builds
take 2.04–3.48 ms. Prefill samples a 130×130 grid and is diagnostic only; the
production proxy still requests its original 16,897 columns lazily. Native and
large sparse jobs do not use this dense optimization, and their small timing
differences in the log are not evidence of an improvement.

Four adjacent 8 m jobs at native-cell minima `[0,-512]`, `[128,-512]`,
`[256,-512]` and `[384,-512]`, step 8 and native edge strides, take 34.751 ms
serially and 14.344 ms with four scoped workers in the same bounded sample.
Both paths generate exactly 66,817 columns and record 771 source hits. Source
miss generation stays under one of 32 spatial shard locks, preserving exact
shared values without a global generation lock. Fixed shard budgets can change
eviction behavior under a larger working set; this small batch does not prove
full-world scaling or the best worker count on the integrated GPU.

Affine source analysis is independent of edge stitching. A 128 m tile at
`[0,-512]`, step 128 cells, initially takes 26.674 ms, including 11.670 ms in
affine bounds. Rebuilding with only the -X edge changed from 128 to 64 cells
takes 6.040 ms, including 0.784 ms in affine bounds, with 252 bound-cache hits
and four misses. Total time also benefits from canonical source-page reuse;
the affine-stage reduction isolates the avoided source analysis. A direct
uncached build of the changed-edge key matches cached certificates, source
height ranges, profiles and indices.

The bound cache belongs to one immutable source snapshot. Its exact key contains
column-box minimum, both extents and both `BoundBudget` limits. Bounded direct
mapping may evict on collisions; it cannot substitute evidence for another key.
Its reserved capacity is charged inside the source allowance and is at most
one eighth of that allowance, capped at 4 MiB globally. Clearing the source cache
invalidates all bound entries. Shared page access stamps can advance during
concurrent job starts; this changes bounded LRU priority, not source identity or
returned values.

Stage telemetry records coarse per-job CPU durations. Source lookup/generation
is nested within its calling appearance or meshing stage, and concurrent job
durations overlap in wall time. `affine_noise_cells` describes evidence attached
to built tiles, including reused bounds; cache hit/miss counts distinguish
avoided analysis calls. Live tile payload telemetry deduplicates cache entries
and still-live evicted Arc allocations, while builder reservations cover
not-yet-admitted batch products separately. These counters are not a continuous
process-RSS measurement.

### First full-view CPU optimization comparison

Exclusive sequential Radeon 760M / Ryzen 5 7640U runs, fixed primary source,
1080p MSAA4, requested 5 px / reserve 4 px, 512 MiB cache and 512 MiB builder
reservation, disk disabled. Settled runs use the existing 20 Hz preparation
polling policy before the 12-second uncapped render measurement.

| Workers | First reserve ready | Frame p95 | Quality-ready measured frames |
| --- | --- | --- | --- |
| 1 | 27.962 s | 10.838 ms | 1308 / 1308 |
| 2 | 24.315 s | 11.059 ms | 1310 / 1310 |
| 4 | 15.602 s | 10.899 ms | 1309 / 1309 |

All runs certified a maximum visible error of 3.995866 px and reported no
unbounded/quality-failing measured frames. This is about 3.08× faster preparation
than the previous recorded 48.082 s checkpoint at the same settings. The new
one/two/four-worker comparison is same-binary; the older result is historical,
not a concurrent thermal/power-controlled comparison.

The full floating-point RGB payload is byte-identical across all three runs and
the previous approved candidate:
`49cd95fa33ba55d5eeca7b2b9bc036f4b160c4826ce4bf85824b7387c1e0691c` (SHA256).
This proves appearance preservation for that view, not moving-view acceptance.
Logs/reports/frozen binary: `output/loading-workers/`; comparison:
`output/loading-workers/comparison.png`.

These static runs preceded the correction from request-epoch ages to actual
preparation-cycle durations. Their predictor latency fields must not be treated
as full refinement-latency measurements. The camera was fixed, so no predicted
motion was used. Later motion results use the corrected cycle tracking.

### Bounded GPU per-patch summary experiment (#71)

The standalone probe now uses the actual 16-native-column (1 m) appearance
profile granularity: 64 patches per 128×128-column canonical page. One GPU
workgroup owns each patch and reduces exact integer exposed-face counts by
axis/material. Canonical height/material generation stays on the CPU; topology
owners and edits are rejected by this bounded experiment. The production
appearance path remains unchanged.

An exclusive Radeon 760M / RADV PHOENIX Vulkan run passed independent canonical
face-oracle parity for 1/4/8-page batches and five resident repeats per batch.
Release-profile timings from `output/loading-gpu-probe/run.log`:

| Pages | CPU canonical generation | CPU count summary | Cold upload/submit/readback | Resident submit/readback median | GPU kernel |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 5.833 ms | 0.209 ms | 2.256 ms | 0.188 ms | 0.020 ms |
| 4 | 24.012 ms | 0.799 ms | 1.536 ms | 0.346 ms | 0.066 ms |
| 8 | 39.670 ms | 1.325 ms | 1.069 ms | 0.294 ms | 0.120 ms |

The initial one-page submission includes a 1.655 ms first-use encode/submit
observation; pipeline construction itself is outside the timed dispatch. Cold
GPU totals include upload enqueue, submission and blocking completion/readback.
Resident medians exclude upload and CPU canonical generation. Eight-page logical
GPU buffers total 2,265,632 bytes including timestamp resolve/readback; driver
and queue staging allocations are separate.

Summing generation and cold-summary stages gives CPU versus GPU totals of
6.042 versus 8.089 ms, 24.811 versus 25.548 ms, and 40.995 versus 40.739 ms.
The GPU kernel is short, but CPU canonical generation dominates those totals.
Resident reuse can save approximately 1.03 ms for eight pages relative to this
standalone CPU integer-count loop. That CPU comparator already avoids production
lookup and repeated float color accumulation, so older production appearance
stage timings cannot be used as its baseline. A future CPU integer-count profile
path also merits comparison, subject to floating appearance parity checks.

This is a validated bounded summary prototype, with no production integration
or simultaneous scene-rendering contention test. It does not demonstrate faster
whole-game GPU terrain generation, improved full-view readiness or the combined
motion quality/performance gate. Full design, reproduction commands and limits
are in [GPU_SUMMARY_PROBE.md](GPU_SUMMARY_PROBE.md).


### Lossless representation and asynchronous persistence

Coarse proxy triangles now use a 64-byte instance when their flat attributes
permit exact packing and that packing saves space. Expanded geometry remains the
fallback. Primitive order, positions, normals, colors and profile IDs are
unchanged. The primary packed capture is byte-identical to the approved float
RGB hash above; GPU terrain residency is 128,994,000 bytes and settled frame p95
is 9.862 ms in `output/loading-packed/far300-workers4`. Cold reserve readiness
was 17.220 s; packing is a memory improvement, not a demonstrated cold speedup.
The fixed cluster metadata limit is now 16,384 (4 MiB camera uniforms), while the
terrain pool remains 512 MiB. Capacity exhaustion retains coherent coverage and
explicitly reports unresolved quality.

Before packing, the 1/4/20 km views exhausted the 512 MiB CPU cache around 7,700
owners without meeting requested quality. Packing let the 1 km view reach 5 px
at 108.272 s with 10,111 resident owners and 311,433,552 GPU terrain bytes. It
still timed out before the 4 px reserve at 120 seconds, so no settled frame-time
claim is possible for that run. These are development measurements preceding
the final planner optimization, preserved in `reference/loading/`.

The asynchronous version-2 disk cache measured 15.648 s cold and 7.192 s on a
fresh-process restart. All 4,984 artifacts loaded without rebuilding; both images
match the approved hash. Cold writes totalled 362,314,408 bytes with no errors or
skipped writes, and the queue peaked at five entries / 5,169,536 payload bytes.
The earlier synchronous version was faster warm (3.640 s) but slowed cold loading
to 29.226 s. These are sequential development runs, not controlled storage-cache
flush experiments; a restart does not imply the OS page cache was cold.

### Streaming scheduling corrections

A retained coherent transaction can finish its bounded batches without rescanning
the entire selected cover between batches. Current demand is checked during
preparation, and publication still requires current-camera validation. Camera
updates no longer prevent a useful batch from starting merely because its epoch
changed. Plan call counts, full/retained counts and elapsed time are reported
separately from worker build sums.

Incremental transaction latency is measured from selection through actual GPU
ownership publication. It drives motion prediction independently of the much
longer initial/full-quality cycle, whose timing remains separately reported.
These corrections require new hardware motion validation; they do not by
themselves establish aircraft-quality acceptance.


### Planner/latency checkpoint, before appearance memoization

All 235 Rust tests and nine Python tests passed; seven manual diagnostics remain
ignored. `output/loading-final-primary` records the unchanged primary image at
14.049 s reserve readiness, frame p50/p95/p99 of 7.766/10.121/11.024 ms and no
missed frame budgets. Planning took 1.397 s over 514 calls; 1,080 intermediate
batch replans were avoided. Incremental publication p95 was 148.7 ms, separately
from the 13.857 s full-quality cycle.

| Far clip | Reserve ready | Selected owners | Maximum certified visible error | Settled frame p95 |
| --- | ---: | ---: | ---: | ---: |
| 300 m | 14.049 s | 1,687 | 3.9959 px | 10.121 ms |
| 1 km | 93.836 s | 11,989 | 3.9998 px | 30.633 ms |
| 4 km | Not within 120 s | 14,682 | 17.0723 px | Not measured |
| 20 km | Not within 120 s | 14,749 | 27.3533 px | Not measured |

The 1 km GPU terrain payload is 352,247,856 bytes; 10,826 draw calls and an
18.187 ms isolated GPU mean accompany the failed 60 FPS frame budget. At 4/20 km,
preparation still had pending work when the fixed test deadline expired, without
an explicit resource refusal. This does not prove those views can never converge.
No fog, reduced far clip or relaxed quality threshold was used to manufacture a
pass. Longer-view screenshots are diagnostic and not matched to the 300 m oracle.

The 3/15/80 m/s turn-return tests kept coverage on every measured frame, but
requested quality passed only 3/2523, 0/2441 and 0/2749 frames respectively.
Their frame p95 values were 8.551/8.483/7.317 ms. A separate 3 m/s stop/resume
passed quality on 162/1543 frames (frame p95 11.563 ms). Thus #64 remains failed:
fast coherent fallback is useful but is not detail keeping up with movement.
The current predictor is a bounded heuristic; surprise turns and accelerated
motion are not guaranteed to have precomputed detail.


### Exact appearance memoization and urgent-native experiment

Patch appearance integration is reusable across edge changes: global patch
minimum, extent and sampling stride define the same accumulation order and
immutable canonical source. A fixed direct-mapped cache reserves at most 4 MiB
inside the existing source budget, reducing source-page capacity rather than
silently increasing memory. Cached extrema addresses preserve the native height
envelope even when the appearance loop is skipped. Tests compare full primitive
and profile float bits, geometry certificates, bounds, negative coordinates,
parallel builders and clearing/source isolation. Collisions only evict entries.

`--adaptive-direct-native on` is an optional scheduling experiment, off by
default. At most one nearest requested-failing native-size leaf already selected
by current priorities skips intermediate proxy steps. Existing memory admission,
edge closure and publication checks remain mandatory. A large upper error bound
is not a proof that native geometry is necessary: this is a scheduling heuristic,
not a changed certificate or quality requirement.


### Final default and optional-path measurements

Release binary SHA256:
`b91dcab37e501642fb9c772664b4a34cb074d5a93d79498eb253b14903597512`.
The full suite passes 239 Rust tests and nine Python tests; seven explicit manual
experiments remain ignored. Formatting and diff checks pass.

With appearance memoization, the final default primary view reaches reserve in
13.796 s with frame p95 10.128 ms. Its float RGB remains byte-identical to the
approved image. Appearance worker-time sum falls from 22.159 s to 20.181 s in
these sequential runs; overall readiness improves only modestly relative to the
14.049 s planner checkpoint. The cache records 100,595 hits / 752,973 misses.
Source generation slightly increases as the fixed budget leaves fewer source
pages; do not describe this as a large end-to-end speedup.

The optional direct-native path reaches reserve in 12.586 s, but native owners
increase from 62 to 123 and frame p95 rises to 12.056 ms. Its image is therefore
not byte-identical. At 3 m/s, requested-quality frames are 78/2281 in the turn
route and 389/1313 in stop/resume; coverage remains complete. The final default
turn test passes quality on 63/2549 frames. Neither mode passes #64, so the direct
path remains opt-in rather than silently changing the approved rendering target.

A fresh-process restart of the existing version-2 cache with the final binary
reaches reserve in 5.029 s (frame p95 9.995 ms): 4,984 hits, zero builds, writes,
misses or errors. This is process-restart reuse with an OS-warm cache. The first
exploration and older async cold measurements above remain separately identified.

The final 4 km aerial test (eye `[0,1200,0]`, target `[0,800,-1000]`, same
source/FOV/resolution/quality) also timed out after 120 s: coverage remained
complete, with 13,573 owners, zero native clusters, 294,202,256 GPU terrain
bytes and 14.215 px maximum certified error. No settled frame statistic is
reported. Thus wide-view cost is not simply excessive nearby native geometry.

The optional direct-native still's color RMSE against the independent 256-spp
reference is 0.011027 overall / 0.014423 on reference-full-coverage pixels,
versus 0.011661 / 0.014986 for the unchanged default. These are color measurements,
not geometry proof: MSAA4 captures have no resolved depth/coverage oracle. The
single-sample geometry comparator correctly refuses to certify them.

## Remaining acceptance work

This is an implemented optimization checkpoint, not completion of all six issue
contracts. #70's bounded parallel preparation has measured end-to-end benefit.
#69 has substantial measured cold improvement, but source evaluation is still
nested within stage timings rather than a complete exclusive-cost breakdown.
#71's GPU probe remains experimental because transfer/source cost prevents a
proven whole-game gain; drawing-contention and production integration are absent.
#72 supports bounded versioned restarts, corruption fallback and per-tile local
edit dependency identity, including the native-column halo. CPU tests demonstrate
unaffected surface-tile reuse across owner edits and current-revision rebinding.
These format-3 changes have no new hardware timing claim; prior format-2 restart
measurements remain historical. #64 still fails moving quality; #73 still fails wide-view
latency/60 FPS and needs extended independent long-range reference crops.

The next substantive work is publication/visibility scheduling and generation
throughput for changing views, followed by reducing thousands of per-cluster
draw submissions. A larger GPU may help drawing but cannot be assumed to remove
these CPU and preparation limits. Do not close failed quality gates based on
fast fallback frame times.


## Follow-up: bounded caches, submission and scheduling

See [follow-up measurements](reference/loading/followup-measurements.json) and
[reference/cache/compute evidence](reference/loading/remaining-issues-notes.md).
All new implementations are greenfield (reuse tier G); no legacy engine was ported.

Geometry-cache eviction now uses an intrusive constant-time LRU, with metadata
charged to the cache allowance. Final-Arc lifetime accounting uses an atomic
counter instead of scanning all weak references. Source statistics take one
snapshot of each shard. These remove repeated bookkeeping as the cache fills.

`--adaptive-refinements 1..32` controls coherent refinement selection separately
from the four-active-builder limit and memory reservations. The interactive
command defaults to eight; the benchmark scripts retain an explicit default of
four for historical controls. Sixteen is an optional faster static-loading choice,
not a claim that larger transactions improve movement latency.

Stable GPU draw sequences use wgpu render bundles by default. Identical camera,
ownership and resident content reuse prepared visibility/uniform work. Geometry,
filter, bin or ownership changes invalidate reuse; camera changes rerun view
preparation. The draw sequence and vertex buffers are unchanged. Disable these
independently with `FRONTIER_RENDER_BUNDLES=0` and
`FRONTIER_PREPARED_VIEW_CACHE=0` for A/B tests. This follows wgpu's
[render-bundle command reuse](https://wgpu.rs/doc/wgpu_core/command/bundle/index.html)
contract; the shared draw encoder preserves the direct-path commands. Bundle
execution resets render-pass state, and subsequent volume drawing binds its own
pipeline/resources. A stable-camera result does not establish moving performance.

`FRONTIER_PROFILE_SOURCE=1` enables fine source lock/lookup/generation timers and
exclusive caller costs. Clock reads and instrumentation inside locks substantially
perturb execution; use the bounded `profile_primary_cold_stages` diagnostic for
attribution, never its timing as uninstrumented game throughput. Inclusive build
stages remain separately labelled. Upload enqueue, publication checking, view
preparation and command encoding CPU times are distinct from GPU execution.

`FRONTIER_DEBT_PREDICTION=1` is an experimental bounded backlog-aware forecast.
It estimates up to 32 transactions of lead time; actual speculative admission
and retention budgets do not grow. Its turn test did not improve enough to
justify enabling it. The default keeps measured incremental publication latency.
Coverage and quality are reported independently: a fast fallback frame is not a
successful quality-ready frame.

### Experimental visible-part geometric bound

`FRONTIER_VISIBLE_ERROR_BOUND=1` tests a tighter vertical-error bound. A tile's
full AABB can cross the camera plane even when its visible floor begins metres
away. The new method clips against side planes expanded by the complete allowed
vertical displacement, so an offscreen proxy cannot hide a visible canonical
counterpart. It uses the actual uploaded per-tile f32 camera matrix and exact mesh
origin, with outward allowances for input reconstruction, matrix products and
both projected endpoints. Possible near-plane crossings and arithmetic overflow
remain uncertified. It does not perform occlusion culling or relax the pixel
budget. The original bound remains the default pending end-to-end evidence.

Arithmetic assumptions follow [WGSL floating-point accuracy](https://gpuweb.github.io/gpuweb/wgsl/#floating-point-accuracy),
including permitted directed rounding and subnormal flushing. Tests cover
counterpart-only silhouettes, near crossings, packed-coordinate reconstruction,
shader-style dot-product evaluation and cancellation at large local offsets.
The bound concerns projected geometry; it is not a proof of identical raster
coverage or color. This distinction also applies to the original geometric
certificate.

### Late capacity regression and material experiment

The wider-distance sweep found a cache-capacity accounting mismatch: the LRU
charged per-entry metadata, but a proposed full replacement counted payload only.
Near the limit, an apparently admissible replacement could evict its own required
tiles and rebuild them repeatedly. Replacement feasibility now includes metadata
for every target key, including unbuilt owners. Exact committed tiles can also
satisfy demand after cache eviction. An obsolete oversized transaction is discarded
before replanning the current camera; only current demand can trigger refusal.
Tests cover metadata-only overflow, no futile rebuild, retained-Arc reuse and a
smaller destination after teleport. A genuine insufficient budget retains coherent
coverage and reports unresolved quality; it does not silently lower the threshold.

An additional exact-material experiment reuses one sampled center column across
its exposed face spans. It preserves all sampled addresses and float accumulation
order. However, the same-binary 300 m run measured 11.713 s without it and 12.304 s
with it (identical RGB); this single pair does not demonstrate a speedup. It stays
opt-in through `FRONTIER_PROFILE_CENTER_REUSE=1`. Source/appearance work remains
an important unresolved throughput cost; less code work in one loop alone does
not establish a faster game.

## Packed coarse quads and reusable interiors

The seed and detailed voxel paths emit packed quads directly. Ground-cover
profiles are assigned before the two directional merge passes. The interior
of an affine-certified tile uses direct comparisons between adjacent column
runs. The existing span sweep owns the thin border and its joins. A bounded
source-owned cache retains interior roofs and walls with patch-local profile
slots, keyed by world revision, tile footprint and pitch. A different neighbor
pitch regenerates the affected boundary and maps the reused faces to current
profile indices. `FRONTIER_COARSE_INTERIOR_CACHE=0` runs the direct interior
builder without memoization.

A source height range and the emitted roof extrema cheaply bound the whole
patch. When that bound is below the certified maximum already accumulated for
the tile, the per-quad affine tests cannot improve its conservative certificate.
All other patches use the affine corner calculation. Large-tile diagnostics
report coarse source-bound time, roof-comparison time, dominated patch count,
and interior cache hits. With `FRONTIER_PROFILE_SOURCE=1` the bound-page cache
also reports lock wait and evaluation sums. These are overlapping worker
measurements. The page evaluation and wait are included in the coarse source
time when the corresponding tile completes. Small tiles retain exact column
enumeration.

The interval evaluator now shares lattice gradients across the point and
range evaluations and omits the unused point Hessian. In a standalone release
probe over 1,728 seed-7 Alpine, Legacy and Seeded World boxes at four pitches,
the bound-field digest matched the baseline. The optimized path took about
82 ms versus 87 ms over alternating samples. A sign-branching interval
multiply made the same probe slower, about 98 ms, and was discarded.

`FRONTIER_COARSE_Y_PITCH_DIVISOR=2|4|8` tests finer Y rounding for dry stair
roofs. The same quantization is applied to both sides of a shared edge.
Voxel-pitch and water samples retain their native heights. Derived cache
identity includes the alternative divisor. The default divisor is 1.

### Radeon 760M measurements

Ryzen 5 7640U, Radeon 760M, RADV Mesa 25.2.8, 1920×1080, MSAA1, and
6.25 cm canonical voxels. The baseline was Frontier commit `0a167a9`. These
experiments extend the current engine in place and use no legacy engine code.
Use the walk-and-turn command above on each release binary. The preset comes
before `--no-adaptive-disk-cache`; every reported run has zero disk hits, misses,
and writes. The first exploratory loop put the disable flag before the preset
and its cache-contaminated timings were discarded.

| Alpine walk and turn | Runs | Frame p50 / p95 / p99, ms | Terrain uploads, MiB |
| --- | ---: | ---: | ---: |
| Baseline | 2 | 24.18 / 32.27 / 46.12 | 131.8 |
| Packed quads and interior reuse, divisor 1 | 2 | 24.52 / 30.73 / 42.08 | 150.8 |
| Packed quads, interior reuse disabled | 1 | 24.47 / 31.78 / 45.95 | 140.8 |
| Packed quads with visible-part error bound | 1 | 24.52 / 30.60 / 44.10 | 145.9 |
| Packed quads, divisor 4 | 2 | 23.80 / 29.79 / 41.25 | 95.0 |
| Packed quads, divisor 4 with visible-part error bound | 1 | 24.04 / 30.34 / 39.85 | 93.1 |
| Packed quads, divisor 8 | 1 | 24.02 / 31.34 / 44.07 | 93.9 |

The numbers average each run's reported quantiles. Coverage held in every
measured frame. No run reached the 3 px target during movement. The optional
visible-part projection reduced frames with an unbounded projection from 37–38
to 8 in one pass and to 0 in the divisor-4 pass. Both fell short of the 3 px
target. The packed path reused
418 and 434 tile interiors across the two divisor-1 walk runs. The route can
build a different number of tiles in the same wall time, so total worker time
alone cannot establish the cost of a tile.

For the stationary Alpine horizon, vegetation was disabled to compare terrain
at a 6 km clip. Seed, camera, exact radius, fog, and target stayed fixed. Each
row is one cold run followed by 189 settled frame samples. Output is in
`output/lod-optimization/horizon-cold-*`.

| 6 km horizon | First quality / reserve, s | Frame p50 / p95 / p99, ms | Reported terrain triangles | Resident clusters |
| --- | ---: | ---: | ---: | ---: |
| Baseline | 15.53 / 21.28 | 20.66 / 22.45 / 23.05 | 2,149,658 | 3,739 |
| Packed quads, divisor 1 | 14.61 / 20.17 | 19.64 / 23.09 / 23.34 | 2,149,658 | 3,739 |
| Packed quads, divisor 4 | 5.78 / 10.24 | 20.17 / 22.54 / 23.62 | 2,247,054 | 2,788 |
| Packed quads, divisor 8 | 4.85 / 9.09 | 21.04 / 24.12 / 24.96 | 2,348,244 | 2,650 |
| Surface | 9.13 / 10.58 | 17.49 / 20.35 / 21.24 | 334,432 | 1,716 |

All tested variants reached the 6 px reserve and had zero pending selection bytes at the
end. The divisor-1 capture matches the baseline PNG pixel for pixel. The
surface lowers GPU time and changes the distant slopes, material transitions,
and shadows. The smaller Y pitch retains stairs and changes their size,
distribution, and shading. Both remain selectable for visual evaluation. Divisor 8 reached quality
sooner than divisor 4 and paid for the extra faces in both frame-time tests.
In the divisor-1 run, large-tile source bounds accounted for 36.18 s of summed
worker time and roof comparison for 0.135 s. Bound-page lock wait was 0.85 s.
Once prepared, the whole GPU span and exact terrain pass dominate the frame
budget, and reducing roof arithmetic further cannot materially improve it.

```sh
FRONTIER_PROFILE_SOURCE=1 FRONTIER_DISTANT_TERRAIN=voxy-seed ./run.sh \
  --preset alpine --seed 7 --vegetation off --vegetation-near 32 --msaa 1 \
  --width 1920 --height 1080 --naadf-radius 64 --naadf-follow off \
  --capture-far 6000 --pixel-error 7.5 --adaptive-refinements 24 \
  --eye -43.625,75,-289.5625 --target 0,490,3300 \
  --cloud-speed 0 --fog-density 0 --streaming-haze 0 \
  --terrain-cache-mib 2179 --build-memory-mib 2179 --terrain-gpu-mib 1452 \
  --plant-cpu-mib 581 --plant-gpu-mib 871 --triangle-budget 4000000 \
  --benchmark-input-lock --adaptive-wait-quality --frames 240 --warmup 50 \
  --no-adaptive-disk-cache --capture-on-exit output/horizon.png \
  --report output/horizon.json
```

The fixed player-height spiky capture used Legacy seed 7, a 300 m clip,
32 m NAADF radius, 5 px target, MSAA1 and no atmosphere or terrain shadows.
The original and packed versions reached the requested target. Their linear
color RMSE against the independent 256-spp native reference was 0.02302 and
0.02304. These asynchronous runs settled with different terrain selections.
The prepared horizon PNG and the fixed-key mesh parity test provide stronger
controls for the packing change.

The divisor-4 spiky capture also met the target and had 0.02167 linear color
RMSE against the same native reference. Its different tile selection and roof
heights change the image. The choice remains opt-in pending motion appearance
acceptance. The library test suite passed 812 tests, with 45 explicit
diagnostics ignored. The interior regression compares the reused packed
payload, shader profiles, bounds and certificate for a changed seam against
the full edge-sweep control and verifies that clearing the cache removes reuse.
The seam and water-face tests also pass with the divisor set to 4 and 8.

## Native box-splat comparison

The `box-splat` renderer projects canonical surface cells from the same
immutable brick snapshot used by the brick ray tracer. Each candidate draws a
screen rectangle. The fragment shader tests the actual cell and writes the
entry depth and exposed-face shading. The CPU builds the bounded cell list
without extracting a greedy mesh. The experiment is available on `fixture`.

The Radeon 760M run uses Mesa RADV 25.2.8, a Ryzen 5 7640U, 1920×1080,
MSAA1, 6.25 cm voxels, and the fixed 16×14×16 m fixture. Each timing row
uses a four-second uncapped run with 60 warmup frames. The GPU measurement
covers the opaque scene pass.

| View | Renderer | GPU p50 / p95 / p99, ms | Frame p50 / p95 / p99, ms |
| --- | --- | ---: | ---: |
| Overview | Greedy mesh | 0.787 / 0.848 / 0.952 | 1.514 / 2.628 / 3.604 |
| Overview | Brick rays | 3.218 / 3.279 / 3.310 | 3.999 / 5.591 / 6.393 |
| Overview | Box splats | 0.650 / 0.682 / 0.703 | 1.383 / 2.308 / 3.145 |
| Cave | Greedy mesh | 1.004 / 1.049 / 1.074 | 1.654 / 2.657 / 3.736 |
| Cave | Brick rays | 4.968 / 5.056 / 5.108 | 5.744 / 8.141 / 8.808 |
| Cave | Box splats | 1.185 / 1.244 / 1.286 | 1.877 / 2.850 / 3.626 |
| Bridge | Greedy mesh | 0.779 / 0.830 / 0.864 | 1.445 / 2.437 / 3.363 |
| Bridge | Brick rays | 3.618 / 3.659 / 3.695 | 4.308 / 5.949 / 7.276 |
| Bridge | Box splats | 1.053 / 1.095 / 1.142 | 1.725 / 2.688 / 3.627 |

The first shader used full-screen rectangles for cells crossing the near
plane. The cave pass reached 35.42 ms GPU p95. Conservative clip-space
bounds for those cells, camera-facing exposure rejection, and a four-vertex
strip brought it down to 1.24 ms. The completed data set has 116,711 submitted
surface cells and uses about 1.78 MiB of splat GPU buffers.

The cave and bridge float32 captures match brick rays in color and depth.
All three views have identical hit coverage to brick rays. One overview
pixel chooses a different face normal at a shared edge, with depth one
float32 step apart. All three backends completed the add-and-restore transaction
in a separate 17-second run. Box-splat publication took 1.33 s for the addition
and 1.52 s for the restoration, including the full fixture snapshot rebuild.
Terrain streaming, GPU cell generation and LOD handoffs require separate
comparisons.

```sh
python3 tools/phasea_volume_check.py --output output/box-splat-visual \
  --backends mesh brick box-splat --views overview cave bridge --capture-only
python3 tools/phasea_volume_check.py --output output/box-splat-timing \
  --backends mesh brick box-splat --views overview cave bridge --edits --edit-seconds 17
```

## Live Alpine coarse projection

`FRONTIER_COARSE_PROJECTION=quads|boxes` enables the comparison on the
seed-sampled coarse terrain path at MSAA1. Omit the variable for the ordinary
packed-quad control. Both candidates read the same trimmed column records.
The vertex shader derives roof and wall quads in `quads` mode. In `boxes` mode
it rejects back-facing columns and draws one projected rectangle per candidate.
The fragment shader resolves the actual exposed plane, depth and material.
Both modes bind the existing appearance profiles and use the same independently
merged raster seam strips. Exact NAADF terrain and the shadow path retain their
current representations. If optional GPU memory would prevent normal terrain
publication, the experiment falls back to the retained packed quads.

The experiment still builds the source runs and the original packed quads on
the CPU. Keeping those quads supplies the existing shadow and fallback paths.
This measures GPU-side primitive expansion and drawing in the live stream.
Fully GPU-resident source generation and compacted indirect emission are
separate experiments.

Measured on the Radeon 760M at 1920×1080, seed 7 and MSAA1. The modes use the
same release binary, the 15-second Alpine walk above, and the stationary 6 km
horizon with the 6 px refinement reserve. Disk caching was disabled.

| View | Mode | Frame p50 / p95 / p99, ms | Opaque GPU p50 / p95 / p99, ms |
| --- | --- | ---: | ---: |
| Walk and turn | Packed quads | 25.159 / 31.553 / 39.430 | 20.346 / 21.818 / 24.557 |
| Walk and turn | GPU-expanded quads | 24.726 / 32.450 / 45.276 | 20.652 / 22.382 / 34.539 |
| Walk and turn | Projected boxes | 25.332 / 39.835 / 46.838 | 21.366 / 23.073 / 33.997 |
| 6 km horizon | Packed quads | 20.046 / 22.936 / 24.084 | 15.235 / 15.354 / 15.383 |
| 6 km horizon | GPU-expanded quads | 29.501 / 31.280 / 31.680 | 24.065 / 24.212 / 24.242 |
| 6 km horizon | Projected boxes | 29.514 / 30.757 / 31.733 | 23.948 / 24.055 / 24.114 |

All modes reached the horizon reserve in 20.1–20.3 s and held it for all 189
frames. The walk had published coverage in every measured frame. No mode met
its 3 px error target on the walk. At the settled horizon, each candidate drew
450,176 columns and 762,461 seam quads across 1,958 visible tiles. The extra
resident payload was about 66 MiB and total upload volume grew from 168 MiB to
about 307 MiB. The packed-quad color path remains the default.

At the matched fixed 6 km pose with atmosphere and shadows off, the packed
control covered 1,285,782 pixels. The quads lost four pixels inside that
coverage and gained 33 pixels in total. The boxes lost 12 interior pixels and
gained 39 pixels in total. With unmerged strips and interpolated box-rectangle
rays the earlier box pass lost 243 interior pixels. The fixed spiky reference
exercised the retained raster path at this experiment's current tile limits.

Local evidence is under `output/coarse-projection-refined-*` and
`output/coarse-projection-comparison`. Repeat the walk and horizon commands in
this file with `FRONTIER_COARSE_PROJECTION=quads` and then `=boxes` to compare.

### Finished-depth occlusion probe

The probe captures the published GPU column records and their exact draw
cameras alongside a headless MSAA1 depth image. It rejects back-facing and
off-screen boxes first. A maximum-depth pyramid checks a padded rectangle
against the nearest possible depth of its box. The optional exact rectangle
scan separates limits of the pyramid from limits of the box bounds.

At the same Alpine 6 km pose with the 6 px target, 449,920 columns were
submitted by 1,957 tiles. Facing and frustum checks rejected 213,297.
Finished-frame depth rejected 48,082 of the remaining 236,623 via the depth
pyramid. That accounts for 14.0% of their summed padded rectangle area.
Scanning every rectangle pixel found 17,549 additional fully occluded
rectangles. Together they account for 21.8% of the area. A per-pixel depth
check against each box's nearest bound could reject 32.9% of that area,
including pixels covered by boxes with partly visible rectangles. This is
rectangle coverage with overlap and padding, so the percentages do not
measure GPU fragment invocations or frame time. The probe uses finished
depth and does not pay the cost of building live occluders.

To reproduce, use the 6 km camera and source settings above. Switch the
window benchmark to a fixed `--capture-linear --capture output/probe.png`,
set `--pixel-error 6`, and set `FRONTIER_COARSE_PROJECTION=boxes` and
`FRONTIER_COARSE_VISIBILITY_PROBE=1`. Keep MSAA1 and disable atmosphere and
terrain shadows for the same depth capture.

```sh
python3 tools/coarse_visibility_probe.py output/probe.png --exact-rectangles \
  --output output/probe-rejection.json
```
