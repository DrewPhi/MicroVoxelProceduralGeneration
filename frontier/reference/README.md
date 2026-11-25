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

The independent f64 traversal computes crossing distances from integer grid planes rather than accumulated deltas. Cell ownership and entered-face bits also compare integer-plane crossing times: `o+d*t` can round onto a neighboring plane a few ulps before its actual crossing, so flooring that value alone can skip a thin interval at nearly tied crossings. Strict entry/exit time comparisons preserve those intervals without an epsilon. The hierarchy contains only exact binary facts: whether an aligned block of 2, 4, 8, 16, 32 or 64 cells contains any exposed face. An empty-surface block can be skipped without approximation, including uniform solid interiors. This is acceleration, not LOD. Tests compare plain and accelerated traversal against exhaustive per-voxel slab intersections, plus exact ties, negative planes, signed zero, partial blocks, near clipping, crop ownership and sRGB transfer. The DDA framing is informed by [Amanatides and Woo](https://www.cse.yorku.ca/~amana/research/grid.pdf); sampling follows the stratification discussion in [PBRT 4](https://pbr-book.org/4ed/Sampling_and_Reconstruction/Stratified_Sampler). All implementation is new in this crate.

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

### Exact kilometer landscape oracle

`landscape` extends the reference to a bounded regional FBM landscape without
allocating a dense three-dimensional volume. It samples **every native 6.25 cm
column**, stores the canonical integer top plane in an i32 array, and constructs
an exact maximum-height hierarchy over 16×16-column blocks. Hierarchy traversal
only rejects space above the maximum; surviving rays traverse native columns
and return their exact exposed top or side face. There is no geometric LOD,
interpolation, coarse height substitute, bottom cap, or artificial crop wall.
This representation is valid only for the unedited FBM heightfield source;
it does not represent caves, overhangs, structures, or edits.

```sh
cargo run --release -p frontier-reference --bin landscape -- \
  --cache /tmp/frontier-native-regional7-1156-3469-1024.bin \
  --output output/km-reference/reference64 --center -1156,3469 --extent 1024 \
  --eye -1156,900,3269 --target -1156,200,3469 --fov-y 35 --far 98304 \
  --width 1920 --height 1080 --spp 64 --threads 8
```

The square extent must be a power of two in native cells, between 1 and 2048 m.
At 1024 m, the 16384² native height array plus hierarchy uses 1,079,334,228 bytes.
Cache identity includes canonical source text, all source parameters, seed,
integer footprint and source version. A mismatched cache is rejected; choose a
new cache path. `--build-only` generates the cache without rendering.

The output contains `color.png`, `linear_rgb.f32` (RGB), `coverage.f32`
(hit fraction), `depth.f32` (one hit-only mean ray distance, NaN on miss), and
`manifest.json`. The landscape depth format differs from the four-channel
fixture depth format. Sampling uses deterministic jittered square stratification (SplitMix64,
`--sample-seed 19` by default) and a box filter, with f64 linear accumulation
and sRGB PNG output. Lighting and flat
material colors use the existing reference contract. CPU paint reproduces the
current renderer's coordinate hash and 32 m/4 m coherent tint; fine grain is
zero at landscape distances. `--no-tint` disables this paint. Materials remain
canonical per native voxel, including the soil/stone beneath surface voxels.
The manifest includes the complete source fingerprint, git revision/dirty state,
cache identity, sampling seed and ray count. The finite crop is explicit;
a missing sample remains sky and is counted in the
manifest's `miss_fraction`. An all-ground view with zero misses avoids comparing
the finite reference boundary against the GPU's larger coverage.

Tests compare 2500 rays against an independent dense native snapshot, including
hit cells, distances, materials and face normals. Separate tests exercise exact
horizontal/top ties, vertical rays, crop walls and persistent cache identity.

### Pixel crops preserve the full camera

The landscape CLI accepts `--crop X,Y,WIDTH,HEIGHT` in top-left viewport pixels.
`--width` and `--height` still specify the **full viewport**: aspect ratio, FOV,
camera rays, jitter hashes and sample indices use global viewport coordinates.
Only the selected pixels are rendered and written. Raw buffers and PNG dimensions
are the crop dimensions. The manifest records output `width`/`height`, full
`viewport_width`/`viewport_height`, and `pixel_crop: [x,y,width,height]` explicitly.
Omitting `--crop` is equivalent to selecting the full viewport. Empty, overflowing
or out-of-viewport rectangles fail before native-column construction.

For a bounded 64×64 center crop of the primary camera with a 1 km forward clip,
reuse an **existing matching** native cache centered at `[0,-512]` meters with a
1024 m extent. Set `REFERENCE_CACHE` to that existing file; the guard below avoids
creating the roughly 1 GiB native array cache accidentally:

```sh
test -f "$REFERENCE_CACHE" && target/release/landscape \
  --cache "$REFERENCE_CACHE" --output output/km-center-reference64 \
  --terrain-recipe legacy --seed 7 --center 0,-512 --extent 1024 \
  --eye 0,94.8875,0 \
  --target -8.731370115810932e-08,94.83756238296628,-0.9987523555755615 \
  --fov-y 60 --near .025 --far 1000 --width 1920 --height 1080 \
  --crop 928,508,64,64 --spp 64 --sample-seed 83 --threads 8
```

The camera and source match the primary fixture; the 1 km far clip is an explicit
extension of its usual 300 m comparison. Pixel cropping reduces ray/output work,
not native snapshot memory. The finite native footprint remains authoritative
only inside its recorded bounds. `finite_domain_miss_samples` and
`finite_domain_miss_fraction` conservatively count missed rays whose near-to-far
horizontal segment leaves those bounds. They identify incomplete oracle coverage;
such misses remain sky and cannot establish a geometry pass. Other misses remain
reported by `miss_fraction`. Compare the matching pixel rectangle from the full
renderer image, and do not treat a center crop as full-view quality acceptance.

CPU tests require byte-identical RGB, coverage and depth between a tiny full
landscape render and its pixel crop, verify sample/ray identity, reject malformed
crops and exercise finite-domain miss classification. They build only an 8 m
native fixture; no kilometer cache or GPU run is required.
