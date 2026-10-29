# Climate and weather direction

Status: the static generation baseline is implemented (2026-10-28); see
[TERRAIN_CLIMATE_RESEARCH.md](TERRAIN_CLIMATE_RESEARCH.md). It provides regional
climate estimates, landforms/geology, climate-conditioned erosion, environment
queries, presets and diagnostic maps. Live weather and physical response below
remain proposed. No weather solver, cloud renderer, ocean, vehicle physics or new
material properties exist. Future solver sizes and budgets are experiments.

## Intended outcome

A shared environmental state should explain climate-dependent landscapes, moving
clouds, rainfall, winds, future seed dispersal and future vehicle/radar behavior.
The first objective is a coherent, inexpensive game simulation. Naturally
organized severe storms are a later research/validation goal, not a guaranteed
consequence of adding moisture and a Coriolis term.

## Three related time scales

1. **Climate baseline:** latitude/season, regional temperature, precipitation,
   wind exposure, geology and drainage statistics used by world generation.
   Begin with coarse parameterized estimates and seasonal sampling; do not run
   centuries of high-resolution weather before generating a playable region.
2. **Live weather:** evolving wind, temperature, water vapor, cloud condensate and
   precipitation around that baseline. Clouds and rainfall share state rather
   than independent visual timers.
3. **Surface response:** soil wetness, snow/water reservoirs and eventually ecology
   respond over their own timescales. One shower does not reclassify a forest or
   regenerate terrain. Long-lived terrain changes use explicit world transactions.

Generation must remain seed/version deterministic. Weather is separately saved
state with a simulation clock; it must not silently alter the procedural baseline.
Terrain edits therefore remain compatible with a stable generator definition.

## Planetary forcing without a simulated planet mesh

The game grid can be treated as a local frame rotating with a fictional planet.
Keep terrain cell coordinates fixed. Parameters provide latitude, rotation rate,
solar direction, seasonal tilt and a day/year clock. Differential solar heating,
surface heat exchange and moisture supply drive motion; Coriolis redirects it
and must not act as an energy source. In local east/north components, use a
latitude-dependent parameter f = 2 * rotation_rate * sin(latitude).

Start with constant latitude (an f-plane), allowing the rotation rate to be zero
in tests. A larger domain may need varying latitude/beta-plane behavior. A small
visual moon can follow an inexpensive prescribed orbit; a later tidal boundary
model can use lunar/solar phase. Neither an N-body simulation nor physically
rotating all terrain voxels is necessary.

Use one documented simulation-time convention. Advancing the sun faster without
advancing thermal/dynamic processes consistently changes the model. Time
acceleration needs a solver budget and validation; begin with real-time weather
and explicit headless fast-forward experiments.

## Spatial hierarchy and tentative budgets

A 10 km terrain region cannot contain the circulation and ocean environment of a
realistic full hurricane. Weather can extend beyond the playable voxel region:
store coarse environmental metadata there, not native terrain volumes.

| Domain | Candidate grid | Coverage | Ten f32 channels, double-buffered |
|---|---|---|---:|
| First local prototype | 64 x 64 x 8, 1 km horizontal spacing | 64 km square | 2.5 MiB |
| Later regional driver | 128 x 128 x 4, 8 km spacing | 1024 km square | 5 MiB |
| Later local weather | 128 x 128 x 16, 500 m spacing | 64 km square | 20 MiB |

These estimates exclude terrain summaries, staggered velocity storage, pressure
solver scratch space, boundary buffers, precipitation reservoirs and GPU rendering.
An initial target is under 96 MiB total weather-owned storage and roughly 1–3 ms
amortized simulation work per rendered frame on the reference laptop. These are
budgets to test, not measured guarantees; cloud drawing has its own GPU budget.

The vertical domain must reach above clouds, potentially around 16 km, with more
layers near the surface. The voxel terrain renderer's current 2048 m ceiling is
not an atmospheric ceiling. Eight/sixteen layers cannot resolve detailed storm
structure; unresolved convection must be explicitly modeled or left out.

Use fixed simulation ticks, bounded substeps and coherent snapshots. Outer forcing
can update more slowly than local weather, but the local solver must substep for
its velocity/grid stability limits. Do not drop required dynamics steps merely
to meet a visual frame deadline: slow simulation time or reduce configured detail.

The first prototype can use prescribed large-scale boundary forcing. Before adding
an outer solver, define transfer of temperature, moisture, momentum and pressure
reference between domains. Start one-way nesting; account for boundary fluxes.
A periodic local map is a test boundary, not a realistic open regional atmosphere.

## What to simulate

Candidate channels: three velocity components, potential temperature, pressure
perturbation, water vapor, cloud liquid, rain, ice and a turbulence summary.
Not all are independent prognostic variables: pressure is constrained by the
chosen dynamics; ice/turbulence can remain absent in the first prototype.

Prototype an acoustic-filtered, buoyancy-aware model, such as an anelastic-style
local solver with a hydrostatic reference atmosphere. Validate dry motion before
moist physics. The coarse regional model needs a compatible pressure/vertical
balance treatment. Choose its detailed dynamics after benchmarks, not by assuming
a smoke shader becomes meteorology when a rotation term is added.

A conceptual local update includes:

1. Surface radiation, heat/moisture exchange, terrain drag and boundary forcing.
2. Transport of momentum, thermal state and moisture on the atmospheric grid.
3. Pressure constraints, buoyancy and rotation, with a consistent integration order.
4. Saturation-based condensation/evaporation with latent heating/cooling.
5. Bounded rain formation and sedimentation, evaporation and surface delivery.
6. Recomputed diagnostics for visibility, instability and precipitation.

