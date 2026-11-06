# Phase A: integrated GPU appearance profiles

Status: opt-in implementation with bounded integrated measurements below. This extends
the [standalone computation probe](GPU_PROFILES.md), whose kernel timings are
not integrated loading or frame-time results. Defaults remain unchanged. Reuse
tier G: new Rust/WGSL implementation; literature below supplies guidance only,
with no copied implementation code.

Decision: retain the experiment, with defaults unchanged. Three paired runs
did not establish a readiness improvement; the median stream first-quality
marker was 10,238.857 ms on CPU and 10,381.163 ms with GPU profiles. This marker
starts with the adaptive stream, not process launch. Runtime source packing
is useful, but the current small-batch integration still increases retained
CPU payload, transfers and moving-frame tails. This does not reject GPU
appearance preparation or settle the broader architecture choice.

## Implementation scope

`FRONTIER_GPU_PROFILES=1` defers appearance integration for eligible adaptive
tiles: FBM heightfield source, no topology owner, no edits, world and tile source
revision zero, width exactly 128 native cells, and a step of at least eight
cells that divides the width and does not exceed it. Native geometry and other
tiles keep their CPU painter. Unsupported snapshots, including more than 1,024
cells of relief, fall back to the CPU appearance path.

Canonical source evaluation, geometry, extrema and error certificates remain
on the CPU. Geometry/certification fills the local canonical cache first; the
owned 128×128-column snapshot plus one-column halo reuses it and queries the
shared source cache for missing inputs. Snapshot time and newly generated
columns are additional costs, separately counted. This does not move terrain
noise to the GPU or remove canonical source work.

Each deferred tile retains an immutable source snapshot and descriptors across
CPU cache eviction and asynchronous publication. The renderer lazily creates
resident compute scratch. A 64-lane workgroup per profile accumulates exact
integer axis/material counts and then forms palette-weighted float colors,
preserving the existing 80-byte profile ABI and slot-zero exact-face fallback.
Eligible tiles skip the corresponding CPU appearance traversal and profile
upload. Compute results copy directly into the tile's pooled profile table;
the integrated path allocates no diagnostic readback or timestamp buffers and
does not map results back to the CPU.

The scratch retains at most eight canonical source pages in CPU/GPU slots.
Reuse requires exact source identity and payload equality; replacement cycles
through slots. Each tile's upload, dispatch and profile copy are submitted
before a later tile can overwrite scratch inputs. Persistent draw tables keep
their own results. Existing source-revision/publication checks and pooled-table
lifetimes still govern presentation. Eight slots bound this additional scratch
cache, not the independent source cache, tile snapshots, staging allocations or
total GPU residency.

The candidate currently bypasses all derived disk-cache reads; deferred tiles
also skip disk writes, while unsupported CPU tiles may still be written. Fair
paired runs must disable disk caching in **both** arms; a CPU disk hit versus a
candidate rebuild measures different work. In-memory/source-cache warmness must
also be recorded explicitly.

## Reproduction and experimental gates

Build and freeze the intended binary before measuring. The iteration harness
does not build; it copies both binaries before launching either arm, hashes
them, records allowlisted environment overrides, and resets all tracked
experiment settings unless explicitly supplied. From `frontier/`, inspect the
frozen primary still command first:

```sh
python3 tools/phasea_iteration_check.py --dry-run --headless \
  --baseline-env FRONTIER_GPU_PROFILES=0 \
  --candidate-env FRONTIER_GPU_PROFILES=1 \
  --output output/phasea/integration-primary
```

Remove `--dry-run` only for the hardware capture. Use a new output directory
per run. Keep the frozen Legacy seed/source/camera, 6.25 cm voxels, 1920×1080,
300 m clip, MSAA1, filtering and lighting from
[PRIMARY_VISUAL_CHECK.md](../../PRIMARY_VISUAL_CHECK.md). These commands do not
enable disk caching. Both arms must use the same binary and all other settings.

For this bounded appearance-only pass, require identical depth and coverage;
report byte RGB and linear-light RGB differences and inspect the images.
Tiny color-rounding differences are expected from reordered float accumulation,
but must be measured rather than assumed harmless. This experimental gate
does not approve a new production visual target, geometry approximation or
renderer. Missing appearance provenance remains unavailable, and motion endpoint
images remain unmatched trajectory frames.

