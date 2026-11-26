# Interactive visual preview

From `frontier/`, launch `./run.sh` to walk in a fresh Eroded world.
The launch prints the seed. Pass `--seed INTEGER` to return to that world,
or add `--flight-walkup` to start beside the Skyhawk on a dirt runway.

The default uses the meshless rolling terrain cache with a 6 km view radius,
procedural vegetation, regional weather, volumetric clouds, a day/night
cycle, and 1× sampling. The exact NAADF radius is 128 voxels (8 m at the
default 6.25 cm voxel size). Open the terrain and vegetation LOD curves in
Settings to adjust projected detail by distance. Press **F4** for the
weather radar.

Use `--weather off` for the procedural sky, `--msaa 4` for 4× sampling, or
`--naadf off` to use the exact column cache for ordinary nearby terrain.
Explicit presets and comparison scenes retain their own launch defaults.
`--terrain-recipe regional` opens the previous Regional preview and
`--packed-terrain` selects the previous mesh renderer. Rebuild with
`cargo build --release --locked` after source changes because the launcher
prefers an existing release binary.

## Aircraft studio

Launch `./run.sh --model-preview` to inspect the Skyhawk in Frontier's aircraft
renderer. Drag to orbit or look around in a seat, right-drag to pan, and scroll
to zoom. The studio has orthographic reference views, a pilot and rear-seat
view, cutaways, part isolation, adjustable lighting and pose sliders. Set the
doors independently. Flap deployment in the studio is a rig check.

For repeatable headless captures, run
`./run.sh --model-preview --view orbit --door 1 --cutaway left --capture /tmp/skyhawk.png`.
Run `./run.sh --model-preview --help` for camera and rig parameters and
`./run.sh --model-preview --inspect` for native voxel counts and rest bounds.

The local `tools/frontier_mcp.py --aircraft-only` stdio server exposes
`inspect_aircraft_model` and `render_aircraft_preview`. The render tool returns
up to six PNG views from the current release binary. It works without an X11
window. After an edit, rebuild the release binary before capturing again.

