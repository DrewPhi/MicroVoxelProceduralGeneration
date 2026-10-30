//! Flow-oriented detail amplification, not a live erosion/fluid solver.
//! Physical anchors keep patterns stable across voxel sizes and tile requests.
use super::{noise::*, terrain::Region, EnvironmentSample};

fn ravines(region: &Region, x: f64, z: f64, wavelength: f64, seed: u64) -> f64 {
    let pitch = wavelength * 4.;
    let gx = (x / pitch).floor();
    let gz = (z / pitch).floor();
    let tx = fade(x / pitch - gx);
    let tz = fade(z / pitch - gz);
    let mut value = 0.;
    for j in 0..2 {
        for i in 0..2 {
            let cx = (gx + i as f64) * pitch;
            let cz = (gz + j as f64) * pitch;
            let g = region.gradient_at(cx, cz);
            let length = g[0].hypot(g[1]).max(0.001);
            let cross = ((x - cx) * g[1] - (z - cz) * g[0]) / length;
            let phase = ((mix(seed ^ mix(cx.to_bits()) ^ mix(cz.to_bits())) >> 11) as f64
                / (1u64 << 53) as f64)
                * std::f64::consts::TAU;
            let w = if i == 0 { 1. - tx } else { tx } * if j == 0 { 1. - tz } else { tz };
            // Elongated troughs parallel to flow, nested at successive scales.
            let p = (cross / wavelength * std::f64::consts::TAU + phase).cos();
            value += w * (0.5 + 0.5 * p).powi(3);
        }
    }
    value
}

pub(super) fn relief(
    region: &Region,
    x: f64,
    z: f64,
    spacing: f64,
    seed: u64,
    e: EnvironmentSample,
) -> f64 {
    let gradient = region.gradient_at(x, z);
    let slope = gradient[0].hypot(gradient[1]);
    let active = smoothstep(0.04, 0.65, slope);
    let rock = 1. - smoothstep(0.3, 3., e.soil_depth_m as f64);
    let floodplain = 1. - smoothstep(20_000., 200_000., e.drainage_area_m2 as f64);
    let regional_variation = 0.8 + 0.4 * noise(x, z, 240., seed ^ 607);
    let mut incision = 0.;
    let mut parent = 1.;
    for (i, (wave, depth)) in [(24., 3.5), (8., 1.25), (2., 0.35)].into_iter().enumerate() {
        let visible = smoothstep(1., 2., wave / spacing);
        if visible == 0. {
            continue;
        }
        let r = ravines(region, x, z, wave, seed ^ (809 + i as u64));
        incision += depth * visible * r * parent;
        parent = 0.4 + 0.6 * r;
    }
    // Anchor spacing must remain fixed: hashing continuously moving anchor
    // coordinates would turn coherent relief into white noise.
    let cut = incision * regional_variation * active * (0.25 + 0.75 * rock) * floodplain;
    // A bounded talus apron proxy at footslopes; not claimed to conserve mass.
    let apron = (1. - smoothstep(0.12, 0.5, slope))
        * smoothstep(0.03, 0.16, slope)
        * rock
        * (0.5 + 0.5 * noise(x, z, 18., seed ^ 612)).max(0.);
    let mut fractures = 0.;
    if rock > 0.1 {
        for (i, wave) in [0.5, 0.125, 0.03125, 0.0078125, 0.001953125]
            .into_iter()
            .enumerate()
        {
            let visibility = smoothstep(1., 2., wave / spacing);
            if visibility == 0. {
                continue;
            }
            let distance = noise(
                x * 0.8 + z * 0.6,
                z * 0.16 - x * 0.12,
                wave,
                seed ^ (1300 + i as u64),
            )
            .abs();
            fractures -= wave * 0.06 * (1. - smoothstep(0.015, 0.1, distance)) * visibility * rock;
        }
    }
    // Curvature distinguishes accumulating footslopes from exposed shoulders.
    let dx = region.gradient_at(x + 16., z)[0] - region.gradient_at(x - 16., z)[0];
    let dz = region.gradient_at(x, z + 16.)[1] - region.gradient_at(x, z - 16.)[1];
    let toe = smoothstep(0.01, 0.18, dx + dz) * (1. - smoothstep(0.3, 0.8, slope));
    let debris = toe * rock * 2.8 * (0.5 + 0.5 * noise(x, z, 42., seed ^ 613));
    let bedding = matches!(
        e.geology,
        super::Geology::Sandstone | super::Geology::Limestone
    );
    let thickness = 7. + 3. * noise(x, z, 150., seed ^ 614);
    let layer = (e.elevation_m as f64 + 0.035 * x - 0.022 * z) / thickness;
    let ledge = if bedding {
        -(0.5 + 0.5 * (layer * std::f64::consts::TAU).cos()).powi(3)
            * 1.6
            * rock
            * smoothstep(0.25, 0.85, slope)
    } else {
        0.
    };
    let visible = smoothstep(1., 2., thickness / spacing);
    (apron + debris) * floodplain - cut * 0.7 + fractures + ledge * visible * floodplain
}

pub(super) fn coverage(
    x: f64,
    z: f64,
    seed: u64,
    weights: [(crate::MaterialId, f32); 5],
) -> crate::MaterialId {
    // Correlated physical-space thresholds retain environmental proportions
    // approximately, while boundaries form patches rather than cell confetti.
    let p = (0.5
        + 0.62 * noise(x, z, 18., seed ^ 920)
        + 0.22 * noise(x, z, 3., seed ^ 921)
        + 0.06 * noise(x, z, 0.35, seed ^ 922))
    .clamp(0., 0.999999);
    let mut sum = 0.;
    for (m, w) in weights {
        sum += w as f64;
        if p < sum {
            return m;
        }
    }
    weights[4].0
}
