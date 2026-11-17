# Frontier Voxel Engine

A Rust voxel-world engine in early development, targeting editable 6.25 cm voxels,
stable distant terrain, caves and bounded streaming work. Production quality and
performance gates remain open.

On the `cellular-automata-game` branch, we are building a desert restoration game
about designing seeds and growing an ecosystem. See [the game plan](GAME_PLAN.md)
for the concept, agreed mechanics, open decisions, and first playable checkpoint.
The first [procedural desert preset](frontier/DESERT.md) is available with
`./run.sh --preset desert --seed 19` from `frontier/`. The
[first bowl interaction](frontier/BOWL.md) supports scooping, carrying, pouring,
spilling and saved soil moisture. [Planting and basic seedling growth](frontier/PLANTING.md)
adds four selectable pockets, visible held items, charged handful pickup/drop,
a black seed, physical burial, sunlight, energy budgets and local root water uptake.

## Build and play

Rust 1.99.1 is pinned in `frontier/rust-toolchain.toml`. Linux requires X11/XWayland,
a graphics driver and the usual C linker/build tools.

```sh
cd frontier
cargo build --release --locked
./run.sh --preset alpine
```

`run.sh` launches the existing release binary; rebuild after source changes.
The default adaptive scene combines exact NAADF voxels within a growing 64 m cube
radius with distant adaptive raster terrain. Startup publishes an 8 m region and
expands in 4 m shells while you move. Regional terrain, voxel appearance filtering,
single-sample rendering and scatter uploads are enabled by default.

- `--preset alpine`: grounded walking, seeded vegetation and a mountain backdrop.
- `--preset seeded-world --seed 19`: streamed climate biomes and static waterways,
  without vegetation. Use `--seed random` to explore another reproducible world.
- `--terrain-recipe legacy`: the original spiky terrain used by the primary visual check.
- `--naadf off`: the raster comparison path; `--msaa 4` explicitly enables MSAA.

Press **Esc** for lighting, atmosphere, quality and loading settings. **F** toggles
flight in walking presets. See the [developer guide](guide_for_devs.md) and
[visual preview](frontier/VISUAL_PREVIEW.md) for controls and experimental scenes.
The opt-in [live-water demo](frontier/LIVE_WATER.md) includes bounded local flow,
editable dams and saved fluid state.

## Development and evidence

Start with the [architecture](frontier/ARCHITECTURE.md),
[agent instructions](frontier/AGENTS.md) and
[implementation roadmap](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/24).
The [documentation index](frontier/DOCUMENTATION.md) separates current entrypoints,
plans and historical checkpoints.

For rendering changes, run the [primary visual check](frontier/PRIMARY_VISUAL_CHECK.md)
first. The [independent offline oracle](frontier/reference/README.md) supplies exact
voxel references; images alone do not establish motion or performance acceptance.
The [November 14 optimization checkpoint](frontier/OPTIMIZATION_NOV14.md) records
measured cache/edit improvements, rejected experiments and large-radius limits.
A fully loaded 256 m run remained below the 30 FPS playtest target on the measured
laptop; the default stays 64 m. This does not close the broader architecture gates.

```sh
cd frontier
cargo fmt --all -- --check
cargo test --workspace --locked
```

Keep generated captures, reports and caches under ignored `frontier/output/`.
Curated evidence under `frontier/reference/` is versioned and should be preserved.

## Legacy references

Older engines and assets remain on their original branches and in Git history:

- [Previous game engine (`main`)](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/tree/main)
- [Analytic experiments (`DrewsAnalyticRendering`)](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/tree/DrewsAnalyticRendering)
- [Historical artifact archive](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/releases/tag/legacy-research-archive-2026-11-04)

Follow the Frontier reuse policy when consulting them; do not restore whole old
runtime trees into this branch. [MIT license](LICENSE).
