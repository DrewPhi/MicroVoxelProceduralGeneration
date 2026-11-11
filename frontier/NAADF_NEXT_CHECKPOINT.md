# Traversal and streaming checkpoint after the Teardown talk

Scope: greenfield experiments (reuse tier G), preserving the working hybrid,
canonical 6.25 cm voxels, terrain, vegetation, horizon and lighting. These six
items correspond to the five optimization areas plus the repeatable walking gate.
Do not equate a visibility microbenchmark with game FPS or a resource capacity
estimate with measured GPU occupancy.

## Implemented experiments

1. **Shader specialization.** Optional per-draw far shaders remove voxel shadow
   traversal only when a conservative sunlight sweep of the actual resident
   vertex bounds cannot touch the full NAADF volume. Window moves and lighting
   changes invalidate prepared selections/bundles. Separate optional primary
   specialization fixes leaf/root format and CPU-proof mode from the immutable
   volume; shared mesh receivers retain general format support. MSAA rebuilds and
   wide-palette/sparse/GPU-proof volumes keep their respective formats.
2. **Brick layouts.** Isolated 4³/8³ software brick traversal, full/tight occupied
   bounds and one-bit occupancy versus material-word queries. Uniform bricks omit
   dense payload. Identical canonical meadow, rowan and mountain fixture exports;
   production NAADF shader provides a separate control.
3. **CPU preparation and reuse.** Optional four-lane fingerprints still compare
   all native words before deduplicating. Exact 1 m column tiles cache heights and
   terrain bands, keyed by source parameters, seed/revision and native-cell origin.
   Edited and volumetric columns bypass reuse; vegetation is always assembled from
   current canonical records. FIFO payload capacity is adjustable and allocates
   only on demand. It does not cache LOD geometry or make stale plants authoritative.
4. **Independent lighting experiment.** Beyond an explicit ray distance, mixed
   quarter-metre leaf bricks can conservatively block sunlight without native
   traversal. Primary visibility stays exact. This can over-darken distant shadow
   details and remains an explicitly approximate, default-off experiment.
5. **Optional hardware RT prototype.** Standalone wgpu 30.0.1 workspace tests
   procedural AABBs plus the same native brick DDA, on the actual adapter. The
   production game stays on wgpu 24.0.5. No triangle voxel surfaces, duplicate mesh
   generation, engine-wide dependency migration or claimed production RT integration.
6. **Repeatable acceptance routes.** Meadow, canonical rowan area and mountain
   starts, continuous grounded circuits and stop/resume, seed 7 at 1920×1080/MSAA1.
   Sequential A/B runs reverse order; report frame tails, primary+shadow GPU spans,
   coverage, loading/preparation, process memory and exact-region margins. Record
   real mouse-turn/walking separately from performance runs.

## Controls and reproduction

Experimental flags (not automatically quality-equivalent improvements):

- `FRONTIER_NAADF_SPLIT_SHADERS=1`
- `FRONTIER_NAADF_SPECIALIZE_FORMAT=1`
- `FRONTIER_NAADF_FAST_LEAF_HASH=1`
- `FRONTIER_NAADF_COLUMN_CACHE=1`, with optional
  `FRONTIER_NAADF_COLUMN_CACHE_MIB=320` (16–2048 MiB payload, plus bounded metadata).
- `FRONTIER_NAADF_SHADOW_COARSE_AFTER_M=16` (0 disables approximation).

```sh
cargo build --release --locked --workspace
python 3 tools/optimization_checkpoint.py --output output/optimization-check --run
python 3 tools/naadf_experiments.py --config reference/naadf-next/exact.json --mode capture --capture-size 1920,1080 --output output/exact-images --run
python 3 tools/naadf_experiments.py --config reference/naadf-next/shadows.json --mode capture --allow-color-error --capture-size 1920,1080 --output output/shadow-images --run
```

Capture comparisons retain color differences and require exact depth. Allowing
color error is explicit and capture-only; it cannot turn a geometry mismatch
into a successful lighting experiment. Benchmark configurations clear inherited
NAADF experiment flags. The primary Legacy spiky visual check remains unchanged.
See [the hardware lab](experiments/voxel-rt/README.md) for its separate commands.

## Measurement corrections and limitations

- Initial 40 MiB column-cache trials had **zero hits** in the radius-64 circuit:
  that budget did not cover the revisited working set. Retain that negative result
  and test sufficient capacity with counters; never claim reuse from allocations.
- Very short microbenchmark warmup produced large order effects. The final harness
  sustains each candidate for one second before sampling and repeats reversed order.
