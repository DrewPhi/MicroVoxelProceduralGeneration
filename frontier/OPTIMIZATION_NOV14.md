# November 14 optimization checkpoint

This checkpoint implements exact CPU preparation/cache improvements and exposes
optional rendering experiments. **Experimental GPU switches remain off unless separately qualified.**
Whole-game timings and promotion decisions are recorded below; no GitHub issue
is closed by this checkpoint alone.
The work targets the existing `frontier-voxel-engine` NAADF/raster hybrid and its
canonical 6.25 cm voxels. Several-mile visibility remains an architecture goal,
not a result demonstrated by these finite-window changes.

## Implemented issue work

| Issue | Implementation and correctness boundary |
| --- | --- |
| #76: local NAADF edits | `naadf/data.rs` indexes edits by exact 16³ chunk and extraction-relative 1 m column. Only affected chunks lose air/full shortcuts; only affected columns bypass cached summaries. Explicit edits still override terrain and plants. Nonaligned negative-coordinate extraction rejects incompatible world-aligned summaries; topology-straddling columns retain exact queries. |
| #77: local vegetation edits | `vegetation.rs` uses 8 m edit buckets and local override-cell mesh fingerprints. Source-independent placement tiles, grouped placements, valid jobs and unaffected LOD history survive edits. Epoch/key cancellation tokens reject changed pending meshes; unrelated older completions remain reusable. Only overlapping tree-shadow residents invalidate. `World` distinguishes immutable source lineages and computes exact edit-map differences, including reverts. Seed/source/vegetation replacement retains broad invalidation; independent reconstructed worlds conservatively reset. |
| #78: geometry before cover | Opt-in `deferred_cover.rs` lets certified geometry publish with explicit appearance debt. One cosmetic worker, one queued job, at most two outstanding jobs and a 16 MiB product cap limit admitted work. Complete profile/reference generations promote together; current owner, source revision and base profiles are checked. `quality_ready` requires both geometric quality and zero appearance debt. GPU-capacity refusals now park the key and release the completion slot; retries follow residency/budget changes. |
| #79: cover-cache coalescing | `ground_cover_cache.rs` provides 16 byte-budgeted shards, one registered producer per key, cancellable waiters and abandonment/panic wakeup. Computation owns no shard lock; recursive dependencies decrease node level. Each shard permits 64 registered flights, then bypasses registration. Retained and evicted-but-live estimates, coalescing, eviction and lock-wait counters are exposed. Source/local-edit keys preserve immutable cache identity. |
| #81: NAADF build work | Fixed column/height/band scratch replaces temporary column vectors; immutable column-cache hits borrow arrays. Root-proof coordinate spans hoist repeated integer division while preserving Jacobi words. The compact-overlay experiment narrows staging to u64 when representable, promoting exactly to u128 for wide addresses/pigments, including overlap-resolution promotion. Canonical address/pigment sort order and overlap precedence remain intact. Chunk-local replacement of the global overlay sort is **not implemented**. |
| #82: per-pose preparation | `adaptive_gpu.rs`/`adaptive_view.rs` separate shadow, resident accounting, ordering, occluder, visibility/uniform, upload and bundle CPU samples. Optional incremental ordering reuses only the previous key sequence under identical owners/residency; every current-camera depth is recomputed and the same total sort still runs. No previous-pose visibility result becomes authoritative. |

Local vegetation placement intentionally ignores overrides in the canonical
source. Keeping its placement tiles after a material edit therefore preserves
source semantics; meshes mask the affected overridden cells. A material-only
change to an already overridden plant cell need not change that mesh identity.

## Optional candidates

Set flags before process startup; several options are cached once. Unset means
the corresponding new experiment is off.

