//! Greenfield bounded octant empty-box cache, inspired by directional empty-space
//! bounds in NAADF (https://github.com/cg-tuwien/NAADF). No reference code copied.
//! Unlike NAADF this is one brick layer, xyz-greedy boxes, and no voxel AADF.
use super::BrickSnapshot;

impl BrickSnapshot {
    /// Rebuild only from this exact immutable occupancy snapshot, before GPU
    /// publication. Edits must build/publish a fresh snapshot; never reuse these
    /// bounds with edited headers. An absent cache uses ordinary brick traversal.
    pub fn build_directional_bounds(&mut self) {
        self.invalidate_directional_bounds();
        let started = std::time::Instant::now();
        let dims = self.brick_size;
        let stride = dims.map(|n| n + 1);
        let index = |p: [usize; 3]| (p[2] * stride[1] + p[1]) * stride[0] + p[0];
        // Summed occupancy permits exact constant-time slab emptiness queries.
        let mut prefix = vec![0u32; stride.iter().product()];
        self.directional_scratch_bytes = prefix.len() * 4;
        for z in 1..=dims[2] {
            for y in 1..=dims[1] {
                for x in 1..=dims[0] {
                    let bi = ((z - 1) * dims[1] + y - 1) * dims[0] + x - 1;
                    let v = u32::from(self.headers[bi * 2] != 0)
                        + prefix[index([x - 1, y, z])]
                        + prefix[index([x, y - 1, z])]
                        + prefix[index([x, y, z - 1])]
                        + prefix[index([x - 1, y - 1, z - 1])];
                    prefix[index([x, y, z])] = v
                        - prefix[index([x - 1, y - 1, z])]
                        - prefix[index([x - 1, y, z - 1])]
                        - prefix[index([x, y - 1, z - 1])];
                }
            }
        }
        let empty = |lo: [usize; 3], hi: [usize; 3]| {
            // Signed arithmetic keeps inclusion/exclusion intermediates valid.
            let mut sum = 0i64;
            for corner in 0u32..8 {
                let p = std::array::from_fn(|a| if corner & (1 << a) == 0 { hi[a] } else { lo[a] });
                let sign = if corner.count_ones() % 2 == 0 { 1 } else { -1 };
                sum += sign * prefix[index(p)] as i64;
            }
            sum == 0
        };
        let mut bounds = vec![0u32; self.headers.len() / 2 * 8];
        for z in 0..dims[2] {
            for y in 0..dims[1] {
                for x in 0..dims[0] {
                    let b = [x, y, z];
                    let bi = (z * dims[1] + y) * dims[0] + x;
                    if self.headers[bi * 2] != 0 {
                        continue;
                    }
                    for octant in 0..8 {
                        let mut lo = b;
                        let mut hi = b.map(|n| n + 1);
                        for a in 0..3 {
                            let positive = octant & (1 << a) != 0;
                            let limit = if positive { dims[a] - hi[a] } else { lo[a] };
                            let mut good = 0;
                            let mut bad = limit + 1;
                            while good + 1 < bad {
                                let n = (good + bad) / 2;
                                let mut qlo = lo;
                                let mut qhi = hi;
                                if positive {
                                    qhi[a] += n;
                                } else {
                                    qlo[a] -= n;
                                }
                                if empty(qlo, qhi) {
                                    good = n;
                                } else {
                                    bad = n;
                                }
                            }
                            if positive {
                                hi[a] += good;
                            } else {
                                lo[a] -= good;
                            }
                        }
                        let length: [usize; 3] = std::array::from_fn(|a| hi[a] - lo[a]);
                        bounds[bi * 8 + octant] =
                            length[0] as u32 | (length[1] as u32) << 10 | (length[2] as u32) << 20;
                    }
                }
            }
        }
        self.directional_bounds = bounds;
        self.directional_build_ms = started.elapsed().as_secs_f64() * 1000.;
    }

    pub fn invalidate_directional_bounds(&mut self) {
        self.directional_bounds.clear();
        self.directional_build_ms = 0.;
        self.directional_scratch_bytes = 0;
    }

