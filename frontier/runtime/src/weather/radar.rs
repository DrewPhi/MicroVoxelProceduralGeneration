//! Weather instrument products from a world atmosphere. The datalink receiver
//! has dated ground-station returns. The onboard instrument samples a beam.
use super::{Air, Snapshot, CELL, EXTENT, SIZE};
use glam::{DVec3, Vec3};

pub const WIDTH: u32 = 96;
pub const HEIGHT: u32 = 60;
pub const DATALINK_SWEEP_SECONDS: f64 = 120.;
pub const DATALINK_LATENCY_SECONDS: f64 = 60.;
const NM: f64 = 1852.;
const FT: f64 = 0.3048;
const BEAM_HALF_WIDTH: f64 = 1.8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum Mode {
    #[default]
    Datalink = 1,
    Wind = 2,
    Onboard = 3,
    Profile = 4,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Controls {
    pub mode: Mode,
    pub range_step: u8,
    pub elevation: i8,
    pub equipped: bool,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            mode: Mode::Datalink,
            range_step: 1,
            elevation: 0,
            equipped: false,
        }
    }
}
impl Controls {
    pub fn range_nm(self) -> u32 {
        1 << self.range_step
    }
    pub fn cycle(&mut self) {
        self.mode = match (self.mode, self.equipped) {
            (Mode::Datalink, _) => Mode::Wind,
            (Mode::Wind, true) => Mode::Onboard,
            (Mode::Onboard, true) => Mode::Profile,
            _ => Mode::Datalink,
        };
        self.elevation = 0;
    }
    pub fn adjust_range(&mut self, delta: i8) {
        self.range_step = (self.range_step as i8 + delta).clamp(0, 3) as u8;
    }
    pub fn adjust_vertical(&mut self, delta: i8) {
        self.elevation = (self.elevation + delta).clamp(-12, 12);
    }
}

#[derive(Clone, Copy)]
pub struct Request {
    pub position: DVec3,
    /// Radians. World +Z forward at yaw zero.
    pub yaw: f32,
    pub controls: Controls,
}

pub struct Image {
    pub pixels: Vec<[u8; 4]>,
    pub controls: Controls,
    pub layer_feet: i32,
    pub product_time: Option<f64>,
    pub profile_feet: [i32; 2],
}

const CLEAR: [u8; 4] = [7, 17, 23, 255];
const UNKNOWN: [u8; 4] = [48, 58, 64, 255];
const SHADOW: [u8; 4] = [110, 96, 93, 255];
const TERRAIN: [u8; 4] = [99, 77, 39, 255];
const OUTSIDE: [u8; 4] = [5, 12, 18, 255];
const ARROW: [u8; 4] = [194, 239, 230, 255];

pub struct Datalink {
    pub scan_time: f64,
    origin: DVec3,
    bins: Vec<Option<f32>>,
}

impl Datalink {
    pub fn scan(snapshot: &Snapshot) -> Self {
        let site =
            snapshot.origin + DVec3::new(f64::from(EXTENT.x) * 0.50, 0., f64::from(EXTENT.z) * 0.5);
        Self::scan_at(snapshot, site)
    }

    pub fn scan_at(snapshot: &Snapshot, mut site: DVec3) -> Self {
        if !snapshot.contains_horizontal(site) {
            return Self {
                scan_time: snapshot.time,
                origin: snapshot.origin,
                bins: vec![None; SIZE[0] * SIZE[2]],
            };
        }
        site.y = f64::from(snapshot.ground(site)) + 28.;
        let max_slope = 19.5_f64.to_radians().tan();
        let min_slope = 0.5_f64.to_radians().tan();
        let mut bins = Vec::with_capacity(SIZE[0] * SIZE[2]);
        for z in 0..SIZE[2] {
            for x in 0..SIZE[0] {
                let target = snapshot.origin
                    + DVec3::new(
                        (x as f64 + 0.5) * f64::from(CELL.x),
                        0.,
                        (z as f64 + 0.5) * f64::from(CELL.z),
                    );
                let d = target - DVec3::new(site.x, 0., site.z);
                let range = d.length();
                let mut slope = min_slope;
                let samples = (range / 320.).ceil().max(1.) as usize;
                for i in 1..samples {
                    let t = i as f64 / samples as f64;
                    let msl = f64::from(snapshot.ground(site + d * t));
                    slope = slope.max((msl - site.y) / (range * t));
                }
                if slope > max_slope {
                    bins.push(None);
                } else {
                    bins.push(snapshot.column_echo_band(
                        target,
                        (site.y + range * slope) as f32,
                        (site.y + range * max_slope + 250.) as f32,
                    ));
                }
            }
        }
        Self {
            scan_time: snapshot.time,
            origin: snapshot.origin,
            bins,
        }
    }

