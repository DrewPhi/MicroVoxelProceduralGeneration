//! Disposable acceleration for exact roots, computed in fixed-size GPU tiles.
use super::{
    stream::{Point, Region},
    Camera,
};
use glam::{DVec3, Vec3};
use std::{
    cell::Cell,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
};
use wgpu::util::DeviceExt;

const SIDE: u32 = 32;
const MAX_EDGE: u32 = 128;
const UNIFORM_BYTES: usize = 128;
const FEEDBACK_BYTES: u64 = 64 * 4;
const SCRATCH_BYTES: usize = SIDE.pow(3) as usize * 4;

pub(super) struct Cache {
    plan: Plan,
    mode: Mode,
    aspect: Cell<f32>,
    counters: Counters,
    feedback: Option<Feedback>,
    feedback_counts: wgpu::Buffer,
    armed: bool,
    in_flight: Arc<AtomicBool>,
    uniform: wgpu::Buffer,
    _texture: wgpu::Texture,
    groups: [wgpu::BindGroup; 2],
    layout: wgpu::PipelineLayout,
    kernels: Option<[wgpu::ComputePipeline; 5]>,
}

pub(super) fn cpu_fallback() -> bool {
    static FALLBACK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FALLBACK.get_or_init(|| {
        std::env::var_os("FRONTIER_NAADF_DISTANCE_CPU_FALLBACK").is_some_and(|v| v == "1")
    })
}

pub(super) fn edge(n: u32) -> u32 {
    n.next_multiple_of(SIDE).min(MAX_EDGE)
}

pub(super) fn bytes(n: u32) -> usize {
    4 * edge(n).pow(3) as usize
        + 2 * SCRATCH_BYTES
        + UNIFORM_BYTES
        + FEEDBACK_BYTES as usize * (1 + usize::from(Mode::requested() == Mode::Feedback))
}

fn texture(device: &wgpu::Device, edge: u32, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("NAADF root distance tiles"),
        size: wgpu::Extent3d {
            width: edge,
            height: edge,
            depth_or_array_layers: edge,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::R32Uint,
        usage,
        view_formats: &[],
    })
}

pub(super) fn empty(device: &wgpu::Device) -> wgpu::TextureView {
    texture(device, 1, wgpu::TextureUsages::TEXTURE_BINDING).create_view(&Default::default())
}

// First compute the camera tile, then alternate either side along each axis.
fn centered(rank: u32, side: u32) -> u32 {
    if rank % 2 == 0 {
        side / 2 + rank / 2
    } else {
        side / 2 - rank.div_ceil(2)
    }
}

