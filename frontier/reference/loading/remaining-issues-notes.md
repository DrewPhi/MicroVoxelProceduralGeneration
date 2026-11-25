# Loading follow-up evidence

Local development evidence for [#69](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/69),
[#71](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/71),
[#72](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/72) and
[#73](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/73).
Read alongside [loading interpretation](../../LOADING_OPTIMIZATION.md),
[disk format](../../DERIVED_DISK_CACHE.md), [GPU probe](../../GPU_SUMMARY_PROBE.md)
and [reference contract](../README.md). The scoped profiling/reuse (#69), bounded
GPU feasibility (#71) and disposable persistence (#72) outcomes are documented
separately from promotion to a production GPU source path. The GPU summary stays
experimental; movement (#64) and wider-view (#73) acceptance still fail their
strict quality/performance gates. [Tracked follow-up reports](followup-measurements.json)
preserve completed run commands, explicit environment, source/camera/adaptive
telemetry, RGB hashes and failed numerical screens. Generated artifacts below
remain in ignored `output/`.

## Local disk dependencies (#72)

`FVDC0003` / representation 3 separates global source/seed/recipe/parameter,
voxel/material/appearance and topology identity from tile dependencies. A sorted
exact edit/material list covers the tile's column footprint plus one native-column
halo, including corners and all vertical coordinates. Global revision counters
are excluded from persisted content identity. Full identity bytes are validated
before decoded geometry receives the requested current revision. Caller revision,
source, ownership and tile eligibility checks still apply before indexed IO.

The current surface builder supports edits inside its separate topology volume
owner; exterior surface tiles sample the immutable FBM base. Halo invalidation is
conservative for these tiles. Arbitrary heightfield edits and persisted volume-owner
geometry remain unsupported. Recreating a cache for a new immutable snapshot can
reuse distant tiles, without making disk geometry canonical world authority.

Dedicated asynchronous IO, bounded queues/encoding/decoding, checksums, atomic
replacement, disk eviction and corruption regeneration remain in force. Snapshot
identity is capped at 4,096 edits. CPU tests passed all 19 disk/writeback cases,
including current-build parity, stale-revision rejection, unaffected reuse,
halo/material identity, parameter changes, unsupported ownership and corruption.
The format-3 cold/restart pair below now supplies a current hardware checkpoint;
earlier format-2 timings remain historical. Edited dependency reuse remains
CPU-tested; the single-corrupt-record hardware check below supplies a separate
fault-recovery checkpoint.

Format-3 sequential cold/restart runs used the same current executable
`1fb671346b28efcbee79c31ae769182d55ee91c7375b654692007055dfbdf387`,
four workers, refinement batch 8, bundles enabled and a newly created 768 MiB
`output/remaining-disk3-cache` directory. Both retain the approved RGB hash
`49cd95fa33ba55d5eeca7b2b9bc036f4b160c4826ce4bf85824b7387c1e0691c`.

| Format-3 disk state | First requested quality | First reserve | Frame p95 | Geometry builds | Disk hits |
| --- | ---: | ---: | ---: | ---: | ---: |
| New empty directory | 10.553 s | 12.028 s | 9.978 ms | 4,578 | 0 |
| Fresh-process reuse | 2.004 s | 2.532 s | 9.883 ms | 0 | 4,578 |

Cold completed 4,578 writes / 344,754,296 bytes, with zero errors, skipped writes,
evictions or pending writes. Queue peaks were six entries / 6,815,744 payload
bytes. Warm loaded every product with zero builds, misses, writes or errors.
Both runs were quality-ready throughout their 705/715 measured frames.
Reports are under `output/remaining-disk3-{cold,warm}/far300-workers4/` and retained
in the tracked JSON. These are sequential process-restart results with an
OS-warm page cache; no storage-cache flush or edited-world hardware test occurred.

A later fault-injection run flipped the last payload byte of exactly one completed
cache record. `output/remaining-disk3-corruption.json` preserves its path and
before/after SHA256. The restart reported **4,577 hits, one invalid/miss, one
geometry build and one replacement write**, with zero IO errors. Reserve readiness
was 3.782 s and frame p95 9.947 ms; all 709 measured frames were quality-ready.
The approved RGB hash was unchanged. This later executable differs from the
cold/warm pair, so this is functional recovery evidence rather than a controlled
timing comparison. The tracked JSON retains both executable identities.

## Resident compute beside drawing (#71)

`GpuSummaryProbe::encode_resident` records the existing exact integer summary
kernel with output clears and no timestamps, submission, mapping, polling or
readback. One startup parity/readback check precedes the optional
`FRONTIER_GPU_SUMMARY_CONTENTION=1` diagnostic. Production canonical generation
and appearance remain CPU paths; probe results do not feed displayed geometry.
Four CPU probe tests passed; the manual GPU test remains explicitly separate.

One same-binary sequential pair on Ryzen 5 7640U / Radeon 760M, Vulkan/RADV,
1920×1080, MSAA4, 300 m, requested 5 px / reserve 4 px, four workers:

| Diagnostic | Probe off | Eight resident pages per frame |
| --- | ---: | ---: |
| Frame p95 | 9.929564 ms | 10.101726 ms |
| Isolated GPU median | 6.550005 ms | 6.626430 ms |
| First requested quality | 12.618690 s | 12.737774 s |
| First reserve | 14.573634 s | 14.768613 s |
| Quality-ready measured frames | 1476/1476 | 1450/1450 |

Reports: `output/remaining-primary/` and `output/remaining-gpu-contention/`.
Both sweeps record executable SHA256
`e93a878bb90e14a6e588d9205242f8ef99d74b25a619ceae2d74fcdc69ed2768`.
The diagnostic allocated 2,265,632 logical buffer bytes, ran 1,512 dispatches,
and reported 50.599 ms startup. Both RGB payloads match the approved
`49cd95fa33ba55d5eeca7b2b9bc036f4b160c4826ce4bf85824b7387c1e0691c` hash.
This pair characterizes bounded contention, without randomized repeats or a
production preparation-speedup claim. Cold upload/source costs, production
floating appearance parity and useful asynchronous consumption remain separate.

## Independent 1 km crops (#73)

The new landscape `--crop X,Y,WIDTH,HEIGHT` retains full viewport projection and
global pixel/sample hashes. Manifests distinguish output dimensions, full
viewport and pixel rectangle. Native traversal/source/cache semantics are unchanged.
All 16 reference CPU tests passed, including exact crop/full RGB, coverage and
depth-byte parity on an 8 m fixture and malformed-crop rejection.

One 1024 m native snapshot centered at `[0,-512]` was generated in 11.623 s;
logical snapshot memory is 1,079,334,228 bytes. Its single disk cache is
`output/remaining-reference-km/native-columns.bin`, identity `bde68bf498c137bb`.
Source: Legacy seed 7, scale 1800 m, amplitude 1000 m, eight octaves, roughness .5,
6.25 cm cells. The primary eye/target/FOV remain fixed; only far extends to 1000 m.
The viewport is 1920×1080 and sample seed 83. Each 64-spp crop rendered in
0.055–0.083 s after roughly one second of cache loading.

| 64×64 crop origin | 64-spp hit fraction | Per-pixel mean hit depth | 16→64 RGB RMSE |
| --- | ---: | ---: | ---: |
| Ridge `[1008,328]` | 0.4320526 | 842.70–889.14 m | 0.006386 |
| Valley `[736,528]` | 0.3704872 | 843.24–1028.22 m | 0.008467 |
| Slope `[928,508]` | 1.0 | 617.24–959.15 m | 0.009761 |

All six 16/64-spp manifests report zero `finite_domain_miss_samples`:
missed rays remain inside the known horizontal source footprint through their
near/far segment. Sky is therefore unambiguous for these crops. Ray distance can
exceed the 1000 m camera-forward clip. All hit-pixel mean distances exceed 300 m;
these references exercise the added distance rather than only the near foreground.
`output/remaining-reference-km/summary.json` preserves full diagnostics, timing,
coverage counts and provenance. Finite-sample RGB differences above are sensitivity
measurements, not error bounds. These three crops do not establish full-view,
20 km, motion, topology or 60 FPS acceptance.

Reproduce a crop with the existing matching cache (change rectangle/name for the
other two). The guard prevents accidental regeneration when the cache is absent:

```sh
test -f output/remaining-reference-km/native-columns.bin && target/release/landscape \
  --cache output/remaining-reference-km/native-columns.bin \
  --output output/remaining-reference-km/ridge64 \
  --terrain-recipe legacy --seed 7 --center 0,-512 --extent 1024 \
  --eye 0,94.8875,0 \
  --target -8.731370115810932e-08,94.83756238296628,-0.9987523555755615 \
  --fov-y 60 --near .025 --far 1000 --width 1920 --height 1080 \
  --crop 1008,328,64,64 --spp 64 --sample-seed 83 --threads 8
```

Matching GPU command, executed exclusively on the laptop session when measuring:

```sh
DISPLAY=:1 python3 tools/adaptive_motion_check.py \
  --output output/remaining-refine16/far1000-workers4 --static-only --settled --seconds 8 \
  --pixel-error 5 --cache-mib 512 --msaa 4 --native-bins on \
  --workers 4 --refinements 16 --far 1000
```

The script reads the primary camera manifest and keeps the source/settings fixed.
A quality timeout is a failure; retain its report instead of reporting settled
FPS. These MSAA4 window captures contain decoded 8-bit target RGB. The current
CLI restricts `--capture-linear` to headless `--capture` with MSAA1, so do not add
that flag to this baseline command or mix its render settings silently.

Compare full GPU image rectangles against the independent 64-spp references:

```sh
for crop in ridge valley slope; do
  python3 tools/landscape_crop_compare.py \
    output/remaining-reference-km/"${crop}64" output/remaining-refine16/far1000-workers4/static.png \
    --report output/remaining-refine16/far1000-workers4/static.json \
    --convergence output/remaining-reference-km/"${crop}16" \
    --output output/remaining-reference-km/"${crop}-gpu16.png"
done
```

The tool checks full camera/viewport/source/parameters/edit state, rejects incomplete
finite-domain misses, crops without resizing, and reports linear RGB differences
plus separate reference-sampling differences. It records candidate readiness and
color encoding. Geometry is explicitly unevaluated. Metadata rejection checks and
one historical 1 km ridge smoke comparison passed; the historical image is not
new-binary acceptance evidence. No GitHub issue state was changed.

## Current 1 km capture and crop appearance

The command above was measured with refinement batch 16 (four active workers),
8 seconds after reserve readiness, unchanged 1920×1080/MSAA4/5 px settings and no
disk cache. `output/remaining-refine16/sweep.json` records executable SHA256
`f54366de27eceb4f75d22ec6361fed3fa54173f06ff28b9eb535974731547b5e`.
The same source and primary camera metadata match all three independent crops.

First requested quality took **46.139 s**; first 4 px reserve took **60.040 s**.
All 504 measured frames retained requested quality, with maximum certified error
3.999775 px. Frame p50/p95/p99 were **13.729/16.886/17.352 ms**;
27 frames exceeded the 16.67 ms budget. Isolated GPU median was 12.630 ms.
The p95 screen therefore **fails the 60 FPS target**, despite substantial
historical readiness/frame improvement. Historical comparisons are sequential
checkpoints, not randomized same-binary trials. The selected cover had 11,989
owners, 5,259,469 reported triangles and 352,376,880 resident GPU terrain bytes;
GPU allocated pool was 369,098,752 bytes and CPU cache 536,588,656 bytes.
Summed worker build time was 164.054 s, which overlaps in wall time; planning
was 7.357 s across 464 calls. No full-view geometry acceptance follows from
readiness or the crop color measurements below.

| Current GPU vs 64-spp crop | All-pixel linear RGB RMSE | Reference-full-terrain RGB RMSE | Reference 16→64 RGB RMSE |
| --- | ---: | ---: | ---: |
| Ridge | 0.015381 | 0.004892 | 0.006386 |
| Valley | 0.036815 | 0.026830 | 0.008467 |
| Slope | 0.005246 | 0.005246 | 0.009761 |

Files `output/remaining-reference-km/{ridge,valley,slope}-gpu16.png` contain
side-by-side crops; matching JSON reports retain metadata, candidate quality,
quantization and reference-sampling limitations. `*-difference-x4.png` exposes
absolute color differences. Valley disagreement is larger than the other sampled
regions and needs inspection; RGB cannot attribute it uniquely to geometry,
filtering or shading. The reference-full-terrain mask uses finite sampled
reference coverage only; it is not independently matched GPU coverage. No depth
or topology pass is inferred from these MSAA4 images.

The comparator accepted actual capture metadata and rejected eight mutations
(incomplete domain, negative/overflowing rectangle, far clip, seed, edit revision,
source scale and viewport size). The lightweight checks are recorded in
`output/remaining-reference-km/comparator-validation.json`. No additional GPU
run or engine build was performed for this comparison.

## Motion checkpoints remain failed (#64)

The fixed-input turn-return checkpoint retained coverage at 3/15/80 m/s, but
requested quality passed only **76/2504, 0/2456 and 0/2748** measured frames.
The optional debt-prediction 3 m/s turn passed **13/2442**, so it is not enabled
as a general improvement. Its 3 m/s stop/resume route passed **260/1494** frames.
These results demonstrate coherent fallback rather than detail keeping up with
movement. The tracked JSON separates earlier pre-fix motion/stop checkpoints
from these completed runs and retains each failed gate. Exit zero only means
the benchmark process completed; it is not quality acceptance.

## Optional visible vertical-error bound

A later same-binary static pair used executable
`b3b033fe80dcba5e25ce59e77830009a7f7c24bc2581b940f4cd0d7d7740bf7e`,
render bundles on by default, and only `FRONTIER_VISIBLE_ERROR_BOUND=1` changed.
Default reserve readiness/frame p95 were **12.502 s / 9.902 ms**; optional-bound
values were **12.222 s / 10.098 ms**. Both met static requested quality, while
the optional selection changed the image hash. The default retained approved RGB;
the optional capture hash was
`bc3e3e98913d883e953f7d6847faa25a515b03df9e2f3d574c27d1976af4cfd9`.

The optional 3 m/s turn retained coverage and recorded zero unbounded-error
frames, but only **80/2473** requested-quality frames (frame p95 8.268 ms).
This small quality fraction still fails movement acceptance, so the option remains
off by default. The motion ledger lacks an executable hash; none is invented.
Reports are `output/remaining-{final-primary,visible-bound,bound-motion}/`.
The forthcoming center-reuse same-binary comparison remains explicitly pending
in the tracked JSON and contributes no performance or acceptance claim here.


## Final experiment selection and distance limits

The final defaults use eight refinement selections per transaction, at most four
active builders, prepared-view reuse and render bundles. Debt prediction,
visible-part projection, center-column material reuse and the GPU summary probe
remain opt-in. The last center-column A/B was byte-identical but did not establish
a speedup (11.713 s control versus 12.304 s candidate), so it was not promoted.

The 16-refinement 1 km checkpoint reached reserve in 60.040 s with frame p95
16.886 ms, versus the preceding 93.836 s / 30.633 ms checkpoint. The later
center-reuse candidate recorded 58.377 s / 16.779 ms, but that does not isolate a
benefit from the material change and is not the final default. These p95 values
still miss the strict 16.67 ms target. Final source, appearance and thresholds
remain unchanged.

The 4/20 km pre-accounting-fix runs failed the 120 s readiness gate, with no
settled frame samples. Their complete failed reports are retained alongside
successful-process reports. They exposed a payload-only replacement-size check
that omitted newly charged cache metadata. The fix includes all target-owner
metadata, reuses committed tiles even after cache eviction, and re-evaluates
oversized stale demand for the current camera. It does not increase the budgets
or certify a coarse fallback as adequate detail.

Validation before the final hardware checks: 272 Rust tests passed, seven manual
diagnostics explicitly ignored, nine Python tests passed, and the release
workspace built successfully. The bounded profiling diagnostic was separately
run and passed; its overhead is documented in [exclusive profile](exclusive-profile.md).


Final binary `ac1b87c115e63cd0c0e0bd983761c0090cc788e6acc52c25740f6939704855c8`
(with experiments off) retained the approved primary RGB hash, reached reserve in
12.376 s and measured frame p95 9.919 ms. At 4 km it reported an explicit current
cache-capacity deficit after 95.565 s, with 15,982 owners and 24.929 px remaining
error; no settled frame samples were taken. This is a failed quality gate, not a
performance pass. Unlike the earlier timeout it no longer treats an oversized
replacement as a cache-resident transaction that can eventually complete.

The final 3 m/s stop/resume retained coverage for all 1,528 measured frames but
met requested detail for only 381; frame p95 was 11.277 ms. Thus #64 is still
unfinished. The measured remaining throughput work is in source/appearance
preparation and retaining useful shared summaries, not simply increasing the
prediction horizon. Inclusive 1 km appearance timings account for roughly 70%
of summed builder time; source work is nested there. This motivates further
profiling/reuse without claiming the experimental fine timers measure normal
production cost proportions.
