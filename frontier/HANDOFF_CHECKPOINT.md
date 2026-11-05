# Native / proxy handoff prototype checkpoint

**Status:** partial, diagnostic prototype; no ownership or production gate passed.
This records the next step after the moving FBM surface work. The proposed fixture
contract and primary geometry clipmap / Transvoxel references remain in
[HANDOFF_PLAN.md](HANDOFF_PLAN.md); that plan is still proposed, not completed.

## Implemented

`--scene hybrid` combines a fixed 64 × 64 m interior meshed at the exact 6.25 cm
native voxel size, an approximately 2 m geometric collar, and the static CPU
surface proxy outside it. Native columns come from the canonical seeded FBM
source. This is a bounded diagnostic view, not a production streamer or proof of
a watertight ownership boundary. It has no caves, edits, moving native window,
collision, or certified proxy error. The default `--scene fbm` remains the moving
surface proxy and does not perform native handoff.

Greenfield implementation; no legacy renderer code was transplanted. Four hybrid
tests compare interior mesh hits against canonical voxel DDA, check one top
surface owner through the collar and cutout, check sampled inner/outer boundary
depth continuity on all four sides, and verify shared proxy anchors. Independent
column tests compare exposed faces, materials and normals against dense world
queries. These finite heightfield tests pass; they do not prove arbitrary volume
ownership or an invisible transition.

The native column mesher builds solid spans instead of allocating a full vertical
prism. The native-only mesh contains 3,327,390 triangles and about 279.5 MB of
mesh data. The combined hybrid contains 3,480,271 triangles, including its collar
and proxy. An isolated release build took 0.45–0.75 s and peaked at 317,148 KiB
RSS. This is a one-time diagnostic build, not an interactive update cost or full
application memory measurement.

## View-distance experiment

The default surface experiment supports `--view-levels 7..11`; each setting keeps
the same 0.5 m finest spacing while extending the square proxy half extent. Three
10-second, 1920 × 1080 realtime flights used seed 7, 35 m/s, no TAA or haze, on a
Radeon 760M / RADV Vulkan system. These are uncapped presentation-inclusive frame
times; GPU timing and memory totals were not captured.

| Levels | Half extent | p50 / p95 / p99 frame ms | Initial preparation ms | End camera-center distance |
|---:|---:|---:|---:|---:|
| 7 | 2.048 km | 1.57 / 2.86 / 5.02 | 606 | 0.74 m |
| 9 | 8.192 km | 1.83 / 3.38 / 5.42 | 744 | 0.74 m |
| 11 | 32.768 km | 1.97 / 3.55 / 5.89 | 883 | 1.72 m |

The 4.096 km half-extent at level 8 and 16.384 km at level 10 follow the same
doubling rule. The reports are in `output/handoff-checkpoint/distance-{7,9,11}.json`.
They show the modest cost of adding outer grids for this fixed flight, not a
distance-quality result: spacing/error bound at the horizon was not measured.
The final hybrid ground report is `output/handoff-checkpoint/hybrid-ground.json`:
p50/p95/p99 are 9.984/12.406/12.609 ms, with zero intervals above 16.67 ms.
This is a stationary free-fly view, positioned about 1.7 m above the canonical
surface and aimed downhill; it has no collision. Ground and aerial images expose
obvious proxy/native aliasing and shading differences, so this does not establish
an “invisible LOD” transition. The workspace has 54 passing tests and one ignored
performance test that was explicitly run and passed.

## Next checkpoint

Next, split native work into reusable chunks and integrate moving ownership,
measure transition image error, and extend the ownership tests to caves, bridges
and edits. The current roughly 280 MB native mesh is a correctness/performance
baseline, not a suitable unit to regenerate every camera move. Preserve the
primary-source rationale in the plan: geometry clipmaps
are heightfield methods ([Hoppe et al.](https://hhoppe.com/geomclipmap.pdf));
Transvoxel joins voxel resolutions, but does not directly certify a separately
generated cubic-block/heightfield boundary ([Lengyel](https://transvoxel.org/)).

Launch commands:

```sh
cd frontier
./run.sh --scene fbm --seed 7 --camera aerial --view-levels 11
./run.sh --scene hybrid --seed 7
./run.sh --scene hybrid --seed 7 --camera aerial
```

Reproduce a distance run with `--scene fbm --camera aerial --view-levels 11
--seconds 10 --route fly --route-speed 35 --route-clock realtime --warmup 30
--uncapped --report output/distance.json --width 1920 --height 1080` (create
`output` first). Repeat levels 7 and 9. For the hybrid measurement use
`--scene hybrid --seconds 10 --warmup 30 --uncapped --report output/hybrid.json
--width 1920 --height 1080`. Add `--capture-on-exit output/view.png` to save the
final frame. These short runs do not establish sustained thermal performance.

Roughly **40% of the rendering-prototype engineering** is complete, as a planning
estimate only. This is not a full-game completion percentage, gate status, or
issue-count measure. Moving native ownership, visual-error measurement, cave/edit
ownership, fast-flight coverage, and renderer comparisons remain substantial work.
See [ARCHITECTURE.md](ARCHITECTURE.md) for open production gates and
[HANDOFF_PLAN.md](HANDOFF_PLAN.md) for the detailed proposed ownership tests.
