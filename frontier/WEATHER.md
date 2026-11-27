# Frontier world weather

The weather simulation is shared by the scene, the flight model, and
instruments. It runs with a walking player, the free camera, or an aircraft.
The default Eroded launch uses regional weather and astronomical lighting.
Use `--weather off` for the procedural sky. Explicit world presets retain
their individual defaults, and accept `--weather regional`.

```sh
./run.sh --preset seeded-world --seed random --weather dynamic
./run.sh --preset alpine --seed 7 --weather rain --weather-age 360
./run.sh --preset alpine --seed 7 --weather convective --weather-age 600 \
  --start-xz 3700,870 --weather-center -41.060160744,-250 \
  --flight-view pilot --flight-altitude 2400 --flight-heading 090 --airborne-radar
```

`--weather clear|rain|convective` selects controlled atmospheric conditions
for comparisons. The convective case starts with a warm, moist perturbation.
It computes cloud and rain from that state. `--weather dynamic` evolves the
regional wind, stability, and humidity on a seeded day/night and synoptic
cycle. `--weather-day` and `--weather-hour` choose the initial weather clock.
`--weather-age SECONDS` runs the solver before spawn, including the
instrument's broadcast schedule. Weather follows elapsed time while the
world is open. For Alpine seed 7 at hour 13, starting on day 8, 18, or 172
and advancing by 600 seconds produces clear air, layered rain, or a
convective storm in the dynamic mode.
Use `--weather-center X,Z` to anchor the simulated volume separately from
the player spawn coordinate. The cloud and rain volume remains in place when
you fly out or return.

## Regional weather

```sh
./run.sh --preset seeded-world --seed random --weather regional --weather-day 183 \
  --flight-view pilot --flight-altitude 1200 --airborne-radar
```

Use F4 for local cloud tops, live and delayed precipitation, a forward vertical
slice, and zoomable regional pressure, upper wind, and rain maps. The jet
arrows sample the wind at 10 km MSL. The time controls display the measured
simulation speed while walking and flight controls continue in real time.

For a repeatable Alpine flight into the weather, use:

```sh
./run.sh --preset alpine --seed 9 --weather regional --weather-day 183 \
  --weather-hour 16.2 --weather-age 1080 --start-xz 500,2300 \
  --flight-view pilot --flight-altitude 1250 --flight-heading 010 --airborne-radar
```

`--weather-latitude` sets the planetary latitude at the center of the domain.
It defaults to 43° N and supports absolute latitudes from 25° to 65°.
`--weather-center X,Z` fixes the initial region and radar site while the local
weather grid follows the player. Optional `--weather-save PATH` saves the
weather on exit and periodically during play. Reuse the same seed, terrain
recipe, regional center, latitude, and start time to resume. For a shorter
visual solar cycle, use `--day-length`. In regional weather the thermal forcing
still uses the simulated astronomical day.

The regional grid has 128 × 12 × 96 cells at 24 km × 1.5 km × 24 km spacing.
It advances every 20 simulated seconds. The hydrostatic approximation
separates column mass waves from internal thermal pressure gradients.
Latitude-dependent Coriolis turns the horizontal flow. The seed and seasonal
temperature contrast initialize a broad jet, warm and cold air masses, and
pressure disturbances. Heating, evaporation, moisture transport, and
condensation evolve the atmosphere. The outer western boundary supplies
large-scale warm and cold air masses. It remains a single finite weather
region with simplified physics.

The local 320 m weather grid receives time-interpolated regional conditions
and samples continuous terrain climate, water, and biome roughness. Moving
the grid preserves overlapping cells and fills newly exposed strips. Local
conditions continue to drive independently sampled wing-section gusts. The
plane also uses sampled temperature, humidity, and pressure to adjust air
density. Clouds use the same local condensate with a coarser regional cloud
and rain volume for distant weather. The distant textures add 576 KiB of
logical GPU texel storage and skip ray marching when the regional summaries
indicate clear air near the view.

The regional mode is a warm-rain prototype. Validation covers initialized
jets, six- and 24-hour regional replays, precipitation, weather-grid recentering,
and a checkpoint round trip. Planet-wide weather streaming and mixed-phase
microphysics remain future work.

The final regional solver passed a 24-hour replay with a 37.2 m/s upper jet,
rain in 1,067 of 12,288 coarse columns, and zero pressure clamps. A resumed
game reproduced the saved rain core and cloud heights.

The release build used Vulkan on the Radeon 760M with normal rolling terrain,
vegetation enabled, and an 8 m NAADF radius. These are individual runs.

| Run | Samples | Full GPU p95 | Frame p95 |
|---|---:|---:|---:|
| 1080p stationary cockpit in rain | 836 frames, 625 GPU | 9.36 ms | 12.23 ms |
| 1440 × 810 live flight, F4 open, weather at 16× | 1,016 frames, 1,015 GPU | 12.58 ms | 17.96 ms |

The 1080p run had a peak simulated rain rate of 16.3 mm/h. A separate 1080p
run during the long CPU verification measured 22.78 ms frame p95. These runs
measure the complete renderer and do not isolate the weather pass. To
reproduce the stationary rain timing, use the Alpine flight command above,
replace the weather age with 1430, and add the following arguments.

```sh
--capture-far 6000 --width 1920 --height 1080 --msaa 1 --uncapped \
  --warmup 45 --seconds 9 --report output/regional-weather/1080p-storm.json
```

## Watching weather and the day/night cycle

```sh
./run.sh --preset seeded-world --seed random --weather dynamic \
  --weather-speed 32 --day-length 1200
```

