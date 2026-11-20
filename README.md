# Frontier World Generation Experiment

Astra's starting point is the current Frontier engine at **6.25 cm per
voxel**, with the Watershed procedural world, adaptive distant terrain, and
exact NAADF terrain near the player. The experiment brief lives in
[frontier/AGENTS.md](frontier/AGENTS.md).

## Fly the Highlands

From `frontier/`, build once and launch a persistent world.

```sh
cargo build --release --locked --bin frontier
./run.sh --preset flight --seed 19 --voxel-size 0.0625 \
  --world-save output/worlds/highlands-19.save
```

Click the window to take the controls. **W/S** increase or decrease airspeed,
**A/D** bank into a turn, and **Down/Up** pitch the nose up or down.
**C** switches between the pilot and exterior chase views. **V** switches
between the aircraft and free camera. **F** switches to walking from the
aircraft. The fuselage, wingtips and wheels check swept paths against the
edited voxels. The chase camera also checks the space behind the aircraft.

To land, bring the wings level, slow below 48 m/s and descend at less than
3.5 m/s onto a firm, nearly level strip. On the ground, **A/D** steer and
**S** brakes. Hold **W** to accelerate, then **Down** to lift the nose once
you reach 29 m/s. Both wheels need support. You can build a runway from
placed voxels or enter the aircraft with **V** while standing on level ground.

The Highlands recipe combines ridged summits, branching foothills, watershed
drainage and snow. The preset renders a 6 km active terrain view with a
world-aligned landscape horizon fading out to 35 km. The horizon samples the
procedural source. A sparse mesh carries saved terrain edits beyond the resident native
volume. Set `--terrain-recipe watershed` after the preset for the earlier terrain.

On the Radeon 760M at 1920×1080, MSAA 1, the seed 19 Highlands flyover
measured **13.65 / 16.20 / 17.95 ms** frame intervals at p50/p95/p99. This
10-second, 80 m/s scripted pass used a 32 m native radius, disabled disk cache
and uncapped presentation. Coarse coverage was ready when timing started.
Detail refinement was still in progress at capture.

With conservative occupied-root bounds used for the native draw, the same
flyover reached **9.07 / 11.41 / 12.84 ms** across 1,071 measured frames.
An airborne turn-return run on seed 8123 with 12.5 cm voxels measured
**7.15 / 10.46 / 12.44 ms**. At ground level the separate stationary
Watershed seed 997 test (1.5 km view, 32 m native radius) measured
**20.29 / 23.86 / 25.05 ms**. Enabling the existing 4-root height tiles
and traversal gate gave **17.83 / 20.27 / 20.96 ms** on that ground test.
The current Watershed and Highlands paths also share conservative empty-ray
prefixes across 8×8 screen tiles. This brought the ground measurement to
**14.56 / 16.90 / 17.50 ms**. Set `FRONTIER_NAADF_PLAYER_BEAMS=0` to run
the control. Native visibility was checked against the independent CPU ray
oracle during partial residency, shell growth and toroidal shifts.

With GPU stage timing enabled, reusing identical beam results reduced the
number of beam passes from 611 to 7. This removed redundant work. The
matched stationary p95 frame time stayed at 21.1 ms in that measurement.
A 9-second, 3 m/s grounded turn-and-return on the same seed measured
**21.37 / 25.76 / 29.89 ms**. Sustained 60 FPS on foot remains open.

An empty native volume now keeps its coverage stable while flying. In the
seed 19 flyover, completed tile builds fell from 1,334 to 145 and visible
terrain triangles increased from 7,168 to 44,224. Frame p50/p95/p99 were
**9.01 / 11.24 / 13.69 ms**. At the surveyed flight spawn on seed 40961,
a separate 10-second, 80 m/s pass measured **6.77 / 8.52 / 9.62 ms**.
These flight measurements include ongoing refinement.

Highlands now samples the same climate, grove density and rock coloration
at shared mesh vertices and on the distant horizon. On the seed 19 flight
route, first terrain coverage arrived in **315 ms** (previously 2,518 ms),
resident face-profile storage fell from **13.2 MiB to 3.1 MiB**, and frame
p50/p95/p99 measured **7.44 / 9.90 / 11.61 ms**. Seed 40961 measured
**6.17 / 8.35 / 9.10 ms** on its surveyed approach. Close ridgeline silhouettes
still show coarse geometry during refinement.

