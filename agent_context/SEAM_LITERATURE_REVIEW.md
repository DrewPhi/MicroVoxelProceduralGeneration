# Seam representation review — 2026-11-03

Research only; no renderer or active world changed.

## Finding

Continuity has established solutions. The conjunction of arbitrary FBM accuracy, five quadrics per large independently owned cube, guaranteed boundary agreement, and unchanged storage/traversal is not established by the papers reviewed. A soft loss cannot turn an infeasible representation into an exact join. The local shared-target experiment improved face disagreement 1027→877 while increasing native error 0.6961%→0.7135%, worsening native crossing-edge counts, and introducing a disconnected face component. This is a representation/optimization warning, not a proof all independent fitting is impossible.

## Mathematical restrictions (our analysis)

A total-degree-two polynomial restricted to a planar cube face is a conic polynomial. A generic irreducible conic shared exactly by two such traces makes their traces proportional. Matching their whole numerical fields is stronger than matching geometry, but positive rescaling freedom does not permit arbitrary independent contours. Matching a finite set of crossing points is not matching a whole boundary curve or its topology.

If two polynomial pieces agree on every face of one cube, their difference vanishes on all six supporting planes, so a nonzero difference is divisible by their product, of degree six. A total-degree-two piece therefore has no independent interior bubble once its complete boundary traces are fixed. This argument concerns one polynomial and fixed full traces, not a general impossibility theorem for min-of-quadrics geometry.

## Best established exact-continuity baseline

Use conforming C0 P2 implicit elements over a face-consistent six-tetrahedron subdivision of the existing large cubes, sharing vertex and edge-midpoint scalar degrees of freedom. Each tetrahedron has one ten-coefficient total-degree-two implicit polynomial. Shared triangular faces have identical quadratic traces. The field is C0, not necessarily gradient continuous; degeneracies/topology still require validation. Ray intersection remains quadratic. Tetrahedra define polynomial domains, not triangle surface rendering.

This is different from restoring a dense 24-tetrahedron microgrid. One single-field six-tet cube would use 60 coefficients unshared versus 50 for the current five-quadric cube (240B vs200B coefficient-only). Shared nodal storage can reduce this, but metadata, traversal and GPU padding must be measured. Equal cell width does not guarantee equal approximation quality. Source complexity can still force more elements. Existing small Hermite experiments already tested a P2 alternative; the current renderer duplicates its six pieces into a 24-piece format. A native six-piece backend is absent. Neither memory nor speed can be inferred from source parameters alone.

Primary references:
- DefElement P2 tetrahedron, basis and ten DOFs: https://defelement.org/elements/examples/tetrahedron-lagrange-equispaced-2.html
- Loop & Blinn, Real-time GPU rendering of piecewise algebraic surfaces, SIGGRAPH 2006: https://doi.org/10.1145/1141911.1141939
- Stoll, Gumhold & Seidel, Incremental Raycasting of Piecewise Quadratic Surfaces on the GPU: https://tu-dresden.de/die_tu_dresden/fakultaeten/fakultaet_informatik/smt/cgv/publikationen/2006/irpqs.pdf

## Best alternative experiment for preserving primitive compression

Replace cube ownership of the geometry with shared analytic CSG primitives. Cubes remain acceleration bins; the same quadric ID and Boolean expression apply on either side of a bin boundary. Fit one small solid across the two-cube fixture, allowing union, intersection and difference as required. Do not independently clip different models at the face, and do not change the expression based solely on which cube contains a point.

CAPRI-Net is a concrete ML precedent for compact quadric CSG fitting, including staged optimization addressing hard min/max gradient starvation. Its published primitives are constrained seven-parameter axis-aligned convex quadrics, whereas ours are general ten-coefficient quadrics. The training encoder is not necessary for direct per-scene fitting. Its CAD benchmark results do not establish terrain accuracy, optimal compression, manifold topology or fast integrated-GPU rendering.

For a fixed finite Boolean expression, boundary candidates are primitive roots (plus any explicit clipping planes). Every candidate root remains a linear/quadratic solve, followed by Boolean validity or interval events. Combining many primitives does not require solving one enormous high-degree polynomial. It can, however, substantially increase candidate/root sorting, membership and acceleration costs. Spatial bounds must conservatively cover every relevant primitive and Boolean dependency, particularly differences and unbounded primitives. AABB bounds are acceleration only, not automatically valid solid clipping planes.

A globally consistent CSG model eliminates disagreements caused by selecting different fitted fields at the same bin face. It does not guarantee a faithful FBM fit, smooth joins, absence of cavities/slivers or a manifold boundary. More than the current primitive budget may be necessary. This is an experimental recommendation, not a published solution to our exact terrain problem.

Primary references:
- Yu et al., CAPRI-Net, CVPR 2022: https://arxiv.org/html/2104.05652
- Authors' implementation: https://github.com/FENGGENYU/CAPRI-Net
- Chen et al., BSP-Net, CVPR 2020 (plane/convex decomposition predecessor): https://openaccess.thecvf.com/content_CVPR_2020/html/Chen_BSP-Net_Generating_Compact_Meshes_via_Binary_Space_Partitioning_CVPR_2020_paper.html

## Other reviewed approaches

- Geometric continuity and Hermite algebraic patch design are classical. They require compatible boundary geometry and degrees of freedom, rather than proving arbitrary fixed cube traces can join cheaply. https://www.sciencedirect.com/science/article/pii/016783969190049H ; https://docs.lib.purdue.edu/cstech/798 ; https://ecommons.cornell.edu/entities/publication/e056a065-8de3-401e-8a25-4e347ba15f93
- MPU blends local quadrics with spatial weights. Generic nonconstant weighting destroys the pure degree-two ray equation. It solves continuity by changing the evaluated representation: https://doi.org/10.1145/882262.882293
- SLIM uses sparse nonconforming low-degree surface elements and blends along view rays. This is an appealing visual approximation but does not give our required view-independent exact occupancy. https://diglib.eg.org/server/api/core/bitstreams/0470a2f1-d7ca-47df-984f-d59b91bb26a7/content ; GPU rendering discussion in Stoll et al. above.
- Sparse voxel corrections define native occupancy only where applied; renderer cost, outer collar transitions and correction storage remain unmeasured. Not a continuous boundary guarantee.

## Recommended next comparison, no world refit

Use the same original two cubes, FBM occupancy and surface samples. Compare the original fit, native-size conforming six-tet P2, and one shared analytic CSG model spanning both cubes. Count actual encoded bytes, primitive candidates per ray, crossings, false air/solid, interior/collar error, face agreement and timed grazing rays. Match bytes where possible and expose extra primitive budgets explicitly. Verify CSG candidate/bin lookup against evaluation of its complete expression, including bins on either side of the old seam. Do not accept agreement alone: inspect connected components and narrow features, and compare the collar's outer edges. Expand to 2x2x2 only after this gate passes.

Recommendation: P2 is the dependable exact-continuity baseline. Shared quadric CSG is the most aligned next research experiment if keeping compact free-form primitives matters more than preserving independent cube-owned geometry. No claim that either is already optimal or faster on this machine.
