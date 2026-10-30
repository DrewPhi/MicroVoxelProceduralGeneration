//! Measured height/color grids. No procedural relief or invented vegetation.
//! Import GeoTIFFs offline with tools/import_volcan.py; runtime stays dependency-free.
use super::{summary, Column, Generator, GeneratorIdentity, Run};
use crate::{materials::Appearance, CellPos, GridScale, MaterialId, Voxel, WorldSpec};
use std::{
    fs::File,
    io::{BufReader, Read},
    ops::Range,
    path::Path,
};

struct Level {
    side: usize,
    heights: Vec<f32>,
    colors: Vec<u16>,
    bounds: Vec<[f32; 2]>,
}
impl Level {
    fn range(&self, i: usize) -> [f32; 2] {
        if self.bounds.is_empty() {
            [self.heights[i]; 2]
        } else {
            self.bounds[i]
        }
    }
}
pub struct Survey {
    spec: WorldSpec,
    levels: Vec<Level>,
    fingerprint: u64,
    pub spawn: [i64; 2],
    /// Original UTM easting/northing at the northwest grid corner.
    pub projected_origin: [f64; 2],
    /// Add to local Y to recover the survey's original elevation datum.
    pub elevation_offset: f64,
}
impl Survey {
    pub fn load(path: &Path) -> crate::Result<Self> {
        let mut input = BufReader::new(File::open(path)?);
        let mut header = [0u8; 56];
        input.read_exact(&mut header)?;
        if &header[..8] != b"MVEARTH1" {
            return Err("Not a MVEARTH1 terrain import".into());
        }
        let u32_at = |i| u32::from_le_bytes(header[i..i + 4].try_into().unwrap());
        let f64_at = |i| f64::from_le_bytes(header[i..i + 8].try_into().unwrap());
        let side = u32_at(8) as usize;
        if side != u32_at(12) as usize || !(2..=8192).contains(&side) {
            return Err("Survey must be square, 2..8192 samples per edge".into());
        }
        let grid = GridScale::new(u32_at(16), u32_at(20))?;
        let edge = side as f64 * grid.meters();
        if (edge - edge.round()).abs() > 1e-6 {
            return Err("Survey edge must be a whole number of meters".into());
        }
        let spec = WorldSpec {
            grid,
            extent_m: edge.round() as u32,
            ..Default::default()
        }
        .validate()?;
        let origin = [f64_at(24), f64_at(32)];
        let offset = f64_at(40);
        let spawn = [u32_at(48) as i64, u32_at(52) as i64];
        if origin.iter().chain([&offset]).any(|v| !v.is_finite())
            || spawn.iter().any(|v| !(0..side as i64).contains(v))
        {
            return Err("Invalid survey origin/spawn".into());
        }
        let count = side * side;
        let mut payload = vec![0; count * 6];
        input.read_exact(&mut payload)?;
        if input.read(&mut [0])? != 0 {
            return Err("Trailing survey data".into());
        }
        // Hash the actual bytes, including placement and resolution, for edit compatibility.
        let fingerprint = header
            .iter()
            .chain(payload.iter())
            .fold(0xcbf29ce484222325u64, |h, b| {
                (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
            });
        let heights: Vec<_> = payload[..count * 4]
            .chunks_exact(4)
            .map(|p| f32::from_le_bytes(p.try_into().unwrap()))
            .collect();
        if heights
            .iter()
            .any(|h| !h.is_finite() || *h < 1. || *h >= spec.ceiling_m as f32 - 2.)
        {
            return Err("Survey contains missing/nonfinite/out-of-range heights".into());
        }
        let colors = payload[count * 4..]
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes(p.try_into().unwrap()))
            .collect();
        drop(payload);
        let mut levels = vec![Level {
            side,
            heights,
            colors,
            bounds: Vec::new(),
        }];
        while levels.last().unwrap().side > 1 {
            let fine = levels.last().unwrap();
            let side = (fine.side + 1) / 2;
            let mut coarse = Level {
                side,
                heights: Vec::with_capacity(side * side),
                colors: Vec::with_capacity(side * side),
                bounds: Vec::with_capacity(side * side),
            };
            for z in 0..side {
                for x in 0..side {
                    let mut h = 0.;
                    let mut n = 0.;
                    let mut rgb = [0f64; 3];
                    let mut bounds = [f32::INFINITY, f32::NEG_INFINITY];
                    for dz in 0..2 {
                        for dx in 0..2 {
                            let (fx, fz) = (x * 2 + dx, z * 2 + dz);
                            if fx >= fine.side || fz >= fine.side {
                                continue;
                            }
                            let i = fz * fine.side + fx;
                            let r = fine.range(i);
                            bounds[0] = bounds[0].min(r[0]);
                            bounds[1] = bounds[1].max(r[1]);
                            h += fine.heights[i];
                            n += 1.;
                            let color = crate::painting::decode_rgb565(Appearance(fine.colors[i]));
                            for c in 0..3 {
                                rgb[c] += (color[c] as f64 / 255.).powf(2.2);
                            }
                        }
                    }
                    coarse.heights.push(h / n);
                    coarse.bounds.push(bounds);
                    coarse.colors.push(
                        crate::painting::encode_rgb565(
                            rgb.map(|c| ((c / n as f64).powf(1. / 2.2) * 255.).round() as u8),
                        )
                        .0,
                    );
                }
            }
            levels.push(coarse);
        }
        Ok(Self {
            spec,
            levels,
            fingerprint,
            spawn,
            projected_origin: origin,
            elevation_offset: offset,
        })
    }
    pub fn memory_bytes(&self) -> usize {
        self.levels
            .iter()
            .map(|l| l.heights.len() * 4 + l.colors.len() * 2 + l.bounds.len() * 8)
            .sum()
    }
    fn sample(&self, x: i64, z: i64, step: i64) -> Option<(f64, Appearance)> {
        if !(0..self.spec.extent_cells()).contains(&x)
            || !(0..self.spec.extent_cells()).contains(&z)
        {
            return None;
        }
        let lod = ((step.max(1) as u64).ilog2() as usize).min(self.levels.len() - 1);
        let l = &self.levels[lod];
        let i =
            ((z as usize >> lod).min(l.side - 1)) * l.side + (x as usize >> lod).min(l.side - 1);
        Some((l.heights[i] as f64, Appearance(l.colors[i])))
    }
    fn column(&self, x: i64, z: i64, step: i64) -> Option<Column> {
        let (h, _) = self.sample(x, z, step)?;
        let ground = (h / self.spec.grid.meters()).floor() as i64;
        Some(Column {
            ground,
            water: ground,
            bedrock: ground,
            surface: MaterialId::SURVEY,
            subsoil: MaterialId::SURVEY,
            rock: MaterialId::SURVEY,
        })
    }
    pub fn summary(&self, origin: [f64; 2], edge: f64) -> Option<summary::TerrainSummary> {
        if !edge.is_finite()
            || edge <= 0.
            || origin
                .iter()
                .any(|v| !v.is_finite() || *v < 0. || *v >= self.spec.extent_m as f64)
        {
            return None;
        }
        let scale = self.spec.grid.meters();
        let step = ((edge / scale).ceil() as i64).max(1);
        let lod = (step as u64).ilog2() as usize;
        let lod = lod.min(self.levels.len() - 1);
        let l = &self.levels[lod];
        let x0 = ((origin[0] / scale) as usize >> lod).min(l.side - 1);
        let z0 = ((origin[1] / scale) as usize >> lod).min(l.side - 1);
        let x1 = (((origin[0] + edge) / scale).ceil() as usize >> lod).min(l.side - 1);
        let z1 = (((origin[1] + edge) / scale).ceil() as usize >> lod).min(l.side - 1);
        let mut low = f32::INFINITY;
        let mut high = f32::NEG_INFINITY;
        for z in z0..=z1 {
            for x in x0..=x1 {
                let r = l.range(z * l.side + x);
                low = low.min(r[0]);
                high = high.max(r[1]);
            }
        }
        Some(summary::TerrainSummary {
            min_m: low as f64,
            max_m: high as f64,
            error_m: (high - low) as f64 * 0.25,
            normal_variation: 0.,
            appearance_variation: 0.,
            water_coverage: 0.,
            features: 0,
            feature_scale_m: f64::INFINITY,
            bounds_kind: summary::BoundsKind::SampledWithResidual,
        })
    }
}
impl Generator for Survey {
    fn spec(&self) -> WorldSpec {
        self.spec
    }
    fn identity(&self) -> GeneratorIdentity {
        GeneratorIdentity {
            seed: 0,
            version: 1,
            definition: self.fingerprint,
        }
    }
    fn voxel(&self, p: CellPos) -> Option<Voxel> {
        let c = self.column(p.x, p.z, 1)?;
        Some(if p.y < 0 || p.y >= c.ground {
            Voxel::AIR
        } else {
            Voxel::new(MaterialId::SURVEY, self.sample(p.x, p.z, 1)?.1)
        })
    }
    fn column_hint(&self, x: i64, z: i64) -> Option<Column> {
        self.column(x, z, 1)
    }
    fn display_column(&self, x: i64, z: i64, step: i64) -> Option<Column> {
        self.column(x, z, step)
    }
    fn appearance_at(&self, p: CellPos, _material: MaterialId) -> Appearance {
        self.sample(p.x, p.z, 1).map_or(Appearance(0), |s| s.1)
    }
    fn display_appearance_at(&self, p: CellPos, _material: MaterialId, step: i64) -> Appearance {
        self.sample(p.x, p.z, step).map_or(Appearance(0), |s| s.1)
    }
    fn runs(&self, x: i64, z: i64, range: Range<i64>) -> Option<Vec<Run>> {
        Some(self.column(x, z, 1)?.runs(range))
    }
    fn terrain_summary(&self, origin: [f64; 2], edge: f64) -> Option<summary::TerrainSummary> {
        self.summary(origin, edge)
    }
}
