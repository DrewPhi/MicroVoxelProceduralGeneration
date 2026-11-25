# Prototype checkpoints 3–5

The bounded prototype now exercises all five planned areas. Checkpoint 3 adds
real volume ownership and edits; 4 improves normal travel and gates teleports;
5 provides measured competing renderers. **This is not completion of the full
G0–G4 production gates.** Visible LODs were explicitly deferred by the user;
arbitrary volume streaming, pixel-error guarantees and aircraft coverage remain.
Greenfield implementation; no legacy runtime transplanted.

Later transition refinements: [four-pass LOD experiment](LOD_POLISH_CHECKPOINT.md).

## Playtest

```sh
cd frontier
cargo build --release --locked
./run.sh --scene hybrid-topology --camera topology --msaa 4
./run.sh --scene hybrid-topology --camera cave --msaa 4
./run.sh --scene hybrid-topology --camera bridge --msaa 4
./run.sh --scene hybrid --msaa 4
```

WASD / mouse / Space / Ctrl free flight, Shift 15 m/s in hybrid scenes. This is
still a camera, not a colliding player. Click to capture mouse; left removes and
right places a voxel within 8 m in the bounded topology owner or original fixture.
Edits are session-only. **T** prepares a destination 512 m away or returns home;
the camera holds until destination GPU coverage is ready. Esc releases mouse.

`--teleport-test` automatically requests cold, cache-return and already-resident
destinations. `--edit-test` adds a 1 m cube above the bridge, then restores the
original materials after publication. Both write diagnostic records with
`--report`. Headless `--capture` renders the initial state; use timed window runs
and `--capture-on-exit` to capture asynchronous updates.

## 3: canonical topology and revision ownership

A fixed **16 × 16 m** canonical FBM feature region contains a tunnel, entrance,
trench and bridge. Exact geometry stays resident at all distances. Both ordinary
native chunks and every proxy level exclude that footprint, so recentering does
not fill cave air or bridge undersides. Edits stage an immutable world snapshot;
replacement world and render geometry publish together. CPU queries continue
using the old revision while the build runs. Boundary-neighbor faces exposed by
an edit are included rather than inventing crop caps.

Tests compare emitted faces and selected moving/far rays with canonical cell
queries/DDA, including a boundary edit. See [ownership evidence and limits](OWNERSHIP_CHECKPOINT3.md).
This is a bounded smoke gate: outside the native neighborhood, the exact owner
perimeter can disagree with the filtered proxy height. Retaining exact features
forever is not a scalable planet solution. No image-wide zero-gap or full G3
claim is made. The underlying authority remains integer 6.25 cm voxels.

## 4: travel and teleport readiness

Previously every new 8 m camera target could cancel the native/collar/upload
pipeline before it published. An accepted target now finishes while it safely
covers the camera. A far turn or teleport supersedes it. Prediction uses bounded
history of measured request-to-GPU-publication p95 latency, a margin and current
velocity. The selection is constrained by the actual 64 m native cover; this
is not a claim that an arbitrary aircraft speed can fit within that cover.

Two bounded native jobs can run concurrently, nearest chunks first. Content
caches survive demand epochs, stale selections cannot publish, and GPU native
chunk writes remain limited to 8 MiB/frame. CPU and GPU cache targets stay
512 MiB/128 chunks; protected GPU transitions have a 1 GiB admission limit.
Pinned snapshots, staging, proxy pages and driver allocations are additional.
Teleport presentation is held until its complete destination selection has
published; CPU readiness alone is insufficient. Loading status remains visible.

On the same laptop, maximum coverage-center offset at 15 m/s improved from
~30 m to **7.28 m**, with 26 coherent publications and zero cancelled builds.
The 3 m/s route reached 4.91 m max; turn/return reached 8.59 m and returned to
zero. At **80 m/s the native path still fails**: 774.85 m lag, 15 cancelled builds,
only two coherent publications. Fast FPS in that stress case is not successful
streaming. Keep hybrid interactive speed at 3 / 15 m/s; full G4 stays open.

