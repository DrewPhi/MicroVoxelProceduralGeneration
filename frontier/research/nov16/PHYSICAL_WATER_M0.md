# Issue 88: CPU numerical reference contract

Status: M0 implementation contract frozen for an **experimental CPU reference**;
M1 physical/visual acceptance and later milestones remain open. Baseline CA stays
available. Greenfield implementation, no copied external source. Native spacing
is 0.0625 m; the initial requested tank is 64×64×32 cells (4×4×2 m).

## State and integration order

Dense f64 structure-of-arrays: cell liquid fractions and atmospheric gauge
pressures; three separately sized, uniquely owned MAC face arrays; immutable
voxel-aligned solid mask during a step. Units are m, s, m/s, kg/m³ and Pa.
Default density is 1000 kg/m³, acceleration [0,-9.81,0] m/s². Viscosity,
surface tension, trapped-air pressure and air inertia are omitted explicitly.
Stationary solids impose free-slip, zero-normal-velocity faces. All outer faces
are sealed; this is a fixed basin experiment, not moving ocean residency.

A requested elapsed interval advances in deterministic CFL-limited substeps:
1. RK2 backtrace of staggered velocity with trilinear interpolation of the old
   field; this is dissipative velocity advection, **not conservative momentum**.
2. Add gravity to faces touching liquid, refresh zero solid/boundary faces.
3. Matrix-free Jacobi-preconditioned CG solves the positive discrete negative
   Laplacian on cells with F>pressure_fraction_floor (default 1e-9); atmospheric p=0 in dry cells. Every disconnected
   fully enclosed component pins one gauge cell. Correct the shared face field
   using the matching pressure gradient and measure the actual residual/divergence.
4. Transport F with those corrected faces. Default: dimensionally split geometric
   VOF, mixed-cell plane reconstruction from fraction gradients, intercept by
   bisection of analytical plane-box volume, donor slab volume flux. A frozen
   beginning-of-substep F>0.5 indicator supplies the Weymouth–Yue directional
   compression term; rotate the sweep order each substep. Each internal flux has
   equal/opposite owners. Divergence/compression and floating-point roundoff are
   measured volume error, never concealed by refill or global renormalization.
   A donor-cell flux option remains a diffusive negative control.
5. Retain the just-computed liquid-adjacent velocities. Before the next RK2
   backtrace, extend each component two face layers into open air with synchronous
   nearest-layer averaging. This creates no liquid and does not couple air inertia.
   Pressure classification refreshes next substep. Surface momentum accuracy is
   still an explicit open gate; extrapolation is not conservative momentum.

Projection currently locates atmospheric pressure at dry cell centres, a
first-order stair-step free surface. It does **not** yet implement a validated
subcell ghost-fluid distance treatment; pressure and VOF geometry are not claimed
identical to Basilisk. Inertial wall impact and energy loss must be measured.
No promotion is permitted based on a conserved but overdamped animation.

Any nonfinite state, failed pressure solve, fraction outside [−1e-12,1+1e-12], or
unresolved CFL constraint rejects the entire requested interval atomically.
Adaptive retry halves dt for transport bounds/CFL failures within a fixed work
budget; it never clips velocity or discards requested time. Fraction violations of at most
1e-12 receive an explicit roundoff-only bound projection; signed and absolute
voxel-volume corrections are checkpointed and reported independently of actual
volume drift. Larger violations reject. This exception avoids unfixable ulp
failures on repeated hydrostatic projection; it is not advertised as exact
conservation, and cumulative correction remains part of acceptance evidence.
The pressure-topology floor is a declared numerical approximation: trace
fractions below it remain in the conserved state/checkpoint and in volume/COM
statistics, but do not activate pressure or gravity/velocity support. Their total
voxel volume is reported separately; they may be advected by supported neighboring
faces. This avoids promoting ulp-sized transport remnants into pressure components.
Finite isolated sub-threshold droplets are consequently not physically resolved.
The configurable floor and all correction ledgers survive checkpoint restart.
The API reports the failure. Diagnostics include time advanced, substeps, PCG
iterations/true residual, full/interface divergence, boundary leakage, volume
and voxel-volume drift, centre of volume and kinetic-energy estimate.

## References and provenance

