//! Bounded, generation-checked suballocation of a single GPU buffer.
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Handle {
    pub slot: u32,
    pub generation: u32,
    pub offset: u64,
    pub size: u64,
}

#[derive(Default)]
struct Slot {
    generation: u32,
    live: bool,
    offset: u64,
    size: u64,
}

pub struct Arena {
    capacity: u64,
    free: BTreeMap<u64, u64>,
    slots: Vec<Slot>,
    reusable: Vec<u32>,
    live_bytes: u64,
    pending: Vec<(u64, Handle)>,
}
impl Arena {
    pub fn new(capacity: u64) -> Self {
        let mut free = BTreeMap::new();
        free.insert(0, capacity);
        Self {
            capacity,
            free,
            slots: Vec::new(),
            reusable: Vec::new(),
            live_bytes: 0,
            pending: Vec::new(),
        }
    }
    pub fn live_bytes(&self) -> u64 {
        self.live_bytes
    }
    pub fn pending_bytes(&self) -> u64 {
        self.pending.iter().map(|(_, h)| h.size).sum()
    }
    pub fn capacity(&self) -> u64 {
        self.capacity
    }
    /// Extend the tail without moving allocations or invalidating handles.
    pub fn grow(&mut self, capacity: u64) {
        assert!(capacity >= self.capacity);
        if capacity == self.capacity {
            return;
        }
        let mut start = self.capacity;
        if let Some((&offset, &len)) = self.free.iter().next_back() {
            if offset + len == start {
                start = offset;
                self.free.remove(&offset);
            }
        }
        self.free.insert(start, capacity - start);
        self.capacity = capacity;
    }
    pub fn packed_size(&self, alignment: u64) -> u64 {
        self.slots
            .iter()
            .filter(|s| s.live)
            .fold(0u64, |end, s| end.next_multiple_of(alignment) + s.size)
    }
    /// Repack live slots once deferred frees are complete. Callers copy old ranges
    /// and replace their handles using this mapping before issuing another draw.
    pub fn compact(&mut self, capacity: u64, alignment: u64) -> Vec<(Handle, Handle)> {
        assert!(self.pending.is_empty());
        assert!(capacity >= self.packed_size(alignment));
        let mut end = 0u64;
        let mut moved = Vec::new();
        for (slot, state) in self.slots.iter_mut().enumerate().filter(|(_, s)| s.live) {
            let old = Handle {
                slot: slot as u32,
                generation: state.generation,
                offset: state.offset,
                size: state.size,
            };
            state.offset = end.next_multiple_of(alignment);
            end = state.offset + state.size;
            let new = Handle {
                offset: state.offset,
                ..old
            };
            moved.push((old, new));
        }
        self.free.clear();
        // Preserve alignment gaps as usable space for smaller allocations.
        let mut previous = 0;
        for (_, h) in &moved {
            if h.offset > previous {
                self.free.insert(previous, h.offset - previous);
            }
            previous = h.offset + h.size;
        }
        if end < capacity {
            self.free.insert(end, capacity - end);
        }
        self.capacity = capacity;
        moved
    }
    pub fn allocate(&mut self, bytes: u64, alignment: u64) -> Option<Handle> {
        let size = bytes.max(4).next_multiple_of(4);
        let candidate = self.free.iter().find_map(|(&start, &len)| {
            let aligned = start.checked_add(alignment - 1)? / alignment * alignment;
            (aligned.checked_add(size)? <= start + len).then_some((start, len, aligned))
        })?;
        let (start, len, aligned) = candidate;
        self.free.remove(&start);
        if aligned > start {
            self.free.insert(start, aligned - start);
        }
        let end = aligned + size;
        if end < start + len {
            self.free.insert(end, start + len - end);
        }
        let slot = self.reusable.pop().unwrap_or_else(|| {
            self.slots.push(Slot::default());
            (self.slots.len() - 1) as u32
        });
        let state = &mut self.slots[slot as usize];
        state.live = true;
        state.offset = aligned;
        state.size = size;
        self.live_bytes += size;
        Some(Handle {
            slot,
            generation: state.generation,
            offset: aligned,
            size,
        })
    }
    pub fn contains(&self, h: Handle) -> bool {
        self.slots.get(h.slot as usize).is_some_and(|s| {
            s.live && s.generation == h.generation && s.offset == h.offset && s.size == h.size
        })
    }
    /// The range remains unavailable until every earlier queue submission completes.
    pub fn retire(&mut self, h: Handle, submission: u64) {
        assert!(self.contains(h), "invalid or stale arena handle");
        let s = &mut self.slots[h.slot as usize];
        s.live = false;
        s.generation = s.generation.wrapping_add(1);
        self.live_bytes -= h.size;
        self.pending.push((submission, h));
    }
    pub fn reclaim(&mut self, completed: u64) {
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].0 > completed {
                i += 1;
                continue;
            }
            let (_, h) = self.pending.swap_remove(i);
            self.reusable.push(h.slot);
            let mut start = h.offset;
            let mut end = h.offset + h.size;
            if let Some((&s, &len)) = self.free.range(..start).next_back() {
                if s + len == start {
                    start = s;
                    self.free.remove(&s);
                }
            }
            if let Some((&s, &len)) = self.free.range(end..).next() {
                if s == end {
                    end = s + len;
                    self.free.remove(&s);
                }
            }
            self.free.insert(start, end - start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deferred_reuse_and_generation() {
        let mut a = Arena::new(512);
        let first = a.allocate(200, 256).unwrap();
        assert_eq!(first.offset, 0);
        a.retire(first, 3);
        assert!(!a.contains(first));
        assert!(a.allocate(200, 256).is_some());
        a.reclaim(2);
        assert!(a.allocate(200, 256).is_none());
        a.reclaim(3);
        let later = a.allocate(200, 256).unwrap();
        assert_eq!(later.offset, 0);
        assert_ne!(first.generation, later.generation);
    }
    #[test]
    fn growth_preserves_live_and_deferred_ranges() {
        let mut a = Arena::new(512);
        let x = a.allocate(256, 256).unwrap();
        let y = a.allocate(256, 256).unwrap();
        a.retire(y, 7);
        a.grow(1024);
        assert!(a.contains(x));
        let z = a.allocate(512, 256).unwrap();
        assert_eq!(z.offset, 512);
        assert!(a.allocate(4, 4).is_none());
        a.reclaim(7);
        let reused = a.allocate(256, 256).unwrap();
        assert_eq!(reused.offset, y.offset);
        assert_ne!(reused.generation, y.generation);
        assert!(a.contains(z));
    }
    #[test]
    fn growth_coalesces_free_tail_despite_fragmentation() {
        let mut a = Arena::new(512);
        let x = a.allocate(128, 4).unwrap();
        let y = a.allocate(128, 4).unwrap();
        a.retire(x, 1);
        a.reclaim(1);
        a.grow(1024);
        assert_eq!(a.allocate(768, 4).unwrap().offset, 256);
        assert!(a.contains(y));
        assert_eq!(a.allocate(128, 4).unwrap().offset, 0);
    }
    #[test]
    fn compaction_moves_high_ranges_preserving_generation_and_alignment() {
        let mut a = Arena::new(4096);
        let low = a.allocate(512, 256).unwrap();
        let high = a.allocate(48, 256).unwrap();
        a.retire(low, 2);
        a.reclaim(2);
        let map = a.compact(512, 256);
        assert_eq!(map.len(), 1);
        assert_eq!(map[0].0, high);
        assert_eq!(map[0].1.offset, 0);
        assert_eq!(map[0].1.generation, high.generation);
        assert!(a.contains(map[0].1));
        assert!(!a.contains(high));
        assert_eq!(a.allocate(48, 256).unwrap().offset, 256);
        assert_eq!(a.capacity(), 512);
    }
    #[test]
    fn adjacent_ranges_coalesce() {
        let mut a = Arena::new(512);
        let x = a.allocate(128, 4).unwrap();
        let y = a.allocate(128, 4).unwrap();
        a.retire(x, 1);
        a.retire(y, 1);
        a.reclaim(1);
        assert_eq!(a.allocate(512, 4).unwrap().offset, 0);
    }
}
