//! Synthesized flight feedback. The audio thread owns the playback device and
//! smooths the aircraft state across blocks, keeping sound I/O off the render thread.
use crate::{aircraft, flight_feedback::Jolt, Camera};
use std::sync::{Arc, Mutex};

const RATE: u32 = 48_000;
const FRAMES: usize = 480;
const TAU: f32 = std::f32::consts::TAU;

#[derive(Clone, Copy, Default)]
struct Sound {
    prop_rate: [f32; 2],
    throttle: f32,
    airspeed: f32,
    tire_speed: f32,
    stall: bool,
    separation: f32,
    gust: f32,
    strain: f32,
    cockpit: bool,
    gear: f32,
    door: f32,
    retract: f32,
    flaps: f32,
    brake: f32,
}

#[derive(Default)]
struct Input {
    state: Sound,
    impact: bool,
    fracture: f32,
    jolt: Jolt,
}

pub(super) struct Audio {
    pub enabled: bool,
    channel: Option<Arc<Mutex<Input>>>,
}

impl Default for Audio {
    fn default() -> Self {
        Self {
            enabled: true,
            channel: None,
        }
    }
}

impl Audio {
    pub fn update(&mut self, plane: Option<&aircraft::Aircraft>, camera: Camera, jolt: Jolt) {
        let plane = plane.filter(|_| self.enabled);
        if plane.is_some() && self.channel.is_none() {
            let shared = Arc::new(Mutex::new(Input::default()));
            let receiver = Arc::downgrade(&shared);
            let _ = std::thread::Builder::new()
                .name("frontier flight sound".into())
                .spawn(move || {
                    if let Err(error) = playback::run(receiver) {
                        eprintln!("Flight sound unavailable: {error}");
                    }
                });
            self.channel = Some(shared);
        }
        if let Some(input) = &self.channel {
            let state = plane.map_or_else(Sound::default, |plane| {
                let pose = plane.presentation(camera);
                let (wing_separation, _) = plane.aerodynamic_state();
                let throttle = plane.effective_throttle(pose.pose.position.y, 1.);
                Sound {
                    prop_rate: plane.engine_rates(pose.pose.position.y),
                    throttle,
                    airspeed: plane.airspeed_mps(),
                    tire_speed: if pose.instruments.grounded {
                        plane.ground_speed_mps()
                    } else {
                        0.
                    },
                    stall: pose.instruments.stall_warning,
                    separation: 0.5 * (wing_separation[0] + wing_separation[1]),
                    gust: plane.feedback.gust(),
                    strain: plane
                        .damage
                        .as_ref()
                        .map_or(0., |d| d.structure.peak_utilization),
                    cockpit: plane.exterior_pose().is_none(),
                    gear: pose.gear_compression.into_iter().fold(0., f32::max),
                    door: pose.door,
                    retract: pose.gear,
                    flaps: pose.instruments.flaps_degrees / 30.,
                    brake: plane.brake_pressure(),
                }
            });
            let mut input = input.lock().unwrap();
            input.state = state;
            if plane.is_some() {
                input.jolt.keep_stronger(jolt);
            } else {
                input.jolt = Jolt::default();
            }
        }
    }

    pub fn impact(&self) {
        if let Some(input) = &self.channel {
            input.lock().unwrap().impact = self.enabled;
        }
    }

    pub fn fracture(&self, mass: f32) {
        if let Some(input) = self.channel.as_ref().filter(|_| self.enabled) {
            let mut input = input.lock().unwrap();
            input.fracture = input.fracture.max((mass / 30.).sqrt().clamp(0.16, 1.));
        }
    }
}

#[derive(Default)]
struct CabinRattle {
    strike: Jolt,
    age: usize,
}

impl CabinRattle {
    fn sample(&mut self, low: f32, mid: f32) -> f32 {
        if self.strike.strength == 0. {
            return 0.;
        }
        let time = self.age as f32 / RATE as f32;
        self.age += 1;
        if time > 0.23 {
            self.strike.strength = 0.;
            return 0.;
        }
        // A seat/fuselage thump excites the panel and loose cabin fittings.
        // The much shorter high-band taps decay before the lower body sound.
        let attack = (time / 0.005).min(1.);
        let body = (-time * 23.).exp()
            * (0.78 * (time * 84. * TAU).sin() + 0.22 * (time * 127. * TAU).sin() + 1.5 * low);
        let tap_one = (-time * 125.).exp();
        let second = (time - 0.033).max(0.);
        let tap_two = (second * 650.).min(1.) * (-second * 200.).exp() * 0.8;
        let hardware = (tap_one + tap_two) * (mid - low);
        self.strike.strength * attack * (0.45 * body + 0.66 * hardware)
    }
}

