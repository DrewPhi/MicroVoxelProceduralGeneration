# Geographic atlas playable checkpoint

## Scope

Connect the saved climate/hydrology atlas to the active Rust game without
replacing rendering, fixed-grid occupancy, collision, edit transactions or
streaming. The new source is selectable as `--world atlas`; the MicroVoxel
launcher starts seed 42 at World Scale 10. The legacy source is explicitly
available as `--world procedural` for comparison. No live weather/fluid physics,
new vegetation, or renderer rewrite is included.

## Implementation

- `generation/atlas/data.rs`: checked immutable binary loading, content identity,
  compact triangle and receiver-channel spatial indexes. Physical coordinates
  remain meters before world-scale compression and voxelization.
- `generation/atlas/refine.rs`: bounded analytic cubic patches with shared edge
  controls. Spatially shared parent channels prevent mismatched cuts near
  confluences. Lake levels are independent of dry shore heights. Small internal
  tributaries taper at boundaries; centimeter surface grain is bounded.
- `generation/atlas/mod.rs`: Generator adapter, bounded sharded patch cache,
  source-space queries, material painting, height limits and LOD metadata.
- `generation/atlas/cache.rs`: canonical compilation parameters, validated seed
  42 reuse, per-seed versioned caches, isolated temporary builds and atomic
  publication. Failed artifacts are retained, never treated as finished worlds.
- Viewer world menu: atlas selection persists through seed/scale/grid changes;
  new seeds compile asynchronously. Imported terrain remains optional.
- World envelope supports 1000 km and higher atlas peaks. Legacy generation
  retains its former limits so enlarged atlas bounds do not inflate its indexes.

The local patch cache holds at most 2048 triangles. Patch eviction or different
query order must not affect terrain. It is inexpensive analytic reconstruction,
so only the atlas is persisted; derived patches are rebuilt. Native appearance
is deterministic and retains one color per voxel on every face.

## Research and limits

The coarse landscape uses the coupled uplift/erosion and climate work documented
in GEOGRAPHIC_ATLAS_MILESTONE.md and ATLAS_CLIMATE_MILESTONE.md. Shared network
constraints follow the design motivation of [Génevaux et al. 2013](https://perso.liris.cnrs.fr/eric.galin/Articles/2013-river-networks.pdf).
[Schott et al. 2024](https://h-schott.github.io/p/mserosion/) remains the comparison
for future multiscale erosion. This implementation is an analytic reconstruction,
**not** an implementation or measured substitute for that erosion algorithm.

The approximately 977 m atlas mesh still limits newly invented geological
structure. Local branches are bounded embellishments representing existing
runoff, not an independently conserved catchment erosion/sediment simulation.
The runtime does not add their discharge to the compiled water budget. Full
child-catchment flux validation, glacial landforms and detailed sediment evolution
are not completed by this checkpoint. The old source remains for comparison
until visual and hardware-performance acceptance.

## Reproduce

```sh
./tools/build_launcher.sh
./MicroVoxel
./MicroVoxel --world atlas --world-scale 1
./MicroVoxel --world procedural
cargo test --locked --workspace --manifest-path successor/Cargo.toml
cargo test --locked --release --manifest-path successor/Cargo.toml -p microvoxel \
  --test atlas_runtime -- --ignored --nocapture
MICROVOXEL_CACHE_ATLAS=1 cargo test --locked --release \
  --manifest-path successor/Cargo.toml -p microvoxel-successor \
  runtime::travel_cache_test::predictive_detail_and_backtracking \
  -- --ignored --exact --nocapture
```

The ignored saved-atlas checks require `output/atlas-climate/seed42`; they never
silently trigger a three-minute compiler job. See cache metadata for the exact
canonical seed/extent/resolution/climate configuration.

## Validation record

- Full workspace: 208 passing tests, no failures (GPU and large fixture tests
  remain explicitly ignored by default).
- Saved seed42 integration: 4096 distributed column queries, reverse request
  order/cache eviction, display-LOD queries followed by unchanged native queries,
  grid/scale identity, rounded boundary coverage and persistent edit reload pass.
- CPU player: 1200 ticks of walking, stopping, resuming with sprint, and jumping;
  no character/terrain overlaps. This is collision validation, not a rendering
  streaming performance result.
- Shared-edge regression: 192 random triangle edges with seven samples each,
  plus all six sectors at the previously failing node 1237, pass. Separate tests
  cover lake shore levels, channel iteration order and local boundary fade.
- Seed42 source loading: 0.6–1.05 s in these runs; approximately 160 MiB source
  payload including spatial indexes and summary hierarchy. This excludes GPU
  meshes, player data and process/allocator overhead.
- Fresh canonical seed1: solver 146.784 s; full compile/validate/load 147.792 s;
  warm reload 0.793 s. Published cache identity, spec and tested columns match.
- Launcher and compiler helper rebuilt. Headless spawn and range captures run
  without runtime errors. Captures/logs: `output/atlas-climate/viewer-atlas-*`.
- GPU travel test compiled but timed out at 120 s before phase completion. This
  session exposes llvmpipe software Vulkan and no `/dev/dri`; hardware streaming
  performance remains unverified. Do not interpret these frame times as laptop
  GPU performance.

Visual review: broad hills/ridges and connected major drainage are present.
Shallow grassy slopes still show dark contour-like marks, and river appearance
needs further visual acceptance. The analytic reconstruction does not yet have
rich sub-kilometer geology. This is a playable inspection checkpoint, not a
claim that the full terrain replacement research plan has passed every gate.

Final close-view correction: river profiles are capped by pre-carve terrain so
unsupported narrow ribbons cannot form spikes above the banks. Compiled lake
levels are handled separately. Seven fast regression tests and 15,925 saved
river cross-section samples pass; unsupported river-water excess is zero.
The final launcher capture `output/atlas-climate/viewer-atlas-range-final.png`
was inspected after rebuilding: pointed water artifacts are gone; narrow
river visibility still breaks up at distance. Log saved beside the PNG.
