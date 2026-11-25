//! Experimental exact cold source payloads. LZ4 block codec, fixed scratch and
//! bounded linear LRU metadata; no camera epoch is part of content identity.
//! The owner must clear this cache when its immutable source snapshot changes.
use std::mem::size_of;
const OVERHEAD: usize = 64;
// lz4_flex 0.11.6 uses a temporary boxed 4096-entry u16 hash table
// for blocks <64 KiB. Reserve its allocator charge even outside encoding.
const CODEC_WORKSPACE: usize = 8192 + OVERHEAD;
#[derive(Clone, Copy, Debug, Default)]
pub struct ColdStats {
    pub enabled_shards: usize,
    pub budget_bytes: usize,
    pub resident_bytes: usize,
    pub entries: usize,
    pub retained_raw_bytes: usize,
    pub hits: u64,
    pub misses: u64,
    pub insertions: u64,
    pub evictions: u64,
    pub encoded_bytes: u64,
    pub raw_bytes: u64,
    pub encode_ns: u64,
    pub decode_ns: u64,
    pub peak_bytes: usize,
}
struct Entry {
    key: [i64; 2],
    data: Box<[u8]>,
    raw_len: usize,
    stamp: u64,
}
pub(crate) struct ColdCache {
    budget: usize,
    entries: Vec<Entry>,
    raw: Vec<u8>,
    compressed: Vec<u8>,
    payload: usize,
    decoded: Option<([i64; 2], usize)>,
    stamp: u64,
    stats: ColdStats,
}
impl ColdCache {
    pub fn new(budget: usize, max_payload: usize) -> Option<Self> {
        if max_payload >= 65536 {
            return None;
        }
        // Metadata is fixed so insertion cannot transiently grow the LRU index.
        let slots = (budget / 4096).clamp(1, 256);
        let output = lz4_flex::block::get_maximum_output_size(max_payload);
        let reserve = slots
            .checked_mul(size_of::<Entry>())?
            .checked_add(max_payload)?
            .checked_add(output)?
            .checked_add(3 * OVERHEAD + CODEC_WORKSPACE)?;
        if reserve.checked_add(output + OVERHEAD)? > budget {
            return None;
        }
        let mut out = Self {
            budget,
            entries: Vec::with_capacity(slots),
            raw: vec![0; max_payload],
            compressed: vec![0; output],
            payload: 0,
            decoded: None,
            stamp: 0,
            stats: ColdStats::default(),
        };
        out.peak();
        Some(out)
    }
    pub fn allocated_bytes(&self) -> usize {
        self.entries.capacity() * size_of::<Entry>()
            + self.raw.capacity()
            + self.compressed.capacity()
            + 3 * OVERHEAD
            + CODEC_WORKSPACE
            + self.payload
    }
    pub fn stats(&self) -> ColdStats {
        ColdStats {
            enabled_shards: 1,
            budget_bytes: self.budget,
            resident_bytes: self.allocated_bytes(),
            entries: self.entries.len(),
            retained_raw_bytes: self.entries.iter().map(|e| e.raw_len).sum(),
            ..self.stats
        }
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.payload = 0;
        self.decoded = None;
    }
    fn peak(&mut self) {
        self.stats.peak_bytes = self.stats.peak_bytes.max(self.allocated_bytes());
        debug_assert!(self.allocated_bytes() <= self.budget);
    }
    pub fn remove(&mut self, key: [i64; 2]) {
        if self.decoded.is_some_and(|v| v.0 == key) {
            self.decoded = None;
        }
        if let Some(i) = self.entries.iter().position(|e| e.key == key) {
            let e = self.entries.swap_remove(i);
            self.payload -= e.data.len() + OVERHEAD;
        }
    }
    pub fn insert(&mut self, key: [i64; 2], len: usize, fill: impl FnOnce(&mut [u8])) -> bool {
        if len > self.raw.len() {
            return false;
        }
        self.decoded = None;
        fill(&mut self.raw[..len]);
        let start = std::time::Instant::now();
        let Ok(n) = lz4_flex::block::compress_into(&self.raw[..len], &mut self.compressed) else {
            return false;
        };
        self.stats.encode_ns += start.elapsed().as_nanos() as u64;
        self.remove(key);
        let charge = n + OVERHEAD;
        while self.allocated_bytes().saturating_add(charge) > self.budget
            || self.entries.len() == self.entries.capacity()
        {
            let Some((i, _)) = self.entries.iter().enumerate().min_by_key(|(_, e)| e.stamp) else {
                return false;
            };
            let victim = self.entries.swap_remove(i);
            if self.decoded.is_some_and(|v| v.0 == victim.key) {
                self.decoded = None;
            }
            self.payload -= victim.data.len() + OVERHEAD;
            self.stats.evictions += 1;
        }
        self.stamp = self.stamp.wrapping_add(1);
        self.entries.push(Entry {
            key,
            data: self.compressed[..n].into(),
            raw_len: len,
            stamp: self.stamp,
        });
        self.payload += charge;
        self.stats.insertions += 1;
        self.stats.raw_bytes += len as u64;
        self.stats.encoded_bytes += n as u64;
        self.peak();
        true
    }
    pub fn get<R>(&mut self, key: [i64; 2], read: impl FnOnce(&[u8]) -> R) -> Option<R> {
        if let Some((decoded_key, len)) = self.decoded {
            if key == decoded_key {
                self.stamp = self.stamp.wrapping_add(1);
                if let Some(e) = self.entries.iter_mut().find(|e| e.key == key) {
                    e.stamp = self.stamp;
                }
                self.stats.hits += 1;
                return Some(read(&self.raw[..len]));
            }
        }
        let Some(entry) = self.entries.iter_mut().find(|e| e.key == key) else {
            self.stats.misses += 1;
            return None;
        };
        self.decoded = None;
        let start = std::time::Instant::now();
        let n =
            lz4_flex::block::decompress_into(&entry.data, &mut self.raw[..entry.raw_len]).ok()?;
        self.stats.decode_ns += start.elapsed().as_nanos() as u64;
        if n != entry.raw_len {
            return None;
        }
        self.stamp = self.stamp.wrapping_add(1);
        entry.stamp = self.stamp;
        self.stats.hits += 1;
        self.decoded = Some((key, n));
        Some(read(&self.raw[..n]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_decode_does_not_reuse_overwritten_scratch() {
        let mut c = ColdCache::new(128 * 1024, 13312).unwrap();
        c.insert([0, 0], 100, |b| b.fill(42));
        c.insert([1, 0], 100, |b| b.fill(73));
        assert_eq!(c.get([0, 0], |b| b.to_vec()), Some(vec![42; 100]));
        let entry = c.entries.iter_mut().find(|e| e.key == [1, 0]).unwrap();
        entry.raw_len = 99; // the valid stream exceeds this output bound
        assert_eq!(c.get([1, 0], |_| ()), None);
        assert!(c.decoded.is_none());
        assert_eq!(c.get([0, 0], |b| b.to_vec()), Some(vec![42; 100]));
        let old_stamp = c.entries.iter().find(|e| e.key == [0, 0]).unwrap().stamp;
        c.get([0, 0], |_| ());
        assert!(c.entries.iter().find(|e| e.key == [0, 0]).unwrap().stamp > old_stamp);
    }
    #[test]
    fn exact_revisit_clear_and_pressure() {
        let mut c = ColdCache::new(128 * 1024, 13312).unwrap();
        for i in 0..1000 {
            let data: Vec<_> = (0..13312)
                .map(|j| ((j * 73 + i * 31) % 251) as u8)
                .collect();
            assert!(c.insert([i, -i], data.len(), |b| b.copy_from_slice(&data)));
            assert_eq!(c.get([i, -i], |b| b == data), Some(true));
            assert!(c.allocated_bytes() <= c.budget);
            assert!(c.stats().peak_bytes <= c.budget);
        }
        assert!(c.stats().evictions > 0);
        c.clear();
        assert_eq!(c.get([999, -999], |_| ()), None);
        assert!(ColdCache::new(128, 13312).is_none());
    }
}
