//! 2D gradient (Perlin) noise with an exact analytic gradient.

/// 32-bit integer avalanche hash ("lowbias32", C. Wellons). WGSL-portable.
#[inline]
pub fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

const D: f64 = std::f64::consts::FRAC_1_SQRT_2;
/// Eight unit gradients: axes and diagonals.
const GRADIENTS: [[f64; 2]; 8] = [
    [1.0, 0.0],
    [-1.0, 0.0],
    [0.0, 1.0],
    [0.0, -1.0],
    [D, D],
    [D, -D],
    [-D, D],
    [-D, -D],
];

#[inline]
fn corner_gradient(ix: i32, iz: i32, seed: u32) -> [f64; 2] {
    let h = hash((ix as u32) ^ hash((iz as u32) ^ hash(seed)));
    GRADIENTS[(h >> 29) as usize]
}

/// Quintic fade `6t^5 - 15t^4 + 10t^3` and its derivative `30t^2(t-1)^2`.
#[inline]
fn fade(t: f64) -> (f64, f64) {
    let t2 = t * t;
    (
        t2 * t * (t * (t * 6.0 - 15.0) + 10.0),
        30.0 * t2 * (t - 1.0) * (t - 1.0),
    )
}

/// Scale so the output spans roughly [-1, 1] (raw 2D Perlin peaks near ±1/√2).
const SCALE: f64 = std::f64::consts::SQRT_2;

/// Perlin noise at `(x, z)` in lattice units. Returns `(value, [d/dx, d/dz])`.
///
/// The value is C2-continuous and exactly zero on lattice points.
pub fn perlin(x: f64, z: f64, seed: u32) -> (f64, [f64; 2]) {
    let (x0, z0) = (x.floor(), z.floor());
    let (fx, fz) = (x - x0, z - z0);
    let (ix, iz) = (x0 as i32, z0 as i32);

    let ga = corner_gradient(ix, iz, seed);
    let gb = corner_gradient(ix.wrapping_add(1), iz, seed);
    let gc = corner_gradient(ix, iz.wrapping_add(1), seed);
    let gd = corner_gradient(ix.wrapping_add(1), iz.wrapping_add(1), seed);

    let va = ga[0] * fx + ga[1] * fz;
    let vb = gb[0] * (fx - 1.0) + gb[1] * fz;
    let vc = gc[0] * fx + gc[1] * (fz - 1.0);
    let vd = gd[0] * (fx - 1.0) + gd[1] * (fz - 1.0);

    let (u, du) = fade(fx);
    let (v, dv) = fade(fz);
    let k = va - vb - vc + vd;

    let value = va + u * (vb - va) + v * (vc - va) + u * v * k;
    let mut grad = [0.0; 2];
    for (axis, g) in grad.iter_mut().enumerate() {
        *g = ga[axis]
            + u * (gb[axis] - ga[axis])
            + v * (gc[axis] - ga[axis])
            + u * v * (ga[axis] - gb[axis] - gc[axis] + gd[axis]);
    }
    grad[0] += du * (vb - va + v * k);
    grad[1] += dv * (vc - va + u * k);

    (value * SCALE, [grad[0] * SCALE, grad[1] * SCALE])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_on_lattice_points() {
        for i in -5..5 {
            for j in -5..5 {
                let (v, _) = perlin(i as f64, j as f64, 7);
                assert!(v.abs() < 1e-12);
            }
        }
    }

    #[test]
    fn gradient_matches_finite_differences() {
        let e = 1e-6;
        for k in 0..2000 {
            let x = (hash(k) as f64 / u32::MAX as f64) * 40.0 - 20.0;
            let z = (hash(k + 99_999) as f64 / u32::MAX as f64) * 40.0 - 20.0;
            let (_, g) = perlin(x, z, 3);
            let dx = (perlin(x + e, z, 3).0 - perlin(x - e, z, 3).0) / (2.0 * e);
            let dz = (perlin(x, z + e, 3).0 - perlin(x, z - e, 3).0) / (2.0 * e);
            assert!((g[0] - dx).abs() < 1e-5, "x {x} z {z}: {} vs {dx}", g[0]);
            assert!((g[1] - dz).abs() < 1e-5, "x {x} z {z}: {} vs {dz}", g[1]);
        }
    }

    #[test]
    fn bounded_and_continuous_across_cells() {
        for k in 0..20_000u32 {
            let x = (hash(k) as f64 / u32::MAX as f64) * 64.0 - 32.0;
            let z = (hash(k ^ 0xabcd) as f64 / u32::MAX as f64) * 64.0 - 32.0;
            assert!(perlin(x, z, 11).0.abs() <= 1.0 + 1e-9);
        }
        for i in -8..8 {
            let x = i as f64;
            let left = perlin(x - 1e-9, 0.37, 5).0;
            let right = perlin(x + 1e-9, 0.37, 5).0;
            assert!((left - right).abs() < 1e-7);
        }
    }
}
