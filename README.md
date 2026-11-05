# Frontier Voxel Engine

A Rust voxel-world engine in early development. The target is an editable 6.25 cm voxel world with stable distant terrain,
caves, and bounded streaming work. Performance targets remain to be demonstrated.

Start with the [architecture](frontier/ARCHITECTURE.md),
[agent instructions](frontier/AGENTS.md), and
[implementation roadmap](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/24).
The [developer guide](guide_for_devs.md) explains the current layout and next steps.

The current playtest launches **seeded FBM terrain over 4.096 × 4.096 km**.
It is an experimental surface renderer with independently moving nested grids and
background sample caching. An exact 6.25 cm FBM crop and the editable tunnel/bridge
fixture remain available as separate correctness references.

```sh
cd frontier
cargo build --release --locked
./run.sh --seed 7 --camera valley
./run.sh --scene fbm --seed 42 --scale 1200 --amplitude 700 --camera aerial
./run.sh --scene fbm-native
./run.sh --scene fixture
```

Rust 1.99.1 is pinned. On Linux the window backend uses X11/XWayland.
See the [developer guide](guide_for_devs.md) for controls and measurements.
The broad landscape currently uses a smooth surface proxy: nearby native voxel
handoff and production quality/performance gates remain ahead.

## Legacy references

The old engines, assets, tools, and research notes have been removed from this
branch's working tree. They remain available on GitHub:

- [Previous game engine (`main`)](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/tree/main)
- [Analytic experiments (`DrewsAnalyticRendering`)](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/tree/DrewsAnalyticRendering)
- [Generated datasets, fitted models, and screenshots](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/releases/tag/legacy-research-archive-2026-11-04)

Consult those as references under the Frontier reuse policy; do not restore an old
runtime into this branch by default.

[MIT license](LICENSE)
