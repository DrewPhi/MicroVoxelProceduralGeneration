//! A complete, non-overlapping quadtree cover. Adjacent leaves differ by at most
//! one level; local patches include all changed neighbor dependencies.
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
    pub fn split(&mut self, key: TileKey) {
        self.tiles.remove(&key);
        for child in key.children() {
            let [x, z] = child.origin();
            if x < EXTENT && z < EXTENT {
                self.tiles.insert(child);
            }
        }
    }
    pub fn balance(&mut self) {
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
/// Display quality is independent of canonical voxel resolution. The error is
/// projected display-cell size, not a proven geometric terrain-error bound.
#[derive(Clone, Copy, Debug)]
pub struct Quality {
    pub pixels: f64,
    pub native_radius: f64,
    pub direction: [f64; 3],
    pub visibility: f64,
}
impl Default for Quality {
    fn default() -> Self {
        Self {
            pixels: 2.0,
            native_radius: 6.0,
            direction: [0., 0., -1.],
            visibility: 24000.,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Selection {
    pub layout: Layout,
    pub requested_pixels: f64,
    pub achieved_pixels: f64,
    pub budget_limited: bool,
}
/// Backwards-compatible deterministic selection for tools.
pub fn select(eye: [f64; 3], height: u32, fov: f64, previous: &Layout, budget: usize) -> Layout {
    select_quality(
        eye,
        height,
        fov,
        previous,
        budget,
        Quality {
            pixels: 4.,
            native_radius: 4.,
            ..Quality::default()
        },
    )
    .layout
}
pub fn select_quality(
    eye: [f64; 3],
    height: u32,
    fov: f64,
    previous: &Layout,
    budget: usize,
    quality: Quality,
) -> Selection {
    select_adaptive(eye, height, fov, previous, budget, quality, |_| None)
}
/// Approximate terrain bounds in meters, and sampled height interpolation error.
/// Selection remains conservative in footprint even when terrain data is absent.
#[derive(Clone, Copy, Debug)]
pub struct TerrainError {
    pub low: f64,
    pub high: f64,
    pub deviation: f64,
}
pub fn select_adaptive(
    eye: [f64; 3],
    height: u32,
    fov: f64,
    previous: &Layout,
    budget: usize,
    quality: Quality,
    mut terrain: impl FnMut(TileKey) -> Option<TerrainError>,
) -> Selection {
    let mut previous_splits = BTreeSet::new();
    for &key in &previous.tiles {
        let mut k = key;
        while let Some(p) = k.parent() {
            previous_splits.insert(p);
            k = p;
        }
    }
    let focal = height as f64 / (2.0 * (fov / 2.0).tan());
    let requested = if quality.pixels.is_finite() {
        quality.pixels.clamp(1., 16.)
    } else {
        2.
    };
    let radius = if quality.native_radius.is_finite() {
        quality.native_radius.clamp(2., 32.)
    } else {
        6.
    };
    let mut threshold = requested;
    for attempt in 0..24 {
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
            let horizontal = dx.hypot(dz);
            if horizontal > quality.visibility.max(radius * 2.) {
                continue;
            }
            let bounds = terrain(key);
            let vertical = bounds.map_or(0., |b| (b.low - eye[1]).max(0.).max(eye[1] - b.high));
            let distance = horizontal.hypot(vertical).max(0.1);
            let importance = bounds.map_or(1., |b| {
                1. + (b.deviation / (key.step() as f64 / 16.).max(1.)).clamp(0., 1.)
            });
            let center = [
                (x + e / 2) as f64 / 16. - eye[0],
                (z + e / 2) as f64 / 16. - eye[2],
            ];
            let length = center[0].hypot(center[1]).max(0.1);
            let facing =
                (center[0] * quality.direction[0] + center[1] * quality.direction[2]) / length;
            // Retain detail around the player for turning. Far behind the camera
            // use four times the projected size, while retaining complete coverage.
            let peripheral = if distance < radius * 2. || length < e as f64 / 16. {
                1.
            } else {
                1. + 3. * ((-facing).max(0.)).min(1.)
            };
            let limit = threshold
                * peripheral
                * if previous_splits.contains(&key) {
                    0.85
                } else {
                    1.15
                };
            if horizontal < radius
                || importance * key.step() as f64 / 16.0 * focal / distance > limit
            {
                result.split(key);
                pending.extend(key.children().filter(|k| result.tiles.contains(k)));
            }
        }
        result.balance();
        if result.tiles.len() <= budget || attempt == 23 {
            return Selection {
                layout: result,
                requested_pixels: requested,
                achieved_pixels: threshold,
                budget_limited: attempt != 0,
            };
        }
        threshold *= 1.25;
    }
    unreachable!()
}
/// One balanced local step toward a desired cover. A patch includes the changed
/// neighbor mesh keys, so swapping it cannot leave stale boundary geometry.
/// Fine tiles outside the desired cover coarsen first to free residency.
pub fn next_patch(active: &Layout, desired: &Layout, eye: [f64; 3]) -> Option<Layout> {
    next_patch_budget(
        active,
        desired,
        eye,
        active.tiles.len().max(desired.tiles.len()) + 64,
    )
}
/// Refinement has first claim on spare residency; coarsen only to free space.
pub fn next_patch_budget(
    active: &Layout,
    desired: &Layout,
    eye: [f64; 3],
    budget: usize,
) -> Option<Layout> {
    if active == desired {
        return None;
    }
    let mut merges = BTreeSet::new();
    for &key in &active.tiles {
        if let Some(parent) = key.parent() {
            let [x, z] = parent.origin();
            if desired.at(x, z).is_some_and(|d| d.level <= parent.level)
                && parent
                    .children()
                    .filter(|c| {
                        let [x, z] = c.origin();
                        x < EXTENT && z < EXTENT
                    })
                    .all(|c| active.tiles.contains(&c))
            {
                merges.insert(parent);
            }
        }
    }
    let distance = |key: TileKey| {
        let [x, z] = key.origin();
        let e = key.edge();
        ((x + e / 2) as f64 / 16. - eye[0]).hypot((z + e / 2) as f64 / 16. - eye[2])
    };
    let mut splits: Vec<_> = active
        .tiles
        .iter()
        .copied()
        .filter(|key| {
            let [x, z] = key.origin();
            key.level < FINEST
                && !desired.tiles.contains(key)
                && desired.at(x, z).is_some_and(|d| d.level > key.level)
        })
        .collect();
    splits.sort_by(|a, b| distance(*a).total_cmp(&distance(*b)));
    if active.tiles.len() + 3 <= budget {
        if let Some(key) = splits.first() {
            let mut next = active.clone();
            next.split(*key);
            next.balance();
            if next.tiles.len() <= budget {
                return Some(next);
            }
        }
    }
    let mut candidates: Vec<_> = merges.into_iter().collect();
    candidates.sort_by(|a, b| distance(*b).total_cmp(&distance(*a)));
    for parent in candidates {
        let [x, z] = parent.origin();
        let e = parent.edge();
        let too_fine = [e / 4, 3 * e / 4].into_iter().any(|offset| {
            [
                [x - 1, z + offset],
                [x + e, z + offset],
                [x + offset, z - 1],
                [x + offset, z + e],
            ]
            .into_iter()
            .any(|[nx, nz]| {
                active
                    .at(nx, nz)
                    .is_some_and(|n| n.level > parent.level + 1)
            })
        });
        if too_fine {
            continue;
        }
        let mut next = active.clone();
        for child in parent.children() {
            next.tiles.remove(&child);
        }
        next.tiles.insert(parent);
        // The neighbor check makes this merge balanced by construction. Avoid
        // cloning/rebalancing every rejected candidate on the rendering thread.
        return Some(next);
    }
    let mut splits: Vec<_> = active
        .tiles
        .iter()
        .copied()
        .filter(|key| {
            let [x, z] = key.origin();
            key.level < FINEST
                && !desired.tiles.contains(key)
                && desired.at(x, z).is_some_and(|d| d.level > key.level)
        })
        .collect();
    splits.sort_by(|a, b| distance(*a).total_cmp(&distance(*b)));
    splits.first().map(|key| {
        let mut next = active.clone();
        next.split(*key);
        next.balance();
        next
    })
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