Teleport readiness: first 512 m destination **1375.6 ms / 64 generated chunks**;
return **539.1 ms / 24 chunks**; revisiting **503.5 ms / 29 chunks**; already-active
destination **0.082 ms / zero chunks / zero native upload bytes**. Cache returns
are partially warm, not fully resident—the cache budget evicts some old chunks.

## 5: renderer comparisons and decision

```sh
./run.sh --scene fixture --renderer mesh --msaa 1
./run.sh --scene fixture --renderer brick --msaa 1
./run.sh --scene fixture --renderer brick-skip --msaa 1
./run.sh --scene fbm --camera aerial --surface-layout clipmap --terrain-static
./run.sh --scene fbm --camera aerial --surface-layout quadtree
```

The brick renderer uses packed material/exposed-face cells in 8³ bricks, uniform
brick skipping and exact face depth. `brick-skip` adds a conservative Chebyshev
empty-brick distance cache. This is **not a complete NAADF implementation**.
The fixture uses about 4.20 MB of brick GPU payload; actual process/driver memory
is not that number. Both ray modes use the same CPU canonical source and honor
sampling halos. At an exactly tied edge they choose the first exposed entered
face; that zero-area normal may differ from the CPU DDA's X-first convention.
Solid-eye behavior follows backface-culled raster geometry, not DDA's t=0 solid
hit. Strict image comparisons use MSAA 1: ray MSAA does not run separate rays at
each coverage sample.

The surface alternative is a **bounded adaptive quadtree probe**, inspired by
CDLOD, using the identical canonical height pages as the clipmap. It selects
balanced 2:1 leaves, instances a reusable 16² grid and joins parent endpoints.
Only small descriptors change with the camera, not CPU terrain vertices.
It automatically freezes the initial source pages; hard selection switches,
no certified error bounds, and no production streaming mean it is not full CDLOD.
The aerial case selected 244 patches / 124,928 triangles versus clipmap's
180,224; both use 9,318,960 bytes of height pages. Last quadtree CPU selection
was 0.416 ms. Reduced geometry changes appearance; FPS alone cannot select it.

**Decision:** keep the moving clipmap + native mesh path as the default. Meshes
beat both ray variants in these fixtures. Empty-distance skips help rays, but
do not reverse the result. The quadtree probe is worth further work, but lacks
the continuous morphing and quality evidence needed to replace the clipmap.
Keep both comparison tools rather than treating a small fixture as a universal
winner. No renderer has been promoted on an unmatched quality comparison.

## Measured evidence

Ryzen 5 7640U / Radeon 760M, RADV PHOENIX Vulkan, Mesa 25.2.8. Seed 7, native
0.0625 m. FBM source v1: scale 1800 m, amplitude 1000 m, 8 octaves, gain 0.5.
Output 1920 × 1080, uncapped, first 60 frames excluded, no fog, no TAA, optional
appearance filter off. Volume/surface comparisons use MSAA 1; travel MSAA 4.
Comparison runs are stationary 6 s; travel runs are 15/15/8/10 s, teleport 20 s.

| Scene / renderer | Frame p50 / p95 / p99 ms | GPU render-pass p95 ms | Frames >16.67 ms |
|---|---:|---:|---:|
| valley / mesh | 1.47 / 2.36 / 3.26 | 0.81 | 0/3749 |
| valley / brick | 4.01 / 5.60 / 5.84 | 3.49 | 0/1333 |
| valley / brick-skip | 2.70 / 4.28 / 4.47 | 2.04 | 0/2009 |
| cave / mesh | 1.63 / 2.48 / 4.30 | 1.04 | 0/3382 |
| cave / brick | 6.06 / 8.21 / 8.55 | 5.49 | 0/896 |
| cave / brick-skip | 5.56 / 7.71 / 8.06 | 4.98 | 0/975 |

| Surface candidate | Frame p50 / p95 / p99 ms | GPU render-pass p95 ms | Frames >16.67 ms |
|---|---:|---:|---:|
| clipmap | 1.45 / 2.32 / 5.50 | 0.83 | 0/3759 |
| quadtree | 1.34 / 2.06 / 5.25 | 0.67 | 0/4014 |

