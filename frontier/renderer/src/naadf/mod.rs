//! Optional exact NAADF window. Leading strips are generated and uploaded
//! asynchronously; the existing renderer covers the exterior and handoff.
mod data;
pub(crate) mod ownership;
mod stream;
#[cfg(test)]
mod tests;
mod upload;
use super::{Camera, Gpu};
use bytemuck::{Pod, Zeroable};
use data::Snapshot;
use frontier_world::{World, VOXEL_SIZE};
use glam::DVec3;
use std::{
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Instant,
};
use wgpu::util::DeviceExt;
const SHADER: &str = concat!(
    "const NAADF_GROUP:u32=0u;const NAADF_BASE:u32=0u;\n",
    include_str!("trace.wgsl"),
    include_str!("ray.wgsl")
);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Request {
    radius: u32,
    budget: usize,
    revision: u64,
    vegetation: bool,
    serial: u64,
    samples: u32,
    follow: bool,
}

// The user ceiling can tighten the estimate, never bypass live RAM headroom.
fn build_budget_limit(requested: usize, available: usize) -> usize {
    requested.min(available)
}

fn memory_headroom(memory: crate::settings::MemorySnapshot) -> Option<usize> {
    memory.measured.then(|| {
        memory
            .available_mib
            .saturating_sub((memory.total_mib / 10).max(1024))
            .saturating_mul(1048576)
    })
}

