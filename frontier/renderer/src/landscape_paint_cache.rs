//! Cache final world-aligned Highlands normals and albedo across mesh builds.
//! Each cache belongs to one immutable procedural world and native voxel grid.
use frontier_world::FbmTerrain;
use std::{mem::size_of, sync::Mutex};

pub(super) type Paint = ([f32; 3], [f32; 3]);

pub(super) fn sample(
    terrain: &FbmTerrain,
    [x, z]: [i64; 2],
    radius: i64,
    vegetation: bool,
) -> Paint {
    let dx = terrain.height_cells(x - radius, z) - terrain.height_cells(x + radius, z);
    let dz = terrain.height_cells(x, z - radius) - terrain.height_cells(x, z + radius);
    let n = glam::Vec3::new(dx as f32, (2 * radius) as f32, dz as f32).normalize();
    let (col, color) = terrain.landscape_column(x, z, vegetation);
    let land = if col.visible_top > col.ground_top {
        col.ground_surface.color()
    } else {
        color
    };
    (n.to_array(), land)
}

#[derive(Clone, Copy)]
struct Entry<T> {
    at: [i64; 2],
    paint: T,
}

struct Shard<T> {
    entries: Box<[Option<Entry<T>>]>,
    hits: u64,
    misses: u64,
}

pub(super) struct Cache<T = Paint> {
    shards: [Mutex<Shard<T>>; 32],
    pub(super) bytes: usize,
}

impl<T: Copy> Cache<T> {
    pub(super) fn new(budget: usize) -> Option<Self> {
        // Charge allocation overhead on every shard plus the owning Arc.
        let fixed = size_of::<Self>() + 64 * 33;
        let slots = budget.saturating_sub(fixed) / (32 * size_of::<Option<Entry<T>>>());
        if slots == 0 {
            return None;
        }
        Some(Self {
            bytes: fixed + 32 * slots * size_of::<Option<Entry<T>>>(),
            shards: std::array::from_fn(|_| {
                Mutex::new(Shard {
                    entries: vec![None; slots].into_boxed_slice(),
                    hits: 0,
                    misses: 0,
                })
            }),
        })
    }

    pub(super) fn get(&self, at: [i64; 2], compute: impl FnOnce() -> T) -> T {
        // Mix high bits down: adjacent flight anchors are often thousands of
        // native cells apart along both axes.
        let h = (at[0] as u64).wrapping_mul(0x9e3779b97f4a7c15)
            ^ (at[1] as u64)
                .wrapping_mul(0xbf58476d1ce4e5b9)
                .rotate_left(29);
        let h = (h ^ (h >> 31)).wrapping_mul(0x94d049bb133111eb);
        let h = h ^ (h >> 30);
        let mut shard = self.shards[(h & 31) as usize].lock().unwrap();
        let slot = (h >> 5) as usize % shard.entries.len();
        if let Some(entry) = shard.entries[slot].filter(|e| e.at == at) {
            shard.hits += 1;
            return entry.paint;
        }
        let paint = compute();
        shard.entries[slot] = Some(Entry { at, paint });
        shard.misses += 1;
        paint
    }

    pub(super) fn stats(&self) -> (u64, u64) {
        self.shards.iter().fold((0, 0), |(hits, misses), shard| {
            let shard = shard.lock().unwrap();
            (hits + shard.hits, misses + shard.misses)
        })
    }

    pub(super) fn clear(&self) {
        for shard in &self.shards {
            shard.lock().unwrap().entries.fill(None);
        }
    }
}
