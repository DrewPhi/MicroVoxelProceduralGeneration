//! Procedural water geometry and finite spatial-reservoir initialization.
//!
//! This module supplies identities, exact native counts and bounded geometry.
//! It does not infer discharge, move water across boundaries, or turn a small
//! tile's stored volume into the head of an entire river or lake.
use crate::{
    discrete_water::regional::{RegionalWater, ReservoirId},
    Cell, FbmRecipe, FbmTerrain, Material, World, voxel_size,
};
use std::collections::{BTreeMap, BTreeSet};

pub const TILE_EDGE: i64 = 16;
const TILE_CELLS: usize = 4096;
const MASK_WORDS: usize = TILE_CELLS / 64;
const MAX_TILES: usize = 4096;

/// Identity is a tuple, never a hash with an assumed absence of collisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReachKey {
    pub source_version: u32,
    pub owner: [i64; 2],
    pub segment: u8,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReachPoint {
    pub x_m: f64,
    pub z_m: f64,
    pub stage_m: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterReach {
    pub key: ReachKey,
    pub upstream: ReachPoint,
    pub downstream: ReachPoint,
    /// Half widths of the canonical water-core profile, not discharge.
    pub upstream_radius_m: f64,
    pub downstream_radius_m: f64,
    pub lake: bool,
}
impl FbmTerrain {
    /// Bounded public view of the existing globally addressed source geometry.
    /// Returns descriptors whose water-core bounding boxes overlap the query.
    /// No hydraulic rate or accumulated watershed discharge is implied.
    pub fn water_reaches(&self, min: [f64; 2], max: [f64; 2]) -> Result<Vec<WaterReach>, String> {
        if self.params().recipe != FbmRecipe::SeededWorld {
            return Err("procedural water descriptors require SeededWorld".into());
        }
        let max_span = 9600. * self.params().horizontal_scale / 1800.;
        if (0..2).any(|a| {
            !min[a].is_finite()
                || !max[a].is_finite()
                || min[a].abs() >= i64::MAX as f64 * voxel_size()
                || max[a].abs() >= i64::MAX as f64 * voxel_size()
                || min[a] > max[a]
                || max[a] - min[a] > max_span
        }) {
            return Err("invalid or excessive procedural water query".into());
        }
        let mut reaches: Vec<_> = self
            .seeded_reaches(min, max)
            .into_iter()
            .filter(|r| {
                let radius = r.width.max(r.width_b);
                (0..2).all(|a| {
                    let (u, v) = if a == 0 {
                        (r.a.x, r.b.x)
                    } else {
                        (r.a.z, r.b.z)
                    };
                    u.min(v) - radius <= max[a] && u.max(v) + radius >= min[a]
                })
            })
            .map(|r| WaterReach {
                key: ReachKey {
                    source_version: self.source_version(),
                    owner: r.owner,
                    segment: r.segment,
                },
                upstream: ReachPoint {
                    x_m: r.a.x,
                    z_m: r.a.z,
                    stage_m: r.a.stage,
                },
                downstream: ReachPoint {
                    x_m: r.b.x,
                    z_m: r.b.z,
                    stage_m: r.b.stage,
                },
                upstream_radius_m: r.width,
                downstream_radius_m: r.width_b,
                lake: r.lake,
            })
            .collect();
        reaches.sort_by_key(|r| r.key);
        reaches.dedup_by_key(|r| r.key);
        Ok(reaches)
    }
}

/// Fixed globally aligned one-metre cube, including negative coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TileKey(pub [i64; 3]);
impl TileKey {
    pub fn containing(c: Cell) -> Self {
        Self([
            c.x.div_euclid(TILE_EDGE),
            c.y.div_euclid(TILE_EDGE),
            c.z.div_euclid(TILE_EDGE),
        ])
    }
    pub fn origin(self) -> Result<Cell, String> {
        let mut a = [0; 3];
        for (i, v) in a.iter_mut().enumerate() {
            *v = self.0[i]
                .checked_mul(TILE_EDGE)
                .filter(|v| v.checked_add(TILE_EDGE).is_some())
                .ok_or("procedural tile coordinate overflow")?;
        }
        Ok(Cell {
            x: a[0],
            y: a[1],
            z: a[2],
        })
    }
    fn cell(self, index: usize) -> Result<Cell, String> {
        let o = self.origin()?;
        Ok(Cell {
            x: o.x + (index % 16) as i64,
            y: o.y + ((index / 16) % 16) as i64,
            z: o.z + (index / 256) as i64,
        })
    }
    fn index(self, c: Cell) -> Option<usize> {
        let o = self.origin().ok()?;
        let d = [
            c.x as i128 - o.x as i128,
            c.y as i128 - o.y as i128,
            c.z as i128 - o.z as i128,
        ];
        if d.iter().any(|&v| !(0..16).contains(&v)) {
            return None;
        }
        Some(d[0] as usize + 16 * (d[1] as usize + 16 * d[2] as usize))
    }
}

/// Exact integer capacity per horizontal native layer. A coarse fractional
/// head is an average over binary cells; it never contains fractional mass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageVolumeTable {
    bottom_y: i64,
    layer_capacity: [u16; 16],
    cumulative: [u64; 17],
}
impl StageVolumeTable {
    fn from_solids(key: TileKey, solids: &[u64; MASK_WORDS]) -> Result<Self, String> {
        let mut layer_capacity = [0u16; 16];
        for i in 0..TILE_CELLS {
            if solids[i / 64] & (1 << (i % 64)) == 0 {
                layer_capacity[(i / 16) % 16] += 1;
            }
        }
        let mut cumulative = [0; 17];
        for y in 0..16 {
            cumulative[y + 1] = cumulative[y] + layer_capacity[y] as u64;
        }
        Ok(Self {
            bottom_y: key.origin()?.y,
            layer_capacity,
            cumulative,
        })
    }
    pub fn capacity(&self) -> u64 {
        self.cumulative[16]
    }
    pub fn layer_capacity(&self) -> &[u16; 16] {
        &self.layer_capacity
    }
    /// Full native water cells below the integer stage plane; no area heuristic.
    pub fn volume_below_stage(&self, stage_y: i64) -> u64 {
        let layer = (stage_y as i128 - self.bottom_y as i128).clamp(0, 16) as usize;
        self.cumulative[layer]
    }
    /// Tile-local stage estimate. Fully flooded tiles require a separate exterior
    /// head: their count alone cannot identify a water surface above the tile.
    pub fn local_head_m(&self, units: u64) -> Result<f64, String> {
        if units > self.capacity() {
            return Err("tile water volume exceeds geometry capacity".into());
        }
        if units == 0 {
            return Ok(self.bottom_y as f64 * voxel_size());
        }
        for y in 0..16 {
            if self.layer_capacity[y] > 0 && units <= self.cumulative[y + 1] {
                return Ok((self.bottom_y as f64
                    + y as f64
                    + (units - self.cumulative[y]) as f64 / self.layer_capacity[y] as f64)
                    * voxel_size());
            }
        }
        Ok((self.bottom_y as f64 + 16.) * voxel_size())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TileGeometry {
    pub key: TileKey,
    pub stage_volume: StageVolumeTable,
    /// Original/source column surface where present; an exterior boundary datum,
    /// not a new infinite reservoir and not inferred from this tile's count.
    pub source_head_m: Option<f64>,
    pub reaches: Vec<ReachKey>,
    solids: [u64; MASK_WORDS],
    initial_water: [u64; MASK_WORDS],
}
impl TileGeometry {
    pub fn sample(world: &World, key: TileKey) -> Result<Self, String> {
        let o = key.origin()?;
        let mut solids = [0; MASK_WORDS];
        let mut water = [0; MASK_WORDS];
        let mut source_head: Option<f64> = None;
        // Reuse each canonical column for all sixteen vertical samples, while
        // retaining edits, vegetation and live dry/wet overrides.
        for z in 0..16 {
            for x in 0..16 {
                let column = world.column(o.x + x, o.z + z);
                if let Some(c) = column.terrain_column {
                    if c.visible_top > c.ground_top {
                        source_head = Some(
                            source_head.map_or(c.visible_top as f64 * voxel_size(), |old| {
                                old.max(c.visible_top as f64 * voxel_size())
                            }),
                        );
                    }
                }
                for y in 0..16 {
                    let i = x as usize + 16 * (y as usize + 16 * z as usize);
                    let material = column.canonical_material(y + o.y);
                    if fluid_solid(material) {
                        solids[i / 64] |= 1 << (i % 64);
                    } else if material == Material::Water {
                        water[i / 64] |= 1 << (i % 64);
                    }
                }
            }
        }
        let terrain = world
            .fbm_terrain()
            .ok_or("procedural water requires FBM source")?;
        let reaches = terrain
            .water_reaches(
                [o.x as f64 * voxel_size(), o.z as f64 * voxel_size()],
                [
                    (o.x + 16) as f64 * voxel_size(),
                    (o.z + 16) as f64 * voxel_size(),
                ],
            )?
            .into_iter()
            .map(|r| r.key)
            .collect();
        Ok(Self {
            key,
            stage_volume: StageVolumeTable::from_solids(key, &solids)?,
            source_head_m: source_head,
            reaches,
            solids,
            initial_water: water,
        })
    }
    pub fn initial_units(&self) -> u64 {
        self.initial_water
            .iter()
            .map(|w| w.count_ones() as u64)
            .sum()
    }
    pub fn solid_at(&self, c: Cell) -> Option<bool> {
        let i = self.key.index(c)?;
        Some(self.solids[i / 64] & (1 << (i % 64)) != 0)
    }
    pub fn initially_wet(&self, c: Cell) -> Option<bool> {
        let i = self.key.index(c)?;
        Some(self.initial_water[i / 64] & (1 << (i % 64)) != 0)
    }
    /// Deterministic binary coarse display/query state for a changed finite
    /// volume. Original occupancy is retained until volume changes; removals
    /// take highest cells and additions fill lowest available cells. This is
    /// a volume placement policy, not detailed momentum evolution.
    pub fn occupied_cells(&self, units: u64) -> Result<Vec<Cell>, String> {
        if units > self.stage_volume.capacity() {
            return Err("coarse tile volume exceeds capacity".into());
        }
        let mut wet = Vec::new();
        let mut empty = Vec::new();
        for i in 0..TILE_CELLS {
            if self.solids[i / 64] & (1 << (i % 64)) != 0 {
                continue;
            }
            let c = self.key.cell(i)?;
            if self.initial_water[i / 64] & (1 << (i % 64)) != 0 {
                wet.push(c)
            } else {
                empty.push(c)
            }
        }
        wet.sort_by_key(|c| (c.y, c.z, c.x));
        if (units as usize) < wet.len() {
            wet.truncate(units as usize)
        } else {
            empty.sort_by_key(|c| (c.y, c.z, c.x));
            wet.extend(empty.into_iter().take(units as usize - wet.len()));
        }
        Ok(wet)
    }
    pub fn initial_head_m(&self) -> Result<f64, String> {
        let units = self.initial_units();
        let local = self.stage_volume.local_head_m(units)?;
        // A completely flooded cube may be metres below the actual free surface.
        Ok(if units > 0 && units == self.stage_volume.capacity() {
            self.source_head_m.map_or(local, |h| h.max(local))
        } else {
            local
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegisteredTile {
    pub reservoir: ReservoirId,
    /// Native occupancy owns spatial capacity after materialization. The old
    /// stage table remains source metadata and cannot refill this tile.
    pub native: bool,
    pub geometry: TileGeometry,
}
/// One-time allocation registry. Existing tiles never resample the source on
/// revisit, so activation cannot regenerate drained water or create supply.
#[derive(Clone, Debug, PartialEq)]
pub struct ProceduralWater {
    identity: [u64; 8],
    tiles: BTreeMap<TileKey, RegisteredTile>,
    regional: RegionalWater,
    next_reservoir: u32,
    max_tiles: usize,
}
impl ProceduralWater {
    fn identity(world: &World) -> Result<[u64; 8], String> {
        let t = world.fbm_terrain().ok_or("procedural water requires FBM")?;
        let p = t.params();
        if p.recipe != FbmRecipe::SeededWorld {
            return Err("procedural water requires SeededWorld".into());
        }
        Ok([
            world.seed(),
            t.source_version() as u64,
            p.recipe as u64,
            p.horizontal_scale.to_bits(),
            p.amplitude.to_bits(),
            p.octaves as u64,
            p.roughness.to_bits(),
            world.vegetation_enabled() as u64,
        ])
    }
    pub fn new(world: &World, max_tiles: usize) -> Result<Self, String> {
        if !(1..=MAX_TILES).contains(&max_tiles) {
            return Err("procedural water tile budget outside 1..4096".into());
        }
        Ok(Self {
            identity: Self::identity(world)?,
            tiles: BTreeMap::new(),
            regional: RegionalWater::new(),
            next_reservoir: 1,
            max_tiles,
        })
    }
    pub fn tile(&self, key: TileKey) -> Option<&RegisteredTile> {
        self.tiles.get(&key)
    }
    pub fn tiles(&self) -> impl Iterator<Item = (&TileKey, &RegisteredTile)> {
        self.tiles.iter()
    }
    pub fn regional(&self) -> &RegionalWater {
        &self.regional
    }
    /// Routing and source rates must be explicitly supplied by the caller.
    pub fn regional_mut(&mut self) -> &mut RegionalWater {
        &mut self.regional
    }
    pub fn register(&mut self, world: &World, key: TileKey) -> Result<&RegisteredTile, String> {
        if Self::identity(world)? != self.identity {
            return Err("procedural water source identity mismatch".into());
        }
        if !self.tiles.contains_key(&key) {
            if self.tiles.len() >= self.max_tiles {
                return Err("procedural water persistent tile budget exhausted".into());
            }
            let geometry = TileGeometry::sample(world, key)?;
            let reservoir = self.next_reservoir;
            let next = reservoir
                .checked_add(1)
                .ok_or("procedural reservoir ID overflow")?;
            let head = geometry.initial_head_m()? * 1000.;
            if head < i64::MIN as f64 || head >= -(i64::MIN as f64) {
                return Err("procedural head outside millimetre range".into());
            }
            self.regional.add_reservoir(
                reservoir,
                geometry.stage_volume.capacity(),
                geometry.initial_units(),
                head.round() as i64,
            )?;
            self.tiles.insert(
                key,
                RegisteredTile {
                    reservoir,
                    native: false,
                    geometry,
                },
            );
            self.next_reservoir = next;
        }
        Ok(&self.tiles[&key])
    }
    pub fn mark_native(&mut self, key: TileKey) -> Result<(), String> {
        let tile = self.tiles.get(&key).ok_or("unregistered native tile")?;
        if tile.native {
            return Ok(());
        }
        self.regional.set_capacity(tile.reservoir, 4096)?;
        self.tiles.get_mut(&key).unwrap().native = true;
        Ok(())
    }
    pub fn snapshot(&self) -> Vec<u8> {
        let mut b = b"DWPGEN02".to_vec();
        for v in self.identity {
            put(&mut b, v)
        }
        put(&mut b, self.max_tiles as u64);
        put(&mut b, self.next_reservoir as u64);
        put(&mut b, self.tiles.len() as u64);
        for (key, tile) in &self.tiles {
            for v in key.0 {
                put(&mut b, v as u64)
            }
            put(&mut b, tile.reservoir as u64);
            put(&mut b, u64::from(tile.native));
            put(
                &mut b,
                tile.geometry.source_head_m.map_or(u64::MAX, f64::to_bits),
            );
            for &v in tile
                .geometry
                .solids
                .iter()
                .chain(&tile.geometry.initial_water)
            {
                put(&mut b, v)
            }
            put(&mut b, tile.geometry.reaches.len() as u64);
            for key in &tile.geometry.reaches {
                for v in [
                    key.source_version as u64,
                    key.owner[0] as u64,
                    key.owner[1] as u64,
                    key.segment as u64,
                ] {
                    put(&mut b, v)
                }
            }
        }
        let regional = self.regional.snapshot();
        put(&mut b, regional.len() as u64);
        b.extend(regional);
        let checksum = checksum(&b);
        put(&mut b, checksum);
        b
    }
    pub fn restore(world: &World, b: &[u8]) -> Result<Self, String> {
        let legacy = b.get(..8) == Some(b"DWPGEN01");
        if b.len() < 112 || (!legacy && b.get(..8) != Some(b"DWPGEN02")) {
            return Err("invalid/truncated procedural water snapshot".into());
        }
        let end = b.len() - 8;
        if checksum(&b[..end]) != u64::from_le_bytes(b[end..].try_into().unwrap()) {
            return Err("procedural water checksum mismatch".into());
        }
        let mut r = Reader { b: &b[..end], p: 8 };
        let mut identity = [0; 8];
        for v in &mut identity {
            *v = r.get()?;
        }
        if identity != Self::identity(world)? {
            return Err("procedural water source identity mismatch".into());
        }
        let max_tiles = usize::try_from(r.get()?).map_err(|_| "invalid tile budget")?;
        let next_reservoir = u32::try_from(r.get()?).map_err(|_| "invalid next reservoir")?;
        let n = r.get()?;
        if !(1..=MAX_TILES).contains(&max_tiles) || n > max_tiles as u64 || next_reservoir == 0 {
            return Err("invalid procedural tile count".into());
        }
        let mut tiles = BTreeMap::new();
        let mut ids = BTreeSet::new();
        for _ in 0..n {
            let key = TileKey([r.get()? as i64, r.get()? as i64, r.get()? as i64]);
            key.origin()?;
            let reservoir = u32::try_from(r.get()?).map_err(|_| "invalid reservoir ID")?;
            if reservoir == 0 || reservoir >= next_reservoir || !ids.insert(reservoir) {
                return Err("duplicate/invalid reservoir mapping".into());
            }
            let native = if legacy {
                false
            } else {
                match r.get()? {
                    0 => false,
                    1 => true,
                    _ => return Err("invalid native tile role".into()),
                }
            };
            let head = r.get()?;
            let source_head_m = if head == u64::MAX {
                None
            } else {
                let h = f64::from_bits(head);
                if !h.is_finite() {
                    return Err("invalid source head".into());
                }
                Some(h)
            };
            let mut solids = [0; MASK_WORDS];
            let mut water = [0; MASK_WORDS];
            for v in solids.iter_mut().chain(&mut water) {
                *v = r.get()?;
            }
            if solids.iter().zip(water).any(|(&s, w)| s & w != 0) {
                return Err("initial water overlaps saved solids".into());
            }
            let reach_count = r.get()?;
            if reach_count > 4096 {
                return Err("excessive tile reach count".into());
            }
            let mut reaches = Vec::new();
            for _ in 0..reach_count {
                let source_version =
                    u32::try_from(r.get()?).map_err(|_| "invalid reach version")?;
                let owner = [r.get()? as i64, r.get()? as i64];
                let segment = u8::try_from(r.get()?).map_err(|_| "invalid reach segment")?;
                if source_version as u64 != identity[1] || segment > 15 {
                    return Err("reach identity mismatch".into());
                }
                reaches.push(ReachKey {
                    source_version,
                    owner,
                    segment,
                });
            }
            if reaches.windows(2).any(|v| v[0] >= v[1]) {
                return Err("duplicate/unsorted reaches".into());
            }
            let geometry = TileGeometry {
                key,
                stage_volume: StageVolumeTable::from_solids(key, &solids)?,
                source_head_m,
                reaches,
                solids,
                initial_water: water,
            };
            if tiles
                .insert(
                    key,
                    RegisteredTile {
                        reservoir,
                        native,
                        geometry,
                    },
                )
                .is_some()
            {
                return Err("duplicate procedural tile".into());
            }
        }
        let length = usize::try_from(r.get()?).map_err(|_| "invalid regional length")?;
        let stop =
            r.p.checked_add(length)
                .filter(|&v| v == r.b.len())
                .ok_or("invalid procedural snapshot trailing size")?;
        let regional = RegionalWater::restore(&r.b[r.p..stop])?;
        for tile in tiles.values() {
            let reservoir = regional
                .reservoir(tile.reservoir)
                .ok_or("tile reservoir missing")?;
            let expected = if tile.native {
                4096
            } else {
                tile.geometry.stage_volume.capacity()
            };
            if reservoir.capacity != expected {
                return Err("tile capacity/geometry mismatch".into());
            }
        }
        Ok(Self {
            identity,
            tiles,
            regional,
            next_reservoir,
            max_tiles,
        })
    }
}
fn fluid_solid(material: Material) -> bool {
    !matches!(material, Material::Air | Material::Water)
}
fn put(b: &mut Vec<u8>, v: u64) {
    b.extend(v.to_le_bytes())
}
fn checksum(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf29ce484222325, |h, &v| {
        (h ^ v as u64).wrapping_mul(0x100000001b3)
    })
}
struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}
impl Reader<'_> {
    fn get(&mut self) -> Result<u64, String> {
        let end = self.p.checked_add(8).ok_or("snapshot offset overflow")?;
        let bytes = self
            .b
            .get(self.p..end)
            .ok_or("truncated procedural water snapshot")?;
        self.p = end;
        Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FbmParams;
    fn world() -> World {
        World::fbm(
            19,
            FbmParams {
                recipe: FbmRecipe::SeededWorld,
                ..Default::default()
            },
        )
        .unwrap()
    }
    fn source_reach(world: &World, lake: bool) -> WaterReach {
        let t = world.fbm_terrain().unwrap();
        for ix in -4..=4 {
            for iz in -4..=4 {
                let x = ix as f64 * 9600.;
                let z = iz as f64 * 9600.;
                let candidates = t.water_reaches([x, z], [x + 9600., z + 9600.]).unwrap();
                if let Some(r) = candidates
                    .into_iter()
                    .find(|r| r.lake == lake && r.upstream.stage_m > 0.)
                {
                    return r;
                }
            }
        }
        panic!("expected generated water fixture")
    }
    fn surface_key(world: &World, r: WaterReach) -> TileKey {
        let x = ((r.upstream.x_m + r.downstream.x_m) * 0.5 / voxel_size()).floor() as i64;
        let z = ((r.upstream.z_m + r.downstream.z_m) * 0.5 / voxel_size()).floor() as i64;
        let c = world.fbm_terrain().unwrap().column_cells(x, z);
        assert!(c.visible_top > c.ground_top);
        TileKey::containing(Cell {
            x,
            y: c.visible_top - 1,
            z,
        })
    }
    #[test]
    fn native_tile_addresses_are_global_and_overflow_checked() {
        let key = TileKey::containing(Cell {
            x: -1,
            y: -16,
            z: -17,
        });
        assert_eq!(key, TileKey([-1, -1, -2]));
        assert_eq!(
            key.origin().unwrap(),
            Cell {
                x: -16,
                y: -16,
                z: -32
            }
        );
        assert!(TileKey([i64::MAX, 0, 0]).origin().is_err());
        assert_eq!(
            key.cell(4095).unwrap(),
            Cell {
                x: -1,
                y: -1,
                z: -17
            }
        );
        assert_eq!(key.index(key.cell(4095).unwrap()), Some(4095));
    }
    #[test]
    fn stage_volume_counts_exact_voxels_including_overhangs_and_dry_tombstones() {
        let mut world = world();
        let key = TileKey([0, 1000, 0]);
        let o = key.origin().unwrap();
        for i in 0..TILE_CELLS {
            let c = key.cell(i).unwrap();
            let y = c.y - o.y;
            let material = if y < c.x.rem_euclid(4) || (y == 12 && c.z == o.z) {
                Material::Stone
            } else if y < 4 {
                Material::Water
            } else {
                Material::Air
            };
            world.set(c, material);
        }
        // Foliage is nonblocking for the player, but canonical fluid cannot
        // replace its material. It must be excluded from water capacity.
        world.set(
            Cell {
                x: o.x,
                y: o.y + 10,
                z: o.z + 2,
            },
            Material::Leaf,
        );
        world.set(
            Cell {
                x: o.x + 1,
                y: o.y + 10,
                z: o.z + 2,
            },
            Material::Flower,
        );
        let dry = Cell {
            x: o.x + 4,
            y: o.y + 3,
            z: o.z + 4,
        };
        world.set_fluid_occupancy([(dry, false)]);
        let geometry = TileGeometry::sample(&world, key).unwrap();
        let mut capacity = 0;
        let mut wet = 0;
        for i in 0..TILE_CELLS {
            let c = key.cell(i).unwrap();
            let m = world.material(c);
            capacity += u64::from(!fluid_solid(m));
            wet += u64::from(m == Material::Water);
            assert_eq!(geometry.solid_at(c), Some(fluid_solid(m)));
            assert_eq!(geometry.initially_wet(c), Some(m == Material::Water));
        }
        assert_eq!(geometry.stage_volume.capacity(), capacity);
        assert_eq!(geometry.initial_units(), wet);
        assert_eq!(geometry.initially_wet(dry), Some(false));
        for y in 0..=16 {
            let expected = (0..TILE_CELLS)
                .filter(|&i| {
                    let c = key.cell(i).unwrap();
                    c.y < o.y + y && !fluid_solid(world.material(c))
                })
                .count() as u64;
            assert_eq!(geometry.stage_volume.volume_below_stage(o.y + y), expected);
        }
        assert!(geometry.stage_volume.local_head_m(capacity + 1).is_err());
    }
    #[test]
    fn generated_lake_tiles_share_the_source_stage_without_whole_lake_allocation() {
        let world = world();
        let reach = source_reach(&world, true);
        let a = surface_key(&world, reach);
        let b = TileKey([a.0[0] + 1, a.0[1], a.0[2]]);
        let ga = TileGeometry::sample(&world, a).unwrap();
        let gb = TileGeometry::sample(&world, b).unwrap();
        assert!(ga.initial_units() > 0 && gb.initial_units() > 0);
        assert_eq!(ga.initial_head_m().unwrap(), gb.initial_head_m().unwrap());
        assert!(ga.reaches.contains(&reach.key));
        assert!(gb.reaches.contains(&reach.key));
        assert_eq!(
            ga.stage_volume
                .volume_below_stage((ga.initial_head_m().unwrap() / voxel_size()).round() as i64),
            ga.initial_units()
        );
        assert!(ga.stage_volume.capacity() <= 4096 && gb.stage_volume.capacity() <= 4096);
    }
    #[test]
    fn public_reach_keys_and_geometry_are_stable_across_query_order() {
        let world = world();
        let reach = source_reach(&world, false);
        let t = world.fbm_terrain().unwrap();
        let x = (reach.upstream.x_m + reach.downstream.x_m) * 0.5;
        let z = (reach.upstream.z_m + reach.downstream.z_m) * 0.5;
        let small = t.water_reaches([x - 1., z - 1.], [x + 1., z + 1.]).unwrap();
        let large = t
            .water_reaches([x - 32., z - 32.], [x + 32., z + 32.])
            .unwrap();
        assert!(small.iter().any(|r| r.key == reach.key));
        for r in small {
            assert_eq!(large.iter().find(|v| v.key == r.key), Some(&r));
        }
        assert!(t.water_reaches([0.; 2], [f64::INFINITY; 2]).is_err());
        assert!(t.water_reaches([0.; 2], [1e9; 2]).is_err());
    }
    #[test]
    fn away_back_registration_and_restart_never_regenerate_exported_water_or_supply() {
        let world = world();
        let key = surface_key(&world, source_reach(&world, true));
        let mut p = ProceduralWater::new(&world, 4).unwrap();
        let tile = p.register(&world, key).unwrap();
        let id = tile.reservoir;
        let units = tile.geometry.initial_units();
        assert!(units > 0);
        let other = TileKey([key.0[0] + 2, key.0[1] + 100, key.0[2]]);
        let other_id = p.register(&world, other).unwrap().reservoir;
        let token = p.regional_mut().take_region(17, id, units).unwrap();
        let token = p
            .regional_mut()
            .exchange_owned(1, token, other_id, units, false)
            .unwrap();
        p.regional_mut().return_region(token, 0).unwrap();
        let initial = p.regional().ledger().initial;
        for _ in 0..10 {
            p.register(&world, key).unwrap();
            p.regional_mut().advance(100_000_000).unwrap();
        }
        assert_eq!(p.regional().reservoir(id).unwrap().volume, 0);
        assert_eq!(p.regional().ledger().initial, initial);
        assert_eq!(p.regional().ledger().supplied, 0);
        assert!(p.regional().conserved());
        let saved = p.snapshot();
        let mut restored = ProceduralWater::restore(&world, &saved).unwrap();
        assert_eq!(p, restored);
        restored.register(&world, key).unwrap();
        assert_eq!(restored.regional().reservoir(id).unwrap().volume, 0);
        assert_eq!(restored.snapshot(), saved);
        let mut wrong = world.clone();
        wrong.seed = 20;
        assert!(ProceduralWater::restore(&wrong, &saved).is_err());
        let mut corrupt = saved;
        corrupt[100] ^= 1;
        assert!(ProceduralWater::restore(&world, &corrupt).is_err());
    }
    #[test]
    fn explicit_sources_and_tile_budget_are_bounded_and_persisted() {
        let world = world();
        let key = TileKey([0, 1000, 0]);
        let mut p = ProceduralWater::new(&world, 1).unwrap();
        let id = p.register(&world, key).unwrap().reservoir;
        p.regional_mut().set_source(id, Some(7)).unwrap();
        p.regional_mut().advance(500_000_000).unwrap();
        assert_eq!(p.regional().ledger().supplied, 3);
        assert!(p.register(&world, TileKey([1, 1000, 0])).is_err());
        let mut other = ProceduralWater::restore(&world, &p.snapshot()).unwrap();
        p.regional_mut().advance(500_000_000).unwrap();
        other.regional_mut().advance(500_000_000).unwrap();
        assert_eq!(p.snapshot(), other.snapshot());
        assert_eq!(p.regional().ledger().supplied, 7);
    }
}
