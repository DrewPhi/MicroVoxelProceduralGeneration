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
}
pub struct Result {
    pub elapsed: Duration,
    pub generation: u64,
    pub selection: lod::Selection,
}
struct Queue {
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
                loop {
                    let request = {
                        let (lock, wake) = &*work;
                        let mut state = lock.lock().unwrap();
                        while state.latest.is_none() && !state.closed {
                            state = wake.wait(state).unwrap();
                        }
                        if state.closed {
                            break;
                        }
                        state.latest.take().unwrap()
                    };
                    let Request {
                        generation,
                        eye,
                        height,
                        fov,
                        previous,
                        budget,
                        quality,
                    } = request;
                    let started = Instant::now();
                    let selection =
                        lod::select_adaptive(eye, height, fov, &previous, budget, quality, |key| {
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
                        });
                    // A stale plan is disposable. Keep the newest queued intent.
                    if work
                        .0
                        .lock()
                        .unwrap()
                        .latest
                        .as_ref()
                        .is_some_and(|r| r.generation > generation)
                    {
                        continue;
                    }
                    if tx
                        .send(Result {
                            elapsed: started.elapsed(),
                            generation,
                            selection,
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
