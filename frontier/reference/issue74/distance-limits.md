# Issue #74 current distance limits

2026-11-06. The current fixed Legacy primary preset passes settled 1 km
requested quality but misses the 16.67 ms whole-frame p95 target. Its 4 km run
refuses preparation with unresolved quality at 102.606 s. This is a measured
wall for this implementation, preset, budgets and laptop; it is not an absolute
engine or representation ceiling. Issue #74 remains open.

Raw commands, provenance, reports, logs, timing files and diagnostic captures
are under `output/issue74/distance-current/{1000,4000}/`. Both cases use executable
SHA256 `b40bded52348fec67e75ab815c971eaf21d94679b7841e7f26c5618b32fe98cd`.
The recorded checkout was dirty at
`5a623f2a960e0b4cfad67454bee06c2f38831292`; this is not a guaranteed executable
build revision. These are single sequential observations, not randomized
performance comparisons.

The workload preserves the primary seed-7 Legacy source, source version 1,
6.25 cm canonical cells, primary player-height start camera, 60° vertical FOV,
0.025 m near clip, 1920×1080, render scale 1, MSAA4, filtering and native bins.
Only the far clip is scaled to 1,000 or 4,000 m. Atmosphere is disabled; there is
no fog-based coverage concealment. Preparation uses four workers, eight selected
refinements per transaction, 512 MiB CPU cache, 512 MiB build allowance, 512 MiB
GPU pool and 8 MiB/frame upload allowance. The source reservation is 32 MiB
within the CPU cache budget. Native-profile reuse is enabled; native-summary
reuse, debt prediction and obsolete-transaction preemption are disabled. No disk
cache is enabled. The adapter is Radeon 760M, Vulkan/RADV, Mesa 25.2.8.

Each stationary case requests four measured seconds after initial 5 px requested
quality and separate 4 px reserve, with 60 warmup frames. Preparation polls at
20 Hz and clears at 1 Hz. The 4 km case never reaches the measurement interval.

## 1 km settled observation

First coverage was 0.216237 s, first requested quality 54.338787 s and first
reserve 72.336983 s. The 217 measured frames all retained coverage and requested
quality, with zero unbounded-error frames and a maximum certified visible error
of 3.999775 px. Whole-frame p50/p95/p99 were
**13.951842 / 17.115961 / 18.908764 ms**, with 12 frames over 16.67 ms.
This fails the whole-frame p95 gate despite successful stationary readiness.

The selected display contains 11,989 resident owners, 10,721 visible owners,
5,257,933 reported triangles and 10,826 draw calls. GPU resident payload is
352,247,856 bytes and allocated pool size is 369,098,752 bytes. CPU cache reports
536,593,976 bytes; selected CPU payload is 420,975,536 bytes. Source/cache/selection
figures overlap and must not be added as independent allocations. Preparation
performed 24,761 builds and 214,567,166 generated columns. Its 167.932 summed
worker-seconds of build accounting overlap in wall time and are not an Amdahl
fraction. Planning totals 17.096 s over the preparation/run accounting window.

The same report includes 217 measured draw-encoding samples paired with those
217 frame intervals. Encoding p50/p95/p99 are
**4.771718 / 7.639020 / 8.590037 ms**. This measures `Gpu::draw` wall time:
queue writes, optional summary dispatch encoding, render-pass begin/end, and
bundle replay or direct draw encoding. It excludes `adaptive.prepare`, command
finish/submit, presentation, atmosphere and UI. It is broader than pure draw API
overhead and does not identify which encoding operation dominates. Quantile
ratios do not establish per-frame exclusive fractions or a guaranteed speedup.
This result warrants isolating large-view encoding costs rather than dismissing
them from the smaller 300 m workload.

