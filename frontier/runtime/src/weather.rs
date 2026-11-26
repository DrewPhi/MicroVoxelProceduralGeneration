//! A terrain-anchored, warm-rain atmospheric volume for gameplay. All public
//! coordinates are world metres, with Y up. Mixing ratios are kg/kg.
use glam::{DVec3, IVec3, Vec3};
use std::sync::Arc;
mod checkpoint;
pub mod radar;
pub mod regional;

pub const SIZE: [usize; 3] = [48, 40, 48];
pub const CELL: Vec3 = Vec3::new(320., 250., 320.);
pub const EXTENT: Vec3 = Vec3::new(
    SIZE[0] as f32 * CELL.x,
    SIZE[1] as f32 * CELL.y,
    SIZE[2] as f32 * CELL.z,
);
pub const STEP_SECONDS: f32 = 2.;
const COUNT: usize = SIZE[0] * SIZE[1] * SIZE[2];
const DX_INV: Vec3 = Vec3::new(1. / 320., 1. / 250., 1. / 320.);
const PRESSURE_WEIGHT: Vec3 = Vec3::new(1. / (320. * 320.), 1. / (250. * 250.), 1. / (320. * 320.));
const RAIN_FALL: f32 = 7.;
const LATENT_HEAT_OVER_CP: f32 = 2480.;
const CLOUD_THRESHOLD: f32 = 0.000055;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Regime {
    Dynamic,
    Regional,
    Clear,
    Rain,
    Convective,
}

impl Regime {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "dynamic" => Some(Self::Dynamic),
            "regional" => Some(Self::Regional),
            "clear" => Some(Self::Clear),
            "rain" => Some(Self::Rain),
            "convective" => Some(Self::Convective),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Air {
    pub wind: Vec3,
    pub temperature_k: f32,
    pub vapor: f32,
    pub cloud: f32,
    pub rain: f32,
    /// Turbulent velocity scale in m/s.
    pub turbulence: f32,
    pub resolved: bool,
}

impl Air {
    pub fn rain_rate_mm_h(self) -> f32 {
        self.rain * 1.08 * RAIN_FALL * 3600.
    }

