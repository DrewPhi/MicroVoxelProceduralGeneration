# November 16 bounded representation experiments

Greenfield experiments for #80/#81/#83/#84; no old runtime or external source
code was copied. The default remains the existing global vegetation sort,
representative coarse cover probe, and live adaptive triangle geometry. These
CPU results do not establish laptop frame-time or visual acceptance gates.

## Edit scheduling foundation

NAADF chooses its next job **after** current publication. Pending local edits
and demanded movement/growth alternate idle turns: continuous following cannot
starve content, and continuous fluid revisions cannot starve following/startup.
An idle camera does not consume a movement turn. Each refresh retains an
immutable world snapshot until atomic publication. Each intervening shift uses
the currently published snapshot. Teleport cancellation, raster restoration,
world epochs and in-flight publication gates precede admission.

`naadf::scheduler::tests` covers two real consecutive strip builds, an edit during
the second, another edit during refresh, one intervening follow build, and the
second refresh. Additional tests exercise unending edit/movement demand and
startup growth. `owns_native_edit_cell` admits a complete representable native
cell only inside published dense coverage and any retiring ownership overlap;
the caller still revalidates the target against the live canonical world.

## #81 chunk-local vegetation ordering: negative synthetic result

`FRONTIER_NAADF_CHUNK_LOCAL_OVERLAY=1` selects in-place radix buckets by native
16³ chunk, followed by ordinary sorting inside chunks. The global sort remains
the control. This composes with `FRONTIER_NAADF_COMPACT_OVERLAY=1` and preserves
address/pigment order, canonical overlap resolution and wide-pigment promotion.
No persistent representation or source identity changes, so no new cache salt
is needed. Native snapshots must remain byte-identical.

The algorithm has at most seven radix levels and three 256-entry stack tables
per level:43,008 bytes on this 64-bit target, charged to workspace admission.
Payload capacity does not increase. Existing `VegetationOverlap` CPU telemetry
includes sorting/resolution; this is summed worker stage time, not frame time.
Cancellation is checked during bucket scans/partitioning and between buckets.

Optimized standalone synthetic benchmark,524,288 entries (512 clustered plants,
1,024 entries each), one run, milliseconds:

| Staging | Global sort | Chunk sort | Payload capacity | Extra scratch bound |
|---|---:|---:|---:|---:|
| Wide 128-bit |24.409|24.492|8,388,608 B|43,008 B|
| Compact 64-bit |12.382|15.031|4,194,304 B|43,008 B|

Chunk sorting has no demonstrated benefit here and remains disabled. This is
not the required 64/128 m moving-window benchmark. Three standalone ordering,
promotion and cancellation tests passed. Full snapshot parity coverage extends
the meadow/Alpine negative-coordinate and edit fixtures across all four
wide/compact × global/chunk combinations. Root coordinates workspace execution.

## #80/#84 stable coarse cover statistics

`FRONTIER_GROUND_COVER_PROBES=16` or `64` enables a 4×4 or 8×8 stratified survey of
canonical 1 m leaf summaries for nodes coarser than level 2. Unset/other values
retain the representative 1-probe control. Each stratum chooses a leaf through
deterministic world-aligned dyadic hash descent. A parent's samples occur in the
corresponding child sample sets. The algorithm never recursively surveys an
unbounded area; each coarse node requests at most 16/64 leaves. Fine levels retain
their existing exact aggregation.

Opted coarse nodes merge coverage-weighted conditional luminance second moments
(including differences between clump means), rather than averaging leaf sigmas.
Canonical placement, flowers, ecological shade, native models, edits and physics
are unchanged. GPU node payload remains 160 bytes in compact mode; this experiment
has not introduced a new GPU texture or claimed bandwidth reduction.

`ground_cover::cache_version()` combines source summary version 5 and probe mode.
Both shared source-summary identity and derived disk identity include it.
Directional and shell modes remain separately keyed. Existing bounded cache,
local edit fingerprints, coalesced requests and producer abandonment apply.
Capture JSON adds coarse probe budget/request count, completed build count and
summed coarse build milliseconds. These aggregate times overlap leaf work.

