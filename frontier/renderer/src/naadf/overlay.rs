//! Optional exact narrow vegetation staging. Address-major/pigment-minor order
//! is identical to the original u128 representation. Wide pigments promote the
//! whole transaction without truncation, including during overlap resolution.
use std::sync::OnceLock;
pub(super) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("FRONTIER_NAADF_COMPACT_OVERLAY").as_deref() == Ok("1"))
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
    pub(super) fn resolve(
        &mut self,
        mut canonical: impl FnMut(u64) -> Result<u32, String>,
        cancel: &impl Fn() -> bool,
    ) -> Result<(), String> {
        match self {
            Self::Narrow(v) => v.sort_unstable(),
            // Keep the ordinary wide control's inner loop unchanged; no enum
            // dispatch per comparison/read when the experiment is disabled.
            Self::Wide(v, _) => {
                v.sort_unstable();
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
        for mode in 0..4 {
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
                .resolve(
                    |a| {
                        calls.push(a);
                        Ok(canonical(a))
                    },
                    &|| false,
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
