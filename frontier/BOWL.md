# Bowl interaction checkpoint

Build and launch from `frontier/`:

```sh
cargo build --release --locked -p frontier-tools --bin frontier
./run.sh --preset desert --seed 19
```

The desert starts with empty hands. Press **2** or **B** to equip the bowl and **C**
to switch between first and third person during play.

- Walk to the pond with **WASD**. **Ctrl** crouches, slows each step, and gives your
  boots more clearance over obstacles. A blocked swing can disturb your balance.
- At the shoreline, hold **Ctrl**, look down, and hold the **left mouse button**
  until water reaches the opening. The bowl holds twenty native water units (4.88 L).
  Release the mouse button and raise your view to carry it away.
- Water responds to changes in speed, turns, and bowl tilt. Hold **right mouse**
  or **R** to pour, or **Q** to tip it farther.
- **E** stows your item and **1–4** swap pockets. **Escape** opens settings and
  releases the mouse. Click the game to resume.

Clean exit saves the bowl, removed pond water, deposited water, moisture and damp
footprints under `output/desert-game/`. `--bowl-save PATH` selects a separate file
for an experiment. Existing files restore automatically and validate seed/source,
physical scale and amplitude. Camera position currently resets to the shore spawn.
Failed/corrupt saves report an error instead of silently resetting water.

## Model and boundaries

`world/src/bowl.rs` owns the finite mass ledger and native-grid droplets. Scoops take
whole units from connected water and spread the resulting small surface deficit over neighboring columns. A bounded local settling queue wakes
on scooping, pouring, and terrain edits. Dig through a shallow bank and water
can fall into the opening. Sand placed below the surface displaces the same
number of water voxels upward, and placement is rejected if they have nowhere
to go. Use empty hands to target solid terrain through water and the bowl to
target the liquid.

Poured droplets travel on the native grid with swept collision. Sand absorbs
whole units into its moisture store. Fluid entering another opening can flow
there while the intact pond sleeps. This is local gravity and leveling on the
voxel grid. It does not simulate a whole watershed or wave dynamics.

