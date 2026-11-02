//! Fit the implicit terrain with `n` 3D quadrics per patch: `F = min_i q_i`.
//!
//! A patch covers `cell_columns × cell_columns` voxel columns (aligned to the
//! voxel grid, so every voxel column belongs to exactly one patch) and a
//! vertical *band* `[y_lo, y_hi]` that contains the source's surface shell plus
//! a margin. Below the band everything is solid, above it everything is air;
//! inside it a voxel is solid when `F(centre) > 0`.
//!
//! Each `q_i` is a full 3D quadric (10 coefficients) in patch-local coordinates
//! normalized to [-1, 1]. Along a ray every `q_i` is quadratic in `t`, so
//! `F > 0` (the intersection of the solids `q_i > 0`) has a closed-form ray hit.
//! `min` acts as CSG intersection: the first quadric models the main surface,
//! extra quadrics carve pockets such as overhang cavities.
//!
//! Fitting (per patch, independent, embarrassingly parallel) is gradient
//! descent on all `n × 10` coefficients:
//! - samples: the source density at voxel centres in the band, truncated to a
//!   few voxels so far-away voxels keep their sign without dominating;
//! - loss: weighted squared error of `F` against the target plus a logistic
//!   term that pushes every voxel to the correct side of zero;
//! - `min` is replaced by a soft-min whose sharpness grows during descent, so
//!   every quadric receives gradient early and the end matches the hard `min`;
//! - optimizer: mini-batch Adam with a cosine-decayed step size, initialized from a
//!   least-squares quadric plus quadrics seeded at the worst false-solid
//!   voxels; the best set by voxel sign errors is kept.

#![allow(clippy::needless_range_loop)] // index loops read clearer in the small linear algebra

use crate::terrain::{Column, Terrain};

pub const COEFFS: usize = 10;

/// Monomials `1, x, y, z, x², y², z², xy, xz, yz` in local coordinates.
#[inline]
fn monomials(u: [f64; 3]) -> [f64; COEFFS] {
    let [x, y, z] = u;
    [1.0, x, y, z, x * x, y * y, z * z, x * y, x * z, y * z]
}

/// A 3D quadric in patch-local coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quadric(pub [f64; COEFFS]);

impl Quadric {
    #[inline]
    pub fn eval(&self, u: [f64; 3]) -> f64 {
        let m = monomials(u);
        let mut s = 0.0;
        for i in 0..COEFFS {
            s += self.0[i] * m[i];
        }
        s
    }

    /// Gradient with respect to local coordinates.
    pub fn gradient(&self, u: [f64; 3]) -> [f64; 3] {
        let c = &self.0;
        let [x, y, z] = u;
        [
            c[1] + 2.0 * c[4] * x + c[7] * y + c[8] * z,
            c[2] + 2.0 * c[5] * y + c[7] * x + c[9] * z,
            c[3] + 2.0 * c[6] * z + c[8] * x + c[9] * y,
        ]
    }
}

/// Fitting hyperparameters.
#[derive(Clone, Debug, PartialEq)]
pub struct FitParams {
    /// Engine voxel size, meters.
    pub voxel_size: f64,
    /// Patch footprint in voxel columns per side.
    pub cell_columns: u32,
    /// Quadrics per patch (`n`): F = min of these.
    pub quadratics: u32,
    /// Gradient-descent steps (Adam).
    pub steps: u32,
    /// Adam step size, in density units (meters) per coefficient.
    pub learning_rate: f64,
    /// Samples per gradient step (mini-batch); 0 = all samples every step.
    pub batch: usize,
    /// Extra voxels of band above and below the source shell.
    pub margin_voxels: u32,
    /// Cap on samples used for fitting (all band voxels are still scored).
    pub max_samples: usize,
    /// Sample every `sample_stride`-th voxel along x, y and z (1 = every voxel).
    /// Larger values are much faster but fit and score a sparser lattice.
    pub sample_stride: u32,
}

