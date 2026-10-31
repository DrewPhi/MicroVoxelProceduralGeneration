# MicroVoxel Procedural Generation

![A voxel mountain valley, grasslands and lake, captured at 1440p in the native mesh renderer](assets/cover.png)

An experimental Rust foundation for an Earth-like voxel world, built without a
game engine. The current default explores a geographic terrain atlas with
**6.25 cm native voxels**, connected drainage, climate-informed materials and
coherent distant terrain LOD. The launcher starts in a **2 km × 2 km valley**
with a cached local erosion and deposition prototype.

Voxel size and landscape compression are independent settings in **Esc → World**.
World Scale `10` compresses source terrain distances tenfold while keeping voxel
centimeters and player size fixed. The full atlas surrounds the inspection valley;
the new erosion refinement is currently bounded to that valley. The earlier
procedural terrain generator and real-data import remain available.

[Latest valley screenshots and capture settings](assets/valley/README.md) show
current terrain honestly: curved rivers and local detail have improved, while
river terraces, distant facets and broad material regions still need work.

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
