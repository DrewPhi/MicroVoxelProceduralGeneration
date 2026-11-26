use glam::{Mat3, Quat, Vec3};

#[derive(Clone, Copy, Default)]
pub(super) struct Jolt {
    pub strength: f32,
    pub pan: f32,
}

impl Jolt {
    pub fn keep_stronger(&mut self, other: Self) {
        if other.strength > self.strength {
            *self = other;
        }
    }
}

pub(super) struct Feedback {
    pub camera_motion: bool,
    attitude: Option<Quat>,
    eye: Vec3,
    eye_velocity: Vec3,
    load_average: Option<Vec3>,
    swing_average: Vec3,
    gust: f32,
    recent_intensity: f32,
    jolt_cooldown: f32,
    pending_jolt: Jolt,
    pub jolts: u32,
}

impl Default for Feedback {
    fn default() -> Self {
        Self {
            camera_motion: true,
            attitude: None,
            eye: Vec3::ZERO,
            eye_velocity: Vec3::ZERO,
            load_average: None,
            swing_average: Vec3::ZERO,
            gust: 0.,
            recent_intensity: 0.,
            jolt_cooldown: 0.,
            pending_jolt: Jolt::default(),
            jolts: 0,
        }
    }
}

impl Feedback {
    pub fn advance(&mut self, body: Mat3, seat_load_g: Vec3, swing: Vec3, airborne: bool, dt: f32) {
        let body = Quat::from_mat3(&body);
        let mut lag = (body.conjugate() * self.attitude.unwrap_or(body)).normalize();
        if lag.w < 0. {
            lag = -lag;
        }
        // Damp motion in aircraft axes. The pilot follows sustained maneuvers,
        // with more freedom around the roll axis than around pitch or yaw.
        let error = lag.to_scaled_axis() * Vec3::new(0.14, 0.13, 0.23).map(|t| (-dt / t).exp());
        self.attitude = Some(
            (body * Quat::from_scaled_axis(error.clamp_length_max(9_f32.to_radians()))).normalize(),
        );

        let average = self.load_average.get_or_insert(seat_load_g);
        *average = average.lerp(seat_load_g, 1. - (-dt / 0.42).exp());
        self.swing_average = self.swing_average.lerp(swing, 1. - (-dt / 0.24).exp());
        let load = seat_load_g - *average;
        let swing = swing - self.swing_average;

        // A restrained, critically damped head response to changes in seat load.
        let limit = Vec3::new(0.035, 0.065, 0.038);
        let target = (-load * Vec3::new(0.13, 0.10, 0.12)).clamp(-limit, limit);
        let displacement = self.eye - target;
        let frequency = 22.;
        let decay = (-frequency * dt).exp();
        let impulse = self.eye_velocity + frequency * displacement;
        self.eye = target + (displacement + impulse * dt) * decay;
        self.eye_velocity = (self.eye_velocity - frequency * impulse * dt) * decay;

        if !airborne {
            self.gust = 0.;
            self.recent_intensity = 0.;
            self.jolt_cooldown = 0.;
            return;
        }
        // Direction and onset come from the changing load, including asymmetric
        // wing lift. The baseline catches up during a sustained turn or climb.
        let intensity = (load * Vec3::new(1.3, 1., 0.65)).length() + 0.16 * swing.length();
        self.gust = ((intensity - 0.035) * 2.5).clamp(0., 1.);
        self.jolt_cooldown = (self.jolt_cooldown - dt).max(0.);
        let onset = intensity - self.recent_intensity;
        self.recent_intensity += (1. - (-dt / 0.15).exp()) * onset;
        // A new strike can stand out even if there was no lull between gusts.
        if intensity > 0.17 && onset > 0.10 && self.jolt_cooldown == 0. {
            self.jolt_cooldown = 0.52;
            self.jolts += 1;
            self.pending_jolt.keep_stronger(Jolt {
                strength: (0.28 + 1.1 * (intensity - 0.17)).min(1.),
                pan: (-3. * load.x + 0.65 * swing.z).clamp(-1., 1.),
            });
        }
    }

