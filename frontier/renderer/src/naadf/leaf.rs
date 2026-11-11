//! GPU-only leaf encoding. Keep canonical build words and all child addresses
//! in their existing format; pack two leaves into each storage-buffer word.
use super::data::Snapshot;

const FULL: u32 = 1 << 30;
const COMPACT_FULL: u32 = 1 << 15;
const EMPTY_MASK: u32 = (1 << 12) - 1; // Six two-bit faces within a 4³ brick.
pub(super) const WIDEN: &str = "NAADF compact leaf palette overflow";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Encoding {
    Compact16,
    Wide32,
}

impl Encoding {
    pub(super) fn is_compact(self) -> bool {
        self == Self::Compact16
    }

    pub(super) fn bits(self) -> u32 {
        if self.is_compact() {
            16
        } else {
            32
        }
    }

    pub(super) fn words_per_brick(self) -> usize {
        if self.is_compact() {
            32
        } else {
            64
        }
    }

    pub(super) fn pack(self, words: &mut Vec<u32>) -> Result<(), String> {
        if !self.is_compact() {
            return Ok(());
        }
        // Each destination is behind all source words still to be read.
        let packed_len = words.len() / 2;
        for i in 0..packed_len {
            let a = compact(words[i * 2]).ok_or(WIDEN)?;
            let b = compact(words[i * 2 + 1]).ok_or(WIDEN)?;
            words[i] = a | (b << 16);
        }
        words.truncate(packed_len);
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn load(self, words: &[u32], address: usize) -> u32 {
        if !self.is_compact() {
            return words[address];
        }
        let code = (words[address / 2] >> ((address & 1) * 16)) & 0xffff;
        if code & COMPACT_FULL != 0 {
            FULL | (code & (COMPACT_FULL - 1))
        } else {
            code
        }
    }
}

fn compact(value: u32) -> Option<u32> {
    if value & FULL != 0 {
        let id = value ^ FULL;
        (id < COMPACT_FULL).then_some(COMPACT_FULL | id)
    } else {
        (value <= EMPTY_MASK).then_some(value)
    }
}

pub(super) struct Prepared {
    pub snapshot: Snapshot,
    pub encoding: Encoding,
    pub wide_bytes: usize,
    pub sparse_root_dims: Option<[u32; 3]>,
}

impl Prepared {
    pub(super) fn new(mut snapshot: Snapshot, force_wide: bool) -> Self {
        let wide_bytes = snapshot.bytes();
        let encoding = if !force_wide
            && snapshot.voxels.len() % 2 == 0
            && snapshot.voxels.iter().all(|&word| compact(word).is_some())
        {
            // The worker compacts in place before it hands off the snapshot.
            Encoding::Compact16.pack(&mut snapshot.voxels).unwrap();
            Encoding::Compact16
        } else {
            Encoding::Wide32
        };
        Self {
            snapshot,
            encoding,
            wide_bytes,
            sparse_root_dims: None,
        }
    }

    pub(super) fn make_sparse_roots(&mut self, cancel: impl Fn() -> bool) {
        let n = self.snapshot.chunks_per_edge;
        if self.snapshot.gpu_distance {
            return;
        }
        if let Some((words, dims)) = super::sparse_roots::atlas(&self.snapshot.chunks, n, cancel) {
            self.wide_bytes -= (self.snapshot.chunks.len() - words.len()) * 4;
            self.snapshot.chunks = words;
            self.sparse_root_dims = Some(dims);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(code: u32) -> u32 {
        if code & COMPACT_FULL != 0 {
            FULL | (code & (COMPACT_FULL - 1))
        } else {
            code
        }
    }

    #[test]
    fn compact_leaves_preserve_all_faces_and_15_bit_pigments() {
        for empty in 0..=EMPTY_MASK {
            assert_eq!(decode(compact(empty).unwrap()), empty);
        }
        for id in 0..COMPACT_FULL {
            let full = FULL | id;
            assert_eq!(decode(compact(full).unwrap()), full);
        }
        assert_eq!(compact(FULL | COMPACT_FULL), None);
        assert_eq!(compact(FULL | (1 << 29)), None);
        assert_eq!(compact(1 << 31), None);
    }
}
