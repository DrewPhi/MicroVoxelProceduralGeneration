# Microvoxel world foundation plan

Status: the shared core, compact edit storage, transactions, save/load, queries,
meshing and native rendering are implemented. A native wgpu/winit application now
uses color-preserving face merging, bounded workers/GPU caching and a balanced,
grid-aligned LOD cover with explicit boundary faces. Native rendering is the
active game target. Obsolete browser and image-to-OBJ tools have been removed.
Material property maps remain empty.
Tick/ID/active-region primitives exist, but simulation systems and player editing
controls remain deferred. This plan describes the longer-term direction; it does
not claim artificial life, circuits, crafting or general body physics are built.

## Objective

Build a small Rust world core that can support editable terrain, construction,
full water voxels, and future material/organism simulations while keeping memory
bounded and performance measurable on a basic laptop. Preserve the existing
terrain generator, canonical painting, native renderer and useful diagnostics.

Extensibility means that storage, rendering, scheduling, and simulation algorithms
can be replaced behind tested contracts. It does not mean every future model can
be efficient without changes, or that every voxel in a vast world runs every tick.

## 1. Authoritative world rules

- One voxel edge is exactly 1/16 meter. Use signed integer voxel coordinates.
- Cell coordinates denote cell minima; a cell owns a half-open unit cube.
- Use Euclidean division/remainders for negative chunk coordinates.
- Camera/player/body positions are grid positions. Camera angles can be continuous.
- Visual interpolation may be added later without changing logical positions.
- Speed/force integration may retain fractional *movement credit* internally;
  it never creates an occupied position between grid cells.
- Distinguish spatial granularity from scalar precision: energy, charge, damage,
  and accumulated movement need defined units and adequate numeric precision.
- A cell has one primary material. Water occupies complete cells initially.
- A voxel has one base color on all its faces. Lighting changes brightness.
- Rendering LOD and camera position never determine authoritative voxel contents.
- Unknown/unloaded data is distinguishable from known air.
- Procedural baseline generation depends on world seed, generator version, and
  coordinates, not generation order or the thread performing the work.

A movement query must check all crossed grid cells, even if a body moves multiple
cells in one tick. A renderer can approximate distant surfaces; collision,
editing, and simulation use authoritative data.

## 2. Shared world core

All consumers use the same world access and change rules:

- Query a cell, a compact column, or a bounded region.
- Read immutable chunk snapshots with revision numbers.
- Validate and commit a bounded batch of edits/transfers atomically.
- Report affected cells, chunk boundaries, and subsystem invalidations.
- Address entities/bodies by stable IDs rather than pointers into storage.
- Save and restore authoritative data separately from disposable render caches.

Keep integer coordinates and material IDs explicit types. The API should support
3D cells immediately, even while the default terrain generator uses compact
height columns. Digging below a height surface must expose existing material,
not regenerate an intact surface on the next frame.

## 3. Small base cells, optional state

Use a compact material ID for ordinary cells. Shared material definitions can
contain density/mass, appearance, opacity, friction, cohesion, bond strength,
electrical/thermal conductivity, and optional biological/resource tags. Only
implement fields with an immediate consumer; retain room for versioned extension.

Avoid a large per-voxel object containing health, charge, organism identity,
velocity, water state, and light values for every rock and air cell.

Allocate extra state only where needed, using chunk-local arrays/bitsets or
subsystem stores. Examples include damage on a stressed beam, charge on an active
conductor, and health on an actual living leaf. An organism's shared energy
account belongs to the organism, not duplicated in every constituent voxel.

Base color must be independent of face direction and camera LOD. Static terrain
can derive appearance seeds from coordinates. Moving body cells need stable local
identities/seeds so their color does not reroll when they move. RGB need not be
stored separately for every generated cell if it can be reproduced from compact
material and appearance data.

## 4. Storage and streaming

Start with a procedural baseline plus persistent changes, not a dense allocation
of the whole world. Benchmark compact columns, uniform blocks, palette-packed
bricks, and expanded active arrays on the same workloads.

A likely design is:

- Spatial region index and cached macro-generation data.
- Small voxel bricks that support uniform, compressed, and expanded forms.
- Dense contiguous access for frequently modified active bricks.
- Sparse overrides for lightly edited procedural terrain, promoted to brick
  storage when edits become dense. Air tombstones must preserve excavations.
- Per-system active bitsets and neighbor-change notifications.
- Explicit CPU/GPU cache limits and eviction policies.

Prototype 16³, 32³, and 64³ bricks (1, 2, and 4 meters at our resolution). Choose
sizes using measurements; render tile size need not equal storage brick size.
A global octree is not a prerequisite. A hierarchical index can be introduced
behind the region API if benchmarks justify it.

