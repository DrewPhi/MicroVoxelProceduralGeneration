//! Greenfield canonical native-cell world. A bounded integer fixture and a
//! CPU-f64 procedural heightfield share material, edit and reference-ray semantics.
//! These are test sources, not the production geology or persistence system.
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

pub mod terrain;
pub use terrain::{FbmError, FbmParams, FbmTerrain, FBM_SOURCE_VERSION};

pub const VOXEL_SIZE: f64 = 0.0625;
pub const SOURCE_VERSION: u32 = 1;
pub const CAVE_CENTER_M: [f64; 3] = [-3.0, 3.0, 2.0];
pub const CAVE_RADIUS_M: f64 = 1.5;
pub const BRIDGE_MIN_M: [f64; 3] = [2.0, 4.0, -2.0];
pub const BRIDGE_MAX_M: [f64; 3] = [7.0, 4.25, -1.0];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Cell {
    pub x: i64,
    pub y: i64,
    pub z: i64,
}
impl Cell {
    /// Half-open cells: -0.001 m belongs to cell -1. Panics for nonfinite or
    /// unaddressable coordinates; raycast handles these inputs by returning None.
    pub fn from_meters(p: [f64; 3]) -> Self {
        checked_cell(p).expect("position must be finite and within the i64 cell lattice")
    }
    pub fn center_m(self) -> [f64; 3] {
        [self.x, self.y, self.z].map(|v| (v as f64 + 0.5) * VOXEL_SIZE)
    }
}
fn checked_cell(p: [f64; 3]) -> Option<Cell> {
    let mut c = [0; 3];
    for i in 0..3 {
        let v = (p[i] / VOXEL_SIZE).floor();
        if !v.is_finite() || v < i64::MIN as f64 || v >= -(i64::MIN as f64) {
            return None;
        }
        c[i] = v as i64;
    }
    Some(Cell {
        x: c[0],
        y: c[1],
        z: c[2],
    })
}