impl Default for FitParams {
    fn default() -> Self {
        Self {
            voxel_size: 0.25,
            cell_columns: 8,
            quadratics: 4,
            steps: 1500,
            learning_rate: 0.1,
            batch: 256,
            margin_voxels: 2,
            max_samples: 6000,
            sample_stride: 1,
        }
    }
}

impl FitParams {
    pub const KEYS: [&'static str; 8] = [
        "fit_cell_columns",
        "fit_quadratics",
        "fit_steps",
        "fit_learning_rate",
        "fit_batch",
        "fit_margin",
        "fit_max_samples",
        "fit_sample_stride",
    ];

    pub fn set(&mut self, key: &str, value: &str) -> Result<bool, String> {
        fn num<T: std::str::FromStr>(key: &str, value: &str) -> Result<T, String> {
            value
                .parse()
                .map_err(|_| format!("{key}: cannot parse {value:?}"))
        }
        match key {
            "fit_cell_columns" => self.cell_columns = num(key, value)?,
            "fit_quadratics" => self.quadratics = num(key, value)?,
            "fit_steps" => self.steps = num(key, value)?,
            "fit_learning_rate" => self.learning_rate = num(key, value)?,
            "fit_batch" => self.batch = num(key, value)?,
            "fit_margin" => self.margin_voxels = num(key, value)?,
            "fit_max_samples" => self.max_samples = num(key, value)?,
            "fit_sample_stride" => self.sample_stride = num(key, value)?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(self.voxel_size.is_finite() && self.voxel_size > 0.0) {
            return Err("voxel_size must be > 0".into());
        }
        if !(1..=256).contains(&self.cell_columns) {
            return Err("fit_cell_columns must be 1..=256".into());
        }
        if !(1..=32).contains(&self.quadratics) {
            return Err("fit_quadratics must be 1..=32".into());
        }
        if !(1..=self.cell_columns.max(1)).contains(&self.sample_stride) {
            return Err("fit_sample_stride must be 1..=fit_cell_columns".into());
        }
        if !(self.learning_rate.is_finite() && self.learning_rate > 0.0) {
            return Err("fit_learning_rate must be > 0".into());
        }
        if self.max_samples < 64 {
            return Err("fit_max_samples must be >= 64".into());
        }
        Ok(())
    }

    /// Patch side length in meters.
    pub fn patch_size(&self) -> f64 {
        self.cell_columns as f64 * self.voxel_size
    }
}

/// One fitted patch.
#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    /// Local coordinates are `(p - center) / half`.
    pub center: [f64; 3],
    pub half: [f64; 3],
    /// Band: solid below `y_lo`, air above `y_hi`.
    pub y_lo: f64,
    pub y_hi: f64,
    pub quadrics: Vec<Quadric>,
    /// Voxel centres scored in the band, and how many disagree with the source.
    pub voxels: usize,
    pub mismatched: usize,
    /// Source surface voxels in the band (solid with air directly above).
    pub surface: usize,
}

impl Patch {
    #[inline]
    pub fn local(&self, p: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|a| (p[a] - self.center[a]) / self.half[a])
    }

    /// `min_i q_i` at a world point (ignores the band) and the active index.
    pub fn eval(&self, p: [f64; 3]) -> (f64, usize) {
        let u = self.local(p);
        let mut best = (f64::INFINITY, 0);
        for (i, q) in self.quadrics.iter().enumerate() {
            let v = q.eval(u);
            if v < best.0 {
                best = (v, i);
            }
        }
        best
    }

    /// Fitted density with the band rule applied: `+1` below, `-1` above.
    pub fn density(&self, p: [f64; 3]) -> f64 {
        if p[1] < self.y_lo {
            1.0
        } else if p[1] > self.y_hi {
            -1.0
        } else {
            self.eval(p).0
        }
    }

    pub fn is_solid(&self, p: [f64; 3]) -> bool {
        self.density(p) > 0.0
    }

    /// World-space gradient of the active quadric.
    pub fn gradient(&self, p: [f64; 3]) -> [f64; 3] {
        let (_, i) = self.eval(p);
        let g = self.quadrics[i].gradient(self.local(p));
        std::array::from_fn(|a| g[a] / self.half[a])
    }

    /// Bytes to store this patch as f32 (coefficients + band).
    pub fn storage_bytes(&self) -> usize {
        self.quadrics.len() * COEFFS * 4 + 8
    }
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    u: [f64; 3],
    d: f64,
    w: f64,
}

