# Issue 75 B1–B3 decision checkpoint

2026-11-07. User preferred adaptive in Step 0.5 and authorized this bounded
experiment pass. Reuse tier G; no old-engine code ported. Three inexpensive
subagents handled source, cache, and profiling tasks; root reviewed, integrated,
and ran renderer comparisons. **Adaptive remains the default.** This is not
closure of #74/#75 or a claim that production streaming goals have passed.

## Decisions

| Experiment | Implemented / measured | Decision |
| --- | --- | --- |
| B1 source-level octave sieve | Revision-checked, world-aligned CPU source API, ownership rejection, exact fallback, bounded sample-error experiment and tests. | Reject this truncation candidate for strict rendering. 2.2× fewer noise calls gave 144 m maximum sampled error; seven retained octaves still gave 2.5 m error with only 1.1× fewer calls. No renderer integration or readiness benefit claimed. |
| B2 compressed cold source | Opt-in bounded LZ4 exact source-page cache integrated into adaptive; Zstd/raw/LZ4 corpus probe; same-budget history and correctness tests; full renderer A/B. | Keep as disabled experiment. More retained history, identical static output, but slower first quality and no demonstrated moving-quality win. Not a rejection of every compressed-cache architecture. |
| B3 GPU meshing | Existing stage accounting audited; source-exclusive scoped hardware profiling and edited-volume CPU extraction/parity probes. | Defer GPU implementation under the conditional gate. Evidence does not establish a significant terrain-loading critical-path opportunity. Revisit for local edit workloads if measured publication latency warrants it. |

Details: [B1 math and measurements](../world/experiments/B1_SOURCE.md),
[B2 implementation](B2_COLD_SOURCE_CACHE.md), [B3 eligibility](B3_GATE.md).

## Renderer evidence

[Raw reports, commands, binary hashes, and comparisons](../reference/issue75/b123-cache-results.json).
Ryzen 5 7640U / Radeon 760M, RADV Vulkan, 1920×1080, MSAA1, canonical 6.25 cm,
Legacy seed 7, scale 1800 m, amplitude 1000 m, eight octaves, roughness .5.
Frozen player camera, 60° vertical FOV, 300 m clip, voxel filter on, atmosphere
off, adaptive pixel error 5, four workers, eight refinements. Both arms use the
same release binary and existing 512 MiB geometry/source cache budget; only
`FRONTIER_SOURCE_COLD_CODEC=none|lz4` changes. No disk-cache option supplied.
`FRONTIER_PROFILE_SOURCE` is unset: per-column clock instrumentation would distort
these performance comparisons. Source identity and mesh appearance are unchanged.

| Run | Codec | First coverage / requested quality / reserve, seconds | Settled frame p50 / p95 / p99, ms |
| --- | --- | --- | --- |
| Static 1 | none | 0.443 / 10.725 / 12.208 | 5.005 / 7.625 / 7.956 |
| Static 1 | LZ4 | 0.199 / 13.973 / 15.619 | 5.029 / 7.621 / 8.111 |
| Static 2, reversed order | LZ4 | 0.219 / 13.797 / 15.385 | 4.938 / 7.434 / 7.728 |
| Static 2, reversed order | none | 0.216 / 10.241 / 11.731 | 5.054 / 7.541 / 7.954 |

The static pairs have byte-identical PNG, float32 RGB, depth and coverage between
arms. They demonstrate endpoint parity, not motion parity or a newly passed exact
oracle threshold. Readiness regressed roughly 30–35% in both run orders. Frame
intervals include presentation; GPU terrain-pass timings are separate in raw
reports. These are two short pairs, not a confidence interval or broad hardware
survey. A third promising-candidate promotion run was unnecessary after rejection.

