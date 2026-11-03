# Issue 20: conservative quadratic envelopes (implementation plan)

Scope: optional acceleration over the existing rendered quadratics. No terrain
refit, new surface, LOD, or change to the source occupancy. Keep grid and all-air
hierarchy as baselines. A performance regression is a valid experiment result.

## Field and voxel margin

In a patch's normalized cube, solid means F(v)=min_j Q_j(v)>0. Choosing any one
branch gives F(v)<=Q_j(v), without requiring branch identities between patches.
For exact voxels, the current gradient-based margin M_j satisfies
Q_j(center)<=Q_j(point)+M_j for a point inside the same grid-aligned voxel.
Thus a leaf envelope Q_j+M_j bounds the relevant continuous and sampled solid
field. Choose the branch with the lowest conservative cube maximum. For the
experimental spline representation, instead bound every piece with a constant
cube maximum; never apply the min-branch argument to piecewise spline pieces.

## Parent construction and proof

Write a quadratic with basis [1,x,y,z,xx,yy,zz,xy,xz,yz]. Reparameterizing
Q(a*u+b) preserves this degree. For octant k, child coordinates satisfy
v=2*u+b_k, b_k in {+1,-1}^3. Express child envelopes in parent coordinates and
average coefficients to obtain an inexpensive candidate P(u). This is a proposal,
not a least-squares fit or a sampled certificate.

For a residual R(v)=child(v)-P(v/2-b_k/2), define
B(R)=r0+|rx|+|ry|+|rz|+max(rxx,0)+max(ryy,0)+max(rzz,0)
     +|rxy|+|rxz|+|ryz|.
For all v in [-1,1]^3, R(v)<=B(R). Set delta=max_k B(R_k), and
U_parent(u)=P(u)+delta. Then U_parent dominates every child everywhere in its
subcube. Induction gives a bound over all descendant source patches. Missing
children outside non-power-of-two scene dimensions are not included; logical
parent coordinates still use the full padded cube.

Build in f64. On GPU packing, raise the constant by the sum of coefficient
rounding errors (each monomial has magnitude <=1 on the cube), then add
conservative roundoff padding. Runtime uncertain signs/tangencies fall back to
the existing traversal. This engineering padding is not a formal verification
of every GPU floating-point operation; random/adversarial CPU/GPU tests remain
required and the feature stays experimental.

## Ray intervals

Substitute u(t)=u0+t*v into U: A*t^2+B*t+C. Clip t to the parent box exit.
Only skip a prefix when U is strictly negative there. A stable quadratic solve
finds the first possibly nonnegative point; linear and tangent/uncertain cases
fall back conservatively. Back off from roots and verify negativity over the
whole proposed interval using endpoints and the interior maximum (when A<0).
Already-solid/possibly-solid starts never skip that interval. This handles
multiple roots by keeping the earliest possible nonnegative interval, not just
choosing an arbitrary root. Resume exact patch/voxel tests at the skip end.

Store envelopes only for parent levels >=1, aligned with existing air descriptors.
One padded 48-byte quadratic per parent; no GPU leaf copies. Additional temporary
CPU storage is allowed during construction and measured. A single composite
upper envelope avoids storing five full parent hierarchies.

## Verification and performance gates

- Affine transformation identity; parent residual domination for random quadrics,
  cube corners/edges, degenerate/linear forms and sampled interior points.
- Odd-sized hierarchies and piecewise source fields; voxel-center margin included.
- CPU interval negativity tests, including linear/constant/tangent/two-root cases.
- GPU/CPU hits and baseline-image comparisons for close, horizon, grazing,
  axis-aligned, inside-solid and boundary starts; continuous and exact voxels.
- Identical-camera 1x/2x/4x warmed GPU comparisons, construction time and bytes.
- Menu: GRID BASELINE / AIR HIERARCHY / QUADRATIC ENVELOPES / hardware unavailable.

References (guidance, not claims this is the published algorithm):
- https://www.pbr-book.org/4ed/Shapes/Managing_Rounding_Error
- https://pure.mpg.de/pubman/item/item_1325622_5/component/file_3590464/i3d08.pdf

## Implemented prototype and measured result (2026-11-03)

- `src/analytic/envelopes.rs`: transforms, parent residual bounds, float packing.
- `src/analytic/raycast.wgsl`: clipped analytic prefix solve + interval maximum
  verification. All-air skipping retained. Mixed-node envelope queries limited
  to levels 2–4 (32/64/128 m cubes at the current 8 m patch edge). Jumps shorter
  than twice the current cell size fall back. A ray queries envelopes at most
  once per source patch, then uses the original intersection path within it.
- Only the first negative prefix is skipped. Later negative intervals are
  reconsidered as traversal enters later regions. No interval enumeration or
  geometric approximation; possible-solid intervals still use source geometry.
- An initial unbounded version performed poorly (many repeated parent queries
  and tiny jumps); the bounded/cached implementation is the selectable version.

Build: 5,932,032 bytes of parent GPU coefficients, usually ~0.12–0.23 s for
combined air/envelope construction on this laptop. Temporary CPU leaf polynomials
are about 66 MB; only one construction level plus its parents is kept at once.
Bounds are built once at scene load, including for baseline modes so menu changes
are immediate. No per-frame fitting, regeneration or terrain edits involved.

Verification:
- 14 analytic unit tests pass, including affine identities, parent domination,
  coefficient packing and voxel-center bounds at all levels of an odd-sized tree.
- `python3 tools/check_quadratic_envelopes.py`: 36 GPU runs across 6 poses,
  continuous/exact voxels and all 3 methods. 2,880 sparse CPU reference rays,
  zero mismatches; paired full images differ by at most 2 byte levels per channel.
- `python3 tools/benchmark_envelopes.py`: near/level/long views, 1/2/4 spatial
  samples plus a continuous long view. Full image checks passed. Private GUI
  menu selection inspected. Raw reports/captures: output/envelopes/.

Representative 960x540 warmed GPU timestamps (ms; individual observations, not
percentiles; Radeon 760M):

| View / samples | Grid | Air hierarchy | Quadratic envelopes |
|---|---:|---:|---:|
| Near, exact voxels / 1 | 1.90 | 2.06 | 2.85 |
| Level, exact voxels / 1 | 3.45 | 3.79 | 5.12 |
| Long, exact voxels / 1 | 7.52 | 5.41 | 6.84 |
| Long, exact voxels / 2 | 15.16 | 10.98 | 13.72 |
| Long, exact voxels / 4 | 30.31 | 21.68 | 27.29 |
| Long, continuous / 1 | 7.14 | 4.38 | 5.64 |

For the long 1-sample voxel view, source quadratic tests/ray drop from 29.21
(air hierarchy) to 25.53, but ~24.73 parent-envelope tests/ray outweigh those
savings. Geometry agrees; this prototype is not a performance win over the air
hierarchy on these scenes. Keep the existing default. The next research question
is whether substantially tighter bounds or cheaper query scheduling can exclude
enough mixed regions to pay for extra evaluation; do not claim speedup from
lower source-test counts alone.
