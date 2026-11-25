# Whole-cell water laboratory: method and limits

Greenfield reuse tier G. This standalone experiment imports no legacy source, adds no dependencies, and does not integrate with the engine or alter its authoritative data. Its purpose is an inspectable first motion checkpoint, not a production fluid implementation or a validated Navier–Stokes solver.

## Authoritative state

A water unit is exactly one occupied 0.0625 m cube: volume 0.000244140625 m³. Every lattice cell is solid, dry, or one water unit; there is no fill fraction, fractional water mass, particle compression, interpolation of occupancy, or mass reservoir in numerical fields. Positions and movement are integer lattice neighbors. The finite 32×24×8 domain measures 2×1.5×0.5 m including its boundary walls. The y axis points upward. The fixture seed is fixed by source code; there is no random simulation input.

Face velocities, displacement credits, and cell pressure are numerical auxiliary state. They do not contain water. In particular, credit between zero and one is waiting displacement, not partial occupancy. The binary mask changes only when a complete legal assignment is accepted.

## Each 0.025-second step

1. Mark faces adjoining water and exclude all solid faces. Apply gravity, 9.81 m/s², to vertical face velocities.
2. Compute provisional divergence. Perform 64 sequential over-relaxed Gauss–Seidel pressure sweeps, relaxation 1.4, with zero dry-cell pressure and omitted solid-face flux. Pressure is clamped nonnegative. Pressure warm-starts from the preceding step.
3. Subtract the pressure gradient from face velocities. Clamp speed to 0.8 cells/step (2 m/s). Integrate signed face displacement credit and saturate it at ±2 cells.
4. Each face with at least one credit proposes one whole-cell movement in the credit direction when its source contains water. Proposals into currently occupied cells are allowed.
5. Sort proposals by descending absolute credit, break ties by fixed face index, and greedily claim at most one destination and one outgoing move per source. Traverse dependencies. Accept chains ending in dry cells and closed cycles; reject chains blocked by stationary water.
6. Clear all accepted sources and then fill all accepted destinations atomically. Debit exactly one displacement credit for each move. Assert unchanged integer water count and zero solid overlap after every step.

The bounded comparison uses a maximum-weight Hungarian assignment for at most 64 water cells. Every source has a zero-score stay option. Its objective is maximum total eligible credit, with injective destination ownership; it permits the same chains and cycles. This is an exact small **resolver assignment** reference, not a more accurate fluid reference. The executable compares 128 deterministic 4³ candidate configurations with 32 occupied cells, verifies legal assignments independently, and records objective loss and timing for the cheap resolver. It does not run the Hungarian solver on the larger five fixture simulations.

## Diagnostics and interpretation

`diagnostics.json` includes exact initial/final mass, solid violations (also enforced by assertions), total accepted/rejected proposals, occupancy changes, pressure-solve residual, projected divergence, speed-clamp and credit-saturation counts, and CPU simulation step p50/p95/p99. Export/JSON/preview costs are excluded from those timings. Timings come from the execution machine, not a laptop/GPU performance gate. The JSONL stream exports every fourth step plus the initial and final frames.

`velocity_energy_proxy` sums half the squared face speeds after conversion to m/s. It has no mass weighting, double-counts neighboring fluid motion, includes active air-boundary faces, and is **not Joules or physical kinetic energy**. Pressure is reported in m²/s² (density-normalized pressure); the Poisson residual is in cell-coordinate units s⁻², and divergence in cells/s. `max_column` counts occupied cells in a column, not its surface height. Mean left/right tops average the highest occupied y coordinate in each column on either side of x=15, excluding that dividing column; they can overestimate liquid level when there is spray. The vessel level difference is an approximate visible-surface diagnostic, not a settled equilibrium guarantee.

The 6-second run preserves mass and solid exclusion in all fixtures. The pool occupancy remains exactly unchanged, with zero accepted moves. Vessels reduce their mean surface-top difference from 12 cells to about 1.38 cells but overshoot and retain motion. Other fixtures also remain in motion. Large clamp/saturation counts and sizable residuals in moving fixtures are evidence of this model's numerical limitations, not success metrics to hide.

## What this checkpoint does not implement

- No velocity advection or transport of momentum with moved water. Velocities belong to fixed faces; they are reset on faces no longer adjoining water. Momentum is a guidance field, not a conserved per-water-unit quantity.
- No physical viscosity, turbulence model, surface tension, contact-angle model, fluid/solid reaction force, or calibrated collision response.
- No guaranteed incompressible projection: fixed iterations and nonnegative-pressure clipping leave residuals. Speed/credit clipping discards numerical momentum/displacement. Quantized occupancy introduces further divergence and pressure mismatch after assignment.
- No energy conservation, damping calibration, time-step convergence, continuum validation, resolution convergence, or quantitative dam-break validation. Stable still water alone proves none of these.
- Fixed score ties introduce directional bias. Greedy arbitration can reject legal higher-value combinations; the bounded exact comparison measures this defect. Dense cycles can move labels/credits without changing occupancy and must not be mistaken for visible mass transport.
- No fractional shape smoothing or rendered surface reconstruction. The preview draws whole cubes and removes exterior walls only for cutaway visibility.
- No GPU path, sparse world scheduling, chunk boundaries, streaming, engine integration, or real-time game acceptance. This small offline lab is deliberately bounded.

## Tests and restart

Compile the Rust test harness as documented in README. Tests cover empty-ended chains, cycles, stationary blockage, target conflicts, exact assignment improvement, all-fixture mass/solid exclusion, stationary pool occupancy, falling occupancy movement, and bit-identical restart continuation. `.restart` files include occupancy, solids, every pressure bit, face velocity/credit bits, tick, dimensions, and initial mass. Diagnostics containing wall-clock timings are intentionally outside deterministic restart state.

Conceptual background only: cooperative motion can require loops/chains rather than serial empty-neighbor updates ([Frontiers in Physics 2020](https://www.frontiersin.org/journals/physics/articles/10.3389/fphy.2020.607480/full)); lattice gas approaches provide discrete-fluid context ([review](https://arxiv.org/html/comp-gas/9905001v1)). No source was transplanted from either publication, and this method is not presented as implementing either model.
