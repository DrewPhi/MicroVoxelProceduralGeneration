# Documentation map

Use the entrypoints below for current operation. Checkpoint and experiment reports
preserve the settings, results and limitations of a particular implementation;
their statements about “current” defaults or future work are historical unless
confirmed by the current guide and code. Do not rewrite old measurements to match
new defaults. Raw `output/` paths in those reports identify local provenance and
may no longer exist; curated reference files remain versioned.

## Start here

| Need | Document |
| --- | --- |
| Build, launch, controls and workspace layout | [Developer guide](../guide_for_devs.md) |
| Design contracts and staged gates | [Architecture](ARCHITECTURE.md), [agent instructions](AGENTS.md) |
| Rendering regression and acceptance | [Primary visual check](PRIMARY_VISUAL_CHECK.md), [validation contract](VALIDATION.md) |
| Interactive appearance controls | [Visual preview](VISUAL_PREVIEW.md) |
| Seeded geography and static water | [Seeded world](SEEDED_WORLD.md) |
| Local fluid simulation and saves | [Live water](LIVE_WATER.md) |
| Alpine scene and vegetation | [Alpine Meadow](ALPINE_MEADOW.md), [vegetation](VEGETATION.md) |
| Native-window streaming | [NAADF streaming](NAADF_STREAMING.md) |
| Adaptive terrain and cache controls | [Adaptive rendering](ADAPTIVE_RENDERING.md), [derived disk cache](DERIVED_DISK_CACHE.md) |
| Canonical voxel semantics | [World crate](world/README.md) |
| Offline exact-voxel oracle | [Reference crate](reference/README.md) |
| Standalone hardware experiments | [Voxel RT lab](experiments/voxel-rt/README.md) |

## Plans and research

- [Master roadmap](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/24)
  and [issue 74 plan](ISSUE74_PLAN.md): staged optimization and acceptance work.
- [Seeded-world design brief](agent_context/seeded_world_issue.md): original design
  intent and acceptance split; consult the feature guides for implemented behavior.
- [Voxel methods](research/ISSUE74_VOXEL_METHODS.md),
  [GPU methods](research/ISSUE74_GPU_METHODS.md) and
  [pixel-adaptive audit](PIXEL_ADAPTIVE_RESEARCH_AUDIT.md): research and bounded gates.

## Measured checkpoints and references

These are evidence snapshots, not a chronological list of requirements to implement.

| Area | Reports |
| --- | --- |
| Latest optimization measurements | [November 14](OPTIMIZATION_NOV14.md) |
| Native traversal and storage | [NAADF optimization](NAADF_OPTIMIZATION.md), [seven experiments](NAADF_SEVEN_EXPERIMENTS.md), [follow-up](NAADF_OPTIMIZATION_FOLLOWUP.md), [next checkpoint](NAADF_NEXT_CHECKPOINT.md) |
| Loading and motion | [Loading optimization](LOADING_OPTIMIZATION.md), [loading evidence](reference/loading/README.md), [playtest smoothness](PLAYTEST_SMOOTHNESS_CHECKPOINT.md) |
| Issue 74 | [Initial results](reference/issue74/README.md), [follow-up narrative](reference/issue74/FOLLOWUP.md), [generated follow-up tables](reference/issue74/followup-metrics.md) |
| Issue 75 | [Reference evidence](reference/issue75/README.md) |
| Source/cache experiments | [B1/B2/B3 checkpoint](research/B123_CHECKPOINT.md) |
| Seeded world and water | [Seeded-world evidence](reference/seeded-world/README.md), [quality follow-up](reference/seeded-world/quality-followup/README.md), [live-water evidence](reference/seeded-world/live-water/README.md) |
| Vegetation | [Streaming](VEGETATION_STREAMING_CHECKPOINT.md), [next checkpoint](VEGETATION_NEXT_CHECKPOINT.md), [distant vegetation](DISTANT_VEGETATION.md), [ground cover](GROUND_COVER.md) |
| Visual targets | [Vegetation reference](VEGETATION_REFERENCE.md), [mountain overlook](MOUNTAIN_OVERLOOK_REFERENCE.md), [Alpine beauty](reference/BEAUTY_CHECKPOINT.md) |
| Early renderer comparisons | [Prototype](PROTOTYPE_CHECKPOINT.md), [moving native](MOVING_NATIVE_CHECKPOINT.md), [ownership](OWNERSHIP_CHECKPOINT3.md), [handoff](HANDOFF_CHECKPOINT.md) |
| Early appearance work | [LOD polish](LOD_POLISH_CHECKPOINT.md), [Regional terrain](REGIONAL_TERRAIN_CHECKPOINT.md), [terrain beauty](BEAUTY_CHECKPOINT.md) |

Other `*_CHECKPOINT.md`, `NAADF_*.md` and reference subdirectory reports retain
additional experiment history. In particular, the two beauty documents cover
different subjects, and `reference/issue74/FOLLOWUP.md` is the narrative while
`followup-metrics.md` contains generated tables; neither pair is duplicate evidence.

## Local generated files

`target/` contains rebuildable Rust binaries. `output/cache/` contains disposable
derived terrain pages; deleting those pages causes regeneration on the next run.
Other `output/` directories can contain captures, expensive independent references
and **authoritative live-water `.save` files**. They are ignored by Git, not
necessarily disposable: preserve saves and reference inputs when cleaning up.
