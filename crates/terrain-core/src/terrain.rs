//! Implicit 3D terrain: a signed density in meters, solid where `density > 0`.
//!
//! ```text
//! density(x, y, z) = h(x, z) - y + S(x, z) * n3(x, y * squash, z)
//! ```
//! - `h`  : 2D FBM heightfield, the large-scale landscape;
//! - `n3` : 3D FBM in [-1, 1], which carves overhangs, ledges and arches;
//! - `S`  : overhang strength in meters, `amplitude * mask(x, z)`, where the
//!   mask is a smoothstepped low-frequency 2D noise controlled by `coverage`.
//!
//! Because `|n3| <= 1`, the surface always lies inside the shell
//! `h - S <= y <= h + S`: below it everything is solid, above it all air. That
//! keeps searches cheap and gives renderers a heightfield bound for skipping
//! empty space. With `S = 0` the terrain is exactly the 2D heightfield.

use crate::fbm::{Fbm, FbmParams};
use crate::noise::hash;
use crate::noise3::{Fbm3, Fbm3Params, PERLIN3_BOUND};

/// Controls for the 3D overhang layer. Lengths in meters.
#[derive(Clone, Debug, PartialEq)]
pub struct OverhangParams {
    /// Maximum vertical displacement of the surface by 3D noise (0 = heightfield).
    pub amplitude: f64,
    /// Wavelength of the largest 3D feature.
    pub wavelength: f64,
    pub octaves: u32,
    pub lacunarity: f64,
    pub gain: f64,
    /// Vertical frequency multiplier: > 1 ledges/strata, < 1 pillars.
    pub squash: f64,
    /// Fraction of the land where overhangs appear, 0..=1.
    pub coverage: f64,
    /// Size of overhang regions vs. smooth regions.
    pub mask_wavelength: f64,
}

impl Default for OverhangParams {
    fn default() -> Self {
        Self {
            amplitude: 16.0,
            wavelength: 24.0,
            octaves: 4,
            lacunarity: 2.0,
            gain: 0.5,
            squash: 2.0,
            coverage: 0.5,
            mask_wavelength: 600.0,
        }
    }
}

impl OverhangParams {
    pub const KEYS: [&'static str; 8] = [
        "overhang_amplitude",
        "overhang_wavelength",
        "overhang_octaves",
        "overhang_lacunarity",
        "overhang_gain",
        "overhang_squash",
        "overhang_coverage",
        "overhang_mask_wavelength",
    ];

    pub fn set(&mut self, key: &str, value: &str) -> Result<bool, String> {
        fn num<T: std::str::FromStr>(key: &str, value: &str) -> Result<T, String> {
            value
                .parse()
                .map_err(|_| format!("{key}: cannot parse {value:?}"))
        }
        match key {
            "overhang_amplitude" => self.amplitude = num(key, value)?,
            "overhang_wavelength" => self.wavelength = num(key, value)?,
            "overhang_octaves" => self.octaves = num(key, value)?,
            "overhang_lacunarity" => self.lacunarity = num(key, value)?,
            "overhang_gain" => self.gain = num(key, value)?,
            "overhang_squash" => self.squash = num(key, value)?,
            "overhang_coverage" => self.coverage = num(key, value)?,
            "overhang_mask_wavelength" => self.mask_wavelength = num(key, value)?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(self.amplitude.is_finite() && self.amplitude >= 0.0) {
            return Err("overhang_amplitude must be >= 0".into());
        }
        if !(0.0..=1.0).contains(&self.coverage) {
            return Err("overhang_coverage must be in 0..=1".into());
        }
        if !(self.mask_wavelength.is_finite() && self.mask_wavelength > 0.0) {
            return Err("overhang_mask_wavelength must be > 0".into());
        }
        Ok(())
    }
}

/// All terrain parameters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerrainParams {
    pub surface: FbmParams,
    pub overhang: OverhangParams,
}

impl TerrainParams {
    pub fn set(&mut self, key: &str, value: &str) -> Result<bool, String> {
        Ok(self.surface.set(key, value)? || self.overhang.set(key, value)?)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.surface.validate()?;
        self.overhang.validate()
    }
}

/// Density and its gradient at a point. The outward surface normal is
/// `-gradient / |gradient|`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample3 {
    pub density: f64,
    pub gradient: [f64; 3],
}

/// The 2D (y-independent) part of the terrain at one `(x, z)`, so vertical
/// scans only pay for the 3D noise.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Column {
    pub x: f64,
    pub z: f64,
    pub height: f64,
    pub height_gradient: [f64; 2],
    /// Overhang strength `S` in meters, and its gradient.
    pub strength: f64,
    pub strength_gradient: [f64; 2],
}

impl Column {
    /// Vertical range that can contain surface: solid below, air above.
    pub fn shell(&self) -> (f64, f64) {
        let r = self.strength * PERLIN3_BOUND;
        (self.height - r, self.height + r)
    }
}

