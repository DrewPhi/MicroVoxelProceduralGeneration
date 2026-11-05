# Lay of the Land: reference study

Date: 2026-10-31. Research only; no engine changes.

## Evidence and limits

Read developer descriptions directly from public YouTube metadata for the three
videos below; sampled up to 120 public comments per video, including available
uploader replies. No English captions were available for the performance video.
Inspected four frames at 15-second intervals from the 480p LOD demonstration and
a public snowy-biome screenshot on the developer's Ko-fi gallery. This is not a
full audiovisual or source-code audit. Samples and metadata remain in /tmp,
not vendored into the repository. Third-party comments are not implementation
specifications. Exact storage, meshing, LOD blending and simulation scheduling
remain unverified.

## Confirmed developer statements

- GPU compute accelerates voxel-data generation, benefiting dependent systems:
  https://www.youtube.com/watch?v=PYu1iwjAxWM
- Explicit cliff generation, connected roads/POIs and the UE4-to-UE5 move for
  Lumen global illumination:
  https://www.youtube.com/watch?v=y1u9QCRDO5g
- A dedicated smooth LOD transition update claims substantially reduced popping:
  https://www.youtube.com/shorts/WfxuCEyK16I
- Layered landscape simulation and distinct biomes are described on the store:
  https://store.steampowered.com/app/2776090/
- Official updates separately address query/traversal performance, world loading,
  shading quality and handheld settings. The handheld 30 FPS goal is a developer
  target, not our independent benchmark:
  https://steamcommunity.com/app/2776090/allnews/

## Visual interpretation and proposed experiments

The sampled scenes contain broken rock silhouettes, local bank structure, dense
middle-distance detail and familiar objects that establish scale. Lighting and
atmosphere separate overlapping surfaces. These are visual observations; their
implementation is not established by the samples.

1. Continue issue #1 with a controlled bare-terrain valley: explicit cliff faces,
   talus toes, benches, gullies and varied river cross-sections, guided by shared
   slope/geology/drainage fields. Compare against the current valley before
   adding foliage. Preserve exact native occupancy and existing river tests.
2. Issue #4: organize material variation into coherent deposits and rock patches.
   Judge at walking distance as well as overview scale; retain one color per voxel.
3. Isolate LOD appearance: frozen fine/coarse paired captures plus slow walking
   recordings. Measure coverage/silhouette, material drift and transition timing
   separately. Existing fades do not prove that these already agree.
4. After the bare valley passes, prototype a modest lighting/depth baseline and
   a few static voxel scale references. Full weather and dynamic plants are not
   prerequisites. Do not equate adopting Unreal's Lumen with our laptop budget.
5. Prototype GPU generation only after profiling a representative bottleneck.
   Benchmark fixed batches end-to-end, including transfers, CPU collision
   availability, GPU rendering contention and deterministic CPU/GPU agreement.
   Retain the CPU path until there is a measured benefit.

The actionable lesson is coordinated terrain, art and presentation at player
scale. Public evidence does not justify a DAG rewrite, a claimed universal GPU
speedup, or attributing a particular proprietary LOD algorithm to this developer.
