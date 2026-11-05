//! Single-worker, bounded demand mailbox and canonical footprint cache.
//!
//! A worker owns one immutable source (seed/parameters/version); camera epochs
//! never enter sample keys. Each level moves on its own fixed global lattice.
//! Complete per-level pages survive cancelled selections, while a single newest
//! demand replaces queued work. A coherent older snapshot may publish only while
//! its finest center is within 24m of current demand; far travel cancels per row.
//! This is an empirical travel budget, not a game-speed or readiness guarantee.
use super::{page_bytes, Sample, TerrainData, MAX_LEVELS, MIN_LEVELS};
use frontier_world::FbmTerrain;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Condvar, Mutex},
    time::Instant,
};
const CACHE_LIMIT: usize = 600_000;
type Key = (i64, i64, u8);
#[derive(Clone, Copy)]
struct Value {
    height: f64,
    color: [f32; 4],
}
pub(super) struct SampleCache {
    pub(super) pages: [Option<([f64; 2], Arc<[Sample]>)>; MAX_LEVELS as usize],
    pub(super) generated_pages: u64,
    pub(super) reused_samples: u64,
    values: HashMap<Key, Value>,
    fifo: VecDeque<Key>,
    hits: u64,
    misses: u64,
}
impl SampleCache {
    pub(super) fn new() -> Self {
        Self {
            pages: std::array::from_fn(|_| None),
            generated_pages: 0,
            reused_samples: 0,
            values: HashMap::new(),
            fifo: VecDeque::new(),
            hits: 0,
            misses: 0,
        }
    }
    fn value(&mut self, source: &FbmTerrain, x: f64, z: f64, step: f64) -> Value {
        let key = (
            (x * 2.).round() as i64,
            (z * 2.).round() as i64,
            (step * 2.).log2() as u8,
        );
        if let Some(v) = self.values.get(&key) {
            self.hits += 1;
            return *v;
        }
        self.misses += 1;
        let mut value = Value {
            height: 0.,
            color: [0., 0., 0., 1.],
        };
        let d = step * 0.25;
        for (x, z) in [
            (x - d, z - d),
            (x + d, z - d),
            (x - d, z + d),
            (x + d, z + d),
        ] {
            let h = source.height_m(x, z);
            value.height += h * 0.25;
            let c = source.surface_color(x, z, h);
            for (dst, src) in value.color[..3].iter_mut().zip(c) {
                *dst += src * 0.25;
            }
        }
        if self.values.len() == CACHE_LIMIT {
            if let Some(old) = self.fifo.pop_front() {
                self.values.remove(&old);
            }
        }
        self.values.insert(key, value);
        self.fifo.push_back(key);
        value
    }
    fn normal(&mut self, source: &FbmTerrain, x: f64, z: f64, step: f64) -> [f32; 4] {
        let dx = self.value(source, x - step, z, step).height
            - self.value(source, x + step, z, step).height;
        let dz = self.value(source, x, z - step, step).height
            - self.value(source, x, z + step, step).height;
        let n = glam::DVec3::new(dx, 2. * step, dz).normalize();
        [n.x as f32, n.y as f32, n.z as f32, 0.]
    }
    pub(super) fn sample(
        &mut self,
        source: &FbmTerrain,
        x: f64,
        z: f64,
        step: f64,
        center: [f64; 2],
    ) -> Sample {
        let own = self.value(source, x, z, step);
        let ps = step * 2.;
        // Parent anchors use the same translated lattice as immutable topology.
        let px = ((x - center[0]) / ps).floor() * ps + center[0];
        let pz = ((z - center[1]) / ps).floor() * ps + center[1];
        let u = (x - px) / ps;
        let v = (z - pz) / ps;
        let [a, b, c, d] = [(px, pz), (px + ps, pz), (px, pz + ps), (px + ps, pz + ps)]
            .map(|(x, z)| self.value(source, x, z, ps));
        let interp = |a: f64, b: f64, c: f64, d: f64| {
            if u + v <= 1. {
                a + (b - a) * u + (c - a) * v
            } else {
                d + (c - d) * (1. - u) + (b - d) * (1. - v)
            }
        };
        let mut color = [0.; 4];
        for (i, out) in color.iter_mut().enumerate() {
            *out = interp(
                a.color[i] as f64,
                b.color[i] as f64,
                c.color[i] as f64,
                d.color[i] as f64,
            ) as f32;
        }
        let normals = [(px, pz), (px + ps, pz), (px, pz + ps), (px + ps, pz + ps)]
            .map(|(x, z)| self.normal(source, x, z, ps));
        let parent_normal = std::array::from_fn(|i| {
            interp(
                normals[0][i] as f64,
                normals[1][i] as f64,
                normals[2][i] as f64,
                normals[3][i] as f64,
            ) as f32
        });
        Sample {
            heights: [
                own.height as f32,
                interp(a.height, b.height, c.height, d.height) as f32,
                0.,
                0.,
            ],
            normal: self.normal(source, x, z, step),
            parent_normal,
            color: own.color,
            parent_color: color,
        }
    }
    fn resident_bytes(&self) -> u64 {
        // Capacity-accounted estimate, including hash-table controls and FIFO;
        // allocator metadata is not measured.
        (self.values.capacity() * (std::mem::size_of::<(Key, Value)>() + 16)
            + self.fifo.capacity() * std::mem::size_of::<Key>()) as u64
            + self
                .pages
                .iter()
                .flatten()
                .map(|(_, p)| (p.len() * std::mem::size_of::<Sample>()) as u64)
                .sum::<u64>()
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct GenerationMetrics {
    pub reused_samples: u64,
    pub generation_ms: f64,
    pub cache_hits: u64,
    pub generated_samples: u64,
    pub generated_pages: u64,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct StreamMetrics {
    pub reused_samples: u64,
    pub requests: u64,
    pub publications: u64,
    pub stale_rejections: u64,
    pub generated_pages: u64,
    pub generated_samples: u64,
    pub cache_hits: u64,
    pub cancellations: u64,
    pub resident_bytes: u64,
    pub inflight_bytes: u64,
    pub upload_bytes: u64,
    pub generation_ms: f64,
}
pub struct TerrainUpdate {
    pub data: TerrainData,
    pub center: [f64; 2],
    pub epoch: u64,
    pub metrics: GenerationMetrics,
}
struct State {
    demand: Option<(u64, [f64; 2])>,
    desired: Option<[f64; 2]>,
    epoch: u64,
    ready: Option<TerrainUpdate>,
    published: Option<[[f64; 2]; MAX_LEVELS as usize]>,
    stop: bool,
    metrics: StreamMetrics,
}
pub struct TerrainStreamer {
    shared: Arc<(Mutex<State>, Condvar)>,
    level_count: u32,
}
fn useful(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs().max((a[1] - b[1]).abs()) <= 24.
}
impl TerrainStreamer {
    #[cfg(test)]
    pub fn new(source: FbmTerrain) -> Self {
        Self::with_levels(source, MIN_LEVELS).expect("default terrain level count")
    }
    pub fn with_levels(source: FbmTerrain, level_count: u32) -> Result<Self, String> {
        if !(MIN_LEVELS..=MAX_LEVELS).contains(&level_count) {
            return Err(format!("view levels must be {MIN_LEVELS}..={MAX_LEVELS}"));
        }
        let shared = Arc::new((
            Mutex::new(State {
                demand: None,
                desired: None,
                epoch: 0,
                ready: None,
                published: None,
                stop: false,
                metrics: StreamMetrics::default(),
            }),
            Condvar::new(),
        ));
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("terrain-samples".into())
            .spawn(move || {
                let mut cache = SampleCache::new();
                loop {
                    let (epoch, center) = {
                        let (lock, wake) = &*worker;
                        let mut s = lock.lock().unwrap();
                        while s.demand.is_none() && !s.stop {
                            s = wake.wait(s).unwrap();
                        }
                        if s.stop {
                            break;
                        }
                        s.metrics.inflight_bytes = page_bytes(level_count) + page_bytes(1);
                        s.demand.take().unwrap()
                    };
                    let start = Instant::now();
                    let hits = cache.hits;
                    let misses = cache.misses;
                    let pages = cache.generated_pages;
                    let reused = cache.reused_samples;
                    // Near demand may supersede selection while this coherent snapshot
                    // is still useful. Far travel/teleport cancels at each generated row.
                    let data = TerrainData::generate_levels(
                        &source,
                        center,
                        &mut cache,
                        level_count,
                        || {
                            let s = worker.0.lock().unwrap();
                            s.stop || !useful(center, s.desired.unwrap())
                        },
                    );
                    let metrics = GenerationMetrics {
                        reused_samples: cache.reused_samples - reused,
                        generation_ms: start.elapsed().as_secs_f64() * 1000.,
                        cache_hits: cache.hits - hits,
                        generated_samples: cache.misses - misses,
                        generated_pages: cache.generated_pages - pages,
                    };
                    let mut s = worker.0.lock().unwrap();
                    s.metrics.cache_hits += metrics.cache_hits;
                    s.metrics.reused_samples += metrics.reused_samples;
                    s.metrics.generated_samples += metrics.generated_samples;
                    s.metrics.resident_bytes = cache.resident_bytes();
                    s.metrics.inflight_bytes = 0;
                    s.metrics.generation_ms = metrics.generation_ms;
                    s.metrics.generated_pages += metrics.generated_pages;
                    if s.stop {
                        break;
                    }
                    if !useful(center, s.desired.unwrap()) || data.is_none() {
                        s.metrics.cancellations += 1;
                        continue;
                    }
                    s.ready = Some(TerrainUpdate {
                        data: data.unwrap(),
                        center,
                        epoch,
                        metrics,
                    });
                }
            })
            .expect("start terrain worker");
        Ok(Self {
            shared,
            level_count,
        })
    }
    pub fn request(&mut self, position: [f64; 2]) -> u64 {
        let center = position.map(|v| v.floor());
        let mut s = self.shared.0.lock().unwrap();
        if s.desired == Some(center) {
            return s.epoch;
        }
        s.epoch += 1;
        s.metrics.requests += 1;
        s.desired = Some(center);
        let epoch = s.epoch;
        s.demand = Some((epoch, center));
        if s.ready.as_ref().is_some_and(|r| !useful(r.center, center)) {
            s.ready = None;
            s.metrics.stale_rejections += 1;
        }
        self.shared.1.notify_one();
        epoch
    }
    pub fn poll(&mut self) -> Option<TerrainUpdate> {
        let mut s = self.shared.0.lock().unwrap();
        let update = s.ready.take()?;
        if update.epoch > s.epoch || !useful(update.center, s.desired.unwrap()) {
            s.metrics.stale_rejections += 1;
            return None;
        }
        s.metrics.publications += 1;
        s.metrics.generation_ms = update.metrics.generation_ms;
        let dirty = (0..self.level_count as usize)
            .filter(|&i| s.published.is_none_or(|p| p[i] != update.data.centers[i]))
            .count();
        s.metrics.upload_bytes += page_bytes(1) * dirty as u64;
        s.published = Some(update.data.centers);
        Some(update)
    }
    pub fn metrics(&self) -> StreamMetrics {
        let state = self.shared.0.lock().unwrap();
        let mut metrics = state.metrics;
        if state.ready.is_some() {
            metrics.inflight_bytes += page_bytes(self.level_count);
        }
        metrics
    }
}
impl Drop for TerrainStreamer {
    fn drop(&mut self) {
        let mut s = self.shared.0.lock().unwrap();
        s.stop = true;
        self.shared.1.notify_one();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn translated_negative_centers_reuse_canonical_cache_and_join() {
        let source = FbmTerrain::new(7, Default::default()).unwrap();
        let mut cache = SampleCache::new();
        for center in [
            [0., 0.],
            [1., 0.],
            [2., 0.],
            [3., 0.],
            [-1., -3.],
            [-64., 127.],
        ] {
            let start = Instant::now();
            let hits = cache.hits;
            let misses = cache.misses;
            let data = TerrainData::generate(&source, center, &mut cache, || false).unwrap();
            eprintln!(
                "terrain center {center:?}: {:.1}ms {} hits {} new footprint samples, {} bytes",
                start.elapsed().as_secs_f64() * 1000.,
                cache.hits - hits,
                cache.misses - misses,
                cache.resident_bytes()
            );
            let count = ((super::super::N * 2 + 1).pow(2)) as usize;
            for level in 0..6 {
                let coarse = &data.vertices[(level + 1) * count..(level + 2) * count];
                for v in data.vertices[level * count..(level + 1) * count]
                    .iter()
                    .filter(|v| v.morph == 1.)
                {
                    let c = coarse
                        .iter()
                        .find(|c| {
                            (0..2).all(|axis| {
                                c.position[axis] as f64 + data.centers[level + 1][axis]
                                    == v.position[axis] as f64 + data.centers[level][axis]
                            })
                        })
                        .unwrap();
                    let a = data.samples[v.sample as usize];
                    let b = data.samples[c.sample as usize];
                    assert_eq!(a.heights[1], b.heights[0]);
                    assert_eq!(a.parent_color, b.color);
                    assert_eq!(a.parent_normal, b.normal);
                }
            }
        }
        let before = cache.misses;
        let a = cache.value(&source, -4., -8., 0.5);
        let b = cache.value(&source, -4., -8., 0.5);
        assert_eq!(a.height, b.height);
        assert!(cache.misses - before <= 1);
        assert!(cache.values.len() <= CACHE_LIMIT);
    }
    #[test]
    fn cancellation_interrupts_generation_at_row_boundaries() {
        let source = FbmTerrain::new(7, Default::default()).unwrap();
        let mut cache = SampleCache::new();
        let rows = std::cell::Cell::new(0);
        assert!(TerrainData::generate(&source, [0., 0.], &mut cache, || {
            rows.set(rows.get() + 1);
            rows.get() > 2
        })
        .is_none());
        assert!(cache.misses > 0);
        assert!(rows.get() <= 3);
    }
    #[test]
    fn coalesced_teleport_demand_only_delivers_destination_coverage() {
        let source = FbmTerrain::new(
            7,
            frontier_world::FbmParams {
                octaves: 1,
                ..Default::default()
            },
        )
        .unwrap();
        for levels in [MIN_LEVELS, MAX_LEVELS] {
            let mut stream = if levels == MIN_LEVELS {
                TerrainStreamer::new(source.clone())
            } else {
                TerrainStreamer::with_levels(source.clone(), levels).unwrap()
            };
            stream.request([0., 0.]);
            let epoch = stream.request([-10000., 10000.]);
            let deadline = Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Some(update) = stream.poll() {
                    assert_eq!(update.center, [-10000., 10000.]);
                    assert_eq!(update.epoch, epoch);
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "destination generation timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let metrics = stream.metrics();
            assert_eq!(metrics.publications, 1);
            assert_eq!(metrics.requests, 2);
            assert_eq!(metrics.upload_bytes, page_bytes(levels));
            assert!(metrics.resident_bytes > page_bytes(levels));
        }
    }
    #[test]
    fn incremental_samples_equal_cold_generation_across_collapsed_edges() {
        let source = FbmTerrain::new(
            7,
            frontier_world::FbmParams {
                octaves: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let mut warm = SampleCache::new();
        for center in [
            [0., 0.],
            [1., 0.],
            [3., 5.],
            [-1., -3.],
            [-64., 127.],
            [127., -64.],
        ] {
            let actual = TerrainData::generate(&source, center, &mut warm, || false).unwrap();
            let expected =
                TerrainData::generate(&source, center, &mut SampleCache::new(), || false).unwrap();
            assert_eq!(
                bytemuck::cast_slice::<Sample, u8>(&actual.samples),
                bytemuck::cast_slice::<Sample, u8>(&expected.samples),
                "incremental contents differ at {center:?}"
            );
        }
        assert!(warm.reused_samples > 400_000);
    }
}