- The rowan ray corpus has one FP32/FP64 near-corner first-cell disagreement out of
  57,600 rays, shared by the independent FP32 DDA and the GPU backends. Both oracle
  contracts/results are retained. It is not a new claim of real-arithmetic exactness.
- Explicit brick payload excludes opaque acceleration structures and scratch.
  Microfixtures are clipped 8 m cubes, not large moving NAADF windows. No full-game
  RT gain or large-window memory advantage is established by those measurements.
- Conservative sweep classification and fewer compiled instructions are not a
  speed claim. Additional pipelines and state switching may cost more than saved work.

## References

- [Tuxedo Labs: Raytracing Voxels in Teardown and Beyond](https://www.youtube.com/watch?v=IM1Dr 98f 3xU), especially 31:40 (bricks/AABBs), 38:12 (renderer comparisons), 43:06 (4³/8³), 49:25 (traversal bottleneck).
- [AMD: Occupancy explained](https://gpuopen.com/learn/occupancy-explained/).
- [wgpu procedural AABB geometry](https://docs.rs/wgpu/30.0.1/wgpu/enum.BlasGeometries.html).
- [NAADF author project](https://www.cg.tuwien.ac.at/research/publications/2026/ulschmid-2026-naadf/).
- [WGSL floating-point evaluation](https://www.w 3.org/TR/WGSL/#floating-point-evaluation).

Final measurements and default decisions are recorded below after the gates finish.

## Results and default decision (2026-11-11)

Hardware: Ryzen 5 7640U / Radeon 760M, RADV Mesa 25.2.8. Full-game timings use
1920×1080, MSAA1, native radius 64 m, 6 km clip, Alpine seed 7, full vegetation,
ordinary lighting/atmosphere and development resources. No interior raster meshes
are retained for certified NAADF ownership. Experimental modes remain **off by
default**: the matched results do not establish a broad whole-frame speedup.

### Practical fix found by the new acceptance route

The Rowan-area start `(-57,-47)` previously timed out at 120 s: one template stayed
missing while thousands of tree mesh jobs completed. Two different tree instances
can share a geometry key; the existing source hash, for example, has symmetric
odd-coordinate seed pairs. An owned instance was evicting the CPU/GPU template
still required by an outside instance. That template was built and discarded again.

The fix retains shared geometry whenever a nondelegated tree instance needs it,
collecting those identities before frustum culling. It also retracts an older
cumulative GPU eviction when an outside placement arrives later. Per-instance
NAADF omission stays unchanged; this does **not** restore redundant inside meshes
or change canonical tree generation/seeds.

The same scene now reached readiness in 11,955 ms with zero pending plant jobs.
The short 1 s readiness capture is not used as an FPS claim. An end-to-end selector
regression covers owned/outside instances sharing geometry, including an earlier
cumulative eviction and repeated updates. This fix is active in the normal game.
See `reference/naadf-next/readiness-{failure,fixed}.json`.

### Whole-game walking

Frame p50/p95/p99 in milliseconds; preparation is request-to-ready p50, not summed
worker timings. Entries in the same row use matching configuration and route.

| Route / candidate | Frame p50 / p95 / p99 | Prepare p50 | Peak process MiB |
|---|---|---:|---:|
| Meadow control, two orders |19.22 /35.10 /44.59;20.26 /36.70 /44.95 |327.7;347.1 |2032;2043 |
| Rowan-area control, fixed ownership |19.78 /35.89 /46.97 |300.9 |2060 |
| Rowan-area format specialization |20.22 /36.62 /46.55 |318.0 |2033 |
| Rowan-area cache/hash |20.59 /37.05 /46.75 |296.5 |2138 |
| Mountain control |21.93 /39.81 /50.74 |294.4 |1611 |
| Mountain format specialization |21.66 /40.95 /51.48 |283.4 |1634 |
| Mountain cache/hash |21.76 /36.81 /47.62 |246.6 |1704 |

The repeated meadow cache trials also got 7,616 hits, but did not improve FPS.
The mountain cache case shortened preparation about 16% and reduced frame tails
in its **single run**; repeat it before calling that a reliable win. Its actual
cache payload was 112.8 MiB under the 320 MiB capacity, plus metadata. Source cache
and hash are combined in these runs, so their effects cannot be separated here.
No global default promotion follows from a conditional, single-scene result.

All accepted walking runs reported zero frames outside exact NAADF coverage.
Distant adaptive quality-ready counters are retained: they are not 100% throughout
movement, and this checkpoint does not claim perfect LOD convergence.

Full numeric controls, repeated meadow/stop-resume, corrected cache capacity,
errors and provenance are in `reference/naadf-next/walking-*.json`. The original
40 MiB zero-hit trials and failed vegetation startup are retained as negative
results, not relabeled successes. The post-fix Rowan and mountain results are
single runs, not reversed-order confirmation.

### Why simpler shaders did not become defaults

Untimed compiler statistics show:

| Shader | Instructions before / after | VGPRs before / after | Capacity before / after |
|---|---:|---:|---:|
| Primary known-format specialization |2786 /1710 |40 /40 |24 /24 |
| Far adaptive shadow specialization |4656 /3854 |128 /128 |8 /8 |

There are no spills. The instruction reductions do not reduce these register
allocation buckets, and the moving-scene trials did not establish a full-frame
win. Capacity is a compiler limit, **not** measured occupancy. Additional pipeline
switching and runtime memory/divergence costs remain possible contributors, not
proven causes. Runtime stall/cache counters were not measured.

### Brick / hardware RT gate

The final harness uses compact 16-bit pigments, one-bit occupancy, tight bounds,
and merges uniform vertical interior runs rather than constructing a BVH primitive
for every identical interior brick. Every policy gets one second of sustained
warmup. Reversed-order medians are consistent; the earlier short-warmup apparent
RT advantage is rejected.

Visibility dispatch p50 for 57,600 rays, milliseconds, first/reversed order:

| Clipped 8 m fixture | Production NAADF shader | Hardware RT 4³, tight/masked | Explicit payload NAADF / RT |
|---|---:|---:|---:|
| Meadow |0.106 /0.108 |approximately 0.143 /0.143 |192,000 /589,224 B |
| Rowan |0.148 /0.148 |approximately 0.147 /0.149 |325,248 /621,344 B |
| Mountain |0.113 /0.112 |approximately 0.153 /0.155 |245,504 /638,280 B |

RT improves on simple software-brick traversal, but did not establish a win over
NAADF here. In particular,8³ is not universally best:4³ wins the RT Rowan case.
Opaque AS/scratch memory is excluded; adding it cannot improve RT's memory total.
These are **unlit visibility microbenchmarks**, not matched full-game lighting or
frame-time measurements. The failed small performance/memory gate does not justify
porting the game's lighting or upgrading its renderer dependencies. No menu RT
backend is shipped. Larger windows and a combination of hardware coarse traversal
with NAADF local traversal remain future experiments, not ruled out by this test.
See `reference/naadf-next/brick-*.json` and the standalone lab README.

### Optional lighting gate

The 10 s static control/16 m/32 m shadow runs gave frame p50 of 18.84 /18.40 /19.15 ms
and primary+shadow span 7.24 /7.02 /7.20 ms. Single runs and small differences do not
establish a reliable gain. In this fixed pose the two approximate modes matched
color/depth exactly; that does not establish accuracy in other views. Both remain
off by default. A separate shadow control retained one extra distant tree LOD;
that unmatched comparison was rejected. The earlier strictly matched exact control
was used to compare the shadow candidates, with metadata checks retained in
`shadow-image-comparison.json`. No geometry differences were accepted as shading wins.

### Validation and review artifacts

- 463 renderer tests and 35 runtime tests pass; shader parsing/validation included.
- GPU canonical gates pass with format specialization and CPU cache/hash enabled:
  compact leaves, wide palette, sparse roots, GPU proofs and shifted publication.
- Final brick suite passes the independent FP32 oracle for all 17 policies across
  three fixtures in both orders. The separate FP64 Rowan near-corner disagreement
  remains explicit; this is not a portable exact-real-number proof.
- The four matched 1080p fixed captures have bit-identical linear RGB and depth.
- The primary Legacy spiky reference comparison was regenerated and inspected.
- Separate actual 720p startup and settled-native walking/stop/resume/mouse-turn
  recordings were inspected. The startup cube took 18.7 s to build, so that clip
  primarily covers initial refinement. A second recording waited 30 s, then
  covered 22.3 m and a 1.28-radian turn with four published NAADF shifts, 4,082
  active-native frames and zero out-of-region frames. Encoder overhead and 720p
  FPS are not 1080p measurements.

Review files remain ignored locally:
`output/naadf-next-captures/`, `output/naadf-next-primary/side-by-side.png`,
`output/naadf-next-playtest/{walking.mp 4,contact-sheet.png}`. Curated numeric evidence
is checked in under `reference/naadf-next/`. The normal launcher is rebuilt.
