//! Bounded #71 experiment. No production geometry/source path uses this probe.
//! Input is an exact CPU canonical page; output is integer face counts only.
use frontier_world::{Material, World, VOXEL_SIZE};
use std::{sync::mpsc, time::Instant};

pub const WIDTH: usize = 128;
pub const HALO_SIDE: usize = WIDTH + 2;
pub const MAX_PAGES: usize = 8;
pub type FaceCounts = [[u32; 5]; 5]; // +Y,+X,-X,+Z,-Z, then Material discriminant.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Column {
    height: i32,
    deep: u32,
    soil: u32,
    top: u32,
}
pub struct CanonicalPage {
    columns: Vec<Column>,
    pub min: [i64; 2],
    pub source_revision: u64,
    source_seed: u64,
    source_params: frontier_world::FbmParams,
}
impl CanonicalPage {
    /// Bounded unedited FBM heightfields only; general edited volumes are rejected.
    pub fn sample(world: &World, min: [i64; 2]) -> Result<Self, String> {
        if world.fbm_terrain().is_none()
            || world.topology_base_cells().is_some()
            || world.edit_cells().next().is_some()
        {
            return Err("summary probe requires unedited canonical FBM heightfield".into());
        }
        min[0]
            .checked_add(WIDTH as i64 + 1)
            .ok_or("page address overflow")?;
        min[1]
            .checked_add(WIDTH as i64 + 1)
            .ok_or("page address overflow")?;
        min[0].checked_sub(1).ok_or("page address overflow")?;
        min[1].checked_sub(1).ok_or("page address overflow")?;
        let mut heights = Vec::with_capacity(HALO_SIDE * HALO_SIDE);
        let mut materials = Vec::with_capacity(HALO_SIDE * HALO_SIDE);
        for z in -1..=WIDTH as i64 {
            for x in -1..=WIDTH as i64 {
                let c = world.column(min[0] + x, min[1] + z);
                let h = (c.height_m() / VOXEL_SIZE) as i64;
                heights.push(h);
                materials.push([c.material(h - 13), c.material(h - 12), c.material(h - 1)]);
            }
        }
        let base = *heights.iter().min().unwrap();
        let top = *heights.iter().max().unwrap();
        // For each axis, every cell contributes at most this range. No u32
        // overflow or i32 subtraction overflow can occur in the shader.
        if top as i128 - base as i128 > (u32::MAX as usize / (WIDTH * WIDTH)) as i128 {
            return Err("page height range exceeds exact u32 count bound".into());
        }
        let columns = heights
            .into_iter()
            .zip(materials)
            .map(|(h, m)| Column {
                height: (h - base) as i32,
                deep: m[0] as u32,
                soil: m[1] as u32,
                top: m[2] as u32,
            })
            .collect();
        Ok(Self {
            columns,
            min,
            source_revision: world.revision(),
            source_seed: world.seed(),
            source_params: world.fbm_terrain().unwrap().params(),
        })
    }
    pub fn cpu_counts(&self) -> FaceCounts {
        self.cpu_patch_counts(128).unwrap()[0]
    }
    pub fn cpu_patch_counts(&self, patch_width: usize) -> Result<Vec<FaceCounts>, String> {
        let side = patch_side(patch_width)?;
        let mut out = vec![[[0; 5]; 5]; side * side];
        for z in 1..=WIDTH {
            for x in 1..=WIDTH {
                let i = z * HALO_SIDE + x;
                let c = self.columns[i];
                let counts = &mut out[((z - 1) / patch_width) * side + (x - 1) / patch_width];
                counts[0][c.top as usize] += 1;
                for (axis, j) in [i + 1, i - 1, i + HALO_SIDE, i - HALO_SIDE]
                    .into_iter()
                    .enumerate()
                {
                    let lo = self.columns[j].height;
                    for (lower, upper, material) in [
                        (lo, c.height - 12, c.deep),
                        (lo.max(c.height - 12), c.height - 1, c.soil),
                        (lo.max(c.height - 1), c.height, c.top),
                    ] {
                        counts[axis + 1][material as usize] += (upper - lower).max(0) as u32;
                    }
                }
            }
        }
        Ok(out)
    }
    /// Independent direct canonical face enumeration, intended for small-range
    /// oracle fixtures. Hard cap keeps test/diagnostic work bounded.
    pub fn oracle(&self, world: &World) -> Result<FaceCounts, String> {
        Ok(self.oracle_patch_counts(world, 128)?[0])
    }
    pub fn oracle_patch_counts(
        &self,
        world: &World,
        patch_width: usize,
    ) -> Result<Vec<FaceCounts>, String> {
        let side = patch_side(patch_width)?;
        if world.revision() != self.source_revision
            || world.seed() != self.source_seed
            || world.fbm_terrain().map(|t| t.params()) != Some(self.source_params)
        {
            return Err("oracle snapshot mismatch".into());
        }
        let mut patches = vec![[[0; 5]; 5]; side * side];
        let mut faces = 0usize;
        for z in 0..WIDTH {
            for x in 0..WIDTH {
                let c = world.column(self.min[0] + x as i64, self.min[1] + z as i64);
                let h = (c.height_m() / VOXEL_SIZE) as i64;
                let counts = &mut patches[(z / patch_width) * side + x / patch_width];
                counts[0][c.material(h - 1) as usize] += 1;
                for (axis, dx, dz) in [(1, 1, 0), (2, -1, 0), (3, 0, 1), (4, 0, -1)] {
                    let n = world.column(self.min[0] + x as i64 + dx, self.min[1] + z as i64 + dz);
                    let lo = (n.height_m() / VOXEL_SIZE) as i64;
                    let nfaces = h.saturating_sub(lo).max(0) as usize;
                    faces = faces.saturating_add(nfaces);
                    if faces > 8_000_000 {
                        return Err("oracle face work limit".into());
                    }
                    for y in lo..h {
                        let m = c.material(y);
                        if m == Material::Air {
                            return Err("non-solid heightfield oracle".into());
                        }
                        counts[axis][m as usize] += 1;
                    }
                }
            }
        }
        Ok(patches)
    }
}
fn patch_side(width: usize) -> Result<usize, String> {
    if ![16, 32, 128].contains(&width) {
        return Err("patch width must be 16,32,128".into());
    }
    Ok(WIDTH / width)
}
#[derive(Clone, Copy, Debug, Default)]
pub struct ProbeTimings {
    pub submit_ms: f64,
    pub wait_readback_ms: f64,
    pub gpu_ms: Option<f64>,
    pub resident_bytes: u64,
}
pub struct GpuSummaryProbe {
    input: wgpu::Buffer,
    output: wgpu::Buffer,
    readback: wgpu::Buffer,
    pipeline: wgpu::ComputePipeline,
    bind: wgpu::BindGroup,
    capacity: usize,
    loaded: usize,
    patch_width: usize,
    timestamps: Option<(wgpu::QuerySet, wgpu::Buffer, wgpu::Buffer)>,
}
impl GpuSummaryProbe {
    pub fn new(device: &wgpu::Device, capacity: usize) -> Result<Self, String> {
        Self::new_with_patch_width(device, capacity, 16)
    }
    pub fn new_with_patch_width(
        device: &wgpu::Device,
        capacity: usize,
        patch_width: usize,
    ) -> Result<Self, String> {
        let side = patch_side(patch_width)?;
        let patches = side * side;
        if capacity == 0 || capacity > MAX_PAGES {
            return Err("summary capacity must be 1..8 pages".into());
        }
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let input = buffer(
            "summary canonical pages",
            (capacity * HALO_SIDE * HALO_SIDE * 16) as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let output = buffer(
            "summary exact counts",
            (capacity * patches * 100) as u64,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );
        let readback = buffer(
            "summary count readback",
            (capacity * patches * 100) as u64,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("canonical face summary experiment"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("gpu_summary_probe.wgsl")
                    .replace(
                        "const PATCH_WIDTH: u32 = 16u;",
                        &format!("const PATCH_WIDTH: u32 = {patch_width}u;"),
                    )
                    .into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("canonical summary"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("canonical summary pages"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        let timestamps = device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| {
                (
                    device.create_query_set(&wgpu::QuerySetDescriptor {
                        label: Some("summary timestamps"),
                        ty: wgpu::QueryType::Timestamp,
                        count: 2,
                    }),
                    buffer(
                        "summary resolve",
                        16,
                        wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    ),
                    buffer(
                        "summary timing readback",
                        16,
                        wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    ),
                )
            });
        Ok(Self {
            input,
            output,
            readback,
            pipeline,
            bind,
            capacity,
            loaded: 0,
            patch_width,
            timestamps,
        })
    }
    /// Enqueue a bounded batch once; run repeatedly to measure resident reuse.
    /// The returned time measures CPU enqueue only; first run includes GPU copy.
    pub fn upload(&mut self, queue: &wgpu::Queue, pages: &[CanonicalPage]) -> Result<f64, String> {
        if pages.is_empty() || pages.len() > self.capacity {
            return Err("summary upload page count".into());
        }
        let start = Instant::now();
        for (i, p) in pages.iter().enumerate() {
            queue.write_buffer(
                &self.input,
                (i * HALO_SIDE * HALO_SIDE * 16) as u64,
                bytemuck::cast_slice(&p.columns),
            );
        }
        self.loaded = pages.len();
        Ok(start.elapsed().as_secs_f64() * 1000.)
    }
    /// Synchronous experiment ONLY. Never call from the production render loop.
    pub fn run_blocking(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<(Vec<FaceCounts>, ProbeTimings), String> {
        if self.loaded == 0 {
            return Err("no canonical pages uploaded".into());
        }
        let start = Instant::now();
        let patches = (WIDTH / self.patch_width).pow(2);
        let n = (self.loaded * patches * 100) as u64;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("summary experiment"),
        });
        encoder.clear_buffer(&self.output, 0, Some(n));
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("summary compute"),
                timestamp_writes: self.timestamps.as_ref().map(|(q, _, _)| {
                    wgpu::ComputePassTimestampWrites {
                        query_set: q,
                        beginning_of_pass_write_index: Some(0),
                        end_of_pass_write_index: Some(1),
                    }
                }),
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.dispatch_workgroups(patches as u32, self.loaded as u32, 1);
        }
        encoder.copy_buffer_to_buffer(&self.output, 0, &self.readback, 0, n);
        if let Some((q, resolve, readback)) = &self.timestamps {
            encoder.resolve_query_set(q, 0..2, resolve, 0);
            encoder.copy_buffer_to_buffer(resolve, 0, readback, 0, 16);
        }
        queue.submit(Some(encoder.finish()));
        let submit_ms = start.elapsed().as_secs_f64() * 1000.;
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
        let mut result = Vec::with_capacity(self.loaded * patches);
        for bytes in data[..n as usize].chunks_exact(100) {
            let mut c = [[0; 5]; 5];
            for (v, b) in c.iter_mut().flatten().zip(bytes.chunks_exact(4)) {
                *v = u32::from_le_bytes(b.try_into().unwrap());
            }
            result.push(c);
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
            ProbeTimings {
                submit_ms,
                wait_readback_ms: wait.elapsed().as_secs_f64() * 1000.,
                gpu_ms,
                resident_bytes: (self.capacity * (HALO_SIDE * HALO_SIDE * 16 + patches * 200))
                    as u64
                    + if self.timestamps.is_some() { 32 } else { 0 },
            },
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::{Cell, FbmParams, FbmRecipe};
    #[test]
    fn shader_validates() {
        let module = naga::front::wgsl::parse_str(include_str!("gpu_summary_probe.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }
    #[test]
    fn canonical_pages_match_independent_faces() {
        for recipe in [
            FbmRecipe::Legacy,
            FbmRecipe::Regional,
            FbmRecipe::RegionalUnwarped,
        ] {
            let w = World::fbm(
                7,
                FbmParams {
                    recipe,
                    ..Default::default()
                },
            )
            .unwrap();
            for min in [[0, -512], [-1024, 128]] {
                let p = CanonicalPage::sample(&w, min).unwrap();
                for width in [16, 32, 128] {
                    assert_eq!(
                        p.cpu_patch_counts(width).unwrap(),
                        p.oracle_patch_counts(&w, width).unwrap()
                    );
                }
            }
        }
    }
    #[test]
    fn rejects_noncanonical_volumes_and_bounds() {
        let mut w = World::fbm(7, FbmParams::default()).unwrap();
        assert!(CanonicalPage::sample(&w, [i64::MAX, 0]).is_err());
        w.set(
            Cell {
                x: 0,
                y: 99999,
                z: 0,
            },
            Material::Stone,
        );
        assert!(CanonicalPage::sample(&w, [0, 0]).is_err());
        assert!(CanonicalPage::sample(
            &World::fbm_topology(7, FbmParams::default()).unwrap(),
            [0, 0]
        )
        .is_err());
    }
    #[test]
    #[ignore = "manual GPU experiment; coordinate hardware authorization"]
    fn measure_gpu_summary_batches() {
        pollster::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("adapter");
            let features = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
            let (device, queue) = adapter
                .request_device(
                    &wgpu::DeviceDescriptor {
                        label: Some("bounded summary probe"),
                        required_features: features,
                        required_limits: wgpu::Limits::default(),
                        memory_hints: wgpu::MemoryHints::MemoryUsage,
                    },
                    None,
                )
                .await
                .unwrap();
            println!("adapter={:?}", adapter.get_info());
            let w = World::fbm(7, FbmParams::default()).unwrap();
            for n in [1, 4, 8] {
                let start = Instant::now();
                let pages: Vec<_> = (0..n)
                    .map(|i| CanonicalPage::sample(&w, [(i as i64 - 4) * 128, -512]).unwrap())
                    .collect();
                let generation_ms = start.elapsed().as_secs_f64() * 1000.;
                let start = Instant::now();
                let expected: Vec<_> = pages
                    .iter()
                    .flat_map(|p| p.cpu_patch_counts(16).unwrap())
                    .collect();
                let cpu_ms = start.elapsed().as_secs_f64() * 1000.;
                for p in &pages {
                    for width in [16, 32, 128] {
                        assert_eq!(
                            p.cpu_patch_counts(width).unwrap(),
                            p.oracle_patch_counts(&w, width).unwrap()
                        );
                    }
                }
                let mut probe = GpuSummaryProbe::new(&device, n).unwrap();
                let upload_ms = probe.upload(&queue, &pages).unwrap();
                let (result, cold) = probe.run_blocking(&device, &queue).unwrap();
                assert_eq!(result, expected);
                println!("pages={n} patch_width=16 patches_per_page=64 generation_ms={generation_ms:.3} cpu_summary_ms={cpu_ms:.3} upload_enqueue_ms={upload_ms:.3} cold={cold:?}");
                for repeat in 0..5 {
                    let (result, t) = probe.run_blocking(&device, &queue).unwrap();
                    assert_eq!(result, expected);
                    println!("pages={n} resident_repeat={repeat} timing={t:?}");
                }
            }
        });
    }
}
