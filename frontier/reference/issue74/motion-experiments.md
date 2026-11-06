# Issue #74 motion scheduling experiment

2026-11-06. The conservative obsolete-transaction experiment did not demonstrate
useful activation or a motion-quality gain. Do not enable it by default. All
four runs retained coarse coverage, but all failed the requested whole-route
quality gate. These results do not complete M3 or issue #74.

## Frozen workload and provenance

Raw reports, complete commands, process logs, captures and environment provenance
are in `output/issue74/motion-current/{control-turn,preempt-turn,preempt-stop,control-stop}/`.
Each directory contains `3mps.json` and `runs.json`. The sequential run order was
control turn, experiment turn, experiment stop/resume, control stop/resume.
There is one observation per condition, without randomized repeats.

All four runs recorded executable SHA256
`b40bded52348fec67e75ab815c971eaf21d94679b7841e7f26c5618b32fe98cd`.
The observed checkout was dirty at commit
`5a623f2a960e0b4cfad67454bee06c2f38831292`; that observation does not establish
the executable's build revision. Controls left
`FRONTIER_OBSOLETE_TRANSACTION_PREEMPT` unset; experiments set it to `1`.
Debt prediction remained disabled. Actual report settings retain native-profile
reuse enabled and native-summary reuse disabled.

The source is the unchanged Legacy seed-7 terrain, source version 1, canonical
6.25 cm voxels, horizontal scale 1,800 m, amplitude 1,000 m, eight octaves and
roughness 0.5. All runs use the primary camera at `[0,94.8875,0]`, 1920×1080,
60° vertical FOV, near 0.025 m, far 300 m, render scale 1, MSAA4, voxel filtering,
native bins and no atmosphere. Camera/input settings were locked. CPU cache,
build allowance and GPU pool allowance are each 512 MiB; uploads are limited to
8 MiB per frame with the existing single-cluster quantum. There are four active
preparation workers and eight selected refinements per transaction. The adapter
is Radeon 760M, Vulkan/RADV, Mesa 25.2.8. Terrain-pass timestamps remain distinct
from whole-frame timings.

Both routes run at 3 m/s for 12 measured seconds on the real-time clock, after
settling to the separate 4 px reserve for the requested 5 px quality contract.
Preparation uses the recorded `clear1Hz_poll20Hz_until_refinement_reserve` policy
and 60 warmup frames. `TurnReturn` travels out through t=2 s, turns 180° in place
over t=2–3 s, returns through t=5 s, then remains at the original position facing
the opposite heading. It is a one-second turn, not an instantaneous camera cut
or a bank. `StopResume` travels through t=2 s, waits until t=4 s, then resumes.
The viewer's reported FBM route-height policy clamps to canonical surface +10 m;
these are not ground-following walking or physics acceptance routes.

## Observations

| Case | Coverage-ready / measured frames | Requested-quality-ready / measured frames | Quality fraction | Unbounded-error frames | Whole-frame p95 ms | Recorded preemptions / abandoned pending jobs |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Control turn | 2,338 / 2,338 | 18 / 2,338 | 0.7699% | 203 | 8.627371 | 0 / 0 |
| Experiment turn | 2,311 / 2,311 | 26 / 2,311 | 1.1251% | 172 | 8.852056 | 0 / 0 |
| Experiment stop/resume | 1,337 / 1,337 | 371 / 1,337 | 27.7487% | 0 | 11.998350 | 0 / 0 |
| Control stop/resume | 1,481 / 1,481 | 327 / 1,481 | 22.0797% | 0 | 11.402428 | 0 / 0 |

| Case | First requested quality / reserve s | Final visible bound px | Peak unresolved owners | Maximum finite route bound px |
| --- | ---: | ---: | ---: | ---: |
| Control turn | 10.592227 / 12.080788 | 122.144589 | 107 | 2,816,001.986104 |
| Experiment turn | 10.289463 / 11.819418 | 122.144589 | 104 | 5,354,036.066779 |
| Experiment stop/resume | 12.613773 / 14.259883 | 9.116934 | 31 | 56.330475 |
| Control stop/resume | 11.704559 / 13.183825 | 9.116132 | 33 | 56.462642 |