Independent dense 64-leaf oracle: seed 7, Alpine, four 8 m regions with integer
metre origins(-64,-48),(-48,-256),(-8,-8),(0,0). All quantities below are source
statistics, **not pixel errors**. RGB is the stored color-times-top-coverage.

| Probes | RGB RMSE | Top coverage RMSE | Conditional luminance sigma RMSE |
|---|---:|---:|---:|
|1 control|0.0122133|0.0803582|0.00343025|
|16 stratified|0.00327179|0.0215640|0.000660624|
|64 stratified|0|0|0.000000138|

The combined dense oracle and candidate work took 119.2 ms in one optimized
standalone harness run using the existing world library. All 22 cover/cache/map
harness tests passed, including negative coordinates, bounded/nested probes,
cache hits, local edits, cancellation, failed producer cleanup and original
control parity. This does not measure asynchronous readiness or GPU frame cost.
The mode remains disabled pending fixed/walking/grazing reference comparisons.

The shader edit only reuses the already loaded extra profile and shares the
identical top-axis lighting response between grass and flower terms. It changes
no uniforms or lighting formula; a compiler may already eliminate those common
expressions. No speedup is claimed.

## #83 offline integer macrocolumn shell: not ready for integration

`adaptive_geometry::voxel_handoff` is an offline probe with no live selector.
It compares native unit tops/sides against integer-height macrocolumn stairs
(step 2/4/8 native cells). All exposed vertical unit faces remain present; no
smooth sloping wall substitutes for them. A full native perimeter and canonical
halo retain boundary ownership against native and 2:1 neighbors. Unit quads avoid
introducing unsplit horizontal face edges. There are no bottom or crop-cap faces.
Work is limited to 128² columns,300,000 faces, bounded relief and exact local integer
addresses. Unsupported steps/alignment, water, edited worlds, fixtures with
volume topology, and excessive relief are rejected instead of approximated.

The exhaustive maximum integer top displacementE bounds the dry subgraph
surface's world-space Hausdorff distance: on each grid edge both vertical
interval endpoints move by at mostE, so the intervals have Hausdorff distance
at mostE too. This is **not a ray-depth, perspective silhouette or pixel bound**.
It cannot admit this geometry under a live screen-space quality contract.

Artifacts: [fixed-color comparison](output/nov 16/voxel-handoff/comparison.png),
[CSV](output/nov 16/voxel-handoff/comparison.csv), and individual OBJ/PPM/PNG files
in that directory. Images use the same orthographic camera for every variant of
each fixture,512×384 pixels, simple axis lighting, no plants, fog or source-color
changes. Each tile is 32×32 native cells (2 m×2 m). The smooth 8 control has 32 triangles
and no measured error certificate; it is an illustrative control, not the live
adaptive topology or a certified performance equivalent.

| Fixture | Representation | Triangles | Vertical envelope |
|---|---|---:|---:|
| Synthetic spikes | Native |12,032|0 m|
| Synthetic spikes | Stairs 2 |10,296|0.75 m|
| Synthetic spikes | Stairs 4 |7,592|1.00 m|
| Synthetic spikes | Stairs 8 |5,392|1.375 m|
| Canonical Alpine | Native |2,224|0 m|
| Canonical Alpine | Stairs 2 |2,220|0.0625 m|
| Canonical Alpine | Stairs 4 |2,236|0.0625 m|
| Canonical Alpine | Stairs 8 |2,236|0.0625 m|

The synthetic saving loses substantial source shape; the canonical crop has no
useful triangle saving. Neither result justifies live integration. Two CPU tests
and the ignored reference exporter passed. Tests cover envelope, native edge
identity, negative/global coordinates,2:1 boundary samples, cancellation, explicit
water/volume/edit exclusions and face-budget rejection. #83 remains unresolved:
perspective depth/edge oracles, real near/far transitions and measured useful
compression are required before any runtime experiment.

## Reproduction

Run from the repository root after sourcing the local toolchain environment:

