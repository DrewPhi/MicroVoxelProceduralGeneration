# Bouchet GPU constrained fitter

This keeps the prior min-of-five quadratics per 8m cube and exact C0 shared-face
constraints. It does not use the rejected 24-tetrahedron spline reconstruction.

## Scalable constraint representation

Each scalar branch is XY bilinear grid + XZ bilinear grid + YZ bilinear grid,
plus one-dimensional `t*(1-t)` quadratic bubbles on each axis. On each cube this
is total degree two; every neighboring face restriction agrees automatically.
On 2x2x2 cubes it spans the same rank-25 space as the prior dense nullspace.
The implementation has redundant gauges but no global dense constraint matrix.
Training jointly changes the shared parameters on the entire region.

`model.py` tests the span, gradients, all-axis joins and baked/direct evaluation.
`prepare.py` calls the portable Rust sampler. Its generated terrain module is
identical to `src/terrain.rs` except for removal of serde derives/imports. The
three source dependencies are vendored with their upstream license notices for
offline builds on cluster compute nodes. Do not change that generated module
without regenerating it and rerunning source parity against `fbm_points`.

The sampler streams bounded batches: two-thirds uniform volume and one-third
near the base surface, at exact native voxel centers. It never materializes the
1.7 trillion-cell world. Validation uses independent RNG seeds; results are
sampled estimates, not exhaustive certification. Training includes a hard-example
pool drawn only from training samples. Best checkpoints use validation error.

## Jobs

Submit from a fresh scratch deployment containing `gpu_fit/`, `world.json` and
`logs/`. The supplied scripts use account `pi_sk2433`. Preflight requests one GPU
in `gpu_devel` for 15 minutes. It builds the Rust sampler offline, fits the old
small group with CUDA, checks source parity and GPU/CPU math, and writes
`preflight-passed.json`. The world job must depend on its success:

```sh
check_id=$(sbatch --parsable gpu_fit/preflight.sbatch)
sbatch --dependency=afterok:"$check_id" gpu_fit/world.sbatch
```

The world script uses the general GPU partition and requests one GPU, 8 CPUs and
32GiB host RAM for two hours. Optimization stops after at most 75 minutes, or
earlier on validation plateau. It samples 16,777,216 training points and 1,048,576
validation points. No H100-specific kernel or silent CPU fallback is used.
Slurm queue estimates can be long and are not training-time estimates.

## Checkpoints and outputs

`result/status.json`, `run.log`: progress and validation. `latest.pt`: optimizer,
model, RNG and step for `--resume`. `best.pt`: best validation checkpoint.
SIGUSR1/SIGTERM request a checkpoint; interruption exits75 rather than claiming
completion. A failed preflight blocks the dependent world job.

At successful completion:

- `metrics.json`: independent validation, seam sampling and provenance.
- `preview-top-down.png`, `preview-heights.npy`: sampled fitted surface diagnostic.
- `shared-parameters.npz`: compact fitted plane/bubble parameters.
- `terrain.json`, `terrain.coefficients.f32`: baked quadratics in Y,Z,X,branch
  order; 12 float32 values per quad (10 coefficients, conservative margin, pad).
  File SHA256 and exact byte count are recorded in the header. The current old
  viewer needs a binary/large-scene loader before opening this full header.
- `center-crop.json`: old nested-JSON format for immediate viewer inspection.

The GPU shader still solves quadratic intersections. This code changes the
fitting parameterization, not the terrain definition or rendering geometry.
Global fixed-label constraints may still limit accuracy: inspect metrics and
previews before replacing the old demo. No claim of fixed five-quad capacity
being sufficient everywhere, or preserved local-test error on the 1km region.

## Full-world fitting diagnosis

The completed world job retained initialization; it was not an improved fit.
See [the fitting audit](../../agent_context/FITTING_AUDIT.md) for measured local
probes: global continuity coupling, a field-scale/hinge-loss mismatch, and the
pilot's different warm start. `audit_initialization.py` inspects an exported
shared-parameter model on a prepared dataset without training or modifying it.
These are evidence and hypotheses to separate with small controlled ablations,
not a claim that the existing result is an optimum.

`local_hermite_experiment.py` runs a CPU-only, bounded cave-crop ablation with
original hinge, shared/independent Hermite quadratics and continuous local P2.
See [setup, results and limitations](../../agent_context/LOCAL_HERMITE_EXPERIMENT.md).
It writes separate models/previews and never replaces the launcher world.
The local ablation requires NumPy, SciPy, Pillow and PyTorch (CPU is sufficient).

## Scalable Hermite GPU job

`hermite.py` builds a scalar sparse design for shared quadratic values and
analytic derivatives, then solves five weighted least-squares systems with
batched preconditioned conjugate gradients. It uses calibrated surface/normal
constraints and signed distances to sampled surfaces, with outer branch
assignment updates. A dense world-scale normal matrix is never allocated.
The continuity space and rendering format stay the same; global capacity limits
still apply. Unlike the earlier preflight, this gate must learn a cave crop from
a height initialization before the dependent full job can start.

`sampler/src/hermite.rs` streams bounded authoritative off-surface records and
projected surface points with normals. Geometry sampling is approximate; it
rejects failed projections and cannot certify exhaustive coverage of thin
features. Training and independent native-center validation/test sets are kept
separate. See [submission and method details](../../agent_context/BOUCHET_HERMITE_JOB.md).

Submit from an isolated deployment with gpu_fit/,world.json,pilot-world.json:

```sh
check_id=$(sbatch --parsable gpu_fit/hermite_preflight.sbatch)
sbatch --dependency=afterok:"$check_id" gpu_fit/hermite_world.sbatch
```

Do not repeat that command if jobs already exist; inspect submission.json and
scheduler status first. Hermite jobs write result/ and leave existing worlds
untouched. A failed GPU gate leaves the full job blocked rather than running
an unverified large fit.
