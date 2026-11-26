//! GPU throughput and parity probe for the canonical Eroded landscape.
use frontier_world::{FbmParams, FbmRecipe, FbmTerrain};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const SOURCE: &str = include_str!("native_column_gpu/eroded.wgsl");

fn main() -> Result<()> {
    pollster::block_on(run())
}

async fn run() -> Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let has = |name: &str| arguments.iter().any(|a| a == name);
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..Default::default()
    });
    let adapter = instance
        .request_adapter(&Default::default())
        .await
        .ok_or("no Vulkan adapter")?;
    let features = adapter.features();
    let info = adapter.get_info();
    eprintln!("{info:?}");
    eprintln!(
        "f64={} int64={} ray_query={}",
        features.contains(wgpu::Features::SHADER_F64),
        features.contains(wgpu::Features::SHADER_INT64),
        features.contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY),
    );
    if has("--capabilities") {
        return Ok(());
    }
    if info.device_type == wgpu::DeviceType::Cpu {
        return Err("the benchmark requires a hardware adapter".into());
    }
    let (device, queue) = adapter
        .request_device(
            &wgpu::DeviceDescriptor {
                required_features: wgpu::Features::SHADER_F64
                    | wgpu::Features::SHADER_INT64
                    | wgpu::Features::TIMESTAMP_QUERY,
                ..Default::default()
            },
            None,
        )
        .await?;
    let params = FbmParams {
        recipe: FbmRecipe::Eroded,
        ..Default::default()
    };
    let seed = arguments
        .iter()
        .find(|s| !s.starts_with("--"))
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(1685576058315138427);
    let terrain = FbmTerrain::new(seed, params)?;
    let count = if has("--quick") { 4096 } else { 1280 * 720 };
    let full_source = !has("--land");
    // Deterministic coverage of the horizon, regardless of workgroup shape.
    let coordinates: Vec<[i32; 2]> = (0..count)
        .map(|i| {
            let x = (i as u32).wrapping_mul(0x9e3779b9) % 192000;
            let z = ((i as u32).wrapping_mul(0xf1bbcddd) ^ i as u32 >> 6) % 192000;
            [-107_008 + x as i32 - 96_000, 97_280 + z as i32 - 96_000]
        })
        .collect();
    let kernel = source_buffer(&device, "samples", bytemuck::cast_slice(&coordinates));
    let parameters = [
        seed,
        params.horizontal_scale.to_bits(),
        params.amplitude.to_bits(),
        params.roughness.to_bits(),
        u64::from(params.octaves),
    ];
    let constants = source_buffer(&device, "parameters", bytemuck::cast_slice(&parameters));
    let start = std::time::Instant::now();
    let descriptors = if full_source {
        terrain
            .eroded_gpu_descriptors([-12688., 80.], [-688., 12080.])
            .ok_or("could not export source descriptors")?
    } else {
        vec![0.]
    };
    let data = source_buffer(&device, "drainage", bytemuck::cast_slice(&descriptors));
    eprintln!(
        "descriptor build {:.3} ms, {} bytes",
        start.elapsed().as_secs_f64() * 1000.,
        descriptors.len() * 8
    );
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Eroded procedural source"),
        source: wgpu::ShaderSource::Wgsl(SOURCE.into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &std::array::from_fn::<_, 4, _>(|i| wgpu::BindGroupLayoutEntry {
            binding: i as u32,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: i != 2 },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("native column evaluation"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("generate"),
        compilation_options: wgpu::PipelineCompilationOptions {
            constants: &std::collections::HashMap::from([(
                "FULL_SOURCE".into(),
                f64::from(full_source),
            )]),
            ..Default::default()
        },
        cache: None,
    });
    use wgpu::BufferUsages as U;
    let bytes = (8 * count) as u64;
    let buffer = |size, usage| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage,
            mapped_at_creation: false,
        })
    };
    let output = buffer(bytes, U::STORAGE | U::COPY_SRC);
    let download = buffer(bytes + 16, U::COPY_DST | U::MAP_READ);
    let stamps = buffer(16, U::QUERY_RESOLVE | U::COPY_SRC);
    let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: None,
        ty: wgpu::QueryType::Timestamp,
        count: 2,
    });
    let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[&kernel, &constants, &output, &data]
            .into_iter()
            .enumerate()
            .map(|(i, buffer)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: buffer.as_entire_binding(),
            })
            .collect::<Vec<_>>(),
    });
    let mut times = Vec::new();
    let mut observed = Vec::<f64>::new();
    for frame in 0..13 {
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                    query_set: &queries,
                    beginning_of_pass_write_index: Some(0),
                    end_of_pass_write_index: Some(1),
                }),
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups((count as u32).div_ceil(64), 1, 1);
        }
        encoder.resolve_query_set(&queries, 0..2, &stamps, 0);
        encoder.copy_buffer_to_buffer(&stamps, 0, &download, bytes, 16);
        if frame == 12 {
            encoder.copy_buffer_to_buffer(&output, 0, &download, 0, bytes);
        }
        queue.submit([encoder.finish()]);
        let offset = if frame == 12 { 0 } else { bytes };
        let mapping = download.slice(offset..);
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        mapping.map_async(wgpu::MapMode::Read, move |result| {
            let _ = send.send(result);
        });
        device.poll(wgpu::Maintain::Wait);
        receive.recv()??;
        let data = mapping.get_mapped_range();
        let stamp: &[u64] = bytemuck::cast_slice(&data[(bytes - offset) as usize..]);
        if frame == 12 {
            observed.extend_from_slice(bytemuck::cast_slice(&data[..bytes as usize]));
        } else if frame > 0 {
            times.push(
                stamp[1].saturating_sub(stamp[0]) as f64
                    * f64::from(queue.get_timestamp_period())
                    * 1e-6,
            );
        }
        drop(data);
        download.unmap();
    }
    let mut maximum_error = 0.0f64;
    let mut quantization_errors = 0;
    for (&[x, z], actual) in coordinates.iter().zip(observed) {
        let x = (f64::from(x) + 0.5) * 0.0625;
        let z = (f64::from(z) + 0.5) * 0.0625;
        let expected = if full_source {
            terrain.sample_fields(x, z).ground_height_m
        } else {
            terrain.sample_climate_surface(x, z)[0]
        };
        maximum_error = maximum_error.max((actual - expected).abs());
        quantization_errors += usize::from((actual * 16.).floor() != (expected * 16.).floor());
    }
    times.sort_by(f64::total_cmp);
    let stage = if full_source { "ground" } else { "uncut land" };
    println!("{stage} samples {count}, seed {seed}, GPU p50 {:.3} ms, p95 {:.3} ms, native height disagreements {quantization_errors}, max error {maximum_error:.9} m",
        times[times.len()/2], times[times.len()-1]);
    Ok(())
}

fn source_buffer(device: &wgpu::Device, label: &str, contents: &[u8]) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents,
        usage: wgpu::BufferUsages::STORAGE,
    })
}
