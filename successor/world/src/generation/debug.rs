//! Regional debug maps. False colors are diagnostics, never canonical voxel paint.
use super::{Biome, EnvironmentSample, Geology, Landform};
use std::{fmt, str::FromStr};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugLayer {
    Elevation,
    Landforms,
    Geology,
    Temperature,
    Seasonality,
    Rainfall,
    Moisture,
    Wetness,
    Soil,
    Slope,
    Drainage,
    Biomes,
}
impl DebugLayer {
    pub const ALL: [Self; 12] = [
        Self::Elevation,
        Self::Landforms,
        Self::Geology,
        Self::Temperature,
        Self::Seasonality,
        Self::Rainfall,
        Self::Moisture,
        Self::Wetness,
        Self::Soil,
        Self::Slope,
        Self::Drainage,
        Self::Biomes,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Elevation => "elevation",
            Self::Landforms => "landforms",
            Self::Geology => "geology",
            Self::Temperature => "temperature",
            Self::Seasonality => "seasonality",
            Self::Rainfall => "rainfall",
            Self::Moisture => "moisture",
            Self::Wetness => "wetness",
            Self::Soil => "soil",
            Self::Slope => "slope",
            Self::Drainage => "drainage",
            Self::Biomes => "biomes",
        }
    }
    pub fn legend(self) -> &'static str {
        match self {
        Self::Elevation=>"0..1024 m: green, brown, white",
        Self::Landforms=>"plain pale green; hills green; plateau ochre; basin blue; mountains brown",
        Self::Geology=>"granite grey; sandstone orange; limestone cream; basalt dark blue-grey",
        Self::Temperature=>"-15..35 C annual mean: blue to red",
        Self::Seasonality=>"0..40 C warm-minus-cold seasonal range: blue to red",
        Self::Rainfall=>"0..3000 mm/year annual-equivalent estimate: tan to blue (saturates above 3000)",
        Self::Moisture=>"0..1 precipitation/(precipitation+potential evaporation): tan to green",
        Self::Wetness=>"0..1 soil/drainage wetness proxy: tan to blue",
        Self::Soil=>"0..8 m loose sediment depth: rock grey to brown",
        Self::Slope=>"0..1.5 rise/run: pale to dark red",
        Self::Drainage=>"log10 256..10000000 m2 rainfall-weighted catchment: pale to blue",
        Self::Biomes=>"bare rock grey; alpine white; tundra lilac; grassland light green; woodland dark green; arid scrub ochre; desert tan; wetland teal; riparian blue-green; labels indicate suitability, no trees generated",
    }
    }
    pub fn color(self, s: EnvironmentSample) -> [u8; 3] {
        match self {
            Self::Elevation => {
                let h = s.elevation_m / 1024.;
                if h < 0.15 {
                    blend([70, 108, 86], [141, 164, 106], h / 0.15)
                } else if h < 0.6 {
                    blend([141, 164, 106], [132, 104, 81], (h - 0.15) / 0.45)
                } else {
                    blend([132, 104, 81], [240, 240, 234], (h - 0.6) / 0.4)
                }
            }
            Self::Landforms => landform_color(s.landform()),
            Self::Geology => geology_color(s.geology),
            Self::Temperature => {
                blend([50, 100, 185], [212, 78, 50], (s.temperature_c + 15.) / 50.)
            }
            Self::Seasonality => blend(
                [70, 128, 173],
                [211, 95, 57],
                (s.warm_season_c - s.cold_season_c) / 40.,
            ),
            Self::Rainfall => blend([211, 193, 145], [35, 90, 175], s.rainfall_mm_year / 3000.),
            Self::Moisture => blend([211, 181, 111], [45, 126, 72], s.moisture),
            Self::Wetness => blend([204, 185, 133], [36, 108, 153], s.ground_wetness),
            Self::Soil => blend([153, 153, 147], [120, 77, 39], s.soil_depth_m / 8.),
            Self::Slope => blend([232, 220, 191], [130, 43, 32], s.slope / 1.5),
            Self::Drainage => blend(
                [223, 224, 200],
                [20, 95, 175],
                (s.drainage_area_m2.max(1.).log10() - 2.4) / 4.6,
            ),
            Self::Biomes => biome_color(s.biome()),
        }
    }
}
impl fmt::Display for DebugLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
impl FromStr for DebugLayer {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|p| p.name() == s)
            .ok_or_else(|| format!("Unknown debug layer '{s}'"))
    }
}
pub fn biome_color(b: Biome) -> [u8; 3] {
    match b {
        Biome::BareRock => [143, 145, 137],
        Biome::Alpine => [235, 240, 242],
        Biome::Tundra => [160, 156, 164],
        Biome::Grassland => [145, 170, 91],
        Biome::Woodland => [53, 103, 63],
        Biome::AridScrub => [175, 160, 95],
        Biome::Desert => [221, 191, 130],
        Biome::Wetland => [50, 122, 114],
        Biome::Riparian => [61, 153, 148],
    }
}
pub fn landform_color(l: Landform) -> [u8; 3] {
    match l {
        Landform::Plain => [183, 204, 126],
        Landform::RollingHills => [99, 156, 91],
        Landform::Plateau => [199, 157, 85],
        Landform::Basin => [89, 149, 177],
        Landform::Mountains => [133, 105, 90],
    }
}
pub fn geology_color(g: Geology) -> [u8; 3] {
    match g {
        Geology::Granite => [149, 150, 145],
        Geology::Sandstone => [190, 132, 81],
        Geology::Limestone => [218, 209, 174],
        Geology::Basalt => [66, 78, 87],
    }
}
fn blend(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0., 1.);
    std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8)
}
#[derive(Clone, Debug)]
pub struct DebugImage {
    pub size: usize,
    pub rgb: Vec<u8>,
}
pub(super) fn image(
    size: usize,
    extent_m: f64,
    layer: DebugLayer,
    sample: impl Fn(f64, f64) -> EnvironmentSample,
) -> Result<DebugImage, String> {
    if !(16..=2048).contains(&size) {
        return Err("debug map size must be 16..2048".into());
    }
    let mut rgb = Vec::with_capacity(size * size * 3);
    for z in 0..size {
        for x in 0..size {
            let p = sample(
                (x as f64 + 0.5) * extent_m / size as f64,
                (z as f64 + 0.5) * extent_m / size as f64,
            );
            rgb.extend_from_slice(&layer.color(p));
        }
    }
    Ok(DebugImage { size, rgb })
}