    pub fn sample(&self, position: DVec3) -> Option<f32> {
        let p = (position - self.origin).as_vec3();
        if p.x < 0. || p.z < 0. || p.x >= EXTENT.x || p.z >= EXTENT.z {
            return None;
        }
        self.bins[(p.z / CELL.z) as usize * SIZE[0] + (p.x / CELL.x) as usize]
    }
}

fn echo_color(z: f32) -> [u8; 4] {
    let dbz = 10. * z.max(0.001).log10();
    if dbz < 18. {
        CLEAR
    } else if dbz < 34. {
        [26, 155, 73, 255]
    } else if dbz < 43. {
        [231, 197, 43, 255]
    } else if dbz < 50. {
        [220, 61, 48, 255]
    } else {
        [193, 51, 179, 255]
    }
}

pub fn render(
    weather: Option<&Snapshot>,
    mosaic: Option<&Datalink>,
    request: Request,
    constant: Vec3,
) -> Image {
    let controls = request.controls;
    let mode = controls.mode;
    let range = f64::from(controls.range_nm()) * NM;
    let layer_feet = (request.position.y / FT).round() as i32 + i32::from(controls.elevation) * 500;
    let y_msl = f64::from(layer_feet) * FT;
    let grid_image = matches!(mode, Mode::Wind | Mode::Datalink);
    let ground_plane = |x: u32, y: u32| {
        let dx = (f64::from(x) + 0.5) / f64::from(WIDTH) - 0.5;
        let dz = (f64::from(y) + 0.5 - f64::from(HEIGHT) * 0.5) / f64::from(WIDTH);
        DVec3::new(
            request.position.x + 2. * range * dx,
            y_msl,
            request.position.z + 2. * range * dz,
        )
    };
    let mut pixels = vec![OUTSIDE; (WIDTH * HEIGHT) as usize];
    let sample_wind = |p| {
        weather.map_or(
            Air {
                wind: constant,
                resolved: true,
                ..Default::default()
            },
            |s| s.sample(p),
        )
    };
    let bearing = f64::from(request.yaw);
    let mut product_time = None;
    let mut profile_feet = [0; 2];
    if grid_image {
        if mode == Mode::Datalink {
            product_time = mosaic.map(|data| data.scan_time);
        }
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let p = ground_plane(x, y);
                let base = if mode == Mode::Datalink {
                    mosaic
                        .and_then(|data| data.sample(p))
                        .map_or_else(|| coverage(x, y), echo_color)
                } else {
                    let air = sample_wind(p);
                    if air.resolved {
                        let speed = air.wind.x.hypot(air.wind.z);
                        let t = (speed / 31.).clamp(0., 1.);
                        [
                            ((14. + t * 224.) as u8),
                            (33. + t * 154.) as u8,
                            (50. + t * 37.) as u8,
                            255,
                        ]
                    } else {
                        coverage(x, y)
                    }
                };
                pixels[(y * WIDTH + x) as usize] = base;
            }
        }
        if mode == Mode::Wind {
            for y in (6..HEIGHT).step_by(12) {
                for x in (8..WIDTH).step_by(16) {
                    let a = sample_wind(ground_plane(x, y));
                    let d = a.wind * Vec3::new(1., 0., 1.);
                    if a.resolved && d.length_squared() > 0.12 {
                        arrow(&mut pixels, x as f32, y as f32, d.x, d.z, ARROW);
                    }
                }
            }
        }
        arrow(
            &mut pixels,
            WIDTH as f32 / 2.,
            HEIGHT as f32 / 2.,
            request.yaw.sin(),
            request.yaw.cos(),
            [255, 218, 115, 255],
        );
    } else if let Some(weather) = weather {
        product_time = Some(weather.time);
        if mode == Mode::Onboard {
            // Ray bins are calculated once in polar coordinates. The cockpit
            // image then projects those gated returns to a forward fan.
            const AZ: usize = 81;
            const BINS: usize = 84;
            let mut fan = vec![CLEAR; AZ * BINS];
            let tilt = f64::from(controls.elevation);
            for az in 0..AZ {
                let relative = (az as f64 / (AZ - 1) as f64 - 0.5) * 110_f64.to_radians();
                scan_ray(
                    weather,
                    request.position,
                    bearing + relative,
                    tilt,
                    range,
                    &mut fan[az * BINS..(az + 1) * BINS],
                );
            }
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    let dx =
                        (f64::from(x) + 0.5 - f64::from(WIDTH) / 2.) / (f64::from(WIDTH) * 0.5);
                    let dz = 1. - (f64::from(y) + 0.5) / f64::from(HEIGHT);
                    let angle = dx.atan2(dz).to_degrees();
                    let dist = dx.hypot(dz);
                    if dist < 1. && angle.abs() <= 55. {
                        let az = ((angle / 110. + 0.5) * (AZ - 1) as f64).round() as usize;
                        let bin = (dist * BINS as f64) as usize;
                        let mut color = fan[az * BINS + bin];
                        if color == CLEAR && ((dist * 4.).fract() < 0.035 || angle.abs() < 0.65) {
                            color = [38, 64, 68, 255];
                        }
                        pixels[(y * WIDTH + x) as usize] = color;
                    }
                }
            }
        } else {
            // The vertical sweep uses the same pulse, finite beam width,
            // attenuation and terrain masking as the horizontal instrument.
            const ELEVATIONS: usize = 121;
            const BINS: usize = 116;
            const LIMIT: f64 = 76.;
            let az = bearing + (f64::from(controls.elevation) * 3.).to_radians();
            let base = request.position.y.min(600.) - 250.;
            let top = (base + 7600.).max(request.position.y + 2600.);
            profile_feet = [(base / FT).round() as i32, (top / FT).round() as i32];
            let slant_max = range.hypot((top - request.position.y).max(request.position.y - base));
            let mut beam = vec![CLEAR; ELEVATIONS * BINS];
            for elevation in 0..ELEVATIONS {
                let tilt = (elevation as f64 / (ELEVATIONS - 1) as f64 * 2. - 1.) * LIMIT;
                scan_ray(
                    weather,
                    request.position,
                    az,
                    tilt,
                    slant_max,
                    &mut beam[elevation * BINS..(elevation + 1) * BINS],
                );
            }
            for x in 0..WIDTH {
                let dist = (f64::from(x) + 0.5) / f64::from(WIDTH) * range;
                let p = request.position + ray(az, 0.) * dist;
                for y in 0..HEIGHT {
                    let h = top - (f64::from(y) + 0.5) / f64::from(HEIGHT) * (top - base);
                    let offset = h - request.position.y;
                    let tilt = offset.atan2(dist).to_degrees();
                    let elev =
                        ((tilt / LIMIT * 0.5 + 0.5) * (ELEVATIONS - 1) as f64).round() as usize;
                    let bin = ((dist.hypot(offset) / slant_max) * BINS as f64) as usize;
                    let color =
                        if weather.contains_horizontal(p) && h <= f64::from(weather.ground(p)) {
                            TERRAIN
                        } else if tilt.abs() > LIMIT || bin >= BINS {
                            coverage(x, y)
                        } else {
                            let c = beam[elev * BINS + bin];
                            if c == CLEAR && (h / 1500.).rem_euclid(1.) < 0.10 {
                                [35, 57, 66, 255]
                            } else {
                                c
                            }
                        };
                    pixels[(y * WIDTH + x) as usize] = color;
                }
            }
            let y = ((top - request.position.y) / (top - base) * f64::from(HEIGHT)) as u32;
            if y < HEIGHT {
                for x in 0..5 {
                    pixels[(y * WIDTH + x) as usize] = [255, 218, 115, 255];
                }
            }
        }
    } else {
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                pixels[(y * WIDTH + x) as usize] = coverage(x, y);
            }
        }
    }
    Image {
        pixels,
        controls,
        layer_feet,
        product_time,
        profile_feet,
    }
}

