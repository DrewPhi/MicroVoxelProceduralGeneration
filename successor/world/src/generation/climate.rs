//! A steady regional moisture sweep, not a time-dependent atmosphere simulation.
//! Upwind dependencies form a DAG on this regular grid: O(cells), no sort needed.
use super::{
    environment::TerrainConfig,
    noise::{noise, smoothstep},
};
#[derive(Clone, Copy, Debug)]
pub(super) struct ClimateCell {
    pub mean: f32,
    pub cold: f32,
    pub warm: f32,
    pub rain: f32,
    pub pet: f32,
    pub moisture: f32,
    pub humidity: f32,
    pub wetness: f32,
    pub slope: f32,
    pub exposure: f32,
}
impl Default for ClimateCell {
    fn default() -> Self {
        Self {
            mean: 12.,
            cold: 5.,
            warm: 19.,
            rain: 1000.,
            pet: 700.,
            moisture: 0.59,
            humidity: 0.7,
            wetness: 0.4,
            slope: 0.1,
            exposure: 0.,
        }
    }
}
fn capacity(temperature: f64) -> f64 {
    30. * (temperature * 0.04).exp()
}
fn growing_temperature(mean: f64, amplitude: f64) -> f64 {
    const SEASONS: [f64; 12] = [
        1., 0.8660254, 0.5, 0., -0.5, -0.8660254, -1., -0.8660254, -0.5, 0., 0.5, 0.8660254,
    ];
    SEASONS
        .iter()
        .map(|s| (mean + amplitude * s).clamp(0., 30.))
        .sum::<f64>()
        / 12.
}
/// Dominant and weaker reverse-season moisture transport. Both solve the same
/// O(cells) dependency sweep; no per-voxel weather or iterative atmosphere.
pub(super) fn compute(
    heights: &[f64],
    side: usize,
    spacing: f64,
    config: TerrainConfig,
    seed: u64,
) -> Vec<ClimateCell> {
    let mut primary = compute_wind(heights, side, spacing, config, seed);
    let mut reverse = config;
    reverse.wind_degrees += 180.;
    let secondary = compute_wind(heights, side, spacing, reverse, seed);
    for (a, b) in primary.iter_mut().zip(secondary) {
        a.rain = 0.8 * a.rain + 0.2 * b.rain;
        a.humidity = 0.8 * a.humidity + 0.2 * b.humidity;
        a.exposure = 0.8 * a.exposure + 0.2 * b.exposure;
        a.moisture = a.rain / (a.rain + a.pet);
    }
    primary
}

