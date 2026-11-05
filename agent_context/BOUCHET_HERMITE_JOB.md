# New 1 km Hermite fitting submission, 2026-11-03

User explicitly requested the full-world GPU job and a new SSH/Duo session.
Interactive SSH succeeded after user approved Duo Push. Reusable control path:
`/tmp/bouchet-ssh-20261103-%C`, user `dss227@bouchet.ycrc.yale.edu`.
Do not reuse the expired 20261102 socket. SSH master was left open.

Isolated deployment:
/nfs/roberts/scratch/pi_sk2433/dss227/voxels/analytic-world-1km-20261103-hermite-v1

Preflight job **31867725**, gpu_devel,1GPU/4CPU/16G/15min.
Full job **31867726**, gpu,1GPU/8CPU/32G/2h, dependency afterok:31867725.
Both use normal research account pi_sk2433. No unrelated jobs changed.
Local submission record: output/bouchet-hermite-20261103/submission.json.
Remote submission.json prevents duplicate submissions in the same deployment.

## Method and validation

Same shared min-of-five total-degree2 fields, exact C0 cube joins,8m patches,
6.25cm native occupancy,1000x424x1000m approved ROI. Same quadratic renderer
export format. The representation's global capacity restriction remains.

New sparse scalar design matrix encodes value and analytic derivative rows.
Five batched preconditioned-CG solves perform fixed-assignment Hermite least
squares without a dense world normal matrix. Outer active-branch assignments,
truncated signed nearest-sampled-surface distances, point/normal constraints,
inside-solid constraints and validation-best selection follow the local method.
Branches with fewer than4assigned surface samples remain anchored, matching the
local fitter's inactive-branch treatment. PCG tolerance1e-4; anchored ridge1e-8.

Full data:1,048,576native off-surface points,524,288surface points,1,048,576
validation and1,048,576independent test samples. Rust continuous field generates
surface points via bounded Newton projection (up to12steps,4m step cap, rejection
of failures/out-of-domain/degenerate normals); central differences give oriented
unit gradients. This is a scalable sampling adaptation of the local lattice
bisection dataset, not an exhaustive guarantee of all topology. Targets are
calibrated distances/gradients, not the old raw hinge objective.

Preflight cold-start cave crop: same16m box [5632,496,4800], no already-fitted warm
start. Requires <=8% sampled disagreement, at least50% relative improvement,
zero sampled seam sign disagreement and nonzero best step. Also CUDA sparse
solver vs dense reference and sparse design/derivative parity checks.

Local sparse CPU learning check improved44.806%->2.304%(beststep22) on the
mixed validation sampler; this is not directly comparable to the prior2.78%
uniform untouched test. Original geometry-gathered pilot gave5.76% before matching
inactive-branch treatment; final frozen-empty-branch implementation is submitted.
Design value/derivative parity and PCG dense check passed (max error~1.15e-5).

Full job:32outer iterations,180CGiterations each,75min fitting wall budget.
Checkpoint latest.pt includes best/current vectors and sample provenance; signal
handling checkpoints between solves/trials. Best checkpoint selected by validation;
test only evaluated at completion. Gate failure blocks the full job automatically.

Outputs: result/status.json,run.log,latest.pt,metrics.json,terrain.json,
terrain.coefficients.f32,shared-parameters.npz,center-crop.json and top-down preview.
The playable world/launcher are unchanged until results are inspected/downloaded.

Code: tools/gpu_fit/hermite.py, hermite_{preflight,world}.sbatch,
sampler/src/hermite.rs. Tar deployment SHA256:
1ea4bbf9f77016040f0c36982824e646dc85b4b9c4b43a3a07d2e95e8a23aaab.
Do not silently deploy later edits into a running job; use a new version/directory.
