//! One-sided land ceiling used by exact streaming. Scalar terrain generation
//! remains authoritative. These enclosures mirror the two pre-drainage
//! displacements in terrain::seeded_world::eroded_relief.
use super::{fade_i, fp_pad, gradient_pair, Aa, Context, I};
use crate::alpine::Value;
use crate::terrain::seeded_world::eroded_relief::{BEDROCK_HEIGHT_BOUND, PROVINCE_HEIGHT_BOUND};

pub(super) trait RangeField: Value {
    fn lift(range: I) -> Self;
    fn range(self) -> I;
    fn div_scalar(self, v: f64) -> Self;
    fn absolute(self) -> Self;
    fn noise(self, z: Self, seed: u64, context: &mut Context) -> Self;
    fn sub(self, other: Self) -> Self {
        self.add(other.scale(-1.))
    }
}

impl RangeField for Aa {
    fn lift(range: I) -> Self {
        Self::interval(range.lo, range.hi)
    }
    fn range(self) -> I {
        Aa::range(self)
    }
    fn div_scalar(self, v: f64) -> Self {
        Aa::div_scalar(self, v)
    }
    fn absolute(self) -> Self {
        Aa::abs(self)
    }
    fn noise(self, z: Self, seed: u64, context: &mut Context) -> Self {
        super::noise(self, z, seed, context)
    }
    fn sub(self, other: Self) -> Self {
        Aa::sub(self, other)
    }
}

impl Value for I {
    fn point(v: f64) -> Self {
        Self::point(v)
    }
    fn add(self, other: Self) -> Self {
        I::add(self, other)
    }
    fn mul(self, other: Self) -> Self {
        I::mul(self, other)
    }
    fn scale(self, v: f64) -> Self {
        I::scale(self, v)
    }
    fn clamp01(self) -> Self {
        unit(self)
    }
    fn smooth_unit(self) -> Self {
        smooth(self)
    }
    fn outside(self, lo: f64, hi: f64) -> bool {
        self.lo > hi || self.hi < lo
    }
    fn is_zero(self) -> bool {
        self.lo == 0. && self.hi == 0.
    }
}

impl RangeField for I {
    fn lift(range: I) -> Self {
        range
    }
    fn range(self) -> I {
        self
    }
    fn div_scalar(self, v: f64) -> Self {
        self.div(Self::point(v))
    }
    fn absolute(self) -> Self {
        abs(self)
    }
    fn noise(self, z: Self, seed: u64, context: &mut Context) -> Self {
        clipped_noise(self, z, seed, context)
    }
    fn sub(self, other: Self) -> Self {
        I::sub(self, other)
    }
}

// A convex blend is monotone in both endpoint values. The remaining
// parameter is affine, so its two endpoints bound all weights.
fn convex_blend(a: I, b: I, t: I) -> I {
    let blend = |a, b, w| I::point(a).add(I::point(b).sub(I::point(a)).scale(w));
    I {
        lo: blend(a.lo, b.lo, t.lo).lo.min(blend(a.lo, b.lo, t.hi).lo),
        hi: blend(a.hi, b.hi, t.lo).hi.max(blend(a.hi, b.hi, t.hi).hi),
    }
}

