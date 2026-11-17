# Local native voxel water (experimental)

Live water is opt-in. Existing Alpine, Regional, Legacy and Seeded World launches keep their static source behavior unless `--live-water on`, `--water-demo` or `--water-model` is supplied. The first sections below describe the original CA path (`--water-model ca`). The supplied whole-voxel waterfall (`discrete`), fixed generated river patch (`procedural`) and player-following water (`moving`) are separate experimental paths described later in this document.

## Original CA path

```sh
cargo build --release --locked
./run.sh \
  --preset seeded-world --seed 19 --water-demo \
  --water-save output/water-demo.save
```

The demo creates a two-chamber stone basin at the starting location. Its left chamber contains explicitly added water. At fluid tick 20 the central dam opens, and water transfers into the right chamber. Press G to open it earlier. Player editing defaults on in live-water mode: press left to preview, release quickly to pick up one voxel, or hold up to 900 ms to pick up a connected handful (at most two native voxels at the default 0.5 L cap). Right click places one owned voxel of the selected material; keys 1–6 select Grass/Soil/Stone/Wood/Sand/Snow. Inventory starts empty and holds 64 cells by default. One native cell is 0.244140625 L; tuning the hand volume below that still selects one cell and the HUD reports the grid minimum. Reach is 2 m and requires current native ownership. Opening a menu or losing focus cancels a pickup. Placement into water is currently rejected. Use `--player-edit off` for the explicit creative/debug left-remove/right-Stone path and P counted water additions. F toggles grounded movement/free flight. Each occupied cell renders as a complete 6.25 cm voxel, including partially filled cells; integer mass does not create a fractional-height water surface.

For a finite reproducible diagnostic use the same seed and source settings with `--water-demo --seconds 10 --capture-on-exit output/water-flow.png --report output/water-flow.json`. Physics ticks are fixed 0.1 seconds, with at most one tick per rendered frame and bounded wall-clock debt. Slow frames slow elapsed physics rather than issuing an unbounded catch-up loop. A demo's dam opening uses its saved tick, not wall time. GPU publication is asynchronous, so capture/report the published revision as well as the fluid tick when assessing visible flow. Live `--capture-on-exit` stops physics and waits for native publication before taking its final GPU image; measured frame statistics exclude that final settlement. A fixed activation schedule and edit sequence replay deterministically; camera/frame-dependent activation is part of that schedule.

`--water-save PATH` loads an existing save before the first renderer build and writes an atomic replacement on clean window exit or completion of a finite run. The FVWATER3 checkpoint contains source identity, terrain edits, finite inventory and action sequence, fluid schema/configuration, tick, integer masses, dry tombstones, dormant cells, loaded regions, awake/sleep counters, pending displacement, additions/removals and demo state. Old FVWATER2 checkpoints import with an empty inventory. Reducing the active radius on reload deactivates excess saved regions into dormant history before applying the new resident capacity. Reuse the same seed, recipe and terrain parameters; a mismatched source is rejected. Saves are authoritative state, separate from disposable terrain/appearance disk caches. Exit serialization and file synchronization happen after the rendering loop; abrupt process termination does not save recent work. Remove/use another save path to create a fresh demo.

Activation is bounded to 27 one-metre regions (110,592 native cells) around the player's feet, or around the nearby demo. At most two regions initialize each rendered frame. Every absent neighboring region is impermeable. Unloaded cells retain dormant mass; loading them resumes exchange without regenerating consumed water. The default history limit is 1,048,576 stored cells, including dry tombstones: reaching it pauses new activation and reports the condition, preserving history. Use `--water-history-cells N` (32,768 through 16,777,216, at least enough for the complete active cube) to increase the cap, including when reloading a checkpoint. Dormant cells currently cost roughly 40 bytes each in the core simulator, in addition to save serialization and renderer memory. `--water-active-radius 2` expands the cube to five metres (512,000 cells); this is an expert setting with substantially higher tick cost. The default radius 1 uses three metres (110,592 cells). This is an intentionally bounded local prototype, not unlimited persistent world streaming. Resident and dormant history can be inspected through the live-water JSON report and periodic console diagnostics.

