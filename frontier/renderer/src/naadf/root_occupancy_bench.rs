//! Synthetic CPU gate only: exact root geometry, no terrain-generation/FPS claim.
use super::*;
use std::time::Instant;

fn regions(next: Region, old: Region) -> Vec<Region> {
    let mut core = next;
    let mut result = vec![];
    for a in 0..3 {
        if core.lo[a] < old.lo[a] {
            let mut r = core;
            r.hi[a] = old.lo[a];
            result.push(r);
            core.lo[a] = old.lo[a];
        }
        if core.hi[a] > old.hi[a] {
            let mut r = core;
            r.lo[a] = old.hi[a];
            result.push(r);
            core.hi[a] = old.hi[a];
        }
    }
    result
}

// Frozen seed construction follows the existing deterministic six-face Jacobi
// contract, independent of the occupancy hierarchy used by the candidates.
fn seed(mut words: Vec<u32>, dims: [usize; 3]) -> Vec<u32> {
    let strides = [dims[1], 1, dims[0] * dims[1]];
    let mut scratch = words.clone();
    for _ in 0..31 {
        let mut changed = false;
        for axis in 0..3 {
            for i in 0..words.len() {
                let old = words[i];
                let mut out = old;
                if old & OCCUPIED == 0 {
                    let coordinate = i / strides[axis] % dims[axis];
                    for side in axis * 2..axis * 2 + 2 {
                        let extent = (old >> (side * 5)) & 31;
                        let gap = extent as usize + 1;
                        let positive = side % 2 != 0;
                        let available = if positive {
                            dims[axis] - coordinate - 1
                        } else {
                            coordinate
                        };
                        if extent == 31 || gap > available {
                            continue;
                        }
                        let adjacent = if positive {
                            i + gap * strides[axis]
                        } else {
                            i - gap * strides[axis]
                        };
                        let neighbor = words[adjacent];
                        if neighbor & OCCUPIED != 0
                            || (0..6).any(|s| {
                                s / 2 != axis
                                    && ((neighbor >> (s * 5)) & 31) < ((old >> (s * 5)) & 31)
                            })
                        {
                            continue;
                        }
                        let grown = (extent + 1 + ((neighbor >> (side * 5)) & 31))
                            .min(31)
                            .min(available as u32);
                        out = (out & !(31 << (side * 5))) | grown << (side * 5);
                    }
                }
                scratch[i] = out;
                changed |= out != old;
            }
            std::mem::swap(&mut words, &mut scratch);
        }
        if !changed {
            break;
        }
    }
    words
}

fn hash(words: impl Iterator<Item = u32>) -> u64 {
    words.fold(0xcbf29ce484222325, |h, word| {
        (h ^ u64::from(word)).wrapping_mul(0x100000001b3)
    })
}

fn size(word: u32) -> u64 {
    (0..3)
        .map(|a| 1 + u64::from((word >> (a * 10)) & 31) + u64::from((word >> (a * 10 + 5)) & 31))
        .product()
}