fn clipped_noise(x: I, z: I, seed: u64, context: &mut Context) -> I {
    if ![x.lo, x.hi, z.lo, z.hi].iter().all(|v| v.is_finite()) {
        context.fallbacks += 1;
        return I { lo: -1., hi: 1. };
    }
    let lo = [x.lo.floor() as i64, z.lo.floor() as i64];
    let hi = [x.hi.floor() as i64, z.hi.floor() as i64];
    let count =
        (hi[0] as i128 - lo[0] as i128 + 1).saturating_mul(hi[1] as i128 - lo[1] as i128 + 1);
    if count <= 0
        || count > i128::from(context.budget.max_cells_per_noise)
        || count > i128::from(context.budget.max_noise_cells.saturating_sub(context.cells))
    {
        context.fallbacks += 1;
        return I { lo: -1., hi: 1. };
    }
    let mut range = I {
        lo: f64::INFINITY,
        hi: f64::NEG_INFINITY,
    };
    for j in lo[1]..=hi[1] {
        for i in lo[0]..=hi[0] {
            // Each individual gradient polynomial is valid inside its own cell.
            // Clip the coordinates before evaluating it, including both sides
            // of a cell edge when the query crosses one.
            let u = unit(x.sub(I::point(i as f64)));
            let v = unit(z.sub(I::point(j as f64)));
            let fade = |t: I| {
                unit(I {
                    lo: fade_i(I::point(t.lo)).lo,
                    hi: fade_i(I::point(t.hi)).hi,
                })
            };
            let fu = fade(u);
            let fv = fade(v);
            let gradient = |dx: i64, dz: i64| {
                let g = gradient_pair(i.wrapping_add(dx), j.wrapping_add(dz), seed);
                u.sub(I::point(dx as f64))
                    .scale(g[0])
                    .add(v.sub(I::point(dz as f64)).scale(g[1]))
            };
            let y0 = convex_blend(gradient(0, 0), gradient(1, 0), fu);
            let y1 = convex_blend(gradient(0, 1), gradient(1, 1), fu);
            let result = convex_blend(y0, y1, fv).scale(1.6);
            range.lo = range.lo.min(result.lo);
            range.hi = range.hi.max(result.hi);
        }
    }
    context.cells += count as u32;
    let pad = fp_pad(4096.);
    clamp(
        I {
            lo: (range.lo - pad).next_down(),
            hi: (range.hi + pad).next_up(),
        },
        -1.,
        1.,
    )
}

fn clamp(v: I, lo: f64, hi: f64) -> I {
    I {
        lo: v.lo.clamp(lo, hi),
        hi: v.hi.clamp(lo, hi),
    }
}
fn unit(v: I) -> I {
    clamp(v, 0., 1.)
}
fn smooth(v: I) -> I {
    let value = |x: f64| {
        let u = I::point(x.clamp(0., 1.));
        u.mul(u).mul(I::point(3.).sub(u.scale(2.)))
    };
    I {
        lo: value(v.lo).lo.max(0.),
        hi: value(v.hi).hi.min(1.),
    }
}
fn ramp(v: I, start: f64, width: f64) -> I {
    smooth(v.sub(I::point(start)).div(I::point(width)))
}
fn abs(v: I) -> I {
    I {
        lo: if v.lo <= 0. && v.hi >= 0. {
            0.
        } else {
            v.lo.abs().min(v.hi.abs())
        },
        hi: v.abs_max(),
    }
}
fn square(v: I) -> I {
    let a = abs(v);
    a.mul(a)
}
fn complement(v: I) -> I {
    I::point(1.).sub(v)
}
fn maximum(a: I, b: I) -> I {
    I {
        lo: a.lo.max(b.lo),
        hi: a.hi.max(b.hi),
    }
}