| Flag | Mechanism / limit |
| --- | --- |
| `FRONTIER_NAADF_CPU_ROOT_REFRESH=cube` | Exact hierarchical occupancy selects a larger whole empty-cube proof for incoming shell roots. Cuboids are selected, never unsafely unioned. O(log n) ancestor lookup can cross shell/worker boundaries. |
| `FRONTIER_NAADF_CPU_ROOT_REFRESH=box` | More expensive monotone cuboid growth checks complete proposed empty boxes against the same occupancy hierarchy. |
| `FRONTIER_NAADF_INTEGER_FASTPATH=1` | Fixed-lane proof decoding and face bit scans; event-time, hit-position and shading float arithmetic remain unchanged. |
| `FRONTIER_NAADF_SPECIALIZE_FORMAT=1` | Specializes immutable root/leaf formats for primary traversal; verify every supported storage format. |
| `FRONTIER_NAADF_CACHE_NODES=3` | Retains the last mixed root using little extra ray state. Saved loads may still lose to register pressure. Earlier cache modes remain experiments. |
| `FRONTIER_NAADF_COMPACT_OVERLAY=1` | Smaller exact vegetation staging where addresses/pigments fit; automatic wide fallback. |
| `FRONTIER_NAADF_COLUMN_CACHE=1` | Reuses exact terrain-only samples outside edited/volume-owned columns. `FRONTIER_NAADF_COLUMN_CACHE_MIB` controls capacity; default 320 MiB when enabled. |
| `FRONTIER_DEFERRED_GROUND_COVER=1` | Geometry-first appearance pipeline. Continuous mandatory terrain work can starve appearance; debt stays visible in readiness/telemetry. Full geometry duplication during GPU promotion also needs temporary headroom. |
| `FRONTIER_TERRAIN_INCREMENTAL_ORDER=1` | Reuses an ordering starting point, with current depths and full exact sorting. |
| `FRONTIER_GROUND_COVER_CACHE_MIB` | Shared cover-cache budget, default 128 MiB. Scratch and consumer-owned summary copies are separate from retained-cache accounting. |

`FRONTIER_NAADF_SHADOW_COARSE_AFTER_M` is a separate **approximate lighting**
experiment: distant mixed-root shadow queries may stop early. It must not be
reported as an exact, quality-equivalent renderer win. Existing height/beam,
GPU-distance/direct-publication and local-exit experiments remain documented in
[NAADF_OPTIMIZATION_FOLLOWUP.md](NAADF_OPTIMIZATION_FOLLOWUP.md); their earlier
negative results are not superseded by these candidates.

Cross-shell occupancy is transaction-local until successful publication.
Unknown cells are occupied, occupied pointer/material words remain unchanged,
and cancelled preparation cannot publish its occupancy. At 512³ reserved root
storage, one hierarchy costs approximately 18.3 MiB; cloning for preparation
temporarily retains approximately **36.6 MiB across both copies**, excluding
root payloads and other build scratch. No extra per-ray GPU lookup or provenance
replay is introduced. The existing bounded 8 m startup radius and 4 m shell
growth remain; retained interiors are not rebuilt globally.

## Correctness evidence and remaining gates

Latest completed renderer suite: **509 passed, 33 ignored** (hardware and
manual diagnostics are run explicitly). The complete Rust workspace passed **642 tests**; five Python experiment-harness
tests also pass. Canonical GPU checks passed the integer-decode/mixed-root-cache variants
across compact, wide and reserved storage, and the cube/box proof modes. These
are correctness gates, not performance results or complete motion acceptance.

The frozen synthetic root-word fixture independently checks every resulting
air cuboid against a dense occupancy oracle, plus occupied-word identity:

| Synthetic final radius | Roots processed across growth | Occupancy updates | Cube refresh | Box refresh |
| --- | ---: | ---: | ---: | ---: |
| 32 m | 262,144 | about 3 ms | about 2 ms | about 0.8 s |
| 64 m | 2,097,152 | about 23 ms | about 20 ms | about 13 s |

These are CPU totals across synthetic growth shells, **not frame times**, FBM
generation timings or FPS. Larger certified boxes do not establish fewer GPU
cycles. The warm cached-column lookup microbenchmark was roughly 1.7× faster;
that ratio likewise does not describe whole preparation or game performance.

| Gate | Status |
| --- | --- |
| 64 m matched old/new walking | measured below; no demonstrated whole-game gain |
| 128/256 m fully loaded coverage and timing | corrected locked gate; results below |
| Fixed-view exact images | canonical GPU rays pass; full-scene partition mismatch is rejected |
| Walking/turning/edits | walking measured; local edit/undo/shadow tests pass; full edit-heavy FPS gate remains |
| Deferred cover latency | opt-in; live validation below; no default promotion |

