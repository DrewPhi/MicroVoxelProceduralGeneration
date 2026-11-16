use super::*;
impl PhysicalFluid {
    /// Independent experimental checkpoint, never confused with CA/water saves.
    pub fn persist_bytes(&self) -> Vec<u8> {
        let mut out = b"FVPHYS01".to_vec();
        let mut n = |n: u64| out.extend_from_slice(&n.to_le_bytes());
        for v in [
            1,
            self.config.dims[0] as u64,
            self.config.dims[1] as u64,
            self.config.dims[2] as u64,
            self.config.pressure_iterations as u64,
            self.config.max_substeps as u64,
            u64::from(self.config.transport == Transport::Geometric),
            self.phase,
        ] {
            n(v);
        }
        for v in [
            self.config.spacing,
            self.config.density,
            self.config.gravity[0],
            self.config.gravity[1],
            self.config.gravity[2],
            self.config.pressure_tolerance,
            self.config.pressure_absolute_tolerance,
            self.config.pressure_fraction_floor,
            self.time,
            self.initial_volume,
            self.roundoff_adjustment,
            self.roundoff_absolute,
        ] {
            n(v.to_bits());
        }
        for &f in &self.fraction {
            n(f.to_bits());
        }
        for &p in &self.pressure {
            n(p.to_bits());
        }
        for a in 0..3 {
            for &v in &self.velocity[a] {
                n(v.to_bits());
            }
        }
        for &s in &self.solid {
            out.push(u8::from(s));
        }
        out
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PhysicalError> {
        let err = PhysicalError::InvalidCheckpoint;
        if !bytes.starts_with(b"FVPHYS01") {
            return Err(err);
        }
        struct Reader<'a>(&'a [u8]);
        impl Reader<'_> {
            fn n(&mut self) -> Result<u64, PhysicalError> {
                let (a, b) = self
                    .0
                    .split_at_checked(8)
                    .ok_or(PhysicalError::InvalidCheckpoint)?;
                self.0 = b;
                Ok(u64::from_le_bytes(a.try_into().unwrap()))
            }
            fn count(&mut self) -> Result<usize, PhysicalError> {
                usize::try_from(self.n()?).map_err(|_| PhysicalError::InvalidCheckpoint)
            }
            fn f(&mut self) -> Result<f64, PhysicalError> {
                let f = f64::from_bits(self.n()?);
                if f.is_finite() {
                    Ok(f)
                } else {
                    Err(PhysicalError::InvalidCheckpoint)
                }
            }
        }
        let mut r = Reader(&bytes[8..]);
        if r.n()? != 1 {
            return Err(err);
        }
        let dims = [r.count()?, r.count()?, r.count()?];
        let pressure_iterations = r.count()?;
        let max_substeps = r.count()?;
        let transport = match r.n()? {
            0 => Transport::DonorCell,
            1 => Transport::Geometric,
            _ => return Err(err),
        };
        let phase = r.n()?;
        let config = PhysicalConfig {
            dims,
            pressure_iterations,
            max_substeps,
            transport,
            spacing: r.f()?,
            density: r.f()?,
            gravity: [r.f()?, r.f()?, r.f()?],
            pressure_tolerance: r.f()?,
            pressure_absolute_tolerance: r.f()?,
            pressure_fraction_floor: r.f()?,
        };
        let time = r.f()?;
        let initial_volume = r.f()?;
        let roundoff_adjustment = r.f()?;
        let roundoff_absolute = r.f()?;
        if dims.iter().any(|&d| d < 2 || d > 1024) {
            return Err(err);
        }
        let n = count(dims).ok_or(err.clone())?;
        let face_count = (0..3)
            .try_fold(0usize, |total, a| {
                count(face_dims(dims, a))?.checked_add(total)
            })
            .ok_or(err.clone())?;
        let expected = n
            .checked_mul(17)
            .and_then(|n| face_count.checked_mul(8)?.checked_add(n))
            .ok_or(err.clone())?;
        if r.0.len() != expected
            || time < 0.
            || initial_volume < 0.
            || roundoff_absolute < roundoff_adjustment.abs()
        {
            return Err(err);
        }
        let mut world = Self::new(config, |_| (false, 0.)).map_err(|_| err.clone())?;
        world.time = time;
        world.phase = phase;
        world.initial_volume = initial_volume;
        world.roundoff_adjustment = roundoff_adjustment;
        world.roundoff_absolute = roundoff_absolute;
        for f in &mut world.fraction {
            *f = r.f()?;
            if !(0. ..=1.).contains(f) {
                return Err(err);
            }
        }
        for p in &mut world.pressure {
            *p = r.f()?;
        }
        for a in 0..3 {
            for v in &mut world.velocity[a] {
                *v = r.f()?;
            }
        }
        for (i, &s) in r.0.iter().enumerate() {
            world.solid[i] = match s {
                0 => false,
                1 => true,
                _ => return Err(err.clone()),
            };
            if world.solid[i] && world.fraction[i] != 0. {
                return Err(err);
            }
        }
        for a in 0..3 {
            let fd = face_dims(dims, a);
            for (i, &v) in world.velocity[a].iter().enumerate() {
                let (l, rr) = adjacent(dims, coord(fd, i), a);
                if !world.open_face(l, rr) && v != 0. {
                    return Err(err);
                }
            }
        }
        if !world.stats().kinetic_energy.is_finite() {
            return Err(err);
        }
        Ok(world)
    }
}
