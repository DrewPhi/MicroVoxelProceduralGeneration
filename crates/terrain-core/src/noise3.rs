//! 3D gradient (Perlin) noise and 3D FBM, both with exact analytic gradients.

use crate::noise::hash;

/// The 12 cube-edge gradients of improved Perlin noise.
const G3: [[f64; 3]; 12] = [
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [1.0, -1.0, 0.0],
    [-1.0, -1.0, 0.0],
    [1.0, 0.0, 1.0],
    [-1.0, 0.0, 1.0],
    [1.0, 0.0, -1.0],
    [-1.0, 0.0, -1.0],
    [0.0, 1.0, 1.0],
    [0.0, -1.0, 1.0],
    [0.0, 1.0, -1.0],
    [0.0, -1.0, -1.0],
];

#[inline]
fn corner_gradient(ix: i32, iy: i32, iz: i32, seed: u32) -> [f64; 3] {
    let h = hash((ix as u32) ^ hash((iy as u32) ^ hash((iz as u32) ^ hash(seed))));
    G3[(h % 12) as usize]
}

#[inline]
fn fade(t: f64) -> (f64, f64) {
    let t2 = t * t;
    (
        t2 * t * (t * (t * 6.0 - 15.0) + 10.0),
        30.0 * t2 * (t - 1.0) * (t - 1.0),
    )
}

/// Bound on `|perlin3|`, conservative: the empirical maximum is about 0.97
/// (see tests), and 1.0 is used for the terrain shell.
pub const PERLIN3_BOUND: f64 = 1.0;

/// Scale so the output spans about [-1, 1].
const SCALE: f64 = 0.95;

/// Perlin noise at `p` in lattice units. Returns `(value, gradient)`.
pub fn perlin3(p: [f64; 3], seed: u32) -> (f64, [f64; 3]) {
    let i0 = p.map(|v| v.floor());
    let f = [p[0] - i0[0], p[1] - i0[1], p[2] - i0[2]];
    let i = i0.map(|v| v as i32);
    let fades = f.map(fade);
    let mut value = 0.0;
    let mut grad = [0.0; 3];
    for c in 0..8u32 {
        let b = [c & 1, (c >> 1) & 1, (c >> 2) & 1];
        let g = corner_gradient(
            i[0].wrapping_add(b[0] as i32),
            i[1].wrapping_add(b[1] as i32),
            i[2].wrapping_add(b[2] as i32),
            seed,
        );
        let d = [f[0] - b[0] as f64, f[1] - b[1] as f64, f[2] - b[2] as f64];
        let dot = g[0] * d[0] + g[1] * d[1] + g[2] * d[2];
        // Per-axis weight u (corner bit 1) or 1-u (bit 0), and its derivative.
        let w: [(f64, f64); 3] = std::array::from_fn(|a| {
            let (u, du) = fades[a];
            if b[a] == 1 { (u, du) } else { (1.0 - u, -du) }
        });
        let weight = w[0].0 * w[1].0 * w[2].0;
        value += weight * dot;
        grad[0] += w[0].1 * w[1].0 * w[2].0 * dot + weight * g[0];
        grad[1] += w[0].0 * w[1].1 * w[2].0 * dot + weight * g[1];
        grad[2] += w[0].0 * w[1].0 * w[2].1 * dot + weight * g[2];
    }
    (value * SCALE, grad.map(|g| g * SCALE))
}

/// Parameters of a 3D FBM layer. Lengths in meters.
#[derive(Clone, Debug, PartialEq)]
pub struct Fbm3Params {
    pub seed: u32,
    pub octaves: u32,
    pub base_wavelength: f64,
    pub lacunarity: f64,
    pub gain: f64,
    /// Multiplies vertical frequency: > 1 gives horizontal ledges/strata,
    /// < 1 gives vertical pillars and flutes.
    pub squash: f64,
}

#[derive(Clone, Copy, Debug)]
struct Octave {
    freq: f64,
    amp: f64,
    cos: f64,
    sin: f64,
    offset: [f64; 3],
    seed: u32,
}

/// 3D FBM normalized so `|value| <= PERLIN3_BOUND`.
#[derive(Clone, Debug)]
pub struct Fbm3 {
    squash: f64,
    octaves: Vec<Octave>,
}

const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;

