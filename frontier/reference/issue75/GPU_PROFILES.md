# GPU appearance-profile construction experiment

The parallel integer-count kernel is worth an integrated test. In this standalone
probe, its resident GPU work took **0.018/0.069/0.122 ms** for 64/256/512 primary
profiles. This is evidence for the computation strategy, **not a terrain-loading
speedup, visual approval, or renderer promotion**. No production path consumes
these computed profiles yet; defaults remain unchanged.

Raw evidence: [ordered-parallel-primary.log](ordered-parallel-primary.log),
SHA-256 `5566204c25c0ce09e79b6e5f3a5de2926b4327ba42655f5d91879dff0fda6f44`.
The final diagnostic passed its exact-area and numerical checks. This record
contains one bounded run, five resident repeats per primary case, and no frame
time distribution or simultaneous rendering measurement.

## Matched input and hardware

- AMD Radeon 760M Graphics, RADV PHOENIX, Vulkan, Mesa
  `25.2.8-0ubuntu0.24.04.2`, Ryzen 5 7640U host. Executable SHA-256 recorded
  immediately after the run:
  `b68db5fd486903ec7f98d9f592950991f35a55d886e36e4740586ba486a129f1`.
- Primary Legacy source: seed 7, horizontal scale 1,800 m, amplitude 1,000 m,
  eight octaves, roughness 0.5, revision 0, native voxel size 0.0625 m.
- Each page contains 128×128 canonical columns plus a one-column halo. Minima in
  native cells are `[-512,-512]`, `[-384,-512]`, `[-256,-512]`, `[-128,-512]`,
  `[0,-512]`, `[128,-512]`, `[256,-512]`, `[384,-512]`. The 1/4/8-page batches
  use the corresponding prefixes. All accepted the unchanged 1,024-cell relief
  bound; measured relief was 102–356 cells.
- Each page supplies 64 disjoint 16×16, stride-one appearance jobs. Both kernels
  use identical CPU-prepared canonical samples and descriptors. These addresses
  exercise the primary source recipe; they are not a camera-selected primary
  render, route, or coverage test.

The cache-only API rejects a missing column or unsupported ownership/source
snapshot. For the diagnostic, its callback explicitly generates canonical CPU
columns and charges that preparation separately. There is no GPU noise/source
implementation and no integration with the runtime's actual source cache.

## Construction and costs

`Ordered` assigns one invocation to each profile and retains CPU z/x/axis/span
accumulation order. `ParallelCounts` assigns one 64-lane workgroup per profile,
accumulates 25 exact integer axis/material counts, then forms palette-weighted
RGB sums. Both produce the existing 80-byte profile ABI; slot zero remains the
exact-face fallback. Resident encoding can feed a storage binding without
readback. The diagnostic maps results solely for verification.

All times below are milliseconds. GPU and diagnostic wall columns are medians
of five resident repeats. CPU and source columns are single observations, not
medians. Diagnostic wall is the paired sum of encode/submit and wait/readback;
GPU duration is contained within that wall time and must not be added again.

| Pages / profiles | CPU source preparation | CPU ordered profiles | CPU count/f64 oracle | Ordered GPU | Parallel GPU | Ordered diagnostic wall | Parallel diagnostic wall |
|---|---:|---:|---:|---:|---:|---:|---:|
| 1 / 64 | 4.594 | 0.192 | 0.198 | 0.639 | 0.018 | 0.832 | 0.147 |
| 4 / 256 | 15.008 | 0.735 | 0.588 | 0.758 | 0.069 | 1.117 | 0.255 |
| 8 / 512 | 31.990 | 2.144 | 1.519 | 0.809 | 0.122 | 1.186 | 0.378 |

Cold costs remain material. Parallel pipeline/allocation calls took
16.505/2.521/1.424 ms and upload enqueue calls took 2.378/0.708/0.322 ms for the
three batches. These are CPU call durations; GPU copy and initialization may
also affect the subsequent cold submission. Cold parallel GPU timestamps were
0.022/0.074/0.122 ms. Source preparation still dominates the largest cold batch;
none of these measurements demonstrates reduced integrated loading latency.

Fixed logical GPU buffers total **2,261,696 bytes** with timestamps, including
both diagnostic readbacks. This excludes pipeline/driver allocations and upload
staging. Each retained CPU canonical page has 270,400 bytes of payload; page
construction temporarily holds sampled-column conversion data, and any runtime
source cache would be additional. Workgroup histogram scratch is 100 bytes per
workgroup. This is a bounded 128-column-domain experiment, not support for all
large sparse adaptive footprints.

The synthetic cases contain one full 128×128 job and one offset 16×16 stride-two
job, with alternating Grass/Wood tops, Soil/Stone bands, and either 1,024-cell
relief or flat heights. Ordered runs once; parallel has five resident repeats.

| Synthetic case | CPU ordered | Ordered GPU, single cold | Parallel GPU, warm median |
|---|---:|---:|---:|
| High relief | 0.359 | 35.389 | 0.627 |
| Flat / zero side area | 0.127 | 21.316 | 0.534 |

Parallel construction removes this serial large-profile pathology, but still
loses to the CPU computation for these two-job cases. Batching/granularity and
contention matter; this does not establish a universal GPU win.

## Numerical result

Every reported face area, including zero side areas, exactly matched both the
ordered CPU painter and independent integer-count oracle. The oracle uses
clipped span lengths, u64 counts, and f64 palette sums. It separates mathematical
color accuracy from the baseline's repeated f32 accumulation.

Ordered matched CPU bits on all primary profiles. Parallel differed in
608/2,388/4,817 channels for the three batches. Across the primary batches its
maximum RGB relative difference from ordered CPU was **3.82e-6**, and maximum
area-normalized absolute color difference was **1.05e-6**. Against the count/f64
oracle, parallel's maxima were **9.52e-8** relative and **3.19e-8** normalized.
These changed bits reflect accumulation order; the histogram result was closer
to the mathematical palette sum in these measurements.

The high-relief synthetic exposes larger CPU rounding: parallel differs from
ordered CPU by 4.15e-4 relative / 1.12e-4 normalized, while differing from the
f64 oracle by only 5.26e-8 relative / 1.69e-8 normalized. Explicit probe bounds
were exact area, at most 5e-4 relative and normalized CPU disagreement, and at
most 2e-6 relative and normalized parallel/oracle disagreement. These are
numerical diagnostic bounds, not accepted image or temporal thresholds.

## Next integrated test

Keep the candidate opt-in. Feed a bounded batch from the existing revision-keyed
canonical column cache, retain source/profile allocation until the matching
world revision can publish, dispatch once when content changes, and bind the
result directly for drawing. Remove the corresponding CPU profile traversal and
upload from that candidate rather than computing both in the measured path.
Retain the CPU path for fallback and separately captured comparisons.

Run the frozen primary player-height 300 m RGB/depth comparison first, preserving
all source, camera, appearance and sampling settings. Then measure cold/warm
loading and 3/15/80 m/s routes with simultaneous drawing: source queries, CPU
profile work actually removed, transfer/staging bytes, GPU contention, publication
latency, resident/pending memory, and whole-frame tails. Preserve canonical
extrema/certificates and explicit volume ownership; this profile-only probe did
not compute or validate them. Full images and motion must assess whether changed
float sums affect the approved appearance.

Reproduce the standalone diagnostic from `frontier/` after building with the
configured toolchain; it is ignored during normal tests:

```sh
cargo test -p frontier-renderer --release --offline \
  gpu_ordered_profiles::tests::measure_gpu_ordered_profiles -- --ignored --nocapture
```