/// These modes build the same proofs over the same world-aligned tile domains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Reset,
    Retain,
    Rays,
    Feedback,
}
impl Mode {
    pub(super) fn requested() -> Self {
        static MODE: std::sync::OnceLock<Mode> = std::sync::OnceLock::new();
        *MODE.get_or_init(
            || match std::env::var("FRONTIER_NAADF_DISTANCE_SCHEDULE").as_deref() {
                Ok("retain") => Self::Retain,
                Ok("rays") => Self::Rays,
                Ok("feedback") => Self::Feedback,
                _ => Self::Reset,
            },
        )
    }
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Reset => "reset",
            Self::Retain => "retain",
            Self::Rays => "rays",
            Self::Feedback => "feedback",
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct Counters {
    pub(super) retained: u64,
    pub(super) invalidated: u64,
    pub(super) submitted: u64,
    pub(super) ray_ranked: u64,
    pub(super) rank_cpu_ns: u64,
    pub(super) feedback_dispatches: u64,
    pub(super) feedback_reads: u64,
    pub(super) feedback_stale: u64,
    pub(super) feedback_prioritized: u64,
    pub(super) feedback_cpu_ns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tile {
    base: Point,
    // A proof depends on every root inside this support. All other roots
    // in its 32³ scratch tile are treated as solid, including off-window cells.
    support: Region,
}
struct Plan {
    region: Region,
    first: Point,
    side: u32,
    tiles: [Option<Tile>; 64],
    order: [u32; 64],
    ready: u64,
}
impl Plan {
    fn new(region: Region, side: u32) -> Self {
        let span = i64::from(side * SIDE);
        let first = std::array::from_fn(|a| {
            let preferred = region.lo[a] + (region.hi[a] - region.lo[a] - span) / 2;
            (preferred + i64::from(SIDE / 2)).div_euclid(i64::from(SIDE))
        });
        let mut plan = Self {
            region,
            first,
            side,
            tiles: [None; 64],
            order: [0; 64],
            ready: 0,
        };
        for rank in 0..side.pow(3) {
            let logical =
                [rank % side, rank / side % side, rank / side.pow(2)].map(|v| centered(v, side));
            let coords: Point = std::array::from_fn(|a| first[a] + i64::from(logical[a]));
            let slot = coords.map(|c| c.rem_euclid(i64::from(side)) as u32);
            let index = slot[0] + side * (slot[1] + side * slot[2]);
            plan.order[rank as usize] = index;
            let base = coords.map(|c| c * i64::from(SIDE));
            let bounds = Region {
                lo: base,
                hi: base.map(|c| c + i64::from(SIDE)),
            };
            let support = region.intersection(bounds);
            if (0..3).all(|a| support.lo[a] < support.hi[a]) {
                plan.tiles[index as usize] = Some(Tile { base, support });
            }
        }
        plan
    }
    fn retain(&mut self, previous: &Self) {
        for i in 0..self.tiles.len() {
            if previous.ready & (1 << i) != 0 && self.tiles[i] == previous.tiles[i] {
                self.ready |= 1 << i;
            }
        }
    }
    fn uniforms(&self) -> [u32; 4] {
        let n = (self.region.hi[0] - self.region.lo[0]) as u32;
        let center = i64::from(n.saturating_sub(MAX_EDGE) / 2);
        let mut base = 0;
        let mut slot = self.side << 24;
        for a in 0..3 {
            let delta = self.first[a] * i64::from(SIDE) - self.region.lo[a] - center;
            let byte = i8::try_from(delta).expect("distance tile base must be near window center");
            base |= u32::from(byte as u8) << (a * 8);
            slot |= (self.first[a].rem_euclid(i64::from(self.side)) as u32) << (a * 8);
        }
        [self.ready as u32, (self.ready >> 32) as u32, base, slot]
    }
    fn candidate(&self, rays: Option<&Rays>, measured: Option<&[u32; 64]>) -> Option<(u32, Tile)> {
        let mut best = None;
        let mut best_score = (0, 0.);
        for &index in &self.order[..self.side.pow(3) as usize] {
            if self.ready & (1 << index) != 0 {
                continue;
            }
            let Some(tile) = self.tiles[index as usize] else {
                continue;
            };
            let Some(rays) = rays else {
                return Some((index, tile));
            };
            let score = (
                measured.map_or(0, |v| v[index as usize]),
                rays.score(tile.support),
            );
            if best.is_none() || score > best_score {
                best = Some((index, tile));
                best_score = score;
            }
        }
        best
    }
}

/// Predict demand with 25 deterministic primary rays through the current
/// viewport. This estimates exposure and distance, not occupancy or hit cost.
struct Rays {
    eye: DVec3,
    far: f64,
    directions: [DVec3; 25],
}
impl Rays {
    fn new(camera: Camera, aspect: f32) -> Self {
        let basis = super::ray_basis(camera, aspect);
        let vector = |b: usize| Vec3::from_slice(&basis[b]).as_dvec3();
        Self {
            eye: camera.position,
            far: f64::from(camera.far),
            directions: std::array::from_fn(|i| {
                let x = (i % 5) as f64 * 0.4 - 0.8;
                let y = (i / 5) as f64 * 0.4 - 0.8;
                (vector(0)
                    + vector(1) * (x * f64::from(basis[1][3]))
                    + vector(2) * (y * f64::from(basis[2][3])))
                .normalize()
            }),
        }
    }
    fn score(&self, region: Region) -> f64 {
        let [lo, hi] = region.bounds();
        self.directions
            .iter()
            .map(|d| {
                let mut entry: f64 = 0.;
                let mut exit = self.far;
                for a in 0..3 {
                    if d[a].abs() < 1e-12 {
                        if self.eye[a] < lo[a] || self.eye[a] >= hi[a] {
                            return 0.;
                        }
                    } else {
                        let t0 = (lo[a] - self.eye[a]) / d[a];
                        let t1 = (hi[a] - self.eye[a]) / d[a];
                        entry = entry.max(t0.min(t1));
                        exit = exit.min(t0.max(t1));
                    }
                }
                if entry >= exit {
                    0.
                } else {
                    (exit - entry) / (1. + entry / f64::from(SIDE))
                }
            })
            .sum()
    }
}

struct PendingFeedback {
    result: mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    camera: Camera,
    revision: u64,
}

struct Feedback {
    buffer: wgpu::Buffer,
    kernel: wgpu::ComputePipeline,
    pending: Option<PendingFeedback>,
    sample: Option<(Camera, [u32; 64])>,
    revision: u64,
    last_probe: Option<u64>,
}
impl Feedback {
    fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.sample = None;
        self.last_probe = None;
    }

    fn compatible(a: Camera, b: Camera) -> bool {
        a.position.distance_squared(b.position) <= 8. * 8.
            && a.direction().dot(b.direction()) >= 0.95
            && a.fov_y == b.fov_y
    }

    fn poll(&mut self, camera: Camera, counters: &mut Counters) {
        let Some(pending) = &self.pending else { return };
        match pending.result.try_recv() {
            Ok(Ok(())) => {
                let start = std::time::Instant::now();
                if pending.revision == self.revision && Self::compatible(pending.camera, camera) {
                    let mapped = self.buffer.slice(..).get_mapped_range();
                    let mut counts = [0; 64];
                    counts.copy_from_slice(bytemuck::cast_slice(&mapped));
                    self.sample = Some((pending.camera, counts));
                    counters.feedback_reads += 1;
                } else {
                    counters.feedback_stale += 1;
                }
                self.buffer.unmap();
                self.pending = None;
                counters.feedback_cpu_ns += start.elapsed().as_nanos() as u64;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            _ => {
                self.pending = None;
            }
        }
    }
}

impl Cache {
    pub(super) fn new(
        device: &wgpu::Device,
        n: u32,
        region: Region,
        roots: &wgpu::TextureView,
    ) -> (Self, wgpu::TextureView) {
        let edge = edge(n);
        let texture = texture(
            device,
            edge,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | if cfg!(test) {
                    wgpu::TextureUsages::COPY_SRC
                } else {
                    wgpu::TextureUsages::empty()
                },
        );
        let view = texture.create_view(&Default::default());
        let scratch: [_; 2] = std::array::from_fn(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("NAADF distance ping pong"),
                size: SCRATCH_BYTES as u64,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("NAADF distance tile coordinates"),
            contents: &[0; UNIFORM_BYTES],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let feedback_counts = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("NAADF sampled ray demand counters"),
            size: FEEDBACK_BYTES,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let buffer = |ty| wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("NAADF root distance build resources"),
            entries: &[
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                },
                buffer(wgpu::BufferBindingType::Storage { read_only: true }),
                buffer(wgpu::BufferBindingType::Storage { read_only: false }),
                wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::R32Uint,
                    view_dimension: wgpu::TextureViewDimension::D3,
                },
                buffer(wgpu::BufferBindingType::Uniform),
                buffer(wgpu::BufferBindingType::Storage { read_only: false }),
            ]
            .into_iter()
            .enumerate()
            .map(|(binding, ty)| wgpu::BindGroupLayoutEntry {
                binding: binding as u32,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty,
                count: None,
            })
            .collect::<Vec<_>>(),
        });
        let groups = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("NAADF root distance tile"),
                layout: &layout,
                entries: &[
                    wgpu::BindingResource::TextureView(roots),
                    scratch[i].as_entire_binding(),
                    scratch[1 - i].as_entire_binding(),
                    wgpu::BindingResource::TextureView(&view),
                    uniform.as_entire_binding(),
                    feedback_counts.as_entire_binding(),
                ]
                .into_iter()
                .enumerate()
                .map(|(binding, resource)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource,
                })
                .collect::<Vec<_>>(),
            })
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("NAADF root distance build"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let mode = Mode::requested();
        let feedback = (mode == Mode::Feedback).then(|| {
            let shader = distance_shader(device);
            Feedback {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("NAADF sampled ray demand readback"),
                    size: FEEDBACK_BYTES,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                kernel: make_kernel(device, &pipeline_layout, &shader, "sample_feedback"),
                pending: None,
                sample: None,
                revision: 0,
                last_probe: None,
            }
        });
        (
            Self {
                plan: Plan::new(region, edge / SIDE),
                mode,
                feedback,
                feedback_counts,
                aspect: Cell::new(16. / 9.),
                counters: Counters::default(),
                armed: false,
                in_flight: Arc::new(AtomicBool::new(false)),
                uniform,
                _texture: texture,
                groups,
                layout: pipeline_layout,
                kernels: None,
            },
            view,
        )
    }

    pub(super) fn retarget(&mut self, region: Region) {
        let mut next = Plan::new(region, self.plan.side);
        if self.mode != Mode::Reset {
            next.retain(&self.plan);
        }
        self.counters.retained += u64::from(next.ready.count_ones());
        self.counters.invalidated += u64::from((self.plan.ready & !next.ready).count_ones());
        self.plan = next;
        if let Some(feedback) = &mut self.feedback {
            feedback.invalidate();
        }
        // Keep the old submission's fence. Queue order also makes retained
        // proofs safe if their previous write is still in flight.
        self.armed = false;
    }

    pub(super) fn progress(&self) -> (u32, u32) {
        (
            self.plan.ready.count_ones(),
            self.plan.tiles.iter().flatten().count() as u32,
        )
    }

    pub(super) fn uniforms(&self, aspect: f32) -> [u32; 4] {
        self.aspect.set(aspect);
        self.plan.uniforms()
    }

    pub(super) fn counters(&self) -> Counters {
        self.counters
    }

    fn sample_feedback(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        n: u32,
        ring: [u32; 3],
        camera: Camera,
    ) {
        let Some(feedback) = &mut self.feedback else {
            return;
        };
        if feedback.pending.is_some()
            || feedback
                .last_probe
                .is_some_and(|last| self.counters.submitted - last < 4)
        {
            return;
        }
        let start = std::time::Instant::now();
        let basis = super::ray_basis(camera, self.aspect.get());
        let eye = (camera.position - DVec3::from_array(self.plan.region.bounds()[0]))
            .as_vec3()
            .extend(camera.far)
            .to_array();
        let mut params = [0u32; UNIFORM_BYTES / 4];
        params[0] = n;
        params[4..7].copy_from_slice(&ring);
        params[8] = u32::from(cpu_fallback());
        params[12..16].copy_from_slice(&self.plan.uniforms());
        params[16..20].copy_from_slice(bytemuck::cast_slice(&eye));
        for i in 0..3 {
            params[20 + 4 * i..24 + 4 * i].copy_from_slice(bytemuck::cast_slice(&basis[i]));
        }
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&params));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("NAADF sampled ray feedback"),
        });
        encoder.clear_buffer(&self.feedback_counts, 0, None);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("NAADF sampled primary root rays"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&feedback.kernel);
            pass.set_bind_group(0, &self.groups[0], &[]);
            pass.dispatch_workgroups(2, 2, 1);
        }
        encoder.copy_buffer_to_buffer(
            &self.feedback_counts,
            0,
            &feedback.buffer,
            0,
            FEEDBACK_BYTES,
        );
        queue.submit(Some(encoder.finish()));
        let (tx, result) = mpsc::channel();
        feedback
            .buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |status| {
                let _ = tx.send(status);
            });
        feedback.pending = Some(PendingFeedback {
            result,
            camera,
            revision: feedback.revision,
        });
        feedback.last_probe = Some(self.counters.submitted);
        self.counters.feedback_dispatches += 1;
        self.counters.feedback_cpu_ns += start.elapsed().as_nanos() as u64;
    }

    pub(super) fn advance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        n: u32,
        ring: [u32; 3],
        camera: Camera,
    ) {
        if let Some(feedback) = &mut self.feedback {
            feedback.poll(camera, &mut self.counters);
        }
        // Give exact publication priority; unchanged tiles remain usable.
        if !self.armed {
            self.armed = true;
            return;
        }
        if self.in_flight.load(Ordering::Acquire) || self.progress().0 == self.progress().1 {
            return;
        }
        self.sample_feedback(device, queue, n, ring, camera);
        let (index, tile) = if matches!(self.mode, Mode::Rays | Mode::Feedback) {
            let start = std::time::Instant::now();
            let rays = Rays::new(camera, self.aspect.get());
            let measured = self
                .feedback
                .as_ref()
                .and_then(|f| f.sample.as_ref())
                .filter(|(pose, _)| Feedback::compatible(*pose, camera))
                .map(|(_, counts)| counts);
            let result = self.plan.candidate(Some(&rays), measured).unwrap();
            if measured.is_some_and(|counts| counts[result.0 as usize] > 0) {
                self.counters.feedback_prioritized += 1;
            }
            self.counters.ray_ranked += 1;
            self.counters.rank_cpu_ns += start.elapsed().as_nanos() as u64;
            result
        } else {
            self.plan.candidate(None, None).unwrap()
        };
        let side = self.plan.side;
        let slot = [index % side, index / side % side, index / side.pow(2)].map(|v| v * SIDE);
        let base: [u32; 3] =
            std::array::from_fn(|a| (tile.base[a] - self.plan.region.lo[a]) as i32 as u32);
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::cast_slice(&[
                n, slot[0], slot[1], slot[2], ring[0], ring[1], ring[2], 0, base[0], base[1],
                base[2], 0,
            ]),
        );
        let kernels = self.kernels.get_or_insert_with(|| {
            let shader = distance_shader(device);
            ["seed", "expand_x", "expand_y", "expand_z", "publish"]
                .map(|entry| make_kernel(device, &self.layout, &shader, entry))
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("NAADF bounded root distance tile"),
        });
        // One seed, five rounds of three-axis Jacobi, one publication. A
        // 32-cube has a maximum face span of 31 root cells.
        let mut ping = 0;
        for kernel in std::iter::once(0)
            .chain((0..5).flat_map(|_| 1..=3))
            .chain(std::iter::once(4))
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("NAADF distance stage"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&kernels[kernel]);
            pass.set_bind_group(0, &self.groups[ping], &[]);
            pass.dispatch_workgroups(SIDE / 4, SIDE / 4, SIDE / 4);
            ping ^= 1;
        }
        self.in_flight.store(true, Ordering::Release);
        queue.submit(Some(encoder.finish()));
        let in_flight = Arc::clone(&self.in_flight);
        queue.on_submitted_work_done(move || in_flight.store(false, Ordering::Release));
        // Later render submissions can use the tile immediately by queue order.
        self.plan.ready |= 1 << index;
        self.counters.submitted += 1;
    }
}