enum Job {
    Build(u64, Request, World, [f64; 3]),
    Install(u64, Snapshot, [usize; 3]),
    Shift(u64, World, stream::Region, usize),
    Finish(stream::Change, bool),
    Reset,
}
enum Reply {
    Built(Snapshot),
    Shifted(stream::Change),
}
struct Worker {
    tx: Option<mpsc::Sender<Job>>,
    rx: mpsc::Receiver<(u64, Result<Reply, String>)>,
    epoch: Arc<AtomicU64>,
    stage: Arc<AtomicU32>,
    handle: Option<thread::JoinHandle<()>>,
}
impl Worker {
    fn new() -> Self {
        let (tx, input) = mpsc::channel::<Job>();
        let (output, rx) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(0));
        let stage = Arc::new(AtomicU32::new(0));
        let e = epoch.clone();
        let s = stage.clone();
        let handle = thread::Builder::new()
            .name("naadf-build".into())
            .spawn(move || {
                let mut state: Option<stream::State> = None;
                while let Ok(job) = input.recv() {
                    match job {
                        Job::Build(g, r, w, eye) => {
                            if e.load(Ordering::Relaxed) != g {
                                continue;
                            }
                            state = None;
                            let result = data::build(&w, eye, r.radius, r.budget, &s, || {
                                e.load(Ordering::Relaxed) != g
                            })
                            .map(Reply::Built);
                            if e.load(Ordering::Relaxed) == g && output.send((g, result)).is_err() {
                                break;
                            }
                        }
                        Job::Install(g, s, c) => {
                            if e.load(Ordering::Relaxed) == g {
                                state = Some(stream::State::new(s, c));
                            }
                        }
                        Job::Shift(g, w, r, budget) => {
                            if e.load(Ordering::Relaxed) != g {
                                continue;
                            }
                            let result = state
                                .as_mut()
                                .ok_or_else(|| "NAADF streaming state unavailable".to_string())
                                .and_then(|s| {
                                    s.prepare(&w, r, budget, &|| e.load(Ordering::Relaxed) != g)
                                });
                            if e.load(Ordering::Relaxed) != g {
                                if let (Some(s), Ok(c)) = (&mut state, result) {
                                    s.finish(c, false);
                                }
                                continue;
                            }
                            if output.send((g, result.map(Reply::Shifted))).is_err() {
                                break;
                            }
                        }
                        Job::Finish(c, commit) => {
                            if let Some(s) = &mut state {
                                s.finish(c, commit);
                            }
                        }
                        Job::Reset => state = None,
                    }
                }
            })
            .expect("NAADF worker");
        Self {
            tx: Some(tx),
            rx,
            epoch,
            stage,
            handle: Some(handle),
        }
    }
    fn send(&self, job: Job) {
        let _ = self.tx.as_ref().unwrap().send(job);
    }
    fn cancel(&self) {
        self.epoch.fetch_add(1, Ordering::Relaxed);
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        self.tx.take();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    matrix: [[f32; 4]; 4],
    inverse: [[f32; 4]; 4],
    eye: [f32; 4],
    extent: [u32; 4],
    viewport: [f32; 4],
    sun: [f32; 4],
    sky: [f32; 4],
    far_matrix: [[f32; 4]; 4],
    shadow: [f32; 4],
    forward: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    projection: [f32; 4],
    ring: [u32; 4],
}
struct Volume {
    resources: Bindings,
    ownership: std::sync::Arc<ownership::Ownership>,
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    _chunks: wgpu::Texture,
    blocks: wgpu::Buffer,
    voxels: wgpu::Buffer,
    palette: wgpu::Buffer,
    bounds: [[f64; 3]; 2],
    ring: [u32; 3],
    capacities: [usize; 3],
    n: u32,
    bytes: usize,
    samples: u32,
    revision: u64,
    stats: [usize; 5],
    build_ms: f64,
}
struct Pending {
    volume: Volume,
    snapshot: Snapshot,
    phase: u8,
    offset: usize,
    upload_ms: f64,
}
impl Pending {
    fn new(gpu: &Gpu, snapshot: Snapshot) -> Result<Self, String> {
        let limits = gpu.device.limits();
        let n = snapshot.chunks_per_edge;
        if n > limits.max_texture_dimension_3d {
            return Err("NAADF region exceeds device 3D texture size".into());
        }
        for (name, bytes) in [
            ("blocks", snapshot.blocks.len().max(1) * 4),
            ("voxels", snapshot.voxels.len().max(1) * 4),
            ("palette", snapshot.palette.len() * 16),
        ] {
            if bytes > limits.max_storage_buffer_binding_size as usize
                || bytes > limits.max_buffer_size as usize
            {
                return Err(format!(
                    "NAADF {name} needs {} MiB; device binding limit is {} MiB",
                    bytes.div_ceil(1048576),
                    limits.max_storage_buffer_binding_size / 1048576
                ));
            }
        }
        let follow = gpu.visual_settings.as_ref().is_some_and(|s| s.naadf_follow);
        let max = limits
            .max_buffer_size
            .min(limits.max_storage_buffer_binding_size as u64) as usize;
        let grow = |used: usize, extra: usize| {
            if follow {
                (used + used / 2)
                    .max(used + extra)
                    .min(max & !255)
                    .max(used)
            } else {
                used
            }
        };
        let capacities = [
            grow(snapshot.blocks.len() * 4, 2 * 1048576),
            grow(snapshot.voxels.len() * 4, 4 * 1048576),
            grow(snapshot.palette.len() * 16, 2 * 1048576),
        ];
        // Snapshot CPU data is still live at this point. Check the actual
        // arena capacities, including follow-mode slack, before allocating.
        let upload_bytes = (n as usize).pow(3) * 4 + capacities.iter().sum::<usize>();
        if let Some(headroom) = memory_headroom(crate::settings::MemorySnapshot::detect()) {
            if upload_bytes > headroom {
                return Err(format!(
                    "NAADF upload needs {} MiB; {} MiB available with system headroom. Try a smaller radius.",
                    upload_bytes.div_ceil(1048576),
                    headroom / 1048576,
                ));
            }
        }
        let chunks = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("NAADF exact chunk words"),
            size: wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R32Uint,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let buffer = |bytes: usize, label: &'static str| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes.max(4) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let blocks = buffer(capacities[0], "NAADF block words");
        let voxels = buffer(capacities[1], "NAADF voxel words");
        let palette = buffer(capacities[2], "NAADF exact plant pigments");
        let uniform = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("NAADF camera"),
                contents: bytemuck::bytes_of(&Uniform::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let layout = layout(&gpu.device);
        let view = chunks.create_view(&Default::default());
        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("NAADF snapshot"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: blocks.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: voxels.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: palette.as_entire_binding(),
                },
            ],
        });
        let pipeline = make_pipeline(gpu, &layout);
        let resources = Bindings {
            uniform: uniform.clone(),
            chunks: view.clone(),
            blocks: blocks.clone(),
            voxels: voxels.clone(),
            palette: palette.clone(),
        };
        let volume = Volume {
            resources,
            ownership: snapshot.ownership.clone(),
            pipeline,
            bind,
            uniform,
            _chunks: chunks,
            blocks,
            voxels,
            palette,
            bounds: snapshot.bounds(),
            ring: [0; 3],
            capacities,
            n,
            bytes: snapshot.bytes(),
            samples: gpu.sample_count,
            revision: snapshot.source_revision,
            stats: [
                snapshot.mixed_chunks,
                snapshot.unique_blocks,
                snapshot.sampled_columns,
                snapshot.plant_cells,
                snapshot.palette.len(),
            ],
            build_ms: snapshot.build_ms,
        };
        Ok(Self {
            volume,
            snapshot,
            phase: 0,
            offset: 0,
            upload_ms: 0.,
        })
    }
    fn advance(&mut self, queue: &wgpu::Queue) -> bool {
        if self.phase >= 4 {
            return true;
        }
        let start = Instant::now();
        let allowance = 8 * 1048576;
        if self.phase == 0 {
            let n = self.volume.n as usize;
            let plane = n * n * 4;
            let layers = (allowance / plane).max(1).min(n - self.offset);
            let bytes = bytemuck::cast_slice::<_, u8>(&self.snapshot.chunks);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.volume._chunks,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: self.offset as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes[self.offset * plane..(self.offset + layers) * plane],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(n as u32 * 4),
                    rows_per_image: Some(n as u32),
                },
                wgpu::Extent3d {
                    width: n as u32,
                    height: n as u32,
                    depth_or_array_layers: layers as u32,
                },
            );
            self.offset += layers;
            if self.offset == n {
                self.phase = 1;
                self.offset = 0;
            }
        } else {
            let (buffer, bytes) = match self.phase {
                1 => (
                    &self.volume.blocks,
                    bytemuck::cast_slice::<_, u8>(&self.snapshot.blocks),
                ),
                2 => (
                    &self.volume.voxels,
                    bytemuck::cast_slice::<_, u8>(&self.snapshot.voxels),
                ),
                _ => (
                    &self.volume.palette,
                    bytemuck::cast_slice::<_, u8>(&self.snapshot.palette),
                ),
            };
            let count = allowance.min(bytes.len() - self.offset);
            if count > 0 {
                queue.write_buffer(
                    buffer,
                    self.offset as u64,
                    &bytes[self.offset..self.offset + count],
                );
            }
            self.offset += count;
            if self.offset == bytes.len() {
                self.phase += 1;
                self.offset = 0;
            }
        }
        self.upload_ms += start.elapsed().as_secs_f64() * 1000.;
        self.phase >= 4
    }
}
pub struct Experiment {
    restoring_raster: bool,
    effective_budget_mib: usize,
    worker: Worker,
    request: Option<Request>,
    epoch: u64,
    active: Option<Volume>,
    pending: Option<Pending>,
    shift: Option<upload::PendingShift>,
    shifting: bool,
    shifts: u64,
    strip_ms: f64,
    strip_columns: usize,
    camera_margin_m: f64,
    min_camera_margin_m: Option<f64>,
    stream_active_frames: u64,
    stream_outside_frames: u64,
    pub status: String,
    pub stage: u32,
    pub upload_ms: f64,
    pub refused: u64,
    published_epoch: u64,
    pub error: Option<String>,
}
impl Default for Experiment {
    fn default() -> Self {
        Self {
            restoring_raster: false,
            effective_budget_mib: 0,
            worker: Worker::new(),
            request: None,
            epoch: 0,
            active: None,
            pending: None,
            shift: None,
            shifting: false,
            shifts: 0,
            strip_ms: 0.,
            strip_columns: 0,
            camera_margin_m: 0.,
            min_camera_margin_m: None,
            stream_active_frames: 0,
            stream_outside_frames: 0,
            status: "Off".into(),
            stage: 0,
            upload_ms: 0.,
            refused: 0,
            published_epoch: 0,
            error: None,
        }
    }
}
impl Experiment {
    pub fn poll(
        &mut self,
        gpu: &Gpu,
        world: &World,
        camera: Camera,
        serial: u64,
        pressure: bool,
        raster_restored: bool,
    ) {
        let Some(s) = &gpu.visual_settings else {
            return;
        };
        for volume in self
            .active
            .iter_mut()
            .chain(self.pending.iter_mut().map(|p| &mut p.volume))
        {
            if volume.samples != gpu.sample_count {
                volume.pipeline = make_pipeline(gpu, &volume.pipeline.get_bind_group_layout(0));
                volume.samples = gpu.sample_count;
            }
        }
        if !s.naadf_enabled {
            if self.request.is_some() {
                self.worker.cancel();
                self.worker.send(Job::Reset);
            }
            self.shift = None;
            self.shifting = false;
            self.request = None;
            self.pending = None;
            self.restoring_raster = self.active.is_some();
            if raster_restored {
                self.active = None;
                self.restoring_raster = false;
            }
            self.status = if self.active.is_some() {
                "Restoring raster coverage before disabling"
            } else {
                "Off"
            }
            .into();
            return;
        }
        if gpu.volume_owner || gpu.adaptive.is_none() {
            self.status = "NAADF experiment currently requires the adaptive terrain scene".into();
            self.error = Some(self.status.clone());
            return;
        }
        let r = Request {
            radius: s.naadf_radius_m,
            budget: if s.naadf_auto_memory {
                usize::MAX
            } else {
                s.naadf_budget_mib.saturating_mul(1048576)
            },
            revision: world.revision(),
            vegetation: world.vegetation_enabled(),
            serial,
            samples: 0,
            follow: s.naadf_follow,
        };
        if self.request != Some(r) {
            self.restoring_raster = self.active.is_some();
            let source_changed = self
                .request
                .is_some_and(|old| old.vegetation != r.vegetation);
            self.worker.cancel();
            self.epoch = self.worker.epoch.load(Ordering::Relaxed);
            self.request = Some(r);
            self.pending = None;
            self.shift = None;
            self.shifting = false;
            if self
                .active
                .as_ref()
                .is_some_and(|a| a.revision != world.revision() || source_changed)
            {
                self.active = None;
            }
            self.worker.stage.store(0, Ordering::Relaxed);
            self.status = "Building exact snapshot".into();
            self.error = None;
            let headroom = memory_headroom(crate::settings::MemorySnapshot::detect())
                .unwrap_or_else(|| s.naadf_budget_mib.saturating_mul(1048576));
            let effective_budget = build_budget_limit(r.budget, headroom);
            self.effective_budget_mib = effective_budget / 1048576;
            let mut job = r;
            job.budget = effective_budget;
            self.worker.send(Job::Build(
                self.epoch,
                job,
                world.clone(),
                camera.position.to_array(),
            ));
        }
        if pressure {
            if self.active.is_some() && self.published_epoch == self.epoch {
                if self.shifting {
                    self.worker.cancel();
                    self.epoch = self.worker.epoch.load(Ordering::Relaxed);
                    self.published_epoch = self.epoch;
                    self.shifting = false;
                    if let Some(p) = self.shift.take() {
                        self.worker.send(Job::Finish(p.change, false));
                    }
                }
                while let Ok((_, result)) = self.worker.rx.try_recv() {
                    if let Ok(Reply::Shifted(c)) = result {
                        self.worker.send(Job::Finish(c, false));
                    }
                }
                self.status = "NAADF streaming paused: low available RAM".into();
            } else {
                self.worker.cancel();
                self.pending = None;
                self.status =
                    "NAADF build cancelled: low available RAM; rebuild here to retry".into();
                self.error = Some(self.status.clone());
            }
            return;
        }
        self.stage = self.worker.stage.load(Ordering::Relaxed);
        while let Ok((epoch, result)) = self.worker.rx.try_recv() {
            if epoch != self.epoch {
                // A stale prepared strip may own reservations in the current
                // arena after pressure cancellation. Full rebuilds replace it.
                if self.active.is_some()
                    && self.published_epoch == self.epoch
                    && !self.restoring_raster
                {
                    if let Ok(Reply::Shifted(c)) = result {
                        self.worker.send(Job::Finish(c, false));
                    }
                }
                continue;
            }
            match result {
                Ok(Reply::Built(snapshot)) => match Pending::new(gpu, snapshot) {
                    Ok(p) => {
                        self.pending = Some(p);
                        self.status = "Uploading complete snapshot".into();
                    }
                    Err(e) => {
                        self.error = Some(e.clone());
                        self.status = e;
                        self.refused += 1;
                    }
                },
                Ok(Reply::Shifted(c)) => {
                    self.shift = Some(upload::PendingShift::new(gpu, c));
                    self.status =
                        "Uploading leading strips; restoring outgoing mesh coverage".into();
                }
                Err(e) => {
                    self.error = Some(e.clone());
                    self.status = e;
                    self.refused += 1;
                    self.shifting = false;
                }
            }
        }
        if let Some(p) = &mut self.pending {
            if p.advance(&gpu.queue) && (!self.restoring_raster || raster_restored) {
                let p = self.pending.take().unwrap();
                self.upload_ms = p.upload_ms;
                if r.follow {
                    self.worker
                        .send(Job::Install(self.epoch, p.snapshot, p.volume.capacities));
                }
                self.active = Some(p.volume);
                self.restoring_raster = false;
                self.published_epoch = self.epoch;
                self.status = if r.follow {
                    "Ready · following player"
                } else {
                    "Ready · fixed region"
                }
                .into();
            }
        }
        // A destination jump supersedes the in-flight shift before any
        // queued strip can publish for the old camera demand.
        if r.follow
            && !self.restoring_raster
            && self.published_epoch == self.epoch
            && self.active.as_ref().is_some_and(|v| {
                (0..3).any(|a| {
                    let x = camera.position[a];
                    x < v.bounds[0][a] - r.radius as f64 * 0.5
                        || x > v.bounds[1][a] + r.radius as f64 * 0.5
                })
            })
        {
            self.worker.cancel();
            self.request = None;
            self.shift = None;
            self.shifting = false;
            self.status = "Preparing NAADF at new location".into();
            return;
        }
        if let (Some(p), Some(v)) = (&mut self.shift, &mut self.active) {
            if p.advance(gpu, v) && raster_restored {
                let p = self.shift.take().unwrap();
                let c = p.publish(gpu, v);
                self.shifts += 1;
                self.strip_ms = c.build_ms;
                self.strip_columns = c.generated_columns;
                self.worker.send(Job::Finish(c, true));
                self.shifting = false;
                self.status = "Ready · following player".into();
            }
        }
        if let Some(v) = &self.active {
            let eye = camera.position.to_array();
            self.camera_margin_m = (0..3)
                .map(|a| (eye[a] - v.bounds[0][a]).min(v.bounds[1][a] - eye[a]))
                .fold(f64::INFINITY, f64::min);
            if r.follow && self.published_epoch == self.epoch {
                self.min_camera_margin_m = Some(
                    self.min_camera_margin_m
                        .map_or(self.camera_margin_m, |m| m.min(self.camera_margin_m)),
                );
                self.stream_active_frames += 1;
                self.stream_outside_frames += u64::from(self.camera_margin_m < VOXEL_SIZE);
            }
            if r.follow
                && !self.shifting
                && !self.restoring_raster
                && self.published_epoch == self.epoch
                && self.error.is_none()
            {
                let region = stream::Region {
                    lo: v.bounds[0].map(|x| x as i64),
                    hi: v.bounds[1].map(|x| x as i64),
                };
                if let Some(next) = stream::target(region, eye) {
                    self.shifting = true;
                    self.status = "Building leading strips".into();
                    self.worker.send(Job::Shift(
                        self.epoch,
                        world.clone(),
                        next,
                        self.effective_budget_mib * 1048576,
                    ));
                } else {
                    self.status = "Ready · following player".into();
                }
            }
        }
    }