    /// Warm-rain reflectivity estimate, in linear Z units.
    pub fn reflectivity(self) -> f32 {
        200. * self.rain_rate_mm_h().powf(1.6)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct CellState {
    wind: Vec3,
    theta: f32,
    vapor: f32,
    cloud: f32,
    rain: f32,
    turbulence: f32,
}

impl CellState {
    fn lerp(self, next: Self, t: f32) -> Self {
        let mix = |a, b| a + (b - a) * t;
        Self {
            wind: self.wind.lerp(next.wind, t),
            theta: mix(self.theta, next.theta),
            vapor: mix(self.vapor, next.vapor),
            cloud: mix(self.cloud, next.cloud),
            rain: mix(self.rain, next.rain),
            turbulence: mix(self.turbulence, next.turbulence),
        }
    }
    fn from_air(air: Air, height: f32) -> Self {
        Self {
            wind: air.wind,
            theta: air.temperature_k + 0.00976 * height,
            vapor: air.vapor,
            cloud: air.cloud,
            rain: air.rain,
            turbulence: air.turbulence,
        }
    }

    fn as_air(self, height: f32, resolved: bool) -> Air {
        Air {
            wind: self.wind,
            temperature_k: self.theta - 0.00976 * height,
            vapor: self.vapor,
            cloud: self.cloud,
            rain: self.rain,
            turbulence: self.turbulence,
            resolved,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Climate {
    pub seed: u64,
    pub regime: Regime,
    pub prevailing_wind: Vec3,
}

pub const SOLAR_DAY_SECONDS: f64 = 86400.;

/// Maps weather solver time onto the local solar calendar. Changing the day
/// length preserves the phase at the current playhead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolarClock {
    anchor: f64,
    day: f64,
    length: f64,
}

impl SolarClock {
    pub fn new(seconds: f64, length: f64) -> Self {
        Self {
            anchor: seconds,
            day: seconds / SOLAR_DAY_SECONDS,
            length,
        }
    }

    pub fn day(self, seconds: f64) -> f64 {
        self.day + (seconds - self.anchor) / self.length
    }

    pub fn retime(&mut self, seconds: f64, length: f64) {
        if self.length != length {
            *self = Self {
                anchor: seconds,
                day: self.day(seconds),
                length,
            };
        }
    }
}

#[derive(Clone, Copy)]
struct Conditions {
    warmth: f32,
    moisture: f32,
    instability: f32,
    flow: Vec3,
    blanket: f32,
    sun: f32,
}

impl Climate {
    fn conditions(self, center: DVec3, seconds: f64) -> Conditions {
        self.with_solar(center, seconds, seconds / SOLAR_DAY_SECONDS)
    }

    fn with_solar(self, center: DVec3, seconds: f64, solar_day: f64) -> Conditions {
        let phase =
            ((self.seed ^ (self.seed >> 28)) & 65535) as f64 * (std::f64::consts::TAU / 65536.);
        let local = seconds / 86400.;
        // Low-frequency, advecting synoptic waves. Moist and dry air can
        // persist across several day-night cycles before the next front.
        let front = (center.x / 240_000. + center.z / 320_000. - local * 0.94 + phase).sin() * 0.64
            + (local * 0.48 + phase * 1.9).sin() * 0.36;
        let sun = ((solar_day.rem_euclid(1.) - 0.25) * std::f64::consts::TAU)
            .sin()
            .max(0.) as f32;
        let (warmth, moisture, instability, blanket) = match self.regime {
            Regime::Clear => (288., 0.28, 0.10, 0.),
            Regime::Rain => (286., 0.99, 0.10, 1.),
            Regime::Convective => (301., 0.88, 1., 0.),
            Regime::Dynamic | Regime::Regional => {
                let moist = (front as f32 * 0.5 + 0.5).clamp(0., 1.);
                let warm = ((local * 0.59 + phase * 0.44).sin() * 0.5 + 0.5) as f32;
                (
                    285. + 14. * warm,
                    0.40 + 0.56 * moist,
                    (warm * moist * 1.45 + sun * 0.12 - 0.23).clamp(0., 1.),
                    ((moist - 0.63) * 3.6 * (1. - warm)).clamp(0., 1.),
                )
            }
        };
        let dir = (phase + seconds / 48_000.) as f32;
        let speed = if self.regime == Regime::Clear {
            2.
        } else if matches!(self.regime, Regime::Dynamic | Regime::Regional) {
            0.8 + (front as f32 + 1.) * 4.8
        } else {
            4. + (front as f32 + 1.) * 3.4
        };
        Conditions {
            warmth,
            moisture,
            instability,
            flow: Vec3::new(dir.cos() * speed, 0., dir.sin() * speed) + self.prevailing_wind,
            blanket,
            sun,
        }
    }
}

impl Conditions {
    fn air(self, height: f32) -> CellState {
        let h = height.max(0.);
        let stability = 0.0058 - 0.0039 * self.instability;
        let theta = self.warmth + stability * h + (h - 7700.).max(0.) * 0.005;
        let p = pressure(h);
        let sat = saturation(theta - 0.00976 * h, p);
        let dry_above = (-h / 4400.).exp();
        let layer = band(h, 650., 2900.);
        let humidity =
            (self.moisture * (0.64 + 0.36 * dry_above) + self.blanket * layer * 0.38).min(1.12);
        let shear = (h / 7200.).clamp(0., 1.5) * (0.4 + self.instability);
        CellState {
            wind: self.flow + Vec3::new(9., 0., -6.) * shear,
            theta,
            vapor: sat * humidity,
            ..Default::default()
        }
    }
}

fn pressure(h: f32) -> f32 {
    101325. * (1. - h.clamp(-400., 18_000.) * 2.25577e-5).powf(5.25588)
}
fn saturation(t: f32, p: f32) -> f32 {
    let c = t.clamp(205., 325.) - 273.15;
    let e = 611.2 * (17.67 * c / (c + 243.5)).exp();
    0.622 * e / (p - e).max(1000.)
}
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}
fn band(height: f32, bottom: f32, top: f32) -> f32 {
    smooth((height - bottom) / 500.) * smooth((top - height) / 720.)
}
fn n(x: usize, y: usize, z: usize) -> usize {
    (z * SIZE[1] + y) * SIZE[0] + x
}
fn coord(i: usize) -> IVec3 {
    IVec3::new(
        (i % SIZE[0]) as i32,
        ((i / SIZE[0]) % SIZE[1]) as i32,
        (i / (SIZE[0] * SIZE[1])) as i32,
    )
}
fn idx(p: IVec3) -> usize {
    n(p.x as usize, p.y as usize, p.z as usize)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    pub cloud_cells: usize,
    pub rain_cells: usize,
    pub cloud_base_m: f32,
    pub cloud_top_m: f32,
    pub peak_rain_mm_h: f32,
    pub rain_core_m: [f64; 3],
    pub updraft_mps: f32,
    pub downdraft_mps: f32,
    pub cloud_water_kg_kg_sum: f32,
    pub rain_water_kg_kg_sum: f32,
}

#[derive(Clone)]
pub struct Snapshot {
    pub time: f64,
    pub origin: DVec3,
    cells: Arc<[CellState]>,
    surface: Arc<[f32]>,
    climate: Climate,
    solar: Option<SolarClock>,
    pub metrics: Metrics,
    pub regional: Option<regional::Frame>,
    gust_shift: DVec3,
    gust_advection: Vec3,
    /// Active cloud/rain bounds in texture coordinates, padded for filtering.
    pub wet_bounds: Option<([f32; 3], [f32; 3])>,
}

impl Snapshot {
    pub fn sample(&self, xyz: DVec3) -> Air {
        let p = (xyz - self.origin).as_vec3();
        if p.cmpge(Vec3::ZERO).all() && p.cmplt(EXTENT).all() {
            let g = p * DX_INV;
            let ground = self.ground(xyz);
            if xyz.y >= f64::from(ground) {
                return interpolate(&self.cells, g - Vec3::splat(0.5)).as_air(xyz.y as f32, true);
            }
        }
        if let Some(air) = self
            .regional
            .as_ref()
            .and_then(|r| r.sample(xyz, self.time))
        {
            return air;
        }
        self.solar
            .map_or_else(
                || self.climate.conditions(xyz, self.time),
                |clock| {
                    self.climate
                        .with_solar(xyz, self.time, clock.day(self.time))
                },
            )
            .air(xyz.y as f32)
            .as_air(xyz.y as f32, false)
    }

    pub fn contains_horizontal(&self, p: DVec3) -> bool {
        let p = p - self.origin;
        p.x >= 0. && p.z >= 0. && p.x < f64::from(EXTENT.x) && p.z < f64::from(EXTENT.z)
    }

    pub fn ground(&self, xyz: DVec3) -> f32 {
        let p = ((xyz - self.origin).as_vec3() * DX_INV).floor();
        let x = (p.x as i32).clamp(0, SIZE[0] as i32 - 1) as usize;
        let z = (p.z as i32).clamp(0, SIZE[2] as i32 - 1) as usize;
        self.surface[z * SIZE[0] + x]
    }

    /// Condensate suitable for upload as RG8. Values are mass mixing ratios
    /// with explicit scale factors matched by the consuming shader.
    pub fn volume_rg8(&self) -> Vec<[u8; 2]> {
        self.cells
            .iter()
            .map(|v| {
                [
                    (255. * (v.cloud * 320.).clamp(0., 1.)) as u8,
                    (255. * (v.rain * 500.).clamp(0., 1.)) as u8,
                ]
            })
            .collect()
    }

    /// Peak linear reflectivity of the temporally interpolated rain column.
    /// Same-domain adjacent snapshots share the grid and solid mask.
    pub fn interpolate_column_echo(&self, next: &Self, x: usize, z: usize, fraction: f32) -> f32 {
        debug_assert_eq!(self.origin, next.origin);
        let mixing_ratio = (0..SIZE[1])
            .map(|y| {
                let index = n(x, y, z);
                let a = self.cells[index].rain;
                a + (next.cells[index].rain - a) * fraction
            })
            .fold(0_f32, f32::max);
        let rate = mixing_ratio * 1.08 * RAIN_FALL * 3600.;
        200. * rate.powf(1.6)
    }

    /// The strongest visible rain echo in a column. Sample in linear Z, and
    /// ignore elevations below the terrain or the lowest unblocked beam.
    pub fn column_echo_band(&self, xyz: DVec3, min_msl: f32, max_msl: f32) -> Option<f32> {
        if !self.contains_horizontal(xyz) {
            return None;
        }
        let p = ((xyz - self.origin).as_vec3() * DX_INV).floor();
        let x = p.x as usize;
        let z = p.z as usize;
        let min_y = ((min_msl.max(self.ground(xyz)) - self.origin.y as f32) / CELL.y)
            .floor()
            .max(0.) as usize;
        let max_y =
            (((max_msl - self.origin.y as f32) / CELL.y).ceil().max(0.) as usize).min(SIZE[1]);
        if min_y >= max_y {
            return None;
        }
        let rain = (min_y..max_y)
            .map(|y| self.cells[n(x, y, z)].rain)
            .fold(0., f32::max);
        Some(200. * (rain * 1.08 * RAIN_FALL * 3600.).powf(1.6))
    }

    pub fn surface_rain(&self, xyz: DVec3) -> f32 {
        if !self.contains_horizontal(xyz) {
            return 0.;
        }
        let y = (self.ground(xyz).max(self.origin.y as f32) + CELL.y * 0.55) as f64;
        self.sample(DVec3::new(xyz.x, y, xyz.z)).rain_rate_mm_h()
    }
}

/// Sample a coherent smaller-scale gust field. The envelope comes only from
/// the resolved shear and overturning. Nearby points share the same eddies.
pub fn gust(now: &Snapshot, position: DVec3, seconds: f64) -> Vec3 {
    let air = now.sample(position);
    let p = (position
        - if now.regional.is_some() {
            // A world-aligned phase survives changes to the local volume origin.
            DVec3::ZERO
        } else {
            now.origin
        }
        - now.gust_shift
        - now.gust_advection.as_dvec3() * (seconds - now.time))
        .as_vec3();
    let eddy = Vec3::new(
        (p.y * 0.11 + p.z * 0.038).sin() + 0.34 * (p.y * 0.48 - p.z * 0.19).sin(),
        (p.x * 0.16 - p.z * 0.073).sin() + 0.35 * (p.x * 0.52 + p.z * 0.19).sin(),
        (p.x * 0.073 + p.y * 0.14).sin() + 0.34 * (p.x * 0.26 - p.y * 0.42).sin(),
    );
    air.wind + eddy * (air.turbulence * 0.58)
}

fn interpolate(cells: &[CellState], p: Vec3) -> CellState {
    let top = Vec3::new(
        (SIZE[0] - 1) as f32,
        (SIZE[1] - 1) as f32,
        (SIZE[2] - 1) as f32,
    );
    let p = p.clamp(Vec3::ZERO, top - Vec3::splat(0.001));
    let lo = p.as_ivec3();
    let f = p - lo.as_vec3();
    let pair = |y, z| {
        let left = idx(lo + IVec3::new(0, y, z));
        cells[left].lerp(cells[left + 1], f.x)
    };
    pair(0, 0)
        .lerp(pair(1, 0), f.y)
        .lerp(pair(0, 1).lerp(pair(1, 1), f.y), f.z)
}

fn grid_origin(center: DVec3, align: bool) -> DVec3 {
    let origin = DVec3::new(
        center.x - f64::from(EXTENT.x) * 0.5,
        0.,
        center.z - f64::from(EXTENT.z) * 0.5,
    );
    if align {
        (origin / CELL.as_dvec3()).round() * CELL.as_dvec3()
    } else {
        origin
    }
}

fn grid_column(origin: DVec3, i: usize) -> DVec3 {
    origin
        + DVec3::new((i % SIZE[0]) as f64 + 0.5, 0., (i / SIZE[0]) as f64 + 0.5) * CELL.as_dvec3()
}

fn solid_mask(surface: &[f32]) -> Vec<bool> {
    (0..COUNT)
        .map(|i| {
            let p = coord(i);
            (p.y as f32 + 0.5) * CELL.y <= surface[p.z as usize * SIZE[0] + p.x as usize]
        })
        .collect()
}

fn grid_neighbors(solid: &[bool]) -> Vec<[usize; 6]> {
    let limit = IVec3::new(SIZE[0] as i32, SIZE[1] as i32, SIZE[2] as i32);
    (0..COUNT)
        .map(|i| {
            let pos = coord(i);
            [
                IVec3::X,
                -IVec3::X,
                IVec3::Y,
                -IVec3::Y,
                IVec3::Z,
                -IVec3::Z,
            ]
            .map(|dir| {
                let p = pos + dir;
                if p.cmpge(IVec3::ZERO).all() && p.cmplt(limit).all() && !solid[idx(p)] {
                    idx(p)
                } else {
                    i
                }
            })
        })
        .collect()
}

pub struct Simulation {
    climate: Climate,
    solar: Option<SolarClock>,
    origin: DVec3,
    center: DVec3,
    time: f64,
    surface: Arc<[f32]>,
    surface_weather: Option<Vec<regional::Surface>>,
    regional: Option<regional::Simulation>,
    solid: Vec<bool>,
    now: Vec<CellState>,
    next: Vec<CellState>,
    pressure: Vec<f32>,
    scratch_pressure: Vec<f32>,
    divergence: Vec<f32>,
    neighbors: Vec<[usize; 6]>,
    saturation_pressure: [f32; SIZE[1]],
    gust_shift: DVec3,
}

impl Simulation {
    pub fn new(
        climate: Climate,
        center: DVec3,
        time: f64,
        terrain: impl Fn(f64, f64) -> f32,
    ) -> Self {
        Self::build(climate, center, time, terrain, None, None)
    }

    pub fn new_regional(
        climate: Climate,
        settings: regional::Settings,
        coarse_surface: impl Fn(f64, f64) -> regional::Surface,
        local_surface: &dyn Fn(f64, f64) -> regional::Surface,
    ) -> Self {
        let regional = regional::Simulation::new(settings, coarse_surface);
        Self::build(
            climate,
            settings.center,
            settings.initial_time,
            |x, z| local_surface(x, z).ground,
            Some(regional),
            Some(local_surface),
        )
    }

    fn build(
        climate: Climate,
        center: DVec3,
        time: f64,
        terrain: impl Fn(f64, f64) -> f32,
        regional: Option<regional::Simulation>,
        local_surface: Option<&dyn Fn(f64, f64) -> regional::Surface>,
    ) -> Self {
        let origin = grid_origin(center, regional.is_some());
        let surface_weather: Option<Vec<_>> = local_surface.map(|sample| {
            (0..SIZE[0] * SIZE[2])
                .map(|i| {
                    let p = grid_column(origin, i);
                    sample(p.x, p.z)
                })
                .collect()
        });
        let surface: Arc<[_]> = (0..SIZE[0] * SIZE[2])
            .map(|i| {
                if let Some(weather) = &surface_weather {
                    return weather[i].ground;
                }
                let p = grid_column(origin, i);
                terrain(p.x, p.z)
            })
            .collect();
        let solid = solid_mask(&surface);
        let conditions = climate.conditions(center, time);
        let ambient: [_; SIZE[1]] =
            std::array::from_fn(|y| conditions.air((y as f32 + 0.5) * CELL.y));
        let offset = Vec3::new(1700., 0., -900.);
        let mut now = Vec::with_capacity(COUNT);
        for (i, &ground) in solid.iter().enumerate() {
            let p = coord(i);
            let xyz = (p.as_vec3() + Vec3::splat(0.5)) * CELL;
            let alt = xyz.y - surface[p.z as usize * SIZE[0] + p.x as usize];
            let mut air = ambient[p.y as usize];
            if let Some(regional) = &regional {
                let position = origin + xyz.as_dvec3();
                if let Some(local) = regional.frame().sample(position, time) {
                    air = CellState::from_air(local, xyz.y);
                }
                if ground {
                    air.wind = Vec3::ZERO;
                    air.cloud = 0.;
                    air.rain = 0.;
                }
            } else if !ground {
                // A small warm, moist boundary-layer disturbance seeds the
                // convective scenario. There is no initial condensate or rain.
                let offset_xz = (xyz - EXTENT * 0.5 - offset) * Vec3::new(1., 0., 1.);
                let perturb = (-offset_xz.length_squared() / (1420. * 1420.)
                    - ((alt - 780.) / 580.).powi(2))
                .exp();
                air.theta += perturb * conditions.instability * 4.7;
                air.vapor += perturb * conditions.instability * 0.0045;
            } else {
                air.wind = Vec3::ZERO;
            }
            now.push(air);
        }
        let neighbors = grid_neighbors(&solid);
        Self {
            climate,
            solar: None,
            origin,
            center,
            time,
            surface,
            surface_weather,
            regional,
            solid,
            now,
            next: vec![CellState::default(); COUNT],
            pressure: vec![0.; COUNT],
            scratch_pressure: vec![0.; COUNT],
            divergence: vec![0.; COUNT],
            neighbors,
            gust_shift: DVec3::ZERO,
            saturation_pressure: std::array::from_fn(|y| pressure((y as f32 + 0.5) * CELL.y)),
        }
    }

    /// Recenter on whole cells. Copies overlapping world cells without
    /// reinitializing active cloud water, rain, or wind.
    pub fn follow(&mut self, view: DVec3, sample: &dyn Fn(f64, f64) -> regional::Surface) -> bool {
        let Some(regional) = &self.regional else {
            return false;
        };
        let desired = grid_origin(view, true);
        let movement = ((desired - self.origin) / CELL.as_dvec3()).as_ivec3();
        if movement.x.abs().max(movement.z.abs()) < 5 {
            return false;
        }
        let old_surface = self.surface_weather.as_ref().unwrap();
        let inside =
            |x: i32, z: i32| (0..SIZE[0] as i32).contains(&x) && (0..SIZE[2] as i32).contains(&z);
        let old_column = |i: usize| {
            let x = (i % SIZE[0]) as i32 + movement.x;
            let z = (i / SIZE[0]) as i32 + movement.z;
            inside(x, z).then(|| z as usize * SIZE[0] + x as usize)
        };
        let surface: Vec<_> = (0..SIZE[0] * SIZE[2])
            .map(|i| {
                old_column(i).map_or_else(
                    || {
                        let p = grid_column(desired, i);
                        sample(p.x, p.z)
                    },
                    |old| old_surface[old],
                )
            })
            .collect();
        self.surface = surface.iter().map(|s| s.ground).collect();
        self.solid = solid_mask(&self.surface);
        for i in 0..COUNT {
            let p = coord(i);
            let from = p + movement;
            self.next[i] = if inside(from.x, from.z) {
                self.now[idx(from)]
            } else {
                let xyz = desired + ((p.as_vec3() + 0.5) * CELL).as_dvec3();
                let height = xyz.y as f32;
                let mut air = regional.frame().sample(xyz, self.time).map_or_else(
                    || self.climate.conditions(xyz, self.time).air(height),
                    |air| CellState::from_air(air, height),
                );
                if self.solid[i] {
                    air.wind = Vec3::ZERO;
                    air.cloud = 0.;
                    air.rain = 0.;
                }
                air
            };
        }
        self.origin = desired;
        self.center = desired + (EXTENT * Vec3::new(0.5, 0., 0.5)).as_dvec3();
        self.neighbors = grid_neighbors(&self.solid);
        self.pressure.fill(0.);
        self.scratch_pressure.fill(0.);
        self.surface_weather = Some(surface);
        std::mem::swap(&mut self.now, &mut self.next);
        true
    }

    pub fn time(&self) -> f64 {
        self.time
    }

    pub fn set_solar_clock(&mut self, clock: SolarClock) {
        self.solar = Some(clock);
    }

    pub fn advance(&mut self) {
        let dt = STEP_SECONDS;
        self.time += f64::from(dt);
        if let Some(regional) = &mut self.regional {
            regional.update_to(self.time);
        }
        let regional = self.regional.as_ref().map(regional::Simulation::frame);
        let conditions = self.solar.map_or_else(
            || self.climate.conditions(self.center, self.time),
            |clock| {
                self.climate
                    .with_solar(self.center, self.time, clock.day(self.time))
            },
        );
        let ambient: [_; SIZE[1]] =
            std::array::from_fn(|y| conditions.air((y as f32 + 0.5) * CELL.y));
        let boundary = regional.map(|regional| {
            let mut profiles = [[CellState::default(); SIZE[1]]; 4];
            for (corner, profile) in profiles.iter_mut().enumerate() {
                let x = if corner & 1 == 0 { 0. } else { EXTENT.x } as f64;
                let z = if corner & 2 == 0 { 0. } else { EXTENT.z } as f64;
                for (y, air) in profile.iter_mut().enumerate() {
                    let height = (y as f32 + 0.5) * CELL.y;
                    let point = self.origin + DVec3::new(x, f64::from(height), z);
                    *air = regional
                        .sample(point, self.time)
                        .map_or(ambient[y], |air| CellState::from_air(air, height));
                }
            }
            profiles
        });
        let regional_wind = regional
            .and_then(|regional| regional.sample(self.center + DVec3::Y * 700., self.time))
            .map_or(conditions.flow, |air| air.wind);
        self.gust_shift += regional_wind.as_dvec3() * f64::from(dt);
        let pressure_ratio = regional.map_or(1., |r| r.pressure_ratio(self.center, self.time));
        let day = self.time / SOLAR_DAY_SECONDS;
        let sunlight = regional.map_or(conditions.sun, |r| {
            regional::solar_elevation(day, day, r.current.latitude_deg).max(0.)
        });
        let mut surface_flux = Vec::new();
        if let Some(surfaces) = &self.surface_weather {
            for (i, surface) in surfaces.iter().enumerate() {
                let thermal = 1. - surface.water * 0.85;
                let wet = (surface.moisture * 0.86 + surface.water * 0.55 + 0.08).min(1.);
                let x = i % SIZE[0];
                let z = i / SIZE[0];
                let cloud = (0..16)
                    .map(|y| self.now[n(x, y + 4, z)].cloud)
                    .fold(0., f32::max);
                let sun = sunlight * (1. - (cloud * 680.).min(0.82));
                let day_heat = 0.9 + 6.0 * (1. - wet);
                let night_cool = (1. - sunlight).powi(3) * 2.4 * (1. - wet);
                let warm = ((surface.warmth - 0.5) * 4. + sun * day_heat - night_cool) * thermal;
                surface_flux.push((warm, wet, 0.22 + surface.roughness * 0.8));
            }
        }
        for i in 0..COUNT {
            let pos = coord(i);
            let height = (pos.y as f32 + 0.5) * CELL.y;
            let y = pos.y as usize;
            let base = boundary.as_ref().map_or(ambient[y], |p| {
                let x = (pos.x as f32 + 0.5) / SIZE[0] as f32;
                let z = (pos.z as f32 + 0.5) / SIZE[2] as f32;
                p[0][y].lerp(p[1][y], x).lerp(p[2][y].lerp(p[3][y], x), z)
            });
            if self.solid[i] {
                self.next[i] = CellState {
                    wind: Vec3::ZERO,
                    cloud: 0.,
                    rain: 0.,
                    ..base
                };
                continue;
            }
            let velocity = self.now[i].wind;
            let departure = pos.as_vec3() - velocity * DX_INV * dt;
            let mut air = interpolate(&self.now, departure);
            let [xp, xm, yp, ym, zp, zm] = self.neighbors[i];
            let column = pos.z as usize * SIZE[0] + pos.x as usize;
            let altitude = height - self.surface[column];
            let horizontal_edge = pos
                .x
                .min(SIZE[0] as i32 - 1 - pos.x)
                .min(pos.z.min(SIZE[2] as i32 - 1 - pos.z));
            let sponge = 1.
                - smooth(horizontal_edge as f32 / 4.)
                    * smooth((SIZE[1] as i32 - 1 - pos.y) as f32 / 5.);
            air = air.lerp(base, sponge * dt / 24.);
            if altitude < 600. {
                let coupling = 1. - smooth(altitude / 600.);
                let p = self.saturation_pressure[y] * pressure_ratio;
                let near_sat = saturation(air.theta - 0.00976 * height, p);
                if let Some(&(heat, wet, roughness)) = surface_flux.get(column) {
                    air.theta += (base.theta + heat - air.theta) * coupling * dt / 900.;
                    air.vapor += (near_sat * wet - air.vapor).max(0.) * wet * coupling * dt / 540.;
                    air.wind.x *= 1. - roughness * coupling * dt / 150.;
                    air.wind.z *= 1. - roughness * coupling * dt / 150.;
                } else {
                    let daytime = if self.climate.regime == Regime::Convective {
                        0.9
                    } else {
                        conditions.sun
                    };
                    let warm = base.theta + 2.7 * daytime * conditions.instability;
                    air.theta += (warm - air.theta) * coupling * dt / 400.;
                    let moisture =
                        near_sat * (conditions.moisture + 0.09 * conditions.instability).min(0.98);
                    air.vapor += (moisture - air.vapor) * coupling * dt / 280.;
                    air.wind.x *= 1. - coupling * dt / 90.;
                    air.wind.z *= 1. - coupling * dt / 90.;
                }
            }

            let p = self.saturation_pressure[y] * pressure_ratio;
            let temp = air.theta - 0.00976 * height;
            let sat = saturation(temp, p);
            // Implicit linearized latent-temperature feedback prevents
            // condensation overshoot. Evaporation gives the heat back.
            let slope = (saturation(temp + 0.25, p) - sat) * 4.;
            let adjustment = (air.vapor - sat) / (1. + LATENT_HEAT_OVER_CP * slope);
            let cloud_change = adjustment.clamp(-air.cloud, air.vapor);
            air.vapor -= cloud_change;
            air.cloud += cloud_change;
            air.theta += LATENT_HEAT_OVER_CP * cloud_change;
            let rain_change = ((air.cloud - 0.00043).max(0.) / 95.
                + air.cloud * (air.rain * 320.).min(2.) / 64.)
                * dt;
            let rain_change = rain_change.min(air.cloud);
            air.cloud -= rain_change;
            air.rain += rain_change;
            let evaporation =
                (-(air.vapor - sat).min(0.) * (air.rain * 250.).min(1.) * dt / 100.).min(air.rain);
            air.rain -= evaporation;
            air.vapor += evaporation;
            air.theta -= LATENT_HEAT_OVER_CP * evaporation;
            let flow_down = if yp == i { 0. } else { self.now[yp].rain };
            air.rain =
                (air.rain + (flow_down - self.now[i].rain) * (RAIN_FALL * dt / CELL.y)).max(0.);
            let buoyancy = 9.81
                * ((air.theta - base.theta) / base.theta + 0.61 * (air.vapor - base.vapor)
                    - air.cloud
                    - air.rain);
            // The regional sounding carries its own balanced condensate and
            // vertical motion. Only local deviations contribute buoyancy here.
            let background_load = if regional.is_some() {
                9.81 * (base.cloud + base.rain)
            } else {
                0.
            };
            air.wind.y += (buoyancy + background_load - (air.wind.y - base.wind.y) / 490.) * dt;
            // Filter only grid-scale velocity to retain resolved updrafts.
            let smooth_wind = [xp, xm, yp, ym, zp, zm]
                .iter()
                .map(|&j| self.now[j].wind)
                .sum::<Vec3>()
                / 6.;
            air.wind = air.wind.lerp(smooth_wind, 0.012).clamp_length_max(62.);
            let du = (self.now[xp].wind - self.now[xm].wind) * (0.5 * DX_INV.x);
            let dv = (self.now[yp].wind - self.now[ym].wind) * (0.5 * DX_INV.y);
            let dw = (self.now[zp].wind - self.now[zm].wind) * (0.5 * DX_INV.z);
            let shear = (du.length_squared() + dv.length_squared() + dw.length_squared()).sqrt();
            let sustain = (shear * 125. + buoyancy.abs() * 7. + air.wind.y.abs() * 0.13).min(11.);
            air.turbulence += (sustain - air.turbulence) * dt / 15.;
            self.next[i] = air;
        }
        self.project();
        std::mem::swap(&mut self.now, &mut self.next);
    }

    fn project(&mut self) {
        for (i, (cell, [xp, xm, yp, ym, zp, zm])) in self
            .next
            .iter()
            .zip(self.neighbors.iter().copied())
            .enumerate()
        {
            // Horizontal boundaries transmit the background flow. Terrain and the
            // upper lid remain impermeable normal to their cell faces.
            let pos = coord(i);
            let face = |j: usize, axis: usize, open: bool| {
                if j != i {
                    (cell.wind[axis] + self.next[j].wind[axis]) * 0.5
                } else if open {
                    cell.wind[axis]
                } else {
                    0.
                }
            };
            self.divergence[i] = (face(xp, 0, pos.x == SIZE[0] as i32 - 1)
                - face(xm, 0, pos.x == 0))
                * DX_INV.x
                + (face(yp, 1, false) - face(ym, 1, false)) * DX_INV.y
                + (face(zp, 2, pos.z == SIZE[2] as i32 - 1) - face(zm, 2, pos.z == 0)) * DX_INV.z;
            // Start the bounded projection from the prior tick's correction.
            self.pressure[i] *= 0.55;
        }
        let w = PRESSURE_WEIGHT;
        for _ in 0..12 {
            for (i, [xp, xm, yp, ym, zp, zm]) in self.neighbors.iter().copied().enumerate() {
                if self.solid[i] {
                    continue;
                }
                let p = &self.pressure;
                self.scratch_pressure[i] =
                    ((p[xp] + p[xm]) * w.x + (p[yp] + p[ym]) * w.y + (p[zp] + p[zm]) * w.z
                        - self.divergence[i])
                        / (2. * (w.x + w.y + w.z));
            }
            std::mem::swap(&mut self.pressure, &mut self.scratch_pressure);
        }
        for (i, [xp, xm, yp, ym, zp, zm]) in self.neighbors.iter().copied().enumerate() {
            if self.solid[i] {
                continue;
            }
            let p = &self.pressure;
            let v = self.next[i].wind
                - Vec3::new(p[xp] - p[xm], p[yp] - p[ym], p[zp] - p[zm]) * (DX_INV * 0.5);
            let mut v = v.clamp_length_max(62.);
            let pos = coord(i);
            if xm == i && pos.x != 0 {
                v.x = v.x.max(0.);
            }
            if xp == i && pos.x != SIZE[0] as i32 - 1 {
                v.x = v.x.min(0.);
            }
            if zm == i && pos.z != 0 {
                v.z = v.z.max(0.);
            }
            if zp == i && pos.z != SIZE[2] as i32 - 1 {
                v.z = v.z.min(0.);
            }
            if ym == i {
                v.y = v.y.max(0.);
            }
            if yp == i {
                v.y = v.y.min(0.);
            }
            self.next[i].wind = v;
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut metrics = Metrics::default();
        let mut lo = IVec3::splat(i32::MAX);
        let mut hi = IVec3::splat(i32::MIN);
        let mut cloud_base = f32::INFINITY;
        for (i, air) in self.now.iter().enumerate() {
            let height = (coord(i).y as f32 + 0.5) * CELL.y;
            if air.cloud >= CLOUD_THRESHOLD {
                metrics.cloud_cells += 1;
                metrics.cloud_top_m = metrics.cloud_top_m.max(height);
                cloud_base = cloud_base.min(height);
            }
            if air.rain >= 0.000015 {
                metrics.rain_cells += 1;
                let rate = air.as_air(height, true).rain_rate_mm_h();
                if rate > metrics.peak_rain_mm_h {
                    metrics.peak_rain_mm_h = rate;
                    metrics.rain_core_m = (self.origin
                        + ((coord(i).as_vec3() + Vec3::splat(0.5)) * CELL).as_dvec3())
                    .to_array();
                }
            }
            if air.cloud >= CLOUD_THRESHOLD || air.rain >= 0.000015 {
                let p = coord(i);
                lo = lo.min(p);
                hi = hi.max(p);
            }
            metrics.cloud_water_kg_kg_sum += air.cloud;
            metrics.rain_water_kg_kg_sum += air.rain;
            metrics.updraft_mps = metrics.updraft_mps.max(air.wind.y);
            metrics.downdraft_mps = metrics.downdraft_mps.min(air.wind.y);
        }
        if cloud_base.is_finite() {
            metrics.cloud_base_m = cloud_base;
        }
        let sizes = EXTENT * DX_INV;
        let wet_bounds = (lo.x != i32::MAX).then(|| {
            (
                ((lo - IVec3::splat(2)).as_vec3() / sizes)
                    .max(Vec3::ZERO)
                    .to_array(),
                ((hi + IVec3::splat(3)).as_vec3() / sizes)
                    .min(Vec3::ONE)
                    .to_array(),
            )
        });
        Snapshot {
            time: self.time,
            cells: Arc::from(self.now.as_slice()),
            surface: self.surface.clone(),
            origin: self.origin,
            climate: self.climate,
            solar: self.solar,
            metrics,
            gust_shift: self.gust_shift,
            gust_advection: self
                .regional
                .as_ref()
                .and_then(|r| r.frame().sample(self.center + DVec3::Y * 700., self.time))
                .map_or_else(
                    || self.climate.conditions(self.center, self.time).flow,
                    |a| a.wind,
                ),
            regional: self.regional.as_ref().map(|r| r.frame().clone()),
            wet_bounds,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regional_weather_keeps_overlapping_clouds_and_resumes_the_same_state() {
        let climate = Climate {
            seed: 9,
            regime: Regime::Regional,
            prevailing_wind: Vec3::ZERO,
        };
        let settings = regional::Settings {
            seed: 9,
            latitude_deg: 43.,
            wind_offset: Vec3::ZERO,
            initial_time: 46800.,
            center: DVec3::ZERO,
        };
        let surface = |_, _| regional::Surface {
            moisture: 0.58,
            ..Default::default()
        };
        let mut sim = Simulation::new_regional(climate, settings, surface, &surface);
        sim.set_solar_clock(SolarClock::new(settings.initial_time, 1200.));
        for _ in 0..18 {
            sim.advance();
        }
        let position = DVec3::new(160., 1875., 160.);
        let before = sim.snapshot();
        assert!(sim.follow(DVec3::X * 2880., &surface));
        let shifted = sim.snapshot();
        assert_ne!(shifted.origin, before.origin);
        assert_eq!(shifted.sample(position).wind, before.sample(position).wind);
        assert_eq!(
            shifted.sample(position).cloud,
            before.sample(position).cloud
        );

        let mut checkpoint = Vec::new();
        sim.write_checkpoint(&mut checkpoint).unwrap();
        let mut restored = Simulation::new_regional(climate, settings, surface, &surface);
        restored.read_checkpoint(checkpoint.as_slice()).unwrap();
        assert_eq!(restored.solar_clock(), sim.solar_clock());
        assert_eq!(restored.snapshot().volume_rg8(), shifted.volume_rg8());
        sim.advance();
        restored.advance();
        assert_eq!(
            restored.snapshot().volume_rg8(),
            sim.snapshot().volume_rg8()
        );
        assert_eq!(
            restored.snapshot().sample(position).wind,
            sim.snapshot().sample(position).wind
        );
        assert!(restored.read_checkpoint(&checkpoint[..100]).is_err());
    }

    #[test]
    fn clear_and_wet_soundings_produce_different_weather_without_initial_clouds() {
        let climate = |regime| Climate {
            seed: 7,
            regime,
            prevailing_wind: Vec3::ZERO,
        };
        let mut wet = Simulation::new(climate(Regime::Rain), DVec3::ZERO, 42_000., |_, _| 200.);
        let mut dry = Simulation::new(climate(Regime::Clear), DVec3::ZERO, 42_000., |_, _| 200.);
        assert_eq!(wet.snapshot().metrics.cloud_cells, 0);
        for _ in 0..75 {
            wet.advance();
            dry.advance();
        }
        let wet = wet.snapshot();
        assert!(wet.metrics.cloud_cells > 800 && wet.metrics.rain_cells > 100);
        assert_eq!(dry.snapshot().metrics.cloud_cells, 0);
        assert!(!wet.sample(DVec3::X * 50_000.).resolved);
    }

    #[test]
    fn air_cannot_enter_an_internal_mountain_face() {
        let climate = Climate {
            seed: 7,
            regime: Regime::Clear,
            prevailing_wind: Vec3::X * 15.,
        };
        let mut sim = Simulation::new(climate, DVec3::ZERO, 12000., |x, _| {
            if x > 0. {
                1650.
            } else {
                50.
            }
        });
        sim.advance();
        let x = SIZE[0] / 2 - 1;
        let z = SIZE[2] / 2;
        assert!(sim.now[n(x, 2, z)].wind.x <= 0.);
        assert!(sim.now[n(x, 8, z)].wind.x > 4.);
    }

    #[test]
    fn dynamic_climate_changes_across_days_and_gusts_are_spatially_coherent() {
        let climate = Climate {
            seed: 7,
            regime: Regime::Dynamic,
            prevailing_wind: Vec3::ZERO,
        };
        let samples: Vec<_> = (0..100)
            .map(|day| climate.conditions(DVec3::ZERO, 46800. + day as f64 * 86400.))
            .collect();
        assert!(samples
            .iter()
            .any(|c| c.moisture < 0.48 && c.instability < 0.4));
        assert!(samples
            .iter()
            .any(|c| c.moisture > 0.85 && c.instability > 0.67));
        assert!(samples.iter().any(|c| c.blanket > 0.35));
        let mut model = Simulation::new(climate, DVec3::ZERO, 46_800., |_, _| 0.);
        for c in &mut model.now {
            c.turbulence = 3.;
        }
        let snap = model.snapshot();
        let p = DVec3::Y * 1000.;
        let center = gust(&snap, p, snap.time);
        assert!((center - gust(&snap, p + DVec3::X * 0.01, snap.time)).length() < 0.025);
        assert!((center - gust(&snap, p + DVec3::X * 7., snap.time)).length() > 0.20);
    }

    #[test]
    #[ignore = "full convective lifecycle and tuning diagnostic"]
    fn convective_lifecycle() {
        let climate = Climate {
            seed: 7,
            regime: Regime::Convective,
            prevailing_wind: Vec3::ZERO,
        };
        let mut sim = Simulation::new(climate, DVec3::ZERO, 42_000., |x, _| {
            (650. * (-((x + 1900.) / 2100.).powi(2)).exp() + 210.) as f32
        });
        let start = std::time::Instant::now();
        let mut peak = Metrics::default();
        for i in 0..=540 {
            if i % 60 == 0 {
                let s = sim.snapshot();
                let m = s.metrics;
                peak.updraft_mps = peak.updraft_mps.max(m.updraft_mps);
                peak.downdraft_mps = peak.downdraft_mps.min(m.downdraft_mps);
                peak.cloud_cells = peak.cloud_cells.max(m.cloud_cells);
                peak.rain_cells = peak.rain_cells.max(m.rain_cells);
                peak.peak_rain_mm_h = peak.peak_rain_mm_h.max(m.peak_rain_mm_h);
                eprintln!("age {i} steps: {m:?} bounds {:?}", s.wet_bounds);
                assert!(
                    m.updraft_mps.is_finite()
                        && m.downdraft_mps.is_finite()
                        && m.updraft_mps <= 62.01
                );
            }
            sim.advance();
        }
        eprintln!("{} ms/step", start.elapsed().as_secs_f64() * 1000. / 541.);
        let m = sim.snapshot().metrics;
        assert!(peak.cloud_cells > 600 && peak.rain_cells > 1000);
        assert!(peak.updraft_mps > 15. && peak.downdraft_mps < -2.2);
        assert!(m.updraft_mps < peak.updraft_mps * 0.4);
        assert!(m.peak_rain_mm_h < peak.peak_rain_mm_h * 0.4);
    }
}