/// Result of a vertical scan through one column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceScan {
    /// Height of the topmost solid surface.
    pub top: f64,
    /// Density gradient at the top surface.
    pub gradient: [f64; 3],
    /// Air was found below the top surface (an overhang or cave mouth).
    pub overhang: bool,
}

#[derive(Clone, Debug)]
pub struct Terrain {
    params: TerrainParams,
    surface: Fbm,
    overhang: Fbm3,
    mask: Fbm,
}

/// Half-width of the mask's smoothstep transition, in mask-noise units.
const MASK_EDGE: f64 = 0.12;

impl Terrain {
    pub fn new(params: TerrainParams) -> Result<Self, String> {
        params.validate()?;
        let o = &params.overhang;
        let seed = params.surface.seed;
        let overhang = Fbm3::new(&Fbm3Params {
            seed: hash(seed ^ 0x0f0f_1234),
            octaves: o.octaves,
            base_wavelength: o.wavelength,
            lacunarity: o.lacunarity,
            gain: o.gain,
            squash: o.squash,
        })?;
        let mask = Fbm::new(FbmParams {
            seed: hash(seed ^ 0x7777_abcd),
            octaves: 3,
            base_wavelength: o.mask_wavelength,
            amplitude: 1.0,
            base_height: 0.0,
            lacunarity: 2.0,
            gain: 0.5,
            rotate_octaves: true,
        })?;
        Ok(Self {
            surface: Fbm::new(params.surface.clone())?,
            overhang,
            mask,
            params,
        })
    }

    pub fn params(&self) -> &TerrainParams {
        &self.params
    }

    /// Largest possible `|y - h|` of any surface point.
    pub fn max_shell(&self) -> f64 {
        self.params.overhang.amplitude * PERLIN3_BOUND
    }

    /// Overhang mask in [0, 1] and its gradient.
    fn mask(&self, x: f64, z: f64) -> (f64, [f64; 2]) {
        let coverage = self.params.overhang.coverage;
        if coverage <= 0.0 || self.params.overhang.amplitude == 0.0 {
            return (0.0, [0.0; 2]);
        }
        if coverage >= 1.0 {
            return (1.0, [0.0; 2]);
        }
        // The mask noise is roughly symmetric around 0 with most mass in ±0.4;
        // map coverage to a threshold so `coverage` ≈ the solid-mask fraction.
        let threshold = (0.5 - coverage) * 0.8;
        let s = self.mask.sample(x, z);
        let t = (s.height - (threshold - MASK_EDGE)) / (2.0 * MASK_EDGE);
        if t <= 0.0 {
            return (0.0, [0.0; 2]);
        }
        if t >= 1.0 {
            return (1.0, [0.0; 2]);
        }
        let m = t * t * (3.0 - 2.0 * t);
        let dm = 6.0 * t * (1.0 - t) / (2.0 * MASK_EDGE);
        (m, [dm * s.gradient[0], dm * s.gradient[1]])
    }

    pub fn column(&self, x: f64, z: f64) -> Column {
        let h = self.surface.sample(x, z);
        let (m, dm) = self.mask(x, z);
        let a = self.params.overhang.amplitude;
        Column {
            x,
            z,
            height: h.height,
            height_gradient: h.gradient,
            strength: a * m,
            strength_gradient: [a * dm[0], a * dm[1]],
        }
    }

    /// Density and gradient at height `y` within a precomputed column.
    pub fn sample_in(&self, c: &Column, y: f64) -> Sample3 {
        let base = c.height - y;
        if c.strength == 0.0 && c.strength_gradient == [0.0; 2] {
            return Sample3 {
                density: base,
                gradient: [c.height_gradient[0], -1.0, c.height_gradient[1]],
            };
        }
        let (n, g) = self.overhang.sample([c.x, y, c.z]);
        Sample3 {
            density: base + c.strength * n,
            gradient: [
                c.height_gradient[0] + c.strength_gradient[0] * n + c.strength * g[0],
                -1.0 + c.strength * g[1],
                c.height_gradient[1] + c.strength_gradient[1] * n + c.strength * g[2],
            ],
        }
    }

    pub fn density_in(&self, c: &Column, y: f64) -> f64 {
        if c.strength == 0.0 {
            return c.height - y;
        }
        c.height - y + c.strength * self.overhang.sample([c.x, y, c.z]).0
    }

    /// Density and gradient at a world point.
    pub fn sample(&self, x: f64, y: f64, z: f64) -> Sample3 {
        self.sample_in(&self.column(x, z), y)
    }

    pub fn is_solid(&self, x: f64, y: f64, z: f64) -> bool {
        self.density_in(&self.column(x, z), y) > 0.0
    }

