# Frontier Voxel Engine

A Rust voxel-world engine in early development. The target is an editable 6.25 cm voxel world with stable distant terrain,
caves, and bounded streaming work. Performance targets remain to be demonstrated.

Start with the [architecture](frontier/ARCHITECTURE.md),
[agent instructions](frontier/AGENTS.md), and
[implementation roadmap](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/24).
The [developer guide](guide_for_devs.md) explains the current layout and next steps.

The first runnable checkpoint is a bounded **16 × 14 × 16 m native-voxel
reference scene** with a mountain, tunnel and bridge. It validates the world,
mesh and camera foundations; it is not yet the distant-terrain/streaming engine.
The laptop 60 FPS target and architecture gates remain unproven.

```sh
cd frontier
cargo build --release --locked
./run.sh
```

Rust 1.99.1 is pinned. On Linux the initial window backend uses X11/XWayland.
See the [developer guide](guide_for_devs.md) for controls, captures and tests.

## Legacy references

The old engines, assets, tools, and research notes have been removed from this
branch's working tree. They remain available on GitHub:

- [Previous game engine (`main`)](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/tree/main)
- [Analytic experiments (`DrewsAnalyticRendering`)](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/tree/DrewsAnalyticRendering)
- [Generated datasets, fitted models, and screenshots](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/releases/tag/legacy-research-archive-2026-11-04)

Consult those as references under the Frontier reuse policy; do not restore an old
runtime into this branch by default.

[MIT license](LICENSE)
