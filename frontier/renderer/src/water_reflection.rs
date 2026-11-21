//! A small terrain panorama for water rays that leave the editable volume.
//! The near voxel ray still resolves individual banks, trees and structures.
use crate::{atmosphere::sun_direction, settings::VisualSettings};
use frontier_world::{Cell, FbmParams, FbmTerrain};
use glam::{DVec3, Vec3};
use std::{
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    time::Instant,
};

const DIRECTIONS: usize = 64;
const ELEVATIONS: usize = 16;
const RINGS: usize = 20;
const ANGLE_MAX: f64 = 1.3;
const FIRST_RANGE: f64 = 16.;
const RANGE_RATIO: f64 = 1.35;

#[derive(Clone, Copy, PartialEq)]
struct Key {
    seed: u64,
    params: FbmParams,
    vegetation: bool,
    anchor: [f64; 3],
    sun: [f32; 3],
    overcast: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Panorama {
    origin: [f32; 4],
    skyline: [f32; DIRECTIONS],
    // RGB radiance and logarithmic hit range. Each group of four words is
    // one WGSL uniform vec4, including across the azimuth wrap.
    color_range: [[u32; ELEVATIONS]; DIRECTIONS],
}

impl Panorama {
    pub(crate) fn enabled(&self) -> bool {
        self.origin[3] > 0.
    }

