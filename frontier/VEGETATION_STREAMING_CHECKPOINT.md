# Vegetation streaming checkpoint — 2026-11-08

Baseline: `9f0c8ac` on `frontier-voxel-engine`. This pass keeps Alpine terrain,
plant placement, unique full-seed growth, pigment rules, physical scale and the
canonical 6.25 cm grid unchanged. It is greenfield work on Frontier (reuse tier
G), not a port of the historical engines. Raw experiments are under ignored
`output/vegetation-push/`; selected evidence is retained below.

## Diagnosis

Terrain can construct useful coarse content directly. Vegetation was expanding
native models, assembling ground cells and then repeatedly reducing and meshing
those cells for each requested LOD. A single worker serialized that work. Ordered
maps and repeated occupancy/AO lookups added overhead. The renderer uploaded
four 44-byte vertices and six u32 indices per quad (200 bytes).

Tree color selection reserved cheap baselines but could spend almost all the
remaining budget on refinements. A changing neighbor/priority order could then
force an already displayed tree back down. Shadow admission independently chose
color meshes under a 200k triangle cap. Color refinement could therefore suddenly
increase shadow cost and eject other casters, even if their identities survived.
Offscreen selection was another independent representation choice.

## Implementation

- Bounded worker pool (1–4 workers, default available threads minus two capped at
  four), bounded job/result queues, cancellable native generation and epoch-safe
  completions. Old camera content may populate valid caches; old source content
  cannot publish or clear a newer pending request.
- Preserve feasible resident tree allocations before admitting refinements;
  require spare budget for new refinements. Distance hysteresis and readiness
  govern representation changes. Only budget-admitted final requests are queued;
  a resident representation counts as coverage without requiring every historical
  LOD to remain cached. Canonical edits invalidate affected content.
- Independent, bounded shadow representation from the same seeded native tree:
  near stride 2 and farther stride 4, with hysteresis and resident fallback.
  All headings use the same 90 m admission / 96 m retention range, with at
  most 256 nearest candidates. Tree proxies reserve 150k triangles, matching
  the GPU pass's existing 50k ground-cover reservation within its 200k total. Color LOD does
  not dictate shadow geometry. GPU caster admission retains
  canonical identities and uses a deadband rather than unstable batch-index ties.
- Exact faster mesher: contiguous accumulation with small inline material
  palettes, bounded local occupancy or sparse brick lookup, sorted faces and
  contiguous greedy extraction. Original ordering/math, AO and float pigments
  retained. Oracle compares complete vertex bits and index order.
- Canonical rooted-component filtering uses a bounded padded local index volume
  and mark-on-enqueue traversal, with sorted sparse fallback. Output remains in
  the original cell order; RNG, connectivity, pigments and collision index are
  unchanged. Independent old/new generation checks compare 120 seeded plants.
- A process-shared, bounded native ground-assembly cache reuses the same complete
  8 m tile across LOD jobs. Identity includes full plant records, origin and local
  edit cells rather than camera/LOD or a lossy hash. Conservative accounting caps
  retained entries at 64 MiB/256 entries; active worker Arcs can outlive eviction
  and are not included in that retained-cache figure.
- Three GPU representations for a controlled experiment: expanded indexed
  (200 B/quad), compact pulled (44 B/quad, six vertex invocations), compact
  indexed (68 B/quad, four reusable vertices). RGB/AO/transmission remain f32.
  These optimize GPU payload only; the CPU mesh cache still stores expanded
  geometry. No new GPU capability required for the representations.
- Separate optional asynchronous GPU timing spans for plant color and shadow
  passes. Unsupported devices omit these measurements. Bounded per-frame
  observations include LOD/shadow changes and species demand/readiness; worker
  telemetry separates placement, assembly and meshing. Worker values are wall
  times that overlap, not exclusive CPU instruction times.

## Research and architecture decision

