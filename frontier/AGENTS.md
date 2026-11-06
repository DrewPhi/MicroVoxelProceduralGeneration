# Frontier agent instructions (read before implementing issues)

This file governs all work under `frontier/` on branch `frontier-voxel-engine`.

## 0. Do not prematurely implement everything
Start with [ARCHITECTURE.md](ARCHITECTURE.md) and [master issue #24](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/24). Follow the staged [FV] issues and architecture gates G0, G1, G2, G3, G4. A renderer being elegant, new, or published does not prove it is the best choice for the target game. Use matched reference scenes and hardware timings. Do not claim the project is guaranteed 'state of the art' without measurements.

## 1. Branches and when to reference them

- **Working branch:** `frontier-voxel-engine`, with new implementation under `frontier/`. At the user's request on 2026-11-04, inherited engines, assets, tests, tools, Cargo files and obsolete CI were removed from this branch for a fresh start. Legacy source remains on its original remote branches and in Git history. Inspect it with `git show` or a separate temporary benchmark checkout; do not restore whole legacy source trees here. This cleanup is not promotion of a new runtime and does not change the legacy branches.
- **`main` / `successor/`: current old voxel/game engine.** Safe to *inspect* any old source to understand a test, data model, or a known failure; never automatically reuse its scheduler, CPU coarse terrain meshing, publication transitions, or LOD-quality approximation.
- **`engine-overhaul`: historical prior overhaul snapshot.** Safe for provenance, contrast and regression investigation only. Do not switch the new implementation to that source tree or blend its old runtime into `frontier/`.
- **`DrewsAnalyticRendering`: independent analytic quadratic/FBM experiments.** Safe to inspect FBM visual direction, scene fixtures, performance profiling, and negative fitting evidence. Do not make quadratic optimizer, shader, patch traversal, or globally matched coefficients a prerequisite for frontier work.
- **`continuous-terrain`: exploratory procedural fitting terrain.** Read-only idea/test reference; no assumptions that it solves new voxel or streaming requirements.
- **External open-source projects:** inspect for algorithms, not as drop-in replacements. Track licenses/dependencies and validate portability to Rust/wgpu/hardware. Avoid copying large code blocks or assuming published numbers transfer to our scenes.

## 2. Reuse tiers — each issue declares its own

- **R (read-only):** old code to understand behavior, reproduce exact failure, or document data semantics; write new implementation.
- **B (benchmark only):** build frozen old binary and compare matched seed/camera/hardware/quality, without transplanting code.
- **P (selective port):** only a narrowly defined, independent and tested algorithm/data structure (e.g. integer cell coordinates, edit journal, bitmask native surface extraction, GPU allocator). Document the exact provenance `branch/path@commit`, dependencies, tests, precise behavior retained, adaptation, and why greenfield is worse. Verify exact voxel equality or performance as applicable. Any old tests must be run against new code.
- **G (greenfield):** avoid old runtime/render architecture even if similar-looking functions exist; use new contracts and tests.

When a task lists R or B, **do not 'just copy' old code**; when it lists P, copying is *not* automatically allowed until the tests/review are in the PR. Every PR should state its tier and source paths.

## 3. Mandatory failure cases

- At 6.25 cm exact voxel/native coordinates, terrain, cave, bridge, tree/building editing and physics agree, regardless of displayed LOD.
- User can see mountain ridges and river valleys several miles away. No forced opaque fog to hide unavailable geometry. Use a no-haze debug/reference setting.
- Walking, sprint, high-speed bank and teleport never publish obsolete visibility selections or stale world revisions. Valid cached content remains reusable across camera epochs. Destination coverage must be coherent before teleport presentation; measure cold readiness time separately.
- A heightfield **cannot** represent a cave opening, overhang, building, edited tower or arbitrary vertical geometry. Separate volume ownership must be conserved at *all significant distances*. Never silently erase openings in coarse geometry.
- Coarse display-cell size is **not** a silhouette-error guarantee. Track actual/permitted geometric deviation and use feature-aware conservative refinement.
- Smooth-looking proxy transitions do not alter underlying integer voxel identity. Direct heightfield-to-exact-block handoff needs depth and ownership tests.
- Floating point GPU & CPU world functions can disagree: insist on stable integer cell address, local origins, explicit canonical source and tested parity/quantization.
- Small jobs remain cancellable during CPU work, upload and publication. No old 500ms replan or 12m prefetch bound.

## 4. Tests and deliverables

**Reference-first visual workflow (user direction, 2026-11-06):** for every visual
feature where practical, first produce a non-real-time, high-quality reference
with explicit source, camera, materials, lighting and sampling. Let the user
approve the visual target, then optimize the interactive result toward it.
Keep exact/native data independent of the display approximation. A beautiful
reference is an artistic target, not proof of a real-time implementation's speed
or correctness. For motion and LOD transitions use offline reference trajectories
and temporal comparisons too; one still cannot certify invisible transitions.
Keep existing correctness tests active while detailed transition polish is deferred.
New Regional/atmospheric targets supplement rather than overwrite the approved
Legacy comparison. #74 coordinates efficiency research; do not silently turn an
unmatched lighting or terrain change into an apparent optimization win.

**Primary visual regression check (user-selected):** the fixed player-height spiky-terrain comparison in [PRIMARY_VISUAL_CHECK.md](PRIMARY_VISUAL_CHECK.md). Run it first for changes to LOD selection, transitions, geometry, filtering or lighting. Preserve the fixed source, camera, voxel size and matched 300 m clip; compare against the independent exact-voxel reference. Keep cave/bridge, regional terrain and motion tests as secondary coverage. A prettier aerial image does not replace this check.

Before marking an issue done: commit code/docs and reproducible tests; include seed/world version, camera route, GPU/CPU model, 1080p resolution, geometry representation, actual native voxel size, frame time p50/p95/p99, page/mesh count, pending memory, and side-by-side images/video where visual impact matters.

No 1 km GPU fit or extremely costly optimization campaign without a CPU or small-world gate. Preserve stable old launch behavior and saved worlds.

## 5. Suggested first implementation tasks

[FV01 isolated workspace](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/25), [FV02 test harness](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/26), [FV03 canonical voxels](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/27), [FV04 precision](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/28). Then a simple mountain/cave/bridge/edit ownership smoke gate (early FV23/FV31), minimal clipmap baseline and renderer A/B. Rich geology/erosion is not a prerequisite for these small experiments. Follow the agreed quality contract in ARCHITECTURE.md: laptop 60 FPS and voxel-faithful, screen-space-bounded appearance must pass together. Full G3/G4 remain later gates.
