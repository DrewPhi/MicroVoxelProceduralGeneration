# Isolated voxel brick / hardware RT experiment

This standalone Cargo workspace uses wgpu 30.0.1. The game remains on 24.0.5;
none of its dependencies or renderer selection are changed. Greenfield algorithms,
with the [Tuxedo Labs talk](https://www.youtube.com/watch?v=IM1Dr98f3xU&t=1900s)
as a design reference and wgpu's procedural-AABB API as the backend.

From `frontier/`:

```sh
cargo build --release --locked -p frontier-tools
./target/release/voxel_fixture output/voxel-fixtures
cargo build --release --locked --manifest-path experiments/voxel-rt/Cargo.toml
./target/release/frontier-voxel-rt-lab output/voxel-fixtures/meadow.vox output/meadow-layouts.json
./target/release/frontier-voxel-rt-lab output/voxel-fixtures/meadow.vox output/meadow-layouts-reverse.json --reverse
```

The parent Cargo target-dir configuration places both executables under
`frontier/target/release/`. Repeat with `rowan` and `mountain`. Each fixture is a
**clipped 8 m cube**, with canonical 6.25 cm voxels, seed 7, Alpine source and
procedural vegetation. This does not measure a 64–256 m streaming window.

All layouts receive the same 57,600 rays from four views. The production NAADF
**occlusion** traversal source is the control, using the exported compact-leaf
hierarchy. Candidates compare software brick traversal versus hardware ray queries,
4³ versus 8³ bricks, full versus tight occupied bounds, and material-word versus
one-bit occupancy reads. Candidate pigment IDs use 16-bit storage like the
compact production leaves. Uniform solid bricks omit their dense payload, and
contiguous vertical runs with identical pigment share one AABB; the native world
is unchanged, including each native hit position. No mesh
or lighting work runs concurrently. Hardware queries select procedural AABBs;
the same native-column DDA resolves the actual voxel within a brick. No triangles
represent voxel faces. Unsupported hardware omits RT cases and reports support.

Each candidate gets sustained GPU warmup before timestamp sampling. GPU spans
exclude readback, compilation, CPU preparation and acceleration structure builds;
the latter's CPU+wait wall time is recorded separately. Explicit payload bytes
exclude opaque driver/BLAS/TLAS/scratch allocation. A reduction in this microbenchmark
is **not** a whole-frame FPS improvement, a streaming result, or a memory win.

Oracle contracts are explicit: an independent CPU FP32 native-plane DDA checks
first hits. A separate FP64 DDA reports first-cell disagreements rather than hiding
near-corner rays. The NAADF shadow interface returns a distance, not a native cell;
its reconstructed-cell differences are retained, with first-hit presence/distance
checked to 0.0001 native cells (6.25 µm). Existing game canonical-ray gates remain
necessary. Passing this finite corpus does not establish a portable WGSL proof.

The prototype has no world-following AS maintenance, renderer-menu integration,
lighting, editing, or full-game frame measurements. Those require a separate
production gate before replacing any part of the working hybrid.
