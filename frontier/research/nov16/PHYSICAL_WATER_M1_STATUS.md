# CPU physical reference: partial M1 evidence, not accepted

The implementation is isolated under `world/src/physical_fluid/`. The current
playtest still uses CA water. No GPU physical solver or runtime promotion is
included. Read [the M0 numerical contract](PHYSICAL_WATER_M0.md) before interpreting
these results. All implementation is greenfield; only published methods were
consulted, with source links in that note.

## What is implemented

A bounded f64 MAC field with physical gravity, free-slip solid faces, atmospheric
pressure, matrix-free Jacobi-PCG, explicit gauges for disconnected enclosed
components, RK2 velocity advection and two-layer air velocity extension. The
geometric VOF path reconstructs mixed cells and transports plane/slab volumes with
rotating Weymouth–Yue sweeps; donor-cell transport is retained as a diffusion
negative control. Pressure is warm-started and its true residual checked.

Fractions retain full precision. At most 1e-12 fraction violations receive an
explicitly counted roundoff projection. Signed/absolute corrections and actual
volume drift are reported independently; nothing is globally renormalized.
The pressure support floor is 1e-9 by default and configurable. All trace volume
remains in the fraction array/checkpoint and is separately reported. It is not
silently deleted or rendered as an extra source.

`PhysicalFluid::step(dt)` either advances the entire interval or leaves the
original state untouched. Bounded adaptive retries never discard elapsed time.
NaN/configuration/pressure/CFL/bounds failures are explicit. `FVPHYS01` checkpoints
include all fractions, MAC faces, solid masks, warm pressure, physical parameters,
time, sweep phase and correction ledgers. Existing CA save formats are untouched.

## Reproduced evidence so far

Standalone optimized tests (15 active component tests plus an ignored offline gate reporter) cover hydrostatic pressure-depth, a manufactured
pressure-gradient system, disconnected enclosed gauges, gravity acceleration,
sharp translated/rotated volume versus donor diffusion, short dam motion, two-second
wall impact/rebound, communicating-vessel redistribution, short timestep
refinement, trace retention, exact CPU restart and atomic failure. These are
component/diagnostic checks, not a validated CFD benchmark suite.

Measured development probes (CPU timings were not isolated performance runs):

| Fixture | Result |
|---|---|
| Native 64×64×32 hydrostatic, 0.1 s | Volume unchanged at reported precision; peak sampled speed 1.19e-11 m/s; trace volume 1.14e-9 voxel volumes; zero wall leakage. About 3.41 s wall including diagnostic work, **offline**. |
| 16×12×4 dam, 2 s, before pressure warm start | Wall impact and centre-of-volume rebound observed numerically. Drift −4.72e-11 voxel volumes; absolute roundoff adjustment 9.90e-13 voxel volumes; zero wall leakage. |
| 24×16×4 communicating vessels, 2 s, before pressure warm start | Level difference decreases through the two-cell bottom channel. Drift −1.83e-10 voxel volumes; zero wall leakage. |
| 6×8×4 hydrostatic, 60 s, rejected pre-warm-start negative control | Volume drift 6.35e-10 voxel volumes, but **motion fails**: peak sampled speed reaches 0.211 m/s (0.0577 m/s at final sample) and vertical centre shifts about 1.5 mm. Conservation alone does not pass hydrostatics. |

The native short probe previously failed with spurious currents because all
positive ulp-sized fractions activated pressure topology. The explicit retained
trace-volume classification fixed that short case. A separate nearly axis-aligned
plane integration cancellation bug caused a 1.86 s dam bounds rejection; exact
piecewise-polynomial integration fixed that case. These failures motivated
regressions; neither is evidence of general correctness.

## Open gates

M1 remains **not accepted**. The warm-start 60-second small hydrostatic rerun
passes: peak speed 3.90e-11 m/s, peak centre shift 4.48e-12 m and peak drift
4.59e-9 voxel volumes; signed/absolute roundoff is independently reported.
No damping or increased support cutoff was applied. A native 64×64×32
60-second probe continues separately; completion is not claimed here. Current
pressure boundaries place atmospheric pressure at dry cell centres, and velocity
advection is dissipative rather than mass/momentum-consistent. Post-transport new
interface cells can have substantially higher divergence than the projected
pre-transport field; both measurements are reported separately.

