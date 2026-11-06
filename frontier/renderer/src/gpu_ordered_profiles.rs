//! Opt-in experiment: construct draw-compatible 80-byte appearance profiles on
//! the GPU from immutable canonical columns already held by a CPU cache.
//!
//! Ordered retains baseline float operation order; ParallelCounts reduces exact
//! integer face/material histograms then forms palette-weighted sums. GPU float
//! arithmetic and changed accumulation need measured parity; neither is promoted.
//! No production renderer calls this module. No source noise is evaluated here.
use crate::column_mesh::SampledColumn;
use frontier_world::{FbmParams, World};
use std::{sync::mpsc, time::Instant};

pub const WIDTH: usize = 128;
const HALO_SIDE: usize = WIDTH + 2;
pub const MAX_PAGES: usize = 8;
pub const MAX_JOBS: usize = 512;
pub type FaceProfile = [[f32; 4]; 5];
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Column {
    height: i32,
    deep: u32,
    soil: u32,
    top: u32,
}
/// Every page includes one canonical axial halo and belongs to one immutable
/// source snapshot. Missing cache entries fail instead of generating terrain.
pub struct CachedPage {
    columns: Vec<Column>,
    pub min: [i64; 2],
    seed: u64,
    params: FbmParams,
    revision: u64,
}
impl CachedPage {
    pub fn from_cache(
        world: &World,
        min: [i64; 2],
        mut lookup: impl FnMut(i64, i64) -> Option<SampledColumn>,
    ) -> Result<Self, String> {
        let params = world
            .fbm_terrain()
            .ok_or("ordered profiles require FBM")?
            .params();
        if world.topology_base_cells().is_some()
            || world.revision() != 0
            || world.edit_cells().next().is_some()
        {
            return Err("ordered profiles require unedited heightfield ownership".into());
        }
        for p in min {
            if (p as i128).abs() + WIDTH as i128 + 1 >= 1_i128 << 53 {
                return Err("ordered profile canonical address range".into());
            }
        }
        let mut samples = Vec::with_capacity(HALO_SIDE * HALO_SIDE);
        let (mut low, mut high) = (i64::MAX, i64::MIN);
        for z in -1..=WIDTH as i64 {
            for x in -1..=WIDTH as i64 {
                let c = lookup(min[0] + x, min[1] + z)
                    .ok_or("ordered profile missing cached canonical column")?;
                if c.bands.iter().any(|m| *m as u32 > 4 || *m as u32 == 0) {
                    return Err("ordered profile columns require solid canonical bands".into());
                }
                low = low.min(c.height);
                high = high.max(c.height);
                samples.push(c);
            }
        }
        // Protect all shader h-12 and hi-lo arithmetic and keep all face-area
        // totals <=2^24, so alpha accumulation is exact even at full-page size.
        if high as i128 - low as i128 > 1024 {
            return Err("ordered profile relief exceeds exact-area experiment bound".into());
        }
        let columns = samples
            .into_iter()
            .map(|c| Column {
                height: (c.height as i128 - low as i128) as i32,
                deep: c.bands[2] as u32,
                soil: c.bands[1] as u32,
                top: c.bands[0] as u32,
            })
            .collect();
        Ok(Self {
            columns,
            min,
            seed: world.seed(),
            params,
            revision: world.revision(),
        })
    }
    pub fn payload_bytes(&self) -> usize {
        self.columns.len() * std::mem::size_of::<Column>()
    }
}
/// Half-open local rectangle. Stride weights match the adaptive CPU painter;
/// native profiles use stride one. Descriptors never alter canonical columns.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ProfileJob {
    page: u32,
    x: u32,
    z: u32,
    width: u32,
    depth: u32,
    stride: u32,
    pad0: u32,
    pad1: u32,
}
impl ProfileJob {
    pub fn new(page: usize, min: [u32; 2], size: [u32; 2], stride: u32) -> Result<Self, String> {
        if page >= MAX_PAGES
            || stride == 0
            || size.contains(&0)
            || size.iter().any(|s| *s > WIDTH as u32 || *s % stride != 0)
            || (0..2).any(|i| min[i].checked_add(size[i]).is_none_or(|n| n > WIDTH as u32))
        {
            return Err("ordered profile descriptor exceeds bounded page".into());
        }
        Ok(Self {
            page: page as u32,
            x: min[0],
            z: min[1],
            width: size[0],
            depth: size[1],
            stride,
            pad0: 0,
            pad1: 0,
        })
    }
}
fn validate(pages: &[CachedPage], jobs: &[ProfileJob]) -> Result<(), String> {
    if pages.is_empty() || pages.len() > MAX_PAGES || jobs.is_empty() || jobs.len() > MAX_JOBS {
        return Err("ordered profile batch must contain 1..8 pages and 1..512 jobs".into());
    }
    let first = &pages[0];
    if pages
        .iter()
        .any(|p| p.seed != first.seed || p.params != first.params || p.revision != first.revision)
        || jobs.iter().any(|j| j.page as usize >= pages.len())
    {
        return Err("ordered profile batch snapshot/page mismatch".into());
    }
    Ok(())
}
/// Ordered baseline oracle. Does not quantize colors or merge child sums.
pub fn cpu_profiles(pages: &[CachedPage], jobs: &[ProfileJob]) -> Result<Vec<FaceProfile>, String> {
    validate(pages, jobs)?;
    let colors = [
        frontier_world::Material::Air,
        frontier_world::Material::Grass,
        frontier_world::Material::Soil,
        frontier_world::Material::Stone,
        frontier_world::Material::Wood,
    ]
    .map(|m| m.color());
    Ok(jobs
        .iter()
        .map(|j| {
            let page = &pages[j.page as usize];
            let mut p = [[0.; 4]; 5];
            let weight = (j.stride * j.stride) as f32;
            let mut add = |axis: usize, material: u32, cells: i32| {
                let a = cells as f32 * weight;
                for (channel, c) in colors[material as usize].into_iter().enumerate() {
                    p[axis][channel] += c * a;
                }
                p[axis][3] += a;
            };
            for z in (j.z..j.z + j.depth).step_by(j.stride as usize) {
                for x in (j.x..j.x + j.width).step_by(j.stride as usize) {
                    let i = (z as usize + 1) * HALO_SIDE + x as usize + 1;
                    let c = page.columns[i];
                    add(0, c.top, 1);
                    for (axis, k) in [i + 1, i - 1, i + HALO_SIDE, i - HALO_SIDE]
                        .into_iter()
                        .enumerate()
                    {
                        let mut lo = page.columns[k].height;
                        for hi in [c.height - 12, c.height - 1, c.height] {
                            if hi > lo {
                                let m = if lo == c.height - 1 {
                                    c.top
                                } else if lo >= c.height - 12 {
                                    c.soil
                                } else {
                                    c.deep
                                };
                                add(axis + 1, m, hi - lo);
                                lo = hi;
                            }
                        }
                    }
                }
            }
            p
        })
        .collect())
}
/// Independent exact integer face-count oracle with f64 palette accumulation.
/// Uses clipped span lengths rather than the ordered painter's running-lo loop.
pub fn f64_count_profiles(
    pages: &[CachedPage],
    jobs: &[ProfileJob],
) -> Result<Vec<[[f64; 4]; 5]>, String> {
    validate(pages, jobs)?;
    let colors = [
        frontier_world::Material::Air,
        frontier_world::Material::Grass,
        frontier_world::Material::Soil,
        frontier_world::Material::Stone,
        frontier_world::Material::Wood,
    ]
    .map(|m| m.color());
    Ok(jobs
        .iter()
        .map(|j| {
            let page = &pages[j.page as usize];
            let mut counts = [[0_u64; 5]; 5];
            let weight = (j.stride * j.stride) as u64;
            for z in (j.z..j.z + j.depth).step_by(j.stride as usize) {
                for x in (j.x..j.x + j.width).step_by(j.stride as usize) {
                    let i = (z as usize + 1) * HALO_SIDE + x as usize + 1;
                    let c = page.columns[i];
                    counts[0][c.top as usize] += weight;
                    for (axis, k) in [i + 1, i - 1, i + HALO_SIDE, i - HALO_SIDE]
                        .into_iter()
                        .enumerate()
                    {
                        let lo = page.columns[k].height;
                        for (lower, upper, material) in [
                            (lo, c.height - 12, c.deep),
                            (lo.max(c.height - 12), c.height - 1, c.soil),
                            (lo.max(c.height - 1), c.height, c.top),
                        ] {
                            counts[axis + 1][material as usize] +=
                                (upper - lower).max(0) as u64 * weight;
                        }
                    }
                }
            }
            let mut result = [[0_f64; 4]; 5];
            for axis in 0..5 {
                for (material, count) in counts[axis].into_iter().enumerate() {
                    for channel in 0..3 {
                        result[axis][channel] += count as f64 * colors[material][channel] as f64;
                    }
                    result[axis][3] += count as f64;
                }
            }
            result
        })
        .collect())
}
/// Ordered remains the default. ParallelCounts computes exact integer areas
/// then palette sums; it does not promise ordered CPU floating-point bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKernel {
    Ordered,
    ParallelCounts,
}
impl ProfileKernel {
    fn shader(self) -> &'static str {
        match self {
            Self::Ordered => include_str!("gpu_ordered_profiles.wgsl"),
            Self::ParallelCounts => include_str!("gpu_parallel_profiles.wgsl"),
        }
    }
    fn workgroups(self, jobs: usize) -> u32 {
        match self {
            Self::Ordered => (jobs as u32).div_ceil(64),
            Self::ParallelCounts => jobs as u32,
        }
    }
}
#[derive(Debug, Default, Clone, Copy)]
pub struct Timings {
    pub encode_submit_ms: f64,
    pub wait_readback_ms: f64,
    pub gpu_ms: Option<f64>,
}
/// Fixed bounded allocation. Output slot zero is initialized to zero. Output is
/// STORAGE and may bind directly to adaptive.wgsl's face_profiles binding; no
/// readback or CPU color expansion is required by encode_resident.
pub struct GpuOrderedProfiles {
    columns: wgpu::Buffer,
    jobs: wgpu::Buffer,
    profiles: wgpu::Buffer,
    readback: wgpu::Buffer,
    pipeline: wgpu::ComputePipeline,
    kernel: ProfileKernel,
    bind: Option<wgpu::BindGroup>,
    loaded_jobs: usize,
    timestamps: Option<(wgpu::QuerySet, wgpu::Buffer, wgpu::Buffer)>,
}
impl GpuOrderedProfiles {
    pub fn new(device: &wgpu::Device) -> Self {
        Self::new_with_kernel(device, ProfileKernel::Ordered)
    }
    pub fn new_with_kernel(device: &wgpu::Device, kernel: ProfileKernel) -> Self {
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let columns = buffer(
            "ordered cached columns",
            (MAX_PAGES * HALO_SIDE * HALO_SIDE * 16) as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let jobs = buffer(
            "ordered profile jobs",
            (MAX_JOBS * 32) as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let profiles = buffer(
            "draw-compatible ordered profiles",
            ((MAX_JOBS + 1) * 80) as u64,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );
        let readback = buffer(
            "optional ordered profile oracle readback",
            ((MAX_JOBS + 1) * 80) as u64,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ordered canonical profile construction"),
            source: wgpu::ShaderSource::Wgsl(kernel.shader().into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("ordered profiles"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let timestamps = device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| {
                (
                    device.create_query_set(&wgpu::QuerySetDescriptor {
                        label: Some("ordered profile timestamps"),
                        ty: wgpu::QueryType::Timestamp,
                        count: 2,
                    }),
                    buffer(
                        "ordered profile resolve",
                        16,
                        wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    ),
                    buffer(
                        "ordered profile timestamp readback",
                        16,
                        wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    ),
                )
            });
        Self {
            columns,
            jobs,
            profiles,
            readback,
            pipeline,
            kernel,
            bind: None,
            loaded_jobs: 0,
            timestamps,
        }
    }
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pages: &[CachedPage],
        jobs: &[ProfileJob],
    ) -> Result<f64, String> {
        validate(pages, jobs)?;
        let started = Instant::now();
        for (i, page) in pages.iter().enumerate() {
            queue.write_buffer(
                &self.columns,
                (i * HALO_SIDE * HALO_SIDE * 16) as u64,
                bytemuck::cast_slice(&page.columns),
            );
        }
        queue.write_buffer(&self.jobs, 0, bytemuck::cast_slice(jobs));
        queue.write_buffer(&self.profiles, 0, &[0; 80]);
        // Limit jobs binding length: padded lanes cannot observe stale descriptors.
        self.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ordered profile batch"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.columns.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.jobs,
                        offset: 0,
                        size: wgpu::BufferSize::new((jobs.len() * 32) as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.profiles.as_entire_binding(),
                },
            ],
        }));
        self.loaded_jobs = jobs.len();
        Ok(started.elapsed().as_secs_f64() * 1000.)
    }
    pub fn profile_buffer(&self) -> &wgpu::Buffer {
        &self.profiles
    }
    pub fn loaded_profiles(&self) -> usize {
        self.loaded_jobs
    }
    /// Logical fixed GPU allocation, including diagnostic readbacks, excluding
    /// driver allocations, pipeline state and queue-upload staging.
    pub fn resident_bytes(&self) -> u64 {
        (MAX_PAGES * HALO_SIDE * HALO_SIDE * 16 + MAX_JOBS * 32 + 2 * (MAX_JOBS + 1) * 80) as u64
            + if self.timestamps.is_some() { 32 } else { 0 }
    }
    pub fn encode_resident(&self, encoder: &mut wgpu::CommandEncoder) -> Result<(), String> {
        self.encode(encoder, false)
    }
    fn encode(&self, encoder: &mut wgpu::CommandEncoder, timed: bool) -> Result<(), String> {
        let bind = self
            .bind
            .as_ref()
            .ok_or("ordered profiles have no uploaded batch")?;
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ordered resident appearance construction"),
            timestamp_writes: if timed {
                self.timestamps
                    .as_ref()
                    .map(|(q, _, _)| wgpu::ComputePassTimestampWrites {
                        query_set: q,
                        beginning_of_pass_write_index: Some(0),
                        end_of_pass_write_index: Some(1),
                    })
            } else {
                None
            },
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.dispatch_workgroups(self.kernel.workgroups(self.loaded_jobs), 1, 1);
        Ok(())
    }
    /// Explicit standalone diagnostic. Never call in the render loop.
    pub fn run_blocking(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<(Vec<FaceProfile>, Timings), String> {
        let started = Instant::now();
        let bytes = ((self.loaded_jobs + 1) * 80) as u64;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ordered profile diagnostic"),
        });
        self.encode(&mut encoder, true)?;
        encoder.copy_buffer_to_buffer(&self.profiles, 0, &self.readback, 0, bytes);
        if let Some((q, resolve, readback)) = &self.timestamps {
            encoder.resolve_query_set(q, 0..2, resolve, 0);
            encoder.copy_buffer_to_buffer(resolve, 0, readback, 0, 16);
        }
        queue.submit(Some(encoder.finish()));
        let encode_submit_ms = started.elapsed().as_secs_f64() * 1000.;
        let wait = Instant::now();
        let map = |buffer: &wgpu::Buffer| {
            let (tx, rx) = mpsc::channel();
            buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            rx
        };
        let rx = map(&self.readback);
        let timing_rx = self.timestamps.as_ref().map(|(_, _, b)| map(b));
        device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let data = self.readback.slice(..).get_mapped_range();
        if data[..80].iter().any(|b| *b != 0) {
            return Err("ordered profile exact-face fallback changed".into());
        }
        let mut result = Vec::with_capacity(self.loaded_jobs);
        for record in data[80..bytes as usize].chunks_exact(80) {
            let mut p = [[0.; 4]; 5];
            for (v, b) in p.iter_mut().flatten().zip(record.chunks_exact(4)) {
                *v = f32::from_le_bytes(b.try_into().unwrap());
            }
            result.push(p);
        }
        drop(data);
        self.readback.unmap();
        let gpu_ms = if let Some(rx) = timing_rx {
            rx.recv()
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            let b = &self.timestamps.as_ref().unwrap().2;
            let data = b.slice(..).get_mapped_range();
            let a = u64::from_le_bytes(data[..8].try_into().unwrap());
            let z = u64::from_le_bytes(data[8..16].try_into().unwrap());
            drop(data);
            b.unmap();
            Some(z.saturating_sub(a) as f64 * queue.get_timestamp_period() as f64 / 1_000_000.)
        } else {
            None
        };
        Ok((
            result,
            Timings {
                encode_submit_ms,
                wait_readback_ms: wait.elapsed().as_secs_f64() * 1000.,
                gpu_ms,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::{FbmRecipe, Material};
    fn synthetic(flat: bool) -> (World, CachedPage) {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let page = CachedPage::from_cache(&world, [-257, -513], |x, z| {
            Some(SampledColumn {
                height: if flat {
                    -300
                } else {
                    -300 + ((x * 17 + z * 31).rem_euclid(1025))
                },
                bands: [
                    if (x + z).rem_euclid(2) == 0 {
                        Material::Grass
                    } else {
                        Material::Wood
                    },
                    Material::Soil,
                    Material::Stone,
                ],
            })
        })
        .unwrap();
        (world, page)
    }
    #[test]
    fn shader_and_draw_profile_abi_validate() {
        assert_eq!(std::mem::size_of::<Column>(), 16);
        assert_eq!(std::mem::size_of::<ProfileJob>(), 32);
        assert_eq!(std::mem::size_of::<FaceProfile>(), 80);
        for kernel in [ProfileKernel::Ordered, ProfileKernel::ParallelCounts] {
            let module = naga::front::wgsl::parse_str(kernel.shader()).unwrap();
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .unwrap();
        }
        assert_eq!(ProfileKernel::Ordered.workgroups(65), 2);
        assert_eq!(ProfileKernel::ParallelCounts.workgroups(65), 65);
    }
    #[test]
    fn cached_only_input_rejects_misses_ownership_and_bad_descriptors() {
        let (world, _) = synthetic(true);
        let mut calls = 0;
        assert!(CachedPage::from_cache(&world, [0, 0], |_, _| {
            calls += 1;
            None
        })
        .is_err());
        assert_eq!(calls, 1);
        assert!(CachedPage::from_cache(&world, [i64::MAX, 0], |_, _| panic!(
            "invalid address must fail before lookup"
        ))
        .is_err());
        let topology = World::fbm_topology(7, FbmParams::default()).unwrap();
        assert!(CachedPage::from_cache(&topology, [0, 0], |_, _| panic!(
            "owner must fail before lookup"
        ))
        .is_err());
        assert!(ProfileJob::new(MAX_PAGES, [0, 0], [16, 16], 1).is_err());
        for (min, size, stride) in [
            ([0, 0], [0, 16], 1),
            ([127, 0], [2, 16], 1),
            ([0, 0], [16, 16], 0),
            ([0, 0], [16, 16], 3),
            ([u32::MAX, 0], [1, 1], 1),
        ] {
            assert!(ProfileJob::new(0, min, size, stride).is_err());
        }
    }
    #[test]
    fn cache_reads_each_column_once_and_flat_sides_stay_zero() {
        let (world, _) = synthetic(true);
        let mut reads = 0;
        let p = CachedPage::from_cache(&world, [-128, -128], |_, _| {
            reads += 1;
            Some(SampledColumn {
                height: i64::MIN,
                bands: [Material::Grass, Material::Soil, Material::Stone],
            })
        })
        .unwrap();
        assert_eq!(reads, HALO_SIDE * HALO_SIDE);
        assert_eq!(p.payload_bytes(), 270400);
        for stride in [1, 2, 4, 8, 16] {
            let j = ProfileJob::new(0, [0, 0], [16, 16], stride).unwrap();
            let p = cpu_profiles(std::slice::from_ref(&p), &[j]).unwrap()[0];
            assert_eq!(p[0][3], 256.);
            assert_eq!(&p[1..], &[[0.; 4]; 4]);
        }
    }
    #[test]
    fn ordered_profiles_match_direct_material_span_oracle_and_recipe_data() {
        for recipe in [
            FbmRecipe::Legacy,
            FbmRecipe::Regional,
            FbmRecipe::RegionalUnwarped,
        ] {
            let world = World::fbm(
                7,
                FbmParams {
                    recipe,
                    horizontal_scale: 128.,
                    amplitude: 20.,
                    ..Default::default()
                },
            )
            .unwrap();
            let min = [-273, -529];
            let page = CachedPage::from_cache(&world, min, |x, z| {
                Some(SampledColumn::canonical(&world, x, z))
            })
            .unwrap();
            for stride in [1, 2, 4] {
                let j = ProfileJob::new(0, [3, 5], [16, 16], stride).unwrap();
                let got = cpu_profiles(std::slice::from_ref(&page), &[j]).unwrap()[0];
                let mut expected = [[0.; 4]; 5];
                let weight = (stride * stride) as f32;
                for z in (min[1] + 5..min[1] + 21).step_by(stride as usize) {
                    for x in (min[0] + 3..min[0] + 19).step_by(stride as usize) {
                        let c = SampledColumn::canonical(&world, x, z);
                        let h = c.height;
                        let mut add = |axis: usize, n: i64, m: Material| {
                            let a = n as f32 * weight;
                            for (k, color) in m.color().into_iter().enumerate() {
                                expected[axis][k] += color * a;
                            }
                            expected[axis][3] += a;
                        };
                        add(
                            0,
                            1,
                            world.material(frontier_world::Cell { x, y: h - 1, z }),
                        );
                        for (axis, dx, dz) in [(1, 1, 0), (2, -1, 0), (3, 0, 1), (4, 0, -1)] {
                            let mut lo = SampledColumn::canonical(&world, x + dx, z + dz).height;
                            for hi in [h - 12, h - 1, h] {
                                if hi > lo {
                                    add(
                                        axis,
                                        hi - lo,
                                        world.material(frontier_world::Cell { x, y: lo, z }),
                                    );
                                    lo = hi;
                                }
                            }
                        }
                    }
                }
                assert_eq!(
                    bytemuck::cast_slice::<_, u8>(&got),
                    bytemuck::cast_slice::<_, u8>(&expected)
                );
            }
        }
        let (_, page) = synthetic(false);
        let j = ProfileJob::new(0, [0, 0], [128, 128], 1).unwrap();
        let got = cpu_profiles(&[page], &[j]).unwrap()[0];
        assert_eq!(got[0][3], 16384.);
        assert!(got[1..].iter().any(|p| p[3] > 0.));
    }
    #[test]
    fn count_oracle_preserves_exact_areas_and_bounds_ordered_roundoff() {
        for flat in [false, true] {
            let (_, page) = synthetic(flat);
            let jobs = [
                ProfileJob::new(0, [0, 0], [128, 128], 1).unwrap(),
                ProfileJob::new(0, [3, 5], [16, 16], 2).unwrap(),
            ];
            let ordered = cpu_profiles(std::slice::from_ref(&page), &jobs).unwrap();
            let oracle = f64_count_profiles(std::slice::from_ref(&page), &jobs).unwrap();
            let rounded: Vec<FaceProfile> = oracle
                .iter()
                .map(|p| p.map(|axis| axis.map(|v| v as f32)))
                .collect();
            compare(ProfileKernel::ParallelCounts, &rounded, &ordered, &oracle);
            assert_eq!(oracle[0][0][3], 16384.);
            assert_eq!(oracle[1][0][3], 256.);
            if flat {
                assert!(oracle
                    .iter()
                    .all(|p| p[1..].iter().flatten().all(|v| *v == 0.)));
            }
        }
        // Reject a batch spanning canonical source identities before GPU upload.
        let (_, first) = synthetic(true);
        let (_, mut other) = synthetic(true);
        other.seed += 1;
        let job = ProfileJob::new(0, [0, 0], [16, 16], 1).unwrap();
        assert!(f64_count_profiles(&[first, other], &[job]).is_err());
    }
    #[derive(Debug, Default)]
    struct Parity {
        bit_mismatched_channels: usize,
        max_ordered_rgb_relative: f64,
        max_ordered_mean_absolute: f64,
        max_oracle_rgb_relative: f64,
        max_oracle_mean_absolute: f64,
    }
    // Normalize RGB area sums by exact axis area to measure material-color drift.
    fn compare(
        kernel: ProfileKernel,
        actual: &[FaceProfile],
        ordered: &[FaceProfile],
        oracle: &[[[f64; 4]; 5]],
    ) -> Parity {
        assert_eq!(actual.len(), ordered.len());
        assert_eq!(actual.len(), oracle.len());
        let mut metrics = Parity::default();
        for ((a, e), o) in actual.iter().zip(ordered).zip(oracle) {
            for axis in 0..5 {
                assert_eq!(a[axis][3], e[axis][3], "GPU/CPU face area");
                assert_eq!(a[axis][3] as f64, o[axis][3], "GPU/exact-count face area");
                let area = o[axis][3].max(1.);
                for channel in 0..4 {
                    let (a, e, o) = (a[axis][channel], e[axis][channel], o[axis][channel]);
                    assert!(a.is_finite());
                    if a.to_bits() != e.to_bits() {
                        metrics.bit_mismatched_channels += 1;
                    }
                    if channel == 3 {
                        continue;
                    }
                    metrics.max_ordered_rgb_relative = metrics
                        .max_ordered_rgb_relative
                        .max((a as f64 - e as f64).abs() / (e as f64).abs().max(1.));
                    metrics.max_ordered_mean_absolute = metrics
                        .max_ordered_mean_absolute
                        .max((a as f64 - e as f64).abs() / area);
                    metrics.max_oracle_rgb_relative = metrics
                        .max_oracle_rgb_relative
                        .max((a as f64 - o).abs() / o.abs().max(1.));
                    metrics.max_oracle_mean_absolute = metrics
                        .max_oracle_mean_absolute
                        .max((a as f64 - o).abs() / area);
                }
            }
        }
        // Exact area is mandatory. CPU agreement permits accumulated rounding;
        // histogram accuracy is judged more tightly against the f64/count oracle.
        assert!(
            metrics.max_ordered_rgb_relative <= 0.0005,
            "ordered agreement {metrics:?}"
        );
        assert!(
            metrics.max_ordered_mean_absolute <= 0.0005,
            "normalized ordered agreement {metrics:?}"
        );
        let oracle_limit = if kernel == ProfileKernel::ParallelCounts {
            0.000002
        } else {
            0.0005
        };
        assert!(
            metrics.max_oracle_rgb_relative <= oracle_limit,
            "count-oracle accuracy {metrics:?}"
        );
        assert!(
            metrics.max_oracle_mean_absolute <= oracle_limit,
            "normalized count-oracle accuracy {metrics:?}"
        );
        metrics
    }
    #[test]
    #[ignore = "manual GPU experiment; coordinate hardware authorization"]
    fn measure_gpu_ordered_profiles() {
        pollster::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("adapter");
            let (device, queue) = adapter
                .request_device(
                    &wgpu::DeviceDescriptor {
                        label: Some("bounded ordered profile probe"),
                        required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
                        required_limits: wgpu::Limits::default(),
                        memory_hints: wgpu::MemoryHints::MemoryUsage,
                    },
                    None,
                )
                .await
                .unwrap();
            println!("ordered-profile adapter={:?}", adapter.get_info());
            let primary_params = FbmParams {
                recipe: FbmRecipe::Legacy,
                horizontal_scale: 1800.,
                amplitude: 1000.,
                octaves: 8,
                roughness: 0.5,
            };
            println!("ordered-profile source=primary_legacy seed=7 params={primary_params:?} voxel_m=0.0625 page_width_cells=128 halo_cells=1 relief_limit_cells=1024 source_revision=0");
            println!("ordered-profile acceptance exact_area=true ordered_rgb_relative_limit=0.0005 ordered_mean_absolute_limit=0.0005 parallel_f64_oracle_rgb_relative_limit=0.000002 parallel_f64_oracle_mean_absolute_limit=0.000002 visual_approval=false");
            for n in [1, 4, 8] {
                let start = Instant::now();
                let world = World::fbm(7, primary_params).unwrap();
                // This benchmark explicitly charges cold canonical source work.
                // A runtime integration must use its revision-keyed source cache.
                let pages: Vec<_> = (0..n)
                    .map(|i| {
                        let min = [(i as i64 - 4) * 128, -512];
                        let (mut low, mut high) = (i64::MAX, i64::MIN);
                        let result = CachedPage::from_cache(&world, min, |x, z| {
                            let column = SampledColumn::canonical(&world, x, z);
                            low = low.min(column.height);
                            high = high.max(column.height);
                            Some(column)
                        });
                        println!("ordered-profile source_page batch_pages={n} page={i} min_cells={min:?} height_min_cells={low} height_max_cells={high} relief_cells={} accepted={}", high as i128-low as i128, result.is_ok());
                        result.unwrap_or_else(|error| panic!("primary Legacy ordered-profile page {min:?} rejected: {error}; experiment bound remains1024cells"))
                    })
                    .collect();
                let source_prepare_ms = start.elapsed().as_secs_f64() * 1000.;
                let mut jobs = vec![];
                for page in 0..n {
                    for z in (0..128).step_by(16) {
                        for x in (0..128).step_by(16) {
                            jobs.push(ProfileJob::new(page, [x, z], [16, 16], 1).unwrap());
                        }
                    }
                }
                let start = Instant::now();
                let expected = cpu_profiles(&pages, &jobs).unwrap();
                let cpu_profile_ms = start.elapsed().as_secs_f64() * 1000.;
                let start = Instant::now();
                let oracle = f64_count_profiles(&pages, &jobs).unwrap();
                let cpu_count_oracle_ms = start.elapsed().as_secs_f64() * 1000.;
                for kernel in [ProfileKernel::Ordered, ProfileKernel::ParallelCounts] {
                    let start = Instant::now();
                    let mut builder = GpuOrderedProfiles::new_with_kernel(&device, kernel);
                    let pipeline_allocation_ms = start.elapsed().as_secs_f64() * 1000.;
                    let upload_enqueue_ms = builder.upload(&device, &queue, &pages, &jobs).unwrap();
                    let (actual, cold) = builder.run_blocking(&device, &queue).unwrap();
                    let parity = compare(kernel, &actual, &expected, &oracle);
                    println!("ordered-profile kernel={kernel:?} pages={n} jobs={} source_prepare_ms={source_prepare_ms:.3} cpu_profile_ms={cpu_profile_ms:.3} cpu_count_oracle_ms={cpu_count_oracle_ms:.3} pipeline_allocation_ms={pipeline_allocation_ms:.3} upload_enqueue_ms={upload_enqueue_ms:.3} resident_bytes={} cold={cold:?} parity={parity:?}",jobs.len(),builder.resident_bytes());
                    for repeat in 0..5 {
                        let (actual, timing) = builder.run_blocking(&device, &queue).unwrap();
                        println!("ordered-profile kernel={kernel:?} pages={n} resident_repeat={repeat} timing={timing:?} parity={:?}",compare(kernel,&actual,&expected,&oracle));
                    }
                }
            }
            // Exercise high relief, non-terrain Wood tops, and all-zero side area.
            for flat in [false, true] {
                let (_, page) = synthetic(flat);
                let jobs = [
                    ProfileJob::new(0, [0, 0], [128, 128], 1).unwrap(),
                    ProfileJob::new(0, [3, 5], [16, 16], 2).unwrap(),
                ];
                let start = Instant::now();
                let expected = cpu_profiles(std::slice::from_ref(&page), &jobs).unwrap();
                let cpu_profile_ms = start.elapsed().as_secs_f64() * 1000.;
                let start = Instant::now();
                let oracle = f64_count_profiles(std::slice::from_ref(&page), &jobs).unwrap();
                let cpu_count_oracle_ms = start.elapsed().as_secs_f64() * 1000.;
                for kernel in [ProfileKernel::Ordered, ProfileKernel::ParallelCounts] {
                    let start = Instant::now();
                    let mut builder = GpuOrderedProfiles::new_with_kernel(&device, kernel);
                    let pipeline_allocation_ms = start.elapsed().as_secs_f64() * 1000.;
                    let upload_enqueue_ms = builder
                        .upload(&device, &queue, std::slice::from_ref(&page), &jobs)
                        .unwrap();
                    let (actual, cold) = builder.run_blocking(&device, &queue).unwrap();
                    println!("ordered-profile kernel={kernel:?} synthetic_flat={flat} jobs={} cpu_profile_ms={cpu_profile_ms:.3} cpu_count_oracle_ms={cpu_count_oracle_ms:.3} pipeline_allocation_ms={pipeline_allocation_ms:.3} upload_enqueue_ms={upload_enqueue_ms:.3} cold={cold:?} parity={:?}",jobs.len(),compare(kernel,&actual,&expected,&oracle));
                    // Large serial128² is a single baseline, not a repeated campaign.
                    let repeats = if kernel == ProfileKernel::Ordered {
                        0
                    } else {
                        5
                    };
                    for repeat in 0..repeats {
                        let (actual, timing) = builder.run_blocking(&device, &queue).unwrap();
                        println!("ordered-profile kernel={kernel:?} synthetic_flat={flat} resident_repeat={repeat} timing={timing:?} parity={:?}",compare(kernel,&actual,&expected,&oracle));
                    }
                }
            }
        });
    }
}
