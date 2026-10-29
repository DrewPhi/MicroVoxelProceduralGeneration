# Voxel player update

Updated 2026-10-28. Character assets, walking physics and in-game camera views
are implemented. The native application starts with a walking player; F5 cycles
first/rear/front views and F6 switches to debug flight. Water remains solid.

## Checklist

### 1. Procedural character and animation inspection — implemented

- [x] Review procedural-animation literature and Rust implementations; record the
      differences between rotating voxel-looking meshes and a voxel-grid body.
- [x] Author an original generic character in meters, with head/torso/limbs,
      beveled shapes, hair, eyes, clothing, scarf and boots.
- [x] Sample the same recipe at 8, 16, 32 and 64 cells/m. Compare 16/32/64 in
      exported previews. The actual world remains at 16 cells/m.
- [x] Reveal smaller features at finer resolution: eye whites/irises, eyebrows,
      cheeks, shirt pocket, boot trim and more finely sampled bevels/hair fringe.
      Each output cell has one solid color; no texture detail below cell size.
- [x] Register semantic character materials into a caller-provided registry;
      retain ordinary four-byte `Voxel` values and empty property maps.
- [x] Implement idle/blink, walk, jump and wave pose sequences. Quantize time to
      a pose index and body facing to 16 headings; rasterize the posed solids
      back onto the axis-aligned grid before meshing.
- [x] Cull hidden faces and reuse the existing color-preserving greedy merger.
- [x] Add a bounded FIFO pose cache and an offscreen native preview command.
- [x] Export static resolution/model-view PNGs and looping APNG animations.
- [x] Verify stable physical height, finer-detail appearance, connected native
      poses, planted supporting feet, deterministic regeneration, hidden-face
      culling, canonical colors and cache bounds.

Preview command (no terrain generation or new dependency required):

```sh
./native/target/release/microvoxel-game --character-preview output/character --seed 42
```

`model_resolution.png`: left to right 6.25 cm / 3.125 cm / 1.5625 cm cells.
`model_views.png`: three views at the actual world resolution.
`idle.png`, `walk.png`, `jump.png`, `wave.png`: animated PNGs, same three columns.
`walk_pose_*.png`: four stills for tools that cannot display APNG.
`README.txt`: actual cell/face/mesh counts, memory and measured generation cost.

### 2. Walking physics and world integration — implemented

- [x] Give the player an integer world root, identity, input state and fixed-step
      controller. Keep local character volumes separate from persistent terrain
      overrides; moving a character must not dig/rebuild the landscape.
- [x] Derive integer collider/envelope bounds from the model and its poses.
      Include the head and make sure visible arms cannot enter solid terrain.
- [x] Sweep the body against canonical cells/column runs, including stored edits;
      unknown space blocks movement, independent of visual LOD.
- [x] Implement grounded state, gravity, jump, wall sliding and a configurable
      whole-cell step height. Accumulate sub-cell velocity credit, never store a
      fractional logical position or teleport over crossed obstacles.
- [x] Check animated pose changes against terrain too: constrain/hold a pose
      where a raised arm or a new facing would enter a wall. Sweeps must include
      the path between old/new occupancies, not just free destination cells.
- [x] Treat water as solid blue ground for now. Defer buoyancy and fluids.
- [x] Drive walk phase from actual accepted travel distance, not the render clock;
      standing against a wall must not keep walking. Use physics for jump root
      motion and select airborne poses without adding the preview jump offset.
- [x] Cache current CPU/GPU pose meshes within bounded banks. Reuse unchanged meshes;
      invalidate on recipe/resolution/palette/pose changes, not every render frame.
- [x] Validate ceilings, corners, slopes, falling, landing, large timesteps,
      chunk boundaries, and every actual movement/pose collision envelope.

### 3. Cameras and presentation — implemented

- [x] F5 cycles first person, rear third person and front third person.
- [x] Snap authoritative camera translation to cells; retain smooth view rotation.
- [x] Sweep a near-plane-sized camera volume toward the desired third-person
      position so terrain cannot hide the player or clip through the camera.
