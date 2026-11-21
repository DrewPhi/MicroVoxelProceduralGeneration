//! World-aligned, revision-owned source certificates for coarse voxel tiles.
//! Tile width and edge stitching do not affect the identity of a source bound.
use frontier_world::{
    affine_bounds::{bound_native_height, BoundBudget, ColumnBox, HeightBound},
    FbmTerrain,
};
use std::{
    mem::size_of,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed},
        Mutex,
    },
};

const SIDE: i64 = 8;
const CELLS: usize = (SIDE * SIDE) as usize;
const SHARDS: usize = 32;
const WAYS: usize = 4;

// Adaptive roofs use f32 coordinates relative to tiles up to 2^20 cells
// across. A decimal pitch can round a corner across the nominal patch edge.
// Enclose the adjacent native columns before evaluating the affine source.
pub(super) fn raster_bound(
    terrain: &FbmTerrain,
    region: ColumnBox,
    budget: BoundBudget,
) -> Option<HeightBound> {
    let pitch = frontier_world::voxel_size();
    let region = raster_domain(region, pitch)?;
    bound_native_height(terrain, region, budget)
}

fn raster_domain(region: ColumnBox, pitch: f64) -> Option<ColumnBox> {
    if pitch.to_bits() & 0x000f_ffff_ffff_ffff == 0 || region.size_cells.contains(&0) {
        return Some(region);
    }
    Some(ColumnBox {
        min: [region.min[0].checked_sub(1)?, region.min[1].checked_sub(1)?],
        size_cells: [
            region.size_cells[0].checked_add(2)?,
            region.size_cells[1].checked_add(2)?,
        ],
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Address {
    page: [i64; 2],
    pitch: u32,
}

struct Page {
    address: Address,
    last_used: u64,
    ready: u64,
    bounds: [Option<HeightBound>; CELLS],
}

#[derive(Default)]
struct Shard {
    sets: Vec<[Option<Box<Page>>; WAYS]>,
    clock: u64,
}

#[derive(Default)]
pub(super) struct Stats {
    pub(super) hits: u64,
    pub(super) misses: u64,
    pub(super) evictions: u64,
    pub(super) batches: u64,
    pub(super) bytes: usize,
    pub(super) lock_ns: u64,
    pub(super) evaluation_ns: u64,
}

pub(super) struct Cache {
    shards: [Mutex<Shard>; SHARDS],
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
    batches: AtomicU64,
    bytes: AtomicUsize,
    lock_ns: AtomicU64,
    evaluation_ns: AtomicU64,
    profile: bool,
}

impl Cache {
    pub(super) fn new(budget: usize) -> Option<Self> {
        let set_bytes = size_of::<[Option<Box<Page>>; WAYS]>() + WAYS * size_of::<Page>();
        let sets = budget.saturating_sub(size_of::<Self>()) / (SHARDS * set_bytes);
        if sets == 0 {
            return None;
        }
        Some(Self {
            shards: std::array::from_fn(|_| {
                Mutex::new(Shard {
                    sets: (0..sets).map(|_| std::array::from_fn(|_| None)).collect(),
                    ..Default::default()
                })
            }),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            batches: AtomicU64::new(0),
            lock_ns: AtomicU64::new(0),
            evaluation_ns: AtomicU64::new(0),
            profile: std::env::var_os("FRONTIER_PROFILE_SOURCE").is_some_and(|v| v == "1"),
            bytes: AtomicUsize::new(
                size_of::<Self>() + SHARDS * sets * size_of::<[Option<Box<Page>>; WAYS]>(),
            ),
        })
    }

    pub(super) fn stats(&self) -> Stats {
        Stats {
            hits: self.hits.load(Relaxed),
            misses: self.misses.load(Relaxed),
            evictions: self.evictions.load(Relaxed),
            batches: self.batches.load(Relaxed),
            bytes: self.bytes.load(Relaxed),
            lock_ns: self.lock_ns.load(Relaxed),
            evaluation_ns: self.evaluation_ns.load(Relaxed),
        }
    }

    pub(super) fn clear(&self) {
        for shard in &self.shards {
            let mut shard = shard.lock().unwrap();
            for set in &mut shard.sets {
                for page in set.iter_mut().flatten() {
                    page.ready = 0;
                }
            }
        }
    }

    /// Request a row segment at a time. Check cancellation before each batch
    /// and hold each lock for at most eight exact source analyses.
    pub(super) fn tile(
        &self,
        terrain: &FbmTerrain,
        min: [i64; 2],
        width: u32,
        pitch: u32,
        cancelled: &mut impl FnMut() -> bool,
    ) -> Result<Option<Vec<Option<HeightBound>>>, String> {
        let step = i64::from(pitch);
        if min.iter().any(|v| v.rem_euclid(step) != 0) {
            return Ok(None);
        }
        let side = i64::from(width / pitch);
        let [x0, z0] = min.map(|v| v.div_euclid(step));
        let mut bounds = Vec::with_capacity((side * side) as usize);
        for z in z0..z0 + side {
            let mut x = x0;
            while x < x0 + side {
                if cancelled() {
                    return Err("adaptive geometry cancelled".into());
                }
                let address = Address {
                    page: [x.div_euclid(SIDE), z.div_euclid(SIDE)],
                    pitch,
                };
                let local_x = x.rem_euclid(SIDE);
                let count = (SIDE - local_x).min(x0 + side - x);
                let first = (z.rem_euclid(SIDE) * SIDE + local_x) as usize;
                self.row(address, first, count as usize, terrain, &mut bounds);
                x += count;
            }
        }
        Ok(Some(bounds))
    }

    fn row(
        &self,
        address: Address,
        first: usize,
        count: usize,
        terrain: &FbmTerrain,
        out: &mut Vec<Option<HeightBound>>,
    ) {
        let key = (address.page[0] as u64).wrapping_mul(0x9e3779b97f4a7c15)
            ^ (address.page[1] as u64).wrapping_mul(0xbf58476d1ce4e5b9)
            ^ u64::from(address.pitch).wrapping_mul(0x94d049bb133111eb);
        let hash = (key ^ (key >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        let hash = hash ^ (hash >> 29);
        let start = self.profile.then(std::time::Instant::now);
        let mut lock = self.shards[hash as usize % SHARDS].lock().unwrap();
        if let Some(start) = start {
            self.lock_ns
                .fetch_add(start.elapsed().as_nanos() as u64, Relaxed);
        }
        let shard = &mut *lock;
        shard.clock = shard.clock.wrapping_add(1);
        let set_count = shard.sets.len();
        let set = &mut shard.sets[(hash >> 32) as usize % set_count];
        let page = if let Some(page) = set.iter_mut().flatten().find(|p| p.address == address) {
            page
        } else {
            let slot = set
                .iter_mut()
                .min_by_key(|p| p.as_ref().map(|p| p.last_used))
                .unwrap();
            if slot.is_some() {
                self.evictions.fetch_add(1, Relaxed);
            } else {
                self.bytes.fetch_add(size_of::<Page>(), Relaxed);
                *slot = Some(Box::new(Page {
                    address,
                    last_used: 0,
                    ready: 0,
                    bounds: [None; CELLS],
                }));
            }
            let page = slot.as_mut().unwrap();
            page.address = address;
            page.ready = 0;
            page
        };
        page.last_used = shard.clock;
        let mut misses = 0;
        let start = self.profile.then(std::time::Instant::now);
        for index in first..first + count {
            let bit = 1_u64 << index;
            if page.ready & bit == 0 {
                let [dx, dz] = [index as i64 % SIDE, index as i64 / SIDE];
                let step = i64::from(address.pitch);
                page.bounds[index] = raster_bound(
                    terrain,
                    ColumnBox {
                        min: [
                            (address.page[0] * SIDE + dx) * step,
                            (address.page[1] * SIDE + dz) * step,
                        ],
                        size_cells: [address.pitch; 2],
                    },
                    BoundBudget::default(),
                );
                page.ready |= bit;
                misses += 1;
            }
            out.push(page.bounds[index]);
        }
        drop(lock);
        if let Some(start) = start.filter(|_| misses > 0) {
            self.evaluation_ns
                .fetch_add(start.elapsed().as_nanos() as u64, Relaxed);
        }
        self.batches.fetch_add(1, Relaxed);
        self.misses.fetch_add(misses, Relaxed);
        self.hits.fetch_add(count as u64 - misses, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_decimal_roofs_fit_their_source_domain_across_large_tiles() {
        let tile = -1048576i64;
        for pitch in [0.03, 0.1] {
            let mut escaped = 0;
            for offset in [0, 7, 127, 16383, 1048560] {
                let patch = ColumnBox {
                    min: [tile + offset; 2],
                    size_cells: [8; 2],
                };
                let domain = raster_domain(patch, pitch).unwrap();
                let low = (offset as f64 * pitch) as f32;
                let high = ((offset + 8) as f64 * pitch) as f32;
                for edge in [low, low + (high - low)] {
                    let x = tile as f64 * pitch + f64::from(edge);
                    let lo = patch.min[0] as f64 * pitch;
                    let hi = (patch.min[0] + 8) as f64 * pitch;
                    escaped += usize::from(x < lo || x > hi);
                    let expanded = domain.min[0] as f64 * pitch
                        ..=(domain.min[0] + i64::from(domain.size_cells[0])) as f64 * pitch;
                    assert!(expanded.contains(&x));
                }
                let binary = raster_domain(patch, 0.0625).unwrap();
                assert_eq!(binary.min, patch.min);
                assert_eq!(binary.size_cells, patch.size_cells);
            }
            assert!(escaped > 0);
        }
    }

    fn bits(bound: HeightBound) -> ([u64; 10], u32, u32) {
        let [x, z] = bound.center_xz;
        let [rx, rz] = bound.half_extent_m;
        let [a, b] = bound.coefficients;
        let [lo, hi] = bound.height_range;
        (
            [
                x,
                z,
                rx,
                rz,
                bound.center_height_m,
                a,
                b,
                bound.remainder_m,
                lo,
                hi,
            ]
            .map(f64::to_bits),
            bound.noise_cells,
            bound.full_range_fallbacks,
        )
    }

    #[test]
    fn overlapping_footprints_reuse_exact_bounds_across_page_edges() {
        let terrain = FbmTerrain::new(7, Default::default()).unwrap();
        let budget = 2 * 1024 * 1024;
        let cache = Cache::new(budget).unwrap();
        let min = [-96, -32];
        let pitch = 16;
        let large = cache
            .tile(&terrain, min, 256, pitch, &mut || false)
            .unwrap()
            .unwrap();
        let nested = cache
            .tile(&terrain, min, 128, pitch, &mut || false)
            .unwrap()
            .unwrap();
        for (i, bound) in nested.into_iter().enumerate() {
            let x = i % 8;
            let z = i / 8;
            let direct = bound_native_height(
                &terrain,
                ColumnBox {
                    min: [min[0] + x as i64 * 16, min[1] + z as i64 * 16],
                    size_cells: [pitch; 2],
                },
                BoundBudget::default(),
            );
            assert_eq!(bound.map(bits), large[z * 16 + x].map(bits));
            assert_eq!(bound.map(bits), direct.map(bits));
        }
        assert_eq!(cache.stats().misses, 256);
        assert_eq!(cache.stats().hits, 64);
        assert!(cache.stats().bytes <= budget);
        assert!(cache.tile(&terrain, min, 128, pitch, &mut || true).is_err());
        cache.clear();
        cache
            .tile(&terrain, min, 128, pitch, &mut || false)
            .unwrap();
        assert_eq!(cache.stats().misses, 320);
    }
}
