//! Persistent overrides only. Missing entries fall back to procedural generation.
//! Sparse, dense, and uniform representations share one lookup contract.
use crate::{
    grid::{CellPos, ChunkPos, CHUNK_VOLUME},
    materials::Voxel,
};
use std::collections::{BTreeMap, BTreeSet};
const PROMOTE_AT: usize = 2048;
const DEMOTE_BELOW: usize = 512;

#[derive(Clone, Debug)]
pub enum Brick {
    Sparse(BTreeMap<u16, Voxel>),
    Dense {
        values: Vec<Voxel>,
        mask: Vec<u64>,
        count: usize,
    },
    Uniform(Voxel),
}
impl Default for Brick {
    fn default() -> Self {
        Self::Sparse(BTreeMap::new())
    }
}
impl Brick {
    pub fn len(&self) -> usize {
        match self {
            Self::Sparse(v) => v.len(),
            Self::Dense { count, .. } => *count,
            Self::Uniform(_) => CHUNK_VOLUME,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn get(&self, index: u16) -> Option<Voxel> {
        let i = index as usize;
        if i >= CHUNK_VOLUME {
            return None;
        }
        match self {
            Self::Sparse(v) => v.get(&index).copied(),
            Self::Uniform(v) => Some(*v),
            Self::Dense { values, mask, .. } => {
                ((mask[i / 64] >> (i % 64)) & 1 != 0).then(|| values[i])
            }
        }
    }
    pub fn set(&mut self, index: u16, value: Option<Voxel>) {
        if matches!(self, Self::Uniform(_)) {
            let Self::Uniform(old) = *self else {
                unreachable!()
            };
            if value == Some(old) {
                return;
            }
            *self = Self::Dense {
                values: vec![old; CHUNK_VOLUME],
                mask: vec![u64::MAX; CHUNK_VOLUME / 64],
                count: CHUNK_VOLUME,
            };
        }
        match self {
            Self::Sparse(values) => {
                if let Some(value) = value {
                    values.insert(index, value);
                } else {
                    values.remove(&index);
                }
            }
            Self::Dense {
                values,
                mask,
                count,
            } => {
                let i = index as usize;
                let bit = 1u64 << (i % 64);
                let present = mask[i / 64] & bit != 0;
                if let Some(value) = value {
                    values[i] = value;
                    mask[i / 64] |= bit;
                    if !present {
                        *count += 1;
                    }
                } else {
                    mask[i / 64] &= !bit;
                    if present {
                        *count -= 1;
                    }
                }
            }
            Self::Uniform(_) => unreachable!(),
        }
    }
    pub fn compact(&mut self) {
        if let Self::Sparse(values) = self {
            if values.len() < PROMOTE_AT {
                return;
            }
            let mut dense = vec![Voxel::AIR; CHUNK_VOLUME];
            let mut mask = vec![0u64; CHUNK_VOLUME / 64];
            for (&i, &v) in values.iter() {
                dense[i as usize] = v;
                mask[i as usize / 64] |= 1 << (i as usize % 64);
            }
            *self = Self::Dense {
                values: dense,
                mask,
                count: values.len(),
            };
        }
        if let Self::Dense {
            values,
            mask,
            count,
        } = self
        {
            if *count == CHUNK_VOLUME && values.iter().all(|v| *v == values[0]) {
                *self = Self::Uniform(values[0]);
            } else if *count < DEMOTE_BELOW {
                let mut sparse = BTreeMap::new();
                for i in 0..CHUNK_VOLUME {
                    if mask[i / 64] & (1 << (i % 64)) != 0 {
                        sparse.insert(i as u16, values[i]);
                    }
                }
                *self = Self::Sparse(sparse);
            }
        }
    }
    pub fn entries(&self) -> Vec<(u16, Voxel)> {
        (0..CHUNK_VOLUME)
            .filter_map(|i| self.get(i as u16).map(|v| (i as u16, v)))
            .collect()
    }
    pub fn memory_bytes(&self) -> usize {
        match self {
            Self::Sparse(v) => v.len() * 32,
            Self::Dense { values, mask, .. } => values.capacity() * 4 + mask.capacity() * 8,
            Self::Uniform(_) => 4,
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct EditStore {
    pub(crate) bricks: BTreeMap<ChunkPos, Brick>,
    columns: BTreeMap<(i64, i64), BTreeSet<i64>>,
}
impl EditStore {
    pub fn get(&self, p: CellPos) -> Option<Voxel> {
        self.bricks.get(&p.chunk())?.get(p.local_index())
    }
    pub fn set(&mut self, p: CellPos, value: Option<Voxel>) {
        let key = p.chunk();
        if value.is_none() && !self.bricks.contains_key(&key) {
            return;
        }
        self.bricks
            .entry(key)
            .or_default()
            .set(p.local_index(), value);
        self.columns
            .entry((key.x, key.z))
            .or_default()
            .insert(key.y);
    }
    pub fn compact(&mut self, keys: &BTreeSet<ChunkPos>) {
        for key in keys {
            if let Some(brick) = self.bricks.get_mut(key) {
                brick.compact();
                if !brick.is_empty() {
                    continue;
                }
            }
            self.bricks.remove(key);
            if let Some(ys) = self.columns.get_mut(&(key.x, key.z)) {
                ys.remove(&key.y);
                if ys.is_empty() {
                    self.columns.remove(&(key.x, key.z));
                }
            }
        }
    }
    pub fn column_edits(&self, x: i64, z: i64) -> Vec<(i64, Voxel)> {
        let p = CellPos::new(x, 0, z);
        let key = p.chunk();
        let mut result = Vec::new();
        if let Some(ys) = self.columns.get(&(key.x, key.z)) {
            for &y in ys {
                let chunk = ChunkPos {
                    x: key.x,
                    y,
                    z: key.z,
                };
                let brick = &self.bricks[&chunk];
                let base = CellPos::new(x, y * 32, z).local_index();
                for offset in 0..32u16 {
                    if let Some(v) = brick.get(base + offset) {
                        result.push((y * 32 + offset as i64, v));
                    }
                }
            }
        }
        result
    }
    pub fn chunk_count(&self) -> usize {
        self.bricks.len()
    }
    pub fn override_count(&self) -> usize {
        self.bricks.values().map(Brick::len).sum()
    }
    pub fn memory_bytes(&self) -> usize {
        self.bricks.values().map(Brick::memory_bytes).sum()
    }
}
