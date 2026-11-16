//! Experimental dense f64 MAC/VOF reference; never selected by the playtest.
//! Numerical contract and limitations: research/nov16/PHYSICAL_WATER_M0.md.
mod checkpoint;
mod pressure;
mod transport;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transport {
    Geometric,
    DonorCell,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhysicalConfig {
    pub dims: [usize; 3],
    pub spacing: f64,
    pub density: f64,
    pub gravity: [f64; 3],
    pub pressure_tolerance: f64,
    pub pressure_absolute_tolerance: f64,
    /// Fractions below this retain volume but do not define pressure topology.
    pub pressure_fraction_floor: f64,
    pub pressure_iterations: usize,
    pub max_substeps: usize,
    pub transport: Transport,
}
impl Default for PhysicalConfig {
    fn default() -> Self {
        Self {
            dims: [64, 64, 32],
            spacing: 0.0625,
            density: 1000.,
            gravity: [0., -9.81, 0.],
            pressure_tolerance: 1e-10,
            pressure_absolute_tolerance: 1e-12,
            pressure_fraction_floor: 1e-9,
            pressure_iterations: 2000,
            max_substeps: 4096,
            transport: Transport::Geometric,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum PhysicalError {
    InvalidConfig,
    InvalidState,
    InvalidCheckpoint,
    WorkLimit,
    Pressure { relative_residual: f64 },
    Cfl { courant: f64 },
    FractionBounds { min: f64, max: f64 },
}
impl std::fmt::Display for PhysicalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for PhysicalError {}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhysicalStats {
    pub time: f64,
    pub liquid_volume: f64,
    pub liquid_mass: f64,
    pub volume_error: f64,
    pub volume_error_voxels: f64,
    pub roundoff_adjustment_voxels: f64,
    pub roundoff_absolute_voxels: f64,
    pub transport_residual_voxels: f64,
    pub trace_volume_voxels: f64,
    pub centre_of_volume: [f64; 3],
    pub kinetic_energy: f64,
    pub max_speed: f64,
    pub max_divergence: f64,
    pub rms_divergence: f64,
    pub full_max_divergence: f64,
    pub interface_max_divergence: f64,
    pub boundary_max_normal_speed: f64,
    pub liquid_cells: usize,
    pub interface_cells: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhysicalStep {
    pub advanced: f64,
    pub substeps: usize,
    pub retries: usize,
    pub pressure_iterations: usize,
    pub pressure_relative_residual: f64,
    pub projected_max_divergence: f64,
    pub max_courant: f64,
    pub stats: PhysicalStats,
}
#[derive(Clone, Debug)]
pub struct PhysicalFluid {
    config: PhysicalConfig,
    fraction: Vec<f64>,
    pressure: Vec<f64>,
    solid: Vec<bool>,
    velocity: [Vec<f64>; 3],
    time: f64,
    phase: u64,
    initial_volume: f64,
    roundoff_adjustment: f64,
    roundoff_absolute: f64,
}
fn count(d: [usize; 3]) -> Option<usize> {
    d[0].checked_mul(d[1])?.checked_mul(d[2])
}
fn index(d: [usize; 3], p: [usize; 3]) -> usize {
    (p[2] * d[1] + p[1]) * d[0] + p[0]
}
fn coord(d: [usize; 3], i: usize) -> [usize; 3] {
    [i % d[0], (i / d[0]) % d[1], i / (d[0] * d[1])]
}
fn face_dims(mut d: [usize; 3], axis: usize) -> [usize; 3] {
    d[axis] += 1;
    d
}
fn adjacent(d: [usize; 3], p: [usize; 3], axis: usize) -> (Option<usize>, Option<usize>) {
    let l = if p[axis] > 0 {
        let mut q = p;
        q[axis] -= 1;
        Some(index(d, q))
    } else {
        None
    };
    let r = if p[axis] < d[axis] {
        Some(index(d, p))
    } else {
        None
    };
    (l, r)
}
fn sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let (mut total, mut correction) = (0., 0.);
    for value in values {
        let y = value - correction;
        let next = total + y;
        correction = (next - total) - y;
        total = next;
    }
    total
}
impl PhysicalFluid {
    pub fn new(
        config: PhysicalConfig,
        mut initial: impl FnMut([usize; 3]) -> (bool, f64),
    ) -> Result<Self, PhysicalError> {
        let n = count(config.dims).ok_or(PhysicalError::InvalidConfig)?;
        // Explicit CPU-reference allocation/work ceiling, not a gameplay radius.
        if config.dims.iter().any(|&d| d < 2 || d > 1024)
            || n > 8_388_608
            || !config.spacing.is_finite()
            || !(1e-9..=1e6).contains(&config.spacing)
            || !config.density.is_finite()
            || !(1e-6..=1e9).contains(&config.density)
            || config
                .gravity
                .iter()
                .any(|v| !v.is_finite() || v.abs() > 1e6)
            || !config.pressure_tolerance.is_finite()
            || !(0. ..1.).contains(&config.pressure_tolerance)
            || !config.pressure_absolute_tolerance.is_finite()
            || config.pressure_absolute_tolerance <= 0.
            || !config.pressure_fraction_floor.is_finite()
            || !(0. ..=1e-3).contains(&config.pressure_fraction_floor)
            || config.pressure_iterations == 0
            || config.max_substeps == 0
        {
            return Err(PhysicalError::InvalidConfig);
        }
        let mut fraction = Vec::with_capacity(n);
        let mut solid = Vec::with_capacity(n);
        for i in 0..n {
            let (s, f) = initial(coord(config.dims, i));
            if !f.is_finite() || !(0. ..=1.).contains(&f) || (s && f != 0.) {
                return Err(PhysicalError::InvalidState);
            }
            fraction.push(f);
            solid.push(s);
        }
        let initial_volume = sum(fraction.iter().copied()) * config.spacing.powi(3);
        Ok(Self {
            config,
            fraction,
            solid,
            pressure: vec![0.; n],
            velocity: std::array::from_fn(|a| vec![0.; count(face_dims(config.dims, a)).unwrap()]),
            time: 0.,
            phase: 0,
            initial_volume,
            roundoff_adjustment: 0.,
            roundoff_absolute: 0.,
        })
    }
    pub fn config(&self) -> PhysicalConfig {
        self.config
    }
    pub fn fraction(&self, p: [usize; 3]) -> Option<f64> {
        p.iter()
            .zip(self.config.dims)
            .all(|(&p, d)| p < d)
            .then(|| self.fraction[index(self.config.dims, p)])
    }
    pub fn is_solid(&self, p: [usize; 3]) -> Option<bool> {
        p.iter()
            .zip(self.config.dims)
            .all(|(&p, d)| p < d)
            .then(|| self.solid[index(self.config.dims, p)])
    }
    pub fn pressure(&self, p: [usize; 3]) -> Option<f64> {
        p.iter()
            .zip(self.config.dims)
            .all(|(&p, d)| p < d)
            .then(|| self.pressure[index(self.config.dims, p)])
    }
    pub fn face_velocity(&self, axis: usize, p: [usize; 3]) -> Option<f64> {
        if axis >= 3 {
            return None;
        }
        let d = face_dims(self.config.dims, axis);
        p.iter()
            .zip(d)
            .all(|(&p, d)| p < d)
            .then(|| self.velocity[axis][index(d, p)])
    }
    /// Fixture initialization only; no terrain mutation/source ledger is implied.
    pub fn initialize_velocity(
        &mut self,
        mut velocity: impl FnMut(usize, [f64; 3]) -> f64,
    ) -> Result<(), PhysicalError> {
        let mut next = self.velocity.clone();
        for a in 0..3 {
            let d = face_dims(self.config.dims, a);
            for (i, v) in next[a].iter_mut().enumerate() {
                let p = coord(d, i);
                let pos = std::array::from_fn(|b| {
                    (p[b] as f64 + if a == b { 0. } else { 0.5 }) * self.config.spacing
                });
                *v = velocity(a, pos);
                if !v.is_finite() {
                    return Err(PhysicalError::InvalidState);
                }
            }
        }
        self.velocity = next;
        self.enforce_faces();
        Ok(())
    }
    fn open_face(&self, l: Option<usize>, r: Option<usize>) -> bool {
        matches!((l,r),(Some(l),Some(r)) if !self.solid[l] && !self.solid[r])
    }
    fn enforce_faces(&mut self) {
        let d = self.config.dims;
        for a in 0..3 {
            let fd = face_dims(d, a);
            for i in 0..self.velocity[a].len() {
                let (l, r) = adjacent(d, coord(fd, i), a);
                if !self.open_face(l, r)
                    || !l
                        .into_iter()
                        .chain(r)
                        .any(|c| self.fraction[c] > self.config.pressure_fraction_floor)
                {
                    self.velocity[a][i] = 0.;
                }
            }
        }
    }
    fn divergence(&self, i: usize) -> f64 {
        let p = coord(self.config.dims, i);
        (0..3)
            .map(|a| {
                let d = face_dims(self.config.dims, a);
                let mut q = p;
                q[a] += 1;
                (self.velocity[a][index(d, q)] - self.velocity[a][index(d, p)])
                    / self.config.spacing
            })
            .sum()
    }
    pub fn stats(&self) -> PhysicalStats {
        let h = self.config.spacing;
        let volume = sum(self.fraction.iter().copied()) * h.powi(3);
        let mut s = PhysicalStats {
            time: self.time,
            liquid_volume: volume,
            liquid_mass: volume * self.config.density,
            volume_error: volume - self.initial_volume,
            volume_error_voxels: (volume - self.initial_volume) / h.powi(3),
            roundoff_adjustment_voxels: self.roundoff_adjustment,
            roundoff_absolute_voxels: self.roundoff_absolute,
            transport_residual_voxels: (volume - self.initial_volume) / h.powi(3)
                - self.roundoff_adjustment,
            ..Default::default()
        };
        let mut div2 = 0.;
        for (i, &f) in self.fraction.iter().enumerate() {
            if f == 0. {
                continue;
            }
            s.liquid_cells += 1;
            if f <= self.config.pressure_fraction_floor {
                s.trace_volume_voxels += f;
            }
            let p = coord(self.config.dims, i);
            let div = self.divergence(i).abs();
            s.max_divergence = s.max_divergence.max(div);
            div2 += div * div;
            if f == 1. {
                s.full_max_divergence = s.full_max_divergence.max(div);
            } else {
                s.interface_cells += 1;
                s.interface_max_divergence = s.interface_max_divergence.max(div);
            }
            for a in 0..3 {
                s.centre_of_volume[a] += f * h.powi(3) * (p[a] as f64 + 0.5) * h;
            }
            let mut speed2 = 0.;
            for a in 0..3 {
                let d = face_dims(self.config.dims, a);
                let mut q = p;
                q[a] += 1;
                let v = 0.5 * (self.velocity[a][index(d, p)] + self.velocity[a][index(d, q)]);
                speed2 += v * v;
            }
            s.kinetic_energy += 0.5 * self.config.density * f * h.powi(3) * speed2;
        }
        if volume > 0. {
            for c in &mut s.centre_of_volume {
                *c /= volume;
            }
        }
        if s.liquid_cells > 0 {
            s.rms_divergence = (div2 / s.liquid_cells as f64).sqrt();
        }
        for a in 0..3 {
            let fd = face_dims(self.config.dims, a);
            for (i, &v) in self.velocity[a].iter().enumerate() {
                s.max_speed = s.max_speed.max(v.abs());
                let (l, r) = adjacent(self.config.dims, coord(fd, i), a);
                if !self.open_face(l, r) {
                    s.boundary_max_normal_speed = s.boundary_max_normal_speed.max(v.abs());
                }
            }
        }
        s
    }
    /// Advance the entire requested interval or return with self unchanged.
    pub fn step(&mut self, dt: f64) -> Result<PhysicalStep, PhysicalError> {
        if !dt.is_finite()
            || dt <= 0.
            || !(self.time + dt).is_finite()
            || self.time + dt == self.time
        {
            return Err(PhysicalError::InvalidState);
        }
        let mut work = self.clone();
        let mut report = PhysicalStep::default();
        let mut remaining = dt;
        while remaining > 0. {
            if report.substeps + report.retries >= self.config.max_substeps {
                return Err(PhysicalError::WorkLimit);
            }
            let speed = work
                .velocity
                .iter()
                .flatten()
                .map(|v| v.abs())
                .fold(0., f64::max);
            let force = work
                .config
                .gravity
                .iter()
                .map(|v| v.abs())
                .fold(0., f64::max);
            let bound = 0.35 * work.config.spacing / (3. * (speed + force * remaining).max(1e-30));
            let mut subdt = remaining.min(bound);
            let mut last_error = None;
            loop {
                if subdt <= dt * 1e-12
                    || report.substeps + report.retries >= self.config.max_substeps
                {
                    return Err(last_error.unwrap_or(PhysicalError::WorkLimit));
                }
                let mut candidate = work.clone();
                match candidate.substep(subdt) {
                    Ok((iterations, residual, courant, div)) => {
                        report.substeps += 1;
                        report.pressure_iterations += iterations;
                        report.pressure_relative_residual =
                            report.pressure_relative_residual.max(residual);
                        report.projected_max_divergence = report.projected_max_divergence.max(div);
                        report.max_courant = report.max_courant.max(courant);
                        work = candidate;
                        break;
                    }
                    Err(e @ (PhysicalError::Cfl { .. } | PhysicalError::FractionBounds { .. })) => {
                        last_error = Some(e);
                        report.retries += 1;
                        subdt *= 0.5;
                    }
                    Err(e) => return Err(e),
                }
            }
            remaining = if subdt == remaining {
                0.
            } else {
                remaining - subdt
            };
        }
        work.time = self.time + dt;
        report.advanced = dt;
        report.stats = work.stats();
        if !report.stats.liquid_volume.is_finite() || !report.stats.kinetic_energy.is_finite() {
            return Err(PhysicalError::InvalidState);
        }
        *self = work;
        Ok(report)
    }
    fn substep(&mut self, dt: f64) -> Result<(usize, f64, f64, f64), PhysicalError> {
        self.advect_velocity(dt);
        let d = self.config.dims;
        for a in 0..3 {
            let fd = face_dims(d, a);
            for i in 0..self.velocity[a].len() {
                let (l, r) = adjacent(d, coord(fd, i), a);
                if self.open_face(l, r)
                    && l.into_iter()
                        .chain(r)
                        .any(|c| self.fraction[c] > self.config.pressure_fraction_floor)
                {
                    self.velocity[a][i] += dt * self.config.gravity[a];
                }
            }
        }
        self.enforce_faces();
        let (iterations, residual) = self.project(dt)?;
        let div = (0..self.fraction.len())
            .filter(|&i| self.fraction[i] > self.config.pressure_fraction_floor)
            .map(|i| self.divergence(i).abs())
            .fold(0., f64::max);
        let courant = self.transport(dt)?;
        self.phase = self.phase.checked_add(1).ok_or(PhysicalError::WorkLimit)?;
        self.enforce_faces();
        Ok((iterations, residual, courant, div))
    }
    fn sample_component(&self, a: usize, pos: [f64; 3]) -> f64 {
        let d = face_dims(self.config.dims, a);
        let mut lo = [0; 3];
        let mut hi = [0; 3];
        let mut t = [0.; 3];
        for b in 0..3 {
            let p = (pos[b] / self.config.spacing - if a == b { 0. } else { 0.5 })
                .clamp(0., (d[b] - 1) as f64);
            lo[b] = p.floor() as usize;
            hi[b] = (lo[b] + 1).min(d[b] - 1);
            t[b] = p - lo[b] as f64;
        }
        let mut v = 0.;
        for mask in 0..8 {
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
            v += w * self.velocity[a][index(d, p)];
        }
        v
    }
    fn extend_velocity(&mut self) {
        let d = self.config.dims;
        for a in 0..3 {
            let fd = face_dims(d, a);
            let mut valid: Vec<_> = (0..self.velocity[a].len())
                .map(|i| {
                    let (l, r) = adjacent(d, coord(fd, i), a);
                    self.open_face(l, r)
                        && l.into_iter()
                            .chain(r)
                            .any(|c| self.fraction[c] > self.config.pressure_fraction_floor)
                })
                .collect();
            for _ in 0..2 {
                let mut next = self.velocity[a].clone();
                let mut next_valid = valid.clone();
                for i in 0..next.len() {
                    if valid[i] {
                        continue;
                    }
                    let p = coord(fd, i);
                    let (l, r) = adjacent(d, p, a);
                    if !self.open_face(l, r) {
                        continue;
                    }
                    let (mut total, mut n) = (0., 0);
                    for b in 0..3 {
                        for sign in [-1, 1] {
                            let q = p[b] as isize + sign;
                            if q >= 0 && q < fd[b] as isize {
                                let mut np = p;
                                np[b] = q as usize;
                                let j = index(fd, np);
                                if valid[j] {
                                    total += self.velocity[a][j];
                                    n += 1;
                                }
                            }
                        }
                    }
                    if n > 0 {
                        next[i] = total / n as f64;
                        next_valid[i] = true;
                    }
                }
                self.velocity[a] = next;
                valid = next_valid;
            }
        }
    }
    fn advect_velocity(&mut self, dt: f64) {
        self.extend_velocity();
        let mut next = self.velocity.clone();
        for a in 0..3 {
            let d = face_dims(self.config.dims, a);
            for (i, v) in next[a].iter_mut().enumerate() {
                let p = coord(d, i);
                let pos = std::array::from_fn(|b| {
                    (p[b] as f64 + if a == b { 0. } else { 0.5 }) * self.config.spacing
                });
                let mid =
                    std::array::from_fn(|b| pos[b] - 0.5 * dt * self.sample_component(b, pos));
                let back = std::array::from_fn(|b| pos[b] - dt * self.sample_component(b, mid));
                *v = self.sample_component(a, back);
            }
        }
        self.velocity = next;
    }
}

#[cfg(test)]
mod tests;
