//! World-space relief with geometry down to native voxel spacing.
//! Neighboring tiles choose their shared edge from the same world-space hash.
use super::{hash, PITCH_CELLS};
use std::sync::OnceLock;

const SIDE: usize = PITCH_CELLS as usize;
const TILES: usize = 256;
pub const NODES: usize = (4 * SIDE * SIDE - 1) / 3;
const AMPLITUDE: [i32; 7] = [16, 14, 9, 6, 4, 2, 1];
pub const HEIGHT_LIMIT: i64 = 55;

pub fn tile_index([x, z]: [i64; 2], seed: u32) -> usize {
    let horizontal = seed ^ 0x1777_2e67;
    let vertical = seed ^ 0xad69_94e9;
    ((hash(x, z, horizontal) & 3)
        | ((hash(x, z + 1, horizontal) & 3) << 2)
        | ((hash(x, z, vertical) & 3) << 4)
        | ((hash(x + 1, z, vertical) & 3) << 6)) as usize
}

pub fn roof(tile: usize) -> i64 {
    i64::from(atlas()[tile * NODES] as i8)
}

pub fn column(tile: usize, x: usize, z: usize) -> i64 {
    i64::from(atlas()[tile * NODES + offset(SIDE) + z * SIDE + x] as i8)
}

/// Each node packs its exact upper bound and mean, both in signed voxel units.
pub fn atlas() -> &'static [u16] {
    static CACHE: OnceLock<Vec<u16>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut all = vec![0; TILES * NODES];
        std::thread::scope(|scope| {
            for (group, chunk) in all.chunks_mut(32 * NODES).enumerate() {
                scope.spawn(move || {
                    for (i, tile) in chunk.chunks_mut(NODES).enumerate() {
                        fill_tile(group * 32 + i, tile);
                    }
                });
            }
        });
        all
    })
}

fn offset(side: usize) -> usize {
    (side * side - 1) / 3
}

fn displaced(value: i32, amplitude: i32, seed: u32, x: usize, z: usize) -> i32 {
    value + (hash(x as i64, z as i64, seed) * amplitude + 64).div_euclid(128)
}

fn edge_curve(code: usize, vertical: bool) -> [i32; SIDE + 1] {
    let mut edge = [0i32; SIDE + 1];
    let salt = if vertical { 0xc582_34a1 } else { 0x481a_60d7 };
    let seed = (code as u32).wrapping_mul(0xf194_d759) ^ salt;
    for (level, amplitude) in AMPLITUDE.into_iter().enumerate() {
        let width = SIDE >> level;
        for from in (0..SIDE).step_by(width) {
            let mid = from + width / 2;
            edge[mid] = displaced(
                (edge[from] + edge[from + width]).div_euclid(2),
                amplitude,
                seed,
                mid,
                level,
            );
        }
    }
    edge
}

fn fill_tile(id: usize, nodes: &mut [u16]) {
    let width = SIDE + 1;
    let mut points = vec![0i32; width * width];
    let [north, south, west, east] =
        std::array::from_fn(|i| edge_curve((id >> (2 * i)) & 3, i >= 2));
    for i in 0..width {
        points[i] = north[i];
        points[SIDE * width + i] = south[i];
        points[i * width] = west[i];
        points[i * width + SIDE] = east[i];
    }
    let seed = (id as u32).wrapping_mul(0x8329_7311) ^ 0x7ebc_b41d;
    for (level, amplitude) in AMPLITUDE.into_iter().enumerate() {
        let stride = SIDE >> level;
        let half = stride / 2;
        // Midpoints are shared by both new children. The boundary is fixed.
        for z in (stride..SIDE).step_by(stride) {
            for x in (half..SIDE).step_by(stride) {
                let i = z * width + x;
                points[i] = displaced(
                    (points[i - half] + points[i + half]).div_euclid(2),
                    amplitude,
                    seed ^ 0x5b93_90f7,
                    x,
                    z,
                );
            }
        }
        for z in (half..SIDE).step_by(stride) {
            for x in (stride..SIDE).step_by(stride) {
                let i = z * width + x;
                points[i] = displaced(
                    (points[i - half * width] + points[i + half * width]).div_euclid(2),
                    amplitude,
                    seed ^ 0xb7d8_4213,
                    x,
                    z,
                );
            }
        }
        for z in (half..SIDE).step_by(stride) {
            for x in (half..SIDE).step_by(stride) {
                let a = (z - half) * width + x - half;
                let b = a + stride * width;
                let mean =
                    (points[a] + points[a + stride] + points[b] + points[b + stride]).div_euclid(4);
                points[z * width + x] = displaced(mean, amplitude, seed, x, z);
            }
        }
    }
    let mut sums = vec![0i32; SIDE * SIDE];
    for z in 0..SIDE {
        for x in 0..SIDE {
            let i = z * width + x;
            let avg = (points[i] + points[i + 1] + points[i + width] + points[i + width + 1])
                .div_euclid(4);
            let height = displaced(avg, 3, seed ^ 0x3a08_c8bb, x, z);
            debug_assert!(i64::from(height.abs()) <= HEIGHT_LIMIT);
            sums[z * SIDE + x] = height;
            nodes[offset(SIDE) + z * SIDE + x] = pack(height, height);
        }
    }
    let mut n = SIDE;
    let mut count = 1;
    while n > 1 {
        let m = n / 2;
        count *= 4;
        for z in 0..m {
            for x in 0..m {
                let child = 2 * (z * n + x);
                let sum = sums[child] + sums[child + 1] + sums[child + n] + sums[child + n + 1];
                let max = [child, child + 1, child + n, child + n + 1]
                    .map(|i| nodes[offset(n) + i] as i8 as i32)
                    .into_iter()
                    .max()
                    .unwrap();
                nodes[offset(m) + z * m + x] = pack(max, (sum + count / 2).div_euclid(count));
                sums[z * m + x] = sum;
            }
        }
        n = m;
    }
}

fn pack(max: i32, mean: i32) -> u16 {
    u16::from(max as u8) | (u16::from(mean as u8) << 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_bounds_and_shared_edges_hold_at_all_scales() {
        let a = atlas();
        for index in 0..TILES {
            let nodes = &a[index * NODES..][..NODES];
            for level in 0..7 {
                let n = SIDE >> level;
                let m = n / 2;
                for z in 0..m {
                    for x in 0..m {
                        let i = offset(n) + 2 * (z * n + x);
                        let hi = [i, i + 1, i + n, i + n + 1]
                            .into_iter()
                            .map(|id| nodes[id] as i8)
                            .max()
                            .unwrap();
                        assert_eq!(hi, nodes[offset(m) + z * m + x] as i8);
                    }
                }
            }
            assert!((-HEIGHT_LIMIT..=HEIGHT_LIMIT).contains(&roof(index)));
        }
        for z in -19..19 {
            for x in -19..19 {
                let a = tile_index([x, z], 72);
                let b = tile_index([x + 1, z], 72);
                let c = tile_index([x, z + 1], 72);
                assert_eq!(a >> 6, (b >> 4) & 3);
                assert_eq!((a >> 2) & 3, c & 3);
            }
        }
    }
}
