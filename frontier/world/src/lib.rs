//! Greenfield canonical native-cell world. A bounded integer fixture and a
//! CPU-f64 procedural heightfield share material, edit and reference-ray semantics.
//! These are test sources, not the production geology or persistence system.
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

pub mod affine_bounds;
mod alpine;
pub mod discrete_water;
pub mod edit;
mod edit_index;
mod edit_pages;
pub use edit_index::EditQueryStats;
pub mod fluid;
mod grid;
pub use grid::{
    configure_voxel_grid, voxel_grid, voxel_grid_args, voxel_size, InvalidVoxelSize, VoxelGrid,
    VOXEL_SIZE,
};
pub mod moving_water;
pub mod physical_fluid;
pub mod procedural_water;
#[cfg(test)]
mod seeded_bounds_tests;
pub mod summary;
pub mod terrain;
pub mod vegetation;
pub mod visual_source;
pub mod volume_summary;
pub use terrain::{
    FbmError, FbmParams, FbmRecipe, FbmTerrain, ALPINE_APPEARANCE_VERSION,
    ALPINE_FBM_SOURCE_VERSION, FBM_SOURCE_VERSION, REGIONAL_APPEARANCE_VERSION,
    REGIONAL_FBM_SOURCE_VERSION,
};

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
        [self.x, self.y, self.z].map(|v| (v as f64 + 0.5) * voxel_size())
    }
}
fn checked_cell(p: [f64; 3]) -> Option<Cell> {
    voxel_grid().checked_cell(p)
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
        delta.map(|d| ((d as f64 + 0.5) * voxel_size()) as f32)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Material {
    Air,
    Grass,
    Soil,
    Stone,
    Wood,
    Leaf,
    Flower,
    Water,
    Sand,
    Snow,
}
impl Material {
    pub fn is_blocking(self) -> bool {
        !matches!(self, Self::Air | Self::Leaf | Self::Flower | Self::Water)
    }
    pub fn color(self) -> [f32; 3] {
        match self {
            Self::Air => [0.0, 0.0, 0.0],
            Self::Grass => [0.18, 0.34, 0.10],
            Self::Soil => [0.32, 0.22, 0.13],
            Self::Stone => [0.35, 0.37, 0.39],
            Self::Wood => [0.41, 0.25, 0.13],
            Self::Leaf => [0.40, 0.53, 0.25],
            Self::Flower => [0.93, 0.92, 0.84],
            Self::Water => [0.035, 0.19, 0.29],
            Self::Sand => [0.62, 0.52, 0.32],
            Self::Snow => [0.88, 0.92, 0.96],
        }
    }
}

// Disposable lineage metadata: snapshots retain only a token, never a chain of
// parent snapshots. Eviction affects query cost, never authoritative edits.
const CHANGE_JOURNAL_CELLS: usize = 4096;
const CHANGE_JOURNAL_RECORDS: usize = 256;
#[derive(Debug, Default)]
struct ChangeJournal {
    next: u64,
    cells: usize,
    records: BTreeMap<u64, (u64, Vec<Cell>)>,
}
impl ChangeJournal {
    fn record(&mut self, parent: u64, cells: Vec<Cell>) -> u64 {
        self.next = self.next.checked_add(1).expect("change token exhausted");
        let token = self.next;
        if cells.len() <= CHANGE_JOURNAL_CELLS {
            while self.records.len() >= CHANGE_JOURNAL_RECORDS
                || self.cells + cells.len() > CHANGE_JOURNAL_CELLS
            {
                let (_, (_, old)) = self.records.pop_first().unwrap();
                self.cells -= old.len();
            }
            self.cells += cells.len();
            self.records.insert(token, (parent, cells));
        }
        token
    }
    fn candidates(&self, mut a: u64, mut b: u64) -> Option<Vec<Cell>> {
        let mut cells = Vec::new();
        // Tokens strictly increase across every branch, so walking the newer
        // side reaches the common ancestor without scanning unrelated records.
        while a != b {
            let newer = if a > b { &mut a } else { &mut b };
            let (parent, delta) = self.records.get(newer)?;
            cells.extend_from_slice(delta);
            *newer = *parent;
        }
        cells.sort_unstable();
        cells.dedup();
        Some(cells)
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
    vegetation: Option<Arc<vegetation::TileCache>>,
    topology_base: Option<i64>,
    edits: Arc<edit_pages::EditPages>,
    fluid: Arc<BTreeMap<Cell, bool>>,
    revision: u64,
    change_token: u64,
    highest_touched_y: Option<i64>,
    changes: Arc<Mutex<ChangeJournal>>,
    edit_index: Arc<Mutex<edit_index::EditIndex>>,
    counter: Arc<AtomicU64>,
}

/// One reversible terrain operation. The page snapshots share untouched leaves.
pub struct TerrainUndo {
    before: Arc<edit_pages::EditPages>,
    after: Arc<edit_pages::EditPages>,
    cells: Vec<Cell>,
}
impl TerrainUndo {
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }
}

/// Terrain-only column snapshot borrowing its world; excludes vegetation.
/// Reuses the canonical source
/// height for many native y queries; material overrides and fixture volumes still
/// apply. Useful for bounded native reference extraction, not a display LOD.
pub struct WorldColumn<'a> {
    world: &'a World,
    x: i64,
    z: i64,
    height: i64,
    terrain_column: Option<terrain::TerrainColumn>,
}
impl WorldColumn<'_> {
    pub fn height_cells(&self) -> i64 {
        self.height
    }
    pub fn ground_height_cells(&self) -> i64 {
        self.terrain_column
            .map_or(self.height, |column| column.ground_top)
    }
    pub fn height_m(&self) -> f64 {
        self.height_cells() as f64 * voxel_size()
    }
    pub fn ground_height_m(&self) -> f64 {
        self.ground_height_cells() as f64 * voxel_size()
    }
    /// Edited terrain before live fluid; excludes separately owned vegetation.
    pub fn material_without_fluid(&self, y: i64) -> Material {
        let c = Cell {
            x: self.x,
            y,
            z: self.z,
        };
        self.world.edits.get(&c).copied().unwrap_or_else(|| {
            self.world
                .source_with_column(c, self.height, self.terrain_column)
        })
    }
    pub fn material(&self, y: i64) -> Material {
        let c = Cell {
            x: self.x,
            y,
            z: self.z,
        };
        self.world.compose_fluid(c, self.material_without_fluid(y))
    }
    /// Canonical edited terrain plus vegetation before fluid composition.
    pub fn canonical_material_without_fluid(&self, y: i64) -> Material {
        let c = Cell {
            x: self.x,
            y,
            z: self.z,
        };
        self.world.material_over_source(
            c,
            || {
                self.world
                    .source_with_column(c, self.height, self.terrain_column)
            },
            || self.world.vegetation_voxel(c),
        )
    }
    fn canonical_material(&self, y: i64) -> Material {
        self.world.compose_fluid(
            Cell {
                x: self.x,
                y,
                z: self.z,
            },
            self.canonical_material_without_fluid(y),
        )
    }
}
impl World {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            terrain: None,
            vegetation: None,
            topology_base: None,
            edits: Arc::default(),
            fluid: Arc::default(),
            revision: 0,
            change_token: 0,
            highest_touched_y: None,
            changes: Arc::default(),
            edit_index: Arc::default(),
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
    /// Bounded ownership smoke fixture on the same FBM landscape. The source
    /// height remains the unedited page generator; this explicit volume owns
    /// x/z=[-8,8)m at every display distance.
    pub fn fbm_topology(seed: u64, params: FbmParams) -> Result<Self, FbmError> {
        let mut world = Self::fbm(seed, params)?;
        world.topology_base = Some(world.height_cells(-48, 0));
        Ok(world)
    }
    pub fn topology_base_cells(&self) -> Option<i64> {
        self.topology_base
    }
    pub fn volume_owned(&self, c: Cell) -> bool {
        self.topology_base.is_some() && (-128..128).contains(&c.x) && (-128..128).contains(&c.z)
    }
    /// Effective edit bounds are exposed for bounded exact extraction, never for LOD.
    pub fn edit_cells(&self) -> impl Iterator<Item = Cell> + '_ {
        self.edits.keys().chain(self.fluid.keys()).copied()
    }
    /// Exact sorted unique override addresses inside half-open native bounds.
    /// The shared lineage index is disposable; candidates are checked against
    /// this snapshot's canonical maps, and capacity exhaustion uses a full scan.
    pub fn edit_cells_in_bounds(&self, min: Cell, max: Cell) -> Vec<Cell> {
        self.edit_cells_in_bounds_with_stats(min, max).0
    }
    pub fn edit_cells_in_bounds_with_stats(
        &self,
        min: Cell,
        max: Cell,
    ) -> (Vec<Cell>, EditQueryStats) {
        self.indexed_edit_cells(
            [min.x, min.y, min.z].map(i128::from),
            [max.x, max.y, max.z].map(i128::from),
        )
    }
    /// Metre-column consumers need every height, including the i64::MAX cell.
    /// X/Z bounds remain half open; no unrepresentable MAX+1 native Cell is needed.
    pub fn edit_cells_in_columns(&self, min: [i64; 2], max: [i64; 2]) -> Vec<Cell> {
        self.indexed_edit_cells(
            [min[0] as i128, i64::MIN as i128, min[1] as i128],
            [max[0] as i128, i64::MAX as i128 + 1, max[1] as i128],
        )
        .0
    }
    /// Authored addresses at all elevations inside a half-open X/Z rectangle.
    /// Returns None once the matching terrain exceeds the caller's cell limit.
    pub fn terrain_edit_cells_in_columns(
        &self,
        min: [i64; 2],
        max: [i64; 2],
        limit: usize,
    ) -> Option<Vec<Cell>> {
        self.edits.cells_in_columns(min, max, limit)
    }
    /// Includes both ends of every contiguous authored material run in the
    /// half-open footprint. Small selections can also contain interior cells.
    pub fn terrain_edit_surface_candidates(
        &self,
        min: [i64; 2],
        max: [i64; 2],
        limit: usize,
    ) -> Option<Vec<Cell>> {
        self.edits
            .cells_in_columns(min, max, limit)
            .or_else(|| self.edits.run_ends_in_columns(min, max, limit))
    }
    /// Conservative inclusive upper Y of terrain or fluid overrides whose X/Z
    /// may intersect this half-open box. Stale lineage buckets can enlarge a
    /// bound; a missing bound proves the region has no overrides.
    pub fn override_y_ceiling(&self, min: [i64; 2], max: [i64; 2]) -> Option<i64> {
        let own = self.highest_touched_y?;
        if self.edits.is_empty() && self.fluid.is_empty() {
            return None;
        }
        self.edit_index.lock().unwrap().upper_y(min, max, own)
    }

    fn indexed_edit_cells(&self, min: [i128; 3], max: [i128; 3]) -> (Vec<Cell>, EditQueryStats) {
        let (candidates, mut stats) = self.edit_index.lock().unwrap().query(min, max, |c| {
            self.edits.contains_key(&c) || self.fluid.contains_key(&c)
        });
        let mut cells = if let Some(candidates) = candidates {
            candidates
        } else {
            self.edit_cells()
                .filter(|c| {
                    stats.full_scan_cells += 1;
                    [c.x, c.y, c.z]
                        .into_iter()
                        .enumerate()
                        .all(|(a, v)| v as i128 >= min[a] && (v as i128) < max[a])
                })
                .collect::<Vec<_>>()
        };
        cells.sort_unstable();
        cells.dedup();
        (cells, stats)
    }
    /// True only for snapshots of one immutable source lineage. Reconstructed
    /// worlds conservatively require source-wide invalidation, even if their
    /// edit revision numbers happen to match. Vegetation toggles/replacements
    /// are source changes; material edits preserve these identities.
    pub fn shares_source_lineage(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.counter, &other.counter)
            && match (&self.vegetation, &other.vegetation) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
    /// Exact override-map difference, including material changes and overrides
    /// removed by returning to an earlier immutable snapshot. Camera-independent.
    /// Recent lineage changes inspect only touched cells (including branches and
    /// reverts). Evicted/oversized deltas and replaced sources fall back to the
    /// exact override scan; journal storage is bounded independently of history.
    pub fn changed_edit_cells(&self, other: &Self) -> Vec<Cell> {
        if Arc::ptr_eq(&self.edits, &other.edits) && Arc::ptr_eq(&self.fluid, &other.fluid) {
            return Vec::new();
        }
        let mut left = BTreeMap::new();
        let mut right = BTreeMap::new();
        let candidates = if self.shares_source_lineage(other) {
            self.changes
                .lock()
                .unwrap()
                .candidates(self.change_token, other.change_token)
        } else {
            None
        };
        candidates
            .unwrap_or_else(|| {
                self.edit_cells()
                    .chain(other.edit_cells())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect()
            })
            .into_iter()
            .filter(|&c| {
                if self.edits.get(&c) == other.edits.get(&c)
                    && self.fluid.get(&c) == other.fluid.get(&c)
                {
                    return false;
                }
                left.entry((c.x, c.z))
                    .or_insert_with(|| self.column(c.x, c.z))
                    .canonical_material(c.y)
                    != right
                        .entry((c.x, c.z))
                        .or_insert_with(|| other.column(c.x, c.z))
                        .canonical_material(c.y)
            })
            .collect()
    }

    /// Authored material at an address, independent of simulation and source.
    pub fn terrain_override(&self, cell: Cell) -> Option<Material> {
        self.edits.get(&cell).copied()
    }
    pub fn shares_terrain_edits(&self, other: &Self) -> bool {
        self.shares_source_lineage(other) && Arc::ptr_eq(&self.edits, &other.edits)
    }
    /// Terrain edits only, for an atomic terrain/fluid save transaction.
    pub fn terrain_edits(&self) -> impl Iterator<Item = (Cell, Material)> + '_ {
        self.edits.iter().map(|(&c, &m)| (c, m))
    }
    /// Restore the complete overlay, including a placed cell whose material
    /// matches the generator. Such cells still participate in edit ownership.
    pub fn restore_terrain_edits(&mut self, values: impl IntoIterator<Item = (Cell, Material)>) {
        let edits: BTreeMap<_, _> = values.into_iter().collect();
        if self.edits.iter().eq(edits.iter()) {
            return;
        }
        let changed = self
            .edits
            .keys()
            .filter(|c| !edits.contains_key(c))
            .chain(
                edits
                    .iter()
                    .filter(|(c, m)| self.edits.get(c) != Some(m))
                    .map(|(c, _)| c),
            )
            .copied()
            .collect();
        self.record_changes(changed);
        self.edits = Arc::new(edit_pages::EditPages::from_sorted(edits));
        self.bump_revision();
    }
    /// Physical terrain/vegetation before live water composition. Air edits open
    /// a cell to fluid; solid edits always take precedence over fluid occupancy.
    pub fn material_without_fluid(&self, c: Cell) -> Material {
        self.material_over_source(c, || self.source(c), || self.vegetation_voxel(c))
    }
    fn material_over_source(
        &self,
        c: Cell,
        source: impl FnOnce() -> Material,
        plant: impl FnOnce() -> Option<vegetation::PlantVoxel>,
    ) -> Material {
        self.edits.get(&c).copied().unwrap_or_else(|| {
            let m = source();
            if m == Material::Air && self.vegetation_enabled() {
                plant().map_or(m, |v| v.material)
            } else {
                m
            }
        })
    }
    fn compose_fluid(&self, c: Cell, base: Material) -> Material {
        if base != Material::Air && base != Material::Water {
            return base;
        }
        self.fluid.get(&c).map_or(
            base,
            |&wet| if wet { Material::Water } else { Material::Air },
        )
    }
    /// Immutable static-render input retaining terrain edits and vegetation.
    /// Cache this snapshot until solids change: creating it each fluid tick
    /// would unnecessarily give static extraction a new revision.
    pub fn without_fluid_snapshot(&self) -> Self {
        let mut snapshot = self.clone();
        if !snapshot.fluid.is_empty() {
            let changed = snapshot.fluid.keys().copied().collect();
            snapshot.fluid = Arc::new(BTreeMap::new());
            snapshot.record_changes(changed);
            snapshot.bump_revision();
        }
        snapshot
    }
    /// Static-render snapshot without fluid overrides inside this half-open box.
    /// Wet cells and dry source-water tombstones outside the box remain owned by
    /// this snapshot. Terrain, vegetation and edits are shared unchanged. Empty
    /// or reversed bounds return an unchanged clone with the same revision.
    pub fn without_fluid_in_bounds(&self, min: Cell, max_exclusive: Cell) -> Self {
        let mut snapshot = self.clone();
        if min.x >= max_exclusive.x || min.y >= max_exclusive.y || min.z >= max_exclusive.z {
            return snapshot;
        }
        // Cell order is x/y/z. Restrict the ordered scan to the requested x
        // slab, then select exact y/z bounds without integer boundary arithmetic.
        let slab_min = Cell {
            x: min.x,
            y: i64::MIN,
            z: i64::MIN,
        };
        let slab_max = Cell {
            x: max_exclusive.x,
            y: i64::MIN,
            z: i64::MIN,
        };
        let removed: Vec<_> = self
            .fluid
            .range(slab_min..slab_max)
            .filter_map(|(&c, _)| {
                (c.y >= min.y && c.y < max_exclusive.y && c.z >= min.z && c.z < max_exclusive.z)
                    .then_some(c)
            })
            .collect();
        if removed.is_empty() {
            return snapshot;
        }
        let fluid = Arc::make_mut(&mut snapshot.fluid);
        for c in &removed {
            fluid.remove(c);
        }
        snapshot.record_changes(removed);
        snapshot.bump_revision();
        snapshot
    }
    /// Batch only changed visible occupancies. Fractional mass remains in the
    /// simulator, while each occupied cell renders on the native voxel lattice.
    pub fn set_fluid_occupancy(&mut self, values: impl IntoIterator<Item = (Cell, bool)>) {
        let mut columns = BTreeMap::new();
        let mut updates = Vec::new();
        let mut visible_changed = false;
        for (c, wet) in values {
            if self.fluid.get(&c) == Some(&wet) {
                continue;
            }
            // Explicit edits already determine canonical terrain/vegetation.
            // A live edited basin must not regenerate its procedural column
            // every time one of its water cells moves.
            let base = self.edits.get(&c).copied().unwrap_or_else(|| {
                columns
                    .entry((c.x, c.z))
                    .or_insert_with(|| self.column(c.x, c.z))
                    .canonical_material_without_fluid(c.y)
            });
            if !matches!(base, Material::Air | Material::Water) {
                // A solid edit hides the cell, but its fluid tombstone must
                // replace any old wet override before the solid is removed.
                if self.fluid.contains_key(&c) {
                    updates.push((c, wet));
                }
                continue;
            }
            let before = self.compose_fluid(c, base);
            let after = if wet { Material::Water } else { Material::Air };
            if before == after && !self.fluid.contains_key(&c) {
                continue;
            }
            visible_changed |= before != after;
            updates.push((c, wet));
        }
        drop(columns);
        if updates.is_empty() {
            return;
        }
        self.record_changes(updates.iter().map(|(c, _)| *c).collect());
        let map = Arc::make_mut(&mut self.fluid);
        map.extend(updates);
        if visible_changed {
            self.bump_revision();
        }
    }

    fn record_changes(&mut self, cells: Vec<Cell>) {
        if let Some(y) = cells.iter().map(|c| c.y).max() {
            self.highest_touched_y = Some(self.highest_touched_y.map_or(y, |old| old.max(y)));
        }
        self.edit_index.lock().unwrap().record(&cells);
        self.change_token = self
            .changes
            .lock()
            .unwrap()
            .record(self.change_token, cells);
    }

    fn bump_revision(&mut self) {
        self.revision = self
            .counter
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
            .expect("world revision exhausted")
            + 1;
    }

    /// Immutable unedited surface source for CPU display-page generation.
    /// Edits remain authoritative through `material`; these pages exclude edits.
    pub fn fbm_terrain(&self) -> Option<&FbmTerrain> {
        self.terrain.as_ref()
    }
    /// Terrain and edits only. Vegetation has separate volume ownership.
    pub fn column(&self, x: i64, z: i64) -> WorldColumn<'_> {
        let terrain_column = self.terrain.as_ref().map(|t| t.column_cells(x, z));
        WorldColumn {
            world: self,
            x,
            z,
            height: terrain_column.map_or_else(|| self.height_cells(x, z), |c| c.visible_top),
            terrain_column,
        }
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn seed(&self) -> u64 {
        self.seed
    }
    pub fn set(&mut self, c: Cell, m: Material) {
        self.set_batch([(c, m)]);
    }
    /// Apply last-write-wins terrain edits atomically in one world revision.
    /// Entries whose final material already matches the snapshot are omitted.
    pub fn set_batch(&mut self, values: impl IntoIterator<Item = (Cell, Material)>) {
        let updates = values
            .into_iter()
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .filter(|&(c, m)| self.material(c) != m)
            .collect();
        self.apply_terrain_batch(updates);
    }

    /// Visit the canonical cells of a half-open box, reusing each procedural
    /// column. Rejecting any cell aborts the entire stamp before publication.
    pub fn edit_terrain_box<E>(
        &mut self,
        min: [i64; 3],
        end: [i64; 3],
        mut select: impl FnMut(Cell, Material) -> Result<Option<Material>, E>,
    ) -> Result<Option<TerrainUndo>, E> {
        let mut updates = Vec::new();
        for x in min[0]..end[0] {
            for z in min[2]..end[2] {
                let column = self.column(x, z);
                for y in min[1]..end[1] {
                    let cell = Cell { x, y, z };
                    let old = column.canonical_material(y);
                    if let Some(next) = select(cell, old)? {
                        if old != next {
                            updates.push((cell, next));
                        }
                    }
                }
            }
        }
        if updates.is_empty() {
            return Ok(None);
        }
        // Edit pages are ordered by (x,y,z). Sampling in (x,z,y) order
        // amortizes terrain evaluation over every layer of a tall stamp.
        if end[1].saturating_sub(min[1]) > 1 {
            updates.sort_unstable_by_key(|&(c, _)| c);
        }
        let cells = updates.iter().map(|(cell, _)| *cell).collect();
        let before = self.edits.clone();
        self.apply_terrain_batch(updates);
        Ok(Some(TerrainUndo {
            before,
            after: self.edits.clone(),
            cells,
        }))
    }

    /// A later terrain write invalidates the stamp. Fluid and other world state
    /// keep their current values, and renderers observe a new forward revision.
    pub fn undo_terrain_batch(&mut self, undo: &TerrainUndo) -> bool {
        if !Arc::ptr_eq(&self.edits, &undo.after) {
            return false;
        }
        self.record_changes(undo.cells.clone());
        self.edits = undo.before.clone();
        self.bump_revision();
        true
    }

    fn apply_terrain_batch(&mut self, updates: Vec<(Cell, Material)>) {
        if updates.is_empty() {
            return;
        }
        self.record_changes(updates.iter().map(|(c, _)| *c).collect());
        Arc::make_mut(&mut self.edits).extend(updates);
        self.bump_revision();
    }

    /// Visible terrain/water top in meters, excluding caves, bridge and edits. Quantized
    /// to canonical columns, so consumers cannot mistake a smooth H for voxels.
    pub fn height_m(&self, x: f64, z: f64) -> f64 {
        let c = Cell::from_meters([x, 0.0, z]);
        self.height_cells(c.x, c.z) as f64 * voxel_size()
    }
    /// Physical ground top, excluding static water and edits.
    pub fn ground_height_m(&self, x: f64, z: f64) -> f64 {
        self.terrain
            .as_ref()
            .map_or_else(|| self.height_m(x, z), |t| t.ground_height_m(x, z))
    }
    pub fn material(&self, c: Cell) -> Material {
        self.compose_fluid(c, self.material_without_fluid(c))
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
        let (height, column) = self.source_column(c.x, c.z);
        self.source_with_column(c, height, column)
    }
    fn source_column(&self, x: i64, z: i64) -> (i64, Option<terrain::TerrainColumn>) {
        let column = self
            .terrain
            .as_ref()
            .filter(|t| t.params().recipe.has_static_water())
            .map(|t| t.column_cells(x, z));
        (
            column.map_or_else(|| self.height_cells(x, z), |v| v.visible_top),
            column,
        )
    }
    fn source_with_column(
        &self,
        c: Cell,
        h: i64,
        column: Option<terrain::TerrainColumn>,
    ) -> Material {
        let mut h = h;
        if let Some(base) = self.topology_base.filter(|_| self.volume_owned(c)) {
            // An open excavation reveals a tunnel mouth; a second trench runs
            // under an elevated bridge. Every change stays strictly inside owner.
            if (-80..-16).contains(&c.x) && (-112..-80).contains(&c.z) {
                h = h.min(base - 64);
            }
            if (32..96).contains(&c.x) && (-48..0).contains(&c.z) {
                h = h.min(base - 64);
            }
            if (16..112).contains(&c.x) && (-32..-16).contains(&c.z) {
                if (base + 16..base + 20).contains(&c.y) {
                    return Material::Wood;
                }
                if (base - 64..base + 16).contains(&c.y)
                    && ((16..20).contains(&c.x) || (108..112).contains(&c.x))
                {
                    return Material::Stone;
                }
            }
            let dx = c.x + 48;
            let dy = c.y - (base - 32);
            if (-80..80).contains(&c.z)
                && dx.abs() <= 24
                && dy.abs() <= 24
                && dx * dx + dy * dy < 24 * 24
            {
                return Material::Air;
            }
        }
        if let Some(terrain) = &self.terrain {
            if terrain.params().recipe.has_static_water() {
                return column.expect("FBM source column").material(c.y);
            }
            return if c.y >= h {
                Material::Air
            } else if c.y == h - 1 {
                let [x, _, z] = c.center_m();
                terrain.surface_material(x, z, h as f64 * voxel_size())
            } else if c.y >= h - 12 {
                if terrain.params().recipe == FbmRecipe::Alpine {
                    let [x, _, z] = c.center_m();
                    if terrain.surface_material(x, z, h as f64 * voxel_size()) == Material::Stone {
                        return Material::Stone;
                    }
                }
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
    raycast_matching(world, origin, direction, max_distance, |m| {
        m != Material::Air
    })
}
/// Canonical solid-target policy: water and nonblocking foliage do not occlude
/// terrain pickup. The first solid still prevents through-wall selection.
pub fn raycast_solid(
    world: &World,
    origin: [f64; 3],
    direction: [f64; 3],
    max_distance: f64,
) -> Option<RayHit> {
    raycast_matching(
        world,
        origin,
        direction,
        max_distance,
        Material::is_blocking,
    )
}
fn raycast_matching(
    world: &World,
    origin: [f64; 3],
    direction: [f64; 3],
    max_distance: f64,
    accepts: impl Fn(Material) -> bool,
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
        if step[i] < 0 && origin[i] == c[i] as f64 * voxel_size() {
            c[i] = c[i].checked_sub(1)?;
        }
        if step[i] != 0 {
            let plane = if step[i] > 0 {
                c[i].checked_add(1)?
            } else {
                c[i]
            };
            next[i] = ((plane as f64 * voxel_size() - origin[i]) / d[i]).max(0.0);
            delta[i] = voxel_size() / d[i].abs();
        }
    }
    let mut distance = 0.0;
    let mut normal = [0; 3];
    let mut column = None;
    let mut plants = vegetation::RayPlants::default();
    loop {
        let cell = Cell {
            x: c[0],
            y: c[1],
            z: c[2],
        };
        let base = world.material_over_source(
            cell,
            || {
                let (height, source) =
                    *column.get_or_insert_with(|| world.source_column(cell.x, cell.z));
                world.source_with_column(cell, height, source)
            },
            || plants.voxel(world, cell),
        );
        let material = world.compose_fluid(cell, base);
        if accepts(material) {
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
                if i != 1 {
                    column = None;
                }
                next[i] += delta[i];
            }
        }
        distance = t;
    }
}

