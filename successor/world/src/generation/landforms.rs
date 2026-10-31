//! Continuous regional relief and geological provinces, independent of biomes.
use super::{
    environment::Geology,
    noise::{noise, octave_position, smoothstep},
};
#[derive(Clone, Copy)]
pub(super) struct LandscapeCell {
    pub weights: [f32; 5],
    pub geology: Geology,
}
impl Default for LandscapeCell {
    fn default() -> Self {
        Self {
            weights: [1., 0., 0., 0., 0.],
            geology: Geology::Granite,
        }
    }
}
pub(super) struct Landscape {
    pub height: f64,
    pub cell: LandscapeCell,
    pub soil: f64,
}
fn ridges(mut x: f64, mut z: f64, seed: u64) -> f64 {
    let (mut sum, mut norm, mut amplitude, mut wavelength, mut weight) = (0., 0., 1., 850., 1.);
    for octave in 0..5 {
        let r = 1. - noise(x, z, wavelength, seed ^ (100 + octave)).abs();
        let v = r * r * weight;
        sum += v * amplitude;
        norm += amplitude;
        weight = (v * 2.).min(1.);
        amplitude *= 0.48;
        wavelength *= 0.5;
        (x, z) = octave_position(x, z);
    }
    sum / norm
}
pub(super) fn landscape(x: f64, z: f64, seed: u64) -> Landscape {
    let province = super::provinces::province(x, z, seed);
    let wx = x + 130. * noise(x, z, 1800., seed ^ 21);
    let wz = z + 130. * noise(x + 513., z - 291., 1800., seed ^ 22);
    let rock_field = noise(wx + 231., wz - 317., 24_000., seed ^ 403);
    let geology = if province.belt > 0.65 && rock_field > 0.3 {
        Geology::Basalt
    } else if rock_field < -0.12 {
        Geology::Sandstone
    } else if rock_field > 0.18 {
        Geology::Limestone
    } else {
        Geology::Granite
    };
    let m = smoothstep(0.15, 0.80, province.belt);
    let p = province.plateau * (1. - m) * (1. - province.foothills * 0.7);
    let b = province.basin * (1. - m - p);
    let h = (province.hill_country.max(province.foothills * 0.85)) * (1. - m - p - b);
    let weights = [(1. - m - p - b - h).max(0.), h, p, b, m];
    let plain = 65. + 12. * noise(wx, wz, 6500., seed ^ 31) + 2. * noise(wx, wz, 900., seed ^ 32);
    let hills =
        100. + 65. * noise(wx, wz, 1700., seed ^ 51) + 12. * noise(wx + 37., wz, 430., seed ^ 52);
    let plateau =
        220. + 35. * noise(wx, wz, 3600., seed ^ 61) + 10. * noise(wx, wz, 900., seed ^ 62);
    let basin = 40. + 12. * noise(wx, wz, 6000., seed ^ 71);
    // The connected uplift survives local ridge noise. Small-scale detail may
    // sculpt a range, but may never invent isolated mountains in the plains.
    let ridge = ridges(wx, wz, seed);
    let backbone = 1450. + 280. * noise(wx, wz, 11_000., seed ^ 94);
    let uplift = province.belt * (backbone * (0.68 + 0.32 * ridge) + 190. * ridge);
    let height = weights
        .iter()
        .zip([plain, hills, plateau, basin, plain])
        .map(|(w, h)| w * h)
        .sum::<f64>()
        + uplift
        + 180. * province.foothills
        + 20. * noise(wx, wz, 38_000., seed ^ 81);
    let soil = (4.5 * weights[0] + 3. * h + 2.2 * p + 6. * b + 0.35 * m)
        * (0.85 + 0.3 * noise(wx, wz, 600., seed ^ 82));
    Landscape {
        height: height.max(4.),
        cell: LandscapeCell {
            weights: weights.map(|v| v as f32),
            geology,
        },
        soil: soil.max(0.12),
    }
}
