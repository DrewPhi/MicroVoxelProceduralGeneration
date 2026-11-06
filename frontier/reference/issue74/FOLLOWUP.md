# Issue 74 follow-up: measured limits

2026-11-06. **#74 remains open.** This pass reaches a capacity/quality wall in the
current fixed preset, not proof that further optimization is impossible. The approved Legacy reference is preserved. Adjacent vertex-page binding is
enabled by default after its matched timing/parity gate; other experiments stay opt-in.

## Implemented and tested

- Measured-frame CPU draw-encoding p50/p95/p99 paired with the same frame intervals;
  excludes warmup/capture-only draws. Includes queue writes and pass/bundle encoding,
  not exclusive driver CPU time. GPU timestamp samples are explicitly unpaired.
- Independent experiments for exact native/proxy summary reuse, bounded source-cache
  reservation, conservative obsolete-transaction preemption and adjacent vertex-page
  bindings. They retain canonical data, conservative bounds, draw order, coherent
  publication and total budgets.
- Independent offline Regional sun/hemisphere lighting and distance-dependent voxel
  grain. Legacy remains the default. The [near-ground crop](regional-oracle-near/README.md)
  has no finite-domain misses; it is a smoke test, not a full approved artistic target.

## Findings

- [Cache reuse](cache-experiments.md): eight identical RGB captures. Summary reuse
  gives only ~1.4% median preparation improvement with overlapping ranges. Larger
  source reservations reduce regeneration but take capacity from meshes. No
  repeatable >=5% preparation win is established.
- [Motion](motion-experiments.md): complete coverage, poor requested-quality retention,
  no recorded transaction preemptions. This conservative change does not solve the
  walking/turning detail deficit.
- [Distance](distance-limits.md): 1 km misses the 16.67 ms p95 target. At 4 km, requested
  quality is refused after 102.6 s with nearly 16,000 owners and essentially no CPU
  mesh-cache headroom. Stop before 20 km, as the planned budget-refusal rule requires.
- [Backends](volume-backend.md): meshes beat both existing brick modes on the bounded
  fixture; ray/mesh edge disagreements remain. This argues against an automatic
  backend switch, not every possible ray renderer.
- [Vertex-page submission](submission-experiment.md): three runs per condition at
  1 km reduce median whole-frame p95 from 20.210 to 17.161 ms (~15.1%), with identical
  RGB/linear output, unchanged GPU allocation and exact 1×-sample depth/coverage
  parity. This bounded optimization is enabled by default; it still misses 16.67 ms.
  Pipeline-only deduplication was rejected because wgpu already does it.

## Structural blockers and next milestones

1. **Motion preparation:** coherent transactions couple useful and obsolete work.
   Cancelling only wholly invisible parent transactions did not demonstrate useful
   activation. Instrument child dependency/deadline costs, then evaluate persistent
   worker queues and finer transaction reprioritization without tearing ownership.
2. **Resident representation:** 4 km exhausts the mesh-cache partition. Larger source
   caches cannot fix that within the same total budget. Tighter certified bounds and
   cheaper resident products need independent silhouette/coverage and memory tests.
3. **Submission:** 1 km CPU encoding is material. Broader batching must remove distinct
   per-owner state, preserve local origins/profile identities, and measure extra
   integrated-GPU bandwidth. Isolated kernel speedups do not imply faster frames.
4. **Missing acceptance cases:** bank/roll, coherent adaptive teleport presentation,
   arbitrary exterior edits, full Regional target approval and temporal reference
   trajectories remain unfinished. Heightfield checks cannot certify arbitrary 3D.

The next milestone should address a measured structural constraint, then repeat
fixed-reference and motion gates. Fast coarse fallback and a nice still cannot
complete #74. No fog hiding missing detail or weakened source is used to pass.

## Validation and provenance

294 Rust tests pass, nine manual probes ignored; 24 Python tests pass. Tests cover
lighting/Legacy equivalence, exact summary values, conservative cancellation,
instance-address equivalence, native bins, memory accounting and measured samples.
Sequential GPU runs use Radeon 760M/RADV without concurrent compilation; CPU model
recorded locally as Ryzen 5 7640U. Raw data/commands remain in `output/issue74/`;
[followup.json](followup.json) and [followup.md](followup.md) summarize provenance.
Changed binaries/partitions, unknown metadata and failures stay separate.

An initial cache sweep accidentally used the old launcher after only its library
rebuilt. It is excluded; valid runs verify active settings and the rebuilt binary.
No current checkout hash is presented as proof of an old executable's identity.

## Optional controls for reproducing experiments

Run one control at a time before combining them:

- `FRONTIER_NATIVE_SUMMARY_REUSE=1`: share exact aligned 16×16 native/proxy profiles.
- `FRONTIER_SOURCE_CACHE_MIB=64`: reserve 64 MiB of the **existing** CPU cache for
  source data. This reduces mesh space; it does not raise the total. Clamped to
  half the total budget; invalid input retains the baseline reservation.
- `FRONTIER_OBSOLETE_TRANSACTION_PREEMPT=1`: conservative demand-only cancellation;
  no demonstrated activation benefit on the tested routes.
- `FRONTIER_COALESCE_VERTEX_PAGES=0`: disable the now-default adjacent whole-page
  vertex bindings to reproduce the original submission control.

Unset the first three variables and set `FRONTIER_COALESCE_VERTEX_PAGES=0` for
the original control. Only the measured submission optimization is now default.
Do not interpret an enabled experimental switch as a recommendation or a passed
quality/performance gate. Commands and actual reported settings accompany the data.

Final promoted-default smoke: primary decoded RGB remains
`158e5d03a5af1de7011e129a2bfe2452058437b03122868a92cb3a7543f61ce0`,
with coalescing reported enabled. The separate 640×360 adaptive-topology smoke
publishes both insert/restore revisions (4096 and 8192), reports 101 visible
clusters, and completes without GPU validation errors. This is a bounded edit
smoke, not an arbitrary-world edit or performance acceptance gate.
