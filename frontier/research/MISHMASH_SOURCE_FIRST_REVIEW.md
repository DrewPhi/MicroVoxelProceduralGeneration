# Issue 75 Step 0: source-first research dossier

Research date: 2026-11-07. Scope: engine architecture, not terrain aesthetics or
simulation. This is a documentation-only investigation; no candidate is promoted.
Read with [issue 75](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/75),
[issue 74](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/74), and
[our integrated experiment](../reference/issue75/INTEGRATION.md).

## Conclusion and evidence limits

The accessible creator statements support trying **direct coarse source generation,
then compressed source caching, then GPU meshing if profiling justifies it**.
They do not establish that replacing our renderer is necessary, or that MishMash
preserves exact distant geometry. Our prior GPU appearance experiment moved a
later stage while retaining expensive canonical generation and certification.
That is not a test of the proposed source-level change.

This is a useful but incomplete Step 0 evidence collection. Reddit creator posts
and replies were readable. After the web reader failed, direct public YouTube HTML
retrieval succeeded for the requested video metadata and description. The linked
reply and full comment thread remain unverified. The ten comment CSV exports mentioned
in the issue were not present among the locally searched files or attached to this
conversation. Consequently, this document does **not** claim to have reviewed those
exports, watched the videos, or verified the missing comments. No proprietary
source was accessed. Missing claims below remain leads, not established facts.

## Primary-source register

All sources were accessed on the research date. Author of R1–R5: **u/MGMishMash**.
Exact publication timestamps could not be independently recovered. Search indexing
reported August 18, 2026 for R1 and October 3, 2026 for the R5 discussion, but cached
pages give inconsistent relative ages. Treat those dates as provisional; do not
infer an implementation chronology from them alone.

