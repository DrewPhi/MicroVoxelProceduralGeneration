//! On-demand native oracle for flight views wider than the dense height cache.
//! Analytical bounds only reject empty space or fully buried intervals. Every
//! reported intersection comes from the dense oracle's native traversal.
use super::{Hit, Landscape, LandscapeRays};
use frontier_world::{
    affine_bounds::{bound_native_height, BoundBudget, ColumnBox},
    voxel_size, FbmTerrain,
};
use std::{
    collections::VecDeque,
    io,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
};

const PAGE: usize = 64;
const SETS: usize = 256;
const WAYS: usize = 8;
type V = [f64; 3];
type CacheEntry = ([i64; 2], Arc<OnceLock<Landscape>>);

#[derive(Default)]
struct Node {
    bound: OnceLock<[f64; 2]>,
    exact_top: OnceLock<f64>,
    children: OnceLock<Box<[Node; 4]>>,
}

pub struct SparseLandscape {
    terrain: FbmTerrain,
    min: [i64; 2],
    size: usize,
    root: Node,
    sets: Vec<Mutex<VecDeque<CacheEntry>>>,
    nodes: AtomicUsize,
    resident_pages: AtomicUsize,
}

impl SparseLandscape {
    pub fn new(terrain: FbmTerrain, min: [i64; 2], size: usize) -> io::Result<Self> {
        if !size.is_power_of_two()
            || !(PAGE..=1 << 23).contains(&size)
            || min.iter().any(|&p| p.abs_diff(0) > 1 << 52)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid sparse native footprint",
            ));
        }
        Ok(Self {
            terrain,
            min,
            size,
            root: Node::default(),
            sets: (0..SETS).map(|_| Mutex::default()).collect(),
            nodes: AtomicUsize::new(1),
            resident_pages: AtomicUsize::new(0),
        })
    }

    fn page(&self, min: [i64; 2]) -> Arc<OnceLock<Landscape>> {
        let [x, z] = min.map(|v| v.div_euclid(PAGE as i64) as u64);
        let hash = x.wrapping_mul(0x9e3779b97f4a7c15) ^ z.wrapping_mul(0x94d049bb133111eb);
        let mut set = self.sets[(hash >> 56) as usize].lock().unwrap();
        if let Some(i) = set.iter().position(|(key, _)| *key == min) {
            let entry = set.remove(i).unwrap();
            let page = entry.1.clone();
            set.push_back(entry);
            return page;
        }
        if set.len() == WAYS {
            set.pop_front();
        } else {
            self.resident_pages.fetch_add(1, Ordering::Relaxed);
        }
        let page = Arc::new(OnceLock::new());
        set.push_back((min, page.clone()));
        page
    }

    fn visit(
        &self,
        node: &Node,
        min: [i64; 2],
        size: usize,
        ray: &Ray,
        t: [f64; 2],
    ) -> Option<Hit> {
        // A neighbour may expose a vertical wall below the minimum roof in
        // this node. Include its one-column halo before rejecting solid space.
        let bound = node.bound.get_or_init(|| {
            let area = ColumnBox {
                min: min.map(|v| v - 1),
                size_cells: [size as u32 + 2; 2],
            };
            bound_native_height(&self.terrain, area, BoundBudget::default())
                .map_or([f64::NEG_INFINITY, f64::INFINITY], |b| b.height_range)
        });
        let top = node.exact_top.get().copied().unwrap_or(bound[1]);
        let y = t.map(|v| ray.origin[1] + v * ray.direction[1]);
        let margin = 16. * f64::EPSILON * (ray.origin[1].abs() + t[1] + 1.);
        if y[0].min(y[1]) > top + margin || y[0].max(y[1]) < bound[0] - margin {
            return None;
        }
        if size == PAGE {
            let slot = self.page(min);
            let page = slot.get_or_init(|| {
                let mut heights = Vec::with_capacity(PAGE * PAGE);
                for z in min[1]..min[1] + PAGE as i64 {
                    for x in min[0]..min[0] + PAGE as i64 {
                        heights.push(
                            self.terrain
                                .height_cells(x, z)
                                .clamp(i64::from(i32::MIN), i64::from(i32::MAX))
                                as i32,
                        );
                    }
                }
                Landscape::from_heights(self.terrain.clone(), min, PAGE, 1, heights, 0)
            });
            let level = page.levels.len() - 1;
            node.exact_top
                .get_or_init(|| page.levels[level].1[0] as f64 * voxel_size());
            return page.node(
                level,
                0,
                0,
                ray.origin,
                ray.direction,
                ray.range[0],
                ray.range[1],
            );
        }
        let children = node.children.get_or_init(|| {
            self.nodes.fetch_add(4, Ordering::Relaxed);
            Box::default()
        });
        let half = size / 2;
        let mut hits = [(0, [0; 2], [0.; 2]); 4];
        let mut count = 0;
        for i in 0..4 {
            let p = [
                min[0] + (i % 2 * half) as i64,
                min[1] + (i / 2 * half) as i64,
            ];
            if let Some(span) = ray.horizontal(p, half, t) {
                hits[count] = (i, p, span);
                count += 1;
            }
        }
        hits[..count].sort_unstable_by(|a, b| a.2[0].total_cmp(&b.2[0]));
        for &(i, p, span) in &hits[..count] {
            if let Some(hit) = self.visit(&children[i], p, half, ray, span) {
                return Some(hit);
            }
        }
        None
    }
}