pub(super) fn land<F: RangeField>(
    seed: u64,
    x: F,
    z: F,
    mut base: F,
    upland: F,
    strata: F,
    rain: F,
    c: &mut Context,
) -> F {
    let noise = |x: F, z: F, seed, c: &mut Context| x.noise(z, seed, c);
    let uplift = upland.range();
    let s = strata.range();
    let moisture = rain.range();
    let h = base.range();
    let mountain = ramp(uplift, 0.28, 0.64).mul(ramp(h, 95., 250.));
    let coastal = ramp(h, 1., 13.).mul(complement(ramp(h, 22., 56.)));
    if mountain.hi + coastal.hi > 0. {
        let spine = noise(
            x.add(strata.scale(290.)).div_scalar(780.),
            z.add(rain.scale(440.)).div_scalar(890.),
            seed ^ 0x51b0,
            c,
        );
        let fork = noise(
            x.sub(spine.scale(92.)).div_scalar(360.),
            z.add(spine.scale(111.)).div_scalar(310.),
            seed ^ 0x51b1,
            c,
        )
        .range();
        let spine = spine.range();
        let arid = ramp(moisture.scale(-1.), 0.025, 0.31);
        let rib = ramp(abs(spine.add(fork.scale(0.39))).scale(-1.), -0.31, 0.30);
        let bowl = ramp(fork, 0.08, 0.52).mul(ramp(spine.scale(-1.), -0.02, 0.57));
        let wet = rib.sub(I::point(0.43)).scale(66.).sub(bowl.scale(42.));
        let dry = ramp(spine, -0.12, 0.26)
            .sub(I::point(0.5))
            .scale(70.)
            .add(fork.scale(16.));
        let relief = complement(arid).mul(wet).add(arid.mul(dry));
        let shore = spine
            .scale(13.)
            .add(fork.scale(5.))
            .mul(arid.scale(0.35).add(I::point(0.65)));
        let displaced = mountain.mul(relief).add(coastal.mul(shore));
        base = base.add(F::lift(clamp(
            displaced,
            -PROVINCE_HEIGHT_BOUND,
            PROVINCE_HEIGHT_BOUND,
        )));
    }
    let h = base.range();
    if h.hi <= 4. {
        return base;
    }
    let exposure = noise(x.div_scalar(930.), z.div_scalar(960.), seed ^ 0x5171, c);
    let fractures = noise(
        x.div_scalar(93.).add(z.div_scalar(190.)),
        z.div_scalar(350.).sub(x.div_scalar(270.)),
        seed ^ 0x5172,
        c,
    )
    .range();
    let tilt = if seed & 1 == 0 { 1. } else { -1. };
    let e = exposure.range();
    let thickness = I::point(39.).add(s.scale(12.)).add(e.scale(4.5));
    let along = x.scale(tilt * 0.81).add(z.scale(0.59));
    let across = x.scale(-0.59).add(z.scale(tilt * 0.81));
    let fan = noise(
        along.add(strata.scale(119.)).div_scalar(180.),
        across.div_scalar(480.),
        seed ^ 0x5175,
        c,
    );
    let branch = noise(
        along.div_scalar(72.).sub(exposure.scale(0.8)),
        across.div_scalar(173.).add(fan.scale(0.66)),
        seed ^ 0x5176,
        c,
    );
    let fan = fan.range();
    let phase = h
        .add(x.scale(0.039).add(z.scale(0.020)).scale(tilt).range())
        .add(s.scale(62.))
        .add(fractures.scale(12.))
        .add(fan.scale(17.))
        .div(thickness);
    let index = I {
        lo: phase.lo.floor(),
        hi: phase.hi.floor(),
    };
    let t = if index.lo == index.hi {
        unit(phase.sub(I::point(index.lo)))
    } else {
        I { lo: 0., hi: 1. }
    };
    let width = I::point(0.17).add(ramp(fractures, -0.38, 0.78).scale(0.21));
    let riser = unit(t.sub(I::point(0.5)).div(width).add(I::point(0.5)));
    let step_offset = smooth(riser).sub(t).scale(0.77);
    let mountain = square(uplift).mul(ramp(e, -0.42, 0.88));
    let mesa = ramp(moisture.scale(-1.), 0.06, 0.34).mul(ramp(s, -0.16, 0.58));
    let strength = maximum(mountain, mesa.scale(0.82)).mul(ramp(h, 18., 45.));
    let joint_mask = ramp(fractures.add(fan.scale(0.30)), -0.43, 0.98)
        .scale(0.68)
        .add(I::point(0.32));
    let erosion = fractures.scale(7.5).mul(ramp(e, -0.18, 0.65));
    let cement = noise(
        along.div_scalar(470.).add(F::lift(index.scale(1.57))),
        across.div_scalar(605.).sub(F::lift(index.scale(0.88))),
        seed ^ 0x51b2,
        c,
    )
    .range();
    let resistance = I::point(0.19).add(ramp(cement, -0.35, 0.73).scale(0.81));
    let hard_face = strength
        .mul(joint_mask)
        .mul(resistance)
        .mul(unit(riser.mul(complement(riser)).scale(4.)));
    let joint = noise(
        along.div_scalar(15.).add(branch.scale(0.8)),
        across.div_scalar(31.),
        seed ^ 0x5177,
        c,
    )
    .range();
    let joint = hard_face.mul(joint.scale(4.8).add(ramp(joint, -0.28, 0.42).scale(1.9)));
    let ledges = strength
        .mul(joint_mask)
        .mul(resistance.mul(step_offset).mul(thickness).add(erosion))
        .add(joint);
    let branch = branch.range();
    let ribs = ramp(abs(fan.add(branch.scale(0.30))).scale(-1.), -0.20, 0.25);
    let relief = strength.mul(
        fan.add(I::point(0.10))
            .scale(38.)
            .add(ribs.sub(I::point(0.35)).scale(21.))
            .add(branch.scale(9.)),
    );
    let lowland = complement(square(uplift))
        .mul(ramp(h, 4., 26.))
        .mul(complement(ramp(h, 155., 185.)));
    let folds = lowland.mul(fan.scale(13.).add(branch.scale(4.)));
    let mut displacement = clamp(
        ledges.add(relief).add(folds),
        -BEDROCK_HEIGHT_BOUND,
        BEDROCK_HEIGHT_BOUND,
    );
    if h.lo <= 4. {
        displacement.lo = displacement.lo.min(0.);
        displacement.hi = displacement.hi.max(0.);
    }
    base.add(F::lift(displacement))
}