An 8 m shading grid adds terrain normals and color to the nearest 2 km square
of the flight view. Shared samples stay fixed as the aircraft crosses grid
boundaries. The GPU texture uses 516 KiB. On the same seed 19 route, two
passes with this field measured **8.05 / 10.42 / 11.90 ms** and
**8.79 / 11.27 / 12.65 ms**. The control (`FRONTIER_FLIGHT_DETAIL=0`)
measured **8.33 / 10.86 / 12.51 ms**.

With 32×32 macro patches, the seed 19 flyover drew 176,512 visible triangles
at capture and measured **8.29 / 10.73 / 11.71 ms**. The 16×16 control drew
55,896 and measured **9.00 / 11.65 / 13.87 ms**. First coverage took 658 ms
and 449 ms respectively. On seed 40961 the two p95 measurements were 7.73
and 8.60 ms. Both routes still had pending refinements at capture. Set
`FRONTIER_HIGHLANDS_PATCH_ROWS=16` for the coarser control.

Cloud banks use the world seed and wind. Terrain beneath a cloud deck and
mountains that rise through it share the same atmospheric pass. A later
1080p flyover measured **7.10 / 8.78 / 9.98 ms** on seed 19 and
**4.63 / 6.25 / 7.78 ms** on seed 40961. Both captures include ongoing
refinement. Proven flat ocean surfaces now report zero vertical error.

Flying above an edited world releases the native volume's terrain ownership
once it contains only unmodified air. A regional flyover 160 m above a saved
excavation and deck measured **4.79 / 6.05 / 6.52 ms**. The same route with
an empty save measured **4.67 / 6.02 / 6.35 ms**. Both ended with 16,712
visible triangles and no native terrain ownership at altitude.

Highlands coastlines intersect the sampled seabed inside each terrain triangle.
Nearby shallows show bed color and the offshore surface reflects animated,
filtered swells. A seed 40961 coastal pass measured **4.13 / 5.63 / 6.55 ms**
at 1080p MSAA 1. A separate 90-second seed 19 route flew 9 km at 100 m/s
with complete coarse coverage in all 10,587 measured frames. It measured
**7.95 / 11.94 / 29.82 ms** and peaked at 537 MiB process RSS. No measured
frame met the five-pixel refinement target. A concurrent build makes the
long route's tail timings less comparable to the isolated 10-second passes.

The native renderer uses a shorter shader when the published volume is proven
dry and the camera is above water. On a flat, dry regional walking route at
1080p, it measured **6.36 / 8.66 / 9.07 ms**. Two runs with
`FRONTIER_NAADF_DRY_PIPELINE=0` measured **8.12 / 10.07 / 10.52 ms** and
**8.24 / 10.40 / 10.82 ms**. Watershed seed 997 contained water in the exact
volume and used the general shader, with walking p95 between 20 and 22 ms.

When the entire exact window and a 128 m approach margin are above the
certified terrain, vegetation and saved edits, flight can park an empty
window. On the 9 km mountain route this reduced strip publications from
1,612 to 727. The parked run measured **6.28 / 10.41 / 16.39 ms** and the
control (`FRONTIER_FLIGHT_PARK_SKY=0`) **6.08 / 8.89 / 29.70 ms**. Both
kept coarse coverage throughout. A separate descent from 300 m to 5.5 m
above sea level resumed native streaming before reaching a saved 3 m deck.
The final native window covered the deck and the save remained byte-identical.

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
Use `--world-save PATH` to edit and save without advancing water simulation.
The adaptive renderer reconstructs edited columns and their exposed walls
outside the native region. This preserves excavations, suspended blocks and
openings in the active terrain view. Large edit sets can exceed the distant
mesh budget, which reduces the area reconstructed around the camera. The
35 km horizon still samples the unedited procedural source.

Directional shadow maps use the same column mask as the visible surface.
The edit-distance check measures seven depth views and compares a sunlit
excavation with shadows enabled and disabled. On the Radeon, both readings
at the floor center were **0.669 / 0.683 / 0.666** in linear RGB.

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
python3 tools/player_edit_check.py --terrain-only --output output/edit-static-smoke
python3 tools/edit_distance_check.py --checkpoint output/edit-static-smoke/world.save \
  --output output/edit-distance-smoke
```

Generated files go under ignored `frontier/output/`. The current engine
and historical development context remain available in the
[Frontier engine branch](https://github.com/DrewPhi/MicroVoxelProceduralGeneration/tree/frontier-voxel-engine).