struct Ray {
    origin: V,
    direction: V,
    range: [f64; 2],
}
impl Ray {
    fn horizontal(&self, min: [i64; 2], size: usize, mut span: [f64; 2]) -> Option<[f64; 2]> {
        for (i, axis) in [0, 2].into_iter().enumerate() {
            let lo = min[i] as f64 * voxel_size();
            let hi = (min[i] + size as i64) as f64 * voxel_size();
            let o = self.origin[axis];
            let d = self.direction[axis];
            if d == 0. {
                if o < lo || o >= hi {
                    return None;
                }
            } else {
                let a = (lo - o) / d;
                let b = (hi - o) / d;
                span[0] = span[0].max(a.min(b));
                span[1] = span[1].min(a.max(b));
                if span[0] > span[1] {
                    return None;
                }
            }
        }
        Some(span)
    }
}

impl LandscapeRays for SparseLandscape {
    fn ray(&self, origin: V, direction: V, near: f64, far: f64) -> Option<Hit> {
        let length = direction.iter().map(|v| v * v).sum::<f64>().sqrt();
        if length == 0.
            || !length.is_finite()
            || !origin.into_iter().all(f64::is_finite)
            || !near.is_finite()
            || !far.is_finite()
            || near < 0.
            || far < near
        {
            return None;
        }
        let ray = Ray {
            origin,
            direction: direction.map(|v| v / length),
            range: [near, far],
        };
        let span = ray.horizontal(self.min, self.size, ray.range)?;
        self.visit(&self.root, self.min, self.size, &ray, span)
    }

    fn memory_bytes(&self) -> usize {
        let count = self.resident_pages.load(Ordering::Relaxed);
        self.nodes.load(Ordering::Relaxed) * std::mem::size_of::<Node>()
            + count * (std::mem::size_of::<Landscape>() + PAGE * PAGE * 4 + (16 + 4 + 1) * 8)
            + self.sets.len() * std::mem::size_of::<Mutex<VecDeque<CacheEntry>>>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::{FbmParams, FbmRecipe};

    #[test]
    fn page_boundaries_and_oblique_rays_agree_with_the_dense_native_oracle() {
        for recipe in [FbmRecipe::Highlands, FbmRecipe::Regional] {
            let terrain = FbmTerrain::new(
                673,
                FbmParams {
                    recipe,
                    horizontal_scale: 180.,
                    ..Default::default()
                },
            )
            .unwrap();
            let min = [-204, -121];
            let dense = Landscape::build(terrain.clone(), min, 256, 2, None).unwrap();
            let sparse = SparseLandscape::new(terrain.clone(), min, 256).unwrap();
            for z in (0..256).step_by(7) {
                for x in [0, 1, 31, 63, 64, 65, 127, 128, 191, 192, 222, 255] {
                    let cell = [min[0] + x, min[1] + z];
                    let h = terrain.height_cells(cell[0], cell[1]) as f64 * voxel_size();
                    for (offset, dir) in [
                        (0., [0., -1., 0.]),
                        (0.5, [-0.42, -0.3, 0.15]),
                        (0.5, [0.35, -0.25, -0.30]),
                        (-0.1, [0.5, 0., 0.5]),
                    ] {
                        let eye = [
                            (cell[0] as f64 + offset) * voxel_size(),
                            h + 0.5,
                            (cell[1] as f64 + offset) * voxel_size(),
                        ];
                        let a = dense.ray(eye, dir, 0., 120.);
                        let b = sparse.ray(eye, dir, 0., 120.);
                        assert_eq!(
                            a.map(|v| (v.cell, v.face, v.material, v.distance.to_bits())),
                            b.map(|v| (v.cell, v.face, v.material, v.distance.to_bits())),
                            "{recipe:?} {eye:?} {dir:?}"
                        );
                    }
                }
            }
        }
    }
}
