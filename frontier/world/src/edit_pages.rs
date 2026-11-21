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
}