/// Weighted least squares for one quadric. Returns `None` if underdetermined.
fn wls<'a>(samples: impl Iterator<Item = &'a Sample>) -> Option<Quadric> {
    let mut a = [[0.0f64; COEFFS]; COEFFS];
    let mut b = [0.0f64; COEFFS];
    let mut count = 0usize;
    for s in samples {
        let m = monomials(s.u);
        for i in 0..COEFFS {
            let wm = s.w * m[i];
            b[i] += wm * s.d;
            for j in 0..=i {
                a[i][j] += wm * m[j];
            }
        }
        count += 1;
    }
    if count < 2 * COEFFS {
        return None;
    }
    let trace: f64 = (0..COEFFS).map(|i| a[i][i]).sum();
    for i in 0..COEFFS {
        // Ridge keeps degenerate layouts (e.g. thin bands) solvable.
        a[i][i] += 1e-7 * trace + 1e-12;
        for j in 0..i {
            a[j][i] = a[i][j];
        }
    }
    cholesky_solve(a, b).map(Quadric)
}

fn cholesky_solve(mut a: [[f64; COEFFS]; COEFFS], mut b: [f64; COEFFS]) -> Option<[f64; COEFFS]> {
    for j in 0..COEFFS {
        let mut d = a[j][j];
        for k in 0..j {
            d -= a[j][k] * a[j][k];
        }
        if d <= 0.0 || !d.is_finite() {
            return None;
        }
        let d = d.sqrt();
        a[j][j] = d;
        for i in j + 1..COEFFS {
            let mut s = a[i][j];
            for k in 0..j {
                s -= a[i][k] * a[j][k];
            }
            a[i][j] = s / d;
        }
    }
    for i in 0..COEFFS {
        let mut s = b[i];
        for k in 0..i {
            s -= a[i][k] * b[k];
        }
        b[i] = s / a[i][i];
    }
    for i in (0..COEFFS).rev() {
        let mut s = b[i];
        for k in i + 1..COEFFS {
            s -= a[k][i] * b[k];
        }
        b[i] = s / a[i][i];
    }
    Some(b)
}

fn min_eval(qs: &[Quadric], u: [f64; 3]) -> (f64, usize) {
    let mut best = (f64::INFINITY, 0);
    for (i, q) in qs.iter().enumerate() {
        let v = q.eval(u);
        if v < best.0 {
            best = (v, i);
        }
    }
    best
}

/// Sign mismatches, then weighted squared error as a tie-breaker.
fn score(qs: &[Quadric], samples: &[Sample]) -> (usize, f64) {
    let mut wrong = 0;
    let mut err = 0.0;
    for s in samples {
        let f = min_eval(qs, s.u).0;
        wrong += ((f > 0.0) != (s.d > 0.0)) as usize;
        err += s.w * (f - s.d) * (f - s.d);
    }
    (wrong, err)
}

fn better(a: (usize, f64), b: (usize, f64)) -> bool {
    a.0 < b.0 || (a.0 == b.0 && a.1 < b.1)
}

/// Weight of the logistic sign term relative to the squared error.
const SIGN_WEIGHT: f64 = 4.0;