fn scan_ray(
    weather: &Snapshot,
    origin: DVec3,
    azimuth: f64,
    elevation: f64,
    range: f64,
    gates: &mut [[u8; 4]],
) {
    let directions =
        [-BEAM_HALF_WIDTH, 0., BEAM_HALF_WIDTH].map(|offset| ray(azimuth, elevation + offset));
    let mut transmission = 1.0_f32;
    let mut blocked = [false; 3];
    let spacing = range / gates.len() as f64;
    for (bin, gate) in gates.iter_mut().enumerate() {
        let distance = (bin as f64 + 0.5) * spacing;
        let mut sum = 0.;
        let mut rain = 0.;
        let mut covered = false;
        for ((direction, weight), blocked) in directions
            .into_iter()
            .zip([0.25, 0.5, 0.25])
            .zip(&mut blocked)
        {
            if *blocked {
                continue;
            }
            let p = origin + direction * distance;
            if weather.contains_horizontal(p) && p.y <= f64::from(weather.ground(p)) {
                *blocked = true;
                continue;
            }
            let air = weather.sample(p);
            if air.resolved {
                covered = true;
                sum += air.reflectivity() * weight;
                rain += air.rain_rate_mm_h() * weight;
            }
        }
        *gate = if blocked.into_iter().all(|v| v) {
            TERRAIN
        } else if !covered {
            UNKNOWN
        } else if transmission < 0.20 {
            SHADOW
        } else {
            echo_color(sum * transmission)
        };
        transmission *= (-(0.000004 + rain * 0.0000028) * (spacing as f32) * 2.).exp();
    }
}

