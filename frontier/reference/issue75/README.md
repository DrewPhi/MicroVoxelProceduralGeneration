# Issue 75: bounded Phase A checkpoint

Follow-up: [iteration 2](ITERATION_2.md) measures two smaller adaptive GPU
representations; [GPU appearance probes](GPU_PROFILES.md) compare serial and
parallel preparation. These are further Phase A evidence, with defaults unchanged.
The next [integrated experiment](INTEGRATION.md) connects parallel profiles to
actual draws, tests lossless source packing, and records loading/motion costs.

The uniform page experiment fails the small Legacy visual gate. Directional
brick skipping preserves the tested ray result but is slower than the existing
Chebyshev skip and mesh on this fixture. This is bounded negative evidence,
not a completed architecture decision or acceptance of an alternative renderer.
The 1 km and Regional campaigns remain deferred after the failed small gate.

Measurements below use the Ryzen 5 7640U/Radeon 760M laptop, RADV/Vulkan,
1920×1080 and MSAA1. Exact commands, binary hashes, source/camera settings and
measurement scopes remain in the linked artifacts. Trials are short and have
not established repeatability, frame stability or production readiness.

## Primary visual gate

The seed-7 Legacy source, 6.25 cm canonical voxels, fixed player-height camera,
300 m clip, filter on and atmosphere off are compared with the frozen 256-spp
native-column reference. Preserved source settings and diagnostics are in
[primary-results.json](primary-results.json); the complete image comparisons are
[uniform pages](pages-reference.png) and [their raw comparison](pages-reference.json),
plus the [adaptive image](adaptive-reference.png).
The final page capture is RGB-identical to the first capture after fixing cache
identity; commands and hashes distinguish the two builds.

| Candidate | Linear RGB RMSE | Missing-coverage pixels | View levels |
| --- | ---: | ---: | ---: |
| Adaptive | 0.02143 | 253 | 7 |
| Existing clipmap | 0.04931 | 11,674 | 9 |
| Uniform pages, 1 m display spacing | 0.03606 | 4,070 | 11 |

Uniform pages improve these diagnostics over the captured clipmap but remain
worse than adaptive and do not meet the visual target. These are candidate
pixel-center versus reference subpixel-mean diagnostics, not certified geometry
bounds. Finite coverage sampling cannot prove thin-feature preservation.
The captures retain their different representation settings and tiny camera
round-trip differences; the numbers alone do not establish matched-quality
performance or approve adaptive.

The final matched-pose static trial records page frame-interval p95 **8.58 ms**,
clipmap **7.57 ms**, and adaptive **6.77 ms**, including presentation. These are
MSAA1 results, not the earlier MSAA4 baseline. This single trial uses different
visual approximations and establishes no optimization win. Page memory counters
cover additional page buffers, not total residency; the experiment retains the
hybrid sampler and native/collar machinery. See [the experiment contract](../../PHASE_A.md).
The final static and motion reports, commands and hashes are preserved in
[page-runs.json](page-runs.json). Page initial preparation was about **1.14 s**
including the retained hybrid work; adaptive first requested quality was about
**10.24 s**, with its refinement reserve at **11.72 s**. These prepare visibly
different approximations, so the difference is not an equivalent-quality speedup.
Earlier `pages-static` launch failures and `pages-static-v2` trials are superseded.

## Bounded volume and ownership checks

The [volume-v2 raw summary](../../output/phase-a/volume-v2/summary.json) preserves
commands, timings, edit records and comparisons; [volume-results.json](volume-results.json)
is the checked-in result snapshot. This is a bounded 16×14×16 m native fixture,
not a several-mile volume renderer benchmark.

| Backend | Static GPU terrain-pass p50, ms |
| --- | ---: |
| Mesh | 0.787 |
| Brick, no skip | 3.740 |
| Chebyshev brick skip | 2.293 |
| Directional brick skip | 2.491 |