Track dirty regions by purpose: geometry, appearance, lighting, connectivity,
and simulation. A color change should not automatically regenerate terrain or
rebuild every mesh. Boundary edits invalidate the appropriate adjacent caches.

## 5. Procedural generation and persistence

Retain our noise/erosion code as a generator that supplies the initial world.
Its continuous mathematics are implementation details; it commits to discrete
cell results at the canonical voxel resolution.

First preserve the existing finite 10 km region and cache its macro data. Then
remove fixed-world assumptions from the core. Cross-region hydrology requires
shared drainage/outlet constraints or a larger macro network; overlapping noise
samples alone will not guarantee consistent rivers and sediment flow.

World saves need seed, generator/material versions, tick, persistent edits,
entity/body state, simulation activation state, and pending transfers/events.
Do not silently regenerate old saves with changed generation rules. Preserve
stable IDs, and include migrations or explicit compatibility rejection.

## 6. Simulation model

Use a fixed logical tick with explicitly scheduled subsystem rates. An initial
20 Hz world tick is a benchmark candidate, not a promise that every future
mechanical model is stable at that rate. Systems may require bounded substeps.

Read a snapshot, produce proposed changes, resolve conflicts deterministically,
and commit. This supports parallel work without concurrent arbitrary mutation
of the same cells. Reuse scratch buffers and process local contiguous batches.

Schedule changed and active regions rather than scanning all cells. Expose work
budgets, backlogs, and activation rules. Do not silently drop material, events,
or simulation time when a budget is exceeded.

Keep simulation activation distinct from rendering visibility. Headless runs
must work without a camera. Initially use an explicit bounded simulation zone;
outside it, persist dormant state. Later introduce documented regional
approximations. Reduced ecological simulation is an approximation and must be
validated; it is not guaranteed equivalent to detailed local simulation.

Deterministic random streams should be keyed by system/entity/tick or explicitly
stored. Thread completion order must not change outcomes. Specify numeric
precision and determinism guarantees rather than assuming integers alone provide
cross-platform reproducibility.

## 7. Capability extension points

These are requirements for interfaces, not a mandate to build each system now.

### Digging, building, and crafting

Atomic material transactions can remove/place cells, transfer inventory items,
and consume recipes. Validate occupied destinations, permissions if introduced,
and resource accounting. Persist edits and invalidate only affected caches.

### Structures, trees, and detached objects

Separate spatial occupancy from a body's structural connectivity. Wood cells
can have bonds and belong to a tree/branch graph. Begin with support/load
approximations and explicit failure thresholds; connectivity to the ground alone
does not model bending strength or torque.

Bond failure can split a connected component into a detached body while retaining
its material, mass, identity, and biological state. Moving a detached body requires
coherent multi-cell collision and an atomic occupancy transfer. This is more than
running the camera's movement code on many independent blocks.

### Discrete water

Begin with whole-voxel water transfers under gravity and local pressure-like
rules. Resolve competing destinations in a second phase, conserve voxel count,
and keep cross-chunk transfers pending until committed. One water voxel represents
1/4096 m³, about 0.244 liters.

Rivers/lakes from generation provide initial water occupancy. Rendering does not
simulate flow. Stable lakes should sleep; disturbed frontiers wake. Pressure,
buoyancy, and coupled floating structures are later models with separate costs.

### Light and plants

Track optical properties by material and cache daylight/occlusion fields. Refine
updates only in affected regions when branches/leaves/terrain change. Shaded
rendering pixels are not an authoritative measure of biological sunlight.

A plant owns a genome, growth strategy, resource accounts, and structural graph.
L-systems or other growth rules propose new cells, but occupancy, resources,
maintenance, and structural constraints decide whether growth succeeds.
Individual leaves may have exposure, maintenance, health, and attachment state.
Those values aggregate into the plant's resource budget. A failing leaf or branch
can detach through the same structural/body system used elsewhere.

### Organisms and evolution

Keep genomes/controllers distinct from material cells and from simulation rules.
Heritable rules propose sensing, growth, reproduction, and actuator actions. Costs
and constraints apply equally to all organisms. Movement/actuation must consume
energy and cannot create mass or obtain unlimited motion from rounding rules.

Strict grid-only deformation introduces directional bias and requires a chosen
mechanical model. Start with an explicit lattice actuator model. Do not assume
continuous soft-body simulation can be reproduced merely by snapping positions.
Evolution is evaluated through reproduction/resource survival, with reproducible
experiments and lineage records. It is not implemented as a rendering feature.

### Electricity and other transport

Expose material conductivity and connected networks, with sources, sinks, and
stored state only on relevant cells/components. Start with a simple, explicitly
specified electrical model before considering full circuits or electromechanics.