#[derive(Clone, Debug)]
pub struct EnvironmentStats {
    pub samples: usize,
    pub landforms: [usize; 5],
    pub geology: [usize; 4],
    pub biomes: [usize; 9],
    pub min_elevation: f32,
    pub max_elevation: f32,
    pub min_temperature: f32,
    pub max_temperature: f32,
    pub mean_rainfall: f64,
    pub mean_moisture: f64,
    pub gentle_fraction: f64,
}
impl fmt::Display for EnvironmentStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f,"Elevation {:.1}..{:.1} m; mean temperature {:.1}..{:.1} C; mean rain {:.0} mm/year; moisture {:.2}; gentle ground {:.1}%",self.min_elevation,self.max_elevation,self.min_temperature,self.max_temperature,self.mean_rainfall,self.mean_moisture,self.gentle_fraction*100.)?;
        for (name, count) in Landform::ALL
            .iter()
            .map(|v| (v.name(), self.landforms[*v as usize]))
            .chain(
                Geology::ALL
                    .iter()
                    .map(|v| (v.name(), self.geology[*v as usize])),
            )
            .chain(
                Biome::ALL
                    .iter()
                    .map(|v| (v.name(), self.biomes[*v as usize])),
            )
        {
            if count > 0 {
                writeln!(
                    f,
                    "  {name}: {:.1}%",
                    count as f64 / self.samples as f64 * 100.
                )?;
            }
        }
        Ok(())
    }
}
pub(super) fn statistics(
    side: usize,
    extent_m: f64,
    sample: impl Fn(f64, f64) -> EnvironmentSample,
) -> EnvironmentStats {
    let mut s = EnvironmentStats {
        samples: side * side,
        landforms: [0; 5],
        geology: [0; 4],
        biomes: [0; 9],
        min_elevation: f32::INFINITY,
        max_elevation: f32::NEG_INFINITY,
        min_temperature: f32::INFINITY,
        max_temperature: f32::NEG_INFINITY,
        mean_rainfall: 0.,
        mean_moisture: 0.,
        gentle_fraction: 0.,
    };
    for z in 0..side {
        for x in 0..side {
            let e = sample(
                (x as f64 + 0.5) * extent_m / side as f64,
                (z as f64 + 0.5) * extent_m / side as f64,
            );
            s.landforms[e.landform() as usize] += 1;
            s.geology[e.geology as usize] += 1;
            s.biomes[e.biome() as usize] += 1;
            s.min_elevation = s.min_elevation.min(e.elevation_m);
            s.max_elevation = s.max_elevation.max(e.elevation_m);
            s.min_temperature = s.min_temperature.min(e.temperature_c);
            s.max_temperature = s.max_temperature.max(e.temperature_c);
            s.mean_rainfall += e.rainfall_mm_year as f64;
            s.mean_moisture += e.moisture as f64;
            s.gentle_fraction += f64::from(e.slope < 0.15);
        }
    }
    s.mean_rainfall /= s.samples as f64;
    s.mean_moisture /= s.samples as f64;
    s.gentle_fraction /= s.samples as f64;
    s
}

/// Sample long-term baseline conditions at integer voxel positions. The map is
/// intentionally coarse and diagnostic; it does not alter the world's palette.
pub fn image_from_world<G: crate::generation::Generator>(
    world: &crate::World<G>,
    layer: DebugLayer,
    size: usize,
) -> Result<DebugImage, String> {
    if !(16..=2048).contains(&size) {
        return Err("debug map size must be 16..2048".into());
    }
    let mut rgb = Vec::with_capacity(size * size * 3);
    for z in 0..size {
        for x in 0..size {
            let px = ((x as f64 + 0.5) * world.spec().extent_cells() as f64 / size as f64).floor()
                as i64;
            let pz = ((z as f64 + 0.5) * world.spec().extent_cells() as f64 / size as f64).floor()
                as i64;
            let e = world
                .environment_at(crate::CellPos::new(px, 0, pz))
                .ok_or("generator does not provide environment data")?;
            rgb.extend_from_slice(&layer.color(e));
        }
    }
    Ok(DebugImage { size, rgb })
}
