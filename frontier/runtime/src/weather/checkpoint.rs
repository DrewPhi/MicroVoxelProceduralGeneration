use super::{grid_neighbors, solid_mask, CellState, Simulation, SolarClock, COUNT, EXTENT, SIZE};
use glam::{DVec3, Vec3};
use std::io::{self, Read, Write};

const FORMAT: [u8; 8] = *b"FVWXREG1";

pub(super) fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(super) fn bytes<const N: usize>(input: &mut impl Read) -> io::Result<[u8; N]> {
    let mut values = [0; N];
    input.read_exact(&mut values)?;
    Ok(values)
}

pub(super) fn write_floats<const N: usize>(
    out: &mut impl Write,
    values: [f32; N],
) -> io::Result<()> {
    // One buffered write per field record.
    let mut chunks = [[0u8; 4]; N];
    for (chunk, value) in chunks.iter_mut().zip(values) {
        *chunk = value.to_le_bytes();
    }
    out.write_all(chunks.as_flattened())
}

pub(super) fn floats<const N: usize>(input: &mut impl Read) -> io::Result<[f32; N]> {
    let mut chunks = [[0u8; 4]; N];
    input.read_exact(chunks.as_flattened_mut())?;
    let values = chunks.map(f32::from_le_bytes);
    if values.iter().all(|v| v.is_finite()) {
        Ok(values)
    } else {
        Err(invalid("nonfinite weather checkpoint value"))
    }
}

pub(super) fn doubles<const N: usize>(input: &mut impl Read) -> io::Result<[f64; N]> {
    let mut chunks = [[0u8; 8]; N];
    input.read_exact(chunks.as_flattened_mut())?;
    let values = chunks.map(f64::from_le_bytes);
    if values.iter().all(|v| v.is_finite()) {
        Ok(values)
    } else {
        Err(invalid("nonfinite weather checkpoint coordinate"))
    }
}

pub(super) fn write_doubles<const N: usize>(
    out: &mut impl Write,
    values: [f64; N],
) -> io::Result<()> {
    out.write_all(values.map(f64::to_le_bytes).as_flattened())
}

impl Simulation {
    pub fn write_checkpoint(&self, mut out: impl Write) -> io::Result<()> {
        let regional = self
            .regional
            .as_ref()
            .ok_or_else(|| invalid("checkpoints require regional weather"))?;
        out.write_all(&FORMAT)?;
        out.write_all(&self.climate.seed.to_le_bytes())?;
        write_doubles(&mut out, [self.time])?;
        write_doubles(&mut out, self.origin.to_array())?;
        write_doubles(&mut out, self.gust_shift.to_array())?;
        let solar = self
            .solar
            .unwrap_or_else(|| SolarClock::new(self.time, super::SOLAR_DAY_SECONDS));
        write_doubles(&mut out, [solar.anchor, solar.day, solar.length])?;
        for s in self.surface_weather.as_ref().unwrap() {
            write_floats(
                &mut out,
                [s.ground, s.warmth, s.moisture, s.water, s.roughness],
            )?;
        }
        for c in &self.now {
            write_floats(
                &mut out,
                [
                    c.wind.x,
                    c.wind.y,
                    c.wind.z,
                    c.theta,
                    c.vapor,
                    c.cloud,
                    c.rain,
                    c.turbulence,
                ],
            )?;
        }
        for &p in &self.pressure {
            write_floats(&mut out, [p])?;
        }
        regional.write_checkpoint(out)
    }

    pub fn read_checkpoint(&mut self, mut input: impl Read) -> io::Result<()> {
        let regional = self
            .regional
            .as_mut()
            .ok_or_else(|| invalid("checkpoints require regional weather"))?;
        if bytes::<8>(&mut input)? != FORMAT
            || u64::from_le_bytes(bytes(&mut input)?) != self.climate.seed
        {
            return Err(invalid(
                "incompatible regional weather checkpoint or world seed",
            ));
        }
        let [time] = doubles(&mut input)?;
        let origin = DVec3::from_array(doubles(&mut input)?);
        let gust_shift = DVec3::from_array(doubles(&mut input)?);
        let [anchor, day, length] = doubles(&mut input)?;
        if origin.y != 0. || origin.abs().max_element() > 2e9 || !(300. ..=86400.).contains(&length)
        {
            return Err(invalid("invalid weather checkpoint origin or day length"));
        }
        let mut surfaces = Vec::with_capacity(SIZE[0] * SIZE[2]);
        for _ in 0..SIZE[0] * SIZE[2] {
            let [ground, warmth, moisture, water, roughness] = floats(&mut input)?;
            if [warmth, moisture, water, roughness]
                .iter()
                .any(|v| !(0. ..=1.).contains(v))
            {
                return Err(invalid("invalid weather surface in checkpoint"));
            }
            surfaces.push(super::regional::Surface {
                ground,
                warmth,
                moisture,
                water,
                roughness,
            });
        }
        let mut now = Vec::with_capacity(COUNT);
        for _ in 0..COUNT {
            let [x, y, z, theta, vapor, cloud, rain, turbulence] = floats(&mut input)?;
            if [vapor, cloud, rain, turbulence].iter().any(|v| *v < 0.) {
                return Err(invalid("negative atmosphere quantity in checkpoint"));
            }
            now.push(CellState {
                wind: Vec3::new(x, y, z),
                theta,
                vapor,
                cloud,
                rain,
                turbulence,
            });
        }
        let mut pressure = Vec::with_capacity(COUNT);
        for _ in 0..COUNT {
            let [p] = floats(&mut input)?;
            pressure.push(p);
        }
        regional.read_checkpoint(input, time)?;
        self.time = time;
        self.origin = origin;
        self.center = origin + (EXTENT * Vec3::new(0.5, 0., 0.5)).as_dvec3();
        self.gust_shift = gust_shift;
        self.solar = Some(SolarClock {
            anchor,
            day,
            length,
        });
        self.surface = surfaces.iter().map(|s| s.ground).collect();
        self.solid = solid_mask(&self.surface);
        self.neighbors = grid_neighbors(&self.solid);
        self.surface_weather = Some(surfaces);
        self.now = now;
        self.pressure = pressure;
        self.scratch_pressure.fill(0.);
        Ok(())
    }

    pub fn solar_clock(&self) -> Option<SolarClock> {
        self.solar
    }
}