fn coverage(x: u32, y: u32) -> [u8; 4] {
    if ((x + y) / 4) % 2 == 0 {
        UNKNOWN
    } else {
        [22, 32, 42, 255]
    }
}

fn ray(bearing: f64, tilt_degrees: f64) -> DVec3 {
    let (s, c) = bearing.sin_cos();
    let (y, xz) = tilt_degrees.to_radians().sin_cos();
    DVec3::new(s * xz, y, c * xz)
}

fn arrow(pixels: &mut [[u8; 4]], x: f32, y: f32, u: f32, v: f32, c: [u8; 4]) {
    let length = u.hypot(v).max(0.01);
    let (u, v) = (u / length, v / length);
    let tip = (x + u * 3., y + v * 3.);
    for (a, b) in [
        ((x - u * 2., y - v * 2.), tip),
        (tip, (x + v * 1.6, y - u * 1.6)),
        (tip, (x - v * 1.6, y + u * 1.6)),
    ] {
        for i in 0..8 {
            let t = i as f32 / 7.;
            let p = (a.0 + (b.0 - a.0) * t).round() as i32;
            let q = (a.1 + (b.1 - a.1) * t).round() as i32;
            if (0..WIDTH as i32).contains(&p) && (0..HEIGHT as i32).contains(&q) {
                pixels[q as usize * WIDTH as usize + p as usize] = c;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wind_mode_reports_uniform_wind_without_synthesizing_hazards() {
        let image = render(
            None,
            None,
            Request {
                yaw: 0.,
                position: DVec3::Y * 1000.,
                controls: Controls {
                    mode: Mode::Wind,
                    ..Default::default()
                },
            },
            Vec3::X * 10.,
        );
        assert_eq!(image.layer_feet, 3281);
        assert_eq!(image.pixels[0], image.pixels[10]);
        assert!(image.pixels.contains(&ARROW));
    }

    #[test]
    fn high_echo_requires_upward_tilt_and_heavy_rain_can_mask_the_far_gates() {
        let climate = super::super::Climate {
            seed: 7,
            regime: super::super::Regime::Clear,
            prevailing_wind: Vec3::ZERO,
        };
        let mut sim = super::super::Simulation::new(climate, DVec3::ZERO, 0., |_, _| 0.);
        for z in (SIZE[2] / 2 + 3)..(SIZE[2] / 2 + 15) {
            for y in 6..13 {
                for x in (SIZE[0] / 2 - 2)..(SIZE[0] / 2 + 3) {
                    sim.now[super::super::n(x, y, z)].rain = 0.007;
                }
            }
        }
        let weather = sim.snapshot();
        let mut flat = [[0; 4]; 64];
        let mut up = flat;
        scan_ray(&weather, DVec3::Y * 900., 0., 0., 6400., &mut flat);
        scan_ray(&weather, DVec3::Y * 900., 0., 12., 6400., &mut up);
        assert!(flat.iter().all(|&v| v == CLEAR));
        assert!(up.contains(&[193, 51, 179, 255]));
        assert!(up.contains(&SHADOW));
    }

    #[test]
    fn stabilized_beam_uses_elevation_with_respect_to_horizon() {
        let center = ray(1.3, 3.) * 10. * NM;
        let span = (ray(1.3, 3. + BEAM_HALF_WIDTH).y - ray(1.3, 3. - BEAM_HALF_WIDTH).y) * 10. * NM;
        assert!((center.y / FT - 3180.).abs() < 10.);
        assert!(span > 1000.);
    }
}
