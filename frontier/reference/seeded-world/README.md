# Seeded world checkpoint evidence

See [the playtest guide](../../SEEDED_WORLD.md) and [issue #87](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/issues/87). Source 7 / appearance 2; native cells 0.0625 m. These records supplement the unchanged Legacy primary reference.

`metrics.csv` and `biomes.csv` record sparse 257×257 CPU galleries for three seeds at 10 km and 100 km. They do not resolve every river or every native voxel. Rebuild with the gallery commands in the guide.

`walk19-summary.json` retains measured hardware, camera, source, quality, resident geometry and memory, frame percentiles, and NAADF statistics from the real-time walk. The full per-frame log and images remain in ignored `output/seeded-world/`. No moving frame met requested distant quality; do not interpret the approximately 59 FPS median as the combined visual-quality gate passing.

Reproduce from `frontier/` after a release build:

```sh
target/release/frontier --preset seeded-world --seed 19 --pixel-error 5 --capture-far 6000 --resource-profile development --no-adaptive-disk-cache --benchmark-input-lock --adaptive-wait-quality --uncapped --warmup 30 --seconds 15 --route walk-turn-return --route-clock realtime --route-speed 2.5 --report output/seeded-world/walk19.json --capture-on-exit output/seeded-world/walk19.png --width 1920 --height 1080
```

The 30 warmup units are frames. Native expansion reached 64 m in 4.88 s before the measured route. Frame-time results exclude screen recording. GPU memory is separate from process RSS. Broader seeds, routes, long travel, distant feature preservation and live fluids remain open.

Validation: 665 workspace Rust tests passed (37 ignored), shader validation and water-aware codec tests passed, and the explicit GPU canonical water/bed/air test passed with both compact and wide leaves. The fixed Legacy comparison was rerun. See `naadf_seeded_world_water_bed_air_matches_canonical_rays` for the GPU test fixture and `seeded_bounds_tests.rs` for independent held-out bound checks.
