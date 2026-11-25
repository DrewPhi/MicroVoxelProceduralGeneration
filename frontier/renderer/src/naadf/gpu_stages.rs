//! Opt-in compute-pass timestamps. No encoder/inside-pass timestamp features.
//! A bounded pool drops samples instead of delaying rendering or rebuilding work.
use std::{
    fmt::Write,
    sync::{mpsc, Arc, Mutex, OnceLock},
};

const POOL: usize = 24;
const HISTORY: usize = 10000;
const STAGES: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum Stage {
    HeightBase,
    HeightReduce,
    Beam,
    RootRepair,
    Feedback,
    Scatter,
}
const ALL: [Stage; STAGES] = [
    Stage::HeightBase,
    Stage::HeightReduce,
    Stage::Beam,
    Stage::RootRepair,
    Stage::Feedback,
    Stage::Scatter,
];
impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::HeightBase => "height_base",
            Self::HeightReduce => "height_reduce_chain",
            Self::Beam => "beam_prepass",
            Self::RootRepair => "root_cache_compute_batch",
            Self::Feedback => "feedback_dispatch",
            Self::Scatter => "scatter_dispatch",
        }
    }
    fn scope(self) -> &'static str {
        match self {
            Self::HeightBase => {
                "actual occupied-height base compute pass; excludes settings queue upload"
            }
            Self::HeightReduce => "first reduction pass begin through last reduction pass end",
            Self::Beam => {
                "actual beam compute pass; excludes CPU target allocation and queue uploads"
            }
            Self::RootRepair => {
                "seed/Jacobi through final cache/direct publication compute pass; one tile batch"
            }
            Self::Feedback => {
                "sampled feedback compute pass; excludes clear, GPU readback copy and mapping"
            }
            Self::Scatter => {
                "actual scatter compute pass; excludes host upload and queue staging copy"
            }
        }
    }
}
pub(crate) fn requested() -> bool {
    static REQUESTED: OnceLock<bool> = OnceLock::new();
    *REQUESTED
        .get_or_init(|| std::env::var_os("FRONTIER_NAADF_GPU_STAGES").is_some_and(|v| v == "1"))
}
fn supported(device: &wgpu::Device) -> bool {
    device.features().contains(wgpu::Features::TIMESTAMP_QUERY)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Use {
    Free,
    Reserved(Stage),
    Deferred(Stage),
    Pending(Stage),
}
struct Pool {
    slots: [Use; POOL],
    attempted: [u64; STAGES],
    dropped: [u64; STAGES],
}
impl Pool {
    fn new() -> Self {
        Self {
            slots: [Use::Free; POOL],
            attempted: [0; STAGES],
            dropped: [0; STAGES],
        }
    }
    fn acquire(&mut self, stage: Stage) -> Option<usize> {
        self.attempted[stage as usize] += 1;
        let slot = self.slots.iter().position(|s| *s == Use::Free);
        if let Some(i) = slot {
            self.slots[i] = Use::Reserved(stage);
        } else {
            self.dropped[stage as usize] += 1;
        }
        slot
    }
    fn defer(&mut self, index: usize) {
        let Use::Reserved(stage) = self.slots[index] else {
            panic!("unreserved timestamp slot")
        };
        self.slots[index] = Use::Deferred(stage);
    }
    fn submit(&mut self, index: usize) -> Stage {
        let (Use::Reserved(stage) | Use::Deferred(stage)) = self.slots[index] else {
            panic!("timestamp slot submitted twice")
        };
        self.slots[index] = Use::Pending(stage);
        stage
    }
    fn release(&mut self, index: usize) {
        self.slots[index] = Use::Free;
    }
}
struct Resources {
    query: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
}
struct State {
    pool: Pool,
    pending: [Option<mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>; POOL],
    stats: [frontier_runtime::FrameStats; STAGES],
    invalid: [u64; STAGES],
    canceled: [u64; STAGES],
}
pub(crate) struct Profiler {
    resources: [Resources; POOL],
    state: Mutex<State>,
    period_ns: f64,
}
pub(crate) struct Ticket {
    profiler: Arc<Profiler>,
    index: usize,
    active: bool,
}
impl Profiler {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Arc<Self>> {
        Self::configured(device, queue, requested())
    }
    fn configured(device: &wgpu::Device, queue: &wgpu::Queue, enabled: bool) -> Option<Arc<Self>> {
        if !enabled || !supported(device) {
            return None;
        }
        Some(Arc::new(Self {
            resources: std::array::from_fn(|_| Resources {
                query: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("NAADF optional compute stage timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: 2,
                }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("NAADF compute stage query resolve"),
                    size: 256,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                readback: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("NAADF asynchronous compute stage timestamps"),
                    size: 16,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
            }),
            period_ns: queue.get_timestamp_period() as f64,
            state: Mutex::new(State {
                pool: Pool::new(),
                pending: std::array::from_fn(|_| None),
                stats: std::array::from_fn(|_| frontier_runtime::FrameStats::new(HISTORY)),
                invalid: [0; STAGES],
                canceled: [0; STAGES],
            }),
        }))
    }
    pub(crate) fn begin(self: &Arc<Self>, stage: Stage) -> Option<Ticket> {
        let index = self.state.lock().unwrap().pool.acquire(stage)?;
        Some(Ticket {
            profiler: Arc::clone(self),
            index,
            active: true,
        })
    }
    fn submitted(&self, index: usize) {
        let mut state = self.state.lock().unwrap();
        state.pool.submit(index);
        let (tx, rx) = mpsc::channel();
        self.resources[index]
            .readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        state.pending[index] = Some(rx);
    }
    /// Only after submitting the original scene/capture encoder containing all
    /// deferred height/beam resolves. External compute submissions commit only
    /// their own ticket and cannot prematurely map this encoder's buffers.
    pub(crate) fn submitted_render(&self) {
        let indices: Vec<_> = self
            .state
            .lock()
            .unwrap()
            .pool
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| matches!(s, Use::Deferred(_)).then_some(i))
            .collect();
        for index in indices {
            self.submitted(index);
        }
    }
    pub(crate) fn poll(&self, device: &wgpu::Device) {
        device.poll(wgpu::Maintain::Poll);
        let mut state = self.state.lock().unwrap();
        for index in 0..POOL {
            let Some(rx) = &state.pending[index] else {
                continue;
            };
            let status = match rx.try_recv() {
                Ok(status) => status.map_err(|_| ()),
                Err(mpsc::TryRecvError::Empty) => continue,
                Err(mpsc::TryRecvError::Disconnected) => Err(()),
            };
            let Use::Pending(stage) = state.pool.slots[index] else {
                panic!("mapped timestamp slot was not submitted")
            };
            if status.is_ok() {
                let bytes = self.resources[index].readback.slice(..).get_mapped_range();
                let a = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
                let b = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
                if b >= a {
                    state.stats[stage as usize]
                        .record((b - a) as f64 * self.period_ns / 1_000_000.);
                } else {
                    state.invalid[stage as usize] += 1;
                }
                drop(bytes);
                self.resources[index].readback.unmap();
            } else {
                state.invalid[stage as usize] += 1;
            }
            state.pending[index] = None;
            state.pool.release(index);
        }
    }
    pub(crate) fn flush(&self, device: &wgpu::Device) {
        device.poll(wgpu::Maintain::Wait);
        self.poll(device);
    }
    pub(crate) fn bytes(&self) -> usize {
        POOL * (256 + 16)
    }
    #[cfg(test)]
    pub(crate) fn samples(&self, stage: Stage) -> usize {
        self.state.lock().unwrap().stats[stage as usize]
            .summary()
            .sample_count
    }
    #[cfg(test)]
    pub(crate) fn assert_settled(&self) {
        let state = self.state.lock().unwrap();
        assert!(state.pool.slots.iter().all(|s| *s == Use::Free));
        assert!(state.invalid.iter().all(|n| *n == 0));
    }

    pub(crate) fn json(&self) -> String {
        let state = self.state.lock().unwrap();
        let pending = state
            .pool
            .slots
            .iter()
            .filter(|s| matches!(s, Use::Pending(_)))
            .count();
        let encoded = state
            .pool
            .slots
            .iter()
            .filter(|s| matches!(s, Use::Deferred(_) | Use::Reserved(_)))
            .count();
        let mut json=format!(concat!(
            "{{\"requested\":true,\"supported\":true,\"enabled\":true,",
            "\"pool_capacity\":{},\"pending\":{},\"encoded_not_submitted\":{},",
            "\"query_count\":{},\"gpu_buffer_bytes\":{},\"cpu_sample_capacity_bytes\":{},",
            "\"query_backend_storage_bytes_not_exposed\":true,",
            "\"sample_policy\":\"bounded rolling 10000 per stage; includes startup and warmup; unpaired with frame statistics\",",
            "\"scope\":\"compute pass timestamps only; uploads, GPU copies, CPU work and query resolve/readback excluded\",",
            "\"primary_shadow_split\":\"no per-ray timestamps; use matched full/primary/voxel-shadows/mesh-shadows ablations for marginal costs, which include changed branch divergence and are not independent additive timings\",",
            "\"stages\":{{"
        ),POOL,pending,encoded,POOL*2,self.bytes(),STAGES*HISTORY*8);
        for (index, stage) in ALL.into_iter().enumerate() {
            let s = state.stats[index].summary();
            let number = |x: Option<f64>| x.map_or("null".into(), |v| v.to_string());
            write!(json,concat!(
                "{}{:?}:{{\"scope\":{:?},\"attempted\":{},\"samples\":{},\"recorded_total\":{},",
                "\"dropped_pool_full\":{},\"invalid_or_map_failed\":{},\"canceled_before_submit\":{},",
                "\"p50_ms\":{},\"p95_ms\":{},\"p99_ms\":{}}}"
            ),if index==0{""}else{","},stage.name(),stage.scope(),state.pool.attempted[index],
                s.sample_count,s.accepted_frames,state.pool.dropped[index],state.invalid[index],
                state.canceled[index],number(s.p50_ms),number(s.p95_ms),number(s.p99_ms)).unwrap();
        }
        json.push_str("}}");
        json
    }
}
impl Ticket {
    pub(crate) fn writes(&self, first: bool, last: bool) -> wgpu::ComputePassTimestampWrites<'_> {
        wgpu::ComputePassTimestampWrites {
            query_set: &self.profiler.resources[self.index].query,
            beginning_of_pass_write_index: first.then_some(0),
            end_of_pass_write_index: last.then_some(1),
        }
    }
    pub(crate) fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        let r = &self.profiler.resources[self.index];
        encoder.resolve_query_set(&r.query, 0..2, &r.resolve, 0);
        encoder.copy_buffer_to_buffer(&r.resolve, 0, &r.readback, 0, 16);
    }
    pub(crate) fn defer(mut self) {
        self.profiler.state.lock().unwrap().pool.defer(self.index);
        self.active = false;
    }
    pub(crate) fn submitted(mut self) {
        self.profiler.submitted(self.index);
        self.active = false;
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        if self.active {
            let mut state = self.profiler.state.lock().unwrap();
            let Use::Reserved(stage) = state.pool.slots[self.index] else {
                panic!("timestamp reservation state")
            };
            state.canceled[stage as usize] += 1;
            state.pool.release(self.index);
        }
    }
}
pub(crate) fn json(profiler: Option<&Profiler>, device: &wgpu::Device) -> String {
    profiler.map_or_else(||format!(
        "{{\"requested\":{},\"supported\":{},\"enabled\":false,\"query_count\":0,\"gpu_buffer_bytes\":0}}",
        requested(),supported(device)),Profiler::json)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pool_is_bounded_and_deferred_encoders_are_not_mapped_by_other_submissions() {
        let mut pool = Pool::new();
        let scene = pool.acquire(Stage::Beam).unwrap();
        pool.defer(scene);
        let external = pool.acquire(Stage::Scatter).unwrap();
        pool.submit(external);
        assert_eq!(pool.slots[scene], Use::Deferred(Stage::Beam));
        assert_eq!(pool.slots[external], Use::Pending(Stage::Scatter));
        for _ in 2..POOL {
            assert!(pool.acquire(Stage::RootRepair).is_some());
        }
        assert!(pool.acquire(Stage::Feedback).is_none());
        assert_eq!(pool.dropped[Stage::Feedback as usize], 1);
        pool.release(external);
        assert_eq!(pool.acquire(Stage::Scatter), Some(external));
        pool.submit(scene);
        assert_eq!(pool.slots[scene], Use::Pending(Stage::Beam));
    }
    #[test]
    fn schema_stage_names_are_unique_and_scopes_exclude_uploads() {
        let mut names = std::collections::HashSet::new();
        for stage in ALL {
            assert!(names.insert(stage.name()));
            assert!(!stage.scope().is_empty());
        }
        assert!(Stage::Scatter.scope().contains("excludes host upload"));
        assert_eq!(POOL * (256 + 16), 6528);
    }
}