A matched offline refinement ladder now uses the same 0.75×0.75×0.25 m dam:
12×12×4 at native h=0.0625, 24×24×8 at h/2 and 48×48×16 at h/4. These refined
numerical grids are reference experiments, not changes to gameplay voxel size.
At 0.25 s the native 99%-volume front is 4.43% from h/4; h/2 is 1.01% from h/4.
However kinetic energy is 19.33% low at native and 6.87% low at h/2 relative to
h/4. Reference energy uncertainty is therefore still material; the physical
motion gate cannot pass from the front alone. There is no converged wave-period
reference yet. Raw samples, parameters and comparisons are preserved under
[physical-water-baseline](physical-water-baseline/metrics.json). There are no inspected 3D physical clips,
validated obstacle wake, physical terrain-edit coupling or streaming-boundary
model. The 60-second closed-volume and hydrostatic-motion gates are demonstrated only
on the small hydrostatic probe; dynamic acceptance remains open.
GPU parity, performance, moving residency, faithful distant liquid edits and
integrated physical pickup remain later gates. Issue 88 is not complete.

## Commands and artifacts

```sh
source /home/drew/Projects/voxels/.toolchains/env.sh
rustc --edition 2021 -O --test frontier/world/src/physical_fluid/mod.rs -o /tmp/physical-tests
/tmp/physical-tests
rustc --edition 2021 -O frontier/world/examples/fluid_physical_probe.rs -o /tmp/physical-probe
/tmp/physical-probe --fixture hydrostatic --seconds 0.1 --sample 0.01
/tmp/physical-probe --fixture dam --nx 16 --ny 12 --nz 4 --seconds 2 --sample 0.01
/tmp/physical-probe --fixture vessels --nx 24 --ny 16 --nz 4 --seconds 2 --sample 0.01
/tmp/physical-probe --fixture dam --nx 16 --ny 12 --nz 4 --seconds 2 --sample 0.025 --frames output/nov16-physical-cpu/dam
```

`--frames` writes a whole-cell 0.5-threshold side-view PPM and complete `.fvphys`
checkpoint for each output sample. The visual threshold never changes simulation
volume. It is a diagnostic occupancy policy, not an approved runtime policy.
These snapshots can also feed an independent CPU voxel renderer. `--checkpoint`
writes the final restart. JSONL reports volume, correction ledger, trace volume,
centre of volume, kinetic energy, front, full/interface divergence, leakage,
substeps, pressure residual/iterations and CPU wall time. The probe has no render
frame-rate input; output-time/event schedules must match for exact replay.

A subsequent numerical pass should first instrument pressure-topology changes
and examine a well-balanced gravity/pressure formulation plus consistent
subcell free-surface distances. Constant gravity is a potential gradient;
subtracting nearly cancelling force and pressure updates must not amplify
fraction roundoff into a different pressure domain. The long small-domain hydrostatic failure was resolved by warm-starting the
actual pressure solve and enforcing its true residual. Changing a support cutoff
or applying undocumented damping is not an acceptance fix. After hydrostatics,
compare mass/momentum-consistent transport against the current dissipative RK2
velocity path using wall rebound and wave-energy/refinement measurements.

## Frozen baseline reproduction and next candidate

```sh
/tmp/physical-probe --fixture dam --nx 12 --ny 12 --nz 4 --seconds 1 --dt 0.0025 --sample 0.025 --frames frontier/output/nov16/physical-dam
/tmp/physical-probe --fixture dam --nx 24 --ny 24 --nz 8 --spacing 0.03125 --seconds 0.5 --dt 0.00125 --sample 0.025 --frames frontier/output/nov16/physical-dam-fine
/tmp/physical-probe --fixture dam --nx 48 --ny 48 --nz 16 --spacing 0.015625 --seconds 0.25 --dt 0.000625 --sample 0.025
/tmp/physical-tests --ignored --nocapture
```

The output scheduler divides each sample interval into equal bounded substeps;
it does not leave an artificial near-zero last step from floating-point addition.
All requested simulation time advances. Base snapshots cover t=0..1 s at 0.025 s,
fine snapshots t=0..0.5 s. The stable FVPHYS01 layout supports independent
isometric voxel rendering with `tools/render_physical_water.py`.

The next controlled candidate will test a PLIC-derived ghost-fluid pressure
interface against this centre-pressure baseline, with consistent theta factors in
both matrix and gradient as described in Bridson section 4.5.1. It is not part of
this frozen baseline, and no accuracy improvement is presumed.
