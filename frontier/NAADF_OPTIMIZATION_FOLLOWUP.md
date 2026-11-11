# NAADF optimization follow-up

This extends the seven experiments at `f390bd0`; it does not overwrite their
negative results. All work is greenfield (reuse tier G), preserving canonical
6.25 cm voxels, source, vegetation, lighting, and hybrid ownership.

The acceptance question is why reduced operation counts fail to reduce total
cost. Instrumented captures answer operation-count questions; uninstrumented,
matched 1080p runs answer performance questions. Diagnostic atomics, readbacks,
shader disassembly and CPU stage timers must not be mistaken for ordinary FPS.

## Work at this checkpoint

- Count actual hierarchy loads, useful and failed height queries, distance gained
  beyond an existing proof, ambiguous-boundary replays, and useful beam coverage.
- Test direct publication of GPU-improved proofs into the root words already read
  by traversal. Preserve occupied roots and invalidate/rebuild safely on changes.
- Separate region preparation wall time from summed worker stage times. Measure
  sampling, plant assembly, hierarchy/deduplication and proof construction.
- Compare exact stack-based block proof construction against existing heap work.
- Exercise a repeating grounded square circuit: four ten-second legs, continuous
  travel, heading changes at corners, repeat every forty seconds. Existing fixed
  turn/return route remains unchanged for historical comparison.
- Use `tools/naadf_workloads.py` to plan/run matched 128/256 m forward, reverse,
  sky-heading and 45-second circuit views. Names describe the camera, not proof of
  a dense forest or mountain-interior workload; inspect captures before assigning
  scene labels. Plans are the default; explicitly add `--run` for measurements.

## Sources

