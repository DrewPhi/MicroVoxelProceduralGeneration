//! Long-term generation inputs and surface suitability. No live weather state.
use super::noise::mix;
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum ClimatePreset {
    #[default]
    Temperate,
    Continental,
    Arid,
    Subpolar,
    Tropical,
}
impl ClimatePreset {
    pub const ALL: [Self; 5] = [
        Self::Temperate,
        Self::Continental,
        Self::Arid,
        Self::Subpolar,
        Self::Tropical,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Temperate => "temperate",
            Self::Continental => "continental",
            Self::Arid => "arid",
            Self::Subpolar => "subpolar",
            Self::Tropical => "tropical",
        }
    }
    pub(super) fn parameters(self) -> ClimateParameters {
        match self {
            Self::Temperate => ClimateParameters {
                sea_temperature: 14.,
                season_amplitude: 7.,
                moisture: 0.8,
                storm_transport: 300_000.,
            },
            Self::Continental => ClimateParameters {
                sea_temperature: 12.,
                season_amplitude: 17.,
                moisture: 0.60,
                storm_transport: 160_000.,
            },
            Self::Arid => ClimateParameters {
                sea_temperature: 25.,
                season_amplitude: 10.,
                moisture: 0.22,
                storm_transport: 80_000.,
            },
            Self::Subpolar => ClimateParameters {
                sea_temperature: 1.,
                season_amplitude: 5.5,
                moisture: 0.72,
                storm_transport: 160_000.,
            },
            Self::Tropical => ClimateParameters {
                sea_temperature: 27.,
                season_amplitude: 2.5,
                moisture: 0.95,
                storm_transport: 260_000.,
            },
        }
    }
}
impl fmt::Display for ClimatePreset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
impl FromStr for ClimatePreset {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|p| p.name() == s)
            .ok_or_else(|| {
                format!(
                    "Unknown climate '{s}'; use temperate, continental, arid, subpolar or tropical"
                )
            })
    }
}
pub(super) struct ClimateParameters {
    pub sea_temperature: f64,
    pub season_amplitude: f64,
    pub moisture: f64,
    pub storm_transport: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainConfig {
    pub refinement: Refinement,
    pub climate: ClimatePreset,
    pub wind_degrees: f64,
    pub boundary_moisture: f64,
}
impl Default for TerrainConfig {
    fn default() -> Self {
        Self::for_climate(ClimatePreset::Temperate)
    }
}
impl TerrainConfig {
    pub fn for_climate(climate: ClimatePreset) -> Self {
        Self {
            refinement: Refinement::Geomorphic,
            climate,
            wind_degrees: 25.,
            boundary_moisture: climate.parameters().moisture,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if !self.wind_degrees.is_finite() || self.wind_degrees.abs() > 360_000. {
            return Err("wind angle must be finite and within +/-360000 degrees".into());
        }
        if !self.boundary_moisture.is_finite() || !(0.0..=1.0).contains(&self.boundary_moisture) {
            return Err("boundary moisture must be 0..1".into());
        }
        Ok(())
    }
    pub fn wind(self) -> [f32; 2] {
        let r = self.wind_degrees.to_radians();
        [r.cos() as f32, r.sin() as f32]
    }
    pub(super) fn fingerprint(self) -> u64 {
        mix(self.climate as u64)
            ^ mix(self.refinement as u64 + 912)
            ^ mix(self.wind_degrees.rem_euclid(360.).to_bits()).rotate_left(19)
            ^ mix(self.boundary_moisture.to_bits()).rotate_left(41)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refinement {
    Noise,
    Geomorphic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Landform {
    Plain,
    RollingHills,
    Plateau,
    Basin,
    Mountains,
}
impl Landform {
    pub const ALL: [Self; 5] = [
        Self::Plain,
        Self::RollingHills,
        Self::Plateau,
        Self::Basin,
        Self::Mountains,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::RollingHills => "rolling hills",
            Self::Plateau => "plateau",
            Self::Basin => "basin",
            Self::Mountains => "mountains",
        }
    }
    pub fn from_weights(w: [f32; 5]) -> Self {
        let mut best = 0;
        for i in 1..5 {
            if w[i] > w[best] {
                best = i;
            }
        }
        Self::ALL[best]
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Geology {
    Granite,
    Sandstone,
    Limestone,
    Basalt,
}
impl Geology {
    pub const ALL: [Self; 4] = [
        Self::Granite,
        Self::Sandstone,
        Self::Limestone,
        Self::Basalt,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Granite => "granite",
            Self::Sandstone => "sandstone",
            Self::Limestone => "limestone",
            Self::Basalt => "basalt",
        }
    }
    /// Generation coefficients, not runtime material/physics properties.
    pub(super) fn erodibility(self) -> f64 {
        match self {
            Self::Granite => 0.055,
            Self::Sandstone => 0.18,
            Self::Limestone => 0.10,
            Self::Basalt => 0.035,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Biome {
    BareRock,
    Alpine,
    Tundra,
    Grassland,
    Woodland,
    AridScrub,
    Desert,
    Wetland,
    Riparian,
}
impl Biome {
    pub const ALL: [Self; 9] = [
        Self::BareRock,
        Self::Alpine,
        Self::Tundra,
        Self::Grassland,
        Self::Woodland,
        Self::AridScrub,
        Self::Desert,
        Self::Wetland,
        Self::Riparian,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::BareRock => "bare rock",
            Self::Alpine => "alpine",
            Self::Tundra => "tundra",
            Self::Grassland => "grassland",
            Self::Woodland => "woodland suitability",
            Self::AridScrub => "arid scrub",
            Self::Desert => "desert",
            Self::Wetland => "wetland",
            Self::Riparian => "riparian",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvironmentSample {
    pub elevation_m: f32,
    pub soil_depth_m: f32,
    pub slope: f32,
    pub temperature_c: f32,
    pub cold_season_c: f32,
    pub warm_season_c: f32,
    pub rainfall_mm_year: f32,
    pub potential_evaporation_mm_year: f32,
    pub moisture: f32,
    pub ground_wetness: f32,
    pub atmospheric_humidity: f32,
    pub orographic_exposure: f32,
    pub drainage_area_m2: f32,
    pub prevailing_wind: [f32; 2],
    pub landform_weights: [f32; 5],
    pub geology: Geology,
}
impl EnvironmentSample {
    /// Continuous surface coverage; biome labels remain useful diagnostics.
    /// Each native cell selects one material. Distant views integrate coverage.
    pub fn surface_cover(
        self,
        rock: crate::materials::MaterialId,
    ) -> [(crate::materials::MaterialId, f32); 5] {
        use crate::materials::MaterialId as M;
        let ramp = |a: f32, b: f32, v: f32| {
            let t = ((v - a) / (b - a)).clamp(0., 1.);
            t * t * (3. - 2. * t)
        };
        let snow = 1. - ramp(-1., 6., self.warm_season_c);
        let exposed = (1. - ramp(0.08, 0.8, self.soil_depth_m)).max(ramp(0.65, 1.6, self.slope));
        let dry = 1. - ramp(0.10, 0.26, self.moisture);
        let wet = ramp(0.72, 0.90, self.ground_wetness) * (1. - ramp(0.04, 0.14, self.slope));
        let remaining = 1. - snow;
        let rock_weight = remaining * exposed;
        let soil = remaining - rock_weight;
        [
            (M::SNOW, snow),
            (rock, rock_weight),
            (M::SAND, soil * dry),
            (M::MUD, soil * (1. - dry) * wet),
            (M::GRASS, soil * (1. - dry) * (1. - wet)),
        ]
    }
    pub fn landform(self) -> Landform {
        Landform::from_weights(self.landform_weights)
    }
    /// A diagnostic label derived from conditions, never a terrain recipe.
    /// Woodland means suitability only; no trees are generated in this version.
    pub fn biome(self) -> Biome {
        if self.warm_season_c < 3. {
            Biome::Alpine
        } else if self.soil_depth_m < 0.22 || self.slope > 1.2 {
            Biome::BareRock
        } else if self.temperature_c < 3.5 {
            Biome::Tundra
        } else if self.ground_wetness > 0.78 && self.slope < 0.07 {
            Biome::Wetland
        } else if self.drainage_area_m2 > 100_000.
            && self.slope < 0.20
            && self.ground_wetness > 0.55
        {
            Biome::Riparian
        } else if self.moisture < 0.17 {
            Biome::Desert
        } else if self.moisture < 0.32 {
            Biome::AridScrub
        } else if self.moisture > 0.56 && self.soil_depth_m > 0.7 {
            Biome::Woodland
        } else {
            Biome::Grassland
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coverage_is_normalized_and_continuous_across_diagnostic_biome_edges() {
        let sample = EnvironmentSample {
            elevation_m: 100.,
            soil_depth_m: 0.22,
            slope: 0.1,
            temperature_c: 14.,
            cold_season_c: 7.,
            warm_season_c: 21.,
            rainfall_mm_year: 600.,
            potential_evaporation_mm_year: 900.,
            moisture: 0.17,
            ground_wetness: 0.5,
            atmospheric_humidity: 0.5,
            orographic_exposure: 0.,
            drainage_area_m2: 10.,
            prevailing_wind: [1., 0.],
            landform_weights: [1., 0., 0., 0., 0.],
            geology: Geology::Granite,
        };
        for moisture in [0., 0.16999, 0.17001, 0.5, 1.] {
            for soil in [0., 0.21999, 0.22001, 2.] {
                let a = EnvironmentSample {
                    moisture,
                    soil_depth_m: soil,
                    ..sample
                };
                let coverage = a.surface_cover(crate::materials::MaterialId::ROCK);
                assert!((coverage.iter().map(|x| x.1).sum::<f32>() - 1.).abs() < 1e-6);
                assert!(coverage.iter().all(|x| (0.0..=1.).contains(&x.1)));
                let b = EnvironmentSample {
                    moisture: moisture + 0.00001,
                    ..a
                }
                .surface_cover(crate::materials::MaterialId::ROCK);
                for (x, y) in coverage.iter().zip(b) {
                    assert!((x.1 - y.1).abs() < 0.001);
                }
            }
        }
    }
    #[test]
    fn configuration_rejects_invalid_numbers_and_identifies_inputs() {
        let base = TerrainConfig::default();
        for wind in [f64::NAN, f64::INFINITY, 360001.] {
            assert!(TerrainConfig {
                wind_degrees: wind,
                ..base
            }
            .validate()
            .is_err());
        }
        for moisture in [f64::NAN, -0.1, 1.1] {
            assert!(TerrainConfig {
                boundary_moisture: moisture,
                ..base
            }
            .validate()
            .is_err());
        }
        let east = TerrainConfig {
            wind_degrees: 0.,
            ..base
        };
        assert_eq!(
            east.fingerprint(),
            TerrainConfig {
                wind_degrees: 360.,
                ..base
            }
            .fingerprint()
        );
        assert_ne!(
            base.fingerprint(),
            TerrainConfig {
                boundary_moisture: 0.,
                ..base
            }
            .fingerprint()
        );
        for preset in ClimatePreset::ALL {
            let config = TerrainConfig::for_climate(preset);
            config.validate().unwrap();
            assert_eq!(preset.name().parse::<ClimatePreset>().unwrap(), preset);
            if preset != base.climate {
                assert_ne!(base.fingerprint(), config.fingerprint());
            }
        }
    }
}
