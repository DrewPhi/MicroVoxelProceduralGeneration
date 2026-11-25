# Phase A iteration 2: indexed adaptive proxy records

The opt-in indexed-proxy mode preserves the captured primary image and reduces
adaptive GPU payload in this bounded experiment. Its measured static timing
change is small. It does not reduce canonical source generation or resolve the
existing motion quality deficit. No renderer default is promoted.

## Hypothesis and scope

The experiment reuses position records within the existing selected adaptive
proxy geometry, replacing repeated triangle positions with indexed position
storage. It tests whether preserving the adaptive representation while reducing
its GPU data is useful. It does not impose a uniform displaced grid or a fixed
native square. Existing error certification, feature-aware selection, native
geometry and volume owners remain responsible for geometry and ownership.
Source generation and the full CPU tile payload remain unchanged.

Baseline is `FRONTIER_INDEXED_PROXY=0`; candidate is mode `1`. Both arms use the
same frozen binary, SHA256
`eb4810e374ccbe88539cacb8bebff237a0f77a63ccfcac73ebeec40cf7627d49`.
The [iteration runner](../../tools/phasea_iteration_check.py) copies both binaries
before either arm starts, records explicit experimental environment settings,
and preserves original reports, commands and camera settings in each manifest.

The frozen primary scene is Legacy seed 7, 6.25 cm canonical voxels, source scale
1800 m, amplitude 1000 m, eight octaves and roughness 0.5. The primary camera,
60-degree FOV, 300 m far clip, 1920×1080, MSAA1, voxel filter on and atmosphere
off are retained. Adaptive settings are 5 px, four workers and eight refinements.
Hardware is Ryzen 5 7640U/Radeon 760M, RADV/Vulkan, Mesa 25.2.8.

## Primary and static evidence

The separate [headless primary comparison](../../output/phase-a-iterations/indexed-primary/comparison.json)
has zero differing PNG pixels, zero linear RGB error, zero coverage disagreement
and zero normalized depth error across 966,894 common-hit pixels. All three
static-window endpoint pairs also have exact RGB/linear RGB/depth agreement.
This proves parity with the existing adaptive captures at these poses; it does
not newly approve their agreement with the independent native reference.

The original headless report predates the runner's explicit camera-roundtrip
tolerance and flags a roughly 6.35e-8 m target component difference from the
frozen camera. Both actual capture cameras are identical. The runner now records
that difference against an explicit 1e-6 m eye/target tolerance while retaining
exact arm-to-arm camera equality. The original artifact is preserved. Missing
headless exposure/material provenance remains unavailable; pixel equality does
not manufacture those fields.

Each static window runs six seconds after initial readiness and 60 warmup frames.
Frame intervals include presentation; `isolated_gpu_ms` has its original report
scope. These short, fixed-order baseline/candidate trials do not establish a
statistically reliable speedup.

| Trial | Frame p50, baseline / mode 1 (ms) | Frame p95 (ms) | Frame p99 (ms) | Isolated GPU (ms) |
| --- | ---: | ---: | ---: | ---: |
| [1](../../output/phase-a-iterations/indexed-static-1/manifest.json) | 4.873 / 4.645 | 7.094 / 6.881 | 7.354 / 7.088 | 4.108 / 3.880 |
| [2](../../output/phase-a-iterations/indexed-static-2/manifest.json) | 4.895 / 4.790 | 7.142 / 7.112 | 7.493 / 7.506 | 4.118 / 3.972 |
| [3](../../output/phase-a-iterations/indexed-static-3/manifest.json) | 4.923 / 4.866 | 7.181 / 7.035 | 7.478 / 7.791 | 4.133 / 3.998 |

The median trial p95 changes from 7.142 to 7.035 ms (about 1.5%); median isolated
GPU changes from 4.118 to 3.972 ms (about 3.5%). Tail behavior is mixed. Initial
preparation ranges from 17.75 to 18.86 seconds across arms; these measurements
provide no clear startup improvement. Mode 1 adds 269.6–281.6 ms of cumulative
CPU packing work during the static preparations.

At the identical settled selection, adaptive resident GPU payload decreases from
128,994,992 to 114,179,920 bytes: 14,815,072 bytes, about 14.8 MB. Allocated pool
storage decreases from 142,606,336 to 125,829,120 bytes: 16 MiB. These are scoped
adaptive counters, not total application GPU residency. The candidate includes
about 5.63 MB of resident position records; record alignment/reservation is
reported separately. Native quad/profile ownership remains unchanged.

The live CPU tile payload remains 406,299,632 bytes in both arms. No canonical
source-generation saving is claimed. Existing upload counters retain their
conservative original accounting; their small numerical difference must not be
presented as a measured packed-transfer reduction. Total memory, driver
allocation overhead and full transient residency remain unavailable.

## Motion diagnostics

The paired six-second real-time turn/return routes retain the same starting
scene and readiness policy. Endpoints follow elapsed wall time and are not
matched trajectory frames, so endpoint RGB/depth comparisons are explicitly
unavailable. Raw reports preserve trajectory readiness and error diagnostics.

