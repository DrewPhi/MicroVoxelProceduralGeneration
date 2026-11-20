# Astra World Generation Brief

Run this experiment on `experiment/astra-worldgen`. Build the most beautiful
procedurally generated world you can at **6.25 cm native voxels**. Aim for a
world worth exploring on foot and from the air. Start from the current
Watershed engine. The generator, renderer, streaming architecture, lighting,
vegetation, and terrain representation are all open to experimentation and
replacement.

## What success looks like

- Distinctive landforms, coherent waterways and shorelines, varied biomes,
  and believable vegetation at walking and landscape scales. Try contrasting
  seeds. Let in-engine images and exploration guide the art direction.
- Edit the actual 6.25 cm terrain during play. Dig, place, and confirm that
  edits affect picking, collision, and rendering. Verify save and reload.
  If you add caves or overhangs, verify their topology through renderer and
  streaming transitions.
- Run well on the Ryzen 5 7640U and Radeon 760M laptop. Measure release builds
  at 1920×1080, starting with MSAA 1. Aim for sustained 60 FPS with stable motion
  and bounded memory. Record frame p50/p95/p99, work or uploads behind hitches,
  and visible behavior during cold starts, walking, flight, sharp turns, and
  edits. Report measured tradeoffs and the settings used.

## Start here

Use the baseline and edit commands in the repository README. On the shared
laptop, the preinstalled Rust toolchain is available through
`../../.toolchains/env.sh` from this directory. Get a measured baseline first,
then make ambitious, testable changes. Seeds 19, 997, and
40961 are useful existing landmarks. Add fresh seeds to avoid tuning to a
single scene. For reproducible runs use the same seed, camera route, resolution,
view distance, and quality settings. Start with the actual adapter and several
representative player views. Keep the working engine recoverable through Git
commits on this experiment branch.

The handoff smoke test on the Radeon 760M ran editable Watershed seed 997
at 1080p MSAA 1, a 1,500 m view distance, and a 32 m exact radius. Nine
seconds at a fixed camera gave frame p50/p95/p99 of 22.5/30.3/32.2 ms.
Measure gameplay and longer view distances separately.

The player height Legacy camera and its matching CPU reference parameters
are in `reference/gallery/player-spiky.json`. The executable reference and
benchmark tools can regenerate captures. Use the independent oracle when
changing near geometry, and test actual movement when changing streaming or
LOD handoffs. Iterate using images and performance together.

Continue autonomously until the user stops the run or the session usage
limit ends it. Keep committing runnable checkpoints and brief evidence as
you go so the next session can resume from the best measured result.
