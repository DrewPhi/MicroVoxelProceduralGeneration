//! Ordered terrain overrides with copy-on-write leaves. A renderer snapshot
//! shares the directory and pages. The next edit copies the small directory
//! and touched leaves while retaining the exact Cell order used in world saves.
use super::{Cell, Material};
use std::{collections::BTreeMap, sync::Arc};

const LEAF: usize = 256;
const FIRST: Cell = Cell {
    x: i64::MIN,
    y: i64::MIN,
    z: i64::MIN,
};
type Leaf = Vec<(Cell, Material)>;

#[derive(Clone, Debug, Default)]
pub(super) struct EditPages {
    pages: BTreeMap<Cell, Arc<Leaf>>,
    bounds: Option<[[i64; 3]; 2]>,
}

impl EditPages {
    fn include(&mut self, cell: Cell) {
        let q = [cell.x, cell.y, cell.z];
        let [lo, hi] = self.bounds.get_or_insert([q, q]);
        for axis in 0..3 {
            lo[axis] = lo[axis].min(q[axis]);
            hi[axis] = hi[axis].max(q[axis]);
        }
    }
    pub(super) fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }
    pub(super) fn from_sorted(values: impl IntoIterator<Item = (Cell, Material)>) -> Self {
        let mut out = Self::default();
        let mut items = values.into_iter();
        loop {
            let page: Leaf = items.by_ref().take(LEAF).collect();
            if page.is_empty() {
                break;
            }
            for &(cell, _) in &page {
                out.include(cell);
            }
            let key = if out.pages.is_empty() {
                FIRST
            } else {
                page[0].0
            };
            out.pages.insert(key, Arc::new(page));
        }
        out
    }

    pub(super) fn get(&self, cell: &Cell) -> Option<&Material> {
        let [lo, hi] = self.bounds?;
        if cell.x < lo[0]
            || cell.x > hi[0]
            || cell.y < lo[1]
            || cell.y > hi[1]
            || cell.z < lo[2]
            || cell.z > hi[2]
        {
            return None;
        }
        let (_, page) = self.pages.range(..=cell).next_back()?;
        let at = page.binary_search_by_key(cell, |v| v.0).ok()?;
        Some(&page[at].1)
    }

    pub(super) fn contains_key(&self, cell: &Cell) -> bool {
        self.get(cell).is_some()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (&Cell, &Material)> {
        self.pages
            .values()
            .flat_map(|page| page.iter().map(|(c, m)| (c, m)))
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = &Cell> {
        self.iter().map(|(c, _)| c)
    }

    pub(super) fn cells_in_columns(
        &self,
        min: [i64; 2],
        max: [i64; 2],
        limit: usize,
    ) -> Option<Vec<Cell>> {
        let mut cells = Vec::new();
        self.visit_column_spans(min, max, |span, all_z| {
            if all_z {
                if span.len() > limit - cells.len() {
                    return None;
                }
                cells.extend(span.iter().map(|&(cell, _)| cell));
            } else {
                for &(cell, _) in span {
                    if (min[1]..max[1]).contains(&cell.z) {
                        if cells.len() == limit {
                            return None;
                        }
                        cells.push(cell);
                    }
                }
            }
            Some(())
        })?;
        Some(cells)
    }

    pub(super) fn run_ends_in_columns(
        &self,
        min: [i64; 2],
        max: [i64; 2],
        limit: usize,
    ) -> Option<Vec<Cell>> {
        struct Run {
            material: Material,
            last_y: i64,
            end_index: Option<usize>,
        }
        let mut cells = Vec::new();
        let mut x = None;
        let mut runs: BTreeMap<i64, Run> = BTreeMap::new();
        self.visit_column_spans(min, max, |span, all_z| {
            for &(cell, material) in span {
                if !all_z && !(min[1]..max[1]).contains(&cell.z) {
                    continue;
                }
                if x != Some(cell.x) {
                    x = Some(cell.x);
                    runs.clear();
                }
                // Save order is X, Y, Z. Each Z within this X slice arrives
                // in increasing Y even when its run spans several COW leaves.
                if let Some(run) = runs.get_mut(&cell.z).filter(|run| {
                    run.material == material && run.last_y.checked_add(1) == Some(cell.y)
                }) {
                    run.last_y = cell.y;
                    if let Some(i) = run.end_index {
                        cells[i] = cell;
                        continue;
                    }
                    run.end_index = Some(cells.len());
                } else {
                    runs.insert(
                        cell.z,
                        Run {
                            material,
                            last_y: cell.y,
                            end_index: None,
                        },
                    );
                }
                if cells.len() == limit {
                    return None;
                }
                cells.push(cell);
            }
            Some(())
        })?;
        Some(cells)
    }

    fn visit_column_spans(
        &self,
        min: [i64; 2],
        max: [i64; 2],
        mut visit: impl FnMut(&[(Cell, Material)], bool) -> Option<()>,
    ) -> Option<()> {
        let Some([lo, hi]) = self.bounds else {
            return Some(());
        };
        if [0, 2]
            .into_iter()
            .enumerate()
            .any(|(i, axis)| min[i] >= max[i] || max[i] <= lo[axis] || min[i] > hi[axis])
        {
            return Some(());
        }
        // The directory and each leaf are in (x, y, z) order. Include the
        // predecessor leaf because it can straddle the first requested X.
        let first_at = Cell { x: min[0], ..FIRST };
        let last_at = Cell { x: max[0], ..FIRST };
        let (&first, _) = self.pages.range(..=first_at).next_back()?;
        let all_z = min[1] <= lo[2] && max[1] > hi[2];
        for (_, leaf) in self.pages.range(first..last_at) {
            let start = leaf.partition_point(|&(cell, _)| cell.x < min[0]);
            let end = leaf.partition_point(|&(cell, _)| cell.x < max[0]);
            visit(&leaf[start..end], all_z)?;
        }
        Some(())
    }

    // World::set_batch has already ordered and deduplicated the writes.
    pub(super) fn extend(&mut self, values: Vec<(Cell, Material)>) {
        if self.pages.is_empty() {
            *self = Self::from_sorted(values);
            return;
        }
        for (cell, material) in values {
            self.include(cell);
            let (_, leaf) = self
                .pages
                .range_mut(..=cell)
                .next_back()
                .expect("first leaf covers every cell");
            let page = Arc::make_mut(leaf);
            match page.binary_search_by_key(&cell, |v| v.0) {
                Ok(i) => page[i].1 = material,
                Err(i) if page.len() < LEAF => page.insert(i, (cell, material)),
                Err(i) => {
                    let mut right = page.split_off(LEAF / 2);
                    if i < LEAF / 2 {
                        page.insert(i, (cell, material));
                    } else {
                        right.insert(i - LEAF / 2, (cell, material));
                    }
                    self.pages.insert(right[0].0, Arc::new(right));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn branched_scattered_updates_preserve_order_and_share_untouched_leaves() {
        let cell = |x| Cell { x, y: -2, z: 3 };
        let init = (0..(LEAF * 4) as i64).map(|i| (cell(2 * i), Material::Wood));
        let mut expected: BTreeMap<_, _> = init.clone().collect();
        let baseline = EditPages::from_sorted(init);
        let mut edited = baseline.clone();
        let untouched = baseline.pages.values().nth(2).unwrap().clone();
        let writes = [
            (FIRST, Material::Air),
            (cell(256), Material::Stone),
            (cell(511), Material::Wood),
            (cell(513), Material::Grass),
            (cell(i64::MAX), Material::Soil),
        ];
        for (c, m) in writes {
            edited.extend(vec![(c, m)]);
            expected.insert(c, m);
        }
        assert!(edited.iter().eq(expected.iter()));
        assert!(edited.pages.values().any(|p| Arc::ptr_eq(p, &untouched)));
        assert_eq!(baseline.get(&cell(256)), Some(&Material::Wood));
        for (c, m) in &expected {
            assert_eq!(edited.get(c), Some(m));
        }
        assert!(edited.pages.values().all(|p| p.len() <= LEAF));
        assert_eq!(baseline.get(&FIRST), None);
    }

    #[test]
    fn bounded_column_reads_span_pages_and_include_extreme_heights() {
        let pages = EditPages::from_sorted((-30..30).flat_map(|x| {
            [i64::MIN, 0, i64::MAX]
                .into_iter()
                .flat_map(move |y| (-3..4).map(move |z| (Cell { x, y, z }, Material::Air)))
        }));
        for (lo, hi) in [
            ([-40, -10], [40, 10]),
            ([-29, -2], [19, 3]),
            ([-1, 0], [0, 1]),
            ([29, 3], [30, 4]),
            ([0, 0], [-3, 10]),
            ([i64::MIN; 2], [-30, 10]),
            ([30, 0], [i64::MAX; 2]),
        ] {
            let exact: Vec<_> = pages
                .keys()
                .copied()
                .filter(|c| (lo[0]..hi[0]).contains(&c.x) && (lo[1]..hi[1]).contains(&c.z))
                .collect();
            assert_eq!(
                pages.cells_in_columns(lo, hi, exact.len()),
                Some(exact.clone())
            );
            if !exact.is_empty() {
                assert_eq!(pages.cells_in_columns(lo, hi, exact.len() - 1), None);
            }
        }
    }
    #[test]
    fn run_ends_keep_gaps_and_material_changes_across_leaf_boundaries() {
        let mut edits = BTreeMap::new();
        for x in -3..4 {
            for z in -5..6 {
                for y in (-300..=290).chain([i64::MIN, i64::MAX]) {
                    if y == z || y == z + 1 {
                        continue;
                    }
                    let m = if y < 42 {
                        Material::Air
                    } else {
                        Material::Stone
                    };
                    edits.insert(Cell { x, y, z }, m);
                }
            }
        }
        let pages = EditPages::from_sorted(edits.iter().map(|(&c, &m)| (c, m)));
        for (low, high) in [([-3, -5], [4, 6]), ([-2, -3], [2, 3]), ([5, 8], [7, 9])] {
            let expected: Vec<_> = edits
                .iter()
                .filter_map(|(&c, &m)| {
                    let at = |y: Option<i64>| y.and_then(|y| edits.get(&Cell { y, ..c })).copied();
                    ((low[0]..high[0]).contains(&c.x)
                        && (low[1]..high[1]).contains(&c.z)
                        && (at(c.y.checked_sub(1)) != Some(m) || at(c.y.checked_add(1)) != Some(m)))
                    .then_some(c)
                })
                .collect();
            let mut actual = pages
                .run_ends_in_columns(low, high, expected.len())
                .unwrap();
            actual.sort_unstable();
            assert_eq!(actual, expected);
            if !expected.is_empty() {
                assert!(pages
                    .run_ends_in_columns(low, high, expected.len() - 1)
                    .is_none());
            }
        }
    }
}
