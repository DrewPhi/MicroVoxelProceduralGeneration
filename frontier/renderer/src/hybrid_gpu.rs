//! Moving heightfield ownership: a coherent replacement uploads incrementally
//! while its predecessor remains drawable. This is not a volume handoff.
use crate::{
    hybrid, mesh,
    native_stream::{ChunkKey, NativeCoverage, NativeStreamer, CHUNK_METERS, CORE_METERS},
    terrain, Camera,
};
use frontier_runtime::travel::{TeleportGate, TravelPredictor};
use glam::{DVec3, Vec3, Vec4};
use std::{
    collections::{HashMap, VecDeque},
    sync::{mpsc, OnceLock},
    time::Instant,
};
use wgpu::util::DeviceExt;

const CHUNK_UPLOAD_BUDGET: u64 = 8 * 1024 * 1024;
pub const GPU_HARD_BYTES: u64 = 1024 * 1024 * 1024;
fn aligned_upload_length(remaining: u64, budget: u64) -> u64 {
    remaining.min(budget) & !3
}
fn coverage_ready(
    center: [f64; 2],
    desired: [f64; 2],
    chunks: impl IntoIterator<Item = bool>,
) -> bool {
    center == desired && chunks.into_iter().all(|ready| ready)
}

type Garbage = Box<dyn Send>;
/// One bounded destruction worker. Deferred owners stop progressing when its
/// queue is full, keeping large CPU allocation frees off the render thread.
struct Recycler {
    sender: mpsc::SyncSender<Garbage>,
    deferred: VecDeque<Garbage>,
}
impl Recycler {
    fn new() -> Self {
        static SENDER: OnceLock<mpsc::SyncSender<Garbage>> = OnceLock::new();
        let sender = SENDER
            .get_or_init(|| {
                let (tx, rx) = mpsc::sync_channel::<Garbage>(16);
                std::thread::Builder::new()
                    .name("hybrid-recycler".into())
                    .spawn(move || {
                        while let Ok(item) = rx.recv() {
                            drop(item)
                        }
                    })
                    .expect("start recycler");
                tx
            })
            .clone();
        Self {
            sender,
            deferred: VecDeque::new(),
        }
    }
    fn flush(&mut self) -> bool {
        while let Some(item) = self.deferred.pop_front() {
            if let Err(error) = self.sender.try_send(item) {
                self.deferred.push_front(match error {
                    mpsc::TrySendError::Full(item) | mpsc::TrySendError::Disconnected(item) => item,
                });
                break;
            }
        }
        self.deferred.len() < 4
    }
    fn retire(&mut self, item: impl Send + 'static) {
        self.deferred.push_back(Box::new(item));
        self.flush();
    }
}
pub struct Prepared {
    pub native: NativeCoverage,
    pub terrain: terrain::TerrainUpdate,
    pub collar: mesh::Mesh,
}
pub struct Stream {
    source: frontier_world::FbmTerrain,
    pub native: NativeStreamer,
    pub terrain: terrain::TerrainStreamer,
    native_ready: Option<NativeCoverage>,
    terrain_ready: Option<terrain::TerrainUpdate>,
    joining: Option<mpsc::Receiver<Result<Prepared, String>>>,
    pub ready: Option<Prepared>,
    requested: bool,
    recycler: Recycler,
    predictor: TravelPredictor,
    clock: Instant,
    selection_started: Instant,
    awaiting_publication: bool,
    published_center: Option<[f64; 2]>,
    teleport: TeleportGate,
}
impl Stream {
    pub fn new(world: frontier_world::World, levels: u32) -> Result<Self, String> {
        Ok(Self {
            source: world.fbm_terrain().unwrap().clone(),
            terrain: terrain::TerrainStreamer::with_levels(
                world.fbm_terrain().unwrap().clone(),
                levels,
            )?,
            native: NativeStreamer::new(world)?,
            native_ready: None,
            terrain_ready: None,
            joining: None,
            ready: None,
            requested: false,
            recycler: Recycler::new(),
            predictor: TravelPredictor::default(),
            clock: Instant::now(),
            selection_started: Instant::now(),
            awaiting_publication: false,
            published_center: None,
            teleport: TeleportGate::default(),
        })
    }
    pub fn discard(&mut self, prepared: Prepared) {
        self.recycler.retire(prepared);
    }
    /// Call only after the complete native/collar/proxy selection reached GPU
    /// publication (including initial construction), never after CPU readiness.
    pub fn published(&mut self, center: [f64; 2]) {
        if center != self.native.desired_center() {
            return;
        }
        if self.awaiting_publication
            && self.published_center.is_some()
            && self.teleport.destination().is_none()
        {
            self.predictor
                .record_latency(self.selection_started.elapsed().as_secs_f64());
        }
        self.published_center = Some(center);
        self.awaiting_publication = false;
        self.teleport.published(center);
    }
    /// Supersedes all ordinary travel immediately; caller retains the presented
    /// camera until teleport_ready(id), then calls finish_teleport(id).
    pub fn request_teleport(&mut self, destination: [f64; 2]) -> Result<u64, String> {
        if destination.iter().any(|v| !v.is_finite() || v.abs() > 1e12) {
            return Err("teleport destination outside supported coordinate range".into());
        }
        let center = destination.map(|v| (v / 8.).round() * 8.);
        let id = self.teleport.request(center);
        self.predictor.reset_motion();
        self.native.request(center);
        self.requested = true;
        self.selection_started = Instant::now();
        self.awaiting_publication = true;
        if self.published_center == Some(center) {
            self.teleport.published(center);
            self.awaiting_publication = false;
        }
        Ok(id)
    }
    pub fn teleport_ready(&self, id: u64) -> bool {
        self.teleport.is_ready(id)
    }
    pub fn finish_teleport(&mut self, id: u64) -> bool {
        self.teleport.finish(id)
    }
    pub fn prediction_horizon_seconds(&self) -> f64 {
        self.predictor.horizon_seconds()
    }
    pub fn poll(&mut self, position: [f64; 2], moving: bool) -> Result<(), String> {
        self.predictor
            .observe(position, self.clock.elapsed().as_secs_f64());
        if self.teleport.destination().is_none() && (moving || !self.requested) {
            let desired = self.native.desired_center();
            // A selected coherent cover is accepted demand until publication,
            // rather than an obsolete visibility snapshot. Abandon it if a turn
            // or jump takes the camera beyond its safe interior (one chunk guard).
            let guard = CORE_METERS * 0.5 - CHUNK_METERS;
            let safe = (0..2).all(|i| (position[i] - desired[i]).abs() <= guard);
            if !self.awaiting_publication || !safe {
                if (0..2).any(|i| (position[i] - desired[i]).abs() > CORE_METERS) {
                    self.predictor.reset_motion();
                }
                let mut target = self.predictor.target(position);
                // Prediction has no fixed distance cap. A 64m native square can
                // only present a target with the current camera in its interior;
                // constrain this selection by that geometric coverage contract.
                for i in 0..2 {
                    let min = ((position[i] - guard) / CHUNK_METERS).ceil() * CHUNK_METERS;
                    let max = ((position[i] + guard) / CHUNK_METERS).floor() * CHUNK_METERS;
                    target[i] = target[i].clamp(min, max);
                }
                let old = self.native.desired_center();
                self.native.request(target);
                if !self.requested || self.native.desired_center() != old {
                    self.selection_started = Instant::now();
                    self.awaiting_publication = true;
                }
                self.requested = true;
            }
        }
        if let Some(error) = self.native.take_error() {
            return Err(error);
        }
        if !self.recycler.flush() {
            return Ok(());
        }
        let desired = self.native.desired_center();
        self.terrain.request_exact(desired);
        if let Some(n) = self.native.take_ready() {
            let old = self.native_ready.replace(n);
            self.recycler.retire(old);
        }
        if let Some(t) = self.terrain.poll() {
            let old = self.terrain_ready.replace(t);
            self.recycler.retire(old);
        }
        if let Some(rx) = &self.joining {
            match rx.try_recv() {
                Ok(result) => {
                    let ready = result?;
                    self.joining = None;
                    if ready.native.center == desired
                        && ready.native.revision == self.native.desired_revision()
                    {
                        self.ready = Some(ready)
                    } else {
                        self.recycler.retire(ready)
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("collar worker disconnected".into())
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.ready.as_ref().is_some_and(|p| {
            p.native.center != desired || p.native.revision != self.native.desired_revision()
        }) {
            let old = self.ready.take();
            self.recycler.retire(old);
        }
        if self.joining.is_none()
            && self.ready.is_none()
            && self
                .native_ready
                .as_ref()
                .zip(self.terrain_ready.as_ref())
                .is_some_and(|(n, t)| {
                    n.center == desired
                        && n.revision == self.native.desired_revision()
                        && n.center == t.center
                })
        {
            let native = self.native_ready.take().unwrap();
            let terrain = self.terrain_ready.take().unwrap();
            let source = self.source.clone();
            let (tx, rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("hybrid-collar".into())
                .spawn(move || {
                    let result =
                        hybrid::build_collar(&source, native.center, &terrain.data, |x, z| {
                            native.height(x, z)
                        })
                        .map(|collar| Prepared {
                            native,
                            terrain,
                            collar,
                        });
                    let _ = tx.send(result);
                })
                .map_err(|e| e.to_string())?;
            self.joining = Some(rx);
        }
        Ok(())
    }
}
struct DrawMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
    origin: DVec3,
    count: u32,
    bytes: u64,
    bounds: [Vec3; 2],
    uploaded: u64,
}
impl DrawMesh {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        mesh: &mesh::Mesh,
        immediate: bool,
        worker_bounds: Option<[[f32; 3]; 2]>,
    ) -> Self {
        let vertices = if immediate {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("native chunk vertices"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            })
        } else {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("incremental native vertices"),
                size: (mesh.vertices.len() * std::mem::size_of::<mesh::Vertex>()).max(4) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let indices = if immediate {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("native chunk indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            })
        } else {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("incremental native indices"),
                size: (mesh.indices.len() * 4).max(4) as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chunk camera"),
            size: 96,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chunk camera"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let bounds = worker_bounds
            .map(|b| [Vec3::from_array(b[0]), Vec3::from_array(b[1])])
            .unwrap_or_else(|| {
                let mut bounds = [Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)];
                for vertex in &mesh.vertices {
                    let p = Vec3::from_array(vertex.position);
                    bounds[0] = bounds[0].min(p);
                    bounds[1] = bounds[1].max(p);
                }
                bounds
            });
        Self {
            vertices,
            indices,
            uniform,
            bind,
            bounds,
            uploaded: if immediate {
                (mesh.vertices.len() * std::mem::size_of::<mesh::Vertex>() + mesh.indices.len() * 4)
                    as u64
            } else {
                0
            },
            origin: DVec3::new(
                mesh.origin.x as f64,
                mesh.origin.y as f64,
                mesh.origin.z as f64,
            ) * frontier_world::VOXEL_SIZE,
            count: mesh.indices.len() as u32,
            bytes: (mesh.vertices.len() * std::mem::size_of::<mesh::Vertex>()
                + mesh.indices.len() * 4) as u64,
        }
    }
    fn ready(&self) -> bool {
        self.uploaded == self.bytes
    }
    fn upload(&mut self, queue: &wgpu::Queue, mesh: &mesh::Mesh, budget: u64) -> u64 {
        let vertex_bytes: &[u8] = bytemuck::cast_slice(&mesh.vertices);
        let index_bytes: &[u8] = bytemuck::cast_slice(&mesh.indices);
        let mut available = budget;
        for (buffer, source, base) in [
            (&self.vertices, vertex_bytes, 0),
            (&self.indices, index_bytes, vertex_bytes.len() as u64),
        ] {
            if self.uploaded < base {
                break;
            }
            let offset = self.uploaded - base;
            if offset >= source.len() as u64 {
                continue;
            }
            let length = aligned_upload_length(source.len() as u64 - offset, available);
            if length == 0 {
                break;
            }
            queue.write_buffer(
                buffer,
                offset,
                &source[offset as usize..(offset + length) as usize],
            );
            self.uploaded += length;
            available -= length;
        }
        budget - available
    }
    fn draw<'a>(
        &'a self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        camera: &Camera,
        aspect: f32,
        ranges: impl IntoIterator<Item = std::ops::Range<u32>>,
    ) {
        if !self.ready() {
            return;
        }
        let matrix = camera.matrix(self.origin, aspect);
        let corners: [Vec4; 8] = std::array::from_fn(|i| {
            matrix
                * Vec3::new(
                    self.bounds[i & 1].x,
                    self.bounds[(i >> 1) & 1].y,
                    self.bounds[(i >> 2) & 1].z,
                )
                .extend(1.)
        });
        if (0..6).any(|plane| {
            corners.iter().all(|p| match plane {
                0 => p.x < -p.w,
                1 => p.x > p.w,
                2 => p.y < -p.w,
                3 => p.y > p.w,
                4 => p.z < 0.,
                _ => p.z > p.w,
            })
        }) {
            return;
        }
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::cast_slice(&matrix.to_cols_array()),
        );
        queue.write_buffer(
            &self.uniform,
            64,
            bytemuck::cast_slice(
                &(camera.position - self.origin)
                    .as_vec3()
                    .extend(1.)
                    .to_array(),
            ),
        );
        let paint_origin = [
            (self.origin.x * 16.) as i64 as u32,
            (self.origin.y * 16.) as i64 as u32,
            (self.origin.z * 16.) as i64 as u32,
            0u32,
        ];
        queue.write_buffer(&self.uniform, 80, bytemuck::cast_slice(&paint_origin));
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        for range in ranges {
            pass.draw_indexed(range, 0, 0..1);
        }
    }
}
pub struct HybridGpu {
    cache: HashMap<ChunkKey, DrawMesh>,
    lru: VecDeque<ChunkKey>,
    active: Option<NativeCoverage>,
    pending: Option<Prepared>,
    collar: Option<DrawMesh>,
    recycler: Recycler,
    pub uploads: u64,
    pub upload_bytes: u64,
    pub upload_ms: f64,
    pub publications: u64,
    pub last_publish_ms: f64,
    pub peak_bytes: u64,
    /// Budget violations; partial uploads keep this at zero.
    pub oversized_uploads: u64,
    pub max_frame_upload_bytes: u64,
    pub stale_rejections: u64,
    pub cache_hits: u64,
    pending_started: Instant,
}
impl HybridGpu {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            lru: VecDeque::new(),
            active: None,
            pending: None,
            collar: None,
            recycler: Recycler::new(),
            uploads: 0,
            upload_bytes: 0,
            upload_ms: 0.,
            publications: 0,
            last_publish_ms: 0.,
            peak_bytes: 0,
            oversized_uploads: 0,
            max_frame_upload_bytes: 0,
            stale_rejections: 0,
            cache_hits: 0,
            pending_started: Instant::now(),
        }
    }
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn stage(&mut self, prepared: Prepared) -> Result<(), String> {
        let active_bytes = self.active.as_ref().map_or(0, |a| {
            a.chunks
                .iter()
                .filter_map(|(key, _)| self.cache.get(key))
                .map(|m| m.bytes)
                .sum::<u64>()
        }) + self.collar.as_ref().map_or(0, |m| m.bytes);
        let incoming_bytes = prepared
            .native
            .chunks
            .iter()
            .filter(|(key, _)| {
                !self
                    .active
                    .as_ref()
                    .is_some_and(|a| a.chunks.iter().any(|(k, _)| k == key))
            })
            .map(|(_, c)| {
                (c.columns.mesh.vertices.len() * std::mem::size_of::<mesh::Vertex>()
                    + c.columns.mesh.indices.len() * 4) as u64
            })
            .sum::<u64>();
        let collar_bytes = (prepared.collar.vertices.len() * std::mem::size_of::<mesh::Vertex>()
            + prepared.collar.indices.len() * 4) as u64;
        if active_bytes + incoming_bytes + collar_bytes > GPU_HARD_BYTES {
            self.recycler.retire(prepared);
            return Err("native replacement exceeds the 1GiB protected transition budget; old cover retained".into());
        }

        self.cache_hits += prepared
            .native
            .chunks
            .iter()
            .filter(|(k, _)| self.cache.get(k).is_some_and(DrawMesh::ready))
            .count() as u64;
        debug_assert!(self.pending.is_none());
        self.pending = Some(prepared);
        self.pending_started = Instant::now();
        self.evict();
        Ok(())
    }
    pub fn center(&self) -> [f64; 2] {
        self.active.as_ref().map_or([0., 0.], |a| a.center)
    }
    pub fn revision(&self) -> u64 {
        self.active.as_ref().map_or(0, |a| a.revision)
    }
    pub fn bytes(&self) -> u64 {
        self.cache.values().map(|m| m.bytes).sum::<u64>()
            + self.collar.as_ref().map_or(0, |m| m.bytes)
    }
    pub fn triangles(&self) -> u32 {
        self.active.as_ref().map_or(0, |a| {
            a.chunks
                .iter()
                .map(|(_, c)| {
                    c.draw_ranges(a.center)
                        .iter()
                        .map(|r| r.end - r.start)
                        .sum::<u32>()
                })
                .sum::<u32>()
                / 3
        }) + self.collar.as_ref().map_or(0, |c| c.count / 3)
    }
    pub fn queued(&self) -> usize {
        self.pending.as_ref().map_or(0, |p| {
            p.native
                .chunks
                .iter()
                .filter(|(k, _)| !self.cache.get(k).is_some_and(DrawMesh::ready))
                .count()
        })
    }
    fn touch(&mut self, key: ChunkKey) {
        self.lru.retain(|k| *k != key);
        self.lru.push_back(key);
    }
    fn evict(&mut self) {
        let mut remaining = self.lru.len();
        while remaining > 0 && (self.cache.len() > 128 || self.bytes() > 512 * 1024 * 1024) {
            remaining -= 1;
            let Some(key) = self.lru.pop_front() else {
                break;
            };
            let protected = self
                .active
                .as_ref()
                .is_some_and(|a| a.chunks.iter().any(|(k, _)| *k == key))
                || self
                    .pending
                    .as_ref()
                    .is_some_and(|a| a.native.chunks.iter().any(|(k, _)| *k == key));
            if protected {
                self.lru.push_back(key)
            } else {
                self.cache.remove(&key);
            }
        }
    }
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        terrain: &mut terrain::TerrainGpu,
        desired: [f64; 2],
        desired_revision: u64,
    ) -> bool {
        if !self.recycler.flush() {
            return false;
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.native.center != desired || p.native.revision != desired_revision)
        {
            let stale = self.pending.take();
            self.recycler.retire(stale);
            self.stale_rejections += 1;
            self.evict();
            return false;
        }
        let Some(pending) = self.pending.as_ref() else {
            return false;
        };
        let started = Instant::now();
        let mut bytes = 0;
        let mut completed = 0;
        let mut attempted = 0;
        let mut touched = Vec::new();
        for (key, chunk) in &pending.native.chunks {
            if self.cache.get(key).is_some_and(DrawMesh::ready) {
                touched.push(*key);
                continue;
            }
            if bytes >= CHUNK_UPLOAD_BUDGET || attempted >= 2 {
                break;
            }
            let mesh = &chunk.columns.mesh;
            let allocation = (mesh.vertices.len() * std::mem::size_of::<mesh::Vertex>()
                + mesh.indices.len() * 4) as u64;
            if !self.cache.contains_key(key) && self.bytes() + allocation > GPU_HARD_BYTES {
                let active = &self.active;
                self.cache.retain(|key, _| {
                    active
                        .as_ref()
                        .is_some_and(|a| a.chunks.iter().any(|(k, _)| k == key))
                        || pending.native.chunks.iter().any(|(k, _)| k == key)
                });
                self.lru.retain(|key| self.cache.contains_key(key));
            }
            let gpu = self
                .cache
                .entry(*key)
                .or_insert_with(|| DrawMesh::new(device, layout, mesh, false, Some(chunk.bounds)));
            bytes += gpu.upload(queue, mesh, CHUNK_UPLOAD_BUDGET - bytes);
            attempted += 1;
            if gpu.ready() {
                completed += 1
            }
            touched.push(*key);
        }
        let complete = coverage_ready(
            pending.native.center,
            desired,
            pending
                .native
                .chunks
                .iter()
                .map(|(key, _)| self.cache.get(key).is_some_and(DrawMesh::ready)),
        );
        for key in touched {
            self.touch(key)
        }
        self.evict();
        self.peak_bytes = self.peak_bytes.max(self.bytes());
        self.max_frame_upload_bytes = self.max_frame_upload_bytes.max(bytes);
        self.uploads += completed;
        self.upload_bytes += bytes;
        self.upload_ms += started.elapsed().as_secs_f64() * 1000.;
        if !complete {
            return false;
        }
        let pending = self.pending.take().unwrap();
        self.collar = Some(DrawMesh::new(device, layout, &pending.collar, true, None));
        terrain.upload(queue, &pending.terrain);
        terrain.set_hybrid_cutout(Some(pending.native.center));
        let old = self.active.replace(pending.native);
        self.recycler.retire((old, pending.terrain, pending.collar));
        self.publications += 1;
        self.last_publish_ms = self.pending_started.elapsed().as_secs_f64() * 1000.;
        self.evict();
        true
    }
    pub fn draw<'a>(
        &'a self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        pipeline: &'a wgpu::RenderPipeline,
        camera: &Camera,
        aspect: f32,
    ) {
        pass.set_pipeline(pipeline);
        if let Some(active) = &self.active {
            for (key, chunk) in &active.chunks {
                if let Some(gpu) = self.cache.get(key) {
                    gpu.draw(
                        queue,
                        pass,
                        camera,
                        aspect,
                        chunk.draw_ranges(active.center),
                    );
                }
            }
        }
        if let Some(collar) = &self.collar {
            collar.draw(queue, pass, camera, aspect, [0..collar.count]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn flat_stream() -> Stream {
        Stream::new(
            frontier_world::World::fbm(
                7,
                frontier_world::FbmParams {
                    amplitude: 0.,
                    octaves: 1,
                    ..Default::default()
                },
            )
            .unwrap(),
            terrain::MIN_LEVELS,
        )
        .unwrap()
    }
    fn prepared(stream: &mut Stream, position: [f64; 2]) -> Prepared {
        let start = Instant::now();
        loop {
            stream.poll(position, true).unwrap();
            if let Some(ready) = stream.ready.take() {
                return ready;
            }
            assert!(
                start.elapsed().as_secs() < 20,
                "coherent preparation timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    #[test]
    fn cold_warm_changed_teleport_waits_for_complete_gpu_publication() {
        let mut stream = flat_stream();
        let first = stream.request_teleport([800., 0.]).unwrap();
        let latest = stream.request_teleport([-800., 0.]).unwrap();
        let ready = prepared(&mut stream, [0., 0.]);
        assert_eq!(ready.native.center, [-800., 0.]);
        assert_eq!(ready.native.revision, stream.native.desired_revision());
        assert!(!stream.teleport_ready(first));
        assert!(!stream.teleport_ready(latest)); // CPU/collar complete is insufficient.
        stream.published(ready.native.center);
        assert!(stream.teleport_ready(latest));
        assert!(stream.finish_teleport(latest));
        let jobs = stream.native.metrics().jobs;
        let warm = stream.request_teleport([-800., 0.]).unwrap();
        assert!(stream.teleport_ready(warm));
        assert_eq!(stream.native.metrics().jobs, jobs);
        assert!(stream.finish_teleport(warm));
    }
    #[test]
    fn nearby_motion_finishes_accepted_target_and_far_turn_supersedes_it() {
        let mut stream = flat_stream();
        stream.poll([0., 0.], true).unwrap();
        let initial = stream.native.desired_revision();
        stream.poll([16., 0.], true).unwrap();
        assert_eq!(stream.native.desired_revision(), initial);
        let ready = prepared(&mut stream, [16., 0.]);
        assert_eq!(ready.native.center, [0., 0.]);
        stream.published(ready.native.center);
        stream.poll([16., 0.], true).unwrap();
        assert!(stream.native.desired_revision() > initial);
        stream.poll([-800., 0.], true).unwrap();
        let latest = stream.native.desired_revision();
        let ready = prepared(&mut stream, [-800., 0.]);
        assert_eq!(ready.native.revision, latest);
        assert_eq!(ready.native.center, [-800., 0.]);
        stream.published(ready.native.center);
        stream.poll([-800., 0.], true).unwrap(); // stop does not create fresh content.
        assert_eq!(stream.native.desired_revision(), latest);
        stream.poll([-784., 0.], true).unwrap(); // resume requests entering coverage.
        assert!(stream.native.desired_revision() > latest);
    }
    #[test]
    fn steep_chunk_uploads_are_aligned_and_strictly_bounded_across_frames() {
        for total in [
            0,
            4,
            CHUNK_UPLOAD_BUDGET - 4,
            CHUNK_UPLOAD_BUDGET,
            CHUNK_UPLOAD_BUDGET + 4,
            10_600_004,
            27_000_000,
        ] {
            let mut remaining = total;
            let mut copied = 0;
            while remaining > 0 {
                let amount = aligned_upload_length(remaining, CHUNK_UPLOAD_BUDGET);
                assert!(amount > 0 && amount <= CHUNK_UPLOAD_BUDGET && amount % 4 == 0);
                remaining -= amount;
                copied += amount;
            }
            assert_eq!(copied, total);
        }
        assert_eq!(aligned_upload_length(100, 7), 4);
    }
    #[test]
    fn only_complete_latest_cover_can_replace_previous_ownership() {
        let center = [8., -16.];
        let mut chunks = [true; 64];
        chunks[63] = false;
        assert!(!coverage_ready(center, center, chunks));
        chunks[63] = true;
        assert!(coverage_ready(center, center, chunks));
        assert!(!coverage_ready(center, [512., 1024.], chunks));
        // A cached completed cover is reusable on return, even after a newer
        // view epoch, but cannot publish into the intervening destination.
        assert!(coverage_ready(center, center, chunks));
    }
}
