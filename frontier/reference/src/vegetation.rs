//! Offline native plant oracle. Median-split instance BVH only skips empty
//! boxes; hits still test actual 6.25cm cells, never coarse proxy geometry.
use crate::{forward_index, material_id, Hit};
use frontier_world::{
    vegetation::{model_for, PlantRecord, PlantVoxel, Species},
    Cell, World, VOXEL_SIZE,
};

#[derive(Clone, Copy, Debug)]
struct Bounds {
    min: [f64; 3],
    max: [f64; 3],
}
impl Bounds {
    fn interval(&self, o: [f64; 3], d: [f64; 3], near: f64, far: f64) -> Option<(f64, f64)> {
        let (mut lo, mut hi) = (near, far);
        for a in 0..3 {
            if d[a] == 0. {
                if o[a] < self.min[a] || o[a] >= self.max[a] {
                    return None;
                }
            } else {
                let t0 = (self.min[a] - o[a]) / d[a];
                let t1 = (self.max[a] - o[a]) / d[a];
                lo = lo.max(t0.min(t1));
                hi = hi.min(t0.max(t1));
            }
        }
        (lo <= hi).then_some((lo, hi))
    }
    fn union(self, other: Self) -> Self {
        Self {
            min: std::array::from_fn(|a| self.min[a].min(other.min[a])),
            max: std::array::from_fn(|a| self.max[a].max(other.max[a])),
        }
    }
}
struct Plant {
    voxels: Box<[PlantVoxel]>,
    bounds: Bounds,
    rank: usize,
}
#[derive(Clone, Copy, Debug)]
pub struct PlantHit {
    pub hit: Hit,
    pub color: [f32; 3],
    rank: usize,
}
impl Plant {
    fn from_record(p: PlantRecord, rank: usize) -> Self {
        let model = model_for(&p);
        let mut voxels: Vec<_> = model
            .voxels
            .iter()
            .map(|v| PlantVoxel {
                cell: p.world_cell(v.cell),
                ..*v
            })
            .collect();
        voxels.sort_unstable_by_key(|v| v.cell);
        let mut min = [i64::MAX; 3];
        let mut max = [i64::MIN; 3];
        for v in &voxels {
            for (a, q) in [v.cell.x, v.cell.y, v.cell.z].into_iter().enumerate() {
                min[a] = min[a].min(q);
                max[a] = max[a].max(q + 1);
            }
        }
        Self {
            voxels: voxels.into_boxed_slice(),
            bounds: Bounds {
                min: min.map(|v| (v - 1) as f64 * VOXEL_SIZE),
                max: max.map(|v| (v + 1) as f64 * VOXEL_SIZE),
            },
            rank,
        }
    }
    fn ray(&self, o: [f64; 3], d: [f64; 3], near: f64, far: f64) -> Option<PlantHit> {
        // Same integer-plane/tied-axis contract as the independent dense oracle;
        // sparse canonical cell lookup replaces its dense storage only.
        let scale = d.iter().fold(0f64, |a, v| a.max(v.abs()));
        if scale == 0. {
            return None;
        }
        let d = d.map(|v| v / scale);
        let norm = d.iter().map(|v| v * v).sum::<f64>().sqrt();
        let d = d.map(|v| v / norm);
        let min = self.bounds.min.map(|v| (v / VOXEL_SIZE) as i64);
        let size = std::array::from_fn::<_, 3, _>(|a| {
            ((self.bounds.max[a] - self.bounds.min[a]) / VOXEL_SIZE) as i64
        });
        let o = std::array::from_fn::<_, 3, _>(|a| o[a] / VOXEL_SIZE - min[a] as f64);
        let dc = d.map(|v| v / VOXEL_SIZE);
        let mut enter = f64::NEG_INFINITY;
        let mut exit = far;
        let mut entries = [f64::NEG_INFINITY; 3];
        for a in 0..3 {
            if dc[a] == 0. {
                if o[a] < 0. || o[a] >= size[a] as f64 {
                    return None;
                }
            } else {
                let t0 = -o[a] / dc[a];
                let t1 = (size[a] as f64 - o[a]) / dc[a];
                entries[a] = t0.min(t1);
                enter = enter.max(entries[a]);
                exit = exit.min(t0.max(t1));
            }
        }
        let mut t = near.max(enter);
        if t >= exit {
            return None;
        }
        let step = dc.map(|v| {
            if v > 0. {
                1
            } else if v < 0. {
                -1
            } else {
                0
            }
        });
        let mut p = [0; 3];
        let mut entered = [false; 3];
        for a in 0..3 {
            (p[a], entered[a]) = forward_index(o[a], dc[a], t);
            if enter >= near && entries[a] == enter {
                p[a] = if step[a] > 0 { 0 } else { size[a] - 1 };
            }
        }
        let mut faces = 0u8;
        for a in 0..3 {
            if step[a] != 0 && ((enter >= near && entries[a] == enter) || entered[a]) {
                faces |= 1 << (a * 2 + usize::from(step[a] < 0));
            }
        }
        loop {
            if (0..3).any(|a| p[a] < 0 || p[a] >= size[a]) {
                return None;
            }
            let world = std::array::from_fn::<_, 3, _>(|a| p[a] + min[a]);
            let cell = Cell {
                x: world[0],
                y: world[1],
                z: world[2],
            };
            if let Ok(i) = self.voxels.binary_search_by_key(&cell, |v| v.cell) {
                let mut visible = faces;
                for face in 0..6 {
                    if visible & (1 << face) != 0 {
                        let mut q = world;
                        q[face / 2] += if face % 2 == 0 { -1 } else { 1 };
                        if self
                            .voxels
                            .binary_search_by_key(
                                &Cell {
                                    x: q[0],
                                    y: q[1],
                                    z: q[2],
                                },
                                |v| v.cell,
                            )
                            .is_ok()
                        {
                            visible &= !(1 << face);
                        }
                    }
                }
                if visible != 0 {
                    let v = self.voxels[i];
                    return Some(PlantHit {
                        hit: Hit {
                            cell: world,
                            distance: t,
                            face: visible.trailing_zeros() as usize,
                            material: material_id(v.material),
                        },
                        color: v.color,
                        rank: self.rank,
                    });
                }
            }
            let next = std::array::from_fn::<_, 3, _>(|a| {
                if step[a] == 0 {
                    f64::INFINITY
                } else {
                    ((p[a] + i64::from(step[a] > 0)) as f64 - o[a]) / dc[a]
                }
            });
            let nt = next.iter().copied().fold(f64::INFINITY, f64::min);
            if !nt.is_finite() || nt > exit || nt > far {
                return None;
            }
            faces = 0;
            for a in 0..3 {
                if next[a] == nt {
                    p[a] += step[a];
                    faces |= 1 << (a * 2 + usize::from(step[a] < 0));
                } else {
                    let (cell, crossed) = forward_index(o[a], dc[a], nt);
                    p[a] = cell;
                    if crossed {
                        faces |= 1 << (a * 2 + usize::from(step[a] < 0));
                    }
                }
            }
            t = nt;
        }
    }
}
struct Node {
    bounds: Bounds,
    first: usize,
    end: usize,
    children: Option<[usize; 2]>,
}
pub struct Vegetation {
    plants: Vec<Plant>,
    nodes: Vec<Node>,
    pub counts: [usize; 3],
    pub voxel_count: usize,
}
impl Vegetation {
    pub fn empty() -> Self {
        Self {
            plants: Vec::new(),
            nodes: Vec::new(),
            counts: [0; 3],
            voxel_count: 0,
        }
    }
    pub fn build(world: &World, min: [i64; 2], max: [i64; 2], threads: usize) -> Self {
        Self::build_filtered(world, min, max, threads, |_| true)
    }
    /// Reject records before materializing native models. The caller must use
    /// conservative bounds covering both primary and shadow-ray footprints.
    pub fn build_filtered(
        world: &World,
        min: [i64; 2],
        max: [i64; 2],
        threads: usize,
        keep: impl Fn(&PlantRecord) -> bool,
    ) -> Self {
        let mut result = Self {
            plants: Vec::new(),
            nodes: Vec::new(),
            counts: [0; 3],
            voxel_count: 0,
        };
        // The same precedence as World::vegetation_voxel. Within each species,
        // tile and root enumeration order is inherited from the canonical query.
        for species in [Species::Rowan, Species::Grass, Species::Edelweiss] {
            let radius = if species == Species::Rowan {
                frontier_world::vegetation::MAX_PLANT_RADIUS_CELLS
            } else {
                8
            };
            let mut records = world.vegetation_records_species(
                min.map(|v| v - radius),
                max.map(|v| v + radius),
                species,
            );
            records.retain(&keep);
            let base = result.plants.len();
            let batch = records.len().div_ceil(threads.max(1).min(64)).max(1);
            let mut groups = std::thread::scope(|scope| {
                let handles: Vec<_> = records
                    .chunks(batch)
                    .enumerate()
                    .map(|(part, records)| {
                        scope.spawn(move || {
                            records
                                .iter()
                                .enumerate()
                                .filter_map(|(i, p)| {
                                    let plant = Plant::from_record(*p, base + part * batch + i);
                                    if i % 10000 == 0 {
                                        eprintln!(
                                            "native {species:?} worker{part}: {i}/{} plants",
                                            records.len()
                                        );
                                    }
                                    (!plant.voxels.is_empty()).then_some(plant)
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .flat_map(|h| h.join().unwrap())
                    .collect::<Vec<_>>()
            });
            result.counts[species as usize] = groups.len();
            result.voxel_count += groups.iter().map(|p| p.voxels.len()).sum::<usize>();
            result.plants.append(&mut groups);
        }
        if !result.plants.is_empty() {
            result.node(0, result.plants.len());
        }
        result
    }
    fn node(&mut self, first: usize, end: usize) -> usize {
        let bounds = self.plants[first..end]
            .iter()
            .map(|p| p.bounds)
            .reduce(Bounds::union)
            .unwrap();
        let index = self.nodes.len();
        self.nodes.push(Node {
            bounds,
            first,
            end,
            children: None,
        });
        if end - first > 8 {
            let axis = (0..3)
                .max_by(|&a, &b| {
                    (bounds.max[a] - bounds.min[a]).total_cmp(&(bounds.max[b] - bounds.min[b]))
                })
                .unwrap();
            let mid = (first + end) / 2;
            self.plants[first..end].select_nth_unstable_by(mid - first, |a, b| {
                (a.bounds.min[axis] + a.bounds.max[axis])
                    .total_cmp(&(b.bounds.min[axis] + b.bounds.max[axis]))
            });
            let left = self.node(first, mid);
            let right = self.node(mid, end);
            self.nodes[index].children = Some([left, right]);
        }
        index
    }
    pub fn memory_bytes(&self) -> usize {
        self.plants
            .iter()
            .map(|p| p.voxels.len() * std::mem::size_of::<PlantVoxel>())
            .sum::<usize>()
            + self.nodes.capacity() * std::mem::size_of::<Node>()
            + self.plants.capacity() * std::mem::size_of::<Plant>()
    }
    pub fn ray(&self, o: [f64; 3], d: [f64; 3], near: f64, far: f64) -> Option<PlantHit> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best = None;
        self.trace(0, o, d, near, far, &mut best);
        best
    }
    fn trace(
        &self,
        id: usize,
        o: [f64; 3],
        d: [f64; 3],
        near: f64,
        far: f64,
        best: &mut Option<PlantHit>,
    ) {
        let far = best.as_ref().map_or(far, |h| h.hit.distance.min(far));
        let n = &self.nodes[id];
        if n.bounds.interval(o, d, near, far).is_none() {
            return;
        }
        if let Some([a, b]) = n.children {
            let ta = self.nodes[a].bounds.interval(o, d, near, far).map(|v| v.0);
            let tb = self.nodes[b].bounds.interval(o, d, near, far).map(|v| v.0);
            let order = if ta.unwrap_or(f64::INFINITY) <= tb.unwrap_or(f64::INFINITY) {
                [a, b]
            } else {
                [b, a]
            };
            for child in order {
                self.trace(child, o, d, near, far, best);
            }
        } else {
            for plant in &self.plants[n.first..n.end] {
                let far = best.as_ref().map_or(far, |h| h.hit.distance.min(far));
                if let Some(candidate) = plant.ray(o, d, near, far) {
                    let replace = best.as_ref().is_none_or(|old| {
                        if candidate.hit.cell == old.hit.cell {
                            candidate.rank < old.rank
                        } else {
                            candidate.hit.distance < old.hit.distance
                        }
                    });
                    if replace {
                        *best = Some(candidate);
                    }
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn instance_bvh_matches_brute_force_and_canonical_materials() {
        let records: Vec<_> = (0..20)
            .map(|i| PlantRecord {
                species: if i % 2 == 0 {
                    Species::Grass
                } else {
                    Species::Edelweiss
                },
                seed: i as u64 + 41,
                variant: (i % 8) as u8,
                root: Cell {
                    x: (i % 5) * 8 - 20,
                    y: 0,
                    z: (i / 5) * 8 - 16,
                },
                rotation: (i % 4) as u8,
            })
            .collect();
        let plants: Vec<_> = records
            .iter()
            .enumerate()
            .map(|(rank, p)| Plant::from_record(*p, rank))
            .collect();
        let mut vegetation = Vegetation {
            plants,
            nodes: Vec::new(),
            counts: [0; 3],
            voxel_count: 0,
        };
        vegetation.node(0, records.len());
        for i in 0..500 {
            let a = i as f64 * 2.3999632297;
            let o = [a.cos() * 3., 1., a.sin() * 3.];
            let d = crate::normalized([-o[0], -o[1], -o[2]]);
            let expected = vegetation
                .plants
                .iter()
                .filter_map(|p| p.ray(o, d, 0.025, 10.))
                .min_by(|a, b| {
                    a.hit
                        .distance
                        .total_cmp(&b.hit.distance)
                        .then(a.rank.cmp(&b.rank))
                });
            let actual = vegetation.ray(o, d, 0.025, 10.);
            assert_eq!(actual.map(|h| h.hit), expected.map(|h| h.hit));
            assert_eq!(actual.map(|h| h.color), expected.map(|h| h.color));
        }
    }
    #[test]
    fn native_plant_rays_match_independent_dense_oracle() {
        for species in [Species::Grass, Species::Edelweiss, Species::Rowan] {
            let p = PlantRecord {
                species,
                seed: 941,
                variant: 3,
                root: Cell { x: -7, y: 3, z: -5 },
                rotation: 1,
            };
            let plant = Plant::from_record(p, 0);
            let min = plant.bounds.min.map(|v| (v / VOXEL_SIZE) as i64);
            let size = std::array::from_fn(|a| {
                ((plant.bounds.max[a] - plant.bounds.min[a]) / VOXEL_SIZE) as usize
            });
            let dense = crate::Snapshot::build(min, size, |c| {
                plant
                    .voxels
                    .binary_search_by_key(
                        &Cell {
                            x: c[0],
                            y: c[1],
                            z: c[2],
                        },
                        |v| v.cell,
                    )
                    .map_or(0, |i| material_id(plant.voxels[i].material))
            });
            for i in 0..1000 {
                let a = i as f64 * 2.399963229728653;
                let center: [f64; 3] =
                    std::array::from_fn(|k| (plant.bounds.min[k] + plant.bounds.max[k]) * 0.5);
                let radius = (0..3)
                    .map(|k| plant.bounds.max[k] - plant.bounds.min[k])
                    .fold(1., f64::max)
                    * 2.;
                let o = [
                    center[0] + a.cos() * radius,
                    center[1] + radius,
                    center[2] + a.sin() * radius,
                ];
                let d = crate::normalized(std::array::from_fn(|k| center[k] - o[k]));
                let actual = plant.ray(o, d, 0.025, 10.).map(|h| h.hit);
                let expected = dense.ray(o, d, 0.025, 10., true);
                assert_eq!(actual.is_some(), expected.is_some());
                if let (Some(a), Some(b)) = (actual, expected) {
                    assert_eq!(a.cell, b.cell);
                    assert_eq!(a.face, b.face, "ray{i} {species:?}: {a:?} vs {b:?}");
                    assert!((a.distance - b.distance).abs() < 1e-9);
                }
            }
        }
    }
}
