# Issue 74 — bounded source-cache experiments

2026-11-06. Eight usable runs under `output/issue74/cache-sweep/` compare three
controls, three native-summary runs, and one run each with 64/128 MiB source
reservations. **Neither experiment is promoted:** summary reuse remains off by
default, and the default source reservation remains unchanged. The separately
archived `output/issue74/invalid-old-binary-cache-sweep/` used an older launcher
that lacked these controls; all of its results are excluded here.

## Identity and matched workload

All eight `runs.json` files record binary SHA256
`b40bded52348fec67e75ab815c971eaf21d94679b7841e7f26c5618b32fe98cd`.
They observe dirty checkout `5a623f2a960e0b4cfad67454bee06c2f38831292`, explicitly
not a guaranteed binary build revision. Reports confirm the actual summary
flag and source reservation. Chronological order was control-a, source64,
summary, source128, control-b, summary-b, control-c, summary-c; this was not a
randomized campaign.

The workload is stationary Legacy seed 7, source version 1, revision 0,
canonical 0.0625 m cells, scale 1800 m, amplitude 1000 m, eight octaves,
roughness 0.5. It uses the fixed player-height camera [0,94.8875,0], FOV 60°,
near 0.025 m, far/coverage 300 m, 1920×1080, MSAA4, voxel filter on, render
scale 1, atmosphere off, exposure 1, sun 35°/135°, four workers and eight
refinements per transaction. Pixel-error request is 5; native bins are on.
The wait policy reaches refinement reserve before a four-second stationary
measurement with 60 warmup frames discarded, uncapped presentation and input
lock. Route speed 3 m/s is recorded but the route is static.

Hardware is the AMD Radeon 760M integrated GPU, Vulkan/RADV PHOENIX,
Mesa 25.2.8-0ubuntu0.24.04.2; CPU model is unrecorded. Each run begins in a
fresh process; disk-cache counters record zero hits/writes. These are cold
in-process preparation comparisons, not warm world-cache revisit measurements.

## Results

Quality and reserve are distinct milestones (`first_quality_ms` and
`first_refinement_ms`); first coverage is not substituted for either.
CPU cache residency includes source and mesh residency. MiB means 1,048,576
bytes. Columns and evictions are end-of-run cumulative counters.

| Run | Source reserve (MiB) | Quality (s) | Reserve (s) | Generated columns | Source page evictions | CPU cache resident (MiB) | Frame p95/p99 (ms) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| control-a | 32 | 10.647 | 12.125 | 33,832,713 | 550,136 | 419.632 | 9.285 / 9.686 |
| source64 | 64 | 10.418 | 11.889 | 30,979,766 | 394,164 | 451.646 | 9.427 / 9.782 |
| summary | 32 | 10.104 | 11.528 | 33,838,402 | 551,139 | 419.622 | 9.304 / 9.810 |
| source128 | 128 | 10.201 | 11.729 | 29,661,168 | 226,125 | 511.416 | 9.213 / 9.864 |
| control-b | 32 | 10.705 | 12.180 | 33,834,090 | 550,808 | 419.636 | 9.188 / 9.732 |
| summary-b | 32 | 10.912 | 12.384 | 33,835,779 | 550,123 | 419.624 | 9.881 / 10.630 |
| control-c | 32 | 10.682 | 12.211 | 33,843,022 | 551,665 | 419.607 | 10.163 / 10.787 |
| summary-c | 32 | 10.531 | 12.012 | 33,835,985 | 549,713 | 419.635 | 9.901 / 10.410 |

| Metric | Control median [min, max], n=3 | Summary median [min, max], n=3 |
| --- | --- | --- |
| Requested quality (s) | 10.682 [10.647, 10.705] | 10.531 [10.104, 10.912] |
| Refinement reserve (s) | 12.180 [12.125, 12.211] | 12.012 [11.528, 12.384] |
| Worker build CPU sum (s) | 30.866 [30.774, 30.954] | 30.458 [29.220, 31.451] |
| Generated columns | 33,834,090 [33,832,713, 33,843,022] | 33,835,985 [33,835,779, 33,838,402] |
| Source page evictions | 550,808 [550,136, 551,665] | 550,123 [549,713, 551,139] |
| Appearance-cache hits | 81,138 [81,122, 81,141] | 81,526 [81,518, 81,535] |
| CPU cache resident (MiB) | 419.632 [419.607, 419.636] | 419.624 [419.622, 419.635] |
| Frame p95 (ms) | 9.285 [9.188, 10.163] | 9.881 [9.304, 9.901] |
| Frame p99 (ms) | 9.732 [9.686, 10.787] | 10.410 [9.810, 10.630] |

