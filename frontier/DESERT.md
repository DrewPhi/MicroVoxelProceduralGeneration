# First desert terrain checkpoint

From `frontier/`, build and launch:

```sh
cargo build --release --locked -p frontier-tools --bin frontier
./run.sh --preset desert --seed 19
```

`--seed random` chooses a reproducible new seed. `--start-xz X,Z` overrides the
shore spawn. F switches between walking and free flight. Native voxels remain
6.25 cm. This preset disables vegetation and live water; ponds are canonical
static water above a real voxel bed. It does not add the old man model. The [bowl interaction](BOWL.md) is now
available in this preset.

## Shared rendering baseline

Desert and alpine now share the same playable renderer policy: detailed appearance
maps, spatial profile filtering, half-footprint filtering, four-cell profile probes,
near-camera error reserve and urgent refinement priority, source-cache allocation,
HDR scene targets, terrain shadows, development resource profiles, sky, clouds,
haze, lighting and exposure. Both presets request pixel error 3 and eleven view
levels, and use the same NAADF/AA defaults and Esc controls. The desert source,
spawn and generated content remain desert-specific; alpine meadow vegetation is
not inserted. Existing planting, pockets and saves are preserved.

`renderer/src/scene_policy.rs` centralizes capabilities previously hard-coded to
Alpine. `VisualSettings::apply_polished_presentation` supplies the shared look.
Historical comparison sources remain unchanged. The disk cache already includes
the appearance mode in its signature, so old desert appearance products cannot
silently satisfy the new policy even when using the same cache directory.

The playable hybrid defaults to one primary sample with native temporal filtering.
For subpixel sand terraces, NAADF estimates the pixel footprint using a local
slope fit from native columns and shades the projected mix of voxel tops and
sides. Exact hit depth is retained. Set `FRONTIER_NATIVE_SAND_FOOTPRINT=off` to
compare with face-by-face shading under the same renderer.
The historical Alpine raster sampling setup can also be used on the desert with
`./run.sh --preset desert --seed 19 --naadf off --msaa 4`. On the raster terrain,
sand, grass and soil receive the soft-ground appearance-map prefilter.

The separate live-water demos are not enabled by the alpine preset either.
The planting bowl's static-lake integration gap is independent of rendering;
copying these settings does not activate the pressure/flow solver for that lake.

## Source

`world/src/desert.rs` composes low-frequency four-octave FBM, elongated warped
noise crests, coherent rocky provinces, shallow dry washes, and centimeter-scale
surface variation. There are no finite map edges within the engine's supported
coordinate range. The new `Desert` recipe has independent source/appearance keys;
existing recipes retain their definitions.

One deterministic oasis descriptor is jittered within each 1.4 km square in
source meters. Ponds have 22–36 m nominal radius, narrower secondary axis, a
noise-distorted shoreline, a roughly 2 m center depth, and a level water stage.
A smooth basin blends into the surrounding terrain. Most ground is dry sand;
coherent raised rocky provinces expose stone. There is no oasis vegetation yet.
The regular descriptor ownership is a first sparse distribution, not a simulated
groundwater model. Dry washes are visual landforms, not a solved drainage network.

The ordinary horizontal scale and amplitude parameters scale the complete
composition. The desert currently uses a fixed authored set of fractal bands;
the legacy octave/roughness flags do not tune this recipe. Dedicated desert
controls can follow visual approval.

Scalar heights and reduced affine bounds evaluate the same algebra, including
basins and water. Queries wider than 5.6 km in authored coordinates use an explicit
global interval to keep descriptor enumeration bounded. Native, coarse appearance, collision, and reference queries
retain separate water and ground runs. Older experimental GPU summary/profile
paths reject this water-bearing recipe rather than treating it as dry terrain.
An empty cloud setting now actually disables the cloud-density pass.

## Validation and captures

Source tests cover multiple seeds, positive/negative descriptor coordinates,
level pond surfaces, underwater bed material, dry surroundings, and conservative
bounds through dune/shore regions. A parser test checks the no-vegetation,
grounded, dry-bank preset. `world/examples/desert_survey.rs` prints deterministic
pond descriptors and potential surrounding backdrops to help reproduce views.

The main screenshot is `output/desert-first/oasis-player.png` (seed 19).
The rocky alternate is `output/desert-first/player-final.png` (seed 7).
Reproduction metadata is in `desert-checkpoint.json`.

The screenshot is an actual 1920×1080 game capture, from 1.7 m above the native
ground after streaming. It is not an AI image. Captures/reports live under
`output/desert-first/`; generated data stays ignored. A static frame does not
certify motion stability or performance. No terrain renderer or LOD tuning is
promoted by this checkpoint. Rock formations, dune asymmetry, damp-bank painting,
and broader visual diversity can be tuned after this first inspection.

Final checks: 196 world tests passed (6 ignored), 556 renderer tests passed
(39 ignored), and `cargo check --workspace --tests --locked` passed. The approved
hybrid primary comparison remains pixel-identical. The final captures have full
coverage, but the adaptive quality gate had not settled: basin/shore affine bounds
are conservative and cause excess refinement, especially across distant ponds.
This is an open desert-specific optimization/LOD-quality task. Frame timings
recorded while compilation ran are provisional. These frames are initial visual
playtests, not strict pixel-error acceptance or settled-quality references.
