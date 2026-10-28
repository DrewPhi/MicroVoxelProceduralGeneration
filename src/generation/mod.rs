//! Generation is a baseline provider, never a camera-dependent world model.
mod terrain;
use crate::{
    grid::{CellPos, VOXEL_SIZE},
    materials::{MaterialId, Voxel},
    painting,
};
use std::{
    collections::{HashMap, VecDeque},
    ops::Range,
    sync::Mutex,
};

pub const GENERATOR_VERSION: u32 = 1;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeneratorIdentity {
    pub seed: u64,
    pub version: u32,
    pub definition: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: i64,
    pub end: i64,
    pub material: MaterialId,
}

/// A terrain optimization. The world API also supports arbitrary 3D generators.
#[derive(Clone, Copy, Debug)]
pub struct Column {
    pub ground: i64,
    pub water: i64,
    pub bedrock: i64,
    pub surface: MaterialId,
    pub subsoil: MaterialId,
}
impl Column {
    pub fn material_at(self, y: i64) -> MaterialId {
        if y < 0 || y >= self.water.max(self.ground) {
            MaterialId::AIR
        } else if y >= self.ground {
            MaterialId::WATER
        } else if y == self.ground - 1 {
            self.surface
        } else if y < self.bedrock {
            MaterialId::ROCK
        } else {
            self.subsoil
        }
    }
    pub fn runs(self, range: Range<i64>) -> Vec<Run> {
        let mut cuts = vec![range.start, range.end];
        for y in [
            0,
            self.bedrock.min(self.ground - 1),
            self.ground - 1,
            self.ground,
            self.water,
        ] {
            if y > range.start && y < range.end {
                cuts.push(y);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        let mut result = Vec::new();
        for pair in cuts.windows(2) {
            push_run(
                &mut result,
                Run {
                    start: pair[0],
                    end: pair[1],
                    material: self.material_at(pair[0]),
                },
            );
        }
        result
    }
}
pub fn push_run(runs: &mut Vec<Run>, run: Run) {
    if run.start >= run.end {
        return;
    }
    if let Some(last) = runs.last_mut() {
        if last.end == run.start && last.material == run.material {
            last.end = run.end;
            return;
        }
    }
    runs.push(run);
}

pub trait Generator: Send + Sync {
    fn identity(&self) -> GeneratorIdentity;
    /// None means unknown/unprovided world, never implicit air.
    fn voxel(&self, pos: CellPos) -> Option<Voxel>;
    fn column_hint(&self, _x: i64, _z: i64) -> Option<Column> {
        None
    }
    fn runs(&self, x: i64, z: i64, range: Range<i64>) -> Option<Vec<Run>> {
        if range.end.checked_sub(range.start)? > 1_000_000 {
            return None;
        }
        let mut result = Vec::new();
        for y in range {
            push_run(
                &mut result,
                Run {
                    start: y,
                    end: y + 1,
                    material: self.voxel(CellPos::new(x, y, z))?.material,
                },
            );
        }
        Some(result)
    }
}

#[derive(Default)]
struct ColumnCache {
    values: HashMap<(i64, i64), Column>,
    fifo: VecDeque<(i64, i64)>,
}
/// Sixteen independent small FIFO caches avoid one global lock on hot queries.
pub struct ProceduralTerrain {
    seed: u64,
    region: terrain::Region,
    columns: [Mutex<ColumnCache>; 16],
}
impl ProceduralTerrain {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            region: terrain::Region::generate(seed),
            columns: std::array::from_fn(|_| Mutex::new(ColumnCache::default())),
        }
    }
    pub fn extent_cells(&self) -> i64 {
        160_000
    }
    fn column(&self, x: i64, z: i64) -> Option<Column> {
        if !(0..160_000).contains(&x) || !(0..160_000).contains(&z) {
            return None;
        }
        let shard = (painting::hash(x as u64 ^ (z as u64).rotate_left(31)) & 15) as usize;
        if let Some(column) = self.columns[shard]
            .lock()
            .unwrap()
            .values
            .get(&(x, z))
            .copied()
        {
            return Some(column);
        }
        // Always sample the native world resolution. LOD only changes which
        // columns the renderer requests, never the contents of those columns.
        let sample = terrain::terrain_surface(
            &self.region,
            x as f64 * VOXEL_SIZE,
            z as f64 * VOXEL_SIZE,
            VOXEL_SIZE,
            self.seed,
        );
        let ground = (sample.height / VOXEL_SIZE).floor() as i64;
        let water = if sample.water > 0.0 {
            (sample.water / VOXEL_SIZE).floor() as i64
        } else {
            ground
        };
        let surface = match sample.material {
            0 => MaterialId::ROCK,
            2 => MaterialId::SAND,
            3 => MaterialId::GRAVEL,
            _ => MaterialId::GRASS,
        };
        let column = Column {
            ground,
            water: water.max(ground),
            bedrock: (sample.bedrock / VOXEL_SIZE).floor() as i64,
            surface: if ground >= 10_400 {
                MaterialId::SNOW
            } else {
                surface
            },
            subsoil: match surface {
                MaterialId::SAND => MaterialId::SAND,
                MaterialId::GRAVEL => MaterialId::GRAVEL,
                _ => MaterialId::SOIL,
            },
        };
        let mut cache = self.columns[shard].lock().unwrap();
        if !cache.values.contains_key(&(x, z)) {
            if cache.values.len() >= 2048 {
                if let Some(old) = cache.fifo.pop_front() {
                    cache.values.remove(&old);
                }
            }
            cache.fifo.push_back((x, z));
            cache.values.insert((x, z), column);
        }
        Some(column)
    }
}
impl Generator for ProceduralTerrain {
    fn identity(&self) -> GeneratorIdentity {
        GeneratorIdentity {
            seed: self.seed,
            version: GENERATOR_VERSION,
            definition: 0x5445525241494e31,
        }
    }
    fn voxel(&self, pos: CellPos) -> Option<Voxel> {
        let material = self.column(pos.x, pos.z)?.material_at(pos.y);
        Some(if material == MaterialId::AIR {
            Voxel::AIR
        } else {
            Voxel::new(material, painting::appearance(pos, self.seed))
        })
    }
    fn column_hint(&self, x: i64, z: i64) -> Option<Column> {
        self.column(x, z)
    }
    fn runs(&self, x: i64, z: i64, range: Range<i64>) -> Option<Vec<Run>> {
        Some(self.column(x, z)?.runs(range))
    }
}

