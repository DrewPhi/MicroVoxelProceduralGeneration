# Offline Alpine landscape target

The existing `landscape` binary remains an exact native-column oracle by default.
The new `--terrain-recipe alpine` selects the independent Alpine source; legacy
and regional recipes remain available. `--lighting regional` selects the
independent CPU implementation of the regional face-lighting contract.

An 8192 m square at 6.25 cm would require 131072² columns (about 69 GB of raw
heights alone). For composing the Alpine meadow and distant skyline, use an
explicit sampled far surface with a separately owned exact native foreground:

```sh
cargo run --offline --release -p frontier-reference --bin landscape -- \
  --terrain-recipe alpine --lighting regional \
  --center X,Z --extent 8192 --sample-stride 16 --native-extent 128 \
  --eye X,Y,Z --target X,Y,Z --far 6000 \
  --width 1280 --height 720 --spp 64 --threads 12 \
  --cache /tmp/alpine-far-1m.bin --output /tmp/alpine-reference
```

Substitute a source-matched camera, center and target. Stride 16 samples one
height per 1 m column; stride 8 samples per 0.5 m column. At 8192 m these use
approximately 270 MB and 1.08 GB for the far snapshot plus its max hierarchy,
respectively. A 128 m native square adds approximately 17 MB. Heights retain the
source's native vertical quantization. The near square is centered on the eye's
x/z position and must fit entirely inside the far footprint. Its size must be a
power of two in native cells. Set `--native-extent 0` to explicitly omit it.

Ray segments inside the native square belong exclusively to native columns.
Segments outside belong to sampled columns, including shadow rays. The hierarchy
bounds only the geometry actually stored. It does not certify extrema of terrain
between far samples. The ownership boundary can have residual submeter geometric
error because the far surface is an approximation. This image is an artistic
composition target, **not an exact full-scene voxel oracle**, a silhouette-error
bound, a gameplay source, or a real-time performance result. The manifest records
that distinction, both grid footprints, spacing, source and cache identities,
finite-domain misses, camera, lighting, and sampling. It preserves existing raw
linear RGB, coverage and depth outputs. No caves, edits, structures or vegetation
are represented by this heightfield-only target.

Add `--shadows --shadow-distance 4096` for hard directional ground shadows. Each
covered subpixel traces one visibility ray through the same partitioned geometry,
with a 1 mm face-normal offset. Shadows remove direct sunlight while retaining
the hemisphere ambient term. They include no ambient occlusion or penumbra;
occluders beyond the finite footprint or the specified shadow distance are
omitted. Both shadow flags default to preserving previous unshadowed images.
No fog or atmosphere masks missing coverage.

Cache files are tied to source text/version, recipe, seed, parameters, footprint,
and sample stride. Reusing a path with a different identity fails explicitly.
A previous frozen reference image is never replaced by generating this target.

Validation: `cargo test --offline -p frontier-reference`. Coverage includes
existing exact-column/dense-native parity and byte-exact pixel crops, sampled
world spacing and oblique traversal, cache stride rejection, exclusive native
ownership, and preservation of ambient light under hard shadows.