/// Gradient descent (Adam) on all coefficients of `F = softmin_k(q_i)`.
///
/// Per-sample loss: `w (F - d)² + SIGN_WEIGHT w σ softplus(-sign(d) F / σ)`,
/// a regression term for shape plus a logistic term whose gradient stays
/// strong for voxels just on the wrong side of zero (σ = tau / 8). The
/// soft-min `F = -ln(Σ exp(-k q_i)) / k` gives each quadric gradient in
/// proportion to `softmax(-k q)_i`; `k` grows geometrically so the end of the
/// descent optimizes (almost) the hard `min` that is rendered. Returns the best
/// set seen by hard-min sign errors, including the starting set.
fn descend(
    start: Vec<Quadric>,
    samples: &[Sample],
    steps: u32,
    lr: f64,
    batch: usize,
    tau: f64,
) -> Vec<Quadric> {
    let n = start.len();
    let unpack = |t: &[f64]| -> Vec<Quadric> {
        t.chunks(COEFFS)
            .map(|c| Quadric(c.try_into().unwrap()))
            .collect()
    };
    let mut best_score = score(&start, samples);
    let mut best = start.clone();
    if steps == 0 || samples.is_empty() || best_score.0 == 0 {
        return best;
    }
    let mut theta: Vec<f64> = start.iter().flat_map(|q| q.0).collect();
    let (mut m1, mut m2) = (vec![0.0; theta.len()], vec![0.0; theta.len()]);
    let mut grad = vec![0.0; theta.len()];
    let mut e = vec![0.0; n];
    let (b1, b2) = (0.9f64, 0.999f64);
    let (k0, k1) = (1.0 / tau, 16.0 / tau);
    let sigma = tau / 8.0;
    let batch = if batch == 0 {
        samples.len()
    } else {
        batch.min(samples.len())
    };
    let inv = 1.0 / batch as f64;
    // Deterministic pseudo-random mini-batches.
    let mut rng = 0x9e37_79b9u32 ^ samples.len() as u32;
    let mut pick = Vec::with_capacity(batch);
    for t in 1..=steps {
        let frac = t as f64 / steps as f64;
        let k = k0 * (k1 / k0).powf(frac);
        grad.fill(0.0);
        pick.clear();
        if batch == samples.len() {
            pick.extend(0..batch);
        } else {
            for _ in 0..batch {
                rng = crate::noise::hash(rng.wrapping_add(t));
                pick.push(rng as usize % samples.len());
            }
        }
        for &idx in &pick {
            let s = &samples[idx];
            let mono = monomials(s.u);
            let mut hard = f64::INFINITY;
            for i in 0..n {
                let c = &theta[i * COEFFS..(i + 1) * COEFFS];
                let mut x = 0.0;
                for j in 0..COEFFS {
                    x += c[j] * mono[j];
                }
                e[i] = x;
                hard = hard.min(x);
            }
            let mut z = 0.0;
            for v in e.iter_mut() {
                *v = (-k * (*v - hard)).exp();
                z += *v;
            }
            let f = hard - z.ln() / k;
            let mut dl = 2.0 * s.w * (f - s.d);
            let sign = if s.d > 0.0 { 1.0 } else { -1.0 };
            // d/dF of σ softplus(-sign F / σ) = -sign * sigmoid(-sign F / σ).
            let sig = 1.0 / (1.0 + (sign * f / sigma).exp());
            dl -= SIGN_WEIGHT * s.w * sign * sig;
            dl *= inv;
            for i in 0..n {
                let g = dl * e[i] / z;
                if g == 0.0 {
                    continue;
                }
                let row = &mut grad[i * COEFFS..(i + 1) * COEFFS];
                for j in 0..COEFFS {
                    row[j] += g * mono[j];
                }
            }
        }
        // Cosine decay of the step size down to 10%.
        let lr_t = lr * (0.1 + 0.45 * (1.0 + (std::f64::consts::PI * frac).cos()));
        let (c1, c2) = (1.0 - b1.powi(t as i32), 1.0 - b2.powi(t as i32));
        for idx in 0..theta.len() {
            m1[idx] = b1 * m1[idx] + (1.0 - b1) * grad[idx];
            m2[idx] = b2 * m2[idx] + (1.0 - b2) * grad[idx] * grad[idx];
            theta[idx] -= lr_t * (m1[idx] / c1) / ((m2[idx] / c2).sqrt() + 1e-12);
        }
        if t % 50 == 0 || t == steps {
            let current = unpack(&theta);
            let sc = score(&current, samples);
            if better(sc, best_score) {
                best_score = sc;
                best = current;
                if best_score.0 == 0 {
                    break;
                }
            }
        }
    }
    best
}

