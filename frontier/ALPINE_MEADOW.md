# Alpine Meadow scene

Launch from `frontier/`:

```sh
./run.sh --preset alpine
```

A separate scene preset: roughly a kilometre-square rolling meadow, a winding
soil path, overlapping foothills and a connected mountain backdrop several
kilometres away. Seed 7 is the composed starting view. The default Regional and
Legacy sources remain unchanged. This is a scene study, not a new global biome
or geology system. [Procedural voxel vegetation](VEGETATION.md), grounded walking
and rowan-wood collision are enabled; water and plant physics remain absent.

The source remains canonical 6.25 cm voxels. Terrain composition is shared across
scalar evaluation, analytic gradients and conservative affine bounds. Exposed
rock columns remain Stone beneath their surface; turf and path retain soil.
The normal adaptive renderer provides the display approximation. Current terrain
is Alpine source **4** / appearance **2**; vegetation source **3** is independent.
The [Cozy Meadow checkpoint](COZY_SCENE.md) records the combined source, lighting,
cache and approximation contracts. Historical sections below remain frozen.

Vegetation source **3** derives plant placement from global habitat, spacing,
terrain and slope rules, then grows each plant from its full record seed.
The grass carpet favors short spreading growth with sparse taller tufts; rowan
maturity follows broad habitat fields, with independently seeded branching and
curved solid trunks. The cohorts supply growth parameters rather than a finite
model bank. The [cozy meadow inspiration](reference/inspiration/cozy-meadow-ai-v1.png)
is an artistic direction, not a matched renderer capture or performance result.

Canonical plant models use bounded shared `Arc` caches: 64 MiB estimated for wood
and 8 MiB for grass/flowers, with at most 512 entries in each cache. Display grass
and flowers batch into globally anchored 8 m tiles. Expanded neighbor queries and
unique native-cell ownership preserve growth across tile edges; edits are filtered
before meshing. Tile contents do not depend on the camera, and complete tiles fade
near the outer range. CPU display meshes are bounded to 512 entries and 160 MiB
of vertex/index payload; the normal GPU pool remains 256 MiB. An independent
16 MiB / 256-entry compact wood cache supports exact collision after full foliage
models evict. Ground-cover instance counters now count submitted tiles, while
tree counters count individual rowans.
See [the vegetation contracts and limits](VEGETATION.md) for detail.

The preset starts at player eye height on the path, with a 6 km far clip,
3-pixel requested geometric error, four-sample MSAA, voxel appearance filtering,
sun elevation 20° and azimuth 22°, exposure 1.30, fog density 0.000045 per meter,
cloud coverage 0.22 and overcast 0.08. Clouds remain stationary. These are
artistic settings, not a new performance guarantee. Walking follows canonical
terrain; Space jumps and F toggles walking/free flight. Esc opens settings.
`--walk off` starts in flight. The
[earlier beauty checkpoint](reference/BEAUTY_CHECKPOINT.md) documents that
treatment and supersampled reference. The current HDR scene, bounded
shadowed air, volumetric cloud approximation and offscreen plant casters are
documented in [Cozy Meadow](COZY_SCENE.md). `--beauty off` disables the treatment.

See [the reference-driven LOD tuning results](ALPINE_LOD_TUNING.md) for the
Alpine-only spatial material filtering and stronger nearby detail selection.

## Lighting and limits

Directional terrain shadows use a 2048-square depth map covering a 2 km light-space
region, focused ahead of the camera. Texel snapping, depth bias, PCF and a coverage
fade reduce artifacts. The far map uses currently published heightfield geometry,
including owners outside the eye frustum. Unloaded terrain and fixed
external volume owners do not cast shadows in this first pass. Far mountains
still receive directional and ambient shading. Shadow allocations total about
20 MiB for the earlier far-map treatment. Current close plant shadows and HDR/air
render targets add separate allocations; whole-frame measurements must include
them, while the opaque-scene GPU timer excludes atmosphere, UI and shadow passes.
The close map includes a bounded set of offscreen near rowans, not a complete
world-wide caster set. See the checkpoint for normal/reference memory bounds.

Implementation is greenfield, using standard shadow-map principles described in
[Microsoft's shadow depth-map guide](https://learn.microsoft.com/en-us/windows/win32/dxtecharts/common-techniques-to-improve-shadow-depth-maps).
No external assets or engine code were imported.

Visual overrides follow the preset, for example:

```sh
./run.sh --preset alpine --sun-elevation 30 --fog-density 0.000015 --terrain-shadows off
```

## Reproducible captures

```sh
./run.sh --preset alpine --width 1920 --height 1080 --capture output/alpine-scene/meadow.png
cargo run --offline -p frontier-world --example alpine_probe -- 7 --json
```

The probe reports canonical camera positions and sampled terrain ranges. Supply
`--eye X,Y,Z --target X,Y,Z` to repeat other views. Headless captures require
settled requested quality; they are not frame-rate benchmarks.

The independent offline artistic preview uses 1 m far columns and an exact
128 m native foreground, with 64 samples per pixel and ray-cast sun shadows.
It is explicitly approximate at distance and is not the exact-native regression
oracle. See [sampled reference documentation](reference/SAMPLED_LANDSCAPE.md).
The frozen Legacy exact-native comparison remains the primary regression check.

Local captures, raw buffers, logs and manifests live under
`output/alpine-scene/` and are ignored by Git.

## Initial scene validation

The validation and initial captures below were archived on **2026-11-07** with
Alpine terrain source **3** and vegetation disabled. The later path refinement
from that date uses terrain source **4** / appearance **2**, also without
vegetation. These historical checkpoints precede the current beauty treatment
and vegetation source **3**. Their recorded settings and measurements do not
certify the current preset; no new source-v3 performance measurement is claimed.

All 342 Rust workspace tests passed (11 profiling/optional tests ignored). The frozen 1080p Legacy capture matched
its pre-change baseline exactly in float32 color, depth and coverage. This is
a still-image regression result; the headless harness does not provide motion
or frame-time certification.

1080p cold captures at one and 1.5 pixels with a 6 km view exceeded the existing
120-second settling limit. No unresolved image was substituted. The scene preset retains
a three-pixel bound; final screenshots use that launch preset. Loading and performance
tuning remain separate work.

## Initial scene captures

![Player-height path view](reference/gallery/alpine/meadow.png)

![Raised overlook](reference/gallery/alpine/overlook.png)

Both 1080p captures settled at the preset three-pixel bound on the Radeon 760M.
The [capture manifest](reference/gallery/alpine/manifest.json) records source,
cameras, commands, actual resident resources and quality state. Distant material
coverage remains visibly coarse and some contour bands remain; these are not
claims of invisible LOD or a completed photorealistic scene.

## Path and native-mesh refinement

This **2026-11-07** checkpoint uses Alpine source 4 / appearance 2: a narrower path with
variable width and irregular turf shoulders, plus coherent soil relief bounded
to ±4 cm. Exact nearby faces now meet at matching voxel-sized edges to repair
visible rasterization pinholes. No fog, image filling or blur hides missing terrain.

![Refined player-height scene](reference/gallery/alpine/meadow-refined.png)

The [refined capture manifest](reference/gallery/alpine/refined-manifest.json)
records the actual settled view. At that checkpoint, all 346 workspace tests passed (11 optional tests
ignored). The [artifact audit](ALPINE_ARTIFACT_AUDIT.md) records coverage checks,
measured memory/frame-time costs, reference differences and the remaining coarse
material-grid limitation. These changes improve the scene; they do not claim to
eliminate distant material blocks or physically present voxel steps.
