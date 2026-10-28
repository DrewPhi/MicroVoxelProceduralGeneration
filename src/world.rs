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
    generator: G,
    materials: MaterialRegistry,
    pub(crate) edits: EditStore,
    revision: u64,
    chunk_revisions: BTreeMap<ChunkPos, u64>,
}
impl<G: Generator> World<G> {
    pub fn new(generator: G) -> Self {
        Self::with_materials(generator, MaterialRegistry::default())
    }
    pub fn with_materials(generator: G, materials: MaterialRegistry) -> Self {
        Self {
            generator,
            materials,
            edits: EditStore::default(),
            revision: 0,
            chunk_revisions: BTreeMap::new(),
        }
    }
    pub fn identity(&self) -> GeneratorIdentity {
        self.generator.identity()
    }
    pub fn materials(&self) -> &MaterialRegistry {
        &self.materials
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn chunk_revision(&self, key: ChunkPos) -> u64 {
        self.chunk_revisions.get(&key).copied().unwrap_or(0)
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
                if run.material == MaterialId::ROCK {
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
