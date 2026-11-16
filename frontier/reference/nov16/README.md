# November 16 serial GPU spot checks

These are three completed cases and one preparation stopped by the global
watchdog, using one frozen release binary. No experiment is promoted. The
static candidate combines probe density and deferred appearance scheduling;
this pair cannot separate their performance effects. The planned combined
tree/AA case produced no measured result.

[Machine-readable commands, hash, conditions and results](summary.json).
Raw reports, logs, display-linear arrays and the run manifest remain under
`frontier/output/nov16/gpu-comparison/`.

## Matched conditions

- AMD Ryzen 5 7640 U, Radeon 760 M integrated GPU, RADV PHOENIX,
  Mesa 25.2.8, Vulkan. Actual adapter verified in every completed report.
- Alpine, seed 7, canonical 6.25 cm cells,1920×1080, render scale 1,1×MSAA,
  pixel-error target 5,1,000 m clip, ordinary Alpine lighting/atmosphere,
  requested/published 64 m native radius,4 m native terrain mesh,
  32 m near vegetation, four adaptive workers, disk cache disabled.
- Cold independent process per case; benchmark waits for native coverage and
  initial refinement reserve. Ten warmup frames, uncapped presentation.
- Static pair samples 2 seconds; motion control samples 10 seconds of the
  `walk-turn-return` realtime route at 2.5 m/s. Only these children use the GPU;
  other project CPU work was paused during the 120 second global run.

Binary SHA 256: `d35b1c32ebb13dfa6d9d2c4896f7832f5c2baf873ce5672edc45baea34aa10c6`.

## Measurements

Wall time includes startup/preparation, warmup, sample interval and exit capture.
Frame percentiles include CPU/presentation. Full-GPU percentiles are asynchronous
unpaired command-span timestamps including shadows, scene, atmosphere and UI;
they exclude prior uploads, CPU preparation and presentation.

| Case | Total wall s | Frame p50/p95/p99 ms | Full GPU p50/p95/p99 ms | Quality-ready frames | Peak process RSS MiB |
|---|---:|---:|---:|---:|---:|
| `static-control` | 29.85 | 19.52 / 22.45 / 23.30 | 18.55 / 18.93 / 19.08 | 90/90 | 827.2 |
| `static-cover16-deferred` | 34.67 | 19.94 / 21.91 / 22.56 | 18.31 / 18.87 / 19.15 | 85/86 | 877.2 |
| `motion-control` | 37.87 | 25.87 / 34.06 / 50.12 | 22.55 / 30.35 / 38.27 | 68/369 | 1152.6 |

All accepted static frames had coherent coverage. Static control was
quality-ready 90/90 frames; the candidate 85/86, with both exit captures fully
geometry/appearance-ready and no pending vegetation. Native publication was
64/64 m, source revision 0, stage 100, without expansion or pending content changes
in both captures. Native GPU payload was 90,941,956 bytes in both cases.

The combined cover candidate required more preparation and memory in this
single run. Shared cover retained-estimate bytes rose from 6,796,224 to 43,456,344;
summed native cover leaf work rose from 5,458 ms to 26,354 ms. Deferred scheduling
completed 424 jobs, promoted 379, had zero payload/allocation refusals and zero
outstanding work at capture. Its first appearance-ready observation was 25.07 s.
The full-GPU p50 difference is small and the frame p50 moves in the other direction;
these short ordered runs do not establish a performance improvement.

Motion control retained coverage 369/369 frames but quality only 68/369. It had
29 unbounded-error frames, peak 36 unresolved clusters and a 50.12 ms frame p99.
This remains a quality/refinement defect during the route; initial settled
readiness does not certify quality throughout movement. Its exit capture is
coherent but is explicitly **not quality-ready**. There is no completed matched
motion candidate, so this is a baseline observation rather than an A/B claim.

The planned tree-matched plus post-AA 0.5 case hit the remaining 17.74 s preparation
budget. It has no accepted frame sample/report/capture and is censored, not an
FPS failure result. The global run stopped at 120.16 s and released the GPU/CPU
workers. No isolated AA or tree result was obtained.

## Actual captured appearance

[Static control](static-control.png),
[16-probe deferred candidate](static-cover 16-deferred.png),
[matched far-hill crop](far-hill-pair.png),
[motion control exit](motion-control.png).

The static capture eye, target, FOV, clip and output dimensions match exactly.
Both were inspected at full resolution and in the same enlarged far-hill crop.
The far meadow population/color pattern changes; foreground native grass and
flowers remain identical in the bottom 180 rows. The sky is also byte-identical.
The static pair shows coherent terrain and trees in these views; this is not a
walking, grazing, silhouette-error or native-reference acceptance check.

Direct display-linear RGB differences from the 8 bit-target decoded arrays:

| Region | RGB RMSE | Changed pixels |
|---|---:|---:|
| Full 1920×1080 |0.002151|2.70%|
| Sky rows 0–630 |0|0%|
| Terrain rows 760–1080 |0.001738|2.08%|
| Far-right hill x 1250–1920,y 680–800 |0.008894|48.37%|
| Near grass rows 900–1080 |0|0%|

These numbers quantify a representation difference, not accuracy: there is no
paired 64 spp native oracle for this camera in this spot check. A smoother-looking
population estimate is not proof of better source fidelity. The separate CPU
64-leaf oracle in [the experiment report](../../NOV 16_REPRESENTATION_EXPERIMENTS.md)
measures source-statistics error, not pixel error.

## Reproduce

From the repository root, after building the same frozen source:

```sh
python3 frontier/tools/nov16_comparison.py --output "$PWD/frontier/output/nov16/my-comparison" --run
```

This helper runs serial cases with a 40 second per-case and 120 second global
watchdog, plus live RAM headroom. It clears inherited `FRONTIER_*` experiment
variables and records every actual command/override. The historical fourth case
combines tree matching and AA; even if it completes in a future run, attribution
requires isolated tree and AA comparisons. Raw images are preserved and only
curated images/summary are stored here. No screenshots or frame measurements
were taken under screen-recording overhead.
