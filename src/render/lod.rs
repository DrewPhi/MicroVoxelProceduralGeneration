//! A complete, non-overlapping quadtree cover. Adjacent leaves differ by at most
//! one level; a changed cover is installed only after its mesh jobs are complete.
use std::collections::{BTreeMap, BTreeSet};
pub const SIDE: i64 = 32;
pub const ROOT: i64 = 262_144;
pub const FINEST: u8 = 13;
pub const EXTENT: i64 = 160_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TileKey {
    pub level: u8,
    pub x: u32,
    pub z: u32,
}
impl TileKey {
    pub fn step(self) -> i64 {
        1 << (FINEST - self.level)
    }
    pub fn edge(self) -> i64 {
        SIDE * self.step()
    }
    pub fn origin(self) -> [i64; 2] {
        [self.x as i64 * self.edge(), self.z as i64 * self.edge()]
    }
    pub fn children(self) -> impl Iterator<Item = Self> {
        (0..4).map(move |i| Self {
            level: self.level + 1,
            x: self.x * 2 + i % 2,
            z: self.z * 2 + i / 2,
        })
    }
    pub fn parent(self) -> Option<Self> {
        (self.level > 0).then(|| Self {
            level: self.level - 1,
            x: self.x / 2,
            z: self.z / 2,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub tiles: BTreeSet<TileKey>,
}
impl Layout {
    pub fn root() -> Self {
        Self {
            tiles: [TileKey {
                level: 0,
                x: 0,
                z: 0,
            }]
            .into(),
        }
    }
    pub fn at(&self, x: i64, z: i64) -> Option<TileKey> {
        if !(0..EXTENT).contains(&x) || !(0..EXTENT).contains(&z) {
            return None;
        }
        for level in (0..=FINEST).rev() {
            let edge = ROOT >> level;
            let key = TileKey {
                level,
                x: (x / edge) as u32,
                z: (z / edge) as u32,
            };
            if self.tiles.contains(&key) {
                return Some(key);
            }
        }
        None
    }
    /// Side order: -X,+X,-Z,+Z. Balance makes midpoint sampling sufficient:
    /// each side has either one coarse neighbor or two equal fine neighbors.
    pub fn neighbors(&self, key: TileKey) -> [u8; 4] {
        let [x, z] = key.origin();
        let e = key.edge();
        [
            [x - 1, z + e / 2],
            [x + e, z + e / 2],
            [x + e / 2, z - 1],
            [x + e / 2, z + e],
        ]
        .map(|p| self.at(p[0], p[1]).map_or(key.level, |n| n.level))
    }
    fn split(&mut self, key: TileKey) {
        self.tiles.remove(&key);
        for child in key.children() {
            let [x, z] = child.origin();
            if x < EXTENT && z < EXTENT {
                self.tiles.insert(child);
            }
        }
    }
    fn balance(&mut self) {
        loop {
            let mut split = BTreeSet::new();
            for &key in &self.tiles {
                let [x, z] = key.origin();
                let e = key.edge();
                for p in [
                    [x - 1, z + e / 2],
                    [x + e, z + e / 2],
                    [x + e / 2, z - 1],
                    [x + e / 2, z + e],
                ] {
                    if let Some(n) = self.at(p[0], p[1]) {
                        if n.level + 1 < key.level {
                            split.insert(n);
                        }
                    }
                }
            }
            if split.is_empty() {
                break;
            }
            for key in split {
                self.split(key);
            }
        }
    }
}
/// Uses screen-space error, a guaranteed native neighborhood, and hysteresis.
/// Coverage includes the full finite region. Frustum culling happens at draw time.
pub fn select(eye: [f64; 3], height: u32, fov: f64, previous: &Layout, budget: usize) -> Layout {
    let mut previous_splits = BTreeSet::new();
    for &key in &previous.tiles {
        let mut k = key;
        while let Some(p) = k.parent() {
            previous_splits.insert(p);
            k = p;
        }
    }
    let focal = height as f64 / (2.0 * (fov / 2.0).tan());
    let mut threshold = 4.0;
    for attempt in 0..16 {
        let mut result = Layout::root();
        let mut pending = vec![*result.tiles.first().unwrap()];
        while let Some(key) = pending.pop() {
            if key.level == FINEST {
                continue;
            }
            let [x, z] = key.origin();
            let e = key.edge();
            let dx = (x as f64 / 16.0 - eye[0])
                .max(0.0)
                .max(eye[0] - (x + e).min(EXTENT) as f64 / 16.0);
            let dz = (z as f64 / 16.0 - eye[2])
                .max(0.0)
                .max(eye[2] - (z + e).min(EXTENT) as f64 / 16.0);
            let distance = dx.hypot(dz).max(0.1);
            let limit = threshold
                * if previous_splits.contains(&key) {
                    0.8
                } else {
                    1.2
                };
            if distance < 4.0 || key.step() as f64 / 16.0 * focal / distance > limit {
                result.split(key);
                pending.extend(key.children().filter(|k| result.tiles.contains(k)));
            }
        }
        result.balance();
        if result.tiles.len() <= budget || attempt == 15 {
            return result;
        }
        threshold *= 1.35;
    }
    unreachable!()
}
/// Neighbor levels are part of mesh identity: a boundary changes when its LOD
/// neighbor changes, even if the tile's own level remains unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MeshKey {
    pub tile: TileKey,
    pub neighbors: [u8; 4],
    pub revision: u64,
}
impl Layout {
    pub fn mesh_keys(&self, revision: u64) -> BTreeMap<TileKey, MeshKey> {
        self.tiles
            .iter()
            .map(|&tile| {
                (
                    tile,
                    MeshKey {
                        tile,
                        neighbors: self.neighbors(tile),
                        revision,
                    },
                )
            })
            .collect()
    }
}