[Bridson & Müller-Fischer, SIGGRAPH 2007 course notes, chapters 2–4](https://www.cs.ubc.ca/~rbridson/fluidsimulation/fluids_notes.pdf)
guide staggered operators, pressure boundary conditions and projection. The
Jacobi preconditioner is deliberately simpler than the notes' MIC variant.
The implementation is written here, not translated from the example solver.

[Basilisk's VOF method documentation](https://basilisk.fr/src/vof.h) motivates
plane reconstruction, geometric donor slabs, rotating directional sweeps,
frozen compression indicator and the scheme-specific CFL≤0.5 restriction.
[Weymouth & Yue (2010)](https://repository.tudelft.nl/record/uuid%3Ad030e6e1-9153-4f27-be72-dc987ac208d6)
is the primary method reference. Our gradient normal estimate, bisection geometry,
dense storage and pressure coupling are separate implementations; no equivalence
or transferred accuracy/performance claim is made.

## Acceptance and reproduction

Run standalone Rust tests using the pinned toolchain, without shared Cargo targets:

```sh
source /home/drew/Projects/voxels/.toolchains/env.sh
rustc --edition 2021 --test frontier/world/src/physical_fluid/mod.rs -o /tmp/physical-tests
/tmp/physical-tests
rustc --edition 2021 -O frontier/world/examples/fluid_physical_probe.rs -o /tmp/physical-probe
/tmp/physical-probe --fixture hydrostatic --seconds 1
```

The probe defaults to 64×64×32; --dt sets integration interval independently
of --sample output timing. --spacing is permitted only for explicitly labeled
offline numerical refinement, never a gameplay voxel-size change. It reports CPU wall time and simulated/wall ratio,
and accepts explicit smaller diagnostic dimensions. It is offline, not a
real-time claim. Matching simulation-time requests and checkpoints define replay;
render frame rate is absent from the reference API. Checkpoints retain all F,
faces, solids, physical configuration, time and sweep phase in a new independent
version; they do not overwrite or silently import CA saves.

Predeclared unit gates: pressure true relative L2 residual ≤1e-8 (absolute floor
1e-12); dt max|div u| ≤1e-6 on the tested small full-liquid fixtures; closed-box
volume drift ≤1e-10 voxel volumes for exact hydrostatics and ≤1e-6 voxel volumes
for short dynamic fixtures. These short tests **do not establish** issue 88's
60-second 0.01% volume gate. Hydrostatic pressure-depth, free-fall acceleration,
enclosed/disconnected gauge, pure transport, dam-front/wall rebound,
communicating-vessel and checkpoint tests must be assessed separately.
The M1 physical gate requires refinement/time-step convergence, a selected finer
offline reference with uncertainty, dam/wave metrics within the proposed 5%,
and inspected offline voxel motion. Record failures honestly; GPU work and
runtime promotion remain gated on that evidence.

## Opt-in ghost-fluid candidate after baseline d806e68

`BoundaryMode::Center` preserves the frozen baseline/default. `Ghost` derives a
signed cell-centre distance from the mixed-cell PLIC plane; full/empty cells use
−/+half-cell distance. Pressure centres have phi<0 (equivalently F>0.5). All
outside-centre liquid remains in F, transport, checkpoints and mass statistics;
its volume is reported as `unpressurized_volume_voxels`. Isolated fractional
liquid without a negative-phi centre is consequently unresolved dynamically.

For a liquid/air pair use theta=−phi_liquid/(phi_air−phi_liquid). Both the
pressure diagonal and the face gradient multiply by 1/theta. The explicit theta
floor is 1e-6, following the numerical safeguard discussed in Bridson section
4.5.1; it changes no liquid volume. Two-layer velocity extension after projection
supplies the unpressurized transport band. This does not make the air band
globally divergence-free: volume/bounds and interface divergence remain gates.
The force/projection/VOF splitting is first order in time; RK2 refers only to
velocity backtracing, not a second-order complete liquid integrator.

The FVPHYS01 byte layout is unchanged. Header word6 is now an explicit flags
field: bit0 geometric transport, bit1 ghost pressure; bits above1 are rejected.
Old0/1 values decode as centre pressure, while2/3 select ghost pressure. There
is no mode inference or ambiguous restart. `--boundary center|ghost` chooses the
probe candidate, and metadata names it. Promotion requires matched tests and the
same physical/refinement gates; implementation alone is not acceptance.

## One bounded higher-order velocity candidate

`VelocityAdvection::LimitedMacCormack` is an opt-in comparison against the default
semi-Lagrangian velocity path. With the old extended velocity frozen as carrier,
advect forward, then backward with RK2 characteristics and trilinear sampling.
Correct the forward value by half the old-minus-backward error. Limit each
component to the minimum/maximum of the eight old source-stencil samples at the
forward departure point. Pressure/solid constraints follow the velocity operator.

Method reference: [Selle et al., An Unconditionally Stable MacCormack Method,
JSC 35 (2008)](https://physbam.stanford.edu/papers/stanford2006-09.pdf). This is a
new implementation of the forward/backward method and local extrema limiter;
no reference source code is copied and no unconditional whole-solver stability
claim is made. The force/VOF splitting remains first order. Neither velocity
candidate is mass/momentum-consistent conservative momentum transport.

Header flags bit2 records the velocity candidate: 0=semi-Lagrangian,
1=limited MacCormack. Known flags are now 0..7; higher bits reject. The binary
layout is unchanged, old flags retain their meaning, and exact restart tests
cover the combined ghost/MacCormack candidate. Periodic sampling is used only
for operator tests (uniform flow, steady shear, analytically translated smooth
wave with error/energy checks), not as the physical tank's boundary model.
`--advection sl|maccormack` selects the probe path. This is the last bounded
velocity-method experiment in this pass; failure to materially improve the same
three-level dam comparison remains an explicit M1 blocker.