Maintain a water ledger across vapor, condensate, rain, ground storage, evaporation
and boundary exchanges. Track energy sources/sinks rather than silently creating
heat. Stable transport is not automatically conservative: basic semi-Lagrangian
advection is useful for graphics but needs an explicit conservation/error policy.
Do not mask an unstable model with arbitrary clipping; quantify any corrections.

Clouds are visible condensate, not solid blocks. A cloud does not universally imply
high wind. Strong updrafts, downdrafts and gust fronts must follow resolved flow
or a stated sub-grid storm model. That model can be parameterized and still share
moisture/heat/momentum budgets; a hand-authored storm object should be labeled as
such rather than advertised as spontaneous meteorology.

## Fit the discrete voxel world

Atmospheric cells are aligned integer multiples of the 6.25 cm grid. Coarse fields
represent aggregate physical quantities; temperature and speed are not spatial
positions. Query them at integer voxel cells without introducing off-grid occupied
positions. The existing fractional movement-credit pattern can support wind-driven
motion while entity locations remain integer cells.

Cloud optics are a derived field, not millions of persistent edits to air voxels.
If clouds are displayed as voxel volumes, their sampled density/appearance stays
constant within a native voxel, with grid-aligned visual LOD and no sub-voxel noise.
Any future visual smoothing is a separate presentation option. Cloud detail must
remain consistent with the simulated water envelope rather than inventing moisture.

Provide conceptual queries such as sample_weather(CellPos, Tick), surface_rainfall,
and precipitation_column. Clouds, seeds, instruments and vehicles consume the same
snapshot. Correlated gust detail may be evaluated from world position/time/seed,
with bounded amplitude and scale; avoid uncorrelated per-frame random forces.
Shelter and terrain-flow corrections can be cached near active interactions.

Keep authoritative updates independent of visibility. Camera position may control
render detail, but must not make a storm or offscreen seed cease to evolve. An
initial fixed regional weather domain is simpler than moving/refining simulation
patches. Future relocation needs state transfer, not regeneration from a seed.

## Future consumers

- **Seeds and organisms:** local wind, drag/settling and deterministic movement
  credits; batch distant seed populations rather than individual particles everywhere.
- **Vehicles/aircraft:** air-relative velocity, gusts and vertical flow from the
  same weather query. Aerodynamic/body models remain separate future systems.
- **Radar:** approximate returns from precipitation/ice and an assumed particle-size
  model. Doppler velocity is the scattering particles' line-of-sight velocity,
  including fall speed/platform motion, not a complete wind-vector image. Add beam
  averaging, terrain blockage and instrument update intervals progressively.
- **Cloud lighting:** condensate determines optical depth, shadows and reduced
  surface sunlight. Cloud rendering need not run at atmospheric update frequency.
- **Waves:** wind speed, duration and fetch can drive a coarse wave-energy model;
  distant swell requires boundary input. Coastal shallow-water motion, tides and
  storm surge are separate extensions, not just a moving water texture. Quantize
  visible/interactable water to whole cells with a volume ledger/residual credits;
  arbitrary height rounding must not create or destroy water. Moon/sun tidal forcing
  acts through an ocean boundary/reservoir model with accounted exchanges.

## Stages and gates

1. **Landforms and climate inputs:** independent geology/uplift, regional heat and
   moisture, prevailing winds/rain shadows, drainage and surface classification.
   Show debug maps and validate varied terrain before adding a runtime atmosphere.
2. **Headless dry weather:** bounded grid, reference atmosphere, transport, pressure,
   thermal forcing, friction and rotation. Test stationary balance, transported
   features, decay and rotation without spurious energy growth.
3. **Moist weather + visible clouds:** evaporation, condensation, precipitation and
   terrain lifting. Demonstrate a cloud forming, moving and dissipating, rainfall
   and a rain shadow, while checking water/heat budgets. Benchmark CPU/GPU separately.
4. **Shared interaction:** seed transport probe, surface wetness and simple radar
   view generated from actual model fields. Verify repeatability and offscreen behavior.
5. **Severe-weather experiments:** add the larger ocean-fed driver and improved
   vertical dynamics; test favorable and unfavorable environments. Distinguish
   emergent rotating lows from imposed vortices. A warm-core hurricane, eyewall,
   tornado or realistic radar signature is a separate validation milestone.
6. **Ocean and long-lived ecology:** wave/tidal coupling, richer surface reservoirs
   and vegetation response after the underlying budgets and costs are established.

Weather may organize convincingly without resolving every vortex or cloud eddy.
A sound starting demonstration is an evolving rain cloud and wind that moves a
seed, not an unvalidated promise to simulate an entire planet on a basic laptop.

## Sources

- [NOAA: tropical cyclone structure and size](https://www.noaa.gov/jetstream/tropical/tropical-cyclone-introduction/tropical-cyclone-structure)
- [NOAA: hurricane formation conditions](https://www.noaa.gov/education/resource-collections/weather-atmosphere/hurricanes)
- [NOAA: how clouds form](https://www.noaa.gov/jetstream/clouds/how-clouds-form)
- [ECMWF researchers: hydrostatic/non-hydrostatic modeling](https://doi.org/10.1002/qj.70144)
- [NVIDIA GPU Gems: real-time 3D fluids](https://developer.nvidia.com/gpugems/gpugems3/part-v-physics-simulation/chapter-30-real-time-simulation-and-rendering-3d-fluids)
- [NWS: radar reflectivity and radial velocity](https://www.weather.gov/mkx/using-radar)
- [NOAA: tides](https://oceanservice.noaa.gov/facts/tides.html)
- [NOAA: wind waves and swell](https://www.noaa.gov/jetstream/ocean/waves)

These motivate the physics distinctions. The proposed hierarchy, grid sizes,
integration stages and performance budgets are engineering choices for this game;
none of the sources proves that this configuration produces realistic hurricanes.
