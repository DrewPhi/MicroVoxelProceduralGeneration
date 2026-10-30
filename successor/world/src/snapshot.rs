//! Immutable, revision-consistent authoritative reads for worker jobs.
use crate::{
    generation::{push_run, Generator, GeneratorIdentity, Run},
    grid::{Bounds, CellPos},
    storage::EditStore,
    world::Lookup,
};
use std::{ops::Range, sync::Arc};

pub const MAX_BULK_CELLS: usize = 32 * 32 * 32;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryError {
    InvalidBounds,
    TooLarge,
}
impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for QueryError {}

pub struct WorldSnapshot<G: Generator> {
    generator: Arc<G>,
    edits: EditStore,
    revision: u64,
    identity: GeneratorIdentity,
}
impl<G: Generator> Clone for WorldSnapshot<G> {
    fn clone(&self) -> Self {
        Self {
            generator: self.generator.clone(),
            edits: self.edits.clone(),
            revision: self.revision,
            identity: self.identity,
        }
    }
}
impl<G: Generator> WorldSnapshot<G> {
    pub(crate) fn new(generator: Arc<G>, edits: EditStore, revision: u64) -> Self {
        let identity = generator.identity();
        Self {
            generator,
            edits,
            revision,
            identity,
        }
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn identity(&self) -> GeneratorIdentity {
        self.identity
    }
    pub fn voxel_at(&self, p: CellPos) -> Lookup {
        self.edits
            .get(p)
            .or_else(|| self.generator.voxel(p))
            .map_or(Lookup::Unknown, Lookup::Known)
    }
    /// Y varies fastest, then X, then Z. Unknown remains explicit.
    pub fn fill_voxels(&self, bounds: Bounds) -> Result<Vec<Lookup>, QueryError> {
        let volume = bounds.volume().ok_or(QueryError::InvalidBounds)?;
        if volume > MAX_BULK_CELLS {
            return Err(QueryError::TooLarge);
        }
        let mut out = Vec::with_capacity(volume);
        for z in bounds.min.z..bounds.max.z {
            for x in bounds.min.x..bounds.max.x {
                for y in bounds.min.y..bounds.max.y {
                    out.push(self.voxel_at(CellPos::new(x, y, z)));
                }
            }
        }
        Ok(out)
    }
    /// Bounded exact material runs, including Air; returns None if the source is unknown.
    pub fn column_runs(
        &self,
        x: i64,
        z: i64,
        range: Range<i64>,
    ) -> Result<Option<Vec<Run>>, QueryError> {
        let len = range
            .end
            .checked_sub(range.start)
            .ok_or(QueryError::InvalidBounds)?;
        if len < 0 {
            return Err(QueryError::InvalidBounds);
        }
        if len as u64 > MAX_BULK_CELLS as u64 {
            return Err(QueryError::TooLarge);
        }
        if range.is_empty() {
            return Ok(Some(Vec::new()));
        }
        let mut out = Vec::new();
        let mut cursor = range.start;
        // Query contiguous baseline intervals through the generator's exact run
        // API. Overridden cells never consult the source, including explicit Air
        // masking an unknown baseline cell. Reset entries are absent here.
        for (y, voxel) in self.edits.column_edits(x, z) {
            if !range.contains(&y) {
                continue;
            }
            if cursor < y {
                let Some(runs) = self.generator.runs(x, z, cursor..y) else {
                    return Ok(None);
                };
                for run in runs {
                    push_run(&mut out, run);
                }
            }
            // y is strictly below range.end, so its successor cannot overflow.
            cursor = y + 1;
            push_run(
                &mut out,
                Run {
                    start: y,
                    end: cursor,
                    material: voxel.material,
                },
            );
        }
        if cursor < range.end {
            let Some(runs) = self.generator.runs(x, z, cursor..range.end) else {
                return Ok(None);
            };
            for run in runs {
                push_run(&mut out, run);
            }
        }
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        materials::{Appearance, MaterialId, Voxel},
        Edit, EditAction, World,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Calls {
        runs: AtomicUsize,
        voxels: AtomicUsize,
    }
    struct Analytic {
        calls: Arc<Calls>,
    }
    impl Analytic {
        fn material(y: i64) -> MaterialId {
            if y < -2 {
                MaterialId::ROCK
            } else if y < 2 {
                MaterialId::SOIL
            } else {
                MaterialId::AIR
            }
        }
    }
    impl Generator for Analytic {
        fn identity(&self) -> GeneratorIdentity {
            GeneratorIdentity {
                seed: 0,
                version: 1,
                definition: 0,
            }
        }
        fn voxel(&self, p: CellPos) -> Option<Voxel> {
            self.calls.voxels.fetch_add(1, Ordering::Relaxed);
            (p.y != 7).then(|| Voxel::new(Self::material(p.y), Appearance(0x1234)))
        }
        fn appearance_at(&self, _pos: CellPos, _material: MaterialId) -> Appearance {
            Appearance(0x5678)
        }
        fn runs(&self, _x: i64, _z: i64, range: Range<i64>) -> Option<Vec<Run>> {
            self.calls.runs.fetch_add(1, Ordering::Relaxed);
            if range.contains(&7) {
                return None;
            }
            let mut cuts = vec![range.start, range.end];
            cuts.extend(
                [-2, 2]
                    .into_iter()
                    .filter(|y| *y > range.start && *y < range.end),
            );
            cuts.sort_unstable();
            Some(
                cuts.windows(2)
                    .map(|pair| Run {
                        start: pair[0],
                        end: pair[1],
                        material: Self::material(pair[0]),
                    })
                    .collect(),
            )
        }
    }
    fn source() -> (World<Analytic>, Arc<Calls>) {
        let calls = Arc::new(Calls::default());
        (
            World::new(Analytic {
                calls: calls.clone(),
            }),
            calls,
        )
    }
    fn clear(calls: &Calls) {
        calls.runs.store(0, Ordering::Relaxed);
        calls.voxels.store(0, Ordering::Relaxed);
    }
    #[test]
    fn unedited_column_uses_one_analytic_run_query() {
        let (world, calls) = source();
        let runs = world
            .read_snapshot()
            .column_runs(-33, -1, -32_000..7)
            .unwrap()
            .unwrap();
        assert_eq!(calls.runs.load(Ordering::Relaxed), 1);
        assert_eq!(calls.voxels.load(Ordering::Relaxed), 0);
        assert_eq!(
            runs,
            vec![
                Run {
                    start: -32_000,
                    end: -2,
                    material: MaterialId::ROCK
                },
                Run {
                    start: -2,
                    end: 2,
                    material: MaterialId::SOIL
                },
                Run {
                    start: 2,
                    end: 7,
                    material: MaterialId::AIR
                },
            ]
        );
    }
    #[test]
    fn overrides_merge_across_signed_chunks_and_exclusive_range_edges() {
        let (mut world, calls) = source();
        let stone = Voxel::new(MaterialId::BASALT, Appearance(0x39e7));
        for (y, value) in [
            (-33, stone),
            (-32, Voxel::AIR),
            (-2, Voxel::new(MaterialId::SOIL, Appearance(9))),
            (-1, Voxel::AIR),
            (0, stone),
            (6, stone),
        ] {
            world
                .apply_actions(&[EditAction::Set(Edit::set(CellPos::new(-33, y, -1), value))])
                .unwrap();
        }
        let before_reset = world.read_snapshot();
        world
            .apply_actions(&[EditAction::Reset {
                pos: CellPos::new(-33, -1, -1),
                expected: Some(Voxel::AIR),
            }])
            .unwrap();
        clear(&calls);
        let runs = world
            .read_snapshot()
            .column_runs(-33, -1, -32..7)
            .unwrap()
            .unwrap();
        assert_eq!(calls.voxels.load(Ordering::Relaxed), 0);
        assert!(calls.runs.load(Ordering::Relaxed) <= 5);
        assert_eq!(runs.first().unwrap().start, -32);
        assert_eq!(runs.last().unwrap().end, 7);
        let truncated = world
            .read_snapshot()
            .column_runs(-33, -1, -32..6)
            .unwrap()
            .unwrap();
        assert_eq!(
            truncated.last(),
            Some(&Run {
                start: 2,
                end: 6,
                material: MaterialId::AIR
            })
        );
        assert!(runs
            .windows(2)
            .all(|w| w[0].end == w[1].start && w[0].material != w[1].material));
        for y in -32..7 {
            let run = runs.iter().find(|r| r.start <= y && y < r.end).unwrap();
            assert_eq!(
                run.material,
                world
                    .voxel_at(CellPos::new(-33, y, -1))
                    .known()
                    .unwrap()
                    .material
            );
        }
        assert_eq!(
            before_reset.voxel_at(CellPos::new(-33, -1, -1)).known(),
            Some(Voxel::AIR)
        );
        assert_eq!(
            world.read_snapshot().column_runs(-33, -1, 7..8).unwrap(),
            None
        );
    }
    #[test]
    fn unknown_gaps_remain_unknown_but_explicit_air_masks_source() {
        let calls = Arc::new(Calls::default());
        let generator = Arc::new(Analytic {
            calls: calls.clone(),
        });
        let mut edits = EditStore::default();
        edits.set(CellPos::new(-1, 7, -1), Some(Voxel::AIR));
        let snapshot = WorldSnapshot::new(generator, edits, 1);
        assert_eq!(
            snapshot.column_runs(-1, -1, 6..9).unwrap(),
            Some(vec![Run {
                start: 6,
                end: 9,
                material: MaterialId::AIR
            }])
        );
        assert_eq!(
            snapshot.column_runs(-1, -1, 7..8).unwrap(),
            Some(vec![Run {
                start: 7,
                end: 8,
                material: MaterialId::AIR
            }])
        );
        assert_eq!(snapshot.column_runs(0, -1, 6..9).unwrap(), None);
        assert_eq!(calls.voxels.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn bounded_queries_and_bulk_appearance_keep_their_contract() {
        let (world, calls) = source();
        let snapshot = world.read_snapshot();
        assert_eq!(
            snapshot.column_runs(0, 0, i64::MAX..i64::MAX).unwrap(),
            Some(vec![])
        );
        assert_eq!(
            snapshot.column_runs(0, 0, 1..0),
            Err(QueryError::InvalidBounds)
        );
        assert_eq!(
            snapshot.column_runs(0, 0, i64::MIN..i64::MAX),
            Err(QueryError::InvalidBounds)
        );
        assert_eq!(
            snapshot.column_runs(0, 0, 0..MAX_BULK_CELLS as i64 + 1),
            Err(QueryError::TooLarge)
        );
        assert_eq!(calls.runs.load(Ordering::Relaxed), 0);
        let bulk = snapshot
            .fill_voxels(Bounds {
                min: CellPos::new(0, -1, 0),
                max: CellPos::new(1, 0, 1),
            })
            .unwrap();
        assert_eq!(
            bulk,
            vec![Lookup::Known(Voxel::new(
                MaterialId::SOIL,
                Appearance(0x1234)
            ))]
        );
    }
}
