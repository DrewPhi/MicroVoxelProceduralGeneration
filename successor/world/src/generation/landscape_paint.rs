//! Surface painting for the landscape experiment. No changes to height or flow.
//! Coverage varies continuously in physical space; each native voxel gets one RGB.
//! Coarse previews filter sub-footprint variation, not independent cell confetti.
use super::{
    climate,
    noise::{mix, noise, smoothstep},
    TerrainConfig,
};
use crate::{CellPos, GridScale};

#[derive(Clone, Copy, Debug)]
pub struct Context {
    pub temperature: f64,
    pub warm_temperature: f64,
    pub moisture: f64,
    pub gradient: [f64; 2],
    pub sediment_m: f64,
    pub drainage_m2: f64,
    pub water_depth_m: f64,
}
#[derive(Clone, Copy, Debug)]
pub struct Paint {
    pub linear_rgb: [f32; 3],
    /// Vegetated ground, bare rock, deposited soil, alpine snow, wet ground.
    pub coverage: [f32; 5],
}
fn linear(rgb: [u8; 3]) -> [f64; 3] {
    rgb.map(|v| {
        let s = v as f64 / 255.;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    })
}
fn blend(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    std::array::from_fn(|i| a[i] * (1. - t) + b[i] * t)
}
fn filtered_noise(p: [f64; 2], wavelength: f64, footprint: f64, seed: u64) -> f64 {
    let visible = smoothstep(1., 3., wavelength / footprint.max(0.001));
    if visible == 0. {
        0.
    } else {
        visible * noise(p[0], p[1], wavelength, seed)
    }
}

/// One solid albedo, independent of face orientation and lighting.
/// Context is sampled/interpolated by the landscape provider, not recalculated here.
pub fn sample(p: [f64; 2], c: Context, footprint: f64, seed: u64) -> Paint {
    let n = |wave, salt| filtered_noise(p, wave, footprint, seed ^ salt);
    let patch = 0.65 * n(1800., 1001) + 0.25 * n(180., 1002) + 0.10 * n(18., 1003);
    let slope = c.gradient[0].hypot(c.gradient[1]);
    // North/south exposure is a gentle climate adjustment, never baked-in lighting.
    let exposure = (c.gradient[1] / (1. + slope)).clamp(-1., 1.);
    let moist = (c.moisture + 0.07 * patch).clamp(0., 1.);
    let cold = 1. - smoothstep(1., 9., c.temperature + 0.8 * patch);
    let dry_cover = blend(
        linear([185, 159, 113]),
        linear([153, 148, 91]),
        smoothstep(0.06, 0.28, moist),
    );
    let grass = blend(
        dry_cover,
        linear([72, 112, 61]),
        smoothstep(0.22, 0.64, moist),
    );
    let grass = blend(grass, linear([139, 137, 104]), cold * 0.65);
    let rock = blend(
        linear([155, 145, 125]),
        linear([131, 143, 147]),
        (0.5 + 0.45 * n(12000., 1010)).clamp(0., 1.),
    );
    let deposit = blend(
        linear([163, 146, 110]),
        linear([119, 110, 83]),
        smoothstep(0.25, 0.65, moist),
    );
    // Soil accumulation moderates exposure without turning every high-flow cell blue.
    let soil = smoothstep(0.4, 8., c.sediment_m);
    let bare = smoothstep(0.25, 0.90, slope + 0.07 * patch) * (1. - soil * 0.55);
    let alpine_bare = cold * 0.32 * (1. - soil);
    let rock_weight = 1. - (1. - bare) * (1. - alpine_bare);
    let alluvial = soil * (1. - smoothstep(0.04, 0.30, slope)) * (0.12 + 0.32 * (1. - moist));
    let valley = smoothstep(1e6, 20e6, c.drainage_m2) * (1. - smoothstep(0.03, 0.22, slope));
    let wet = valley * smoothstep(0.25, 0.6, moist) * 0.32;
    // Broad melt transition plus exposure and coherent variation avoids contour stripes.
    let snow = (1. - smoothstep(1., 6., c.warm_temperature + 1.8 * exposure + 1.2 * patch))
        * (1. - smoothstep(0.65, 1.5, slope))
        * smoothstep(0.12, 0.45, moist);
    let rest = 1. - snow;
    let weights = [
        rest * (1. - rock_weight) * (1. - alluvial) * (1. - wet),
        rest * rock_weight,
        rest * (1. - rock_weight) * alluvial,
        snow,
        rest * (1. - rock_weight) * (1. - alluvial) * wet,
    ];
    let palettes = [
        grass,
        rock,
        deposit,
        linear([227, 234, 230]),
        linear([69, 100, 68]),
    ];
    let mut color = [0.; 3];
    for (weight, palette) in weights.iter().zip(palettes) {
        for channel in 0..3 {
            color[channel] += weight * palette[channel];
        }
    }
    // Wavelengths are physical and suppressed below the sampling footprint.
    let variation = 1.
        + 0.08 * n(3200., 1021)
        + 0.035 * n(240., 1022)
        + 0.025 * n(12., 1023)
        + 0.015 * n(0.4, 1024);
    for channel in &mut color {
        *channel = (*channel * variation).clamp(0., 1.);
    }
    Paint {
        linear_rgb: color.map(|v| v as f32),
        coverage: weights.map(|v| v as f32),
    }
}