Electrical current, mechanical load, nutrients, heat, and light may share spatial
neighbors and invalidation mechanisms. They must not be forced to share one graph
or one update equation; their connections and conservation rules differ.

## 8. Source organization

The implemented layout has a dependency-free core crate and a native application
crate. The core's `src/bin/` holds the noise-image diagnostic and query benchmark.
No game engine is required.

Current boundaries:

- `grid`: coordinates, bounds and units.
- `materials` / `painting`: definitions and canonical per-voxel appearance.
- `world`: reads, snapshots, transactional changes and revisions.
- `storage`: edited bricks and overrides over the procedural baseline.
- `generation`: terrain algorithms and the current macro-region provider.
- `simulation`: tick, active-region and stable-ID primitives; systems deferred.
- `meshing`: bounded general 3D surface extraction.
- `render`: color-preserving merging, terrain LOD and boundary stitching.
- `persistence`: versioned edit data and baseline compatibility checks.
- `native/src`: window, camera, GPU renderer, streaming and benchmarks.

Keep these boundaries clear enough to change compression, move hot code or add
systems without having those systems independently redefine world occupancy.

## 9. Implementation order and gates

| Stage | Deliverable | Evidence required before expanding |
| --- | --- | --- |
| 0. Preserve baseline | Version source, seeded scenes and benchmark evidence | Current native baseline reproducible |
| 1. Discrete contracts | Shared grid/material/world types; canonical cell queries; full water cells; one face-independent voxel color | Coordinate/negative-boundary tests, view-independent contents, visible side-color/water checks |
| 2. Editing and persistence | Dig/place across brick boundaries; sparse edits; save/load | Excavations survive reload; untouched regions remain implicit; local mesh invalidation |
| 3. Renderer integration | Rendering, collision, selection, and logical camera positions use core | Grid positions verified; smooth angles; near/far LOD changes never change cell identity |
| 4. Streaming and budgets | Bounded caches, region provider, profiling, job limits | Long travel/turning/editing runs show bounded memory and no unbounded queues |
| 5. Simulation substrate | Headless ticks, active sets, proposals/commit, state hashing | Same inputs reproduce same results; ordering/conflict and conservation tests |
| 6. Small capability probes | A water transfer, a loaded/breakable beam, and an energy-consuming grid actuator | Cross-region transfers conserve resources; body split/motion preserves identity and occupancy |
| 7. One integrated experiment | Optional small light/growth/failure/detritus/consumer scene | Systems compose through shared world changes; no one-off bypasses of core rules |

The integrated experiment is a test of the foundation, not a promise to implement
the exact tree/bug scenario. Select final capability probes before Stage 6 based
on which assumptions remain least proven.

## 10. Measurement and correctness

Record actual laptop hardware, build settings, seed, active volume, edit rate,
visible distance, and population. Distinguish generator time, simulation time,
meshing time, uploads, and rendering. Track p50/p95 frame and tick time, memory,
bytes per modified brick, pending jobs, active cells, and generated regions.

Initial targets to validate (not measured guarantees):

- Aim for a 30 fps inspection view, with bounded simulation work below its frame
  budget; measure subsystems independently and choose a simulation allocation.
- Give resident world/simulation caches an explicit configurable memory budget,
  initially testing around 256 MiB, separately from GPU/driver overhead.
- Keep storage/renderer queues bounded under rapid travel and mass edits.
- Support a headless experiment runner to compare algorithms and simulation cost.

Core tests include negative coordinates, chunk boundaries, generation-order
independence, atomic conflicting edits, save/load, water/material/energy accounting,
stable appearance under movement, body splits, swept grid movement, and unchanged
world state across camera/LOD changes. Camera movement math can use internal
credits, but visible logical positions must remain grid aligned.

For tree/light or circuit approximations, document what is conserved and what is
an external source/sink (sunlight, exported water, dissipation). A conservation test
cannot substitute for checking whether a physical approximation behaves sensibly.

## Research context

These inform possible techniques; none is a required dependency or a drop-in
implementation of our strict grid-only model.

- OpenVDB overview: sparse/uniform storage and iteration over active values.
  https://www.openvdb.org/documentation/doxygen/overview.html
- Palubicki et al. (2009), Self-organizing tree models for image synthesis:
  growth shaped by light/space competition and internal regulation.
  https://www.algorithmicbotany.org/papers/selforg.sig2009.html
- Hiller and Lipson, Dynamic Simulation of Soft Heterogeneous Objects, and
  Voxelyze: material/voxel mechanical simulation. Its continuous deformation model
  is a reference, not the same as our proposed discrete lattice actuation.
  https://arxiv.org/abs/1212.2845
  https://github.com/jonhiller/Voxelyze