After the primary check, use the same paired harness without `--headless` for
settled static windows, then separately labelled `--route turn-return --speed
3`, `15`, and `80` runs. Record original reports, logs, hashes and machine/GPU
details. Whole-frame p50/p95/p99 include simultaneous drawing; enqueue CPU time
is not GPU execution time. `cold_ready_ms` uses the actual first-quality event,
`adaptive.first_quality_ms`, and remains null when missing. Report-time elapsed
stream duration is not cold readiness. Preserve source/profile counters and
logical scratch/upload/copy scopes separately from total memory, which remains
unavailable unless independently measured. Temporal acceptance still needs
matched trajectory frames and inspection of holes, shimmer and transitions.

## Sources and limits

The official [wgpu 24.0.5 Queue documentation](https://docs.rs/wgpu/24.0.5/wgpu/struct.Queue.html#method.write_buffer),
verified locally in
`/tmp/voxel-cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-24.0.5/src/api/queue.rs`,
documents deferred `write_buffer` transfers before explicitly submitted commands
and temporary staging costs. It motivates submitting each scratch batch before
reusing its inputs; a fast enqueue call does not establish completed GPU work.

AMD's [Optimizing histogram in HIP](https://rocm-handbook.amd.com/projects/amd-rocm-optimization-guide/en/latest/patterns/examples/histogram.html)
explains workgroup-local histogram accumulation and contention. It informs the
integer-count strategy; this implementation uses its own WGSL profile layout
and reduction. Published HIP behavior and timings do not establish performance
on this renderer or laptop.

Paired observations below do not establish production visual approval, a memory
saving or a general performance win.

## Lossless packed snapshot follow-up

The current implementation packs each canonical column into four bytes: an
11-bit relative height and three 3-bit material IDs. The unchanged 1,024-cell
relief bound and supported material IDs make this encoding lossless. A page is
67,600 payload bytes; eight retained CPU pages total 540,800 bytes. Fixed logical
runtime GPU scratch is 598,224 bytes, excluding staging, draw tables and driver
allocations. These allocation sizes do not establish total-memory savings.

The packed primary manifests record frozen binary SHA-256
`830cc579cbced65a7fece2b51fbb9cdc9bc32df56fa4ed3b9733255e9e5362e6`
for both arms. Every arm's PNG, float linear RGB, depth and coverage buffer is
byte-for-byte identical to its corresponding unpacked primary buffer. Raw
metadata and the eight SHA-256 comparisons are preserved under
[integration-results.json](integration-results.json), experiment `packed-primary`.
Thus packing preserves the initial primary appearance result, including its
small CPU-versus-GPU rounding differences and incomplete headless provenance.

Group every result by the
actual `FRONTIER_GPU_PROFILES` setting in its manifest provenance: static pair
two deliberately reverses arm labels (`baseline=1`, `candidate=0`). Arm names
alone therefore do not identify the computation path.

### Three packed static pairs

All six runs use the packed hash above and the same settings as the initial
six-second static pair, with profile-center reuse left at its default. Raw JSON
is retained in [integration-results.json](integration-results.json), experiments
`packed-static-{1,2,3}`; its `summaries.static-summary` preserves the
actual environment grouping and extracted scopes. All process errors are empty.
Tests reported for this implementation: 324 Rust tests passed, ten ignored;
31 Python tests passed. Motion and the separate CPU-center-reuse ablation below
are not included in these static results.

| Pair | Actual profile mode | First quality, ms | Frame p50 / p95 / p99, ms | Render GPU p50 / p95 / p99, ms |
|---|---|---:|---:|---:|
| 1 | CPU (`0`) | 10,791.420 | 4.857 / 6.630 / 6.932 | 4.099 / 4.309 / 4.470 |
| 1 | GPU (`1`) | 10,174.740 | 4.952 / 6.731 / 7.129 | 4.151 / 4.407 / 4.527 |
| 2 | CPU (`0`, candidate arm) | 10,133.692 | 4.902 / 6.674 / 7.211 | 4.141 / 4.344 / 4.572 |
| 2 | GPU (`1`, baseline arm) | 10,381.163 | 4.952 / 6.798 / 7.243 | 4.173 / 4.375 / 4.606 |
| 3 | CPU (`0`) | 10,238.857 | 4.943 / 6.902 / 7.526 | 4.154 / 4.467 / 4.757 |
| 3 | GPU (`1`) | 10,486.333 | 5.034 / 7.881 / 9.232 | 4.200 / 4.447 / 4.594 |

The median first-quality observations are 10,238.857 ms CPU and 10,381.163 ms
GPU. GPU first quality improves in pair one and regresses in pairs two and
three; frame p50/p95/p99 all rise within every pair. Six settled windows do not
identify causes or establish a general performance ranking, but these paired
observations do not demonstrate a readiness or frame-time win.

The following scoped counters are final or cumulative values from one run,
summarized as median [minimum–maximum] across the three runs of each mode.
They are not frame distributions, and byte scopes must not be added when they
overlap.

| Metric | CPU (`0`) | Packed GPU (`1`) |
|---|---:|---:|
| Final live tile payload, bytes | 406,299,632 | 502,884,336 |
| Selected CPU payload, bytes | 145,784,240 | 235,623,824 |
| CPU cache bytes | 440,005,504 [440,002,272–440,020,608] | 536,489,720 [536,480,344–536,493,784] |
| Source-cache columns | 1,476,737 [1,475,713–1,476,801] | 1,477,009 [1,475,953–1,477,009] |
| Generated canonical columns | 33,833,538 [33,832,698–33,837,978] | 33,397,787 [33,387,806–33,397,824] |
| Source-cache hits | 18,529,617 [18,525,101–18,529,688] | 18,345,746 [18,345,253–18,355,306] |
| Derived generated + cache-hit requests | 52,363,079 [52,362,315–52,363,226] | 51,743,112 [51,743,040–51,743,570] |
| Combined appearance + certificate CPU stage, ms | 22,400.784 [22,000.083–23,195.357] | 21,764.157 [20,382.284–21,791.645] |
| Snapshot CPU time, ms | 0 | 402.275 [378.596–411.192] |
| Snapshot-generated columns | 0 | 856 [846–858] |
| Cumulative uploads, bytes | 340,892,464 [340,719,600–341,207,472] | 508,924,768 [504,762,208–509,017,536] |
| Scoped GPU allocated bytes | 150,994,944 [142,606,336–150,994,944] | 143,204,560 [143,204,560–151,593,168] |
| Profile source uploads, bytes | 0 | 158,589,600 [156,020,800–158,657,200] |
| Profile job/fallback-zero uploads, bytes | 0 | 7,060,960 [6,955,520–7,063,088] |
| GPU result copies / CPU profile uploads avoided, bytes | 0 | 17,367,520 [17,108,480–17,372,720] |
| Profile enqueue CPU time, ms | 0 | 271.550 [257.788–272.549] |
| Compute initialization CPU time, ms | 0 | 2.429 [2.128–2.554] |

Each GPU run retained 598,224 logical scratch GPU bytes and 540,800 CPU source
cache bytes with zero readback; GPU profile execution duration remains null.
There were 2,336–2,375 dispatches, 211,520–214,784 profiles and 28 source-page
reuse hits per run. Final resident cluster count was 1,687 in every arm.

Source work belongs to the CPU stage making the first request. Skipping CPU
appearance can move generation into certificate work, so appearance reduction
alone cannot be read as source savings. Even the combined worker-stage sums
exclude separately reported snapshot overhead and are not wall latency. Cache
population changes with retained tile memory; fewer generated columns do not
prove a new source algorithm or equivalent cache work. The packed path still
adds 96,584,704 bytes of final live tile payload and substantial upload traffic.
Logical allocation variation also reflects pooled-buffer capacity and does not
measure whole-process residency. No default change, production promotion or
issue closure follows from this experiment.

### Separate profile-center-reuse ablation

One additional static pair explicitly enabled `FRONTIER_PROFILE_CENTER_REUSE=1`
in both arms, leaving all other settings matched. It is preserved separately as
`center-reuse-static` in [integration-results.json](integration-results.json).
CPU/GPU first quality was 10,899.168 / 9,707.336 ms; frame p50/p95/p99 was
4.934/6.906/7.797 versus 4.939/6.574/7.069 ms. Combined appearance+certificate
worker time was 23,154.213 / 19,688.675 ms, plus 373.297 ms snapshot time on
the GPU path. Generated source columns were 33,834,471 / 33,402,000, while
source-cache hits were 18,527,694 / 18,341,342. Live CPU payload remained
406,299,632 / 502,884,336 bytes.

This single pair favors GPU readiness and frame tails, unlike the three default
center-setting pairs. It supplies a CPU alternative comparison, but neither
isolates the effect of center reuse nor establishes its query savings: both
arms enabled it, and independent runs vary. Keep this observation separate
from the three-pair group and benchmark stronger CPU reuse alongside future GPU
batching changes.

### Six-second realtime turn-return routes

Each 3/15/80 m/s pair retained the original default center setting, initial
quality wait and 60 warmup frames; route duration was six seconds. All use the
same packed binary hash above. Raw reports and manifests are preserved as
`motion-3`, `motion-15` and `motion-80` in the consolidated evidence. These are
one pair per speed; realtime endpoints and sample counts differ. No endpoint
pixel/depth comparison establishes temporal equality.

| Speed, m/s | Profile mode | Frame p50 / p95 / p99, ms | Coverage-ready frames | Quality-ready frames | Unbounded-error frames | Initial first quality, ms |
|---|---|---:|---:|---:|---:|---:|
| 3 | CPU | 2.460 / 6.455 / 8.992 | 1826/1826 (100%) | 89/1826 (4.87%) | 553/1826 (30.28%) | 10,252.553 |
| 3 | GPU | 2.566 / 6.449 / 9.765 | 1756/1756 (100%) | 104/1756 (5.92%) | 505/1756 (28.76%) | 9,559.613 |
| 15 | CPU | 3.038 / 5.940 / 8.584 | 1750/1750 (100%) | 0/1750 (0%) | 114/1750 (6.51%) | 10,042.144 |
| 15 | GPU | 3.049 / 6.373 / 10.884 | 1694/1694 (100%) | 0/1694 (0%) | 117/1694 (6.91%) | 9,752.187 |
| 80 | CPU | 2.696 / 5.064 / 6.728 | 1885/1885 (100%) | 0/1885 (0%) | 521/1885 (27.64%) | 10,038.052 |
| 80 | GPU | 2.735 / 5.282 / 9.001 | 1781/1781 (100%) | 0/1781 (0%) | 462/1781 (25.94%) | 9,733.705 |

Quality-cycle latency reports contain only one sample per run: CPU/GPU p95
values are 11,676.480/10,857.928 ms at 3 m/s,
11,478.248/11,049.597 ms at 15 m/s and 11,419.996/11,093.305 ms at 80 m/s.
Their p99 is unavailable. These largely capture initial preparation, not a
distribution of completed moving-view quality cycles. Finite visible error
maxima reach millions of pixels in both arms; peak unresolved clusters are
109/109, 129/130 and 187/190 respectively. Coverage alone therefore does not
meet the geometric/temporal quality gate. Pending GPU profile work does not by
itself explain these error values or establish their cause.

The GPU path's frame p99 rises at each speed. Its shorter first-readiness
observation does not compensate for unresolved moving-view quality, extra
memory or transfers. This result evaluates the current batch/cache integration,
not the viability of GPU appearance construction as a strategy. Follow up with
greater canonical source/page reuse and batching across appearance compute and
CPU certification, compared against stronger CPU center reuse. Preserve full
quality and latency evidence while improving those scopes; defaults stay as
they are and no issue is closed by this pass.

## Initial unpacked snapshot iteration

These initial results use the older 16-byte column snapshots, before the
separate lossless packing follow-up. They must not be attributed to a later
binary. Both frozen arms in both manifests report SHA-256
`14d195c8f2065dd844c4a8f730563a51e4751d187dd52f9dcfd393d0b2bf97cb`.
Raw manifests, comparisons, camera/sidecar metadata and static benchmark JSON
are preserved under [integration-results.json](integration-results.json), keyed by experiment and original filename.
Full captures and logs remain in `output/phase-a-integration/primary` and
`output/phase-a-integration/unpacked-static`.

The frozen headless primary comparison has zero depth error and zero coverage
disagreement on 966,894 common-hit pixels. Thirty-three of 2,073,600 RGB pixels
differ, by at most one byte per channel. PNG-derived linear-light RMSE is
5.4756e-6. The settled static endpoint independently has zero depth error and
coverage disagreement on 967,122 common hits, with 14 differing RGB pixels,
maximum one byte, and PNG-derived linear-light RMSE 3.6034e-6. These are raw
buffer diagnostics: missing appearance/lighting provenance keeps the headless
automated evidence incomplete. Static benchmark metadata supplies provenance
for its comparison, but unavailable total-memory measurements remain incomplete.
No new visual approval follows.

The static pair used the Ryzen 5 7640U and Radeon 760M (RADV PHOENIX, Vulkan,
Mesa 25.2.8-0ubuntu0.24.04.2), four preparation workers, disk cache disabled,
initial quality wait, 60 warmup frames and a six-second uncapped static window.
This is one sequential pair, not a repeat distribution or motion test.

| Metric | CPU profiles | Unpacked GPU profiles |
|---|---:|---:|
| First quality readiness, ms | 10,681.369 | 10,300.671 |
| Frame p50 / p95 / p99, ms | 4.924 / 6.732 / 6.928 | 4.880 / 5.488 / 7.177 |
| Final live tile payload, bytes | 406,299,632 | 523,692,912 |
| Selected CPU bytes | 145,784,240 | 494,802,224 |
| Scoped GPU allocated bytes | 142,606,336 | 153,215,568 |
| Cumulative upload bytes | 341,230,592 | 985,742,240 |
| Generated canonical columns | 33,837,188 | 33,394,733 |

The candidate dispatched 2,360 batches / 213,440 profiles, uploaded 630,572,800
source bytes, copied 17,264,000 result bytes and avoided that same number of CPU
profile upload bytes. The eight-slot source cache had only 28 hits against
2,332 uploads. Resident compute scratch was 2,220,624 logical GPU bytes with
2,163,200 retained CPU source bytes; readback was zero and GPU duration was
unavailable. Snapshot work generated 873 additional columns and accumulated
426.557 ms of CPU time.

CPU appearance-stage sum fell from 19,710.603 to 8,208.678 ms while certificate
stage sum rose from 3,226.896 to 12,354.532 ms. Those worker sums include canonical
query work in the caller stage, so they do not establish exclusive shader or
source cost savings. The approximately 3.6% first-readiness reduction in one
pair accompanies materially greater retained CPU payload and upload traffic;
frame p99 increased. This motivates measuring the packed follow-up separately,
without claiming a loading, memory or frame-time win from this initial pair.

## Final correctness checks and next experiment

[Additional validation](integration-validation.json) preserves buffer hashes,
secondary commands/metadata and actual float32 capture comparisons. The primary
CPU-versus-GPU float32 linear-color RMSE is 4.5332e-8, maximum absolute channel
error 8.9407e-7. These are capture-buffer values, distinct from the PNG-decoded
metrics above. Rounding changes 223,743 float32 pixels while only 33 PNG pixels
cross a quantization boundary. Depth and coverage remain identical.

The cave fallback pair is byte-identical in PNG, float32 RGB, depth and coverage.
Combining GPU profiles with `FRONTIER_INDEXED_PROXY=2` also preserves all four
buffers relative to GPU profiles alone at the primary pose. Default-mode buffers
match the earlier frozen default capture exactly. These bounded tests do not
certify all scenes, edits or moving-frame equivalence.

Validation: 324 ordinary Rust tests passed, 10 remained ignored; 32 Python tests
passed. The ignored GPU numerical test was additionally run and passed with the
packed source format, including ordered/parallel kernels, exact areas and the
independent count/f64 oracle. Its [raw log](integration-gpu-correctness.log) is
from a debug build and is correctness evidence, not a performance comparison.
Formatting and diff checks passed.

The next useful Phase A experiment is a shared canonical-page lifetime and
batching design: retain each useful source page once, reuse it for certificates
and appearance, and amortize transfers/submissions across several jobs. Compare
that against the CPU reuse path with equal memory budgets and the same quality
target. Avoid retaining one extra page copy per display tile or treating faster
kernel execution as proof of faster streaming. Exact canonical geometry and
volume ownership remain independent of this derived-data representation.