pub fn voxel_rgb(pos: CellPos, grid: GridScale, c: Context, seed: u64) -> [u8; 3] {
    // The provider supplies represented water depth. Sub-voxel films do not
    // repaint a land voxel. Every face uses this same water albedo when occupied.
    if c.water_depth_m >= grid.meters() {
        return [44, 96, 108];
    }
    let point = grid.point(pos);
    let mut color = sample(
        [
            point[0] + grid.meters() * 0.5,
            point[2] + grid.meters() * 0.5,
        ],
        c,
        grid.meters(),
        seed,
    )
    .linear_rgb;
    let hash = mix(seed
        ^ mix(pos.x as u64)
        ^ mix(pos.y as u64).rotate_left(21)
        ^ mix(pos.z as u64).rotate_left(42));
    let grain = 1. + (((hash >> 11) as f64 / (1u64 << 53) as f64) - 0.5) * 0.035;
    for v in &mut color {
        *v = (*v as f64 * grain).clamp(0., 1.) as f32;
    }
    color.map(|v| {
        let v = v as f64;
        let s = if v <= 0.0031308 {
            12.92 * v
        } else {
            1.055 * v.powf(1. / 2.4) - 0.055
        };
        (s * 255.).round() as u8
    })
}

/// Compile once per saved landscape. Uses existing steady climate model on the
/// logical sample lattice (jitter approximated), actual triangle slopes, and the
/// conserved sediment/drainage fields. No weather or vegetation simulation.
pub fn contexts(
    nodes: &[[f64; 8]],
    triangles: &[[u32; 3]],
    water: &[f64],
    side: usize,
    extent: f64,
    seed: u64,
    config: TerrainConfig,
) -> Vec<Context> {
    assert_eq!(nodes.len(), side * side);
    assert_eq!(water.len(), nodes.len());
    let heights: Vec<_> = nodes.iter().map(|n| n[2]).collect();
    let climate = climate::compute(&heights, side, extent / (side - 1) as f64, config, seed);
    let climate: Vec<_> = climate
        .iter()
        .map(|c| super::atlas_climate::Cell {
            mean: c.mean,
            warm: c.warm,
            moisture: c.moisture,
            ..Default::default()
        })
        .collect();
    contexts_with_climate(nodes, triangles, water, &climate, None)
}

pub fn contexts_with_climate(
    nodes: &[[f64; 8]],
    triangles: &[[u32; 3]],
    water: &[f64],
    climate: &[super::atlas_climate::Cell],
    discharge: Option<&[f64]>,
) -> Vec<Context> {
    assert_eq!(climate.len(), nodes.len());
    assert_eq!(water.len(), nodes.len());
    if let Some(q) = discharge {
        assert_eq!(q.len(), nodes.len());
    }
    let mut slopes = vec![[0.; 3]; nodes.len()];
    for t in triangles {
        let [a, b, c] = t.map(|i| nodes[i as usize]);
        let (dx1, dz1, dh1) = (b[0] - a[0], b[1] - a[1], b[2] - a[2]);
        let (dx2, dz2, dh2) = (c[0] - a[0], c[1] - a[1], c[2] - a[2]);
        let det = dx1 * dz2 - dx2 * dz1;
        assert!(det > 0.);
        let g = [(dh1 * dz2 - dh2 * dz1) / det, (dx1 * dh2 - dx2 * dh1) / det];
        for &i in t {
            let s = &mut slopes[i as usize];
            s[0] += g[0] * det;
            s[1] += g[1] * det;
            s[2] += det;
        }
    }
    nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let c = climate[i];
            let g = slopes[i];
            Context {
                temperature: c.mean as f64,
                warm_temperature: c.warm as f64,
                moisture: c.moisture as f64,
                gradient: [g[0] / g[2].max(1e-9), g[1] / g[2].max(1e-9)],
                sediment_m: n[6],
                // Convert annual discharge to reference equivalent area for the
                // existing wet-valley response. Dry catchments are not lush just
                // because they have a large geometric area.
                drainage_m2: discharge.map_or(n[5], |q| q[i] / 0.3),
                water_depth_m: (water[i] - n[2]).max(0.),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> Context {
        Context {
            temperature: 10.,
            warm_temperature: 17.,
            moisture: 0.5,
            gradient: [0.1, 0.1],
            sediment_m: 1.,
            drainage_m2: 1e5,
            water_depth_m: 0.,
        }
    }
    #[test]
    fn coverage_is_normalized_and_transitions_are_continuous() {
        for i in 0..1000 {
            let mut c = context();
            c.temperature = -10. + i as f64 * 0.03;
            c.warm_temperature = c.temperature + 7.;
            c.moisture = i as f64 / 1000.;
            let a = sample([1234., 4321.], c, 0.0625, 42);
            assert!((a.coverage.iter().sum::<f32>() - 1.).abs() < 1e-6);
            assert!(a.coverage.iter().all(|v| *v >= 0.));
            c.temperature += 1e-6;
            c.moisture += 1e-6;
            let b = sample([1234., 4321.], c, 0.0625, 42);
            for k in 0..3 {
                assert!((a.linear_rgb[k] - b.linear_rgb[k]).abs() < 1e-5);
            }
        }
    }
    #[test]
    fn coarse_samples_remove_small_noise_and_voxels_have_stable_rgb() {
        assert_eq!(filtered_noise([14., 27.], 12., 195., 42), 0.);
        let p = CellPos::new(16, 200, 32);
        let g = GridScale::default();
        assert_eq!(
            voxel_rgb(p, g, context(), 42),
            voxel_rgb(p, g, context(), 42)
        );
        let mut c = context();
        c.gradient = [1., 0.];
        assert!(
            sample([100., 200.], c, 1., 42).coverage[1]
                > sample([100., 200.], context(), 1., 42).coverage[1]
        );
        c = context();
        c.warm_temperature = -4.;
        assert!(sample([100., 200.], c, 1., 42).coverage[3] > 0.9);
    }
}
