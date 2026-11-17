# Planting and first seedling growth

From `frontier/`, rebuild and play:

```sh
cargo build --release --locked -p frontier-tools --bin frontier
./run.sh --preset desert --seed 19
```

Your desert save now starts with one seed. Existing bowl-only saves migrate
without resetting their water or damp ground. `--bowl-save PATH` still selects a
separate experiment/save slot; the garden and water now share that atomic file.

## Try it

1. Scoop water at the pond with the bowl, then move onto dry sand.
2. Press **B** to put the bowl away and use your hands.
3. Aim down at nearby sand and **left click** to dig a small two-cell-deep hole.
   The removed sand is retained in your hands (0.488 L at the native scale).
4. Keep aiming at the hole and press **P** to place your single seed.
5. **Right click** to cover it using the sand you dug up.
6. Press **B**, then hold **right mouse** to water the patch around the seed.
7. Watch it germinate and grow. Water near its base again when it becomes thirsty.

The hole selection has a small aim tolerance. Actions require a canonical solid
hit within 2.2 m; digging underwater, planting into blocked holes and duplicating
the seed are rejected. The HUD explains whether the seed needs covering, moisture,
energy or further watering. The seed, hole, tissue, resource state, bowl and wet
patches survive a clean exit/restart. Camera position still resets to the shore.

## What grows

`world/src/garden.rs` generates a small branching program from repeating rules:
extend a root, extend the shoot, attach early leaves, then repeat a main tip,
rotating lateral tip and leaf fan. These are integer-cell steps with explicit
parent connections. A blocked action is skipped and its unsupported descendants
cannot grow; another branch can continue. No finished mesh or plant model is
placed. All grown tissue becomes canonical editable world voxels.

There is one starter pattern in this checkpoint. A visual growth editor,
reproduction/mutations and seed collection are later steps. This is a modest
seedling, not the final tree/organism system.

Tissue roles are seed, root, stem and photosynthetic leaf. Wood/Leaf are their
current renderer materials; simulation roles remain separate. The initial seed
contains 10 arbitrary energy units. Buried, moist seeds take four simulation
seconds to germinate. Growth proposes one new cell per second and waits if its
energy or hydration budget cannot pay for it.

Each new tissue cell costs energy; existing roots, stems and leaves cost energy
per second. Exposed leaves earn energy only when hydrated. Local native rays
check self-shading/nearby obstruction; a few farther height samples approximate
terrain shadow. This is an intentionally bounded lighting approximation, not a
certified visibility calculation over the entire world.

Nearby stored bowl moisture supplies roots within a bounded 37.5 cm neighborhood.
This approximates local wet-soil access; it does not solve a soil diffusion field.
Whole water units are transferred to a small plant reserve. Spending one whole
unit funds four tissue-growth actions through a hydration allowance; that
allowance is numerical state, not fractional fluid mass. Periodic transpiration
also spends whole units. Water uptake, storage and transpiration are counted:

```
source water removed = held + airborne + soil moisture + deposited + plant transfer
plant transfer = stored plant water + transpired water
```

Transpiration is an explicit counted sink; no atmospheric water simulation is
introduced. Current budgets are tuned for 6.25 cm cells and accelerated game time.
No offline catch-up occurs. Seedlings pause under shortages; tissue death,
structural breakage, nutrient use, decomposition and biological reproduction are
not implemented by this checkpoint. The existing damp-color footprint is still a
visual placeholder; it does not instantly recolor as roots consume moisture.

## Persistence and verification

`BOWL2` adds the transferred-water ledger while reading legacy `BOWL1` saves.
A `GARDEN1` section stores the seed stock, loose sand, holes, actual grown tissue,
program cursors, resource reserves, clocks and counters. Restore validates counts,
program membership, parent support, seed conservation and the paired water ledger.
Renderer initialization keeps the immutable distant source separate from these
native edits, as it does for the bowl's damp patches.

Core checks exercise dry/unburied seeds, water-limited pauses, sunlight/energy
limits, connected growth, stone obstacles, exact restart continuation and old-save
migration. The renderer integration check uses real scooped/poured water, saves
a growing plant, reloads the combined state and verifies native tissue materials.

To record the actual interaction and restart check:

```sh
python3 tools/record_bowl.py --plant --output output/my-planting-playtest
```

The tool drives its own PID-checked window: walk, scoop, dig, plant, cover, water,
watch growth and step back. It does not inject plants or soil moisture. Reports,
video, frames, controls, binary hash and explicit restart checks are retained.
A still/video does not establish broad biome simulation or distant edited-plant
LOD quality. Native display currently supplies the visible planted geometry.

The final actual-input run is `output/planting-final-playtest/bowl.mp4`; its
angled view is `output/planting-final-playtest/seedling.png`. One bowl (8 whole
water units) produced a 29-cell seedling with 13 leaf cells before water shortage
paused growth. The plant and paired water ledger survived restart. Replay
commands, counters and binary hashes are retained in `planting-checkpoint.json`.

The workspace suite passed 825 tests (45 ignored). After adding a corrupt-save
water-counter guard, the final world suite passed 206 tests (6 ignored). The
combined renderer garden-save test passed, workspace test targets compile, and
the approved hybrid primary image remains pixel-identical. Final source differs
from the recorded binary only by the extra impossible-save-counter guard.
