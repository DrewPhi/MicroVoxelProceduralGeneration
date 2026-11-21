//! Disposable spatial candidates shared across one source lineage. Bits only
//! accumulate, including divergent snapshot branches; canonical maps always
//! decide membership. Exhausting the budget disables acceleration, never history.
use crate::Cell;
use std::collections::BTreeMap;

pub(crate) const INDEX_BUDGET: usize = 32 * 1024 * 1024;
const FIXED_OVERHEAD: usize = 4096;
// 512 payload bytes plus a conservative allowance for tree nodes/keys/allocator
// overhead. The root/node slack is additionally covered by FIXED_OVERHEAD.
const BUCKET_BUDGET: usize = 1024;
type Bucket = [i64; 3];
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EditQueryStats {
    pub index_complete: bool,
    pub buckets_examined: usize,
    pub words_examined: usize,
    pub candidate_cells: usize,
    pub set_bits_examined: usize,
    pub canonical_lookups: usize,
    pub full_scan_cells: usize,
}
#[derive(Debug)]
pub(crate) struct EditIndex {
    buckets: BTreeMap<Bucket, Box<[u64; 64]>>,
    extent: Option<[Bucket; 2]>,
    complete: bool,
    budget: usize,
}
impl Default for EditIndex {
    fn default() -> Self {
        Self::with_budget(INDEX_BUDGET)
    }
}
impl EditIndex {
    pub(crate) fn with_budget(budget: usize) -> Self {
        Self {
            buckets: BTreeMap::new(),
            extent: None,
            complete: true,
            budget,
        }
    }
    pub(crate) fn record(&mut self, cells: &[Cell]) {
        if !self.complete {
            return;
        }
        for &c in cells {
            let p = [c.x, c.y, c.z];
            let bucket = p.map(|x| x.div_euclid(16));
            if !self.buckets.contains_key(&bucket) {
                if self.buckets.len() >= self.budget.saturating_sub(FIXED_OVERHEAD) / BUCKET_BUDGET
                {
                    self.complete = false;
                    self.buckets.clear();
                    self.extent = None;
                    return;
                }
                self.buckets.insert(bucket, Box::new([0; 64]));
                if let Some(bounds) = &mut self.extent {
                    for a in 0..3 {
                        bounds[0][a] = bounds[0][a].min(bucket[a]);
                        bounds[1][a] = bounds[1][a].max(bucket[a]);
                    }
                } else {
                    self.extent = Some([bucket, bucket]);
                }
            }
            let local = p.map(|x| x.rem_euclid(16) as usize);
            let bit = local[0] + 16 * (local[1] + 16 * local[2]);
            self.buckets.get_mut(&bucket).unwrap()[bit / 64] |= 1 << (bit % 64);
        }
    }
    pub(crate) fn upper_y(&self, min: [i64; 2], max: [i64; 2], snapshot_max: i64) -> Option<i64> {
        if (0..2).any(|a| min[a] >= max[a]) {
            return None;
        }
        if !self.complete {
            return Some(snapshot_max);
        }
        let [first, last] = self.extent?;
        let lo = min.map(|v| v.div_euclid(16));
        let hi = max.map(|v| (v - 1).div_euclid(16));
        if lo[0] > last[0] || lo[1] > last[2] || hi[0] < first[0] || hi[1] < first[2] {
            return None;
        }
        let y = if lo[0] <= first[0] && lo[1] <= first[2] && hi[0] >= last[0] && hi[1] >= last[2] {
            last[1]
        } else {
            self.buckets
                .range([lo[0], i64::MIN, i64::MIN]..=[hi[0], i64::MAX, i64::MAX])
                .filter(|(p, _)| (lo[1]..=hi[1]).contains(&p[2]))
                .map(|(p, _)| p[1])
                .max()?
        };
        Some(snapshot_max.min(y * 16 + 15))
    }
    /// i128 upper bounds permit an all-height query including i64::MAX, while
    /// the public 3-D API retains ordinary half-open i64 bounds.
    #[cfg(test)]
    pub(crate) fn candidates(
        &self,
        min: [i128; 3],
        max: [i128; 3],
    ) -> (Option<Vec<Cell>>, EditQueryStats) {
        self.query(min, max, |_| true)
    }
    pub(crate) fn query(
        &self,
        min: [i128; 3],
        max: [i128; 3],
        mut accepts: impl FnMut(Cell) -> bool,
    ) -> (Option<Vec<Cell>>, EditQueryStats) {
        let mut stats = EditQueryStats {
            index_complete: self.complete,
            ..Default::default()
        };
        let min = min.map(|v| v.max(i64::MIN as i128));
        let max = max.map(|v| v.min(i64::MAX as i128 + 1));
        if (0..3).any(|a| min[a] >= max[a]) {
            return (Some(vec![]), stats);
        }
        if !self.complete {
            return (None, stats);
        }
        let Some(extent) = self.extent else {
            return (Some(vec![]), stats);
        };
        let lo: [i64; 3] =
            std::array::from_fn(|a| (min[a].div_euclid(16) as i64).max(extent[0][a]));
        let hi: [i64; 3] =
            std::array::from_fn(|a| ((max[a] - 1).div_euclid(16) as i64).min(extent[1][a]));
        if (0..3).any(|a| lo[a] > hi[a]) {
            return (Some(vec![]), stats);
        }
        let requested = (0..3)
            .try_fold(1u128, |count, a| {
                count.checked_mul((hi[a] as i128 - lo[a] as i128 + 1) as u128)
            })
            .unwrap_or(u128::MAX);
        let mut cells = Vec::new();
        let mut visit = |bucket: Bucket, words: &[u64; 64]| {
            for (word_index, &word) in words.iter().enumerate() {
                stats.words_examined += 1;
                let mut bits = word;
                while bits != 0 {
                    stats.set_bits_examined += 1;
                    let bit = word_index * 64 + bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    let local = [bit % 16, (bit / 16) % 16, bit / 256];
                    let p: [i128; 3] =
                        std::array::from_fn(|a| bucket[a] as i128 * 16 + local[a] as i128);
                    if (0..3).all(|a| p[a] >= min[a] && p[a] < max[a]) {
                        let cell = Cell {
                            x: p[0] as i64,
                            y: p[1] as i64,
                            z: p[2] as i64,
                        };
                        stats.candidate_cells += 1;
                        stats.canonical_lookups += 1;
                        if accepts(cell) {
                            cells.push(cell);
                        }
                    }
                }
            }
        };
        if requested <= self.buckets.len() as u128 {
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        stats.buckets_examined += 1;
                        let bucket = [x, y, z];
                        if let Some(words) = self.buckets.get(&bucket) {
                            visit(bucket, words);
                        }
                    }
                }
            }
        } else {
            for (&bucket, words) in &self.buckets {
                stats.buckets_examined += 1;
                if (0..3).all(|a| bucket[a] >= lo[a] && bucket[a] <= hi[a]) {
                    visit(bucket, words);
                }
            }
        }
        (Some(cells), stats)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ceiling_covers_exact_negative_extreme_and_unindexed_overrides() {
        let mut index = EditIndex::default();
        let rows = [
            Cell {
                x: -17,
                y: 43,
                z: 5,
            },
            Cell {
                x: 37,
                y: 80,
                z: -100,
            },
            Cell {
                x: 8,
                y: i64::MAX,
                z: 8,
            },
        ];
        index.record(&rows);
        assert_eq!(index.upper_y([-20, 0], [-16, 16], i64::MAX), Some(47));
        assert_eq!(index.upper_y([0, 0], [16, 16], i64::MAX), Some(i64::MAX));
        assert_eq!(index.upper_y([0, 0], [16, 16], 10), Some(10));
        assert_eq!(index.upper_y([17, 17], [22, 22], 100), None);
        assert_eq!(index.upper_y([37, -100], [37, -90], 100), None);
        let mut tiny = EditIndex::with_budget(0);
        tiny.record(&rows);
        assert_eq!(tiny.upper_y([17, 17], [22, 22], 87), Some(87));
    }
    #[test]
    fn negative_half_open_and_extreme_bounds_are_exact() {
        let cells = [
            Cell {
                x: -17,
                y: -1,
                z: -16,
            },
            Cell { x: -16, y: 0, z: 0 },
            Cell {
                x: i64::MIN,
                y: i64::MAX,
                z: 0,
            },
            Cell {
                x: i64::MAX,
                y: i64::MIN,
                z: 0,
            },
        ];
        let mut index = EditIndex::default();
        index.record(&cells);
        for (min, max) in [
            ([-17, -1, -16], [-16, 0, -15]),
            ([i64::MIN; 3], [i64::MAX; 3]),
        ] {
            let (actual, _) = index.candidates(min.map(i128::from), max.map(i128::from));
            let mut actual = actual.unwrap();
            actual.sort_unstable();
            let mut expected: Vec<_> = cells
                .into_iter()
                .filter(|c| {
                    [c.x, c.y, c.z]
                        .into_iter()
                        .enumerate()
                        .all(|(a, v)| v >= min[a] && v < max[a])
                })
                .collect();
            expected.sort_unstable();
            assert_eq!(actual, expected);
        }
        let (actual, _) = index.candidates([i64::MIN as i128; 3], [i64::MAX as i128 + 1; 3]);
        assert_eq!(actual.unwrap().len(), 4);
    }
    #[test]
    fn budget_exhaustion_disables_candidates_without_unbounded_growth() {
        let mut index = EditIndex::with_budget(FIXED_OVERHEAD + BUCKET_BUDGET);
        index.record(&[Cell { x: 0, y: 0, z: 0 }]);
        assert!(index.complete);
        index.record(&[Cell { x: 16, y: 0, z: 0 }]);
        assert!(!index.complete);
        assert!(index.buckets.is_empty());
        assert!(index.candidates([-100; 3], [100; 3]).0.is_none());
    }
    fn oracle(world: &crate::World, min: Cell, max: Cell) -> Vec<Cell> {
        let mut cells: Vec<_> = world
            .edit_cells()
            .filter(|c| {
                c.x >= min.x
                    && c.x < max.x
                    && c.y >= min.y
                    && c.y < max.y
                    && c.z >= min.z
                    && c.z < max.z
            })
            .collect();
        cells.sort_unstable();
        cells.dedup();
        cells
    }
    #[test]
    fn shared_branch_candidates_reverts_hidden_fluid_and_bulk_restore_match_full_scan() {
        use crate::{Material, World};
        let base = World::new(7);
        let a = Cell {
            x: -17,
            y: 16000,
            z: -33,
        };
        let b = Cell {
            x: 17,
            y: 16001,
            z: 33,
        };
        let mut left = base.clone();
        let mut right = base.clone();
        left.set(a, Material::Stone);
        right.set(b, Material::Wood);
        let wet = Cell {
            x: -16,
            y: 16002,
            z: -32,
        };
        left.set_fluid_occupancy([(wet, true)]);
        left.set(wet, Material::Stone);
        let before = left.revision();
        left.set_fluid_occupancy([(wet, false)]);
        assert_eq!(
            left.revision(),
            before,
            "hidden tombstone does not change visible revision"
        );
        // Reverting a snapshot still shares candidate union from both branches.
        for world in [&base, &left, &right] {
            for (min, max) in [
                (
                    Cell {
                        x: -32,
                        y: 15999,
                        z: -48,
                    },
                    Cell {
                        x: 32,
                        y: 16003,
                        z: 48,
                    },
                ),
                (
                    a,
                    Cell {
                        x: a.x + 1,
                        y: a.y + 1,
                        z: a.z + 1,
                    },
                ),
                (Cell { x: 0, y: 0, z: 0 }, Cell { x: 0, y: 0, z: 0 }),
            ] {
                assert_eq!(
                    world.edit_cells_in_bounds(min, max),
                    oracle(world, min, max)
                );
            }
        }
        let min = Cell {
            x: -100,
            y: 15900,
            z: -100,
        };
        let max = Cell {
            x: 100,
            y: 16100,
            z: 100,
        };
        let (cells, stats) = left.edit_cells_in_bounds_with_stats(min, max);
        assert_eq!(cells, vec![a, wet]);
        assert!(stats.index_complete);
        assert_eq!(left.edit_cells_in_columns([-100, -100], [100, 100]), cells);
        let mut restored = World::new(7);
        restored.set_batch(left.terrain_edits());
        restored.set_fluid_occupancy(left.fluid.iter().map(|(&c, &wet)| (c, wet)));
        assert_eq!(
            restored.edit_cells_in_bounds(min, max),
            oracle(&restored, min, max)
        );
        assert_eq!(restored.material(wet), left.material(wet));
    }
    #[test]
    fn index_capacity_fallback_preserves_exact_snapshot_membership() {
        use crate::{Material, World};
        let mut world = World::new(7);
        world.edit_index = std::sync::Arc::new(std::sync::Mutex::new(EditIndex::with_budget(
            FIXED_OVERHEAD + BUCKET_BUDGET,
        )));
        world.set_batch([
            (
                Cell {
                    x: -17,
                    y: 16000,
                    z: 0,
                },
                Material::Stone,
            ),
            (
                Cell {
                    x: 17,
                    y: 16000,
                    z: 0,
                },
                Material::Wood,
            ),
        ]);
        let min = Cell {
            x: -100,
            y: 15900,
            z: -100,
        };
        let max = Cell {
            x: 100,
            y: 16100,
            z: 100,
        };
        let (cells, stats) = world.edit_cells_in_bounds_with_stats(min, max);
        assert_eq!(cells, oracle(&world, min, max));
        assert!(!stats.index_complete);
        assert_eq!(stats.full_scan_cells, 2);
        assert_eq!(stats.canonical_lookups, 0);
    }
    #[test]
    #[ignore = "standalone synthetic million-override query count/timing diagnostic"]
    fn million_override_local_query_matches_full_scan_with_bounded_candidate_work() {
        use crate::{Material, World};
        use std::time::Instant;
        let mut world = World::new(7);
        let start = Instant::now();
        world.set_batch((0..1_000_000i64).map(|i| {
            (
                Cell {
                    x: i % 1000 - 500,
                    y: 16000,
                    z: i / 1000 - 500,
                },
                Material::Stone,
            )
        }));
        let build_ms = start.elapsed().as_secs_f64() * 1000.;
        let min = Cell {
            x: -2,
            y: 15999,
            z: -2,
        };
        let max = Cell {
            x: 2,
            y: 16001,
            z: 2,
        };
        let expected = oracle(&world, min, max);
        let start = Instant::now();
        let (actual, stats) = world.edit_cells_in_bounds_with_stats(min, max);
        let index_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let full = oracle(&world, min, max);
        let full_ms = start.elapsed().as_secs_f64() * 1000.;
        assert_eq!(actual, expected);
        assert_eq!(actual, full);
        assert_eq!(actual.len(), 16);
        assert!(stats.index_complete);
        assert!(stats.buckets_examined <= 8);
        assert!(stats.canonical_lookups <= 16);
        assert_eq!(stats.full_scan_cells, 0);
        eprintln!("million_override_query history={} build_ms={build_ms:.3} index_ms={index_ms:.6} full_ms={full_ms:.6} stats={stats:?}",world.edit_cells().count());
    }
}