- [x] Hide head/body as appropriate in first person. Show the arm from this same
      recipe and palette, not a separate mesh with unrelated proportions.
- [x] Retain an explicit fly/debug toggle and update controls/help.
- [x] Preview walking from all views on actual generated terrain; record CPU,
      mesh-cache and frame-time observations on the basic laptop target.

### 4. Later systems

- [ ] Fix/tune river and lake generation before adding live water dynamics.
- [ ] Design active-region water simulation separately from character movement.
- [ ] Add live weather around the existing immutable climate baseline.

## Contracts and boundaries

Recipe/pose mathematics may use continuous coordinates, as terrain generation
does. Only sampled whole cells leave the generator. Displaying the character
must never rotate, scale or interpolate its cubes off the selected grid. A
heading change regenerates cells rather than applying a GPU bone matrix.

Center sampling is not conservative overlap voxelization. Very thin features
can disappear at coarse resolutions, and cell counts can vary between poses.
This is not a mass-conserving biological/deformable-body simulation. Persistent
anatomy, voxel identity across deformation and physical joint constraints will
need a separate design if future gameplay requires them.

The first model has one fixed body proportion recipe, with seeded clothing/hair
variation. It is a foundation for more body parameters, not a full avatar editor.
The jump preview includes an explicitly separate integer-cell root track; it is
not a gravity demonstration. Idle/walk/wave stay at the same root. Resolution
changes are isolated asset experiments and do not change terrain/grid units.

See [PLAYER_RESEARCH.md](PLAYER_RESEARCH.md) for references and source review.


## Implementation details and present limits

- `src/physics.rs`: exact occupancy shapes, six leading-cell masks, axis sweeps,
  conservative changed-row pose transition checks and full-volume camera booms.
- `src/player.rs`: fixed 60 Hz controller, fractional velocity credit, integer
  root, gravity/jump, wall sliding, four-cell stepping, footprint-aware spawn,
  distance-driven gait and bounded collision/pose caches.
- `native/src/player_view.rs`: F5 views and near-plane-aware boom retraction.
- `native/src/gpu.rs`: separate integer-root character instances and a 16-entry
  GPU pose/arm mesh cache; terrain storage and save compatibility are unchanged.

Pose transition clearance may be conservative in narrow spaces. Walking uses an
inspection gait rather than terrain-aware foot IK. First-person arm extension is
part of the actual collision pose and can be held back by nearby terrain. The
body is hidden in first person; a blocked camera boom retracts toward eye height.
A fully retracted camera automatically uses the arm-only view rather than showing
the inside of the head. F6 pauses player simulation and returns to its existing
root. There is no rigid-body pushing, swimming, fall damage or player save yet.

Validation: fixed-tick results at 30/60/120/144 FPS, gravity/landing, single jumps,
low ceilings, sprint/wall sliding, step climbing, exact gaps, unknown blocking,
water-as-ground, pose obstruction, camera retraction and a native generated-world
walk/jump/camera smoke test. Measurements are recorded in PERFORMANCE.md.


## Look-following refinement

Implemented separate head yaw/pitch, movement-led torso orientation, idle head
turns with delayed torso catch-up, and look-relative first-person hand aiming.
Head yaw uses 11.25-degree steps and pitch 7.5-degree steps; torso headings remain
22.5-degree steps. Small idle looks leave the torso in place, large looks bring
it around, and a steady look begins torso alignment after about half a second.
These are game-specific Minecraft-style choices rather than a bit-identical
implementation of any Minecraft version.

All articulated solids are voxelized before rendering and collision. A stable
head center keeps the fixed eye anchor protected during pitch changes. Raised
arm ownership is tracked through part overlaps instead of inferred from a crop
box. Pose banks stay bounded; moving/turning a blocked pose is still constrained
by terrain clearance. No new slope/climbing rules or terrain-aware IK were added.

Unscheduled ideas: slope limits, foot placement, climbing/all-fours poses, and
later wind-reactive foliage/branch motion. These should remain separate from the
small player look controller and do not replace the water/weather roadmap.
