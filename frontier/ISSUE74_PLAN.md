# Issue #74: evidence, bounded milestones and decisions

2026-11-06. Owner: [consolidated issue #74](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/74).
This is an investigation and implementation plan, not completion of the epic or
promotion of a renderer. Follow [AGENTS.md](AGENTS.md),
[ARCHITECTURE.md](ARCHITECTURE.md) and the
[fixed primary visual check](PRIMARY_VISUAL_CHECK.md). External and legacy code
remain read-only references; proposed implementation work is greenfield unless
separately approved as a documented selective port.

The [first measured checkpoint](reference/issue74/README.md) now supplies refreshed
baselines, report provenance, exact native-material reuse and allocation cleanup.
The local Regional native-profile speedup does not translate into an established
whole-view speedup; M1's broader latency gate remains open. M0 instrumentation
is improved, but exclusive low-overhead source profiling and full route/distance
coverage are still incomplete.

The [follow-up investigation](reference/issue74/FOLLOWUP.md) now reproduces the
4 km cache-capacity refusal and strict motion-quality failures. Minor cache and
cancellation candidates do not establish a sufficient improvement. Stop the
20 km sweep at this refusal; do not mark M3/M4 or the epic complete.
The [vertex-page experiment](reference/issue74/submission-experiment.md) passes
the bounded M2 keep screen and is enabled by default with an explicit off switch.
Its measured 1 km p95 remains above the overall 60-FPS target.

## Reference first, then real time

For every feasible visual feature, produce a non-real-time exact/reference image
first, review it with the user, and optimize real-time output toward that approved
target. A fast implementation that misses the target remains unfinished. Freeze
world/source revision, seed, canonical 6.25 cm cells, camera, projection, clipping,
lighting, materials and output/filter settings with each target. Declare finite
reference domains and supersampling; compare depth, coverage, normals and material
as well as RGB where available. A reference renderer is an oracle only for the
semantics it actually implements.

The approved Legacy seed-7 player-height 300 m target stays fixed. The Regional
preview is a separate aesthetic target; its offline oracle has not yet been
updated to the preview's lighting. Do not report its current RGB mismatch as a
geometry verdict or its pleasant appearance as Legacy regression acceptance.
Develop a matching offline Regional image and obtain approval before tuning its
real-time lighting/appearance. New reference work may run slowly; production
performance requirements still apply to the resulting runtime.

Transition polish comes after still geometry/appearance agreement. Before tuning
transitions, generate reference trajectories for stationary/grazing views, short
pans, walking, boundary crossings, flight/bank, teleport and edits as applicable.
Compare temporal coverage, silhouette motion, material stability and ghosting.
A single screenshot cannot certify transitions, moiré suppression or streaming.
Atmospheric/perceptual profiles may be specified separately; they do not pass the
fixed no-haze geometry gate by implication.

## Renewed baseline and its limits

Existing artifacts are under `output/issue74/baseline/`; source reports, captures,
commands, binary hashes and process logs remain the evidence. All three cases use
four preparation workers and eight refinements per transaction. These are small
samples, not a randomized performance claim.

| Case | First requested quality | First reserve | Settled frame p95 |
| --- | ---: | ---: | ---: |
| 300 m A | 10.77 s | 12.24 s | 10.26 ms |
| 300 m B | 10.85 s | 12.28 s | 10.17 ms |
| 1 km | 53.45 s | 71.06 s | 16.906 ms |

First coverage is a separate milestone and must not be substituted for requested
quality or reserve. The 1 km p95 exceeds the 16.67 ms whole-frame target; static
settling does not establish motion acceptance. No new 4 km/20 km acceptance or
sustained flight result is implied. Earlier 4 km capacity/quality failure remains
an unresolved case, not a successful settled run.

For 300 m A, the **31.21 worker-seconds** of build accounting break down as
appearance 64.4%, affine bounds 11.4%, certificates 10.0%, remainder 9.7%, finalize
3.6% (rounding applies). These are worker CPU sums, not fractions of cold wall
latency or frame time. Source calls are included in caller stages; affine and
certificate/inclusive accounting require care. Workers overlap each other,
publication and GPU activity. Disabled exclusive profiling does not measure zero
source cost. Do not use these percentages as Amdahl fractions or promise a wall
speedup from eliminating one stage.

`tools/issue74_report.py --output REPORT.json RUN_DIR ...` aggregates existing
reports, with Markdown alongside. It preserves raw fields, command/sweep identity,
first coverage/quality/reserve, frame/GPU tails, CPU accounting caveats, memory,
cache and SHA256 of decoded RGB bytes. Missing metadata stays unknown. Unknown
presets and interactive settings changes cannot form a frozen comparison group.
Old sidecars disclose atmosphere/scale but omit full sun/exposure/fog settings;
those runs retain useful timings without establishing complete preset identity.
A numerical screen never establishes visual or temporal acceptance. New report
metadata must describe actual rendered settings and distinguish terrain-pass GPU
timestamps from whole-frame cost.

## Ranked hypotheses, not measured benefits

| Priority | Hypothesis and evidence | Next discriminator | Main risk |
| --- | --- | --- | --- |
| 1 | Repeated exact appearance/source preparation dominates recorded worker work | Count repeated native-column/material calls and cache reuse on fixed 300 m | Cache traffic/memory costs more than recomputation; material drift |
| 2 | Affine bound temporary allocation adds avoidable CPU cost | Same certified bound workload, fixed-capacity scratch versus baseline | Changed bounds, overflow or hidden stack/memory growth |
| 3 | Submission/state overhead limits settled large views | CPU encode plus GPU timings on identical selected owners with bundles retained | Extra storage/indirection worsens integrated GPU bandwidth |
| 4 | Motion repeats demand or prepares detail after its deadline | Per-transaction deadline, cache, cancellation and publication traces | Coarse fallback gives good FPS while reference quality trails |
| 5 | Long-range bound looseness/residency creates disproportionate work | Owner/error/memory counts by distance and silhouette importance | False geometry/feature loss from weakened bounds |
| 6 | Hierarchical brick traversal specializes better for volume-heavy scenes | Small exact meshing versus traversal/skip-cache A/B | New update/streaming machinery exceeds any render saving |

Exact native-painting reuse and affine-allocation probes completed in the first
checkpoint: canonical parity passed, but neither established a whole-view speedup.
The follow-up [cache experiments](reference/issue74/cache-experiments.md) and
[bounded backend comparison](reference/issue74/volume-backend.md) likewise do not
justify default promotion. Keep experiment controls separable; isolated kernels
and single faster runs do not establish an end-to-end benefit.

## Ordered milestones

### M0 — freeze observations and approved targets

Retain the working binary/control, recorded adapter/driver, source identity and
full command. Add actual visual/projection settings and per-frame upload budget
to reports; separate unknown historical values from new measurements. Record
output/internal resolution, MSAA/filter, warmup, worker/batch count, reserve,
CPU/build/GPU budgets and disk-cache initial state. Record git state and binary
hash where available; do not assign today's git commit to an old executable.

Deliver: report aggregator plus meaningful failure/grouping tests; existing-run
summary; unchanged Legacy target and an explicit Regional oracle gap. Finish the
missing offline Regional target before runtime aesthetic optimization. Stop
comparisons whose camera/source/shading or budgets cannot be reconciled; report
them separately instead of filling metadata from assumptions.

### M1 — bounded exact source/summary work

Run native-painting reuse and affine scratch probes separately on the small fixed
fixture. Test canonical materials, mixed faces, negative coordinates and boundary
columns; affine output must retain certified semantics and safe overflow handling.
For reusable pages, keys must include source/appearance/edit revisions and region;
local edits invalidate dependent summaries. Camera epochs change demand, not the
validity of world content. Cached sampled estimates cannot become proofs of
occupancy or silhouette bounds.

Compare baseline/change/baseline in randomized order with at least three usable
warm runs, plus separately labeled cold preparation. Measure first quality and
reserve, worker costs, calls/hits/allocations, CPU/build/GPU memory and frame tails.
Retain a change only if canonical/reference equality passes and improvement
exceeds observed variation, with a provisional target of at least 5% preparation
latency or a justified reduction of scarce memory without worse latency. Otherwise
reject or mark inconclusive. Do not run kilometre sweeps to rescue a small-case
regression. Earlier center-column material-reuse negatives remain counterevidence;
explain what this probe changes before generalizing.

### M2 — submission and visibility

First isolate CPU encoding versus GPU draw cost with current pooled geometry and
render-bundle reuse. If encoding is below 5% of the relevant total cost, prioritize
source/streaming instead of broad submission redesign. Compare compatible-state
batching before GPU-driven culling. Per-cluster dynamic uniforms, vertex/index
slices and profile bindings prevent a mechanical multi-draw substitution.

Negotiate pinned wgpu 24 capabilities and retain direct/bundle fallback. Compare
exactly the same owners/native bins; verify origins, material/profile identity,
coverage and depth. Proposed keep screen: at least 10% whole-frame p95 improvement
or 0.5 ms CPU encode saving without tail/memory/image regression. Stop on absent
required features or savings erased by extra GPU traffic. Add conservative current-
view frustum/Hi-Z only after batching has evidence; camera cuts, near-plane cases,
cave mouths and uncertain occlusion must keep valid coverage.

### M3 — motion scheduling and temporal targets

Generate and approve matched reference trajectories before tuning appearance of
motion. Instrument time-to-need, p95/p99 generation/upload/publication, demand
changes, retained world cache hits, cancellations, stale revisions, deadline misses
and speculative waste. Revisit 3/15/80 m/s, stop/resume, sharp turns, reverse,
flight/bank and warm/cold teleport on bounded scenes first.

Schedule coherent covers and high-error newly visible content without withholding
urgent native/collision work. Cancel obsolete demand but retain valid content;
never publish obsolete ownership. Keep destination readiness separate from
teleport presentation. Win requires fewer reference-quality misses and improved
latency/tails under equal budgets, not simply higher FPS. Stop on holes, false
cave caps, stale revisions or persistent shape changes masked with fades. The
historic movement failures stay open until this evidence passes.

### M4 — controlled distance scaling

After M1–M3 small gates, change only far range: 300 m, 1 km, then bounded 4 km;
20 km is a feasibility investigation after memory/work estimates justify it.
Measure ground and aerial views separately. Attribute source/summary work,
selected/resident/visible owners, fragmentation, pending/build memory and upload
pressure by distance. Include small features and openings; projected cell size
alone does not bound silhouette loss. Reference crops may make slow exact
long-range validation feasible, but declare their finite domains and exclusions.

Stop at budget refusal or unresolved reference quality; report time-to-failure,
peak memory and remaining error instead of settled FPS. No artificial opaque fog,
shortened far plane or silent source reduction in the strict comparison. A
separate perceptual flight preset needs its own approved targets and timings.
This plan does not promise 60 FPS at 20 km.

### M5 — competing representation decision, coordinated with #75

Coordinate shared experiments/evidence with issue #75 rather than creating a
second independent renderer roadmap. Begin with a bounded 128³ canonical region:
current exact meshes, hierarchical empty/uniform/dense brick traversal, then the
same traversal with optional NAADF-style directional skip caches. Keep base
traversal valid while caches rebuild. Exercise insertion/removal, a thin tower,
cave/bridge, grazing view, cold/warm cache and invalidation extent. Include all
build/update/transfer costs and CPU mirrors in memory. Reject any skipped inserted
cell, resurrected removed cell or hidden traversal-limit miss.

Aokana-like regional partitioning/tile grouping and terrain-only reusable grids
are later specializations if profiling justifies them. Do not adopt lossy
occupancy aggregation, learned radiance density, dense global indices or a CUDA
pipeline as canonical game data. Require a material whole-workload benefit on the
actual laptop and exact/reference fidelity before adding a permanent backend.
If update/ownership costs dominate, retain the existing path and document why.

### M6 — transition polish and final combined gate

Once approved stills and reference trajectories exist, address boundary geometry,
color/lighting consistency, subpixel filtering, moiré, dither/TAA and hysteresis.
Compare stationary/grazing/panning/flyover outputs for both spatial error and
history artifacts. Account for overlap draws/history buffers; reject ghosting or
loss of resolvable voxel edges. Fades cannot repair missing ownership or excessive
geometric error. Report strict and perceptual results separately.

The epic remains open until combined cold/revisit latency, motion quality,
capacity, exact edits and 1080p whole-frame p95 ≤16.67 ms are demonstrated on the
documented Radeon 760M laptop profile, with p99/missed frames and honest unmet
ranges. Source preparation, GPU timing, still quality and streaming cannot pass
on each other's behalf.

## Research provenance and unresolved access/license gaps

Read [voxel methods](research/ISSUE74_VOXEL_METHODS.md) and
[GPU/landscape methods](research/ISSUE74_GPU_METHODS.md) for inspected source paths,
limitations and bounded A/B proposals. CuRast's default code license is AGPL v3;
SVRaster limits code/derivatives to research/evaluation. CDLOD root MIT and older
per-file zlib notices need reconciliation before reuse. NAADF MIT does not cover
all dependencies/assets by implication. Aokana implementation license, Teardown
recording/transcript and proprietary Atomontage internals remain evidence gaps;
PlanetSmith's supplied video could not be verified. Public demos and repository
access are not blanket grants to transplant code. No full external code/license
clearance or architecture selection is claimed by these notes.