#[cfg(test)]
mod live_water_overlay_tests {
    use super::*;
    #[test]
    fn static_snapshot_retains_edits_without_mutating_live_water() {
        let mut world = World::fbm(7, Default::default()).unwrap();
        let c = Cell {
            x: 0,
            y: world.height_cells(0, 0) + 30,
            z: 0,
        };
        let stone = Cell { x: 1, ..c };
        world.set(stone, Material::Stone);
        world.set_fluid_occupancy([(c, true)]);
        let frozen = world.without_fluid_snapshot();
        assert_eq!(frozen.material(c), Material::Air);
        assert_eq!(frozen.material(stone), Material::Stone);
        assert_eq!(world.material(c), Material::Water);
        assert_ne!(frozen.revision(), world.revision());
        assert_eq!(frozen.changed_edit_cells(&world), vec![c]);
        world.set_fluid_occupancy([(c, false)]);
        assert_eq!(frozen.material(c), Material::Air);
        assert_eq!(
            frozen.without_fluid_snapshot().revision(),
            frozen.revision()
        );
    }
    #[test]
    fn bounded_static_snapshots_preserve_dormant_wet_cells_and_dry_tombstones() {
        let mut world = World::fbm(7, Default::default()).unwrap().with_vegetation();
        let y = 16000;
        let a_wet = Cell { x: -2, y, z: -2 };
        let a_dry = Cell { x: -1, y, z: -2 };
        let a_hidden = Cell {
            x: -2,
            y: y + 1,
            z: -2,
        };
        let b_wet = Cell { x: 2, y, z: 2 };
        let b_dry = Cell { x: 3, y, z: 2 };
        let on_x_max = Cell { x: 0, y, z: -2 };
        let on_y_max = Cell {
            x: -2,
            y: y + 2,
            z: -2,
        };
        let on_z_max = Cell { x: -2, y, z: 0 };
        world.set_batch([(a_dry, Material::Water), (b_dry, Material::Water)]);
        world.set_fluid_occupancy([
            (a_wet, true),
            (a_dry, false),
            (a_hidden, true),
            (b_wet, true),
            (b_dry, false),
            (on_x_max, true),
            (on_y_max, true),
            (on_z_max, true),
        ]);
        world.set(a_hidden, Material::Stone);
        let original = world.clone();
        let a = world.without_fluid_in_bounds(
            Cell { x: -3, y, z: -3 },
            Cell {
                x: 0,
                y: y + 2,
                z: 0,
            },
        );
        let b = world.without_fluid_in_bounds(
            Cell { x: 2, y, z: 2 },
            Cell {
                x: 4,
                y: y + 1,
                z: 3,
            },
        );
        assert_eq!(a.material(a_wet), Material::Air);
        assert_eq!(a.material(a_dry), Material::Water);
        assert_eq!(a.material(a_hidden), Material::Stone);
        assert_eq!(a.material(b_wet), Material::Water);
        assert_eq!(a.material(b_dry), Material::Air);
        assert_eq!(b.material(a_wet), Material::Water);
        assert_eq!(b.material(a_dry), Material::Air);
        assert_eq!(b.material(b_wet), Material::Air);
        assert_eq!(b.material(b_dry), Material::Water);
        for c in [on_x_max, on_y_max, on_z_max] {
            assert_eq!(a.material(c), Material::Water);
        }
        assert_eq!(*world.fluid, *original.fluid);
        assert_eq!(world.revision(), original.revision());
        assert_eq!(world.change_token, original.change_token);
        for snapshot in [&a, &b] {
            assert!(snapshot.shares_source_lineage(&world));
            assert!(Arc::ptr_eq(&snapshot.edits, &world.edits));
            assert_eq!(
                snapshot.terrain_edits().collect::<Vec<_>>(),
                world.terrain_edits().collect::<Vec<_>>()
            );
            for c in [a_wet, a_dry, a_hidden, b_wet, b_dry] {
                assert_eq!(
                    snapshot.material_without_fluid(c),
                    world.material_without_fluid(c)
                );
            }
        }
        let mut removed = vec![a_wet, a_dry, a_hidden];
        removed.sort_unstable();
        assert_eq!(
            world
                .changes
                .lock()
                .unwrap()
                .candidates(a.change_token, world.change_token),
            Some(removed)
        );
        assert_eq!(
            world
                .changes
                .lock()
                .unwrap()
                .candidates(b.change_token, world.change_token),
            Some(vec![b_wet, b_dry])
        );
    }
    #[test]
    fn empty_reversed_and_disjoint_fluid_bounds_do_not_change_snapshot_revision() {
        let mut world = World::new(7);
        let c = Cell {
            x: -4,
            y: 16000,
            z: -4,
        };
        world.set_fluid_occupancy([(c, true)]);
        let bounds = [
            (c, c),
            (Cell { x: c.x + 1, ..c }, c),
            (
                c,
                Cell {
                    x: c.x + 1,
                    y: c.y - 1,
                    z: c.z + 1,
                },
            ),
            (
                c,
                Cell {
                    x: c.x + 1,
                    y: c.y + 1,
                    z: c.z - 1,
                },
            ),
            (
                Cell {
                    x: i64::MAX - 1,
                    y: i64::MIN,
                    z: i64::MIN,
                },
                Cell {
                    x: i64::MAX,
                    y: i64::MAX,
                    z: i64::MAX,
                },
            ),
        ];
        for (min, max) in bounds {
            let snapshot = world.without_fluid_in_bounds(min, max);
            assert_eq!(snapshot.revision(), world.revision());
            assert_eq!(snapshot.change_token, world.change_token);
            assert!(Arc::ptr_eq(&snapshot.fluid, &world.fluid));
            assert_eq!(snapshot.material(c), Material::Water);
        }
    }
    #[test]
    fn native_column_overlay_snapshots_diff_and_solidity_agree() {
        let mut world = World::fbm(7, Default::default()).unwrap();
        let c = Cell {
            x: 0,
            y: world.height_cells(0, 0) + 4,
            z: 0,
        };
        let initial = world.clone();
        world.set_fluid_occupancy([(c, false)]);
        assert_eq!(world.revision(), initial.revision());
        assert_eq!(world.edit_cells().count(), 0);
        world.set_fluid_occupancy([(c, true)]);
        assert_eq!(world.material(c), Material::Water);
        assert_eq!(world.column(c.x, c.z).material(c.y), Material::Water);
        assert_eq!(initial.material(c), Material::Air);
        assert_eq!(world.changed_edit_cells(&initial), vec![c]);
        let wet = world.clone();
        let revision = world.revision();
        world.set_fluid_occupancy([(c, true)]);
        assert_eq!(revision, world.revision());
        world.set_fluid_occupancy([(c, false)]);
        assert_eq!(world.changed_edit_cells(&wet), vec![c]);
        assert_eq!(world.material(c), Material::Air);
        world.set(c, Material::Stone);
        world.set_fluid_occupancy([(c, true)]);
        assert_eq!(world.material(c), Material::Stone);
        assert_eq!(world.column(c.x, c.z).material(c.y), Material::Stone);
        world.set_fluid_occupancy([(c, false)]);
        world.set(c, Material::Air);
        assert_eq!(world.material(c), Material::Air);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn full_edit_diff(a: &World, b: &World) -> Vec<Cell> {
        a.edit_cells()
            .chain(b.edit_cells())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter(|&c| {
                (a.edits.get(&c) != b.edits.get(&c) || a.fluid.get(&c) != b.fluid.get(&c))
                    && a.column(c.x, c.z).canonical_material(c.y)
                        != b.column(c.x, c.z).canonical_material(c.y)
            })
            .collect()
    }
    #[test]
    fn bounded_journal_matches_scan_across_branches_reverts_and_eviction() {
        let mut world = World::new(9);
        let oldest = world.clone();
        for x in 0..2000 {
            world.set(Cell { x, y: 10000, z: 0 }, Material::Stone);
        }
        assert!(world
            .changes
            .lock()
            .unwrap()
            .candidates(world.change_token, oldest.change_token)
            .is_none());
        assert_eq!(
            world.changed_edit_cells(&oldest),
            full_edit_diff(&world, &oldest)
        );
        let base = world.clone();
        let mut snapshots = vec![base.clone()];
        let mut random = 7349u64;
        for tick in 0..600 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            if tick % 7 == 0 {
                world = snapshots[(random as usize) % snapshots.len()].clone();
            }
            let c = Cell {
                x: (random % 31) as i64,
                y: 10000,
                z: 0,
            };
            if tick % 3 == 0 {
                world.set_fluid_occupancy([(c, random & 32 != 0)]);
            } else {
                world.set(
                    c,
                    if random & 64 == 0 {
                        Material::Air
                    } else {
                        Material::Stone
                    },
                );
            }
            let other = &snapshots[(random as usize / 32) % snapshots.len()];
            assert_eq!(
                world.changed_edit_cells(other),
                full_edit_diff(&world, other)
            );
            assert_eq!(
                other.changed_edit_cells(&world),
                full_edit_diff(other, &world)
            );
            if tick % 11 == 0 {
                snapshots.push(world.clone());
            }
            let journal = world.changes.lock().unwrap();
            assert!(journal.cells <= CHANGE_JOURNAL_CELLS);
            assert!(journal.records.len() <= CHANGE_JOURNAL_RECORDS);
        }
        // An immediate delta costs one candidate even with thousands of stored edits.
        let before = world.clone();
        world.set(
            Cell {
                x: -1,
                y: 10000,
                z: 0,
            },
            Material::Wood,
        );
        assert_eq!(
            world
                .changes
                .lock()
                .unwrap()
                .candidates(world.change_token, before.change_token)
                .unwrap()
                .len(),
            1
        );
        let replacement = World::new(44);
        assert_eq!(
            world.changed_edit_cells(&replacement),
            full_edit_diff(&world, &replacement)
        );
        let replacement = base.clone().with_vegetation();
        assert_eq!(
            world.changed_edit_cells(&replacement),
            full_edit_diff(&world, &replacement)
        );
        // An over-budget single batch is omitted, with exact scan fallback.
        let before = world.clone();
        world.set_fluid_occupancy((0..CHANGE_JOURNAL_CELLS + 1).map(|x| {
            (
                Cell {
                    x: x as i64,
                    y: 20000,
                    z: 0,
                },
                true,
            )
        }));
        assert!(world
            .changes
            .lock()
            .unwrap()
            .candidates(world.change_token, before.change_token)
            .is_none());
        assert_eq!(
            world.changed_edit_cells(&before),
            full_edit_diff(&world, &before)
        );
    }

