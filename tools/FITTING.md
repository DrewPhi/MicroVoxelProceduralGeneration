# First fitting experiment

Run from the repository root after `cargo build --release --bin fbm_samples`:

```sh
python3 tools/fit.py --recipe output/fit-first/input-recipe.json \
  --output output/fit-first --n 5 --patches 4 --span 32
```

Requirements: NumPy and Pillow. BLAS thread count is capped at one; Rust export
uses Rayon (set RAYON_NUM_THREADS to cap it). No PyTorch/autograd required.

The Rust exporter samples the authoritative density at every native center in
a bounded cube and stores a little-endian f32 volume, C-order [Y,Z,X]. Python
uses that exact target. Every patch fits five general implicit quadratics using
local [-1,1] coordinates, hard-min ownership, regularized least squares, damped
updates accepted only when the actual training loss decreases, and four starts.
Uniform + near-boundary training samples are used. The final native occupancy
validation is exhaustive within the exported cube. Sampling-validation statistics
are advisory; exact native disagreement is authoritative. No perfect-fit or
global-optimum claim is made, and n does not grow automatically in this run.

Watch `run.log`, `status.json` and incremental `patch_metrics.json`. Checkpoints
have zero coefficients in untrained patches; consult completed count. The final
`terrain.json` includes only a fully trained model with bounds/normalization,
coefficient order, density sign and original recipe; this is the Rust renderer
interchange format. Coefficient JSON is float64; conversion to f32 needs separate
validation. `coefficients.npy` is the same array in NumPy form.

Completed outputs: `metrics.json`, `comparison-surface.png`,
`comparison-slice.png`, `comparison-isometric.png`, individual `fbm-*` / `fit-*`
images, packed occupancy (`*.bits`, LSB-first X/Z/Y), and `index.html` gallery.
Original is left; fitted is right. Both isometric views use the same exact-cell
reference, not the future analytic surface intersection. Height previews show
the highest solid voxel in this cropped cube and cannot reveal all cave errors.
The slice and exhaustive voxel metrics cover interior differences. Preview
coloring is only a comparison aid and is not a material fit.

Limitations: independent patches can disagree at shared boundaries; boundary
continuity is not enforced in this baseline. The goal is to measure capacity and
fitting quality before adding constraints. Density is not a signed distance, so
density RMSE is not geometric distance. Raw byte occupancy and density files are
ignored scratch fixtures; fitted coefficient bytes are reported separately.

## Occupancy-first run

`--occupancy` switches to hard-min sign-margin fitting. Every restart candidate
is ranked by exhaustive native solid/air disagreement; density regression is no
longer the selection objective. Misclassified native centers are added to the
training set (bounded to 32768 points). Stop immediately on a certified zero-error
patch, or report `plateau_not_exact` after `--patience` unsuccessful restarts and
`--min-restarts`. Inner updates stop after 64 iterations without a training
classification improvement, even if the density surrogate keeps changing.

`--target-dir` reuses the exact Rust fixture after recipe validation. `--baseline`
retains original sample sequences and coefficients; `--warm-checkpoint` supplies
additional completed candidates from a stopped run. All patches are rechecked
because their previous density objectives differ. Completed occupancy patches
are never selected with more native disagreements than their checked warm starts.
Zero sample error alone is never labeled an exact native match. The tiny full
pipeline smoke fixture achieved zero disagreements; this is not evidence that
five quadratics suffice for the main terrain. Main run: output/fit-occupancy/.