Bounds are conservative projected certificate bounds, not measured reference
image displacements. Quality fractions are frame-weighted, not duration-weighted.
One offending owner fails a whole frame; the fractions do not imply a linear
throughput multiplier required for success. Small p95 frame times describe
rendering the available cover and do not establish that detail kept up.

Every process exited zero and reported no benchmark interruption. Process
completion and the report's `benchmark_failed=false` field do not establish
requested route quality. Neither route supplies an approved temporal reference
comparison, and these reports do not certify temporal appearance.

There were zero recorded transaction preemptions and zero recorded abandoned
pending jobs in every run. The enabled runs therefore supply no demonstrated
useful transaction abandonment. Cancellation callback attempts are not counted
separately: the callback can recheck a pose before the next plan independently
rechecks the latest pose. Zero plan-time preemptions must not be inflated into a
claim that every callback evaluation was false.

The differing frame counts, quality fractions, first-readiness times and tails
are observations from sequential asynchronous runs. They are not evidence of a
causal speedup or quality improvement, particularly without useful recorded
activation. Default promotion is rejected on this evidence.

## Why the predicate may not activate

The opt-in predicate requires every original selected refinement parent's
conservative surface bounds to be outside both current and forecast frusta,
while an unoccluded current requested-quality failure exists. It retains work
when provenance or projection is uncertain. Selected parents can span much
larger regions than their children; conservative vertical envelopes can intersect
a view even when much of the prepared detail has become irrelevant. A single
still-visible or forecast-needed parent retains the entire coherent replacement.
The forecast can also protect a footprint during a turn. These are plausible
reasons for zero recorded preemptions, not measured attribution: the reports
do not count which predicate clause retained each transaction.

The condition deliberately protects coherent coverage, source identity and
continuous-motion progress. Weakening bounds or canceling on every camera epoch
would lose those protections and resurrect previously demonstrated starvation.
Increasing prediction duration alone has already failed to establish motion
quality in the earlier debt-prediction checkpoint.

## Next bounded design work

Before another scheduler change, record per-transaction selected parents, actual
child/closed-edge dependencies, time-to-need, completed and pending work, worker
idle time and the exact reason a preemption check retained demand. Separate
generation, upload and publication deadlines, and distinguish valid reusable
products from work that never contributes to a presented cover.

Granular cancellation needs a dependency model for independently publishable
closed covers. It must identify a genuinely obsolete subset without dropping
neighbor edge requirements, source revisions, topology ownership or complete
coverage. Prepare a larger shared transaction only when measurements show that
its planning/source reuse exceeds its longer publication latency and retained
memory. Existing source shards already share exact data across batches; a larger
logical batch does not itself create new source reuse.

A persistent bounded worker queue is a separate throughput candidate. Current
scoped batches join up to four jobs before admitting another batch, so unequal
job durations can leave workers idle. Measure that idle time first. Any proposed
queue must retain at most the authorized active jobs and byte reservations,
provide cancellation checkpoints, admit completed canonical products safely,
and preserve exact source/material/certificate results. No benefit from this
unimplemented design is claimed here.

Actual bank remains unsupported: camera and route poses have yaw/pitch but no
roll, and the view matrix uses a fixed world-up vector. Adaptive destination
presentation gating also remains unsupported: `--teleport-test` accepts moving
hybrid scenes only, and `Viewer::begin_teleport` requires a hybrid stream.
Adaptive coverage/preemption tests are not a replacement for preparing a
destination while retaining the old view, validating destination ownership and
revision, then atomically publishing destination camera and owners. Cold/warm,
changed-destination, capacity-refusal and stale-revision cases remain necessary.

Adaptive-topology exact-owner edits are implemented and have an edit-test path;
combined motion/edit acceptance remains a missing gate. Arbitrary edits outside
that explicit volume owner are unsupported. These feature gaps and the failed
motion-quality measurements keep issue #74 open.