    /// Scan a column downward through its shell in `step` increments: find the
    /// top surface (refined by bisection) and whether air lies beneath it.
    /// Features thinner than `step` can be missed.
    pub fn scan(&self, c: &Column, step: f64) -> SurfaceScan {
        let (lo, hi) = c.shell();
        if hi - lo <= 0.0 {
            return SurfaceScan {
                top: c.height,
                gradient: [c.height_gradient[0], -1.0, c.height_gradient[1]],
                overhang: false,
            };
        }
        let mut above = hi;
        let mut y = hi - step;
        loop {
            if y <= lo || self.density_in(c, y) > 0.0 {
                break;
            }
            above = y;
            y -= step;
        }
        let y = y.max(lo);
        // Bisect between solid `y` and air `above`.
        let (mut s, mut a) = (y, above);
        for _ in 0..30 {
            let m = 0.5 * (s + a);
            if self.density_in(c, m) > 0.0 {
                s = m;
            } else {
                a = m;
            }
        }
        let top = 0.5 * (s + a);
        let mut overhang = false;
        let mut probe = y - step;
        while probe > lo {
            if self.density_in(c, probe) <= 0.0 {
                overhang = true;
                break;
            }
            probe -= step;
        }
        SurfaceScan {
            top,
            gradient: self.sample_in(c, top).gradient,
            overhang,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rand(k: u32) -> f64 {
        hash(k) as f64 / u32::MAX as f64
    }

    fn terrain(coverage: f64) -> Terrain {
        let mut p = TerrainParams::default();
        p.overhang.coverage = coverage;
        Terrain::new(p).unwrap()
    }

    #[test]
    fn density_gradient_matches_finite_differences() {
        let t = terrain(0.6);
        let e = 1e-4;
        let mut checked_overhang = 0;
        for k in 0..3000 {
            let (x, z) = (
                rand(k) * 4000.0 - 2000.0,
                rand(k ^ 0xbeef) * 4000.0 - 2000.0,
            );
            let c = t.column(x, z);
            if c.strength > 0.0 {
                checked_overhang += 1;
            }
            let y = c.height + (rand(k ^ 0x1111) - 0.5) * 2.0 * c.strength.max(1.0);
            let g = t.sample(x, y, z).gradient;
            let fd = [
                (t.sample(x + e, y, z).density - t.sample(x - e, y, z).density) / (2.0 * e),
                (t.sample(x, y + e, z).density - t.sample(x, y - e, z).density) / (2.0 * e),
                (t.sample(x, y, z + e).density - t.sample(x, y, z - e).density) / (2.0 * e),
            ];
            for a in 0..3 {
                assert!((g[a] - fd[a]).abs() < 1e-4, "axis {a}: {g:?} vs {fd:?}");
            }
        }
        assert!(checked_overhang > 100, "too few overhang columns tested");
    }

    #[test]
    fn surface_stays_inside_shell() {
        let t = terrain(1.0);
        for k in 0..5000 {
            let (x, z) = (rand(k) * 3000.0, rand(k ^ 0x4242) * 3000.0);
            let c = t.column(x, z);
            let (lo, hi) = c.shell();
            assert!(t.density_in(&c, lo - 1e-6) > 0.0);
            assert!(t.density_in(&c, hi + 1e-6) < 0.0);
        }
    }

    #[test]
    fn zero_amplitude_is_the_heightfield() {
        let mut p = TerrainParams::default();
        p.overhang.amplitude = 0.0;
        let t = Terrain::new(p.clone()).unwrap();
        let f = Fbm::new(p.surface).unwrap();
        for k in 0..500 {
            let (x, z) = (rand(k) * 1000.0, rand(k ^ 9) * 1000.0);
            let h = f.height(x, z);
            assert!(t.is_solid(x, h - 1e-6, z) && !t.is_solid(x, h + 1e-6, z));
            assert_eq!(t.scan(&t.column(x, z), 0.5).top, h);
        }
    }

    #[test]
    fn coverage_controls_overhang_area() {
        let frac = |cov: f64| {
            let t = terrain(cov);
            let n = 4000;
            (0..n)
                .filter(|&k| {
                    t.column(rand(k) * 20_000.0, rand(k ^ 5) * 20_000.0)
                        .strength
                        > 0.0
                })
                .count() as f64
                / n as f64
        };
        let (a, b, c) = (frac(0.0), frac(0.5), frac(1.0));
        assert_eq!(a, 0.0);
        assert_eq!(c, 1.0);
        assert!((0.2..0.8).contains(&b), "coverage 0.5 gave {b}");
    }

    #[test]
    fn scan_finds_top_and_overhangs() {
        let t = terrain(1.0);
        let mut overhangs = 0;
        for k in 0..2000 {
            let (x, z) = (rand(k) * 2000.0, rand(k ^ 0x77) * 2000.0);
            let c = t.column(x, z);
            let s = t.scan(&c, 0.25);
            assert!(
                t.density_in(&c, s.top - 1e-3) > 0.0,
                "below top must be solid"
            );
            // Nothing solid above the top (checked at scan resolution).
            let mut y = s.top + 0.25;
            while y < c.shell().1 {
                assert!(t.density_in(&c, y) <= 0.0);
                y += 0.25;
            }
            overhangs += s.overhang as usize;
        }
        assert!(overhangs > 0, "full coverage should produce some overhangs");
    }
}
