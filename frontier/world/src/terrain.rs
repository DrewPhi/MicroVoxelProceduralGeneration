//! CPU-canonical, seeded mountain heightfield, independent of renderer spacing.
//! Greenfield gradient noise/FBM composition; no external implementation copied.
use crate::{Cell, Material, VOXEL_SIZE};

/// Noise/composition and material definition version, independent of the fixture.
pub const FBM_SOURCE_VERSION: u32 = 1;
/// Regional composition version; recipe must also be included in source/cache keys.
pub const REGIONAL_FBM_SOURCE_VERSION: u32 = 2;
/// Regional material painting version. Elevation/source v2 remains unchanged.
pub const REGIONAL_APPEARANCE_VERSION: u32 = 3;
/// Authored Austrian meadow and connected mountain backdrop.
pub const ALPINE_FBM_SOURCE_VERSION: u32 = 3;
pub const ALPINE_APPEARANCE_VERSION: u32 = 1;

/// Explicit source identity: the legacy recipe retains existing worlds exactly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FbmRecipe {
    #[default]
    Legacy,
    /// Regional mountains with a gentle continuous domain warp.
    Regional,
    /// Same regional bands without warp, for matched source comparisons.
    RegionalUnwarped,
    /// Rolling meadow at origin with a connected range several kilometers north.
    Alpine,
}

