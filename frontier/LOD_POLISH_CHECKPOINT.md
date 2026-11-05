# Four-pass LOD transition experiment

Greenfield changes on `frontier-voxel-engine`, following prototype checkpoints
3–5. Terrain generation, canonical voxel size and occupancy are unchanged.
**Transitions are not invisible yet.** The remaining large defect is the change
from resolvable native stair geometry to a smooth two-metre collar/proxy; this
pass improves stability/shading without claiming to solve that representation gap.

## Iterations and decisions

1. Tested perimeter-only appearance filtering and a grazing microfacet-occlusion
   heuristic. Ground/aerial captures exposed dark patches and bright slivers;
   rejected that heuristic. No fog, blur or generated terrain changes were used.
2. Replaced it with bounded face-area shading at distant/grazing angles, retaining
   projected weighting only at reliable nearby angles. Added the residual-height
   gradient term to morph normals (zero at both endpoints). Also fixed a separate
   temporal defect: per-level morph anchors now survive page publication. They
   follow camera translation immediately and settle publication residuals with a
   0.15 s time constant, maximum 32 m/s correction and 50 ms dt cap. The anchors
   stay inside the page-safe bounds, preserving fine/coarse joins. Disjoint jumps
   or opposing bounds can still require a clamp; no universal zero-pop claim.
3. Widened the optional native appearance band from six to twelve metres and
   changed footprint thresholds to start filtering below two pixels, reaching
   full eligibility below one pixel. The smaller singular value of the local
   footprint avoids treating a grazing but wide face as unresolvable. Larger
   native faces retain their material and face lighting. This is a shading
   heuristic, not a screen-space geometry/silhouette bound.
4. Moved four macro sample reads and appearance evaluation below an early return
   for fragments with no blend contribution. Derivatives remain above divergent
   control flow. Pass3/pass4 captures match exactly in two views; the other two
   each differ by one sRGB byte in one pixel. Made 4×MSAA the default; retained
   `--voxel-filter off` as default because appearance filtering still has bias.

## Measurements

Seed7, FBM source v1 defaults, 6.25cm native cells, Ryzen5 7640U/Radeon760M,
RADV PHOENIX Vulkan/Mesa25.2.8; 1920×1080, 4×MSAA, no fog/TAA. Same initial
camera, realtime routes, 12 seconds, 60 warmup frames, uncapped. All jobs ran
sequentially. GPU timestamps are sampled asynchronous render-pass timings;
CPU generation/uploads/presentation are not included in that GPU number.

| Profile / route | frame p50 / p95 / p99 ms | GPU p95 ms | >16.67ms frames |
|---|---:|---:|---:|
| Before / walk-stop-resume 3m/s | 6.18 / 8.91 / 9.98 | 6.76 | 5/1807 |
| Final default / walk | 6.40 / 8.88 / 10.12 | 6.81 | 5/1770 |
| Optional filter / walk | 6.29 / 8.92 / 10.29 | 6.86 | 3/1796 |
| Before / turn-return 15m/s | 7.39 / 10.62 / 13.44 | 9.15 | 5/1500 |
| Final default / turn-return | 7.47 / 10.91 / 13.00 | 8.96 | 7/1496 |
| Optional filter / turn-return | 7.37 / 10.40 / 13.20 | 8.97 | 7/1508 |

These short runs show similar costs, not a demonstrated speedup or guaranteed
60fps at every instant. Generation/upload/cache budgets are unchanged; morph
state adds eleven world-space 2D anchors and 176 uniform bytes, not new meshes.

The regression reproduces the old 1.50–2.39m common-vertex height jumps when a
page publishes after 16–30m of lag. Retaining the anchor gives **zero instantaneous
displacement in those tests**, then bounded convergence. Cold captures initialize
at their target; they do not prove temporal continuity. Tests also cover camera
response, endpoint joins, negative/far coordinates and teleport bounds.

The new fixed-camera `--capture-coverage-shift 8` diagnostic changes only the
requested coverage. Final experimental appearance reduces full-image linear
RGB RMSE between cover selections from .003300 to .002637 (~20%). This metric
measures one seed/view's representation change, **not** exact voxel correctness
or global seam disappearance. The ordinary frozen camera ground/aerial/moved
captures still show the native/proxy boundary and coarse facets.

Against the existing 3840×2160 MSAA reference, native-heavy bottom-corner RMSE
was .015244 before versus .016158 with the experimental filter (~6% worse).
Whole-frame errors include deliberately different distant lighting, so are not
aliasing-only metrics. This is why the filter remains optional rather than being
promoted on a prettier average color. A proper intermediate voxel-faithful
representation / projected-error handoff is still needed. Do not hide this by
extending the current expensive native square indiscriminately or blurring it.

Validation: 85 workspace tests pass, one existing expensive benchmark ignored;
WGSL validation, Rust formatting and Python syntax checks pass. Two Sol6.1
medium workers handled shader and temporal-morph work; root integrated,
performed four rounds of captures, compared images and ran GPU timings.

## Try and reproduce

```sh
./run.sh --scene hybrid
./run.sh --scene hybrid --voxel-filter on
# Optional original single-sample AA comparison:
./run.sh --scene hybrid --msaa 1
python3 tools/lod_transition_sweep.py my-default --filter off --benchmark
python3 tools/lod_transition_sweep.py my-filter --filter on --benchmark
```

`--capture-coverage-shift METERS` is a headless hybrid-only X offset bounded to
±24m. It keeps the camera/world fixed and changes initial requested coverage;
it does not alter interactive demand or world generation. Test artifacts stay
under ignored `output/lod-polish/`, including frozen before/pass binaries,
three-view captures for each pass, coverage-shift comparisons, JSON timings and
`sampling-comparison.json`. The release launcher contains the final changes.

The clipmap transition principle follows [Asirvatham & Hoppe, GPU Gems 2](https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry):
geometry and shading must transition together at shared levels. The per-page
publication filter and native appearance experiment here are independent local
implementations, not an assertion that the paper solves voxel/heightfield
ownership. No external or legacy runtime source was copied.
