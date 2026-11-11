//! Incremental toroidal window. Content stays valid independently of camera
//! demand. New arena slots remain unreachable until an atomic root publication.
use super::{
    data::{self, ColumnSummary, Pigment, Snapshot},
    leaf::Encoding,
    ownership::Ownership,
};
use frontier_world::{Cell, World};
use std::{
    collections::{BTreeMap, HashMap},
    hash::{DefaultHasher, Hash, Hasher},
    sync::{atomic::AtomicU32, Arc},
    time::Instant,
};
const CHILD: u32 = 1 << 31;
const FULL: u32 = 1 << 30;
const MASK: u32 = FULL - 1;
pub type Point = [i64; 3];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub lo: Point,
    pub hi: Point,
}
impl Region {
    pub fn dims(self) -> [usize; 3] {
        std::array::from_fn(|a| (self.hi[a] - self.lo[a]) as usize)
    }
    pub fn bounds(self) -> [[f64; 3]; 2] {
        [self.lo.map(|v| v as f64), self.hi.map(|v| v as f64)]
    }
    pub fn intersection(self, other: Self) -> Self {
        Self {
            lo: std::array::from_fn(|a| self.lo[a].max(other.lo[a])),
            hi: std::array::from_fn(|a| self.hi[a].min(other.hi[a])),
        }
    }
}
pub fn difference(a: Region, b: Region) -> Vec<Region> {
    let mut core = a;
    let mut out = vec![];
    for axis in 0..3 {
        let low = b.lo[axis].clamp(core.lo[axis], core.hi[axis]);
        if low > core.lo[axis] {
            let mut r = core;
            r.hi[axis] = low;
            out.push(r);
            core.lo[axis] = low;
        }
        let high = b.hi[axis].clamp(core.lo[axis], core.hi[axis]);
        if high < core.hi[axis] {
            let mut r = core;
            r.lo[axis] = high;
            out.push(r);
            core.hi[axis] = high;
        }
    }
    out.into_iter()
        .filter(|r| (0..3).all(|a| r.lo[a] < r.hi[a]))
        .collect()
}
pub fn target(region: Region, eye: [f64; 3]) -> Option<Region> {
    let n = region.hi[0] - region.lo[0];
    let step = (n / 16).clamp(2, 32).min(n / 2);
    let delta: Point = std::array::from_fn(|a| {
        let offset = eye[a] - (region.lo[a] + n / 2) as f64;
        if offset.abs() > step as f64 * 0.5 {
            if offset > 0. {
                step
            } else {
                -step
            }
        } else {
            0
        }
    });
    (delta != [0; 3]).then(|| Region {
        lo: std::array::from_fn(|a| region.lo[a] + delta[a]),
        hi: std::array::from_fn(|a| region.hi[a] + delta[a]),
    })
}
#[derive(Debug)]
struct Arena {
    capacity: usize,
    top: usize,
    free: BTreeMap<usize, usize>,
}
impl Arena {
    fn new(top: usize, capacity: usize) -> Self {
        Self {
            capacity,
            top,
            free: BTreeMap::new(),
        }
    }
    fn take(&mut self, count: usize) -> Result<Vec<usize>, String> {
        if self.free.values().sum::<usize>() + self.capacity - self.top < count {
            return Err("NAADF streaming arena full".into());
        }
        let mut out = Vec::with_capacity(count);
        while out.len() < count {
            if let Some((&start, &len)) = self.free.first_key_value() {
                self.free.remove(&start);
                let take = len.min(count - out.len());
                out.extend(start..start + take);
                if take < len {
                    self.free.insert(start + take, len - take);
                }
            } else {
                let take = count - out.len();
                out.extend(self.top..self.top + take);
                self.top += take;
            }
        }
        Ok(out)
    }
    fn release(&mut self, mut start: usize, mut len: usize) {
        if let Some((&p, &n)) = self.free.range(..start).next_back() {
            if p + n == start {
                self.free.remove(&p);
                start = p;
                len += n;
            }
        }
        if let Some((&p, &n)) = self.free.range(start..).next() {
            if start + len == p {
                self.free.remove(&p);
                len += n;
            }
        }
        self.free.insert(start, len);
    }
}
pub struct Write {
    pub kind: u8,
    pub word: usize,
    pub data: Vec<u32>,
}
pub struct Roots {
    pub region: Region,
    pub nodes: Vec<u32>,
}
pub struct Change {
    pub region: Region,
    pub ring: [u32; 3],
    pub roots: Vec<Roots>,
    pub writes: Vec<Write>,
    pub owner: Arc<Ownership>,
    pub retiring: Arc<Ownership>,
    pub build_ms: f64,
    pub generated_columns: usize,
    pub shared_leaves: usize,
    palette_base: usize,
    leaf_slots: Vec<usize>,
    block_slots: Vec<usize>,
    columns: HashMap<(i64, i64), ColumnSummary>,
}
pub struct State {
    pub region: Region,
    n: usize,
    ring: [usize; 3],
    // Retirement only needs the mixed roots that own arena allocations.
    mixed_roots: HashMap<u32, u32>,
    #[cfg(test)]
    chunks: Vec<u32>,
    gpu_distance: bool,
    blocks: Vec<u32>,
    columns: Vec<ColumnSummary>,
    refs: Vec<u32>,
    leaves: Arena,
    leaf_encoding: Encoding,
    block_slots: Arena,
    palette: Vec<Pigment>,
    pigments: HashMap<[u32; 4], u32>,
    palette_capacity: usize,
}
fn key(p: &Pigment) -> [u32; 4] {
    [
        p.rgb[0].to_bits(),
        p.rgb[1].to_bits(),
        p.rgb[2].to_bits(),
        p.material,
    ]
}
fn cell(p: Point) -> Cell {
    Cell {
        x: p[0] * 16,
        y: p[1] * 16,
        z: p[2] * 16,
    }
}
impl State {
    pub fn new_with_encoding(s: Snapshot, capacities: [usize; 3], leaf_encoding: Encoding) -> Self {
        let bounds = s.bounds();
        let region = Region {
            lo: bounds[0].map(|v| v as i64),
            hi: bounds[1].map(|v| v as i64),
        };
        let words_per_leaf = leaf_encoding.words_per_brick();
        let mut refs = vec![0u32; s.voxels.len() / words_per_leaf];
        for v in &s.blocks {
            if v & CHILD != 0 {
                refs[(v & MASK) as usize / 64] += 1;
            }
        }
        let pigments = s
            .palette
            .iter()
            .enumerate()
            .map(|(i, p)| (key(p), i as u32))
            .collect();
        let mut mixed_roots = HashMap::with_capacity(s.mixed_chunks);
        for (index, &word) in s.chunks.iter().enumerate() {
            if word & CHILD != 0 {
                mixed_roots.insert(index as u32, word);
            }
        }
        Self {
            region,
            n: s.chunks_per_edge as usize,
            ring: [0; 3],
            mixed_roots,
            #[cfg(test)]
            chunks: s.chunks,
            gpu_distance: s.gpu_distance,
            columns: s.columns,
            leaves: Arena::new(
                s.voxels.len() / words_per_leaf,
                capacities[1] / (words_per_leaf * 4),
            ),
            leaf_encoding,
            block_slots: Arena::new(s.blocks.len() / 64, capacities[0] / 256),
            blocks: s.blocks,
            refs,
            palette: s.palette,
            pigments,
            palette_capacity: capacities[2] / 16,
        }
    }
    fn index(&self, p: Point) -> usize {
        let c: [usize; 3] = std::array::from_fn(|a| {
            ((p[a] - self.region.lo[a] + self.ring[a] as i64).rem_euclid(self.n as i64)) as usize
        });
        (c[2] * self.n + c[0]) * self.n + c[1]
    }
    fn summary(&self, x: i64, z: i64) -> Option<ColumnSummary> {
        if x < self.region.lo[0]
            || x >= self.region.hi[0]
            || z < self.region.lo[2]
            || z >= self.region.hi[2]
        {
            return None;
        }
        let cx = (x - self.region.lo[0] + self.ring[0] as i64).rem_euclid(self.n as i64) as usize;
        let cz = (z - self.region.lo[2] + self.ring[2] as i64).rem_euclid(self.n as i64) as usize;
        Some(self.columns[cz * self.n + cx])
    }
    fn owner(&self, r: Region, updates: &HashMap<(i64, i64), ColumnSummary>) -> Arc<Ownership> {
        let safe: Vec<bool> = (0..self.n * self.n)
            .map(|i| {
                let x = r.lo[0] + (i % self.n) as i64;
                let z = r.lo[2] + (i / self.n) as i64;
                updates
                    .get(&(x, z))
                    .copied()
                    .or_else(|| self.summary(x, z))
                    .is_some_and(|s| {
                        s.fast && s.low > r.lo[1] * 16 + 1 && s.high < r.hi[1] * 16 - 1
                    })
            })
            .collect();
        Arc::new(Ownership::new(r.bounds(), self.n, &safe))
    }
    pub fn prepare(
        &mut self,
        world: &World,
        next: Region,
        limit: usize,
        cancel: &(dyn Fn() -> bool + Sync),
    ) -> Result<Change, String> {
        let start = Instant::now();
        let mut regions = vec![];
        for region in difference(next, self.region) {
            // Four independent strips share canonical source caches. Work stays
            // off the render thread and avoids rebuilding retained columns.
            let dims = region.dims();
            let axis = if dims[0] >= dims[2] { 0 } else { 2 };
            let width = dims[axis];
            for part in 0..4 {
                let mut r = region;
                r.lo[axis] += (width * part / 4) as i64;
                r.hi[axis] = region.lo[axis] + (width * (part + 1) / 4) as i64;
                if r.lo[axis] < r.hi[axis] {
                    regions.push(r);
                }
            }
        }
        let cached = |x, z| self.summary(x, z);
        let gpu_distance = self.gpu_distance;
        let mut snapshots = vec![];
        for batch in regions.chunks(4) {
            if cancel() {
                return Err("NAADF shift superseded".into());
            }
            let results: Vec<_> = std::thread::scope(|scope| {
                let handles: Vec<_> = batch
                    .iter()
                    .map(|&r| {
                        let cached = &cached;
                        scope.spawn(move || {
                            data::build_region(
                                world,
                                cell(r.lo),
                                r.dims(),
                                limit / 4,
                                &AtomicU32::new(0),
                                Some(cached),
                                gpu_distance,
                                cancel,
                            )
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| {
                        h.join()
                            .map_err(|_| "NAADF strip worker panicked".to_string())
                            .and_then(|r| r)
                    })
                    .collect()
            });
            for r in results {
                snapshots.push(r?);
            }
        }
        if cancel() {
            return Err("NAADF shift superseded".into());
        }
        let mut leaf_alloc = vec![];
        let mut block_alloc = vec![];
        let old_palette = self.palette.len();
        let result = (|| {
            let mut roots = vec![];
            let mut writes: Vec<Write> = vec![];
            let mut leaf_index = HashMap::<u64, (usize, usize)>::new();
            let mut shared_leaves = 0;
            let mut columns = HashMap::new();
            let mut generated = 0;
            for (region, mut s) in regions.into_iter().zip(snapshots) {
                generated += s.sampled_columns;
                let mut map = Vec::with_capacity(s.palette.len());
                for p in &s.palette {
                    let k = key(p);
                    let id = if let Some(v) = self.pigments.get(&k) {
                        *v
                    } else {
                        if self.palette.len() == self.palette_capacity {
                            return Err("NAADF streaming palette full".into());
                        }
                        let id = self.palette.len() as u32;
                        self.palette.push(*p);
                        self.pigments.insert(k, id);
                        id
                    };
                    map.push(id);
                }
                let bs = self.block_slots.take(s.blocks.len() / 64)?;
                block_alloc.extend_from_slice(&bs);
                for v in &mut s.voxels {
                    if *v & FULL != 0 {
                        *v = FULL | map[(*v & MASK) as usize];
                    }
                }
                self.leaf_encoding.pack(&mut s.voxels)?;
                // Share equal leaves across leading strips in the same
                // transaction. A hash only selects a candidate; equality
                // checks every packed word, including empty face distances.
                let words = self.leaf_encoding.words_per_brick();
                let mut ls = Vec::with_capacity(s.voxels.len() / words);
                let mut fresh = Vec::new();
                for leaf in s.voxels.chunks_exact(words) {
                    let mut hasher = DefaultHasher::new();
                    leaf.hash(&mut hasher);
                    let hash = hasher.finish();
                    let prior = leaf_index.get(&hash).and_then(|&(w, i)| {
                        (writes[w].data[i..i + words] == *leaf)
                            .then_some((writes[w].word + i) / words)
                    });
                    if let Some(slot) = prior {
                        ls.push(slot);
                        shared_leaves += 1;
                    } else {
                        fresh.push((ls.len(), hash));
                        ls.push(0);
                    }
                }
                let allocated = self.leaves.take(fresh.len())?;
                leaf_alloc.extend_from_slice(&allocated);
                let mut first = 0;
                while first < fresh.len() {
                    let mut last = first + 1;
                    while last < fresh.len() && allocated[last] == allocated[last - 1] + 1 {
                        last += 1;
                    }
                    let mut data = Vec::with_capacity((last - first) * words);
                    for (&(local, hash), &slot) in
                        fresh[first..last].iter().zip(&allocated[first..last])
                    {
                        ls[local] = slot;
                        leaf_index.entry(hash).or_insert((writes.len(), data.len()));
                        data.extend_from_slice(&s.voxels[local * words..(local + 1) * words]);
                    }
                    writes.push(Write {
                        kind: 1,
                        word: allocated[first] * words,
                        data,
                    });
                    first = last;
                }
                for v in &mut s.blocks {
                    if *v & CHILD != 0 {
                        *v = CHILD | (ls[(*v & MASK) as usize / 64] * 64) as u32;
                    } else if *v & FULL != 0 {
                        *v = FULL | map[(*v & MASK) as usize];
                    }
                }
                for v in &mut s.chunks {
                    if *v & CHILD != 0 {
                        *v = CHILD | (bs[(*v & MASK) as usize / 64] * 64) as u32;
                    } else if *v & FULL != 0 {
                        *v = FULL | map[(*v & MASK) as usize];
                    }
                }
                self.blocks.resize(self.block_slots.top * 64, 0);
                for (local, &slot) in bs.iter().enumerate() {
                    self.blocks[slot * 64..slot * 64 + 64]
                        .copy_from_slice(&s.blocks[local * 64..local * 64 + 64]);
                }
                {
                    let (slots, values, words) = (bs, s.blocks, 64);
                    let mut i = 0;
                    while i < slots.len() {
                        let mut j = i + 1;
                        while j < slots.len() && slots[j] == slots[j - 1] + 1 {
                            j += 1;
                        }
                        writes.push(Write {
                            kind: 0,
                            word: slots[i] * words,
                            data: values[i * words..j * words].to_vec(),
                        });
                        i = j;
                    }
                }
                for z in 0..s.dims[2] {
                    for x in 0..s.dims[0] {
                        columns.insert(
                            (region.lo[0] + x as i64, region.lo[2] + z as i64),
                            s.columns[z * s.dims[0] + x],
                        );
                    }
                }
                roots.push(Roots {
                    region,
                    nodes: s.chunks,
                });
            }
            if self.palette.len() > old_palette {
                writes.push(Write {
                    kind: 2,
                    word: old_palette * 4,
                    data: bytemuck::cast_slice::<Pigment, u32>(&self.palette[old_palette..])
                        .to_vec(),
                });
            }
            let owner = self.owner(next, &columns);
            let overlap = self.region.intersection(next);
            let mut old = self.region;
            old.lo[1] = overlap.lo[1];
            old.hi[1] = overlap.hi[1];
            let retiring = self.owner(old, &HashMap::new()).crop(overlap.bounds());
            let ring = std::array::from_fn(|a| {
                (self.ring[a] as i64 + next.lo[a] - self.region.lo[a]).rem_euclid(self.n as i64)
                    as u32
            });
            Ok(Change {
                region: next,
                ring,
                roots,
                writes,
                owner,
                retiring,
                build_ms: start.elapsed().as_secs_f64() * 1000.,
                generated_columns: generated,
                shared_leaves,
                palette_base: old_palette,
                leaf_slots: leaf_alloc.clone(),
                block_slots: block_alloc.clone(),
                columns,
            })
        })();
        if result.is_err() {
            self.rewind_palette(old_palette);
            for p in leaf_alloc {
                self.leaves.release(p, 1);
            }
            for p in block_alloc {
                self.block_slots.release(p, 1);
            }
        }
        result
    }
    fn rewind_palette(&mut self, len: usize) {
        self.palette.truncate(len);
        self.pigments.retain(|_, i| (*i as usize) < len);
    }
    pub fn finish(&mut self, change: Change, commit: bool) {
        if !commit {
            self.rewind_palette(change.palette_base);
            for p in change.leaf_slots {
                self.leaves.release(p, 1);
            }
            for p in change.block_slots {
                self.block_slots.release(p, 1);
            }
            return;
        }
        let n = self.n;
        let old = self.region;
        let ring = self.ring;
        let new = change.region;
        for (_, word) in self.mixed_roots.extract_if(|&index, _| {
            let index = index as usize;
            let slot = [(index / n) % n, index % n, index / (n * n)];
            (0..3).any(|axis| {
                if old.lo[axis] == new.lo[axis] {
                    return false;
                }
                let cell = old.lo[axis] + ((slot[axis] + n - ring[axis]) % n) as i64;
                cell < new.lo[axis] || cell >= new.hi[axis]
            })
        }) {
            let slot = (word & MASK) as usize / 64;
            for word in &self.blocks[slot * 64..slot * 64 + 64] {
                if word & CHILD != 0 {
                    let p = (*word & MASK) as usize / 64;
                    self.refs[p] -= 1;
                    if self.refs[p] == 0 {
                        self.leaves.release(p, 1);
                    }
                }
            }
            self.block_slots.release(slot, 1);
        }
        self.refs.resize(self.leaves.top, 0);
        for &slot in &change.block_slots {
            for word in &self.blocks[slot * 64..slot * 64 + 64] {
                if word & CHILD != 0 {
                    self.refs[(*word & MASK) as usize / 64] += 1;
                }
            }
        }
        for r in &change.roots {
            let [nx, ny, nz] = r.region.dims();
            for z in 0..nz {
                for x in 0..nx {
                    for y in 0..ny {
                        let word = r.nodes[(z * nx + x) * ny + y];
                        if !cfg!(test) && word & CHILD == 0 {
                            continue;
                        }
                        let p = [
                            r.region.lo[0] + x as i64,
                            r.region.lo[1] + y as i64,
                            r.region.lo[2] + z as i64,
                        ];
                        let index = self.index(p);
                        if word & CHILD != 0 {
                            debug_assert!(!self.mixed_roots.contains_key(&(index as u32)));
                            self.mixed_roots.insert(index as u32, word);
                        }
                        #[cfg(test)]
                        {
                            self.chunks[index] = word;
                        }
                    }
                }
            }
        }
        for ((x, z), s) in change.columns {
            let cx =
                (x - self.region.lo[0] + self.ring[0] as i64).rem_euclid(self.n as i64) as usize;
            let cz =
                (z - self.region.lo[2] + self.ring[2] as i64).rem_euclid(self.n as i64) as usize;
            self.columns[cz * self.n + cx] = s;
        }
        self.region = change.region;
        self.ring = change.ring.map(|v| v as usize);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn difference_is_disjoint_and_toroidal_window_follows_negative_coordinates() {
        let a = Region {
            lo: [-8; 3],
            hi: [8; 3],
        };
        let b = Region {
            lo: [-6, -10, -5],
            hi: [10, 6, 11],
        };
        let r = difference(b, a);
        for z in b.lo[2]..b.hi[2] {
            for y in b.lo[1]..b.hi[1] {
                for x in b.lo[0]..b.hi[0] {
                    let p = [x, y, z];
                    let inside = (0..3).all(|k| p[k] >= a.lo[k] && p[k] < a.hi[k]);
                    assert_eq!(
                        r.iter()
                            .filter(|r| (0..3).all(|k| p[k] >= r.lo[k] && p[k] < r.hi[k]))
                            .count(),
                        usize::from(!inside)
                    );
                }
            }
        }
        assert_eq!(target(a, [0.; 3]), None);
        assert!(target(a, [-3., 0., 0.]).unwrap().lo[0] < a.lo[0]);
    }
    #[test]
    fn arena_reclaims_and_coalesces() {
        let mut a = Arena::new(0, 16);
        assert_eq!(a.take(8).unwrap(), (0..8).collect::<Vec<_>>());
        a.release(2, 2);
        a.release(4, 2);
        assert_eq!(a.take(4).unwrap(), vec![2, 3, 4, 5]);
        assert!(a.take(20).is_err());
    }
    #[test]
    fn shifted_window_matches_fresh_voxels_and_pigments_across_wraps() {
        shifted_window_matches_fresh_voxels_and_pigments(false, 3);
    }
    #[test]
    fn shifted_wide_window_matches_fresh_voxels_and_pigments_across_wraps() {
        shifted_window_matches_fresh_voxels_and_pigments(true, 3);
    }
    fn shifted_window_matches_fresh_voxels_and_pigments(force_wide: bool, radius: u32) {
        let world = World::fbm(
            7,
            frontier_world::FbmParams {
                recipe: frontier_world::FbmRecipe::Alpine,
                ..Default::default()
            },
        )
        .unwrap()
        .with_vegetation();
        let eye = [-41., world.height_m(-41., -250.) + 1.7, -250.];
        let build = |eye| {
            data::build(
                &world,
                eye,
                radius,
                128 * 1048576,
                &AtomicU32::new(0),
                || false,
            )
            .unwrap()
        };
        let prepared = super::super::leaf::Prepared::new(build(eye), force_wide);
        let encoding = prepared.encoding;
        assert_eq!(encoding.is_compact(), !force_wide);
        let first = prepared.snapshot;
        let mut voxels = first.voxels.clone();
        let mut state =
            State::new_with_encoding(first, [16 * 1048576, 32 * 1048576, 1048576], encoding);
        let mut shared = 0;
        let mut total = 0;
        for delta in [[2, 0, -2], [-2, 2, 2], [-2, -2, 2], [2, 0, -2]] {
            let next = Region {
                lo: std::array::from_fn(|a| state.region.lo[a] + delta[a]),
                hi: std::array::from_fn(|a| state.region.hi[a] + delta[a]),
            };
            let aborted = state
                .prepare(&world, next, 128 * 1048576, &|| false)
                .unwrap();
            state.finish(aborted, false);
            let c = state
                .prepare(&world, next, 128 * 1048576, &|| false)
                .unwrap();
            if delta[1] == 0 {
                assert!(c.generated_columns < (state.n * 16).pow(2));
            }
            shared += c.shared_leaves;
            total += c.shared_leaves;
            for w in &c.writes {
                if w.kind == 1 {
                    total += w.data.len() / encoding.words_per_brick();
                    voxels.resize(voxels.len().max(w.word + w.data.len()), 0);
                    voxels[w.word..w.word + w.data.len()].copy_from_slice(&w.data);
                }
            }
            state.finish(c, true);
            let reference = build(next.lo.map(|v| v as f64 + radius as f64));
            let n = state.n * 16;
            for z in (0..n).step_by(3) {
                for x in (0..n).step_by(3) {
                    for y in (0..n).step_by(2) {
                        let global = [
                            next.lo[0] + (x / 16) as i64,
                            next.lo[1] + (y / 16) as i64,
                            next.lo[2] + (z / 16) as i64,
                        ];
                        let index = state.index(global);
                        let mut v = state.chunks[index];
                        assert_eq!(
                            state.mixed_roots.get(&(index as u32)).copied(),
                            (v & CHILD != 0).then_some(v)
                        );
                        if v & CHILD != 0 {
                            v = state.blocks[(v & MASK) as usize
                                + (x % 16) / 4
                                + 4 * ((y % 16) / 4 + 4 * ((z % 16) / 4))];
                            if v & CHILD != 0 {
                                v = encoding.load(
                                    &voxels,
                                    (v & MASK) as usize + x % 4 + 4 * (y % 4 + 4 * (z % 4)),
                                );
                            }
                        }
                        let expected = reference.material([x, y, z]);
                        let value = |v: u32, p: &[Pigment]| {
                            (v & FULL != 0).then(|| key(&p[(v & MASK) as usize]))
                        };
                        assert_eq!(
                            value(v, &state.palette),
                            value(expected, &reference.palette),
                            "at {global:?} + {x},{y},{z}"
                        );
                    }
                }
            }
        }
        assert!(shared > 0);
        eprintln!("NAADF seed-7 Alpine radius {radius} m, four wrapped shifts: {shared}/{total} incoming leaves shared ({} bytes avoided, {}-bit leaves)",
            shared * encoding.words_per_brick() * 4, encoding.bits());
    }
}
