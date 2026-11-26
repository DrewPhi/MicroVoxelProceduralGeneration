//! Rotating, hydrostatic perturbation model for synoptic weather. Coordinates
//! follow the engine map: +X east, -Z north, +Y up.
use super::checkpoint::{doubles, floats, invalid, write_doubles, write_floats};
use super::{pressure, saturation, Air, LATENT_HEAT_OVER_CP};
use glam::{DVec3, Vec2, Vec3};
use std::{
    io::{self, Read, Write},
    sync::Arc,
};

pub const SIZE: [usize; 3] = [128, 12, 96];
pub const SPACING: f32 = 24_000.;
pub const LAYER: f32 = 1500.;
pub const STEP: f64 = 20.;
const CELLS: usize = SIZE[0] * SIZE[2];
const VOLUME: usize = CELLS * SIZE[1];
const GRAVITY: f32 = 9.81;
const OMEGA: f32 = 7.292_115e-5;
const EARTH_RADIUS: f32 = 6_371_000.;
const WAVE_SPEED: f32 = 190.;
const INV_DISTANCE: f32 = 0.5 / SPACING;
const INITIAL_SPEED: f32 = 8.;

pub fn solar_declination(calendar_days: f64) -> f32 {
    23.44_f32.to_radians()
        * (std::f64::consts::TAU * ((calendar_days - 80.) / 365.25).rem_euclid(1.)).sin() as f32
}

pub fn solar_elevation(solar_day: f64, calendar_days: f64, latitude_deg: f32) -> f32 {
    let hour = (std::f64::consts::TAU * (solar_day.rem_euclid(1.) - 0.5)) as f32;
    let (sd, cd) = solar_declination(calendar_days).sin_cos();
    let (sl, cl) = latitude_deg.to_radians().sin_cos();
    sl * sd + cl * cd * hour.cos()
}

fn frontal_band(x: f32, z: f32, shift: f32, phase: f32) -> (f32, f32) {
    let front_x = shift - 0.63 * z + 70_000. * (z / 184_000. + phase).sin();
    // Along-front variation separates or connects bands as the curvature
    // and lower-tropospheric moisture supply change across the domain.
    let longitude = x / 380_000.;
    let latitude = z / 560_000.;
    let wave = |offset: f32, seed_phase: f32| (offset + seed_phase).sin() - seed_phase.sin();
    let curvature = 136_000. * wave(longitude + latitude * 0.8, phase * 1.71)
        + 79_000. * wave(longitude * 1.61 - latitude, phase * 0.53 + 1.3);
    let shape = ((x - front_x + curvature) / 111_000.).sin();
    let connection = 0.24
        + 0.76
            * (1.0 + 0.50 * wave(longitude * 1.31 - latitude * 1.77, phase * 1.52)).clamp(0., 1.);
    let warm = 0.5 + 0.5 * (shape * 2.7).tanh();
    let convergence = (-(shape / 0.30).powi(2)).exp() * connection;
    (warm, convergence)
}

#[derive(Clone, Copy, Debug)]
pub struct Surface {
    pub ground: f32,
    /// Normalized, long-term temperature and moisture from the terrain source.
    pub warmth: f32,
    pub moisture: f32,
    pub water: f32,
    pub roughness: f32,
}

