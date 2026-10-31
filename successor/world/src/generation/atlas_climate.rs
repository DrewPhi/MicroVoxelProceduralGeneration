//! Coarse, steady annual climate for a saved geographic atlas. No live weather.
//! Two O(N) upwind sweeps advect ocean-supplied vapor. Land only recycles a
//! fraction of precipitation; it cannot magically restore the boundary humidity.
//! Annual rainfall/PET are calibrated climatic estimates, not a weather forecast.
use super::noise::noise;

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub latitude_at_z0: f64,
    pub wind_degrees: f64,
    pub boundary_humidity: f64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            latitude_at_z0: 35.,
            wind_degrees: 25.,
            boundary_humidity: 0.65,
        }
    }
}
impl Settings {
    pub fn valid(self) -> bool {
        self.latitude_at_z0.is_finite()
            && self.latitude_at_z0.abs() <= 75.
            && self.wind_degrees.is_finite()
            && self.wind_degrees.abs() <= 360_000.
            && (0. ..=1.).contains(&self.boundary_humidity)
    }
}

/// Fixed export order: mean/cold/warm Celsius, rainfall/PET/runoff mm/year,
/// moisture suitability [0,1], approximate distance to ocean in kilometers.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cell {
    pub mean: f32,
    pub cold: f32,
    pub warm: f32,
    pub rain: f32,
    pub pet: f32,
    pub runoff: f32,
    pub moisture: f32,
    pub coast_km: f32,
}
impl Cell {
    pub fn fields(self) -> [f32; 8] {
        [
            self.mean,
            self.cold,
            self.warm,
            self.rain,
            self.pet,
            self.runoff,
            self.moisture,
            self.coast_km,
        ]
    }
    pub fn from_fields(v: [f32; 8]) -> Self {
        Self {
            mean: v[0],
            cold: v[1],
            warm: v[2],
            rain: v[3],
            pet: v[4],
            runoff: v[5],
            moisture: v[6],
            coast_km: v[7],
        }
    }
}

/// Turc–Pike annual partition, n=2. AET cannot exceed rainfall or PET.
/// Applying this catchment-scale relation locally is a coarse approximation.
pub fn runoff(rain: f64, pet: f64) -> f64 {
    if rain <= 0. {
        return 0.;
    }
    (rain - rain * pet / rain.hypot(pet)).max(0.)
}

/// Eight-neighbor chamfer distance (not exact geodesic distance). Two sweeps,
/// no heap, reused for every climate update because the coast is fixed.
pub fn coast_distance(ocean: &[bool], side: usize, spacing: f64) -> Vec<f32> {
    assert_eq!(ocean.len(), side * side);
    let mut d: Vec<f32> = ocean
        .iter()
        .map(|&wet| if wet { 0. } else { 2_000_000. })
        .collect();
    for reverse in [false, true] {
        for zi in 0..side {
            for xi in 0..side {
                let x = if reverse { side - 1 - xi } else { xi };
                let z = if reverse { side - 1 - zi } else { zi };
                let i = z * side + x;
                for (dx, dz) in if reverse {
                    [(1, 0), (-1, 1), (0, 1), (1, 1)]
                } else {
                    [(-1, 0), (-1, -1), (0, -1), (1, -1)]
                } {
                    let nx = x as isize + dx;
                    let nz = z as isize + dz;
                    if nx >= 0 && nz >= 0 && nx < side as isize && nz < side as isize {
                        let length = if dx != 0 && dz != 0 {
                            std::f64::consts::SQRT_2
                        } else {
                            1.
                        };
                        d[i] = d[i]
                            .min(d[nz as usize * side + nx as usize] + (spacing * length) as f32);
                    }
                }
            }
        }
    }
    d
}

fn capacity(t: f64) -> f64 {
    20. * (0.055 * (t - 10.)).exp()
}

