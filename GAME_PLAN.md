# Desert restoration game

Working concept for the `cellular-automata-game` branch, based on our discussion.
The game has no final title yet. This document records the intended experience;
it is not a claim that the engine already implements these systems.

## The premise

You are an old man who arrives in a massive procedurally generated desert with
one seed. There is initially no vegetation except at sparse oases, where you can
collect water. You can redesign the seed's growth pattern, plant it, care for it,
and help its descendants gradually turn the desert into a living landscape.

Your main tools are your hands, a bowl, and a visual seed editor. The central
pleasure is creating unusual plants, watching them develop, understanding their
successes and failures, and seeing life become able to spread without your help.

## Design principles

- **Emergence from a small vocabulary.** A few kinds of plant tissue and shared
  physical and biological mechanics should produce many different plants.
- **Design growth, not a finished model.** A seed contains a developmental
  pattern. Its actual shape depends on resources, surroundings, and structural
  survival. Plants are generated rather than selected from prebuilt models.
- **Visual and approachable.** Designing a plant should be enjoyable through a
  GUI, without writing code or managing a large list of conditional rules.
- **Everything inhabits the voxel world.** Plants, soil, litter, seeds, and water
  have spatial presence. The starting native voxel edge is 6.25 cm.
- **Failure can leave progress behind.** A dead plant can improve the ground for
  future generations. Restoration should accumulate through local interactions.
- **Understandable consequences.** Players should be able to discover why a
  plant stopped growing, lost a branch, or thrived.

## The core loop

1. Find water and choose somewhere to plant.
2. Inspect or edit a seed's growth pattern in the visual editor.
3. Dig a small hole, put the seed in it, and bury it.
4. Scoop water into the bowl at an oasis, carry it, and pour it onto the soil.
   The bowl can spill water.
5. Watch the seed germinate and follow its growth pattern as resources permit.
6. Observe its health and structure; tend it or revise a descendant's design.
7. Collect produced seeds, plant them elsewhere, or let them disperse naturally.
8. Over generations, establish plants that reproduce and improve their habitat.

The long-term goal is to restore a lively organic biome. Exact objectives,
progression, and any ending are still open. We have not agreed on survival meters,
combat, crafting trees, or a conventional win/lose structure.

## Seed design: an L-system-like growth pattern

The preferred direction is a visual branching grammar rather than a large
collection of programmable “if this, then that” behaviors.

A pattern might express: grow a stem, split into two growing tips, extend the
branches, add photosynthetic tissue, and repeat. The editor should let the player
manipulate the pattern and its parameters visually: branching angles, segment
lengths, repetition, and where different tissues develop.

The pattern proposes growth steps. Shared physiology decides whether a step can
be afforded, while environmental responses influence its direction and outcome.
For example, tips may bend toward light and roots toward moisture. Players should
not need to program basic resource accounting or structural mechanics themselves.

Any seed can be picked up and reprogrammed. The precise grammar, editor controls,
and allowed parameters need a small playable experiment before we commit to them.
An accelerated growth preview with clear resource/strain feedback is a promising
editor aid, not yet a settled interface design.

## Plant tissue and resource costs

Start with very few functional tissue categories. Candidate roles are roots,
photosynthetic tissue, structural/supporting tissue, and reproductive tissue.
The final material list is not decided.

- Every growth step costs energy. Creating tissue also requires water and any
  material resources we choose to represent.
- Living tissue has an ongoing maintenance cost. Excessive branching and
  unproductive shaded tissue can become a burden.
- Photosynthetic tissue gains energy through exposure to sunlight, subject to
  available water. Shading by the plant itself and its neighbors should matter.
- Roots acquire water from their surroundings; growing and maintaining them
  costs energy too.
- Growth waits when resources are insufficient. Prolonged shortages may cause
  tissue death; the timing and allocation policy remain to be designed.
- Reinforcement must consume resources. High stress cannot create free wood.

We should distinguish living tissue from dead structural wood: dead wood can
continue supporting a plant without the same biological maintenance cost.
Costs should reflect physical amount rather than arbitrary voxel count, so a
future change in voxel resolution does not accidentally change the biology.

Whether energy and water move through an explicit tissue network, a plant-wide
reserve, or a combination is an important open simulation choice.

## Structure, growth, and breakage

