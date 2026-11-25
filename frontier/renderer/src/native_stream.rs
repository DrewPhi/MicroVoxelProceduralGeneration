//! Bounded immutable FBM column streaming. Camera demand is separate from content
//! identity. One coordinator runs at most two chunk jobs and cancels between
//! pairs; a complete latest square
//! is the only publication unit. No edits or arbitrary volume sources are accepted.
use crate::{
    column_mesh::{self, ColumnMesh},
    mesh::Vertex,
};
use frontier_world::{Cell, World};
use std::{
    collections::{HashMap, VecDeque},
    ops::Range,
    sync::{Arc, Condvar, Mutex},
    time::Instant,
};

pub const CHUNK_CELLS: i64 = 128;
pub const CHUNK_METERS: f64 = 8.;
pub const CORE_METERS: f64 = 64.;
pub const CACHE_CHUNKS: usize = 128;
pub const CACHE_BYTES: usize = 512 * 1024 * 1024;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ChunkKey {
    pub x: i64,
    pub z: i64,
}
impl ChunkKey {
    pub fn origin(self) -> Cell {
        Cell {
            x: self.x * CHUNK_CELLS,
            y: 0,
            z: self.z * CHUNK_CELLS,
        }
    }
}
pub struct NativeChunk {
    pub key: ChunkKey,
    pub columns: ColumnMesh,
    /// Local-space bounds computed by the worker for render-side frustum culling.
    pub bounds: [[f32; 3]; 2],
    /// Index ranges: interior, -x, +x, -z, +z. Exterior core walls are omitted at draw time.
    pub ranges: [Range<u32>; 5],
    pub bytes: usize,
}
impl NativeChunk {
    fn build(world: &World, key: ChunkKey) -> Result<Self, String> {
        let mut columns = column_mesh::build(world, key.origin(), [128, 128])?;
        let mut bounds = [[f32::INFINITY; 3], [f32::NEG_INFINITY; 3]];
        let mut groups: [Vec<u32>; 5] = std::array::from_fn(|_| Vec::new());
        for (q, v) in columns.mesh.vertices.chunks_exact(4).enumerate() {
            for vertex in v {
                for axis in 0..3 {
                    bounds[0][axis] = bounds[0][axis].min(vertex.position[axis]);
                    bounds[1][axis] = bounds[1][axis].max(vertex.position[axis]);
                }
            }
            let group = if v.iter().all(|v| v.position[0] == 0.) && v[0].normal[0] != 0. {
                1
            } else if v.iter().all(|v| v.position[0] == 8.) && v[0].normal[0] != 0. {
                2
            } else if v.iter().all(|v| v.position[2] == 0.) && v[0].normal[2] != 0. {
                3
            } else if v.iter().all(|v| v.position[2] == 8.) && v[0].normal[2] != 0. {
                4
            } else {
                0
            };
            groups[group].extend_from_slice(&columns.mesh.indices[q * 6..q * 6 + 6]);
        }
        columns.mesh.indices.clear();
        let ranges = groups.map(|g| {
            let start = columns.mesh.indices.len() as u32;
            columns.mesh.indices.extend(g);
            start..columns.mesh.indices.len() as u32
        });
        // Extraction grows geometrically. Reclaim spare capacity on this worker,
        // before caching or sharing with render-side staging.
        columns.mesh.vertices.shrink_to_fit();
        columns.mesh.indices.shrink_to_fit();
        let bytes = columns.mesh.vertices.capacity() * std::mem::size_of::<Vertex>()
            + columns.mesh.indices.capacity() * 4
            + columns.heights.allocated_bytes();
        Ok(Self {
            key,
            columns,
            bounds,
            ranges,
            bytes,
        })
    }
    pub fn draw_ranges(&self, center: [f64; 2]) -> Vec<Range<u32>> {
        let c = center.map(|v| (v / CHUNK_METERS).round() as i64);
        let mut ranges = vec![self.ranges[0].clone()];
        for (i, enabled) in [
            self.key.x != c[0] - 4,
            self.key.x != c[0] + 3,
            self.key.z != c[1] - 4,
            self.key.z != c[1] + 3,
        ]
        .into_iter()
        .enumerate()
        {
            if enabled {
                ranges.push(self.ranges[i + 1].clone());
            }
        }
        ranges
    }
}
pub struct NativeCoverage {
    pub revision: u64,
    pub center: [f64; 2],
    pub chunks: Vec<(ChunkKey, Arc<NativeChunk>)>,
}
impl NativeCoverage {
    pub fn height(&self, x: i64, z: i64) -> Option<i64> {
        let key = ChunkKey {
            x: x.div_euclid(CHUNK_CELLS),
            z: z.div_euclid(CHUNK_CELLS),
        };
        self.chunks
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, c)| c.columns.heights.height(x, z))
    }
}
#[derive(Default, Clone, Copy, Debug)]
pub struct NativeMetrics {
    pub requests: u64,
    pub jobs: u64,
    pub cache_hits: u64,
    pub reused_chunks: u64,
    pub cancellations: u64,
    pub stale_rejections: u64,
    pub publications: u64,
    pub cache_chunks: usize,
    pub cache_bytes: usize,
    pub generated_bytes: u64,
    pub pending_chunks: usize,
    pub generation_ms: f64,
    pub last_job_ms: f64,
    pub rejected_chunks: u64,
}
struct State {
    desired: Option<(u64, [i64; 2])>,
    ready: Option<NativeCoverage>,
    stop: bool,
    metrics: NativeMetrics,
    error: Option<String>,
}
pub struct NativeStreamer {
    shared: Arc<(Mutex<State>, Condvar)>,
    worker: Option<std::thread::JoinHandle<()>>,
}
fn keys(c: [i64; 2]) -> Vec<ChunkKey> {
    let mut out = Vec::with_capacity(64);
    for z in c[1] - 4..c[1] + 4 {
        for x in c[0] - 4..c[0] + 4 {
            out.push(ChunkKey { x, z });
        }
    }
    // Complete nearby chunks first. The set remains the same complete square;
    // urgent center coverage never waits behind speculative far corners.
    out.sort_by_key(|k| {
        (
            (2 * (k.x - c[0]) + 1).pow(2) + (2 * (k.z - c[1]) + 1).pow(2),
            k.z,
            k.x,
        )
    });
    out
}
fn snapped(previous: Option<[i64; 2]>, p: [f64; 2]) -> [i64; 2] {
    std::array::from_fn(|i| {
        if let Some(old) = previous {
            if (p[i] - old[i] as f64 * 8.).abs() <= 5. {
                return old[i];
            }
        }
        (p[i] / 8.).round() as i64
    })
}
struct Cache {
    map: HashMap<ChunkKey, Arc<NativeChunk>>,
    lru: VecDeque<ChunkKey>,
    bytes: usize,
}
impl Cache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            lru: VecDeque::new(),
            bytes: 0,
        }
    }
    fn get(&mut self, key: ChunkKey) -> Option<Arc<NativeChunk>> {
        let found = self.map.get(&key)?.clone();
        self.lru.retain(|k| *k != key);
        self.lru.push_back(key);
        Some(found)
    }
    fn insert(&mut self, chunk: Arc<NativeChunk>, protected: &[ChunkKey]) -> Result<(), String> {
        if chunk.bytes > CACHE_BYTES {
            return Err("native chunk exceeds 512MiB cache budget".into());
        }
        while self.map.len() >= CACHE_CHUNKS || self.bytes + chunk.bytes > CACHE_BYTES {
            let Some(index) = self.lru.iter().position(|k| !protected.contains(k)) else {
                return Err("native coherent coverage exceeds 512MiB cache budget".into());
            };
            let key = self.lru.remove(index).unwrap();
            self.bytes -= self.map.remove(&key).unwrap().bytes;
        }
        self.bytes += chunk.bytes;
        self.lru.push_back(chunk.key);
        self.map.insert(chunk.key, chunk);
        Ok(())
    }
}
impl NativeStreamer {
    pub fn new(world: World) -> Result<Self, String> {
        if world.fbm_terrain().is_none() || world.revision() != 0 {
            return Err("native streaming requires immutable unedited FBM".into());
        }
        let shared = Arc::new((
            Mutex::new(State {
                desired: None,
                ready: None,
                stop: false,
                metrics: NativeMetrics::default(),
                error: None,
            }),
            Condvar::new(),
        ));
        let worker_shared = shared.clone();
        let worker = std::thread::Builder::new()
            .name("native-chunks".into())
            .spawn(move || {
                let (lock, wake) = &*worker_shared;
                let mut cache = Cache::new();
                let mut completed = 0;
                loop {
                    let (revision, center) = {
                        let mut s = lock.lock().unwrap();
                        while !s.stop && s.desired.is_none_or(|(r, _)| r == completed) {
                            s = wake.wait(s).unwrap();
                        }
                        if s.stop {
                            break;
                        }
                        s.desired.unwrap()
                    };
                    let wanted = keys(center);
                    let mut chunks = Vec::with_capacity(64);
                    let mut failed = false;
                    for (index, key) in wanted.iter().enumerate() {
                        {
                            let mut s = lock.lock().unwrap();
                            if s.stop {
                                return;
                            }
                            if s.desired != Some((revision, center)) {
                                s.metrics.cancellations += 1;
                                failed = true;
                                break;
                            }
                            s.metrics.pending_chunks = 64 - index;
                        }
                        let chunk = if let Some(c) = cache.get(*key) {
                            let mut s = lock.lock().unwrap();
                            s.metrics.cache_hits += 1;
                            s.metrics.reused_chunks += 1;
                            c
                        } else {
                            let start = Instant::now();
                            // Two bounded jobs improve entering-edge throughput. No
                            // queued thread pool or stale backlog: reconsider demand
                            // after this pair, retaining both valid cached chunks.
                            let next = wanted[index + 1..]
                                .iter()
                                .copied()
                                .find(|k| !cache.map.contains_key(k));
                            let (result, extra) = std::thread::scope(|scope| {
                                let source = &world;
                                let other = next.map(|k| {
                                    scope.spawn(move || NativeChunk::build(source, k).map(Arc::new))
                                });
                                let result = NativeChunk::build(&world, *key).map(Arc::new);
                                let extra = other
                                    .map(|job| job.join().expect("native paired worker panicked"));
                                (result, extra)
                            });
                            let mut s = lock.lock().unwrap();
                            s.metrics.jobs += 1 + u64::from(extra.is_some());
                            s.metrics.last_job_ms = start.elapsed().as_secs_f64() * 1000.;
                            s.metrics.generation_ms += s.metrics.last_job_ms;
                            if let Some(extra) = extra {
                                match extra.and_then(|c| {
                                    cache.insert(c.clone(), &wanted)?;
                                    Ok(c)
                                }) {
                                    Ok(c) => s.metrics.generated_bytes += c.bytes as u64,
                                    Err(error) => {
                                        s.error = Some(error);
                                        s.metrics.rejected_chunks += 1;
                                        failed = true;
                                        break;
                                    }
                                }
                            }
                            match result.and_then(|c| {
                                cache.insert(c.clone(), &wanted)?;
                                Ok(c)
                            }) {
                                Ok(c) => {
                                    s.metrics.generated_bytes += c.bytes as u64;
                                    c
                                }
                                Err(error) => {
                                    s.error = Some(error);
                                    s.metrics.rejected_chunks += 1;
                                    failed = true;
                                    break;
                                }
                            }
                        };
                        chunks.push((*key, chunk));
                        let mut s = lock.lock().unwrap();
                        s.metrics.cache_bytes = cache.bytes;
                        s.metrics.cache_chunks = cache.map.len();
                    }
                    let mut s = lock.lock().unwrap();
                    s.metrics.pending_chunks = 0;
                    if s.desired != Some((revision, center)) {
                        s.metrics.stale_rejections += 1;
                        continue;
                    }
                    completed = revision;
                    if !failed {
                        s.ready = Some(NativeCoverage {
                            revision,
                            center: center.map(|v| v as f64 * 8.),
                            chunks,
                        });
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }
    pub fn request(&self, camera: [f64; 2]) -> u64 {
        let (lock, wake) = &*self.shared;
        let mut s = lock.lock().unwrap();
        // Keep key multiplication and canonical f64 lattice addresses exact.
        if camera.iter().any(|v| !v.is_finite() || v.abs() > 1e12) {
            s.error = Some("native camera outside supported coordinate range".into());
            return s.desired.map_or(0, |(r, _)| r);
        }
        let center = snapped(s.desired.map(|(_, c)| c), camera);
        if s.desired.is_some_and(|(_, old)| old == center) {
            return s.desired.unwrap().0;
        }
        let revision = s.desired.map_or(1, |(r, _)| r + 1);
        s.desired = Some((revision, center));
        s.ready = None;
        s.error = None;
        s.metrics.requests += 1;
        wake.notify_one();
        revision
    }
    pub fn desired_revision(&self) -> u64 {
        self.shared.0.lock().unwrap().desired.map_or(0, |(r, _)| r)
    }
    pub fn desired_center(&self) -> [f64; 2] {
        self.shared
            .0
            .lock()
            .unwrap()
            .desired
            .map_or([0., 0.], |(_, c)| c.map(|v| v as f64 * 8.))
    }
    pub fn take_ready(&self) -> Option<NativeCoverage> {
        let mut s = self.shared.0.lock().unwrap();
        let out = s.ready.take();
        if out.is_some() {
            s.metrics.publications += 1;
        }
        out
    }
    pub fn metrics(&self) -> NativeMetrics {
        self.shared.0.lock().unwrap().metrics
    }
    pub fn take_error(&self) -> Option<String> {
        self.shared.0.lock().unwrap().error.take()
    }
}
impl Drop for NativeStreamer {
    fn drop(&mut self) {
        {
            let mut s = self.shared.0.lock().unwrap();
            s.stop = true;
            self.shared.1.notify_one();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    fn flat() -> World {
        World::fbm(
            7,
            frontier_world::FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap()
    }
    fn ready(s: &NativeStreamer) -> NativeCoverage {
        let start = Instant::now();
        loop {
            if let Some(c) = s.take_ready() {
                return c;
            }
            assert!(s.take_error().is_none());
            assert!(
                start.elapsed().as_secs() < 30,
                "native worker timed out: {:?}",
                s.metrics()
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    #[test]
    fn stop_wait_resume_edges_and_turn_away_return_reuse_content() {
        let s = NativeStreamer::new(flat()).unwrap();
        let first = s.request([0., 0.]);
        let a = ready(&s);
        assert_eq!(a.revision, first);
        assert_eq!(a.chunks.len(), 64);
        let jobs = s.metrics().jobs;
        assert_eq!(s.request([4.9, 0.]), first);
        assert!(s.take_ready().is_none());
        s.request([5.1, 0.]);
        let b = ready(&s);
        assert_eq!(b.center, [8., 0.]);
        assert_eq!(s.metrics().jobs - jobs, 8);
        for (k, c) in &a.chunks {
            if let Some((_, other)) = b.chunks.iter().find(|(key, _)| key == k) {
                assert!(Arc::ptr_eq(c, other));
            }
        }
        s.request([0., 0.]);
        let returned = ready(&s);
        assert_eq!(s.metrics().jobs, jobs + 8);
        for ((k, a), (other, b)) in a.chunks.iter().zip(&returned.chunks) {
            assert_eq!(k, other);
            assert!(Arc::ptr_eq(a, b));
        }
        assert!(s.metrics().cache_bytes <= CACHE_BYTES);
        assert!(s.metrics().cache_chunks <= CACHE_CHUNKS);
        drop(s);
    }
    #[test]
    fn coalesces_obsolete_demand_without_publishing_it() {
        let s = NativeStreamer::new(flat()).unwrap();
        s.request([0., 0.]);
        s.request([800., -800.]);
        let latest = s.request([-800., 800.]);
        let coverage = ready(&s);
        assert_eq!(coverage.revision, latest);
        assert_eq!(coverage.center, [-800., 800.]);
        assert_eq!(
            coverage
                .chunks
                .iter()
                .map(|(k, _)| *k)
                .collect::<BTreeSet<_>>(),
            keys([-100, 100]).into_iter().collect()
        );
        assert!(s.take_ready().is_none());
    }
    fn faces(mesh: &crate::mesh::Mesh) -> BTreeSet<([i64; 3], usize, bool, [u32; 3])> {
        let mut result = BTreeSet::new();
        for q in mesh.vertices.chunks_exact(4) {
            let axis = q[0].normal.iter().position(|n| *n != 0.).unwrap();
            let positive = q[0].normal[axis] > 0.;
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            let p = q[0].position.map(|p| (p as f64 / 0.0625).round() as i64);
            let end = q[2].position.map(|p| (p as f64 / 0.0625).round() as i64);
            for a in p[u]..end[u] {
                for b in p[v]..end[v] {
                    let mut cell = p;
                    cell[u] = a;
                    cell[v] = b;
                    cell[axis] -= i64::from(positive);
                    cell[0] += mesh.origin.x;
                    cell[1] += mesh.origin.y;
                    cell[2] += mesh.origin.z;
                    assert!(result.insert((cell, axis, positive, q[0].color.map(f32::to_bits))));
                }
            }
        }
        result
    }
    #[test]
    fn neighboring_chunk_halos_have_exact_single_face_ownership() {
        let w = World::fbm(
            7,
            frontier_world::FbmParams {
                horizontal_scale: 16.,
                amplitude: 8.,
                ..Default::default()
            },
        )
        .unwrap();
        let a = NativeChunk::build(&w, ChunkKey { x: -1, z: 0 }).unwrap();
        let b = NativeChunk::build(&w, ChunkKey { x: 0, z: 0 }).unwrap();
        let big = column_mesh::build(
            &w,
            Cell {
                x: -128,
                y: 0,
                z: 0,
            },
            [256, 128],
        )
        .unwrap();
        let mut actual = faces(&a.columns.mesh);
        for face in faces(&b.columns.mesh) {
            assert!(actual.insert(face), "duplicated chunk interface face");
        }
        assert_eq!(actual, faces(&big.mesh));
        for z in 0..128 {
            assert_eq!(
                a.columns.heights.height(0, z),
                b.columns.heights.height(0, z)
            );
        }
        assert_eq!(
            a.ranges.last().unwrap().end as usize,
            a.columns.mesh.indices.len()
        );
        assert_eq!(a.draw_ranges([24., 0.]).len(), 4);
    }
    #[test]
    fn resident_bytes_report_actual_compacted_mesh_capacity() {
        let world = World::fbm(7, Default::default()).unwrap();
        let chunk = NativeChunk::build(&world, ChunkKey { x: -1, z: 0 }).unwrap();
        let mesh = &chunk.columns.mesh;
        assert_eq!(mesh.vertices.capacity(), mesh.vertices.len());
        assert_eq!(mesh.indices.capacity(), mesh.indices.len());
        assert_eq!(
            chunk.bytes,
            mesh.vertices.capacity() * std::mem::size_of::<Vertex>()
                + mesh.indices.capacity() * std::mem::size_of::<u32>()
                + chunk.columns.heights.allocated_bytes()
        );
        let expected = chunk.bytes;
        let mut cache = Cache::new();
        cache.insert(Arc::new(chunk), &[]).unwrap();
        assert_eq!(cache.bytes, expected);
    }
    #[test]
    fn byte_budget_rejects_oversize_and_preserves_protected_content() {
        let w = flat();
        let mut cache = Cache::new();
        let mut a = NativeChunk::build(&w, ChunkKey { x: 0, z: 0 }).unwrap();
        a.bytes = CACHE_BYTES;
        cache.insert(Arc::new(a), &[]).unwrap();
        let b = Arc::new(NativeChunk::build(&w, ChunkKey { x: 1, z: 0 }).unwrap());
        assert!(cache.insert(b.clone(), &[ChunkKey { x: 0, z: 0 }]).is_err());
        assert_eq!(cache.bytes, CACHE_BYTES);
        cache.insert(b, &[]).unwrap();
        assert_eq!(cache.map.len(), 1);
        assert!(cache.bytes < CACHE_BYTES);
        let mut huge = NativeChunk::build(&w, ChunkKey { x: 2, z: 0 }).unwrap();
        huge.bytes = CACHE_BYTES + 1;
        assert!(cache.insert(Arc::new(huge), &[]).is_err());
        assert_eq!(cache.map.len(), 1);
        assert!(NativeStreamer::new(World::new(0)).is_err());
    }
}