#[test]
#[ignore = "explicit GPU timestamps: TIMESTAMP_QUERY only, async stage pool, original dispatch count"]
fn gpu_stage_timestamps_use_base_feature_without_repeating_work() {
    use wgpu::util::DeviceExt;
    pollster::block_on(async {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .unwrap();
        if !adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            eprintln!("TIMESTAMP_QUERY unavailable; optional stage profiling remains disabled");
            return;
        }
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    required_features: wgpu::Features::TIMESTAMP_QUERY,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert!(Profiler::configured(&device, &queue, false).is_none());
        let profiler = Profiler::configured(&device, &queue, true).unwrap();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("stage timestamp dispatch-count oracle"),
            source: wgpu::ShaderSource::Wgsl(
                "@group(0) @binding(0) var<storage,read_write> counter:array<u32>;
                 @compute @workgroup_size(1) fn main(){counter[0]+=1u;}"
                    .into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let counter = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &[0; 4],
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: counter.as_entire_binding(),
            }],
        });
        let mut expected = 0;
        for stage in ALL {
            let ticket = profiler.begin(stage).unwrap();
            let mut encoder = device.create_command_encoder(&Default::default());
            let passes = if stage == Stage::HeightReduce { 3 } else { 1 };
            for index in 0..passes {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: None,
                    timestamp_writes: (index == 0 || index + 1 == passes)
                        .then(|| ticket.writes(index == 0, index + 1 == passes)),
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups(1, 1, 1);
                expected += 1;
            }
            ticket.resolve(&mut encoder);
            if stage == Stage::Beam {
                ticket.defer();
                queue.submit([encoder.finish()]);
                profiler.submitted_render();
            } else {
                queue.submit([encoder.finish()]);
                ticket.submitted();
            }
        }
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&counter, 0, &readback, 0, 4);
        queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        profiler.flush(&device);
        rx.recv().unwrap().unwrap();
        let bytes = readback.slice(..).get_mapped_range();
        assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), expected);
        let state = profiler.state.lock().unwrap();
        for stage in ALL {
            let index = stage as usize;
            let stats = state.stats[index].summary();
            assert_eq!(stats.sample_count, 1, "{:?}", stage);
            assert!(stats.p50_ms.unwrap().is_finite());
            assert_eq!(state.pool.attempted[index], 1);
            assert_eq!(state.pool.dropped[index], 0);
            assert_eq!(state.invalid[index], 0);
        }
        assert!(state.pool.slots.iter().all(|s| *s == Use::Free));
        drop(state);
        eprintln!("NAADF GPU stage schema: {}", profiler.json());
    });
}
