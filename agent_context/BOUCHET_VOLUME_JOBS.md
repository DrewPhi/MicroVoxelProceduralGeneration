# 1 km volumetric P2 + overlapping CSG jobs — submitted 2026-11-03

User explicitly requested both full 1km jobs rather than intermediate-region gates.
VPN restored; old SSH master expired. New interactive login/Duo push approved.
Control path `/tmp/bouchet-ssh-20261103-new-%C`, user dss227@bouchet.ycrc.yale.edu,
ControlPersist8h. Network/control-socket access requires escalation in this session.

## Submission

Remote directory:
`/nfs/roberts/scratch/pi_sk2433/dss227/voxels/analytic-world-1km-20261103-volume-v1`

- Preparation **31935788**, day,16CPU/32GB/2h, generating shared density nodes.
- P2 **31935789**, gpu,1GPU/8CPU/32GB/2h, afterok:31935788.
- CSG **31935790**, same resources, independently afterok:31935788.

Normal pi_sk2433 account, no paid priority partition. No unrelated jobs touched.
Last observed preparation PENDING(Priority); fits PENDING(Dependency).
Submission is journaled remotely and locally at
`output/bouchet-volume-20261103-v1/submission.json`. Do not duplicate jobs blindly.
Bundle SHA256 aa7201b72fd75283ca67e1d9db46773ec666002ef27a0a61c6238b50c6eb01c4.
Source snapshot is immutable once submitted; use a new deployment for changes.

## Scope and data

Same approved 1000x424x1000m ROI, 6.25cm native occupancy, 8m original cells,
125x53x125 cubes. All reused sample manifests match the world dictionary exactly;
local and remote JSON have different whitespace SHA hashes but identical JSON.
No exhaustive trillion-voxel volume is generated.

Reused read-only via symlinks:
- 16,777,216 native training centers from 20261102-gpu-v1/train-data.
- 524,288 authoritative projected surface points/normals from
  20261103-hermite-v1/geometric-data.
- 1,048,576 validation and separate1,048,576 test samples from hermite-v1.

New `node-sampler` writes6,741,107 globally shared density nodes (4m spacing),
26,964,428raw bytes. Values are authoritative density/8m, NOT signed distances.
Source/world/binary checksums live in shared-data/nodes.json.

## Models and fitting

P2: one total-degree2 field per Kuhn tetrahedron,6per8m cube; shared nodal values
ensure C0 traces.6,741,107 trainable scalar values. No tiny-cell superspline grid.

CSG: scalable adaptation of the pair experiment.828,125 groups of5general
quadrics (41,406,250 coefficients,165,625,000raw bytes), centered on8m cubes.
Each support box has16m edge, so groups overlap across cube boundaries. The
world is one union of box-bounded quadric intersections. Point queries gather
up to8groups. These are NOT independent clipped blocks and NOT ten quadrics
attempting to fit the whole world. Box planes are explicit analytic primitives,
with possible artificial planar geometry. A future renderer must test those
planes as well as roots and preserve Boolean dependencies. Mathematical solid
membership is globally consistent; source topology and smoothness are not assured.

Both initialize from authoritative source density nodes; CSG uses a constant
27sample/10coefficient least-squares map and small branch perturbations. GPU
training uses classification hinges, surface zeros and analytic-normal targets,
Adam,6000updates or30min budget, best sampled-validation selection. This full
runner uses hard Boolean assignments, not the pair script's initial soft-to-hard
curriculum. Therefore it is a new scalable optimization experiment, not evidence
of identical convergence to the local run. Best step0 is explicitly reported if
optimization cannot improve source initialization. Separate test data are only
scored for the final chosen model.

Checkpoints include current model/optimizer and best weights, periodically and on
USR1/TERM. `--resume` is provided. RNG state is not saved, so resume is not bitwise
deterministic. Jobs have no cross-model learning gate; each runs after data prep.

## Local verification before submission

- Python compile and shell syntax checks pass; vendored offline Rust sampler builds.
-45new node records bit-for-bit equal authoritative fbm_points after float32 scaling.
- P2 reproduces a global quadratic within1.8e-15.
- Both analytic gradients match independent finite differences.
- CSG neighborhood solid membership agrees with brute-force global group union.
- Both CPU end-to-end20step smoke fits train, validate, checkpoint, test and export.
- P2 smoke12,000shared-face samples:0occupancy disagreements; maxfloat32jump5.96e-8.

Smoke data use the original8x16x8m pair. This source-initialized scalable optimizer
has NOT achieved the previous small fitter's1.1/1.27% results:20step smoke tests
were5.59%P2 and7.47%CSG on8192test centers. They establish functioning pipelines,
not a quality gate or prediction for the full job. GPU self-tests repeat at job start.

## Outputs / next access

`result-p2/` and `result-csg/` each contain status.json,run.log,run-config.json,
latest.pt,best.pt(if improved),metrics.json,model.json,raw parameter.npy arrays,
top-down.png,center-slice.png,preview-heights.npy.

These new model.json files are NOT accepted by the existing viewer. They are
full-ROI fitted data for a later loader/evaluator; do not replace the launcher or
claim GPU rendering performance from training results. Current game unchanged.

Check `squeue -j31935788,31935789,31935790`, then `sacct` and the isolated logs.
Download small metrics/previews before transferring large checkpoints. Preparation
failure leaves fits blocked by afterok; investigate/fix rather than submit duplicates.

Code: tools/gpu_fit/volume_models.py,volume_fit.py,volume_prepare.py,
volume_{prepare,p2,csg}.sbatch,volume_run.sh,sampler/src/nodes.rs.
