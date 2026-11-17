# Bounded native whole-cell water

Greenfield engine integration of the `experiments/discrete_water` method. Every
occupied cell is exactly one native 0.0625 m cube. Numerical pressure, face
velocity, displacement and source emission clocks never contain fluid mass.

`DiscreteWater::new(origin, dimensions)` creates a closed region. Terrain must be
sampled into `set_solid` before adding water or sources. Out-of-region queries
are dry and `contains` distinguishes them from simulated cells. Boundaries are
impermeable by default; explicit open faces below allow accounted exchange.
Maximum allocation is two million cells. Native migration primitives and coupled
regional accounting are exposed here; `world::moving_water` composes them into
a camera-following controller. Neither layer simulates worldwide hydrology.

Each substep derives provisional face velocities from transported cell momentum,
applies gravity, projects nonnegative pressure with at most 32 alternating SOR
sweeps, then accumulates face displacement. Whole-cell proposals use injective
matching and dependency traversal: empty-ended chains and cycles publish
simultaneously, while stationary-ended chains are rejected. Accepted transfers
carry the complete reconstructed cell momentum vector. Rejected full-cell
requests cancel their displacement credit rather than accumulating a blocked
burst. Fair deterministic ties rotate using the saved tick. Greedy matching
can still reject legal better combinations; it is not globally optimal.

The adaptive substep uses velocity and gravitational acceleration, with a
0.65-cell target and 0.025 s maximum. There is no speed clamp. At most 32
substeps run per call; `StepStats.deferred_dt` explicitly returns unprocessed
time for the caller to retain. The projection's residual and divergence are
reported rather than hidden. Pressure can amplify a provisional velocity, so
this timestep estimate is not a proven post-projection CFL bound. The inexact pressure correction uses a scalar line search minimizing weighted
face kinetic guidance energy; it cannot increase that energy. Cell reconstruction
always averages both normal faces, including zero blocked/missing-face velocity.
Together with injective momentum transport, this gives a nonincreasing cell
kinetic guidance-energy bound when gravity and external supply are disabled,
up to floating point roundoff. The global line search can underproject an entire
region because of one poor local pressure solve; it is an explicit numerical
safeguard rather than physical viscosity. Stats report before/after projection
energy and how often its correction was reduced. Gravity still adds energy, and
whole-cell position quantization does not obey an exact kinetic-plus-potential
energy law. Pressure
iterations stop early on a small maximum update; this is a numerical stopping
heuristic, not a continuum convergence guarantee.

Sources default to 40 whole-cell attempts/second and can be configured with
`set_source_rate`. Their fractional emission phase is a clock only. Attempts
against an occupied source are counted as suppressed supply and discarded;
opening a blocked source does not release stored water. Outlets remove whole
occupied cells. The exact identity is:

```
ledger.added + ledger.source_added
  == mass + ledger.removed + ledger.outlet_removed
```

`source_suppressed` reports attempted supply that never entered the world and
is excluded from that identity. Terrain placement overlapping water or either
endpoint rejects the edit without changing the simulation. The runtime must
prevalidate `water_at`, `is_source`, and `is_outlet` before publishing the paired
terrain edit. Explicit `set_water` additions/removals update the same ledger.

`DWPROD04` snapshots (with backward `DWPROD03`/`DWPROD02` restore) store dimensions, integer origin, tick, exact ledger,
occupancy/solids, every numerical field's bits, endpoints and source clocks.
Restore validates lengths, bounds, finite values, flags, checksum and ledger.
Restart continues bit-identically on the same implementation/platform.
The checksum detects corruption; it is not authentication. Snapshot state is
41 bytes/cell in memory, 42 bytes/cell on disk plus fixed/source metadata.
Topology is implicit without adjacency lists; scratch memory and computation
are bounded by the region and candidate count. Work still scans the dense
region, so a small active region is required for interactive use. Momentum
reconstruction/projection and quantization are dissipative, and there is no
physical viscosity, surface tension, calibrated energy conservation or validated
Navier–Stokes convergence.

Run the focused library tests:

```
source /home/drew/Projects/voxels/.toolchains/env.sh
cargo test -p frontier-world discrete_water --lib --offline
```

Coverage includes still water, sustained waterfall/outlet transport and exact
ledger conservation, bit-identical restart continuation, corrupt restart,
atomic edit rejection, adaptive work limits, acceleration above 4 m/s, source
rate independence from substep partition, blocked supply suppression and legal
simultaneous chains/cycles versus blocked chains.

## Regional reservoir foundation

