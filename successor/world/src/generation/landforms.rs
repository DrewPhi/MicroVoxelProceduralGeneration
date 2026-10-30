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
    let wx = x + 130. * noise(x, z, 1800., seed ^ 21);
    let wz = z + 130. * noise(x + 513., z - 291., 1800., seed ^ 22);
    let uplift = 0.5 + 0.9 * noise(wx, wz, 4200., seed ^ 11);
    let rock_field = noise(wx + 231., wz - 317., 2400., seed ^ 403);
    let geology = if uplift > 0.73 && rock_field > 0.05 {
        Geology::Basalt
    } else if rock_field < -0.12 {
        Geology::Sandstone
    } else if rock_field > 0.18 {
        Geology::Limestone
    } else {
        Geology::Granite
    };
    let m = smoothstep(0.58, 0.83, uplift);
    let p = smoothstep(-0.03, 0.35, noise(wx - 917., wz + 1201., 2700., seed ^ 42)) * (1. - m);
    let b = smoothstep(0.02, 0.37, noise(wx + 2153., wz - 731., 3300., seed ^ 43)) * (1. - m - p);
    let h =
        smoothstep(-0.18, 0.22, noise(wx + 57., wz + 527., 1900., seed ^ 44)) * (1. - m - p - b);
    let weights = [(1. - m - p - b - h).max(0.), h, p, b, m];
    let plain = 38. + 10. * noise(wx, wz, 2100., seed ^ 31) + 2. * noise(wx, wz, 400., seed ^ 32);
    let hills =
        85. + 55. * noise(wx, wz, 700., seed ^ 51) + 16. * noise(wx + 37., wz, 180., seed ^ 52);
    let raw_plateau =
        190. + 55. * noise(wx, wz, 1300., seed ^ 61) + 15. * noise(wx, wz, 370., seed ^ 62);
    let terrace = (raw_plateau / 28.).floor() * 28.
        + 28. * smoothstep(0.12, 0.88, (raw_plateau / 28.).fract());
    let plateau = 0.65 * raw_plateau + 0.35 * terrace;
    let basin =
        18. + 22. * noise(wx, wz, 1600., seed ^ 71).abs() + 4. * noise(wx, wz, 500., seed ^ 72);
    let ridge = ridges(wx, wz, seed);
    // Geology changes structure, not only its color. Physical bedding anchors
    // vary slowly; broad resistant benches survive to middle viewing distances.
    let mountain = match geology {
        Geology::Sandstone | Geology::Limestone => {
            let raw = 140. + 650. * ridge.powf(0.88);
            let thickness = 36. + 18. * noise(wx, wz, 900., seed ^ 93);
            let layer = (raw + 0.025 * wx - 0.018 * wz) / thickness;
            let bench = (layer.floor() + smoothstep(0.28, 0.68, layer.fract())) * thickness
                - 0.025 * wx
                + 0.018 * wz;
            0.58 * raw + 0.42 * bench
        }
        Geology::Basalt => 140. + 650. * smoothstep(0.05, 0.9, ridge),
        Geology::Granite => 140. + 650. * ridge.powf(1.12),
    };
    let heights = [plain, hills, plateau, basin, mountain];
    let height = weights.iter().zip(heights).map(|(w, h)| w * h).sum::<f64>()
        + 12. * noise(wx, wz, 5000., seed ^ 81);
    let soil = (4.5 * weights[0] + 3. * h + 2.2 * p + 6. * b + 0.55 * m)
        * (0.85 + 0.3 * noise(wx, wz, 600., seed ^ 82));
    Landscape {
        height: height.max(4.),
        cell: LandscapeCell {
            weights: weights.map(|v| v as f32),
            geology,
        },
        soil: soil.max(0.2),
    }
}
