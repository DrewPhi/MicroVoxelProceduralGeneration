# MicroVoxel Procedural Generation

![A voxel mountain valley, grasslands and lake, captured at 1440p in the native mesh renderer](assets/cover.png)

An experimental Rust foundation for an Earth-like voxel world, built without a
game engine. Explore a procedurally generated **10 km × 10 km** landscape made of
**6.25 cm voxels**, with diverse landforms, climate-driven erosion, drainage, sediment, biome
suitability and exact native voxels and coherent distant terrain LOD.

Slope-guided ravines, coherent material patches and sparse caves/rock arches add
structure across scales. World resolution is configurable; 6.25 cm remains the
default, while finer interactive resolutions are experimental. **Esc → World**
also offers independent landscape compression: World Scale `10` fits the same
terrain into 1 km square while keeping voxel centimeters and player size fixed.

The native **wgpu + winit** engine uses cached terrain patches, shared GPU buffers
and local LOD transitions. Near the player it merges exposed faces while preserving each
voxel's color. World generation, collision and picking share the same integer grid;
untouched terrain stays procedural and only edits need persistent storage.

Walk and jump through the world as a procedural voxel character, with first-person
and third-person views. Press **Esc** to tune terrain detail, native render radius,
cache size and haze while watching frame times. A starting point for exploring terrain, biomes, lighting,
sky and future simulation.

[Developer guide](guide_for_devs.md) · [Project notes](agent_context/README.md) · [MIT license](LICENSE)

The native preview includes selectable mesh and experimental voxel ray-casting
terrain backends (`--renderer mesh|ray`, or the Esc menu), with shared procedural
world data and player collision. See [the developer guide](guide_for_devs.md) for
build instructions and reproducible renderer comparisons.
