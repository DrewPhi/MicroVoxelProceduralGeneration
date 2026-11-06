# Issue #74: voxel methods review

Reviewed 2026-11-06. Research only; no external source was copied into the runtime,
compiled, or benchmarked. Legacy policy: **R** for this repository's architecture;
external implementations are algorithm references, not approved ports. Read
`frontier/AGENTS.md` and `ARCHITECTURE.md` first. Conclusions below distinguish
source observations from proposed experiments. A renderer paper is not evidence
that this project's exact 0.0625 m world meets its laptop frame budget.

## Decisions this evidence supports now

1. Establish depth/material/coverage agreement with the independent exact reference
   in the fixed player-height 300 m spiky scene before optimizing or polishing
   transitions. Add/remove, cave-mouth and bridge cases must also pass.
2. Compare a simple hierarchical brick traversal against the *same* traversal with
   optional directional empty-space caches. NAADF's bounded hierarchy is the most
   immediately testable idea; its dense global index is not our streaming design.
3. Keep material identity separate from display color compression. Aokana's
   occupancy downsampling is an explicit quality counterexample, not a conservative
   feature-preservation rule.
4. Budget full allocation and update traffic, including CPU mirrors, edit buffers,
   empty-space queues, temporal history and staging. Static compressed voxel bytes
   alone substantially understate the cost.

All A/B proposals below are future experiments, not measured wins. Use identical
source revision, camera, clipping, resolution and shading; report disagreement
counts, GPU/CPU p50/p95/p99, memory and upload bytes. Verify the actual Radeon 760M
adapter/driver and device features rather than inferring them from a product name.

## 1. Aokana — useful partitioning, unsuitable occupancy rule