    pub fn ready(&self) -> bool {
        self.active.is_some() && self.published_epoch == self.epoch && self.error.is_none()
    }
    pub fn bounds_for(&self, camera: Camera) -> Option<[[f64; 3]; 2]> {
        let a = self.active.as_ref()?;
        let _ = camera;
        Some([
            a.bounds[0].map(|v| v + VOXEL_SIZE),
            a.bounds[1].map(|v| v - VOXEL_SIZE),
        ])
    }
    pub(crate) fn desired_owner(&self) -> Option<std::sync::Arc<ownership::Ownership>> {
        if self.restoring_raster {
            None
        } else if let Some(s) = &self.shift {
            Some(s.change.retiring.clone())
        } else {
            self.active.as_ref().map(|a| a.ownership.clone())
        }
    }

    pub(crate) fn retirement(&self) -> Option<ownership::Retirement> {
        Some(ownership::Retirement {
            old: self.active.as_ref()?.bounds,
            kept: self.shift.as_ref()?.change.retiring.bounds,
        })
    }
    pub(crate) fn restore_owner_id(&self) -> Option<u64> {
        if self.restoring_raster || self.request.is_none() {
            Some(0)
        } else {
            self.shift.as_ref().map(|s| s.change.retiring.id)
        }
    }
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, far: &'a wgpu::BindGroup) {
        if let Some(a) = &self.active {
            pass.set_pipeline(&a.pipeline);
            pass.set_bind_group(0, &a.bind, &[]);
            pass.set_bind_group(1, far, &[]);
            pass.draw(0..3, 0..1);
        }
    }
    pub fn prepare(&self, gpu: &Gpu, camera: Camera, width: u32, height: u32) {
        let Some(a) = &self.active else {
            return;
        };
        let origin = DVec3::from_array(a.bounds[0]);
        let matrix = camera.matrix(origin, width as f32 / height as f32);
        let s = gpu.visual_settings.clone().unwrap_or_default();
        let sun = if s.beauty {
            crate::atmosphere::sun_direction(s.sun_elevation, s.sun_azimuth)
        } else {
            glam::Vec3::new(-0.5, 0.8, -0.3).normalize()
        };
        let basis = ray_basis(camera, width as f32 / height as f32);
        gpu.queue.write_buffer(
            &a.uniform,
            0,
            bytemuck::bytes_of(&Uniform {
                matrix: matrix.to_cols_array_2d(),
                inverse: matrix.inverse().to_cols_array_2d(),
                eye: (camera.position - origin).as_vec3().extend(0.).to_array(),
                extent: [a.n * 16, a.n * 16, a.n * 16, a.n],
                viewport: [width as f32, height as f32, a.samples as f32, 0.],
                sun: [sun.x, sun.y, sun.z, if s.beauty { 1. } else { 0. }],
                sky: [
                    s.overcast,
                    0.,
                    0.,
                    if s.visibility_benchmark { 1. } else { 0. },
                ],
                far_matrix: gpu
                    .adaptive
                    .as_ref()
                    .map_or(glam::Mat4::IDENTITY, |a| a.far_shadow_projection(&camera))
                    .to_cols_array_2d(),
                forward: basis[0],
                right: basis[1],
                up: basis[2],
                projection: basis[3],
                ring: [a.ring[0], a.ring[1], a.ring[2], 0],
                shadow: [
                    if s.terrain_shadows && !s.visibility_benchmark {
                        1.
                    } else {
                        0.
                    },
                    0.,
                    0.,
                    0.,
                ],
            }),
        );
    }
    pub(crate) fn shadow_bindings(&self) -> Option<(u64, Bindings)> {
        self.active
            .as_ref()
            .map(|a| (a.ownership.id, a.resources.clone()))
    }
    pub fn gpu_bytes(&self) -> usize {
        let allocated = |a: &Volume| (a.n as usize).pow(3) * 4 + a.capacities.iter().sum::<usize>();
        self.active.as_ref().map_or(0, allocated)
            + self.pending.as_ref().map_or(0, |p| allocated(&p.volume))
    }
    pub fn json(&self) -> String {
        let a = self.active.as_ref();
        let bounds = a.map(|a| a.bounds);
        let region = bounds.map_or("null".into(), |b| format!("{:?}", b));
        format!("{{\"status\":{:?},\"stage\":{},\"requested_radius_m\":{},\"resident_radius_m\":{},\"world_bounds_m\":{},\"native_voxel_m\":0.0625,\"requested_build_budget_mib\":{},\"gpu_payload_bytes\":{},\"build_ms\":{},\"upload_cpu_ms\":{},\"mixed_chunks\":{},\"unique_blocks\":{},\"sampled_columns\":{},\"plant_cells\":{},\"palette_entries\":{},\"color_channel_bits\":10,\"refused\":{},\"fixed_snapshot\":{},\"stream_shifts\":{},\"stream_build_ms\":{},\"stream_sampled_columns\":{},\"camera_margin_m\":{},\"min_camera_margin_m\":{},\"stream_active_frames\":{},\"stream_outside_frames\":{},\"initial_snapshot_payload_bytes\":{},\"lighting\":\"primary_visibility_lambert_without_GI\",\"includes_distant_and_boundary_raster_overhead\":true,\"interior_meshes_omitted\":true,\"supports_direct_voxel_shadows\":true,\"effective_build_budget_mib\":{},\"auto_memory\":{}}}",
            self.status,self.stage,self.request.map_or(0,|r|r.radius),a.map_or(0,|a|a.n/2),region,
            self.request.filter(|r|r.budget!=usize::MAX).map_or(0,|r|r.budget/1048576),self.gpu_bytes(),a.map_or(0.,|a|a.build_ms),self.upload_ms,
            a.map_or(0,|a|a.stats[0]),a.map_or(0,|a|a.stats[1]),a.map_or(0,|a|a.stats[2]),a.map_or(0,|a|a.stats[3]),a.map_or(0,|a|a.stats[4]),self.refused,!self.request.is_some_and(|r|r.follow),self.shifts,self.strip_ms,self.strip_columns,self.camera_margin_m,self.min_camera_margin_m.map_or("null".into(),|v|v.to_string()),self.stream_active_frames,self.stream_outside_frames,a.map_or(0,|a|a.bytes),self.effective_budget_mib,self.request.is_some_and(|r|r.budget==usize::MAX))
    }
}