impl Fbm3 {
    pub fn new(p: &Fbm3Params) -> Result<Self, String> {
        if !(1..=16).contains(&p.octaves) {
            return Err(format!("3D octaves must be 1..=16 (got {})", p.octaves));
        }
        for (name, v) in [
            ("3D wavelength", p.base_wavelength),
            ("3D lacunarity", p.lacunarity),
            ("3D gain", p.gain),
            ("squash", p.squash),
        ] {
            if !(v.is_finite() && v > 0.0) {
                return Err(format!("{name} must be > 0 (got {v})"));
            }
        }
        let total: f64 = (0..p.octaves).map(|i| p.gain.powi(i as i32)).sum();
        let unit = |h: u32| h as f64 / u32::MAX as f64;
        let octaves = (0..p.octaves)
            .map(|i| {
                let seed = hash(p.seed ^ hash(i.wrapping_mul(0x85eb_ca6b) ^ 0x3d3d));
                // Rotate about the vertical axis only, so `squash` keeps its meaning.
                let angle = GOLDEN_ANGLE * i as f64;
                Octave {
                    freq: p.lacunarity.powi(i as i32) / p.base_wavelength,
                    amp: p.gain.powi(i as i32) / total,
                    cos: angle.cos(),
                    sin: angle.sin(),
                    offset: [
                        unit(hash(seed ^ 1)) * 256.0,
                        unit(hash(seed ^ 2)) * 256.0,
                        unit(hash(seed ^ 3)) * 256.0,
                    ],
                    seed,
                }
            })
            .collect();
        Ok(Self {
            squash: p.squash,
            octaves,
        })
    }

    /// Value and gradient at world position `p` (meters).
    pub fn sample(&self, p: [f64; 3]) -> (f64, [f64; 3]) {
        let [x, y, z] = p;
        let mut value = 0.0;
        let mut grad = [0.0; 3];
        for o in &self.octaves {
            let q = [
                (o.cos * x - o.sin * z) * o.freq + o.offset[0],
                y * self.squash * o.freq + o.offset[1],
                (o.sin * x + o.cos * z) * o.freq + o.offset[2],
            ];
            let (n, g) = perlin3(q, o.seed);
            value += o.amp * n;
            let s = o.amp * o.freq;
            // Chain rule through q: horizontal rotation, vertical squash.
            grad[0] += s * (o.cos * g[0] + o.sin * g[2]);
            grad[1] += s * self.squash * g[1];
            grad[2] += s * (-o.sin * g[0] + o.cos * g[2]);
        }
        (value, grad)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rand(k: u32) -> f64 {
        hash(k) as f64 / u32::MAX as f64
    }

    #[test]
    fn perlin3_gradient_matches_finite_differences() {
        let e = 1e-6;
        for k in 0..3000 {
            let p = [
                rand(k) * 40.0 - 20.0,
                rand(k ^ 0xa5a5) * 40.0 - 20.0,
                rand(k ^ 0x5a5a_0000) * 40.0 - 20.0,
            ];
            let (_, g) = perlin3(p, 9);
            for a in 0..3 {
                let (mut lo, mut hi) = (p, p);
                lo[a] -= e;
                hi[a] += e;
                let fd = (perlin3(hi, 9).0 - perlin3(lo, 9).0) / (2.0 * e);
                assert!((g[a] - fd).abs() < 1e-5, "axis {a}: {} vs {fd}", g[a]);
            }
        }
    }

    #[test]
    fn perlin3_within_bound() {
        let mut max: f64 = 0.0;
        for k in 0..1_000_000u32 {
            let p = [
                rand(k) * 64.0,
                rand(k ^ 0x1234_5678) * 64.0,
                rand(k ^ 0x8765_4321) * 64.0,
            ];
            max = max.max(perlin3(p, 4).0.abs());
        }
        assert!(max <= PERLIN3_BOUND, "max {max}");
        assert!(max > 0.6, "noise suspiciously flat: {max}");
    }

    #[test]
    fn fbm3_gradient_matches_finite_differences() {
        let f = Fbm3::new(&Fbm3Params {
            seed: 3,
            octaves: 4,
            base_wavelength: 40.0,
            lacunarity: 2.0,
            gain: 0.5,
            squash: 1.7,
        })
        .unwrap();
        let e = 1e-5;
        for k in 0..2000 {
            let p = [
                rand(k) * 500.0,
                rand(k ^ 77) * 200.0 - 100.0,
                rand(k ^ 0xffff) * 500.0,
            ];
            let (_, g) = f.sample(p);
            for a in 0..3 {
                let (mut lo, mut hi) = (p, p);
                lo[a] -= e;
                hi[a] += e;
                let fd = (f.sample(hi).0 - f.sample(lo).0) / (2.0 * e);
                assert!((g[a] - fd).abs() < 1e-6, "axis {a}: {} vs {fd}", g[a]);
            }
        }
    }
}
