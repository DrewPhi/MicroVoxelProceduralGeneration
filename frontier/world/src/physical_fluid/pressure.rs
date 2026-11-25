use super::*;
fn dot(a: &[f64], b: &[f64]) -> f64 {
    sum(a.iter().zip(b).map(|(a, b)| a * b))
}
impl PhysicalFluid {
    pub(super) fn project(&mut self, dt: f64) -> Result<(usize, f64), PhysicalError> {
        let d = self.config.dims;
        let n = self.fraction.len();
        let wet: Vec<_> = (0..n).map(|i| self.supports_pressure(i)).collect();
        let phi = if self.config.boundary == BoundaryMode::Ghost {
            self.signed_centres()
        } else {
            Vec::new()
        };
        let coefficient = |liquid: usize, air: usize| {
            if self.config.boundary == BoundaryMode::Center {
                1.
            } else {
                let theta = (-phi[liquid] / (phi[air] - phi[liquid])).clamp(GHOST_THETA_FLOOR, 1.);
                1. / theta
            }
        };
        let mut pin = vec![false; n];
        let mut visited = vec![false; n];
        // Find every pressure component and remove enclosed null spaces explicitly.
        for start in 0..n {
            if !wet[start] || visited[start] {
                continue;
            }
            let mut stack = vec![start];
            visited[start] = true;
            let mut air = false;
            while let Some(i) = stack.pop() {
                let p = coord(d, i);
                for a in 0..3 {
                    for sign in [-1, 1] {
                        let q = p[a] as isize + sign;
                        if q < 0 || q >= d[a] as isize {
                            continue;
                        }
                        let mut np = p;
                        np[a] = q as usize;
                        let j = index(d, np);
                        if self.solid[j] {
                            continue;
                        }
                        if !wet[j] {
                            air = true;
                        } else if !visited[j] {
                            visited[j] = true;
                            stack.push(j);
                        }
                    }
                }
            }
            if !air {
                pin[start] = true;
            }
        }
        let mut diag = vec![1.; n];
        let mut edges = vec![[usize::MAX; 6]; n];
        let mut rhs = vec![0.; n];
        let scale = self.config.density * self.config.spacing.powi(2) / dt;
        for i in 0..n {
            if !wet[i] || pin[i] {
                continue;
            }
            let p = coord(d, i);
            diag[i] = 0.;
            rhs[i] = -self.divergence(i) * scale;
            for a in 0..3 {
                for (side, sign) in [-1, 1].into_iter().enumerate() {
                    let q = p[a] as isize + sign;
                    if q < 0 || q >= d[a] as isize {
                        continue;
                    }
                    let mut np = p;
                    np[a] = q as usize;
                    let j = index(d, np);
                    if self.solid[j] {
                        continue;
                    }
                    diag[i] += if wet[j] { 1. } else { coefficient(i, j) };
                    if wet[j] && !pin[j] {
                        edges[i][2 * a + side] = j;
                    }
                }
            }
            if diag[i] == 0. {
                return Err(PhysicalError::InvalidState);
            }
        }
        let apply = |x: &[f64], out: &mut [f64]| {
            for i in 0..n {
                out[i] = diag[i] * x[i];
                for &j in &edges[i] {
                    if j != usize::MAX {
                        out[i] -= x[j];
                    }
                }
            }
        };
        let norm = dot(&rhs, &rhs).sqrt();
        let target =
            (self.config.pressure_tolerance * norm).max(self.config.pressure_absolute_tolerance);
        // Reuse the previous gauge pressure, while resetting newly dry/pinned
        // rows. Checkpoints retain this numerical state for exact CPU replay.
        let mut x = self.pressure.clone();
        for i in 0..n {
            if !wet[i] || pin[i] {
                x[i] = 0.;
            }
        }
        let mut r = vec![0.; n];
        apply(&x, &mut r);
        for i in 0..n {
            r[i] = rhs[i] - r[i];
        }
        let mut z: Vec<_> = r.iter().zip(&diag).map(|(r, d)| r / d).collect();
        let mut direction = z.clone();
        let mut rz = dot(&r, &z);
        let mut ap = vec![0.; n];
        let mut iterations = 0;
        loop {
            if dot(&r, &r).sqrt() <= target {
                apply(&x, &mut ap);
                for i in 0..n {
                    r[i] = rhs[i] - ap[i];
                }
                if dot(&r, &r).sqrt() <= target {
                    break;
                }
                // Reliable residual replacement: recursive CG convergence can
                // precede the actual equation's convergence at tight f64 tolerances.
                for i in 0..n {
                    z[i] = r[i] / diag[i];
                    direction[i] = z[i];
                }
                rz = dot(&r, &z);
            }
            if iterations >= self.config.pressure_iterations {
                break;
            }
            apply(&direction, &mut ap);
            let denom = dot(&direction, &ap);
            if !denom.is_finite() || denom <= 0. {
                return Err(PhysicalError::Pressure {
                    relative_residual: dot(&r, &r).sqrt() / norm.max(1e-30),
                });
            }
            let alpha = rz / denom;
            for i in 0..n {
                x[i] += alpha * direction[i];
                r[i] -= alpha * ap[i];
                z[i] = r[i] / diag[i];
            }
            let next = dot(&r, &z);
            let beta = next / rz;
            for i in 0..n {
                direction[i] = z[i] + beta * direction[i];
            }
            rz = next;
            iterations += 1;
        }
        // Check the actual equation, not just recursively updated CG residuals.
        apply(&x, &mut ap);
        for i in 0..n {
            r[i] = rhs[i] - ap[i];
        }
        let error = dot(&r, &r).sqrt();
        let relative = error / norm.max(self.config.pressure_absolute_tolerance);
        if !error.is_finite() || error > target {
            return Err(PhysicalError::Pressure {
                relative_residual: relative,
            });
        }
        self.pressure = x;
        let gradient = dt / (self.config.density * self.config.spacing);
        for a in 0..3 {
            let fd = face_dims(d, a);
            for i in 0..self.velocity[a].len() {
                let (l, r) = adjacent(d, coord(fd, i), a);
                if let (Some(l), Some(r)) = (l, r) {
                    if self.open_face(Some(l), Some(r)) && (wet[l] || wet[r]) {
                        let weight = if wet[l] && wet[r] {
                            1.
                        } else if wet[l] {
                            coefficient(l, r)
                        } else {
                            coefficient(r, l)
                        };
                        self.velocity[a][i] -=
                            gradient * weight * (self.pressure[r] - self.pressure[l]);
                    }
                }
            }
        }
        if self.velocity.iter().flatten().any(|v| !v.is_finite()) {
            return Err(PhysicalError::InvalidState);
        }
        Ok((iterations, relative))
    }
}