/// Subtract integer cell origins before converting to f32 for a GPU/local scene.
/// Keep origins within 2^53 cells when roundtripping through world-meter f64.
#[derive(Clone, Copy, Debug)]
pub struct LocalOrigin(pub Cell);
impl LocalOrigin {
    pub fn center_relative_m(self, cell: Cell) -> [f32; 3] {
        let delta = [
            cell.x as i128 - self.0.x as i128,
            cell.y as i128 - self.0.y as i128,
            cell.z as i128 - self.0.z as i128,
        ];
        delta.map(|d| ((d as f64 + 0.5) * VOXEL_SIZE) as f32)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Material {
    Air,
    Grass,
    Soil,
    Stone,
    Wood,
}
impl Material {
    pub fn color(self) -> [f32; 3] {
        match self {
            Self::Air => [0.0, 0.0, 0.0],
            Self::Grass => [0.26, 0.53, 0.16],
            Self::Soil => [0.40, 0.27, 0.15],
            Self::Stone => [0.48, 0.50, 0.54],
            Self::Wood => [0.57, 0.34, 0.14],
        }
    }
}

/// Clones share an immutable edit map; the next mutation uses copy-on-write.
/// Revision changes only for effective material changes and is unique among
/// divergent clones of this source lineage. Independently constructed worlds
/// need a separate runtime identity in cache keys. No save journal yet.
#[derive(Clone, Debug)]
pub struct World {
    seed: u64,
    terrain: Option<FbmTerrain>,
    edits: Arc<BTreeMap<Cell, Material>>,
    revision: u64,
    counter: Arc<AtomicU64>,
}

/// Read-only column snapshot borrowing its world. Reuses the canonical source
/// height for many native y queries; material overrides and fixture volumes still
/// apply. Useful for bounded native reference extraction, not a display LOD.
pub struct WorldColumn<'a> {
    world: &'a World,
    x: i64,
    z: i64,
    height: i64,
}
impl WorldColumn<'_> {
    pub fn height_m(&self) -> f64 {
        self.height as f64 * VOXEL_SIZE
    }
    pub fn material(&self, y: i64) -> Material {
        let c = Cell {
            x: self.x,
            y,
            z: self.z,
        };
        self.world
            .edits
            .get(&c)
            .copied()
            .unwrap_or_else(|| self.world.source_with_height(c, self.height))
    }
}
impl World {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            terrain: None,
            edits: Arc::default(),
            revision: 0,
            counter: Arc::new(AtomicU64::new(0)),
        }
    }
    /// Procedural heightfield source. No fixture cave or bridge is added.
    pub fn fbm(seed: u64, params: FbmParams) -> Result<Self, FbmError> {
        let terrain = FbmTerrain::new(seed, params)?;
        Ok(Self {
            terrain: Some(terrain),
            ..Self::new(seed)
        })
    }
    /// Immutable unedited surface source for CPU display-page generation.
    /// Edits remain authoritative through `material`; these pages exclude edits.
    pub fn fbm_terrain(&self) -> Option<&FbmTerrain> {
        self.terrain.as_ref()
    }
    pub fn column(&self, x: i64, z: i64) -> WorldColumn<'_> {
        WorldColumn {
            world: self,
            x,
            z,
            height: self.height_cells(x, z),
        }
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn seed(&self) -> u64 {
        self.seed
    }
    pub fn set(&mut self, c: Cell, m: Material) {
        if self.material(c) == m {
            return;
        }
        let next = self
            .counter
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
            .expect("world revision exhausted")
            + 1;
        Arc::make_mut(&mut self.edits).insert(c, m);
        self.revision = next;
    }
    /// Base terrain top in meters, excluding caves, bridge and edits. Quantized
    /// to canonical columns, so consumers cannot mistake a smooth H for voxels.
    pub fn height_m(&self, x: f64, z: f64) -> f64 {
        let c = Cell::from_meters([x, 0.0, z]);
        self.height_cells(c.x, c.z) as f64 * VOXEL_SIZE
    }
    pub fn material(&self, c: Cell) -> Material {
        self.edits
            .get(&c)
            .copied()
            .unwrap_or_else(|| self.source(c))
    }
    fn height_cells(&self, x: i64, z: i64) -> i64 {
        if let Some(terrain) = &self.terrain {
            return terrain.height_cells(x, z);
        }
        // Bounded quadratic mountain/ridge, with deterministic broad rolling hills.
        let dx = (x as i128 + 48).abs().min(128) as i64;
        let dz = (z as i128 - 32).abs().min(160) as i64;
        let mountain =
            (16_384 - dx * dx).max(0) * (25_600 - dz * dz).max(0) * 128 / 16_384 / 25_600;
        let rolling = smooth_noise(x, z, self.seed);
        let mut h = 16 + mountain + rolling / 4;
        // A trench across the bridge span; sharp sides deliberately exercise 3D ownership.
        if (48..96).contains(&x) && (-64..0).contains(&z) {
            h = -16;
        }
        h
    }
    fn source(&self, c: Cell) -> Material {
        self.source_with_height(c, self.height_cells(c.x, c.z))
    }
    fn source_with_height(&self, c: Cell, h: i64) -> Material {
        if let Some(terrain) = &self.terrain {
            return if c.y >= h {
                Material::Air
            } else if c.y == h - 1 {
                let [x, _, z] = c.center_m();
                terrain.surface_material(x, z, h as f64 * VOXEL_SIZE)
            } else if c.y >= h - 12 {
                Material::Soil
            } else {
                Material::Stone
            };
        }
        // Bridge deck and posts. Deck bounds [2,7)x[4,4.25)x[-2,-1)m.
        if (32..112).contains(&c.x) && (-32..-16).contains(&c.z) {
            if (64..68).contains(&c.y) {
                return Material::Wood;
            }
            if (0..64).contains(&c.y) && ((32..36).contains(&c.x) || (108..112).contains(&c.x)) {
                return Material::Stone;
            }
        }
        if c.y >= h {
            return Material::Air;
        }
        // Through-tunnel: x=-3m, center y=3m, radius1.5m, z=-12..16m.
        let dx = c.x as i128 + 48;
        let dy = c.y as i128 - 48;
        if (-192..256).contains(&c.z)
            && dx.abs() <= 24
            && dy.abs() <= 24
            && dx * dx + dy * dy < 24 * 24
        {
            return Material::Air;
        }
        if c.y == h - 1 {
            Material::Grass
        } else if c.y >= h - 12 {
            Material::Soil
        } else {
            Material::Stone
        }
    }
}
fn smooth_noise(x: i64, z: i64, seed: u64) -> i64 {
    fn sample(x: i64, z: i64, s: u64) -> i64 {
        let mut h = (x as u64).wrapping_mul(0x9e3779b97f4a7c15)
            ^ (z as u64).wrapping_mul(0xbf58476d1ce4e5b9)
            ^ s;
        h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        h = (h ^ (h >> 27)).wrapping_mul(0x94d049bb133111eb);
        ((h ^ (h >> 31)) % 49) as i64
    }
    let gx = x.div_euclid(256);
    let gz = z.div_euclid(256);
    let u = x.rem_euclid(256);
    let v = z.rem_euclid(256);
    let a = sample(gx, gz, seed) * (256 - u) + sample(gx + 1, gz, seed) * u;
    let b = sample(gx, gz + 1, seed) * (256 - u) + sample(gx + 1, gz + 1, seed) * u;
    (a * (256 - v) + b * v) / 65536
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub cell: Cell,
    pub material: Material,
    pub distance: f64,
    pub normal: [i32; 3],
}