Terrain solids and fluid state are separate. Placing stone preserves displaced mass in a pending store when no adjacent loaded space is available. Removing terrain wakes neighboring water. The canonical `World` and `WorldColumn` apply the same fluid occupancy overlay; dry overrides prevent static generation from refilling drained cells. Each tick batches visible occupancy changes into one world revision; mass changes with identical occupancy do not invalidate geometry. Native publication replaces changed metre roots rather than rebuilding the whole nearby volume. Root-level empty-space skip proofs that overlap the persistent edited-root bounding box use a conservative one-root fallback after edits, preventing an old air skip from hiding newly added water. Proofs disjoint from that box and exact block/leaf data remain valid. Existing experimental GPU-distance/height/beam modes do not provide the same dynamic safety contract.

The live driver requires the adaptive scene and NAADF enabled with following enabled and GPU root distance disabled. Dynamic edits must stay inside the resident native region for faithful presentation. Static distant appearance still derives from the immutable procedural water/terrain source; dormant edited cells outside native ownership remain authoritative in saves and CPU queries but do not yet have independent far-distance volumetric geometry. Do not interpret a distant static shoreline as a fluid-state rendering guarantee. This CA path has no swimming, transparency/refraction, pressure solver, rain/ocean reservoirs, erosion, accumulated watershed discharge or loading-order-independent elapsed physics.


## Research and implementation provenance