[Paper, sections 3–4](https://arxiv.org/html/2505.02017v1).

The paper partitions geometry into shallow 256³ SVDAG chunks, uses 4³ occupancy
leaves, separates colors, and assigns chunk work to 8×8 screen tiles. Previous-frame
Hi-Z culling has a current-frame recovery pass. Visibility uses 64-bit atomic max.
LOD aggregates eight voxels and retains occupancy only when at least two are
occupied: a single occupied child can vanish. Distance/size selection therefore
does not certify silhouette or cave error. Preprocessed DAG/LOD data and static
scene measurements do not establish sustained edit or procedural-generation cost.
The reported San Miguel 64K case uses 424.11 MB VRAM but 23,233.09 MB disk; results
use RTX 3060 Ti, not 760M. Paper license is CC BY 4.0; no implementation license
was verified. These are paper results, not portable performance promises.

**Project inference/A/B:** retain shallow regional partitioning as a candidate;
test tile lists versus per-pixel brick traversal without changing geometry. First
test isolated one-cell pillars and diagonal ridges against that occupancy rule.
Require zero false empty results for canonical queries. Keep display reduction
separate. Measure tile duplication and total memory. Check 64-bit atomic support
before selecting this visibility implementation; a portable alternative may assign
exclusive pixel ownership and reduce candidate hits locally.

## 2. NAADF — inspect the implementation, including its dense floor

[TU Wien publication](https://www.cg.tuwien.ac.at/research/publications/2026/ulschmid-2026-naadf/)
describes nested axis-aligned distance caches, background updates and dynamic
overlays, plus a separate GI/temporal reconstruction contribution. Reported speedups
are relative to the paper's baselines; none is a prediction for our renderer.
The [repository README](https://github.com/cg-tuwien/NAADF) documents no LOD,
up to 16384³ cells, GPU generation, CPU editing, and a Windows MonoGame compute
fork. At 6.25 cm, 16384 cells span **1024 m**, not several miles.

Actual source reviewed (links track `main`, not a frozen commit; cloning failed
with network DNS resolution, so this review cannot claim a pinned checkout):

- [`rayTracing.fxh`](https://raw.githubusercontent.com/cg-tuwien/NAADF/main/NAADF/Content/shaders/render/rayTracing.fxh),
  `shootRay`: loads a chunk for each 16³ region, optionally a 4³ block and then
  packed voxel data. Empty entries encode six directional distances, with two bits
  per direction inside voxels/blocks and five at chunk level. Traversal intersects
  the resulting empty extent along the ray; this is not scalar SDF sphere tracing.
  Primary rays have a 120-step cap; entity candidates have a 16-entry array and
  entity traversal a 20-step cap. Those limits require explicit overflow/miss tests
  before reuse, particularly for long grazing views. Integer origin plus fractional
  local ray origin is relevant to precision, but does not prove global addressing.
- [`boundsCommon.fxh`](https://raw.githubusercontent.com/cg-tuwien/NAADF/main/NAADF/Content/shaders/world/data/boundsCommon.fxh),
  `ComputeBounds4`: three synchronized x/y/z sweeps over a 64-entry shared cell
  update directional bounds using neighboring empty-state/bound comparisons.
  [`chunkCalc.fx`](https://raw.githubusercontent.com/cg-tuwien/NAADF/main/NAADF/Content/shaders/world/data/chunkCalc.fx)
  hashes nonuniform blocks, compares their 32 packed words before sharing them,
  and collapses uniform blocks/chunks. This is limited block deduplication, not
  unrestricted deep-DAG editing. Hash insertion uses atomic reservation and bounded
  spin/probe loops; GPU scheduling assumptions and failure paths need review in a
  WGSL rewrite. Start without this concurrent construction complexity.
- [`EditingHandler.cs`](https://raw.githubusercontent.com/cg-tuwien/NAADF/main/NAADF/World/Data/EditingHandler.cs),
  `processChunks`: reconstructs each edited chunk's 64 blocks, hashes/reuses or
  allocates nonuniform voxel payloads, frees old references, and batches changed
  voxel/block/chunk records. CPU work includes locks and `Parallel.For`.
  [`ChangeHandler.cs`](https://raw.githubusercontent.com/cg-tuwien/NAADF/main/NAADF/World/Data/ChangeHandler.cs)
  performs a surrounding group flood fill (distance increments of four, queueing
  while the prior distance is below 28), then directional propagation. Thus edits
  have regional invalidation cost, not merely one changed word. Preallocated change
  arrays include 2M chunk pairs, 2M block words and 5M voxel words: about 44 MB CPU
  payload before other state or corresponding GPU buffers.
- [`WorldBoundHandler.cs`](https://raw.githubusercontent.com/cg-tuwien/NAADF/main/NAADF/World/Data/WorldBoundHandler.cs)
  allocates 96 queue slots per 4³ group of chunks and runs five bounded refinement
  iterations per update. The queue storage alone is 384 bytes/group, equivalent
  to six bytes per chunk. Background work has an adjustable dispatch budget.
  Disabling refinement is a useful correctness/performance comparison, provided
  bounds begin conservative and edits invalidate them safely.
- [`WorldData.cs`](https://raw.githubusercontent.com/cg-tuwien/NAADF/main/NAADF/World/Data/WorldData.cs)
  allocates a dense CPU chunk array and GPU 3D chunk texture, even for empty space;
  GPU entries are four bytes, or eight with entities. At 16384³ cells there are
  1024³ chunks: **4 GiB CPU chunk indices + 4/8 GiB GPU indices**, before payloads.
  Its bounds queue allocation adds **6 GiB** for that cube. These are arithmetic
  consequences of source allocations, not measured RSS. Shallow terrain reduces
  the vertical dimension, but this remains incompatible with blindly allocating
  a multi-mile cube on a laptop. Sparse/region paging is additional engineering.

**Portability/license:** compute traversal uses integer buffers, shifts and ordinary
floating arithmetic rather than hardware RT. That makes a fresh WGSL experiment
plausible, not a working Rust/wgpu port. Rewrite synchronization, resource formats,
queue capacities, boundary/NaN behavior and allocation strategy; account for shared
memory bandwidth on the 760M. The repository's
[MIT license](https://raw.githubusercontent.com/cg-tuwien/NAADF/main/LICENSE)
permits reuse with notice retention. Dependencies and imported scene assets require
their own review. No code was copied here.

**Small falsifiable A/B:** a bounded 128³ canonical 8 m region containing the same
spiky source, cave, bridge and an isolated inserted voxel. A = hierarchical
empty/uniform/dense bricks; B = identical hierarchy plus cached directional skips.
Compare exact hit cell/material/depth with caches cold, warm, disabled and rebuilding
during repeated add/remove. Fail on any skipped inserted cell or resurrected removed
cell. Record total steps, cache build/update time, invalidated region, bytes copied,
allocated CPU/GPU bytes and frame tails. Only broaden the footprint if the primary
visual reference and edit oracle pass; do not start with GI or 32-frame history.

## 3. Teardown / supplied GPC recording — access limit, no borrowed claims

The supplied [YouTube recording](https://www.youtube.com/watch?v=IM1Dr98f3xU)
and its oEmbed endpoint both failed retrieval (cache miss). No transcript, slides
or primary-source algorithm text was available in this review. Search surfaced
secondary descriptions referring to GPC **2025**, released in 2026; that is not
sufficient to certify the requested talk's date or technical claims. In particular,
do not attribute a specific bitmap hierarchy, Vulkan migration, RT requirement,
memory improvement or edit-time guarantee to this recording from those summaries.

**Applicability, memory, portability and license:** unverified for this talk. A
commercial game's destruction demonstrates a product, not this project's unbounded
procedural source, 6.25 cm canonical cells, horizon, streaming or 760M/wgpu budget.
No reusable source/license was verified. Keep it as a follow-up reference once a
transcript or slides can be inspected.

**Small falsifiable A/B:** retain our own benchmark of many independent edited
objects against one static world hierarchy, with identical geometry and measured
object-candidate overhead. This is a project experiment, explicitly **not** an
implementation of an algorithm verified from the inaccessible talk.

## 4. Atomontage / Virtual Matter — product evidence, hidden algorithms

The [official site](https://www.atomontage.com/) claims progressively streamed,
editable microvoxels and browser/mobile distribution. Its
[developer documentation](https://www.virtualmatter.ai/developers) describes
authoritative server simulation, browser WASM/native clients, SDK Lua, and server
voxelization of imported assets. This distinguishes a hosted platform from a
published local renderer implementation. The inspected primary sources do not
disclose the exact voxel hierarchy, codec, LOD occupancy rule, ray traversal,
error bounds, cache revision protocol, bandwidth cost or resident memory.

**Applicability/limits:** persistent edits and progressive delivery motivate
revision-keyed regional caches, but these pages cannot establish their algorithm
or prove exact 6.25 cm procedural terrain with several-mile visibility. Mobile
support does not establish Rust/wgpu portability or a 760M frame budget; server
work and transmission must be separated from client rendering. No renderer source
or open-source engine license was verified. Public SDK access is not permission
to copy an undisclosed engine. Do not infer octrees, surface-only compression,
bits/voxel or seamless topology-preserving LOD from marketing or forum guesses.

**Small falsifiable A/B:** our own canonical source with a fixed-size region cache:
A discards requested detail until ready; B retains coherent coarse coverage while
replacing pages by revision. Measure cold arrival, visible holes/false coverage,
peak pending bytes and edit-to-visible latency. This tests progressive availability
as a product requirement; it does not claim to reproduce Virtual Matter internals.

## Cross-cutting portability check and remaining evidence

[wgpu feature documentation](https://docs.rs/wgpu/latest/wgpu/struct.Features.html)
lists 64-bit integer atomic min/max as a feature that must be queried, not assumed.
Check the project's pinned wgpu version and actual adapter before importing
Aokana's visibility-buffer design. Compute software traversal can be evaluated
without choosing experimental hardware ray queries. None of this review ran on a
760M, so it supplies no new timing or visual acceptance result.

NAADF's TU Wien full PDF failed web retrieval because its 48.8 MB response exceeded
the tool limit; implementation details above come from actual raw source files,
README and publication abstract, not a purported full-paper reading. Aokana's
full HTML methods/results were accessible. Teardown's recording and Atomontage's
undisclosed engine internals remain explicit evidence gaps. Resolve those gaps
before promoting their alleged algorithms into architecture requirements.