Initial timings collected while Lean loaded the CPU are provisional. The old
capture harness used Constrained resources; its 160 MiB tree budget refused
required content. The harness now explicitly selects **Development** resources.
Do not compare a refused/partial old capture against complete new content, or
mix these resource profiles into an apparent speedup. Configurations live under
[reference/optimization-nov14/](reference/optimization-nov14/).

## Reproduction and promotion gates

Run from `frontier/` with the pinned Rust toolchain. Useful CPU filters:

```sh
cargo test -p frontier-renderer naadf::data::cpu_tests
cargo test -p frontier-renderer vegetation::edit_tests
cargo test -p frontier-world local_edit_diff_and_source_lineage
cargo test -p frontier-renderer local_edit_invalidates_only_overlapping_shadow_residents
cargo test -p frontier-renderer ground_cover::shared_cache::tests
cargo test -p frontier-renderer adaptive_geometry::deferred_cover::tests
cargo test -p frontier-renderer appearance_capacity_refusal_releases_completion
cargo test -p frontier-renderer incremental_order
cargo test -p frontier-renderer root_distance::occupancy
```

The ignored `frozen_shell_refresh_microbenchmark` is an optimized CPU diagnostic;
`FRONTIER_NAADF_ROOT_REFRESH_DUMP` optionally writes exact before/cube/box words.
GPU filters include `naadf_exact_fastpaths_match_canonical_rays` and the existing
canonical wide/reserved/traversal matrix; hardware tests require explicit
`-- --ignored`. Run expensive hardware gates
serially, separately from diagnostic counters and CPU profiling.

Before promotion, run the fixed player-height
[PRIMARY_VISUAL_CHECK.md](PRIMARY_VISUAL_CHECK.md) first, with its unchanged
source/camera and exact reference. Require matched color/depth/material and
canonical rays, including negative coordinates, edits, wraps, corners and
grazing rays. Then test cave/bridge/plant overlap, repeated place/break and
reverts, source changes, moving ownership, stop/resume, fast turns and teleport.
Compare walking imagery as well as settled stills. Keep 1920×1080, MSAA1,
vegetation, shadows, requested radius and resource profile matched; disclose
every lighting or quality difference. Record adapter/driver, binary fingerprint,
source/seed, startup, GPU/CPU/frame tails, RAM/VRAM including transient payloads,
pending work, geometry/appearance readiness and exact coverage. No fog, omitted
plants, smaller native coverage or relaxed quality gate may manufacture a win.

## Provenance and research

Reuse tier **G**: new implementations in the current Frontier workspace. Frozen
pre-change current-branch CPU/Jacobi builders, canonical world queries and exact
ray/image references are comparison oracles; no old runtime or scheduler was
transplanted. Internal reference algorithms retained in tests are not new
production architectures.