- [NAADF paper and author project](https://www.cg.tuwien.ac.at/research/publications/2026/ulschmid-2026-naadf/): conservative cached empty cuboids and asynchronous improvement.
- [AMD occupancy explanation](https://gpuopen.com/learn/occupancy-explained/): register use and concurrent waves; fewer loads alone do not guarantee faster shaders.
- [Vulkan profiling guide](https://docs.vulkan.org/guide/latest/profiling.html): measure stalls, occupancy and workload costs rather than infer them from source.

Hardware remains Ryzen 5 7640U / Radeon 760M / RADV Mesa 25.2.8. No RGP/RGA/UMR
executable is initially installed here. Claims about register pressure must remain
hypotheses unless compiler statistics or hardware profiling substantiate them.

## Compiler evidence for per-ray caching

Cold RADV shader compilation of the frozen previous executable gives the matched
primary pixel shader 40 VGPRs / estimated 24 subgroups per SIMD in control versus
48 VGPRs / estimated20 in both cache modes. Instruction counts are 2783 control,
3026 coordinate-cache and 2932 compact-cache. There are no register spills.
The compact mode reduces prescheduling register demand but not the final allocated
bucket. These compiler limits substantiate an occupancy cost; they are not measured
runtime occupancy, cache misses or memory-stall counters. Full provenance and the
actual shader identification are in `reference/naadf-followup/shaderstats.json`.

## CPU instrumentation scope

`FRONTIER_NAADF_CPU_PROFILE=1` enables coarse stage clocks and explicit temporary
allocation requests. Region stage times are summed worker *wall intervals*, not
OS CPU cycles; parallel-region wall is reported separately and must not be added
to those totals. This includes unclassified time rather than attributing it to
an assumed stage. `FRONTIER_NAADF_STACK_BLOCK_BOUNDS=1` selects the identical
stack-based block proof builder; canonical arrays and proof words must match.

## One coarse height probe

FRONTIER_NAADF_HEIGHT_PROBE_LEVEL=2 probes one selected maximum-mip tile and
returns to ordinary exact AADF traversal when the proof is insufficient. With
the default 8-root base tile, level 2 covers 32×32 metres. The level is clamped to
the available pyramid. This enables the existing pyramid without generating
another root representation. Pair it with FRONTIER_NAADF_HEIGHT_GATE=1 to query
only empty roots and require more reach than the known AADF exit. Rejected
maxima are reused within the selected tile below its bound. Corner/stall guards
remain active; defaults and the full-pyramid control remain available.

This opt-in refinement responds to the measured lookup ratio, not a speed claim.
The gated pyramid needed about 8.1 height reads per primary root load avoided in
the forward diagnostic view. A single probe trades some opportunities for one
height read per attempted query. reference/naadf-followup/height-probe.json is
the uninstrumented timing configuration; height-probe-diagnostics.json enables
counters. Run sequentially with tools/naadf_experiments.py using radius 128 and
--mode capture for the diagnostic configuration; omit that mode for timing.

## Direct proof publication scope and provenance

`FRONTIER_NAADF_DIRECT_ROOTS=1` with GPU distance enabled publishes monotone
improvements into ordinary root words. It automatically retains CPU fallback
proofs; `FRONTIER_NAADF_ROOT_REPAIR=1` selects the same seeded repair for the
optional-atlas comparison. A reserved `CHILD|FULL` root tag marks only an air
proof that actually changed. Unchanged CPU proofs remain byte-identical and
cannot trigger direct-mode native replay. Tags follow retained physical root
slots across shifts, even after their original tiles leave the scheduling plan;
fresh incoming strip words overwrite them. Root consumers normalize the tag,
including height bounds, shadows, native replay, and ray feedback. The publisher
uses one additional 128 KiB original-word scratch buffer to distinguish actual
growth from a no-op. Source-format and GPU publication tests check reserved tags,
unchanged occupied pointers, tagged air, and queued shifts.

The scheduler still covers at most a central **128 m cube**. At radius 64 m this
covers the whole window, so a streaming run at that radius can exercise incoming
strip repair. At radii 128/256 m, a short circuit with roughly 35 m net extent
never brings incoming outer strips into the central cube; such a run measures
maintenance overhead but cannot establish the value of repairing those strips.
Use a long enough straight route, or explicitly disclose that restriction.
Scheduling-plan expansion is outside this experiment.

For fixed comparisons, set `FRONTIER_NAADF_SETTLE_ROOT_DISTANCE=1` to keep the
headless capture startup gate open until every scheduled tile is ready. Require
`gpu_root_distance_tiles_ready == gpu_root_distance_tiles_total` in the saved
metadata; a 1/64-ready capture does not certify the fully built cache. Fresh CPU
proofs may already be converged, so a fully built GPU schedule may change no
root words. `gpu_root_distance_direct_publications` counts tile submissions,
**not proof growth or tagged roots**. Production totals for those changes remain
unmeasured; GPU oracle census checks validate provenance but do not measure work
saved in a rendered route. Instrumented ray loads, replay counts, and matched
streamed geometry provide separate evidence of useful work.

The v2 settled fixed capture at 1920×1080, radius 64 m, seed 7 and bounds
`[-106,-36,-314]..[22,92,-186]` is bit-identical to the CPU control in both
linear RGB and depth. The direct run reports 64/64 ready tiles and 64 tile
publications. Matched camera, source and vegetation metadata passed the harness
checks. Artifacts are under `output/naadf-followup/v2/settled-direct-capture/`,
using launcher SHA-256
`8665497fcc5ee1b532d8c58e9333e8352ce3c0ec616c60ca01727953e63adfd8`.
This does not establish how many proofs changed or a streaming performance gain.
The opt-in GPU gates also pass direct corner/near-corner primary and shadow
parity, zero replay on unmodified CPU proofs, tagged-air misses, occupied-pointer
preservation, history beyond the current scheduling plan, queued shift publication,
and the five-state canonical ray oracle. See `v2/direct-gpu.log` in the same
artifact directory tree.

## Local exit repair prototype

The measured radius-64 motion case made the direct path much slower than CPU
proof traversal (52.84 ms versus 19.24 ms whole frame): whole-ray native replay
can overwhelm any saved root lookups. `FRONTIER_NAADF_LOCAL_EXIT_REPAIR=1` is a
new opt-in experiment, requiring direct roots; it is not a promotion or a speed
claim. It keeps the v2 global fallback for rejected local repairs and stalls.

The local handoff operates immediately at an ambiguous empty-cuboid exit rather
than replaying from the eye after a later hit. For an integer plane `k` on axis
`j`, define the same function used by native replay:

```
T_j(k) = (f32(k) - o_j) / d_j
```

If `B_j` is the outgoing face of the certified-empty cuboid, the exit event is
`tau = min_j T_j(B_j)` over nonzero directions. For a candidate native cell `c`,
its entry and exit events on an axis are:

- Positive direction: `entry = T_j(c)`, `leave = T_j(c+1)`.
- Negative direction: `entry = T_j(c+1)`, `leave = T_j(c)`.
- Zero direction: keep the current cell coordinate and generate no face.

The helper finds `entry <= tau < leave`, correcting the approximate coordinate
by at most one cell. Every axis whose entry equals `tau` contributes an entry
face, including a transverse native plane inside the larger cuboid. It returns
`(cell, tau, face_mask)` directly to the existing hierarchy; the caller does not
recompute `floor(o+d*tau)` or perform any occupancy reads during the handoff.

**Conditional discrete-DDA argument:** assuming a valid current DDA state and
strictly ordered, deterministic native plane events, the first event leaving
the cuboid is `tau`. Every previously traversed native cell lies in the certified
empty cuboid. The interval tests uniquely identify the cell immediately after
that event, and their equality tests recover all simultaneous entry planes.
The next ordinary hierarchy lookup therefore tests the first cell that might be
solid, with native entry-face priority. The same argument applies to shadows.
This derives from the integer cell/event state model of
[OpenVDB's DDA/HDDA](https://www.openvdb.org/documentation/doxygen/DDA_8h_source.html),
rather than treating a rounded reconstructed position as authoritative.

Acceptance also requires finite bounded operands, a forward event, strictly
ordered neighboring entry/exit times, consistent cuboid membership, and a spatial
engineering envelope below 0.25 native cells. Large cancellation/amplification,
subnormal nonzero direction components, and extreme outside origins reject the
shortcut. Failure is sticky for the ray and preserves global native replay even
if subsequent traversal misses. Successful local corrections are diagnostic event
19; events 30/31 still count global replay and its native cells.

**Numerical limitation:** the envelope is an engineering filter, not a complete
portable error proof for every allowed WGSL evaluation. [WGSL's numeric rules](https://www.w3.org/TR/WGSL/#floating-point-evaluation)
allow 2.5-ULP division error, reassociation/fusion and do not specify one rounding
mode; `fma` does not supply a portable exact-product residual. The prototype's
conditional contract is consistency with the shared native float-event function,
not exact real-number intersection for every conforming implementation. A general
real-ray proof needs filtered exact predicates or a separately specified arithmetic
contract. [Shewchuk's adaptive predicates](https://www.cs.cmu.edu/~quake/robust.html)
provide the relevant model of a cheap filter followed by exact sign evaluation.

A fixed epsilon or a fixed two-cell rewind cannot replace those conditions:
near-parallel rays can remain arbitrarily close to a grid plane, and a rounded
starting cell can stay on the wrong side throughout the local interval. The
[corrected robust BVH analysis](https://jcgt.org/published/0002/02/02/)
also distinguishes conservative box overlap from robust primitive intersection;
its padding rule does not itself establish voxel identity or an entry normal.
Further implementation requires independent grazing/corner/native parity and
matched image tests before any performance result can justify using this path.

## Measured CPU preparation

The instrumented 128 m, 35-second circuit recorded seven successful shifts with
median preparation 1,308 ms. The parallel-build interval was about 95% of total
preparation wall time. Summed worker-wall shares were approximately 31% column
sampling, 24% leaf construction/deduplication, 23% vegetation stages, 8% root
proofs, and 1.4% block proofs; unclassified region time was explicitly retained.
These proportions do not sum separate parallel stages into a sequential total.
The profiling run is not used for ordinary FPS claims. See
[`cpu-profile128.json`](reference/naadf-followup/cpu-profile128.json).

Stack scratch removes about 17,400 temporary Vec requests per larger shift, but
it targets the small block-proof bucket. Small-fixture timing ranges overlap;
there is no established total speedup. Exact column extrema reuse attacks repeated
height/deep-band classification instead. In its small fixture the classification
stage decreased, but other unchanged stages varied too: attributing the whole
prepare-time difference to extrema would be unjustified. Larger matched runs
are recorded separately. See
[`cpu-extrema-fixture.json`](reference/naadf-followup/cpu-extrema-fixture.json).

## Conditional height-bound win

At an elevated eye `(-41.06016,100,-250)` looking toward `(-39,110,0)`, radius 128,
1080p and the ordinary full renderer, reversed-order repeats gave:

| Order | Policy | Frame p50/p95/p99 ms | NAADF span p50 ms |
| --- | --- | --- | ---: |
| 1 | Control | 12.80 / 15.30 / 16.04 | 1.80 |
| 2 | Full mip bounds | 12.02 / 14.59 / 15.47 | 1.06 |
| 3 | Full mip bounds | 12.26 / 15.05 / 15.80 | 1.09 |
| 4 | Control | 13.07 / 15.71 / 16.49 | 1.98 |

The ray span improves substantially while complete frames improve about 6%.
Most frame cost in this view lies elsewhere; these are independent timestamp
summaries, not per-frame paired components. Ground-level results remain losses.
An earlier single elevated beam run was essentially tied (12.45 vs12.42ms),
not an established gain. The result supports workload-dependent selection, not
turning every hint on globally.

## V2 motion and larger-radius controls

All these runs use the same final v2 executable, source, 6.25 cm native cells,
1080p, MSAA1, full lighting and adaptive distant coverage. Scatter is enabled in
all motion candidates; certified interior raster clusters remain omitted.

At radius 64, where the repair scheduler covers the entire volume, 45 seconds
of continuously repeated walking/turning produced:

| Policy | Frame p50/p95/p99 ms | NAADF span p50 ms |
| --- | --- | ---: |
| CPU proofs + scatter | 19.24 / 35.95 / 43.39 | 8.67 |
| Seeded repair with optional lookup | 22.54 / 37.61 / 47.57 | 11.00 |
| Direct publication, global native replay | 52.84 / 107.53 / 121.96 | 36.15 |

Each completed 13 shifts with zero frames outside exact coverage. Correctness is
not a speed endorsement: global native replay remains an expensive way to resolve
corner ambiguity. The v3 local-exit experiment targets that cost; it is a new
candidate rather than a relabeling of this losing result.

The 128/256 m forward, reverse and sky-heading views plus 45-second circuits
are saved in [`measurements.json`](reference/naadf-followup/measurements.json).
Differences in larger static views are generally small and not established from
single runs. At radius 256, circuit p50 was 23.90 ms control versus 24.01 ms direct;
p99 was 41.44 versus 44.15 ms. Neither lost exact coverage. The scheduler cap means
these short larger-radius circuits do not establish incoming-strip repair value.
Initial radius 256 builds took roughly153–158 seconds; process peakRSS was about
3.9–4.2 GB in those fixed views. This startup cost is distinct from steady frames.

The v3 local prototype passes the independent 11-case GPU event/hit fixture:
exact and perturbed corners, a 96-cell grazing skip that defeats rounded-position
classification, zero components, moderate outside origins, and rejected extreme
origins. The valid grazing case executes event 19 with zero global native
replays; the extreme-origin case retains the fallback. Direct publication,
provenance/wrap, queued publication and canonical ray regressions also pass.
The canonical traversal matrix passes 12 configurations in both ordinary and
direct-root modes (144 batches total). These are hardware gates, not a portable
proof for every WGSL implementation.

Settled radius-64 captures at 1920×1080 are bit-identical in linear RGB and depth
for control, direct, and local-direct. Both GPU modes report 64/64 ready tiles.
The v3 control and direct-with-local-off images also match their frozen v2
counterparts exactly. Configuration: `reference/naadf-followup/settled-local-exit.json`;
artifacts: `output/naadf-followup/v3/settled-local-capture/`, including
`legacy-v2-comparison.json`; launcher SHA-256
`52c6c900a79ba22637306997dee23ef32bee66182b50fa1815ac751dd52fa8a3`.
The compiler reports 56 VGPRs / 18 estimated subgroups for local repair versus
40 / 24 for direct alone, with no spills. That cost applies even when local
work is rare; fewer native replay visits do not establish faster rendering.
Whole-route timing remains a separate gate.

## Final local-exit and CPU classification comparisons

The guarded local-exit candidate passed the independent 11-case native-event
fixture and both 72-batch canonical variant runs (4,718,592 coverage decisions).
Fully settled 1080p control/direct/local captures match in color and depth. The
original fixed Legacy PNG/color/depth also remain unchanged. These correctness
results do not imply a performance win.

Same v3 executable, radius 64,1080p, 45-second circuit, scatter enabled throughout:

| Policy | Frame p50/p95/p99 ms | NAADF span p50 ms |
| --- | --- | ---: |
| CPU proofs | 19.22 / 36.14 / 44.18 | 8.66 |
| Direct proofs + global native replay | 56.78 / 109.70 / 126.05 | 38.37 |
| Direct proofs + guarded local exit | 24.02 / 36.89 / 46.28 | 12.32 |

Local exit removes most of the prototype regression, but still trails the CPU
control. The compiler gives the primary shader40 VGPRs / 24-subgroup capacity for
control/direct versus 56 / 18 for local repair; this is a real capacity tradeoff,
not a runtime stall measurement. Full mip bounds allocate 48 / 20; gated and
single-probe shaders 64 / 16. There are no spills. The single probe therefore saves
reads without reducing that allocation bucket. Its128m ground-view frame p50 was
20.22ms versus18.69ms control, and ray span10.12 versus8.11ms. Neither is promoted.
See [`shaderstats-v3.json`](reference/naadf-followup/shaderstats-v3.json).

The actual mountain-near test uses `--start-xz 0,3000`, grounded automatically
at `(0,647.45,3000)` and looking toward the Alpine range. At radius 128, control
frame p50/p95/p99 was 19.32/22.14/22.96 ms, local-direct 19.90/23.05/23.91 ms.
These are single runs and do not establish a small difference.

Exact column-extrema reuse was tested at radius 128 in reversed 35-second circuit
runs, all with scatter and identical quality. Median worker preparation was
1,211.5/1,353.7ms for the two controls and1,311.9/1,241.8ms for the two extrema
runs. Frame medians likewise crossed order. The smaller classification fixture
improves its targeted stage, but these whole-route results do not establish a
large preparation or FPS win. It remains opt-in.

## Actual GPU maintenance timings

`FRONTIER_NAADF_GPU_STAGES=1` enables a bounded, asynchronous 24-slot timestamp
pool using only the ordinary TIMESTAMP_QUERY feature. Disabled profiling creates
zero queries and zero GPU buffers. Enabled resources are 48 queries and 6,528 bytes
of explicit buffers plus 480,000 bytes of logical CPU sample capacity; opaque
backend query storage is not exposed. Samples drop rather than blocking frames.
Only explicit capture/report flush waits. No shader or dispatch is duplicated.

Two real-scene 64 m / 1080p, 12-second circuits separately exercised repair/feedback
and height/beam work. They are instrumented diagnostics, not FPS comparisons. Stage histories include
initial readiness and warmup as well as the active route:

| Original compute work | Samples | GPU p50 ms | GPU p95 ms |
| --- | ---: | ---: | ---: |
| Root repair batch, one tile | 125 | 0.238 | 0.399 |
| Feedback dispatch | 32 | 0.034 | 0.040 |
| Height base | 5 | 0.120 | 0.131 |
| Height reduction chain | 5 | 0.013 | 0.027 |
| Beam prepass | 503 | 0.016 | 0.021 |
| Scatter dispatch, repair run | 4 | 0.316 | 0.942 |

All finished with zero pending/encoded, dropped or invalid samples. Small sample
counts have weak tail statistics. GPU host staging/copies, query resolve/readback
and CPU work are explicitly outside these compute spans. Full-frame pacing and
stream publication remain necessary for end-to-end cost. These measurements
show why eliminating some maintenance dispatches alone cannot explain or remove
multi-millisecond per-ray overhead. See
[`gpu-stage-real-scene.json`](reference/naadf-followup/gpu-stage-real-scene.json).

Internal primary versus shadow queries cannot be individually timestamped by
this portable pass-level mechanism. Explicit 64 m static full/primary/voxel-shadow/
mesh-shadow ablations are recorded in
[`shadow-ablations.json`](reference/naadf-followup/shadow-ablations.json).
Full versus primary-only NAADF-span p50 was6.75 versus6.04ms in those single
runs. Switching queries changes lighting and divergence; these marginal results
are not independent additive timings or quality-equivalent optimization wins.

## Acceptance and reproduction

Keep the existing CPU-proof traversal as the default. Scatter remains the
recommended optional streaming improvement from the preceding checkpoint.
Additional bounds, caches, repair and beam modes remain experiments. The seven
families are implemented, refined and measured; negative results are retained
with their implementation/workload limitations rather than called impossible.
No opaque fog, smaller native region, reduced vegetation, disabled shadows or
lowered resolution is hidden in a winning comparison.

```sh
# Normal recommended streaming playtest from frontier/
FRONTIER_NAADF_SCATTER_UPLOAD=1 ./run.sh --preset alpine --naadf-radius 128 --naadf-follow on --msaa 1 --resource-profile development
# Exact CPU classification experiment
FRONTIER_NAADF_SCATTER_UPLOAD=1 FRONTIER_NAADF_COLUMN_EXTREMA=1 ./run.sh --preset alpine --naadf-radius 128 --naadf-follow on --msaa 1 --resource-profile development
# Reproduce the local repair comparison (fresh output directory)
python3 tools/naadf_experiments.py --config reference/naadf-followup/local-motion.json --binary target/release/frontier --output output/local-recheck --radius 64 --route walk-circuit --seconds 45 --run
# Profile compute stages; inspect naadf_gpu_stages in the JSON report
FRONTIER_NAADF_GPU_STAGES=1 FRONTIER_NAADF_SCATTER_UPLOAD=1 ./run.sh --preset alpine --naadf-radius 64 --naadf-follow on --msaa 1 --resource-profile development --adaptive-wait-quality --benchmark-input-lock --route walk-circuit --seconds 12 --uncapped --report output/stage-profile.json
```

`tools/naadf_workloads.py` reproduces the checked-in 128/256 m camera/motion matrix.
`tools/naadf_traversal_counters.py` summarizes explicitly instrumented captures.
Normal timing harnesses reject atomic, CPU-clock and GPU-stage diagnostic flags.
All performance binaries and commands are fingerprinted in the measurement data.
The final profiling-only binary is SHA-256
`6e1a1cd7813378267fa7b64a3cf78774ab4ec5ba6333ed1dda9181fce6cb0651`;
all 8 WGSL files are byte-identical to frozen v3. Default-off control/direct/local
1080p color/depth are unchanged. Final uninstrumented64m control frame
p50/p95/p99 was 17.89/20.80/21.60 ms; this is a sanity run, not a new optimization
claim. Validation: 460 renderer tests, 35 runtime tests, the earlier native/canonical
GPU gates, timestamp-only-device dispatch-count test, all-six-stage canonical
hook test and enabled/disabled capture-flush tests passed.

Next work should target measured costs: terrain sampling/leaf deduplication and
vegetation preparation on CPU; common-path shader state/register lifetimes and
query cost on GPU. A ray-guided cache policy or another bounds format should be
judged on a scene where its maintained data is useful. Runtime cache misses,
actual wave occupancy and memory stalls remain unmeasured; compiler limits and
load counts are evidence, not substitutes for those counters.
