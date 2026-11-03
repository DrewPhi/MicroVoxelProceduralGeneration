# Continuous analytic renderer checkpoint

Entry: `src/bin/analytic.rs`; schema + f64 CPU reference: `src/analytic/scene.rs`; persistent wgpu resources + readback: `gpu.rs`; compute: `raycast.wgsl`; fullscreen presentation: `present.wgsl`.

Launch `./analytic` from repository root. Default model `output/fit-occupancy/terrain.json`: 32 m cube, 4³ patches, five implicit total-degree-two functions per patch, solid iff all Q > 0. Coefficients rebased to patch-local [-1,1]; ray segments rebased at each patch entry. GPU storage uses 48 B per quadratic = 15,360 B for current model. Model global origin retained separately; camera coordinates scene-local meters.

Traversal is ordered uniform patch-grid DDA, NOT voxel DDA or field ray marching. Within each visited patch, solve every quadratic and reject roots outside the patch interval or outside other functions' solid halfspaces. Independent f64 reference brute-forces all patch boxes. Outward normal is minus active quadric gradient. Current shading is Lambert/ambient, slope coloring and light fog. No voxelization, LOD, shadow rays, seam repair, edits, physics, fitting or generation on renderer startup.

User explicitly deferred patch-boundary discontinuities and discrete voxel rendering. Missing boundary faces/cracks from fitted discontinuities are visible and should be fixed in generation/fitting in a future pass; do not confuse them with solver correctness.

Controls documented in README. Mouse grab falls back Locked -> Confined; Esc releases; focus loss clears keys. Window title shows frame rate, GPU compute timing, CPU encode time and sparse traversal averages. Every 30 frames, asynchronous three-slot readback of optional timestamps plus one probe per 32x32 screen pixels. Timings are combined traversal/intersection/shading, not separate claims. Headless synchronized timing includes waits; first-frame shader warmup can dominate short runs. No hardware GPU accessible here: llvmpipe CPU Vulkan only.

Verification on 2026-11-02:
- `cargo test --offline`: all 17 tests pass (7 authoring tests duplicated in exporter binary, 3 renderer tests).
- GPU shader validation + captures at default, closer exterior, interior and horizontal side cameras: 540 sparse ray checks total; 0 hit/miss or distance mismatches at 2 mm tolerance. Largest observed distance discrepancy 0.024 mm.
- Reports/captures `output/analytic/{preview,overview,interior,side}.{png,json}` ignored locally.
- Native presentation tested on private Xvfb :97 (software adapter).

Cached wgpu 24.0.5 used intentionally; encoder timestamp feature requested only when supported. Rust environment on this host: RUSTUP_HOME=/tmp/voxel-rustup CARGO_HOME=/tmp/voxel-cargo and toolchain bin added to PATH. Cargo default-run remains fbm.

## Voxel comparison modes

Added Esc clickable testing menu with continuous, snapped-hit approximation, exact voxels and diagnostic views. Menu implemented as a compact built-in bitmap overlay in WGSL (no GUI dependency). `Frame.right.w` is menu visibility; `Frame.up.w` is voxel size in meters. Modes: 0 continuous, 1 patch heatmap, 2 quad heatmap, 3 snapped appearance, 4 exact voxel DDA. Shortcuts 4 and 5 select modes 3 and 4.

Snapped mode quantizes the material sample and shading orientation only; ray depth and silhouettes stay continuous. Exact mode classifies fitted Q at voxel centers, uses integer cell traversal and axis-aligned entry normals, and treats outside the fitted box as air. Exact mode intentionally includes box cut faces and patch discontinuity walls implied by center occupancy. Same cell material across faces. Inside-solid camera returns t=0, as occupancy intersection dictates.

Exact test baseline is not a claim that voxel DDA is the eventual accelerated analytic strategy. Counters report cell visits in mode 4, patch visits otherwise. `scene.reference_voxels` recomputes boundary times in f64 rather than accumulating f32 deltas. GPU exterior check: 80 rays, zero mismatches, max discrepancy 0.209 mm; snapped check: 80 rays, zero mismatches. Native Esc menu click-switch/resume smoke test passed on private Xvfb. `cargo test --bin analytic`: 4 tests pass, including discrete plane bounds. Captures under output/analytic/voxels*.png, snapped.png and camera.png; do not interpret llvmpipe performance as real hardware throughput.

## Analytic empty-space acceleration (supersedes exact baseline description)

Mode 4 now uses analytic skipping; mode 5 is the retained full cell reference (`--mode voxel-reference`, shortcut 6). Existing Esc Exact Voxels button selects accelerated mode 4. Mode 3 remains only approximate snapped appearance.

For a patch-local quadratic, component gradient maxima on [-1,1]^3 are bounded by |linear| + 2|diagonal| + sum |cross terms|. Multiply their sum by r=voxel_m/patch_edge_m to bound Q(center)-Q(ray). Precompute this margin M once from uploaded f32 coefficients, add conservative f32 padding, and store it in previously unused coefficient slot 10 (q.c.z); buffer stays 48 bytes per function. Q(ray)+M is quadratic in t. If negative, solve its first possible positive root with the stable quadratic formula, jump forward without crossing the patch boundary, and recover the destination cell entry time and face analytically. Residual uncertain cells still use exact occupancy tests. No finite-difference gradients, SDF assumption, numerical root search or LOD. Degenerate/uncertain discriminants fall back locally. Non-voxel-aligned patches disable skipping. Root backoff plus coefficient-scale padding handles f32 uncertainty; this is not a formal directed-rounding interval implementation.

Both reference and accelerated modes now recompute boundary times from integer coordinates instead of accumulating deltas. This removed baseline rounding drift: all five 320x240 final captures match pixel-for-pixel (384,000 pixels total). Final five-camera checks: 800 f64 CPU comparisons across accelerated/reference, zero mismatches. Four additional synthetic scenes (sphere, saddle, intersection of two quadrics, discontinuous air/solid patch boundary), two camera angles each: 640 more comparisons, zero mismatches. Unit test checks center bound for 20,000 randomized quadratics/cell points. All 5 renderer unit tests pass.

Two-repeat initial sweep (llvmpipe CPU Vulkan, 320x240) saw accelerated 4.1–9.7 ms versus reference 28–75 ms. Final single-repeat sweep after integer-boundary fix: overview 4.65 vs 34.72 ms; oblique 7.95 vs 73.87; horizontal 8.21 vs 73.13; overhead 7.04 vs 66.17; underside 8.54 vs 27.83. These are late-frame compute samples, not FPS predictions or percentile guarantees. Close horizontal mean cell visits dropped 540.4 -> 14.0. Source output: output/analytic/analytic-skip-final/summary.json. Initial repeated sweep: output/analytic/analytic-skip-profile/. GPU timings vary on the shared software adapter.

Reference reading: Knoll et al. 2007, https://www.sci.utah.edu/~knolla/rtia-rt07.pdf (conservative interval exclusion principle); our specific closed-form voxel-center envelope is separately derived and documented above. tools/check_analytic_renderer.py reproduces synthetic checks. tools/benchmark_renderer.py supports --modes and --verify.
