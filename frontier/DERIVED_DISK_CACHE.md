# Disposable completed-geometry disk cache (#72)

Greenfield implementation: `renderer/src/derived_disk_cache.rs`. No legacy
scheduler, cache implementation or new dependencies are imported. Packed finished
geometry avoids repeating source generation, exact appearance accumulation and
certificate construction. Source pages remain the canonical CPU acceleration
layer; disk records are never a save, edit journal or world authority.

Each record explicitly encodes little-endian scalar fields, never Rust memory.
Format `FVDC0002` / representation 2 includes optional 64-byte lossless proxy
triangle records; records from older representations are disposable misses.
The header records format and representation versions. Identity includes canonical
source version, seed, voxel size, FBM source/appearance versions, recipe and every
parameter, topology owner, revision and sorted exact edit coordinates/materials.
Tile identity includes region, interior/edge strides and revision. Full identity
bytes are checked after lookup; a filename hash collision can only cause a miss.
An FNV checksum detects accidental corruption, not malicious forgery. Representation
version must change with geometry, ownership, appearance or certificate semantics.
The cache belongs to one immutable world snapshot and must be recreated on edits.

The optional cache uses one dedicated IO worker. Preparation jobs wait only for
positively indexed reads; absent keys are rejected by a local bounded index
without an IO round trip. Reads take priority over queued writes, while an
already active filesystem operation runs to completion. Optional completed-tile
writes are queued without blocking preparation on encoding or fsync. A process-held OS file lock excludes other cache owners
from the directory and releases automatically after a crash. Initialization
indexes at most 16,384 directory entries and removes incomplete temporary files;
operations use the bounded in-memory index, with no per-tile directory scan.
Eviction follows accesses within a run (startup order is directory order).
Disk allowance includes temporary writes. Files are flushed and atomically
renamed; interrupted writes cannot expose partially completed records. A failure,
missing record, version mismatch, oversize or checksum/structural failure falls
back to ordinary generation. Cache initialization failure disables this optional
acceleration without changing source authority.

Entries are uncompressed, avoiding decompression bombs and compressor memory.
Encoding checks a conservative scalar-count size estimate before allocating;
reading checks file size and reads exactly that size into one bounded buffer.
Decoding checks every vector count against remaining bytes before allocating,
checks finite scalar fields, bounds ordering, triangle/profile indices, native
axes/materials and bin ranges. Encoded buffer plus decoded vector payload is at
most approximately twice the entry limit, plus bounded identity/index/allocator
metadata and caller-held tiles. Before allocation, each record is additionally
limited to half of `build_memory::reservation(key)`, so simultaneous encoded and
decoded payloads fit the admitted builder reservation. Unsupported keys are
rejected before file reading. Encoding uses the same conservative per-key limit. This is an IO-specific bound, not a whole-renderer
memory guarantee. Identity creation refuses more than 4,096 edits rather than
allocating an unbounded per-cache snapshot. Disk directories should be dedicated
to this cache; the entry cap counts unrelated directory entries too.

CPU-only development-profile diagnostic (one pass, other compilation active;
not a renderer/hardware performance gate): Legacy seed 7, default FBM, tile
minimum [-128,-512], width 128 canonical cells, 6.25 cm voxels. These
measurements precede format 2; its additional empty proxy-vector count adds
eight bytes to otherwise identical unpacked/native records:

| Interior/edge stride | Build | Flush/write | Restart read/decode | Disk bytes | Decoded payload |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 (native quads) | 16.344 ms | 3.017 ms | 2.654 ms | 636,108 | 635,348 |
| 8 (proxy) | 7.118 ms | 1.420 ms | 0.321 ms | 126,696 | 125,928 |

These bounded examples support testing completed geometry before a larger source
page persistence implementation. They do not establish whole-view restart
latency, moving quality, disk tail latency or the final laptop frame budget.

Reproduce checks and the diagnostic with:

```sh
cargo test -p frontier-renderer derived_disk_cache --lib --locked -- --nocapture
```

Tests cover cold/warm restart native/proxy exact encoded-field parity,
seed/recipe/parameter/topology/edit invalidation (including equal-revision edit
content), corrupt/oversized/truncated/version-invalid records and regeneration,
malformed vector lengths, indices, nonfinite fields, inverted bounds/ranges, negative certificates and invalid native bin metadata, bounded disk eviction and interrupted-temp
cleanup, exclusive lock release, per-key memory rejection before allocation/read, stable filename/index/removal events, and the diagnostic above. Camera selections and
GPU residency are deliberately absent from the persisted payload.


## Bounded optional background writeback

`renderer/src/derived_disk_writeback.rs` owns the single disk cache and its OS
lock. The write queue accepts at most 64 immutable tile Arcs / 64 MiB of payload,
including the active write. Full, duplicate or unavailable queues skip optional
writes and report the skip count. Queue size, peak payload/count, completed writes,
errors and the reserved encode-buffer allowance are reported separately.
The IO encoder has a separate maximum 64 MiB scratch allowance, constrained
further by the per-key builder reservation. These allowances do not replace
cache, selected/pending Arc, source-page, driver or whole-process accounting.

Read failures remove stale index entries; per-operation replacement/corruption/LRU
removal events update the client index without rescanning the directory or
walking all cached keys after every store. Filename calculation clones a small
prehashed identity keyer, preserving the complete identity check inside each
record. Cache entries, queued tiles and IO failures never change canonical source
or edit authority.

First-quality and preparation timing must record pending writes before an
explicit flush. A tool can request `flush()` and measure drain time separately;
shutdown also drains admitted writes on the preparation worker's teardown.
Bounding the remaining work does not guarantee a wall-clock shutdown deadline:
OS encoding/fsync/write calls can still take variable time. A full queue can
leave some generated products uncached; successful shutdown does not imply that
every generated tile was admitted or saved. The cache remains disposable.

Writeback tests check the negative-index fast path, shutdown restart reuse,
explicit flush releasing even the in-flight Arc, recovery after an optional
write error, and skipping an oversized queued payload without retaining it.

## Full-view asynchronous format-2 checkpoint

The fixed primary Legacy seed-7 view, 1920×1080, MSAA4, 300 m clip, requested
5 px / internal 4 px reserve and four CPU workers reached first reserve readiness
in **15.648 s with a cold disk cache**, and **7.192 s after restart with that cache**.
Both captures retain byte-identical approved floating RGB. These are stationary
preparation results, not a moving-view or long-range readiness pass.

The cold run built 4,984 tiles and completed 4,984 optional writes totaling
362,314,408 bytes, with zero write errors or skipped writes. The write queue
peaked at five entries / 5,169,536 payload bytes, below its 64-entry / 64 MiB
allowance. At the recorded final snapshot there were no pending writes.
Worker write time totaled 3.995 s and overlapped preparation; it must not be
added to first-readiness time. Raw reports/capture: `output/loading-async-cold/`.

The warm run recorded 4,984 disk hits, zero geometry builds, zero writes/errors,
and 2.018 s of cumulative read/decode time for the same 362.3 MB of records.
Raw reports/capture: `output/loading-async-warm/`.
The 7.192 s wall readiness also contains planning, publication, uploads and other
preparation work; cumulative IO time is a separate stage counter.

An earlier synchronous disk implementation reached warm readiness in about
3.64 s but increased cold readiness to about 29 s. The asynchronous implementation
therefore improves cold startup while retaining a useful restart improvement;
it does not establish the fastest possible warm read pipeline. Queue/channel,
index, scheduling and IO costs should be measured before further optimization.
The cache remains optional, and these timings do not change source, appearance,
quality or memory acceptance requirements.
