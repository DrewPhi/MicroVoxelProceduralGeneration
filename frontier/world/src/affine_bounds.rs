//! Bounded reduced affine arithmetic over the unchanged canonical FBM source.
//! Two shared world-coordinate symbols survive; nonlinear/roundoff uncertainty
//! is an independent radius. Multiple noise-cell polynomials share those symbols.
use crate::{terrain::hash, FbmRecipe, FbmTerrain, VOXEL_SIZE};

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
        let mut error = 0.0_f64;
        for v in vertices {
            if !v.iter().all(|v| v.is_finite()) {
                return None;
            }
            let mut coordinate = [I::point(0.0); 2];
            for a in 0..2 {
                let position = v[if a == 0 { 0 } else { 2 }];
                if position < self.center_xz[a] - self.half_extent_m[a]
                    || position > self.center_xz[a] + self.half_extent_m[a]
                {
                    return None;
                }
                let q = I::point(position)
                    .sub(I::point(self.center_xz[a]))
                    .div(I::point(self.half_extent_m[a]));
                coordinate[a] = I {
                    lo: q.lo.max(-1.0),
                    hi: q.hi.min(1.0),
                };
            }
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
        [0, 1].map(|a| (region.min[a] as f64 + region.size_cells[a] as f64 * 0.5) * VOXEL_SIZE);
    let radius = region.size_cells.map(|v| v as f64 * VOXEL_SIZE * 0.5);
    let x = Aa {
        c: center[0],
        a: [radius[0], 0.0],
        r: VOXEL_SIZE * 0.5,
    };
    let z = Aa {
        c: center[1],
        a: [0.0, radius[1]],
        r: VOXEL_SIZE * 0.5,
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
    let height = height.add(Aa {
        c: -VOXEL_SIZE * 0.5,
        a: [0.0; 2],
        r: VOXEL_SIZE * 0.5,
    });
    let range = height.range();
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
fn fade_range(t: I) -> (I, I, I) {
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
    let mut dd = d;
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
/// Value, U/V gradient and UU/UV/VV Hessian of one noise-cell polynomial.
fn noise_jet(u: I, v: I, i: i64, j: i64, seed: u64) -> [I; 6] {
    let (fu, du, ddu) = fade_range(u);
    let (fv, dv, ddv) = fade_range(v);
    let grad = |dx: i64, dz: i64| {
        let g = gradient_pair(i.wrapping_add(dx), j.wrapping_add(dz), seed);
        (
            u.sub(I::point(dx as f64))
                .scale(g[0])
                .add(v.sub(I::point(dz as f64)).scale(g[1])),
            g,
        )
    };
    let line = |dz: i64| {
        let (g0, a) = grad(0, dz);
        let (g1, b) = grad(1, dz);
        let delta = g1.sub(g0);
        let dax = b[0] - a[0];
        let daz = b[1] - a[1];
        [
            lerp_i(g0, g1, fu),
            I::point(a[0]).add(du.mul(delta)).add(fu.scale(dax)),
            I::point(a[1]).add(fu.scale(daz)),
            ddu.mul(delta).add(du.scale(2.0 * dax)),
            du.scale(daz),
        ]
    };
    let a = line(0);
    let b = line(1);
    [
        lerp_i(a[0], b[0], fv),
        lerp_i(a[1], b[1], fv),
        lerp_i(a[2], b[2], fv).add(dv.mul(b[0].sub(a[0]))),
        lerp_hessian(a[3], b[3], fv),
        lerp_hessian(a[4], b[4], fv).add(dv.mul(b[1].sub(a[1]))),
        dv.mul(b[2].sub(a[2]))
            .scale(2.0)
            .add(ddv.mul(b[0].sub(a[0]))),
    ]
    .map(|x| x.scale(1.6))
}
fn noise_taylor(u: Aa, v: Aa, i: i64, j: i64, seed: u64) -> Aa {
    let point = noise_jet(I::point(u.c), I::point(v.c), i, j, seed);
    let range = noise_jet(u.range(), v.range(), i, j, seed);
    let mut r = up_add(
        up_mul(point[1].abs_max(), u.r),
        up_mul(point[2].abs_max(), v.r),
    );
    r = up_add(
        r,
        up_mul(
            0.5,
            up_add(
                up_add(
                    up_mul(range[3].abs_max(), up_mul(u.radius(), u.radius())),
                    up_mul(2.0 * range[4].abs_max(), up_mul(u.radius(), v.radius())),
                ),
                up_mul(range[5].abs_max(), up_mul(v.radius(), v.radius())),
            ),
        ),
    );
    // The actual branch is evaluated only inside its unit cell. 32768 eps
    // covers source gradient/fade/interpolation roundoff there; polynomial
    // extensions outside the cell are only conservative mathematical enclosures.
    r = up_add(r, fp_pad(4096.0));
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
struct Context {
    budget: BoundBudget,
    cells: u32,
    fallbacks: u32,
}
#[cfg(test)]
thread_local! {
    // Preserve the previous allocation/reduction path for exact same-binary A/B.
    static RETAIN_NOISE_BRANCHES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
fn noise(x: Aa, z: Aa, seed: u64, c: &mut Context) -> Aa {
    #[cfg(test)]
    if RETAIN_NOISE_BRANCHES.get() {
        return noise_impl::<true>(x, z, seed, c);
    }
    noise_impl::<false>(x, z, seed, c)
}
fn noise_impl<const RETAIN: bool>(x: Aa, z: Aa, seed: u64, c: &mut Context) -> Aa {
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
    let mut branches = Vec::new();
    let mut first: Option<Aa> = None;
    let mut radius = 0.0_f64;
    for j in lo[1]..=hi[1] {
        for i in lo[0]..=hi[0] {
            c.cells += 1;
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
            let branch = if taylor.r < ordinary.r {
                taylor
            } else {
                ordinary
            };
            if RETAIN {
                branches.push(branch);
            } else {
                let reference = *first.get_or_insert_with(|| {
                    radius = branch.r;
                    branch
                });
                // Keep the original first-branch comparison and j/i reduction
                // order, including outward rounding. Only storage is removed.
                let delta = up_add(
                    I::point(branch.c).sub(I::point(reference.c)).abs_max(),
                    up_add(
                        I::point(branch.a[0])
                            .sub(I::point(reference.a[0]))
                            .abs_max(),
                        I::point(branch.a[1])
                            .sub(I::point(reference.a[1]))
                            .abs_max(),
                    ),
                );
                radius = radius.max(up_add(delta, branch.r));
            }
        }
    }
    if !RETAIN {
        return Aa {
            r: radius,
            ..first.expect("positive branch count was checked above")
        };
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
fn elevation(t: &FbmTerrain, x: Aa, z: Aa, c: &mut Context) -> Aa {
    let p = t.params();
    let seed = t.seed();
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
    fn bound_bits(bound: HeightBound) -> ([u64; 10], u32, u32) {
        (
            [
                bound.center_xz[0],
                bound.center_xz[1],
                bound.half_extent_m[0],
                bound.half_extent_m[1],
                bound.center_height_m,
                bound.coefficients[0],
                bound.coefficients[1],
                bound.remainder_m,
                bound.height_range[0],
                bound.height_range[1],
            ]
            .map(f64::to_bits),
            bound.noise_cells,
            bound.full_range_fallbacks,
        )
    }
    #[test]
    fn streamed_noise_branches_match_retained_bounds_bit_for_bit() {
        for recipe in [
            FbmRecipe::Legacy,
            FbmRecipe::Regional,
            FbmRecipe::RegionalUnwarped,
        ] {
            for seed in [7, 113] {
                let terrain = FbmTerrain::new(
                    seed,
                    FbmParams {
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
                        assert_eq!(
                            before.map(bound_bits),
                            after.map(bound_bits),
                            "{recipe:?} seed={seed} region={region:?} budget={budget:?}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn streamed_noise_branch_counts_match_retained_oracle() {
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
            assert_eq!(before, after);
            assert_eq!((after.1, after.2), (cells, fallbacks));
        }
    }
    #[test]
    #[ignore = "bounded same-binary CPU allocation A/B; no timing threshold"]
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
        let lo = min.map(|v| v as f64 * VOXEL_SIZE);
        let side = cells as f64 * VOXEL_SIZE;
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
        let center = min.map(|v| (v as f64 + cells as f64 * 0.5) * VOXEL_SIZE);
        let mut worst = 0.0_f64;
        for z in 0..cells {
            for x in 0..cells {
                let q = [
                    (min[0] as f64 + x as f64 + 0.5) * VOXEL_SIZE,
                    (min[1] as f64 + z as f64 + 0.5) * VOXEL_SIZE,
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
                    let lo = min.map(|v| v as f64 * VOXEL_SIZE);
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
                                lo[0] + (x as f64 + 0.5) * VOXEL_SIZE,
                                lo[1] + (z as f64 + 0.5) * VOXEL_SIZE,
                            );
                            for dx in [0.0, 1.0] {
                                for dz in [0.0, 1.0] {
                                    let p = [
                                        lo[0] + (x as f64 + dx) * VOXEL_SIZE,
                                        lo[1] + (z as f64 + dz) * VOXEL_SIZE,
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