struct Mixer {
    phase: [f32; 2],
    horn_phase: f32,
    roll_phase: f32,
    engine_gain: [f32; 2],
    horn_gain: f32,
    wind_gain: f32,
    tire_gain: f32,
    buffet_gain: f32,
    servo_gain: f32,
    bump: f32,
    tearing: f32,
    creak_gain: f32,
    cabin: CabinRattle,
    previous: Sound,
    random: u32,
    low: f32,
    mid: f32,
    motor_rate: [f32; 2],
}

impl Default for Mixer {
    fn default() -> Self {
        Self {
            phase: [0., 1.],
            horn_phase: 0.,
            roll_phase: 0.,
            engine_gain: [0.; 2],
            horn_gain: 0.,
            wind_gain: 0.,
            tire_gain: 0.,
            buffet_gain: 0.,
            servo_gain: 0.,
            bump: 0.,
            tearing: 0.,
            creak_gain: 0.,
            cabin: CabinRattle::default(),
            previous: Sound::default(),
            random: 19,
            low: 0.,
            mid: 0.,
            motor_rate: [72.; 2],
        }
    }
}

impl Mixer {
    fn block(&mut self, sound: Sound, impact: bool, mut jolt: Jolt, samples: &mut [[i16; 2]]) {
        let gear_hit = (sound.gear - self.previous.gear - 0.004).max(0.) * 9.5;
        self.bump = self.bump.max(gear_hit.min(0.42));
        if impact {
            self.bump = self.bump.max(0.85);
        }
        if jolt.strength > 0. {
            jolt.strength *= if sound.cockpit { 1. } else { 0.22 };
            self.cabin = CabinRattle {
                strike: jolt,
                age: 0,
            };
        }
        let servo = ((sound.flaps - self.previous.flaps).abs() * 120.
            + (sound.door - self.previous.door).abs() * 38.
            + (sound.retract - self.previous.retract).abs() * 90.)
            .min(1.);
        self.previous = sound;
        let interior = if sound.cockpit { 0.64 } else { 1. };
        let engine = sound.prop_rate.map(|rate| {
            if rate > 0. {
                (0.16 + 0.16 * sound.throttle) * interior
            } else {
                0.
            }
        });
        let wind = (sound.airspeed / 76.).clamp(0., 1.25).powf(1.6)
            * (0.23 + sound.gust * 0.16)
            * interior;
        let tires = (sound.tire_speed / 20.).clamp(0., 1.) * (0.11 + 0.05 * sound.brake);
        let buffet = sound.separation * (sound.airspeed / 30.).clamp(0., 1.) * 0.31;
        let creak = ((sound.strain - 0.59) / 0.62).clamp(0., 1.) * 0.12 * interior;
        let horn = if sound.stall {
            if sound.cockpit {
                0.17
            } else {
                0.055
            }
        } else {
            0.
        };
        let dt = 1. / RATE as f32;

        for sample in samples {
            for (i, target) in engine.into_iter().enumerate() {
                self.engine_gain[i] += 0.0008 * (target - self.engine_gain[i]);
                self.motor_rate[i] += 0.0007 * (sound.prop_rate[i].max(72.) - self.motor_rate[i]);
                self.phase[i] = (self.phase[i] + self.motor_rate[i] * dt).rem_euclid(TAU);
            }
            self.wind_gain += 0.00055 * (wind - self.wind_gain);
            self.tire_gain += 0.001 * (tires - self.tire_gain);
            self.horn_gain += 0.003 * (horn - self.horn_gain);
            self.buffet_gain += 0.001 * (buffet - self.buffet_gain);
            self.creak_gain += 0.0007 * (creak - self.creak_gain);
            self.servo_gain += 0.001 * (servo * 0.10 - self.servo_gain);

            self.horn_phase = (self.horn_phase + 704. * TAU * dt).rem_euclid(TAU);
            self.roll_phase =
                (self.roll_phase + (19. + 2.2 * sound.tire_speed) * TAU * dt).rem_euclid(TAU);

            self.random = self
                .random
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            let noise = (self.random >> 8) as f32 / 8_388_608. - 1.;
            self.low += 0.038 * (noise - self.low);
            self.mid += 0.27 * (noise - self.mid);
            let motor: f32 = self
                .phase
                .into_iter()
                .zip(self.engine_gain)
                .map(|(phase, gain)| {
                    ((2. * phase).sin() * 0.50
                        + phase.sin() * 0.20
                        + (4. * phase).sin() * 0.19
                        + (6. * phase).sin() * 0.06
                        + self.low * 0.32)
                        * gain
                })
                .sum();
            let tire = (self.mid * 0.77 + self.roll_phase.sin() * 0.19) * self.tire_gain;
            let alert =
                (self.horn_phase.sin() + 0.22 * (3. * self.horn_phase).sin()) * self.horn_gain;
            let rumble = self.buffet_gain * self.low * (1.9 + 0.6 * self.roll_phase.sin());
            let impact = self.bump * (self.low * 1.6 + self.roll_phase.sin() * 0.46);
            self.bump *= 0.99959;
            let servo = self.servo_gain * ((7. * self.roll_phase).sin() * 0.47 + 0.25 * self.mid);
            let cabin = self.cabin.sample(self.low, self.mid);
            let creak = self.creak_gain
                * ((19. * self.roll_phase + self.low * 9.).sin() * 0.16 + self.mid * 0.56);
            let snap =
                self.tearing * (0.9 * self.mid + 0.43 * (noise - self.mid) + 0.53 * self.low);
            self.tearing *= 0.9991;
            let center = motor + tire + rumble + alert + impact + servo + cabin + creak + snap;
            let hiss = self.wind_gain * (1.35 * self.mid + 0.22 * (noise - self.mid));
            let side = self.wind_gain * 0.16 * (self.low - self.mid)
                - cabin * 0.42 * self.cabin.strike.pan;
            *sample = [pcm(center + hiss + side), pcm(center + hiss - side)];
        }
    }
}

