//! Bounded exact source values in lazy 32×32 native-column pages.
//!
//! A page starts with one inline value and then a sorted sparse vector. Dense
//! storage is allocated only after actual demand reaches the promotion threshold;
//! validity bits distinguish missing values from default placeholders. No source
//! values are generated here. The owning source wrapper must clear/recreate this
//! cache when its immutable source identity or relevant revision changes.
//!
//! Heap accounting includes payload capacities, conservative hash-table/control
//! and allocator overhead, and old+new allocation overlap during growth/promotion.
//! The fixed cache object and caller-owned copies of returned values are excluded.
use std::{collections::HashMap, mem::size_of};

pub const PAGE_SIDE: usize = 32;
const PAGE_CELLS: usize = PAGE_SIDE * PAGE_SIDE;
const BIT_WORDS: usize = PAGE_CELLS / 64;
const PROMOTE_AT: usize = 128;
const ALLOCATION_OVERHEAD: usize = 64;
const CONTROL_PADDING: usize = 16;
type PageKey = [i64; 2];

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub hits: u64,
    pub misses: u64,
    pub insertions: u64,
    pub page_evictions: u64,
    pub evicted_columns: u64,
    pub promotions: u64,
    pub table_compactions: u64,
    pub rejected_insertions: u64,
    /// Conservative heap peak, including simultaneous old/new allocations.
    pub peak_allocated_bytes: usize,
}
#[derive(Clone, Copy)]
struct Indexed<T: Copy> {
    index: u16,
    value: T,
}
struct Dense<T: Copy> {
    values: Vec<T>,
    valid: [u64; BIT_WORDS],
    len: usize,
}
enum Data<T: Copy> {
    Inline(Indexed<T>),
    Sparse(Vec<Indexed<T>>),
    Dense(Box<Dense<T>>),
}
impl<T: Copy> Data<T> {
    fn get(&self, index: u16) -> Option<T> {
        match self {
            Self::Inline(v) => (v.index == index).then_some(v.value),
            Self::Sparse(values) => values
                .binary_search_by_key(&index, |v| v.index)
                .ok()
                .map(|i| values[i].value),
            Self::Dense(page) => {
                let i = index as usize;
                ((page.valid[i / 64] >> (i % 64)) & 1 != 0).then(|| page.values[i])
            }
        }
    }
    fn update(&mut self, index: u16, value: T) -> bool {
        match self {
            Self::Inline(v) if v.index == index => {
                v.value = value;
                true
            }
            Self::Sparse(values) => {
                if let Ok(i) = values.binary_search_by_key(&index, |v| v.index) {
                    values[i].value = value;
                    true
                } else {
                    false
                }
            }
            Self::Dense(page) => {
                let i = index as usize;
                if (page.valid[i / 64] >> (i % 64)) & 1 == 0 {
                    return false;
                }
                page.values[i] = value;
                true
            }
            _ => false,
        }
    }
    fn len(&self) -> usize {
        match self {
            Self::Inline(_) => 1,
            Self::Sparse(v) => v.len(),
            Self::Dense(p) => p.len,
        }
    }
    fn bytes(&self) -> usize {
        match self {
            Self::Inline(_) => 0,
            Self::Sparse(v) => sparse_bytes::<T>(v.capacity()),
            Self::Dense(p) => dense_bytes::<T>(p.values.capacity()),
        }
    }
}
struct Page<T: Copy> {
    data: Data<T>,
    previous: Option<PageKey>,
    next: Option<PageKey>,
    touched_job: u64,
}

