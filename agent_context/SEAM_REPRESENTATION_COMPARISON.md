# Two-patch representation comparison — 2026-11-03

Local CPU experiment only. No renderer, active scene, cluster jobs, or world files changed.
Implementation: `tools/compare_seam_representations.py`.

## Fixture and representations

Same original pair as issue #23: two vertically adjacent 8 m cubes at physical
origin [5680,529.8125,4760], 8x16x8 m extent, 6.25 cm native voxels. Authoritative
Rust FBM supplies 853 surface crossings/normals and occupancy labels. The original
native density cache supplies exhaustive validation on all 4,194,304 centers.
Training uses 16,384 uniform native centers plus 8,192 additional collar centers;
32,768 separate random native samples select checkpoints. Finite sampling is not a
proof of FBM topology preservation.

- Original: five independent quadrics per cube; 100 coefficients across the pair.
- Shared CSG: a single expression `max(min(Q0..Q4), min(Q5..Q9))`, positive=solid,
  across BOTH cubes. Ten general quadrics, no cube-specific clipping/field switch.
  This tests a union of two intersections, not arbitrary learned CSG trees or a
  full CAPRI-Net implementation. No pretrained model or encoder is used.
- P2: six face-consistent Kuhn tetrahedra per original cube, one quadratic per
  tetrahedron, 45 shared vertex/edge-midpoint values. C0 field, not C1 normals.
  Mathematical domain tetrahedra are not rendered triangle surface geometry.

No microscopic element grid or correction override is used by either alternative.
Both use closed-form quadratic ray candidates. Exact voxel raycasting in the game
is not integrated or benchmarked by this experiment.

## Two fitting passes

First pass: signed nearest-sampled-surface distance proxy, surface roots and unit
normal targets. P2 linear solve 0.12s; shared CSG soft-to-hard Adam curriculum 40s.
Native error: original 0.696%, P2 3.190%, CSG 2.404%. Both alternatives have zero
shared-face disagreements but lose small features. This is not a successful
reconstruction simply because the boundary agrees.

Second pass: convex squared-hinge native-label fitting for P2, and 20s additional
soft-to-hard classification-focused CSG optimization initialized from pass one.
P2 refinement tries three occupancy weights and selects on the same validation
sample set. The CSG optimizer uses two deterministic starts and only selects hard
Boolean-phase checkpoints (or its starting model if better). Training is nonconvex
for CSG; these results do not establish its representational optimum. No iteration
is selected using exhaustive native results. The first run took 52.61s total; the
refined run took 33.93s including validation/previews/reference timing. P2 fitting
and refinement in the second run took 1.76s.

## Final exhaustive results

| Metric | Original | Conforming P2 | Shared CSG |
|---|---:|---:|---:|
| Native wrong cells |29,198|46,342|53,311|
| Native error |0.6961%|1.1049%|1.2710%|
| Shared-face disagreements /16,384 |1,027|0|0|
| FBM errors on shared face, lower/upper |658/763|512/512|704/704|
| Interior wrong cells |25,994|44,096|48,962|
| Three-layer collar wrong cells |3,204|2,246|4,349|
| Outer collar-edge wrong cells |1,551|1,980|3,441|
| Air components on shared face |1/1|1/1|2/2|

Boundary component counts use four-connected native-center pixels; they are
selected diagnostic slices, not certificates of 3D topology. Source reference has
one air component here. Inspection of `comparison-faces.png` and
`comparison-slice.png` favors P2 on this fixture. CSG introduces a disconnected
boundary air pocket, and both alternatives shift surface/cave shapes compared
with FBM. Zero tile disagreement alone does not certify correct geometry.

## Storage

Actual experimental binary payloads in `output/seam-representation-refined`:

- Original coefficients: 400 B (480 B with existing GPU alignment).
- CSG coefficients: 400 B, plus 48 B of group offsets/primitive IDs = 448 B.
- P2 shared nodal values: 180 B with implicit fixed regular topology; alternatively
  bake 12 quadrics into 480 B (576 B with existing GPU alignment).

Nodal and baked P2 are alternatives, not additive requirements. Bounds, global
lookup, headers and GPU/backend overhead are not included. These are actual small
payload files, not a measured end-to-end renderer memory footprint. JSON exports
are diagnostic models and are NOT loadable scene files in the existing viewer.
Float32 round-trip native occupancy was exhaustively audited: all three error
counts are unchanged (`encoding-audit.json`). P2 audit evaluates shared nodes;
this does not verify independently quantized baked coefficients' GPU continuity.

## Analytic ray and continuity checks

P2 reproduces a global quadratic to 8.9e-16. Independently baked adjacent
polynomials disagree by at most 6.2e-16 at 14,000 internal-face samples, including
faces within cubes. Mathematical trace agreement follows from shared P2 DOFs;
finite-precision GPU accuracy is not tested here.

CPU continuous reference casts 1,024 rays (outside and grazing views), tests all
primitives without acceleration, and includes finite-fixture cut faces. Median
three post-warmup timings: original 5.71ms, P2 18.70ms, CSG 5.87ms. These are
Python/NumPy reference implementations: P2 constructs a dense nodal design while
CSG evaluates a dense quadratic matrix, so these numbers MUST NOT be used to
predict relative GPU speed. All-primitives root slots per ray: original20, P224,
CSG20. Actual production traversal would reject spatial domains first.

For each model, 32 independent fine-sampling + bisection sanity rays agree with
analytic first crossings (96/96), maximum distance error <2e-10m. Sampling can
miss features thinner than its spacing, so this is not an exhaustive ray proof.
Tangential touches without occupancy transitions are outside this crossing test.

## Conclusion

Both alternatives remove tile-boundary field disagreements without increasing
polynomial degree. Neither currently beats the original's native reconstruction.
P2 is the better next candidate on this particular fixture: a closer shared-face
shape, no new face component, smaller nodal payload, and a fast fitting solve.
CSG's cheap analytic evaluation is promising but its fit/topology needs work.
Neither is ready for a 1km refit or replacement based on this test. A later native
six-piece GPU prototype and additional crops are required before making a speed
or global compression decision. Do not infer all splines are expensive from the
old dense 24-piece implementation, or all CSG is inaccurate from this fixed small
assembly and bounded optimizer.

## Reproduce

From the branch root, with existing fixture and `target/release/fbm_points`:

```
python3 tools/compare_seam_representations.py
python3 tools/compare_seam_representations.py --classification --seconds 20 \
  --warm-start output/seam-representation-comparison/csg-model.json \
  --output output/seam-representation-refined
```

Outputs: `index.html`, `metrics.json`, native face/slice/isometric PNGs, training
history, JSON diagnostic models, compact binary payloads. The additional
`encoding-audit.json` was produced after the run by loading these payloads and
re-evaluating native occupancy. Renderer/current world remain untouched.
