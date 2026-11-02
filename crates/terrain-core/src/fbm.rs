//! Fractional Brownian motion heightfield in world meters, with analytic gradient.

use crate::noise::{hash, perlin};

/// Tunable FBM parameters. Heights and lengths are in meters.
#[derive(Clone, Debug, PartialEq)]
pub struct FbmParams {
    pub seed: u32,
    /// Number of noise layers.
    pub octaves: u32,
    /// Wavelength of the first (largest) octave, meters.
    pub base_wavelength: f64,
    /// Height range: the terrain stays within `base_height ± amplitude`.
    pub amplitude: f64,
    /// Height of the terrain's mid level, meters.
    pub base_height: f64,
    /// Frequency multiplier between octaves (usually ~2).
    pub lacunarity: f64,
    /// Amplitude multiplier between octaves (usually ~0.5). Lower = smoother.
    pub gain: f64,
    /// Rotate each octave's lattice to hide axis-aligned artifacts.
    pub rotate_octaves: bool,
}

impl Default for FbmParams {
    fn default() -> Self {
        Self {
            seed: 1,
            octaves: 8,
            base_wavelength: 1024.0,
            amplitude: 120.0,
            base_height: 0.0,
            lacunarity: 2.0,
            gain: 0.5,
            rotate_octaves: true,
        }
    }
}

impl FbmParams {
    /// Names accepted by [`FbmParams::set`].
    pub const KEYS: [&'static str; 8] = [
        "seed",
        "octaves",
        "base_wavelength",
        "amplitude",
        "base_height",
        "lacunarity",
        "gain",
        "rotate_octaves",
    ];

    /// Set a parameter by name from text (for config files and CLI flags).
    /// Returns `Ok(false)` if `key` is not an FBM parameter.
    pub fn set(&mut self, key: &str, value: &str) -> Result<bool, String> {
        fn num<T: std::str::FromStr>(key: &str, value: &str) -> Result<T, String> {
            value
                .parse()
                .map_err(|_| format!("{key}: cannot parse {value:?}"))
        }
        match key {
            "seed" => self.seed = num(key, value)?,
            "octaves" => self.octaves = num(key, value)?,
            "base_wavelength" => self.base_wavelength = num(key, value)?,
            "amplitude" => self.amplitude = num(key, value)?,
            "base_height" => self.base_height = num(key, value)?,
            "lacunarity" => self.lacunarity = num(key, value)?,
            "gain" => self.gain = num(key, value)?,
            "rotate_octaves" => self.rotate_octaves = num(key, value)?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub fn validate(&self) -> Result<(), String> {
        let positive = [
            ("base_wavelength", self.base_wavelength),
            ("lacunarity", self.lacunarity),
            ("gain", self.gain),
        ];
        for (name, v) in positive {
            if !(v.is_finite() && v > 0.0) {
                return Err(format!("{name} must be > 0 (got {v})"));
            }
        }
        if !(self.amplitude.is_finite() && self.amplitude >= 0.0) {
            return Err(format!("amplitude must be >= 0 (got {})", self.amplitude));
        }
        if !self.base_height.is_finite() {
            return Err("base_height must be finite".into());
        }
        if !(1..=24).contains(&self.octaves) {
            return Err(format!("octaves must be 1..=24 (got {})", self.octaves));
        }
        Ok(())
    }

    /// Wavelength of the finest octave, meters. Patch cells must be a few
    /// times smaller than this for the fit to keep that detail.
    pub fn shortest_wavelength(&self) -> f64 {
        self.base_wavelength / self.lacunarity.powi(self.octaves as i32 - 1)
    }
}

/// Height and analytic gradient `[dh/dx, dh/dz]` at a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub height: f64,
    pub gradient: [f64; 2],
}

/// One precomputed layer: `amp * perlin(R * p * freq + offset)`.
#[derive(Clone, Copy, Debug)]
struct Octave {
    freq: f64,
    amp: f64,
    cos: f64,
    sin: f64,
    offset: [f64; 2],
    seed: u32,
}

/// Evaluable FBM heightfield. Construct once, sample many times.
#[derive(Clone, Debug)]
pub struct Fbm {
    params: FbmParams,
    octaves: Vec<Octave>,
}

const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;

impl Fbm {
    pub fn new(params: FbmParams) -> Result<Self, String> {
        params.validate()?;
        // Normalize so the summed octaves stay within ±amplitude.
        let total: f64 = (0..params.octaves)
            .map(|i| params.gain.powi(i as i32))
            .sum();
        let octaves = (0..params.octaves)
            .map(|i| {
                let seed = hash(params.seed ^ hash(i.wrapping_mul(0x9e37_79b9)));
                let angle = if params.rotate_octaves {
                    GOLDEN_ANGLE * i as f64
                } else {
                    0.0
                };
                let unit = |h: u32| h as f64 / u32::MAX as f64;
                Octave {
                    freq: params.lacunarity.powi(i as i32) / params.base_wavelength,
                    amp: params.amplitude * params.gain.powi(i as i32) / total,
                    cos: angle.cos(),
                    sin: angle.sin(),
                    // Shift each lattice so octave zero-crossings don't line up.
                    offset: [unit(hash(seed ^ 1)) * 256.0, unit(hash(seed ^ 2)) * 256.0],
                    seed,
                }
            })
            .collect();
        Ok(Self { params, octaves })
    }

