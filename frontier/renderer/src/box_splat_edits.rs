//! Sparse edit publication for the bounded fixture. The canonical World is
//! authoritative; this stores only the visible-surface records being rendered.
use crate::brick_ray::{material_id, EXPOSED_FACE_MASK};
use frontier_world::{Cell, Material, World};
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;

pub(super) type Splat = [u32; 4];
type Key = [u32; 3];

// Copy up to two unchanged records between writes to reduce queue call count.
const WRITE_GAP: usize = 2;

pub(super) struct EditableSplats {
    pub(super) records: Vec<Splat>,
    pub(super) capacity: usize,
    origin: Cell,
    size: [usize; 3],
    slots: HashMap<Key, usize>,
    revision: Option<u64>,
    exposed_faces: usize,
}

pub(super) struct Uploads {
    pub(super) ranges: Vec<Range<usize>>,
    pub(super) logical_writes: usize,
    pub(super) changed_records: usize,
    pub(super) affected_cells: usize,
}

fn offset(cell: Cell, axis: usize, by: i64) -> Cell {
    let mut p = [cell.x, cell.y, cell.z];
    p[axis] += by;
    Cell {
        x: p[0],
        y: p[1],
        z: p[2],
    }
}

fn key([x, y, z, _]: Splat) -> Key {
    [x, y, z]
}

fn face_count(word: u32) -> usize {
    (word & EXPOSED_FACE_MASK).count_ones() as usize
}

impl EditableSplats {
    pub(super) fn new(
        records: Vec<Splat>,
        origin: Cell,
        size: [usize; 3],
        capacity: usize,
    ) -> Self {
        let slots = records
            .iter()
            .copied()
            .enumerate()
            .map(|(i, r)| (key(r), i))
            .collect();
        let exposed_faces = records.iter().map(|r| face_count(r[3])).sum();
        Self {
            records,
            capacity,
            origin,
            size,
            slots,
            revision: None,
            exposed_faces,
        }
    }

    pub(super) fn exposed_faces(&self) -> usize {
        self.exposed_faces
    }

    pub(super) fn slot_capacity(&self) -> usize {
        self.slots.capacity()
    }

    fn key_of(&self, cell: Cell) -> Option<Key> {
        let relative = [
            cell.x - self.origin.x,
            cell.y - self.origin.y,
            cell.z - self.origin.z,
        ];
        relative
            .iter()
            .zip(self.size)
            .all(|(&p, n)| (0..n as i64).contains(&p))
            .then(|| relative.map(|p| p as u32))
    }

    fn cell_of(&self, key: Key) -> Cell {
        Cell {
            x: self.origin.x + i64::from(key[0]),
            y: self.origin.y + i64::from(key[1]),
            z: self.origin.z + i64::from(key[2]),
        }
    }

    fn affected_cells(&self, old: &World, new: &World) -> BTreeSet<Key> {
        // An edit just outside the owned volume can change an internal face.
        // Both bounds include exactly one outside cell on each side.
        let low = Cell {
            x: self.origin.x - 1,
            y: self.origin.y - 1,
            z: self.origin.z - 1,
        };
        let high = Cell {
            x: self.origin.x + self.size[0] as i64 + 1,
            y: self.origin.y + self.size[1] as i64 + 1,
            z: self.origin.z + self.size[2] as i64 + 1,
        };
        let mut candidates = old.edit_cells_in_bounds(low, high);
        candidates.extend(new.edit_cells_in_bounds(low, high));
        candidates.sort_unstable();
        candidates.dedup();
        let mut affected = BTreeSet::new();
        for cell in candidates {
            if old.material(cell) == new.material(cell) {
                continue;
            }
            affected.extend(self.key_of(cell));
            for axis in 0..3 {
                affected.extend(self.key_of(offset(cell, axis, -1)));
                affected.extend(self.key_of(offset(cell, axis, 1)));
            }
        }
        affected
    }

    fn new_word(&self, world: &World, key: Key) -> u32 {
        let cell = self.cell_of(key);
        let material = world.material(cell);
        if material == Material::Air {
            return 0;
        }
        let mut faces = 0;
        for axis in 0..3 {
            for (positive, sign) in [(false, -1), (true, 1)] {
                if world.material(offset(cell, axis, sign)) == Material::Air {
                    faces |= 1 << (8 + axis * 2 + usize::from(positive));
                }
            }
        }
        if faces == 0 {
            0
        } else {
            faces | u32::from(material_id(material))
        }
    }

