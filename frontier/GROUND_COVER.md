# Distant grass and flower appearance

This display-only layer extends canonical procedural grass and Edelweiss onto
coarse terrain outside the published NAADF cube. Rowan trees keep their separate
rooted, volumetric distant representation. Native plant cells, placement,
collision and edits remain authoritative.

## Representation

`renderer/src/ground_cover.rs` measures actual plant voxels in world-aligned
1 m probes: visibility-conditioned green-carrier face color/area, eight directional first-hit coverage
samples, top coverage, three height-band coverages, height and color variation.
Rare blossom heads use the canonical birth-probability field and the mean
projected footprint measured from 24 procedurally grown flower models. This
avoids promoting one lucky blossom observation into an 8 m white patch. The
green carrier and unresolved blossom radiance are reconstructed separately.
This is a distribution estimate gated by sampled habitat, not an exact average
of every placed flower.

Opacity and exposed area are separate: overlapping faces cannot accumulate
into opaque vegetation. The small directional estimator wraps its local probe
horizontally; this assumes locally stationary ground cover, rather than tracing
the complete distant vegetation volume.

The 2 m and 4 m summaries restrict their children exactly. Larger nodes estimate
from one representative native probe; they are bounded statistical approximations,
not exact full-region averages. Tiles use at most a 7×7 world-aligned map
(normally 6×6, including the apron), with a minimum 8 m population spacing
independent of render-cell size. Fine color variation is reconstructed
separately, so native mesh refinement does not trigger denser plant surveys.
Maps are bilinearly reconstructed with shared border coordinates at equal scale.
Different-scale boundaries can still differ statistically. Native probes are
reused between camera epochs and retained in a bounded 262,144-entry cache
(40 MiB base summary payload at capacity, plus keys and allocation overhead;
full directional mode can add another 160 MiB of lobe payload).

The original terrain appearance is shaded first. An artistic luminance
calibration retains 85% of its low-frequency brightness, transferring plant hue
and contrast. This preserves the approved meadow palette: separate substrate
and plant probes are not a full joint ground/vegetation radiance prefilter. The new layer then mixes conditional six-face plant lighting using measured view coverage. Face mixtures
come from first visible probe hits. The default uses a compact averaged-face
mixture with eight directional opacity measurements. The optional directional
mode retains the joint color/normal distribution separately for all eight
views. Both are approximations to continuous visibility, rather than exact
full-scene ground/plant integration.
Shaded face moments guide unresolved brightness variance as the sun changes.
World-locked,
footprint-attenuated color/flower variation reconstructs some unresolved texture;
it does not preserve the exact location of each far flower. No distant native
plant mesh or extra NAADF cube is generated. An 8 m handoff outside the active
NAADF box accommodates existing partial native ground patches.

`World::vegetation_ground_records_in` applies the same canonical placement rules
only to candidates in the queried rectangle. Supported tree records use a
separate 4,096-entry indexed cache for ecological shade, so small probes cannot
evict the 128 larger ground pages. This avoids generating a whole grass page
for a small coarse appearance probe; parity tests check the page query.

## Cache and edits

Derived geometry cache identity includes vegetation state, ground-cover version
and shell mode. Its dependency halo includes cover probes and plant roots.
In-memory summary keys include source parameters and a local edit fingerprint.
Deleted/overridden plant cells are omitted from the sampled carrier. Unrelated
edits reuse existing probes. Rare distant blossoms remain a rule-based population
estimate rather than individually edit-preserving flower identities.
This is sampled appearance invalidation, not a replacement for arbitrary edited
volume geometry or tree ownership.

## Controls

Enabled by default with vegetation. For a same-binary comparison:

```
FRONTIER_GROUND_COVER=off ./run.sh --preset alpine --naadf-radius 128
```

`FRONTIER_GROUND_COVER_DIRECTIONAL=on` selects the more detailed joint
view-conditioned lighting field. It is off by default. The default does not
allocate, generate or upload those extra lobe records, and its GPU pipelines
specialize out the extra directional reads. Source and derived-cache identities
separate the two modes. A parity test verifies identical native-probe opacity
with and without the extra records.