`regional::RegionalWater` accounts for large volumes without allocating their
individual voxels. This is a bounded accounting graph, **not an automatically
routed procedural river system**. A caller supplies at most 8192 reservoirs,
8192 directed connections and 8192 active detailed owners. Each reservoir has
an integer capacity, stored native-cell count and declared head in millimetres.
Connections carry a configured maximum integer cells/second only when the
source's declared head exceeds the target's. No terrain drainage graph,
channel cross-section, stage-to-volume relation, river surface or sea boundary
condition is inferred here.

Source, connection and outlet phases use exact integer nanosecond clocks.
`advance` accepts at most one second per call and processes sources, connections
in ID order, then outlets. A connection executes once per call. Downstream
connections can receive upstream water during that call; graph ordering and
time partition may affect the spatial distribution, while the integer ledger
always conserves volume. Blocked source attempts increment `suppressed` and
are discarded. Connections/outlets discard unavailable flow attempts without
banking future bursts. The regional identity is:

```
ledger.initial + ledger.supplied
  == stored_units() + detailed_units() + ledger.drained
```

`transfer` requires a globally increasing caller transaction ID; duplicate and
stale requests reject without any state change. Rejections do not consume IDs.
This is retry-safe rejection, not a persistent history of every transfer result.
The caller retains its acknowledgment. `advance` itself is a new clock advance
on every call; a transport delivering duplicate tick commands must deduplicate
those commands before invoking it.

`take_region` transfers volume ownership into a detailed domain and returns a
versioned token. The owning reservoir keeps matching reserved capacity, so
coarse sources cannot refill the borrowed space before detailed water returns.
`return_region` requires the token and the exact occupied-cell count; mismatches
reject rather than adjusting mass. `exchange_owned` moves whole units between
a detailed owner and a coarse reservoir, updates ownership volume and returns
a new token; the old token becomes stale. Callers must pair that exchange with
the local detailed occupancy transaction and persist both atomically. This
module alone cannot make cross-system publication/save atomic. Numerical
momentum and spatial water placement remain the detailed solver's responsibility.

The `DWRGN001` snapshot includes graph metadata, volumes, reserved capacities,
exact clocks, counters, transfer high-water mark and detailed ownership tokens.
Restore checks checksum, counts, IDs, capacities, ownership sums and conservation.
Tests cover source/connection/outlet counter sums, duplicate and stale transfer
rejection, failed-transfer atomicity, blocked source suppression, detailed
capacity reservation, stale ownership rejection, exact return counts and
bit-identical restart continuation. This accounting layer stays independent of geometry. `coupled::CoupledWater` and
`world::moving_water` now provide the paired runtime transactions and local routing.

## Explicit open faces on fixed detailed domains

`OpenBoundary` describes one actual face leading outside a nonsolid domain cell.
It supplies a global stage, exterior velocity and finite whole-unit import/export
quotas. `set_boundary(id, condition)` updates conditions and retains the face's
velocity/displacement state. `boundaries()` and `boundary(id)` expose remaining
quotas; `remove_boundary` returns that face to the closed-domain default. Sources
and outlets cannot overlap boundary cells. A physical face has one ID; aliases
reject rather than producing duplicate water.

This binary solver imposes zero free-surface pressure at the first **dry cell
center**, including inside the domain. Exterior stage is therefore quantized to
that identical lattice convention: the first dry y index is
`ceil(head_m / VOXEL_SIZE - 0.5)`. Exterior pressure is gravity times the distance
in cells below that index, in the solver's cell-coordinate pressure units. Using
continuum pressure at the occupied center instead would introduce a half-cell
pressure mismatch and drain an otherwise still lake across a tile seam. The
static open-lake regression specifically checks that this mismatch is absent.
This matches the existing quantized model; it is not a claim of continuum free
surface accuracy or a new smooth surface representation.

Exterior pressure acts as prescribed forcing before the unknown-pressure solve.
Open faces participate in divergence, pressure degree, pressure correction,
normal momentum reconstruction and displacement integration. External whole-unit
ports join the same injective dependency resolver as interior moves. An accepted
export can unblock an entire interior chain, and an import can occupy a cell
vacated in that same atomic assignment. Accepted incoming momentum includes the
projected normal face speed and the prescribed tangential exterior velocity.

Every accepted transfer appears in `StepStats.boundary_transfers` with boundary
ID, cell, direction, exactly one unit and its momentum. Only accepted transfers
decrement quotas or change the mass ledger. The detailed identity becomes:

```
added + source_added + boundary_imported
  == mass + removed + outlet_removed + boundary_exported
```

