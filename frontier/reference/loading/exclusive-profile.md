# Bounded exclusive CPU diagnostic (#69)

2026-11-05, AMD Ryzen 5 7640U, release profile. Reproduction from `frontier/`:

```sh
FRONTIER_PROFILE_SOURCE=1 cargo test --release --locked \
  -p frontier-renderer profile_primary_cold_stages \
  --lib -- --ignored --nocapture --test-threads=1 \
  >output/remaining-bounded-profile.log 2>&1
```

The test passed in 4.71 seconds. It uses the unchanged primary Legacy seed-7
source (1,800 m scale, 1,000 m amplitude, eight octaves, roughness 0.5,
6.25 cm canonical cells) and a fresh 128 MiB cache for each cold case.
These are CPU tile diagnostics, excluding selection convergence, GPU work,
disk I/O and cache construction. They do not measure whole-view readiness.

**Instrumentation substantially perturbs execution.** Detailed source profiling
is off by default and adds no per-column clock reads when disabled. When enabled,
multiple clock reads and bookkeeping execute while a source shard is locked.
Even these uncontended serial cold cases record roughly 1.42 microseconds per
source-lock measurement. Clock overhead also remains in exclusive caller/remainder
stages and increases contention in parallel work. Do not infer normal-production
bottleneck proportions or performance gains from this table. The instrumented
full-view attempt timed out at 120 seconds; it is not a quality/readiness result.

All times below are milliseconds, rounded to three decimals. Each row is one
cold job; widths and steps are native-cell counts.

| Minimum X/Z | Width / step | Worker total | Source lock | Source lookup/admission | Canonical generation |
| --- | ---: | ---: | ---: | ---: | ---: |
| `[0,-512]` | 128 / 8 | 201.799 | 23.992 | 50.101 | 27.857 |
| `[-256,-1536]` | 128 / 16 | 200.079 | 24.168 | 50.092 | 27.784 |
| `[512,-3072]` | 128 / 32 | 199.701 | 24.239 | 49.995 | 28.136 |
| `[0,-512]` | 128 / 1 | 205.880 | 24.023 | 50.450 | 28.231 |
| `[0,-512]` | 2048 / 128 | 263.101 | 29.385 | 64.684 | 34.497 |

| Width / step | Appearance exclusive | Certificate exclusive | Native mesh exclusive | Affine | Finalize | Remaining geometry/orchestration |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 128 / 8 | 94.673 | 0.961 | 0 | 0 | 0.189 | 4.027 |
| 128 / 16 | 96.311 | 0.660 | 0 | 0 | 0.011 | 1.053 |
| 128 / 32 | 96.443 | 0.551 | 0 | 0 | 0.006 | 0.331 |
| 128 / 1 | 3.725 | 0 | 98.758 | 0 | 0.560 | 0.133 |
| 2048 / 128 | 118.452 | 0 | 0 | 11.617 | 0.302 | 4.165 |

The unrounded exclusive categories sum to the worker total with zero observed
ledger discrepancy in every row. Source-lock time includes shard address/lock
acquisition; lookup includes source-page lookup, insertion, promotion and
eviction. Source time is removed from its enclosing appearance, certificate or
native stage; remaining source time is removed from the geometry remainder.
Parallel worker sums overlap in wall time. Main-thread geometry-cache snapshot,
admission and eviction-check costs are separate from worker totals: respectively
0.002–0.004, 0.004–0.005 and about 0.001 ms per cold admission here. No geometry
victim was required by these small cold cases.

The four-adjacent-tile diagnostic records 818.327 ms serial and 235.878 ms with
four workers, with identical 66,817 generated columns and 771 source hits.
Those instrumented wall timings are not a controlled production worker-count
comparison. The batch lines report counts/wall time; the cold lines print the
complete exclusive ledger.

Exact-output checks remain assertions, not visual inference. This diagnostic
compares dense/sparse face profiles, certificates and indices, then compares
cold/prefilled primitive attribute bits, profiles and certificates. The separate
`optional_source_profile_preserves_bits_and_partitions_worker_cost` regression
compares profiling enabled/disabled proxy primitive bits, packed native-quad
bytes, profiles, certificates and source counts. These checks cover their bounded
fixtures; the timed-out full-view profile does not establish full-view image
parity. See [loading optimization](../../LOADING_OPTIMIZATION.md) for normal-mode
visual/performance acceptance and its remaining failures.
