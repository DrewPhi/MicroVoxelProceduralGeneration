//! Optional exact narrow vegetation staging. Address-major/pigment-minor order
//! is identical to the original u128 representation. Wide pigments promote the
//! whole transaction without truncation, including during overlap resolution.
use std::sync::OnceLock;
pub(super) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("FRONTIER_NAADF_COMPACT_OVERLAY").as_deref() == Ok("1"))
}
pub(super) fn chunk_local_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| std::env::var("FRONTIER_NAADF_CHUNK_LOCAL_OVERLAY").as_deref() == Ok("1"))
}
pub(super) fn priority_enabled(living_woods: bool) -> bool {
    crate::woodland_volume::render_option("FRONTIER_NAADF_PLANT_PRIORITY", living_woods)
}
pub(super) fn clip_enabled(living_woods: bool) -> bool {
    crate::woodland_volume::render_option("FRONTIER_NAADF_PLANT_CLIP", living_woods)
}

// In-place MSD radix buckets group native 16³ chunks. Only three 256-entry
// tables are needed per address byte; payload capacity never grows. Within a
// chunk ordinary sorting retains address-major/pigment-minor precedence.
pub(super) const CHUNK_SORT_SCRATCH_BYTES: usize = 7 * 3 * 256 * std::mem::size_of::<usize>();

fn sort_chunks<T: Copy + Ord>(
    values: &mut [T],
    chunk: impl Fn(T) -> u64 + Copy,
    cancel: &impl Fn() -> bool,
) -> Result<(), String> {
    fn partition<T: Copy + Ord>(
        values: &mut [T],
        shift: u32,
        chunk: impl Fn(T) -> u64 + Copy,
        cancel: &impl Fn() -> bool,
    ) -> Result<(), String> {
        if cancel() {
            return Err("NAADF build superseded".into());
        }
        if values.len() < 256 {
            values.sort_unstable();
            return Ok(());
        }
        let mut ends = [0usize; 256];
        for (i, &value) in values.iter().enumerate() {
            if i % 4096 == 0 && cancel() {
                return Err("NAADF build superseded".into());
            }
            ends[((chunk(value) >> shift) & 255) as usize] += 1;
        }
        let mut starts = [0usize; 256];
        let mut sum = 0;
        for bucket in 0..256 {
            starts[bucket] = sum;
            sum += ends[bucket];
            ends[bucket] = sum;
        }
        let mut next = starts;
        let mut visits = 0usize;
        for bucket in 0..256 {
            if cancel() {
                return Err("NAADF build superseded".into());
            }
            while next[bucket] < ends[bucket] {
                if visits % 4096 == 0 && cancel() {
                    return Err("NAADF build superseded".into());
                }
                visits += 1;
                let index = next[bucket];
                let destination = ((chunk(values[index]) >> shift) & 255) as usize;
                if destination == bucket {
                    next[bucket] += 1;
                } else {
                    values.swap(index, next[destination]);
                    next[destination] += 1;
                }
            }
        }
        for bucket in 0..256 {
            if cancel() {
                return Err("NAADF build superseded".into());
            }
            let values = &mut values[starts[bucket]..ends[bucket]];
            if shift == 0 {
                values.sort_unstable();
            } else if values.len() > 1 {
                partition(values, shift - 8, chunk, cancel)?;
            }
        }
        Ok(())
    }
    let mut max = 0u64;
    for (i, &value) in values.iter().enumerate() {
        if i % 4096 == 0 && cancel() {
            return Err("NAADF build superseded".into());
        }
        max = max.max(chunk(value));
    }
    let shift = (63 - max.max(1).leading_zeros()) / 8 * 8;
    partition(values, shift, chunk, cancel)
}

