# FBM builder checkpoint

Branch: DrewsAnalyticRendering. First authoring tool only; analytic fitting and
rendering intentionally remain subsequent experiments.

User constraints: explicit Preview button; no automatic/live regeneration;
10 km square world; default 6.25 cm native voxels; 3D occupancy with caves and
overhangs; small readable Rust components. Main goal is generating targets for
future min-of-implicit-quadrics fitting, not expanding a game engine.

A global meter-space density field is sampled at cell centers, solid iff positive.
Normalized fBM defines landforms; separate 3D noise displaces the surface; two
noise bands intersect to carve tunnel-like voids below an adjustable roof depth.
The field is not an SDF. No gradient-zero patch border constraints are imposed.

Validation: six release tests cover deterministic global queries, independent
resolution, cave subtraction, exact flat-ground DDA, 150 random oblique rays
against explicit voxel-box intersections, recipe roundtrip/cancellation and
binary fixture agreement including cancelled-export preservation. Formatting and
build pass. Clippy was unavailable in the installed temporary Rust toolchain.

Manual GUI smoke: private Xvfb + software OpenGL; actual Preview button clicked,
completed overview displayed. Default headless output inspected for all views.
Observed local timings (not hardware GPU benchmarks): 512² overview 0.08 s,
320×240 local exact occupancy 1.01 s, 512×384 cutaway 0.01 s, native 16 m export
4.71 s. All output is ignored under output/. GUI controls/PNG are tested using
the same render path as headless mode.

Density reference: https://developer.nvidia.com/gpugems/gpugems3/part-i-geometry/chapter-1-generating-complex-procedural-terrains-using-gpu
Noise reference/source: https://github.com/Auburn/FastNoiseLite
GUI: https://github.com/emilk/egui

Do not claim that the overview renders all native voxels, that the temporary DDA
is the analytic renderer, that the terrain is geologically realistic, or that
noise caves are guaranteed connected. Recipes version the field definition;
bump the version if canonical generation changes later.

Preview clarification: default selected crop has a broad cave opening; rendering
with cave_width=0 fills that gap. It is not two independently generated layers.
Local/isometric/cutaway albedo now uses uncarved surface-relative depth so cave
floors and exposed interior cut faces are gray stone, with green only in a thin
outer skin. Albedo remains independent of viewed face. Seven release tests pass.
