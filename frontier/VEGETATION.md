# Procedural voxel vegetation

Run from `frontier/`:

```sh
./run.sh --preset alpine
```

The Alpine preset enables procedural vegetation, walking and the
[beauty treatment](reference/BEAUTY_CHECKPOINT.md). Later flags override
the preset: `--vegetation off` disables plants and `--walk off` starts in flight.
Vegetation currently requires `--scene adaptive`. Existing comparison scenes
retain their previous defaults. The current combined art checkpoint is documented
in [Cozy Meadow](COZY_SCENE.md); older captures below retain their original settings.

Use WASD to move, Shift for faster movement, and Space to jump while walking.
F toggles walking/flight. Esc opens settings, including vegetation and walking
toggles. In flight, Space moves upward. Walking follows canonical terrain and
collides with rowan wood; soft leaves and flowers do not block the player.

## Canonical source

Plants are generated throughout requested terrain regions from the world seed.
There are no authored root coordinates, fixed demonstration meadow bounds, or
saved placement lists. Globally addressed candidate cells, smooth meadow/flower
colony fields, deterministic spacing, terrain material, and slope tests determine
placement. Paths, unsuitable terrain, and explicit volume ownership are excluded.
Negative coordinates and neighboring tiles use the same global rules.

`world/src/vegetation.rs` contains the Rust generators and placement queries:

- Grass uses connected voxel strokes, varied blade heights, and curved lean.
- Stylized edelweiss uses low foliage, stems, pale star-shaped flower heads,
  and contrasting centers.
- Rowan uses bounded space-colonization branching, connected curved wood,
  a thick flared trunk, connected rounded leaf sprays, and scattered pale blossom
  clusters. Terminal sprays remain; intermediate sprays compete for space. Moderate
  young-leaf pigment follows branch age and position within a spray.

Plant source version **3** grows every plant from its record's full 64-bit seed.
Eight grass cohorts, three flower cohorts and eight rowan maturity cohorts supply
bounded growth parameters; they are not a bank of 19 fixed shapes. Meadow fields
favor low spreading grass with sparse taller tufts. Broad habitat fields influence
rowan maturity, while each seed controls its branching and trunk character.
A plant record fixes its root cell, seed, cohort and quarter turn. Palettes derive
from the approved visual references and are converted from sRGB to linear RGB.
Model generation retains the rooted face-connected component.

`model_for` returns an `Arc<PlantModel>` from a content cache keyed by species,
cohort and full seed. Growth runs outside the cache lock. Woody models have a
64 MiB estimated cache budget; grass and flowers share a separate 8 MiB budget.
Each cache also holds at most 512 entries. The estimates include voxel storage
and conservative index overhead; active references and in-progress generation
are additional bounded working storage. Eviction discards reusable output and
does not change canonical growth. The former fixed-bank `prewarm_models` hook
is a compatibility no-op; models are generated on demand, with cancellable
rowan growth on the streaming worker. Source version is reported in metadata.

Every completed rowan growth also populates a separate compact wood-occupancy
cache, bounded to 16 MiB and 256 entries. Sorted local `i16` triples retain exact
wood cells without foliage, pigment or the full voxel index. Walking queries
check this cache after authoritative edit overrides and terrain, applying the
record's exact inverse quarter turn. Eviction of a large display/source model
therefore does not require rebuilding foliage for a retained wood entry. A first
cold query with no retained wood still generates the canonical plant once.

The checkpoint retains Alpine terrain source **4**, independently of vegetation
source **3**. The canonical lattice remains **0.0625 m (6.25 cm)**. `World::material` composes
terrain, vegetation, and authoritative edit overrides. Collision queries skip
soft `Leaf` and `Flower` materials and retain solid wood. Terrain column queries
remain terrain-only, so plants do not contaminate terrain height-page generation.
Display settings and coarse template stride do not redefine native cell identity.

## Streaming and display

`renderer/src/vegetation.rs` maintains camera-demand tiles on a bounded worker.
Placement requests are species-specific: distant tree tiles never generate the
entire ground-cover meadow. World placement tiles are 16 m; display demand tiles
are 32 m. Camera epochs cancel obsolete demand, and revision changes replace the
immutable world snapshot before publishing vegetation for the new revision.

Native model voxels become exposed faces, then compatible coplanar faces merge
greedily. Each tree mesh key includes its full seed, cohort, stride and edit
identity. Roots and quarter turns transform the generated local geometry.
Integer roots are rebased before conversion to floating point.