Plant voxels carry weight and require support. A plant that grows too tall or
extends an unsupported branch can bend, break a branch, or collapse. Structural
failure should happen where support fails rather than always killing the whole
plant. Detached parts become physical dead plant material where appropriate.

Adaptive supporting tissue may invest in reinforcement around stressed regions,
provided the plant has resources. Its exact mechanics and how much the seed
design controls it remain open. The desired result is a meaningful tradeoff
between collecting resources, reproducing, and building a survivable structure.

## Reproduction and inheritance

Mature plants can make and drop seeds carrying their developmental program.
Small genetic mutations may alter that program or its parameters. Descendants
therefore vary, and their ability to survive and reproduce determines which
lineages spread. Mutation frequency, reproductive cost, and seed dispersal rules
remain tuning/design decisions.

Seeds can establish without the player: dust storms can bury them, and rain can
provide water. Players can also bury and water them deliberately. Autonomous
reproduction is a central goal, rather than every plant requiring manual placement.

## Soil and the restoration cycle

Dead leaves, roots, and branches become litter. With moisture, decomposition
returns organic matter and nutrients to the ground. Sand gradually becomes more
fertile and better able to retain water, improving future seedlings' prospects.
Dry litter decomposes slowly; rain or watering accelerates the process.

A small initial soil model could track moisture, organic matter, and available
nutrients. Plants consume resources and dead material recycles them. Fertility
should not appear without a source of material.

Ground color can gradually shift from pale sand toward richer brown as organic
matter accumulates. Green should primarily indicate living growth. The exact
soil model and color mapping are proposals to validate during implementation.

This creates the restoration loop: plant growth produces litter, litter improves
soil, improved soil supports more life, and new generations expand the habitat.

## The desert and weather

The world is large, streamed, seeded, and procedurally generated, with sparse
oases and enough geographical variation to make exploration worthwhile. Existing
vegetation is disabled for the desert except where an oasis calls for it.

Rain and dust storms should eventually participate in seed establishment and
soil moisture. The game does not require a full planetary weather simulation to
test this loop. Weather frequency, oasis distribution, and the ecological limits
on spreading into the desert are still open.

## First playable checkpoint

Build one enjoyable **seed-to-seed cycle** before expanding the simulation:

1. A small playable portion of the procedural desert with an accessible oasis.
2. The old man, one editable seed, digging/planting, and a bowl that scoops,
   carries, pours, and spills water.
3. A small visual growth grammar that produces visibly different plants.
4. Sunlight, moisture, growth/maintenance costs, and resource-limited development.
5. Basic support and breakage that makes ambitious growth visibly risky.
6. Seed production, collection, replanting, and persistence across save/reload.

The acceptance question is: **Is it fun to grow a peculiar plant, understand
what happened to it, and change its offspring's pattern to help it survive?**

Next, add small mutations, decomposition and soil improvement, followed by rain,
dust-driven burial/dispersal, and autonomous multigeneration spread. This order
is a proposed implementation sequence; the complete game vision includes all
of those interactions.

## Engine foundation and constraints

This branch begins at engine checkpoint `38fe8d9`. Procedural terrain, native
voxel rendering, vegetation experiments, editing, and local water simulation
provide starting capabilities. Programmable growing plants, resource transport,
structural plant failure, ecology, and the bowl interaction still need to be built.

The current water solver has a moving 2 m detailed cube within a 4 m native
footprint. It is not yet evidence that an entire oasis can be simulated at native
detail. Plant and soil simulation will likewise need bounded active work,
persistence, and explicit treatment of distant or inactive regions.

The laptop target remains 1080p at 60 FPS where possible, with 30 FPS acceptable
during development. Simulation and canonical voxel identity must remain
independent of visual LOD. Optimize against actual gameplay, and measure long
sessions and save/reload as the ecosystem grows.

## Decisions to make during prototyping

- The smallest useful growth grammar and a genuinely enjoyable visual editor.
- Tissue properties, resource transport, maintenance, starvation, and reproduction.
- The structural approximation and affordable behavior for detached material.
- How seeds germinate, disperse, mutate, and establish in different soil conditions.
- How the player reads plant health, resource shortages, and stress.
- How to simulate established ecosystems outside the player's immediate area.
- Whether the experience stays an open-ended restoration sandbox or gains
  explicit goals and progression.

These are open decisions, not reasons to add every possible system before the
first seed-to-seed playtest.