    pub fn params(&self) -> &FbmParams {
        &self.params
    }

    /// Height and exact gradient at world position `(x, z)` in meters.
    pub fn sample(&self, x: f64, z: f64) -> Sample {
        let mut height = self.params.base_height;
        let mut gradient = [0.0; 2];
        for o in &self.octaves {
            // q = R p * freq + offset, with R = [[c, -s], [s, c]].
            let qx = (o.cos * x - o.sin * z) * o.freq + o.offset[0];
            let qz = (o.sin * x + o.cos * z) * o.freq + o.offset[1];
            let (n, g) = perlin(qx, qz, o.seed);
            height += o.amp * n;
            // Chain rule: d/dp = freq * R^T * g.
            let s = o.amp * o.freq;
            gradient[0] += s * (o.cos * g[0] + o.sin * g[1]);
            gradient[1] += s * (-o.sin * g[0] + o.cos * g[1]);
        }
        Sample { height, gradient }
    }

    pub fn height(&self, x: f64, z: f64) -> f64 {
        self.sample(x, z).height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(n: u32, span: f64) -> impl Iterator<Item = (f64, f64)> {
        (0..n).map(move |k| {
            let u = hash(k) as f64 / u32::MAX as f64;
            let v = hash(k ^ 0x5555_5555) as f64 / u32::MAX as f64;
            ((u - 0.5) * span, (v - 0.5) * span)
        })
    }

    #[test]
    fn gradient_matches_finite_differences() {
        let fbm = Fbm::new(FbmParams::default()).unwrap();
        let e = 1e-4;
        for (x, z) in points(2000, 8000.0) {
            let g = fbm.sample(x, z).gradient;
            let dx = (fbm.height(x + e, z) - fbm.height(x - e, z)) / (2.0 * e);
            let dz = (fbm.height(x, z + e) - fbm.height(x, z - e)) / (2.0 * e);
            assert!((g[0] - dx).abs() < 1e-5, "{g:?} vs {dx}");
            assert!((g[1] - dz).abs() < 1e-5, "{g:?} vs {dz}");
        }
    }

    #[test]
    fn stays_within_amplitude() {
        let p = FbmParams {
            base_height: 50.0,
            ..Default::default()
        };
        let fbm = Fbm::new(p.clone()).unwrap();
        for (x, z) in points(20_000, 20_000.0) {
            assert!((fbm.height(x, z) - p.base_height).abs() <= p.amplitude + 1e-9);
        }
    }

    #[test]
    fn deterministic_and_seeded() {
        let a = Fbm::new(FbmParams::default()).unwrap();
        let b = Fbm::new(FbmParams::default()).unwrap();
        let c = Fbm::new(FbmParams {
            seed: 2,
            ..Default::default()
        })
        .unwrap();
        let mut differs = false;
        for (x, z) in points(100, 5000.0) {
            assert_eq!(a.sample(x, z), b.sample(x, z));
            differs |= a.height(x, z) != c.height(x, z);
        }
        assert!(differs);
    }

    #[test]
    fn set_and_validate() {
        let mut p = FbmParams::default();
        assert_eq!(p.set("octaves", "5"), Ok(true));
        assert_eq!(p.octaves, 5);
        assert_eq!(p.set("nope", "1"), Ok(false));
        assert!(p.set("gain", "abc").is_err());
        p.gain = 0.0;
        assert!(p.validate().is_err());
        assert_eq!(FbmParams::default().shortest_wavelength(), 1024.0 / 128.0);
    }
}
