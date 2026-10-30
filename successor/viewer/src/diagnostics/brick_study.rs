//! Opt-in bounded volumetric GPU DDA gate, independent of the production renderer.
//! Algorithm references (no copied source): PBRT 4e grid DDA
//! https://pbr-book.org/4ed/Volume_Scattering/Media and the NAADF project
//! https://www.cg.tuwien.ac.at/research/publications/2026/ulschmid-2026-naadf/ .
use bytemuck::{Pod, Zeroable};
use microvoxel::{
    generation::{Generator, GeneratorIdentity},
    materials::{Appearance, MaterialId, Voxel},
    render::brick_trace::{BrickTrace, Trace},
    Bounds, CellPos, Edit, EditAction, World,
};
use std::{fmt::Write as _, path::Path, time::Instant};
use wgpu::util::DeviceExt;
const WIDTH: u32 = 256;
const HEIGHT: u32 = 144;
const CELLS_PER_BRICK: usize = 32 * 32 * 32;
const UNKNOWN: u32 = u32::MAX;
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Ray {
    origin: [f32; 4],
    direction: [f32; 4],
    controls: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Hit {
    cell_status: [i32; 4],
    value_work: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    minimum: [i32; 4],
    maximum: [i32; 4],
    dimensions: [u32; 4],
}
struct Fixture {
    entropy: bool,
}
impl Generator for Fixture {
    fn identity(&self) -> GeneratorIdentity {
        GeneratorIdentity {
            seed: 5,
            version: 1,
            definition: u64::from(self.entropy),
        }
    }
    fn voxel(&self, p: CellPos) -> Option<Voxel> {
        if p.x < -24 && p.z >= 44 {
            return None;
        }
        let cube = (-20..-5).contains(&p.x) && (-8..12).contains(&p.y) && (2..19).contains(&p.z);
        let hill = (3..23).contains(&p.x) && (-8..13).contains(&p.y) && (8..39).contains(&p.z);
        let cave = (8..18).contains(&p.x) && (-7..5).contains(&p.y) && (8..34).contains(&p.z);
        if p.y >= -8 && !cube && !(hill && !cave) {
            return Some(Voxel::AIR);
        }
        let material = if cube {
            MaterialId::BASALT
        } else {
            MaterialId::ROCK
        };
        let color = if self.entropy {
            let hash = microvoxel::painting::hash(
                (p.x as u64).wrapping_mul(73856093)
                    ^ (p.y as u64).wrapping_mul(19349663)
                    ^ (p.z as u64).wrapping_mul(83492791),
            );
            hash as u16
        } else if cube {
            0xc986
        } else if hill {
            // Authored cell colors distinguish the rear/inner cave wall in an
            // unshaded image; the GPU receives these original voxel values.
            if p.z >= 34 || p.x == 7 || p.x == 18 || p.y == 5 {
                0x22ca
            } else {
                0x8e1b
            }
        } else {
            0x63cb
        };
        Some(Voxel::new(material, Appearance(color)))
    }
}
fn bounds() -> Bounds {
    Bounds {
        min: CellPos::new(-32, -32, -32),
        max: CellPos::new(32, 32, 64),
    }
}
fn pack(v: Voxel) -> u32 {
    u32::from(v.material.0) | (u32::from(v.appearance.0) << 16)
}
fn indexed<G: Generator>(world: &World<G>) -> Vec<u32> {
    let b = bounds();
    let mut cells = Vec::with_capacity(12 * CELLS_PER_BRICK);
    for bz in 0..3 {
        for by in 0..2 {
            for bx in 0..2 {
                // Y-fastest native brick order, with X then Z.
                for z in 0..32 {
                    for x in 0..32 {
                        for y in 0..32 {
                            let p = CellPos::new(
                                b.min.x + bx * 32 + x,
                                b.min.y + by * 32 + y,
                                b.min.z + bz * 32 + z,
                            );
                            cells.push(world.voxel_at(p).known().map_or(UNKNOWN, pack));
                        }
                    }
                }
            }
        }
    }
    cells
}
fn ray(origin: [f32; 3], direction: [f32; 3], distance: f32, steps: u32) -> Ray {
    Ray {
        origin: [origin[0], origin[1], origin[2], distance],
        direction: [direction[0], direction[1], direction[2], 0.],
        controls: [steps, 0, 0, 0],
    }
}
fn rays() -> Vec<Ray> {
    let mut rays = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let u =
                ((x as f32 + 0.5) / WIDTH as f32 * 2. - 1.) * (WIDTH as f32 / HEIGHT as f32) * 0.58;
            let v = (1. - (y as f32 + 0.5) / HEIGHT as f32 * 2.) * 0.58;
            rays.push(ray([0.5, 9.5, -27.5], [u, v - 0.12, 1.], 75., 256));
        }
    }
    // Explicit contract rays supplement the image's ordinary camera rays.
    rays.extend([
        ray([0.5, 9.5, -27.5], [0., 0., 1.], 1., 256), // Known Air -> Miss.
        ray([-25.5, 20.5, 43.5], [0., 0., 1.], 4., 256), // Unknown.
        ray([0.5, 9.5, -27.5], [0., 0., 1.], 75., 1),  // Budget.
        ray([0.5, 9.5, -27.5], [0., 0., 0.], 75., 256),
        ray([0.5, 9.5, -27.5], [1., 1., 1.], 75., 256), // Tied corners.
        ray([-5., 0., 2.], [-1., 0., 0.], 4., 256),     // Exact integer boundary.
        ray([-32.5, 0.5, 0.5], [1., 0., 0.], 75., 256), // Outside index.
        ray([-31.5, -7.5, 0.5], [-1., -1., 0.], 75., 256),
        ray([0.5, 9.5, -27.5], [0., 0., 1.], -1., 256),
        ray([0.5, 9.5, -27.5], [0., 0., 1.], 75., 0),
    ]);
    rays
}
fn expected<G: Generator>(world: &World<G>, oracle: &BrickTrace, ray: Ray) -> Hit {
    let origin = [
        ray.origin[0] as f64,
        ray.origin[1] as f64,
        ray.origin[2] as f64,
    ];
    let direction = [
        ray.direction[0] as f64,
        ray.direction[1] as f64,
        ray.direction[2] as f64,
    ];
    let mut out = Hit {
        cell_status: [0, 0, 0, 3],
        value_work: [0; 4],
    };
    match oracle.trace(
        origin,
        direction,
        ray.origin[3] as f64,
        ray.controls[0] as usize,
    ) {
        Trace::Hit { pos, material } => {
            out.cell_status = [pos.x as i32, pos.y as i32, pos.z as i32, 1];
            let voxel = world.voxel_at(pos).known().unwrap();
            assert_eq!(voxel.material, material);
            out.value_work[0] = pack(voxel);
        }
        Trace::Unknown(pos) => out.cell_status = [pos.x as i32, pos.y as i32, pos.z as i32, 2],
        Trace::Miss => out.cell_status[3] = 0,
        Trace::Incomplete => {}
    }
    out
}
fn matches(a: Hit, b: Hit) -> bool {
    a.cell_status[3] == b.cell_status[3]
        && (a.cell_status[3] == 0 || a.cell_status[3] == 3 || a.cell_status == b.cell_status)
        && (a.cell_status[3] != 1 || a.value_work[0] == b.value_work[0])
}
fn pixels(hits: &[Hit]) -> Vec<u8> {
    hits.iter()
        .take((WIDTH * HEIGHT) as usize)
        .flat_map(|h| match h.cell_status[3] {
            1 => {
                let rgb = h.value_work[0] >> 16;
                [
                    ((rgb >> 11 & 31) * 255 / 31) as u8,
                    ((rgb >> 5 & 63) * 255 / 63) as u8,
                    ((rgb & 31) * 255 / 31) as u8,
                ]
            }
            0 => [132, 182, 222],
            2 => [210, 60, 180],
            _ => [240, 160, 30],
        })
        .collect()
}
fn png(path: &Path, pixels: &[u8]) -> microvoxel::Result<()> {
    let mut encoder = png::Encoder::new(std::fs::File::create(path)?, WIDTH, HEIGHT);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(pixels)?;
    Ok(())
}
struct Bench {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    adapter: String,
    timestamps: bool,
}
struct Measurements {
    hits: Vec<Hit>,
    upload_ms: f64,
    gpu_ms: Vec<f64>,
    dispatch_wait_ms: Vec<f64>,
    readback_ms: f64,
}
impl Bench {
    fn new() -> microvoxel::Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .ok_or("No brick study GPU")?;
        let features = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("standalone brick DDA study"),
                required_features: features,
                required_limits:
                    wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
            },
            None,
        ))?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brick DDA"),
            source: wgpu::ShaderSource::Wgsl(include_str!("brick_study.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("plain volumetric brick DDA"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Ok(Self {
            device,
            queue,
            pipeline,
            adapter: format!(
                "{} ({:?})",
                adapter.get_info().name,
                adapter.get_info().backend
            ),
            timestamps: !features.is_empty(),
        })
    }
    fn read(&self, buffer: &wgpu::Buffer) -> microvoxel::Result<Vec<u8>> {
        let (tx, rx) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let bytes = buffer.slice(..).get_mapped_range().to_vec();
        buffer.unmap();
        Ok(bytes)
    }
    fn run(&self, cells: &[u32], rays: &[Ray]) -> microvoxel::Result<Measurements> {
        let start = Instant::now();
        let buffer = |label, bytes: &[u8], usage| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytes,
                    usage,
                })
        };
        let source = buffer(
            "bricked exact material/color",
            bytemuck::cast_slice(cells),
            wgpu::BufferUsages::STORAGE,
        );
        let ray_buffer = buffer(
            "identical CPU/GPU camera rays",
            bytemuck::cast_slice(rays),
            wgpu::BufferUsages::STORAGE,
        );
        let b = bounds();
        let params = Params {
            minimum: [b.min.x as i32, b.min.y as i32, b.min.z as i32, 0],
            maximum: [b.max.x as i32, b.max.y as i32, b.max.z as i32, 0],
            dimensions: [2, 2, 3, rays.len() as u32],
        };
        let params = buffer(
            "bounded index",
            bytemuck::bytes_of(&params),
            wgpu::BufferUsages::UNIFORM,
        );
        let make = |label, size, usage| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let bytes = rays.len() as u64 * std::mem::size_of::<Hit>() as u64;
        let output = make(
            "exact GPU hits and work",
            bytes,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let read = make(
            "full frame readback",
            bytes,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brick DDA inputs"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: source.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: ray_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: output.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params.as_entire_binding(),
                },
            ],
        });
        self.queue.submit(std::iter::empty());
        self.device.poll(wgpu::Maintain::Wait);
        let upload_ms = start.elapsed().as_secs_f64() * 1000.;
        let queries = self.timestamps.then(|| {
            self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("brick study timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            })
        });
        let resolve = make(
            "timestamp resolve",
            256,
            wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        );
        let times = make(
            "timestamp readback",
            16,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let mut gpu_ms = Vec::new();
        let mut dispatch_wait_ms = Vec::new();
        for iteration in 0..12 {
            let start = Instant::now();
            let mut encoder = self.device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("full-frame volumetric DDA"),
                    timestamp_writes: queries.as_ref().map(|query_set| {
                        wgpu::ComputePassTimestampWrites {
                            query_set,
                            beginning_of_pass_write_index: Some(0),
                            end_of_pass_write_index: Some(1),
                        }
                    }),
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups((rays.len() as u32).div_ceil(64), 1, 1);
            }
            if let Some(queries) = &queries {
                encoder.resolve_query_set(queries, 0..2, &resolve, 0);
                encoder.copy_buffer_to_buffer(&resolve, 0, &times, 0, 16);
            }
            self.queue.submit(Some(encoder.finish()));
            self.device.poll(wgpu::Maintain::Wait);
            let wall_ms = start.elapsed().as_secs_f64() * 1000.;
            let gpu_time = if self.timestamps {
                let bytes = self.read(&times)?;
                let first = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                let last = u64::from_le_bytes(bytes[8..].try_into().unwrap());
                (last - first) as f64 * self.queue.get_timestamp_period() as f64 / 1e6
            } else {
                f64::NAN
            };
            if iteration >= 3 {
                gpu_ms.push(gpu_time);
                dispatch_wait_ms.push(wall_ms);
            }
        }
        let start = Instant::now();
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&output, 0, &read, 0, bytes);
        self.queue.submit(Some(encoder.finish()));
        let bytes = self.read(&read)?;
        let hits = bytes
            .chunks_exact(std::mem::size_of::<Hit>())
            .map(bytemuck::pod_read_unaligned)
            .collect();
        let readback_ms = start.elapsed().as_secs_f64() * 1000.;
        Ok(Measurements {
            hits,
            upload_ms,
            gpu_ms,
            dispatch_wait_ms,
            readback_ms,
        })
    }
}
fn percentiles(values: &[f64]) -> [f64; 2] {
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    [
        values[values.len() / 2],
        values[((values.len() as f64 * 0.95).ceil() as usize - 1).min(values.len() - 1)],
    ]
}
/// Writes three full-frame CPU/GPU pairs plus exact comparison and timing data.
/// Uses its own bounded synthetic source; no production runtime state is changed.
pub fn run(directory: &Path) -> microvoxel::Result<()> {
    std::fs::create_dir_all(directory)?;
    let bench = Bench::new()?;
    let rays = rays();
    let mut report = format!("Standalone plain 32^3 brick DDA experimental gate\nGPU: {}\nimage {}x{}, {} identical rays (including 10 contract probes)\n12 bricks, canonical index [-32,-32,-32]..[32,32,64], 4 B per exact cell, no DAG, no empty-space skip\nHit pixels decode original RGB565 appearance without palette substitution or shading; miss blue, unknown magenta, incomplete orange.\nCrossing order uses fresh boundary numerators and FMA-compensated product comparisons to avoid accumulated f32 boundary drift.\n9 measured dispatches after 3 warmup; GPU timestamps exclude upload/readback; dispatch+wait is synchronized host wall time, not gameplay frame time.\n", bench.adapter, WIDTH, HEIGHT, rays.len());
    let mut failures = 0;
    for (name, entropy, edited) in [
        ("cave-cube", false, false),
        ("edited-bridge", false, true),
        ("entropy-bridge", true, true),
    ] {
        let mut world = World::new(Fixture { entropy });
        if edited {
            let mut actions = Vec::new();
            for z in 21..25 {
                for x in -9..14 {
                    for y in 1..3 {
                        actions.push(EditAction::Set(Edit::set(
                            CellPos::new(x, y, z),
                            Voxel::new(
                                MaterialId::BASALT,
                                Appearance(if entropy {
                                    microvoxel::painting::hash(
                                        (x as u64).wrapping_mul(31)
                                            ^ y as u64
                                            ^ (z as u64).wrapping_mul(7919),
                                    ) as u16
                                } else {
                                    0xff40
                                }),
                            ),
                        )));
                    }
                }
            }
            // A narrow roof opening crosses the cave's volumetric boundary.
            for y in 5..13 {
                actions.push(EditAction::Set(Edit::set(
                    CellPos::new(12, y, 20),
                    Voxel::AIR,
                )));
            }
            world.apply_actions(&actions)?;
        }
        let start = Instant::now();
        let cells = indexed(&world);
        let build_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let oracle = BrickTrace::build(&world.read_snapshot(), bounds())?;
        let oracle_build_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let expected: Vec<_> = rays.iter().map(|r| expected(&world, &oracle, *r)).collect();
        let cpu_trace_ms = start.elapsed().as_secs_f64() * 1000.;
        let measured = bench.run(&cells, &rays)?;
        let mut mismatches = 0;
        let mut statuses = [0usize; 4];
        for (i, (gpu, cpu)) in measured.hits.iter().zip(&expected).enumerate() {
            statuses[gpu.cell_status[3] as usize] += 1;
            if !matches(*gpu, *cpu) {
                if mismatches < 8 {
                    writeln!(report, "{name}: mismatch ray {i}: GPU {gpu:?}, CPU {cpu:?}")?;
                }
                mismatches += 1;
            }
        }
        failures += mismatches;
        png(
            &directory.join(format!("{name}-gpu.png")),
            &pixels(&measured.hits),
        )?;
        png(
            &directory.join(format!("{name}-cpu.png")),
            &pixels(&expected),
        )?;
        let mut difference = Vec::new();
        for (gpu, cpu) in measured
            .hits
            .iter()
            .zip(&expected)
            .take((WIDTH * HEIGHT) as usize)
        {
            difference.extend(if matches(*gpu, *cpu) {
                [0, 0, 0]
            } else {
                [255, 0, 0]
            });
        }
        png(
            &directory.join(format!("{name}-difference.png")),
            &difference,
        )?;
        let visits: u64 = measured
            .hits
            .iter()
            .map(|h| u64::from(h.value_work[1]))
            .sum();
        writeln!(report, "{name}: revision {}; {} exact comparisons, {mismatches} mismatches; [miss,hit,unknown,incomplete]={statuses:?}; {visits} native-cell visits; {} source bytes", world.revision(), rays.len(), cells.len()*4)?;
        writeln!(report, "  source pack {:.3} ms; CPU oracle build {:.3} ms; CPU identical-ray trace {:.3} ms; buffer allocation+upload+wait {:.3} ms; GPU kernel p50/p95 {:?} ms; dispatch+wait p50/p95 {:?} ms; full hit readback+map+copy {:.3} ms", build_ms, oracle_build_ms, cpu_trace_ms, measured.upload_ms, percentiles(&measured.gpu_ms), percentiles(&measured.dispatch_wait_ms), measured.readback_ms)?;
        writeln!(
            report,
            "  GPU kernel samples ms: {:?}; dispatch+wait samples ms: {:?}",
            measured.gpu_ms, measured.dispatch_wait_ms
        )?;
    }
    writeln!(report, "Decision: {}. Retain as a bounded correctness/traversal candidate only; no default-renderer adoption. Full native payload uses 1.5 MiB for 12 bricks, including known Air. No sparse residency, brick skipping, acceleration invalidation, movement/LOD, presentation, shadows, or matched raster benchmark is implemented. f32 GPU arithmetic is checked against f64 CPU on this local corpus only, not arbitrary world-scale rays. Per-scene rebuild/upload is measured, not incremental editing latency. No FPS or speedup claim follows from these samples.", if failures == 0 { "PASS exact local CPU/GPU gate" } else { "REJECT: CPU/GPU mismatch" })?;
    writeln!(report, "Primary references inspected: https://pbr-book.org/4ed/Volume_Scattering/Media ; https://www.cg.tuwien.ac.at/research/publications/2026/ulschmid-2026-naadf/ . No external implementation copied; this is plain grid DDA, not a reproduction of NAADF.")?;
    std::fs::write(directory.join("report.txt"), &report)?;
    print!("{report}");
    if failures != 0 {
        return Err(format!("brick DDA gate failed with {failures} exact mismatches").into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn brick_shader_validates() {
        let module = naga::front::wgsl::parse_str(include_str!("brick_study.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
    #[test]
    fn oracle_contract_probes_distinguish_air_unknown_and_budget() {
        let world = World::new(Fixture { entropy: false });
        let oracle = BrickTrace::build(&world.read_snapshot(), bounds()).unwrap();
        let rays = rays();
        let probes: Vec<_> = rays[(WIDTH * HEIGHT) as usize..]
            .iter()
            .map(|r| expected(&world, &oracle, *r).cell_status[3])
            .collect();
        assert_eq!(probes, [0, 2, 3, 3, 3, 1, 3, 3, 3, 3]);
    }
    #[test]
    #[ignore = "requires compute GPU; writes full-frame diagnostic artifacts"]
    fn gpu_full_frame_exact_gate() {
        run(Path::new("output/redesign/brick-study")).unwrap();
    }
}
