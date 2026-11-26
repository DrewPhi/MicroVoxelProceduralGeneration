//! Sky lighting and a procedural lunar voxel volume in sky-local coordinates.
use glam::Vec3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub sun: Vec3,
    pub moon: Vec3,
    pub daylight: f32,
    pub twilight: f32,
    pub moonlight: f32,
}

impl Light {
    pub fn at(day: f64) -> Self {
        Self::orbit_at(day, 38., 10_f32.to_radians())
    }

    pub fn regional(day: f64, seconds: f64, latitude_deg: f32) -> Self {
        let declination = frontier_runtime::weather::regional::solar_declination(seconds / 86400.);
        Self::orbit_at(day, latitude_deg, declination)
    }

    fn orbit_at(day: f64, latitude: f32, declination: f32) -> Self {
        let hour = ((day.rem_euclid(1.) - 0.5) * std::f64::consts::TAU) as f32;
        let orbit = |angle: f32, declination: f32| {
            let (sin, cos) = angle.sin_cos();
            let (sd, cd) = declination.sin_cos();
            let (sl, cl) = latitude.to_radians().sin_cos();
            Vec3::new(-cd * sin, sl * sd + cl * cd * cos, sl * cd * cos - cl * sd)
        };
        let elongation =
            ((day.rem_euclid(29.53) / 29.53 + 0.44).rem_euclid(1.) * std::f64::consts::TAU) as f32;
        let sun = orbit(hour, declination);
        let moon = orbit(hour - elongation, declination + 0.092 * elongation.sin());
        let daylight = smooth(-0.10, 0.25, sun.y);
        let twilight = smooth(-0.22, 0.025, sun.y) * (1. - smooth(0.01, 0.27, sun.y));
        let moonlight = (0.5 - 0.5 * sun.dot(moon)) * smooth(-0.02, 0.24, moon.y) * (1. - daylight);
        Self {
            sun,
            moon,
            daylight,
            twilight,
            moonlight,
        }
    }

    pub fn terrain_direction(self) -> Vec3 {
        let moon = Vec3::new(self.moon.x, self.moon.y.max(0.07), self.moon.z).normalize();
        let sun = Vec3::new(self.sun.x, self.sun.y.max(0.07), self.sun.z).normalize();
        moon.lerp(sun, smooth(-0.09, 0.085, self.sun.y)).normalize()
    }
}

fn smooth(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

pub const MOON_SIZE: u32 = 56;

/// Solid voxels carry a grayscale albedo. Crater bowls and raised rims modify
/// the actual occupied boundary, so they shade and occlude as the light moves.
pub fn moon_voxels() -> &'static [u8] {
    static VOLUME: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    VOLUME.get_or_init(|| {
        let n = MOON_SIZE as usize;
        let mut craters = Vec::new();
        for i in 0..58 {
            let y = 1. - (i as f32 + 0.5) / 29.;
            let theta = i as f32 * 2.3999631;
            let r = (1. - y * y).sqrt();
            let center = Vec3::new(r * theta.cos(), y, r * theta.sin());
            let weight = ((i * 17 + 27) % 59) as f32 / 58.;
            craters.push((
                center,
                0.069 + 0.22 * weight.powi(3),
                0.020 + 0.048 * weight.powi(2),
            ));
        }
        for (p, width, depth) in [
            (Vec3::new(-0.33, 0.26, -0.91), 0.22, 0.076),
            (Vec3::new(0.31, -0.42, -0.85), 0.21, 0.077),
            (Vec3::new(0.42, 0.27, -0.86), 0.15, 0.052),
        ] {
            craters.push((p.normalize(), width, depth));
        }
        (0..n * n * n)
            .map(|i| {
                let p = Vec3::new((i % n) as f32, (i / n % n) as f32, (i / (n * n)) as f32);
                let p = (p + 0.5) / (n as f32 * 0.5) - 1.;
                let r = p.length();
                if r > 0.98 {
                    return 0;
                }
                if r < 0.79 {
                    return 146;
                }
                let d = p / r;
                let mut surface = 0.94;
                let mut rim = 0.;
                for &(center, width, depth) in &craters {
                    let distance = d.distance(center) / width;
                    let bowl = 1. - smooth(0.18, 0.94, distance);
                    let lip = smooth(0.73, 0.93, distance) * (1. - smooth(0.96, 1.24, distance));
                    surface += depth * (0.35 * lip - bowl);
                    rim += 0.12 * lip;
                }
                let maria = smooth(0.55, 0.95, d.dot(Vec3::new(-0.48, 0.32, -0.82))).max(smooth(
                    0.72,
                    0.99,
                    d.dot(Vec3::new(0.41, -0.39, -0.82)),
                ));
                let mottling =
                    (d.x * 28. + (d.z * 17.).sin()).sin() * (d.y * 21. + d.z * 12.).sin();
                if r > surface {
                    0
                } else {
                    ((0.70 - 0.36 * maria + 0.075 * mottling + rim).clamp(0.16, 0.95) * 255.) as u8
                }
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sunlight_moves_east_to_west_and_moon_rises_at_night() {
        assert!(Light::at(0.25).sun.x > 0.9);
        assert!(Light::at(0.75).sun.x < -0.9);
        assert!(Light::at(0.5).daylight > 0.98);
        assert_eq!(Light::at(0.).daylight, 0.);
        assert!(Light::at(0.).moon.y > 0.3);
        let moon = moon_voxels();
        assert_eq!(moon.len(), MOON_SIZE.pow(3) as usize);
        assert!(moon.contains(&0) && moon.iter().filter(|&&v| v > 0).count() > 60_000);
    }
}