/// A starting quadric for another `min` term: least squares that matches the
/// source near the worst false-solid voxel not yet used as a seed, and stays
/// `lift` above the current fit elsewhere (so it only carves locally). With no
/// such voxel, an inactive copy of the first quadric raised by `lift`.
fn seed_quadric(
    qs: &[Quadric],
    samples: &[Sample],
    seeds: &mut Vec<[f64; 3]>,
    lift: f64,
) -> Quadric {
    let dist2 = |a: [f64; 3], b: [f64; 3]| -> f64 { (0..3).map(|k| (a[k] - b[k]).powi(2)).sum() };
    let worst = samples
        .iter()
        .filter(|s| !seeds.iter().any(|t| dist2(*t, s.u) < 0.25 * 0.25))
        .filter_map(|s| {
            let f = min_eval(qs, s.u).0;
            (f > 0.0 && s.d <= 0.0).then_some((s, s.w * (f - s.d)))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1));
    let inactive = || {
        let mut q = qs[0];
        q.0[0] += lift;
        q
    };
    let Some((seed, _)) = worst else {
        return inactive();
    };
    seeds.push(seed.u);
    let train: Vec<Sample> = samples
        .iter()
        .map(|s| {
            if dist2(s.u, seed.u) < 0.5 * 0.5 {
                Sample { w: 4.0 * s.w, ..*s }
            } else {
                Sample {
                    d: min_eval(qs, s.u).0 + lift,
                    ..*s
                }
            }
        })
        .collect();
    wls(train.iter()).unwrap_or_else(inactive)
}

/// Density targets are clamped to `±TRUNCATE * tau` (tau = 2 voxels).
const TRUNCATE: f64 = 4.0;
/// Minimum sample weight relative to on-surface samples.
const WEIGHT_FLOOR: f64 = 0.1;