pub(super) enum Overlay {
    Narrow(Vec<u64>),
    Wide(Vec<u128>, usize),
}
impl Overlay {
    pub(super) fn new(compact: bool) -> Self {
        if compact {
            Self::Narrow(Vec::new())
        } else {
            Self::Wide(Vec::new(), 0)
        }
    }
    fn widen(&mut self) {
        if let Self::Narrow(values) = self {
            let mut wide = Vec::with_capacity(values.capacity());
            wide.extend(
                values
                    .iter()
                    .map(|v| ((v >> 16) as u128) << 32 | (v & 65535) as u128),
            );
            let promotion_peak = values.capacity() * 8 + wide.capacity() * 16;
            *self = Self::Wide(wide, promotion_peak);
        }
    }
    pub(super) fn push(&mut self, address: u64, pigment: u32) {
        if pigment > 65535 || address > (u64::MAX >> 16) {
            self.widen();
        }
        match self {
            Self::Narrow(v) => v.push((address << 16) | u64::from(pigment)),
            Self::Wide(v, _) => v.push(((address as u128) << 32) | u128::from(pigment)),
        }
    }
    pub(super) fn push_priority(&mut self, address: u64, pigment: u32, priority: u32) {
        let Self::Wide(values, _) = self else {
            unreachable!("priority requires wide overlay storage");
        };
        values
            .push((u128::from(address) << 64) | (u128::from(priority) << 32) | u128::from(pigment));
    }

