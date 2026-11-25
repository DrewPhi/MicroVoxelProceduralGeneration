//! Exact occupancy-only support for optional cross-shell CPU proof repair.
//! No materials or pointers are retained. Unknown cells start occupied, and
//! hierarchy updates are immediate, so an unpopulated slot cannot prove air.
use super::super::stream::{Point, Region};

const OCCUPIED: u32 = 0xc000_0000;

/// Explicit experiment only; the normal strip-local CPU path stays unchanged.
pub(crate) fn requested() -> Option<&'static str> {
    static MODE: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();
    *MODE.get_or_init(
        || match std::env::var("FRONTIER_NAADF_CPU_ROOT_REFRESH").as_deref() {
            Ok("cube") => Some("cube"),
            Ok("box") => Some("box"),
            _ => None,
        },
    )
}

#[derive(Clone)]
pub(crate) struct Occupancy {
    n: usize,
    levels: Vec<Vec<u64>>,
}

impl Occupancy {
    pub(crate) fn new(n: usize) -> Self {
        assert!(n > 0);
        let mut levels = vec![];
        let mut edge = n.next_power_of_two();
        loop {
            levels.push(vec![u64::MAX; edge.pow(3).div_ceil(64)]);
            if edge == 1 {
                break;
            }
            edge /= 2;
        }
        Self { n, levels }
    }

    pub(crate) fn bytes(&self) -> usize {
        self.levels.iter().map(|v| v.len() * 8).sum()
    }

    fn edge(&self, level: usize) -> usize {
        self.n.next_power_of_two() >> level
    }

    fn bit(&self, level: usize, p: [usize; 3]) -> bool {
        let e = self.edge(level);
        let i = (p[2] * e + p[0]) * e + p[1];
        self.levels[level][i / 64] & (1 << (i % 64)) != 0
    }

    fn assign(&mut self, level: usize, p: [usize; 3], value: bool) -> bool {
        let e = self.edge(level);
        let i = (p[2] * e + p[0]) * e + p[1];
        let mask = 1 << (i % 64);
        let word = &mut self.levels[level][i / 64];
        let changed = (*word & mask != 0) != value;
        *word = (*word & !mask) | if value { mask } else { 0 };
        changed
    }

    pub(crate) fn set(&mut self, world: Point, occupied: bool) {
        let mut p = world.map(|v| v.rem_euclid(self.n as i64) as usize);
        if !self.assign(0, p, occupied) {
            return;
        }
        for level in 1..self.levels.len() {
            p = p.map(|v| v / 2);
            let value = occupied
                || (0..8).any(|i| {
                    let child = std::array::from_fn(|a| p[a] * 2 + ((i >> a) & 1));
                    self.bit(level - 1, child)
                });
            if !self.assign(level, p, value) {
                break;
            }
        }
    }

    /// Same z/x/y ordering as Snapshot::chunks. Call for every incoming strip
    /// before refreshing any strip. Work on a clone until publication succeeds.
    pub(crate) fn write(&mut self, region: Region, words: &[u32]) {
        self.write_cancelled(region, words, &|| false).unwrap();
    }

    pub(crate) fn write_cancelled(
        &mut self,
        region: Region,
        words: &[u32],
        cancel: &impl Fn() -> bool,
    ) -> Result<(), String> {
        let [nx, ny, nz] = region.dims();
        assert_eq!(words.len(), nx * ny * nz);
        for z in 0..nz {
            for x in 0..nx {
                for y in 0..ny {
                    if ((z * nx + x) * ny + y) % 4096 == 0 && cancel() {
                        return Err("NAADF occupancy update superseded".into());
                    }
                    let p = [x, y, z];
                    self.set(
                        std::array::from_fn(|a| region.lo[a] + p[a] as i64),
                        words[(z * nx + x) * ny + y] & OCCUPIED != 0,
                    );
                }
            }
        }
        Ok(())
    }

    fn intersects(&self, level: usize, p: [usize; 3], lo: [usize; 3], hi: [usize; 3]) -> bool {
        if !self.bit(level, p) {
            return false;
        }
        let scale = 1 << level;
        let base = p.map(|v| v * scale);
        if (0..3).any(|a| base[a] >= hi[a] || base[a] + scale <= lo[a]) {
            return false;
        }
        if level == 0 || (0..3).all(|a| lo[a] <= base[a] && base[a] + scale <= hi[a]) {
            return true;
        }
        (0..8).any(|i| {
            self.intersects(
                level - 1,
                std::array::from_fn(|a| p[a] * 2 + ((i >> a) & 1)),
                lo,
                hi,
            )
        })
    }