    #[test]
    fn terrain_undo_restores_authorship_and_publishes_a_forward_revision() {
        let mut world = World::new(19);
        let a = Cell {
            x: -3,
            y: 300,
            z: 2,
        };
        let b = Cell { x: -2, ..a };
        world.set(a, Material::Wood);
        let before = world.clone();
        let end = [b.x + 1, a.y + 1, a.z + 1];
        let min = [a.x, a.y, a.z];
        let aborted = world.edit_terrain_box(min, end, |c, _| {
            if c == b {
                Err("blocked")
            } else {
                Ok(Some(Material::Snow))
            }
        });
        assert!(aborted.is_err());
        assert_eq!(before.revision(), world.revision());
        assert!(world.shares_terrain_edits(&before));
        let undo = world
            .edit_terrain_box(min, end, |c, _| {
                Ok::<_, ()>(Some(if c == a {
                    Material::Air
                } else {
                    Material::Snow
                }))
            })
            .unwrap()
            .unwrap();
        let stamped = world.clone();
        assert_eq!(undo.cells(), &[a, b]);
        assert!(world.undo_terrain_batch(&undo));
        assert!(world.revision() > stamped.revision());
        assert!(world.shares_terrain_edits(&before));
        assert_eq!(world.terrain_override(a), Some(Material::Wood));
        assert_eq!(world.terrain_override(b), None);
        assert_eq!(world.changed_edit_cells(&stamped), vec![a, b]);
        assert!(!world.undo_terrain_batch(&undo));
        let mut advanced = stamped.clone();
        advanced.set(b, Material::Sand);
        assert!(!advanced.undo_terrain_batch(&undo));
        assert_eq!(advanced.terrain_override(b), Some(Material::Sand));
    }

