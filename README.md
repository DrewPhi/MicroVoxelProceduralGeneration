# Frontier Voxel Engine

A fresh Rust voxel-world engine, starting with its architecture and implementation
plan. The target is an editable 6.25 cm voxel world with stable distant terrain,
caves, and bounded streaming work. Performance targets remain to be demonstrated.

Start with the [architecture](frontier/ARCHITECTURE.md),
[agent instructions](frontier/AGENTS.md), and
[implementation roadmap](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/24).
The [developer guide](guide_for_devs.md) explains the current layout and next steps.

This branch currently contains planning documents only. There is no runnable
engine, Cargo workspace, or build workflow yet. Those belong to the staged
Frontier implementation, starting with
[FV01: isolated workspace](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/25).

## Legacy references

The old engines, assets, tools, and research notes have been removed from this
branch's working tree. They remain available on GitHub:

- [Previous game engine (`main`)](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/tree/main)
- [Analytic experiments (`DrewsAnalyticRendering`)](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/tree/DrewsAnalyticRendering)
- [Generated datasets, fitted models, and screenshots](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/releases/tag/legacy-research-archive-2026-11-04)

Consult those as references under the Frontier reuse policy; do not restore an old
runtime into this branch by default.

[MIT license](LICENSE)
