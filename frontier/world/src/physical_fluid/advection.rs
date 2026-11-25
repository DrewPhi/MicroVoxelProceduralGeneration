//! Frozen-carrier semi-Lagrangian and bounded MacCormack velocity operators.
//! Greenfield method implementation; see M0 for reference/accuracy limitations.
use super::*;
impl PhysicalFluid {
    fn stencil(&self, a: usize, pos: [f64; 3], periodic: bool) -> [(usize, f64); 8] {
        let d = face_dims(self.config.dims, a);
        let (mut lo, mut hi, mut t) = ([0; 3], [0; 3], [0.; 3]);
        for b in 0..3 {
            let p = pos[b] / self.config.spacing - if a == b { 0. } else { 0.5 };
            if periodic {
                let floor = p.floor();
                let n = self.config.dims[b] as isize;
                lo[b] = (floor as isize).rem_euclid(n) as usize;
                hi[b] = (lo[b] + 1) % self.config.dims[b];
                t[b] = p - floor;
            } else {
                let p = p.clamp(0., (d[b] - 1) as f64);
                lo[b] = p.floor() as usize;
                hi[b] = (lo[b] + 1).min(d[b] - 1);
                t[b] = p - lo[b] as f64;
            }
        }
        std::array::from_fn(|mask| {
            let mut p = [0; 3];
            let mut w = 1.;
            for b in 0..3 {
                if mask & (1 << b) == 0 {
                    p[b] = lo[b];
                    w *= 1. - t[b];
                } else {
                    p[b] = hi[b];
                    w *= t[b];
                }
            }
            (index(d, p), w)
        })
    }
    fn sample_values(&self, a: usize, values: &[f64], pos: [f64; 3], periodic: bool) -> f64 {
        let mut v = 0.;
        for (i, w) in self.stencil(a, pos, periodic) {
            v += w * values[i];
        }
        v
    }
    fn backtrace(&self, pos: [f64; 3], dt: f64, periodic: bool) -> [f64; 3] {
        let mid = std::array::from_fn(|b| {
            pos[b] - 0.5 * dt * self.sample_values(b, &self.velocity[b], pos, periodic)
        });
        std::array::from_fn(|b| {
            pos[b] - dt * self.sample_values(b, &self.velocity[b], mid, periodic)
        })
    }
    fn face_position(&self, a: usize, i: usize) -> [f64; 3] {
        let p = coord(face_dims(self.config.dims, a), i);
        std::array::from_fn(|b| (p[b] as f64 + if a == b { 0. } else { 0.5 }) * self.config.spacing)
    }
    fn advected(&self, source: &[Vec<f64>; 3], dt: f64, periodic: bool) -> [Vec<f64>; 3] {
        std::array::from_fn(|a| {
            (0..source[a].len())
                .map(|i| {
                    let back = self.backtrace(self.face_position(a, i), dt, periodic);
                    self.sample_values(a, &source[a], back, periodic)
                })
                .collect()
        })
    }
    pub(super) fn advect_velocity(&mut self, dt: f64) {
        self.advect_velocity_with_sampling(dt, false);
    }
    /// Periodic sampling is an operator-test fixture only, not a tank boundary.
    pub(super) fn advect_velocity_with_sampling(&mut self, dt: f64, periodic: bool) {
        if !periodic {
            self.extend_velocity();
        }
        let mut forward = self.advected(&self.velocity, dt, periodic);
        if self.config.advection == VelocityAdvection::LimitedMacCormack {
            let backward = self.advected(&forward, -dt, periodic);
            for a in 0..3 {
                for i in 0..forward[a].len() {
                    let back = self.backtrace(self.face_position(a, i), dt, periodic);
                    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
                    for (j, _) in self.stencil(a, back, periodic) {
                        min = min.min(self.velocity[a][j]);
                        max = max.max(self.velocity[a][j]);
                    }
                    let corrected = forward[a][i] + 0.5 * (self.velocity[a][i] - backward[a][i]);
                    // Strict local component extrema limiter, not a mass correction
                    // or global energy damping. Projection follows this operator.
                    forward[a][i] = corrected.clamp(min, max);
                }
            }
        }
        self.velocity = forward;
    }
}
