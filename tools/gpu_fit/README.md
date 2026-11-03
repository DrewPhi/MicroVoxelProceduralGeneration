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
