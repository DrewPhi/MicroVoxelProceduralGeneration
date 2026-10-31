# Issue 3: terrain memory budget and LOD fallback

## Diagnosis

The matched seed-42 valley capture at world scale 10, quality 100, native radius
32 m, tile ceiling 8192 and memory budget 768 MiB initially requested 5978 tiles.
It degraded to 3431 active tiles / effective budget 3456, visible P95 error
3.92 px. An instrumented baseline reproduced the same result.

The failed required allocation was in the color arena: 134204544 live bytes in
a 134217728-byte pool, trying to allocate 53264 bytes. No pending GPU frees were
present. Managed commitment was 297930784 bytes against an 805306368-byte budget.
The adapter itself permits only 128 MiB in one storage binding. The old renderer
bound each entire arena and therefore treated the binding limit as a pool limit.
Increasing the total memory slider could not remove this bottleneck.

## Change

Draws now bind aligned ranges within the same pooled buffers. Shader indices are
relative to each draw's range. Pool capacity can exceed the storage binding limit,
while each draw remains checked against it. The adapter's supported buffer size
is explicitly negotiated, capped at 1 GiB (the maximum UI memory budget), without
allocating that capacity at device creation. Conservative unrelated limits stay
unchanged. See the wgpu 24 Limits documentation:
https://docs.rs/wgpu/24.0.0/wgpu/struct.Limits.html

Bindings are cached per resident draw and rebuilt when pool buffers grow or
compact. They are not rebuilt per frame. Allocations remain pooled; draw counts
are unchanged. Alignment padding and bind-group objects add some overhead; the
managed budget accounts for terrain buffer capacity and in-flight migrations,
not all driver-private bookkeeping or total process memory.

Compaction remaps ranges and rebuilds bindings; queued-copy lifetime protection,
migration headroom, bounded cache eviction and required-cover fallback remain.
A hard binding limit on an individual draw still produces an explicit error.
New diagnostics report per-pool live, pending, free and reserved capacity, total
commitment, migration capacity, configured ceiling and exact allocation failure.

## Verification

Evidence and matched capture logs are under output/lod-budget-issue3/.
The adapter in this session is llvmpipe Vulkan software rendering; frame times
must not be interpreted as laptop hardware GPU performance.

## Matched capture results

| Measurement | Before | After |
| --- | ---: | ---: |
| Planned/active tiles | reduced to 3431 | 5978 / 5978 |
| Effective tile ceiling | 3456 | 6144 |
| Memory pressure | true | false |
| Visible P95 error | 3.92 px | 2.61 px |
| Native support probes | 10/10 | 10/10 |
| Outstanding jobs | 0 | 0 |
| Managed buffer commitment | 259.98 MiB | 489.45 MiB |
| Live arena data | 203.04 MiB | 373.77 MiB |
| Generated mesh jobs | 8195 | 5978 |

The larger requested cover now publishes without quality degradation. The policy
still caps quality-100 settings at 6144 tiles, and balancing selects 5978; an
8192 CLI ceiling is an upper limit, not a requirement to allocate 8192 tiles.
The requested 1 px error is still not achieved: selection budget, balancing and
terrain error estimates remain separate limitations. Broad background facets
and native river terraces remain visibly present. This fix does not change
terrain geometry, native occupancy or the painting algorithm.

The after capture took 114.79 s to settle versus 136.25 s before, but builds/tests
ran concurrently, so these are diagnostic observations, not controlled timing
claims. Rendering more geometry also costs more: software rendering measured
1065.20 ms/frame after versus 518.03 before (three synchronized frames only).
The change enables higher detail within the requested buffer budget; it is not
an unconditional FPS optimization. Real GPU profiling is still required.

Release workspace checks passed: 221 tests. All 13 GPU tests passed including
actual render/readback equality with color data beyond the adapter storage
binding boundary, compaction, transitions, lower-budget migration, and mesh/ray
agreement. The 3000-tick asynchronous world-switch/run/idle/resume regression passed:
162 startup native-support misses, zero idle misses, zero resumed misses, and
561 frames free of a blocking publication target. No pressure-driven quality
fallback occurred during the logged route. It took 525.36 s on software Vulkan;
this is a correctness regression, not a real-time hardware benchmark.

The Linux launcher was rebuilt. Formatting and git diff whitespace checks pass.
Resolution is tracked in GitHub issue #3; the fix is published with this record.