    /// Returns None before any record, slot map, revision, or exposure count
    /// changes. Delete first so mixed edits never need transient extra slots.
    pub(super) fn apply(&mut self, old: &World, new: &World) -> Option<Uploads> {
        if !old.shares_source_lineage(new)
            || old.fbm_terrain().is_some()
            || new.fbm_terrain().is_some()
            || self.revision.is_some_and(|value| value != old.revision())
        {
            return None;
        }
        let affected = self.affected_cells(old, new);
        let mut removals = Vec::new();
        let mut writes = Vec::new();
        let mut additions = 0;
        let mut exposed_faces = self.exposed_faces;
        for key in affected.iter().copied() {
            let slot = self.slots.get(&key).copied();
            let before = slot.map_or(0, |i| self.records[i][3]);
            let after = self.new_word(new, key);
            if before == after {
                continue;
            }
            exposed_faces = exposed_faces - face_count(before) + face_count(after);
            if after == 0 {
                removals.push(key);
            } else {
                writes.push([key[0], key[1], key[2], after]);
                additions += usize::from(slot.is_none());
            }
        }
        let final_len = self.records.len() - removals.len() + additions;
        if final_len > self.capacity {
            return None;
        }
        let changed_records = removals.len() + writes.len();
        let mut touched = Vec::with_capacity(changed_records);
        for removed_key in removals {
            let i = self.slots.remove(&removed_key).unwrap();
            self.records.swap_remove(i);
            if let Some(&moved) = self.records.get(i) {
                self.slots.insert(key(moved), i);
                touched.push(i);
            }
        }
        for record in writes {
            let key = key(record);
            if let Some(&i) = self.slots.get(&key) {
                self.records[i] = record;
                touched.push(i);
            } else {
                let i = self.records.len();
                self.slots.insert(key, i);
                self.records.push(record);
                touched.push(i);
            }
        }
        debug_assert_eq!(self.records.len(), final_len);
        self.exposed_faces = exposed_faces;
        self.revision = Some(new.revision());

        touched.retain(|&i| i < final_len);
        touched.sort_unstable();
        touched.dedup();
        let mut ranges: Vec<Range<usize>> = Vec::new();
        for &i in &touched {
            if let Some(range) = ranges.last_mut().filter(|r| i <= r.end + WRITE_GAP) {
                range.end = i + 1;
            } else {
                ranges.push(i..i + 1);
            }
        }
        Some(Uploads {
            ranges,
            logical_writes: touched.len(),
            changed_records,
            affected_cells: affected.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brick_ray::BrickSnapshot;

    const ORIGIN: Cell = Cell {
        x: -8,
        y: 1024,
        z: -8,
    };
    const SIZE: [usize; 3] = [17, 16, 17];

    fn records(world: &World) -> Vec<Splat> {
        BrickSnapshot::build(|p| world.material(p), ORIGIN, SIZE).collect_surface_cells()
    }

    #[test]
    fn incremental_fixture_splats_match_full_rebuild_and_patches() {
        let edge = ORIGIN;
        let outside = offset(edge, 0, -1);
        let middle = offset(edge, 0, 8);
        let east = offset(middle, 2, 1);
        let west = offset(middle, 2, -1);
        let mut old = World::new(7);
        old.set_batch([
            (edge, Material::Wood),
            (middle, Material::Stone),
            (east, Material::Wood),
        ]);
        let mut updated = old.clone();
        updated.set_batch([
            (outside, Material::Stone),
            (middle, Material::Air),
            (east, Material::Soil),
            (west, Material::Leaf),
        ]);
        let mut editable = EditableSplats::new(records(&old), ORIGIN, SIZE, 16);
        let mut mirror = editable.records.clone();
        mirror.resize(16, [0; 4]);
        for (prior, next) in [(&old, &updated), (&updated, &old)] {
            let uploads = editable.apply(prior, next).unwrap();
            assert!(uploads.logical_writes > 0);
            for range in uploads.ranges {
                mirror[range.clone()].copy_from_slice(&editable.records[range]);
            }
            assert_eq!(mirror[..editable.records.len()], editable.records);
            let mut sorted = editable.records.clone();
            sorted.sort_unstable();
            let mut oracle = records(next);
            oracle.sort_unstable();
            assert_eq!(sorted, oracle);
            assert_eq!(
                editable.exposed_faces(),
                oracle.iter().map(|r| face_count(r[3])).sum()
            );
            for (i, &record) in editable.records.iter().enumerate() {
                assert_eq!(editable.slots.get(&key(record)), Some(&i));
            }
        }
    }

    #[test]
    fn incremental_fixture_capacity_fallback_preserves_state() {
        let mut old = World::new(7);
        old.set(ORIGIN, Material::Wood);
        let mut updated = old.clone();
        updated.set(offset(ORIGIN, 0, 3), Material::Stone);
        let initial = records(&old);
        let mut editable = EditableSplats::new(initial.clone(), ORIGIN, SIZE, initial.len());
        assert!(editable.apply(&old, &updated).is_none());
        assert_eq!(editable.records, initial);
        assert_eq!(editable.slots.len(), initial.len());
        assert_eq!(editable.exposed_faces(), 6);
        assert_eq!(editable.revision, None);
        assert!(editable.apply(&World::new(8), &old).is_none());
    }
}
