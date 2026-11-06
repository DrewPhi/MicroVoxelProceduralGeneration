# Issue 74 — bounded volume backend comparison

2026-11-06. This is one four-second stationary run per backend, using the
previously released `5a623f2` binary, before this turn's source-cache and summary
experiments. It is separate evidence from those experiments. Raw reports,
captures, depth files, logs and parity results are under
`output/issue74/volume-current/` (paths relative to `frontier/`). No binary hash
was recorded with these volume reports.

## Frozen fixture and measured results

The finite native fixture is 256×224×256 canonical 6.25 cm cells: **16×14×16 m**,
with minimum cell [-128,-32,-128]. It includes volumetric content; it does not
exercise streaming or distant LOD. Reports identify seed 7, source version 1,
world revision 0, camera [11,9,-15], yaw -0.6435011, pitch -0.2914568,
60° vertical FOV, near 0.025 m and far 20,000 m. The far plane does not expand
this finite fixture. Output is 1920×1080, render scale 1, MSAA1, voxel filtering
off, atmosphere off, exposure 1, sun elevation/azimuth 35°/135°, and no menu.
Captures have NDC [0,1] depth and RGB decoded from the 8-bit target.

Hardware: AMD Radeon 760M integrated GPU, Vulkan/RADV PHOENIX,
Mesa 25.2.8-0ubuntu0.24.04.2. Vsync is off; 60 warmup frames are discarded.
CPU model is not recorded in these reports.

| Backend | Accepted frames | Frame p50/p95/p99 (ms) | Terrain-pass GPU p50/p95/p99 (ms) | GPU samples / dropped |
| --- | ---: | --- | --- | --- |
| mesh | 2,430 | 1.496 / 2.742 / 3.642 | 0.794 / 0.873 / 0.918 | 1,839 / 602 |
| brick | 774 | 4.562 / 6.246 / 6.348 | 3.803 / 3.868 / 3.893 | 579 / 208 |
| brick-skip | 1,178 | 3.062 / 4.730 / 4.840 | 2.328 / 2.356 / 2.374 | 882 / 309 |

Frame intervals include presentation. GPU timestamps cover only the terrain
render pass; whole-frame GPU cost is unrecorded. All reports have zero missed
budget frames and no benchmark failure/interruption. Mesh has 41,203 greedy
quads / 82,406 triangles, representing 181,362 exposed native faces. Both brick
reports record 4,198,592 brick GPU bytes. This is not complete backend memory:
used/committed memory is null, mesh GPU memory is not fully reported, and
capture render texture/readback each add 16,588,800 bytes. Startup mesh logs
record 0.97/0.85/0.88 seconds respectively, without a comparable breakdown of
brick construction, upload or update costs.

The recorded draw-encode totals/calls are mesh 203.039 ms/2,492, brick
70.993 ms/836 and brick-skip 101.701 ms/1,240, including warmup/capture. These
unequal-duration sample totals do not establish an encoding bottleneck.

## Parity and decision

`brick-parity.json` and `brick-skip-parity.json` report the same mesh comparison:
2,757 pixels (0.13296%) differ by more than one sRGB byte, maximum channel error
188, mean absolute error 0.05429 bytes and RMSE 1.72431 bytes. Each has **83
coverage disagreements**, 465,305 common hit pixels, maximum normalized depth
error 0.00049758 and p99 error 0.00000035763. These are NDC depth errors, not
world-space distance bounds. Equality of the two comparison summaries alone
does not prove the two ray captures are pixel-identical.

Skip traversal is faster than plain brick traversal in this short fixture;
mesh is faster than either. The 83 coverage disagreements remain unresolved.
No repeated/randomized timing campaign, edit/update gate, motion gate or
long-range comparison is established. Retain the existing mesh path and the
brick paths as bounded research candidates; this result does not promote a
general volume backend or satisfy G2/G3/M5.

## Reproduction

The recorded launch loop was run from `frontier/` against the `5a623f2` release:

```bash
for backend in mesh brick brick-skip; do
  DISPLAY=:1 target/release/frontier --scene fixture --renderer "$backend" \
    --width 1920 --height 1080 --msaa 1 --voxel-filter off --atmosphere off \
    --benchmark-input-lock --uncapped --warmup 60 --seconds 4 \
    --report "output/issue74/volume-current/$backend.json" \
    --capture-on-exit "output/issue74/volume-current/$backend.png" \
    > "output/issue74/volume-current/$backend.log" 2>&1
done
```

Parity was computed by importing `compare()` from
`tools/compare_volume_capture.py`, with its default tolerance 1 and both depth
files. Equivalent CLI reproduction for either ray backend is:

```bash
python3 tools/compare_volume_capture.py \
  output/issue74/volume-current/mesh.png \
  output/issue74/volume-current/brick.png \
  --mesh-depth output/issue74/volume-current/mesh.depth.f32 \
  --ray-depth output/issue74/volume-current/brick.depth.f32
```

Substitute `brick-skip` for the ray file prefix for the other comparison.

## Rejected redundant-state micro-optimization

Inspection of pinned `wgpu-core 24.0.5` already finds pipeline deduplication in
`src/command/bundle.rs:1611`, `wgpu_render_bundle_set_pipeline`: its
`current_pipeline.set_and_check_redundant()` returns before recording a repeated
pipeline. `src/command/render.rs:2864`, `render_pass_set_pipeline`, checks the
same state at line 2871 and returns at lines 2874–2877 after validating the pass
has not ended. Both files also deduplicate bind-group commands (bundle line
1593; render line 2832). Verified local source:
`/tmp/voxel-cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-core-24.0.5/`.

An additional application check for identical consecutive pipeline state is
therefore rejected as a priority optimization: the pinned implementation already
removes the redundant recorded commands. This does not establish that all API
call overhead is free or that distinct draw bindings can be batched; actual
encoding savings still require a matched measurement.
