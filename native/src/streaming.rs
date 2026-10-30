//! Latest-priority bounded jobs. Queued work is replaced, not left in a FIFO
//! after a turn. Only active/completed jobs survive until acknowledged.
use microvoxel::{
    generation::Terrain,
    render::{
        lod::MeshKey,
        ray::Brick,
        terrain::{Sampler, TerrainMesh},
    },
    World,
};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::{
        mpsc::{self, Receiver},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Job {
    key: MeshKey,
    ray: bool,
}
struct PageBatch {
    page: [i64; 2],
    jobs: VecDeque<Job>,
}
pub enum Payload {
    Mesh(
        TerrainMesh,
        Vec<microvoxel::render::triangulation::Prepared>,
        i64,
    ),
    Ray(Brick),
}
pub struct Completed {
    pub key: MeshKey,
    pub ray: bool,
    pub payload: Result<Payload, String>,
    pub elapsed: Duration,
}
#[derive(Default)]
struct Queue {
    queued: VecDeque<PageBatch>,
    busy: BTreeSet<Job>,
    wanted: BTreeSet<Job>,
    urgent: BTreeSet<Job>,
    closed: bool,
}
pub struct Workers {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    pub results: Receiver<Completed>,
    pub count: usize,
    spec: microvoxel::WorldSpec,
}
impl Workers {
    pub fn new(world: Arc<World<Terrain>>) -> Self {
        let spec = world.spec();
        let count =
            thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(2).clamp(1, 4));
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let (tx, results) = mpsc::sync_channel(count);
        let summaries = Arc::new(Mutex::new(
            microvoxel::render::terrain::SummaryCache::default(),
        ));
        for id in 0..count {
            let work = queue.clone();
            let world = world.clone();
            let summaries = summaries.clone();
            let tx = tx.clone();
            thread::Builder::new()
                .name(format!("voxel-build-{id}"))
                .spawn(move || {
                    let mut sampler = Sampler::with_summaries(world, summaries);
                    loop {
                        let job = {
                            let (lock, wake) = &*work;
                            let mut q = lock.lock().unwrap();
                            let job = loop {
                                if q.closed {
                                    break None;
                                }
                                let position = q.queued.iter().position(|page| {
                                    page.jobs.iter().any(|j| {
                                        id != 0 || q.urgent.is_empty() || q.urgent.contains(j)
                                    })
                                });
                                if let Some(position) = position {
                                    let job_position = q.queued[position]
                                        .jobs
                                        .iter()
                                        .position(|j| {
                                            id != 0 || q.urgent.is_empty() || q.urgent.contains(j)
                                        })
                                        .unwrap();
                                    let job = q.queued[position].jobs.remove(job_position);
                                    if q.queued[position].jobs.is_empty() {
                                        q.queued.remove(position);
                                    }
                                    break job;
                                }
                                q = wake.wait(q).unwrap();
                            };
                            let Some(job) = job else {
                                break;
                            };
                            q.busy.insert(job);
                            job
                        };
                        let cancelled = || {
                            let q = work.0.lock().unwrap();
                            q.closed
                                || !q.wanted.contains(&job)
                                || (id == 0 && !q.urgent.is_empty() && !q.urgent.contains(&job))
                        };
                        let start = Instant::now();
                        let payload =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                if job.ray {
                                    sampler.ray_brick(job.key, cancelled).map(Payload::Ray)
                                } else {
                                    sampler
                                        .mesh_cancellable(job.key, cancelled)
                                        .and_then(|mesh| {
                                            if cancelled() {
                                                return Err("mesh cancelled".into());
                                            }
                                            let prepared = mesh.prepare(job.key);
                                            let solid_top = sampler.solid_top(job.key);
                                            Ok(Payload::Mesh(mesh, prepared, solid_top))
                                        })
                                }
                            }))
                            .unwrap_or_else(|panic| {
                                let message = panic
                                    .downcast_ref::<String>()
                                    .map(String::as_str)
                                    .or_else(|| panic.downcast_ref::<&str>().copied())
                                    .unwrap_or("unknown panic");
                                Err(format!("terrain build failed: {message}"))
                            });
                        if tx
                            .send(Completed {
                                key: job.key,
                                ray: job.ray,
                                payload,
                                elapsed: start.elapsed(),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .expect("voxel worker");
        }
        Self {
            queue,
            results,
            count,
            spec,
        }
    }
    pub fn retain(&self, keys: BTreeSet<MeshKey>) {
        let mut q = self.queue.0.lock().unwrap();
        q.wanted.retain(|j| keys.contains(&j.key));
        let wanted = q.wanted.clone();
        for page in &mut q.queued {
            page.jobs.retain(|j| wanted.contains(j));
        }
        q.queued.retain(|page| !page.jobs.is_empty());
    }
    pub fn acknowledge(&self, key: MeshKey, ray: bool) {
        self.queue.0.lock().unwrap().busy.remove(&Job { key, ray });
    }
    pub fn schedule(&self, keys: impl IntoIterator<Item = (MeshKey, bool)>, ray: bool) {
        let (lock, wake) = &*self.queue;
        let mut q = lock.lock().unwrap();
        let entries: Vec<_> = keys.into_iter().collect();
        q.urgent = entries
            .iter()
            .filter(|(_, urgent)| *urgent)
            .map(|(key, _)| Job { key: *key, ray })
            .collect();
        let jobs: Vec<_> = entries
            .into_iter()
            .map(|(key, _)| Job { key, ray })
            .collect();
        q.wanted = jobs.iter().copied().collect();
        q.queued.clear();
        let grid = microvoxel::render::lod::Grid::new(self.spec);
        let mut admitted = 0;
        for job in jobs {
            if !q.busy.contains(&job) {
                let [x, z] = job.key.tile.origin_in(grid);
                let page = self.spec.page(microvoxel::CellPos::new(x, 0, z));
                if let Some(batch) = q.queued.iter_mut().find(|batch| batch.page == page) {
                    batch.jobs.push_back(job);
                } else {
                    q.queued.push_back(PageBatch {
                        page,
                        jobs: VecDeque::from([job]),
                    });
                }
                admitted += 1;
                if admitted >= self.count * 2 {
                    break;
                }
            }
        }
        wake.notify_all();
    }
    pub fn outstanding(&self) -> usize {
        let q = self.queue.0.lock().unwrap();
        q.queued.iter().map(|p| p.jobs.len()).sum::<usize>() + q.busy.len()
    }
}
impl Drop for Workers {
    fn drop(&mut self) {
        let (lock, wake) = &*self.queue;
        lock.lock().unwrap().closed = true;
        wake.notify_all();
    }
}