```sh
source ../.toolchains/env.sh
cargo test --manifest-path frontier/Cargo.toml -p frontier-renderer naadf::scheduler::tests
cargo test --manifest-path frontier/Cargo.toml -p frontier-renderer narrow_and_promoted_layouts
cargo test --manifest-path frontier/Cargo.toml -p frontier-renderer compact_overlay_preserves_exact_snapshots
cargo test --manifest-path frontier/Cargo.toml -p frontier-renderer benchmark_chunk_local_sort -- --ignored --nocapture
cargo test --manifest-path frontier/Cargo.toml -p frontier-renderer ground_cover:: -- --nocapture
cargo test --manifest-path frontier/Cargo.toml -p frontier-renderer voxel_handoff::
FRONTIER_HANDOFF_OUTPUT="$PWD/frontier/output/nov16/voxel-handoff" cargo test --manifest-path frontier/Cargo.toml -p frontier-renderer export_voxel_handoff_reference_comparison -- --ignored --nocapture
```

For isolated CPU validation without a Cargo build, use the existing world rlib
from a compatible toolchain. This is how the offline geometry tests/export were
run; substitute the actual current hash. Optimizing this harness does not
rebuild or optimize its world dependency.

```sh
world_rlib=$(ls -t frontier/target/debug/deps/libfrontier_world-*.rlib | head -1)
rustc --edition 2021 --test -O frontier/renderer/src/voxel_handoff.rs --extern "frontier_world=$world_rlib" -L dependency=frontier/target/debug/deps -o /tmp/frontier-handoff-tests
/tmp/frontier-handoff-tests --nocapture
FRONTIER_HANDOFF_OUTPUT="$PWD/frontier/output/nov16/voxel-handoff" /tmp/frontier-handoff-tests --ignored --nocapture
```

The overlay module needs a wrapper because its visibility is parent-scoped:

```sh
printf '#[path = "%s/frontier/renderer/src/naadf/overlay.rs"]\nmod overlay;\n' "$PWD" > /tmp/frontier-overlay-harness.rs
rustc --edition 2021 --test -O /tmp/frontier-overlay-harness.rs -o /tmp/frontier-overlay-tests
/tmp/frontier-overlay-tests --nocapture
/tmp/frontier-overlay-tests --ignored --nocapture
```

Ground-cover standalone validation uses the real source/cache/map modules and a
minimal unused geometry container; no source/summary implementation is stubbed:

```sh
cat > /tmp/frontier-cover-harness.rs <<EOF_HARNESS
#[path = "$PWD/frontier/renderer/src/spatial_appearance.rs"] mod spatial_appearance;
mod adaptive_geometry {
    pub struct TileKey { pub min:[i64;2], pub width_cells:u32, pub step_cells:u32 }
    pub struct GeometryTile {
        pub key:TileKey, pub exact:bool, pub gpu_profiles:Option<()>,
        pub face_profiles:Vec<crate::spatial_appearance::Profile>,
        pub face_profile_indices:Vec<u32>, pub bytes:usize
    }
}
#[path = "$PWD/frontier/renderer/src/ground_cover.rs"] mod ground_cover;
EOF_HARNESS
bytemuck_rlib=$(ls -t frontier/target/debug/deps/libbytemuck-*.rlib | head -1)
rustc --edition 2021 --test -O /tmp/frontier-cover-harness.rs --extern "frontier_world=$world_rlib" --extern "bytemuck=$bytemuck_rlib" -L dependency=frontier/target/debug/deps -o /tmp/frontier-cover-tests
/tmp/frontier-cover-tests --nocapture
```

OBJ and PPM files are produced directly by the ignored exporter. The PNG contact
sheet is a convenience conversion with Pillow; the PPM/OBJ are the reproducible
primary outputs. Generated evidence is ignored build output, not saved worlds.

References remain those of [GROUND_COVER.md](GROUND_COVER.md): Bruneton & Neyret's
[forest coverage/appearance work](https://doi.org/10.1111/j.1467-8659.2012.03016.x)
and [GPU Gems 3,chapter 4](https://developer.nvidia.com/gpugems/gpugems3/part-i-geometry/chapter-4-next-generation-speedtree-rendering)
for separating silhouette and lighting concerns. The local hash estimator and
integer-shell probe are independently implemented experiments, not copies or
claims to reproduce those papers' results.