    /// Bounds must lie inside the current published/prepared region. Wrapped
    /// physical queries split into at most eight ordinary hierarchy queries.
    pub(crate) fn empty(&self, bounds: Region) -> bool {
        let dims = bounds.dims();
        assert!(dims.iter().all(|&v| v > 0 && v <= self.n));
        let lo = bounds.lo.map(|v| v.rem_euclid(self.n as i64) as usize);
        let split: [[(usize, usize); 2]; 3] = std::array::from_fn(|a| {
            let end = lo[a] + dims[a];
            [(lo[a], end.min(self.n)), (0, end.saturating_sub(self.n))]
        });
        for i in 0..8 {
            let b: [(usize, usize); 3] = std::array::from_fn(|a| split[a][(i >> a) & 1]);
            if b.iter().any(|&(l, h)| l == h) {
                continue;
            }
            if self.intersects(
                self.levels.len() - 1,
                [0; 3],
                b.map(|v| v.0),
                b.map(|v| v.1),
            ) {
                return false;
            }
        }
        true
    }

    /// Cheap alternative to cuboid searches: find the largest aligned empty
    /// ancestor in O(log n), clip to the logical window, and choose its box
    /// only when it certifies more cells than the strip-local seed. Boxes are
    /// selected whole, never unioned. This can trade one face extent for another.
    pub(crate) fn refresh_cubes(
        &self,
        region: Region,
        domain: Region,
        words: &mut [u32],
        cancel: &impl Fn() -> bool,
    ) -> Result<usize, String> {
        assert!((0..3).all(|a| region.lo[a] >= domain.lo[a] && region.hi[a] <= domain.hi[a]));
        assert!(domain.dims().iter().all(|&v| v <= self.n));
        let [nx, ny, nz] = region.dims();
        assert_eq!(words.len(), nx * ny * nz);
        let volume = |word: u32| -> u32 {
            (0..3)
                .map(|a| 1 + ((word >> (a * 10)) & 31) + ((word >> (a * 10 + 5)) & 31))
                .product()
        };
        let mut changed = 0;
        for z in 0..nz {
            for x in 0..nx {
                for y in 0..ny {
                    let i = (z * nx + x) * ny + y;
                    if i % 4096 == 0 && cancel() {
                        return Err("NAADF proof refresh superseded".into());
                    }
                    let old = words[i];
                    if old & OCCUPIED != 0 {
                        continue;
                    }
                    let q = [x, y, z];
                    let world: Point = std::array::from_fn(|a| region.lo[a] + q[a] as i64);
                    let physical = world.map(|v| v.rem_euclid(self.n as i64) as usize);
                    // A mismatched/incomplete caller snapshot can never create a proof.
                    if self.bit(0, physical) {
                        continue;
                    }
                    let mut level = 0;
                    while level + 1 < self.levels.len()
                        && !self.bit(level + 1, physical.map(|v| v >> (level + 1)))
                    {
                        level += 1;
                    }
                    let edge = 1usize << level;
                    let mut proof = 0;
                    for a in 0..3 {
                        let offset = physical[a] % edge;
                        let low = (offset as i64).min(world[a] - domain.lo[a]).min(31) as u32;
                        let high = ((edge - 1 - offset) as i64)
                            .min(domain.hi[a] - 1 - world[a])
                            .min(31) as u32;
                        proof |= low << (a * 10);
                        proof |= high << (a * 10 + 5);
                    }
                    if volume(proof) > volume(old) {
                        words[i] = proof;
                        changed += 1;
                    }
                }
            }
        }
        Ok(changed)
    }

