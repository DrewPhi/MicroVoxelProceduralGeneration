//! Coherent surface response to the final refined landform, in source meters.
//! At world scale 10, ten source meters represent one game meter. No weather,
//! height generation, per-voxel random selection, or face-dependent lighting.
use crate::generation::{
    landscape_paint::{self, Context, Paint},
    noise::{noise, smoothstep},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct Local {
    /// Final height derivatives (source meters / source meter).
    pub slope: [f64; 2],
    /// Height Laplacian in inverse source meters; positive means concave ground.
    pub curvature: f64,
    /// Unsigned distance to river centerline, in source meters; infinity if absent.
    pub river_distance: f64,
    /// Full river width in source meters; zero means no local river.
    pub river_width: f64,
    /// Additional deposited sediment thickness in source meters.
    pub deposit: f64,
}

fn filtered(p: [f64; 2], wave: f64, footprint: f64, seed: u64) -> f64 {
    let visible = smoothstep(1., 3., wave / footprint.max(0.001));
    if visible == 0. {
        0.
    } else {
        visible * noise(p[0], p[1], wave, seed)
    }
}

fn linear(rgb: [u8; 3]) -> [f32; 3] {
    rgb.map(|v| {
        let s = v as f64 / 255.;
        if s <= 0.04045 {
            (s / 12.92) as f32
        } else {
            ((s + 0.055) / 1.055).powf(2.4) as f32
        }
    })
}

// Snow reduces local overlay strength. Apply exactly the same mixture to every
// coverage component and RGB so partially snowy ground remains consistent.
fn overlay(paint: &mut Paint, category: usize, amount: f64, rgb: [u8; 3]) {
    let amount = amount.clamp(0., 1.) as f32;
    let available = 1. - paint.coverage[3];
    let mixture = amount * available;
    for weight in &mut paint.coverage {
        *weight *= 1. - mixture;
    }
    paint.coverage[category] += mixture;
    let palette = linear(rgb);
    for i in 0..3 {
        paint.linear_rgb[i] =
            paint.linear_rgb[i] * (1. - amount * available) + palette[i] * amount * available;
    }
}

/// Coordinates, sampling footprint and descriptor lengths are source meters.
/// The five coverage categories match `landscape_paint::Paint` exactly.
pub fn sample(p: [f64; 2], footprint: f64, seed: u64, mut c: Context, local: Local) -> Paint {
    c.gradient = local.slope;
    c.sediment_m += local.deposit.max(0.);
    let mut paint = landscape_paint::sample(p, c, footprint, seed);
    let patch = 0.75 * filtered(p, 150., footprint, seed ^ 0xabc113)
        + 0.25 * filtered(p, 30., footprint, seed ^ 0xabc114);
    let slope = local.slope[0].hypot(local.slope[1]);
    let concave = smoothstep(0., 0.025, local.curvature);
    let sediment = smoothstep(0.2, 6., c.sediment_m);
    let bare = smoothstep(0.35, 1.15, slope + patch * 0.06) * (1. - 0.65 * sediment);
    overlay(&mut paint, 1, bare * 0.65, [143, 139, 125]);
    let bank = if local.river_width > 0. {
        let edge_distance = (local.river_distance.abs() - local.river_width * 0.5).max(0.);
        1. - smoothstep(0., (local.river_width * 0.8).max(12.), edge_distance)
    } else {
        0.
    };
    let gentle = 1. - smoothstep(0.2, 0.75, slope);
    let soil = gentle * (sediment * (0.20 + concave * 0.20) + bank * 0.55);
    // Dry banks expose pale gravel; moisture darkens fine sediment and supports
    // stable wet-ground cover, without treating every drainage line as wet.
    let moist = smoothstep(0.25, 0.75, c.moisture);
    let dry_gravel = [171, 158, 130];
    let damp_soil = [111, 103, 79];
    let soil_rgb = std::array::from_fn(|i| {
        (dry_gravel[i] as f64 * (1. - moist) + damp_soil[i] as f64 * moist).round() as u8
    });
    overlay(&mut paint, 2, soil, soil_rgb);
    overlay(&mut paint, 4, bank * gentle * moist * 0.42, [74, 94, 62]);
    let variation = (1. + patch * 0.045) as f32;
    paint.linear_rgb = paint.linear_rgb.map(|v| (v * variation).clamp(0., 1.));
    paint
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context(moisture: f64) -> Context {
        Context {
            temperature: 14.,
            warm_temperature: 22.,
            moisture,
            gradient: [0.; 2],
            sediment_m: 0.,
            drainage_m2: 0.,
            water_depth_m: 0.,
        }
    }
    #[test]
    fn coverage_is_normalized_and_geometry_exposes_rock() {
        for i in 0..100 {
            let local = Local {
                slope: [i as f64 / 50., 0.],
                deposit: i as f64 / 10.,
                river_width: 20.,
                river_distance: i as f64,
                ..Local::default()
            };
            let p = sample([32., 71.], 0.625, 42, context(0.5), local);
            assert!((p.coverage.iter().sum::<f32>() - 1.).abs() < 1e-6);
            assert!(p.coverage.iter().all(|v| *v >= 0.));
        }
        let flat = sample([32., 71.], 1., 42, context(0.5), Local::default());
        let steep = sample(
            [32., 71.],
            1.,
            42,
            context(0.5),
            Local {
                slope: [1., 0.],
                ..Local::default()
            },
        );
        assert!(steep.coverage[1] > flat.coverage[1]);
    }
    #[test]
    fn dry_and_wet_banks_are_distinct() {
        let bank = Local {
            river_width: 20.,
            river_distance: 12.,
            deposit: 2.,
            ..Local::default()
        };
        let dry = sample([32., 71.], 1., 42, context(0.05), bank);
        let wet = sample([32., 71.], 1., 42, context(0.9), bank);
        assert!(wet.coverage[4] > dry.coverage[4]);
        assert_ne!(dry.linear_rgb, wet.linear_rgb);
        let far = sample(
            [32., 71.],
            1.,
            42,
            context(0.05),
            Local {
                river_distance: 1000.,
                ..bank
            },
        );
        assert!(dry.coverage[2] > far.coverage[2]);
    }
    #[test]
    fn filtering_removes_unresolved_variation() {
        assert_eq!(filtered([17., 33.], 30., 100., 42), 0.);
        assert_eq!(filtered([17., 33.], 150., 200., 42), 0.);
        let a = sample([17., 33.], 1., 42, context(0.5), Local::default());
        let b = sample([17., 33.], 1., 42, context(0.5), Local::default());
        assert_eq!(a.linear_rgb, b.linear_rgb);
    }
}
