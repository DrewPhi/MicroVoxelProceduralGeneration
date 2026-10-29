# Voxel World Template reference review

Reviewed 2026-10-28. Reference: [jbikker/WrldTmpl8](https://github.com/jbikker/WrldTmpl8),
commit [`741fd01`](https://github.com/jbikker/WrldTmpl8/tree/741fd01a7a825107a57ab17b0052f92a5e8c9e36).
This is a source inspection, not a performance comparison or an implementation.
No third-party source or assets have been copied into this project.

## Similar world model, different rendering architecture

Both projects represent colored voxel cells and compress uniform regions. Our
current application rasterizes merged exposed rectangles with separate cell-color
atlases. Their OpenCL renderer traverses voxel data directly; replacing our main
renderer with that approach would be a separate experiment, not a shader swap.

## Useful ideas and where they are implemented

| Reference | Observed technique | Possible use here |
|---|---|---|
| [`world.cpp`, `UpdateSkylights`](https://github.com/jbikker/WrldTmpl8/blob/741fd01a7a825107a57ab17b0052f92a5e8c9e36/template/world.cpp#L419) | Integrates sky lighting for the six axis-aligned normals | Cheap directional sky illumination in our existing raster renderer |
| [`trace.cl`, `TraceRay`](https://github.com/jbikker/WrldTmpl8/blob/741fd01a7a825107a57ab17b0052f92a5e8c9e36/cl/trace.cl#L98) | Hierarchical traversal skips empty groups before visiting bricks/cells | Future GPU ray-query prototype, shadows or lighting probes; not required for current rasterization |
| [`world.h`, voxel access/dirty tracking](https://github.com/jbikker/WrldTmpl8/blob/741fd01a7a825107a57ab17b0052f92a5e8c9e36/template/world.h#L231) | Tracks changed bricks, recycles empty ones and represents uniform values compactly | Reference for future native live-edit synchronization; core transactions/invalidation already exist here |
| [`world.cpp`, render/commit flow](https://github.com/jbikker/WrldTmpl8/blob/741fd01a7a825107a57ab17b0052f92a5e8c9e36/template/world.cpp#L1426) | Staging buffers, asynchronous transfers and bounded changed-brick work | Study update latency and upload scheduling when adding editing; our queues already have backpressure |
| [`kernels.cl`, `finalize`](https://github.com/jbikker/WrldTmpl8/blob/741fd01a7a825107a57ab17b0052f92a5e8c9e36/cl/kernels.cl#L144) | Reprojects frame history and constrains history colors using the current neighborhood | Optional visual antialiasing/denoising experiment; preserve authoritative cell data and assess ghosting |

## Assumptions not to adopt without measurement

The inspected [configuration](https://github.com/jbikker/WrldTmpl8/blob/741fd01a7a825107a57ab17b0052f92a5e8c9e36/template/common.h)
uses a 1024-cell extent per axis, 8-cell bricks, a default two-byte packed payload,
and a fixed brick pool. Those constants imply 1 GiB of brick payload capacity in
each mirrored host/device pool; that arithmetic is not a measured resident-memory
figure. At our voxel scale, the default extent corresponds to 64 m per axis.
It is not evidence of out-of-the-box 10 km streaming support.

Some comments describe an older eight-bit payload, while active configuration
selects sixteen bits. Read active code/configuration rather than treating comments
or promotional ray-throughput figures as measurements for our laptop.

Their repository's LICENSE is CC0-1.0; bundled dependencies/assets need their own
provenance review if actually reused. Current work is an architectural study only.

## Suggested sequence

1. Continue independent landforms, climate and biome development in our generator.
2. For the next lighting pass, prototype six-direction skylight on top of the
   current renderer; consider per-voxel contact shading without sub-voxel texture.
3. When live editing is added, compare bounded dirty-region upload approaches.
4. Evaluate temporal reconstruction and a small voxel-ray-tracing prototype only
   against identical scenes, resolution and hardware, reporting costs separately.

These are candidate improvements, not promises of speedups. Keep the current
Rust core, integer occupancy, canonical appearance and bounded streaming rules.
