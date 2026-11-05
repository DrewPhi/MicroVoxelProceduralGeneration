# Guide for developers

## Current state

Branch: `frontier-voxel-engine`. This is a planning-only starting point. The old
source trees, tests, launchers, assets, Cargo manifests, and Rust CI have been
removed from this branch. No new engine implementation or runnable build exists.

## Layout

| Path | Purpose |
|---|---|
| `frontier/ARCHITECTURE.md` | World contracts, candidate renderers, streaming, research, and acceptance gates |
| `frontier/AGENTS.md` | Instructions for staged work and selective legacy reuse |
| `README.md` | Project status, roadmap, and archive links |
| `LICENSE` | MIT license |

## Begin implementation

Read both Frontier documents and the
[master issue #24](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/24).
Start with [FV01 #25](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/25)
for the isolated workspace, followed by the benchmark harness, canonical voxel
contract, and coordinate precision work. Create build commands and CI alongside
that implementation; do not reuse obsolete commands from the archived engine.

The planned workspace separates `frontier/world`, `frontier/renderer`,
`frontier/runtime`, and `frontier/tools`. These are planned modules, not existing
crates. Renderer choices must pass the architecture's measured comparison gates.

## Read old code without restoring it here

Remote branches preserve the reference engines. For example:

```sh
git fetch origin
git show origin/main:successor/README.md
git show origin/DrewsAnalyticRendering:agent_context/FITTING_AUDIT.md
```

For reproducible baselines, record the exact commit and use a separate temporary
checkout when needed. Apply the R/B/P/G reuse tiers in `frontier/AGENTS.md`.
Generated legacy data are preserved in the
[archive release](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/releases/tag/legacy-research-archive-2026-11-04).