/// One cache belongs to one immutable canonical source snapshot. Returned values
/// are copies, so page eviction never invalidates a caller's native data.
pub struct PageCache<T: Copy + Default> {
    budget: usize,
    pages: HashMap<PageKey, Page<T>>,
    table_buckets: usize,
    oldest: Option<PageKey>,
    newest: Option<PageKey>,
    job: u64,
    columns: usize,
    payload_bytes: usize,
    stats: Stats,
}
impl<T: Copy + Default> PageCache<T> {
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            pages: HashMap::new(),
            table_buckets: 0,
            oldest: None,
            newest: None,
            job: 1,
            columns: 0,
            payload_bytes: 0,
            stats: Stats::default(),
        }
    }
    /// Touch a page at most once per geometry job, not once per source column.
    pub fn begin_job(&mut self) {
        if self.job == u64::MAX {
            for page in self.pages.values_mut() {
                page.touched_job = 0;
            }
            self.job = 1;
        } else {
            self.job += 1;
        }
    }
    pub fn get(&mut self, address: [i64; 2]) -> Option<T> {
        let (key, index) = address_parts(address);
        let (value, needs_touch) = self
            .pages
            .get(&key)
            .map(|p| (p.data.get(index), p.touched_job != self.job))
            .unwrap_or((None, false));
        if needs_touch {
            self.touch(key);
        }
        if value.is_some() {
            self.stats.hits += 1;
        } else {
            self.stats.misses += 1;
        }
        value
    }
    /// Best-effort cache admission. Failure never changes the caller's freshly
    /// generated canonical value; it only means that value remains uncached.
    pub fn insert(&mut self, address: [i64; 2], value: T) -> bool {
        let accepted = self.insert_inner(address, value);
        if accepted {
            self.stats.insertions += 1;
            self.note_peak(0);
        } else {
            self.stats.rejected_insertions += 1;
        }
        debug_assert!(self.allocated_bytes() <= self.budget);
        accepted
    }
    pub fn allocated_bytes(&self) -> usize {
        self.table_bytes().saturating_add(self.payload_bytes)
    }
    pub fn len(&self) -> usize {
        self.columns
    }
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
    pub fn stats(&self) -> Stats {
        self.stats
    }
    /// Release all data and backing tables when the source/revision changes.
    /// Lifetime counters remain available for diagnostics.
    pub fn clear(&mut self) {
        self.pages = HashMap::new();
        self.table_buckets = 0;
        self.oldest = None;
        self.newest = None;
        self.columns = 0;
        self.payload_bytes = 0;
    }
    fn insert_inner(&mut self, address: [i64; 2], value: T) -> bool {
        let (key, index) = address_parts(address);
        if !self.pages.contains_key(&key) {
            if !self.ensure_slot() {
                return false;
            }
            let page = Page {
                data: Data::Inline(Indexed { index, value }),
                previous: self.newest,
                next: None,
                touched_job: self.job,
            };
            if let Some(tail) = self.newest {
                self.pages.get_mut(&tail).unwrap().next = Some(key);
            } else {
                self.oldest = Some(key);
            }
            self.newest = Some(key);
            self.pages.insert(key, page);
            self.columns += 1;
            return true;
        }
        // Dense insertion never allocates. Keep this common native-scan path
        // to one page lookup when the page was already touched by this job.
        let dense_touch = if let Some(page) = self.pages.get_mut(&key) {
            if let Data::Dense(dense) = &mut page.data {
                let i = index as usize;
                let present = (dense.valid[i / 64] >> (i % 64)) & 1 != 0;
                dense.values[i] = value;
                dense.valid[i / 64] |= 1_u64 << (i % 64);
                if !present {
                    dense.len += 1;
                    self.columns += 1;
                }
                Some(page.touched_job != self.job)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(needs_touch) = dense_touch {
            if needs_touch {
                self.touch(key);
            }
            return true;
        }
        self.touch(key);
        if self.pages.get_mut(&key).unwrap().data.update(index, value) {
            return true;
        }
        self.compact_table(Some(key));
        let promote = matches!(&self.pages[&key].data, Data::Sparse(v) if v.len() >= PROMOTE_AT);
        if promote {
            self.promote(key);
        }
        if let Data::Dense(page) = &mut self.pages.get_mut(&key).unwrap().data {
            let i = index as usize;
            page.values[i] = value;
            page.valid[i / 64] |= 1_u64 << (i % 64);
            page.len += 1;
            self.columns += 1;
            return true;
        }
        let (old_capacity, old_len) = match &self.pages[&key].data {
            Data::Inline(_) => (0, 1),
            Data::Sparse(v) => (v.capacity(), v.len()),
            Data::Dense(_) => unreachable!(),
        };
        if old_capacity > old_len {
            let Data::Sparse(values) = &mut self.pages.get_mut(&key).unwrap().data else {
                unreachable!()
            };
            let position = values
                .binary_search_by_key(&index, |v| v.index)
                .unwrap_err();
            values.insert(position, Indexed { index, value });
            self.columns += 1;
            return true;
        }
        let capacity = if old_capacity == 0 {
            4
        } else {
            old_capacity.saturating_mul(2).min(PAGE_CELLS)
        };
        let new_bytes = sparse_bytes::<T>(capacity);
        if !self.make_room(new_bytes, Some(key)) {
            return false;
        }
        self.note_peak(new_bytes);
        let mut values = Vec::new();
        if values.try_reserve_exact(capacity).is_err() {
            return false;
        }
        match &self.pages[&key].data {
            Data::Inline(v) => values.push(*v),
            Data::Sparse(v) => values.extend_from_slice(v),
            Data::Dense(_) => unreachable!(),
        }
        let position = values
            .binary_search_by_key(&index, |v| v.index)
            .unwrap_err();
        values.insert(position, Indexed { index, value });
        let old_bytes = self.pages[&key].data.bytes();
        let actual_bytes = sparse_bytes::<T>(values.capacity());
        debug_assert!(actual_bytes <= new_bytes);
        self.pages.get_mut(&key).unwrap().data = Data::Sparse(values);
        self.payload_bytes = self.payload_bytes - old_bytes + actual_bytes;
        self.columns += 1;
        true
    }
    fn promote(&mut self, key: PageKey) -> bool {
        let new_bytes = dense_bytes::<T>(PAGE_CELLS);
        if !self.make_room(new_bytes, Some(key)) {
            return false;
        }
        self.note_peak(new_bytes);
        let mut values = Vec::new();
        if values.try_reserve_exact(PAGE_CELLS).is_err() {
            return false;
        }
        values.resize(PAGE_CELLS, T::default());
        let mut dense = Dense {
            values,
            valid: [0; BIT_WORDS],
            len: 0,
        };
        let Data::Sparse(old) = &self.pages[&key].data else {
            return false;
        };
        for entry in old {
            let i = entry.index as usize;
            dense.values[i] = entry.value;
            dense.valid[i / 64] |= 1_u64 << (i % 64);
            dense.len += 1;
        }
        let old_bytes = self.pages[&key].data.bytes();
        let actual_bytes = dense_bytes::<T>(dense.values.capacity());
        debug_assert!(actual_bytes <= new_bytes);
        self.pages.get_mut(&key).unwrap().data = Data::Dense(Box::new(dense));
        self.payload_bytes = self.payload_bytes - old_bytes + actual_bytes;
        self.stats.promotions += 1;
        true
    }
    fn touch(&mut self, key: PageKey) {
        let page = self.pages.get_mut(&key).unwrap();
        if page.touched_job == self.job {
            return;
        }
        page.touched_job = self.job;
        if self.newest == Some(key) {
            return;
        }
        let (previous, next) = (page.previous, page.next);
        self.unlink(previous, next);
        let tail = self.newest;
        let page = self.pages.get_mut(&key).unwrap();
        page.previous = tail;
        page.next = None;
        if let Some(tail) = tail {
            self.pages.get_mut(&tail).unwrap().next = Some(key);
        } else {
            self.oldest = Some(key);
        }
        self.newest = Some(key);
    }
    fn unlink(&mut self, previous: Option<PageKey>, next: Option<PageKey>) {
        if let Some(previous) = previous {
            self.pages.get_mut(&previous).unwrap().next = next;
        } else {
            self.oldest = next;
        }
        if let Some(next) = next {
            self.pages.get_mut(&next).unwrap().previous = previous;
        } else {
            self.newest = previous;
        }
    }
    fn evict(&mut self, key: PageKey) {
        let page = self.pages.remove(&key).unwrap();
        self.unlink(page.previous, page.next);
        self.payload_bytes -= page.data.bytes();
        self.columns -= page.data.len();
        self.stats.page_evictions += 1;
        self.stats.evicted_columns += page.data.len() as u64;
        if self.pages.is_empty() {
            // Empty but retained hash capacity would otherwise block promotion.
            self.pages = HashMap::new();
            self.table_buckets = 0;
            self.oldest = None;
            self.newest = None;
        }
    }
    fn make_room(&mut self, additional: usize, protected: Option<PageKey>) -> bool {
        if additional > self.budget {
            return false;
        }
        while self.allocated_bytes().saturating_add(additional) > self.budget {
            let Some(mut victim) = self.oldest else {
                return false;
            };
            if Some(victim) == protected {
                let Some(next) = self.pages[&victim].next else {
                    return false;
                };
                victim = next;
            }
            self.evict(victim);
        }
        true
    }
    fn ensure_slot(&mut self) -> bool {
        loop {
            let capacity = self.pages.capacity();
            if self.pages.len() < capacity {
                return true;
            }
            let buckets = self.table_buckets.saturating_mul(2).max(4);
            let new_table = table_charge::<T>(buckets);
            if new_table > self.budget && capacity > 0 {
                self.evict(self.oldest.unwrap());
                continue;
            }
            if !self.make_room(new_table, None) {
                return false;
            }
            if self.pages.capacity() != capacity || self.pages.len() < capacity {
                continue;
            }
            // The old table remains allocated while HashMap rehashes into new.
            self.note_peak(new_table);
            if self.pages.try_reserve(1).is_err() {
                return false;
            }
            // Reservation never shrinks. Deletions/tombstones may reduce the
            // public capacity without releasing buckets, so retain that ledger.
            self.table_buckets = self.table_buckets.max(raw_buckets(self.pages.capacity()));
            debug_assert!(self.allocated_bytes() <= self.budget);
        }
    }
    fn compact_table(&mut self, protected: Option<PageKey>) {
        let capacity = self.table_buckets;
        if capacity < 32 || self.pages.len().saturating_mul(4) >= capacity {
            return;
        }
        let desired = self.pages.len().max(1);
        let buckets = desired
            .saturating_mul(8)
            .saturating_add(6)
            .checked_div(7)
            .and_then(usize::checked_next_power_of_two)
            .unwrap_or(usize::MAX)
            .max(4);
        let new_table = table_charge::<T>(buckets);
        if !self.make_room(new_table, protected) || self.pages.capacity() == 0 {
            return;
        }
        self.note_peak(new_table);
        let mut replacement = HashMap::new();
        if replacement.try_reserve(self.pages.len()).is_err() {
            return;
        }
        let actual_buckets = raw_buckets(replacement.capacity());
        debug_assert!(table_charge::<T>(actual_buckets) <= new_table);
        let old = std::mem::replace(&mut self.pages, replacement);
        self.table_buckets = actual_buckets;
        self.pages.extend(old);
        self.stats.table_compactions += 1;
        debug_assert!(self.allocated_bytes() <= self.budget);
    }
    fn table_bytes(&self) -> usize {
        table_charge::<T>(self.table_buckets)
    }
    fn note_peak(&mut self, temporary_bytes: usize) {
        self.stats.peak_allocated_bytes = self
            .stats
            .peak_allocated_bytes
            .max(self.allocated_bytes().saturating_add(temporary_bytes));
        debug_assert!(self.stats.peak_allocated_bytes <= self.budget);
    }
}
fn address_parts(address: [i64; 2]) -> (PageKey, u16) {
    let key = address.map(|v| v.div_euclid(PAGE_SIDE as i64));
    let x = address[0].rem_euclid(PAGE_SIDE as i64) as usize;
    let z = address[1].rem_euclid(PAGE_SIDE as i64) as usize;
    (key, (z * PAGE_SIDE + x) as u16)
}
fn raw_buckets(capacity: usize) -> usize {
    if capacity == 0 {
        return 0;
    }
    capacity
        .saturating_mul(8)
        .saturating_add(6)
        .checked_div(7)
        .and_then(usize::checked_next_power_of_two)
        .unwrap_or(usize::MAX)
        .max(4)
}
fn table_charge<T: Copy>(buckets: usize) -> usize {
    if buckets == 0 {
        return 0;
    }
    buckets
        .saturating_mul(size_of::<(PageKey, Page<T>)>().saturating_add(1))
        .saturating_add(CONTROL_PADDING + ALLOCATION_OVERHEAD)
}
fn sparse_bytes<T: Copy>(capacity: usize) -> usize {
    if capacity == 0 {
        0
    } else {
        capacity
            .saturating_mul(size_of::<Indexed<T>>())
            .saturating_add(ALLOCATION_OVERHEAD)
    }
}
fn dense_bytes<T: Copy>(capacity: usize) -> usize {
    capacity
        .saturating_mul(size_of::<T>())
        .saturating_add(size_of::<Dense<T>>())
        .saturating_add(2 * ALLOCATION_OVERHEAD)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet, VecDeque};
    #[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
    struct Column {
        height: i64,
        bands: [u8; 3],
    }
    fn column(p: [i64; 2]) -> Column {
        let height = p[0]
            .wrapping_mul(0x37a1)
            .wrapping_add(p[1].wrapping_mul(0x17b9));
        Column {
            height,
            bands: [
                height.rem_euclid(5) as u8,
                (height >> 3).rem_euclid(5) as u8,
                (height >> 7).rem_euclid(5) as u8,
            ],
        }
    }
    fn valid(cache: &PageCache<Column>) {
        assert!(cache.allocated_bytes() <= cache.budget);
        assert!(cache.stats.peak_allocated_bytes <= cache.budget);
        assert_eq!(
            cache.payload_bytes,
            cache.pages.values().map(|p| p.data.bytes()).sum::<usize>()
        );
        assert_eq!(
            cache.len(),
            cache.pages.values().map(|p| p.data.len()).sum::<usize>()
        );
        let (mut key, mut previous, mut visited) = (cache.oldest, None, HashSet::new());
        while let Some(k) = key {
            assert!(visited.insert(k), "LRU cycle");
            let page = &cache.pages[&k];
            assert_eq!(page.previous, previous);
            previous = Some(k);
            key = page.next;
            if let Data::Dense(d) = &page.data {
                assert_eq!(
                    d.len,
                    d.valid
                        .iter()
                        .map(|v| v.count_ones() as usize)
                        .sum::<usize>()
                );
            }
        }
        assert_eq!(visited.len(), cache.pages.len());
        assert_eq!(previous, cache.newest);
    }
    #[test]
    fn exact_values_and_negative_page_boundaries() {
        let mut c = PageCache::new(128 * 1024);
        for x in [-33, -32, -31, -1, 0, 31, 32, 33] {
            for z in [-33, -32, -31, -1, 0, 31, 32, 33] {
                let p = [x, z];
                assert!(c.insert(p, column(p)));
            }
        }
        for x in [-33, -32, -31, -1, 0, 31, 32, 33] {
            for z in [-33, -32, -31, -1, 0, 31, 32, 33] {
                assert_eq!(c.get([x, z]), Some(column([x, z])));
            }
        }
        for (p, height) in [
            ([i64::MIN, i64::MAX], i64::MIN),
            ([i64::MAX, i64::MIN], i64::MAX),
        ] {
            let value = Column {
                height,
                bands: [4, 3, 2],
            };
            assert!(c.insert(p, value));
            assert_eq!(c.get(p), Some(value));
        }
        valid(&c);
    }
    #[test]
    fn dense_promotion_keeps_missing_cells_invalid() {
        let mut c = PageCache::new(32 * 1024);
        for i in 0..=PROMOTE_AT {
            let p = [(i % 32) as i64, (i / 32) as i64];
            assert!(c.insert(p, column(p)));
        }
        assert_eq!(c.stats().promotions, 1);
        assert_eq!(c.len(), PROMOTE_AT + 1);
        assert_eq!(c.get([31, 31]), None);
        assert!(c.insert([31, 31], Column::default()));
        assert_eq!(c.get([31, 31]), Some(Column::default()));
        for i in 0..=PROMOTE_AT {
            let p = [(i % 32) as i64, (i / 32) as i64];
            assert_eq!(c.get(p), Some(column(p)));
        }
        valid(&c);
    }
    #[test]
    fn sparse_coarse_queries_and_halos_never_allocate_dense_pages() {
        let mut c = PageCache::new(128 * 1024);
        for i in 0..200 {
            assert!(c.insert([i * 1024, -i * 2048], column([i * 1024, -i * 2048])));
        }
        assert_eq!(c.stats().promotions, 0);
        assert!(c.pages.values().all(|p| matches!(p.data, Data::Inline(_))));
        for z in 0..32 {
            assert!(c.insert([-1, z], column([-1, z])));
        }
        assert!(matches!(c.pages[&[-1, 0]].data, Data::Sparse(_)));
        valid(&c);
    }
    #[test]
    fn promotion_reserves_old_and_new_storage_before_allocating() {
        let table = table_charge::<Column>(4);
        let final_dense = table + dense_bytes::<Column>(PAGE_CELLS);
        let mut c = PageCache::new(final_dense + 16);
        for i in 0..=PROMOTE_AT {
            let p = [(i % 32) as i64, (i / 32) as i64];
            assert!(c.insert(p, column(p)));
        }
        // The final dense page would fit, but promotion's simultaneous sparse
        // and dense allocations do not; retaining sparse data is the safe path.
        assert_eq!(c.stats().promotions, 0);
        assert!(matches!(c.pages[&[0, 0]].data, Data::Sparse(_)));
        valid(&c);
    }
    #[test]
    fn lru_is_touched_once_per_job_and_clear_invalidates_source_revision() {
        let mut c = PageCache::new(table_charge::<Column>(4) + 8);
        for p in [[0, 0], [32, 0], [64, 0]] {
            assert!(c.insert(p, column(p)));
        }
        c.begin_job();
        assert_eq!(c.get([0, 0]), Some(column([0, 0])));
        assert!(c.insert([96, 0], column([96, 0])));
        assert_eq!(c.get([32, 0]), None);
        assert_eq!(c.get([0, 0]), Some(column([0, 0])));
        valid(&c);
        c.clear();
        assert_eq!(c.allocated_bytes(), 0);
        assert_eq!(c.len(), 0);
        assert_eq!(c.get([0, 0]), None);
        let revised = Column {
            height: -9,
            bands: [4, 4, 4],
        };
        assert!(c.insert([0, 0], revised));
        assert_eq!(c.get([0, 0]), Some(revised));
        c.job = u64::MAX;
        c.begin_job();
        assert_eq!(c.get([0, 0]), Some(revised));
        valid(&c);
    }
    #[test]
    fn mixed_pressure_keeps_values_lru_and_allocation_accounting_consistent() {
        let mut c = PageCache::new(64 * 1024);
        for job in 0..24 {
            c.begin_job();
            let base = [(job % 7) * 32, (job % 5) * 32];
            for i in 0..400 {
                let p = [base[0] + i % 32, base[1] + i / 32];
                let value = column(p);
                if c.insert(p, value) {
                    assert_eq!(c.get(p), Some(value));
                }
                valid(&c);
            }
            for i in 0..12 {
                let p = [job * 4096 + i * 128, -job * 2048 - i * 64];
                let value = column(p);
                if c.insert(p, value) {
                    assert_eq!(c.get(p), Some(value));
                }
                valid(&c);
            }
        }
        assert!(c.stats().promotions > 0);
        assert!(c.stats().page_evictions > 0);
    }
    #[test]
    fn tombstones_do_not_reduce_the_allocation_ledger() {
        let mut c = PageCache::new(1024 * 1024);
        for i in 0..1000 {
            assert!(c.insert([i * 32, 0], column([i * 32, 0])));
        }
        let buckets = c.table_buckets;
        let bytes = c.table_bytes();
        for _ in 0..800 {
            c.evict(c.oldest.unwrap());
        }
        assert_eq!(c.table_buckets, buckets);
        assert_eq!(c.table_bytes(), bytes);
        c.compact_table(None);
        assert!(c.table_buckets < buckets);
        for i in 800..1000 {
            assert_eq!(c.get([i * 32, 0]), Some(column([i * 32, 0])));
        }
        valid(&c);
    }
    #[test]
    fn zero_and_tiny_budgets_reject_cache_admission_without_fabricating_data() {
        for budget in [0, 1, 32, 128] {
            let mut c = PageCache::<Column>::new(budget);
            assert!(!c.insert([0, 0], column([0, 0])));
            assert_eq!(c.get([0, 0]), None);
            assert_eq!(c.allocated_bytes(), 0);
            valid(&c);
        }
    }
    struct Baseline {
        values: HashMap<[i64; 2], Column>,
        order: VecDeque<[i64; 2]>,
        limit: usize,
        generated: u64,
        hits: u64,
    }
    impl Baseline {
        fn new(budget: usize) -> Self {
            Self {
                values: HashMap::new(),
                order: VecDeque::new(),
                limit: budget / 160,
                generated: 0,
                hits: 0,
            }
        }
        fn query(&mut self, p: [i64; 2]) -> Column {
            if let Some(&value) = self.values.get(&p) {
                self.hits += 1;
                return value;
            }
            self.generated += 1;
            let value = column(p);
            if self.limit > 0 {
                while self.values.len() >= self.limit {
                    let key = self.order.pop_front().unwrap();
                    self.values.remove(&key);
                }
                self.values.insert(p, value);
                self.order.push_back(p);
            }
            value
        }
    }
    fn trace(mut begin: impl FnMut(), mut query: impl FnMut([i64; 2])) {
        begin();
        for z in -8..8 {
            for x in -8..8 {
                query([x * 1024, z * 1024]);
            }
        }
        for _ in 0..5 {
            for base in [[0, 0], [128, 0], [0, 128]] {
                begin();
                for z in -1..=128 {
                    for x in -1..=128 {
                        let p = [base[0] + x, base[1] + z];
                        query(p);
                        query([p[0] + 1, p[1]]);
                        query([p[0], p[1] + 1]);
                    }
                }
            }
        }
    }
    #[test]
    fn deterministic_primary_like_trace_reuses_dense_native_columns() {
        use std::cell::RefCell;
        let budget = 1024 * 1024;
        let paged = RefCell::new(PageCache::new(budget));
        let mut generated = 0u64;
        let start = std::time::Instant::now();
        trace(
            || paged.borrow_mut().begin_job(),
            |p| {
                let mut cache = paged.borrow_mut();
                let value = if let Some(v) = cache.get(p) {
                    v
                } else {
                    generated += 1;
                    let value = column(p);
                    cache.insert(p, value);
                    value
                };
                assert_eq!(value, column(p));
            },
        );
        let paged_ms = start.elapsed().as_secs_f64() * 1000.0;
        let mut baseline = Baseline::new(budget);
        let start = std::time::Instant::now();
        trace(
            || {},
            |p| {
                assert_eq!(baseline.query(p), column(p));
            },
        );
        let baseline_ms = start.elapsed().as_secs_f64() * 1000.0;
        let cache = paged.borrow();
        valid(&cache);
        println!("source trace budget={budget} paged_generated={generated} baseline_generated={} paged_hits={} baseline_hits={} paged_ms={paged_ms:.3} baseline_ms={baseline_ms:.3} allocated={} peak={} pages={} columns={} promotions={}",baseline.generated,cache.stats().hits,baseline.hits,cache.allocated_bytes(),cache.stats().peak_allocated_bytes,cache.page_count(),cache.len(),cache.stats().promotions);
        assert!(
            generated * 2 < baseline.generated,
            "paged={generated}, baseline={}",
            baseline.generated
        );
    }
}