fn distance_shader(device: &wgpu::Device) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("NAADF root distance build"),
        source: wgpu::ShaderSource::Wgsl(include_str!("root_distance.wgsl").into()),
    })
}

fn make_kernel(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    entry: &str,
) -> wgpu::ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: Some(layout),
        module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn centered_rank_matches_traversal_shader_inverse() {
        for side in 1..=4 {
            for rank in 0..side {
                let p = super::centered(rank, side);
                assert!(p < side);
                let centre = side / 2;
                let inverse = if p < centre {
                    2 * (centre - p) - 1
                } else {
                    2 * (p - centre)
                };
                assert_eq!(rank, inverse);
            }
        }
    }

    #[test]
    fn retarget_keeps_only_identical_support_and_rejects_reused_slots() {
        use super::*;
        let region = |lo: Point| Region {
            lo,
            hi: lo.map(|x| x + 128),
        };
        let mut old = Plan::new(region([-64; 3]), 4);
        old.ready = u64::MAX;
        let mut shifted = Plan::new(region([-48, -64, -64]), 4);
        shifted.retain(&old);
        assert_eq!(shifted.ready.count_ones(), 48);
        for i in 0..64 {
            let before = old.tiles[i].unwrap();
            let after = shifted.tiles[i].unwrap();
            let retained = shifted.ready & (1u64 << i) != 0;
            assert_eq!(retained, before == after);
        }

        // The same world tile can be partially exposed in two windows with
        // different support. The old proof is not published for that slot.
        let mut clipped = Plan::new(region([-40, -64, -64]), 4);
        shifted.ready = u64::MAX;
        clipped.retain(&shifted);
        assert_eq!(clipped.ready.count_ones(), 48);
        assert!(clipped
            .tiles
            .iter()
            .zip(&shifted.tiles)
            .any(|(new, old)| new.unwrap().base == old.unwrap().base
                && new.unwrap().support != old.unwrap().support));

        let mut jumped = Plan::new(region([64, -64, -64]), 4);
        jumped.retain(&old);
        assert_eq!(jumped.ready, 0);
    }

    #[test]
    fn shader_uniform_mapping_selects_correct_world_tile_and_texel() {
        use super::*;
        for n in [8, 32, 40, 68, 96, 128, 254, 512] {
            let side = edge(n) / SIDE;
            for lo in [[-219, 4, 28], [91, -65, 78], [1000016, -320, 17]] {
                let region = Region {
                    lo,
                    hi: lo.map(|x| x + i64::from(n)),
                };
                let plan = Plan::new(region, side);
                let [_, _, packed_base, packed_slot] = plan.uniforms();
                assert_eq!(packed_slot >> 24, side);
                for (index, tile) in plan
                    .tiles
                    .iter()
                    .enumerate()
                    .filter_map(|(i, t)| t.map(|t| (i, t)))
                {
                    for cell in [tile.support.lo, tile.support.hi.map(|x| x - 1)] {
                        let slot: [u32; 3] = std::array::from_fn(|a| {
                            let byte = |word: u32| (word >> (a * 8)) as u8;
                            let base = i64::from(n.saturating_sub(MAX_EDGE) / 2)
                                + i64::from(byte(packed_base) as i8);
                            let local = (cell[a] - lo[a] - base) as u32;
                            assert!(local < side * SIDE);
                            assert_eq!(local % SIDE, (cell[a] - tile.base[a]) as u32);
                            (local / SIDE + u32::from(byte(packed_slot))) % side
                        });
                        assert_eq!(
                            index,
                            (slot[0] + side * (slot[1] + side * slot[2])) as usize
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn measured_demand_can_change_rank_and_never_requeues_ready_tiles() {
        use super::*;
        let mut plan = Plan::new(
            Region {
                lo: [-32; 3],
                hi: [32; 3],
            },
            2,
        );
        let camera = Camera {
            position: DVec3::new(-5., -5., -5.),
            yaw: 0.,
            pitch: 0.,
            far: 128.,
            fov_y: 60.,
        };
        let rays = Rays::new(camera, 16. / 9.);
        let visual = plan.candidate(Some(&rays), None).unwrap().0;
        let index = (visual + 1) % 8;
        let mut counts = [0; 64];
        counts[index as usize] = 12;
        assert_eq!(plan.candidate(Some(&rays), Some(&counts)).unwrap().0, index);
        plan.ready |= 1 << index;
        assert_eq!(
            plan.candidate(Some(&rays), Some(&counts)).unwrap().0,
            visual
        );
    }

    #[test]
    #[ignore = "explicit hardware GPU oracle: retained root proofs across physical tile and root ring wrap"]
    fn retained_gpu_tiles_remain_conservative_across_wraps() {
        use super::*;
        pollster::block_on(async {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                ..Default::default()
            });
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let (device, queue) = adapter
                .request_device(&Default::default(), None)
                .await
                .unwrap();
            let n = 64u32;
            let origin = [-32i64; 3];
            let roots = texture(
                &device,
                n,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            );
            let region_at = |lo: Point| Region {
                lo,
                hi: lo.map(|c| c + i64::from(n)),
            };
            let occupied = |[x, y, z]: Point| {
                x.rem_euclid(11) == 1 || y.rem_euclid(13) == 3 || z.rem_euclid(17) == 8
            };
            let (mut cache, _) = Cache::new(
                &device,
                n,
                region_at(origin),
                &roots.create_view(&Default::default()),
            );
            cache.mode = Mode::Retain;
            let extent = wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            };
            let layout = wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(n * 4),
                rows_per_image: Some(n),
            };
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("NAADF retained tile oracle"),
                size: u64::from(n.pow(3)) * 4,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let camera = Camera::fixture();
            let verify = |cache: &Cache| {
                let mut encoder = device.create_command_encoder(&Default::default());
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &cache._texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &readback,
                        layout,
                    },
                    extent,
                );
                queue.submit(Some(encoder.finish()));
                let (tx, rx) = mpsc::channel();
                readback
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
                device.poll(wgpu::Maintain::Wait);
                rx.recv().unwrap().unwrap();
                let view = readback.slice(..).get_mapped_range();
                let words: &[u32] = bytemuck::cast_slice(&view);
                for (index, tile) in cache.plan.tiles.iter().enumerate() {
                    if cache.plan.ready & (1 << index) == 0 {
                        continue;
                    }
                    let tile = tile.unwrap();
                    let slot = [index % 2, index / 2 % 2, index / 4].map(|x| x * 32);
                    let Region { lo, hi } = tile.support;
                    for z in (lo[2]..hi[2]).step_by(7) {
                        for y in (lo[1]..hi[1]).step_by(5) {
                            for x in (lo[0]..hi[0]).step_by(3) {
                                let p = [x, y, z];
                                let q: [usize; 3] = std::array::from_fn(|a| {
                                    slot[a] + (p[a] - tile.base[a]) as usize
                                });
                                let word = words[q[0] + n as usize * (q[1] + n as usize * q[2])];
                                if occupied(p) {
                                    assert_eq!(word, 0, "occupied root {p:?}");
                                    continue;
                                }
                                let a: [i64; 3] = std::array::from_fn(|a| {
                                    p[a] - i64::from((word >> (a * 10)) & 31)
                                });
                                let b: [i64; 3] = std::array::from_fn(|a| {
                                    p[a] + i64::from((word >> (a * 10 + 5)) & 31)
                                });
                                assert!(
                                    (0..3).all(|axis| a[axis] >= lo[axis] && b[axis] < hi[axis]),
                                    "proof {a:?}..={b:?} escapes {tile:?}"
                                );
                                for z in a[2]..=b[2] {
                                    for y in a[1]..=b[1] {
                                        for x in a[0]..=b[0] {
                                            assert!(!occupied([x, y, z]), "stale proof at {p:?}");
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                drop(view);
                readback.unmap();
            };
            for (step, lo) in [
                origin,
                [-16, -32, -32],
                [0, -32, -32],
                [16, -32, -16],
                [32, -32, -16],
                [48, -32, -16],
            ]
            .into_iter()
            .enumerate()
            {
                let ring: [u32; 3] =
                    std::array::from_fn(|a| (lo[a] - origin[a]).rem_euclid(i64::from(n)) as u32);
                let mut words = vec![0u32; n.pow(3) as usize];
                for z in 0..n {
                    for x in 0..n {
                        for y in 0..n {
                            let p: Point = std::array::from_fn(|a| lo[a] + i64::from([x, y, z][a]));
                            let [rx, ry, rz] =
                                [(x + ring[0]) % n, (y + ring[1]) % n, (z + ring[2]) % n];
                            words[(ry + n * (rx + n * rz)) as usize] =
                                if occupied(p) { 1 << 30 } else { 0 };
                        }
                    }
                }
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &roots,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytemuck::cast_slice(&words),
                    layout,
                    extent,
                );
                queue.submit(std::iter::empty());
                if step > 0 {
                    cache.retarget(region_at(lo));
                    let (ready, total) = cache.progress();
                    assert!(ready > 0 && ready < total);
                    verify(&cache);
                }
                while cache.progress().0 < cache.progress().1 {
                    cache.advance(&device, &queue, n, ring, camera);
                    device.poll(wgpu::Maintain::Wait);
                }
                verify(&cache);
            }
        });
    }

    #[test]
    fn compute_shader_validates() {
        let shader = naga::front::wgsl::parse_str(include_str!("root_distance.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&shader)
        .unwrap();
    }
}
