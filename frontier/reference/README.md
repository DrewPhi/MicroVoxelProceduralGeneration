# Independent native-cell offline oracle (issue #66 P0)

Greenfield (reuse tier G): this crate uses canonical `frontier_world::World` material queries, material colors and the native fixture bounds. It does **not** call `frontier_world::raycast`, import renderer traversal, rasterize renderer meshes, or use display LOD. This is a bounded correctness oracle, not a distant-terrain or performance acceptance claim.

```sh
cargo run --release -p frontier-reference -- \
  --output /tmp/reference64 --width 1920 --height 1080 --spp 64 \
  --camera fixture --seed 7 --sample-seed 0 --threads 8
cargo run --release -p frontier-reference -- \
  --output /tmp/reference256 --width 1920 --height 1080 --spp 256 \
  --eye 11,9,-15 --target '-1,3,1' --fov-y 60 --seed 7 --sample-seed 0
```

The argument parser expects separate option values. All vector arguments use comma-separated meters. Presets `fixture`, `cave` and `bridge` match the native renderer. Apply presets before overrides. `--near` defaults to 0.025 meters and `--far` to 20000 meters; these are camera-forward clip distances, converted to ray distances per sample. `--plain` disables acceleration. Output dimensions default to 1920×1080. All positive square sample counts up to 65536 are supported; 64 and 256 are the comparison baseline. Change `--sample-seed` for an independent jitter realization.

Sampling uses one deterministic jittered sample per stratum of an 8×8 or 16×16 pixel grid, a box pixel filter, and no exposure change. Per-pixel/sample integer hashing is independent of thread scheduling. RGB accumulates in f64 linear light; only the final raw storage is f32. PNG uses the IEC sRGB transfer, with no tone mapping. Lighting duplicates the *contract* of native flat-material directional/hemispherical shading; there are no shadows, fog or approximated distant surfaces.

The source is clipped to min cell `[-128,-32,-128]`, size `[256,224,256]`, at exactly 0.0625 m per cell. Canonical halo neighbors determine exposed faces; the crop cannot invent solid walls where the world continues. The oracle renders front-facing exposed entry faces. Starting inside solid geometry does not create a synthetic t=0 hit or an interior back-face hit. A near plane exactly on an exposed entry surface is inclusive; a near plane after that surface clips it. Edge/corner crossings advance every tied axis. For simultaneous exposed entered faces the lowest axis wins; hidden tied faces do not conceal other exposed faces. Signed zero is parallel. Invalid rays return no hit.

The independent f64 traversal computes crossing distances from integer grid planes rather than accumulated deltas. The hierarchy contains only exact binary facts: whether an aligned block of 2, 4, 8, 16, 32 or 64 cells contains any exposed face. An empty-surface block can be skipped without approximation, including uniform solid interiors. This is acceleration, not LOD. Tests compare plain and accelerated traversal against exhaustive per-voxel slab intersections, plus exact ties, negative planes, signed zero, partial blocks, near clipping, crop ownership and sRGB transfer. The DDA framing is informed by [Amanatides and Woo](https://www.cse.yorku.ca/~amana/research/grid.pdf); sampling follows the stratification discussion in [PBRT 4](https://pbr-book.org/4ed/Sampling_and_Reconstruction/Stratified_Sampler). All implementation is new in this crate.

Output is top-left origin, row-major, interleaved **little-endian IEEE754 f32**:

| File | Channels |
| --- | --- |
| `linear_rgb.f32` | linear R,G,B, including linear sky clear on misses |
| `coverage.f32` | fraction of samples with a hit |
| `depth.f32` | hit-only ray-distance min, max, mean (meters), population variance (m²); all NaN on no-hit pixels |
| `normals.f32` | sample fractions for −X,+X,−Y,+Y,−Z,+Z |
| `materials.f32` | sample fractions for Grass,Soil,Stone,Wood |

Each histogram sums to coverage. `color.png` is the display image. `coverage.png`, `depth.png`, `normals.png`, and `materials.png` are diagnostics; depth is normalized over image hits and is not a metric buffer. `manifest.json` records camera, source/seed/revision, jitter seed, exact bounds/voxel size, filter, lighting, git HEAD/dirty state, compiled-source FNV-1a fingerprint (world and reference source), timings, CPU, snapshot allocation and Linux process peak RSS. The hash identifies dirty-source runs and is not a cryptographic integrity guarantee. Process peak RSS includes snapshot construction and output allocations. Rendering is independent of machine/thread count.

`center_coverage.f32` and `center_depth.f32` contain one independent ray through each pixel center (coverage 0/1, ray distance in meters with NaN on misses). They permit matched geometry checks against a 1× GPU capture without interpreting finite-sample coverage/depth ranges as exact center geometry. `center_manifest.json` records the matched camera and independent source/seed/edit revision/bounds/voxel/provenance fields. Comparisons must validate these against the multisample manifest; `--centers-only` deliberately does not rewrite that older manifest. A world-only `canonical_source_fnv1a64` additionally distinguishes canonical-source changes from metadata-only changes to the full compiled-source fingerprint. These are written by normal renders. To supplement an existing reference directory without rerendering its multisample buffers or replacing its main manifest, run the same camera/dimensions/seed arguments with `--centers-only`.

The CLI integration test renders a small fixture twice and requires byte identity of all seven raw buffers across two-worker accelerated and one-worker plain traversal, including jittered sampling and center provenance fields.
