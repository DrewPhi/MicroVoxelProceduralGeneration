//! Scheduling primitives only. No physics, biology, properties, or gameplay rules.
use crate::grid::ChunkPos;
use std::collections::BTreeSet;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Tick(pub u64);
#[derive(Clone, Debug, Default)]
pub struct Clock {
    tick: Tick,
}
impl Clock {
    pub fn tick(&self) -> Tick {
        self.tick
    }
    pub fn advance(&mut self) -> Option<Tick> {
        self.tick.0 = self.tick.0.checked_add(1)?;
        Some(self.tick)
    }
}
/// Stable ordering, deduplication, and bounded draining. Independent of cameras.
#[derive(Clone, Debug, Default)]
pub struct ActiveChunks {
    pending: BTreeSet<ChunkPos>,
}
impl ActiveChunks {
    pub fn wake(&mut self, key: ChunkPos) {
        self.pending.insert(key);
    }
    pub fn wake_all(&mut self, keys: impl IntoIterator<Item = ChunkPos>) {
        self.pending.extend(keys);
    }
    pub fn take(&mut self, budget: usize) -> Vec<ChunkPos> {
        (0..budget)
            .filter_map(|_| self.pending.pop_first())
            .collect()
    }
    pub fn len(&self) -> usize {
        self.pending.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityId(pub u64);
#[derive(Debug, Default)]
pub struct IdAllocator {
    next: u64,
}
impl IdAllocator {
    pub fn allocate(&mut self) -> Option<EntityId> {
        self.next = self.next.checked_add(1)?;
        Some(EntityId(self.next))
    }
}