#[derive(Clone)]
pub(crate) struct Bindings {
    pub uniform: wgpu::Buffer,
    pub chunks: wgpu::TextureView,
    pub blocks: wgpu::Buffer,
    pub voxels: wgpu::Buffer,
    pub palette: wgpu::Buffer,
}
pub(crate) fn binding_layout(first: u32) -> Vec<wgpu::BindGroupLayoutEntry> {
    (0..5)
        .map(|i| wgpu::BindGroupLayoutEntry {
            binding: first + i,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: match i {
                0 => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                1 => wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                },
                _ => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            },
            count: None,
        })
        .collect()
}
fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("NAADF exact hierarchy"),
        entries: &binding_layout(0),
    })
}
impl Bindings {
    pub fn entries(&self, first: u32) -> Vec<wgpu::BindGroupEntry<'_>> {
        vec![
            wgpu::BindGroupEntry {
                binding: first,
                resource: self.uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: first + 1,
                resource: wgpu::BindingResource::TextureView(&self.chunks),
            },
            wgpu::BindGroupEntry {
                binding: first + 2,
                resource: self.blocks.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: first + 3,
                resource: self.voxels.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: first + 4,
                resource: self.palette.as_entire_binding(),
            },
        ]
    }
    pub fn empty(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("inactive NAADF"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R32Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let buffer = |bytes: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("inactive NAADF words"),
                contents: bytes,
                usage,
            })
        };
        Self {
            uniform: buffer(
                bytemuck::bytes_of(&Uniform::zeroed()),
                wgpu::BufferUsages::UNIFORM,
            ),
            chunks: texture.create_view(&Default::default()),
            blocks: buffer(&[0; 4], wgpu::BufferUsages::STORAGE),
            voxels: buffer(&[0; 4], wgpu::BufferUsages::STORAGE),
            palette: buffer(&[0; 16], wgpu::BufferUsages::STORAGE),
        }
    }
}

fn make_pipeline(gpu: &Gpu, layout: &wgpu::BindGroupLayout) -> wgpu::RenderPipeline {
    let shader = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("NAADF hierarchical exact rays"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
    let pl = gpu
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("NAADF primary visibility"),
            bind_group_layouts: &[
                &layout,
                &crate::adaptive_gpu::adaptive_shadow::sampling_layout(&gpu.device),
            ],
            push_constant_ranges: &[],
        });
    gpu.device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("NAADF exact native voxel pass"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.scene_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: gpu.sample_count,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        })
}

fn ray_basis(camera: Camera, aspect: f32) -> [[f32; 4]; 4] {
    let forward = camera.direction().normalize();
    let right = forward.cross(glam::Vec3::Y).normalize();
    let up = right.cross(forward);
    let p = glam::Mat4::perspective_rh(camera.fov_y.to_radians(), aspect, 0.025, camera.far);
    [
        forward.extend(0.).to_array(),
        right.extend(1. / p.x_axis.x).to_array(),
        up.extend(1. / p.y_axis.y).to_array(),
        [0.025, camera.far, -p.z_axis.z, p.w_axis.z],
    ]
}
