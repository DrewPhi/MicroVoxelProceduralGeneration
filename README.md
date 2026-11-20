# Frontier World Generation Experiment

Astra's starting point is the current Frontier engine at **6.25 cm per
voxel**, with the Watershed procedural world, adaptive distant terrain, and
exact NAADF terrain near the player. The experiment brief lives in
[frontier/AGENTS.md](frontier/AGENTS.md).

## Play the baseline

Build from the repository root, then start the Watershed preset.

```sh
cd frontier
cargo build --release --locked --bin frontier
./run.sh --preset watershed --seed 997 --voxel-size 0.0625
```

**F** toggles walking and flight. **Esc** opens settings. **F12** saves a
screenshot. The default radius of 1,024 voxels is 64 m at this grid size.

## Play with persistent terrain edits

```sh
./run.sh --preset watershed --seed 997 --voxel-size 0.0625 \
  --live-water on --water-save output/worlds/watershed-997.save
```

Click to capture the mouse. Left click and release to collect terrain.
Right click places the selected material from inventory. Keys **1–6**
select materials. Edit within the resident exact region and exit normally
to flush the save. Reopen with the same seed, recipe, and voxel size.
The existing save path goes through the local water driver. Distant terrain
currently derives from the procedural source and can omit edits outside the
exact region.

## Code and checks

- `frontier/world/` holds procedural terrain, the canonical lattice, edits,
  vegetation, and simulation.
- `frontier/renderer/` holds the viewport, NAADF, LODs, streaming, and shaders.
- `frontier/runtime/` holds frame timing and reproducible camera routes.
- `frontier/reference/` holds the independent CPU rendering tools.
- `frontier/tools/` holds benchmarks, the launch binary, and input checks.

From `frontier/`, use `cargo check --locked --workspace --all-targets` and
`cargo test --locked -p frontier-world` for development checks. To verify
digging, placing, inventory, and saving on Linux/X11, run the smoke test
after building the release launcher.

```sh
python3 tools/player_edit_check.py --output output/edit-smoke
```

Generated files go under ignored `frontier/output/`. The current engine
and historical development context remain available in the
[Frontier engine branch](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/tree/frontier-voxel-engine).