    #[test]
    fn restored_overlay_keeps_placed_source_material_and_removes_stale_edits() {
        let mut world = World::new(19);
        let soil = Cell { x: 0, y: -40, z: 0 };
        let stale = Cell { x: 1, ..soil };
        let original = world.material(soil);
        world.set(stale, Material::Air);
        world.restore_terrain_edits([(soil, original)]);
        assert_eq!(
            world.terrain_edits().collect::<Vec<_>>(),
            [(soil, original)]
        );
        assert_eq!(
            world.edit_cells_in_bounds(soil, Cell { x: 2, y: -39, z: 1 }),
            [soil]
        );
    }

    #[test]
    fn batch_edits_use_one_revision_and_last_write_wins() {
        let mut world = World::new(5);
        let a = Cell {
            x: 0,
            y: 10000,
            z: 0,
        };
        let b = Cell { x: 1, ..a };
        let initial = world.clone();
        world.set_batch([
            (a, Material::Wood),
            (b, Material::Stone),
            (a, Material::Air),
        ]);
        assert_eq!(world.revision(), 1);
        assert_eq!(world.material(a), Material::Air);
        assert_eq!(world.changed_edit_cells(&initial), vec![b]);
        let revision = world.revision();
        world.set_batch([(b, Material::Air), (b, Material::Stone)]);
        assert_eq!(world.revision(), revision);
    }