fn pcm(value: f32) -> i16 {
    (value.clamp(-1., 1.) * 32767.) as i16
}

#[cfg(target_os = "linux")]
mod playback {
    use super::{Input, Mixer, FRAMES, RATE};
    use std::ffi::{c_char, c_int, c_void};
    use std::ptr::null;
    use std::sync::{Mutex, Weak};

    #[repr(C)]
    struct SampleSpec {
        format: c_int,
        rate: u32,
        channels: u8,
    }
    #[repr(C)]
    struct BufferAttrs {
        max_length: u32,
        target_length: u32,
        prebuffer: u32,
        min_request: u32,
        fragment_size: u32,
    }

    type Open = unsafe extern "C" fn(
        *const c_char,
        *const c_char,
        c_int,
        *const c_char,
        *const c_char,
        *const SampleSpec,
        *const c_void,
        *const BufferAttrs,
        *mut c_int,
    ) -> *mut c_void;
    type Write = unsafe extern "C" fn(*mut c_void, *const c_void, usize, *mut c_int) -> c_int;
    type Free = unsafe extern "C" fn(*mut c_void);

    struct Stream {
        ptr: *mut c_void,
        free: Free,
    }
    impl Drop for Stream {
        fn drop(&mut self) {
            // The worker owns this Pulse connection and drops it before the library.
            unsafe {
                (self.free)(self.ptr);
            }
        }
    }