    fn blend(&mut self, old: &Self, t: f32) {
        for (new, old) in self.skyline.iter_mut().zip(old.skyline) {
            *new = old + (*new - old) * t;
        }
        for (new, old) in self
            .color_range
            .iter_mut()
            .flatten()
            .zip(old.color_range.iter().flatten())
        {
            let a = old.to_le_bytes();
            let b = new.to_le_bytes();
            *new = u32::from_le_bytes(std::array::from_fn(|i| {
                (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * t).round() as u8
            }));
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Sample {
    y: f64,
    color: Vec3,
}

fn build(terrain: &FbmTerrain, key: Key) -> Panorama {
    use bytemuck::Zeroable;
    let mut out = Panorama::zeroed();
    let ranges: [f64; RINGS] = std::array::from_fn(|i| FIRST_RANGE * RANGE_RATIO.powi(i as i32));
    let directions: [[f64; 2]; DIRECTIONS] = std::array::from_fn(|i| {
        let (x, z) = (std::f64::consts::TAU * i as f64 / DIRECTIONS as f64).sin_cos();
        [x, z]
    });
    let samples: [[Sample; RINGS]; DIRECTIONS] = std::array::from_fn(|i| {
        let [dx, dz] = directions[i];
        ranges.map(|distance| {
            let cell = Cell::from_meters([
                key.anchor[0] + dx * distance,
                0.,
                key.anchor[2] + dz * distance,
            ]);
            let (column, paint) = terrain.landscape_column(cell.x, cell.z, key.vegetation);
            Sample {
                y: (column.ground_top as f64 * frontier_world::voxel_size()) - key.anchor[1],
                color: Vec3::from_array(if column.visible_top > column.ground_top {
                    column.ground_surface.color()
                } else {
                    paint
                }),
            }
        })
    });
    let sun = Vec3::from_array(key.sun);
    let sunlight =
        Vec3::new(2.12, 1.44, 0.83).lerp(Vec3::new(2., 1.78, 1.46), smooth((sun.y - 0.06) / 0.58));
    for i in 0..DIRECTIONS {
        let [dx, dz] = directions[i];
        let mut maximum = -std::f64::consts::FRAC_PI_2;
        let mut next = 0;
        let mut last = 0u32;
        for (j, &r) in ranges.iter().enumerate() {
            let s = samples[i][j];
            let angle = s.y.atan2(r);
            if angle <= maximum {
                continue;
            }
            let before = j.saturating_sub(1);
            let after = (j + 1).min(RINGS - 1);
            let radial =
                (samples[i][after].y - samples[i][before].y) / (ranges[after] - ranges[before]);
            let tangent = (samples[(i + 1) % DIRECTIONS][j].y
                - samples[(i + DIRECTIONS - 1) % DIRECTIONS][j].y)
                / (2. * r * (std::f64::consts::TAU / DIRECTIONS as f64).sin());
            let n = Vec3::new(
                -(dx * radial + dz * tangent) as f32,
                1.,
                -(dz * radial - dx * tangent) as f32,
            )
            .normalize();
            let ambient =
                Vec3::new(0.12, 0.145, 0.075).lerp(Vec3::new(0.25, 0.33, 0.43), 0.5 + 0.5 * n.y);
            let rgb = s.color
                * (ambient * (1. + key.overcast * 0.3)
                    + sunlight * (sun.dot(n).max(0.) * (1. - 0.72 * key.overcast)));
            let [red, green, blue] = rgb
                .to_array()
                .map(|v| (v.clamp(0., 2.) * 127.5).round() as u32);
            // The last byte is a radial ring index, reconstructed in metres.
            last = red | green << 8 | blue << 16 | (j as u32) << 24;
            while next < ELEVATIONS && next as f64 * ANGLE_MAX / (ELEVATIONS - 1) as f64 <= angle {
                out.color_range[i][next] = last;
                next += 1;
            }
            maximum = angle;
        }
        out.skyline[i] = maximum as f32;
        out.color_range[i][next..].fill(last);
    }
    out
}

fn smooth(value: f32) -> f32 {
    let x = value.clamp(0., 1.);
    x * x * (3. - 2. * x)
}

// One worker retains the generator's thread-local drainage and climate caches
// while the player follows a river. Only one request can be outstanding.
struct Worker {
    request: Sender<(FbmTerrain, Key)>,
    result: Receiver<(Key, Panorama)>,
    busy: bool,
}

impl Worker {
    fn start() -> Option<Self> {
        let (request, jobs) = mpsc::channel::<(FbmTerrain, Key)>();
        let (done, result) = mpsc::channel();
        std::thread::Builder::new()
            .name("water reflections".into())
            .spawn(move || {
                for (terrain, key) in jobs {
                    if done.send((key, build(&terrain, key))).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Self {
            request,
            result,
            busy: false,
        })
    }
}

#[derive(Default)]
pub(crate) struct Reflections {
    active: Option<(Key, Panorama)>,
    worker: Option<Worker>,
    previous: Option<(Key, Panorama)>,
    published: Option<Instant>,
}

impl Reflections {
    pub(crate) fn prepare(
        &mut self,
        terrain: &FbmTerrain,
        vegetation: bool,
        eye: DVec3,
        anchor: [f64; 3],
        settings: &VisualSettings,
        capture: bool,
    ) -> Panorama {
        use bytemuck::Zeroable;
        let key = Key {
            seed: terrain.seed(),
            params: terrain.params(),
            vegetation,
            anchor,
            sun: sun_direction(settings.sun_elevation, settings.sun_azimuth).to_array(),
            overcast: settings.overcast,
        };
        if let Some(worker) = &mut self.worker {
            match worker.result.try_recv() {
                Ok(done) => {
                    if done.0 == key || self.active.as_ref().is_none_or(|(old, _)| *old != key) {
                        self.previous = self.active.replace(done);
                        self.published = Some(Instant::now());
                    }
                    worker.busy = false;
                }
                Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.active.as_ref().is_none_or(|(old, _)| *old != key) {
            if capture {
                self.active = Some((key, build(terrain, key)));
            } else {
                if self.worker.is_none() {
                    self.worker = Worker::start();
                }
                if let Some(w) = self.worker.as_mut().filter(|w| !w.busy) {
                    w.busy = w.request.send((terrain.clone(), key)).is_ok();
                }
            }
        }
        let Some((installed, mut panorama)) = self.active else {
            return Panorama::zeroed();
        };
        if installed.seed != key.seed
            || installed.params != key.params
            || installed.vegetation != vegetation
        {
            return Panorama::zeroed();
        }
        let mut anchor = DVec3::from_array(installed.anchor);
        let mut strength = 1.;
        let t = if capture {
            1.
        } else {
            self.published
                .map_or(1., |time| smooth(time.elapsed().as_secs_f32() / 0.30))
        };
        if t < 1. {
            if let Some((before, old)) = &self.previous {
                let previous_anchor = DVec3::from_array(before.anchor);
                if before.seed == key.seed
                    && before.params == key.params
                    && before.vegetation == vegetation
                    && previous_anchor.distance(anchor) < 100.
                {
                    panorama.blend(old, t);
                    anchor = previous_anchor.lerp(anchor, f64::from(t));
                } else {
                    strength = t;
                }
            } else {
                strength = t;
            }
        } else {
            self.previous = None;
        }
        panorama.origin = (anchor - eye).as_vec3().extend(strength).to_array();
        panorama
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    #[test]
    fn transition_blends_angular_radiance_without_carrying_between_channels() {
        let mut a = Panorama::zeroed();
        let mut b = a;
        a.color_range[63][15] = u32::from_le_bytes([250, 41, 12, 4]);
        b.color_range[63][15] = u32::from_le_bytes([10, 65, 84, 18]);
        a.skyline[63] = 0.25;
        b.skyline[63] = 0.75;
        b.blend(&a, 0.5);
        assert_eq!(b.color_range[63][15].to_le_bytes(), [130, 53, 48, 11]);
        assert_eq!(b.skyline[63], 0.5);
        assert_eq!(std::mem::size_of::<Panorama>(), 16 + 64 * 4 + 64 * 16 * 4);
    }

    #[test]
    fn a_flat_sea_has_an_open_skyline_in_every_direction() {
        let params = FbmParams {
            recipe: frontier_world::FbmRecipe::Regional,
            amplitude: 0.,
            ..Default::default()
        };
        let terrain = FbmTerrain::new(7, params).unwrap();
        let key = Key {
            seed: 7,
            params,
            vegetation: false,
            anchor: [-124., 0.125, 8.],
            sun: [0., 1., 0.],
            overcast: 0.,
        };
        let panorama = build(&terrain, key);
        let furthest = FIRST_RANGE * RANGE_RATIO.powi(RINGS as i32 - 1);
        let horizon = (-0.125_f64).atan2(furthest) as f32;
        assert!(panorama.skyline.iter().all(|h| (*h - horizon).abs() < 1e-6));
    }
}