- [Binary Greedy Meshing](https://github.com/cgerikj/binary-greedy-meshing/blob/master/src/mesher.h)
  uses occupancy/face masks and compact quad records. We inspected the source as
  algorithmic guidance. Frontier's new mesher is independently written and
  preserves its own palette/AO reduction; this is not a copied bitmask mesher or
  a claim that published timings transfer to foliage. The trusted-coordinate
  integer hasher uses the public-domain [SplitMix64 finalizer](https://prng.di.unimi.it/splitmix.c),
  documented at its use site.
- [AMD procedural grass](https://gpuopen.com/learn/mesh_shaders/mesh_shaders-procedural_grass_rendering/)
  demonstrates reducing distant geometry while compensating coverage. Smooth
  blade width scaling is not automatically compatible with canonical blocks;
  no density reduction or smooth blade replacement is disguised as optimization.
- [Real-Time GPU Tree Generation, HPG 2025](https://github.com/Bloodwyn/gptree)
  demonstrates compact procedural branch/leaf work and separate shadow
  generation optimizations. Its mesh-node/work-graph requirements are not part
  of this portable wgpu implementation. GPU generation is not assumed free on
  an integrated GPU already busy with drawing.
- [Nanite Foliage documentation](https://dev.epicgames.com/documentation/unreal-engine/nanite-foliage)
  describes separate instancing, pixel-scale aggregate voxels and skinning.
  The feature is experimental; it reinforces testing multiple representations,
  not replacing this engine with Nanite or claiming equal capability.
- [Voxel Tools instancing](https://voxel-tools.readthedocs.io/en/latest/instancing/)
  shows vegetation attached to a voxel world with specialized ranges and
  representations. Its documented coarse-placement errors are a reason to keep
  canonical roots independent of displayed terrain.
- Firsthand developer discussions: [localized microvoxel DDA vegetation](https://www.reddit.com/r/VoxelGameDev/comments/1uo7tel/microvoxel_trees_and_grass/),
  [voxel forest BVH instances](https://www.reddit.com/r/VoxelGameDev/comments/1l7r6un/windy_voxel_forest/),
  [Rust mesh grass over voxel terrain](https://www.reddit.com/r/GraphicsProgramming/comments/1v3pa7k/ive_been_working_on_meshbased_grass_and_a_shared/).
  These are useful implementation reports, not matched laptop benchmarks.
  The last uses nonvoxel ribbons; we do not silently adopt that visual trade.

The shared engine contract is integer world identity, edits, priorities, resource
budgets, visibility and lighting. Terrain, ground cover and trees can retain
specialized builders. No need to remesh terrain for an independently changing
plant or replace the successful terrain pipeline.

## Convergence defect found by combined validation

A 120-second static settled run failed despite terrain quality being ready.
Vegetation completed 10,824 tree and 6,627 ground jobs, with 21 requests still
missing. All current color candidates were resident and there were zero selected
mesh evictions. The scheduler was repeatedly constructing unselected variants
that could not remain in the bounded cache: speculative shadow targets and
mandatory LOD8 baselines were requested independently of final admission.
The failure is retained as `static-thrash-reproduction.json/.log` in raw output.
The correction must make budget-admitted demand converge; increasing timeout,
cache size or weakening settled readiness would hide the defect.

## Early experiments (before final source/filter and motion validation)

The exact mesher CPU benchmark alternates old/new execution, nine samples per
case, and compares full output. Native Rowan: 16.95 to 4.99 ms (3.40x), native
assembled grass fixture: 19.40 to 6.09 ms (3.18x). Coarse results improve 1.31–2.86x.
These are face-extraction benchmarks, not total streaming speedups.

Full canonical Rowan generation, 15 alternating samples for seed 123 at three
variants: 9.95 to 4.53 ms, 19.44 to 9.38 ms, 23.04 to 10.82 ms (2.07–2.20x).
The connectivity stage alone improves 42–47x, which is not the end-to-end gain.
Five seeds times eight variants times three species (120 models) independently
match the old model implementation's bounds, ordered voxels, material and RGB bits.

Sequential 720p dense settled backend experiment (same camera/materials/source):

| GPU representation | Selected color triangles | Vegetation color p50, ms | GPU vegetation cache, MiB | Frame p50 / p95 / p99, ms |
| --- | ---: | ---: | ---: | --- |
| Expanded indexed | 979,362 | 4.86 | 163.24 | 15.30 / 18.39 / 19.69 |
| Compact pulled | 979,362 | 2.98 | 36.07 | 12.97 / 15.41 / 16.28 |
| Compact indexed | 981,728 | 3.04 | 52.66 | 13.06 / 15.66 / 17.03 |

Compact pulled remains the default. The first two have matching triangle totals;
this does not by itself establish identical selected individual LODs. The third
has slightly different history-dependent admission. Frozen-content GPU parity
is checked separately rather than claiming asynchronous world captures identical.
These measurements precede the final connectivity optimization.

Terrain-first opaque drawing was also tested. It increased full GPU p50 from
12.38 to 13.88 ms in the dense view while plant color cost stayed near 3 ms.
Keeping opaque plants first avoids terrain shading behind the meadow. The
original order remains default; `FRONTIER_VEGETATION_DRAW_ORDER=after` retains
the rejected diagnostic without modifying selection or canonical data.

Cold 18-second 3 m/s walk/stop/resume worker-count experiment, before the final
connectivity optimization: four workers supplied all current species candidates
around 4 s, two workers around 8 s, one worker still missed ground/flower
candidates at 12 s. Four workers also had worse frame tails while producing more
content (p95 21.32 vs 17.35 ms for one worker); this is a latency/content trade,
not an equal-quality FPS comparison. There were zero selected mesh evictions
in these runs. The earlier 1m color +200k shadow caps bounded active expanded geometry
to approximately 114.44 MiB, below the 160 MiB CPU mesh-cache limit. The final
150k tree-shadow allocation lowers that bound to 109.67 MiB. Worker models,
result queues and retained ground assembly are separate; no total RSS hard bound
is claimed. Final report includes Linux process RSS/high-water measurements.

## Reproduction and evidence

The frozen-content GPU parity test passed on the Radeon 760M: all three backends
produce **identical RGBA8 and Depth32 output**, with 1,080 covered pixels from
native seeded plants, four rotations, negative/rebased origins, lighting and
canonical dither. It avoids asynchronous world selection entirely. Its local
expanded/compact/indexed buffers were 5,104,784 / 1,123,352 / 1,735,880 bytes.
This is a pipeline parity check, not a performance benchmark or broad forest
coverage test. Run explicitly (GPU required):

```sh
cargo test -p frontier-renderer frozen_native_gpu_parity --lib -- --ignored --nocapture
```

Diagnostic controls:

```sh
./run.sh --preset alpine
FRONTIER_VEGETATION_WORKERS=1 ./run.sh --preset alpine
FRONTIER_VEGETATION_MESHER=reference ./run.sh --preset alpine
FRONTIER_VEGETATION_GROUND_CACHE=off ./run.sh --preset alpine
FRONTIER_VEGETATION_GPU=expanded ./run.sh --preset alpine
FRONTIER_VEGETATION_GPU=compact ./run.sh --preset alpine
FRONTIER_VEGETATION_GPU=compact-indexed ./run.sh --preset alpine
python3 tools/record_walk.py --output output/my-vegetation-walk
```

Do not mix recordings/builds with timings. CPU mesher benchmarks isolate face
extraction; they exclude plant growth, placement, uploads and GPU rendering.
Settled FPS alone does not prove cold walking can keep up. Cold runs must report
content readiness alongside frame times. Frame observations include warmup and
startup, while measured benchmark percentiles have their separate interval.


## Final checkpoint evidence

Release launcher built; **486 workspace tests pass, 14 opt-in tests ignored**.
The explicitly executed frozen-content GPU parity test also passes. The fixed
Legacy player-height primary PNG and linear float RGB are byte-identical to
frozen `9f0c8ac`. Canonical plant source remains version 3; no density, shape,
physical size, pigment or terrain-generator changes were used to claim speed.

The combined static convergence test now completes with 209 placement, 161 tree
and 153 ground jobs, **zero pending requests**, and no selected mesh evictions.
Its final 344 presented observations show zero color LOD changes, shadow LOD
changes or caster additions/removals. A separate settled turn/return test has
741 late stationary frames with the same zero event counts and zero pending
vegetation. This is measured convergence, rather than a longer timeout.

Laptop: Ryzen 5 7640U / Radeon 760M, RADV Vulkan Mesa 25.2.8. Dense view eye
`[-43.625,31.0625,-289.5625]`, target `[-43.625,31.3625,-259.5625]`, 60° vertical
FOV, 6 km clip, 4×MSAA, detailed maps, soft-ground filtering. 720p uses pixel
error 3; 1080p uses 4.5 for the same angular bound. Vegetation is process-cold;
terrain uses the explicitly disclosed existing derived disk cache. Runs are
sequential and uncapped; no builds or recording overlap these timings.

| Run | Baseline frame p50 / p95 / p99, ms | Final frame p50 / p95 / p99, ms |
| --- | --- | --- |
| 1280×720 dense, settled | 15.45 / 17.60 / 18.15 | 13.92 / 17.17 / 18.29 |
| 1920×1080 dense, settled | 23.16 / 25.51 / 26.90 | 21.82 / 26.23 / 27.37 |
| 1280×720 grounded walk/stop/resume, 3 m/s | 20.16 / 30.92 / 35.05 | 18.13 / 29.67 / 34.12 |

These are whole frame intervals, not reciprocal GPU-only times. Broader stable
shadow coverage and history-dependent fine admission changed some representation
choices; this is the final experience comparison, not a promise of identical
per-frame geometry or a universal tail improvement. The frozen representation
experiment above isolates the GPU encoding choice more tightly.

Additional final runs:

- Grounded 6 m/s turn/return, 720p: **10.59 / 22.06 / 28.50 ms**. Vegetation
  finishes settled; terrain under the new heading is still refining at exit.
- Grounded 6 m/s walk at 720p output, **960×540 internal rendering**:
  **14.36 / 21.56 / 24.64 ms**. This explicit image-resolution trade is not the
  default, not identical output, and still does not meet the 16.67 ms tail target.
- Process-cold 20 s, 6 m/s walk: final **949,468 plant triangles, 7 pending**;
  frozen baseline **697,198 triangles, 57 pending**. Final median frame interval
  is 9.85 versus 11.03 ms, but content/readiness differs: do not call that an
  equal-quality FPS speedup. New positions still require loading/refinement.
- The 3 m/s measured walking observations have CPU preparation p95 24.39 ms:
  terrain 22.28 ms, vegetation 3.07 ms (individual percentiles do not add).
  At p95 at most one flower tile and one tree candidate are missing across all
  visible ranges; distance is not recorded, so this does not certify zero near
  misses. Dense walking remains primarily constrained by terrain preparation.
- Final vegetation GPU buffers are about **29 MiB settled**, 79 MiB after the
  3 m/s route, 95 MiB after turning and 116 MiB after the longer 6 m/s scaled
  route, within the unchanged 256 MiB GPU cap. Whole-process Linux peak RSS is
  approximately **905–1,434 MiB** across these runs, including terrain, workers,
  graphics-driver mappings and startup; this is not GPU memory or a hard bound.

Actual X11 input recording includes ordinary startup, walking, stop/resume and a
1.28-radian mouse turn. I inspected the contact sheet and largest transient
changes: the largest changes occur during the deliberate turn; no isolated
geometry spike was identified. The late stationary interval has zero LOD/caster
change counters. Initial coarse vegetation and subsequent refinement remain
visible. The recording is 20 FPS, perturbs timing, and cannot certify every
presented frame. A second seed was captured and inspected to supplement the
fixed dense view.

**Decision:** retain the compact pulled mesh path, the faster canonical model
filter/mesher, bounded parallel jobs and convergent request selection. Localized
voxel traversal and GPU generation remain future candidates, not unimplemented
requirements quietly described as finished. The measured low-risk path delivered
substantial vegetation improvements without a terrain or renderer rewrite.

**Stopping point:** ready for a vegetation playtest. Static LOD/caster churn is
resolved in the tested cases and cold loading progresses much faster. This does
**not** close the complete engine's laptop 60 FPS production gate; native 1080p
and dense walking tails still miss it. Further terrain CPU preparation/batching,
GPU terrain/atmosphere work and prediction should be evaluated separately using
these probes. No claim of globally optimal or universally artifact-free rendering.

Retained evidence:

- [Before/after dense scene](reference/gallery/vegetation-streaming/before-after.png).
- [Final scene](reference/gallery/vegetation-streaming/alpine-after.png).
- [Second seed](reference/gallery/vegetation-streaming/seed31.png).
- [Walking/turning contact sheet](reference/gallery/vegetation-streaming/walking-contact.png).
- [Measurements, command manifest, budgets and primary hashes](reference/gallery/vegetation-streaming/measurements.json).
- [Hardware parity output](reference/gallery/vegetation-streaming/gpu-parity.txt).

Full logs, observations and `recording-final/walking.mp4` are local ignored files
under `output/vegetation-push/`. The command manifest records the actual executable
SHA-256, explicit cache path and settings for every final timing/capture run.