    /// Keep the earliest plant in the canonical traversal order at each cell.
    /// Strip the temporary priority so the usual chunk assembler sees its
    /// ordinary address/palette layout.
    pub(super) fn resolve_priority(
        &mut self,
        cancel: &impl Fn() -> bool,
        chunk_local: bool,
    ) -> Result<(), String> {
        let Self::Wide(values, _) = self else {
            unreachable!("priority requires wide overlay storage");
        };
        if chunk_local {
            sort_chunks(values, |v| (v >> 76) as u64, cancel)?;
        } else {
            values.sort_unstable();
        }
        let mut kept = 0;
        let mut previous = None;
        for i in 0..values.len() {
            if i % 4096 == 0 && cancel() {
                return Err("NAADF build superseded".into());
            }
            let value = values[i];
            let address = value >> 64;
            if previous != Some(address) {
                values[kept] = (address << 32) | u128::from(value as u32);
                kept += 1;
                previous = Some(address);
            }
        }
        values.truncate(kept);
        Ok(())
    }
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Narrow(v) => v.len(),
            Self::Wide(v, _) => v.len(),
        }
    }
    pub(super) fn bytes(&self) -> usize {
        match self {
            Self::Narrow(v) => v.capacity() * 8,
            Self::Wide(v, peak) => (v.capacity() * 16).max(*peak),
        }
    }
    pub(super) fn address(&self, i: usize) -> u64 {
        match self {
            Self::Narrow(v) => v[i] >> 16,
            Self::Wide(v, _) => (v[i] >> 32) as u64,
        }
    }
    fn pigment(&self, i: usize) -> u32 {
        match self {
            Self::Narrow(v) => (v[i] & 65535) as u32,
            Self::Wide(v, _) => v[i] as u32,
        }
    }
    fn set(&mut self, i: usize, address: u64, pigment: u32) {
        if pigment > 65535 || address > (u64::MAX >> 16) {
            self.widen();
        }
        match self {
            Self::Narrow(v) => v[i] = (address << 16) | u64::from(pigment),
            Self::Wide(v, _) => v[i] = ((address as u128) << 32) | u128::from(pigment),
        }
    }
    #[cfg(test)]
    pub(super) fn resolve(
        &mut self,
        canonical: impl FnMut(u64) -> Result<u32, String>,
        cancel: &impl Fn() -> bool,
    ) -> Result<(), String> {
        self.resolve_ordered(canonical, cancel, false)
    }
    pub(super) fn resolve_ordered(
        &mut self,
        mut canonical: impl FnMut(u64) -> Result<u32, String>,
        cancel: &impl Fn() -> bool,
        chunk_local: bool,
    ) -> Result<(), String> {
        match self {
            Self::Narrow(v) => {
                if chunk_local {
                    sort_chunks(v, |v| v >> 28, cancel)?;
                } else {
                    v.sort_unstable();
                }
            }
            // Keep the ordinary wide control's inner loop unchanged; no enum
            // dispatch per comparison/read when the experiment is disabled.
            Self::Wide(v, _) => {
                if chunk_local {
                    sort_chunks(v, |v| (v >> 44) as u64, cancel)?;
                } else {
                    v.sort_unstable();
                }
                let (mut compact, mut i) = (0, 0);
                while i < v.len() {
                    if i % 4096 == 0 && cancel() {
                        return Err("NAADF build superseded".into());
                    }
                    let address = v[i] >> 32;
                    let mut j = i + 1;
                    while j < v.len() && v[j] >> 32 == address {
                        j += 1;
                    }
                    let pigment = if j - i == 1 {
                        v[i] as u32
                    } else {
                        canonical(address as u64)?
                    };
                    v[compact] = (address << 32) | pigment as u128;
                    compact += 1;
                    i = j;
                }
                v.truncate(compact);
                return Ok(());
            }
        }
        let (mut compact, mut i) = (0, 0);
        while i < self.len() {
            if i % 4096 == 0 && cancel() {
                return Err("NAADF build superseded".into());
            }
            let address = self.address(i);
            let mut j = i + 1;
            while j < self.len() && self.address(j) == address {
                j += 1;
            }
            let pigment = if j - i == 1 {
                self.pigment(i)
            } else {
                canonical(address)?
            };
            self.set(compact, address, pigment);
            compact += 1;
            i = j;
        }
        match self {
            Self::Narrow(v) => v.truncate(compact),
            Self::Wide(v, _) => v.truncate(compact),
        }
        Ok(())
    }
    pub(super) fn for_each(&self, start: usize, end: usize, mut visit: impl FnMut(u64, u32)) {
        match self {
            Self::Narrow(v) => {
                for &p in &v[start..end] {
                    visit(p >> 16, (p & 65535) as u32)
                }
            }
            Self::Wide(v, _) => {
                for &p in &v[start..end] {
                    visit((p >> 32) as u64, p as u32)
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    // Frozen original wide sort/compaction, independent from Overlay::resolve.
    fn reference(mut values: Vec<u128>, mut canonical: impl FnMut(u64) -> u32) -> Vec<u128> {
        values.sort_unstable();
        let (mut compact, mut i) = (0, 0);
        while i < values.len() {
            let a = values[i] >> 32;
            let mut j = i + 1;
            while j < values.len() && values[j] >> 32 == a {
                j += 1;
            }
            let ty = if j - i == 1 {
                values[i] as u32
            } else {
                canonical(a as u64)
            };
            values[compact] = (a << 32) | ty as u128;
            compact += 1;
            i = j;
        }
        values.truncate(compact);
        values
    }
    #[test]
    fn narrow_and_promoted_layouts_match_frozen_wide_order_and_overlaps() {
        for variant in 0..8 {
            let mode = variant % 4;
            let chunk_local = variant >= 4;
            let mut overlay = Overlay::new(true);
            let mut values = Vec::new();
            let mut rng = 8191u64;
            for i in 0..12000 {
                rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                let a = (rng % 4097) * 4096 + (i % 4096) as u64;
                let ty = if mode == 1 && i == 9000 {
                    65536
                } else {
                    (rng & 65535) as u32
                };
                overlay.push(a, ty);
                values.push(((a as u128) << 32) | ty as u128);
                if i % 3 == 0 {
                    overlay.push(a, ty ^ 7);
                    values.push(((a as u128) << 32) | (ty ^ 7) as u128);
                }
            }
            // Includes maximum supported native address, chunk boundaries, and
            // a value requiring the general u128 address fallback.
            for a in [
                0,
                4095,
                4096,
                (1u64 << 39) - 1,
                if mode == 3 { 1u64 << 49 } else { 7 },
            ] {
                overlay.push(a, 5);
                values.push(((a as u128) << 32) | 5);
            }
            let mut calls = Vec::new();
            let mut expected_calls = Vec::new();
            let canonical = |a: u64| {
                if mode == 2 {
                    65536 + (a % 100) as u32
                } else {
                    (a % 65536) as u32
                }
            };
            let expected = reference(values, |a| {
                expected_calls.push(a);
                canonical(a)
            });
            overlay
                .resolve_ordered(
                    |a| {
                        calls.push(a);
                        Ok(canonical(a))
                    },
                    &|| false,
                    chunk_local,
                )
                .unwrap();
            let mut actual = Vec::new();
            overlay.for_each(0, overlay.len(), |a, p| {
                actual.push(((a as u128) << 32) | p as u128)
            });
            assert_eq!(actual, expected);
            assert_eq!(calls, expected_calls);
            assert_eq!(matches!(overlay, Overlay::Narrow(_)), mode == 0);
        }
    }
    #[test]
    fn chunk_local_sort_cancels_before_and_during_partition() {
        use std::cell::Cell;
        for allowance in [0, 3, 50] {
            let mut overlay = Overlay::new(false);
            for i in 0..20000 {
                overlay.push(((i * 7919) % 10007) * 4096 + i % 4096, 5);
            }
            let polls = Cell::new(0);
            assert!(overlay
                .resolve_ordered(
                    |_| Ok(7),
                    &|| {
                        let n = polls.get();
                        polls.set(n + 1);
                        n >= allowance
                    },
                    true
                )
                .is_err());
        }
    }

    #[test]
    #[ignore = "explicit synthetic overlay sorting timing; not a moving-window benchmark"]
    fn benchmark_chunk_local_sort() {
        for compact in [false, true] {
            let mut results = Vec::new();
            for chunk_local in [false, true] {
                let mut overlay = Overlay::new(compact);
                let mut rng = 8191u64;
                for plant in 0..512u64 {
                    rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let chunk = rng % 8192;
                    for voxel in 0..1024u64 {
                        overlay.push(
                            chunk * 4096 + (voxel * 7919 + plant) % 4096,
                            (plant % 65535) as u32,
                        );
                    }
                }
                let entries = overlay.len();
                let bytes = overlay.bytes();
                let start = std::time::Instant::now();
                overlay
                    .resolve_ordered(|a| Ok((a % 65535) as u32), &|| false, chunk_local)
                    .unwrap();
                let ms = start.elapsed().as_secs_f64() * 1000.;
                eprintln!("synthetic_overlay compact={compact} chunk_local={chunk_local} entries={entries} payload_bytes={bytes} scratch_bound_bytes={} sort_resolve_ms={ms:.3}", if chunk_local { CHUNK_SORT_SCRATCH_BYTES } else { 0 });
                let mut result = Vec::new();
                overlay.for_each(0, overlay.len(), |a, p| result.push((a, p)));
                results.push(result);
            }
            assert_eq!(results[0], results[1]);
        }
    }

    #[test]
    fn narrow_payload_halves_staging_and_cancellation_and_error_propagate() {
        let mut narrow = Overlay::new(true);
        let mut wide = Overlay::new(false);
        for i in 0..4096 {
            narrow.push(i, 5);
            wide.push(i, 5);
        }
        assert_eq!(narrow.bytes() * 2, wide.bytes());
        assert!(narrow
            .resolve(|_| panic!("unexpected overlap"), &|| true)
            .is_err());
        let mut narrow = Overlay::new(true);
        narrow.push(1, 5);
        narrow.push(1, 6);
        assert_eq!(
            narrow
                .resolve(|_| Err("canonical failed".into()), &|| false)
                .unwrap_err(),
            "canonical failed"
        );
    }
}
