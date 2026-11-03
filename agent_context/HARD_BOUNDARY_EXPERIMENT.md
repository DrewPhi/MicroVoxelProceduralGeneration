# Next experiment: compact quadrics with an exact shared face

Status: research/proposal only. No new training job launched by this review.

## Relevant literature and its actual scope

- Garrity & Warren, *Geometric continuity*, CAGD 8 (1991), 51–65. https://doi.org/10.1016/0167-8396(91)90049-H . Publisher abstract and author publication summary support implicit geometric continuity via rescaling. This is a general algebraic framework, not an off-the-shelf min-of-five voxel fitter. Do not claim it solves our optimization or branch matching.
- Guo, *Representation of arbitrary shapes using implicit quadrics*, The Visual Computer 9 (1993), 267–277. https://doi.org/10.1007/BF01908449 . Local smooth piecewise-quadric construction on polyhedral facets requires compatibility conditions. Useful warning against promising arbitrary shape fidelity plus low degree plus unchanged primitive count plus smooth joins. Publisher abstract reviewed; do not imply access to all proofs.
- Bindel, Cornell CS 6210, *Constraints, sparsity, and projectors*, 2025-10-08. https://www.cs.cornell.edu/courses/cs6210/2025fa/lec/2025-10-08.html . Direct variable elimination/null-space methods enforce linear equalities without ill-conditioned huge penalty weights; dense null-space bases can destroy sparsity. Full notes read. Use a small explicit parameterization for our first local test, not a dense global null-space matrix.
- Peters, *Geometric Continuity*, handbook chapter, 2001. https://www.cise.ufl.edu/research/SurfLab/papers/01handbook.pdf . General distinction between geometric matching and parameter/field matching. Relevant follow-up source, not evidence that min-of-quadrics is already solved.

Our recommendation is a specialization assembled from standard polynomial identities and exact equality parameterization, not a newly found published end-to-end algorithm.

## Geometry and exact parameterization

Keep two original 8m cubes, five full 3D quadratics each, native voxel edge 0.0625m. Problem pair YZX [0,0,3] and [1,0,3]; shared physical face is local y_A=+1, y_B=-1. x,z tangential coordinates agree.

For each corresponding branch j define:

Q_Aj(x,y_A,z) = T_j(x,z) + (y_A - 1) L_Aj(x,y_A,z)
Q_Bj(x,y_B,z) = T_j(x,z) + (y_B + 1) L_Bj(x,y_B,z)

T is a general 2D total-degree-two polynomial (6 parameters), L_A and L_B are affine in three coordinates (4 parameters each). Products remain total degree two. Both restrictions equal T everywhere on the face. Min_j Q_Aj = Min_j Q_Bj there; solid iff all branches >0 therefore also matches. This is complete for a matched pair with equal face restrictions, not an arbitrary function approximator.

Positive proportionality of branch face restrictions would also preserve boundary signs. For this single-face occupancy experiment, each B branch can be divided by its positive proportionality factor without changing the patch occupancy, so equality is a reasonable gauge choice. Do not add a redundant nonlinear scaling experiment initially. Multi-face scale compatibility is a later issue. Never allow negative branch rescaling.

A polynomial vanishing on a plane factors by its linear plane equation, which gives this representation. There is no additional stored field, blend evaluation, residual voxel array, subdivision or LOD. Export back into the same ten monomial coefficients per quadric: 100 coefficients total, 480 B in current padded GPU format. Fitting has 5*(6+4+4)=70 free parameters rather than 100. This preserves storage/render representation, not unconstrained expressivity or fit accuracy. Start C0 only; no gradient/G1 promise. Exclude collapsed/degenerate solutions through occupancy margins, initialization and validation.

## Branch correspondence and optimization

1. Read baseline coefficients and native Rust density fixture; preserve all originals. Existing exact native baseline: 29,198 errors / 4,194,304 centers = 0.6961346%. Shared face: 1,027 disagreements /16,384 points.
2. Restrict each of five quadrics to its face, normalize only by positive factors, and enumerate all 5! =120 pairings. Warm-project each pairing into the exact parameterization. Rank projected candidates on a fixed native holdout and boundary target accuracy, not raw independent density amplitudes. Keep at most three candidates for iterative fitting. Enumeration is not 120 full training runs.
3. Fit sign margins/occupancy, following existing alternating hard-min branch assignment machinery or direct gradients through the active branches. Positive samples require all branches positive; negative samples require at least one negative branch. Each update changes shared parameters, so it cannot violate the equality. This joint fit remains nonconvex because owners change.
4. Use sampled interior, narrow-band and boundary-adjacent native centers; add misclassified counterexamples. Use the Rust fbm_points exporter for true shared-face labels rather than averaging neighboring density samples. Fix validation samples and seeds. Early-stop plateau; cap total local optimization at three minutes, plus final exhaustive validation. No cluster allocation for this pilot.
5. Bake back to old coefficients and validate algebraic face restrictions (six coefficients per matched pair), including normalized residual and rank. Check float32 export separately: native classification, boundary signs, CPU/GPU hits and before/after images. Floating-point arithmetic is not a formal exact-arithmetic certificate.

## Go/no-go and required outputs

- Shared face polynomial equality by construction; zero measured boundary sign mismatches after f32 export. Report true boundary occupancy against FBM too, to reject matching-but-wrong surfaces.
- Fixed n=5 per patch, fixed two original patches, same renderer and coefficient payload. No adaptive patch growth or extra quadrics in the first test.
- Native error preferred <= original 0.696%; practical exploratory target <=0.80% (a target, not a guarantee). Report false air/solid and a surface-band error separately; total accuracy alone can hide lost thin features.
- Check all incident outer faces against fixed original neighbors and report regressions explicitly. Only the central face is hard constrained; this two-patch experiment does not claim a globally seamless full world. Use the isolated cropped pair for visual comparison.
- Log branch pairing, parameter/constraint rank, sample seeds, time, native errors, six-coefficient residuals, f32 error metrics and images. Export experimental terrain.json separately; do not replace default world automatically.

## Why a larger compatibility test is mandatory afterward

Numerical rank audit performed locally on 2026-11-02 for exact per-branch equality on every internal face, with consistent fixed branch labels on a uniform cube grid:

- 1x2x1: 20 unconstrained coefficients, constraint rank 6, 14 free.
- 2x2x2: 80 coefficients, rank 55, 25 free.
- 3x3x3: 270 coefficients, rank 224, 46 free.

Computed by restricting the 10-term monomial basis to each signed local face and taking the assembled matrix rank (NumPy). Counts are for one branch and this specific matching model, not a theorem about all possible min-envelope representations. They demonstrate that dense face equalities can be highly restrictive. A promising two-patch fit must be followed by a small 2x2x2 group and a constraint-rank audit before any whole-world/HPC run. If it fails, distinguish insufficient capacity, bad branch pairing, optimizer failure, and overly strong all-branch matching. Do not assume that increasing iterations fixes structural limitations.
