# Issue 74 — research and first measured checkpoint

2026-11-06. This checkpoint does **not** complete #74. See the
[ordered plan](../../ISSUE74_PLAN.md), [GPU research](../../research/ISSUE74_GPU_METHODS.md)
and [voxel research](../../research/ISSUE74_VOXEL_METHODS.md).

## What changed

Greenfield changes only; no external engine code copied.

- Native surface profiles reuse each column's already sampled canonical top
  material. Regional cover previously repeated its analytic slope/moisture work
  for those same columns. Side strata, iteration and floating-point accumulation
  order are unchanged. One byte per sampled halo column is retained and charged
  to the workspace (16,900 extra bytes for a 128×128-column tile).
  Enabled by default; `FRONTIER_NATIVE_PROFILE_REUSE=0` retains the direct painter
  for matched same-binary A/B.
- Affine noise-bound reduction streams its branches instead of allocating a
  temporary vector per noise call. Branch order, outward rounding, coefficients,
  fallback counts and certificates remain identical to the retained test oracle.
- Timing reports now record actual presentation/projection settings, current
  MSAA/filter after menu changes, upload budgets and the experiment switch.
  GPU timestamps are explicitly terrain-pass-only. Interactive settings changes
  disqualify a report as a frozen-preset benchmark.
- Benchmark provenance captures actual executable SHA256, UTC start, allowlisted
  experiment flags and observed checkout state. Checkout state is **not** asserted
  to be the binary's build revision. Aggregation retains missing metadata and
  failed runs rather than treating unknown settings as matching.

## Evidence and decisions

[Machine-readable reports](checkpoint.json), [compact table](checkpoint.md) and
[bounded CPU output](cpu-probes.txt) preserve the commands, configurations, full
timing reports and decoded-RGB hashes. Generated full images/logs remain in
`output/issue74/`. Baseline executable is from pushed `98b12e2`; the new executable
was built from that checkout plus this checkpoint's uncommitted changes. Executable
hashes in the reports identify the actual runs. These are sequential development
observations, not randomized, sustained performance acceptance.

Hardware: Ryzen 5 7640U / Radeon 760M, RADV/Mesa Vulkan on this laptop.
All GPU cases use 1920×1080, 4×MSAA, native 0.0625m cells, four preparation workers,
eight refinement selections, requested 5px / reserve 4px, 512MiB CPU cache, no disk
cache and atmosphere off. Source, camera, output and route metadata are retained
per run; Regional and Legacy are **different presets**, not speed comparisons.

**Fixed Legacy baseline:** 300m requested readiness 10.771/10.853s, reserve
12.244/12.280s, frame p95 10.265/10.170ms. Candidate requested 11.105s,
reserve 12.570s, p95 10.187ms. All three decoded RGB captures are byte-identical.
No demonstrated whole-view speedup. The separate 1km baseline required 53.447s
for requested quality and 71.057s for reserve, p95 16.906ms: it still misses the
strict 16.67ms target. This uses batch 8, so do not compare causally against the
historical batch-16 60-second checkpoint.

**Regional same-binary ABBA:** 1km clip, eye (0,93.95,0), looking along +Z,
three measured seconds after settling. Direct painter reserve 10.176/9.914s;
reuse 10.148/10.440s. Frame p95 4.818/5.032ms versus 4.940/4.754ms.
All four RGB captures are byte-identical. This short scene has only 11 native
clusters. There is **no established full-view latency or FPS improvement**.

**Bounded native CPU subpass:** one 128×128-column Regional tile, same binary
ABBA. Direct painting 4.305/4.291ms versus reused 0.462/0.471ms, with exact
profiles and geometry. This removes 16,384 duplicate material evaluations.
Legacy results show no clear benefit. The benchmark excludes initial extraction,
planning, uploads, drawing and whole-view loading. Keep as an exact local work
reduction, not a ninefold engine speedup.

**Affine CPU subpass:** eight alternating batches of 1,024 bound evaluations
retained the same digest; times overlap and drift with warmup. No convincing
timing benefit. Keep as bounded allocation cleanup only. Neither cleanup satisfies
the plan's larger preparation-latency gate or resolves long-range/moving quality.

The 300m control reports 31.210 worker-seconds of builds: 20.091s appearance
(including source queries), 3.553s affine bounds, 3.126s certificate, 1.129s
finalization, 0.287s native mesh and remainder. These are **overlapping worker
sums**, not wall-time percentages or exclusive source costs. The appearance
workload remains the next substantial investigation; full per-column clock
instrumentation previously perturbed execution too strongly to rank exclusive
costs reliably.

## Validation and next step

279 ordinary Rust tests pass, nine manual probes intentionally ignored.
Both new ignored CPU probes were run explicitly. Python report/provenance tests
and the existing tool suite pass. Release workspace builds. The fixed Legacy
GPU regression runs before Regional captures; geometry, native materials and
image payloads remain unchanged. No scheduler, visual transition, culling or
alternate renderer has been promoted.

Next: bounded source/appearance-page reuse experiments that preserve canonical
results and cancellation/memory limits, followed by compatible draw batching
only where timing supports it. Exact 16×16 native/proxy profile reuse is a
candidate; simply summing child floating profiles changes accumulation order.
Do not run a broad renderer rewrite or claim motion/4–20km acceptance from this
checkpoint. The new Regional atmosphere still needs a matching approved offline
lighting reference before aesthetic/runtime approximation changes.

## Reproduce

From `frontier/`, build with the pinned toolchain:

```sh
cargo test --workspace --locked
cargo build --release --workspace --locked
cargo test --release --locked -p frontier-world profile_streamed_noise_bounds --lib -- --ignored --nocapture --test-threads=1
cargo test --release --locked -p frontier-renderer profile_native_top_material_reuse --lib -- --ignored --nocapture --test-threads=1
python3 tools/adaptive_loading_check.py --output output/issue74/recheck --workers 4 --refinements 8 --distances 300 --seconds 5
python3 tools/issue74_report.py --output output/issue74/report.json output/issue74
python3 -m unittest discover -s tools -p 'test_*.py'
```

The Regional A/B's full commands and explicit experiment environment are stored
in `checkpoint.json`. Repeat in direct/reuse/reuse/direct order in separate
processes, without concurrent compilation or GPU workloads. Historical reports
missing new fields remain distinct unknown-metadata groups.
