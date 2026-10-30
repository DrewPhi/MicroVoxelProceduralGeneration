//! Deterministic gradient noise shared by regional generation stages.
pub(super) fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

pub(super) fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + t * (b - a)
}
pub(super) fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
pub(super) fn smoothstep(a: f64, b: f64, value: f64) -> f64 {
    let t = ((value - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(super) fn gradient_dot(x: i64, z: i64, dx: f64, dz: f64, seed: u64) -> f64 {
    let hash = mix(seed ^ mix(x as u64) ^ mix(z as u64).rotate_left(32));
    let d = std::f64::consts::FRAC_1_SQRT_2;
    match hash & 7 {
        0 => dx,
        1 => -dx,
        2 => dz,
        3 => -dz,
        4 => (dx + dz) * d,
        5 => (dx - dz) * d,
        6 => (-dx + dz) * d,
        _ => (-dx - dz) * d,
    }
}

pub(super) fn perlin(x: f64, z: f64, seed: u64) -> f64 {
    let gx = x.floor() as i64;
    let gz = z.floor() as i64;
    let dx = x - x.floor();
    let dz = z - z.floor();
    let top = lerp(
        gradient_dot(gx, gz, dx, dz, seed),
        gradient_dot(gx + 1, gz, dx - 1.0, dz, seed),
        fade(dx),
    );
    let bottom = lerp(
        gradient_dot(gx, gz + 1, dx, dz - 1.0, seed),
        gradient_dot(gx + 1, gz + 1, dx - 1.0, dz - 1.0, seed),
        fade(dx),
    );
    lerp(top, bottom, fade(dz)) * std::f64::consts::SQRT_2
}

pub(super) fn noise(x: f64, z: f64, wavelength: f64, seed: u64) -> f64 {
    perlin(x / wavelength, z / wavelength, seed)
}

// Rotated, offset octaves avoid lining up every layer's lattice corners.
pub(super) fn octave_position(x: f64, z: f64) -> (f64, f64) {
    (0.8 * x - 0.6 * z + 19.19, 0.6 * x + 0.8 * z + 73.73)
}
