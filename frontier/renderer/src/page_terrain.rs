//! Phase A1: bounded, world-aligned source pages and one reusable GPU grid.
//!
//! This deliberately uniform-spacing baseline is not CDLOD or an error bound.
//! Only immutable FbmTerrain is heightfield-safe. The hybrid coordinator owns
//! exact near geometry and the fixed volume footprint; pages never own caves.
//! NativeStreamer::desired_revision is a camera transaction epoch. Only the
//! world's source revision enters CPU/GPU content keys; camera epochs gate cover
//! publication and can never invalidate otherwise reusable page samples.
use frontier_world::{FbmParams, FbmTerrain};
use std::sync::{Arc, Condvar, Mutex};
use wgpu::util::DeviceExt;

pub const SIDE: usize = 32;
const ROW: usize = SIDE + 1;
const SAMPLES: usize = ROW * ROW;
pub const COVER_SIDE: i64 = 24;
pub const MAX_PAGES: usize = (COVER_SIDE * COVER_SIDE) as usize;
const CACHE_PAGES: usize = MAX_PAGES * 2;
pub const HALF_EXTENT_M: f64 = COVER_SIDE as f64 * SIDE as f64 * 0.5;
pub const GPU_BYTES: u64 =
    (MAX_PAGES * SAMPLES * 48 + MAX_PAGES * 16 + SIDE * SIDE * 6 * 4 + 144) as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    tile: [i64; 2],
    source_revision: u64,
    step: u32,
    source: [u64; 8],
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct Sample {
    height: [f32; 4],
    normal: [f32; 4],
    color: [f32; 4],
}
#[derive(Clone)]
struct Page {
    key: Key,
    samples: Arc<[Sample]>,
}
pub struct Cover {
    pub center: [f64; 2],
    pages: Vec<Page>,
    pub source_revision: u64,
    /// Native cover transaction only; never part of a source-page cache key.
    pub native_epoch: u64,
}
impl Cover {
    pub fn matches(
        &self,
        center: [f64; 2],
        source_revision: u64,
        native_epoch: u64,
        step: u32,
    ) -> bool {
        self.source_revision == source_revision
            && self.native_epoch == native_epoch
            && self.center == snapped_center(center, step)
    }
    /// A source refresh can publish against an already-current native cover;
    /// a new camera transaction must still wait for its matching native cover.
    pub fn can_publish(
        &self,
        source_revision: u64,
        step: u32,
        active_native: ([f64; 2], u64),
        desired_native: ([f64; 2], u64),
    ) -> bool {
        active_native == desired_native
            && self.matches(desired_native.0, source_revision, desired_native.1, step)
    }
    fn for_native_epoch(mut self, native_epoch: u64) -> Self {
        self.native_epoch = native_epoch;
        self
    }
}
pub fn snapped_center(center: [f64; 2], step: u32) -> [f64; 2] {
    snap(center, step).map(|v| v as f64 * SIDE as f64 * step as f64)
}
pub fn guaranteed_radius(step: u32) -> f64 {
    (HALF_EXTENT_M - SIDE as f64 - 8.) * step as f64
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Stats {
    pub generated_pages: u64,
    pub reused_pages: u64,
    pub cancelled_jobs: u64,
    pub publications: u64,
    pub cpu_cache_pages: usize,
    pub last_generation_ms: f64,
    pub uploaded_pages: u64,
    pub uploaded_bytes: u64,
}
struct Cache {
    source: Option<(u64, FbmParams, u32, u32, u64)>,
    pages: std::collections::VecDeque<Page>,
    stats: Stats,
}
impl Cache {
    fn new() -> Self {
        Self {
            source: None,
            pages: Default::default(),
            stats: Stats::default(),
        }
    }
    fn build(
        &mut self,
        source: &FbmTerrain,
        center: [i64; 2],
        source_revision: u64,
        step: u32,
        cancelled: impl Fn() -> bool,
    ) -> Option<Cover> {
        self.build_region(source, center, source_revision, step, COVER_SIDE, cancelled)
    }
    fn build_region(
        &mut self,
        source: &FbmTerrain,
        center: [i64; 2],
        source_revision: u64,
        step: u32,
        side: i64,
        cancelled: impl Fn() -> bool,
    ) -> Option<Cover> {
        let identity = (
            source.seed(),
            source.params(),
            source.source_version(),
            source.appearance_version(),
            source_revision,
        );
        if self.source != Some(identity) {
            self.pages.clear();
            self.source = Some(identity);
        }
        let p = source.params();
        let source_key = [
            source.seed(),
            p.recipe as u64,
            p.horizontal_scale.to_bits(),
            p.amplitude.to_bits(),
            p.octaves as u64,
            p.roughness.to_bits(),
            source.source_version() as u64,
            source.appearance_version() as u64,
        ];
        let started = std::time::Instant::now();
        let mut pages = Vec::with_capacity(MAX_PAGES);
        for z in -side / 2..side / 2 {
            for x in -side / 2..side / 2 {
                if cancelled() {
                    self.stats.cancelled_jobs += 1;
                    return None;
                }
                let key = Key {
                    tile: [center[0] + x, center[1] + z],
                    source_revision,
                    step,
                    source: source_key,
                };
                let page = if let Some(i) = self.pages.iter().position(|p| p.key == key) {
                    self.stats.reused_pages += 1;
                    self.pages.remove(i).unwrap()
                } else {
                    let Some(page) = generate_page(source, key, &cancelled) else {
                        self.stats.cancelled_jobs += 1;
                        return None;
                    };
                    self.stats.generated_pages += 1;
                    page
                };
                pages.push(page.clone());
                if self.pages.len() == CACHE_PAGES {
                    self.pages.pop_front();
                }
                self.pages.push_back(page);
            }
        }
        self.stats.cpu_cache_pages = self.pages.len();
        self.stats.last_generation_ms = started.elapsed().as_secs_f64() * 1000.;
        Some(Cover {
            center: center.map(|v| (v * SIDE as i64) as f64 * step as f64),
            pages,
            source_revision,
            native_epoch: 0,
        })
    }
}
// Identical one-meter display filtering to TerrainData level one: four CPU
// canonical columns, not independent GPU noise or a new canonical world.
fn value(source: &FbmTerrain, x: f64, z: f64, step: f64) -> (f64, [f32; 4]) {
    let mut height = 0.;
    let mut color = [0., 0., 0., 1.];
    let d = step * 0.25;
    for (x, z) in [
        (x - d, z - d),
        (x + d, z - d),
        (x - d, z + d),
        (x + d, z + d),
    ] {
        let h = source.height_m(x, z);
        height += h * 0.25;
        for (dst, c) in color[..3].iter_mut().zip(source.surface_color(x, z, h)) {
            *dst += c * 0.25;
        }
    }
    (height, color)
}
fn generate_page(source: &FbmTerrain, key: Key, cancelled: impl Fn() -> bool) -> Option<Page> {
    const HALO: usize = ROW + 2;
    let step = key.step as f64;
    let mut values = Vec::with_capacity(HALO * HALO);
    for z in -1..=ROW as i64 {
        if cancelled() {
            return None;
        }
        for x in -1..=ROW as i64 {
            values.push(value(
                source,
                (key.tile[0] * SIDE as i64 + x) as f64 * step,
                (key.tile[1] * SIDE as i64 + z) as f64 * step,
                step,
            ));
        }
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for z in 0..ROW {
        for x in 0..ROW {
            let i = (z + 1) * HALO + x + 1;
            let (h, color) = values[i];
            let n = glam::DVec3::new(
                values[i - 1].0 - values[i + 1].0,
                2. * step,
                values[i - HALO].0 - values[i + HALO].0,
            )
            .normalize();
            samples.push(Sample {
                height: [h as f32, 0., 0., 0.],
                normal: [n.x as f32, n.y as f32, n.z as f32, 0.],
                color,
            });
        }
    }
    Some(Page {
        key,
        samples: samples.into(),
    })
}
#[cfg(test)]
fn sample(source: &FbmTerrain, x: f64, z: f64, step: f64) -> Sample {
    let (h, color) = value(source, x, z, step);
    let dx = value(source, x - step, z, step).0 - value(source, x + step, z, step).0;
    let dz = value(source, x, z - step, step).0 - value(source, x, z + step, step).0;
    let n = glam::DVec3::new(dx, 2. * step, dz).normalize();
    Sample {
        height: [h as f32, 0., 0., 0.],
        normal: [n.x as f32, n.y as f32, n.z as f32, 0.],
        color,
    }
}
fn snap(center: [f64; 2], step: u32) -> [i64; 2] {
    center.map(|v| (v / (SIDE as f64 * step as f64)).floor() as i64)
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Demand {
    center: [i64; 2],
    source_revision: u64,
    native_epoch: u64,
    epoch: u64,
}
struct Mailbox {
    demand: Option<Demand>,
    ready: Option<(Demand, Cover)>,
    stop: bool,
    stats: Stats,
}
/// One worker, one newest demand, one completed cover, and <=1152 cached pages.
/// Camera epochs cancel selection, never the identity of completed source pages.
pub struct Stream {
    mailbox: Arc<(Mutex<Mailbox>, Condvar)>,
    worker: Option<std::thread::JoinHandle<()>>,
    desired: Option<Demand>,
    epoch: u64,
    step: u32,
}
impl Stream {
    pub fn new(source: FbmTerrain, step: u32) -> Self {
        assert!([1, 2, 4, 8, 16].contains(&step));
        let mailbox = Arc::new((
            Mutex::new(Mailbox {
                demand: None,
                ready: None,
                stop: false,
                stats: Stats::default(),
            }),
            Condvar::new(),
        ));
        let shared = mailbox.clone();
        let worker = std::thread::spawn(move || {
            let mut cache = Cache::new();
            let mut completed = None;
            loop {
                let demand = {
                    let (lock, wake) = &*shared;
                    let mut state = lock.lock().unwrap();
                    while !state.stop && (state.demand.is_none() || state.demand == completed) {
                        state = wake.wait(state).unwrap();
                    }
                    if state.stop {
                        break;
                    }
                    state.demand.unwrap()
                };
                let cover =
                    cache.build(&source, demand.center, demand.source_revision, step, || {
                        let s = shared.0.lock().unwrap();
                        s.stop || s.demand != Some(demand)
                    });
                let mut state = shared.0.lock().unwrap();
                state.stats = cache.stats;
                if state.stop {
                    break;
                }
                if state.demand == Some(demand) {
                    if let Some(cover) = cover {
                        state.ready = Some((demand, cover.for_native_epoch(demand.native_epoch)));
                        completed = Some(demand);
                    }
                }
            }
        });
        Self {
            mailbox,
            worker: Some(worker),
            desired: None,
            epoch: 0,
            step,
        }
    }
    pub fn request(&mut self, center: [f64; 2], source_revision: u64, native_epoch: u64) {
        let center = snap(center, self.step);
        if self.desired.is_some_and(|d| {
            d.center == center
                && d.source_revision == source_revision
                && d.native_epoch == native_epoch
        }) {
            return;
        }
        self.epoch += 1;
        let demand = Demand {
            center,
            source_revision,
            native_epoch,
            epoch: self.epoch,
        };
        self.desired = Some(demand);
        let mut state = self.mailbox.0.lock().unwrap();
        state.demand = Some(demand);
        state.ready = None;
        self.mailbox.1.notify_one();
    }
    pub fn poll(&mut self) -> Option<Cover> {
        let mut state = self.mailbox.0.lock().unwrap();
        state
            .ready
            .take()
            .and_then(|(d, c)| (Some(d) == self.desired).then_some(c))
    }
    pub fn stats(&self) -> Stats {
        self.mailbox.0.lock().unwrap().stats
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        {
            let mut s = self.mailbox.0.lock().unwrap();
            s.stop = true;
            self.mailbox.1.notify_one();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    offset: [f32; 2],
    slot: u32,
    pad: u32,
}
fn cached_slot(slots: &[Option<Key>], key: Key) -> Option<usize> {
    slots.iter().position(|k| *k == Some(key))
}
pub struct PageGpu {
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    samples: wgpu::Buffer,
    instances: wgpu::Buffer,
    indices: wgpu::Buffer,
    slots: Vec<Option<Key>>,
    active: Vec<(Key, u32)>,
    pub center: [f64; 2],
    pub source_revision: u64,
    pub stats: Stats,
    step: u32,
    upload_cpu_ms: f64,
    last_upload_bytes: u64,
    max_upload_bytes: u64,
}
impl PageGpu {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, sample_count: u32) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("page grid camera"),
            size: 144,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let samples = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bounded height/material page pool"),
            size: (MAX_PAGES * SAMPLES * std::mem::size_of::<Sample>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("page instances"),
            size: (MAX_PAGES * 16) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("page grid"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("page grid"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: samples.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: instances.as_entire_binding(),
                },
            ],
        });
        let shader_source = shader_source();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("reusable height page grid"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("page grid"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("experimental uniform source pages"),
            layout: Some(&pipeline_layout),
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
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });
        let mut topology = Vec::<u32>::with_capacity(SIDE * SIDE * 6);
        for z in 0..SIDE {
            for x in 0..SIDE {
                let a = (z * ROW + x) as u32;
                let row = ROW as u32;
                topology.extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
            }
        }
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("one immutable32x32grid"),
            contents: bytemuck::cast_slice(&topology),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            pipeline,
            bind,
            uniform,
            samples,
            instances,
            indices,
            slots: vec![None; MAX_PAGES],
            active: Vec::new(),
            center: [0.; 2],
            source_revision: 0,
            stats: Stats::default(),
            step: 1,
            upload_cpu_ms: 0.,
            last_upload_bytes: 0,
            max_upload_bytes: 0,
        }
    }
    pub fn upload(&mut self, queue: &wgpu::Queue, cover: &Cover) {
        let started = std::time::Instant::now();
        let before = self.stats.uploaded_bytes;
        self.active.clear();
        for page in &cover.pages {
            let slot = cached_slot(&self.slots, page.key).unwrap_or_else(|| {
                let slot = self
                    .slots
                    .iter()
                    .position(|k| k.is_none() || !cover.pages.iter().any(|p| Some(p.key) == *k))
                    .expect("bounded complete cover fits pool");
                queue.write_buffer(
                    &self.samples,
                    (slot * SAMPLES * 48) as u64,
                    bytemuck::cast_slice(&page.samples),
                );
                self.stats.uploaded_pages += 1;
                self.stats.uploaded_bytes += (SAMPLES * 48) as u64;
                self.slots[slot] = Some(page.key);
                slot
            });
            self.active.push((page.key, slot as u32));
        }
        self.center = cover.center;
        self.source_revision = cover.source_revision;
        self.step = cover.pages[0].key.step;
        self.stats.publications += 1;
        self.last_upload_bytes = self.stats.uploaded_bytes - before;
        self.max_upload_bytes = self.max_upload_bytes.max(self.last_upload_bytes);
        self.upload_cpu_ms += started.elapsed().as_secs_f64() * 1000.;
    }
    pub fn contains(&self, position: glam::DVec3, far: f32) -> bool {
        (position.x - self.center[0]).abs() + far as f64 <= HALF_EXTENT_M * self.step as f64
            && (position.z - self.center[1]).abs() + far as f64 <= HALF_EXTENT_M * self.step as f64
    }
    pub fn json(&self, cpu: Stats, retained_gpu_bytes: u64) -> String {
        format!("{{\"layout\":\"bounded_uniform_worldaligned_pages\",\"full_CDLOD\":false,\"proxy_error_certified\":false,\"source_revision\":{},\"camera_epoch_in_content_key\":false,\"source_voxel_m\":0.0625,\"display_spacing_m\":{},\"side_pages\":{},\"active_pages\":{},\"half_extent_m\":{},\"guaranteed_radius_m\":{},\"grid_topologies\":1,\"triangles\":{},\"draw_calls\":1,\"gpu_bytes\":{},\"gpu_bytes_scope\":\"page_buffers_only\",\"retained_clipmap_gpu_bytes\":{},\"page_and_retained_clipmap_gpu_bytes\":{},\"native_and_volume_bytes_counted\":false,\"retained_clipmap_generation\":true,\"upload_budget_per_frame_enforced\":false,\"cpu_cache_pages\":{},\"cpu_cache_payload_bytes\":{},\"cpu_payload_hard_bound_bytes\":{},\"staging_payload_hard_bound_bytes\":{},\"generated_pages\":{},\"reused_pages\":{},\"cancelled_jobs\":{},\"last_generation_ms\":{},\"publications\":{},\"uploaded_pages\":{},\"uploaded_bytes\":{},\"upload_cpu_ms\":{},\"max_frame_upload_bytes\":{},\"legacy_shared_lighting\":true,\"collar_sample_spacing_matched\":{},\"heightfield_safe_source_only\":true,\"exact_volume_owner_preserved\":true}}",
            self.source_revision,self.step,COVER_SIDE,self.active.len(),HALF_EXTENT_M*self.step as f64,guaranteed_radius(self.step),self.active.len()*SIDE*SIDE*2,GPU_BYTES,retained_gpu_bytes,GPU_BYTES+retained_gpu_bytes,cpu.cpu_cache_pages,cpu.cpu_cache_pages*SAMPLES*48,(CACHE_PAGES+2*MAX_PAGES)*SAMPLES*48,MAX_PAGES*SAMPLES*48,cpu.generated_pages,cpu.reused_pages,cpu.cancelled_jobs,cpu.last_generation_ms,self.stats.publications,self.stats.uploaded_pages,self.stats.uploaded_bytes,self.upload_cpu_ms,self.max_upload_bytes,self.step==1)
    }
    pub fn prepare(
        &self,
        queue: &wgpu::Queue,
        camera: &crate::Camera,
        aspect: f32,
        near: [f64; 2],
        volume: bool,
        voxel_filter: bool,
    ) {
        let origin = glam::DVec3::new(self.center[0], 0., self.center[1]);
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::cast_slice(&camera.matrix(origin, aspect).to_cols_array()),
        );
        let eye = (camera.position - origin).as_vec3();
        queue.write_buffer(
            &self.uniform,
            64,
            bytemuck::cast_slice(&[
                [eye.x, eye.y, eye.z, if voxel_filter { 1. } else { 0. }],
                [
                    (near[0] - self.center[0]) as f32,
                    (near[1] - self.center[1]) as f32,
                    34.,
                    1.,
                ],
                [
                    -self.center[0] as f32,
                    -self.center[1] as f32,
                    8.,
                    if volume { 1. } else { 0. },
                ],
            ]),
        );
        let paint = [
            (self.center[0] * 16.) as i64 as u32,
            0,
            (self.center[1] * 16.) as i64 as u32,
            0,
        ];
        queue.write_buffer(&self.uniform, 112, bytemuck::cast_slice(&paint));
        queue.write_buffer(
            &self.uniform,
            128,
            bytemuck::cast_slice(&[self.step as f32, 0., 0., 0.]),
        );
        let instances: Vec<Instance> = self
            .active
            .iter()
            .map(|(key, slot)| Instance {
                offset: [
                    (key.tile[0] * SIDE as i64) as f64 * self.step as f64 - self.center[0],
                    (key.tile[1] * SIDE as i64) as f64 * self.step as f64 - self.center[1],
                ]
                .map(|v| v as f32),
                slot: *slot,
                pad: 0,
            })
            .collect();
        queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&instances));
    }
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..(SIDE * SIDE * 6) as u32, 0, 0..self.active.len() as u32);
    }
}
fn shader_source() -> String {
    let common = include_str!("terrain.wgsl")
        .split("struct Camera")
        .next()
        .unwrap();
    format!("{}\n{}", common, include_str!("page_terrain.wgsl"))
}

#[cfg(test)]
#[path = "page_terrain_tests.rs"]
mod tests;
