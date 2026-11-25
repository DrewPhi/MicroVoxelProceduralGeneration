# Existing local-edit and cache optimizations: acceptance audit

Audit date: 2026-11-16. Starting checkpoint: `38fcf40`, with the coordinated
November 16 working-tree changes. This is an acceptance reconciliation for
[#76](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/76),
[#77](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/77), and
[#79](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/79).
None is ready to close on the evidence below. No rendering radius, native
resolution, canonical source, or visual quality setting was reduced for this audit.

The implementation and historical measurements are documented in
[OPTIMIZATION_NOV14.md](../../OPTIMIZATION_NOV14.md). That checkpoint reports
642 workspace tests and 509 renderer tests (33 ignored); those are historical
results, not a claim that this working tree has rerun every gate. The root agent
coordinates fresh compilation/tests and GPU measurements; this audit launches
no concurrent compiler or benchmark.

## #76: local NAADF edit gates

Implementation: [naadf/data.rs](../../renderer/src/naadf/data.rs),
[column_cache.rs](../../renderer/src/naadf/column_cache.rs), and
[cpu_tests.rs](../../renderer/src/naadf/cpu_tests.rs).

| Requirement | Evidence | Status / limit |
|---|---|---|
| Preindex edited 16³ chunks and metre columns | `edits: HashMap<chunk, Vec<address,value>>`, extraction-relative `edited_columns`, and per-column edit extrema | Implemented; collects from the complete override history before region filtering |
| Cached summaries depend only on local edits | Cached summary shortcut requires `!column_edited`; exact terrain samples bypass column cache only for affected columns | Implemented; distant edits retain source-cache identity |
| Only affected chunks lose air/full shortcuts | November 16 separates immutable `source_fast` assembly from whole-column `fast` summary/cache eligibility; chunk shortcuts require no local edits or plant overlay | Implemented in the working tree after this audit identified the earlier column-wide fallback. Immutable TerrainColumn bands cannot be poisoned by edits at their sample heights; exact edits apply last |
| Exact edits override plants and terrain | Explicit canonical-material edit entries replace the assembled values; volume ownership and nonaligned extractions conservatively bypass unsafe paths | Implemented and covered by canonical tests |
| Count fast-skipped / constructed / locally invalidated chunks | Snapshot now records `fast_skipped_chunks`, `constructed_chunks`, `edit_blocked_chunks`; existing preparation metrics expose columns, leaves and stage time | Implemented builder counters; not yet aggregated into published viewer telemetry |
| One/many edits, negatives, boundaries, removal and pigments | `sparse_edits_preserve_canonical_cells_and_untouched_column_pigments`, `compact_overlay_preserves_exact_snapshots_with_plants_negative_edges_and_edits`, `nonaligned_negative_column_edits_cannot_take_air_or_solid_summary_shortcuts`, `nonaligned_topology_columns_match_canonical_at_ownership_and_feature_boundaries` | Implemented test coverage; not the complete 0/1/100/1000-edit performance matrix |
| Cache-on/off exactness and local reuse | Ignored `source_column_cache_reuses_distant_edits_and_bypasses_only_affected_tile` checks exact chunks/blocks/leaves/pigments and sampled-column counts | Must run separately in fresh processes with cache off and on |
| Matched 1080p 0/1/100/1000 edits, preparation, memory, frame tails, readiness | November 14 walking comparison has aggregate work measurements | Unmeasured as an issue-specific edit-density matrix; aggregate walking is not this gate |

The audit found that the November 14 phrase “only affected chunks lose air/full
shortcuts” overstated the then-current column-wide fallback. The narrow November
16 correction samples immutable `FbmTerrain::column_cells` for eligible columns,
keeps cached/summary eligibility false when edited, and gates skip proofs on
chunk-local edit/plant presence. Non-FBM, topology and nonaligned ownership gates
remain conservative. Exact edit material values still apply after source fill
and plant overlay. It adds no new cache use for edited columns.

New regression `edits_at_different_heights_keep_unaffected_chunks_in_the_same_column_fast`
checks Legacy/Alpine/SeededWorld at negative nonaligned coordinates with a surface
band edit, underground removal and fluid addition in distinct roots. It checks
skip/construction counts and all-cell canonical material/pigment parity. The root
coordinator must report its fresh execution result; this document does not infer
that a newly added test has passed.

Narrow remaining work: aggregate those counters through streamed-region
publication, then execute the matched edit-density matrix. No whole-game gain is
claimed from a builder-level optimization.

## #77: vegetation invalidation

Implementation: [vegetation.rs](../../renderer/src/vegetation.rs),
[vegetation_edit_tests.rs](../../renderer/src/vegetation_edit_tests.rs),
[shadow_lifecycle.rs](../../renderer/src/shadow_lifecycle.rs), and World's
`shares_source_lineage` / `changed_edit_cells` APIs.

| Requirement | Evidence | Status / limit |
|---|---|---|
| Local mesh identity | `PlantEdits` indexes override addresses in 8 m buckets; grouped ground masks use their exact tile footprint; tree masks include maximum canonical radius/height | Implemented. Fingerprints intentionally describe masked addresses, not the material at an already masked address |
| Preserve independent placement, templates, grouping and hysteresis | Same-lineage `set_world` retains placement tiles/groups, epoch and independent jobs; keys prevent obsolete changed content from drawing | Implemented. Placement derives from immutable terrain/ecology; retaining these tiles is source-correct |
| Local pending-job cancellation and late result validation | Per-key cancellation tokens and content-key checks; unedited tree templates remain reusable at independent placements | Implemented, including ABA undo/redo and token replacement regression tests |
| True source changes reset broad state | Different source lineage / vegetation cache clears tile jobs, placement, grouping and LOD histories; source-independent unedited tree templates can survive | Implemented; reconstructed worlds conservatively reset rather than trusting equal revision numbers |
| Shadow invalidation | `invalidate_cells` drops only overlapping rooted bounds, including both stale sides of an active shadow transition | Covered by two targeted shadow lifecycle tests |
| Negative edges, cross-tile masks and distant edit stability | `local_edit_id_ignores_distant_edits_revision_and_tracks_negative_tile_edges`, `indexed_edit_ids_match_flat_sets_at_negative_boundaries` | Implemented tests |
| Retained jobs/caches and repeated place/break/reverts | `local_edit_snapshot_retains_placement_templates_jobs_and_hysteresis`, `queued_local_edit_results_reject_changed_keys_and_reuse_unaffected_content`, both ABA tests | Implemented tests |
| Exact plant/terrain assembly | `cached_native_assembly_equals_source_and_applies_edits`, `ground_tiles_preserve_seeded_rotated_native_cells_and_edits` | Existing canonical assembly tests; not a complete visual edit-heavy motion/reload gate |
| Counters and measured end-to-end replacement cost | `local_edit_stats_json` reports local updates, retained placement tiles, cancelled jobs and live tokens; normal vegetation reports pending work, uploads, resident bytes and LOD changes | Partial: issue-specific affected/unaffected mesh counts and event-to-replacement latency are not a complete dedicated data series |
| Save/reload, moving turns, repeated edit-heavy walking at 1080p | November 14 documents walking/turning and local CPU tests | No matched baseline/new edit-heavy walking with full tails, GPU bytes, RAM and per-edit transition inspection; save/load tests for new player transactions do not certify vegetation motion |

Narrow next work: record edit sequence/release and visible mesh-publication
latencies alongside existing upload/miss counters during one fixed grounded
edit/turn/reload route. Keep every required plant and the same ownership/radius.

## #79: coalesced ground-cover cache

Implementation: [ground_cover_cache.rs](../../renderer/src/ground_cover_cache.rs)
and [ground_cover.rs](../../renderer/src/ground_cover.rs).

| Requirement | Evidence | Status / limit |
|---|---|---|
| Same-key concurrent coalescing | Sixteen shards, per-key `Flight`, one registered `Producer`, waiters share immutable result | `same_key_has_one_producer_and_exact_results` uses eight simultaneous consumers |
| Independent computation does not serialize | Shard locks protect lookup/publication only; computation owns no shard lock; recursive dependency levels decrease | `independent_keys_build_concurrently` requires both producers to enter before either completes |
| Cancellation / abandonment / panics | Waiters poll cancellation; producer Drop marks abandoned and notifies; subsequent request retries | `cancellation_abandonment_and_panics_allow_retry`, `recursive_builder_error_releases_parent_reservation` |
| Retained byte budget and bounded metadata | Per-shard FIFO eviction under byte estimate; 64 registered flights per shard | `retained_bytes_and_inflight_metadata_are_bounded`; hot shards cannot borrow another shard's unused budget |
| Saturation behavior | After 64 distinct flights in a shard, new keys bypass registration | Bounded metadata intentionally allows duplicate work for bypassed keys. “Every product built once” applies to registered, unsaturated flights, not arbitrarily overloaded operation |
| Exact old-builder arithmetic | Uncached oracle traverses the same children in the same arithmetic order; no shared cache lookup | `cached_recursive_summaries_match_uncached_float_bits_with_edits` compares bytes of compact and optional directional payloads; run in separate compact/directional processes |
| Revision/epoch identity | Source recipe/parameters/vegetation version and world-aligned coordinates are key fields; local address/material fingerprint separates edits | `local_edit_changes_only_dependent_cached_field_nodes`, `source_identity_separates_seed_params_and_world_kinds` |
| Live memory accounting | Retained estimates include summary, optional 640-byte directional payload, two key copies and estimated table/Arc overhead; waiting Flights retaining evicted Cached values are tracked | `directional_payload_and_evicted_waiter_values_are_accounted`. Consumer-owned Summary clones, allocator fragmentation, shard containers and scratch are separate, so telemetry is not exact process RSS |
| Instrumentation | Hits, misses, coalescing, bypasses, abandoned producers, eviction, lock wait, estimated retained/live bytes; leaf compute time | Implemented. Miss/coalesced counts do not directly count duplicate computation on saturated bypasses |
| Startup and 128 m turn/return 1080p before/after at equal RAM | November 14 aggregate walking and large-radius runs | No isolated matched cache control at equal retained-byte budget; no issue-specific p95 appearance latency comparison |

Do not count the new optional 16/64-probe ground-cover estimator as a #79 cache
speedup: its content, compute demand and identity differ. Test cache parity with
the same probe mode and directional mode on both sides.

Narrow next work: add a repeatable cache-control switch for measurements, or use
a frozen binary with the same uncached arithmetic and memory policy. Record
bypasses separately from coalesced duplicates and measure the same startup /
128 m turn-return route with equal source demand and readiness.

## Spatial candidate index follow-up (November 16)

The inspection found full-history work before local cache lookups and during
vegetation snapshot updates. A bounded derived index now addresses those caller
scans without rewriting authoritative terrain/fluid storage.

- `world/src/edit_index.rs` stores 16³ native buckets as 512-byte bitsets. A
  source-lineage-shared union retains candidate addresses from every snapshot
  branch. Each query checks candidates against its own immutable terrain/fluid
  maps before allocating result entries, so old branches and reverts cannot acquire another branch's overrides.
- The budget is 32 MiB, with conservative 1 KiB charges per 512-byte bucket plus
  a fixed overhead allowance. Reaching the cap marks the shared index incomplete
  and drops derived buckets; every subsequent query uses an exact full scan.
  Canonical edits, dormant water and dry tombstones are never evicted.
- `record_changes` feeds the index before the bounded delta journal, including
  hidden fluid-map updates whose visible world revision remains unchanged.
- `World::edit_cells_in_bounds(min,max)` returns sorted unique addresses within
  half-open native bounds. `edit_cells_in_columns` supports all-height consumers,
  including the native `i64::MAX` height through checked internal i128 bounds.
- Requested bucket ranges clip to the index's known extent. Queries choose the
  smaller of bounded coordinate enumeration and stored-bucket traversal. Public
  diagnostic counts distinguish bucket lookups, words, set bits, canonical-map
  checks and fallback history scans.
- NAADF edit indexing now requests its X/Z footprint once and derives edited
  columns from the resulting extrema. Cover fingerprints/leaf masks query local
  X/Z bounds. Vegetation production fingerprints are lazy snapshot-backed local
  queries; edited tree/ground assembly also requests only its actual footprint.
- Cover cache version moves from 5 to 6 because fingerprints now use sorted
  unique addresses, rather than the terrain-map-then-fluid-map chain (which could
  contain duplicates). Estimator arithmetic and visible content are unchanged.

Standalone optimized world tests (`rustc --edition=2021 --test -O
frontier/world/src/lib.rs -o /tmp/nov16-world-index-tests`) passed **128 tests,
6 ignored**. Four new ordinary index tests cover negative/extreme bounds,
source branches/reverts, hidden-fluid tombstones, bulk restore and cap fallback
against a full-scan oracle. The root coordinator still runs renderer caller
regressions and the final workspace suite.

The ignored `million_override_local_query_matches_full_scan_with_bounded_candidate_work`
also passed. Its synthetic history is one million Stone overrides at native
Y=16000, arranged as a 1000×1000 X/Z square. A 4×2×4 native query returns the
same 16 sorted addresses as full scanning:

| Work | Indexed query | Full-scan oracle |
|---|---:|---:|
| Retained override history | 1,000,000 | 1,000,000 |
| Bucket lookups | 4 | — |
| Bitset words visited | 256 | — |
| Set bits visited | 1,024 | — |
| Canonical map membership checks | 16 | — |
| History addresses scanned | 0 | 1,000,000 |
| One query elapsed time | 0.0144 ms | 9.225 ms |

Constructing that synthetic batch plus index took 281 ms. This is one optimized
CPU diagnostic on the development machine, not a controlled frame benchmark,
renderer speedup or whole-game memory result. Exact candidate counts and the
full-scan equality are the useful acceptance evidence.

Remaining limits are explicit: index saturation falls back to O(history), broad
requests may traverse all retained candidate buckets, and shared candidate
unions retain false positives from old branches until saturation. Authoritative
`Arc<BTreeMap>` copy-on-write can still clone a large map after a retained
snapshot; this change does not make canonical mutation chunk-local. The World
change journal likewise still falls back exactly when old lineage records have
been evicted. No shader, representation or canonical state changes are hidden
inside this indexing improvement.

## Reproduction requests

From `frontier/`, after sourcing the pinned `.toolchains/env.sh`, run tests
serially under the root build coordinator:

```sh
cargo test -p frontier-renderer naadf::data::cpu_tests
cargo test -p frontier-renderer vegetation::edit_tests
cargo test -p frontier-renderer local_edit_invalidates_only_overlapping_shadow_residents
cargo test -p frontier-renderer local_edit_during_shadow_transition
cargo test -p frontier-renderer ground_cover::shared_cache::tests
cargo test -p frontier-renderer ground_cover::cache_parity_tests
FRONTIER_GROUND_COVER_DIRECTIONAL=on cargo test -p frontier-renderer ground_cover::cache_parity_tests
FRONTIER_NAADF_COLUMN_CACHE=0 cargo test -p frontier-renderer source_column_cache_reuses_distant_edits_and_bypasses_only_affected_tile -- --ignored
FRONTIER_NAADF_COLUMN_CACHE=1 cargo test -p frontier-renderer source_column_cache_reuses_distant_edits_and_bypasses_only_affected_tile -- --ignored
```

Historical 1080p old/new walking is expressly negative for a whole-game win:
November 14 reports median means 24.73 → 24.84 ms and slightly worse new p95/p99,
with approximately 8% less aggregate native-stream CPU work and 6% less cover
leaf work. Those work totals include preparation and differ slightly in source
demand. They establish neither per-edit latency nor isolated cache benefit.