The hybrid renderer uses a bounded ray into the bed for refraction and a
short reflected ray for nearby banks. Sky reflection takes over at distance,
and distant coarse water uses an approximate tint. Ripple normals move with
time while the water's geometry stays on the voxel grid. When the camera enters
water, visibility falls with distance and the surface refracts or reflects
the view back into the pond. Toggle **F** for free flight to inspect the
waterline and underwater view. [Dielectric reflection and refraction](https://pbr-book.org/4ed/Reflection_Models/Specular_Reflection_and_Transmission) · [Exponential transmittance](https://pbr-book.org/4ed/Volume_Scattering/Transmittance)

The ledger is `source units removed = held + airborne + soil moisture + deposited + plant transfer`.
[Planting and seedling growth](PLANTING.md) accounts for plant storage and
transpiration. Legacy bowl-only saves migrate with zero plant transfer. Returning and
recollecting water preserves it. The serialized state includes droplets in flight,
source-removal tombstones, moisture, damp footprints, emission phase, settling
queue and counters.
A size/identity/finite-value/duplicate/ledger check guards restore; writes use a
temporary sibling followed by rename. This is a single-player checkpoint file,
not a synchronized multi-writer save service.

Moisture is independent of fertility: watering creates no nutrients or organic
matter. The first visual implementation marks damp surface cells with the existing
Soil color/material palette. Their original desert sand source is unchanged and
the separate moisture record is authoritative; a dedicated wetness appearance
channel remains a later rendering improvement. There is no standalone soil evaporation; the plant update separately counts transpiration.

The avatar sweeps oriented boots against canonical terrain cells. The capsule
uses the available step support from the gait, and blocked foot motion can
disturb balance depending on the incoming motion and the planted foot. Player
motion, grip, and water share a 120 Hz gameplay step. First and third person
read that state through independent camera transforms.

The avatar, bowl, held water, and airborne water are rendered on the terrain's
6.25 cm integer lattice. Continuous pose and bowl containment give the model
subcell motion between the cells that it occupies. The grip follows the
animated torso with translational and rotational compliance. A small
pressure-corrected parcel solver carries exactly one native unit per parcel.
The bowl's wall and floor constrain parcels, and crossing the opening transfers
a parcel into the existing world or droplet ledger. Current saves restore the
held amount into a settled bowl.

`--pond-test` places a short stone ledge on a level patch of the seeded desert bank for
repeatable playtests. The contact solver queries that ledge like ordinary world
geometry. The default desert launch uses the seeded terrain.

## Verification

Core tests cover capacity, unreachable/blocked scooping, actual source removal,
pouring/absorption, restart in flight, return-and-rescoop conservation, malformed
ledger rejection and persistent moisture. `cargo test -p frontier-world --lib`
passed 200 tests (6 ignored) before the final emitter-position adjustment; the
focused bowl tests were rerun afterward. `cargo check --workspace --tests --locked`
is the integration compile gate. The release binary is rebuilt for the playtest.

Record separate runs driven through the game window on a Vulkan-capable X11 desktop:

```sh
python3 tools/record_pond_trip.py first-person --width 1920 --height 1080 --output output/pond-attempt-2-demo
python3 tools/record_pond_trip.py third-person --width 1920 --height 1080 --output output/pond-attempt-2-demo
```

The tool uses ordinary keyboard and mouse input, checks slow passage over the
rocks, captures a full bowl at the pond, and records the actual obstructed footstep
and resulting spill. It verifies the water ledger. It keeps both runs in separate
save files alongside the recordings and reports.

The game restart check caught and fixed an initial integration error: restored
wet terrain edits cannot be passed into the immutable far-surface builder.
The bowl now uses the same immutable far-source/native-edit ownership split as
the live-water modes. Reload is checked in a second actual game process.

Earlier bowl checkpoint video: `output/bowl-lip-playtest/bowl.mp4` (actual 1080p input-driven run).
The player scooped 15 native water units, poured 8 and spilled 7; all 15 ended
in saved soil moisture. Both damp patches (58 cells total) survived actual game
restart. The earlier restart failure was corrected afterward without changing
interaction/render code in the recorded video; binary hashes and commands are
recorded in `bowl-checkpoint.json`. The recording's median/p95/p99 frame intervals
are saved there with recording overhead disclosed. This is not a general engine
performance or whole-world hydrodynamics acceptance claim.

## Attempt 1 pond and stumble playtest

The archived 1080p runs use desert seed 19, source
version 2, 6.25 cm voxels, a 16 m NAADF radius and Voxy-seed distance geometry
with a 300 m clip. The route crosses the rocks slowly toward the pond and hits
them at normal walking speed on the way back. Each view fills to 4.88 L, carries
the entire load through the turn, then spills it on contact. Both reports
verify water conservation with no water left in the bowl or in flight at exit.

Recorded on an AMD Ryzen 5 7640U with Radeon 760M Vulkan graphics. Frame
intervals include live terrain work and X11 presentation and capture overhead.

| View | Video | Spill | Frame p50 / p95 / p99 | End resident / pending terrain clusters |
| --- | --- | --- | --- | --- |
| First person | `pond-trip-attempt-1-first-person.mp4` | 16.4 s | 35.0 / 46.5 / 49.7 ms | 2152 / 0 |
| Third person | `pond-trip-attempt-1-third-person.mp4` | 17.0 s | 35.2 / 49.9 / 55.1 ms | 2422 / 22 |

End-of-run GPU terrain allocations were 164 MiB and 172 MiB, respectively.
Both reports recorded zero pending CPU terrain selection bytes at exit.
The videos are available in the [generated test archive](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/releases/tag/frontier-generated-archive-2026-11-19).


## Attempt 2 playtest

The two 1080p takes under `output/pond-attempt-2/` use desert seed 19, source
version 2, a 300 m clip, Voxy-seed distance geometry, a 16 m NAADF radius,
and the level shoreline fixture. Each take transferred twenty native units
into the bowl, spilled one during the turn, and spilled the remaining
nineteen after the boot caught the fixture stone. Both saves account for all
twenty units. The movies use standard keyboard and mouse inputs.

| View | Contact | Frame p50 / p95 / p99 | Resident / pending terrain clusters |
| --- | --- | --- | --- |
| First person | 21.4 s | 35.6 / 47.8 / 51.6 ms | 1735 / 0 |
| Third person | 21.3 s | 44.6 / 53.9 / 68.3 ms | 2581 / 0 |

AMD Ryzen 5 7640U, Radeon 760M Vulkan. GPU terrain allocations ended at
156 MiB and 172 MiB. Frame durations include recording and presentation overhead.