Native-summary reuse shares only complete matching 16×16/stride-1 profiles;
it does not add child float sums or reuse clipped/sampled footprints. The
median quality reduction is **1.42%**, reserve reduction **1.38%**. The summary
range overlaps controls and includes a slower run. The initial favorable pair
does not establish a repeatable ≥5% preparation improvement. Generated source
work is effectively unchanged, and frame-tail medians do not establish a
benefit. Keep `FRONTIER_NATIVE_SUMMARY_REUSE=1` experimental and default off.

All runs have the same **512 MiB total CPU cache reservation**. The source/mesh
split is 32/480 MiB by default, 64/448 MiB or 128/384 MiB with the override.
Larger source reservations reduce available mesh capacity; they do not increase
the overall cache budget. Source peaks are near their reservations: controls
and summary runs 33,553,824–33,553,920 bytes, source64 67,108,096 bytes,
source128 134,217,008 bytes. The 128 MiB run ends at 536,258,480 bytes total CPU
cache residency, close to its 536,870,912-byte limit; live mesh payload is
402,099,248 bytes versus 406,299,632 in the other runs.

Against the three-control median, source64/source128 quality is 2.48%/4.51%
lower and reserve is 2.39%/3.70% lower. They generate fewer columns and evict
fewer pages, but each has only one sample and neither demonstrates the ≥5%
repeat gate. Retain the default split. A later repetition must account for
mesh pressure and revisit behavior as well as source work.

GPU pool allocation varies between 136 and 144 MiB across runs; the reported
build-memory peak reservation is 460,217,472 bytes in all eight. These are
separate accounting domains and must not be added to CPU cache residency as if
all represented distinct payloads. Process used/committed memory is null.
Worker stage sums include source work and overlap across four workers; they
are not wall-time fractions. Whole-frame GPU timing is unrecorded.

## Image equality and limits

SHA256 of decoded RGB bytes was computed independently for **all eight** PNGs
with Pillow `convert('RGB').tobytes()`. Every run has:

```text
158e5d03a5af1de7011e129a2bfe2452058437b03122868a92cb3a7543f61ce0
```

All reports reach quality/reserve without benchmark failure and preserve this
stationary capture. This is exact RGB equality between these runs, not a new
oracle approval or proof of temporal, topology, edit or long-range correctness.
Existing exact profile/metadata/mesh tests remain the semantic checks. The
small stationary result does not complete #74/M1 or justify kilometre sweeps.

## Reproduction from recorded commands

Each run's `runs.json` stores the full argv and experiment environment. For
example, controls leave both `FRONTIER_NATIVE_SUMMARY_REUSE` and
`FRONTIER_SOURCE_CACHE_MIB` unset; summary variants set only the former to `1`;
source64/source128 set only the latter to `64`/`128`. Other recorded experiment
variables are unset. The argv is identical except output paths:

```text
target/release/frontier --scene adaptive --pixel-error 5.0 --adaptive-cache-mib 512
  --terrain-recipe legacy --seed 7 --width 1920 --height 1080 --msaa 4
  --voxel-filter on --uncapped --warmup 60 --benchmark-input-lock --route static
  --route-clock realtime --route-speed 3.0 --seconds 4.0 --capture-far 300.0
  --adaptive-workers 4 --adaptive-refinements 8 --eye 0.0,94.8875,0.0
  --target -8.731370115810932e-08,94.83756238296628,-0.9987523555755615
  --fov-y 60 --capture-on-exit <run>/static.png --report <run>/static.json
  --adaptive-wait-quality --native-bins on
```

To replay exact recorded argv/environment in the same workspace, run from
`frontier/` (this writes the recorded output paths):

```python
import json, os, subprocess
from pathlib import Path
run_name = 'summary-b'  # choose one of the eight directories above
record = json.loads(Path(f'output/issue74/cache-sweep/{run_name}/runs.json').read_text())[0]
env = os.environ.copy()
for key, value in record['capture_provenance']['experiments'].items():
    if value is None:
        env.pop(key, None)
    else:
        env[key] = value
subprocess.run(record['command'], env=env, check=True)
```

The binary hash, actual report settings and reservation must match before
combining a replay with this group. Current checkout identity alone is not
sufficient binary provenance.
