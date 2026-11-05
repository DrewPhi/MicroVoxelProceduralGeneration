# Local Hermite fitting ablation, 2026-11-03

Script: tools/gpu_fit/local_hermite_experiment.py. CPU only; no cluster jobs or
change to the active world. The final run completed in30.79s including source
sampling, fitting, native-center test evaluation and coarse previews. Training
was27.53s, of which the original hinge baseline consumed25s (5729steps). An
initial1000step baseline was worse; do not quote its24% error as the final baseline.

## Setup

Authoritative Rust FBM,16m cube at world origin[5632,496,4800],2x2x2patches of8m,
native6.25cm voxels. A deterministic search selected a cave-heavy crop (~55% air
removed from an otherwise solid volume). This is a selected underground case,
not a representative random-world average or a surface-landscape benchmark.

3500surface crossings located on coarse lattice edges with8Rust bisection passes;
gradients from central differences of source density, normalized to unit length.
Density is not assumed to be an SDF.8192native training centers receive signed,
truncated nearest-sampled-surface distances, scaled by8m patch edge. Coefficients
are calibrated by point+gradient least squares; source sign labels remain the
occupancy authority. Surface assignments start with source-guided top/cave
separation and spatial clusters, then use active minimum branches.

The Hermite variants change assignments, objective and solver together. This
experiment tests the proposed package, not each change in isolation. Frozen
assignments use ridge least squares; nonconvex branch selection has no global
optimality claim. Both cube variants have5quadratics/patch. Shared-basis Hermite
uses the exact rank25C0 scalar space (125free parameters across5branches).
Independent fitting uses400coefficients and does not constrain joins.

Local P2 uses one continuous field over6Kuhn tetrahedra/cube,125vertex/edge
values, fit by one linear solve. Degree remains2. To reuse the current viewer,
its quadratics are exactly duplicated onto the existing24-tetrahedra partition;
containment in Kuhn tetrahedra is checked at vertices. This is C0, not C1.

## Untouched native-center test (65536samples, seed11937)

| Method | Errors | Error | GPU coefficients |
|---|---:|---:|---:|
| Height initialization |35349|53.94%|1920B|
| Original shared hinge,25s |6064|9.25%|1920B|
| Shared Hermite,0.69s |1824|2.78%|1920B|
| Independent Hermite,0.97s |2904|4.43%|1920B|
| Local P2 Hermite,0.03s |1551|2.37%|9216B current export|

Validation uses a separate65536samples to select iterations. The table uses an
additional test set generated afterward. Neither is exhaustive native coverage.
Shared Hermite and local P2 each have zero sign disagreements at6144sampled
internal-face points, with field jumps~1e-15. Independent Hermite has393face-sign
disagreements on its sampled faces. These do not certify every face point.

P2's4.8xGPUcoefficient cost is caused by the current24-piece export. A native
6-piece evaluator could store48quadrics/group (2304B), versus40quadrics(1920B)
for min-of5cube fields:20%extra baked payload, not yet an implemented/measured
backend. Compact P2 source uses125float values. Its volume-local degrees grow
with3D world extent, whereas the current plane-shared basis grows mostly with
pairwise extent; global storage/fit cost still needs measurement.

## Verification and artifacts

P2 quadratic reproduction~1.8e-15; analytic derivatives versus finite differences
~5.3e-10. A representable shared scalar linear least-squares recovery passes;
this checks the linear subproblem, not the full nonconvex optimizer.

All three improved models load in the existing renderer. Outside and inside-cave
80ray GPU/CPU voxel comparisons passed (480rays total,0mismatches). Checked
exports in output/local-hermite-experiment-checked match final timed exports
byte-for-byte. No renderer/model-format changes were required.

Final artifacts: output/local-hermite-experiment-timed/{index.html,metrics.json,
before-after.png,comparison-isometric.png,comparison-slice.png}. Subfolders hold
loadable terrain.json files. Previews deliberately sample25cm voxels and use
unpainted stone; metrics query native6.25cm centers. Render-grid aliasing in these
small isometric PNGs is not a fitting error.

Reproduce (requires existing local authoring world, initialized shared parameters
and target/release/fbm_points):

```
python3 tools/gpu_fit/local_hermite_experiment.py \
  --output output/local-hermite-new-run --seconds 100
./analytic --scene output/local-hermite-experiment-timed/shared_hermite/terrain.json \
  --mode voxels --camera 8,8,8,0,0 --speed 2
```

The100s limit is a training budget, not an OS hard timeout. Individual bounded
linear solves and previews finish after the check; current run is~31s total.

Next: test4x4x4and8x8x8groups and multiple crops to isolate global capacity from
optimization. Shared Hermite already improves this crop substantially without
extra rendering memory. P2 is a promising local-space comparison, not an automatic
replacement justified by one case. Original hinge also learns here; the full-world
failure cannot be attributed solely to continuity from this small experiment.

Literature guidance:
- https://www.ag.jku.at/pubs/sccg05.pdf (point+normal implicit fitting)
- https://defelement.org/elements/examples/tetrahedron-lagrange-equispaced-2.html
  (local P2 basis)
