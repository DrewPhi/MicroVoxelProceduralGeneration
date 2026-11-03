//! Global, meter-space authoring field. Patches never alter its definition.
use fastnoise_lite::{FastNoiseLite, NoiseType};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Params {
    pub seed: i32,
    pub world_m: f64,
    pub ceiling_m: f64,
    pub voxel_cm: f64,
    pub base_m: f64,
    pub relief_m: f64,
    pub wavelength_m: f64,
    pub octaves: usize,
    pub gain: f64,
    pub lacunarity: f64,
    pub overhang_m: f64,
    pub rock_scale_m: f64,
    pub cave_scale_m: f64,
    pub cave_width: f64,
    pub cave_roof_m: f64,
}
impl Default for Params {
    fn default() -> Self {
        Self {
            seed: 42,
            world_m: 10_000.,
            ceiling_m: 1024.,
            voxel_cm: 6.25,
            base_m: 350.,
            relief_m: 280.,
            wavelength_m: 1800.,
            octaves: 7,
            gain: 0.5,
            lacunarity: 2.,
            overhang_m: 12.,
            rock_scale_m: 40.,
            cave_scale_m: 55.,
            cave_width: 0.23,
            cave_roof_m: 5.,
        }
    }
}
impl Params {
    pub fn validate(&self) -> Result<(), String> {
        let values = [
            self.world_m,
            self.ceiling_m,
            self.voxel_cm,
            self.base_m,
            self.relief_m,
            self.wavelength_m,
            self.gain,
            self.lacunarity,
            self.overhang_m,
            self.rock_scale_m,
            self.cave_scale_m,
            self.cave_width,
            self.cave_roof_m,
        ];
        if values.iter().any(|v| !v.is_finite()) {
            return Err("Parameters must be finite".into());
        }
        if !(16. ..=100_000.).contains(&self.world_m)
            || !(16. ..=4096.).contains(&self.ceiling_m)
            || !(0.5..=200.).contains(&self.voxel_cm)
            || !(1..=12).contains(&self.octaves)
            || self.base_m < 0.
            || self.base_m > self.ceiling_m
            || self.relief_m < 0.
            || self.relief_m > 2000.
            || !(1. ..=100_000.).contains(&self.wavelength_m)
            || !(0. ..=0.9).contains(&self.gain)
            || !(1.1..=4.).contains(&self.lacunarity)
            || !(0. ..=100.).contains(&self.overhang_m)
            || self.rock_scale_m < 1.
            || self.cave_scale_m < 1.
            || !(0. ..=0.7).contains(&self.cave_width)
            || !(0. ..=100.).contains(&self.cave_roof_m)
        {
            return Err("Parameters outside supported authoring ranges".into());
        }
        let max_frequency = self.lacunarity.powi(self.octaves as i32 - 1) / self.wavelength_m;
        if max_frequency * self.world_m.max(self.ceiling_m) > 100_000_000. {
            return Err(
                "Noise coordinates too large; reduce octaves/lacunarity or increase wavelength"
                    .into(),
            );
        }
        Ok(())
    }
    pub fn voxel_m(&self) -> f64 {
        self.voxel_cm / 100.
    }
}