impl Default for Surface {
    fn default() -> Self {
        Self {
            ground: 0.,
            warmth: 0.5,
            moisture: 0.5,
            water: 0.,
            roughness: 0.45,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Layer {
    wind: Vec2,
    theta: f32,
    vapor: f32,
    cloud: f32,
    rain: f32,
    lift: f32,
}

impl Layer {
    fn mix(self, other: Self, t: f32) -> Self {
        let mix = |a, b| a + (b - a) * t;
        Self {
            wind: self.wind.lerp(other.wind, t),
            theta: mix(self.theta, other.theta),
            vapor: mix(self.vapor, other.vapor),
            cloud: mix(self.cloud, other.cloud),
            rain: mix(self.rain, other.rain),
            lift: mix(self.lift, other.lift),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub seed: u64,
    pub latitude_deg: f32,
    pub wind_offset: Vec3,
    pub initial_time: f64,
    pub center: DVec3,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Column {
    pub pressure_hpa: f32,
    pub jet_mps: f32,
    pub cloud_base_m: f32,
    pub cloud_top_m: f32,
    pub cloud_water: f32,
    pub rain_mm_h: f32,
}

#[derive(Clone)]
pub struct Snapshot {
    pub time: f64,
    pub origin: DVec3,
    pub latitude_deg: f32,
    layers: Arc<[Layer]>,
    mass: Arc<[f32]>,
    columns: Arc<[Column]>,
    water: Arc<[[u8; 2]]>,
}

impl Snapshot {
    fn coordinates(&self, position: DVec3) -> Option<Vec3> {
        let p = (position - self.origin).as_vec3();
        (p.x >= 0. && p.z >= 0. && p.x < SPACING * SIZE[0] as f32 && p.z < SPACING * SIZE[2] as f32)
            .then(|| p / Vec3::new(SPACING, LAYER, SPACING) - 0.5)
    }

    pub fn sample(&self, position: DVec3) -> Option<Air> {
        let point = self.coordinates(position)?;
        let a = sample_layers(&self.layers, point);
        let height = position.y as f32;
        let shear = (sample_layers(&self.layers, point + Vec3::Y * 0.5).wind
            - sample_layers(&self.layers, point - Vec3::Y * 0.5).wind)
            .length();
        Some(Air {
            wind: Vec3::new(a.wind.x, a.lift, a.wind.y),
            temperature_k: a.theta - 0.00976 * height.clamp(-400., 18_000.),
            vapor: a.vapor,
            cloud: a.cloud,
            rain: a.rain,
            turbulence: (0.35 * (shear - 7.).max(0.) + a.lift.abs() * 0.6).min(4.),
            resolved: false,
        })
    }

    pub fn pressure_ratio(&self, position: DVec3) -> f32 {
        self.coordinates(position).map_or(1., |p| {
            1. + sample_plane(p.xz(), |c| self.mass[c]) / (287. * 288.)
        })
    }

    pub fn volume(&self) -> &[[u8; 2]] {
        &self.water
    }

    pub fn weather_visible(&self, eye: DVec3, distance: f32) -> bool {
        let p = (eye - self.origin).as_vec3() / SPACING;
        let radius = distance / SPACING + 1.;
        let (x0, x1) = ((p.x - radius).floor() as i32, (p.x + radius).ceil() as i32);
        let (z0, z1) = ((p.z - radius).floor() as i32, (p.z + radius).ceil() as i32);
        (z0.max(0)..z1.min(SIZE[2] as i32)).any(|z| {
            (x0.max(0)..x1.min(SIZE[0] as i32)).any(|x| {
                let c = self.columns[plane(x as usize, z as usize)];
                c.cloud_water > 0.000012 || c.rain_mm_h > 0.15
            })
        })
    }

    pub fn columns(&self) -> &[Column] {
        &self.columns
    }
}

use glam::Vec3Swizzles;

#[derive(Clone)]
pub struct Frame {
    pub previous: Arc<Snapshot>,
    pub current: Arc<Snapshot>,
}

impl Frame {
    pub fn fraction(&self, time: f64) -> f32 {
        ((time - self.previous.time) / STEP).clamp(0., 1.) as f32
    }

    pub fn sample(&self, position: DVec3, time: f64) -> Option<Air> {
        let a = self.previous.sample(position)?;
        let b = self.current.sample(position)?;
        let t = self.fraction(time);
        let mix = |a, b| a + (b - a) * t;
        Some(Air {
            wind: a.wind.lerp(b.wind, t),
            temperature_k: mix(a.temperature_k, b.temperature_k),
            vapor: mix(a.vapor, b.vapor),
            cloud: mix(a.cloud, b.cloud),
            rain: mix(a.rain, b.rain),
            turbulence: mix(a.turbulence, b.turbulence),
            resolved: false,
        })
    }

    pub fn pressure_ratio(&self, position: DVec3, time: f64) -> f32 {
        let a = self.previous.pressure_ratio(position);
        a + (self.current.pressure_ratio(position) - a) * self.fraction(time)
    }
}

pub(super) struct Simulation {
    settings: Settings,
    origin: DVec3,
    time: f64,
    layers: Vec<Layer>,
    next: Vec<Layer>,
    mass: Vec<f32>,
    next_mass: Vec<f32>,
    geopotential: Vec<f32>,
    frontal_shift: f32,
    frontal_phase: f32,
    reference: Vec<Layer>,
    terrain: Vec<Surface>,
    coriolis: [f32; SIZE[2]],
    frame: Frame,
}

fn index(column: usize, y: usize) -> usize {
    (column / SIZE[0] * SIZE[1] + y) * SIZE[0] + column % SIZE[0]
}

fn plane(x: usize, z: usize) -> usize {
    z * SIZE[0] + x
}

fn sample_plane(p: Vec2, get: impl Fn(usize) -> f32) -> f32 {
    let (a, b, f) = corners(p);
    let mix = |from, to, t| from + (to - from) * t;
    let lo = mix(get(plane(a[0], a[1])), get(plane(b[0], a[1])), f.x);
    let hi = mix(get(plane(a[0], b[1])), get(plane(b[0], b[1])), f.x);
    mix(lo, hi, f.y)
}

fn corners(p: Vec2) -> ([usize; 2], [usize; 2], Vec2) {
    let p = p.clamp(
        Vec2::ZERO,
        Vec2::new((SIZE[0] - 1) as f32, (SIZE[2] - 1) as f32),
    );
    let a = [p.x as usize, p.y as usize];
    let b = [(a[0] + 1).min(SIZE[0] - 1), (a[1] + 1).min(SIZE[2] - 1)];
    (a, b, p.fract())
}

fn sample_layers(layers: &[Layer], p: Vec3) -> Layer {
    let (a, b, f) = corners(p.xz());
    let py = p.y.clamp(0., (SIZE[1] - 1) as f32);
    let bot = py as usize;
    let top = (bot + 1).min(SIZE[1] - 1);
    let at = |x, z, y| layers[index(plane(x, z), y)];
    let row = |y| {
        at(a[0], a[1], y)
            .mix(at(b[0], a[1], y), f.x)
            .mix(at(a[0], b[1], y).mix(at(b[0], b[1], y), f.x), f.y)
    };
    row(bot).mix(row(top), py.fract())
}

fn altitude(y: usize) -> f32 {
    (y as f32 + 0.5) * LAYER
}

fn phase(seed: u64, salt: u64) -> f32 {
    let mut n = seed.wrapping_add(salt).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    n = (n ^ (n >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    n = (n ^ (n >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    ((n ^ (n >> 31)) >> 40) as f32 / (1 << 24) as f32
}

impl Simulation {
    pub(super) fn new(settings: Settings, surface: impl Fn(f64, f64) -> Surface) -> Self {
        let Settings {
            seed,
            latitude_deg,
            wind_offset,
            initial_time,
            center,
        } = settings;
        let origin = center
            - DVec3::new(
                f64::from(SPACING) * SIZE[0] as f64 * 0.5,
                0.,
                f64::from(SPACING) * SIZE[2] as f64 * 0.5,
            );
        let latitude = latitude_deg.to_radians();
        let f0 = 2. * OMEGA * latitude.sin();
        let beta = 2. * OMEGA * latitude.cos() / EARTH_RADIUS;
        let distance = |i, n| (i as f32 + 0.5 - n as f32 * 0.5) * SPACING;
        let coriolis = std::array::from_fn(|z| f0 - beta * distance(z, SIZE[2]));
        let terrain: Vec<_> = (0..CELLS)
            .map(|c| {
                surface(
                    origin.x + (c % SIZE[0]) as f64 * f64::from(SPACING) + f64::from(SPACING) * 0.5,
                    origin.z + (c / SIZE[0]) as f64 * f64::from(SPACING) + f64::from(SPACING) * 0.5,
                )
            })
            .collect();
        let season = (std::f64::consts::TAU * (initial_time / (365.25 * 86400.) - 0.53)).cos()
            as f32
            * latitude.signum();
        let jet_z = (phase(seed, 17) - 0.5) * 440_000. - season * latitude.signum() * 190_000.;
        let hemisphere = f0.signum();
        let jet_speed = 37. - 9. * season;
        let jet = |h: f32| (-((h - 10_300.) / 3980.).powi(2)).exp() * jet_speed;
        let jet_width = 265_000.;
        let theta_ref = |z: f32, h: f32| {
            let h = h.max(0.);
            let shear = (jet(h + 200.) - jet(h - 200.)) / 400.;
            let cross = ((z - jet_z) / jet_width).tanh();
            // Thermal wind: north is -Z. Integrate the cross-jet geopotential
            // gradient in Z to initialize the associated temperature contrast.
            let thermal = 292. / GRAVITY * f0 * jet_width * shear * cross;
            290. + 4. * season
                + 0.00355 * h
                + 0.012 * (h - 10_200.).max(0.)
                + 3. * hemisphere * cross
                + thermal
        };
        let mut reference = vec![Layer::default(); VOLUME];
        let mut layers = vec![Layer::default(); VOLUME];
        let mut mass = vec![0.; CELLS];
        let wave = phase(seed, 29) * std::f32::consts::TAU;
        let frontal_shift = (phase(seed, 31) - 0.5) * 540_000.;
        let disturbance = |x: f32, z: f32| {
            let along = x / 380_000. + wave;
            let belt = (-((z - jet_z) / 660_000.).powi(2)).exp();
            680. * belt * (along.sin() + 0.31 * (along * 1.81 + z / 610_000.).cos())
        };
        for (c, mass) in mass.iter_mut().enumerate() {
            let x = distance(c % SIZE[0], SIZE[0]);
            let z = distance(c / SIZE[0], SIZE[2]);
            let f = coriolis[c / SIZE[0]];
            let land = terrain[c];
            let (front, convergence) = frontal_band(x, z, frontal_shift, wave);
            *mass = disturbance(x, z);
            let ux = (disturbance(x, z + SPACING) - disturbance(x, z - SPACING)) * INV_DISTANCE / f;
            let uz =
                -(disturbance(x + SPACING, z) - disturbance(x - SPACING, z)) * INV_DISTANCE / f;
            for y in 0..SIZE[1] {
                let h = altitude(y);
                let below_jet = (-h / 4300.).exp();
                let jet_flow = jet(h) / (1. + ((z - jet_z) / jet_width).sinh().powi(2));
                let theta = theta_ref(z, h);
                let saturated = saturation(theta - 0.00976 * h, pressure(h));
                let rh = (0.30
                    + 0.30 * below_jet
                    + (land.moisture - 0.5) * below_jet * 0.46
                    + land.water * below_jet * 0.08)
                    .clamp(0.10, 0.95);
                let base = Layer {
                    theta,
                    wind: Vec2::new(INITIAL_SPEED + jet_flow + wind_offset.x, wind_offset.z),
                    vapor: saturated * rh,
                    ..Default::default()
                };
                let layer = index(c, y);
                reference[layer] = base;
                let frontal_layer = (-((h - 2300.) / 3100.).powi(2)).exp();
                layers[layer] = Layer {
                    theta: theta + (front - 0.5) * 6.5 * below_jet,
                    vapor: saturated
                        * (rh
                            + (front - 0.30) * (0.47 + 0.26 * land.moisture) * frontal_layer
                            + convergence * 0.50 * frontal_layer)
                            .clamp(0.08, 1.42),
                    wind: base.wind + Vec2::new(ux, uz) * (0.47 + 0.36 * below_jet),
                    ..base
                };
            }
        }
        let empty = Arc::new(Snapshot {
            time: initial_time,
            origin,
            latitude_deg,
            layers: Arc::from([]),
            columns: Arc::from([]),
            water: Arc::from([]),
            mass: Arc::from([]),
        });
        let mut sim = Self {
            settings,
            origin,
            time: initial_time,
            layers,
            next: vec![Layer::default(); VOLUME],
            mass,
            next_mass: vec![0.; CELLS],
            geopotential: vec![0.; VOLUME],
            frontal_shift,
            frontal_phase: wave,
            reference,
            terrain,
            coriolis,
            frame: Frame {
                current: empty.clone(),
                previous: empty,
            },
        };
        sim.saturate_initial();
        sim.frame.current = Arc::new(sim.snapshot());
        sim.update_to(initial_time);
        sim
    }

    fn saturate_initial(&mut self) {
        for c in 0..CELLS {
            for y in 0..SIZE[1] {
                let a = &mut self.layers[index(c, y)];
                let h = altitude(y);
                condense(a, h, 0.70);
            }
        }
    }

    pub(super) fn update_to(&mut self, time: f64) {
        while time >= self.time {
            self.advance();
            let snapshot = Arc::new(self.snapshot());
            self.frame.previous = std::mem::replace(&mut self.frame.current, snapshot);
        }
    }

    pub(super) fn frame(&self) -> &Frame {
        &self.frame
    }

    fn advance(&mut self) {
        let dt = STEP as f32;
        for c in 0..CELLS {
            let mut phi = 0.;
            let mut average = 0.;
            for y in 0..SIZE[1] {
                let i = index(c, y);
                let a = self.layers[i];
                let b = self.reference[i];
                let buoyancy = GRAVITY
                    * ((a.theta - b.theta) / b.theta + 0.61 * (a.vapor - b.vapor)
                        - a.cloud
                        - a.rain);
                self.geopotential[i] = phi + 0.5 * LAYER * buoyancy;
                average += self.geopotential[i];
                phi += LAYER * buoyancy;
            }
            // Separate the external (column mass) mode from pressure
            // variations due to internal thermal and moisture stratification.
            let correction = self.mass[c] - average / SIZE[1] as f32;
            for y in 0..SIZE[1] {
                self.geopotential[index(c, y)] += correction;
            }
        }
        let day = self.time / 86400.;
        let sun = solar_elevation(day, day, self.settings.latitude_deg);
        for c in 0..CELLS {
            let x = c % SIZE[0];
            let z = c / SIZE[0];
            let east = plane((x + 1).min(SIZE[0] - 1), z);
            let west = plane(x.saturating_sub(1), z);
            let north = plane(x, z.saturating_sub(1));
            let south = plane(x, (z + 1).min(SIZE[2] - 1));
            let topography = self.terrain[c];
            let gradient = Vec2::new(
                self.terrain[east].ground - self.terrain[west].ground,
                self.terrain[south].ground - self.terrain[north].ground,
            ) * INV_DISTANCE;
            let edge = x.min(SIZE[0] - 1 - x).min(z.min(SIZE[2] - 1 - z)) as f32;
            let damping = 0.035 * (1. - (edge / 6.).clamp(0., 1.)).powi(2);
            let inflow = if x < 6 {
                let x_m = (x as f32 + 0.5 - SIZE[0] as f32 * 0.5) * SPACING;
                let z_m = (z as f32 + 0.5 - SIZE[2] as f32 * 0.5) * SPACING;
                // Supply broad warm and cold air masses at the outer western
                // boundary as the upstream air crosses it. Dynamics govern
                // inland moisture transport and where it condenses.
                let offset = (self.time - self.settings.initial_time) as f32 * INITIAL_SPEED;
                Some(frontal_band(
                    x_m - offset,
                    z_m,
                    self.frontal_shift,
                    self.frontal_phase,
                ))
            } else {
                None
            };
            let ground_lift = self.layers[index(c, 0)].wind.dot(gradient).clamp(-2., 2.);
            let diagnose_divergence = |y| {
                let [e, w, s, n] = [east, west, south, north].map(|col| index(col, y));
                ((self.layers[e].wind.x - self.layers[w].wind.x)
                    + (self.layers[s].wind.y - self.layers[n].wind.y))
                    * INV_DISTANCE
            };
            let divergence: [f32; SIZE[1]] = std::array::from_fn(diagnose_divergence);
            let mean = divergence.iter().sum::<f32>() / SIZE[1] as f32;
            let mut lift = ground_lift;
            for (y, &divergence) in divergence.iter().enumerate() {
                let i = index(c, y);
                let [e, w, s, n] = [east, west, south, north].map(|col| index(col, y));
                let here = self.layers[i];
                let base = self.reference[i];
                let h = altitude(y);
                // Vertically integrate continuity with zero velocity at the
                // lid and the orographic lower boundary velocity.
                let dw = -LAYER * (divergence - mean) - ground_lift / SIZE[1] as f32;
                let layer_lift = (lift + 0.5 * dw).clamp(-2., 2.);
                lift += dw;
                let position = Vec3::new(
                    x as f32 - here.wind.x * dt / SPACING,
                    y as f32 - layer_lift * dt / LAYER,
                    z as f32 - here.wind.y * dt / SPACING,
                );
                let mut a = sample_layers(&self.layers, position);
                let phi = &self.geopotential;
                let pressure_gradient = Vec2::new(phi[e] - phi[w], phi[s] - phi[n]) * INV_DISTANCE;
                let delta_wind = here.wind - base.wind;
                let turn = Vec2::new(-delta_wind.y, delta_wind.x) * self.coriolis[z];
                a.wind += (turn - pressure_gradient) * dt;
                let neighbor_wind = (self.layers[e].wind
                    + self.layers[w].wind
                    + self.layers[s].wind
                    + self.layers[n].wind)
                    * 0.25;
                a.wind += (neighbor_wind - here.wind) * 0.012;
                a.lift = layer_lift;
                if y == 0 {
                    let cloud_cover = (self.layers[index(c, 1)].cloud * 500.).min(0.86);
                    let land = 1. - topography.water;
                    let solar = sun.max(0.) * (1. - cloud_cover);
                    let dry = (1. - topography.moisture) * land;
                    let target =
                        base.theta + (topography.warmth - 0.5) * 14. + solar * (3. + 10. * dry)
                            - (-sun).max(0.) * (1. + 4. * dry);
                    a.theta += (target - a.theta) * dt / (10_800. + topography.water * 36_000.);
                    let vapor = saturation(a.theta - 0.00976 * h, pressure(h));
                    let wet = (0.10 + 0.82 * topography.moisture + topography.water * 0.35).min(1.);
                    // Dry ground limits evaporation. It does not delete moist
                    // air brought in by a front.
                    a.vapor += (wet * vapor - a.vapor).max(0.) * dt / 10_800. * wet;
                    a.wind *= 1. - dt * (0.10 + topography.roughness * 0.45) / 9000.;
                }
                condense(&mut a, h, 1.);
                let converted = ((a.cloud - 0.00034).max(0.) * dt / 800.).min(a.cloud);
                a.cloud -= converted;
                a.rain += converted;
                let dry = (saturation(a.theta - 0.00976 * h, pressure(h)) - a.vapor).max(0.);
                let evaporated = (dry * a.rain * 72. * dt / 120.).min(a.rain);
                a.rain -= evaporated;
                a.vapor += evaporated;
                a.theta -= evaporated * LATENT_HEAT_OVER_CP;
                let above = if y + 1 < SIZE[1] {
                    self.layers[index(c, y + 1)].rain
                } else {
                    0.
                };
                a.rain = (a.rain + (above - here.rain) * 6. * dt / LAYER).max(0.);
                let exterior = inflow.map_or(base, |(warm, convergence)| {
                    let envelope = (-((h - 1800.) / 3400.).powi(2)).exp();
                    let vapor = saturation(base.theta - 0.00976 * h, pressure(h));
                    Layer {
                        theta: base.theta + (warm - 0.5) * 6.5 * (-h / 4300.).exp(),
                        vapor: (base.vapor
                            + vapor * ((warm - 0.30) * 0.48 + convergence * 0.39) * envelope)
                            .max(0.),
                        ..base
                    }
                });
                a = a.mix(exterior, damping);
                a.wind = a.wind.clamp_length_max(105.);
                self.next[i] = a;
            }
        }
        // Update column mass with the new momentum. Advancing both from the
        // old state injects energy into pressure waves each step.
        for c in 0..CELLS {
            let x = c % SIZE[0];
            let z = c / SIZE[0];
            let east = plane((x + 1).min(SIZE[0] - 1), z);
            let west = plane(x.saturating_sub(1), z);
            let north = plane(x, z.saturating_sub(1));
            let south = plane(x, (z + 1).min(SIZE[2] - 1));
            let edge = x.min(SIZE[0] - 1 - x).min(z.min(SIZE[2] - 1 - z)) as f32;
            let damping = 0.035 * (1. - (edge / 6.).clamp(0., 1.)).powi(2);
            let mut divergence = 0.;
            for y in 0..SIZE[1] {
                let [e, w, s, n] = [east, west, south, north].map(|col| index(col, y));
                divergence += self.next[e].wind.x - self.next[w].wind.x + self.next[s].wind.y
                    - self.next[n].wind.y;
            }
            let column_div = divergence * INV_DISTANCE / SIZE[1] as f32;
            let adjacent =
                (self.mass[east] + self.mass[west] + self.mass[south] + self.mass[north]) * 0.25;
            let filtered = self.mass[c] + (adjacent - self.mass[c]) * 0.018;
            self.next_mass[c] = ((filtered - WAVE_SPEED * WAVE_SPEED * dt * column_div)
                * (1. - damping))
                .clamp(-2600., 2600.);
        }
        self.time += STEP;
        std::mem::swap(&mut self.layers, &mut self.next);
        std::mem::swap(&mut self.mass, &mut self.next_mass);
    }

    fn snapshot(&self) -> Snapshot {
        self.snapshot_for(self.time, &self.layers, &self.mass)
    }

    fn snapshot_for(&self, time: f64, layers: &[Layer], mass: &[f32]) -> Snapshot {
        let columns = (0..CELLS)
            .map(|c| {
                let mut result = Column {
                    pressure_hpa: 1013.25 * (1. + mass[c] / (287. * 288.)),
                    ..Default::default()
                };
                for y in 0..SIZE[1] {
                    let a = layers[index(c, y)];
                    if (5..10).contains(&y) {
                        result.jet_mps = result.jet_mps.max(a.wind.length());
                    }
                    if a.cloud >= 0.000055 {
                        if result.cloud_top_m == 0. {
                            result.cloud_base_m = altitude(y);
                        }
                        result.cloud_top_m = altitude(y);
                        result.cloud_water = result.cloud_water.max(a.cloud);
                    }
                    if y == 0 {
                        result.rain_mm_h = a.rain * 1.08 * 6. * 3600.;
                    }
                }
                result
            })
            .collect();
        Snapshot {
            time,
            origin: self.origin,
            latitude_deg: self.settings.latitude_deg,
            layers: Arc::from(layers),
            mass: Arc::from(mass),
            columns,
            water: layers
                .iter()
                .map(|a| {
                    [
                        (a.cloud * 600. * 255.).min(255.) as u8,
                        (a.rain * 550. * 255.).min(255.) as u8,
                    ]
                })
                .collect(),
        }
    }
    pub(super) fn write_checkpoint(&self, mut out: impl Write) -> io::Result<()> {
        write_doubles(&mut out, [self.settings.initial_time])?;
        write_doubles(&mut out, self.origin.to_array())?;
        write_floats(&mut out, [self.settings.latitude_deg])?;
        write_floats(&mut out, self.settings.wind_offset.to_array())?;
        for state in [&self.frame.previous, &self.frame.current] {
            write_doubles(&mut out, [state.time])?;
            for a in state.layers.iter() {
                write_floats(
                    &mut out,
                    [
                        a.wind.x, a.wind.y, a.theta, a.vapor, a.cloud, a.rain, a.lift,
                    ],
                )?;
            }
            for &mass in state.mass.iter() {
                write_floats(&mut out, [mass])?;
            }
        }
        Ok(())
    }

    pub(super) fn read_checkpoint(
        &mut self,
        mut input: impl Read,
        local_time: f64,
    ) -> io::Result<()> {
        if doubles::<1>(&mut input)? != [self.settings.initial_time]
            || doubles::<3>(&mut input)? != self.origin.to_array()
            || floats::<1>(&mut input)? != [self.settings.latitude_deg]
            || floats::<3>(&mut input)? != self.settings.wind_offset.to_array()
        {
            return Err(invalid(
                "regional checkpoint requires the original center, latitude, start time and wind",
            ));
        }
        let mut read_state = || {
            let [time] = doubles(&mut input)?;
            let mut layers = Vec::with_capacity(VOLUME);
            for _ in 0..VOLUME {
                let [u, v, theta, vapor, cloud, rain, lift] = floats(&mut input)?;
                if [vapor, cloud, rain].iter().any(|v| *v < 0.) {
                    return Err(invalid("negative regional atmosphere quantity"));
                }
                layers.push(Layer {
                    wind: Vec2::new(u, v),
                    theta,
                    vapor,
                    cloud,
                    rain,
                    lift,
                });
            }
            let mut mass = Vec::with_capacity(CELLS);
            for _ in 0..CELLS {
                let [value] = floats(&mut input)?;
                mass.push(value);
            }
            Ok(self.snapshot_for(time, &layers, &mass))
        };
        let previous = read_state()?;
        let current = read_state()?;
        let mut trailing = [0; 1];
        if current.time - previous.time != STEP
            || !(previous.time..current.time).contains(&local_time)
            || input.read(&mut trailing)? != 0
        {
            return Err(invalid(
                "regional checkpoint has invalid timestamps or trailing data",
            ));
        }
        self.time = current.time;
        self.layers.copy_from_slice(&current.layers);
        self.mass.copy_from_slice(&current.mass);
        self.frame = Frame {
            previous: Arc::new(previous),
            current: Arc::new(current),
        };
        Ok(())
    }
}

fn condense(a: &mut Layer, h: f32, rate: f32) {
    let t = a.theta - 0.00976 * h;
    let p = pressure(h);
    let sat = saturation(t, p);
    let slope = (saturation(t + 0.5, p) - sat) * 2.;
    let water =
        (rate * (a.vapor - sat) / (1. + LATENT_HEAT_OVER_CP * slope)).clamp(-a.cloud, a.vapor);
    a.cloud += water;
    a.vapor -= water;
    a.theta += water * LATENT_HEAT_OVER_CP;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "six simulated hours of regional weather"]
    fn six_hour_run_retains_jet_and_avoids_saturated_pressure_clamps() {
        let settings = Settings {
            seed: 9,
            latitude_deg: 43.,
            wind_offset: Vec3::ZERO,
            initial_time: 187.5 * 86400.,
            center: DVec3::ZERO,
        };
        let mut sim = Simulation::new(settings, |x, z| Surface {
            ground: (1800. * (-(x / 30000.).powi(2) - (z / 90000.).powi(2)).exp()) as f32,
            water: f32::from(x < -100_000.),
            moisture: if x > 0. { 0.24 } else { 0.72 },
            ..Default::default()
        });
        let start = sim.frame().previous.clone();
        sim.update_to(settings.initial_time + 6. * 3600.);
        let end = &sim.frame().previous;
        let clipped = end.mass.iter().filter(|p| p.abs() >= 2590.).count();
        let rain = end.columns().iter().filter(|p| p.rain_mm_h > 0.1).count();
        let jet = end.columns().iter().map(|p| p.jet_mps).fold(0., f32::max);
        let height = DVec3::Y * 2000.;
        eprintln!("weather after 6h: jet {jet:.1} m/s, rain in {rain} columns, {clipped} pressure clamps; wind {:?} -> {:?}",
            start.sample(height).unwrap().wind, end.sample(height).unwrap().wind);
        assert!(jet > 27.);
        assert!(clipped < CELLS / 100);
        assert!(rain > 0 && rain < CELLS / 2);
        assert!(end
            .layers
            .iter()
            .all(|a| a.wind.is_finite() && (235. ..=490.).contains(&a.theta)));
    }

    #[test]
    fn reproducible_jet_cloud_bands_and_regional_motion() {
        let settings = Settings {
            seed: 7,
            latitude_deg: 43.,
            wind_offset: Vec3::ZERO,
            initial_time: 13. * 3600.,
            center: DVec3::ZERO,
        };
        let mut model = Simulation::new(settings, |_, _| Surface::default());
        let first = model.frame().previous.clone();
        let replay = Simulation::new(settings, |_, _| Surface::default());
        assert_eq!(
            first.layers.as_ref(),
            replay.frame().previous.layers.as_ref()
        );
        let column = first
            .columns()
            .iter()
            .max_by(|a, b| a.jet_mps.total_cmp(&b.jet_mps))
            .unwrap();
        assert!(column.jet_mps > 35.);
        let wet = first
            .columns()
            .iter()
            .filter(|c| c.cloud_water > 0.0001)
            .count();
        assert!(wet > 0 && wet < CELLS / 2, "cloudy columns {wet}");
        model.update_to(settings.initial_time + 1800.);
        let changed = model
            .frame()
            .current
            .layers
            .iter()
            .zip(first.layers.iter())
            .filter(|(a, b)| (a.vapor - b.vapor).abs() > 0.0002)
            .count();
        assert!(changed > VOLUME / 40, "changed cells {changed}");
        assert!(model
            .frame()
            .current
            .layers
            .iter()
            .all(|a| a.wind.is_finite()
                && a.theta.is_finite()
                && a.vapor >= 0.
                && a.cloud >= 0.
                && a.rain >= 0.));
        assert!(model
            .frame()
            .sample(DVec3::splat(9e6), settings.initial_time)
            .is_none());
    }
}
