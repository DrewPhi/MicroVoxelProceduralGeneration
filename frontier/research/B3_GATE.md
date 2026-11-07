# B3 GPU meshing eligibility audit

2026-11-07. Tier G: read-only audit of current frontier code and preserved local
measurements plus bounded source/volume diagnostics; no runtime changes, copied
external code or GPU meshing implementation.

**Decision: defer B3 implementation.** Current evidence does not identify CPU
native meshing as a consequential readiness bottleneck. Re-evaluate after B1/B2
with matched source data and source-exclusive profiling. This is a conditional
rejection for this workload, not a general claim against GPU meshing.

## Measured evidence

The frozen packed static reports in
[`integration-results.json`](../reference/issue75/integration-results.json),
`experiments/packed-static-{1,2,3}/{baseline,candidate}.json/adaptive`, report:

| Pair | Actual mode | First quality wall ms | Native mesh worker ms | Appearance + certificate worker ms |
| --- | --- | ---: | ---: | ---: |
| 1 | CPU profiles | 10,791.420 | 253.783 | 23,195.357 |
| 1 | GPU profiles | 10,174.740 | 277.500 | 20,382.284 |
| 2 | CPU profiles (candidate arm) | 10,133.692 | 255.156 | 22,000.083 |
| 2 | GPU profiles (baseline arm) | 10,381.163 | 274.837 | 21,764.157 |
| 3 | CPU profiles | 10,238.857 | 254.786 | 22,400.784 |
| 3 | GPU profiles | 10,486.333 | 268.109 | 21,791.645 |

Hardware and frozen binary are documented in
[`INTEGRATION.md`](../reference/issue75/INTEGRATION.md): Ryzen 5 7640U,
Radeon 760M, four preparation workers, 1080p, Legacy seed 7, 6.25 cm cells,
300 m clip, disk cache disabled. The native stage is roughly 1.1–1.4% of the
combined native/appearance/certificate worker sums. That ratio describes work
accounting only: it is neither an end-to-end speedup prediction nor the fraction
of first-quality wall latency spent waiting on mesh output. Reports accumulate
work beyond first quality, and workers overlap. Even removing all measured native
stage work would remove only about 254 ms of summed CPU work in each CPU static
run, including source work; this does not support a >10% readiness claim.

The preserved 15/80 m/s CPU route reports likewise contain only 493.841/463.000 ms
native stage work against 31,680.314/31,391.808 ms appearance + certificate work.
They do not establish a moving-view quality pass or edit-latency distribution.

## Actual timing and probe scopes

- `adaptive_geometry.rs::build_surface`, `step_cells == 1`, times
  `column_mesh::build_sampled` as `native_mesh_ms`. Its callback queries the
  shared canonical source cache. This is mesh extraction **plus source lock,
  lookup, generation and insertion**, not exclusive extraction. Native appearance
  and profiles run after this timer, under appearance accounting.
- `SourceSamples::profiled_column` separates lock, lookup and canonical generation
  only with `FRONTIER_PROFILE_SOURCE=1`. `GeometryCache::stats` subtracts caller
  source costs from stages when constructing `exclusive_cpu_profile`. All six
  packed static reports have `enabled=false`; their zero source timings mean
  unavailable diagnostics, not free generation. The exclusive-named mesh value
  consequently equals the inclusive native stage there.
- `proxy_ms` is a residual from total build accounting, including more than native
  face extraction. Do not count the entire proxy residual as GPU-meshable work.
- `gpu_probe.rs` timestamps the asynchronous terrain render-pass subset, with
  dropped samples on saturation. It does not measure upload, compute, submit,
  whole-frame GPU time or readiness. Frame percentiles remain separate evidence.
- Existing GPU profile/summary kernels construct appearance histograms from CPU
  canonical snapshots. They do not output face geometry or remove CPU source
  generation. Profile enqueue CPU time is not compute execution; integrated
  profile `gpu_ms` is null. The bounded numerical summary probe does not establish
  production meshing throughput.

Thus the available evidence cannot attribute the dominant appearance/certificate
stages to meshing, or promise that moving them to a GPU face kernel solves them.
The prior GPU profile experiment also adds source upload/snapshot cost; its three
static pairs do not establish readiness or frame-tail improvement.

## Bounded diagnostics performed in this pass

