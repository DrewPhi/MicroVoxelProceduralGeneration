# Whole-voxel water integration checkpoint

This is a separate binary-occupancy method from the earlier fractional physical
reference. A water unit always occupies one 6.25 cm cell. Floating-point pressure,
momentum and movement credits guide transport; they do not represent fractional
mass. This is a game-oriented discrete liquid, not a validated Navier–Stokes solver.

## Implemented components

- `world/src/discrete_water`: bounded whole-unit transport, injective simultaneous
  movement, pressure guidance, adaptive substeps, exact source/outlet ledger,
  save/restart, explicit open pressure faces and finite transfer quotas.
- `discrete_water/regional.rs`: finite coarse reservoirs and ownership tokens,
  atomic neighbor-chain transfers and explicit environmental input/output.
- `discrete_water/coupled.rs`: atomic local/regional transactions. A staged copy
  prevents a failed ownership update from publishing half a water transfer.
  This correctness baseline is intended for small tiles, not whole lakes.
- `world/src/procedural_water.rs`: existing river/lake descriptors, fixed spatial
  identities, native geometry counts and local stage-volume tables. It does not
  infer a worldwide runoff network or watershed discharge.
- Separate immutable NAADF water snapshots with exact depth and shared shadows.
  Unchanged snapshots avoid revalidation; following-volume shifts retarget the
  retained snapshot atomically instead of moving water with the camera.
- `world/src/moving_water.rs`: a connected 32³ native solver with a 64³ local
  footprint, persistent native tile history, exact migration, bounded dormant
  pressure-chain routing and counted river boundary supply.
- Worker ownership, bounded in-flight work, dry terrain edits, inventory/save
  integration and explicit simulation lag diagnostics.

Source supply is counted external input, not simulated rainfall. Blocked source
attempts are discarded rather than saved for a burst. Whole-unit conservation
includes all accepted imports and exports. Distant static source water is not
proof that an edited shoreline has a corresponding dynamic distant proxy.

## Correctness evidence

The checks exercise still water, a supplied waterfall, dam release, mass ledgers,
pressure energy safeguard, corrupt saves, bit-identical restarts, finite boundary
quotas, still-lake boundary agreement, coupled imports/exports and atomic rollback.
Runtime tests cover quick/held pickup, placement rejection, and saving while the
worker owns a pending simulation. GPU tests run on the Radeon 760M include wet
additions, dry overrides, primary/shadow depth and immutable origin retargeting.

The fixed Legacy primary comparison with dynamic water disabled was exactly
pixel-identical in linear RGB to `output/nov16-primary-after`. Final matching
capture is `output/moving-water-primary-hybrid`; maximum absolute linear RGB
difference is 0.0. `cargo test --workspace --lib --bins --locked` passed 812 tests,
with 45 opt-in/ignored tests excluded. The explicit ignored
`dynamic_water_primary_and_shadow_gpu_parity` test then passed on the Radeon GPU.
`tools/discrete_water_edit_check.py` passed all five groups (discrete, moving and
fixed procedural actors, inventory transactions and placement geometry). Python
tools compile, Rust formatting and `git diff --check` pass. The release binary
was rebuilt after help-text cleanup. See `validation.json` and `measurements.json`.

An extra capture mistakenly selected `--scene adaptive --pixel-error 1` rather
than the approved `hybrid` primary. Its strict quality-settle gate timed out at
120 seconds despite complete coverage and zero pending work. That separate
capture gate remains unresolved; it was not weakened to produce a pass. The
correct fixed hybrid primary passed and is the comparison reported above.

## Measured performance, with limitations

Hardware: Ryzen 5 7640U, integrated Radeon 760M, RADV Vulkan. Simulation-only paired
fixture: 48×40×48 cells, dam plus 32 sources, 160 calls of 0.025 simulated seconds.
Caching wet-neighbor stencils and visiting only wet-adjacent faces reduced median
from approximately 27.5 ms to 19.7 ms per call. The latter p95/p99 were
33.52/34.47 ms. The final snapshot hash, mass, tick and pressure sweep count were
identical. These are CPU solver timings, not whole-game frame times.

Earlier live recording `output/discrete-water-fixed1080` used 1920×1080, no
vegetation, an 8 m NAADF radius and the deeper supplied-pool demo. It ran before
these performance changes. Recorded frame p50/p95/p99 were 17.25/47.35/54.10 ms;
13.6 simulated seconds elapsed during 20.14 wall seconds. The water ledger was
exact (8816 added, 1755 removed, 7061 remaining). This exposed a real slowdown;
it is not a 60 FPS acceptance result. Recording adds overhead. Later reports
must retain this distinction rather than replacing simulation time with wall time.

