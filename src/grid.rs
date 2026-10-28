//! Coordinates and indexing, shared by every subsystem.

pub const VOXELS_PER_METER: i64 = 16;
pub const VOXEL_SIZE: f64 = 1.0 / VOXELS_PER_METER as f64;
pub const CHUNK_EDGE: i64 = 32;
pub const CHUNK_VOLUME: usize = 32 * 32 * 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellPos {
    pub x: i64,
    pub y: i64,
    pub z: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChunkPos {
    pub x: i64,
    pub y: i64,
    pub z: i64,
}

impl CellPos {
    pub const fn new(x: i64, y: i64, z: i64) -> Self {
        Self { x, y, z }
    }
    pub fn meters(self) -> [f64; 3] {
        [
            self.x as f64 * VOXEL_SIZE,
            self.y as f64 * VOXEL_SIZE,
            self.z as f64 * VOXEL_SIZE,
        ]
    }
    pub fn containing(point: [f64; 3]) -> Option<Self> {
        if !point
            .iter()
            .all(|n| n.is_finite() && n.abs() < (i64::MAX / 32) as f64)
        {
            return None;
        }
        Some(Self::new(
            (point[0] * 16.0).floor() as i64,
            (point[1] * 16.0).floor() as i64,
            (point[2] * 16.0).floor() as i64,
        ))
    }
    pub fn chunk(self) -> ChunkPos {
        ChunkPos {
            x: self.x.div_euclid(CHUNK_EDGE),
            y: self.y.div_euclid(CHUNK_EDGE),
            z: self.z.div_euclid(CHUNK_EDGE),
        }
    }
    /// Y is contiguous: terrain columns and vertical runs are common hot paths.
    pub fn local_index(self) -> u16 {
        let x = self.x.rem_euclid(CHUNK_EDGE) as usize;
        let y = self.y.rem_euclid(CHUNK_EDGE) as usize;
        let z = self.z.rem_euclid(CHUNK_EDGE) as usize;
        ((z * 32 + x) * 32 + y) as u16
    }
    pub fn offset(self, delta: [i64; 3]) -> Option<Self> {
        Some(Self::new(
            self.x.checked_add(delta[0])?,
            self.y.checked_add(delta[1])?,
            self.z.checked_add(delta[2])?,
        ))
    }
}
impl ChunkPos {
    pub fn origin(self) -> Option<CellPos> {
        Some(CellPos::new(
            self.x.checked_mul(CHUNK_EDGE)?,
            self.y.checked_mul(CHUNK_EDGE)?,
            self.z.checked_mul(CHUNK_EDGE)?,
        ))
    }
    pub fn cell(self, index: u16) -> Option<CellPos> {
        if index as usize >= CHUNK_VOLUME {
            return None;
        }
        let i = index as i64;
        self.origin()?
            .offset([(i / 32) % 32, i % 32, i / (32 * 32)])
    }
    pub fn bounds(self) -> Option<Bounds> {
        let min = self.origin()?;
        Some(Bounds {
            min,
            max: min.offset([32, 32, 32])?,
        })
    }
}

/// Half-open bounds; no allocation or iteration is allowed without checking size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub min: CellPos,
    pub max: CellPos,
}
impl Bounds {
    pub fn contains(self, p: CellPos) -> bool {
        p.x >= self.min.x
            && p.y >= self.min.y
            && p.z >= self.min.z
            && p.x < self.max.x
            && p.y < self.max.y
            && p.z < self.max.z
    }
    pub fn volume(self) -> Option<usize> {
        let x = usize::try_from(self.max.x.checked_sub(self.min.x)?).ok()?;
        let y = usize::try_from(self.max.y.checked_sub(self.min.y)?).ok()?;
        let z = usize::try_from(self.max.z.checked_sub(self.min.z)?).ok()?;
        x.checked_mul(y)?.checked_mul(z)
    }
}
