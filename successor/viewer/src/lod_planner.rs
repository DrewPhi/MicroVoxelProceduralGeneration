//! Latest-intent planner. One queued request and one result keep CPU/memory work
//! bounded. Terrain-error sampling and quadtree selection never block a frame.
use microvoxel::{
    generation::Terrain,
    render::lod::{self, Layout, Quality},
    World,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        mpsc::{self, Receiver},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
pub struct Request {
    pub generation: u64,
    pub eye: [f64; 3],
    pub height: u32,
    pub fov: f64,
    pub previous: Layout,
    pub budget: usize,
    pub quality: Quality,
    pub prefetch_eye: Option<[f64; 3]>,
}
pub struct Result {
    epoch: u64,
    pub elapsed: Duration,
    pub generation: u64,
    pub selection: lod::Selection,
    pub prefetch: Vec<lod::MeshKey>,
}
struct Queue {
    epoch: u64,
    world: Arc<World<Terrain>>,
    latest: Option<Request>,
    closed: bool,
}
pub struct Planner {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    pub results: Receiver<Result>,
}
impl Planner {
    pub fn new(world: Arc<World<Terrain>>) -> Self {
        let queue = Arc::new((
            Mutex::new(Queue {
                epoch: 0,
                world: world.clone(),
                latest: None,
                closed: false,
            }),
            Condvar::new(),
        ));
        let (tx, results) = mpsc::sync_channel(1);
        let work = Arc::clone(&queue);
        thread::Builder::new()
            .name("voxel-lod-planner".into())
            .spawn(move || {
                let mut errors = BTreeMap::new();
                let mut recent = VecDeque::new();
                let mut revision = world.revision();
                let mut cached_world = world;
                loop {
                    let (request, world, epoch) = {
                        let (lock, wake) = &*work;
                        let mut state = lock.lock().unwrap();
                        while state.latest.is_none() && !state.closed {
                            state = wake.wait(state).unwrap();
                        }
                        if state.closed {
                            break;
                        }
                        (
                            state.latest.take().unwrap(),
                            state.world.clone(),
                            state.epoch,
                        )
                    };
                    if !Arc::ptr_eq(&cached_world, &world) || revision != world.revision() {
                        errors.clear();
                        recent.clear();
                        revision = world.revision();
                        cached_world = world.clone();
                    }
                    let Request {
                        generation,
                        eye,
                        height,
                        fov,
                        previous,
                        budget,
                        quality,
                        prefetch_eye,
                    } = request;
                    let started = Instant::now();
                    let mut estimate_terrain = |key: lod::TileKey| {
                        let mut estimate = if let Some(&cached) = errors.get(&key) {
                            cached
                        } else {
                            let computed = {
                                let grid = previous.grid;
                                let [x, z] = key.origin_in(grid);
                                let edge = key.edge_in(grid);
                                if let Some(summary) = world.terrain_summary(
                                    [x as f64 * grid.cell_m(), z as f64 * grid.cell_m()],
                                    edge as f64 * grid.cell_m(),
                                ) {
                                    lod::TerrainError {
                                        low: summary.min_m,
                                        high: summary.max_m,
                                        deviation: summary.error_m
                                            * (1. + summary.normal_variation.min(1.))
                                            + summary.appearance_variation
                                                * (edge as f64 * grid.cell_m() * 0.02).min(1.),
                                        feature_scale: summary.feature_scale_m,
                                    }
                                } else {
                                    lod::TerrainError {
                                        low: 0.,
                                        high: world.spec().ceiling_m as f64,
                                        deviation: 0.,
                                        feature_scale: f64::INFINITY,
                                    }
                                }
                            };
                            if errors.len() >= 16384 {
                                if let Some(old) = recent.pop_front() {
                                    errors.remove(&old);
                                }
                            }
                            errors.insert(key, computed);
                            recent.push_back(key);
                            computed
                        };
                        // Reuse already-built child summaries without requiring their
                        // generation. Refinement tightens the known parent envelope.
                        for child in key.children() {
                            if let Some(finer) = errors.get(&child) {
                                estimate.low = estimate.low.min(finer.low);
                                estimate.high = estimate.high.max(finer.high);
                                estimate.deviation = estimate.deviation.max(finer.deviation);
                                estimate.feature_scale =
                                    estimate.feature_scale.min(finer.feature_scale);
                            }
                        }
                        Some(estimate)
                    };
                    let selection = microvoxel::render::clipmap::select(
                        eye,
                        height,
                        fov,
                        &previous,
                        budget,
                        quality,
                        &mut estimate_terrain,
                    );
                    let mut prefetch = Vec::new();
                    if let Some(future) = prefetch_eye {
                        let forecast = microvoxel::render::clipmap::select(
                            future,
                            height,
                            fov,
                            &selection.layout,
                            budget,
                            quality,
                            &mut estimate_terrain,
                        );
                        let visible: std::collections::BTreeSet<_> = selection
                            .layout
                            .mesh_keys(world.revision())
                            .into_values()
                            .collect();
                        let grid = previous.grid;
                        prefetch = forecast
                            .layout
                            .mesh_keys(world.revision())
                            .into_values()
                            .map(|mut key| {
                                // Native interiors are camera-independent. Preload one
                                // reusable interior rather than many moving border variants.
                                if key.tile.level == grid.finest {
                                    key.neighbors = [grid.finest; 4];
                                    key.corner_levels = None;
                                }
                                key
                            })
                            .filter(|k| k.tile.level + 2 >= grid.finest && !visible.contains(k))
                            .collect();
                        prefetch.sort_by(|a, b| {
                            let distance = |k: &lod::MeshKey| {
                                let [x, z] = k.tile.origin_in(grid);
                                let half = k.tile.edge_in(grid) as f64 * grid.cell_m() * 0.5;
                                (x as f64 * grid.cell_m() + half - future[0])
                                    .hypot(z as f64 * grid.cell_m() + half - future[2])
                            };
                            distance(a).total_cmp(&distance(b))
                        });
                        prefetch.truncate(48);
                    }
                    // Numeric world revisions may repeat across independent sources.
                    // Epoch validation also rejects plans already buffered at replacement.
                    {
                        let state = work.0.lock().unwrap();
                        if state.closed
                            || state.epoch != epoch
                            || state
                                .latest
                                .as_ref()
                                .is_some_and(|r| r.generation > generation)
                        {
                            continue;
                        }
                    }
                    if tx
                        .send(Result {
                            epoch,
                            elapsed: started.elapsed(),
                            generation,
                            selection,
                            prefetch,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("LOD planner");
        Self { queue, results }
    }
    pub fn replace_world(&self, world: Arc<World<Terrain>>) {
        let mut q = self.queue.0.lock().unwrap();
        q.epoch = q.epoch.wrapping_add(1);
        q.world = world;
        q.latest = None;
    }
    pub fn accepts(&self, result: &Result) -> bool {
        let state = self.queue.0.lock().unwrap();
        !state.closed && state.epoch == result.epoch
    }
    pub fn request(&self, request: Request) {
        let (lock, wake) = &*self.queue;
        lock.lock().unwrap().latest = Some(request);
        wake.notify_one();
    }
}
impl Drop for Planner {
    fn drop(&mut self) {
        let (lock, wake) = &*self.queue;
        lock.lock().unwrap().closed = true;
        wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacing_world_rejects_buffered_plan_even_when_revision_and_generation_repeat() {
        let world = Arc::new(World::new(Terrain::new(5)));
        let grid = lod::Grid::new(world.spec());
        let (_, results) = mpsc::channel();
        let planner = Planner {
            queue: Arc::new((
                Mutex::new(Queue {
                    world,
                    epoch: 0,
                    latest: None,
                    closed: false,
                }),
                Condvar::new(),
            )),
            results,
        };
        let old = Result {
            prefetch: Vec::new(),
            epoch: 0,
            generation: 7,
            elapsed: Duration::ZERO,
            selection: lod::Selection {
                layout: Layout {
                    grid,
                    tiles: Default::default(),
                },
                native_policy_radius: 0.,
                requested_pixels: 1.,
                achieved_pixels: 1.,
                budget_limited: false,
            },
        };
        assert!(planner.accepts(&old));
        let baseline = planner.queue.0.lock().unwrap().world.baseline().clone();
        planner.replace_world(Arc::new(World::new(baseline)));
        assert!(!planner.accepts(&old));
        let fresh = Result { epoch: 1, ..old };
        assert!(planner.accepts(&fresh));
    }
}