pub fn compute(
    heights: &[f64],
    positions: &[[f64; 2]],
    ocean: &[bool],
    coast: &[f32],
    side: usize,
    spacing: f64,
    settings: Settings,
    seed: u64,
) -> Vec<Cell> {
    assert!(settings.valid() && side >= 3 && spacing > 0.);
    assert_eq!(heights.len(), side * side);
    assert_eq!(positions.len(), heights.len());
    assert_eq!(ocean.len(), heights.len());
    assert_eq!(coast.len(), heights.len());
    let mut cells = vec![Cell::default(); heights.len()];
    for (i, c) in cells.iter_mut().enumerate() {
        let [x, z] = positions[i];
        let latitude = (settings.latitude_at_z0 + z / 111_320.).clamp(-85., 85.);
        let sea_temperature = 28. - 0.0085 * latitude * latitude
            + 1.5 * noise(x + 173_731., z - 897_123., 380_000., seed ^ 0xc110);
        let mean = sea_temperature - 0.0062 * heights[i].max(0.);
        let inland = 1. - (-(coast[i] as f64) / 180_000.).exp();
        let amplitude = 3.5 + 5. * (latitude.abs() / 60.).min(1.) + 9. * inland;
        let growing = (0..12)
            .map(|m| {
                (mean + amplitude * (m as f64 * std::f64::consts::TAU / 12.).cos()).clamp(0., 30.)
            })
            .sum::<f64>()
            / 12.;
        *c = Cell {
            mean: mean as f32,
            cold: (mean - amplitude) as f32,
            warm: (mean + amplitude) as f32,
            pet: (58.93 * growing).max(60.) as f32,
            coast_km: coast[i] / 1000.,
            ..Cell::default()
        };
    }
    for (angle, weight) in [
        (settings.wind_degrees, 0.85),
        (settings.wind_degrees + 155., 0.15),
    ] {
        sweep(
            heights,
            ocean,
            side,
            spacing,
            angle,
            weight,
            settings.boundary_humidity,
            &mut cells,
        );
    }
    for c in &mut cells {
        c.runoff = runoff(c.rain as f64, c.pet as f64) as f32;
        c.moisture = c.rain / (c.rain + c.pet);
    }
    cells
}