Press **F4**, or select **Weather radar**, to open the movable, resizable
map beside the world view. Its gold marker and cone show the camera's
position and direction. **Live clouds** shows condensed cloud water and
cloud-top height. **Live rain** shows peak precipitation echoes from those
same weather snapshots. **Datalink radar** shows the delayed ground-station
product and its age. The map works on foot, in free view, and in the aircraft.

Use **Pause**, the speed presets, or the **Speed** slider in the radar or in
**Esc → World time**. The clock advances the sky and weather with fixed
solver steps. Higher requested speeds are limited by solver throughput.
Aircraft and walking controls use real time. A shorter **Day length** lets
you see sunrise, sunset, and moonrise sooner and also shortens the solar
heating cycle in the weather. Regional fronts use elapsed simulation time.

Weather launches enable the day/night cycle. Use `--day-night off` for fixed
lighting or `--day-night on` to enable the cycle without the weather solver.
The cratered moon is raycast against a procedural 56³ voxel volume, with
phases illuminated by the moving sun. Its apparent size is enlarged for
on-screen visibility.

## Pilot instruments

**N** cycles the left display and **M** cycles the right. Either display can
show Weather. Within that page, **K** cycles the products. **DATALINK** is a
north-up precipitation mosaic from a simulated
ground station, with the product's age in minutes and hatched no-coverage
areas. It uses the latest delivered station scan. **WIND** shows a north-up
wind slice with brightness from calm to 60 kt and arrows indicating the
direction of flow. The altitude is in feet MSL.

With `--airborne-radar`, **WX SWEEP** and **PROFILE** are available as well.
WX uses the antenna's level horizon, finite beam width, reflectivity,
terrain masking, and attenuation. PROFILE makes a vertical sweep through a
selected bearing with an MSL altitude scale. Gray, brown, and shaded
returns indicate different observation limits. The wind page samples the
resolved wind and the other pages show precipitation.

| Key | Function |
|---|---|
| K | Cycle weather products |
| Comma / period | Decrease / increase range |
| Semicolon / apostrophe | Lower / raise the wind layer in 500 ft steps, change WX tilt in 1° steps, or change profile bearing in 3° steps |

Datalink ages and scan limits follow the distinction made in the [Cessna NAV III guide](https://static.garmin.com/pumac/190-02823-00_a.pdf). The optional antenna uses the operating concepts in the [Garmin airborne radar guide](https://support.garmin.com/sv-SE/?faq=CsJa044j8UA90xqGfStBz9).

## Engine service and limits

`frontier_runtime::weather::Simulation` accepts a terrain-height source and
publishes immutable `Snapshot` values. Games can sample wind, temperature,
vapor, cloud water, rainwater, and turbulence at any world point. The runtime
also exposes surface rain rate and weather observation products.

The resolved region is 48 × 40 × 48 cells, covering roughly 15.4 × 15.4 km
horizontally and 10 km vertically, with coarse regional conditions outside.
The solver advances advection, buoyancy, pressure correction, condensation,
cloud evaporation, warm-rain formation, sedimentation, and evaporative
cooling in two-second steps. The live worker publishes adjacent snapshots.
Flight samples interpolated wind and smaller coherent gusts at its
aerodynamic sections. The cloud pass interpolates the same condensate,
uses its wet bounds to skip clear air, and stops at terrain depth.
World-space rendering detail shapes the edges within the simulated field.
The live weather pass uses quarter-resolution scattering and a smaller
sample budget. Headless captures use half-resolution scattering and more
lighting samples.

Stratiform and convective weather are the validated cases in this
implementation. Mixed-phase ice and hail, dynamically streamed storm
regions, persistent save-state weather, and quantitative validation of
supercells or outflow-triggered secondary cells require further work.
The solver is a game-scale warm-rain model. Useful references include
[UCAR's idealized WRF cases](https://www2.mmm.ucar.edu/wrf/site/users_guide/idealized.html)
and [NOAA's thunderstorm processes](https://www.nssl.noaa.gov/education/svrwx101/thunderstorms/faq/).

## Verification

```sh
cargo test --release --locked -p frontier-runtime weather:: -- --include-ignored --nocapture
cargo test --locked -p frontier-renderer --lib
```

Headless captures include weather state in `.capture.json` and finite window
runs include it in their `--report` JSON. The report records cloud heights,
peak rain, wind and turbulence at the observer, and the age of the datalink
product.

The live Alpine seed-7 flight used a Radeon 760M with Vulkan/RADV, a
1920 × 1080 window, 6 km far distance, and vegetation and NAADF disabled.
Clear and convective runs shared the same spawn and initial heading. The
flight model advanced naturally in each run. Timings are from one
20-second run per setting after a 35-frame warmup.

| Weather and render scale | GPU samples | GPU p50 / p95 / p99 (ms) | Frame p95 (ms) |
|---|---:|---:|---:|
| Clear, 1.00 | 806 | 15.70 / 17.59 / 18.11 | 47.39 |
| Convective, 1.00 | 791 | 16.52 / 18.68 / 19.35 | 42.99 |
| Convective, 0.80 | 1111 | 11.36 / 13.16 / 13.63 | 34.27 |

The measured GPU command span includes the scene, atmosphere, and cockpit.
Frame timings also include CPU and presentation work. The tested cases
missed the 16.67 ms frame target at p95. To reproduce the storm test,
append the following options to the convective flight command above, then
click the viewport to engage flight controls. Repeat with `--weather clear`
or `--render-scale 0.80` for the other cases.

```sh
--capture-far 6000 --width 1920 --height 1080 --warmup 35 --uncapped \
  --vegetation off --naadf off --seconds 20 --report output/weather-bench.json
```