    #[test]
    fn local_edit_diff_and_source_lineage_handle_divergence_and_reverts() {
        let source = World::new(7).with_vegetation();
        let mut edited = source.clone();
        let c = Cell {
            x: -1,
            y: 10000,
            z: -513,
        };
        edited.set(c, Material::Stone);
        assert!(source.shares_source_lineage(&edited));
        assert_eq!(source.changed_edit_cells(&edited), vec![c]);
        assert_eq!(edited.changed_edit_cells(&source), vec![c]);
        assert!(edited.changed_edit_cells(&edited.clone()).is_empty());
        let mut divergent = source.clone();
        divergent.set(c, Material::Air);
        assert_eq!(edited.changed_edit_cells(&divergent), vec![c]);
        assert!(!source.shares_source_lineage(&World::new(7).with_vegetation()));
        assert!(!source.shares_source_lineage(&source.clone().with_vegetation_enabled(false)));
    }
    #[test]
    fn topology_footprint_is_canonical_and_independent_of_camera() {
        let params = FbmParams {
            octaves: 1,
            ..Default::default()
        };
        let topology = World::fbm_topology(7, params).unwrap();
        let plain = World::fbm(7, params).unwrap();
        let base = topology.topology_base_cells().unwrap();
        for (x, z) in [
            (-129, 0),
            (-128, 0),
            (127, 0),
            (128, 0),
            (0, -129),
            (0, -128),
            (0, 127),
            (0, 128),
            (-1, -1),
        ] {
            assert_eq!(
                topology.height_m(x as f64 * voxel_size(), z as f64 * voxel_size()),
                plain.height_m(x as f64 * voxel_size(), z as f64 * voxel_size())
            );
            for y in base - 80..base + 24 {
                let c = Cell { x, y, z };
                if !topology.volume_owned(c) {
                    assert_eq!(topology.material(c), plain.material(c));
                }
            }
        }
        let cave = Cell {
            x: -48,
            y: base - 32,
            z: 0,
        };
        assert_eq!(topology.material(cave), Material::Air);
        let o = [-3.03125, (base as f64 - 31.5) * voxel_size(), -6.8];
        let hit = raycast(&topology, o, [0., 0., 1.], 20.).unwrap();
        assert_eq!(hit.cell.z, 80); // real tunnel back wall, never a mouth cap
        let deck = Cell {
            x: 64,
            y: base + 16,
            z: -24,
        };
        assert_eq!(topology.material(deck), Material::Wood);
        let below = [4.03125, (base as f64 - 20.) * voxel_size(), -1.46875];
        let underside = raycast(&topology, below, [0., 1., 0.], 5.).unwrap();
        assert_eq!(underside.cell.y, base + 16);
        assert_eq!(underside.normal, [0, -1, 0]);
    }
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
            a.material(Cell::from_meters([-3.0, h - voxel_size(), 2.0])),
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
        let hit = raycast(
            &w,
            [-voxel_size(), 100.03125, 0.03125],
            [2.0, 0.0, 0.0],
            1.0,
        )
        .unwrap();
        assert_eq!(hit.cell, p);
        assert_eq!(hit.normal, [-1, 0, 0]);
        assert_eq!(hit.distance, voxel_size());
        assert!(raycast(
            &w,
            [-voxel_size(), 100.03125, 0.03125],
            [1.0, 0.0, 0.0],
            voxel_size()
        )
        .is_some());
        assert!(raycast(
            &w,
            [-voxel_size(), 100.03125, 0.03125],
            [1.0, 0.0, 0.0],
            voxel_size() / 2.0
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
        assert_eq!(hit.distance, 1.0 - voxel_size() / 2.0);
    }
    #[test]
    fn diagonal_rays_resample_crossed_columns_and_preserve_edit_and_water_ownership() {
        let mut world = World::fbm(
            19,
            FbmParams {
                recipe: FbmRecipe::Highlands,
                ..Default::default()
            },
        )
        .unwrap();
        let size = voxel_size();
        for x in -10..10 {
            let origin = Cell {
                x,
                y: world.height_cells(x, 5) + 12,
                z: 5,
            };
            let at = |n| Cell {
                x: x + n,
                y: origin.y - n,
                z: 5 - n,
            };
            if x % 2 == 0 {
                world.set_batch([(at(3), Material::Air), (at(9), Material::Wood)]);
                world.set_fluid_occupancy([(at(3), true)]);
            }
            // From a cell center, all three planes tie on every step. This
            // gives an independent sequence of cells without using DDA state.
            for solid in [false, true] {
                let expected = (0..=40).find_map(|n| {
                    let cell = at(n);
                    let material = world.material(cell);
                    (if solid {
                        material.is_blocking()
                    } else {
                        material != Material::Air
                    })
                    .then_some((cell, material))
                });
                let pick = if solid { raycast_solid } else { raycast };
                let result = pick(
                    &world,
                    origin.center_m(),
                    [1., -1., -1.],
                    40. * size * 3f64.sqrt(),
                );
                assert_eq!(result.map(|hit| (hit.cell, hit.material)), expected);
            }
        }
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
