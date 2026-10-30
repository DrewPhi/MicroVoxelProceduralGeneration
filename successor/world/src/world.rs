//! One authoritative read/change interface for tools, renderers, and simulation.
use crate::{
    generation::{push_run, Column, Generator, GeneratorIdentity, Run},
    grid::{CellPos, ChunkPos, CHUNK_VOLUME},
    materials::{MaterialId, MaterialRegistry, Voxel},
    storage::EditStore,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup {
    Known(Voxel),
    Unknown,
}
impl Lookup {
    pub fn known(self) -> Option<Voxel> {
        match self {
            Self::Known(v) => Some(v),
            Self::Unknown => None,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Edit {
    pub pos: CellPos,
    pub value: Voxel,
    pub expected: Option<Voxel>,
}
impl Edit {
    pub fn set(pos: CellPos, value: Voxel) -> Self {
        Self {
            pos,
            value,
            expected: None,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum EditAction {
    Set(Edit),
    Reset {
        pos: CellPos,
        expected: Option<Voxel>,
    },
}
#[derive(Clone, Debug, Default)]
pub struct ChangeSet {
    pub revision: u64,
    pub changed_cells: Vec<CellPos>,
    pub geometry: BTreeSet<ChunkPos>,
    pub appearance: BTreeSet<ChunkPos>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    TooLarge,
    Duplicate(CellPos),
    Unknown(CellPos),
    Conflict(CellPos),
    UnknownMaterial(MaterialId),
    RevisionOverflow,
}
impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for EditError {}

pub struct World<G: Generator> {
    instance_id: u64,
    generator: Arc<G>,
    materials: MaterialRegistry,
    pub(crate) edits: EditStore,
    revision: u64,
    chunk_revisions: BTreeMap<ChunkPos, u64>,
}
impl<G: Generator> World<G> {
    pub fn water_footprint(
        &self,
        x: i64,
        z: i64,
        step: i64,
    ) -> Option<crate::generation::river::WaterFootprint> {
        if self.region_changed_since(x, z, step, 0) {
            return None;
        }
        let scale = self.spec().grid.meters();
        self.generator
            .water_footprint([x as f64 * scale, z as f64 * scale], step as f64 * scale)
    }
    pub fn baseline(&self) -> &G {
        &self.generator
    }
    pub fn new(generator: G) -> Self {
        Self::with_materials(generator, MaterialRegistry::default())
    }
    pub fn with_materials(generator: G, materials: MaterialRegistry) -> Self {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            instance_id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            generator: Arc::new(generator),
            materials,
            edits: EditStore::default(),
            revision: 0,
            chunk_revisions: BTreeMap::new(),
        }
    }
    pub fn identity(&self) -> GeneratorIdentity {
        self.generator.identity()
    }
    pub fn cache_namespace(&self) -> u64 {
        self.instance_id
    }
    pub fn spec(&self) -> crate::WorldSpec {
        self.generator.spec()
    }
    pub fn has_volumetric_features(&self, origin: [f64; 2], edge: f64) -> bool {
        self.generator.has_volumetric_features(origin, edge)
    }
    pub fn terrain_summary(
        &self,
        origin: [f64; 2],
        edge: f64,
    ) -> Option<crate::generation::summary::TerrainSummary> {
        let mut summary = self.generator.terrain_summary(origin, edge)?;
        let s = self.spec().grid.meters();
        for key in self.edits.bricks.keys() {
            let p = key.origin()?;
            let x = p.x as f64 * s;
            let z = p.z as f64 * s;
            let e = 32. * s;
            if x < origin[0] + edge
                && x + e > origin[0]
                && z < origin[1] + edge
                && z + e > origin[1]
            {
                summary.min_m = summary.min_m.min(p.y as f64 * s);
                summary.max_m = summary.max_m.max((p.y + 32) as f64 * s);
                summary.error_m = summary.error_m.max(e);
                summary.feature_scale_m = summary.feature_scale_m.min(s);
            }
        }
        Some(summary)
    }
    pub fn display_voids(&self, x: i64, z: i64, step: i64) -> Vec<[i64; 2]> {
        if self.region_changed_since(x, z, step, 0) {
            Vec::new()
        } else {
            self.generator.display_voids(x, z, step)
        }
    }
    pub fn display_solids(&self, x: i64, z: i64, step: i64) -> Vec<Run> {
        if self.region_changed_since(x, z, step, 0) {
            Vec::new()
        } else {
            self.generator.display_solids(x, z, step)
        }
    }
    pub fn environment_at(&self, pos: CellPos) -> Option<crate::generation::EnvironmentSample> {
        self.generator.environment_at(pos)
    }
    pub fn appearance_at(
        &self,
        pos: CellPos,
        material: MaterialId,
    ) -> crate::materials::Appearance {
        self.edits
            .get(pos)
            .filter(|v| v.material == material)
            .map_or_else(
                || self.generator.appearance_at(pos, material),
                |v| v.appearance,
            )
    }
    pub fn materials(&self) -> &MaterialRegistry {
        &self.materials
    }
    pub fn display_appearance_at(
        &self,
        pos: CellPos,
        material: MaterialId,
        step: i64,
    ) -> crate::materials::Appearance {
        self.edits
            .get(pos)
            .filter(|v| v.material == material)
            .map_or_else(
                || self.generator.display_appearance_at(pos, material, step),
                |v| v.appearance,
            )
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// A consistent revision for background workers. Cloning shares the source
    /// and every override brick; subsequent edits copy only touched bricks.
    pub fn read_snapshot(&self) -> crate::snapshot::WorldSnapshot<G> {
        crate::snapshot::WorldSnapshot::new(
            self.generator.clone(),
            self.edits.clone(),
            self.revision,
        )
    }
    /// Fork a revision for a new immutable worker snapshot. The source and
    /// untouched override bricks remain shared; only edited bricks are copied.
    pub fn fork_revision(&self) -> Self {
        self.validation_copy()
    }
    /// Version of a physical render footprint, including its filter/halo support.
    pub fn region_revision(&self, x: i64, z: i64, edge: i64) -> u64 {
        if edge <= 0 {
            return 0;
        }
        let Some(x1) = x.checked_add(edge - 1) else {
            return self.revision;
        };
        let Some(z1) = z.checked_add(edge - 1) else {
            return self.revision;
        };
        let (x0, z0) = (x.div_euclid(32), z.div_euclid(32));
        let (x1, z1) = (x1.div_euclid(32), z1.div_euclid(32));
        self.chunk_revisions
            .iter()
            .filter(|(p, _)| p.x >= x0 && p.x <= x1 && p.z >= z0 && p.z <= z1)
            .map(|(_, r)| *r)
            .max()
            .unwrap_or(0)
    }
    pub(crate) fn validation_copy(&self) -> Self {
        Self {
            instance_id: self.instance_id,
            generator: self.generator.clone(),
            materials: self.materials.clone(),
            edits: self.edits.clone(),
            revision: self.revision,
            chunk_revisions: self.chunk_revisions.clone(),
        }
    }
    pub fn fill_voxels(
        &self,
        bounds: crate::Bounds,
    ) -> Result<Vec<Lookup>, crate::snapshot::QueryError> {
        self.read_snapshot().fill_voxels(bounds)
    }
    pub fn chunk_revision(&self, key: ChunkPos) -> u64 {
        self.chunk_revisions.get(&key).copied().unwrap_or(0)
    }
    /// Local display-cache invalidation; no voxel generation or dense reads.
    pub fn region_changed_since(&self, x: i64, z: i64, edge: i64, revision: u64) -> bool {
        if edge <= 0 {
            return true;
        }
        if revision == self.revision {
            return false;
        }
        let low = ChunkPos {
            x: x.div_euclid(32),
            y: i64::MIN,
            z: i64::MIN,
        };
        let Some(x_end) = x.checked_add(edge).and_then(|v| v.checked_sub(1)) else {
            return true;
        };
        let Some(z_end) = z.checked_add(edge).and_then(|v| v.checked_sub(1)) else {
            return true;
        };
        let high = ChunkPos {
            x: x_end.div_euclid(32),
            y: i64::MAX,
            z: i64::MAX,
        };
        let z0 = z.div_euclid(32);
        let z1 = z_end.div_euclid(32);
        self.chunk_revisions
            .range(low..=high)
            .any(|(key, value)| *value > revision && (z0..=z1).contains(&key.z))
    }
    pub fn storage(&self) -> &EditStore {
        &self.edits
    }
    pub(crate) fn baseline_at(&self, p: CellPos) -> Option<Voxel> {
        self.generator.voxel(p)
    }
    pub fn voxel_at(&self, p: CellPos) -> Lookup {
        self.edits
            .get(p)
            .or_else(|| self.generator.voxel(p))
            .map_or(Lookup::Unknown, Lookup::Known)
    }
    /// Reset removes an override and reveals the procedural baseline.
    pub fn apply_actions(&mut self, actions: &[EditAction]) -> Result<ChangeSet, EditError> {
        if actions.len() > 65536 {
            return Err(EditError::TooLarge);
        }
        let mut edits = Vec::with_capacity(actions.len());
        for action in actions {
            edits.push(match *action {
                EditAction::Set(edit) => edit,
                EditAction::Reset { pos, expected } => Edit {
                    pos,
                    value: self.baseline_at(pos).ok_or(EditError::Unknown(pos))?,
                    expected,
                },
            });
        }
        self.apply(&edits)
    }
    /// Fully validated before mutation. Air overrides are real excavation data.
    pub fn apply(&mut self, edits: &[Edit]) -> Result<ChangeSet, EditError> {
        if edits.len() > 65536 {
            return Err(EditError::TooLarge);
        }
        let mut seen = BTreeSet::new();
        let mut changes = Vec::new();
        for edit in edits {
            if !seen.insert(edit.pos) {
                return Err(EditError::Duplicate(edit.pos));
            }
            if self.materials.get(edit.value.material).is_none() {
                return Err(EditError::UnknownMaterial(edit.value.material));
            }
            let old = self
                .voxel_at(edit.pos)
                .known()
                .ok_or(EditError::Unknown(edit.pos))?;
            if edit.expected.is_some() && edit.expected != Some(old) {
                return Err(EditError::Conflict(edit.pos));
            }
            if old != edit.value {
                let baseline = self
                    .generator
                    .voxel(edit.pos)
                    .ok_or(EditError::Unknown(edit.pos))?;
                changes.push((*edit, old, baseline));
            }
        }
        if changes.is_empty() {
            return Ok(ChangeSet {
                revision: self.revision,
                ..Default::default()
            });
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(EditError::RevisionOverflow)?;
        let mut result = ChangeSet {
            revision,
            ..Default::default()
        };
        let mut touched = BTreeSet::new();
        for (edit, old, baseline) in changes {
            self.edits.set(
                edit.pos,
                if baseline == edit.value {
                    None
                } else {
                    Some(edit.value)
                },
            );
            let key = edit.pos.chunk();
            touched.insert(key);
            result.changed_cells.push(edit.pos);
            result.appearance.insert(key);
            if old.material != edit.value.material {
                result.geometry.insert(key);
                for delta in [
                    [-1, 0, 0],
                    [1, 0, 0],
                    [0, -1, 0],
                    [0, 1, 0],
                    [0, 0, -1],
                    [0, 0, 1],
                ] {
                    if let Some(neighbor) = edit.pos.offset(delta) {
                        if neighbor.chunk() != key {
                            result.geometry.insert(neighbor.chunk());
                        }
                    }
                }
            }
        }
        self.edits.compact(&touched);
        for key in result.geometry.iter().chain(result.appearance.iter()) {
            self.chunk_revisions.insert(*key, revision);
        }
        self.revision = revision;
        Ok(result)
    }
    pub fn snapshot(&self, key: ChunkPos) -> Option<ChunkSnapshot> {
        let origin = key.origin()?;
        let mut cells = Vec::with_capacity(CHUNK_VOLUME);
        for i in 0..CHUNK_VOLUME {
            cells.push(self.voxel_at(key.cell(i as u16)?).known()?);
        }
        Some(ChunkSnapshot {
            key,
            origin,
            revision: self.chunk_revision(key),
            cells,
        })
    }
    pub fn column_runs(&self, x: i64, z: i64, range: Range<i64>) -> Option<Vec<Run>> {
        if range.start >= range.end {
            return Some(Vec::new());
        }
        let baseline = self.generator.runs(x, z, range.clone())?;
        let edits = self.edits.column_edits(x, z);
        if edits.is_empty() {
            return Some(baseline);
        }
        let mut cuts = vec![range.start, range.end];
        for run in &baseline {
            cuts.push(run.start);
            cuts.push(run.end);
        }
        for &(y, _) in &edits {
            if range.contains(&y) {
                cuts.push(y);
                if let Some(end) = y.checked_add(1) {
                    cuts.push(end.min(range.end));
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        let mut result = Vec::new();
        let mut base_index = 0;
        for pair in cuts.windows(2) {
            while base_index + 1 < baseline.len() && baseline[base_index].end <= pair[0] {
                base_index += 1;
            }
            let material = edits
                .binary_search_by_key(&pair[0], |e| e.0)
                .ok()
                .map(|i| edits[i].1.material)
                .unwrap_or(baseline[base_index].material);
            push_run(
                &mut result,
                Run {
                    start: pair[0],
                    end: pair[1],
                    material,
                },
            );
        }
        Some(result)
    }
    /// Band-limit only an unchanged procedural column. Edited columns keep their
    /// exact surface; sparse edits never depend on the camera's display settings.
    pub fn display_column(&self, x: i64, z: i64, spacing: i64) -> Option<Column> {
        if self.edits.column_edits(x, z).is_empty() {
            self.generator.display_column(x, z, spacing)
        } else {
            self.surface_column(x, z)
        }
    }
    /// Coarse generation never asks for a native appearance on an unedited column.
    pub fn display_sample(
        &self,
        x: i64,
        z: i64,
        spacing: i64,
    ) -> Option<(Column, Option<crate::generation::EnvironmentSample>)> {
        if self.edits.column_edits(x, z).is_empty() {
            self.generator.display_sample(x, z, spacing)
        } else {
            Some((self.surface_column(x, z)?, None))
        }
    }
    pub fn display_surface_cover(&self, pos: CellPos) -> Option<[(MaterialId, f32); 5]> {
        if self.edits.column_edits(pos.x, pos.z).is_empty() {
            self.generator.surface_cover_at(pos)
        } else {
            None
        }
    }
    pub fn display_coverage_at(
        &self,
        pos: CellPos,
        spacing: i64,
    ) -> Option<[(MaterialId, f32); 5]> {
        if self.region_changed_since(pos.x - spacing / 2, pos.z - spacing / 2, spacing, 0) {
            None
        } else {
            self.generator.display_cover_at(pos, spacing)
        }
    }
    /// Optimized metadata for terrain LOD. Holes remain in column_runs/voxel_at.
    pub fn surface_column(&self, x: i64, z: i64) -> Option<Column> {
        let baseline = self.generator.column_hint(x, z)?;
        let changes = self.edits.column_edits(x, z);
        if changes.is_empty() {
            return Some(baseline);
        }
        let highest = changes
            .last()
            .map_or(baseline.water.max(baseline.ground), |e| {
                e.0.saturating_add(1)
                    .max(baseline.water.max(baseline.ground))
            });
        let runs = self.column_runs(x, z, 0..highest.max(1))?;
        let mut ground = 0;
        let mut water = 0;
        let mut surface = MaterialId::AIR;
        let mut bedrock = 0;
        for run in runs {
            if run.material == MaterialId::WATER {
                water = water.max(run.end);
            } else if run.material != MaterialId::AIR {
                ground = run.end;
                surface = run.material;
                if run.material.is_bedrock() {
                    bedrock = run.end;
                }
            }
        }
        Some(Column {
            ground,
            water: water.max(ground),
            bedrock,
            surface,
            subsoil: baseline.subsoil,
            rock: baseline.rock,
        })
    }
    pub(crate) fn replace_edits(&mut self, store: EditStore, revision: u64) {
        self.edits = store;
        self.revision = revision;
        self.chunk_revisions.clear();
        for key in self.edits.bricks.keys() {
            self.chunk_revisions.insert(*key, revision);
        }
    }
}
#[derive(Clone, Debug)]
pub struct ChunkSnapshot {
    pub key: ChunkPos,
    pub origin: CellPos,
    pub revision: u64,
    cells: Vec<Voxel>,
}
impl ChunkSnapshot {
    pub fn at(&self, p: CellPos) -> Option<Voxel> {
        (p.chunk() == self.key).then(|| self.cells[p.local_index() as usize])
    }
    pub fn cells(&self) -> &[Voxel] {
        &self.cells
    }
}
