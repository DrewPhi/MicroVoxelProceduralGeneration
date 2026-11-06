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
  fallback. Speculative refinement is bounded and can share one preparation slot after
  current requested quality passes; it does not establish successful motion by itself.
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
#72 supports bounded versioned restarts and corruption fallback, but edit identity
currently invalidates conservatively rather than providing per-dependency local
persistence reuse. #64 still fails moving quality; #73 still fails wide-view
latency/60 FPS and needs extended independent long-range reference crops.

The next substantive work is publication/visibility scheduling and generation
throughput for changing views, followed by reducing thousands of per-cluster
draw submissions. A larger GPU may help drawing but cannot be assumed to remove
these CPU and preparation limits. Do not close failed quality gates based on
fast fallback frame times.
