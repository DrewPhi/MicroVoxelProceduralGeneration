//! Latest-intent planner. One queued request and one result keep CPU/memory work
//! bounded. Terrain-error sampling and quadtree selection never block a frame.
use microvoxel::{
    generation::ProceduralTerrain,
    render::lod::{self, Layout, Quality},
    World,
};
use std::{
    collections::BTreeMap,
    sync::{
        mpsc::{self, Receiver},
        Arc, Condvar, Mutex,
    },
    thread,
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
    pub fn new(world: Arc<World<ProceduralTerrain>>) -> Self {
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
                    if errors.len() > 8192 {
                        errors.clear();
                    }
                    let selection =
                        lod::select_adaptive(eye, height, fov, &previous, budget, quality, |key| {
                            Some(*errors.entry(key).or_insert_with(|| {
                                let [x, z] = key.origin();
                                let edge = key.edge();
                                let mut heights = [0.; 9];
                                for j in 0..3 {
                                    for i in 0..3 {
                                        let p = microvoxel::CellPos::new(
                                            (x + edge * i / 2).min(lod::EXTENT - 1),
                                            0,
                                            (z + edge * j / 2).min(lod::EXTENT - 1),
                                        );
                                        heights[(j * 3 + i) as usize] = world
                                            .environment_at(p)
                                            .map_or(0., |e| e.elevation_m as f64);
                                    }
                                }
                                let low =
                                    heights.iter().copied().fold(f64::INFINITY, f64::min) - 4.;
                                let high =
                                    heights.iter().copied().fold(f64::NEG_INFINITY, f64::max) + 4.;
                                let deviation = (heights[4]
                                    - (heights[0] + heights[2] + heights[6] + heights[8]) / 4.)
                                    .abs();
                                lod::TerrainError {
                                    low,
                                    high,
                                    deviation,
                                }
                            }))
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