The design uses the newer 172S high-wing silhouette, four-seat cabin and
dual-screen layout from the [Textron Skyhawk product card](https://cessna.txtav.com/-/media/cessna/files/product-cards/piston/skyhawk_product_card.pdf).
The exterior paint and flight display graphics are Frontier artwork.

Start on foot beside the voxel Skyhawk on a tested natural strip with
`./run.sh --preset seeded-world --seed 1878846491374251281 --flight-walkup`.
Click to take control. Walk up to either cabin door and press **V** to board.
**Shift** increases throttle and **Ctrl** decreases it. Releasing either
holds the power setting. **Q/E** operate the rudder and steer on the ground.
**A/D** roll left/right. **W/S** pitch down/up. **Z/X** extend/retract the
flaps in 10° steps. **Page Up/Down** adjust nose-up/down trim. **Space**
applies wheel brakes. Tap pitch, roll, or rudder for a small deflection, or
hold for roughly one second to reach full travel. The surfaces return to
neutral when released. Holding **Space** builds brake pressure gradually.

For departure in Frontier, use full power, steer down the strip, and ease
**S** through rotation. The natural-strip test uses 10° flaps and starts
raising the nose at roughly 45 kt before climbing out. For a landing, find a
long dry stretch, reduce power, level the wings, approach around 60–70 kt,
then ease the nose up
to arrest the descent. Idle the throttle and brake after touchdown. **V**
steps out when stopped and leaves the plane where you parked it. Natural
strip selection checks a 360 m dry corridor near spawn, with a 288 m
search if a longer strip is unavailable.

**C** switches pilot/chase view. Hold **right mouse** to look around. **O**
operates the doors when stopped. **H** engages heading/altitude hold in flight.
**[ / ]** tune COM. The pilot's six round instruments show airspeed,
attitude, altitude, turn rate, heading and vertical speed. The upper right
screen is the north-up terrain radar, with contours, surface water, ownship
and the launch point. The lower screen shows the direction the wind comes
from, speed in knots and vertical motion in ft/min. **WX N/A** marks the
weather placeholder.

Press **Y**, or select **Show airflow** in the Esc menu. Cyan voxel tracers
illustrate the approximate flow around the airframe. Green lift and amber
drag arrows show the section forces calculated by the flight model. Lift
arrows turn red as a section stalls. The overlay works in pilot and chase
views. **Flight sound** in the Esc menu mutes or restores the synthesized
propeller, wind, suspension and wheel noise, stall horn, buffet and impact
cues. Changes in the loads at the pilot's seat add a cabin thump and panel
rattle. Linux playback uses PulseAudio or PipeWire's PulseAudio service.
The cockpit and chase cameras follow rapid aircraft motion with a short
lag. Press **J** or select **Camera inertia** in the Esc menu to switch
between the damped and rigid views.

### Airframe damage and wreckage

The rolling-terrain flight mode accumulates damage from local airflow,
maneuvers, landing loads and impacts. The wings flex under load. Creaks
build near yield and a sharp sound marks a separation. Detached sections
and loose skin voxels inherit the motion at the tear and continue to fall
outside the camera view. They can strike the terrain, split further and
settle. Wreckage and voxel scars persist when you leave and return during
the same session.

To test a terrain strike on a flat, editable fixture, start the following
flight and click the game window. The initial descent is resolved by the
regular flight controls, aerodynamics and terrain contacts.

```sh
./run.sh --scene adaptive --terrain-recipe legacy --amplitude 0 \
  --flight-view chase --eye 0,32,0 --target 0,12,24 --atmosphere on
```

The solver uses calibrated structural joints and boundary contact samples
from the voxel model. The 6.25 cm aircraft voxels are assigned to structural
assemblies, with load-bearing members and skin sharing the load at each
joint. Detached assemblies use rigid-body dynamics and material-dependent
terrain edits. The report includes fragment positions, settled state, and
the number of removed terrain cells.

Use `--flight-parked --flight-view pilot` to start seated or
`--flight-view chase` to start airborne. Replace the seed with `random` for
a different world.

## Flight physics

The Skyhawk integrates three translational and three rotational degrees of
freedom at a fixed 120 Hz. Its 28 wing, tail and fin sections share the
render model's geometry and hinge travels. Each section uses local airflow
from translation, rotation, wind and propwash to apply a force and a torque
about the center of gravity. The model also includes propeller and engine
reaction, altitude-dependent air density, sprung wheels, steering and brakes.
Terrain contact samples the edited voxel world. The model parameters allow
fuel and payload mass and location to affect the center of gravity and inertia.
The playable aircraft starts with a fixed loading.

Flight-model references include the [JSBSim force and moment formulation](https://jsbsim-team.github.io/jsbsim-reference-manual/user/concepts/forces-and-moments/),
the [Cessna 172S operating handbook](https://edwardsfss.com/wp-content/uploads/2024/01/Cessna-172-Pilot-Operating-Handbook.pdf),
and [NASA general-aviation spin flight tests](https://ntrs.nasa.gov/citations/19870007382).
The clean, idle-power steady lift peak scaled to 2,550 lb gives 52.9 kt at
sea level. The longitudinal model balances lift, thrust and drag at
125.5 kt in full-power sea-level trim. Separated flow, wake effects, and
high-angle control authority use engineering approximations. The spin test
demonstrates in-game autorotation. Spin rates and trajectories still need
aircraft-specific flight-test validation.

Use this launch for extra altitude during stall experiments.

```sh
./run.sh --preset seeded-world --seed 1878846491374251281 --flight-view chase --flight-altitude 1600
```

Reduce power, use back pressure to increase angle of attack and add rudder
as the stall develops. In the deterministic test, releasing the rudder while
holding back pressure sustains about 1.9 rotations over seven seconds in
either direction. Neutralizing the ailerons, opposing the rotation and
releasing back pressure restores attached flow, after which the test returns
to wings-level flight. All of these inputs use the normal flight update.

Add `--flight-wind 3.5,0,0` for a 3.5 m/s wind along the world's +X axis.
Wind moves air relative to each aerodynamic section. The runtime uses a
constant wind setting and the section sampler can also accept spatially
varying wind. The graphics cloud settings control visual clouds.

```sh
cargo test --release --locked -p frontier-renderer --lib aircraft::tests -- --nocapture
cargo test --release --locked -p frontier-renderer --lib flight_tick_budget -- --ignored --nocapture
```

The rolling renderer shades static water from cached depth and bed color.
NAADF adds precise bed refraction and nearby reflection, then fades these
details at its coverage edge. Coarse samples can miss narrow rivers, and
edits remain visible within the local exact window. Live water, packed
quality probes, a requested view distance beyond 9 km, and a requested
NAADF radius above 512 voxels automatically select the packed renderer.

Press **Esc** to open or close settings. Sun elevation/azimuth, exposure, haze
density/height falloff, cloud coverage/speed and overcast update live. **Clear air
· disable atmosphere** removes atmospheric compositing for an unobscured terrain
check. Regional weather drives cloud density, precipitation, and local wind
from the same simulation. [Weather controls](WEATHER.md) lists the launch
options and radar pages.

Image quality and loading controls take effect with **Apply quality & loading**.
Applying rebuilds the streaming configuration while retaining the current seed
and camera pose; allow terrain to refine after a change. The overlay shows frame
time, pending work, refinement readiness and memory counters.

| Control | Effect |
|---|---|
| Terrain error (px) | Packed terrain only. Lower values request finer geometry and may reduce FPS. |
| View distance (m) | Changes how far terrain coverage is requested. |
| Render scale | Sets internal scene resolution; the settings overlay stays at output resolution. |
| MSAA | Selects off or 4× edge coverage sampling. |
| Filter distant voxel detail | Filters distant appearance while keeping canonical voxel identity. |
| Workers | Sets CPU preparation concurrency within the available worker limit. |
| Refinements / batch | Sets refinement selections per coherent loading transaction. |
| Cache budget (MiB) | Sets the in-memory terrain cache budget. |
| Upload / frame (MiB) | Sets the per-frame upload budget. |

Click the view to capture the mouse. Use WASD to move, the mouse to look,
Space/Ctrl to move vertically and Shift to accelerate. This is a free-fly
preview without player collision. Press **F12** to save a PNG capture.

The landforms are the existing Regional composition. The new painting derives
grass, soil and exposed rock from landform slope, elevation and continuous
moisture; it does not generate independent brown/gray lowland patches. Each
native voxel keeps one material on all its faces apart from lighting. Legacy
remains available with `--terrain-recipe legacy`; see the
[painting checkpoint](REGIONAL_PAINTING.md) for source identity and tests.

This checkpoint focuses on visual review; optimization is postponed. The workspace passes 276 tests (7 deliberately ignored), including shader validation.
The fixed Legacy player-view check differs from the previous baseline in only
two pixels by one 8-bit level; geometry and source remain unchanged.
GPU menu smoke checks exercised Esc, Resume and changing MSAA through Apply.
A separate GPU capture verified clear air at 75% render scale with MSAA off.
This is not a sustained performance or production-quality certification. Water, drainage simulation and weather physics are not implemented.


Final preview captures (generated locally, ignored by Git):

- `output/visual-preview/player-final.png`: seed 7, 1280×720, 4×MSAA,
  3px requested terrain error, 4km clip, eye 1.7m above terrain.
- `output/visual-preview/aerial-final.png`: same source, aerial camera, 6km clip.

These show the current Regional landforms with conservative meadow cover.
Bare rock requires steep slopes; altitude alone no longer paints a hill gray.