/// Fit patch `(pi, pj)` (patch indices on the world grid) to the source terrain.
pub fn fit_patch(terrain: &Terrain, fp: &FitParams, pi: i64, pj: i64) -> Patch {
    let vs = fp.voxel_size;
    let p = fp.cell_columns as usize;
    let size = fp.patch_size();
    let (x0, z0) = (pi as f64 * size, pj as f64 * size);
    let cols: Vec<Column> = (0..p * p)
        .map(|k| {
            terrain.column(
                x0 + ((k % p) as f64 + 0.5) * vs,
                z0 + ((k / p) as f64 + 0.5) * vs,
            )
        })
        .collect();
    let (lo, hi) = cols.iter().fold((f64::MAX, f64::MIN), |(lo, hi), c| {
        let (a, b) = c.shell();
        (lo.min(a), hi.max(b))
    });
    // Band on voxel boundaries; centres below it are below every column's
    // shell (solid in the source), centres above it are above (air).
    let m = fp.margin_voxels as f64 * vs;
    let k_lo = ((lo - m) / vs).floor() as i64;
    let k_hi = (((hi + m) / vs).ceil() as i64).max(k_lo + 1);
    let (y_lo, y_hi) = (k_lo as f64 * vs, k_hi as f64 * vs);
    let center = [x0 + 0.5 * size, 0.5 * (y_lo + y_hi), z0 + 0.5 * size];
    let half = [0.5 * size, 0.5 * (y_hi - y_lo), 0.5 * size];

    let tau = 2.0 * vs;
    // Strided lattice that always keeps the patch borders (where extrapolation
    // is worst) and the band's top and bottom layers.
    let st = fp.sample_stride as usize;
    let with_end = |n: usize| -> Vec<usize> {
        let mut v: Vec<usize> = (0..n).step_by(st).collect();
        if v.last() != Some(&(n - 1)) {
            v.push(n - 1);
        }
        v
    };
    let axis = with_end(p);
    let layer_ids: Vec<i64> = with_end((k_hi - k_lo) as usize)
        .into_iter()
        .map(|k| k_lo + k as i64)
        .collect();
    let mut samples = Vec::with_capacity(axis.len() * axis.len() * layer_ids.len());
    for &j in &axis {
        for &i in &axis {
            let c = &cols[j * p + i];
            for &k in &layer_ids {
                let y = (k as f64 + 0.5) * vs;
                // Truncated density target with a weight floor: voxels far
                // from the surface keep their sign and still count, so the
                // fit cannot cheaply turn high air (or deep rock) solid/empty.
                let d = terrain
                    .density_in(c, y)
                    .clamp(-TRUNCATE * tau, TRUNCATE * tau);
                let pt = [c.x, y, c.z];
                samples.push(Sample {
                    u: std::array::from_fn(|a| (pt[a] - center[a]) / half[a]),
                    d,
                    w: WEIGHT_FLOOR + (1.0 - WEIGHT_FLOOR) / (1.0 + (d / tau) * (d / tau)),
                });
            }
        }
    }
    let stride = samples.len().div_ceil(fp.max_samples).max(1);
    let fit_set: Vec<Sample> = samples.iter().step_by(stride).copied().collect();

    // Least-squares start. Fallback: the flat plane d = mid - y.
    let first = wls(fit_set.iter()).unwrap_or_else(|| {
        let mut c = [0.0; COEFFS];
        c[2] = -half[1];
        Quadric(c)
    });
    // Descend with one quadric first: the n = 1 result is the baseline that
    // the joint n-quadric descent must beat.
    let lr = fp.learning_rate;
    let mut qs = descend(vec![first], &fit_set, fp.steps / 3, lr, fp.batch, tau);
    if fp.quadratics > 1 && score(&qs, &fit_set).0 > 0 {
        let lift = 2.0 * vs;
        let mut seeds = Vec::new();
        let mut init = qs.clone();
        for _ in 1..fp.quadratics {
            let q = seed_quadric(&init, &fit_set, &mut seeds, lift);
            init.push(q);
        }
        let joint = descend(init, &fit_set, fp.steps, lr, fp.batch, tau);
        if better(score(&joint, &fit_set), score(&qs, &fit_set)) {
            qs = joint;
        }
    }

    // Drop quadrics that are never the minimum inside the band.
    let mut used = vec![false; qs.len()];
    for s in &samples {
        used[min_eval(&qs, s.u).1] = true;
    }
    let qs: Vec<Quadric> = qs
        .into_iter()
        .zip(used)
        .filter_map(|(q, u)| u.then_some(q))
        .collect();
    let mismatched = samples
        .iter()
        .filter(|s| (min_eval(&qs, s.u).0 > 0.0) != (s.d > 0.0))
        .count();
    // Samples are stored column by column, bottom to top.
    let layers = layer_ids.len();
    let surface = samples
        .chunks(layers)
        .map(|col| {
            col.windows(2)
                .filter(|w| w[0].d > 0.0 && w[1].d <= 0.0)
                .count()
        })
        .sum();
    Patch {
        center,
        half,
        y_lo,
        y_hi,
        quadrics: qs,
        voxels: samples.len(),
        mismatched,
        surface,
    }
}

/// A rectangular region of fitted patches.
#[derive(Clone, Debug)]
pub struct FittedTerrain {
    pub params: FitParams,
    /// Patch index of the first patch.
    pub pi0: i64,
    pub pj0: i64,
    /// Patches per side.
    pub ni: usize,
    pub nj: usize,
    /// Row-major (`j * ni + i`).
    pub patches: Vec<Patch>,
}

impl FittedTerrain {
    /// Patch index range covering `[x0, x1] × [z0, z1]`.
    pub fn region(fp: &FitParams, x0: f64, z0: f64, x1: f64, z1: f64) -> (i64, i64, usize, usize) {
        let s = fp.patch_size();
        let (pi0, pj0) = ((x0 / s).floor() as i64, (z0 / s).floor() as i64);
        let (pi1, pj1) = ((x1 / s).ceil() as i64, (z1 / s).ceil() as i64);
        (
            pi0,
            pj0,
            (pi1 - pi0).max(1) as usize,
            (pj1 - pj0).max(1) as usize,
        )
    }