## Final moving-region playtest

Launch from `frontier/`:

```sh
./run.sh --preset seeded-world --seed 19 --water-model moving --water-river-rate 128 --naadf-radius 8 --width 1920 --height 1080 --water-save output/my-moving-water.save
```

The 8 m render radius is the measured comparison setting, not the physics radius.
The solver is a connected 2 m cube, surrounded by a 4 m native footprint. The
ordinary renderer's larger radius can be selected independently. Use rate `0`
for strictly finite local water. Existing CA and fixed demo modes remain available.

Actual grounded walking, returning, turning and disk reload were recorded with:

```sh
python3 tools/record_water_walk.py --output output/moving-water-walk1080-v2 --width 1920 --height 1080 -- --vegetation off
```

The harness drives its own PID-checked game window. Both phases passed actual
motion, migration, continued simulation, conservation, checkpoint and no-paused-
error assertions. Contact frames were inspected: native water remains in the
riverbank as the player moves/turns; it does not move with the render volume.
Water still uses the existing opaque voxel appearance, not a new liquid shader.

| Run | Frame p50 / p95 / p99 (ms) | Simulated / accounted wall seconds | Peak process RSS |
| --- | --- | --- | --- |
| Walking, video recording | 21.53 / 47.15 / 58.43 | 21.21 / 22.05 | 400 MiB |
| Reload/return, video recording | 16.86 / 39.59 / 52.96 | 44.69 / 44.69 cumulative | 435 MiB |
| Stationary, no recorder | 25.15 / 26.80 / 28.75 | 24.83 / 24.83 | 294 MiB |

All are 1920×1080, seed 19, vegetation off, 8 m NAADF radius on the laptop above.
The stationary view differs from the moving camera and cannot isolate recording
cost. Recorded walking has significant tail spikes; these results are not a
locked-60-FPS claim. Stationary median is approximately 40 FPS. GPU reported mean
for that run was 22.84 ms. No benchmark ran alongside a Rust compilation.

After reload: 14 migrations, 173 retained tiles, 48.7 MiB native payload, zero
remaining simulation debt and no paused worker. Ledger identity was exact:
`140212 initial + 619 supplied = 140196 stored/detailed + 635 drained`.
The initial count grows only when previously unseen canonical tiles register;
revisits restore their saved state. The stationary saturated inlet accepted no
supply and removed only its permitted 16-cell bootstrap. Thus a configured rate
is a maximum attempted budget, not a guaranteed river discharge.

Local evidence (ignored generated files):
- `output/moving-water-walk1080-v2/result.json`
- `output/moving-water-walk1080-v2/{walk,reload_return}/walking.mp4`
- per-phase `game.json`, `checks.json`, `manifest.json`, `world.save`, PNGs
- `output/moving-water-bench.json`, static no-recorder benchmark
- `output/discrete-water-optimized1080/water.mp4`, supplied waterfall demo

The first recorder attempt rejected the new `FVMWATR2` header despite a valid
save. The corrected harness accepts both supported envelope versions; the full
second walking/reload run above passed. No runtime state was discarded to make
that check pass.

## Boundaries of this checkpoint

This completes the bounded game-integration foundation, not worldwide continuum
hydrodynamics. Native transport, momentum, pressure, edits, rendering ownership,
conservative boundaries, moving residency, persistence and renewable river
budgets are integrated. Adjacent inactive water uses explicit quasistatic chains;
unknown routes stay pending rather than inventing water. Closed lakes are finite.

Full upstream cells suppress supply. For maintained channels, downstream removal
may lead accepted input by at most 16 cells (3.90625 L). This prevents a blocked
inlet from emptying a river; it does not guarantee an uninterrupted visible
waterfall in every procedural channel. The supplied waterfall fixture separately
exercises continuous source/outlet flow.

The 4096-tile history cap pauses new activation with state retained. Paging that
history to disk, arbitrary-world hydrology, distant edited shoreline publication,
wet-cell solid placement/displacement, swimming, erosion, physical waves and
water shading are not certified by this checkpoint. Physics is intentionally
local; a whole lake is never expanded into a dense native solver. The gameplay
can move forward on this base without treating these later features as already
implemented.