Terrain-pass GPU p50/p95/p99 are 12.979621 / 13.797896 / 14.442842 ms over 161
asynchronous timestamp samples. They are not paired with every encoding/frame
sample and are not whole-frame GPU times. Do not add CPU and GPU percentiles or
subtract one from another to manufacture exclusive stage costs. Render bundles
are enabled and active at the final snapshot. Still reference/crop acceptance,
motion quality and bank/teleport behavior are separate gates.

## 4 km preparation refusal

The terminal `Error:` line in `4000/static.log` contains an outer JSON-quoted
error string and an escaped inner adaptive JSON object. The compact tracked
[failure evidence](distance-4000-failure.json) was extracted with `json.loads`
for the outer string, a checked literal failure prefix, then `json.loads` for
the inner object. No evaluation of log text was used. The artifact records the
source-log SHA256, executable hash and selected raw fields, plus explicitly
labeled derived capacity arithmetic.

The failure snapshot reports **102.606186 s** since stream start, complete
coverage, requested quality false, reserve false and unresolved true. Neither
first requested quality nor first reserve was reached. There are **976 current
unresolved visible owners**, with maximum current CPU/GPU projected error
**24.929336 px** against the 5 px request. CPU pending jobs and GPU pending
uploads are both zero; there is no hidden successful convergence inferred from
an empty queue.

| Refusal snapshot | Value |
| --- | ---: |
| Resident / visible owners | 15,982 / 14,775 |
| Native owners | 43 |
| Reported triangles / draw calls | 5,597,446 / 14,848 |
| CPU cache bytes / total budget | 531,331,136 / 536,870,912 |
| Actual source-cache bytes / reserved budget | 28,034,304 / 33,554,432 |
| Derived resident mesh plus LRU / mesh allowance | 503,296,832 / 503,316,480 |
| Derived remaining mesh allowance | 19,648 bytes |
| Selected CPU payload | 501,061,648 bytes |
| GPU resident payload / allocated pool | 414,162,688 / 427,819,008 bytes |
| GPU pool budget | 536,870,912 bytes |
| GPU cluster slots used / capacity | 15,982 / 16,384 |
| GPU available cluster slots | 402 |
| Builds / generated columns | 32,041 / 212,677,302 |
| Peak live tile payload | 502,695,888 bytes |
| Peak builder reservation / build budget | 399,648,512 / 536,870,912 bytes |

The mesh allowance subtracts the reserved 32 MiB source budget from the CPU cache
budget. Resident mesh plus LRU subtracts actual source-cache bytes from reported
CPU cache bytes. This distinguishes nearly exhausted mesh capacity from unused
reserved source capacity. The snapshot shows strong CPU mesh admission pressure;
it does not record the exact rejected replacement's required bytes. GPU resource
limitation is false, 402 GPU cluster slots remain, and GPU allocation is below
its pool budget. Do not report this as a demonstrated GPU allocation failure or
claim that raising one capacity alone would achieve quality and frame-time gates.

The largest reported offender is a source-revision-0 owner at native-cell minimum
`[8192,-65536]`, width 4,096 cells and step 512 cells. Its cached vertical residual
is 100.356779 m and its projected certificate bound is 24.929336 px. Its current
bounds are visible and current occlusion does not reject it. These are conservative
bounds rather than measured image displacements; no certificate was weakened.

A normal `4000/static.json` failure report was also written. Its final diagnostic
snapshot is later, at 102.747573 s; the process wall time is 102.892965 s and exit
code is 1. These timestamps describe different events. The report has zero
accepted measured frames and null frame/GPU percentiles, so no 4 km settled FPS
result exists. Its diagnostic capture shows unresolved quality and is not a
passing reference image.

## Stop rule

No 20 km run follows this unresolved 4 km preparation refusal. Under M4, stop
at capacity refusal or unresolved reference quality, preserve time-to-failure,
memory and remaining error, then investigate bounded source/summary/residency
costs before expanding distance. A larger budget, different preset, alternative
representation or new scheduling design would require a separate measured
experiment with the same source/quality protections. The present evidence
establishes the current fixed-preset wall, not a universal distance limit.