    pub fn patch_index(&self, x: f64, z: f64) -> Option<usize> {
        let s = self.params.patch_size();
        let i = (x / s).floor() as i64 - self.pi0;
        let j = (z / s).floor() as i64 - self.pj0;
        (i >= 0 && j >= 0 && (i as usize) < self.ni && (j as usize) < self.nj)
            .then(|| j as usize * self.ni + i as usize)
    }

    pub fn patch_at(&self, x: f64, z: f64) -> Option<&Patch> {
        self.patch_index(x, z).map(|i| &self.patches[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::TerrainParams;

    fn terrain(amplitude: f64) -> Terrain {
        let mut p = TerrainParams::default();
        p.overhang.amplitude = amplitude;
        p.overhang.coverage = 1.0;
        Terrain::new(p).unwrap()
    }

    #[test]
    fn solver_recovers_exact_quadric() {
        let truth = Quadric([0.3, -0.2, 0.9, 0.1, 0.5, -0.4, 0.2, 0.15, -0.3, 0.05]);
        let mut samples = Vec::new();
        for i in 0..6 {
            for j in 0..6 {
                for k in 0..6 {
                    let u = [
                        i as f64 / 2.5 - 1.0,
                        j as f64 / 2.5 - 1.0,
                        k as f64 / 2.5 - 1.0,
                    ];
                    samples.push(Sample {
                        u,
                        d: truth.eval(u),
                        w: 1.0,
                    });
                }
            }
        }
        let q = wls(samples.iter()).unwrap();
        for (a, b) in q.0.iter().zip(truth.0) {
            assert!((a - b).abs() < 1e-5, "{q:?}");
        }
    }

    #[test]
    fn gradient_matches_finite_differences() {
        let q = Quadric([0.3, -0.2, 0.9, 0.1, 0.5, -0.4, 0.2, 0.15, -0.3, 0.05]);
        let u = [0.2, -0.4, 0.7];
        let g = q.gradient(u);
        let e = 1e-6;
        for a in 0..3 {
            let (mut lo, mut hi) = (u, u);
            lo[a] -= e;
            hi[a] += e;
            assert!((g[a] - (q.eval(hi) - q.eval(lo)) / (2.0 * e)).abs() < 1e-6);
        }
    }

    #[test]
    fn heightfield_patches_fit_almost_exactly() {
        let t = terrain(0.0);
        let fp = FitParams::default();
        let (mut voxels, mut wrong) = (0, 0);
        for pi in 0..20 {
            for pj in 0..5 {
                let p = fit_patch(&t, &fp, pi * 7, pj * 11);
                voxels += p.voxels;
                wrong += p.mismatched;
            }
        }
        let rate = wrong as f64 / voxels as f64;
        assert!(rate < 0.02, "heightfield mismatch {rate}");
    }

    #[test]
    fn more_quadrics_never_hurt() {
        let t = terrain(16.0);
        let mut totals = Vec::new();
        for n in [1, 4] {
            let fp = FitParams {
                quadratics: n,
                ..Default::default()
            };
            let wrong: usize = (0..30)
                .map(|k| fit_patch(&t, &fp, k * 3, -k).mismatched)
                .sum();
            totals.push(wrong);
        }
        assert!(
            totals[1] <= totals[0],
            "n=4 {} vs n=1 {}",
            totals[1],
            totals[0]
        );
    }

    #[test]
    fn band_rule_matches_source_outside_band() {
        let t = terrain(16.0);
        let fp = FitParams::default();
        let p = fit_patch(&t, &fp, 5, 9);
        let (x, z) = (p.center[0], p.center[2]);
        assert!(p.is_solid([x, p.y_lo - 0.1, z]) && t.is_solid(x, p.y_lo - 0.1, z));
        assert!(!p.is_solid([x, p.y_hi + 0.1, z]) && !t.is_solid(x, p.y_hi + 0.1, z));
    }
}
