# Procedural voxel character research

Reviewed 2026-10-28. New implementation is original Rust; no external character
assets, animation source or library implementation were copied or vendored. The
core remains dependency-free and the preview uses existing wgpu/PNG dependencies.

## Literature

- [Perlin, *Real Time Responsive Animation with Personality*, 1995; NYU project
  bibliography](https://mrl.cs.nyu.edu/projects/improv/): procedural rhythms and
  layered motion motivate compact parameterized poses. Our small pose vocabulary
  uses fixed phase samples, not the original system or its implementation.
- [Frisken et al., *Adaptively Sampled Distance Fields*, SIGGRAPH 2000](https://graphics.stanford.edu/courses/cs468-03-fall/Papers/frisken00adaptively.pdf):
  implicit solids separate shape definition from sample resolution. We use small
  analytic beveled boxes and center-inside tests, not an adaptive distance-field
  tree, reconstructed smooth surfaces or the paper's code.
- [Schwarz and Seidel, *Fast Parallel Surface and Solid Voxelization on GPUs*,
  SIGGRAPH Asia 2010](https://michael-schwarz.com/research/publ/2010/vox/): useful
  distinction between conservative overlap and thinner voxelizations. Our tiny
  analytic character uses CPU center sampling; it does not implement their GPU
  triangle/box tests and does not guarantee all touched cells are occupied.
- [Lysenko, *Meshing in a Minecraft Game*, 2012](https://0fps.net/2012/06/30/meshing-in-a-minecraft-game/):
  hidden-face culling and greedy coplanar merging. The existing project merger
  also preserves independently colored cells in a nearest-cell color atlas.

## Rust source review

| Project / inspected source | Useful ideas | Decision |
|---|---|---|
| [Veloren `character/run.rs`](https://github.com/veloren/veloren/blob/master/voxygen/anim/src/character/run.rs), [animation API](https://veloren.gitlab.io/veloren/veloren_voxygen_anim/index.html) | Procedural gait parameters and separation of animation from skeleton/body attributes | Inspiration only. Bone rotations of already-meshed voxels would violate our global grid rule; we voxelize after posing. GPL-3.0 project; no source/assets imported |
| [block-mesh `greedy.rs`](https://github.com/bonsairobo/block-mesh-rs/blob/main/src/greedy.rs) | Reusable work buffers, visibility versus merge compatibility, boundary padding | Retain our existing greedy merger. MIT/Apache-2.0 reference; no added dependency |
| [binary-greedy-meshing](https://github.com/Inspirateur/binary-greedy-meshing) | Occupancy bit masks, packed quads and reusable meshing data | Candidate if profiling many animated bodies makes meshing expensive. Its documented fixed 62³ interior is not a natural fit for all our character sizes. Do not assume its published benchmark speedups apply to this workload |
| [Rapier controller guide](https://rapier.rs/docs/user_guides/rust/character_controller/), [`character_controller.rs`](https://github.com/dimforge/rapier/blob/master/src/control/character_controller.rs) | Desired motion separated from corrected motion, autostep headroom, contact filtering and self-exclusion | Used as a correctness/design checklist for the implemented controller. A general continuous-coordinate rigid-body solver is not currently needed; no physics dependency added |

The GitHub sources were inspected through the connected GitHub app. Reference
file blob hashes: Veloren run `260fb6b192b18e20117357bcb999e3c3af5d0487`;
block-mesh greedy `5d8a6d4349e518e1490fbb92cfd152d92488a32a`;
Rapier controller `888724915fa43d53180d02387f58ecfb6241a444`. These identify
inspected file contents, not pinned runtime dependencies.

## Implementation choices

`src/character/recipe.rs` authors a character in meters. Each posed primitive
is restricted to its transformed bounding box before voxel center tests. This
avoids testing every primitive throughout the entire enclosing volume. Sampling
assigns an ordinary material/appearance pair to each occupied integer cell.
Features below useful resolution are simplified: eye whites, brows, cheeks and
clothing trim appear at finer grids. No smooth normals, textures or sub-cell
color noise are used.

`animation.rs` contains deterministic clip/phase/facing selection. Continuous
pose math remains inside generation. `volume.rs` exposes cells and meshes only
faces adjacent to air; all part intersections share one occupancy field, so
internal faces between limbs and torso are removed.

`PoseCache` owns one recipe/resolution/palette combination. It keeps at most 16
frames and a caller-defined byte limit; regeneration is deterministic. Its byte
counter measures retained voxel arrays and arm ownership indices, not process RSS, temporary mesh
scratch or external `Arc` owners. The preview uses a 16 MiB limit per resolution.
At native resolution a whole clip fits comfortably; 64 cells/m intentionally
retains fewer frames under the same limit.

The preview exports streaming image frames and reuses the terrain GPU rendering
pipeline with an explicit cell-size uniform. World rendering still passes
exactly 0.0625 m; alternate asset resolutions never alter canonical world units.
Preview mesh uploads are inspection/export work, not a real-time character
throughput benchmark. The game separately caches up to 16 immutable pose/arm
meshes and translates them by an integer root.

## Limitations to carry forward

- Voxelizing rotated solids may change occupied cell counts/material ownership.
  We promise a discrete, connected native pose, not conserved voxel mass.
- Asset generation alone does not check terrain. The in-game player now checks
  unit-step root sweeps and conservative row envelopes of changed pose occupancy.
  A pose can be held near obstructions; the active tick shape stays exact.
- The walk is an inspection gait. In-game phase now follows accepted travel
  distance; terrain-aware foot IK remains unimplemented.
- Jump root offsets are a separate preview track, quantized to cells. In-game
  gravity supplies root motion and does not add that decorative offset.
- Native material properties remain empty. Anatomy, inertia, joints, health,
  crafting and fluids have not been introduced.

The complete implementation order is in [PLAYER_UPDATE_PLAN.md](PLAYER_UPDATE_PLAN.md).


## Head/body and first-person aim refinement

[Mojang's player packet documentation](https://mojang.github.io/bedrock-protocol-docs/1.26.51/packets/move-player-packet/)
distinguishes look rotation from body orientation. The
[official entity modeling guide](https://learn.microsoft.com/en-us/minecraft/creator/documents/entitymodelingandanimation?view=minecraft-bedrock-stable)
also describes separately articulated parts. We use the general head/body
separation, with original game-specific movement/idle-follow rules, not copied
Minecraft code or version-specific timing guarantees.

Look pose keys now include relative head yaw/pitch. The torso follows walking
with bounded strafe offsets; an idle head can look aside before the body catches
up. The first-person hand uses the eye's right/forward/up basis. General 3D part
transforms are sampled back to cells, and anatomical right-arm ownership survives
head/body overlap for correct first-person filtering. Larger raised-arm volumes
and ownership indices are included in pose-cache memory accounting.
