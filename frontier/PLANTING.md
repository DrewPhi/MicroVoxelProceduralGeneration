# Planting and recursive growth

From `frontier/`, rebuild and play:

```sh
cargo build --release --locked -p frontier-tools --bin frontier
./run.sh --preset desert --seed 19
```

New desert saves start with **empty hands**, a **black seed in pocket 1**, and
a **bowl in pocket 2**. There are four pockets, each holding one item or one
handful. Keys **1–4** swap that pocket with your hand; **E** stows the held item
in the first empty pocket. A full inventory rejects stowing without losing items.
The bowl keeps its water while stored. **B** remains a bowl shortcut.

Hold the black seed and press **G** to open the Seed workshop. Drag tiles
from the palette to add actions, drag existing tiles to reorder them, or select
a tile and use **Earlier** and **Later**. **Next round** advances the preview
through the active tips. **Rule text** lets you edit the same program directly.
**Apply to seed** commits the draft and returns to the game. **Esc** or **Cancel**
closes the workshop without applying the draft.

## Try it

1. Select pocket **2**, scoop pond water with **left click**, walk to dry sand.
2. Press **E** to stow the bowl and empty your hands.
3. Aim at nearby sand: tap **left click** for one voxel, or hold and release
   for up to four (0.977 L). The selection outline and charge bar show the amount.
4. Carry the handful, **E** to pocket it, or **right click or R** elsewhere to drop it.
5. Select the black seed's pocket and **right click or R** into the hole.
6. Gather another handful, aim into the hole and **right click or R** to bury the seed.
7. Select the bowl's pocket and hold **right click or R** to water the planting spot.
8. Stow the bowl and watch the shoot emerge. Water again when it becomes thirsty.

Seed placement and sand placement use actual canonical cells; there is no
special plant/cover shortcut. Placement settles vertically onto support and
rejects water or overlap with your body. A deeper hole requires a longer emerging
shoot. Gather within 2.2 m; aiming steeply downhill can put the ground beyond reach.
Currently, soil containing stored bowl moisture cannot be pocketed, to preserve
its water until wet-soil transport is implemented. Sand settles as a bounded
column operation, not a general granular-physics simulation.

The HUD lists all pocket contents, the hand and action feedback. Terrain edits,
pockets, seed, tissue, bowl and wet patches survive clean exit/restart. Camera
position still resets to the shore. `--bowl-save PATH` selects another save slot.

## What grows

`world/src/growth.rs` defines a small recursive bud grammar. The runtime in
`world/src/garden.rs` executes it lazily, only when more growth is needed:

- **Shoot → stem upward + shoot**, with leaves every two segments and a lateral
  branch every eight. Successive branch sites rotate around the trunk.
- **Branch → stem outward/upward + branch**, with leaves every two segments,
  rising every third segment and forking into a perpendicular branch every twelve.
  The child uses exactly the same branch rule, so it can fork again.
- **Leaf → terminal photosynthetic voxel.** Three initial root cells and an
  emerging shoot bootstrap the seed. Taller main shoots add a cross of stem cells
  around their centre, producing a thicker trunk.

Unprogrammed seeds and older saves use the four saved growth intervals. A seed
edited in the workshop uses its own tip rules after germination. Every growth
instruction has a face-adjacent parent. A tip whose stem fails to grow stops,
while other branches continue. Adaptive stems can queue adjacent reinforcement
when supported tissue load increases. Reinforcement pays the same water and
energy costs as new wood.

There is no fixed number of generations. The engine currently guards each plant
at 65,536 tissue cells, 512 queued tips and 262,144 generated instructions; the
HUD reports the cell/instruction safety limit. New tips beyond the queue budget
are suppressed while admitted tips continue. This keeps recursive growth bounded
without a finished seedling shape. The load model currently covers reinforcement.
Branch failure, death, reproduction, mutation and decomposition are future work.

Tissue roles are seed, root, stem and photosynthetic leaf. Seed/Wood/Leaf are their
current renderer materials; simulation roles remain separate. The initial seed
contains 10 arbitrary energy units. Buried, moist seeds take four simulation
seconds to germinate. Small seedlings attempt one new cell per second; larger lit canopies can attempt
up to eight. Each cell pays its full energy and water cost. Growth waits when
those resources are insufficient.

Each new tissue cell costs energy; existing roots, stems and leaves cost energy
per second. Exposed leaves earn energy only when hydrated. Energy storage is 30 units plus
0.04 per living tissue voxel; adding capacity does not grant energy. Local native rays
check self-shading/nearby obstruction; a few farther height samples approximate
terrain shadow. For large plants, at most 64 leaves are tested per second and
the lit fraction estimates total photosynthetic area; the deterministic sample
rotates with plant age. Maintenance still includes every tissue cell. This is an intentionally bounded lighting approximation, not a
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

`BOWL2` retains the transferred-water ledger while reading legacy `BOWL1` saves.
`GARDEN3` stores the recursive rules, instruction cursor/history, queued buds,
emergence and tissue, plus an applied seed program when present. `GARDEN1`/`GARDEN2` remain readable: their existing seedling
and unfinished steps are preserved, then a recursive shoot continues at their tip.
`POCKETS1` stores pockets, hand, terrain edits and the loose-material ledger:
`picked + migrated = placed + carried`. Exactly one bowl and the remaining
seed stock are validated. Legacy loose sand/soil migrate into available slots;
excess becomes actual nearby piles. Native seed material is ID 10; the first ten
material IDs and legacy inventory format are unchanged.

Restore validates seed/water/material conservation, growth program membership,
parent support and resource bounds. Pocket edits restore before actual tissue;
restoration does not clear physically placed burial sand. Renderer initialization
keeps the immutable distant source separate from these native edits.

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

Current pocket/handful recording: `output/pockets-release-playtest/bowl.mp4`.
Its final plant view is `output/pockets-release-playtest/seedling.png`. The run
picked up and placed nine cells, grew 25 tissue cells including 10 leaves, and
passed disk reload with both ledgers intact. Verification and replay metadata
are documented in `pockets-checkpoint.json`. Earlier planting recordings describe the previous
shortcut interaction and are retained only as historical checkpoints.

## Accelerated recursive-growth fixture

```sh
cargo run --release -p frontier-world --example grow_recursive -- output/recursive-plant-scaled-energy/world.bowl 4000
```

This executes 4,000 simulation seconds using the same runtime growth rules. It
supplies finite pond water directly into local soil, bypassing bowl transport for
validation. It is not an actual-input playthrough or evidence that one bowl
produces a mature plant. The measured fixture grew 14,965 cells including 5,908
leaves, reached 7 m, and retained 512 queued tips. It used 3,952 whole water units.
The output is a loadable desert save plus a plain cell export for inspection.
See `recursive-growth-checkpoint.json` for verification and capture settings.