pub struct Terrain {
    pub p: Params,
    land: FastNoiseLite,
    rock: FastNoiseLite,
    cave_a: FastNoiseLite,
    cave_b: FastNoiseLite,
}
fn noise(seed: i32) -> FastNoiseLite {
    let mut n = FastNoiseLite::with_seed(seed);
    n.set_noise_type(Some(NoiseType::OpenSimplex2));
    n.set_frequency(Some(1.));
    n
}
impl Terrain {
    pub fn new(p: Params) -> Self {
        Self {
            land: noise(p.seed),
            rock: noise(p.seed.wrapping_add(1009)),
            cave_a: noise(p.seed.wrapping_add(2027)),
            cave_b: noise(p.seed.wrapping_add(3041)),
            p,
        }
    }
    fn fbm(&self, n: &FastNoiseLite, point: [f64; 3], scale: f64, octaves: usize) -> f64 {
        let (mut frequency, mut amplitude, mut sum, mut weight) = (1. / scale, 1., 0., 0.);
        for _ in 0..octaves {
            sum += amplitude
                * n.get_noise_3d(
                    point[0] * frequency,
                    point[1] * frequency,
                    point[2] * frequency,
                ) as f64;
            weight += amplitude;
            frequency *= self.p.lacunarity;
            amplitude *= self.p.gain;
        }
        sum / weight
    }
    pub fn base_height(&self, x: f64, z: f64) -> f64 {
        self.p.base_m
            + self.p.relief_m
                * self.fbm(&self.land, [x, 0., z], self.p.wavelength_m, self.p.octaves)
    }
    /// Positive is solid. Density is not a signed-distance estimate.
    pub fn density(&self, point: [f64; 3]) -> f64 {
        self.density_with_base_height(point, self.base_height(point[0], point[2]))
    }
    /// Same authoring field with a cached base height for repeated column queries.
    /// Callers must supply base_height(point[0], point[2]).
    pub fn density_with_base_height(&self, point: [f64; 3], base_height: f64) -> f64 {
        let depth = base_height - point[1];
        let mut d = self.uncarved_density(point, depth);
        if self.p.cave_width > 0. && depth > self.p.cave_roof_m {
            let a = self.fbm(&self.cave_a, point, self.p.cave_scale_m, 2);
            let b = self.fbm(&self.cave_b, point, self.p.cave_scale_m, 2);
            // Intersect two noise bands for tunnel-like voids, with a smooth roof gate.
            let gate = ((depth - self.p.cave_roof_m) / 5.).clamp(0., 1.);
            let width = self.p.cave_width * gate;
            d = d.min((a * a + b * b - width * width) * self.p.cave_scale_m);
        }
        d
    }
    fn uncarved_density(&self, point: [f64; 3], depth: f64) -> f64 {
        depth
            + if self.p.overhang_m > 0. {
                self.p.overhang_m * self.fbm(&self.rock, point, self.p.rock_scale_m, 3)
            } else {
                0.
            }
    }
    /// Surface-relative depth proxy, before cave removal. Using this prevents
    /// underground cave floors from being painted as new grass surfaces.
    pub fn surface_depth(&self, point: [f64; 3]) -> f64 {
        self.uncarved_density(point, self.base_height(point[0], point[2]) - point[1])
    }
    pub fn solid(&self, c: [i64; 3]) -> bool {
        let s = self.p.voxel_m();
        let point = c.map(|v| (v as f64 + 0.5) * s);
        point[0] >= 0.
            && point[2] >= 0.
            && point[0] < self.p.world_m
            && point[2] < self.p.world_m
            && point[1] >= 0.
            && point[1] < self.p.ceiling_m
            && self.density(point) > 0.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn global_queries_are_order_independent_and_grid_sampling_is_explicit() {
        let p = Params::default();
        let s = p.voxel_m();
        let a = Terrain::new(p.clone());
        let b = Terrain::new(p);
        for c in [
            [0, 100, 0],
            [1023, 4000, 1024],
            [1024, 4000, 1024],
            [80000, 5000, 80000],
        ] {
            let point = c.map(|v| (v as f64 + 0.5) * s);
            let value = a.density(point);
            assert_eq!(
                value,
                a.density_with_base_height(point, a.base_height(point[0], point[2]))
            );
            let _ = a.density([200., 40., 200.]);
            assert_eq!(value, b.density(point));
            assert_eq!(a.solid(c), value > 0.);
        }
    }
    #[test]
    fn resolution_does_not_change_density_and_caves_only_remove_solid() {
        let a = Terrain::new(Params::default());
        let mut p = a.p.clone();
        p.voxel_cm = 25.;
        let b = Terrain::new(p);
        assert_eq!(a.density([12., 300., 25.]), b.density([12., 300., 25.]));
        let mut p = a.p.clone();
        p.cave_width = 0.;
        let uncarved = Terrain::new(p);
        for i in 0..100 {
            let point = [i as f64 * 2., 280., i as f64];
            assert!(a.density(point) <= uncarved.density(point));
        }
        assert!(!a.solid([-1, 1, 1]));
        assert!(!a.solid([1, -1, 1]));
    }
}
