//! Isolated visibility microbenchmark: no modification of the production backend.
mod baseline;
use bytemuck::{Pod, Zeroable};
use std::{error::Error, fs, path::Path, time::Instant};
use wgpu::util::DeviceExt;
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Brick {
    lo: [f32; 3],
    pointer: u32,
    hi: [f32; 3],
    mask: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Ray {
    o: [f32; 4],
    d: [f32; 4],
}
struct Layout {
    grid: Vec<u32>,
    bricks: Vec<Brick>,
    values: Vec<u32>,
    masks: Vec<u32>,
    unmerged_bricks: usize,
}
fn layout(v: &[u32], n: usize, b: usize, tight: bool) -> Layout {
    let g = n / b;
    let mut out = Layout {
        grid: vec![0; g * g * g],
        bricks: vec![],
        values: vec![],
        masks: vec![],
        unmerged_bricks: 0,
    };
    for z in 0..g {
        for y in 0..g {
            for x in 0..g {
                let mut values = vec![0; b * b * b];
                let mut lo = [b; 3];
                let mut hi = [0; 3];
                for dz in 0..b {
                    for dy in 0..b {
                        for dx in 0..b {
                            let value = v[x * b + dx + n * (y * b + dy + n * (z * b + dz))];
                            values[dx + b * (dy + b * dz)] = value;
                            if value != 0 {
                                for (a, k) in [dx, dy, dz].into_iter().enumerate() {
                                    lo[a] = lo[a].min(k);
                                    hi[a] = hi[a].max(k + 1);
                                }
                            }
                        }
                    }
                }
                if hi[0] == 0 {
                    continue;
                }
                if !tight {
                    lo = [0; 3];
                    hi = [b; 3];
                }
                let mut masks = vec![0u32; b * b * b / 32];
                for (i, &value) in values.iter().enumerate() {
                    if value != 0 {
                        masks[i / 32] |= 1 << (i % 32);
                    }
                }
                let uniform = values.iter().all(|v| *v == values[0]);
                out.grid[x + g * (y + g * z)] = out.bricks.len() as u32 + 1;
                out.bricks.push(Brick {
                    lo: std::array::from_fn(|a| ([x, y, z][a] * b + lo[a]) as f32),
                    hi: std::array::from_fn(|a| ([x, y, z][a] * b + hi[a]) as f32),
                    pointer: if uniform {
                        0x80000000 | values[0]
                    } else {
                        out.values.len() as u32
                    },
                    mask: out.masks.len() as u32,
                });
                if !uniform {
                    out.values.extend(values);
                    out.masks.extend(masks);
                }
            }
        }
    }
    // Uniform interior runs need one AABB, not a BVH primitive for every
    // brick in the ground. Preserve exact pigment identity; nonuniform bricks
    // keep their native addresses and independently tested tight bounds.
    out.unmerged_bricks = out.bricks.len();
    let mut merged = Vec::new();
    for z in 0..g {
        for x in 0..g {
            let mut y = 0;
            while y < g {
                let id = out.grid[x + g * (y + g * z)];
                if id == 0 {
                    y += 1;
                    continue;
                }
                let mut brick = out.bricks[id as usize - 1];
                let mut end = y + 1;
                if brick.pointer & 0x80000000 != 0 {
                    while end < g {
                        let next = out.grid[x + g * (end + g * z)];
                        if next == 0 || out.bricks[next as usize - 1].pointer != brick.pointer {
                            break;
                        }
                        brick.hi[1] = out.bricks[next as usize - 1].hi[1];
                        end += 1;
                    }
                }
                let new_id = merged.len() as u32 + 1;
                merged.push(brick);
                for yy in y..end {
                    out.grid[x + g * (yy + g * z)] = new_id;
                }
                y = end;
            }
        }
    }
    out.bricks = merged;
    // Keep pigment IDs compact like the production leaves; occupancy masks
    // are optional extra acceleration, not a hidden larger material format.
    assert!(out.values.iter().all(|v| *v <= 65535));
    for i in 0..out.values.len() / 2 {
        out.values[i] = out.values[i * 2] | (out.values[i * 2 + 1] << 16);
    }
    out.values.truncate(out.values.len() / 2);
    out
}
fn rays(n: usize) -> Vec<Ray> {
    let mut out = Vec::new();
    let n = n as f32;
    // Four views of the identical clipped fixture, including grazing angles.
    for view in 0..4 {
        for y in 0..90 {
            for x in 0..160 {
                let uv = [
                    (x as f32 + 0.371) / 160. * 2. - 1.,
                    (y as f32 + 0.617) / 90. * 2. - 1.,
                ];
                let (o, mut d) = match view {
                    0 => (
                        [n * 0.5, n * 0.55, -n * 0.2],
                        [uv[0] * 0.8, -0.12 - uv[1] * 0.45, 1.],
                    ),
                    1 => (
                        [n * 0.5, n * 0.55, n * 1.2],
                        [uv[0] * 0.8, -0.12 - uv[1] * 0.45, -1.],
                    ),
                    2 => (
                        [-n * 0.2, n * 0.9, n * 0.5],
                        [1., -0.45 - uv[1] * 0.35, uv[0] * 0.8],
                    ),
                    _ => (
                        [n * 0.5, n * 1.2, n * 0.5],
                        [uv[0] * 0.8, -1., uv[1] * 0.45],
                    ),
                };
                let length = d.iter().map(|v| v * v).sum::<f32>().sqrt();
                for v in &mut d {
                    *v /= length;
                }
                out.push(Ray {
                    o: [o[0], o[1], o[2], 0.],
                    d: [d[0], d[1], d[2], 0.],
                });
            }
        }
    }
    out
}
fn oracle(v: &[u32], n: usize, r: Ray) -> u32 {
    let o = r.o.map(f64::from);
    let d = r.d.map(f64::from);
    let mut near = 0.0f64;
    let mut far = 1e20f64;
    for a in 0..3 {
        if d[a] == 0. {
            if o[a] < 0. || o[a] >= n as f64 {
                return 0;
            }
        } else {
            let t0 = -o[a] / d[a];
            let t1 = (n as f64 - o[a]) / d[a];
            near = near.max(t0.min(t1));
            far = far.min(t0.max(t1));
        }
    }
    if near >= far {
        return 0;
    }
    let mut c: [i32; 3] =
        std::array::from_fn(|a| ((o[a] + d[a] * near).floor() as i32).clamp(0, n as i32 - 1));
    for _ in 0..3 * n + 3 {
        if c.iter().any(|&v| v < 0 || v >= n as i32) {
            return 0;
        }
        let i = c[0] as usize + n * (c[1] as usize + n * c[2] as usize);
        if v[i] != 0 {
            return i as u32 + 1;
        }
        let t: [f64; 3] = std::array::from_fn(|a| {
            if d[a] == 0. {
                1e20
            } else {
                (f64::from(c[a] + i32::from(d[a] > 0.)) - o[a]) / d[a]
            }
        });
        let next = t.into_iter().fold(f64::INFINITY, f64::min);
        if next > far {
            return 0;
        }
        for a in 0..3 {
            if t[a] == next {
                c[a] += if d[a] > 0. { 1 } else { -1 };
            }
        }
    }
    0
}
fn oracle_f32(v: &[u32], n: usize, r: Ray) -> u32 {
    let o = r.o;
    let d = r.d;
    let mut near = 0.0f32;
    let mut far = 1e20f32;
    for a in 0..3 {
        if d[a] == 0. {
            if o[a] < 0. || o[a] >= n as f32 {
                return 0;
            }
        } else {
            let t0 = -o[a] / d[a];
            let t1 = (n as f32 - o[a]) / d[a];
            near = near.max(t0.min(t1));
            far = far.min(t0.max(t1));
        }
    }
    if near >= far {
        return 0;
    }
    let mut c: [i32; 3] =
        std::array::from_fn(|a| ((o[a] + d[a] * near).floor() as i32).clamp(0, n as i32 - 1));
    for _ in 0..3 * n + 3 {
        if c.iter().any(|&v| v < 0 || v >= n as i32) {
            return 0;
        }
        let i = c[0] as usize + n * (c[1] as usize + n * c[2] as usize);
        if v[i] != 0 {
            return i as u32 + 1;
        }
        let t: [f32; 3] = std::array::from_fn(|a| {
            if d[a] == 0. {
                1e20
            } else {
                ((c[a] + i32::from(d[a] > 0.)) as f32 - o[a]) / d[a]
            }
        });
        let next = t.into_iter().fold(f32::INFINITY, f32::min);
        if next > far {
            return 0;
        }
        for a in 0..3 {
            if t[a] == next {
                c[a] += if d[a] > 0. { 1 } else { -1 };
            }
        }
    }
    0
}
fn buffer(
    device: &wgpu::Device,
    label: &str,
    bytes: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: if bytes.is_empty() { &[0; 32] } else { bytes },
        usage,
    })
}
fn read(device: &wgpu::Device, b: &wgpu::Buffer) -> Vec<u8> {
    let (tx, rx) = std::sync::mpsc::channel();
    b.map_async(wgpu::MapMode::Read, .., move |r| tx.send(r).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let bytes = b.get_mapped_range(..).unwrap().to_vec();
    b.unmap();
    bytes
}
fn warm(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    bind: &wgpu::BindGroup,
    rays: usize,
) -> Result<(), Box<dyn Error>> {
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < 1. {
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[]);
            for _ in 0..32 {
                pass.dispatch_workgroups((rays as u32).div_ceil(64), 1, 1);
            }
        }
        queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::wait_indefinitely())?;
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn Error>> {
    pollster::block_on(run())
}
async fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    let input = Path::new(
        args.get(1)
            .ok_or("usage: voxel-rt FIXTURE.vox OUTPUT.json")?,
    );
    let output = Path::new(args.get(2).ok_or("output path required")?);
    let bytes = fs::read(input)?;
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
        .collect();
    if words.len() < 4 || words[0] != 0x46564f58 || words[1] != 1 {
        return Err("invalid voxel fixture".into());
    }
    let n = words[2] as usize;
    let dense = &words[4..];
    if dense.len() != n * n * n {
        return Err("truncated fixture".into());
    }
    let rays = rays(n);
    let expected: Vec<_> = rays.iter().map(|&r| oracle_f32(dense, n, r)).collect();
    let f64_differences = rays
        .iter()
        .zip(&expected)
        .filter(|(r, e)| oracle(dense, n, **r) != **e)
        .count();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = instance.request_adapter(&Default::default()).await?;
    let rt = adapter
        .features()
        .contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY);
    let timestamp = adapter.features().contains(
        wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
    );
    let mut features = wgpu::Features::empty();
    if rt {
        features |= wgpu::Features::EXPERIMENTAL_RAY_QUERY;
    }
    if timestamp {
        features |=
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    }
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            required_features: features,
            required_limits: adapter.limits(),
            experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
            ..Default::default()
        })
        .await?;
    let rays_gpu = buffer(
        &device,
        "matched rays",
        bytemuck::cast_slice(&rays),
        wgpu::BufferUsages::STORAGE,
    );
    let result_size = rays.len() as u64 * 8;
    let result = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hits"),
        size: result_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("oracle readback"),
        size: result_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let queries = timestamp.then(|| {
        device.create_query_set(&wgpu::QuerySetDescriptor {
            label: None,
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        })
    });
    let resolve = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let time_read = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let (control, pass) = baseline::measure(&device, &queue, input, &rays, &expected, timestamp)?;
    let mut records = vec![control];
    let mut failed = !pass;
    let reverse = args.iter().any(|a| a == "--reverse");
    for b in if reverse { [8, 4] } else { [4, 8] } {
        for tight in if reverse {
            [true, false]
        } else {
            [false, true]
        } {
            let start = Instant::now();
            let l = layout(dense, n, b, tight);
            let prepare_ms = start.elapsed().as_secs_f64() * 1000.;
            let params = buffer(
                &device,
                "params",
                bytemuck::cast_slice(&[n as u32, b as u32, (n / b) as u32, rays.len() as u32]),
                wgpu::BufferUsages::UNIFORM,
            );
            let grid = buffer(
                &device,
                "sparse grid",
                bytemuck::cast_slice(&l.grid),
                wgpu::BufferUsages::STORAGE,
            );
            let bricks = buffer(
                &device,
                "brick bounds",
                bytemuck::cast_slice(&l.bricks),
                wgpu::BufferUsages::STORAGE,
            );
            let values = buffer(
                &device,
                "native values",
                bytemuck::cast_slice(&l.values),
                wgpu::BufferUsages::STORAGE,
            );
            let masks = buffer(
                &device,
                "occupancy",
                bytemuck::cast_slice(&l.masks),
                wgpu::BufferUsages::STORAGE,
            );
            let mut tlas = None;
            let mut blas = None;
            let build_start = Instant::now();
            if rt && !l.bricks.is_empty() {
                let bounds: Vec<f32> = l
                    .bricks
                    .iter()
                    .flat_map(|v| v.lo.into_iter().chain(v.hi))
                    .collect();
                let gpu_bounds = buffer(
                    &device,
                    "procedural AABBs",
                    bytemuck::cast_slice(&bounds),
                    wgpu::BufferUsages::BLAS_INPUT,
                );
                let size = wgpu::BlasAABBGeometrySizeDescriptor {
                    primitive_count: l.bricks.len() as u32,
                    flags: wgpu::AccelerationStructureGeometryFlags::empty(),
                };
                let built = device.create_blas(
                    &wgpu::CreateBlasDescriptor {
                        label: Some("voxel bricks"),
                        flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
                        update_mode: wgpu::AccelerationStructureUpdateMode::Build,
                    },
                    wgpu::BlasGeometrySizeDescriptors::AABBs {
                        descriptors: vec![size.clone()],
                    },
                );
                let mut top = device.create_tlas(&wgpu::CreateTlasDescriptor {
                    label: Some("voxel scene"),
                    flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
                    update_mode: wgpu::AccelerationStructureUpdateMode::Build,
                    max_instances: 1,
                });
                top[0] = Some(wgpu::TlasInstance::new(
                    &built,
                    [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.],
                    0,
                    255,
                ));
                let entry = wgpu::BlasBuildEntry {
                    blas: &built,
                    geometry: wgpu::BlasGeometries::AabbGeometries(vec![wgpu::BlasAabbGeometry {
                        size: &size,
                        stride: 24,
                        aabb_buffer: &gpu_bounds,
                        primitive_offset: 0,
                    }]),
                };
                let mut encoder = device.create_command_encoder(&Default::default());
                encoder.build_acceleration_structures([&entry], [&top]);
                queue.submit([encoder.finish()]);
                device.poll(wgpu::PollType::wait_indefinitely())?;
                tlas = Some(top);
                blas = Some(built);
            }
            let build_ms = build_start.elapsed().as_secs_f64() * 1000.;
            for mask in if reverse {
                [true, false]
            } else {
                [false, true]
            } {
                for hardware in if reverse {
                    [true, false]
                } else {
                    [false, true]
                } {
                    if hardware && tlas.is_none() {
                        continue;
                    }
                    let source = if hardware {
                        format!(
                            "enable wgpu_ray_query;\n{}\n{}",
                            include_str!("trace.wgsl"),
                            include_str!("hardware.wgsl")
                        )
                    } else {
                        include_str!("trace.wgsl").into()
                    };
                    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("voxel layout experiment"),
                        source: wgpu::ShaderSource::Wgsl(source.into()),
                    });
                    let pipeline =
                        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                            label: None,
                            layout: None,
                            module: &module,
                            entry_point: Some(if hardware { "hardware_main" } else { "main" }),
                            compilation_options: wgpu::PipelineCompilationOptions {
                                constants: &[("use_mask", if mask { 1. } else { 0. })],
                                ..Default::default()
                            },
                            cache: None,
                        });
                    let buffers = [&params, &grid, &bricks, &values, &masks, &rays_gpu, &result];
                    let mut entries: Vec<_> = buffers
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| !hardware || *i != 1)
                        .map(|(i, b)| wgpu::BindGroupEntry {
                            binding: i as u32,
                            resource: b.as_entire_binding(),
                        })
                        .collect();
                    if hardware {
                        entries.push(wgpu::BindGroupEntry {
                            binding: 7,
                            resource: wgpu::BindingResource::AccelerationStructure(
                                tlas.as_ref().unwrap(),
                            ),
                        });
                    }
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &pipeline.get_bind_group_layout(0),
                        entries: &entries,
                    });
                    warm(&device, &queue, &pipeline, &bind, rays.len())?;
                    let mut samples = vec![];
                    for batch in 0..10 {
                        let start = Instant::now();
                        let mut encoder = device.create_command_encoder(&Default::default());
                        if let Some(q) = &queries {
                            encoder.write_timestamp(q, 0);
                        }
                        {
                            let mut pass = encoder.begin_compute_pass(&Default::default());
                            pass.set_pipeline(&pipeline);
                            pass.set_bind_group(0, &bind, &[]);
                            for _ in 0..8 {
                                pass.dispatch_workgroups((rays.len() as u32).div_ceil(64), 1, 1);
                            }
                        }
                        if let Some(q) = &queries {
                            encoder.write_timestamp(q, 1);
                            encoder.resolve_query_set(q, 0..2, &resolve, 0);
                            encoder.copy_buffer_to_buffer(&resolve, 0, &time_read, 0, 16);
                        }
                        encoder.copy_buffer_to_buffer(&result, 0, &readback, 0, result_size);
                        queue.submit([encoder.finish()]);
                        device.poll(wgpu::PollType::wait_indefinitely())?;
                        let wall = start.elapsed().as_secs_f64() * 1000. / 8.;
                        if batch == 0 {
                            let hit = read(&device, &readback);
                            let actual: Vec<u32> = hit
                                .chunks_exact(8)
                                .map(|v| u32::from_le_bytes(v[..4].try_into().unwrap()))
                                .collect();
                            let mismatches =
                                expected.iter().zip(&actual).filter(|(a, b)| a != b).count();
                            if mismatches != 0 {
                                eprintln!("oracle mismatch b={b} tight={tight} mask={mask} rt={hardware}: {mismatches}");
                                failed = true;
                            }
                            let record=format!("{{\"pigment_bits\":16,\"brick\":{b},\"tight\":{tight},\"mask\":{mask},\"hardware\":{hardware},\"mismatches\":{mismatches},\"bricks\":{},\"unmerged_bricks\":{},\"explicit_payload_bytes\":{},\"layout_prepare_ms\":{prepare_ms},\"rt_build_wait_wall_ms\":{build_ms}",l.bricks.len(),l.unmerged_bricks,l.grid.len()*4+l.bricks.len()*32+l.values.len()*4+l.masks.len()*4);
                            records.push(record);
                        }
                        let gpu = if timestamp {
                            let data = read(&device, &time_read);
                            let a = u64::from_le_bytes(data[..8].try_into().unwrap());
                            let b = u64::from_le_bytes(data[8..].try_into().unwrap());
                            (b - a) as f64 * f64::from(queue.get_timestamp_period()) / 1e6 / 8.
                        } else {
                            wall
                        };
                        if batch >= 2 {
                            samples.push(gpu);
                        }
                    }
                    samples.sort_by(f64::total_cmp);
                    let median = (samples[3] + samples[4]) * 0.5;
                    records.last_mut().unwrap().push_str(&format!(
                        ",\"dispatch_p50_ms\":{median},\"samples_ms\":{samples:?}}}"
                    ));
                    eprintln!("b{b} tight={tight} mask={mask} rt={hardware}: {median:.4} ms");
                }
            }
            drop(blas);
        }
    }
    fs::write(output,format!("{{\"scope\":\"clipped-fixture visibility microbenchmark; one second sustained warmup per candidate; not full game FPS; AS opaque memory excluded; no RT streaming\",\"fixture\":{:?},\"adapter\":{:?},\"hardware_rt_supported\":{rt},\"gpu_timestamps\":{timestamp},\"rays\":{},\"f32_vs_f64_first_hit_differences\":{f64_differences},\"oracle_contract\":\"CPU FP32 native-plane DDA; FP64 differences retained\",\"fp32_oracle_checks_pass\":{},\"cases\":[{}]}}\n",input.to_string_lossy(),format!("{:?}",adapter.get_info()),rays.len(),!failed,records.join(",")))?;
    if failed {
        return Err("voxel oracle mismatch; results not accepted as optimization".into());
    }
    Ok(())
}