| Travel | Frame p50 / p95 / p99 ms | GPU render-pass p95 ms | Frames >16.67 ms |
|---|---:|---:|---:|
| Walk 3 m/s | 6.29 / 8.91 / 10.18 | 6.88 | 7/2246 |
| Sprint 15 m/s | 4.03 / 7.96 / 10.97 | 5.93 | 9/3117 |
| Turn/return 15 m/s | 7.30 / 11.55 / 14.25 | 9.55 | 6/968 |
| 80 m/s stress (coverage FAIL) | 3.19 / 5.57 / 7.86 | 2.81 | 2/2810 |
| Teleport readiness | 9.27 / 12.01 / 14.22 | 8.78 | 8/2022 |

GPU timestamps use asynchronous readback; GPU rendering excludes CPU generation,
queue uploads and presentation. Three readback slots can saturate: these runs
skipped roughly 7–25% of GPU measurements, reported as `gpu_timing_dropped`.
GPU percentiles are sampled and can miss tails; frame percentiles cover all
accepted post-warmup frames. A few last in-flight timing samples are not drained.
Short runs do not prove sustained thermal performance or every seed/speed.

Matched volume PNGs/depths: both ray variants are identical in the three views.
Mesh/ray RGB disagreements above one sRGB byte are 0.133% (overview), 0.0616%
(cave), 0.00357% (bridge). Coverage differs in 83 / 76 / 6 pixels respectively.
P99 normalized depth error on common hits is <=1.02e-6, but larger outliers
remain. This is **not exact GPU pixel parity**; precision/raster edge conventions
need further investigation. The ray alternative therefore remains experimental.
The canonical CPU occupancy/skip/halo tests pass, including an independently
found tied-entry regression. Do not equate those tests with GPU image proof.

Edit verification at 1280×720: the hybrid topology owner and all three fixture
backends published both the 4096-cell cube addition and its restoration. Final
RGB images are pixel-identical to matched unedited captures. The added cube was
also visually inspected. Add/restore input-to-publication times: topology
2.56/2.59 s, fixture mesh 2.23/2.66 s, brick 3.39/3.58 s, brick-skip 3.58/3.48 s.
This verifies staged publication and restoration, **not responsive incremental
editing**: each edit still rebuilds the bounded owner/snapshot. Short 4–8 s runs
ended before restoration completed; use at least 18 s on this hardware (the
reproduction sweep allows 30 s and asserts both publications).

Final workspace validation: **82 tests pass, one expensive benchmark ignored**;
Rust formatting and Python syntax checks pass.

## Reproduce and inspect

```sh
cargo test --workspace --locked
python3 tools/prototype_sweep.py --mode captures
python3 tools/prototype_sweep.py --mode benchmarks
python3 tools/prototype_sweep.py --mode edits
python3 tools/compare_volume_capture.py   output/prototype-3-5/fixture-valley-mesh.png   output/prototype-3-5/fixture-valley-brick.png   --mesh-depth output/prototype-3-5/fixture-valley-mesh.depth.f32   --ray-depth output/prototype-3-5/fixture-valley-brick.depth.f32
```

Single-sample captures include raw row-major little-endian Depth32Float sidecars;
MSAA captures do not resolve depth. All generated PNG/JSON/log/depth artifacts
stay under ignored `output/prototype-3-5/`. `topology-topology.png`, `topology-cave.png`,
`topology-bridge.png`, and the `surface-*` and `fixture-*` comparisons were inspected.
The defaults and prototype status are explained in [the developer guide](../guide_for_devs.md).

Research: [Strugar's CDLOD paper and reference](https://github.com/fstrugar/CDLOD)
(zlib), [NAADF source and algorithm description](https://github.com/cg-tuwien/NAADF)
(MIT). These informed independent probes; no external algorithm source was copied.
The volume path retains the exact ownership contract from
[HANDOFF_PLAN.md](HANDOFF_PLAN.md); appearance transitions remain the explicitly
deferred [checkpoint 2 work](MOVING_NATIVE_CHECKPOINT.md).