    pub fn frame(&self, body: Mat3) -> Mat3 {
        match self.attitude {
            Some(attitude) if self.camera_motion => Mat3::from_quat(attitude),
            _ => body,
        }
    }

    pub fn eye(&self) -> Vec3 {
        if self.camera_motion {
            self.eye
        } else {
            Vec3::ZERO
        }
    }

    pub fn gust(&self) -> f32 {
        self.gust
    }

    pub fn take_jolt(&mut self) -> Jolt {
        std::mem::take(&mut self.pending_jolt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapid_bank_moves_the_airframe_ahead_of_the_camera_then_settles() {
        let mut feedback = Feedback::default();
        let dt = 1. / 120.;
        feedback.advance(Mat3::IDENTITY, Vec3::Y, Vec3::ZERO, true, dt);
        // Cross upside down so headings and roll may wrap in the rendered camera.
        for tick in 1..=204 {
            let body = Mat3::from_rotation_z(tick as f32 * 0.018);
            feedback.advance(body, Vec3::Y, Vec3::Z * 2.16, true, dt);
            let gap = feedback.frame(body).y_axis.angle_between(body.y_axis);
            if tick > 40 {
                assert!((0.09..0.160).contains(&gap), "{gap}");
            }
        }
        let body = Mat3::from_rotation_z(204. * 0.018);
        feedback.camera_motion = false;
        assert_eq!(feedback.frame(body), body);
        feedback.camera_motion = true;
        for _ in 0..240 {
            feedback.advance(body, Vec3::Y, Vec3::ZERO, true, dt);
        }
        assert!(feedback.frame(body).y_axis.abs_diff_eq(body.y_axis, 0.001));
    }

    #[test]
    fn airborne_load_moves_the_head_and_produces_one_directional_cue() {
        let mut feedback = Feedback::default();
        let dt = 1. / 120.;
        let body = Mat3::IDENTITY;
        feedback.advance(body, Vec3::Y, Vec3::ZERO, true, dt);
        let gust = Vec3::new(0.22, 1.48, 0.);
        for _ in 0..18 {
            feedback.advance(body, gust, Vec3::ZERO, true, dt);
        }
        assert!(feedback.eye().y < -0.02);
        assert!(feedback.gust() > 0.5);
        let jolt = feedback.take_jolt();
        assert!(jolt.strength > 0.5 && jolt.pan < -0.5);
        for _ in 0..240 {
            feedback.advance(body, gust, Vec3::ZERO, true, dt);
        }
        assert_eq!(feedback.jolts, 1);
        assert!(feedback.eye().length() < 0.002);
        assert_eq!(feedback.take_jolt().strength, 0.);

        feedback.advance(body, Vec3::Y, Vec3::ZERO, false, dt);
        assert_eq!(feedback.gust(), 0.);
        assert_eq!(feedback.take_jolt().strength, 0.);
    }

    #[test]
    fn repeated_gusts_trigger_distinct_strikes_without_a_lull() {
        let mut feedback = Feedback::default();
        let dt = 1. / 120.;
        for tick in 0..132 {
            // Keep a shifting lateral load between sharp vertical gusts.
            let time = tick as f32 * dt;
            let lateral = 0.14 * (time * 19.).sin();
            let hit = if (24..31).contains(&tick) || (91..99).contains(&tick) {
                0.36
            } else {
                0.
            };
            feedback.advance(
                Mat3::IDENTITY,
                Vec3::new(lateral, 1. + hit, 0.),
                Vec3::ZERO,
                true,
                dt,
            );
        }
        assert!(
            feedback.jolts >= 2 && feedback.jolts <= 5,
            "{}",
            feedback.jolts
        );
    }
}
