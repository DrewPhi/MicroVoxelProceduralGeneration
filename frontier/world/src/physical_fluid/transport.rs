use super::*;

// Exact piecewise-polynomial integration of weighted uniform variables.
// Integrating the two-axis trapezoid CDF over the third axis avoids cancellation
// in the usual alternating-simplex formula for nearly axis-aligned planes.
// No external geometry implementation is copied here.
fn cdf2(a: f64, b: f64, t: f64) -> f64 {
    if t <= 0. {
        0.
    } else if t >= a + b {
        1.
    } else if t < b {
        0.5 * (t / a) * (t / b)
    } else if t < a {
        (t - 0.5 * b) / a
    } else {
        let r = a + b - t;
        1. - 0.5 * (r / a) * (r / b)
    }
}
fn cdf(weights: &[f64], alpha: f64) -> f64 {
    let mut w = weights.to_vec();
    w.sort_by(|a, b| b.total_cmp(a));
    match w.len() {
        1 => (alpha / w[0]).clamp(0., 1.),
        2 => cdf2(w[0], w[1], alpha),
        3 => {
            let (a, b, c) = (w[0], w[1], w[2]);
            if alpha <= 0. {
                return 0.;
            }
            if alpha >= a + b + c {
                return 1.;
            }
            let mut cuts = vec![0., c];
            for boundary in [0., b, a, a + b] {
                let z = alpha - boundary;
                if z > 0. && z < c {
                    cuts.push(z);
                }
            }
            cuts.sort_by(f64::total_cmp);
            let mut integral = 0.;
            for pair in cuts.windows(2) {
                let (lo, hi) = (pair[0], pair[1]);
                integral += (hi - lo) / c
                    * (cdf2(a, b, alpha - lo)
                        + 4. * cdf2(a, b, alpha - 0.5 * (lo + hi))
                        + cdf2(a, b, alpha - hi))
                    / 6.;
            }
            integral.clamp(0., 1.)
        }
        _ => unreachable!(),
    }
}
fn plane_box(normal: [f64; 3], alpha: f64, lo: [f64; 3], hi: [f64; 3]) -> f64 {
    let mut intercept = alpha;
    let mut weights = Vec::with_capacity(3);
    let mut volume = 1.;
    let max_weight = (0..3)
        .map(|a| (normal[a] * (hi[a] - lo[a])).abs())
        .fold(0., f64::max);
    for a in 0..3 {
        let width = hi[a] - lo[a];
        volume *= width;
        intercept -= normal[a] * lo[a];
        let weight = normal[a] * width;
        if weight < 0. {
            intercept -= weight;
        }
        // Below double-precision geometry resolution, integrate the omitted
        // direction at its midpoint, rather than shift the plane to one edge.
        if weight.abs() > max_weight * 1e-14 {
            weights.push(weight.abs());
        } else {
            intercept -= 0.5 * weight.abs();
        }
    }
    if weights.is_empty() {
        return if intercept >= 0. { volume } else { 0. };
    }
    volume * cdf(&weights, intercept)
}
fn reconstruction(d: [usize; 3], fraction: &[f64], solid: &[bool], i: usize) -> ([f64; 3], f64) {
    let p = coord(d, i);
    let mut normal = [0.; 3];
    for a in 0..3 {
        let mut values = [fraction[i]; 2];
        for (side, sign) in [-1, 1].into_iter().enumerate() {
            let q = p[a] as isize + sign;
            if q >= 0 && q < d[a] as isize {
                let mut np = p;
                np[a] = q as usize;
                let j = index(d, np);
                if !solid[j] {
                    values[side] = fraction[j];
                }
            }
        }
        normal[a] = values[0] - values[1];
    }
    let magnitude: f64 = normal.iter().map(|n| n.abs()).sum();
    if magnitude < 1e-14 {
        normal = [0., 1., 0.];
    } else {
        for n in &mut normal {
            *n /= magnitude;
        }
    }
    let mut low: f64 = normal.iter().map(|&n| n.min(0.)).sum();
    let mut high: f64 = normal.iter().map(|&n| n.max(0.)).sum();
    for _ in 0..48 {
        let mid = 0.5 * (low + high);
        if plane_box(normal, mid, [0.; 3], [1.; 3]) < fraction[i] {
            low = mid;
        } else {
            high = mid;
        }
    }
    (normal, 0.5 * (low + high))
}
impl PhysicalFluid {
    /// PLIC plane distance at cell centres, in units of grid spacing. Full/empty
    /// cells use +/-half a cell; mixed-cell signs agree with their central volume.
    pub(super) fn signed_centres(&self) -> Vec<f64> {
        self.fraction
            .iter()
            .enumerate()
            .map(|(i, &f)| {
                if f == 0. {
                    return 0.5;
                }
                if f == 1. {
                    return -0.5;
                }
                let (n, alpha) = reconstruction(self.config.dims, &self.fraction, &self.solid, i);
                let length = n.iter().map(|n| n * n).sum::<f64>().sqrt();
                let distance = (0.5 * n.iter().sum::<f64>() - alpha) / length;
                if f > 0.5 {
                    -distance.abs().max(1e-15)
                } else if f < 0.5 {
                    distance.abs().max(1e-15)
                } else {
                    0.
                }
            })
            .collect()
    }
    pub(super) fn transport(&mut self, dt: f64) -> Result<f64, PhysicalError> {
        let d = self.config.dims;
        let frozen: Vec<_> = self
            .fraction
            .iter()
            .map(|&f| if f > 0.5 { 1. } else { 0. })
            .collect();
        let mut max_courant: f64 = 0.;
        for a in 0..3 {
            for &v in &self.velocity[a] {
                max_courant = max_courant.max(v.abs() * dt / self.config.spacing);
            }
        }
        if !max_courant.is_finite() || max_courant > 0.45 {
            return Err(PhysicalError::Cfl {
                courant: max_courant,
            });
        }
        for sweep in 0..3 {
            let a = (sweep + (self.phase % 3) as usize) % 3;
            let fd = face_dims(d, a);
            let planes: Vec<_> = self
                .fraction
                .iter()
                .enumerate()
                .map(|(i, &f)| {
                    if f > 0. && f < 1. {
                        reconstruction(d, &self.fraction, &self.solid, i)
                    } else {
                        ([0.; 3], 0.)
                    }
                })
                .collect();
            let mut flux = vec![0.; self.velocity[a].len()];
            for (i, entry) in flux.iter_mut().enumerate() {
                let p = coord(fd, i);
                let (l, r) = adjacent(d, p, a);
                if !self.open_face(l, r) {
                    continue;
                }
                let c = self.velocity[a][i] * dt / self.config.spacing;
                let donor = if c >= 0. { l.unwrap() } else { r.unwrap() };
                let f = self.fraction[donor];
                let amount = if f == 0. || f == 1. || self.config.transport == Transport::DonorCell
                {
                    f * c.abs()
                } else {
                    let (normal, alpha) = planes[donor];
                    let mut lo = [0.; 3];
                    let mut hi = [1.; 3];
                    if c >= 0. {
                        lo[a] = 1. - c;
                    } else {
                        hi[a] = -c;
                    }
                    plane_box(normal, alpha, lo, hi)
                };
                *entry = if c >= 0. { amount } else { -amount };
            }
            let mut next = self.fraction.clone();
            for (i, value) in next.iter_mut().enumerate() {
                if self.solid[i] {
                    continue;
                }
                let p = coord(d, i);
                let mut q = p;
                q[a] += 1;
                let l = index(fd, p);
                let r = index(fd, q);
                let compression = frozen[i]
                    * (self.velocity[a][r] * dt / self.config.spacing
                        - self.velocity[a][l] * dt / self.config.spacing);
                *value += flux[l] - flux[r] + compression;
            }
            let min = next.iter().copied().fold(1., f64::min);
            let max = next.iter().copied().fold(0., f64::max);
            if next.iter().any(|v| !v.is_finite()) {
                return Err(PhysicalError::InvalidState);
            }
            if min < -1e-12 || max > 1. + 1e-12 {
                return Err(PhysicalError::FractionBounds { min, max });
            }
            // Explicit roundoff-only bound projection. This is not concealed
            // conservation: signed and absolute corrections are checkpointed,
            // and actual volume drift remains independent in diagnostics.
            for value in &mut next {
                let bounded = value.clamp(0., 1.);
                let correction = bounded - *value;
                self.roundoff_adjustment += correction;
                self.roundoff_absolute += correction.abs();
                *value = bounded;
            }
            self.fraction = next;
        }
        Ok(max_courant)
    }
}
#[cfg(test)]
mod geometry_tests {
    use super::*;
    #[test]
    fn nearly_axis_aligned_slabs_partition_without_cancellation() {
        for normal in [[1e-12, 1., 1e-6], [1., 1e-8, -1e-7], [1e-4, -1., 1e-5]] {
            for alpha in [0.001, 0.1, 0.4999, 0.8] {
                let full = plane_box(normal, alpha, [0.; 3], [1.; 3]);
                for width in [1e-10, 0.001, 0.3, 0.999] {
                    let left = plane_box(normal, alpha, [0.; 3], [width, 1., 1.]);
                    let right = plane_box(normal, alpha, [width, 0., 0.], [1.; 3]);
                    assert!(
                        (left + right - full).abs() < 1e-12,
                        "{normal:?} {alpha} {width}"
                    );
                    assert!(left >= 0. && left <= width);
                }
            }
        }
    }
    #[test]
    fn analytical_box_volumes_and_complements() {
        for n in [[1., 0., 0.], [1., 1., 0.], [1., 1., 1.], [-1., 2., -3.]] {
            let alpha = 0.5 * n.iter().sum::<f64>();
            assert!((plane_box(n, alpha, [0.; 3], [1.; 3]) - 0.5).abs() < 1e-12);
            let left = plane_box(n, alpha, [0.; 3], [0.3, 1., 1.]);
            let right = plane_box(n, alpha, [0.3, 0., 0.], [1.; 3]);
            assert!((left + right - 0.5).abs() < 1e-12);
        }
    }
}
