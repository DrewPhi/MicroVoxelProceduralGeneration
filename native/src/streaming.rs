//! Latest-priority bounded jobs. Queued work is replaced, not left in a FIFO
//! after a turn. Only active/completed jobs survive until acknowledged.
use microvoxel::{
    generation::ProceduralTerrain,
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
    queued: VecDeque<Job>,
    busy: BTreeSet<Job>,
    wanted: BTreeSet<Job>,
    urgent: BTreeSet<Job>,
    closed: bool,
}
pub struct Workers {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    pub results: Receiver<Completed>,
    pub count: usize,
}
impl Workers {
    pub fn new(world: Arc<World<ProceduralTerrain>>) -> Self {
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
                                let position = q.queued.iter().position(|j| {
                                    id != 0 || q.urgent.is_empty() || q.urgent.contains(j)
                                });
                                if let Some(position) = position {
                                    break q.queued.remove(position);
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
                        let payload = if job.ray {
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
                        };
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
        }
    }
    pub fn retain(&self, keys: BTreeSet<MeshKey>) {
        let mut q = self.queue.0.lock().unwrap();
        q.wanted.retain(|j| keys.contains(&j.key));
        let wanted = q.wanted.clone();
        q.queued.retain(|j| wanted.contains(j));
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
        for job in jobs {
            if !q.busy.contains(&job) {
                q.queued.push_back(job);
                if q.queued.len() >= self.count * 2 {
                    break;
                }
            }
        }
        wake.notify_all();
    }
    pub fn outstanding(&self) -> usize {
        let q = self.queue.0.lock().unwrap();
        q.queued.len() + q.busy.len()
    }
}
impl Drop for Workers {
    fn drop(&mut self) {
        let (lock, wake) = &*self.queue;
        lock.lock().unwrap().closed = true;
        wake.notify_all();
    }
}