In the first static pair, hot resident columns drop from 1,475,713 to 732,584.
LZ4 also retains 30,436,536 serialized raw bytes (13 bytes per valid column),
approximately 2.08× the combined resident column history of the baseline. This
is a scoped source-history observation, not a 2× reduction in whole-engine RAM.
Cold scratch/index/payload reservation is 12,586,112 bytes inside the existing
source budget. Cumulative encoding/decoding costs were 1,555/176 worker-ms, and
newly generated columns increased from 33.84 million to 36.03 million. Cold page
hits include payload hits with a missing sparse column; they are not all recovered
column hits. Cumulative compressed volume is not resident memory.

A six-second real-time 15 m/s turn-return pair retained coverage on every measured
frame in both arms, but **zero frames passed strict requested quality** in either.
Frame p95/p99 was 7.048/9.208 ms baseline versus 6.495/8.471 ms LZ4. This does not
establish a rendering speedup at equivalent resolved detail: both paths were
refining, and real-time endpoints need not match. LZ4 had more unbounded-error
frames (355 versus 109). No >20% revisit-readiness or quality-retention gain is
demonstrated. Thus no promotion and no costly 1/4/20 km, all-speed, teleport or
edit-storm campaign for this losing candidate. Those acceptance items remain open.

## Why stop this optimization pass here?

This tested a specific lossy source reduction and a specific synchronous cold
cache; neither won the required combined gates. It does not establish an optimal
renderer. Future lossless source reuse, bounded background compression, indexed
cold lookup and whole-page restoration could change the result. None is required
to begin evaluating a small dressed scene, and none should be claimed solved.

The next useful product checkpoint is **one bounded Regional valley**, initially
static: coherent material coverage, basic light/shadows, atmosphere with a no-haze
toggle, and static water before water simulation. Produce a high-quality reference
first and have the user approve it. Keep the bare Legacy fixture and its exact
oracle unchanged. Track movement quality explicitly; high settled FPS does not
mean detail keeps up. The current result supports starting that visual work,
not declaring unrestricted travel or production LOD transitions complete.

## Reproduction

From `frontier/`, after building release:

```sh
python3 tools/phasea_iteration_check.py --output output/my-b2-static --seconds 4 --baseline-env FRONTIER_SOURCE_COLD_CODEC=none --candidate-env FRONTIER_SOURCE_COLD_CODEC=lz4
python3 tools/phasea_iteration_check.py --output output/my-b2-return --seconds 6 --route turn-return --speed 15 --baseline-env FRONTIER_SOURCE_COLD_CODEC=none --candidate-env FRONTIER_SOURCE_COLD_CODEC=lz4
```

Use hardware device access and verify the adapter; software llvmpipe runs are not
laptop GPU measurements. The existing source-profile clock mode is diagnostic
only and must not be mixed into these arms. B3 records its stopped/incomplete
profiling attempts separately rather than claiming them as completed trials.

## Validation and bounded history test

The full workspace suite passed 330 tests (11 ignored) before the final capacity
fixture; that additional focused test passed separately. All 33 Python harness
tests passed. The ignored release codec probe also passed all per-iteration byte
round trips. Six cached/direct edited-volume mesh comparisons retained exact
vertices and indices. `git diff --check` passed. Existing dead-code warnings
remain; no claim of a warning-free build.

At a fixed 256 KiB budget with 24 identical-demand pages, the capacity fixture
recovered 15,360 baseline columns versus 24,576 cold-tier columns for flat data
(1.60×), and 15,360 versus 16,384 for varying data (1.067×). Values are checked
exactly at each recovered address. This deliberately limited history does not
measure saturation capacity on all terrains. It reinforces that the 2.08× live
history observation in one renderer endpoint is not a universal 2× guarantee.
Reproduce with `cargo test -p frontier-renderer same_budget_unique_source_history_capacity -- --nocapture`.

The [codec corpus](../reference/issue75/b2-codecs.csv) includes raw, LZ4 and Zstd1
for Legacy/Regional pages and empty/dense/cave/edited bricks. Repeated recipe names
follow the four coordinate pairs listed in `source_cache_probe.rs`. This data is
codec-only; the runtime experiment caches columns, not those 3D bricks.