fn sweep(
    heights: &[f64],
    ocean: &[bool],
    side: usize,
    spacing: f64,
    angle: f64,
    weight: f64,
    boundary_humidity: f64,
    cells: &mut [Cell],
) {
    let (v, u) = angle.to_radians().sin_cos();
    let total = u.abs() + v.abs();
    let wx = u.abs() / total;
    let wz = v.abs() / total;
    let sx = if u >= 0. { 1isize } else { -1 };
    let sz = if v >= 0. { 1isize } else { -1 };
    let distance = spacing / total;
    let mut vapor = vec![0f32; heights.len()];
    let mut condensate = vec![0f32; heights.len()];
    let mut cap = vec![0f32; heights.len()];
    for (i, c) in cells.iter().enumerate() {
        cap[i] = capacity(c.mean as f64) as f32;
    }
    for zi in 0..side {
        for xi in 0..side {
            let x = if sx > 0 { xi } else { side - 1 - xi };
            let z = if sz > 0 { zi } else { side - 1 - zi };
            let i = z * side + x;
            let h = heights[i].max(0.);
            let capacity = cap[i] as f64;
            let upstream = |nx: isize, nz: isize| {
                if nx < 0 || nz < 0 || nx >= side as isize || nz >= side as isize {
                    (capacity * boundary_humidity, h, 0.)
                } else {
                    let j = nz as usize * side + nx as usize;
                    (vapor[j] as f64, heights[j].max(0.), condensate[j] as f64)
                }
            };
            let (qx, hx, cx) = upstream(x as isize - sx, z as isize);
            let (qz, hz, cz) = upstream(x as isize, z as isize - sz);
            let mut q = wx * qx + wz * qz;
            if ocean[i] {
                q += (0.92 * capacity - q).max(0.) * (1. - (-distance / 45_000.).exp());
            }
            let condensed = (q - 0.92 * capacity).max(0.);
            let remaining = q - condensed;
            let lift = (h - wx * hx - wz * hz).max(0.);
            let lifted = remaining * (1. - (-lift / 1400.).exp());
            let cloud = wx * cx + wz * cz + condensed + lifted;
            // Finite fallout distance spreads orographic rain downwind instead
            // of dropping an entire air column onto a single numerical node.
            let fallout = cloud * (1. - (-distance / 18_000.).exp());
            let background = (remaining - lifted) * (1. - (-distance / 330_000.).exp());
            let loss = fallout + background;
            condensate[i] = (cloud - fallout) as f32;
            // A bounded fraction can return via local evapotranspiration. Ocean
            // replenishment is above; dry land has no independent humidity source.
            let recycled = if ocean[i] { 0. } else { loss * 0.22 };
            vapor[i] = (remaining - lifted - background + recycled) as f32;
            // Effective annual transport includes the duty cycle of rain-bearing wind.
            cells[i].rain += (weight * loss / distance * 12_000_000.) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scene(side: usize, ridge: bool, wind: f64, coast: bool, humidity: f64) -> Vec<Cell> {
        let spacing = 5000.;
        let positions: Vec<_> = (0..side * side)
            .map(|i| [(i % side) as f64 * spacing, (i / side) as f64 * spacing])
            .collect();
        let h: Vec<_> = positions
            .iter()
            .map(|p| {
                if ridge {
                    2500. * (-((p[0] - 250_000.) / 30_000.).powi(2)).exp()
                } else {
                    100.
                }
            })
            .collect();
        let ocean: Vec<_> = positions.iter().map(|p| coast && p[0] < 100_000.).collect();
        let d = coast_distance(&ocean, side, spacing);
        compute(
            &h,
            &positions,
            &ocean,
            &d,
            side,
            spacing,
            Settings {
                wind_degrees: wind,
                boundary_humidity: humidity,
                ..Settings::default()
            },
            42,
        )
    }
    #[test]
    fn rain_shadow_reverses_with_wind() {
        let a = scene(101, true, 0., false, 0.8);
        let b = scene(101, true, 180., false, 0.8);
        let avg = |v: &[Cell], r: std::ops::Range<usize>| {
            r.clone().map(|x| v[50 * 101 + x].rain as f64).sum::<f64>() / r.len() as f64
        };
        assert!(avg(&a, 40..48) > avg(&a, 53..61) * 2.);
        assert!(avg(&b, 53..61) > avg(&b, 40..48) * 2.);
    }
    #[test]
    fn ocean_supplies_dry_boundary_and_inland_seasons_strengthen() {
        let dry = scene(101, false, 0., false, 0.);
        assert!(dry.iter().all(|c| c.rain == 0. && c.runoff == 0.));
        let wet = scene(101, false, 0., true, 0.);
        assert!(wet[50 * 101 + 25].rain > 100.);
        assert!(wet[50 * 101 + 25].rain > wet[50 * 101 + 95].rain);
        assert!(
            wet[50 * 101 + 95].warm - wet[50 * 101 + 95].cold
                > wet[50 * 101 + 25].warm - wet[50 * 101 + 25].cold
        );
        assert_eq!(
            wet.iter().map(|c| c.fields()).collect::<Vec<_>>(),
            scene(101, false, 0., true, 0.)
                .iter()
                .map(|c| c.fields())
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn annual_partition_conserves_water_and_obeys_energy_limit() {
        for p in [0., 0.1, 10., 100., 1000., 10000.] {
            for pet in [0., 10., 100., 1000., 10000.] {
                let q = runoff(p, pet);
                let evap = p - q;
                assert!(q >= 0. && q <= p && evap <= pet + 1e-9);
                assert!((q + evap - p).abs() < 1e-9);
            }
        }
        assert!(runoff(1500., 700.) > runoff(300., 700.) * 10.);
    }
}
