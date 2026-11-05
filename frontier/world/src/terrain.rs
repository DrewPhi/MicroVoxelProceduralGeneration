//! CPU-canonical, seeded mountain heightfield, independent of renderer spacing.
//! Greenfield gradient noise/FBM composition; no external implementation copied.
use crate::{Cell, Material, VOXEL_SIZE};

/// Noise/composition and material definition version, independent of the fixture.
pub const FBM_SOURCE_VERSION: u32 = 1;

/// Physical source settings, not display/LOD settings. Changing them changes the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FbmParams {
    /// Base horizontal wavelength in meters, in [16, 10_000_000].
    pub horizontal_scale: f64,
    /// Vertical relief scale in meters, in [0, 100_000].
    pub amplitude: f64,
    /// Number of octaves, in [1, 16]; frequency doubles each octave.
    pub octaves: u32,
    /// Relative amplitude of successive octaves, in [0, 1].
    pub roughness: f64,
}
impl Default for FbmParams {
    fn default() -> Self {
        Self {
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
    /// Same discrete material as the native top voxel. `height` is the canonical
    /// top plane from `height_m`; x/z are reserved for future source variation.
    pub fn surface_material(&self, _x: f64, _z: f64, height: f64) -> Material {
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

fn hash(x: i64, z: i64, seed: u64) -> u64 {
    let mut h = (x as u64).wrapping_mul(0x9e3779b97f4a7c15)
        ^ (z as u64).wrapping_mul(0xbf58476d1ce4e5b9)
        ^ seed;
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d049bb133111eb);
    h ^ (h >> 31)
}
fn noise(x: f64, z: f64, seed: u64) -> f64 {
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