/// Exact native-cell reference DDA; direction magnitude has no effect on distances.
/// On a grid plane, the forward cell owns t=0 (negative directions use the lower
/// cell). Interior starts return distance0/normal0. Tied axes advance together,
/// skipping cells touched only at an edge/corner; normal uses the lowest tied axis.
/// max_distance is inclusive. Invalid/nonfinite/zero rays return None.
pub fn raycast(
    world: &World,
    origin: [f64; 3],
    direction: [f64; 3],
    max_distance: f64,
) -> Option<RayHit> {
    if !max_distance.is_finite() || max_distance < 0.0 || !direction.iter().all(|x| x.is_finite()) {
        return None;
    }
    let scale = direction.iter().fold(0.0_f64, |a, b| a.max(b.abs()));
    if scale == 0.0 {
        return None;
    }
    let scaled = direction.map(|d| d / scale);
    let len = scaled.iter().map(|d| d * d).sum::<f64>().sqrt();
    let d = scaled.map(|v| v / len);
    let initial = checked_cell(origin)?;
    let mut c = [initial.x, initial.y, initial.z];
    let step = d.map(|v| {
        if v > 0.0 {
            1_i64
        } else if v < 0.0 {
            -1
        } else {
            0
        }
    });
    let mut next = [f64::INFINITY; 3];
    let mut delta = [f64::INFINITY; 3];
    for i in 0..3 {
        if step[i] < 0 && origin[i] / VOXEL_SIZE == (origin[i] / VOXEL_SIZE).floor() {
            c[i] = c[i].checked_sub(1)?;
        }
        if step[i] != 0 {
            let plane = if step[i] > 0 {
                c[i].checked_add(1)?
            } else {
                c[i]
            };
            next[i] = ((plane as f64 * VOXEL_SIZE - origin[i]) / d[i]).max(0.0);
            delta[i] = VOXEL_SIZE / d[i].abs();
        }
    }
    let mut distance = 0.0;
    let mut normal = [0; 3];
    loop {
        let cell = Cell {
            x: c[0],
            y: c[1],
            z: c[2],
        };
        let material = world.material(cell);
        if material != Material::Air {
            return Some(RayHit {
                cell,
                material,
                distance,
                normal,
            });
        }
        let t = next.iter().copied().fold(f64::INFINITY, f64::min);
        if t > max_distance || !t.is_finite() {
            return None;
        }
        normal = [0; 3];
        // Exact ties only: near-corner slivers remain canonical ray intersections.
        for i in 0..3 {
            if next[i] == t {
                if normal == [0; 3] {
                    normal[i] = -(step[i] as i32);
                }
                c[i] = c[i].checked_add(step[i])?;
                next[i] += delta[i];
            }
        }
        distance = t;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn negative_grid_and_local_precision() {
        assert_eq!(
            Cell::from_meters([-0.001, -0.0625, -0.062501]),
            Cell {
                x: -1,
                y: -1,
                z: -2
            }
        );
        for sign in [-1, 1] {
            let base = Cell {
                x: sign * 1_600_000,
                y: 0,
                z: -sign * 1_600_000,
            };
            for dx in -10..10 {
                let c = Cell {
                    x: base.x + dx,
                    y: 3,
                    z: base.z - 7,
                };
                assert_eq!(Cell::from_meters(c.center_m()), c);
                assert_eq!(
                    LocalOrigin(base).center_relative_m(c),
                    [(dx as f32 + 0.5) / 16.0, 3.5 / 16.0, -6.5 / 16.0]
                );
            }
        }
    }
    #[test]
    fn source_edits_and_snapshot_revisions() {
        let mut a = World::new(42);
        let mut b = a.clone();
        let cave = Cell::from_meters(CAVE_CENTER_M);
        assert_eq!(a.material(cave), Material::Air);
        assert_eq!(
            a.material(Cell::from_meters([4.0, 4.1, -1.5])),
            Material::Wood
        );
        let h = a.height_m(-3.0, 2.0);
        assert!((8.0..11.0).contains(&h));
        assert_eq!(
            a.material(Cell::from_meters([-3.0, h - VOXEL_SIZE, 2.0])),
            Material::Grass
        );
        a.set(cave, Material::Stone);
        b.set(cave, Material::Wood);
        assert_ne!(a.revision(), b.revision());
        assert_eq!(a.material(cave), Material::Stone);
        assert_eq!(b.material(cave), Material::Wood);
        let r = a.revision();
        a.set(cave, Material::Stone);
        assert_eq!(a.revision(), r);
        a.set(cave, Material::Air);
        assert_eq!(a.material(cave), Material::Air);
        for x in -50..50 {
            let c = Cell {
                x: x * 37,
                y: 40,
                z: x * 23,
            };
            assert_eq!(World::new(42).material(c), World::new(42).material(c));
        }
    }
    #[test]
    fn ray_boundary_ties_and_invalid_inputs() {
        let mut w = World::new(0);
        let p = Cell {
            x: 0,
            y: 1600,
            z: 0,
        };
        w.set(p, Material::Wood);
        let hit = raycast(&w, [-VOXEL_SIZE, 100.03125, 0.03125], [2.0, 0.0, 0.0], 1.0).unwrap();
        assert_eq!(hit.cell, p);
        assert_eq!(hit.normal, [-1, 0, 0]);
        assert_eq!(hit.distance, VOXEL_SIZE);
        assert!(raycast(
            &w,
            [-VOXEL_SIZE, 100.03125, 0.03125],
            [1.0, 0.0, 0.0],
            VOXEL_SIZE
        )
        .is_some());
        assert!(raycast(
            &w,
            [-VOXEL_SIZE, 100.03125, 0.03125],
            [1.0, 0.0, 0.0],
            VOXEL_SIZE / 2.0
        )
        .is_none());
        assert!(raycast(&w, [0.0, 100.03125, 0.03125], [-1.0, 0.0, 0.0], 0.0).is_none());
        assert_eq!(
            raycast(&w, [0.0, 100.03125, 0.03125], [1.0, 0.0, 0.0], 0.0)
                .unwrap()
                .cell,
            p
        );
        let mut corner = World::new(0);
        corner.set(
            Cell {
                x: 1,
                y: 1600,
                z: 0,
            },
            Material::Stone,
        );
        corner.set(
            Cell {
                x: 1,
                y: 1601,
                z: 0,
            },
            Material::Wood,
        );
        let h = raycast(&corner, [0.03125, 100.03125, 0.03125], [1.0, 1.0, 0.0], 1.0).unwrap();
        assert_eq!(h.material, Material::Wood);
        assert_eq!(h.normal, [-1, 0, 0]);
        for (o, d, m) in [
            ([f64::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], 1.0),
            ([0.0; 3], [0.0; 3], 1.0),
            ([0.0; 3], [f64::INFINITY, 0.0, 0.0], 1.0),
            ([0.0; 3], [1.0, 0.0, 0.0], -1.0),
        ] {
            assert!(raycast(&w, o, d, m).is_none());
        }
    }
    #[test]
    fn cave_through_ray_and_far_edit() {
        let mut w = World::new(42);
        assert!(raycast(&w, [-3.0, 3.0, -10.0], [0.0, 0.0, 1.0], 24.0).is_none());
        let far = Cell::from_meters([100_000.0, 100.0, -100_000.0]);
        w.set(far, Material::Wood);
        let center = far.center_m();
        let start = [center[0] - 1.0, center[1], center[2]];
        let hit = raycast(&w, start, [1.0, 0.0, 0.0], 2.0).unwrap();
        assert_eq!(hit.cell, far);
        assert_eq!(hit.distance, 1.0 - VOXEL_SIZE / 2.0);
    }
    fn box_distance(o: [f64; 3], d: [f64; 3], lo: [f64; 3], hi: [f64; 3]) -> Option<f64> {
        let n = d.iter().map(|x| x * x).sum::<f64>().sqrt();
        let mut enter = 0.0_f64;
        let mut exit = 10.0_f64;
        for i in 0..3 {
            if d[i] == 0.0 {
                if o[i] < lo[i] || o[i] >= hi[i] {
                    return None;
                }
            } else {
                let a = (lo[i] - o[i]) * n / d[i];
                let b = (hi[i] - o[i]) * n / d[i];
                enter = enter.max(a.min(b));
                exit = exit.min(a.max(b));
            }
        }
        (enter < exit).then_some(enter)
    }
    #[test]
    fn dda_matches_independent_box_oracle() {
        let mut w = World::new(7);
        for x in -3..4 {
            for y in 1600..1605 {
                for z in -2..3 {
                    w.set(Cell { x, y, z }, Material::Wood);
                }
            }
        }
        let lo = [-3.0 / 16.0, 100.0, -2.0 / 16.0];
        let hi = [4.0 / 16.0, 100.0 + 5.0 / 16.0, 3.0 / 16.0];
        let mut state = 19_u64;
        for _ in 0..500 {
            let mut rnd = || {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((state >> 32) as f64 / u32::MAX as f64) * 2.0 - 1.0
            };
            let o = [rnd(), 100.15 + rnd(), rnd()];
            let d = [rnd(), rnd(), rnd()];
            let expected = box_distance(o, d, lo, hi);
            let got = raycast(&w, o, d, 10.0);
            assert_eq!(
                got.is_some(),
                expected.is_some(),
                "origin={o:?} direction={d:?}"
            );
            if let (Some(g), Some(e)) = (got, expected) {
                assert!((g.distance - e).abs() < 1e-10, "{g:?} vs {e}");
            }
        }
    }
}