#[cfg(test)]
mod tests {
    use crate::{
        affine_bounds::{
            bound_eroded_ground_ceiling, bound_eroded_ground_drainage,
            bound_eroded_ground_interval, BoundBudget, ColumnBox,
        },
        voxel_size, FbmParams, FbmRecipe, FbmTerrain,
    };

    #[test]
    fn roof_contains_native_ground_across_relief_and_negative_coordinates() {
        let params = FbmParams {
            recipe: FbmRecipe::Eroded,
            ..Default::default()
        };
        for seed in [7, 1685576058315138427] {
            let terrain = FbmTerrain::new(seed, params).unwrap();
            for [x, z] in [
                [-6688, 6080],
                [7936, 1792],
                [-1, -1],
                [9600, -9600],
                [-14012, 3272],
            ] {
                for side in [16, 64, 256, 1024] {
                    let min = [x * 16 + 3, z * 16 - 5];
                    let region = ColumnBox {
                        min,
                        size_cells: [side; 2],
                    };
                    let top = bound_eroded_ground_ceiling(&terrain, region, BoundBudget::default())
                        .unwrap();
                    let interval =
                        bound_eroded_ground_interval(&terrain, region, BoundBudget::default())
                            .unwrap();
                    let drainage =
                        bound_eroded_ground_drainage(&terrain, region, BoundBudget::default())
                            .unwrap();
                    assert!(drainage <= interval);
                    let edge = i64::from(side) - 1;
                    for dz in [0, edge / 4, edge / 2, edge * 3 / 4, edge] {
                        for dx in [0, edge / 4, edge / 2, edge * 3 / 4, edge] {
                            let ground = terrain.column_cells(min[0] + dx, min[1] + dz).ground_top
                                as f64
                                * voxel_size();
                            assert!(
                                ground <= top && ground <= interval && ground <= drainage,
                                "seed {seed} at {min:?} + [{dx},{dz}]: {ground} exceeds one of {top}, {interval}, {drainage}"
                            );
                        }
                    }
                }
            }
        }
    }
}
