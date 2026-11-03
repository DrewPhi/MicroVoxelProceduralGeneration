# Full-world fitting audit, 2026-11-03

The envelope experiment was pushed as 57898e9. This audit does not modify the
terrain, fitter or active renderer model and submits no GPU/cluster job.

## Established observations

1. The completed full-world run made 15,000 updates, retained best_step=0,
   and exported its initialization. Saved validation: 12.2341% disagreement;
   final update was reported as 15.2864%. This is not evidence of an optimum.
2. The continuity basis is globally coupled. Per branch it is
   A(x,y)+B(x,z)+C(y,z)+U(x)+V(y)+W(z). An XY coefficient affects all Z;
   an X bubble coefficient affects an entire YZ slab. Each branch has zero
   alternating mixed XYZ corner difference. The minimum of branches need not
   have that property, so this is not an impossibility proof for the composite.
3. Full grid125x53x125 has828125patches. Five independent 10-coefficient
   quadratics per patch would have41,406,250coefficients. Current shared basis
   has148,935raw parameters (147,410independent scalar degrees across5branches).
   Per-branch scalar rank is sum((na+1)(nb+1),pairs)-2=29,482. For2x2x2 the
   scalar rank was25 versus80unconstrainedcoefficients. Scaling that exact C0
   construction globally makes the local-freedom restriction much more severe.
4. A nonzero total-degree2 polynomial cannot vanish on all6faces of a cube:
   divisibility by the6face linear factors would require degree>=6. Thus an
   arbitrary independent interior quadratic correction cannot preserve all
   existing face fields; neighbor changes/coupling are intrinsic to that choice.
   This does not rule out different geometry constraints, partitions or more pieces.
5. Pilot preflight used --initialize pilot-initial.json (already-fitted2x2x2
   model). It improved557/65536errors to553/65536. The full run instead used
   initialize_surface (height only). Preflight verified math/export/parity,
   not effective learning of a difficult3D target from a height initialization.

## Local authoritative probe

65,536native-center samples, seed77123, original1km region/recipe. Exported
step-zero shared parameters were evaluated locally onCPU, with4Torch threads.
Matched no-cave ablation uses identical indices; only cave_width changes to0.

- Original-target error8111/65536=12.3764%.
- No-cave-target error1143/65536=1.7441%.
-6968points were removed by the cave field; initialization predicts solid at
  all6968. These account for~86% of the probe's original-target errors.
- Scale all branch fields by a positive factor: occupancy remains identical.
  Current balanced_loss falls from31.3598 atscale1 to0.0005974 atscale0.001,
  while error stays12.3764%. A near-zero field can therefore produce a large
  loss reduction without a better boundary. This demonstrates an objective/
  initialization scale mismatch, not that the historical run followed this
  exact collapse path or that hinge loss can never learn a useful boundary.
- Initial gradient norm on8192probe records is0.14764 (nonzero). Negative
  argmin assignments are distributed across all5branches (7053,7149,7039,
  6774,6868). Completely disconnected gradients and total initial branch
  starvation are not supported by this probe; later collapse is unmeasured.
- Positive loss sums every branch but divides only by positive sample count;
  negative loss penalizes only the minimum branch. This makes relative class
  pressure change with branch count. It may be intentional constraint weighting,
  but must be controlled in1-vs5branch comparisons.

Saved old metrics establish failure but do not identify a unique root cause.
The remote per-step log could not be fetched because Bouchet DNS resolution
failed in this session. No new cluster job was attempted.

## Next controlled experiment

Use the same small3D crop and native-center train/validation points for:
1. A representable synthetic teacher, to verify the optimizer learns a known
   member of the current model family from the same height-style initialization.
2. ActualFBM with independent patch coefficients versus hard-C0 shared fields.
3. Both losses/weightings and field-scale-normalized initializations, logging
   occupancy error separately from hinge loss, gradients and branch utilization.
4. Increase2x2x2 to4x4x4/8x8x8 only after the smaller comparison is understood.

If the synthetic teacher fails, fix optimization first. If independent patches
fitFBM well and hard-C0 fields do not, change continuity/representation before
anotherworld-scale run. Preserve the analytic rendering interface and measure
memory/branch count. Do not assume longer training or anotherGPU fixes capacity.

Reproduce the cheap probe (using existing local world/export and Rust sampler):

```
python3 tools/gpu_fit/prepare.py --world output/world-1km/world.json \
  --output output/fitting-audit/probe \
  --sampler tools/gpu_fit/sampler/target/release/terrain-sampler \
  --samples 65536 --seed 77123
python3 tools/gpu_fit/audit_initialization.py \
  --dataset output/fitting-audit/probe \
  --parameters output/world-1km-initialized/shared-parameters.npz \
  --output output/fitting-audit/initial-audit.json
```

Optional --no-caves-dataset accepts a separately prepared identical-coordinate
probe with cave_width=0. Raw samples/logs remain ignored output artifacts.