#[test]
#[ignore = "synthetic bounded CPU benchmark; run optimized with --ignored --nocapture"]
fn frozen_shell_refresh_microbenchmark() {
    for radius in [32usize, 64] {
        let n = radius * 2;
        let root = |x: usize, y: usize, z: usize| {
            let height = n / 2 + ((x * 13 + z * 7 + (x ^ z) * 3) % 17) - 8;
            if y < height {
                0x4000_0007
            } else if y == height + 7 && (x * 11 + z * 19) % 53 == 0 {
                0x8000_1234
            } else {
                0
            }
        };
        // Dense 3-D prefix oracle is intentionally test-only; production uses
        // compact occupancy bits and never allocates this O(n^3) u32 array.
        let e = n + 1;
        let mut prefix = vec![0i64; e.pow(3)];
        let index = |x, y, z| (z * e + x) * e + y;
        for z in 1..=n {
            for x in 1..=n {
                for y in 1..=n {
                    prefix[index(x, y, z)] = i64::from(root(x - 1, y - 1, z - 1) & OCCUPIED != 0)
                        + prefix[index(x - 1, y, z)]
                        + prefix[index(x, y - 1, z)]
                        + prefix[index(x, y, z - 1)]
                        - prefix[index(x - 1, y - 1, z)]
                        - prefix[index(x - 1, y, z - 1)]
                        - prefix[index(x, y - 1, z - 1)]
                        + prefix[index(x - 1, y - 1, z - 1)];
                }
            }
        }
        let mut map = Occupancy::new(n);
        let mut previous = Region {
            lo: [radius as i64; 3],
            hi: [radius as i64; 3],
        };
        let mut frozen_hash = 0;
        let mut total = 0usize;
        let mut changed = [0usize; 2];
        let mut before_volume = 0;
        let mut after_volume = [0u64; 2];
        let mut repair_ms = [0.; 2];
        let mut update_ms = 0.;
        let mut proof_words: [Vec<u32>; 3] = std::array::from_fn(|_| Vec::with_capacity(n.pow(3)));
        for half in (8..=radius).step_by(4) {
            let domain = Region {
                lo: [(radius - half) as i64; 3],
                hi: [(radius + half) as i64; 3],
            };
            let mut strips = vec![];
            for region in regions(domain, previous) {
                let [nx, ny, nz] = region.dims();
                let mut words = vec![];
                for z in 0..nz {
                    for x in 0..nx {
                        for y in 0..ny {
                            words.push(root(
                                region.lo[0] as usize + x,
                                region.lo[1] as usize + y,
                                region.lo[2] as usize + z,
                            ));
                        }
                    }
                }
                let words = seed(words, region.dims());
                frozen_hash ^= hash(words.iter().copied());
                total += words.len();
                before_volume += words
                    .iter()
                    .filter(|&&v| v & OCCUPIED == 0)
                    .map(|&v| size(v))
                    .sum::<u64>();
                proof_words[0].extend_from_slice(&words);
                strips.push((region, words));
            }
            let start = Instant::now();
            for (region, words) in &strips {
                map.write(*region, words);
            }
            update_ms += start.elapsed().as_secs_f64() * 1000.;
            for mode in 0..2 {
                for (region, words) in &strips {
                    let mut output = words.clone();
                    let start = Instant::now();
                    changed[mode] += if mode == 0 {
                        map.refresh_cubes(*region, domain, &mut output, &|| false)
                            .unwrap()
                    } else {
                        map.refresh(*region, domain, &mut output, &|| false)
                            .unwrap()
                    };
                    repair_ms[mode] += start.elapsed().as_secs_f64() * 1000.;
                    proof_words[mode + 1].extend_from_slice(&output);
                    let [nx, ny, nz] = region.dims();
                    for z in 0..nz {
                        for x in 0..nx {
                            for y in 0..ny {
                                let i = (z * nx + x) * ny + y;
                                let word = output[i];
                                if word & OCCUPIED != 0 {
                                    assert_eq!(word, words[i]);
                                    continue;
                                }
                                after_volume[mode] += size(word);
                                let q = [x, y, z];
                                let p: Point = std::array::from_fn(|a| region.lo[a] + q[a] as i64);
                                let lo: [usize; 3] = std::array::from_fn(|a| {
                                    (p[a] - i64::from((word >> (a * 10)) & 31)) as usize
                                });
                                let hi: [usize; 3] = std::array::from_fn(|a| {
                                    (p[a] + 1 + i64::from((word >> (a * 10 + 5)) & 31)) as usize
                                });
                                assert!((0..3).all(|a| lo[a] as i64 >= domain.lo[a]
                                    && hi[a] as i64 <= domain.hi[a]));
                                let mut occupied = 0;
                                for corner in 0..8 {
                                    let p: [usize; 3] = std::array::from_fn(|a| {
                                        if corner & (1 << a) == 0 {
                                            lo[a]
                                        } else {
                                            hi[a]
                                        }
                                    });
                                    let sign = if (corner as u32).count_ones() % 2 == 1 {
                                        1
                                    } else {
                                        -1
                                    };
                                    occupied += sign * prefix[index(p[0], p[1], p[2])];
                                }
                                assert_eq!(occupied, 0);
                            }
                        }
                    }
                }
            }
            previous = domain;
        }
        let start = Instant::now();
        let mut empty_queries = 0;
        for i in 0..10000usize {
            let lo = [(i * 13) % n, (i * 29) % n, (i * 7) % n].map(|v| v as i64);
            let hi = std::array::from_fn(|a| (lo[a] + 17).min(n as i64));
            empty_queries += usize::from(std::hint::black_box(map.empty(Region { lo, hi })));
        }
        let query_ms = start.elapsed().as_secs_f64() * 1000.;
        let after_hash = [
            hash(proof_words[1].iter().copied()),
            hash(proof_words[2].iter().copied()),
        ];
        if let Some(path) = std::env::var_os("FRONTIER_NAADF_ROOT_REFRESH_DUMP") {
            let path = std::path::PathBuf::from(path);
            std::fs::create_dir_all(&path).unwrap();
            for (mode, words) in ["before", "cube", "box"].into_iter().zip(&proof_words) {
                let mut bytes = Vec::with_capacity(words.len() * 4);
                for word in words {
                    bytes.extend_from_slice(&word.to_le_bytes());
                }
                std::fs::write(path.join(format!("radius{radius}-{mode}.bin")), bytes).unwrap();
            }
        }
        eprintln!("{{\"test\":\"synthetic_frozen_shell_proof_digests\",\"radius\":{radius},\"before_fnv64\":\"{:016x}\",\"cube_fnv64\":\"{:016x}\",\"box_fnv64\":\"{:016x}\",\"box_queries\":10000,\"box_query_ms\":{query_ms},\"empty_query_results\":{empty_queries}}}",hash(proof_words[0].iter().copied()),after_hash[0],after_hash[1]);
        eprintln!("{{\"test\":\"synthetic_frozen_shell_refresh\",\"radius\":{radius},\"root_words\":{total},\"frozen_hash\":\"{frozen_hash:016x}\",\"occupancy_bytes\":{},\"occupancy_update_ms\":{update_ms},\"cube_refresh_ms\":{},\"box_refresh_ms\":{},\"cube_changed_roots\":{},\"box_changed_roots\":{},\"before_certified_volume_sum\":{before_volume},\"cube_certified_volume_sum\":{},\"box_certified_volume_sum\":{}}}",map.bytes(),repair_ms[0],repair_ms[1],changed[0],changed[1],after_volume[0],after_volume[1]);
    }
}