Grass and flowers use globally anchored **8 m geometry tiles**, separately by
species. The worker queries canonical roots in an expanded neighbor range, then
clips native cells to the tile's half-open x/z bounds. Every ground-cover cell
has one tile owner, including growth extending across an edge. Edited native
cells are omitted before meshing. The key includes the global tile coordinates,
source-derived seed, stride and edit revision. This batches many unique seeded
plants into one mesh instead of uploading one template per grass root.
A tile's complete geometry is independent of camera position, frustum and the
placement neighbors currently resident. Camera demand selects and fades tiles;
it never changes the mesh contents associated with a key.

Stride 1 retains the native lattice. Automatically generated strides 2, 4, and 8
aggregate occupied native-grid cells, majority material, and palette coverage.
At foliage strides 4/8, separate wood meshes use stride 2/4 and foliage stops
at its native canopy base. This preserves curved trunks behind foreground
ridges; native strides 1/2 and canonical colors stay unchanged.
Projected voxel size selects the quality target; the preferred tree target is capped
at 4. Ground-cover coarse meshes cap vertical aggregation at two native cells,
so horizontal resource fallback does not inflate short grass into 0.5 m cubes.
Every visible tree first reserves an actual stride-8 coverage template; nearby
refinements may use the remaining tree budget. Resource-driven stride-8 fallback
is explicit and counted, so near trees do not erase the distant forest. A resident template supplies temporary coverage while a new
stride is prepared. Ground cover also reserves actual stride-8 tile coverage,
then admits refinements through strides 4, 2 and 1 within the remaining triangle
budget. The preferred ground target retains native detail inside 8 m, stride 2
inside 16 m, and stride 4 farther away, subject to resource fallback. Work that
would exceed the reserved coverage budget is not queued; rejected geometry does
not refresh cache residency. Worker results trim vector capacity to content size
before admission. Demanded coverage meshes remain warm, and up to 2,048 compact
mesh-cost records survive geometry eviction so an oversized refinement is not
repeatedly regenerated. These aggregates are display approximations, not new worlds.

Default ranges are 28 m for grass, 56 m for edelweiss, and 220 m for rowans.
Ground-cover tiles retain all canonical roots and fade together near the outer
range using canonical-cell dithering. Tile selection includes the tile extent
around its center. There is no per-root distance thinning. Trees also fade near
their outer range and are not randomly density-thinned.

Default bounds are 384 placement-demand tiles, 512 CPU meshes with a 160 MiB
vertex/index payload cap, 12,000 submitted instances,
and 1,000,000 submitted triangles per frame. Trees are considered first but are
capped at 65% of that triangle budget. Flowers receive up to 10%, followed by
grass using the remainder, including unused tree/flower room. Camera-frustum
rejection happens before geometry budgets, while placement tiles stay warm for
turning around. A nearby ground-cover request receives a worker slot on
alternating frames while distant tree coverage warms.
The GPU template pool is bounded to 256 MiB. Budget rejection can reduce visible
coverage and is reported, alongside per-species instance and triangle counts.
Readiness statistics include demand that has not yet entered the bounded queue.
In source v3, grass and flower instance/rejection counters count geometry tiles,
while rowan counters count individual trees. These counters cannot be compared
directly with the per-root ground-cover counts in the source-v1 measurements below.

The opt-in `FRONTIER_VEGETATION_REFERENCE=1` mode requests native vegetation,
raises the color budget to 32,000,000 triangles / 20,000 instances, and uses explicit
bounded caches of 768 MiB CPU geometry and 1,024 MiB GPU templates. Normal mode
retains its 160 MiB CPU / 256 MiB GPU bounds and 1,000,000 color-triangle cap.
The reference flag never enlarges budgets implicitly after a failure. A rejected
or unsettled reference is incomplete. Terrain remains the adaptive display source;
native plants do not make the entire capture an exact-native terrain oracle.

Up to eight nearest offscreen rowans within 32 m may enter separate shadow-only
batches. Their geometry uses automatically generated stride 2 near the camera or
stride 4 under distance/resource pressure, with an 80,000-triangle admission cap.
These jobs follow color and ground-cover requests. Their stable seeded keys reuse
normal mesh storage; they never appear in color draws merely to cast a shadow.
Visible and offscreen casters share the existing 200,000-triangle depth budget,
including a reserved close-ground allocation. Consequently these bounds still
limit shadow completeness; arbitrary unloaded trees do not cast.

Settled frames reuse a prepared selection only when exact camera/view, projection,
viewport, world revision and all vegetation settings match, no results arrive,
and no placement/mesh jobs remain pending. Reuse clones instance arrays and mesh
`Arc` handles, preserving canonical content and relative LRU ages. Any changed
key, new result or world snapshot releases the retained frame before eviction.
This avoids repeated grouping while stationary; it does not optimize moving
camera selection. GPU instance buffers also skip writes when their packed bytes
are identical, including origin, fade, rotation and shadow membership.