    pub fn directional_bytes(&self) -> usize {
        self.directional_bounds.len() * 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::{Cell, Material};

    fn prove_boxes(snapshot: &BrickSnapshot) {
        let dims = snapshot.brick_size;
        for bi in 0..snapshot.headers.len() / 2 {
            let b = [
                bi % dims[0],
                bi / dims[0] % dims[1],
                bi / (dims[0] * dims[1]),
            ];
            for octant in 0..8 {
                let packed = snapshot.directional_bounds[bi * 8 + octant];
                if snapshot.headers[bi * 2] != 0 {
                    assert_eq!(packed, 0, "occupied bricks must never have skip bounds");
                    continue;
                }
                let length: [usize; 3] =
                    std::array::from_fn(|a| ((packed >> (a * 10)) & 1023) as usize);
                let lo: [usize; 3] = std::array::from_fn(|a| {
                    assert!((1..=dims[a]).contains(&length[a]));
                    if octant & (1 << a) != 0 {
                        b[a]
                    } else {
                        b[a] + 1 - length[a]
                    }
                });
                let hi: [usize; 3] = std::array::from_fn(|a| lo[a] + length[a]);
                for a in 0..3 {
                    assert!(lo[a] <= b[a] && b[a] < hi[a] && hi[a] <= dims[a]);
                }
                for z in lo[2]..hi[2] {
                    for y in lo[1]..hi[1] {
                        for x in lo[0]..hi[0] {
                            let j = (z * dims[1] + y) * dims[0] + x;
                            assert_eq!(
                                snapshot.headers[j * 2],
                                0,
                                "box crosses occupancy: bi={bi} octant={octant} xyz={x},{y},{z}"
                            );
                        }
                    }
                }
            }
        }
        // Every region traversed by a skip consists entirely of empty bricks;
        // this proof covers every ray direction/entry point, including tied axes.
    }

    #[test]
    fn random_occupancy_octant_boxes_are_empty_including_partial_boundaries() {
        for seed in 0..32u64 {
            let size = [41, 35, 25];
            let mut s = BrickSnapshot::build(
                |p| {
                    if p.x < 0 || p.y < 0 || p.z < 0 {
                        return Material::Air;
                    }
                    let [x, y, z] = [p.x as u64, p.y as u64, p.z as u64];
                    let hash = (x / 8).wrapping_mul(73856093)
                        ^ (y / 8).wrapping_mul(19349663)
                        ^ (z / 8).wrapping_mul(83492791)
                        ^ seed.wrapping_mul(997);
                    if hash % 11 == 0 && x % 8 == seed % 8 && y % 8 == (seed / 2) % 8 {
                        Material::Wood
                    } else {
                        Material::Air
                    }
                },
                Cell::default(),
                size,
            );
            assert!(s.directional_bounds.is_empty());
            s.build_directional_bounds();
            prove_boxes(&s);
            assert_eq!(s.directional_bytes(), s.headers.len() / 2 * 32);
            assert_eq!(
                s.directional_scratch_bytes,
                s.brick_size.map(|n| n + 1).iter().product::<usize>() * 4
            );
        }
    }

    #[test]
    fn adding_and_removing_on_brick_and_crop_boundaries_rebuilds_bounds() {
        let size = [33, 25, 17];
        let min = Cell {
            x: -17,
            y: -21,
            z: -9,
        };
        let mut edits = std::collections::HashSet::new();
        for p in [[0, 0, 0], [7, 8, 16], [8, 7, 8], [32, 24, 16], [16, 16, 0]] {
            edits.insert(p);
            let mut next = BrickSnapshot::build(
                |c| {
                    if edits.contains(&[c.x - min.x, c.y - min.y, c.z - min.z]) {
                        Material::Stone
                    } else {
                        Material::Air
                    }
                },
                min,
                size,
            );
            assert!(
                next.directional_bounds.is_empty(),
                "new revision starts without acceleration"
            );
            next.build_directional_bounds();
            prove_boxes(&next);
            next.invalidate_directional_bounds();
            assert!(next.directional_bounds.is_empty());
            assert_eq!(next.directional_bytes(), 0);
        }
        for p in [[8, 7, 8], [0, 0, 0], [32, 24, 16], [7, 8, 16], [16, 16, 0]] {
            edits.remove(&p);
            let mut next = BrickSnapshot::build(
                |c| {
                    if edits.contains(&[c.x - min.x, c.y - min.y, c.z - min.z]) {
                        Material::Stone
                    } else {
                        Material::Air
                    }
                },
                min,
                size,
            );
            next.build_directional_bounds();
            prove_boxes(&next);
        }
    }

    #[test]
    fn empty_scene_boxes_cover_each_complete_forward_octant() {
        let mut s = BrickSnapshot::build(|_| Material::Air, Cell::default(), [35, 29, 23]);
        s.build_directional_bounds();
        prove_boxes(&s);
        for (bi, packed) in s.directional_bounds.chunks_exact(8).enumerate() {
            let d = s.brick_size;
            let b = [bi % d[0], bi / d[0] % d[1], bi / (d[0] * d[1])];
            for octant in 0..8 {
                for a in 0..3 {
                    let expected = if octant & (1 << a) != 0 {
                        d[a] - b[a]
                    } else {
                        b[a] + 1
                    };
                    assert_eq!(((packed[octant] >> (a * 10)) & 1023) as usize, expected);
                }
            }
        }
    }
}
