//! Bounded, coalescing GPU suballocation. No relocation of live allocations.
//! Reuse is safe because all writes use the same ordered wgpu queue: a new
//! write executes after earlier submitted draws. Callers must submit a frame
//! before calling update again; unsubmitted encoders may not retain slices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Allocation {
    pub offset: u64,
    pub size: u64,
}
pub(super) struct Arena {
    free: Vec<Allocation>,
    capacity: u64,
}
impl Arena {
    pub fn new(capacity: u64) -> Self {
        Self {
            free: vec![Allocation {
                offset: 0,
                size: capacity,
            }],
            capacity,
        }
    }
    pub fn allocate(&mut self, bytes: u64, alignment: u64) -> Option<Allocation> {
        for i in 0..self.free.len() {
            let block = self.free[i];
            let start = block.offset.checked_add(alignment - 1)? / alignment * alignment;
            let end = start.checked_add(bytes)?;
            if end > block.offset + block.size {
                continue;
            }
            self.free.remove(i);
            if start > block.offset {
                self.free.push(Allocation {
                    offset: block.offset,
                    size: start - block.offset,
                });
            }
            if end < block.offset + block.size {
                self.free.push(Allocation {
                    offset: end,
                    size: block.offset + block.size - end,
                });
            }
            return Some(Allocation {
                offset: start,
                size: bytes,
            });
        }
        None
    }
    pub fn release(&mut self, allocation: Allocation) {
        assert!(allocation.offset + allocation.size <= self.capacity);
        self.free.push(allocation);
        self.free.sort_by_key(|a| a.offset);
        let mut i = 0;
        while i + 1 < self.free.len() {
            assert!(
                self.free[i].offset + self.free[i].size <= self.free[i + 1].offset,
                "overlapping GPU reservation"
            );
            if self.free[i].offset + self.free[i].size == self.free[i + 1].offset {
                let next = self.free.remove(i + 1);
                self.free[i].size += next.size;
            } else {
                i += 1;
            }
        }
    }
    pub fn used(&self) -> u64 {
        self.capacity - self.free.iter().map(|a| a.size).sum::<u64>()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aligned_reservations_and_coalesced_reuse() {
        let mut arena = Arena::new(1024);
        let a = arena.allocate(44, 256).unwrap();
        let b = arena.allocate(44, 256).unwrap();
        assert_eq!(a.offset, 0);
        assert_eq!(b.offset, 256);
        assert!(arena.allocate(800, 256).is_none());
        arena.release(a);
        arena.release(b);
        assert_eq!(arena.used(), 0);
        assert_eq!(arena.allocate(1024, 256).unwrap().size, 1024);
    }
}
