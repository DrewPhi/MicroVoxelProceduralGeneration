# Frontier Voxel Engine

A Rust voxel-world engine in early development. The target is an editable 6.25 cm voxel world with stable distant terrain,
caves, and bounded streaming work. Performance targets remain to be demonstrated.

Start with the [architecture](frontier/ARCHITECTURE.md),
[agent instructions](frontier/AGENTS.md), and
[implementation roadmap](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/24).
The [developer guide](guide_for_devs.md) explains the current layout and next steps.

The default playtest launches seeded FBM terrain over a 4.096 × 4.096 km square.
It is an experimental surface renderer with independently moving nested grids
and background sample caching. `--scene hybrid` combines a moving 64 × 64 m
exact-voxel interior, reusable 8 m chunks, an approximate 2 m collar, and the GPU
proxy. Matched replacements publish together after bounded chunk uploads.
`--scene hybrid-static` preserves the old fixed CPU reference. These FBM scenes
represent surface terrain only: cave/edit handoff and collision remain absent,
and transitions can remain visible. The editable tunnel/bridge fixture is a
separate correctness reference.

```sh
cd frontier
cargo build --release --locked
./run.sh --seed 7 --camera valley
./run.sh --scene fbm --seed 42 --scale 1200 --amplitude 700 --camera aerial
./run.sh --scene fbm-native
./run.sh --scene fbm --seed 7 --camera aerial --view-levels 11
./run.sh --scene hybrid --seed 7 --msaa 4 --voxel-filter off
./run.sh --scene hybrid-static --seed 7
./run.sh --scene fixture
```

Rust 1.99.1 is pinned. On Linux the window backend uses X11/XWayland.
See the [developer guide](guide_for_devs.md) for controls and measurements.
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