| ID | Exact source | Evidence and qualification |
| --- | --- | --- |
| R1 | [Improving Rendering Distance](https://www.reddit.com/r/VoxelGameDev/comments/1vrqluv/improving_rendering_distance_in_my_micro_voxel/) | Direct creator post: meshing rather than RT/DDA; resolution sieve throughout generation/features/stamps; independent macro edit caches; temporary transitions; visibility follows loaded bands. Self-reported 10–15 km at 45–50 FPS on M1 Pro, without a matched benchmark specification. |
| R2 | [Transition explanation](https://www.reddit.com/r/VoxelGameDev/comments/1vrqluv/comment/p4i855d/) | Direct reply: temporarily draw both levels, then retain one. Continuous blending looks better but costs more vertex/discard work. |
| R3 | [Vertex bottleneck reply](https://www.reddit.com/r/VoxelGameDev/comments/1vrqluv/comment/p4i8k3l/) | Reply text retrieved in R1's thread; permalink itself returned cache miss. About two thirds of his frame attributed to vertex work on that GPU. Self-report, not an independent profile. |
| R4 | [Finite world explanation](https://www.reddit.com/r/VoxelGameDev/comments/1vrqluv/comment/p4jug2r/) | Direct reply: roughly 24×24 km precomputed global heightmap; continuing local variation/caves outside it; connected island sectors described as a future plan. |
| R5 | [Initial engine and cache discussion](https://www.reddit.com/r/GraphicsProgramming/comments/1udkyzj/i_built_a_microvoxel_engine/), [cache reply permalink](https://www.reddit.com/r/GraphicsProgramming/comments/1udkyzj/comment/pdnll9t/) | Initial post: 0.1 m voxels, 16×16 km world, 40–100 FPS and 3.2 GB shared memory on M1 Pro. Later reply text available in parent thread: default 256 MB compressed LRU, about 5,000 64³ chunks; remeshing still required but cheaper than generation. Disk saves manipulated chunks. Permalink fetch unavailable. |
| Y1 | [MishMash95 channel](https://www.youtube.com/@MishMash95), [Generating Massive Micro Voxel landscapes](https://www.youtube.com/watch?v=xGkWWfO87no), [issue-linked reply](https://www.youtube.com/watch?v=xGkWWfO87no&lc=Ugyle83OSYHtF4xqZcR4AaABAg.A_daM4_PrsoA_eDaPTDGWr) | Video metadata/description retrieved from public HTML (`ytInitialPlayerResponse`): author MishMash, channel @MishMash95, published 2026-08-17T22:52:15-07:00 (August 18 UTC). Description reports Ultra at 45–50 FPS on M1 Pro, 5–6 GB system memory and 3 GB VRAM; explicitly acknowledges popping/transitions. Linked comment remains unverified. |

Crossposts of R1 in GraphicsProgramming, proceduralgeneration and gameenginedevs
are the same creator's account, not independent corroboration. Other commenters'
algorithms are not attributed to MishMash.

## What the evidence supports

**Direct coarse generation (R1).** The sieve applies to features and stamps as well
as terrain. His reported 1/64 chunk takes comparable generation time to a native
chunk while covering more space. This is not a promise of constant cost for arbitrary
edits or a proof of silhouette accuracy. Our inference: avoiding expensive fine
source evaluation matters more than merely reducing the number of rendered cells.

**Separate macro ownership and edits (R1).** Coarse representations can persist
without native copies resident. Deterministic prop IDs connect distant props to
changes. Distant grass/foliage is explicitly approximate. Our design should retain
an authoritative edit journal and revision-keyed derived caches; independent cache
lifetime must not become independently conflicting world truth.

**Temporary transitions and scheduling (R1–R3).** Creator reports roughly 30% lower
GPU cost for temporary transitions and prioritizes fine bands. Visibility can
shrink when streaming falls behind. These are disclosed quality tradeoffs, not
proof of invisible transitions. Our no-haze reference must remain available;
adaptive fog cannot pass the quality gate by hiding failures.

**Compressed source, not retained meshes (R5).** Recently visited chunks can be
decompressed and remeshed instead of regenerated. Cache benefit depends on source
cost, remeshing cost, hit rate and memory budget. Count props, metadata, decoder
scratch, in-flight jobs and hot meshes too. The 5,000-chunk statement differs from
the issue's 20,000-chunk claim; different contents/dates could explain it, but we
have not verified that explanation.

**Finite macro data and hardware (R4–R5).** The 16 km and 24 km descriptions may
reflect development, not a contradiction; dates are insufficiently reliable to
prove the sequence. A precomputed global heightmap is a different workload from
unbounded on-demand procedural evaluation. His 10 cm voxels also differ from our
6.25 cm cells. Neither FPS nor cache capacity transfers directly to this laptop.

The Y1 description supplies a larger memory budget than the initial R5 post.
Treat these as different reported configurations; do not sum overlapping unified
memory categories without knowing the accounting. No output resolution or matched
quality specification establishes a laptop-to-laptop comparison.

## Unverified claims and outstanding questions

| Claim from issue / research question | Current status | Consequence |
| --- | --- | --- |
| ~20,000 voxel-plus-prop chunks in 256 MB | Not verified; R5 supplies ~5,000 compressed 64³ chunks instead | Do not use either as our capacity estimate; measure representative payloads. |
| ~0.4 ms decompression versus ~7 ms generation | Not verified from accessible creator material | Treat as motivation only; measure both plus remeshing/publication. |
| GPU meshing, with CPU terrain generation | Meshing confirmed; processor split not established by retrieved statements | B3 remains a hypothesis, not a replicated MishMash implementation. |
| Separate background silhouette system | Not independently verified | Keep B7 a separately labelled perceptual experiment. |
| Specific frustum/occlusion culling scheme | Not established here | Do not invent Hi-Z, meshlet, octree or hardware-RT details. |
| Raster versus DDA full-pipeline comparison | Meshing choice is confirmed; no matched comparative benchmark retrieved | Cannot conclude raster universally wins, nor that faster DDA kernels improve total frame time. |

To finish source coverage, ingest the ten exports when available, preserve video
IDs and creator-reply links, distinguish creator from audience comments, and record
actual timestamps. Verify whether the performance numbers include props, compressed
metadata and remeshing. No need to request or reconstruct proprietary code.

## Independent technical support (not claims about his engine)

- **Asirvatham and Hoppe, GPU Gems 2 (2005):**
  [GPU-based geometry clipmaps](https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry)
  use reusable nested grids, incremental updates and GPU terrain processing.
  This supports separating source data from draw topology. It is a heightfield
  technique, not a solution to arbitrary cave/bridge/edit ownership, and its
  historical FPS figures are not predictions for us.
- **Zstandard maintainers:** [official source and benchmarks](https://github.com/meta/zstd).
  **LZ4 maintainers:** [official source and benchmarks](https://github.com/lz4/lz4).
  These provide a sensible ratio-versus-latency comparison for B2. Public general
  corpus throughput cannot predict small terrain-block latency. Freeze versions
  and check licenses when introducing a dependency; this pass copies no code.

## Research → experiment matrix

The order below follows updated #75. All implementation follows Step 0.5 user review.

| Priority | Concept and local evidence | Smallest falsifiable experiment | Decision |
| --- | --- | --- | --- |
| B1 | Source-level coarse generation. Existing GPU profiles retain canonical source work; [integration](../reference/issue75/INTEGRATION.md) did not establish an end-to-end win. | CPU fixture for `generate_visual(region, spacing, source_revision, edit_revision)`, feed existing adaptive geometry. Count expensive feature/noise evaluations separately from lookups. Compare native oracle, feature bounds and requested-quality readiness. | At least 2× fewer expensive evaluations and >20% readiness improvement above variance, preserving strict quality/edit gates; stop or redirect if generation is not dominant. |
| B2 | Compressed cold canonical source cache, distinct from hot draw products. Existing [source page cache](../renderer/src/source_page_cache.rs) and [derived disk cache](../DERIVED_DISK_CACHE.md) are not evidence that this exact two-tier experiment has won. | Replay revisit routes with no cold tier, Zstd and LZ4 at equal total memory. Include Legacy, Regional, empty, dense, cave and edited payloads. Measure compression, decompression, remesh and publication latency. | 2× useful retained history and >20% revisit readiness improvement, no unacceptable cold or tail regression. |
| B3 | GPU meshing, conditional on CPU meshing cost. Moving appearance alone was insufficient. | One revision-keyed source brick to bounded GPU face output, explicit overflow fallback, matched native faces and no redundant CPU geometry. Include upload/sync/publication. | >10% end-to-end benefit or material edit-latency gain; reject kernel-only wins. |
| B4 | Draw batching independent of ownership; vertex pressure is a possible bottleneck, not automatically ours. | Pool/batch an identical fixed visible set; retain culling and edit identities; measure CPU submission and GPU work separately. | Keep only an attributable whole-frame gain at unchanged images/memory limits. |
| B5 | Independent coarse edit cache supported by R1. | Edit a cave/tower across boundaries, evict native data, reload coarse and native representations. | Exact authoritative edit parity, correct invalidation, no disappearing volume owners. |
| B6 | Idle compressed prefetch | After B2 wins, compare same route and budget with/without prefetch; cancel on direction change. | Revisit benefit without starving forward/nearby work or inflating tails. |
| B7 | Background silhouette; creator implementation unverified | Separately labelled permissive visual mode, matched full-resolution captures and motion review. | User acceptance does not change strict reference thresholds. |
| B8 | Specialized raster macro surface plus voxel volume owners | Existing page/hybrid candidates first; cave/bridge and edited topology alongside landscapes. | Better total cost at acceptable appearance; no SVO/DAG adoption for novelty. |

### B1 mathematical/correctness boundary

Sampling the full generator less often is not enough if each sample still performs
all fine work. For an additive noise source with bounded terms
`h(x)=sum a_k n_k(x)`, `|n_k|<=1`, omitted terms have height bound
`E<=sum |a_k|`. That supplies a conservative uncertainty, not permission to silently
erase a feature. Domain warps, nonlinear erosion, material selection and Boolean
caves need their own propagated bounds or exact fallback. Keep feature IDs and
world-coordinate alignment stable across resolutions. Thin features require explicit
ownership/refinement even if the coarse samples miss them.

Test adjacent regions at different spacings, negative coordinates, source revisions
and edits. Native gameplay queries remain exact. Capture errors without fog and
include motion; a still image cannot establish stable transitions.

## Immediate next gate, not a renderer rewrite

Step 0.5 asks the user to review the **existing uniform GPU page candidate**. It
failed the strict numeric gate but has not been personally rejected by the user.
Show matched, unaltered adaptive/page images and equal crops; label composites and
heatmaps. Include the exact oracle where practical. The issue's ~1.14 s page
preparation and ~10.24 s adaptive requested-quality figures are different fidelity
and work, **not an equal-quality speedup**. Preserve reported RMSE/coverage errors.

Only after explicit keep/specialize/reject feedback proceed into B1–B3. A promising
still needs motion comparison before promotion. #74 owns the shared benchmark and
production gates; teleport remains lower priority than normal travel. This research
pass does not complete #75, Step 0.5, or #74.

## Subsequent user decision — 2026-11-07

The user inspected the adaptive/page comparison, agreed the adaptive (left) image
is better, and authorized B1, B2 and conditional B3. Step 0.5 is complete; retain
adaptive as the baseline. This decision does not resolve the evidence gaps above.