This is a greenfield integer implementation. [Heintz et al., The Game of Flow (2017)](https://downloads.hci.informatik.uni-wuerzburg.de/Heintz2017aa.pdf) motivates local conservative transfers, horizontal diffusion, gravity, and evaluating each face once. Its published GPU timings do not predict this CPU implementation's cost or this laptop's integrated renderer performance. We use fixed integer capacity, deterministic proposal/acceptance, explicit dormant state and displaced-mass bookkeeping. This is a game-oriented cellular automaton; it does not solve incompressible Navier–Stokes equations.

The source-free `cargo run -p frontier-world --release --example fluid_probe` reports bounded cell visits and conservation for an isolated box. Use the actual viewer's reports for combined physics, generation, publication and rendering costs. A sleeping box's median tick time is not an active-flow performance measurement.


## Physical solver research and current playtest

The original in-game CA path remains available. The optional whole-voxel
`discrete`, `procedural` and `moving` paths described below use pressure/momentum guided
quantized transfers and remain experimental. The separate
[CPU physical reference](research/nov16/PHYSICAL_WATER_M0.md) adds pressure,
velocity and geometric volume transport for offline numerical validation.
Its [acceptance status](research/nov16/PHYSICAL_WATER_M1_STATUS.md) records unresolved
dynamic energy/refinement gates, so it is not promoted into gameplay or a GPU
backend. The initial long still-water failure was corrected and retested. Accurate mass alone does not establish a correct liquid simulation.

The [November 16 checkpoint](research/nov16/README.md) covers player-edit controls,
the four audit fixes and optional representation experiments. The viewer's
input acceptance check is `python3 tools/player_edit_check.py --output
output/my-edit-check`; it drives only its own child window and verifies the saved
inventory/action ledger. Keep simulation saves when deleting disposable caches.

## Whole-voxel supplied waterfall demo

Select `--preset seeded-world --water-model discrete --water-save PATH` to use
binary native-voxel occupancy, pressure/momentum guided transfers and continuous
counted supply/outlet boundaries. This is a bounded 48 × 40 × 48 cell (3 × 2.5 ×
3 m) demonstration, with a raised supply shelf and a receiving basin with an
initial shallow pool. It is not procedural river activation or a validated
continuum solver. Region borders are sealed; digging the basin redirects water
within this region. Terrain pickup/placement also works outside it, where this
demo does not simulate water. Source and outlet cells are protected against
placement; wet-cell placement rejects rather than silently deleting water.

Simulation runs on a dedicated worker with one owned state and at most one
completed response. Terrain/inventory transactions are accepted only while the
state is local; an action during an unfinished step reports a retry message.
Every accepted edit changes the worker's solid state before the next step.
Occupancy and its dense GPU snapshot publish together; terrain rendering uses
a cached fluid-free source that changes only after terrain edits. Worker work
never clones or discards an authoritative simulation because a newer frame is
requested. Deferred simulation time is retained and saved. Slow workers can
reduce simulation speed; reported `simtime` is actual simulated time.

Discrete saves have an independent `FVDWATR1` envelope; existing CA `FVWATER2/3`
files are preserved and remain readable with `--water-model ca`. The discrete
checkpoint includes source identity, terrain edits, inventory counts/action
sequence, source/outlet metadata, binary occupancy, momentum, pressure, transfer
credits, source emission clock, simulated time and deferred time. Clean exit
drains the single pending step before the atomic save. Reusing a CA path for a
discrete run fails with a format diagnostic rather than converting it.
`wall_elapsed` and `wall_lag` disclose simulation slowdown relative to scheduling
wall time; they survive restart. No unbounded catch-up queue is created.

A click during an unfinished worker step queues one action for at most 500 ms.
Commit rechecks material/inventory sequence, native ownership, reach and player
body clearance. Moving more than 25 cm, changing heading, opening the menu or
teleporting cancels the queued action. Further clicks wait for that action.
After terrain edits, physics and new water GPU uploads wait for the native
renderer to publish the matching fluid-free solid source revision; previous
coherent occupancy remains visible while that source rebuilds.

## Fixed procedural river patch comparison

```sh
./run.sh --preset seeded-world --seed 19 --water-model procedural \
  --water-save output/procedural-water.save
```

This separate experimental mode finds a shallow bank on an existing generated
river near the requested start and positions the camera beside it. It creates
no stone basin and adds no water or source rate. One fixed, world-aligned 16³
native tile (1 m³) runs the detailed solver; six adjacent finite spatial tiles
provide prescribed-head open faces. It does **not** follow the camera, simulate
an entire river/lake, accumulate watershed discharge or implement regional rain.
The existing static procedural world continues outside these seven tiles.

Each tile's initial binary occupancy, solid mask and per-layer capacity come
from canonical columns, including terrain edits and dry fluid overrides.
Versioned reach identity uses the source version, drainage-node owner and path
segment. Revisit registration never regenerates initial water or repeats source
supply. Reservoir IDs are allocated from full spatial keys and persisted; hashes
are not used as authoritative identities. No discharge is inferred from river
width. Explicit regional source rates are supported by the accounting API but
are not configured by this runtime.

The detailed owner has a separate finite reservation bucket. Every accepted
whole-cell boundary transfer commits the local state and regional ownership
ledger together. External head updates use the neighboring tile's remaining
integer volume and stage table. Fully submerged tiles retain a sampled source
stage with a local cross-section approximation for changes; this is not a
whole-waterbody stage model. Coarse occupancy preserves its original binary
layout until volume changes, then removes highest cells or fills lowest free
cells deterministically. These coarse changes also update canonical queries and
the visible water snapshot, so exported detailed water is not counted twice.

Native pickup/placement works in the detailed tile, including reopening or
closing dry hydraulic faces. Wet placement rejects. Editing the six coarse
neighbor tiles reports that detailed activation is required; this fixed
comparison does not activate neighboring detailed tiles. Terrain outside this finite footprint follows
the normal inventory edit path. Source revision gates and the 500 ms input queue
also apply to this mode.

`FVPWATR1` saves contain terrain/inventory, the detailed tile's complete numerical
state, coarse geometry and volumes, ownership tokens, transfer serials, source
identity and elapsed/deferred time. Clean exit drains one pending worker result.
A failed paired step returns its owned state, pauses simulation with a diagnostic
and retains it for saving. The registry and coupled checkpoints must agree on
restore; saved solids must match the restored canonical terrain. CA and the
supplied waterfall retain their independent save formats.

Focused checks are `cargo test -p frontier-world procedural_water --lib` and
`cargo test -p frontier-renderer procedural_water_live --lib`. They cover exact
geometry/counting, generated lake stage seams, persistent finite ownership,
absence of implicit supply, failed-worker state retention, and clean worker/save
inventory continuation. This fixed comparison keeps its original tile active.
The moving mode below adds local player-following residency; distant edited
shoreline publication and complete inactive-region hydrology remain separate work.

## Moving connected native water

```sh
./run.sh --preset seeded-world --seed 19 --water-model moving \
  --water-river-rate 128 --naadf-radius 8 --width 1920 --height 1080 \
  --water-save output/moving-water.save
```

This comparison launch uses 1920×1080 and an 8 m NAADF radius (a 16 m cube).
The [moving-water validation](research/nov16/water-integration/README.md) includes
walking/turning/reload videos and measured frame times. Physics remains the 2 m
detailed window and 4 m local footprint below. Normal interactive NAADF defaults
remain at 64 m; omit the explicit radius to use that renderer setting. The 8 m
measurements do not establish performance at larger radii.

The moving mode keeps one connected 32³ detailed region (2 m per side) near the
player's feet. A complete ring of native 16³ tiles surrounds it, giving a 4 m
local footprint. Tile-aligned migration preserves overlapping cells, dormant
occupancy, momentum, pressure and globally owned face credits. Persistent tile
identity prevents camera movement or reload from regenerating drained water.
The camera follows the player's saved pose on reload. The fixed `procedural`
mode remains available as the smaller integration comparison.

A single regional ledger accounts for the active owner and every materialized
spatial tile. Ghost transfers reserve the actual adjacent wet/dry cell, carry
its momentum and commit its native state and regional count together. Dormant
fringe exchanges can trigger a bounded chain of adjacent water moves to a real
surface donor or a lower dry destination. These moves conserve complete cells,
permute momentum and cannot increase gravitational potential. This is an
explicit quasistatic physics LOD outside the detailed solver. Accepted native
boundary transfers can leave an unresolved vacancy when a donor path is unknown
or the work budget is exhausted. Pending routing preserves that state without
inventing donor cells or refill water. One shared query budget and a persistent
deduplicated pending queue bound this work. Native cells untouched by exchange
or routing retain their saved numerical state.

`--water-river-rate` configures whole native cells per simulated second at
eligible upstream/downstream exterior continuations of canonical river
features. The initial default is 128 cells/s (0.03125 m³/s); this is an explicit
simulation parameter, not discharge inferred from channel width or a calibrated
watershed model. A saved rate is retained unless the flag explicitly overrides
it. A value of zero selects finite water without environmental supply/drainage.
Lakes remain finite. Feature clocks and attempted/accepted/suppressed supply
persist through migration and restart; registration itself supplies no water.

The configured rates are attempted boundary budgets. Full supply cells, blocked
paths, unavailable water and the maintained-channel guard can reduce accepted
flow to zero. With positive input and output rates, each canonical channel
allows downstream removal to lead accepted upstream addition by at most 16
whole cells (3.90625 L). This bounds downstream depletion when supply is blocked;
it does not promise a constant accepted rate.

The simulation worker receives bounded elapsed-time requests of at most 0.25 s
and retains unprocessed time. It returns the same owned state on failure.
Reports expose simulated time, deferred time, migrations, pending routing work,
conservation and actual native payload bytes. The persistent history cap is
4096 tiles; reaching a limit pauses activation with the existing state intact.
History grows as the player travels and is saved separately from disposable
render caches. Dense GPU water covers the current 4 m footprint; the static
native source preserves authoritative dormant fluid outside that footprint.
On reload, exact dormant occupancy is restored in bounded block batches before
constructing renderer sources. Complete distant edited shoreline rendering and
elapsed hydrology for inactive regions remain later validation/integration work.