Raw metadata, commands, frozen hashes and measurements are retained in
[`b3-gate-results.json`](../reference/issue75/b3-gate-results.json).
The frozen binary hash is `830cc579cbced65a7fece2b51fbb9cdc9bc32df56fa4ed3b9733255e9e5362e6`.
A 300 m profiler attempt selected llvmpipe inside the sandbox and was cancelled;
a second hardware attempt was also cancelled incomplete to keep the task bounded.
Neither supplies readiness measurements. Actual device access for the completed
40 m clip diagnostic selected the Radeon 760M. All other source, seed, camera,
resolution and quality parameters are recorded; this clip is explicitly different
from the primary 300 m gate.

With CPU profiles and `FRONTIER_PROFILE_SOURCE=1`, the completed 40 m diagnostic
reported first quality at 25,579.682 ms. Its cumulative inclusive native stage was
5,681.542 ms; source-exclusive native extraction was 2,752.437 ms. Exclusive
appearance/certificate were 24,848.716/8,160.158 ms. Source lock, lookup and
generation sums were 19,784.741/17,773.545/2,840.839 ms. These are overlapping
worker sums, not wall intervals or additive first-quality latency components.
The per-column clock instrumentation increases lock hold time and perturbs work;
this result must not be compared to the historical uninstrumented primary as a
regression or a speedup. Native extraction is demonstrably nonzero, but remains
smaller than appearance/certificate work in this scoped diagnostic.

The standalone [`b3_fixture_profile.rs`](../tools/b3_fixture_profile.rs) builds
against existing frozen release world/bytemuck dependencies and uses the existing
CPU mesher, without modifying production Rust modules. It materializes a 66³
material halo for a 64³ brick, then extracts from that cached array, and separately
extracts directly from the world. Three repeats cover a fixture cave with a small
air edit and a bridge brick. All cached/direct vertices and indices match exactly.

| Fixture | Source materialization ms, range | Cached extraction ms, range | Direct source + extraction ms, range | Native faces / triangles |
| --- | ---: | ---: | ---: | ---: |
| Edited cave | 21.460–23.891 | 21.220–28.221 | 43.442–44.237 | 12,032 / 216 |
| Bridge | 9.538–29.117 | 12.216–27.472 | 22.712–43.221 | 6,982 / 2,424 |

Each halo is 287,496 logical material bytes. Cached extraction includes the
mesher's own halo copy, masks, greedy merging and allocation; it is not an isolated
face-mask kernel. These CPU-only intervals exclude upload, GPU competition and
publication, and do not establish edit latency. Source and cached timings are
separate passes, so do not substitute their sum for the measured direct interval.
The large variability and possible concurrent compilation make all new timing
values exploratory. They justify checking the edited-volume critical path before
rejecting GPU meshing for that use case, but supply no transfer-inclusive GPU win
or measured >10% end-to-end opportunity. No expensive further run was launched.

## Smallest next gate and conditional prototype

First use the existing source profiler in an isolated, bounded primary workload
with CPU profiles, disk cache disabled and frozen source/camera/quality. Keep
instrumented and normal runs separate because per-column clocks perturb work.
Record stage deltas through first quality rather than treating final cumulative
worker counters as that interval. If an edited cave/dense-brick workload is the
motivation, measure its input-to-coherent-publication critical path separately;
heightfield static timings cannot rule out that use case.

Only if post-B1/B2 measurements show material source-exclusive face extraction
on the critical path, implement one revision-keyed bounded source brick plus
one-cell halo, emitting the existing face ABI into a fixed-capacity GPU buffer.
Compare with the CPU mesher on exactly the same immutable material data. Test
empty, solid, checkerboard worst-case, cave boundary and edited-brick payloads;
compare integer face ownership/material/axis and native depth/coverage. Use an
explicit overflow flag and CPU fallback, rejecting stale revisions before
publication. Successful GPU jobs must skip redundant CPU geometry construction.

Measure source preparation, upload, completed compute timestamps, allocation,
synchronization, draw consumption and coherent publication separately. Diagnostic
readback is allowed for parity tests but must not be inserted into the intended
production path. Keep one brick and bounded output until parity and measured
transfer-inclusive benefit pass. Advance only for >10% end-to-end improvement
above repeat variance or a material edit-latency gain at unchanged quality and
memory limits; a fast isolated kernel is insufficient. Until that condition is
observed, B3 stays deferred with no new GPU meshing code or default changes.