    /// Monotone cuboid growth. Each binary search checks the complete proposed
    /// box; independently safe axis distances are never combined unsafely.
    /// Occupied pointer/material words remain byte-identical. No GPU provenance
    /// tag or traversal change is involved.
    pub(crate) fn refresh(
        &self,
        region: Region,
        domain: Region,
        words: &mut [u32],
        cancel: &impl Fn() -> bool,
    ) -> Result<usize, String> {
        assert!((0..3).all(|a| region.lo[a] >= domain.lo[a] && region.hi[a] <= domain.hi[a]));
        assert!(domain.dims().iter().all(|&v| v <= self.n));
        let [nx, ny, nz] = region.dims();
        assert_eq!(words.len(), nx * ny * nz);
        let mut changed = 0;
        for z in 0..nz {
            for x in 0..nx {
                for y in 0..ny {
                    let i = (z * nx + x) * ny + y;
                    if i % 4096 == 0 && cancel() {
                        return Err("NAADF proof refresh superseded".into());
                    }
                    let old = words[i];
                    if old & OCCUPIED != 0 {
                        continue;
                    }
                    let q = [x, y, z];
                    let p: Point = std::array::from_fn(|a| region.lo[a] + q[a] as i64);
                    let mut proof = old;
                    let mut bounds = Region {
                        lo: std::array::from_fn(|a| p[a] - i64::from((old >> (a * 10)) & 31)),
                        hi: std::array::from_fn(|a| {
                            p[a] + 1 + i64::from((old >> (a * 10 + 5)) & 31)
                        }),
                    };
                    for side in 0..6 {
                        let a = side / 2;
                        let positive = side % 2 != 0;
                        let shift = side * 5;
                        let mut low = (proof >> shift) & 31;
                        let mut high = 31.min(if positive {
                            domain.hi[a] - 1 - p[a]
                        } else {
                            p[a] - domain.lo[a]
                        } as u32);
                        while low < high {
                            let mid = (low + high).div_ceil(2);
                            let mut candidate = bounds;
                            if positive {
                                candidate.hi[a] = p[a] + 1 + i64::from(mid);
                            } else {
                                candidate.lo[a] = p[a] - i64::from(mid);
                            }
                            if self.empty(candidate) {
                                low = mid;
                            } else {
                                high = mid - 1;
                            }
                        }
                        if positive {
                            bounds.hi[a] = p[a] + 1 + i64::from(low);
                        } else {
                            bounds.lo[a] = p[a] - i64::from(low);
                        }
                        proof = (proof & !(31 << shift)) | (low << shift);
                    }
                    changed += usize::from(proof != old);
                    words[i] = proof;
                }
            }
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hierarchy_queries_match_dense_after_wrap_updates() {
        for n in [3usize, 8, 13] {
            let mut map = Occupancy::new(n);
            assert!(!map.empty(Region {
                lo: [0; 3],
                hi: [1; 3]
            }));
            let mut dense = vec![true; n.pow(3)];
            for k in 0..n.pow(3) * 2 {
                let i = (k * 29 + k / 7) % dense.len();
                let p = [i % n, i / n % n, i / n.pow(2)];
                let occupied = k % 5 == 0;
                dense[i] = occupied;
                map.set(p.map(|v| v as i64 - n as i64), occupied);
                let lo = [k % n, (k * 3) % n, (k * 7) % n].map(|v| v as i64 - n as i64);
                let width = 1 + k % n;
                let bounds = Region {
                    lo,
                    hi: lo.map(|v| v + width as i64),
                };
                let expected = !(0..width.pow(3)).any(|j| {
                    let q = [j % width, j / width % width, j / width.pow(2)];
                    let p: [usize; 3] = std::array::from_fn(|a| {
                        (lo[a] + q[a] as i64).rem_euclid(n as i64) as usize
                    });
                    dense[p[0] + n * (p[1] + n * p[2])]
                });
                assert_eq!(map.empty(bounds), expected, "n={n} k={k}");
            }
        }
    }

    #[test]
    fn cross_shell_proofs_are_monotone_and_never_cover_an_occupied_root() {
        let n = 16;
        let domain = Region {
            lo: [-7, -3, -19],
            hi: [9, 13, -3],
        };
        let mut map = Occupancy::new(n);
        let mut words = vec![0; n.pow(3)];
        for z in 0..n {
            for x in 0..n {
                for y in 0..n {
                    if (x * 13 + y * 5 + z * 17) % 37 == 0 {
                        words[(z * n + x) * n + y] = 0x8000_1234;
                    }
                }
            }
        }
        map.write(domain, &words);
        let mut strip = Region {
            lo: domain.lo,
            hi: domain.hi,
        };
        strip.hi[0] = strip.lo[0] + 2;
        let mut incoming = Vec::new();
        for z in 0..n {
            for x in 0..2 {
                for y in 0..n {
                    incoming.push(words[(z * n + x) * n + y]);
                }
            }
        }
        assert!(
            map.refresh(strip, domain, &mut incoming, &|| false)
                .unwrap()
                > 0
        );
        let before = incoming.clone();
        map.refresh(strip, domain, &mut incoming, &|| false)
            .unwrap();
        for z in 0..n {
            for x in 0..2 {
                for y in 0..n {
                    let i = (z * 2 + x) * n + y;
                    let word = incoming[i];
                    if words[(z * n + x) * n + y] & OCCUPIED != 0 {
                        assert_eq!(word, 0x8000_1234);
                        continue;
                    }
                    for side in 0..6 {
                        assert!(((word >> (side * 5)) & 31) >= ((before[i] >> (side * 5)) & 31));
                    }
                    let q = [x, y, z];
                    let p: Point = std::array::from_fn(|a| domain.lo[a] + q[a] as i64);
                    let bounds = Region {
                        lo: std::array::from_fn(|a| p[a] - i64::from((word >> (a * 10)) & 31)),
                        hi: std::array::from_fn(|a| {
                            p[a] + 1 + i64::from((word >> (a * 10 + 5)) & 31)
                        }),
                    };
                    assert!(map.empty(bounds));
                }
            }
        }
        assert!(map.refresh(strip, domain, &mut incoming, &|| true).is_err());
    }

    #[test]
    fn occupancy_memory_is_small_relative_to_root_words() {
        assert!(Occupancy::new(128).bytes() < 128usize.pow(3) / 7 + 64);
    }

    #[test]
    fn cube_selection_crosses_shells_clips_domain_and_preserves_solids() {
        let n = 16usize;
        let domain = Region {
            lo: [-5, -9, -17],
            hi: [11, 7, -1],
        };
        let mut map = Occupancy::new(n);
        let mut source = vec![0; n.pow(3)];
        // One exact occupied pointer breaks some ancestors, with all other
        // cells air. World/domain and torus alignments deliberately differ.
        source[(4 * n + 7) * n + 8] = 0x8000_1234;
        map.write(domain, &source);
        let before = source.clone();
        assert!(
            map.refresh_cubes(domain, domain, &mut source, &|| false)
                .unwrap()
                > 0
        );
        for z in 0..n {
            for x in 0..n {
                for y in 0..n {
                    let i = (z * n + x) * n + y;
                    let word = source[i];
                    if before[i] & OCCUPIED != 0 {
                        assert_eq!(word, before[i]);
                        continue;
                    }
                    let q = [x, y, z];
                    let p: Point = std::array::from_fn(|a| domain.lo[a] + q[a] as i64);
                    let bounds = Region {
                        lo: std::array::from_fn(|a| p[a] - i64::from((word >> (a * 10)) & 31)),
                        hi: std::array::from_fn(|a| {
                            p[a] + 1 + i64::from((word >> (a * 10 + 5)) & 31)
                        }),
                    };
                    assert!((0..3)
                        .all(|a| bounds.lo[a] >= domain.lo[a] && bounds.hi[a] <= domain.hi[a]));
                    assert!(map.empty(bounds));
                }
            }
        }
        // Prove inserting new geometry invalidates the correct ancestors.
        let obstacle = [0, 0, -8];
        map.set(obstacle, true);
        assert!(!map.empty(Region {
            lo: obstacle,
            hi: obstacle.map(|v| v + 1)
        }));
        let mut air = vec![0; n.pow(3)];
        let q = std::array::from_fn::<_, 3, _>(|a| (obstacle[a] - domain.lo[a]) as usize);
        air[(q[2] * n + q[0]) * n + q[1]] = 0x4000_0012;
        map.refresh_cubes(domain, domain, &mut air, &|| false)
            .unwrap();
        assert_eq!(air[(q[2] * n + q[0]) * n + q[1]], 0x4000_0012);
    }
}

#[cfg(test)]
#[path = "root_occupancy_bench.rs"]
mod benchmarks;