Finite quotas represent explicit supply/capacity valves. A direction whose quota
is exhausted has zero flux and increments `boundary_blocked` when pressure would
request it. Both exhausted directions close the face completely. Such unilateral
blocking and whole-cell assignment can leave divergence; that diagnostic is not
hidden. The prescribed exterior head can do work, so the no-external-force
kinetic-energy bound does not apply to a domain exchanging with these faces.
The timestep estimate includes exterior speed and prescribed pressure forcing.

The solver owns **no external reservoir**. Before stepping, the caller must
reserve quotas from a finite regional reservoir, then pair accepted reports with
regional ownership changes before publishing the detailed snapshot. Applying a
report twice, sharing a physical face between domains, or changing one ledger
without the other are caller errors; `coupled::CoupledWater` supplies an atomic
staging baseline for this contract. No moving-window remapping or automatic
terrain river routing is implemented by the boundary API.

`DWPROD03` snapshots include boundary conditions, remaining quotas, numerical face
state and import/export counters. Restore also accepts closed `DWPROD02` saves.
Wet pressure neighborhoods are cached once per substep in the original six-face
summation order; sweeps and residual calculations reuse them without changing
pressure iteration count, arithmetic or traversal.


Internal face work uses a list built from wet-cell neighborhoods and sorted in
original `(cell_index, axis)` order. Previously active faces that became dry
have their numerical fields cleared. This topology cache is derived, not saved;
restore reconstructs its required clearing set from persisted nonzero fields.
A matched 48×40×48 dam/source CPU run (160 requested 0.025 s steps, 849 adaptive
substeps, 27,168 pressure sweeps) produced the exact same final snapshot bytes
before and after both topology optimizations. On Ryzen 5 7640U, the stencil-only
baseline measured 24.46/41.46/42.45 ms p50/p95/p99 and active-face version measured
19.73/33.52/34.47 ms in one matched order. These are CPU simulation-call timings,
not full-game FPS; they still show a substantial remaining simulation cost.

## Exact connected-window migration primitives

`NativeBlock` exports a fixed detailed tile's exact occupied/solid flags,
cell momentum/pressure, positive-axis velocity/credit fields, source rate/phase,
outlets and explicit boundary snapshots. `DWNAT001` snapshots retain all these
bits. The nested cell-ledger baseline records that block's occupied count;
it does not create a second regional ownership ledger. The manager must retain
one global ledger and the tile's existing ownership token while dormant.

`DiscreteWater::from_native_blocks` assembles a connected cuboid from complete,
nonoverlapping blocks. `migrate` preserves every overlapping native cell field
bit-for-bit and imports only newly entered cells. Missing or duplicate coverage,
solid/water/endpoint overlap and invalid numerical/source state reject without
mutating the old simulation. Input count/cell budgets are bounded. Migration
records entered/departed whole units in `migration_imported/exported`; these
explain the detailed window's changing count, and are not external source/sink
supply in the regional ledger. Sleeping cells are not time-advanced or rebuilt
from a reservoir stage. Tests shift A+B to B+C and back to A+B, checking exact
saved tile bits, overlap, clocks, active-plus-dormant counts and restart behavior.

Positive native faces belong to their lower global cell, including faces that
cross tile boundaries. Exterior ports are separate outward-normal state. Export
their `NativeBoundaryState` before migration; its `positive_face()` converts the
normal velocity/credit into canonical positive orientation. A manager must write
that state to the lower-cell tile and, when appropriate, the new internal face.
`from_positive_face` converts it back when a face becomes exterior. A negative
exterior face can be owned by a dormant tile outside the detailed box; updating
that dormant owner is the manager's responsibility. Reassembly deliberately
unbinds all ports. The manager rebuilds/rebinds them and ownership in the same
staged transaction, rather than silently carrying stale heads or quotas into a
new geometry. Migration itself makes no pressure continuity claim when the
physical boundary changes.

`BoundaryGhost` optionally supplies the exact dormant outside cell's wet bit,
nonnegative solver-unit pressure and momentum. It overrides coarse head-derived
pressure, wetness and velocity. Core flux guards reject import from dry ghosts
and export into occupied ghosts independently of supplied quotas. Each accepted
import empties that ghost; export fills it and carries momentum. The manager
must apply the same event to the actual dormant cell and global ownership once,
with per-cell reservations and transaction rollback on mismatch. A sleeping
pressure field is lagged physical guidance, not a converged solve of the whole
world. Whole-unit conservation and exact dormant occupancy are distinct from
continuum boundary accuracy.

`DWPROD04` persists migration counters and optional exact ghosts, with backward
restore of closed `DWPROD02` and open `DWPROD03` checkpoints. Regression tests
verify old closed source clocks and old open face fields migrate without changing
their numerical state.