| Route | Frame p95, baseline / mode 1 (ms) | Frame p99 (ms) | Coverage-ready frames | Quality-ready frames, baseline / mode 1 |
| --- | ---: | ---: | ---: | ---: |
| [3 m/s](../../output/phase-a-iterations/indexed-motion-3/manifest.json) | 7.425 / 6.828 | 9.902 / 12.988 | 100% / 100% | 95/1698 (5.6%) / 97/1683 (5.8%) |
| [80 m/s](../../output/phase-a-iterations/indexed-motion-80/manifest.json) | 5.364 / 5.683 | 7.287 / 11.371 | 100% / 100% | 0/1858 / 0/1736 |

Coverage readiness is not quality readiness. Both routes still record unbounded
error frames and unresolved clusters. The experiment does not fix motion quality,
prove invisible transitions or establish a faster renderer under equivalent
moving quality. The lower walking p95 is accompanied by worse p99; the fast
candidate has worse p95 and p99 in this single pair.

## Mode 2: derived face records

Mode 2 (`FRONTIER_INDEXED_PROXY=2`) additionally derives face information from
indexed positions. Its reports explicitly flag possible normal roundoff; exact
agreement at these tested poses does not prove identical shading for every
triangle/source fixture. The binary SHA256 is
`b09b5c039ffe579173cce027902affa6ec2824ebf53c009f3d12c23580f2b1ad`, shared by
its baseline and candidate arms. Mode 2 retains the existing selection,
certification and owners, and still makes no source-generation saving claim.

The [mode-2 primary](../../output/phase-a-iterations/derived-primary/comparison.json)
and all three static-window pairs have zero differing PNG pixels, zero linear
RGB error, zero coverage disagreement and zero normalized depth error. The
primary has 966,894 common-hit pixels. Headless metadata limitations remain as
above. Mode-2 motion has **not** been tested.

| Trial | Frame p50, baseline / mode 2 (ms) | Frame p95 (ms) | Frame p99 (ms) | Isolated GPU (ms) |
| --- | ---: | ---: | ---: | ---: |
| [1](../../output/phase-a-iterations/derived-static-1/manifest.json) | 5.724 / 5.200 | 8.805 / 8.417 | 9.197 / 9.088 | 4.683 / 4.207 |
| [2](../../output/phase-a-iterations/derived-static-2/manifest.json) | 5.871 / 5.241 | 8.900 / 8.309 | 9.383 / 8.610 | 4.764 / 4.239 |
| [3](../../output/phase-a-iterations/derived-static-3/manifest.json) | 5.764 / 5.196 | 8.786 / 8.426 | 9.187 / 8.917 | 4.704 / 4.217 |
| Median of trials | 5.764 / 5.200 | 8.805 / 8.417 | 9.197 / 8.917 | 4.704 / 4.217 |

Median trial p95 decreases about 4.4%; median isolated GPU decreases about 10.3%.
These are short sequential baseline/candidate trials. The later baseline itself
is slower than the earlier mode-1 baseline, so absolute timings across those
campaigns must not be used to rank mode 1 against mode 2. Fixed arm ordering and
uncontrolled clock/thermal effects remain limitations.

Settled adaptive GPU payload decreases from 128,994,992 to 97,142,160 bytes:
31,852,832 bytes (about 31.9 MB). Allocated pool storage decreases from
142,606,336 to 109,051,904 bytes: 32 MiB. These counters exclude application-wide
residency and other scopes described above. Live CPU tile payload remains
406,299,632 bytes. Mode 2 adds 409.9–434.3 ms cumulative CPU packing work; startup
preparation spans 20.39–21.34 seconds across both arms and provides no clear
startup benefit. Upload accounting remains conservative. No default changes.

The separate [Legacy cave secondary pair](iteration2-cave.json), using
`adaptive-topology --camera cave` with MSAA1, 1920×1080 and the same filter,
atmosphere, 300 m clip and adaptive settings, is also byte-identical in PNG,
linear RGB, depth and coverage between modes 0 and 2. Mode 2 reports 887 derived
clusters among 888 indexed clusters, exercising the fallback path. Its artifact
records metadata and hashes. This is bounded cave endpoint evidence, not proof
of motion, edits or topology preservation at every distance.

## Preserved evidence and validation

[iteration2-results.json](iteration2-results.json) preserves the compact JSON
manifests, comparisons, capture sidecars and original benchmark reports for the
mode-1 primary/static/motion and mode-2 primary/static campaigns. Large images,
depth buffers and binary copies remain in their original output directories.
Commands, source/camera settings, environment modes, hashes and scoped memory
counters are retained. Performance claims above concern these measured captures
and trials; neither artifact validity nor exact endpoint parity approves the
visual target or moving quality.

The independent [GPU profiling probes](GPU_PROFILES.md) document their own
methods, hashes and limitations. They must not be substituted for these complete
primary and window runs. Validation reported by the parent run: 319 ordinary
Rust tests passed, 10 ignored; the standalone ignored GPU probe passed separately;
all 30 Python tests passed. The final Rust suite includes mode 2 and the parallel
profile implementation.

The evidence supports continued bounded investigation of reusable source
positions while preserving certified selection and owners. It does not complete
#75 or justify the deferred 1 km/Regional campaign. Existing adaptive defaults
remain in place.
