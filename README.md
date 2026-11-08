# Frontier Voxel Engine

A Rust voxel-world engine in early development. The target is an editable 6.25 cm voxel world with stable distant terrain,
caves, and bounded streaming work. Performance targets remain to be demonstrated.

Start with the [architecture](frontier/ARCHITECTURE.md),
[agent instructions](frontier/AGENTS.md), and
[implementation roadmap](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/24).
The [developer guide](guide_for_devs.md) explains the current layout and next steps.
The [#74 optimization plan](frontier/ISSUE74_PLAN.md) and
[first measured checkpoint](frontier/reference/issue74/README.md) track the current research and remaining performance work.

An [independent offline voxel reference](frontier/reference/README.md) now provides
supersampled images, coverage/depth data and matched GPU comparisons. See the
[reference checkpoint](frontier/REFERENCE_CHECKPOINT.md) for measured results and scope.

The default playtest launches the experimental adaptive renderer with Regional
terrain, detailed appearance maps, voxel appearance filtering and 4×MSAA. Press **Esc** for live light and
atmosphere controls plus image-quality and loading settings. See the
[interactive visual preview](frontier/VISUAL_PREVIEW.md) for controls and scope.
Use `--terrain-recipe legacy` for the original spiky terrain.
Use `--preset alpine` for the [Alpine Meadow scene](frontier/ALPINE_MEADOW.md),
now with [seeded voxel grass, edelweiss and flowering rowans](frontier/VEGETATION.md),
grounded walking, a winding path and mountain backdrop. See the [playability pass](frontier/PLAYABILITY_CHECKPOINT.md) for nearby streaming, cache defaults, camera easing and motion capture tools. The [cozy meadow checkpoint](frontier/COZY_SCENE.md) adds full-seed growth, fuller crowns, denser ground cover, HDR lighting and shadowed near haze. Press F for flight;
`--vegetation off --walk off` restores the bare-terrain camera preview.
The [Alpine beauty checkpoint](frontier/reference/BEAUTY_CHECKPOINT.md) documents
the default lighting treatment, `--beauty off` and its bounded raster reference.
See the [terrain comparison](frontier/REGIONAL_TERRAIN_CHECKPOINT.md) and
[basic lighting/painting pass](frontier/BEAUTY_CHECKPOINT.md).
`--scene hybrid` combines moving exact native terrain chunks with a GPU proxy;
`--scene hybrid-static` keeps the earlier fixed reference. The new
`--scene hybrid-topology` adds canonical caves, a bridge and editable voxels in a
fixed **16 × 16 m volume ownership zone**. Exact volume geometry remains resident
at every distance while terrain coverage moves. This is a bounded correctness
prototype; distant boundary seams and the production ownership gate remain open.

The editable `fixture` also supports `--renderer mesh|brick|brick-skip` for a
bounded greedy-mesh versus exact brick-ray comparison. `--scene fbm
--surface-layout clipmap|quadtree` compares the moving nested-grid baseline with
a static quadtree selection probe. These experiments do not establish a production
renderer winner, collision, persistence, or laptop performance acceptance.

The default `--scene adaptive` path replaces the fixed native square
with world-aligned clusters selected by projected geometry error. It uses pooled
GPU storage and canonical face/material summaries; `adaptive-topology` retains
the exact cave/bridge owner. See [adaptive rendering](frontier/ADAPTIVE_RENDERING.md)
for controls, mathematical guarantees, and validation limits. It is a candidate
under comparison with the existing renderer, not a promotion of all quality or
performance gates.

```sh
cd frontier
cargo build --release --locked
./run.sh --preset alpine
./run.sh --seed 7 --camera valley
./run.sh --scene fbm --seed 42 --scale 1200 --amplitude 700 --camera aerial
./run.sh --scene fbm-native
./run.sh --scene fbm --seed 7 --camera aerial --view-levels 11
./run.sh --scene hybrid --seed 7
./run.sh --scene hybrid-static --seed 7
./run.sh --scene hybrid-topology --camera topology
./run.sh --scene hybrid-topology --camera cave
./run.sh --scene hybrid-topology --camera bridge
./run.sh --scene hybrid --teleport-test --seconds 30
./run.sh --scene fixture --renderer mesh --msaa 1
./run.sh --scene fixture --renderer brick --msaa 1
./run.sh --scene fixture --renderer brick-skip --msaa 1
./run.sh --scene fbm --surface-layout quadtree --camera aerial
./run.sh --scene adaptive --pixel-error 4
./run.sh --scene adaptive-topology --camera cave --pixel-error 4
```

Rust 1.99.1 is pinned. On Linux the window backend uses X11/XWayland.
See the [developer guide](guide_for_devs.md) for controls and measurements.
See the [prototype checkpoints 3–5](frontier/PROTOTYPE_CHECKPOINT.md) for the
current comparison evidence and limitations, and the
[bounded topology ownership notes](frontier/OWNERSHIP_CHECKPOINT3.md) for the
canonical volume contract. Press **T** in either moving hybrid scene to prepare
and publish a teleport destination before switching the camera. GPU render-pass
timestamps are recorded automatically when supported by the adapter.
See the [moving native checkpoint](frontier/MOVING_NATIVE_CHECKPOINT.md) for
measurements and limits, and the [earlier handoff checkpoint](frontier/HANDOFF_CHECKPOINT.md)
for the fixed reference and view-distance experiment. `--msaa 1|4` controls sample
coverage; `--voxel-filter on|off` enables an appearance comparison. For a repeatable
headless route view, combine `--capture PATH --route fly --route-speed 3
--capture-route-time 4`; coverage is prepared at that fixed route pose before capture.
Production ownership, appearance, and performance gates remain ahead.

## Legacy references

The old engines, assets, tools, and research notes have been removed from this
branch's working tree. They remain available on GitHub:

- [Previous game engine (`main`)](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/tree/main)
- [Analytic experiments (`DrewsAnalyticRendering`)](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/tree/DrewsAnalyticRendering)
- [Generated datasets, fitted models, and screenshots](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/releases/tag/legacy-research-archive-2026-11-04)

Consult those as references under the Frontier reuse policy; do not restore an old
runtime into this branch by default.

[MIT license](LICENSE)
