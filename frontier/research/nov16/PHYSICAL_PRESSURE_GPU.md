# Preparatory GPU pressure component for #88

This is an isolated matrix diagnostic, **not physical-fluid M 3 acceptance**.
The CPU physical-fluid M 1 candidate, CA playtest, world state and renderer default
are unchanged. No GPU advection, transport, ghost/interface construction,
boundary conditions, velocity projection, streaming or liquid display is added.

Greenfield implementation in
[physical_pressure_gpu.rs](../../renderer/src/physical_pressure_gpu.rs) and
[physical_pressure_gpu.wgsl](../../renderer/src/physical_pressure_gpu.wgsl),
following the pressure/CG formulation in [Bridson & Müller-Fischer, SIGGRAPH 2007
course notes, chapter 4](https://www.cs.ubc.ca/~rbridson/fluidsimulation/fluids_notes.pdf).
No external source code was copied. The existing CPU operator was inspected for
its matrix contract; none of its ghost or physical boundary construction is
transplanted into this component.

## Accepted matrix contract

`PressureRow` is 32 bytes: six optional neighbor indices, one positive diagonal,
and an RHS. Each link contributes−1; `MISSING` means no off-diagonal term.
Only finite symmetric, diagonally dominant M-matrices are accepted. Every
connected component must include a strictly dominant row, guaranteeing SPD for
this restricted matrix class. Missing links must already incorporate any
Dirichlet contribution in the supplied diagonal/RHS. Eliminate gauges explicitly:
an identity pin row has no links, and neighboring rows omit reciprocal links to
that pin while retaining their original diagonal contribution.

The component rejects asymmetric/duplicate/self/out-of-domain links, singular
unanchored Neumann components, invalid tolerances/iterations and memory/device
limits. It does not claim to accept every SPD matrix. Nonunit off-diagonal
weights are unsupported; the current CPU pressure operator's unit fluid-fluid
links and separately modified diagonal fit this subset.

Limits:1..64³ rows,1..2048 encoded iterations, relative tolerance 1e−7..0.1,
absolute tolerance 1e−12..1, and an explicit byte budget. The default is 256
iterations, relative 1e−5, absolute 1e−6 and 32 MiB. At 64³ rows, resident buffers plus
one final diagnostic staging buffer account for 16,793,696 bytes (16.016 MiB).
Caller input storage, driver pipeline allocations and CPU validation scratch
are not included in this GPU-buffer byte count.

## GPU-resident algorithm and synchronization

The baseline is Jacobi-preconditioned conjugate gradients, starting from zero.
Input rows are immutable. GPU buffers retain x/r/p/Ap, block dot-product sums,
PCG coefficients, norms, iteration count and status. Each iteration encodes:

1. Matrix-times-direction plus block dot products.
2. Final GPU reduction and alpha update.
3. Solution update.
4. **Separate dispatch** recomputing `r=b−Ax`, Jacobi rho and residual norm.
5. Final GPU reduction, convergence/breakdown test and beta update.
6. Direction update.

Workgroup reductions never substitute for a global dependency barrier: all
solution writes complete before the true-residual dispatch reads neighboring
solutions. At most 1024 block sums feed the final 256-thread strided reduction.
No floating-point atomic add or CPU readback is used per iteration.

A fixed maximum dispatch schedule is diagnostic/budgeted rather than adaptive
CPU iteration control. GPU status gates subsequent arithmetic after convergence
or failure; the host still submits the remaining bounded dispatches. Status 0 is
active,1 converged,2 numerical breakdown/nonfinite,3 iteration limit. No fallback
forces success. Final status must be checked before an accepted pressure update.
Residual replacement every iteration makes stopping use the actual GPU solution,
but f32 conditioning/roundoff can still prevent a requested tolerance.

## Public interface and future connection point

```rust
use frontier_renderer::physical_pressure_gpu::{
    PhysicalPressureGpu, PressureConfig, PressureRow, MISSING,
};
// Build rows from an independently validated boundary/topology operator.
let rows = vec![PressureRow {
    neighbors: [MISSING; 6], diagonal: 1.0, rhs: 2.0,
}];
let solve = PhysicalPressureGpu::new(&device, &rows, PressureConfig::default())?;
let mut encoder = device.create_command_encoder(&Default::default());
solve.encode(&mut encoder);
queue.submit(Some(encoder.finish()));
// Future GPU consumers can bind solve.solution_vectors() and solve.state_buffer().
// The solution is the first scalar of each16-byte row. Require status1.
// This explicit diagnostic copy is only for final tests/debugging:
let diagnostic = solve.readback_diagnostic(&device, &queue)?;
assert_eq!(diagnostic.state.status, 1);
```

A later bridge can convert the CPU pressure builder's validated diagonal,
neighbor and RHS arrays to these rows, retaining its dry/solid/gauge identities
and boundary coefficients. It must independently compare the assembled f32
operator and resulting pressure-gradient/velocity update with the f64 reference.
No direct connection exists now. Warm starts, weighted links, GPU operator
construction, a cheaper convergence schedule and better preconditioners remain
future choices requiring parity and conditioning evidence.

## Correctness evidence

Three CPU-only tests pass: Naga/WGSL and host layout validation; input/SPD/budget
rejection; and manufactured matrices solved independently with a dense f64
unpreconditioned CG oracle.

The ignored actual-hardware test passed on Radeon 760M integrated graphics,
RADV PHOENIX/Mesa 25.2.8, Vulkan. Every fixture has 400 rows (10×8×5), spanning two
256-thread workgroups including a partially populated tail. RHS values are
manufactured from known binary-exact pressures. The test compares x to the
independent dense f64 solver and the known field, independently recomputes the
final true residual in f64, and requires the declared relative 2e−6 tolerance
(or the absolute 1e−6 target when larger), without force-convergence.

| Matrix | GPU iterations | Independent relative true residual | Maximum pressure error vs f64 |
|---|---:|---:|---:|
| Dirichlet |22|1.61401e−6|1.72108e−5|
| Pinned Neumann |54|1.31604e−6|1.70469e−5|
| Disconnected obstacle, separately pinned components |58|1.83251e−6|1.99676e−5|

All report status 1. Each case's resident-plus-diagnostic byte accounting is 25,728
bytes. The raw log is `frontier/output/nov16/pressure-gpu/parity.log`.
CPU physical probes were active during GPU validation, so **no timing or
performance conclusion is drawn**. This establishes only these manufactured
component cases, not physical projection accuracy or cross-vendor robustness.

An ignored 128 K-row component microbenchmark is available but has **not been run**.
It reports command encoding plus GPU completion plus final diagnostic readback
wall time, not isolated GPU timestamps or game FPS. Run only on a quiet machine;
status 3 is an explicit iteration-limited diagnostic, never a fabricated success.

## Reproduce

```sh
cargo test --manifest-path frontier/Cargo.toml -p frontier-renderer physical_pressure_gpu::tests
cargo test --manifest-path frontier/Cargo.toml -p frontier-renderer gpu_manufactured_pressure_matches_dense_f64_oracle -- --ignored --nocapture
# Optional later, only with an idle CPU/GPU:
cargo test --manifest-path frontier/Cargo.toml -p frontier-renderer gpu_pressure_128k_component_microbenchmark -- --ignored --nocapture
```

Validation here used a standalone optimized test binary linked against existing
compatible debug dependencies, avoiding a shared Cargo build:

```sh
source ../.toolchains/env.sh
rustc --edition 2021 --test -O frontier/renderer/src/physical_pressure_gpu.rs \
  --extern wgpu=frontier/target/debug/deps/libwgpu-78f2097d9ca06447.rlib \
  --extern naga=frontier/target/debug/deps/libnaga-0bf31e9425e86290.rlib \
  --extern bytemuck=frontier/target/debug/deps/libbytemuck-8ef885441feccbcd.rlib \
  --extern pollster=frontier/target/debug/deps/libpollster-a5eb8b10d721044e.rlib \
  -L dependency=frontier/target/debug/deps -o /tmp/frontier-pressure-gpu-tests
/tmp/frontier-pressure-gpu-tests --nocapture
/tmp/frontier-pressure-gpu-tests gpu_manufactured_pressure_matches_dense_f64_oracle --ignored --nocapture
```

Dependency hashes/toolchain must match the local build. The ignored hardware test
rejects a CPU/software adapter. Neither it nor this preparatory component changes
M 1 acceptance or promotes a fluid backend.
