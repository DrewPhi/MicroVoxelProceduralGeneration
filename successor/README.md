# Native engine

This workspace contains the current engine: dependency-free authoritative world
code in `world/` and the wgpu/winit application in `viewer/`. Run from the repository
root; see [the developer guide](../guide_for_devs.md).

```sh
cargo run --locked --release --manifest-path successor/Cargo.toml -p microvoxel-successor
cargo test --locked --workspace --manifest-path successor/Cargo.toml
```

Native cells remain exact. Ordinary distant terrain uses connected, filtered
surface patches; edited and volumetric footprints use replacement run geometry.
LOD transitions affect only changed patches. Worker-local caches, bitmask face
collection and shared GPU arenas reduce repeated work and allocation overhead.

The selectable `ray` backend remains experimental. `--brick-study` is a separate
GPU DDA correctness/performance experiment, not a selectable gameplay backend.
Neither requires DAG storage. See the [implementation record](../agent_context/ENGINE_REDESIGN_IMPLEMENTATION.md)
for measured results and limits.

## Reproducible comparisons

`tools/benchmark_successor.py` compares against a frozen local baseline binary.
It hashes binaries and records commands, logs and captures under ignored
`output/redesign/benchmarks/`. Build the current engine first, then:

```sh
python3 tools/benchmark_successor.py --dry-run
python3 tools/benchmark_successor.py --trials 5 --mode screenshots
```

The default baseline is `output/redesign/baseline/microvoxel-game`; it is a local
snapshot, not distributed by Git. Supply `--old PATH` on another machine. The
current binary is `successor/target/release/microvoxel-successor`. Defaults use
1080p, 120 synchronized frames and no haze. Invocation wall time includes startup
and readback and must not be reported as frame time. Read the engine's frame and
terrain-pass timestamp measurements separately.

Match hardware, drivers, cameras, FOV, world settings, memory budgets and quality.
Projected cell size is a sampling target, not a silhouette-error guarantee.
Different representations can produce different images even at the same target.
Repeated paired trials are required before claiming an overall speedup.