Both skip paths match the unskipped ray capture exactly in RGB and normalized
depth for the static and restored-edit endpoints. Against mesh, all ray paths
retain **83 coverage disagreements** and nonzero color/depth residuals; exact
ray parity does not mean exact mesh parity. GPU timestamps cover the terrain
pass only, and dropped timing samples are reported. All four edit trials restore
the original materials; last input-to-publication latency is **1.56–1.61 s**.
Restored endpoints do not certify the intervening edit transition.

The [topology comparisons](../../output/phase-a/topology/comparisons.json), also
preserved in [topology-comparisons.json](topology-comparisons.json), compare pages
with the existing hybrid owner at cave, bridge and combined poses. Coverage
mismatches are respectively **0, 4,203 and 7,408 pixels**. The exact volume owner
remains enabled, but composition parity is unresolved. A zero cave coverage
mismatch does not certify topology at all distances or quality during motion.
The final stationary page/cave edit run also completed add/restore transactions:
world and page-source revisions both reached 8192 without camera movement.
See [page-edit.json](page-edit.json). Whole-page regeneration on edits remains
conservative; localized invalidation is not implemented.

## Pending evidence and next gate

The final six-second real-time turn/return page routes completed at 3/15/80 m/s,
with no reported benchmark failure or finite-cover overrun. They are bounded
reuse/readiness diagnostics, not offline-reference temporal quality approval.

| Speed, m/s | Frame p95 / p99, ms | Pages generated / reused | Pages uploaded |
| ---: | ---: | ---: | ---: |
| 3 | 7.98 / 9.16 | 600 / 1,128 | 624 |
| 15 | 8.33 / 9.26 | 600 / 4,584 | 624 |
| 80 | 7.80 / 10.37 | 720 / 11,376 | 864 |

Counts include the initial 576 pages. Reuse counts are lookups, not unique pages.
The walking rerun generated 600 pages versus 1,728 before correcting a prototype
bug: the existing native stream calls its camera transaction counter a revision.
It must not enter source-content cache keys. World/source revision and native
publication epoch are now separate, with regression tests for CPU and GPU reuse.
Return travel can reuse CPU pages while reuploading evicted GPU slots.

The page grid plus retained clipmap buffers use **61,085,328 bytes** at the tested
11 levels, before native meshes, targets and staging. The 3 m/s report additionally
records about **316 MB** of native GPU payload. This prototype has no demonstrated
total-memory advantage. Full-cover uploads are bounded but not frame-budgeted.

The existing selected quadtree probe also remains unmatched in appearance and
uses frozen source pages. A follow-up needs shared appearance and adaptive
geometric-error selection before a fair architecture comparison. Complete this
bounded checkpoint before expanding the costly reference campaign.

**Validation:** the full workspace suite passed (306 tests, 9 ignored) after the
cache-identity fix. The final publication change passed all seven targeted page
tests, including its new regression; 26 Python tests passed. WGSL validation is
included. No default renderer was replaced. #74/#75 remain open.
The final adaptive and clipmap captures are byte-identical to their frozen
pre-change PNG, linear RGB, depth and coverage files at the primary pose:
[default-parity.json](default-parity.json).

## Algorithm references and next experiment

This is greenfield code; no external implementation was copied. Reusable topology
and moving source storage are motivated by [GPU geometry clipmaps](https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry).
[CDLOD](https://github.com/fstrugar/CDLOD) motivates selected reusable grids; the
uniform prototype does not implement CDLOD. Directional boxes are inspired by
[NAADF](https://github.com/cg-tuwien/NAADF), but use eight octant, axis-greedy boxes
per brick rather than its complete nested representation. These results do not
measure the full published algorithms.

Keep the current adaptive renderer. The next useful experiment must preserve
resolvable voxel faces and silhouettes with projected-error-driven ownership,
then reuse source pages underneath that selection. Remove the duplicate full
clipmap dependency, add budgeted uploads and compare the exact same visual target
before scaling outward. Further uniform-grid or backend timing alone will not
resolve the observed quality loss.
