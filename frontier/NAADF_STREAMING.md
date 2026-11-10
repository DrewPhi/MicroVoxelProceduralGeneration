# Moving NAADF window

NAADF now follows the player when enabled. The first cube is still a cold CPU
build. After it publishes, movement shifts the cube in small, integer-meter
steps and reuses the overlapping data. The existing renderer covers the rest
of the world. `--naadf-follow off` preserves the fixed-region benchmark mode.
The **Follow player** setting is also available in the Esc menu.

From the repository root:

```sh
./frontier/run.sh --preset alpine --naadf-radius 32 --msaa 1
```

A 256 m half-extent can use the previously validated manual estimate ceiling:

```sh
./frontier/run.sh --preset alpine --naadf-radius 256 --naadf-budget-mib 10240 --msaa 1 --width 1920 --height 1080
```

## How the window moves

1. The camera moves beyond half a step from the current center. A step is
   roughly one-eighth of the radius, bounded to small integer-meter shifts.
   Most of the previous cube therefore remains useful while the new band builds.
2. Up to four workers generate only the entering strips from the canonical
   world. Retained height summaries can skip resampling columns whose new
   vertical slab is entirely air or uniform deep material. Plant overlays and
   edits must be accounted for before using this shortcut.
3. The render thread uploads new block/leaf data into unused GPU arena slots.
   Root words go to temporary strip textures. The old root mapping, bounds and
   ray coverage remain active. Raster and plant coverage is restored only in
   the outgoing area; the intersection retains its ownership certificate.
   Only vegetation needed to cover that outgoing area can hold up a normal
   shift, so unrelated detail jobs do not block publication.
4. Once the upload and restoration are ready, a queue-ordered copy publishes
   the new root words and toroidal offset together. The CPU worker retires old
   chunks and releases a shared leaf slot only after its last root reference
   is gone. Subsequent uploads reuse those slots.

The GPU reads the same texture and arenas throughout normal travel. CPU arena
bookkeeping, remapping, world generation and reclamation stay off the render
thread. Prior proven-empty root distances remain safe in overlapping world
coordinates; newly generated distances are bounded to their source strip.
The renderer clamps traversal to the currently published cube. Source changes
invalidate the old generation, and low-memory cancellation preserves the last
complete window. If travel outruns the available data, the exterior renderer
remains responsible for coverage.

This is a greenfield extension of the existing NAADF representation, using the
incremental overlap/wraparound principle from
[GPU geometry clipmaps](https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry).
The [NAADF reference](https://github.com/cg-tuwien/NAADF) informed the hierarchy
and exact rays. No external code was copied. The streamed region uses one native
resolution; it does not introduce additional voxel LODs inside the cube.

## Scope and costs

- The initial cube still has the full cold-build cost. A large jump can require
  a fresh destination cube. Previously evicted strips are regenerated on
  reentry; overlapping strips are reused in place.
- Following reserves extra block/leaf arena capacity and keeps CPU root and
  reference-count metadata. `gpu_payload_bytes` reports allocated root/arena
  capacity; `initial_snapshot_payload_bytes` reports the original logical
  snapshot. Temporary staging textures and other renderer resources are
  separate. An arena exhaustion is surfaced rather than overwriting live data.
- The appearance/lighting seam between the two representations is still
  possible, especially at small radii. Coverage continuity does not imply
  invisible color transitions, nor does one walking test establish aircraft,
  arbitrary teleports, edited-world throughput or all-terrain guarantees.

## Reproduction

`tools/record_walk.py` drives actual grounded walking, stopping and turning;
recording overhead makes it unsuitable as an isolated performance benchmark.
The finite sweep runner can now measure a grounded real-time route separately:

```sh
cd frontier
python3 tools/naadf_radius_sweep.py --radii 256 --naadf-budget-mib 10240 --route walk-stop-resume --route-speed 2.5 --seconds 120 --output output/my-naadf-walk --run
```

The report records completed shifts, last strip build and sampled-column count,
minimum camera margin and number of active frames outside the exact cube. A
camera-margin result is a coverage measurement, not a proof of identical
exterior LOD or shading at every pixel.

## Walking results

Measured on the Radeon 760M at 1920×1080, MSAA 1, with Alpine seed 7 and
6.25 cm voxels. The 256 m radius test used a 120-second grounded route with
a stop and resume, a requested walking speed of 2.5 m/s, and the normal
6 km view distance.

- **256 m radius:** 260.17 m horizontal displacement, eight published
  shifts (256 m forward), and zero outside frames out of 5,090 tracked.
  The player crossed the original cube boundary and stayed at least
  205.75 m inside the nearest face of the moving cube. Median/p95/p99
  frame times were 23.59/34.88/39.20 ms. The last strip build took 4.62 s
  off the render thread. The cold cube took 162.59 s to build. Allocated
  NAADF root/arena payload was 2.41 GiB; peak whole-process RSS was 6.47 GiB.
- **8 m radius, recorded walk/stop/turn:** 16 published shifts, zero
  outside frames out of 2,144 tracked, and a minimum 5.86 m margin.
  The [contact sheet](reference/gallery/naadf-streaming/walk-contact.png)
  shows the near voxel coverage and the still visible appearance boundary.
- **Before scoping the retirement check:** the same 256 m route managed
  only three shifts and reached a 91.90 m margin. Distant plant jobs
  could delay the handoff even after the outgoing area had fallback coverage.

The [measurements](reference/gallery/naadf-streaming/measurements.json)
include the route, hardware, memory and renderer counters. See the
[final 256 m frame](reference/gallery/naadf-streaming/walk256-end.png).
Workspace tests passed (553); the GPU canonical oracle passed separately.
With NAADF disabled, the primary reference PNG, linear color and depth
matched the baseline byte for byte.
