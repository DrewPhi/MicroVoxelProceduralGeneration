# B2 compressed cold source cache checkpoint

Greenfield experiment; no runtime default promoted. `FRONTIER_SOURCE_COLD_CODEC=lz4`
enables an LZ4 cold tier in the adaptive renderer's existing source-page cache.
Omitting the variable (or `none`) preserves the original hot cache. No old engine
code is ported. The immutable source wrapper owns both tiers and clears/recreates
them on source/edit revision changes; camera epochs do not identify content.

## Data and memory contract

This caches **exactly the existing `SampledColumn` payload**, not full canonical
voxel bricks, props, edits, geometry, or a replacement for volumetric ownership.
Each valid entry serializes an explicit little-endian u16 page index, i64 height,
and three material bytes (13 bytes). It never reads struct padding. Missing
entries remain missing. Negative addresses retain Euclidean page indexing.
The cache's pre-existing source snapshot owns content validity; it cannot be
shared across independent snapshots without clearing.

The cold tier receives half of each shard's existing page reservation, rather
than increasing the total memory budget. The remaining hot tier is smaller.
Each of 32 shards reserves raw scratch, compressed output scratch, fixed-capacity
entry metadata, conservative allocator overhead, and the LZ4 transient 8192-byte
hash table plus allocator overhead. Full cold reservation is charged to source
cache memory, even while cold payload slots are empty. Tiny budgets that cannot
fit this fixed scratch retain the baseline cache. Cold insertion evicts payloads
before copying compressed output; metadata cannot grow. Page-cache table growth
and dense promotions retain the existing old-plus-new overlap accounting.

The pinned LZ4 implementation creates its hash table per encode; this allocation
is reserved, not eliminated. A per-shard last-decoded page reuses raw scratch
across column reads. Sorted entry indices use binary search. Decode and compression
run under the existing source-shard lock and can increase contention and frame
tails. A write drops a prior cold page rather than merging cached partial history;
this conservatively sacrifices reuse. No cold payload is treated as authoritative
world truth. Scratch is invalidated before failed decode and on source clear.

Current dependencies: `lz4_flex=0.11.6` (MIT), `zstd=0.13.3` (MIT),
`zstd-safe=7.4.0` / `zstd-sys=2.1.2+zstd.1.5.7` (BSD-3-Clause, per their
crate manifests; bundled Zstandard includes its upstream license files). Zstd is a dev dependency for comparison only:
its dynamically growing contexts are measured by `sizeof()` in the probe, but
are not integrated into the hard-budget runtime cache. A bounded output buffer
alone does not bound a Zstd context.

## Reproduce

Use the repository's documented Rust toolchain/environment, then:

```sh
cargo test -p frontier-renderer --lib source_ -- --nocapture
cargo test -p frontier-renderer --release --lib source_cache_probe::tests::codec_probe -- --ignored --nocapture
```

Run codec timings serially with other benchmark jobs. The ignored probe compares
raw copying, LZ4, and Zstd level 1 on four Legacy and four Regional canonical
32x32 column pages, empty/dense/cave 32-cubed canonical material bricks, and an
edited fixture brick. It checks restored bytes on every iteration, uses five
warmups and 100 measured repetitions, and prints CSV p50/p95 encode/decode
latencies, payload sizes, and codec context memory. Raw-copy codec has zero codec
context; LZ4 includes its temporary hash allocation. Shared input/output/decoded
buffers are separate from printed codec-context memory. These warm microseconds
are not end-to-end revisit readiness or cold initialization measurements.

For renderer A/B, use identical source reservation, source, camera, route, build,
workers, display settings and output resolution, varying only the environment
variable. Report source cold enabled shards, reserved/resident bytes, retained raw
bytes and entries, page hits, cumulative encoded/raw bytes and encode/decode time.
Cold page hits count a found page payload even when the requested sparse index
is absent; they do not directly count recovered source columns.
Cumulative encoded volume is **not** retained history. Include geometry remeshing,
source lock/lookup time, requested-quality readiness and publication in the route
comparison. A warm route fitting entirely in hot memory does not exercise the
cold tier; memory-pressure revisit needs a separate matched run.

## Gate

Measured outcome: [B1–B3 checkpoint](B123_CHECKPOINT.md) and
[codec corpus CSV](../reference/issue75/b2-codecs.csv). Two ordered static pairs
preserved exact buffers but regressed requested-quality readiness by roughly
30–35%; the 15 m/s return route did not establish quality retention. Keep disabled.
The codec microbenchmark is lossless but cannot override those whole-view results.


Focused correctness tests cover exact eviction/revisit, negative coordinates,
source clear, memory pressure, failed decode scratch invalidation, LRU refresh,
and existing page-cache promotion/accounting regressions. Runtime promotion still
requires 2x useful retained history and >20% revisit readiness improvement at the
same total memory with acceptable forward/cold latency and tails. Compression
ratio or codec throughput alone cannot pass B2. Measurements and pipeline decision
belong in the paired run report; this file makes no performance-win claim.