/// Physical source settings, not display/LOD settings. Changing them changes the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FbmParams {
    pub recipe: FbmRecipe,
    /// Base horizontal wavelength in meters, in [16, 10_000_000].
    pub horizontal_scale: f64,
    /// Vertical relief scale in meters, in [0, 100_000].
    pub amplitude: f64,
    /// Octaves in [1, 16]: legacy ridge bands or regional fine-detail bands.
    /// Frequency doubles each octave; regional macro/ridge bands each have three.
    pub octaves: u32,
    /// Relative octave amplitude in [0, 1]. Regional gain is 0.8 * roughness,
    /// so default fine-band slope energy decays instead of accumulating.
    pub roughness: f64,
}
impl Default for FbmParams {
    fn default() -> Self {
        Self {
            recipe: FbmRecipe::Legacy,
            horizontal_scale: 1800.0,
            amplitude: 1000.0,
            octaves: 8,
            roughness: 0.5,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FbmError(pub &'static str);
impl std::fmt::Display for FbmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for FbmError {}
impl FbmParams {
    pub fn validate(&self) -> Result<(), FbmError> {
        if !self.horizontal_scale.is_finite()
            || !(16.0..=10_000_000.0).contains(&self.horizontal_scale)
        {
            return Err(FbmError(
                "horizontal_scale must be finite and in [16, 10000000] meters",
            ));
        }
        if !self.amplitude.is_finite() || !(0.0..=100_000.0).contains(&self.amplitude) {
            return Err(FbmError(
                "amplitude must be finite and in [0, 100000] meters",
            ));
        }
        if !(1..=16).contains(&self.octaves) {
            return Err(FbmError("octaves must be in [1, 16]"));
        }
        if !self.roughness.is_finite() || !(0.0..=1.0).contains(&self.roughness) {
            return Err(FbmError("roughness must be finite and in [0, 1]"));
        }
        Ok(())
    }
}

/// Immutable heightfield only: no caves, water, structures, or edit overrides.
/// CPU f64 defines the source; GPU consumers upload samples rather than re-run noise.
#[derive(Clone, Debug)]
pub struct FbmTerrain {
    seed: u64,
    params: FbmParams,
}
impl FbmTerrain {
    pub fn new(seed: u64, params: FbmParams) -> Result<Self, FbmError> {
        params.validate()?;
        Ok(Self { seed, params })
    }
    pub fn seed(&self) -> u64 {
        self.seed
    }
    pub fn params(&self) -> FbmParams {
        self.params
    }
    /// Version of the selected canonical composition. Include params (including
    /// recipe), seed, this version and `appearance_version` in any persistent
    /// source/cache identity. Elevation and material recipes evolve separately.
    pub fn source_version(&self) -> u32 {
        match self.params.recipe {
            FbmRecipe::Legacy => FBM_SOURCE_VERSION,
            FbmRecipe::Alpine => ALPINE_FBM_SOURCE_VERSION,
            FbmRecipe::Regional | FbmRecipe::RegionalUnwarped => REGIONAL_FBM_SOURCE_VERSION,
        }
    }
    pub fn appearance_version(&self) -> u32 {
        match self.params.recipe {
            FbmRecipe::Legacy => 0,
            FbmRecipe::Alpine => ALPINE_APPEARANCE_VERSION,
            FbmRecipe::Regional | FbmRecipe::RegionalUnwarped => REGIONAL_APPEARANCE_VERSION,
        }
    }
    /// Canonical column top plane (meters), excluding edits. Coordinates select
    /// half-open native columns, including at negative boundaries. The smooth
    /// generator is evaluated at that column's center and rounded down to 6.25cm.
    /// Nonfinite/unaddressable coordinates panic like `Cell::from_meters`.
    /// For reliable meter addressing remain within 2^53 native cells.
    pub fn height_m(&self, x: f64, z: f64) -> f64 {
        let c = Cell::from_meters([x, 0.0, z]);
        self.height_cells(c.x, c.z) as f64 * VOXEL_SIZE
    }
    pub(crate) fn height_cells(&self, x: i64, z: i64) -> i64 {
        let x = (x as f64 + 0.5) * VOXEL_SIZE / self.params.horizontal_scale;
        let z = (z as f64 + 0.5) * VOXEL_SIZE / self.params.horizontal_scale;
        (self.elevation(x, z) / VOXEL_SIZE).floor() as i64
    }
    fn elevation(&self, x: f64, z: f64) -> f64 {
        match self.params.recipe {
            FbmRecipe::Legacy => self.legacy_elevation(x, z),
            FbmRecipe::Alpine => crate::alpine::elevation(x, z, self.params, true, |x, z, salt| {
                noise(x, z, self.seed ^ salt)
            }),
            FbmRecipe::Regional | FbmRecipe::RegionalUnwarped => self.regional_elevation(x, z),
        }
    }
    fn legacy_elevation(&self, x: f64, z: f64) -> f64 {
        // Continuous low-frequency domain warp bends ridgelines without a biome map.
        let wx = x + 0.32 * noise(x * 0.55 + 17.3, z * 0.55 - 8.7, self.seed ^ 0x1234);
        let wz = z + 0.32 * noise(x * 0.55 - 31.9, z * 0.55 + 4.2, self.seed ^ 0x5678);
        let broad = noise(wx * 0.42 + 3.1, wz * 0.42 - 7.8, self.seed ^ 0x9abc);
        let (mut sum, mut norm, mut weight, mut freq) = (0.0, 0.0, 1.0, 1.0);
        for octave in 0..self.params.octaves {
            let n = noise(
                wx * freq + 12.7,
                wz * freq - 29.1,
                self.seed.wrapping_add(octave as u64 * 0x9e3779b9),
            );
            let ridge = 1.0 - (n * 1.7).abs().min(1.0);
            sum += weight * (0.75 * ridge * ridge + 0.25 * n);
            norm += weight;
            weight *= self.params.roughness;
            freq *= 2.0;
        }
        self.params.amplitude * (0.85 * sum / norm + 0.4 * broad - 0.12)
    }
    /// B1 experiment only. Keep the warp, broad band and full-source denominator;
    /// dropping octaves must never renormalize the retained terrain into a new world.
    pub(crate) fn visual_legacy_height(&self, cell: [i64; 2], retained: u32) -> f64 {
        if retained == self.params.octaves {
            return self.height_cells(cell[0], cell[1]) as f64 * VOXEL_SIZE;
        }
        let x = (cell[0] as f64 + 0.5) * VOXEL_SIZE / self.params.horizontal_scale;
        let z = (cell[1] as f64 + 0.5) * VOXEL_SIZE / self.params.horizontal_scale;
        let wx = x + 0.32 * noise(x * 0.55 + 17.3, z * 0.55 - 8.7, self.seed ^ 0x1234);
        let wz = z + 0.32 * noise(x * 0.55 - 31.9, z * 0.55 + 4.2, self.seed ^ 0x5678);
        let broad = noise(wx * 0.42 + 3.1, wz * 0.42 - 7.8, self.seed ^ 0x9abc);
        let (mut sum, mut norm, mut weight, mut freq) = (0.0, 0.0, 1.0, 1.0);
        for octave in 0..self.params.octaves {
            if octave < retained {
                let n = noise(
                    wx * freq + 12.7,
                    wz * freq - 29.1,
                    self.seed.wrapping_add(octave as u64 * 0x9e3779b9),
                );
                let ridge = 1.0 - (n * 1.7).abs().min(1.0);
                sum += weight * (0.75 * ridge * ridge + 0.25 * n);
            }
            norm += weight;
            weight *= self.params.roughness;
            freq *= 2.0;
        }
        let h = self.params.amplitude * (0.85 * sum / norm + 0.4 * broad - 0.12);
        (h / VOXEL_SIZE).floor() * VOXEL_SIZE
    }
    fn regional_elevation(&self, x: f64, z: f64) -> f64 {
        // x/z are canonical meters / horizontal_scale. Constants below correspond
        // to physical wavelengths at the default 1800m scale, never display LOD.
        let sample = |x: f64, z: f64, salt: u64| noise(x, z, self.seed ^ salt);
        let (wx, wz) = if self.params.recipe == FbmRecipe::Regional {
            (
                x + (100.0 / 1800.0) * sample(x * 0.4 + 17.3, z * 0.4 - 8.7, 0x1234),
                z + (100.0 / 1800.0) * sample(x * 0.4 - 31.9, z * 0.4 + 4.2, 0x5678),
            )
        } else {
            (x, z)
        };
        let gain = 0.8 * self.params.roughness;
        let bands = |wavelength: f64, count: u32, salt: u64, ridged: bool| {
            let (mut sum, mut norm, mut weight) = (0.0, 0.0, 1.0);
            let mut frequency = 1800.0 / wavelength;
            for octave in 0..count {
                let n = sample(
                    wx * frequency + 12.7,
                    wz * frequency - 29.1,
                    salt.wrapping_add(octave as u64 * 0x9e3779b9),
                );
                // Smooth absolute value avoids the legacy ridge's derivative cusp.
                let value = if ridged {
                    let ridge = 1.0 - (n * n + 0.04).sqrt();
                    ridge * ridge
                } else {
                    n
                };
                sum += weight * value;
                norm += weight;
                weight *= gain;
                frequency *= 2.0;
            }
            sum / norm
        };
        // A separate 3–6km province field controls where ridges exist. Squared
        // smoothstep gives quiet valley interiors and smooth mountain margins.
        let province = (sample(wx * 0.3 + 3.1, wz * 0.3 - 7.8, 0xa53c)
            + 0.35 * sample(wx * 0.6 - 19.4, wz * 0.6 + 2.6, 0x71b9))
            / 1.35;
        let t = ((province + 0.25) / 0.6).clamp(0.0, 1.0);
        let smooth = t * t * (3.0 - 2.0 * t);
        let mountain = smooth * smooth;
        let macro_relief = 90.0 * bands(2500.0, 3, 0x9abc, false);
        let ridges = 320.0 * mountain * bands(1400.0, 3, 0x24da, true);
        let foothills = 12.0 * (0.3 + 0.7 * mountain) * bands(300.0, 1, 0x83e1, false);
        let detail = 1.5 * (0.2 + 0.8 * mountain) * bands(40.0, self.params.octaves, 0xf13d, false);
        // At defaults: 19 noise evaluations (17 unwarped), no per-voxel slope
        // queries. Raising octaves adds exactly one detail evaluation per band.
        self.params.amplitude / 1000.0 * (macro_relief + ridges + foothills + detail)
    }
    /// Slope of the actual regional macro, ridge and foothill composition.
    /// Fine 40m-and-smaller relief is intentionally excluded from cover: a grass
    /// hummock must not become a rock outcrop just because it contains small bumps.
    /// Analytic derivatives avoid four additional full height queries per column.
    fn regional_cover_landform(&self, x: f64, z: f64) -> Field {
        let sample = |x: Field, z: Field, salt| {
            let [v, dx, dz] = noise_gradient(x.v, z.v, self.seed ^ salt);
            Field {
                v,
                dx: dx * x.dx + dz * z.dx,
                dz: dx * x.dz + dz * z.dz,
            }
        };
        let x = Field {
            v: x / self.params.horizontal_scale,
            dx: 1.0,
            dz: 0.0,
        };
        let z = Field {
            v: z / self.params.horizontal_scale,
            dx: 0.0,
            dz: 1.0,
        };
        let (wx, wz) = if self.params.recipe == FbmRecipe::Regional {
            (
                x.add(
                    sample(x.scale(0.4).offset(17.3), z.scale(0.4).offset(-8.7), 0x1234)
                        .scale(100.0 / 1800.0),
                ),
                z.add(
                    sample(x.scale(0.4).offset(-31.9), z.scale(0.4).offset(4.2), 0x5678)
                        .scale(100.0 / 1800.0),
                ),
            )
        } else {
            (x, z)
        };
        let bands = |wavelength: f64, count: u32, salt: u64, ridged: bool| {
            let (mut sum, mut norm, mut weight) = (Field::constant(0.0), 0.0, 1.0);
            let mut frequency = 1800.0 / wavelength;
            for octave in 0..count {
                let n = sample(
                    wx.scale(frequency).offset(12.7),
                    wz.scale(frequency).offset(-29.1),
                    salt.wrapping_add(octave as u64 * 0x9e3779b9),
                );
                let value = if ridged {
                    let root = (n.v * n.v + 0.04).sqrt();
                    let ridge = Field {
                        v: 1.0 - root,
                        dx: -n.v * n.dx / root,
                        dz: -n.v * n.dz / root,
                    };
                    ridge.mul(ridge)
                } else {
                    n
                };
                sum = sum.add(value.scale(weight));
                norm += weight;
                weight *= 0.8 * self.params.roughness;
                frequency *= 2.0;
            }
            sum.scale(1.0 / norm)
        };
        let province = sample(
            wx.scale(0.3).offset(3.1),
            wz.scale(0.3).offset(-7.8),
            0xa53c,
        )
        .add(
            sample(
                wx.scale(0.6).offset(-19.4),
                wz.scale(0.6).offset(2.6),
                0x71b9,
            )
            .scale(0.35),
        )
        .scale(1.0 / 1.35);
        let t = province.offset(0.25).scale(1.0 / 0.6).unit_clamp();
        let smooth = t.mul(t).mul(t.scale(-2.0).offset(3.0));
        let mountain = smooth.mul(smooth);
        let macro_relief = bands(2500.0, 3, 0x9abc, false).scale(90.0);
        let ridges = mountain.mul(bands(1400.0, 3, 0x24da, true)).scale(320.0);
        let foothills = mountain
            .scale(0.7)
            .offset(0.3)
            .mul(bands(300.0, 1, 0x83e1, false))
            .scale(12.0);
        let relief = macro_relief.add(ridges).add(foothills);
        relief.scale(self.params.amplitude / 1000.0)
    }
    fn regional_cover_slope(&self, x: f64, z: f64) -> f64 {
        let field = self.regional_cover_landform(x, z);
        field.dx.hypot(field.dz) / self.params.horizontal_scale
    }
    /// Same discrete material as the native top voxel. `height` is the canonical
    /// top plane from `height_m`. Cover follows physical landform slope, elevation
    /// and continuous moisture, never a display normal or an independent patch map.
    /// Every face of a voxel retains that voxel's one canonical material.
    pub fn surface_material(&self, x: f64, z: f64, height: f64) -> Material {
        if self.params.recipe == FbmRecipe::Alpine {
            let [x, _, z] = Cell::from_meters([x, 0.0, z]).center_m();
            let sx = x * 1800.0 / self.params.horizontal_scale;
            let sz = z * 1800.0 / self.params.horizontal_scale;
            if (-550.0..1500.0).contains(&sz) && (sx - alpine_path_center_m(sz)).abs() < 1.45 {
                return Material::Soil;
            }
            let f = crate::alpine::elevation(
                Field {
                    v: x / self.params.horizontal_scale,
                    dx: 1.0 / self.params.horizontal_scale,
                    dz: 0.0,
                },
                Field {
                    v: z / self.params.horizontal_scale,
                    dx: 0.0,
                    dz: 1.0 / self.params.horizontal_scale,
                },
                self.params,
                false,
                |x: Field, z: Field, salt| {
                    let [v, dx, dz] = noise_gradient(x.v, z.v, self.seed ^ salt);
                    Field {
                        v,
                        dx: dx * x.dx + dz * z.dx,
                        dz: dx * x.dz + dz * z.dz,
                    }
                },
            );
            let relative = if self.params.amplitude > 0.0 {
                height / self.params.amplitude
            } else {
                0.0
            };
            // Coherent turf retention breaks the constant slope/elevation
            // contour only inside its transition belt. Two meter-scale bands
            // produce connected rock/turf tongues rather than random cells.
            let cover = 0.75 * noise(sx / 120.0 + 3.7, sz / 120.0 - 8.1, self.seed ^ 0xac01)
                + 0.25 * noise(sx / 28.0 - 17.2, sz / 28.0 + 4.8, self.seed ^ 0xac02);
            return alpine_cover_material(f.dx.hypot(f.dz), relative, cover);
        }
        if self.params.recipe != FbmRecipe::Legacy {
            let [x, _, z] = Cell::from_meters([x, 0.0, z]).center_m();
            let slope = self.regional_cover_slope(x, z);
            // Broad rainfall/soil-retention variation moves a cover boundary only
            // where the landform already permits exposed rock or thin dry soil.
            let moisture = noise(x / 900.0 + 13.7, z / 900.0 - 5.9, self.seed ^ 0x8ac1);
            let relative_height = if self.params.amplitude > 0.0 {
                height / self.params.amplitude
            } else {
                0.0
            };
            return regional_cover_material(slope, relative_height, moisture);
        }
        if self.params.amplitude > 0.0 && height > self.params.amplitude * 0.48 {
            Material::Stone
        } else {
            Material::Grass
        }
    }
    pub fn surface_color(&self, x: f64, z: f64, height: f64) -> [f32; 3] {
        self.surface_material(x, z, height).color()
    }
}

/// Center of the authored 2.9m walking path, in default-scale scene meters.
/// Cover is evaluated at native column centers; this is independent of display LOD.
pub fn alpine_path_center_m(z: f64) -> f64 {
    16.0 + 60.0 * (z / 310.0).sin() + 0.055 * z
}

impl crate::alpine::Value for Field {
    fn point(v: f64) -> Self {
        Self::constant(v)
    }
    fn add(self, b: Self) -> Self {
        self.add(b)
    }
    fn scale(self, k: f64) -> Self {
        self.scale(k)
    }
    fn mul(self, b: Self) -> Self {
        self.mul(b)
    }
    fn clamp01(self) -> Self {
        self.unit_clamp()
    }
}

// A bounded spatial cover signal changes only the rock transition zone;
// neither gentle lowland nor clearly exposed cliffs depend on that signal.
fn alpine_cover_material(slope: f64, relative_height: f64, cover: f64) -> Material {
    if slope > 0.68 + 0.20 * cover || relative_height > 0.93 + 0.14 * cover {
        Material::Stone
    } else {
        Material::Grass
    }
}

// Regional cover is an authored highland-meadow model, not erosion simulation.
// Rolling ground keeps continuous turf, including high meadows. Only genuinely
// steep faces lose cover; moisture alone never paints lowland patches.
fn regional_cover_material(slope: f64, relative_height: f64, moisture: f64) -> Material {
    let highland = ((relative_height - 0.09) / 0.17).clamp(0.0, 1.0);
    // About 35–39 degrees: elevation changes the threshold only mildly and
    // can never expose a gentle summit solely because of its altitude.
    let rock_limit = 0.78 + 0.035 * moisture - 0.045 * highland;
    if slope > rock_limit {
        Material::Stone
    } else if slope > rock_limit - 0.045 && moisture < -0.15 {
        Material::Soil
    } else {
        Material::Grass
    }
}

/// Value and derivatives for composition of the material-support landform.
#[derive(Clone, Copy)]
struct Field {
    v: f64,
    dx: f64,
    dz: f64,
}
impl Field {
    fn constant(v: f64) -> Self {
        Self {
            v,
            dx: 0.0,
            dz: 0.0,
        }
    }
    fn offset(self, v: f64) -> Self {
        Self {
            v: self.v + v,
            ..self
        }
    }
    fn scale(self, v: f64) -> Self {
        Self {
            v: self.v * v,
            dx: self.dx * v,
            dz: self.dz * v,
        }
    }
    fn add(self, b: Self) -> Self {
        Self {
            v: self.v + b.v,
            dx: self.dx + b.dx,
            dz: self.dz + b.dz,
        }
    }
    fn mul(self, b: Self) -> Self {
        Self {
            v: self.v * b.v,
            dx: self.dx * b.v + self.v * b.dx,
            dz: self.dz * b.v + self.v * b.dz,
        }
    }
    fn unit_clamp(self) -> Self {
        if self.v <= 0.0 {
            Self::constant(0.0)
        } else if self.v >= 1.0 {
            Self::constant(1.0)
        } else {
            self
        }
    }
}

/// Derivative of the same clamped gradient noise used by canonical height.
fn noise_gradient(x: f64, z: f64, seed: u64) -> [f64; 3] {
    let (ix, iz) = (x.floor() as i64, z.floor() as i64);
    let u = Field {
        v: x - x.floor(),
        dx: 1.0,
        dz: 0.0,
    };
    let v = Field {
        v: z - z.floor(),
        dx: 0.0,
        dz: 1.0,
    };
    let fade = |t: Field| {
        let f = t.v * t.v * t.v * (t.v * (t.v * 6.0 - 15.0) + 10.0);
        let d = 30.0 * t.v * t.v * (t.v - 1.0) * (t.v - 1.0);
        Field {
            v: f,
            dx: d * t.dx,
            dz: d * t.dz,
        }
    };
    let grad = |dx: i64, dz: i64| {
        let a = u.offset(-(dx as f64));
        let b = v.offset(-(dz as f64));
        match hash(ix.wrapping_add(dx), iz.wrapping_add(dz), seed) & 7 {
            0 => a,
            1 => a.scale(-1.0),
            2 => b,
            3 => b.scale(-1.0),
            4 => a.add(b).scale(std::f64::consts::FRAC_1_SQRT_2),
            5 => a.add(b.scale(-1.0)).scale(std::f64::consts::FRAC_1_SQRT_2),
            6 => b.add(a.scale(-1.0)).scale(std::f64::consts::FRAC_1_SQRT_2),
            _ => a.add(b).scale(-std::f64::consts::FRAC_1_SQRT_2),
        }
    };
    let lerp = |a: Field, b: Field, t: Field| a.add(b.add(a.scale(-1.0)).mul(t));
    let n = lerp(
        lerp(grad(0, 0), grad(1, 0), fade(u)),
        lerp(grad(0, 1), grad(1, 1), fade(u)),
        fade(v),
    )
    .scale(1.6);
    if n.v <= -1.0 || n.v >= 1.0 {
        [n.v.clamp(-1.0, 1.0), 0.0, 0.0]
    } else {
        [n.v, n.dx, n.dz]
    }
}

pub(crate) fn hash(x: i64, z: i64, seed: u64) -> u64 {
    let mut h = (x as u64).wrapping_mul(0x9e3779b97f4a7c15)
        ^ (z as u64).wrapping_mul(0xbf58476d1ce4e5b9)
        ^ seed;
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d049bb133111eb);
    h ^ (h >> 31)
}
pub(crate) fn noise(x: f64, z: f64, seed: u64) -> f64 {
    let ix = x.floor() as i64;
    let iz = z.floor() as i64;
    let u = x - x.floor();
    let v = z - z.floor();
    let fade = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let grad = |dx: i64, dz: i64| {
        let a = u - dx as f64;
        let b = v - dz as f64;
        match hash(ix.wrapping_add(dx), iz.wrapping_add(dz), seed) & 7 {
            0 => a,
            1 => -a,
            2 => b,
            3 => -b,
            4 => (a + b) * std::f64::consts::FRAC_1_SQRT_2,
            5 => (a - b) * std::f64::consts::FRAC_1_SQRT_2,
            6 => (-a + b) * std::f64::consts::FRAC_1_SQRT_2,
            _ => (-a - b) * std::f64::consts::FRAC_1_SQRT_2,
        }
    };
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    (1.6 * lerp(
        lerp(grad(0, 0), grad(1, 0), fade(u)),
        lerp(grad(0, 1), grad(1, 1), fade(u)),
        fade(v),
    ))
    .clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{raycast, World};

    #[test]
    fn legacy_frozen_default_samples() {
        // Exact f64 composition bits protect existing source versions, including
        // negative coordinates; regional recipes must never replace this path.
        for (seed, expected) in [
            (
                7,
                [
                    4641019009156913366,
                    4636257216527400564,
                    13864745307180359289,
                ],
            ),
            (
                77,
                [
                    4638551457045119077,
                    4645312551394537888,
                    4627992865413181849,
                ],
            ),
            (
                0xdead_beef,
                [
                    4639859225356663404,
                    13861718415259605412,
                    13848455563250511380,
                ],
            ),
        ] {
            let a = FbmTerrain::new(seed, FbmParams::default()).unwrap();
            assert_eq!(a.params().recipe, FbmRecipe::Legacy);
            assert_eq!(a.source_version(), 1);
            assert_eq!(
                [(-1.3125, 2.75), (0.0, 0.0), (53.375, -74.625)]
                    .map(|(x, z)| a.elevation(x, z).to_bits()),
                expected
            );
        }
    }

    #[test]
    fn regional_columns_and_shared_anchors_are_canonical() {
        for recipe in [FbmRecipe::Regional, FbmRecipe::RegionalUnwarped] {
            let params = FbmParams {
                recipe,
                ..Default::default()
            };
            let world = World::fbm(0x19a3, params).unwrap();
            let a = world.fbm_terrain().unwrap();
            assert_eq!(a.source_version(), 2);
            let coordinates = [-100_000.0, -1800.0625, -0.0625, -0.000001, 0.0, 1800.0];
            let samples: Vec<_> = coordinates
                .iter()
                .map(|&x| a.height_m(x, -x - 0.03125))
                .collect();
            for (&x, &h) in coordinates.iter().zip(&samples).rev() {
                let z = -x - 0.03125;
                assert_eq!(a.height_m(x, z), h);
                assert_eq!(world.height_m(x, z), h);
                let c = Cell::from_meters([x, h - VOXEL_SIZE, z]);
                let [cx, _, cz] = c.center_m();
                assert_eq!(a.height_m(cx - 0.02, cz + 0.02), h);
                assert_eq!(world.column(c.x, c.z).height_m(), h);
                assert_eq!(world.material(Cell { y: c.y + 1, ..c }), Material::Air);
                assert_eq!(world.material(c), a.surface_material(cx, cz, h));
            }
            let fine: Vec<_> = (-128..=128)
                .map(|i| a.height_m(i as f64 * 2.0, -48.0))
                .collect();
            for i in (-8..=8).rev() {
                assert_eq!(
                    fine[(i * 16 + 128) as usize],
                    a.height_m(i as f64 * 32.0, -48.0)
                );
            }
        }
    }

    #[test]
    fn gradient_noise_matches_finite_differences() {
        for seed in [7, 113, 0xdead_beef] {
            for (x, z) in [
                (-3.21, 8.76),
                (-0.0001, 0.0001),
                (0.0, 0.0),
                (1.0, -2.0),
                (19.345, 7.891),
            ] {
                let [value, dx, dz] = noise_gradient(x, z, seed);
                let e = 1e-6;
                assert!((value - noise(x, z, seed)).abs() < 1e-14);
                assert!(
                    (dx - (noise(x + e, z, seed) - noise(x - e, z, seed)) / (2.0 * e)).abs() < 1e-7
                );
                assert!(
                    (dz - (noise(x, z + e, seed) - noise(x, z - e, seed)) / (2.0 * e)).abs() < 1e-7
                );
            }
        }
    }

    #[test]
    fn regional_landform_gradient_matches_physical_slopes() {
        for recipe in [FbmRecipe::Regional, FbmRecipe::RegionalUnwarped] {
            for scale in [900.0, 1800.0, 3600.0] {
                let terrain = FbmTerrain::new(
                    113,
                    FbmParams {
                        recipe,
                        horizontal_scale: scale,
                        ..Default::default()
                    },
                )
                .unwrap();
                for (x, z) in [(-3123.4, 8765.4), (-0.01, 0.03), (19345.0, 7891.0)] {
                    let f = terrain.regional_cover_landform(x, z);
                    let e = 0.001;
                    let dx = (terrain.regional_cover_landform(x + e, z).v
                        - terrain.regional_cover_landform(x - e, z).v)
                        / (2.0 * e);
                    let dz = (terrain.regional_cover_landform(x, z + e).v
                        - terrain.regional_cover_landform(x, z - e).v)
                        / (2.0 * e);
                    assert!((f.dx / scale - dx).abs() < 1e-7);
                    assert!((f.dz / scale - dz).abs() < 1e-7);
                    assert!((terrain.regional_cover_slope(x, z) - dx.hypot(dz)).abs() < 1e-7);
                }
            }
        }
    }

    #[test]
    fn regional_cover_requires_landform_exposure() {
        for moisture in [-1.0, -0.5, 0.0, 0.5, 1.0] {
            // Moisture never creates random bare patches on gentle low ground.
            assert_eq!(
                regional_cover_material(0.08, 0.03, moisture),
                Material::Grass
            );
            assert_eq!(
                regional_cover_material(0.9, 0.03, moisture),
                Material::Stone
            );
            assert_eq!(
                regional_cover_material(0.1, 0.30, moisture),
                Material::Grass
            );
        }
        // Rolling hills stay meadow at every altitude/moisture combination.
        for elevation in [0.0, 0.15, 0.30, 1.0] {
            for moisture in [-1.0, 0.0, 1.0] {
                assert_eq!(
                    regional_cover_material(0.6, elevation, moisture),
                    Material::Grass
                );
            }
        }
        assert_eq!(regional_cover_material(0.74, 0.03, -0.5), Material::Soil);
        assert_eq!(regional_cover_material(0.74, 0.03, 0.5), Material::Grass);
        // Zero-amplitude sources cannot contain steep or high cover regions.
        let terrain = FbmTerrain::new(
            7,
            FbmParams {
                recipe: FbmRecipe::Regional,
                amplitude: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        for (x, z) in [(-2000.0, 3000.0), (0.0, 0.0), (9000.0, -8000.0)] {
            assert_eq!(terrain.regional_cover_slope(x, z), 0.0);
            assert_eq!(terrain.surface_material(x, z, 0.0), Material::Grass);
        }
    }

    #[test]
    fn regional_paint_is_seeded_coherent_and_native_column_constant() {
        for recipe in [FbmRecipe::Regional, FbmRecipe::RegionalUnwarped] {
            for seed in [7, 113, 997] {
                let params = FbmParams {
                    recipe,
                    ..Default::default()
                };
                let world = World::fbm(seed, params).unwrap();
                let terrain = world.fbm_terrain().unwrap();
                let repeated = FbmTerrain::new(seed, params).unwrap();
                assert_eq!(terrain.source_version(), 2);
                assert_eq!(terrain.appearance_version(), REGIONAL_APPEARANCE_VERSION);
                let mut counts = [0; 3];
                let mut adjacent_changes = 0;
                for ix in -24..24 {
                    for iz in -24..24 {
                        let (x, z) = (ix as f64 * 331.0, iz as f64 * 329.0);
                        let h = terrain.height_m(x, z);
                        let cell = Cell::from_meters([x, h - VOXEL_SIZE, z]);
                        let [cx, _, cz] = cell.center_m();
                        let material = terrain.surface_material(x, z, h);
                        counts[match material {
                            Material::Grass => 0,
                            Material::Soil => 1,
                            Material::Stone => 2,
                            other => panic!("unexpected terrain paint: {other:?}"),
                        }] += 1;
                        assert_eq!(material, repeated.surface_material(x, z, h));
                        assert_eq!(material, world.material(cell));
                        assert_eq!(material, world.column(cell.x, cell.z).material(cell.y));
                        assert_eq!(material.color(), terrain.surface_color(x, z, h));
                        // Includes both sides of zero and negative cell edges:
                        // every point in a native column selects one base color.
                        for (dx, dz) in [(-0.03, -0.03), (0.03, 0.03)] {
                            assert_eq!(material, terrain.surface_material(cx + dx, cz + dz, h));
                        }
                        let nx = cx + VOXEL_SIZE;
                        adjacent_changes += usize::from(
                            material != terrain.surface_material(nx, cz, terrain.height_m(nx, cz)),
                        );
                    }
                }
                assert!(
                    counts[0] * 5 > counts.iter().sum::<usize>() * 4,
                    "seed={seed} {recipe:?}: {counts:?}"
                );
                // Paint is coherent in meters, not a random native-cell mosaic.
                assert!(adjacent_changes < 20, "adjacent changes={adjacent_changes}");
            }
        }
    }

    #[test]
    fn regional_reduces_local_slopes_on_held_out_seeds() {
        // Broad fixed physical survey, independent of any display page size.
        // These seeds differ from the canonical/material fixtures above.
        for seed in [113, 997, 0x6d28, 0xdead_beef] {
            let legacy = FbmTerrain::new(seed, FbmParams::default()).unwrap();
            for recipe in [FbmRecipe::Regional, FbmRecipe::RegionalUnwarped] {
                let regional = FbmTerrain::new(
                    seed,
                    FbmParams {
                        recipe,
                        ..Default::default()
                    },
                )
                .unwrap();
                let mut old_slopes = Vec::new();
                let mut new_slopes = Vec::new();
                let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
                let mut quiet = 0;
                for ix in -24..24 {
                    for iz in -24..24 {
                        let (x, z) = (ix as f64 * 487.0 + 37.0, iz as f64 * 463.0 - 91.0);
                        let slope = |terrain: &FbmTerrain| {
                            let dx =
                                (terrain.height_m(x + 2.0, z) - terrain.height_m(x - 2.0, z)) / 4.0;
                            let dz =
                                (terrain.height_m(x, z + 2.0) - terrain.height_m(x, z - 2.0)) / 4.0;
                            dx.hypot(dz)
                        };
                        old_slopes.push(slope(&legacy));
                        let s = slope(&regional);
                        new_slopes.push(s);
                        quiet += usize::from(s < 0.15);
                        let h = regional.height_m(x, z);
                        lo = lo.min(h);
                        hi = hi.max(h);
                    }
                }
                old_slopes.sort_by(f64::total_cmp);
                new_slopes.sort_by(f64::total_cmp);
                let n = new_slopes.len();
                for percentile in [50, 95, 99] {
                    let i = n * percentile / 100;
                    assert!(
                        new_slopes[i] < old_slopes[i] * 0.5,
                        "seed={seed} recipe={recipe:?} p{percentile}: {} vs {}",
                        new_slopes[i],
                        old_slopes[i]
                    );
                }
                assert!(quiet > n / 2, "quiet fraction={quiet}/{n}");
                assert!(hi - lo > 150.0, "lost regional relief: {lo}..{hi}");
            }
        }
    }

    #[test]
    fn regional_parameters_change_source_without_hidden_clamps() {
        let p = FbmParams {
            recipe: FbmRecipe::Regional,
            ..Default::default()
        };
        let base = FbmTerrain::new(431, p).unwrap();
        for params in [
            FbmParams {
                horizontal_scale: 3600.0,
                ..p
            },
            FbmParams {
                amplitude: 500.0,
                ..p
            },
            FbmParams { octaves: 1, ..p },
            FbmParams {
                roughness: 0.8,
                ..p
            },
            FbmParams {
                recipe: FbmRecipe::RegionalUnwarped,
                ..p
            },
        ] {
            let other = FbmTerrain::new(431, params).unwrap();
            assert_eq!(other.params(), params);
            assert!((0..24).any(|i| base.height_m(i as f64 * 221.0, -1315.8)
                != other.height_m(i as f64 * 221.0, -1315.8)));
        }
        let flat = FbmTerrain::new(
            431,
            FbmParams {
                amplitude: 0.0,
                ..p
            },
        )
        .unwrap();
        assert_eq!(flat.height_m(-431.0, 128.0), 0.0);
    }

    #[test]
    fn rejects_invalid_parameters() {
        let p = FbmParams::default();
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            assert!(FbmTerrain::new(
                7,
                FbmParams {
                    horizontal_scale: value,
                    ..p
                }
            )
            .is_err());
            assert!(FbmTerrain::new(
                7,
                FbmParams {
                    amplitude: value,
                    ..p
                }
            )
            .is_err());
            assert!(FbmTerrain::new(
                7,
                FbmParams {
                    roughness: value,
                    ..p
                }
            )
            .is_err());
        }
        for q in [
            FbmParams {
                horizontal_scale: 0.0,
                ..p
            },
            FbmParams {
                horizontal_scale: 1e8,
                ..p
            },
            FbmParams {
                amplitude: 100001.0,
                ..p
            },
            FbmParams { octaves: 0, ..p },
            FbmParams { octaves: 17, ..p },
            FbmParams {
                roughness: 1.01,
                ..p
            },
        ] {
            assert!(FbmTerrain::new(7, q).is_err());
        }
        assert!(FbmTerrain::new(
            7,
            FbmParams {
                amplitude: 0.0,
                roughness: 0.0,
                octaves: 1,
                ..p
            }
        )
        .is_ok());
    }
    #[test]
    fn seeded_mountains_and_valleys_are_coherent() {
        let a = FbmTerrain::new(7, FbmParams::default()).unwrap();
        let b = FbmTerrain::new(7, FbmParams::default()).unwrap();
        let other = FbmTerrain::new(8, FbmParams::default()).unwrap();
        let (mut lo, mut hi, mut differences) = (f64::INFINITY, f64::NEG_INFINITY, 0);
        let mut low = 0;
        let mut high = 0;
        for x in -32..32 {
            for z in -32..32 {
                let (x, z) = (x as f64 * 300.0, z as f64 * 300.0);
                let h = a.height_m(x, z);
                assert_eq!(h, b.height_m(x, z));
                assert_eq!((h / VOXEL_SIZE).fract(), 0.0);
                differences += usize::from(h != other.height_m(x, z));
                lo = lo.min(h);
                hi = hi.max(h);
                low += usize::from(h < 100.0);
                high += usize::from(h > 400.0);
                // Adjacent native columns remain correlated, with no tile boundaries.
                assert!((h - a.height_m(x + VOXEL_SIZE, z)).abs() < 2.0);
            }
        }
        assert!(hi - lo > 500.0, "relief={lo}..{hi}");
        assert!(low > 40 && high > 40, "valleys={low} mountains={high}");
        assert!(differences > 4000);
    }
    #[test]
    fn negative_columns_native_material_rays_and_edits_agree() {
        let mut world = World::fbm(7, FbmParams::default()).unwrap();
        for x in [
            -1800.0625, -1800.0, -0.0625, -0.000001, 0.0, 1800.0, 100_000.0,
        ] {
            let z = -x - 0.03125;
            let terrain = world.fbm_terrain().unwrap();
            let h = terrain.height_m(x, z);
            assert_eq!(h, world.height_m(x, z));
            let c = Cell::from_meters([x, h - VOXEL_SIZE, z]);
            let [cx, _, cz] = c.center_m();
            assert_eq!(terrain.height_m(cx, cz), h);
            assert_eq!(terrain.height_m(cx - 0.02, cz + 0.02), h);
            assert_eq!(world.material(c), terrain.surface_material(x, z, h));
            assert_eq!(world.material(c).color(), terrain.surface_color(x, z, h));
            assert_eq!(world.material(Cell { y: c.y + 1, ..c }), Material::Air);
            assert_eq!(world.material(Cell { y: c.y - 20, ..c }), Material::Stone);
            let hit = raycast(&world, [cx, h + 1.0, cz], [0.0, -1.0, 0.0], 2.0).unwrap();
            assert_eq!(hit.cell, c);
            assert_eq!(hit.distance, 1.0);
            world.set(c, Material::Air);
            assert_eq!(world.material(c), Material::Air);
            assert_eq!(world.height_m(x, z), h); // unedited source page stays explicit
        }
    }
    #[test]
    fn display_spacing_cannot_reinterpret_world_scale() {
        let a = FbmTerrain::new(77, FbmParams::default()).unwrap();
        // Shared anchors in independent 2m and 32m CPU pages agree exactly.
        let fine: Vec<_> = (-128..=128)
            .map(|i| a.height_m(i as f64 * 2.0, -48.0))
            .collect();
        for i in -8..=8 {
            assert_eq!(
                fine[((i * 16) + 128) as usize],
                a.height_m(i as f64 * 32.0, -48.0)
            );
        }
        let flat = World::fbm(
            7,
            FbmParams {
                amplitude: 0.0,
                ..FbmParams::default()
            },
        )
        .unwrap();
        assert_eq!(flat.height_m(-3.0, 2.0), 0.0);
        assert_eq!(
            flat.material(Cell::from_meters(crate::CAVE_CENTER_M)),
            Material::Air
        );
        assert_eq!(
            flat.material(Cell::from_meters([4.0, 4.1, -1.5])),
            Material::Air
        );
        assert_eq!(
            flat.material(Cell::from_meters([-3.0, -1.0, 2.0])),
            Material::Stone
        );
    }

    #[test]
    fn cached_columns_keep_fixture_volume_and_edit_authority() {
        for mut world in [World::new(7), World::fbm(7, FbmParams::default()).unwrap()] {
            let edited = Cell {
                x: -48,
                y: 48,
                z: 32,
            };
            world.set(edited, Material::Wood);
            for (x, z) in [(-48, 32), (64, -24), (80, -48), (0, 0)] {
                let column = world.column(x, z);
                assert_eq!(
                    column.height_m(),
                    world.height_m(x as f64 * VOXEL_SIZE, z as f64 * VOXEL_SIZE)
                );
                let top = (column.height_m() / VOXEL_SIZE) as i64;
                for y in (-32..160).chain(top - 2..=top + 2) {
                    assert_eq!(column.material(y), world.material(Cell { x, y, z }));
                }
            }
        }
    }
}

#[cfg(test)]
mod alpine_derivative_tests {
    use super::*;
    #[test]
    fn alpine_cover_transition_keeps_meadow_and_exposed_cliffs_coherent() {
        for cover in [-1.0, -0.5, 0.0, 0.5, 1.0] {
            assert_eq!(alpine_cover_material(0.25, 0.1, cover), Material::Grass);
            assert_eq!(alpine_cover_material(0.9, 0.6, cover), Material::Stone);
            assert_eq!(alpine_cover_material(0.1, 1.1, cover), Material::Stone);
        }
        assert_eq!(alpine_cover_material(0.68, 0.6, -0.5), Material::Stone);
        assert_eq!(alpine_cover_material(0.68, 0.6, 0.5), Material::Grass);
        assert_eq!(alpine_cover_material(0.1, 0.93, -0.5), Material::Stone);
        assert_eq!(alpine_cover_material(0.1, 0.93, 0.5), Material::Grass);
    }
    #[test]
    fn alpine_analytic_macro_slope_matches_centered_derivative() {
        let p = FbmParams {
            recipe: FbmRecipe::Alpine,
            ..Default::default()
        };
        let seed = 7;
        let value = |x: f64, z: f64| {
            crate::alpine::elevation(
                x / p.horizontal_scale,
                z / p.horizontal_scale,
                p,
                false,
                |x, z, salt| noise(x, z, seed ^ salt),
            )
        };
        for (x, z) in [
            (0., 0.),
            (-800., 1600.),
            (1200., 2600.),
            (-250., 3300.),
            (1500., 4600.),
        ] {
            let f = crate::alpine::elevation(
                Field {
                    v: x / p.horizontal_scale,
                    dx: 1.0 / p.horizontal_scale,
                    dz: 0.,
                },
                Field {
                    v: z / p.horizontal_scale,
                    dx: 0.,
                    dz: 1.0 / p.horizontal_scale,
                },
                p,
                false,
                |x: Field, z: Field, salt| {
                    let [v, dx, dz] = noise_gradient(x.v, z.v, seed ^ salt);
                    Field {
                        v,
                        dx: dx * x.dx + dz * z.dx,
                        dz: dx * x.dz + dz * z.dz,
                    }
                },
            );
            let e = 0.001;
            assert!((f.dx - (value(x + e, z) - value(x - e, z)) / (2. * e)).abs() < 1e-6);
            assert!((f.dz - (value(x, z + e) - value(x, z - e)) / (2. * e)).abs() < 1e-6);
        }
    }
}