`FRONTIER_GROUND_COVER_SHELL=on` enables the experimental thin porous height
layer. It replays existing coarse geometry using a measured height and
world-locked coverage mask; no extra native CPU mesh is constructed. It writes
depth and uses screen-door coverage instead of alpha blending, including float
capture targets. Bounds are expanded, but solid occlusion certificates remain
terrain-only. The shell does not cast new grass shadows. It is off by default
until its static and motion tradeoffs justify promotion.

Capture/benchmark JSON includes `ground_cover`: cache hits/misses, retained
summary payload, aggregate leaf build time and shell state. Leaf time is a sum
across background workers, not wall-clock loading latency or GPU frame time.

## Provenance and limits

Greenfield implementation using multiscale coverage/appearance principles from
Bruneton & Neyret, *Real-time Realistic Rendering and Lighting of Forests*
(2012), https://doi.org/10.1111/j.1467-8659.2012.03016.x, and silhouette/lighting
separation discussed in GPU Gems 3 chapter 4,
https://developer.nvidia.com/gpugems/gpugems3/part-i-geometry/chapter-4-next-generation-speedtree-rendering.
No external source code was copied. These references motivate the design;
their rendering methods and published performance numbers are not implemented
or claimed here. This layer does not certify invisible LOD transitions,
full-volume edits at all distances, or 60 FPS on every scene.

## Acceptance evidence

[Native / previous / default comparison](reference/gallery/ground-cover/comparison.png),
[appearance methods](reference/gallery/ground-cover/methods.png),
[playtest frame](reference/gallery/ground-cover/playtest.png),
[motion frames](reference/gallery/ground-cover/motion.png), and
[full measurements and reproduction commands](reference/gallery/ground-cover/measurements.json).
Raw captures and actual walking/startup videos are under ignored
`output/ground-cover/completed/`; recordings are 720p20 and are not timing evidence.

Same-binary single 15-second walking/turn sample after 30 seconds warmup and
settled initial coverage, Radeon 760M, 1080p, MSAA1, 128 m NAADF, Alpine seed7,
6 km clip, development resources (10 requested terrain workers, up to four active
builds), 32 refinements/transaction and 16 MiB upload budget:

| Configuration | Frame p50 / p95 / p99 ms | Mean isolated scene GPU ms | Peak process RSS GiB |
| --- | --- | --- | --- |
| Layer disabled | 18.94 / 27.88 / 34.76 | 13.56 | 2.09 |
| Compact default | 20.03 / 29.31 / 37.74 | 15.06 | 2.35 |
| Full directional | 20.79 / 30.47 / 46.47 | 15.83 | 2.51 |
| Compact + shell | 20.99 / 29.65 / 47.93 | 16.10 | 2.37 |

Frame intervals include presentation; GPU times are the existing isolated scene
measurement, not whole-frame GPU cost. RSS is whole-process high-water usage,
not complete GPU allocation accounting. Single-run differences are not a
statistical performance guarantee. All measured frames retained terrain coverage.
The layer still slows terrain refinement after turns: default quality-ready
frames were 165/664 versus baseline 327/696. These moving-route quality counts
are diagnostic, not a native-detail radius or visual-error certification.

In the fixed hill-interior crop, native / previous / new mean luminance is
0.1720 / 0.1734 / 0.1722. Within-8-pixel-block luminance deviation increases
from 0.0093 to 0.0162, toward native 0.0368. Pointwise RGB MSE does not improve
there (0.00176 to 0.00180): stochastic phase and geometric detail still differ.
This improves coverage texture while preserving the palette; it does not close
the native-reference gap. The porous shell made little difference in the
mountain view and requires another coarse geometry replay, so it remains off.
Its submitted triangle work exceeds the base-terrain triangle counter.

532 tests pass (470 renderer, 62 world; 31 existing ignored), including shader
validation, directional/compact probe-opacity parity, local edit dependence,
source identity and strict profile-map decoding. The primary spiky comparison
passes. Both actual startup and settled recordings verify more than 22 m of
walking and 1.28 radians of heading change. Inspected timestamped frames show
continuous meadow coverage, while startup visibly refines. This is no claim
that every transition or every frame-time spike is eliminated.
