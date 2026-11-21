//! Bounded reduced affine arithmetic over the unchanged canonical FBM source.
//! Two shared world-coordinate symbols survive; nonlinear/roundoff uncertainty
//! is an independent radius. Multiple noise-cell polynomials share those symbols.
use crate::{terrain::hash, voxel_size, FbmRecipe, FbmTerrain};

#[derive(Clone, Copy, Debug)]
pub struct ColumnBox {
    pub min: [i64; 2],
    pub size_cells: [u32; 2],
}
#[derive(Clone, Copy, Debug)]
pub struct ProxyPlane {
    pub height_at_center: f64,
    pub dx: f64,
    pub dz: f64,
}
#[derive(Clone, Copy, Debug)]
pub struct BoundBudget {
    pub max_noise_cells: u32,
    pub max_cells_per_noise: u32,
}
impl Default for BoundBudget {
    fn default() -> Self {
        Self {
            max_noise_cells: 4096,
            max_cells_per_noise: 4,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ResidualBound {
    pub error_m: f64,
    pub height_range: [f64; 2],
    pub noise_cells: u32,
    pub full_range_fallbacks: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct HeightBound {
    pub center_xz: [f64; 2],
    pub half_extent_m: [f64; 2],
    pub center_height_m: f64,
    pub coefficients: [f64; 2],
    pub remainder_m: f64,
    pub height_range: [f64; 2],
    pub noise_cells: u32,
    pub full_range_fallbacks: u32,
}
impl HeightBound {
    /// The affine source-minus-proxy difference reaches its extrema at triangle
    /// vertices. This avoids extrapolating a skinny fan's plane across its AABB.
    pub fn triangle_error(&self, vertices: [[f64; 3]; 3]) -> Option<f64> {
        self.vertex_error(&vertices)
    }
    /// The two triangles of a quad share two corners. Bound both with one
    /// interval evaluation per corner.
    pub fn quad_error(&self, vertices: [[f64; 3]; 4]) -> Option<f64> {
        self.vertex_error(&vertices)
    }
    /// Conservative discrepancy between any source height in this patch and
    /// any roof height in the given range. Correlation with X/Z is ignored.
    pub fn roof_range_error(&self, [lo, hi]: [f64; 2]) -> f64 {
        I::point(self.height_range[1])
            .sub(I::point(lo))
            .hi
            .max(I::point(hi).sub(I::point(self.height_range[0])).hi)
    }

    /// Bound a piecewise planar surface using every distinct triangle vertex.
    /// The caller supplies the complete vertex set inside this bound.
    pub fn vertex_error(&self, vertices: &[[f64; 3]]) -> Option<f64> {
        let mut error = 0.0_f64;
        for &v in vertices {
            if !v.iter().all(|v| v.is_finite()) {
                return None;
            }
            let xz = [v[0], v[2]];
            if (0..2).any(|a| {
                xz[a] < self.center_xz[a] - self.half_extent_m[a]
                    || xz[a] > self.center_xz[a] + self.half_extent_m[a]
            }) {
                return None;
            }
            if self.height_range == [v[1]; 2] {
                continue;
            }
            let coordinate: [I; 2] = std::array::from_fn(|a| {
                let q = I::point(xz[a])
                    .sub(I::point(self.center_xz[a]))
                    .div(I::point(self.half_extent_m[a]));
                I {
                    lo: q.lo.max(-1.0),
                    hi: q.hi.min(1.0),
                }
            });
            let value = I::point(self.center_height_m)
                .add(I::point(self.coefficients[0]).mul(coordinate[0]))
                .add(I::point(self.coefficients[1]).mul(coordinate[1]))
                .sub(I::point(v[1]));
            error = error.max(up_add(value.abs_max(), self.remainder_m));
        }
        Some(error)
    }
}
/// Every canonical column top over this box, including the native-center
/// offsets and floor quantization. No native source columns are enumerated.
pub fn bound_native_height(
    terrain: &FbmTerrain,
    region: ColumnBox,
    budget: BoundBudget,
) -> Option<HeightBound> {
    if region.size_cells.contains(&0) {
        return None;
    }
    let end = [
        region.min[0] as i128 + region.size_cells[0] as i128,
        region.min[1] as i128 + region.size_cells[1] as i128,
    ];
    if (0..2).any(|a| {
        (region.min[a] as i128 - 1).abs() > (1_i128 << 32) || (end[a] + 1).abs() > (1_i128 << 32)
    }) {
        return None;
    }
    let center =
        [0, 1].map(|a| (region.min[a] as f64 + region.size_cells[a] as f64 * 0.5) * voxel_size());
    let radius = region.size_cells.map(|v| v as f64 * voxel_size() * 0.5);
    let x = Aa {
        c: center[0],
        a: [radius[0], 0.0],
        r: voxel_size() * 0.5,
    };
    let z = Aa {
        c: center[1],
        a: [0.0, radius[1]],
        r: voxel_size() * 0.5,
    };
    let mut context = Context {
        budget,
        cells: 0,
        fallbacks: 0,
    };
    let p = terrain.params();
    let height = elevation(
        terrain,
        x.div_scalar(p.horizontal_scale),
        z.div_scalar(p.horizontal_scale),
        &mut context,
    );
    let height = if height.r == 0. && height.a == [0.; 2] {
        // A proven constant (notably the sea plane) quantizes to one exact
        // height. Giving it an artificial one-voxel error refines flat oceans.
        Aa::point((height.c / voxel_size()).floor() * voxel_size())
    } else {
        height.add(Aa {
            c: -voxel_size() * 0.5,
            a: [0.0; 2],
            r: voxel_size() * 0.5,
        })
    };
    // Interval arithmetic deliberately rounds outward. A proven point
    // needs no arithmetic to express its bounds (including exact sea level).
    let range = if height.r == 0. && height.a == [0.; 2] {
        I::point(height.c)
    } else {
        height.range()
    };
    if ![
        height.c,
        height.a[0],
        height.a[1],
        height.r,
        range.lo,
        range.hi,
    ]
    .iter()
    .all(|v| v.is_finite())
    {
        return None;
    }
    Some(HeightBound {
        center_xz: center,
        half_extent_m: radius,
        center_height_m: height.c,
        coefficients: height.a,
        remainder_m: height.r,
        height_range: [range.lo, range.hi],
        noise_cells: context.cells,
        full_range_fallbacks: context.fallbacks,
    })
}
pub fn bound_proxy_residual(
    terrain: &FbmTerrain,
    region: ColumnBox,
    plane: ProxyPlane,
    budget: BoundBudget,
) -> Option<ResidualBound> {
    if ![plane.height_at_center, plane.dx, plane.dz]
        .iter()
        .all(|v| v.is_finite())
    {
        return None;
    }
    let height = bound_native_height(terrain, region, budget)?;
    let proxy = Aa::coefficients(
        I::point(plane.height_at_center),
        [
            I::point(plane.dx).mul(I::point(height.half_extent_m[0])),
            I::point(plane.dz).mul(I::point(height.half_extent_m[1])),
        ],
        0.0,
    );
    let residual = Aa {
        c: height.center_height_m,
        a: height.coefficients,
        r: height.remainder_m,
    }
    .sub(proxy);
    Some(ResidualBound {
        error_m: residual.range().abs_max(),
        height_range: height.height_range,
        noise_cells: height.noise_cells,
        full_range_fallbacks: height.full_range_fallbacks,
    })
}

#[derive(Clone, Copy, Debug)]
struct I {
    lo: f64,
    hi: f64,
}
impl I {
    fn point(v: f64) -> Self {
        Self { lo: v, hi: v }
    }
    fn add(self, b: Self) -> Self {
        Self {
            lo: (self.lo + b.lo).next_down(),
            hi: (self.hi + b.hi).next_up(),
        }
    }
    fn sub(self, b: Self) -> Self {
        self.add(Self {
            lo: -b.hi,
            hi: -b.lo,
        })
    }
    fn mul(self, b: Self) -> Self {
        let q = [
            self.lo * b.lo,
            self.lo * b.hi,
            self.hi * b.lo,
            self.hi * b.hi,
        ];
        Self {
            lo: q.into_iter().fold(f64::INFINITY, f64::min).next_down(),
            hi: q.into_iter().fold(f64::NEG_INFINITY, f64::max).next_up(),
        }
    }
    fn div(self, b: Self) -> Self {
        if b.lo <= 0.0 && b.hi >= 0.0 {
            return Self {
                lo: f64::NEG_INFINITY,
                hi: f64::INFINITY,
            };
        }
        self.mul(Self {
            lo: (1.0 / b.hi).next_down(),
            hi: (1.0 / b.lo).next_up(),
        })
    }
    fn scale(self, v: f64) -> Self {
        self.mul(Self::point(v))
    }
    fn mid(self) -> f64 {
        self.lo + (self.hi - self.lo) * 0.5
    }
    fn radius(self, c: f64) -> f64 {
        (c - self.lo).abs().max((self.hi - c).abs()).next_up()
    }
    fn abs_max(self) -> f64 {
        self.lo.abs().max(self.hi.abs()).next_up()
    }
}
fn up_add(a: f64, b: f64) -> f64 {
    (a + b).next_up()
}
fn up_mul(a: f64, b: f64) -> f64 {
    (a * b).next_up()
}
fn fp_pad(scale: f64) -> f64 {
    up_add(up_mul(8.0 * f64::EPSILON, scale.abs()), f64::from_bits(1))
}
#[derive(Clone, Copy, Debug)]
struct Aa {
    c: f64,
    a: [f64; 2],
    r: f64,
}
impl Aa {
    fn point(c: f64) -> Self {
        Self {
            c,
            a: [0.0; 2],
            r: 0.0,
        }
    }
    fn interval(lo: f64, hi: f64) -> Self {
        let i = I { lo, hi };
        let c = i.mid();
        Self {
            c,
            a: [0.0; 2],
            r: i.radius(c),
        }
    }
    fn coefficients(c: I, a: [I; 2], r: f64) -> Self {
        let center = c.mid();
        let axes = a.map(I::mid);
        let error = up_add(
            c.radius(center),
            up_add(a[0].radius(axes[0]), a[1].radius(axes[1])),
        );
        Self {
            c: center,
            a: axes,
            r: up_add(r, error),
        }
    }
    fn radius(self) -> f64 {
        up_add(up_add(self.a[0].abs(), self.a[1].abs()), self.r)
    }
    fn range(self) -> I {
        let r = self.radius();
        I {
            lo: (self.c - r).next_down(),
            hi: (self.c + r).next_up(),
        }
    }
    fn add(self, b: Self) -> Self {
        let pad = fp_pad(up_add(self.range().abs_max(), b.range().abs_max()));
        Self::coefficients(
            I::point(self.c).add(I::point(b.c)),
            [0, 1].map(|i| I::point(self.a[i]).add(I::point(b.a[i]))),
            up_add(up_add(self.r, b.r), pad),
        )
    }
    fn neg(self) -> Self {
        Self {
            c: -self.c,
            a: self.a.map(|v| -v),
            r: self.r,
        }
    }
    fn sub(self, b: Self) -> Self {
        self.add(b.neg())
    }
    fn scale(self, k: f64) -> Self {
        self.mul(Self::point(k))
    }
    fn div_scalar(self, k: f64) -> Self {
        if !k.is_finite() || k == 0.0 {
            return Self {
                c: 0.0,
                a: [0.0; 2],
                r: f64::INFINITY,
            };
        }
        let r = up_add(
            (self.r / k.abs()).next_up(),
            fp_pad(self.range().abs_max() / k.abs()),
        );
        Self::coefficients(
            I::point(self.c).div(I::point(k)),
            self.a.map(|a| I::point(a).div(I::point(k))),
            r,
        )
    }
    fn mul(self, b: Self) -> Self {
        let nonlinear = up_add(
            up_add(up_mul(self.c.abs(), b.r), up_mul(b.c.abs(), self.r)),
            up_mul(self.radius(), b.radius()),
        );
        let pad = fp_pad(up_mul(self.range().abs_max(), b.range().abs_max()));
        Self::coefficients(
            I::point(self.c).mul(I::point(b.c)),
            [0, 1].map(|i| {
                I::point(self.c)
                    .mul(I::point(b.a[i]))
                    .add(I::point(b.c).mul(I::point(self.a[i])))
            }),
            up_add(nonlinear, pad),
        )
    }
    fn unary(self, value: I, derivative: I, curvature: f64, operation_pad: f64) -> Self {
        let d = derivative.mid();
        let range = self.radius();
        let error = up_add(
            up_mul(derivative.radius(d), range),
            up_mul(0.5 * curvature, up_mul(range, range)),
        );
        Self::coefficients(
            value,
            [0, 1].map(|i| I::point(d).mul(I::point(self.a[i]))),
            up_add(up_add(up_mul(d.abs(), self.r), error), operation_pad),
        )
    }
    fn fade(self) -> Self {
        let v = I::point(self.c);
        let value = fade_i(v);
        let d = v
            .mul(v)
            .mul(v.sub(I::point(1.0)))
            .mul(v.sub(I::point(1.0)))
            .scale(30.0);
        let range = self.range();
        let mut curvature = fade_second(range.lo).abs().max(fade_second(range.hi).abs());
        for t in [(3.0 - 3.0_f64.sqrt()) / 6.0, (3.0 + 3.0_f64.sqrt()) / 6.0] {
            if t >= range.lo && t <= range.hi {
                curvature = curvature.max(fade_second(t).abs());
            }
        }
        curvature = up_add(curvature, fp_pad(1024.0 * (1.0 + range.abs_max()).powi(3)));
        self.unary(
            value,
            d,
            curvature,
            fp_pad(1024.0 * (1.0 + range.abs_max()).powi(5)),
        )
    }
    fn clamp(self, lo: f64, hi: f64) -> Self {
        let range = self.range();
        if range.hi <= lo {
            return Self::point(lo);
        }
        if range.lo >= hi {
            return Self::point(hi);
        }
        if range.lo >= lo && range.hi <= hi {
            return self;
        }
        self.piecewise(&[lo, hi], |v| v.clamp(lo, hi), &[])
    }
    fn abs(self) -> Self {
        let i = self.range();
        if i.lo >= 0.0 {
            self
        } else if i.hi <= 0.0 {
            self.neg()
        } else {
            self.piecewise(&[0.0], f64::abs, &[])
        }
    }
    fn piecewise(self, breaks: &[f64], f: impl Fn(f64) -> f64, critical: &[f64]) -> Self {
        let i = self.range();
        let slope = if i.hi > i.lo {
            (f(i.hi) - f(i.lo)) / (i.hi - i.lo)
        } else {
            0.0
        };
        let mut residual = I {
            lo: f64::INFINITY,
            hi: f64::NEG_INFINITY,
        };
        for v in [i.lo, i.hi]
            .into_iter()
            .chain(breaks.iter().copied())
            .chain(critical.iter().copied())
        {
            if v < i.lo || v > i.hi {
                continue;
            }
            let y = I::point(f(v)).sub(I::point(slope).mul(I::point(v)));
            residual.lo = residual.lo.min(y.lo);
            residual.hi = residual.hi.max(y.hi);
        }
        let offset = residual.mid();
        let error = residual.radius(offset);
        let scaled = self.scale(slope).add(Self::point(offset));
        Self {
            r: up_add(scaled.r, up_add(error, fp_pad(i.abs_max() + 1.0))),
            ..scaled
        }
    }
    fn legacy_ridge(self) -> Self {
        let g = |n: f64| {
            let r = 1.0 - (n * 1.7).abs().min(1.0);
            0.75 * r * r + 0.25 * n
        };
        let i = self.range();
        let slope = if i.hi > i.lo {
            (g(i.hi) - g(i.lo)) / (i.hi - i.lo)
        } else {
            0.0
        };
        let out = self.piecewise(
            &[-1.0 / 1.7, 0.0, 1.0 / 1.7],
            g,
            &[(slope - 2.8) / 4.335, (slope + 2.3) / 4.335],
        );
        Self {
            r: up_add(out.r, fp_pad(512.0 * (1.0 + i.abs_max()).powi(2))),
            ..out
        }
    }
    fn regional_ridge(self) -> Self {
        let n = I::point(self.c);
        let q = n.mul(n).add(I::point(0.04));
        let root = I {
            lo: q.lo.sqrt().next_down(),
            hi: q.hi.sqrt().next_up(),
        };
        let r = I::point(1.0).sub(root);
        let value = r.mul(r);
        let derivative = n.scale(2.0).sub(n.scale(2.0).div(root));
        let i = self.range();
        let closest = if i.lo <= 0.0 && i.hi >= 0.0 {
            0.0
        } else {
            i.lo.abs().min(i.hi.abs())
        };
        let farthest = i.abs_max();
        let second = |v: f64| 2.0 - 0.08 / (v * v + 0.04).powf(1.5);
        let curvature = up_add(
            second(closest).abs().max(second(farthest).abs()),
            fp_pad(1024.0),
        );
        let taylor = self.unary(
            value,
            derivative,
            curvature,
            fp_pad(512.0 * (1.0 + i.abs_max()).powi(2)),
        );
        let r = Self::point(1.0).sub(self.mul(self).add(Self::point(0.04)).sqrt());
        let generic = r.mul(r);
        if taylor.r < generic.r {
            taylor
        } else {
            generic
        }
    }
    fn sqrt(self) -> Self {
        let range = self.range();
        if range.lo <= 0.0 {
            return Self::interval(0.0, range.hi.max(0.0).sqrt().next_up());
        }
        let value = I {
            lo: self.c.sqrt().next_down(),
            hi: self.c.sqrt().next_up(),
        };
        let derivative = I {
            lo: (0.5 / value.hi).next_down(),
            hi: (0.5 / value.lo).next_up(),
        };
        let lower = (range.lo * range.lo.sqrt().next_down()).next_down();
        let curvature = (0.25 / lower).next_up();
        self.unary(value, derivative, curvature, fp_pad(range.hi.sqrt()))
    }
}
fn fade_i(t: I) -> I {
    t.mul(t)
        .mul(t)
        .mul(t.mul(t.scale(6.0).sub(I::point(15.0))).add(I::point(10.0)))
}
fn fade_second(t: f64) -> f64 {
    60.0 * t * (2.0 * t * t - 3.0 * t + 1.0)
}
fn fade_range<const SECOND: bool>(t: I) -> (I, I, I) {
    let f0 = fade_i(I::point(t.lo));
    let f1 = fade_i(I::point(t.hi));
    let value = I {
        lo: f0.lo,
        hi: f1.hi,
    };
    let mut d = I {
        lo: f64::INFINITY,
        hi: f64::NEG_INFINITY,
    };
    for q in [t.lo, t.hi, 0.0, 0.5, 1.0] {
        if q < t.lo || q > t.hi {
            continue;
        }
        let x = I::point(q);
        let dm = x
            .mul(x)
            .mul(x.sub(I::point(1.0)))
            .mul(x.sub(I::point(1.0)))
            .scale(30.0);
        d.lo = d.lo.min(dm.lo);
        d.hi = d.hi.max(dm.hi);
    }
    if !SECOND {
        return (value, d, I::point(0.0));
    }
    let mut dd = I {
        lo: f64::INFINITY,
        hi: f64::NEG_INFINITY,
    };
    for q in [
        t.lo,
        t.hi,
        (3.0 - 3.0_f64.sqrt()) / 6.0,
        (3.0 + 3.0_f64.sqrt()) / 6.0,
    ] {
        if q < t.lo || q > t.hi {
            continue;
        }
        let x = I::point(q);
        let dm = x
            .scale(60.0)
            .mul(x.mul(x).scale(2.0).sub(x.scale(3.0)).add(I::point(1.0)));
        dd.lo = dd.lo.min(dm.lo);
        dd.hi = dd.hi.max(dm.hi);
    }
    let pad = fp_pad(1024.0 * (1.0 + t.abs_max()).powi(3));
    dd.lo = (dd.lo - pad).next_down();
    dd.hi = (dd.hi + pad).next_up();
    (value, d, dd)
}
fn gradient_pair(i: i64, j: i64, seed: u64) -> [f64; 2] {
    let k = std::f64::consts::FRAC_1_SQRT_2;
    match hash(i, j, seed) & 7 {
        0 => [1.0, 0.0],
        1 => [-1.0, 0.0],
        2 => [0.0, 1.0],
        3 => [0.0, -1.0],
        4 => [k, k],
        5 => [k, -k],
        6 => [-k, k],
        _ => [-k, -k],
    }
}
fn lerp_i(a: I, b: I, t: I) -> I {
    a.add(b.sub(a).mul(t))
}
fn lerp_hessian(a: I, b: I, t: I) -> I {
    if t.lo >= 0.0 && t.hi <= 1.0 {
        I {
            lo: a.lo.min(b.lo),
            hi: a.hi.max(b.hi),
        }
    } else {
        lerp_i(a, b, t)
    }
}
/// Evaluate either the value and U/V gradient at the expansion point, or the
/// UU/UV/VV Hessian over a box in one noise-cell polynomial.
fn noise_jet<const SECOND: bool>(u: I, v: I, gradients: [[f64; 2]; 4]) -> [I; 3] {
    let (fu, du, ddu) = fade_range::<SECOND>(u);
    let (fv, dv, ddv) = fade_range::<SECOND>(v);
    let grad = |dx: usize, dz: usize| {
        let g = gradients[dx + dz * 2];
        (
            u.sub(I::point(dx as f64))
                .scale(g[0])
                .add(v.sub(I::point(dz as f64)).scale(g[1])),
            g,
        )
    };
    let line = |dz: usize| {
        let (g0, a) = grad(0, dz);
        let (g1, b) = grad(1, dz);
        let delta = g1.sub(g0);
        let dax = b[0] - a[0];
        let daz = b[1] - a[1];
        [
            lerp_i(g0, g1, fu),
            I::point(a[0]).add(du.mul(delta)).add(fu.scale(dax)),
            I::point(a[1]).add(fu.scale(daz)),
            if SECOND {
                ddu.mul(delta).add(du.scale(2.0 * dax))
            } else {
                I::point(0.0)
            },
            if SECOND { du.scale(daz) } else { I::point(0.0) },
        ]
    };
    let a = line(0);
    let b = line(1);
    if SECOND {
        [
            lerp_hessian(a[3], b[3], fv),
            lerp_hessian(a[4], b[4], fv).add(dv.mul(b[1].sub(a[1]))),
            dv.mul(b[2].sub(a[2]))
                .scale(2.)
                .add(ddv.mul(b[0].sub(a[0]))),
        ]
    } else {
        [
            lerp_i(a[0], b[0], fv),
            lerp_i(a[1], b[1], fv),
            lerp_i(a[2], b[2], fv).add(dv.mul(b[0].sub(a[0]))),
        ]
    }
    .map(|x| x.scale(1.6))
}
fn cell_gradients(i: i64, j: i64, seed: u64) -> [[f64; 2]; 4] {
    std::array::from_fn(|index| {
        gradient_pair(
            i.wrapping_add((index % 2) as i64),
            j.wrapping_add((index / 2) as i64),
            seed,
        )
    })
}
fn noise_taylor(u: Aa, v: Aa, i: i64, j: i64, seed: u64) -> Aa {
    let gradients = cell_gradients(i, j, seed);
    let point = noise_jet::<false>(I::point(u.c), I::point(v.c), gradients);
    let range = noise_jet::<true>(u.range(), v.range(), gradients);
    taylor_enclosure(u, v, point, range.map(I::abs_max))
}
fn taylor_enclosure(u: Aa, v: Aa, point: [I; 3], hessian: [f64; 3]) -> Aa {
    let ru = u.radius();
    let rv = v.radius();
    let first = up_add(
        up_mul(point[1].abs_max(), u.r),
        up_mul(point[2].abs_max(), v.r),
    );
    let second = up_add(
        up_add(
            up_mul(hessian[0], up_mul(ru, ru)),
            up_mul(2. * hessian[1], up_mul(ru, rv)),
        ),
        up_mul(hessian[2], up_mul(rv, rv)),
    );
    // Cover floating-point evaluation of the source gradient, fade and interpolation.
    let r = up_add(up_add(first, up_mul(0.5, second)), fp_pad(4096.0));
    Aa::coefficients(
        point[0],
        [0, 1].map(|a| {
            point[1]
                .mul(I::point(u.a[a]))
                .add(point[2].mul(I::point(v.a[a])))
        }),
        r,
    )
    .clamp(-1.0, 1.0)
}
// Quintic fade has zero first and second derivatives at each cell edge.
// Shared corner gradients therefore define a C2 field across all crossed cells.
// Bound its Hessian on each cell intersection and expand once at the real center.
fn crossing_taylor(x: Aa, z: Aa, ix: I, iz: I, lo: [i64; 2], hi: [i64; 2], seed: u64) -> Aa {
    let cell = [x.c.floor() as i64, z.c.floor() as i64];
    let coord = |at: f64, origin: i64| I::point(at).sub(I::point(origin as f64));
    let point = noise_jet::<false>(
        coord(x.c, cell[0]),
        coord(z.c, cell[1]),
        cell_gradients(cell[0], cell[1], seed),
    );
    let clip = |range: I, n: i64| {
        let r = range.sub(I::point(n as f64));
        I {
            lo: r.lo.max(0.),
            hi: r.hi.min(1.),
        }
    };
    let mut hessian = [0.0_f64; 3];
    for j in lo[1]..=hi[1] {
        for i in lo[0]..=hi[0] {
            let jet = noise_jet::<true>(clip(ix, i), clip(iz, j), cell_gradients(i, j, seed));
            for k in 0..3 {
                hessian[k] = hessian[k].max(jet[k].abs_max());
            }
        }
    }
    taylor_enclosure(x, z, point, hessian)
}
struct Context {
    budget: BoundBudget,
    cells: u32,
    fallbacks: u32,
}
#[cfg(test)]
thread_local! {
    // Compare against the original whole-box polynomial extensions in CPU tests.
    static RETAIN_NOISE_BRANCHES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
fn noise(x: Aa, z: Aa, seed: u64, c: &mut Context) -> Aa {
    let ix = x.range();
    let iz = z.range();
    if !ix.lo.is_finite() || !ix.hi.is_finite() || !iz.lo.is_finite() || !iz.hi.is_finite() {
        c.fallbacks += 1;
        return Aa::interval(-1.0, 1.0);
    }
    let lo = [ix.lo.floor() as i64, iz.lo.floor() as i64];
    let hi = [ix.hi.floor() as i64, iz.hi.floor() as i64];
    let count = (hi[0] as i128 - lo[0] as i128 + 1) * (hi[1] as i128 - lo[1] as i128 + 1);
    if count <= 0
        || count > c.budget.max_cells_per_noise as i128
        || count > c.budget.max_noise_cells.saturating_sub(c.cells) as i128
    {
        c.fallbacks += 1;
        return Aa::interval(-1.0, 1.0);
    }
    c.cells += count as u32;
    #[cfg(test)]
    if RETAIN_NOISE_BRANCHES.get() {
        return retained_cell_extension(x, z, lo, hi, seed);
    }
    if count == 1 {
        single_cell_noise(x, z, lo[0], lo[1], seed)
    } else {
        crossing_taylor(x, z, ix, iz, lo, hi, seed)
    }
}

fn single_cell_noise(x: Aa, z: Aa, i: i64, j: i64, seed: u64) -> Aa {
    let u = x.sub(Aa::point(i as f64));
    let v = z.sub(Aa::point(j as f64));
    let fu = u.fade();
    let fv = v.fade();
    let grad = |dx: i64, dz: i64| {
        let a = u.sub(Aa::point(dx as f64));
        let b = v.sub(Aa::point(dz as f64));
        let k = std::f64::consts::FRAC_1_SQRT_2;
        match hash(i.wrapping_add(dx), j.wrapping_add(dz), seed) & 7 {
            0 => a,
            1 => a.neg(),
            2 => b,
            3 => b.neg(),
            4 => a.add(b).scale(k),
            5 => a.sub(b).scale(k),
            6 => a.neg().add(b).scale(k),
            _ => a.neg().sub(b).scale(k),
        }
    };
    let lerp = |a: Aa, b: Aa, t: Aa| a.add(b.sub(a).mul(t));
    let ordinary = lerp(
        lerp(grad(0, 0), grad(1, 0), fu),
        lerp(grad(0, 1), grad(1, 1), fu),
        fv,
    )
    .scale(1.6)
    .clamp(-1.0, 1.0);
    let taylor = noise_taylor(u, v, i, j, seed);
    if taylor.r < ordinary.r {
        taylor
    } else {
        ordinary
    }
}

#[cfg(test)]
fn retained_cell_extension(x: Aa, z: Aa, lo: [i64; 2], hi: [i64; 2], seed: u64) -> Aa {
    let mut branches = Vec::new();
    for j in lo[1]..=hi[1] {
        for i in lo[0]..=hi[0] {
            branches.push(single_cell_noise(x, z, i, j, seed));
        }
    }
    // Test oracle: retain the previous Vec implementation verbatim.
    let reference = branches[0];
    let mut radius = reference.r;
    for b in branches {
        let delta = up_add(
            I::point(b.c).sub(I::point(reference.c)).abs_max(),
            up_add(
                I::point(b.a[0]).sub(I::point(reference.a[0])).abs_max(),
                I::point(b.a[1]).sub(I::point(reference.a[1])).abs_max(),
            ),
        );
        radius = radius.max(up_add(delta, b.r));
    }
    Aa {
        r: radius,
        ..reference
    }
}
impl crate::alpine::Value for Aa {
    fn point(v: f64) -> Self {
        Self::point(v)
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
        self.clamp(0.0, 1.0)
    }
    fn outside(self, lo: f64, hi: f64) -> bool {
        let range = self.range();
        range.hi < lo || range.lo > hi
    }
    fn is_zero(self) -> bool {
        self.c == 0. && self.a == [0.; 2] && self.r == 0.
    }
}
/// Exact extrema structure for an independently interval-bounded positive
/// weighted mean. Material/profile intervals are monotone inputs, so their
/// lower endpoints minimize and upper endpoints maximize. With targets fixed,
/// d(mean)/d(weight_i) has sign target_i-mean. Thus at a minimum all targets
/// below the optimum take maximum weight, all above take minimum weight; at
/// a maximum the choices reverse. Sorting targets leaves only n+1 threshold
/// assignments to check, including equal-target optima. Outward I arithmetic
/// encloses each assignment; no sampled source extrema enter this proof.
fn positive_weighted_mean_range(base: I, contributions: &[(I, I)]) -> I {
    let extreme = |upper: bool| {
        let mut ordered: Vec<_> = contributions
            .iter()
            .map(|(w, p)| (*w, if upper { p.hi } else { p.lo }))
            .collect();
        ordered.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut numerator = I::point(if upper { base.hi } else { base.lo });
        let mut denominator = I::point(1.);
        for (w, p) in &ordered {
            let weight = I::point(if upper { w.hi } else { w.lo });
            numerator = numerator.add(weight.scale(*p));
            denominator = denominator.add(weight);
        }
        let ratio = |numerator: I, denominator: I| {
            // Every true denominator includes the fixed unit base weight.
            numerator.div(I {
                lo: denominator.lo.max(1.),
                hi: denominator.hi.max(1.),
            })
        };
        let first = ratio(numerator, denominator);
        let mut result = if upper { first.hi } else { first.lo };
        for (w, p) in ordered {
            let delta = if upper {
                I::point(w.lo).sub(I::point(w.hi))
            } else {
                I::point(w.hi).sub(I::point(w.lo))
            };
            numerator = numerator.add(delta.scale(p));
            denominator = denominator.add(delta);
            let candidate = ratio(numerator, denominator);
            result = if upper {
                result.max(candidate.hi)
            } else {
                result.min(candidate.lo)
            };
        }
        result
    };
    I {
        lo: extreme(false),
        hi: extreme(true),
    }
}

/// Distance to a fixed segment is 1-Lipschitz. Evaluate its center with the
/// same outward arithmetic and extend by the box circumradius. Intersecting
/// this with the ordinary interval avoids the lost x/z correlation in u²+v²
/// (especially for diagonal reaches) without rejecting an unsampled feature.
fn segment_distance_lipschitz(x: Aa, z: Aa, a: [f64; 2], b: [f64; 2]) -> I {
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let denominator = dx * dx + dz * dz;
    let cx = Aa::point(x.c);
    let cz = Aa::point(z.c);
    let f = if denominator > 0. {
        cx.sub(Aa::point(a[0]))
            .scale(dx)
            .add(cz.sub(Aa::point(a[1])).scale(dz))
            .div_scalar(denominator)
            .clamp(0., 1.)
    } else {
        Aa::point(0.)
    };
    let u = cx.sub(Aa::point(a[0])).sub(f.scale(dx));
    let v = cz.sub(Aa::point(a[1])).sub(f.scale(dz));
    let center = u.mul(u).add(v.mul(v)).sqrt().range();
    let rx = x.range().sub(I::point(x.c)).abs_max();
    let rz = z.range().sub(I::point(z.c)).abs_max();
    let radius = Aa::point(rx)
        .mul(Aa::point(rx))
        .add(Aa::point(rz).mul(Aa::point(rz)))
        .sqrt()
        .range()
        .hi;
    // Reserve the finite-precision projection arithmetic at every point,
    // including nearly coincident endpoints; canonical source uses f64 too.
    let magnitude = x.range().abs_max()
        + z.range().abs_max()
        + a[0].abs()
        + a[1].abs()
        + b[0].abs()
        + b[1].abs()
        + radius
        + 1.;
    let radius = up_add(radius, fp_pad(512. * magnitude));
    I {
        lo: (center.lo - radius).next_down().max(0.),
        hi: (center.hi + radius).next_up(),
    }
}

/// Outward-rounded evaluation of SeededWorld's geometry. Climate changes only
/// materials. Keep this composition paired with the canonical source: the
/// held-out tests below exercise land, water, banks and cache boundaries.
fn seeded_elevation(t: &FbmTerrain, x: Aa, z: Aa, c: &mut Context) -> Aa {
    let p = t.params();
    let sx = x.scale(1800.).div_scalar(p.horizontal_scale);
    let sz = z.scale(1800.).div_scalar(p.horizontal_scale);
    let mut offset_band = |sx: Aa, sz: Aa, wavelength: f64, count: u32, gain: f64, salt: u64| {
        let (mut weight, mut frequency, mut sum, mut norm) =
            (1., 1. / wavelength, Aa::point(0.), 0.);
        for octave in 0..count {
            let n = noise(
                sx.scale(frequency).add(Aa::point(13.7)),
                sz.scale(frequency).sub(Aa::point(8.3)),
                t.seed() ^ salt.wrapping_add(octave as u64 * 0x9e3779b9),
                c,
            );
            sum = sum.add(n.scale(weight));
            norm += weight;
            weight *= gain;
            frequency *= 2.;
        }
        sum.div_scalar(norm)
    };
    let mut band = |w, n, gain, salt| offset_band(sx, sz, w, n, gain, salt);
    let continent = band(48_000., 3, 0.45, 0x5101);
    let province = band(22_000., 3, 0.45, 0x5102);
    let m = province.add(Aa::point(0.08)).div_scalar(0.48).clamp(0., 1.);
    let mountain = m.mul(m).mul(Aa::point(3.).sub(m.scale(2.)));
    let ridge = Aa::point(1.).sub(band(3600., 4, 0.48, 0x5105).abs());
    let hills = band(1800., 3, 0.45, 0x5106);
    let fine = band(90., p.octaves, 0.65 * p.roughness, 0x5107);
    let authored = if p.recipe == FbmRecipe::Highlands {
        let spur = Aa::point(1.).sub(
            offset_band(
                sx.add(hills.scale(410.)),
                sz.sub(hills.scale(260.)),
                960.,
                4,
                0.52,
                0x5174,
            )
            .abs(),
        );
        crate::terrain::seeded_world::highland_relief(continent, mountain, ridge, hills, fine, spur)
    } else if p.recipe.uses_watersheds() {
        let strata = band(1050., 3, 0.45, 0x5121);
        crate::terrain::seeded_world::watershed_relief(
            continent, mountain, ridge, hills, fine, strata,
        )
    } else {
        continent
            .scale(400.)
            .add(Aa::point(65.))
            .add(mountain.mul(mountain).mul(ridge.mul(ridge)).scale(1150.))
            .add(hills.scale(45.))
            .add(fine.scale(3.))
    };
    let base = authored.scale(p.amplitude / 1000.);
    let rx = x.range();
    let rz = z.range();
    let spacing = 2400. * p.horizontal_scale / 1800.;
    if rx.hi - rx.lo > spacing * 4. || rz.hi - rz.lo > spacing * 4. {
        // Huge root regions use the proven finite source envelope; no graph
        // search or mistaken empty candidate list may certify absent channels.
        let [lo, hi] = crate::terrain::seeded_world::global_height_range(p);
        return Aa::interval(lo.next_down(), hi.next_up());
    }
    let reaches = t.seeded_reaches([rx.lo, rz.lo], [rx.hi, rz.hi]);
    let smooth = |v: I| {
        let f = |q: f64| {
            let q = I::point(q.clamp(0., 1.));
            q.mul(q).mul(I::point(3.).sub(q.scale(2.)))
        };
        I {
            lo: f(v.lo).lo,
            hi: f(v.hi).hi,
        }
    };
    let base_range = base.range();
    // Each entry is one positive influence weight and its target valley
    // profile. Normalize with bounded coefficients instead of dividing two
    // huge, correlated interval sums near a river centerline.
    let mut contributions: Vec<(I, I)> = Vec::new();
    let mut stage_upper = 0.0_f64;
    let mut guaranteed_stage: Option<f64> = None;
    let mut guaranteed_head = 0.;
    let mut original_head = 0.;
    let mut original_floor_delta = 0.0_f64;
    let mut possible_lake_lower = f64::INFINITY;
    let mut wet_lakes = Vec::new();
    let mut only_sea_reaches = true;
    let mut inset = None;
    for r in reaches {
        let dx = r.b.x - r.a.x;
        let dz = r.b.z - r.a.z;
        let denom = dx * dx + dz * dz;
        let fraction = if denom > 0. {
            x.sub(Aa::point(r.a.x))
                .scale(dx)
                .add(z.sub(Aa::point(r.a.z)).scale(dz))
                .div_scalar(denom)
                .clamp(0., 1.)
        } else {
            Aa::point(0.)
        };
        let u = x.sub(Aa::point(r.a.x)).sub(fraction.scale(dx));
        let v = z.sub(Aa::point(r.a.z)).sub(fraction.scale(dz));
        let ordinary = u.mul(u).add(v.mul(v)).sqrt().range();
        let tight = segment_distance_lipschitz(x, z, [r.a.x, r.a.z], [r.b.x, r.b.z]);
        let distance = I {
            lo: ordinary.lo.max(tight.lo),
            hi: ordinary.hi.min(tight.hi),
        };
        let stage = Aa::point(r.a.stage)
            .add(fraction.scale(r.b.stage - r.a.stage))
            .range();
        let width = Aa::point(r.width)
            .add(fraction.scale(r.width_b - r.width))
            .range();
        let outer = r.outer_radius();
        if distance.lo >= outer || stage.hi < 0. {
            continue;
        }
        only_sea_reaches &= r.a.stage == 0. && r.b.stage == 0.;
        let fully_active = stage.lo >= 0. && distance.hi < outer;
        let d = I {
            lo: distance.lo.max(0.),
            hi: distance.hi.min(outer).max(0.),
        };
        let stage = I {
            lo: stage.lo.max(0.),
            hi: stage.hi.max(0.),
        };
        let amplitude = p.amplitude / 1000.;
        let depth = r.depth * amplitude;
        let bank = I::point(0.7 * amplitude);
        let shaped = p.has_shaped_banks() && d.lo < width.scale(1.8).hi;
        let bank_d = if shaped {
            use crate::terrain::seeded_world::{bank_inset, BAR_BAND, COVE_BAND};
            let inset = *inset.get_or_insert_with(|| {
                let mut band =
                    |(w, salt)| noise(sx.div_scalar(w), sz.div_scalar(w), t.seed() ^ salt, c);
                bank_inset(band(BAR_BAND), band(COVE_BAND)).range()
            });
            let r = d.div(width);
            let positive = I {
                lo: inset.lo.max(0.),
                hi: inset.hi.max(0.),
            };
            let negative = I {
                lo: inset.lo.min(0.),
                hi: inset.hi.min(0.),
            };
            let bar =
                positive.mul(I::point(1.).sub(smooth(r.sub(I::point(1.)).div(I::point(0.8)))));
            let cove =
                negative.mul(I::point(1.).sub(smooth(r.sub(I::point(0.66)).div(I::point(0.34)))));
            let limit = I::point(32.).scale(p.horizontal_scale).div(I::point(1800.));
            let lever = I {
                lo: d.lo.min(width.lo).min(limit.lo),
                hi: d.hi.min(width.hi).min(limit.hi),
            };
            d.add(lever.mul(bar.add(cove)))
        } else {
            d
        };
        let inner = |d: I| {
            stage
                .sub(I::point(depth))
                .add(I::point(depth).add(bank).mul(smooth(d.div(width))))
        };
        let outer_profile = |d: I| {
            let q = smooth(d.sub(width).div(I::point(outer).sub(width)));
            stage.add(bank).add(base_range.sub(stage).sub(bank).mul(q))
        };
        let profile_at = |bank_d: I| {
            let mut profile = if bank_d.hi <= width.lo {
                inner(bank_d)
            } else if bank_d.lo >= width.hi {
                outer_profile(bank_d)
            } else {
                let a = inner(I {
                    lo: bank_d.lo,
                    hi: bank_d.hi.min(width.hi),
                });
                let b = outer_profile(I {
                    lo: bank_d.lo.max(width.lo),
                    hi: bank_d.hi,
                });
                I {
                    lo: a.lo.min(b.lo),
                    hi: a.hi.max(b.hi),
                }
            };
            if p.recipe.uses_watersheds() {
                profile.lo = profile.lo.min(base_range.lo);
                profile.hi = profile.hi.min(base_range.hi);
            } else if stage.lo == 0. && base_range.lo < 0. {
                // The source permits cuts, but no raised banks, where a zero-stage
                // reach is already below sea level.
                profile.lo = profile.lo.min(base_range.lo);
                if r.a.stage == 0. && r.b.stage == 0. && base_range.hi < 0. {
                    profile.hi = profile.hi.min(base_range.hi);
                }
            }
            profile
        };
        let profile = profile_at(bank_d);
        if shaped {
            // Both beds use identical positive weights, including the original
            // land with weight one. Their difference lies within the convex
            // hull of the individual profile differences.
            original_floor_delta = original_floor_delta.min(profile_at(d).sub(profile).lo);
        }
        let taper = I::point(1.).sub(smooth(d.div(I::point(outer))));
        let square = I {
            lo: (d.lo * d.lo).next_down().max(0.),
            hi: (d.hi * d.hi).next_up(),
        };
        let epsilon = width.mul(width).scale(1e-8);
        let denominator = I {
            lo: square.lo.max(epsilon.lo),
            hi: square.hi.max(epsilon.hi),
        };
        let raw = I::point(outer * outer)
            .div(denominator)
            .mul(taper)
            .mul(taper);
        let weight = I {
            lo: if fully_active { raw.lo.max(0.) } else { 0. },
            hi: raw.hi.max(0.),
        };
        contributions.push((weight, profile));
        if r.lake && p.recipe.uses_watersheds() && !p.legacy_riverbanks {
            possible_lake_lower = possible_lake_lower.min(stage.lo);
        }
        if d.lo < width.hi {
            stage_upper = stage_upper.max(stage.hi);
            if r.lake {
                possible_lake_lower = possible_lake_lower.min(stage.lo);
            }
            if fully_active && distance.hi < width.lo {
                if guaranteed_stage.is_none_or(|old| stage.lo > old) {
                    guaranteed_stage = Some(stage.lo);
                    original_head = I::point(depth)
                        .sub(I::point(depth).add(bank).mul(smooth(d.div(width))))
                        .lo
                        .max(0.);
                    guaranteed_head = I::point(depth)
                        .sub(I::point(depth).add(bank).mul(smooth(bank_d.div(width))))
                        .lo
                        .max(0.);
                }
                if r.lake && profile.hi < stage.lo {
                    wet_lakes.push(stage);
                }
            }
        }
    }
    if only_sea_reaches && base_range.hi < 0. {
        return Aa::point(0.);
    }
    if contributions.is_empty() {
        // Preserve coordinate correlation away from hydrology influence.
        if base_range.lo >= 0. {
            return base;
        }
        if base_range.hi <= 0. {
            return Aa::point(0.);
        }
        return base.add(base.abs()).scale(0.5);
    }
    let sum = contributions
        .iter()
        .fold(I::point(0.), |sum, (w, _)| sum.add(*w));
    let mut ground = base_range;
    let mut hull = base_range;
    let mut magnitude = base_range.abs_max();
    for (w, profile) in &contributions {
        hull.lo = hull.lo.min(profile.lo);
        hull.hi = hull.hi.max(profile.hi);
        magnitude = magnitude.max(profile.abs_max());
        let other_lo = (sum.lo - w.hi).next_down().max(0.);
        let other_hi = (sum.hi - w.lo).next_up().max(0.);
        let alpha = I {
            lo: I::point(w.lo)
                .div(I::point(1.).add(I::point(w.lo)).add(I::point(other_hi)))
                .lo
                .max(0.),
            hi: I::point(w.hi)
                .div(I::point(1.).add(I::point(w.hi)).add(I::point(other_lo)))
                .hi
                .min(1.),
        };
        ground = ground.add(alpha.mul(profile.sub(base_range)));
    }
    // Preserve the shared normalization instead of treating each alpha as an
    // independent contribution. Intersect independent conservative enclosures.
    let normalized = positive_weighted_mean_range(base_range, &contributions);
    ground.lo = ground.lo.max(normalized.lo);
    ground.hi = ground.hi.min(normalized.hi);
    // Positive normalization is a convex combination. Reserve summation and
    // normalization roundoff even when different cache regions enumerate the
    // same contributing descriptors in a different order.
    let roundoff = fp_pad(64. * (contributions.len() + 1) as f64 * magnitude);
    ground.lo = (ground.lo.max(hull.lo) - roundoff).next_down();
    ground.hi = (ground.hi.min(hull.hi) + roundoff).next_up();
    let bounded_stage = |stage: I, ground: I| {
        if !p.recipe.uses_watersheds() {
            return stage;
        }
        use crate::terrain::seeded_world::{WATERSHED_MAX_WATER_DEPTH, WATERSHED_WATER_ABOVE_LAND};
        let a = p.amplitude / 1000.;
        let land_ceiling = base_range.add(I::point(WATERSHED_WATER_ABOVE_LAND * a));
        let depth_ceiling = ground.add(I::point(WATERSHED_MAX_WATER_DEPTH * a));
        I {
            lo: stage
                .lo
                .min(land_ceiling.lo.max(0.))
                .min(depth_ceiling.lo.max(0.)),
            hi: stage
                .hi
                .min(land_ceiling.hi.max(0.))
                .min(depth_ceiling.hi.max(0.)),
        }
    };
    if let Some(stage) = guaranteed_stage {
        let mut lower = stage.min(possible_lake_lower);
        if p.recipe.uses_watersheds() && !p.legacy_riverbanks {
            lower = lower.min(
                I::point(ground.lo)
                    .add(I::point(guaranteed_head))
                    .lo
                    .max(0.),
            );
        }
        if p.has_shaped_banks() {
            let original_ground = I::point(ground.lo)
                .add(I::point(original_floor_delta))
                .sub(I::point(2. * roundoff));
            let old_head = original_head.min(
                crate::terrain::seeded_world::WATERSHED_MAX_WATER_DEPTH * (p.amplitude / 1000.),
            );
            lower = lower.min(original_ground.add(I::point(old_head)).lo.max(0.));
        }
        // Prove a wholly wet region without carrying its hidden bed into the
        // visible error. Test the signed weighted excess above the water level;
        // this stays useful even when several huge centerline weights overlap.
        let mut excess = base_range.sub(I::point(lower));
        for (w, profile) in &contributions {
            excess = excess.add(w.mul(profile.sub(I::point(lower))));
        }
        let error = fp_pad(64. * (contributions.len() + 1) as f64 * magnitude * (1. + sum.hi));
        if excess.hi + error < 0. {
            ground.hi = ground
                .hi
                .min((lower + (excess.hi + error) / (1. + sum.hi)).next_up());
        }
        for lake in wet_lakes {
            if ground.hi < lake.lo && ground.hi < possible_lake_lower {
                stage_upper = stage_upper.min(lake.hi);
            }
        }
        let stage = bounded_stage(
            I {
                lo: lower,
                hi: stage_upper,
            },
            ground,
        );
        Aa::interval(
            ground.lo.max(0.).max(stage.lo),
            ground.hi.max(stage.hi).max(0.),
        )
    } else {
        let stage = bounded_stage(
            I {
                lo: 0.,
                hi: stage_upper,
            },
            ground,
        );
        Aa::interval(ground.lo.max(0.), ground.hi.max(stage.hi).max(0.))
    }
}

fn elevation(t: &FbmTerrain, x: Aa, z: Aa, c: &mut Context) -> Aa {
    let p = t.params();
    let seed = t.seed();
    if p.recipe.has_static_water() {
        return seeded_elevation(
            t,
            x.scale(p.horizontal_scale),
            z.scale(p.horizontal_scale),
            c,
        );
    }
    if p.recipe == FbmRecipe::Alpine {
        return crate::alpine::elevation(x, z, p, true, |x, z, salt| noise(x, z, seed ^ salt, c));
    }
    let (wx, wz) = if p.recipe == FbmRecipe::Legacy {
        (
            x.add(
                noise(
                    x.scale(0.55).add(Aa::point(17.3)),
                    z.scale(0.55).sub(Aa::point(8.7)),
                    seed ^ 0x1234,
                    c,
                )
                .scale(0.32),
            ),
            z.add(
                noise(
                    x.scale(0.55).sub(Aa::point(31.9)),
                    z.scale(0.55).add(Aa::point(4.2)),
                    seed ^ 0x5678,
                    c,
                )
                .scale(0.32),
            ),
        )
    } else if p.recipe == FbmRecipe::Regional {
        (
            x.add(
                noise(
                    x.scale(0.4).add(Aa::point(17.3)),
                    z.scale(0.4).sub(Aa::point(8.7)),
                    seed ^ 0x1234,
                    c,
                )
                .scale(100.0 / 1800.0),
            ),
            z.add(
                noise(
                    x.scale(0.4).sub(Aa::point(31.9)),
                    z.scale(0.4).add(Aa::point(4.2)),
                    seed ^ 0x5678,
                    c,
                )
                .scale(100.0 / 1800.0),
            ),
        )
    } else {
        (x, z)
    };
    if p.recipe == FbmRecipe::Legacy {
        let broad = noise(
            wx.scale(0.42).add(Aa::point(3.1)),
            wz.scale(0.42).sub(Aa::point(7.8)),
            seed ^ 0x9abc,
            c,
        );
        let (mut sum, mut norm, mut weight, mut frequency) = (Aa::point(0.0), 0.0, 1.0, 1.0);
        for octave in 0..p.octaves {
            let n = noise(
                wx.scale(frequency).add(Aa::point(12.7)),
                wz.scale(frequency).sub(Aa::point(29.1)),
                seed.wrapping_add(octave as u64 * 0x9e3779b9),
                c,
            );
            sum = sum.add(n.legacy_ridge().scale(weight));
            norm += weight;
            weight *= p.roughness;
            frequency *= 2.0;
        }
        return sum
            .scale(0.85)
            .div_scalar(norm)
            .add(broad.scale(0.4))
            .sub(Aa::point(0.12))
            .scale(p.amplitude);
    }
    let gain = 0.8 * p.roughness;
    let bands = |wavelength: f64, count: u32, salt: u64, ridged: bool, c: &mut Context| {
        let (mut sum, mut norm, mut weight, mut frequency) =
            (Aa::point(0.0), 0.0, 1.0, 1800.0 / wavelength);
        for octave in 0..count {
            let n = noise(
                wx.scale(frequency).add(Aa::point(12.7)),
                wz.scale(frequency).sub(Aa::point(29.1)),
                seed ^ salt.wrapping_add(octave as u64 * 0x9e3779b9),
                c,
            );
            let value = if ridged { n.regional_ridge() } else { n };
            sum = sum.add(value.scale(weight));
            norm += weight;
            weight *= gain;
            frequency *= 2.0;
        }
        sum.div_scalar(norm)
    };
    let province = noise(
        wx.scale(0.3).add(Aa::point(3.1)),
        wz.scale(0.3).sub(Aa::point(7.8)),
        seed ^ 0xa53c,
        c,
    )
    .add(
        noise(
            wx.scale(0.6).sub(Aa::point(19.4)),
            wz.scale(0.6).add(Aa::point(2.6)),
            seed ^ 0x71b9,
            c,
        )
        .scale(0.35),
    )
    .div_scalar(1.35);
    let q = province
        .add(Aa::point(0.25))
        .div_scalar(0.6)
        .clamp(0.0, 1.0);
    let smooth = q.mul(q).mul(Aa::point(3.0).sub(q.scale(2.0)));
    let mountain = smooth.mul(smooth);
    bands(2500.0, 3, 0x9abc, false, c)
        .scale(90.0)
        .add(mountain.mul(bands(1400.0, 3, 0x24da, true, c)).scale(320.0))
        .add(
            Aa::point(0.3)
                .add(mountain.scale(0.7))
                .mul(bands(300.0, 1, 0x83e1, false, c))
                .scale(12.0),
        )
        .add(
            Aa::point(0.2)
                .add(mountain.scale(0.8))
                .mul(bands(40.0, p.octaves, 0xf13d, false, c))
                .scale(1.5),
        )
        .scale(p.amplitude / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{terrain, FbmParams};
    fn retained<T>(enabled: bool, f: impl FnOnce() -> T) -> T {
        struct Reset(bool);
        impl Drop for Reset {
            fn drop(&mut self) {
                RETAIN_NOISE_BRANCHES.set(self.0);
            }
        }
        let _reset = Reset(RETAIN_NOISE_BRANCHES.replace(enabled));
        f()
    }
    #[test]
    fn positive_weighted_mean_extrema_enclose_all_adversarial_corners() {
        for base in [
            I {
                lo: -1000.,
                hi: 1000.,
            },
            I::point(500.),
        ] {
            for count in 1..=5 {
                let contributions: Vec<_> = (0..count)
                    .map(|i| {
                        let weight = if i % 2 == 0 {
                            I { lo: 0., hi: 1e10 }
                        } else {
                            I { lo: 1e5, hi: 1e10 }
                        };
                        let profile = I {
                            lo: (i as f64 - 2.) * 100.,
                            hi: (i as f64 - 2.) * 100. + 30.,
                        };
                        (weight, profile)
                    })
                    .collect();
                let range = positive_weighted_mean_range(base, &contributions);
                for b in [base.lo, base.hi] {
                    for weights in 0..1usize << count {
                        for targets in 0..1usize << count {
                            let (mut numerator, mut denominator) = (b, 1.);
                            for (i, (w, p)) in contributions.iter().enumerate() {
                                let w = if weights & (1 << i) == 0 { w.lo } else { w.hi };
                                let p = if targets & (1 << i) == 0 { p.lo } else { p.hi };
                                numerator += w * p;
                                denominator += w;
                            }
                            let mean = numerator / denominator;
                            assert!(
                                mean >= range.lo && mean <= range.hi,
                                "{mean} outside {range:?}"
                            );
                        }
                    }
                }
            }
        }
        let range = positive_weighted_mean_range(
            I::point(500.),
            &[(I { lo: 1e8, hi: 1e10 }, I { lo: 20., hi: 21. })],
        );
        assert!(
            range.lo > 19.99 && range.hi < 21.01,
            "dominant wet target must suppress hidden upland: {range:?}"
        );
    }

    #[test]
    fn segment_distance_lipschitz_encloses_diagonal_and_endpoint_boxes() {
        for (a, b) in [([0., 0.], [1200., 900.]), ([-20., 30.], [-20., 30.])] {
            for center in [[400., 350.], [-10., -10.], [1210., 890.]] {
                let x = Aa {
                    c: center[0],
                    a: [4., 0.],
                    r: 0.03125,
                };
                let z = Aa {
                    c: center[1],
                    a: [0., 4.],
                    r: 0.03125,
                };
                let range = segment_distance_lipschitz(x, z, a, b);
                assert!(range.hi - range.lo < 12.);
                for ix in -10..=10 {
                    for iz in -10..=10 {
                        let p = [
                            center[0] + ix as f64 * 0.403125,
                            center[1] + iz as f64 * 0.403125,
                        ];
                        let d = [b[0] - a[0], b[1] - a[1]];
                        let den = d[0] * d[0] + d[1] * d[1];
                        let f = if den > 0. {
                            ((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / den
                        } else {
                            0.
                        }
                        .clamp(0., 1.);
                        let distance = ((p[0] - a[0] - f * d[0]).powi(2)
                            + (p[1] - a[1] - f * d[1]).powi(2))
                        .sqrt();
                        assert!(
                            distance >= range.lo && distance <= range.hi,
                            "{distance} outside {range:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn crossed_cell_and_reference_bounds_enclose_native_terrain_across_budgets() {
        for recipe in [
            FbmRecipe::Legacy,
            FbmRecipe::Regional,
            FbmRecipe::RegionalUnwarped,
            FbmRecipe::Highlands,
            FbmRecipe::Watershed,
        ] {
            for seed in [7, 113] {
                let terrain = FbmTerrain::new(
                    seed,
                    FbmParams {
                        legacy_riverbanks: false,
                        alluvial_deposits: true,
                        shaped_banks: true,
                        recipe,
                        horizontal_scale: 1800.0,
                        amplitude: 1000.0,
                        octaves: 8,
                        roughness: 0.5,
                    },
                )
                .unwrap();
                for (min, size_cells) in [
                    ([0, 0], [1, 1]),
                    ([-129, -33], [128, 64]),
                    ([0, -512], [128, 128]),
                    ([-2048, 2047], [512, 256]),
                    ([8192, -16384], [2048, 2048]),
                ] {
                    for budget in [
                        BoundBudget::default(),
                        BoundBudget {
                            max_noise_cells: 2,
                            max_cells_per_noise: 4,
                        },
                        BoundBudget {
                            max_noise_cells: 4096,
                            max_cells_per_noise: 16,
                        },
                        BoundBudget {
                            max_noise_cells: 0,
                            max_cells_per_noise: 0,
                        },
                    ] {
                        let region = ColumnBox { min, size_cells };
                        let before =
                            retained(true, || bound_native_height(&terrain, region, budget));
                        let after =
                            retained(false, || bound_native_height(&terrain, region, budget));
                        for bound in [before.unwrap(), after.unwrap()] {
                            assert!(bound.noise_cells <= budget.max_noise_cells);
                            for i in 0..12 {
                                let cell = [0, 1].map(|a| {
                                    min[a] + i64::from((i * [73, 113][a]) % size_cells[a])
                                });
                                let point = cell.map(|v| (v as f64 + 0.5) * voxel_size());
                                let height =
                                    terrain.height_cells(cell[0], cell[1]) as f64 * voxel_size();
                                let plane = bound.center_height_m
                                    + (0..2)
                                        .map(|a| {
                                            bound.coefficients[a] * (point[a] - bound.center_xz[a])
                                                / bound.half_extent_m[a]
                                        })
                                        .sum::<f64>();
                                assert!(
                                    (plane - height).abs()
                                        <= bound.remainder_m + fp_pad(plane.abs() + height.abs()),
                                    "{recipe:?} seed={seed} region={region:?} budget={budget:?}"
                                );
                                assert!(
                                    height >= bound.height_range[0]
                                        && height <= bound.height_range[1]
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn crossing_noise_encloses_correlated_warps_and_independent_residuals() {
        for seed in [7, 19, 673, 40961] {
            for center in [[-13., 7.], [-0.97, -8.02], [0.0, 0.49], [1000.001, -999.99]] {
                for radius in [0.0001, 0.05, 0.35, 0.65] {
                    let x = Aa {
                        c: center[0],
                        a: [radius, -0.38 * radius],
                        r: 0.08 * radius,
                    };
                    let z = Aa {
                        c: center[1],
                        a: [0.51 * radius, radius],
                        r: 0.13 * radius,
                    };
                    let mut context = Context {
                        budget: BoundBudget {
                            max_noise_cells: 64,
                            max_cells_per_noise: 16,
                        },
                        cells: 0,
                        fallbacks: 0,
                    };
                    let bound = noise(x, z, seed, &mut context);
                    assert_eq!(context.fallbacks, 0);
                    for i in -8..=8 {
                        for j in -8..=8 {
                            let a = f64::from(i) / 8.;
                            let b = f64::from(j) / 8.;
                            let plane = bound.c + bound.a[0] * a + bound.a[1] * b;
                            for sx in [-1., 1.] {
                                for sz in [-1., 1.] {
                                    let u = x.c + x.a[0] * a + x.a[1] * b + sx * x.r;
                                    let v = z.c + z.a[0] * a + z.a[1] * b + sz * z.r;
                                    let source = terrain::noise(u, v, seed);
                                    assert!((source-plane).abs() <= bound.r + fp_pad(1.),
                                    "seed={seed} at {u},{v} actual={source} plane={plane} bound={bound:?}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn crossed_cell_noise_respects_the_original_work_budget() {
        // A single cell, either crossed axis, both axes, and a budget fallback.
        for (center, extent, limit, cells, fallbacks) in [
            ([0.5, 0.5], 0.1, 4, 1, 0),
            ([1.0, 0.5], 0.1, 4, 2, 0),
            ([0.5, 1.0], 0.1, 4, 2, 0),
            ([1.0, 1.0], 0.1, 4, 4, 0),
            ([1.0, 1.0], 0.1, 1, 0, 1),
        ] {
            let x = Aa {
                c: center[0],
                a: [extent, 0.0],
                r: 0.0,
            };
            let z = Aa {
                c: center[1],
                a: [0.0, extent],
                r: 0.0,
            };
            let run = |retain| {
                let mut context = Context {
                    budget: BoundBudget {
                        max_noise_cells: 4096,
                        max_cells_per_noise: limit,
                    },
                    cells: 0,
                    fallbacks: 0,
                };
                let result = retained(retain, || noise(x, z, 7, &mut context));
                (
                    [result.c, result.a[0], result.a[1], result.r].map(f64::to_bits),
                    context.cells,
                    context.fallbacks,
                )
            };
            let before = run(true);
            let after = run(false);
            assert_eq!((before.1, before.2), (cells, fallbacks));
            assert_eq!((after.1, after.2), (cells, fallbacks));
        }
    }
    #[test]
    #[ignore = "bounded same-binary noise enclosure A/B; no timing threshold"]
    fn profile_streamed_noise_bounds() {
        let terrain = FbmTerrain::new(
            7,
            FbmParams {
                horizontal_scale: 1800.0,
                amplitude: 1000.0,
                octaves: 8,
                roughness: 0.5,
                ..Default::default()
            },
        )
        .unwrap();
        // Alternate order to expose warmup/order effects, keeping work fixed.
        for order in [[true, false], [false, true], [true, false], [false, true]] {
            for retain in order {
                let started = std::time::Instant::now();
                let mut digest = 0_u64;
                retained(retain, || {
                    for repeat in 0..4 {
                        for z in 0..16 {
                            for x in 0..16 {
                                let bound = bound_native_height(
                                    &terrain,
                                    ColumnBox {
                                        min: [x * 128, z * 128 - 512],
                                        size_cells: [128; 2],
                                    },
                                    BoundBudget::default(),
                                )
                                .unwrap();
                                digest ^= std::hint::black_box(bound.remainder_m.to_bits())
                                    .rotate_left(repeat);
                            }
                        }
                    }
                });
                eprintln!("noise_bounds retained={retain} calls=1024 elapsed_ms={:.3} digest={digest:016x}", started.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
    #[test]
    fn shared_symbols_and_independent_remainders_survive_products() {
        let a = Aa {
            c: 1.0,
            a: [0.2, -0.1],
            r: 0.01,
        };
        let b = Aa {
            c: -0.4,
            a: [0.1, 0.3],
            r: 0.03,
        };
        let p = a.mul(b);
        for i in -10..=10 {
            for j in -10..=10 {
                let (x, z) = (i as f64 / 10.0, j as f64 / 10.0);
                for e in [-1.0, 1.0] {
                    for f in [-1.0, 1.0] {
                        let exact = (a.c + a.a[0] * x + a.a[1] * z + e * a.r)
                            * (b.c + b.a[0] * x + b.a[1] * z + f * b.r);
                        assert!((exact - p.c - p.a[0] * x - p.a[1] * z).abs() <= p.r);
                    }
                }
            }
        }
    }
    #[test]
    fn crossed_noise_cells_keep_convergent_shared_coordinates() {
        for radius in [0.05, 0.01, 0.001] {
            let x = Aa {
                c: 1.0,
                a: [radius, 0.0],
                r: 0.000001,
            };
            let z = Aa {
                c: -2.0,
                a: [0.0, radius],
                r: 0.000001,
            };
            let mut c = Context {
                budget: BoundBudget::default(),
                cells: 0,
                fallbacks: 0,
            };
            let result = noise(x, z, 7, &mut c);
            assert_eq!(c.fallbacks, 0);
            assert!(result.r < 1.0);
            for i in -8..=8 {
                for j in -8..=8 {
                    let (u, v) = (i as f64 / 8.0, j as f64 / 8.0);
                    for e in [-1.0, 1.0] {
                        for f in [-1.0, 1.0] {
                            let sample = terrain::noise(
                                x.c + x.a[0] * u + e * x.r,
                                z.c + z.a[1] * v + f * z.r,
                                7,
                            );
                            assert!(
                                (sample - result.c - result.a[0] * u - result.a[1] * v).abs()
                                    <= result.r,
                                "radius={radius} sample={sample} bound={result:?}"
                            );
                        }
                    }
                }
            }
        }
    }
    fn plane(t: &FbmTerrain, min: [i64; 2], cells: u32) -> ProxyPlane {
        let lo = min.map(|v| v as f64 * voxel_size());
        let side = cells as f64 * voxel_size();
        let h = t.height_m(lo[0], lo[1]);
        let dx = (t.height_m(lo[0] + side, lo[1]) - h) / side;
        let dz = (t.height_m(lo[0], lo[1] + side) - h) / side;
        ProxyPlane {
            height_at_center: h + (dx + dz) * side * 0.5,
            dx,
            dz,
        }
    }
    fn exhaustive(t: &FbmTerrain, min: [i64; 2], cells: u32, p: ProxyPlane) -> f64 {
        let center = min.map(|v| (v as f64 + cells as f64 * 0.5) * voxel_size());
        let mut worst = 0.0_f64;
        for z in 0..cells {
            for x in 0..cells {
                let q = [
                    (min[0] as f64 + x as f64 + 0.5) * voxel_size(),
                    (min[1] as f64 + z as f64 + 0.5) * voxel_size(),
                ];
                let actual = t.height_m(q[0], q[1]);
                let proxy =
                    p.height_at_center + p.dx * (q[0] - center[0]) + p.dz * (q[1] - center[1]);
                worst = worst.max((actual - proxy).abs());
            }
        }
        worst
    }
    #[test]
    fn proven_flat_ocean_retains_exact_zero_projectable_error() {
        let terrain = FbmTerrain::new(
            40961,
            FbmParams {
                recipe: FbmRecipe::Highlands,
                ..Default::default()
            },
        )
        .unwrap();
        let region = ColumnBox {
            min: [-19968, -4096],
            size_cells: [64; 2],
        };
        let bound = bound_native_height(&terrain, region, BoundBudget::default()).unwrap();
        assert_eq!(bound.height_range, [0.; 2]);
        let [x, z] = region.min.map(|a| a as f64 * voxel_size());
        let w = region.size_cells[0] as f64 * voxel_size();
        let vertices = [[x, 0., z], [x + w, 0., z], [x, 0., z + w]];
        assert_eq!(bound.triangle_error(vertices), Some(0.));
        assert_eq!(bound.triangle_error([[x - w, 0., z]; 3]), None);
    }

    #[test]
    fn both_recipes_enclose_exhaustive_native_columns() {
        for recipe in [
            FbmRecipe::Legacy,
            FbmRecipe::Regional,
            FbmRecipe::RegionalUnwarped,
        ] {
            for seed in [7, 113] {
                let t = FbmTerrain::new(
                    seed,
                    FbmParams {
                        recipe,
                        ..Default::default()
                    },
                )
                .unwrap();
                for cells in [8, 32, 128] {
                    let min = [-37, 53];
                    let p = plane(&t, min, cells);
                    let b = bound_proxy_residual(
                        &t,
                        ColumnBox {
                            min,
                            size_cells: [cells; 2],
                        },
                        p,
                        BoundBudget::default(),
                    )
                    .unwrap();
                    let exact = exhaustive(&t, min, cells, p);
                    assert!(
                        exact <= b.error_m,
                        "{recipe:?} seed={seed} cells={cells} actual={exact} AA={b:?}"
                    );
                }
            }
        }
    }
    #[test]
    fn bounded_work_and_numeric_domain_are_explicit() {
        let t = FbmTerrain::new(7, FbmParams::default()).unwrap();
        let region = ColumnBox {
            min: [0, 0],
            size_cells: [512; 2],
        };
        let p = plane(&t, region.min, 512);
        let b = bound_proxy_residual(
            &t,
            region,
            p,
            BoundBudget {
                max_noise_cells: 2,
                max_cells_per_noise: 4,
            },
        )
        .unwrap();
        assert!(b.noise_cells <= 2);
        assert!(b.full_range_fallbacks > 0);
        assert!(bound_native_height(
            &t,
            ColumnBox {
                min: [1_i64 << 40, 0],
                ..region
            },
            BoundBudget::default()
        )
        .is_none());
    }
    #[test]
    #[ignore = "streaming exhaustive 32/64/128/256m validation and cost survey"]
    fn survey_32_to_256_meters() {
        for recipe in [FbmRecipe::Legacy, FbmRecipe::Regional] {
            let t = FbmTerrain::new(
                113,
                FbmParams {
                    recipe,
                    ..Default::default()
                },
            )
            .unwrap();
            for side in [32, 64, 128, 256] {
                let cells = side * 16;
                let min = [137, -293];
                let p = plane(&t, min, cells);
                let start = std::time::Instant::now();
                let b = bound_proxy_residual(
                    &t,
                    ColumnBox {
                        min,
                        size_cells: [cells; 2],
                    },
                    p,
                    BoundBudget::default(),
                )
                .unwrap();
                let aa_us = start.elapsed().as_secs_f64() * 1e6;
                let start = std::time::Instant::now();
                let actual = exhaustive(&t, min, cells, p);
                let native_ms = start.elapsed().as_secs_f64() * 1e3;
                println!("{recipe:?} side_m={side} aa_us={aa_us:.2} native_ms={native_ms:.2} actual_m={actual:.6} bound_m={:.6} cells={} fallbacks={}",b.error_m,b.noise_cells,b.full_range_fallbacks);
                assert!(actual <= b.error_m);
            }
        }
    }
    #[test]
    fn held_out_fan_triangles_eight_to_thirty_two_meters() {
        for recipe in [FbmRecipe::Legacy, FbmRecipe::Regional] {
            for seed in [113, 997] {
                let t = FbmTerrain::new(
                    seed,
                    FbmParams {
                        recipe,
                        ..Default::default()
                    },
                )
                .unwrap();
                for side in [8, 16, 32] {
                    let cells = side * 16;
                    let min = [137, -293];
                    let lo = min.map(|v| v as f64 * voxel_size());
                    let side_m = side as f64;
                    let vertex = |x: f64, z: f64| [x, t.height_m(x, z), z];
                    let center = vertex(lo[0] + side_m * 0.5, lo[1] + side_m * 0.5);
                    let corners = [
                        vertex(lo[0], lo[1]),
                        vertex(lo[0], lo[1] + side_m),
                        vertex(lo[0] + side_m, lo[1] + side_m),
                        vertex(lo[0] + side_m, lo[1]),
                    ];
                    let triangles: [[[f64; 3]; 3]; 4] =
                        std::array::from_fn(|i| [center, corners[i], corners[(i + 1) % 4]]);
                    let start = std::time::Instant::now();
                    let aa = bound_native_height(
                        &t,
                        ColumnBox {
                            min,
                            size_cells: [cells; 2],
                        },
                        BoundBudget::default(),
                    )
                    .unwrap();
                    let quad_error = aa.quad_error(corners).unwrap();
                    let pair_error = aa
                        .triangle_error([corners[0], corners[1], corners[2]])
                        .unwrap()
                        .max(
                            aa.triangle_error([corners[0], corners[2], corners[3]])
                                .unwrap(),
                        );
                    assert_eq!(quad_error, pair_error);
                    let error = triangles
                        .iter()
                        .map(|tri| aa.triangle_error(*tri).unwrap())
                        .fold(0.0, f64::max);
                    let aa_us = start.elapsed().as_secs_f64() * 1e6;
                    let start = std::time::Instant::now();
                    let mut actual = 0.0_f64;
                    for z in 0..cells {
                        for x in 0..cells {
                            let h = t.height_m(
                                lo[0] + (x as f64 + 0.5) * voxel_size(),
                                lo[1] + (z as f64 + 0.5) * voxel_size(),
                            );
                            for dx in [0.0, 1.0] {
                                for dz in [0.0, 1.0] {
                                    let p = [
                                        lo[0] + (x as f64 + dx) * voxel_size(),
                                        lo[1] + (z as f64 + dz) * voxel_size(),
                                    ];
                                    for tri in triangles {
                                        let a = [tri[1][0] - tri[0][0], tri[1][2] - tri[0][2]];
                                        let b = [tri[2][0] - tri[0][0], tri[2][2] - tri[0][2]];
                                        let q = [p[0] - tri[0][0], p[1] - tri[0][2]];
                                        let det = a[0] * b[1] - a[1] * b[0];
                                        let u = (q[0] * b[1] - q[1] * b[0]) / det;
                                        let v = (a[0] * q[1] - a[1] * q[0]) / det;
                                        if u >= 0.0 && v >= 0.0 && u + v <= 1.0 {
                                            let proxy = tri[0][1]
                                                + u * (tri[1][1] - tri[0][1])
                                                + v * (tri[2][1] - tri[0][1]);
                                            actual = actual.max((proxy - h).abs());
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let native_ms = start.elapsed().as_secs_f64() * 1e3;
                    println!("fan {recipe:?} seed={seed} side_m={side} AA_us={aa_us:.2} native_ms={native_ms:.2} actual_m={actual:.6} bound_m={error:.6} remainder_m={:.6} cells={} fallbacks={}",aa.remainder_m,aa.noise_cells,aa.full_range_fallbacks);
                    assert!(
                        actual <= error,
                        "{recipe:?} seed={seed} side={side} actual={actual} bound={error}"
                    );
                }
            }
        }
    }
}