    pub(super) fn run(channel: Weak<Mutex<Input>>) -> Result<(), Box<dyn std::error::Error>> {
        // The runtime library is optional. These signatures mirror pulse/simple.h.
        let lib = unsafe { libloading::Library::new("libpulse-simple.so.0")? };
        let open = unsafe { *lib.get::<Open>(b"pa_simple_new\0")? };
        let write = unsafe { *lib.get::<Write>(b"pa_simple_write\0")? };
        let free = unsafe { *lib.get::<Free>(b"pa_simple_free\0")? };
        let format = if cfg!(target_endian = "little") { 3 } else { 4 }; // PA_SAMPLE_S16NE
        let spec = SampleSpec {
            format,
            rate: RATE,
            channels: 2,
        };
        let attrs = BufferAttrs {
            max_length: u32::MAX,
            target_length: FRAMES as u32 * 4 * 3,
            prebuffer: u32::MAX,
            min_request: FRAMES as u32 * 4,
            fragment_size: u32::MAX,
        };
        let mut error = 0;
        let ptr = unsafe {
            open(
                null(),
                c"Frontier".as_ptr(),
                1,
                null(),
                c"Skyhawk".as_ptr(),
                &spec,
                null(),
                &attrs,
                &mut error,
            )
        };
        if ptr.is_null() {
            return Err(format!("PulseAudio/PipeWire connection error {error}").into());
        }
        let stream = Stream { ptr, free };
        let mut mixer = Mixer::default();
        let mut samples = [[0_i16; 2]; FRAMES];
        loop {
            let Some(shared) = channel.upgrade() else {
                break;
            };
            let (state, impact, jolt, fracture) = {
                let mut input = shared.lock().unwrap();
                (
                    input.state,
                    std::mem::take(&mut input.impact),
                    std::mem::take(&mut input.jolt),
                    std::mem::take(&mut input.fracture),
                )
            };
            drop(shared);
            mixer.tearing = mixer.tearing.max(fracture * 0.72);
            mixer.block(state, impact, jolt, &mut samples);
            // Pulse copies these PCM bytes before returning. Only this worker may block here.
            if unsafe {
                write(
                    stream.ptr,
                    samples.as_ptr().cast(),
                    std::mem::size_of_val(&samples),
                    &mut error,
                )
            } < 0
            {
                return Err(format!("PulseAudio/PipeWire write error {error}").into());
            }
        }
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
mod playback {
    pub(super) fn run(
        _: std::sync::Weak<std::sync::Mutex<super::Input>>,
    ) -> Result<(), &'static str> {
        Err("flight playback currently uses the Linux PulseAudio/PipeWire backend")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn engine_warning_and_impact_produce_distinct_bounded_audio() {
        let mut samples = [[0; 2]; 4 * FRAMES];
        let mut mixer = Mixer::default();
        let level = |pcm: &[[i16; 2]]| pcm.iter().map(|s| i64::from(s[0]).abs()).sum::<i64>();
        let silent = Sound::default();
        mixer.block(silent, false, Jolt::default(), &mut samples);
        assert_eq!(level(&samples), 0);
        let flight = Sound {
            prop_rate: [190., 0.],
            throttle: 0.6,
            airspeed: 47.,
            cockpit: true,
            ..silent
        };
        mixer.block(flight, false, Jolt::default(), &mut samples);
        let engine = level(&samples);
        let stall = Sound {
            stall: true,
            separation: 0.6,
            ..flight
        };
        mixer.block(stall, false, Jolt::default(), &mut samples);
        assert!(engine > 1_000_000 && level(&samples) > engine);
        mixer.block(silent, true, Jolt::default(), &mut samples);
        assert!(level(&samples) > 3_000_000);
        assert!(samples
            .iter()
            .all(|s| s[0].abs() < i16::MAX && s[1].abs() < i16::MAX));
    }

    #[test]
    fn cabin_jolt_is_directional_short_and_quieter_outside() {
        let pulse = Jolt {
            strength: 0.9,
            pan: -0.8,
        };
        let mut samples = [[0; 2]; 25 * FRAMES];
        let level = |channel: usize, pcm: &[[i16; 2]]| {
            pcm.iter().map(|s| i64::from(s[channel]).abs()).sum::<i64>()
        };
        Mixer::default().block(
            Sound {
                cockpit: true,
                ..Sound::default()
            },
            false,
            pulse,
            &mut samples,
        );
        let cabin = level(0, &samples);
        assert!(cabin > 1_000_000 && 10 * cabin > 18 * level(1, &samples));
        assert_eq!(level(0, &samples[24 * FRAMES..]), 0);
        assert!(samples
            .iter()
            .flatten()
            .all(|s| (-32767..32767).contains(s)));
        Mixer::default().block(Sound::default(), false, pulse, &mut samples);
        assert!(3 * level(0, &samples) < cabin);
    }
}