## Event-driven dormant relaxation planner

`dormant_relax::plan` supplies a bounded quasistatic simulation-LOD response to a
real accepted boundary exchange. Frozen exact ghosts alone cannot maintain flow:
a consumed border cell stays empty even when bulk water remains behind it.
The planner therefore supports an import vacancy and an exported filled cell.
It never runs merely because a tile loads, invents reservoir supply, mutates
world state or allocates more world tiles.

For a vacancy it searches only connected wet dormant cells for a real surface
donor with known dry space above, at or above the vacancy. It returns a chain
that shifts every whole unit one lattice neighbor and leaves the vacancy at
that surface. For a filled cell it finds a wet chain to a known dry destination
at or below the filled cell and returns end-first moves. Sources and targets
are injective; sequential application always encounters an empty target. The
source momentum vectors are permuted without scaling, preserving kinetic
energy, while the summed occupied-cell y coordinate cannot increase. Potential
energy lost by this coarse relaxation is dissipated rather than converted into
new momentum. This is an explicit quasistatic approximation, not the detailed
pressure projection and not a validated continuum solver.

The query callback must exclude the active cuboid and unknown/unmaterialized
cells. Unknown does not mean air. Shared work budget counts every distinct cell
query, including surface probes and unknown cells; per-plan memoization avoids
repeated expensive lookups. Breadth-first paths prefer shortest routes, then
higher surface donors or lower destinations, with deterministic coordinate
ties. No known surface in a submerged component means Pending, never an invented
interior bubble or a mass extraction from a buried cell.

The caller persists pending events and applies block mutations plus regional
transfers atomically. A StaleEvent means the triggering cell no longer has the
requested wet/dry state, so the controller can retire the superseded event.
Budget/unknown/unresolved events require explicit retry/work-debt handling;
they must not trigger source regeneration. Cross-tile hops update both exact
native counts and their regional reservoir records in the legal move order.
Existing Eulerian pressure should not be transported with momentum; the plan's
integer `equilibrium_head_y` provides a declared local hydrostatic estimate for
newly occupied cells. Pressure remains lagged guidance. Surface availability,
search limits and pending counts remain material accuracy/performance limits.

Planner tests cover legal import/export chains, exact counts and kinetic energy,
nonincreasing potential, deterministic tie choice, excluded unknown/active paths,
solid barriers, absent submerged surfaces, shared query limits and stale/invalid
state rejection.

## Counted environmental river flow clocks

`environmental_flow::EnvironmentalFlow` provides optional game-configured
whole-cell upstream supply and downstream removal budgets for continuing rivers
at the **outside of the materialized halo**. It has no cell, geometry, stage or
regional ownership authority. Rates are explicit integer native cells/second,
not calibrated drainage discharge, rainfall or inferred watershed hydrology.
Lakes require zero rates and remain finite by default.

Geometric reach segments normalize internally to `(source_version, owner,
segment=0)`. All sixteen pieces of a generated drainage edge therefore share one
clock; active inputs deduplicate after normalization. Configuring or encountering
a feature never emits water, resets its phase or refills a tile. Only `advance`
using actual simulated nanoseconds produces attempts. Inactive channels freeze,
with no wall-clock/offline catch-up. Calls accept at most one simulated second.

Each nonempty budget has a monotone serial and remains pending until settled.
The manager selects actual legal exterior continuation cells, stages native and
regional ledger changes, then calls `settle` with the accepted integer additions
and removals. Partial/blocked attempts are counted as suppressed upstream supply
or unsatisfied downstream removal and discarded. They never accumulate for a
later burst. Duplicate, stale, tampered and excessive settlements reject without
changing helper state. All outstanding budgets must settle before another clock
advance; a failed larger transaction must discard its staged helper copy too.

Feature state includes exact integer clock phases, active elapsed time, attempted,
accepted and blocked counters, and pending budget metadata. `DWENV001` checkpoints
preserve it across migration/restart. The helper's counts describe accounting;
the global regional ledger remains the authoritative external mass balance.
The manager must choose true upstream/downstream halo crossings, not interior
cells or newly loaded tiles, and persist helper/native/regional changes together.
A perpetual source represents modeled inflow from the unmodeled exterior world;
it does not claim the finite nearby reservoir became inexhaustible.

Tests verify one versus sixteen segment inputs produce identical snapshots,
source/outlet counts and phases are independent of simulated-time partition,
blocked flow never banks a burst, registration/inactivity never emit water,
lakes remain finite, settlements are atomic/retry-safe, and pending/settled
restart continuation is bit-identical.