/// Cheap deterministic generator for tests and small simulation experiments.
#[derive(Clone, Copy, Debug)]
pub struct FlatTerrain {
    pub height: i64,
    pub water: i64,
    pub seed: u64,
}
impl Generator for FlatTerrain {
    fn identity(&self) -> GeneratorIdentity {
        GeneratorIdentity {
            seed: self.seed,
            version: GENERATOR_VERSION,
            definition: painting::hash(self.height as u64)
                ^ painting::hash(self.water as u64).rotate_left(23)
                ^ 0x464c4154,
        }
    }
    fn voxel(&self, p: CellPos) -> Option<Voxel> {
        let material = self.column_hint(p.x, p.z)?.material_at(p.y);
        Some(if material == MaterialId::AIR {
            Voxel::AIR
        } else {
            Voxel::new(material, painting::appearance(p, self.seed))
        })
    }
    fn column_hint(&self, _x: i64, _z: i64) -> Option<Column> {
        Some(Column {
            ground: self.height,
            water: self.water.max(self.height),
            bedrock: (self.height - 3).max(0),
            surface: MaterialId::GRASS,
            subsoil: MaterialId::SOIL,
        })
    }
    fn runs(&self, x: i64, z: i64, range: Range<i64>) -> Option<Vec<Run>> {
        Some(self.column_hint(x, z)?.runs(range))
    }
}
