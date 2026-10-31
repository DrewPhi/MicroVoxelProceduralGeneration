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
    epoch: u64,
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
    epoch: u64,
    pub key: MeshKey,
    pub ray: bool,
    pub payload: Result<Payload, String>,
    pub elapsed: Duration,
}
#[derive(Default)]
struct Queue {
    epoch: u64,
    world: Option<Arc<World<Terrain>>>,
    summaries: Option<microvoxel::render::terrain::SharedSummaries>,
    queued: VecDeque<PageBatch>,
    busy: BTreeSet<Job>,
    wanted: BTreeSet<Job>,
    urgent: BTreeSet<Job>,
    prefetch: BTreeSet<Job>,
    closed: bool,
}
impl Queue {
    fn next_job(&mut self, id: usize) -> Option<Job> {
        let allowed = |j: &Job| {
            (id != 0 || self.urgent.is_empty() || self.urgent.contains(j))
                && (!self.prefetch.contains(j) || self.urgent.is_empty())
        };
        // Page grouping must never let speculative jobs jump ahead of real
        // requests on a later page.
        let real = self
            .queued
            .iter()
            .flat_map(|p| &p.jobs)
            .any(|j| allowed(j) && !self.prefetch.contains(j));
        let eligible = |j: &Job| allowed(j) && (!real || !self.prefetch.contains(j));
        let page = self
            .queued
            .iter()
            .position(|p| p.jobs.iter().any(eligible))?;
        let job = self.queued[page].jobs.iter().position(eligible).unwrap();
        let result = self.queued[page].jobs.remove(job);
        if self.queued[page].jobs.is_empty() {
            self.queued.remove(page);
        }
        result
    }
}
pub struct Workers {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    pub results: Receiver<Completed>,
    pub count: usize,
}
impl Workers {
    pub fn new(world: Arc<World<Terrain>>) -> Self {
        let count =
            thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(2).clamp(1, 4));
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let (tx, results) = mpsc::sync_channel(count);
        let summaries = Arc::new(Mutex::new(
            microvoxel::render::terrain::SummaryCache::default(),
        ));
        queue.0.lock().unwrap().world = Some(world.clone());
        queue.0.lock().unwrap().summaries = Some(summaries.clone());
        for id in 0..count {
            let work = queue.clone();
            let world = world.clone();
            let summaries = summaries.clone();
            let tx = tx.clone();
            thread::Builder::new()
                .name(format!("voxel-build-{id}"))
                .spawn(move || {
                    let mut current_world = world.clone();
                    let mut sampler = Sampler::with_summaries(world, summaries);
                    loop {
                        let (job, job_world, job_summaries) = {
                            let (lock, wake) = &*work;
                            let mut q = lock.lock().unwrap();
                            let job = loop {
                                if q.closed {
                                    break None;
                                }
                                if let Some(job) = q.next_job(id) {
                                    break Some(job);
                                }
                                q = wake.wait(q).unwrap();
                            };
                            let Some(job) = job else {
                                break;
                            };
                            q.busy.insert(job);
                            (
                                job,
                                q.world.as_ref().unwrap().clone(),
                                q.summaries.as_ref().unwrap().clone(),
                            )
                        };
                        if !Arc::ptr_eq(&current_world, &job_world) {
                            current_world = job_world;
                            sampler = Sampler::with_summaries(current_world.clone(), job_summaries);
                        }
                        let build_key = MeshKey {
                            revision: current_world.revision(),
                            ..job.key
                        };
                        let cancelled = || {
                            let q = work.0.lock().unwrap();
                            q.closed
                                || q.epoch != job.epoch
                                || !q.wanted.contains(&job)
                                || ((id == 0 || q.prefetch.contains(&job))
                                    && !q.urgent.is_empty()
                                    && !q.urgent.contains(&job))
                        };
                        let start = Instant::now();
                        let payload =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                if job.ray {
                                    sampler.ray_brick(build_key, cancelled).map(Payload::Ray)
                                } else {
                                    sampler.display_mesh(build_key, cancelled).and_then(|mesh| {
                                        if cancelled() {
                                            return Err("mesh cancelled".into());
                                        }
                                        let prepared = mesh.prepare(job.key);
                                        // Sampled terrain bounds must not serve as proof of occlusion.
                                        let solid_top = 0;
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
                                epoch: job.epoch,
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
    pub fn replace_world(&self, world: Arc<World<Terrain>>) {
        let mut q = self.queue.0.lock().unwrap();
        q.epoch = q.epoch.wrapping_add(1);
        q.world = Some(world);
        q.summaries = Some(Arc::new(Mutex::new(
            microvoxel::render::terrain::SummaryCache::default(),
        )));
        q.queued.clear();
        q.wanted.clear();
        q.urgent.clear();
        q.prefetch.clear();
        self.queue.1.notify_all();
    }
    pub fn retain(&self, keys: BTreeSet<MeshKey>) {
        let mut q = self.queue.0.lock().unwrap();
        q.wanted.retain(|j| keys.contains(&j.key));
        let wanted = q.wanted.clone();
        q.urgent.retain(|job| wanted.contains(job));
        q.prefetch.retain(|job| wanted.contains(job));
        for page in &mut q.queued {
            page.jobs.retain(|j| wanted.contains(j));
        }
        q.queued.retain(|page| !page.jobs.is_empty());
        self.queue.1.notify_all();
    }
    /// Acknowledge this exact world/job namespace before accepting its payload.
    /// An old completion must never release a new world's identical regional key.
    pub fn acknowledge(&self, result: &Completed) -> bool {
        let mut q = self.queue.0.lock().unwrap();
        q.busy.remove(&Job {
            epoch: result.epoch,
            key: result.key,
            ray: result.ray,
        });
        !q.closed && q.epoch == result.epoch
    }
    #[cfg(test)]
    pub fn schedule(&self, keys: impl IntoIterator<Item = (MeshKey, bool)>, ray: bool) {
        self.schedule_with_prefetch(keys, ray, std::iter::empty());
    }
    pub fn schedule_with_prefetch(
        &self,
        keys: impl IntoIterator<Item = (MeshKey, bool)>,
        ray: bool,
        prefetch: impl IntoIterator<Item = MeshKey>,
    ) {
        let (lock, wake) = &*self.queue;
        let mut q = lock.lock().unwrap();
        let epoch = q.epoch;
        let mut entries: Vec<_> = keys.into_iter().collect();
        let required: BTreeSet<_> = entries.iter().map(|(k, _)| *k).collect();
        let warm: Vec<_> = prefetch
            .into_iter()
            .filter(|k| !required.contains(k))
            .take(48)
            .collect();
        q.prefetch = warm
            .iter()
            .map(|key| Job {
                epoch,
                key: *key,
                ray,
            })
            .collect();
        entries.extend(warm.into_iter().map(|key| (key, false)));
        q.urgent = entries
            .iter()
            .filter(|(_, urgent)| *urgent)
            .map(|(key, _)| Job {
                epoch,
                key: *key,
                ray,
            })
            .collect();
        let jobs: Vec<_> = entries
            .into_iter()
            .map(|(key, _)| Job { epoch, key, ray })
            .collect();
        q.wanted = jobs.iter().copied().collect();
        q.queued.clear();
        let spec = q.world.as_ref().unwrap().spec();
        let grid = microvoxel::render::lod::Grid::new(spec);
        let mut admitted = 0;
        let mut seen = BTreeSet::new();
        for job in jobs {
            if seen.insert(job) && !q.busy.contains(&job) {
                let [x, z] = job.key.tile.origin_in(grid);
                let page = spec.page(microvoxel::CellPos::new(x, 0, z));
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

#[cfg(test)]
mod tests {
    use super::*;
    use microvoxel::render::lod::TileKey;
    fn workers() -> Workers {
        let (_, results) = mpsc::channel();
        Workers {
            queue: Arc::new((
                Mutex::new(Queue {
                    world: Some(Arc::new(World::new(Terrain::new(5)))),
                    ..Queue::default()
                }),
                Condvar::new(),
            )),
            results,
            count: 2,
        }
    }
    fn key() -> MeshKey {
        MeshKey {
            corner_levels: None,
            tile: TileKey {
                level: 0,
                x: 0,
                z: 0,
            },
            neighbors: [0; 4],
            revision: 0,
        }
    }
    #[test]
    fn real_jobs_precede_speculation_across_pages_and_urgent_work_blocks_it() {
        let warm = Job {
            epoch: 0,
            key: key(),
            ray: false,
        };
        let real = Job {
            key: MeshKey {
                tile: TileKey { x: 1, ..key().tile },
                ..key()
            },
            ..warm
        };
        let mut q = Queue::default();
        q.prefetch.insert(warm);
        q.queued.push_back(PageBatch {
            page: [0, 0],
            jobs: [warm].into(),
        });
        q.queued.push_back(PageBatch {
            page: [1, 0],
            jobs: [real].into(),
        });
        assert_eq!(q.next_job(1), Some(real));
        q.urgent.insert(real);
        assert_eq!(q.next_job(1), None);
        q.urgent.clear();
        assert_eq!(q.next_job(1), Some(warm));
    }

    #[test]
    fn old_world_completion_cannot_acknowledge_identical_new_job() {
        let workers = workers();
        let key = key();
        let old = Completed {
            epoch: 0,
            key,
            ray: false,
            payload: Err("cancelled".into()),
            elapsed: Duration::ZERO,
        };
        workers.queue.0.lock().unwrap().busy.insert(Job {
            epoch: 0,
            key,
            ray: false,
        });
        // Independent baseline worlds commonly both have numeric revision zero.
        let baseline = workers
            .queue
            .0
            .lock()
            .unwrap()
            .world
            .as_ref()
            .unwrap()
            .baseline()
            .clone();
        workers.replace_world(Arc::new(World::new(baseline)));
        workers.schedule([(key, true)], false);
        let fresh = workers.queue.0.lock().unwrap().queued[0].jobs[0];
        assert_eq!(fresh.epoch, 1);
        workers.queue.0.lock().unwrap().busy.insert(fresh);
        assert!(!workers.acknowledge(&old));
        assert!(workers.queue.0.lock().unwrap().busy.contains(&fresh));
        let new = Completed { epoch: 1, ..old };
        assert!(workers.acknowledge(&new));
        assert!(!workers.queue.0.lock().unwrap().busy.contains(&fresh));
    }
    #[test]
    fn repeated_priorities_stay_bounded_and_duplicate_jobs_are_unique() {
        let workers = workers();
        let keys: Vec<_> = (0..100)
            .map(|x| {
                (
                    MeshKey {
                        tile: TileKey { x, ..key().tile },
                        ..key()
                    },
                    false,
                )
            })
            .collect();
        for _ in 0..100 {
            workers.schedule(keys.iter().copied().chain(keys.iter().copied()), false);
            assert!(workers.outstanding() <= workers.count * 2);
            let q = workers.queue.0.lock().unwrap();
            let jobs: Vec<_> = q
                .queued
                .iter()
                .flat_map(|page| page.jobs.iter().copied())
                .collect();
            assert_eq!(
                jobs.iter().copied().collect::<BTreeSet<_>>().len(),
                jobs.len()
            );
        }
        workers.schedule([(key(), true)], false);
        workers.retain(BTreeSet::new());
        let q = workers.queue.0.lock().unwrap();
        assert!(q.queued.is_empty() && q.urgent.is_empty());
    }
}