fn compute_wind(
    heights: &[f64],
    side: usize,
    spacing: f64,
    config: TerrainConfig,
    seed: u64,
) -> Vec<ClimateCell> {
    let parameters = config.climate.parameters();
    let mut result = vec![ClimateCell::default(); heights.len()];
    for (i, cell) in result.iter_mut().enumerate() {
        let x = (i % side) as f64 * spacing;
        let z = (i / side) as f64 * spacing;
        let sea = parameters.sea_temperature + 2.5 * noise(x + 1711., z - 1973., 7500., seed ^ 402);
        let mean = sea - 0.0062 * heights[i];
        let amplitude = parameters.season_amplitude;
        cell.mean = mean as f32;
        cell.cold = (mean - amplitude) as f32;
        cell.warm = (mean + amplitude) as f32;
        cell.pet = (58.93 * growing_temperature(mean, amplitude)).max(80.) as f32;
    }
    let angle = config.wind_degrees.to_radians();
    let u = angle.cos();
    let v = angle.sin();
    let total = u.abs() + v.abs();
    let wx = u.abs() / total;
    let wz = v.abs() / total;
    let sx = if u >= 0. { 1isize } else { -1 };
    let sz = if v >= 0. { 1isize } else { -1 };
    let distance = spacing / total;
    let mut vapor = vec![0f32; heights.len()];
    for zi in 0..side {
        let z = if sz > 0 { zi } else { side - 1 - zi };
        for xi in 0..side {
            let x = if sx > 0 { xi } else { side - 1 - xi };
            let i = z * side + x;
            let regional_source = (1.
                + 0.25 * noise(x as f64 * spacing, z as f64 * spacing, 55_000., seed ^ 3401))
            .clamp(0.65, 1.35);
            let boundary =
                config.boundary_moisture * regional_source * 0.88 * capacity(result[i].mean as f64);
            let upstream = |nx: isize, nz: isize| {
                if nx < 0 || nz < 0 || nx >= side as isize || nz >= side as isize {
                    (boundary, heights[i])
                } else {
                    let n = nz as usize * side + nx as usize;
                    (vapor[n] as f64, heights[n])
                }
            };
            let (qx, hx) = upstream(x as isize - sx, z as isize);
            let (qz, hz) = upstream(x as isize, z as isize - sz);
            let q = wx * qx + wz * qz;
            let upstream_h = wx * hx + wz * hz;
            let lift = (heights[i] - upstream_h).max(0.);
            let cap = capacity(result[i].mean as f64);
            let condensed = (q - cap * 0.88).max(0.);
            let background = q * (distance / 22_000.);
            let lifted = q * (lift / 900.).min(0.15);
            let loss = (condensed + background + lifted).clamp(0., q);
            // Replenishment is a prescribed climatic source, not live evaporation
            // withdrawn from soil/water. Annual units are calibrated estimates.
            let remaining = q - loss;
            let recharge = (boundary - remaining).max(0.) * (distance / 50_000.);
            vapor[i] = (remaining + recharge).min(cap) as f32;
            let rain = (loss / distance * parameters.storm_transport).clamp(0., 6000.);
            result[i].rain = rain as f32;
            result[i].humidity = (vapor[i] as f64 / cap).clamp(0., 1.) as f32;
            result[i].exposure = ((heights[i] - upstream_h) / spacing).clamp(-1., 1.) as f32;
        }
    }
    // A small fixed-radius average suppresses one-cell orographic spikes. It
    // preserves the domain sum up to boundary effects; it is not a water solver.
    let raw: Vec<f32> = result.iter().map(|c| c.rain).collect();
    for z in 0..side {
        for x in 0..side {
            let mut sum = 0.;
            let mut count = 0.;
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let nx = x as isize + dx;
                    let nz = z as isize + dz;
                    if nx >= 0 && nz >= 0 && nx < side as isize && nz < side as isize {
                        sum += raw[nz as usize * side + nx as usize];
                        count += 1.;
                    }
                }
            }
            let c = &mut result[z * side + x];
            c.rain = sum / count;
            c.moisture = c.rain / (c.rain + c.pet);
        }
    }
    result
}
pub(super) fn finish(
    cells: &mut [ClimateCell],
    heights: &[f64],
    soil: &[f64],
    flow: &[f64],
    lakes: &[f64],
    side: usize,
    spacing: f64,
) {
    for z in 0..side {
        for x in 0..side {
            let i = z * side + x;
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(side - 1);
            let z0 = z.saturating_sub(1);
            let z1 = (z + 1).min(side - 1);
            let dx =
                (heights[z * side + x1] - heights[z * side + x0]) / ((x1 - x0) as f64 * spacing);
            let dz =
                (heights[z1 * side + x] - heights[z0 * side + x]) / ((z1 - z0) as f64 * spacing);
            let slope = dx.hypot(dz);
            let convergence = ((flow[i] / spacing).max(1.) / (slope + 0.025)).ln();
            let drainage_wetness = smoothstep(5.5, 10.5, convergence);
            let retention = smoothstep(0.2, 4., soil[i]);
            let flat = 1. - smoothstep(0.03, 0.30, slope);
            let c = &mut cells[i];
            c.slope = slope as f32;
            c.wetness =
                (0.50 * c.moisture as f64 + 0.35 * drainage_wetness + 0.15 * retention * flat)
                    .clamp(0., 1.) as f32;
            if lakes[i] > heights[i] {
                c.wetness = 1.;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ridge_casts_a_rain_shadow_and_reversing_wind_reverses_it() {
        let side = 101;
        let h: Vec<_> = (0..side * side)
            .map(|i| {
                let x = (i % side) as f64;
                900. * (-((x - 50.) / 9.).powi(2)).exp()
            })
            .collect();
        let mut config = TerrainConfig::default();
        config.wind_degrees = 0.;
        let a = compute(&h, side, 100., config, 42);
        config.wind_degrees = 180.;
        let b = compute(&h, side, 100., config, 42);
        let avg = |c: &[ClimateCell], range: std::ops::Range<usize>| {
            range
                .clone()
                .map(|x| c[50 * side + x].rain as f64)
                .sum::<f64>()
                / range.len() as f64
        };
        assert!(avg(&a, 32..48) > avg(&a, 53..70) * 1.5);
        assert!(avg(&b, 53..70) > avg(&b, 32..48) * 1.5);
    }
    #[test]
    fn lapse_rate_and_seasonal_ranges_are_consistent() {
        let side = 3;
        let h = vec![1000.; 9];
        let c = compute(&h, side, 16., TerrainConfig::default(), 1);
        let flat = compute(&vec![0.; 9], side, 16., TerrainConfig::default(), 1);
        assert!((flat[4].mean - c[4].mean - 6.2).abs() < 0.001);
        assert!(c[4].cold < c[4].mean && c[4].mean < c[4].warm);
    }
    #[test]
    fn dry_boundary_and_presets_produce_bounded_distinct_climates() {
        let h = vec![100.; 31 * 31];
        let config = TerrainConfig {
            boundary_moisture: 0.,
            ..TerrainConfig::default()
        };
        let dry = compute(&h, 31, 100., config, 42);
        assert!(dry
            .iter()
            .all(|c| c.rain == 0. && c.moisture == 0. && c.humidity == 0.));
        let arid = compute(
            &h,
            31,
            100.,
            TerrainConfig::for_climate(super::super::ClimatePreset::Arid),
            42,
        );
        let tropical = compute(
            &h,
            31,
            100.,
            TerrainConfig::for_climate(super::super::ClimatePreset::Tropical),
            42,
        );
        let cold = compute(
            &h,
            31,
            100.,
            TerrainConfig::for_climate(super::super::ClimatePreset::Subpolar),
            42,
        );
        assert!(tropical[400].rain > arid[400].rain * 2.);
        assert!(tropical[400].moisture > arid[400].moisture);
        assert!(cold[400].mean < arid[400].mean - 15.);
        for c in arid.iter().chain(&tropical).chain(&cold) {
            assert!(c.rain.is_finite() && c.pet.is_finite() && c.pet > 0.);
            assert!((0. ..=1.).contains(&c.moisture));
            assert!((0. ..=1.).contains(&c.humidity));
        }
    }
    #[test]
    fn convergent_deep_flat_ground_is_wetter_and_lakes_are_saturated() {
        let h = vec![100.; 9];
        let mut c = vec![ClimateCell::default(); 9];
        let soil = vec![4.; 9];
        let mut flow = vec![256.; 9];
        flow[4] = 10_000_000.;
        let mut lakes = vec![0.; 9];
        lakes[0] = 101.;
        finish(&mut c, &h, &soil, &flow, &lakes, 3, 16.);
        assert!(c[4].wetness > c[8].wetness);
        assert_eq!(c[0].wetness, 1.);
        assert_eq!(c[4].slope, 0.);
    }
}