## Scope, provenance, and validation

This is deterministic static generation, not live growth, ecology simulation,
plant physics, or a complete botanical model. Plants receive scene lighting and
atmosphere. Beauty mode adds bounded tree shadows and plant ambient occlusion;
see [Cozy Meadow](COZY_SCENE.md) for current budgets and approximation limits,
and the [earlier beauty checkpoint](reference/BEAUTY_CHECKPOINT.md) for its historical
reference settings. `--beauty off` disables that
treatment independently of vegetation.

Edited plant residual meshes can omit overridden native cells and refresh by
world revision. This renderer capability does not enable arbitrary terrain edits
in the Alpine comparison scene; that scene's existing edit restrictions remain.
Coarse silhouette error and transitions are empirical. No seamless-transition,
certified geometric-error, or measured laptop performance claim is made here.

Legacy reference policy: **G** for new canonical placement, streaming, meshing,
and GPU integration; **R** for approved local Python vegetation prototypes and
algorithmic-botany guidance used to establish appearance and growth structure.
No old engine streaming, LOD, scheduling, or renderer implementation was copied.
The local prototypes are references, not a runtime Python dependency or an
authored placement source.

Focused tests cover deterministic placement and species queries, rooted model
connectivity, canonical voxel semantics, outward face winding, exposed-face
merging, palette coverage, coarse negative-grid alignment, cancellation, cache
bounds, seeded geometry variation, rotated ground-tile/edit/border parity, compact
wood occupancy and edit parity, offscreen caster membership, exact static-frame
reuse/invalidation, triangle reservation, and shader validation.

## Playtest evidence

The images, measurements and validation below were archived on **2026-11-08**
with vegetation source **1** and Alpine terrain source **4**, before source-v2
and source-v3 growth and the current beauty treatment. They are historical
evidence, not measurements of the current preset. Source-v3 visual quality,
loading cost and frame times require new captures and measurements.

![Native player-height meadow](reference/gallery/vegetation/player-view.png)

[Wider view](reference/gallery/vegetation/meadow.png) ·
[Approved offline composition](reference/gallery/vegetation/approved-arrangement.png) ·
[Recorded commands, camera, counters and memory](reference/gallery/vegetation/measurements.json).
The offline composition is an artistic target with different placement and lighting,
not a matched pixel oracle for this runtime source.

Measured on Ryzen 5 7640U / Radeon 760M (RADV PHOENIX, Vulkan, Mesa 25.2.8).
Each motion run is six seconds at 3 m/s, uncapped, after settled terrain **and**
vegetation demand, with 60 discarded warmup frames. The route is automated
forward flight, not a player-collision benchmark.

| Setting | Frame interval p50 / p95 / p99 (ms) | Opaque scene GPU mean (ms) |
|---|---:|---:|
| 1080p, MSAA1, no atmosphere/shadows, vegetation off | 13.50 / 21.44 / 23.91 | 8.99 |
| Same settings, vegetation on | 15.46 / 23.73 / 26.86 | 10.38 |
| 720p normal Alpine preset, MSAA4 + atmosphere + terrain shadows | 15.18 / 20.41 / 22.92 | 8.64 |

These are single short runs. They show a usable playtest, not locked 60 FPS,
production performance acceptance, or a temporal transition-quality proof.
GPU timestamps cover the opaque terrain+vegetation pass and exclude atmosphere,
UI and shadow passes; frame intervals include those costs and presentation.

At the fixed 1600x900 default capture: 979 grass instances, 281 edelweiss and
101 rowans were drawn in 14 batches, using 18.48 MiB of plant GPU storage.
All visible trees received coverage; 95 used a coarser resource fallback.
There were zero unresolved tree representations and zero pending jobs. Ground
cover budgets rejected 68 grass and 12 flower candidates; this is bounded display
selection, not removal from canonical source. Plant source/template caches are
independent of the terrain's larger caches. Exact per-frame counts vary with view.

Historical validation: 423 workspace tests passed (existing ignored probes remain ignored).
A live X11 smoke test exercised 6m walking, jumping, landing, F-flight and Esc menu
open/close. Canonical body checks at baseline, post-walk and landing positions
found zero blocking-cell overlap and feet exactly on the sampled ground. The
fixed spiky primary comparison remains bitwise unchanged with vegetation off.
The smoke scripts, raw float captures and detailed logs remain ignored under
`output/vegetation-integration/`; reproducible release flags/counters are archived above.
