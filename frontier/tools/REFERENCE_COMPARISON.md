# Independent reference comparison

From `frontier/`, with Rust/Cargo, NumPy and Pillow installed:

```sh
python3 tools/reference_gate.py --output /tmp/frontier-reference-gate
```

The driver builds release binaries, renders the native fixture at 1920×1080
with 64 and 256 reference samples per pixel using independent sample seeds
19 and 83, captures the existing GPU mesh renderer at the identical explicit
camera, then writes JSON metrics and side-by-side/difference PNGs. All child
processes inherit the environment: choose the desired GPU/ICD and check the
recorded adapter in `gpu.capture.json`. Software Vulkan is usable for correctness
smokes; it is not a laptop GPU performance measurement.

Use `--camera fixture|cave|bridge`, `--width 320 --height 180`, `--seed 7`,
`--threads 8`, or `--skip-build` as needed. Binary overrides are `--gpu-bin`
and `--reference-bin`. The output contains `reference64/`, `reference256/`,
`gpu.png` with raw sidecars, `comparison/`, and `run.json`.

To compare previously generated outputs:

```sh
python3 tools/reference_compare.py /tmp/frontier-reference-gate/reference256 \
  /tmp/frontier-reference-gate/gpu.png \
  --convergence /tmp/frontier-reference-gate/reference64 \
  --output /tmp/frontier-reference-gate/comparison
python3 -m unittest discover -s tools -p test_reference_compare.py
```

GPU capture options are `--scene fixture --eye X,Y,Z --target X,Y,Z --fov-y 60
--msaa 1 --voxel-filter off --capture-linear --capture /tmp/gpu.png`. The explicit
camera is currently limited to the bounded native fixture because streaming
coverage still derives from preset cameras. Default runtime appearance is
unchanged. `--capture-linear` uses the existing shaders with an unclipped
RGBA32Float attachment. Its RGB sidecar preserves linear shader output; its PNG
preview converts to sRGB and clips to display range. Ordinary captures decode
their 8-bit target and identify this quantization in metadata. Single-sample
captures write binary coverage and zero-to-one perspective depth. The comparison
converts depth to distance along each pixel-center ray.

Metrics distinguish exact pixel-center ray comparisons (when center files are
present) from box-filter reference coverage and hit-depth ranges. Sampled depth
ranges accept multiple visible surfaces within a pixel; they cannot prove that
an intermediate wrong surface is absent. Partial coverage is reported separately
from false/missing coverage against fully covered/empty reference pixels. The
center comparison reports raster edge-fill and GPU floating-point differences
without suppressing them. The default depth tolerance is 2 mm and can be changed
with `--depth-tolerance`.

Camera, dimensions, seed, source version, edit revision, and native crop bounds
must match. Center-only files also need a matching `center_manifest.json`, so
stale center data cannot silently validate another world. Reference convergence
reports both sample seeds and the observed radiance/coverage difference; it does
not establish a formal convergence bound.

Color error remains separate from geometry. The reference samples direct light
and hemispherical fill independently; the real-time shader uses its existing
appearance approximation. Reference normal/material histograms are available,
but GPU normal/material attachment parity is not measured here. This fixture
comparison does not certify hybrid ownership, distant proxy error, streaming,
temporal stability, or a 60 FPS quality profile.