The [NAADF paper](https://doi.org/10.1111/cgf.70413) and
[authors' implementation](https://github.com/cg-tuwien/NAADF) motivate disposable
empty cuboids inside an authoritative nested voxel hierarchy. Their published
performance is not a measurement of this renderer or streaming world.
[AMD's occupancy explanation](https://gpuopen.com/learn/occupancy-explained/)
explains why saved loads or instructions can be offset by register allocation
and reduced latency hiding. Compiler occupancy estimates alone do not establish
actual cache misses, stalls or end-to-end speed.

## Rejected final-face-mask experiment

A standalone one-byte-per-root finalized-face mask preserved the frozen Jacobi
output on 72 rectangular/layout/pattern cases. It skipped saturated or
permanently blocked faces while keeping both ping-pong buffers synchronized.
Six reversed-order warm 64³ fixture pairs showed roughly 12–17% regressions for
air/flat/rough terrain, despite an approximately 9% improvement for dense random
occupancy. The extra mask traffic, branches and synchronization copies outweighed
the skipped work on the terrain cases. No production mask was added. This is a
CPU microbenchmark rejection, not a whole-game timing comparison.

## Full-scene correctness captures

The fixed spiky player-height primary reference comparison was regenerated and
visually inspected. The existing far-field smoothing remains; this optimization
pass does not claim to eliminate that quality gap.

At 1920×1080, Alpine seed 7, radius 64 m, Development resources, atmosphere off,
all seven experimental captures completed with native 6.25 cm data. The original
bitwise comparison found exact color/depth parity for integer decode alone,
mixed-root cache alone, cross-shell cubes and compact overlays. Combined integer
plus root-cache and incremental ordering differed at only three adjacent raster
background pixels outside NAADF: nine RGB components (maximum 8.48e-6) and one
depth component (5.96e-8). Those runs had a different adaptive partition (2,396
visible clusters versus 2,384). Even the integer-only run differed by eight
submitted background triangles despite identical pixels. These are **unmatched
partition captures**, not proof that shader arithmetic changed, and not accepted
exact full-scene gates. The harness now rejects mismatched partition counters
explicitly while retaining the image-difference metrics. Canonical GPU-ray
tests separately establish the tested traversal variants' exact voxel hits.

## Matched 64 m walking measurements

Hardware: AMD Ryzen 5 7640U CPU, Radeon 760M integrated GPU, RADV PHOENIX /
Mesa 25.2.8, Vulkan.
All rows use Alpine terrain recipe, seed 7, native 6.25 cm voxels, 1920×1080,
MSAA1, pixel error 5, native mesh radius 4 m, vegetation near 32 m, full lighting
and atmosphere, Development resources, no persistent disk cache, 30 warmup frames,
and a 15 s `walk-turn-return` route at 2.5 m/s. Timings are unrecorded runs.
The CPU Lean workload had stopped; normal desktop activity remains.

Frozen old build: `5ddcb90`; new build includes this checkpoint. Runs were
sequential **old → new → new → old**, with fresh processes and identical options.
Whole-process RSS is separate from explicit GPU allocation accounting.

| Run | Frame p50 / p95 / p99 (ms) | NAADF GPU p50 (ms) | Full 64 m publication (s) | Peak RSS (GiB) |
| --- | --- | ---: | ---: | ---: |
| old 1 | 24.54 / 33.62 / 51.64 | 15.16 | 8.87 | 1.831 |
| new 1 | 24.89 / 36.74 / 55.68 | 15.58 | 9.11 | 1.836 |
| new 2 | 24.80 / 35.49 / 55.01 | 15.66 | 9.11 | 1.825 |
| old 2 | 24.92 / 35.52 / 52.41 | 15.65 | 9.33 | 1.854 |

There is **no demonstrated whole-game FPS or startup win** in these runs.
The mean of each pair's median frame times is 24.73 → 24.84 ms (about 40 FPS);
p95/p99 tails were slightly worse in the new pair. More repetitions would be
needed to separate that small regression from run variation.
Reported cumulative NAADF stream-build CPU time averaged 384 → 355 ms (about
8% lower), and ground-cover leaf CPU work 39.09 → 36.81 s (about 6% lower). These
are work-counter comparisons across two runs, include preparation/startup, and
are **not frame latency or a guaranteed speedup**. Source demand varied slightly
with the real-time route. The main measured native GPU span remains around
15–16 ms, so cheaper CPU work alone does not produce a large frame-rate gain.

### Optional switch screening

The seven-way 64 m matrix used the same 1080p route, two reversed-order passes.
Its final wrapper was interrupted before the last control finished; that control
was rerun separately. Completed per-run median frame times:

| Candidate | Pass 1 / pass 2 (ms) | Decision |
| --- | --- | --- |
| control | 23.16 / 23.95 (separate final run) | retain |
| integer decode | 24.66 / 25.90 | off |
| mixed-root cache | 25.62 / 25.57 | off |
| integer + mixed-root cache | 25.63 / 25.64 | off |
| cross-shell cube proofs | 26.47 / 25.82 | off |
| compact overlay staging | 25.25 / 25.22 | off; exact staging-memory experiment |
| incremental ordering | 24.97 / 25.25 | off |

None earns default promotion. Each candidate's frame time includes full lighting,
vegetation and distant meshes. The rejected CPU box-refresh and finalized-face
mask experiments likewise are not enabled. These negative results do not imply
that all possible traversal optimizations have been exhausted.

### Locked benchmark startup gate correction

The first 256 m validation was rejected: it started the route during expansion
and finished with only 128 m resident. Its frame numbers are not a 256 m result.
The ordinary `ready()` predicate intentionally means that the initial inner
region is usable. Locked benchmarks now use the existing full requested-region
capture predicate instead, so timing cannot begin at a smaller radius. Normal
interactive progressive startup is unchanged. All 64 m old/new comparison logs
confirm full 64 m publication before their route-start event. `--warmup 30`
means **frames**, not seconds; startup preparation and route warmup are separate.

## Fully loaded large-radius result

With the corrected benchmark gate and the same full-lit 1080p Alpine walk:

| Radius | Full startup | Frame p50 / p95 / p99 | NAADF GPU p50 | Peak process RSS | Logical NAADF GPU payload |
| --- | ---: | --- | ---: | ---: | ---: |
| 128 m | 33.69 s | 29.33 / 40.83 / 54.65 ms | 20.31 ms | 1.68 GiB | 0.34 GiB |
| 256 m | 143.29 s | 49.06 / 52.36 / 54.06 ms | 35.62 ms | 1.90 GiB | 1.58 GiB |

The 256 m region was fully resident before route start, no frame was outside
NAADF, and expansion was complete. This is around **20 FPS**, below the 30 FPS
playtest target. The limiting measured span is now native rendering rather than
a memory-budget refusal. Process RSS and logical GPU payload are different
accounting views on shared system RAM; neither alone nor their sum is an exact
whole-game physical memory total. Driver residency and transient GPU allocations
need separate accounting. This is one short route, not a universal worst case.

Coverage was coherent for all 282 measured frames, but distant adaptive quality
was ready for only 15; error diagnostics still flagged unresolved far geometry.
Full native-radius coverage does **not** certify distant quality or invisible LOD
transitions. Do not claim the 256 m result passes all architecture gates.

The fully loaded 128 m run reached approximately **34 FPS** by median frame
interval; its tails still exceed the target. This is a more practical playtest
setting on this laptop than 256 m, not a guarantee for every scene.

## Deferred-cover live validation

The opt-in 640×360, 32 m static GPU capture drained its appearance debt to zero:
880 atomic promotions, five cancelled jobs, one stale completion rejected, no
capacity or payload refusals. Geometry coverage appeared at 12.8 ms; complete
appearance arrived at 10.49 s. Cosmetic worker compute totaled 6.66 s and uploads
72.24 MB. This proves the live pipeline runs, not that it improves matched-frame
quality or walking latency. Its final distant partition differed from control,
so the exact image gate rejected the pair (6,069 RGB components and 949 depth
values differed). It remains off; continuous-motion appearance starvation and
duplicate geometry during promotion remain reasons to withhold default use.

Curated per-run provenance and counters are in
[measurements.json](reference/optimization-nov14/measurements.json), with
[capture gates](reference/optimization-nov14/capture-gates.json). Raw captures,
logs and video remain ignored under `output/optimization-nov14/`. Failed and
unmatched experiments are retained rather than relabeled as wins. The historical
Lean-loaded baseline and the initial partial 256 m run are excluded from the
accepted measurements.

## Motion inspection and default decision

A separate 1280×720 X11 recording used the final build, a 128 m radius and a
60 s settle. Native expansion completed at 39.44 s, before recording began.
The capture verified 22.23 m of grounded movement and a 1.28-radian heading
change, with walking, pauses, resume and a turn. Sampled frames retain nearby
grass, flowers and trees without large missing-region holes. This is a sampled
visual inspection, not proof of glitch-free temporal behavior; conservative
distant quality counters still show refinement during movement. Video timings
are not used as benchmark results. The report records zero outside-region frames.
The video is `output/optimization-nov14/final-walk128/walking.mp4`.

Default decisions: retain local edit/cache/scratch improvements and detailed
diagnostics; leave all newly introduced rendering/appearance flags off. Keep
the existing default 64 m radius. Do not close #74/#75 or the issue-specific full
acceptance gates based on this checkpoint. Primary native GPU visibility/shadow
cost is the next measured target; increasing RAM alone will not turn the loaded
256 m case into 30–60 FPS. Sparse root/storage work would be required before
claiming support beyond the current 256 m implementation limit.
