//! Same rays and canonical cells through the production NAADF traversal source.
use super::*;
pub(super) fn measure(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    input: &Path,
    rays: &[Ray],
    expected: &[u32],
    timestamp: bool,
) -> Result<(String, bool), Box<dyn Error>> {
    let raw = fs::read(input.with_extension("aadf"))?;
    let w: Vec<u32> = raw
        .chunks_exact(4)
        .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
        .collect();
    if w.len() < 7 || w[0] != 0x41414446 || w[1] != 1 {
        return Err("invalid hierarchy".into());
    }
    let n = w[2];
    let mut offset = 7usize;
    let root = &w[offset..offset + w[3] as usize];
    offset += w[3] as usize;
    let blocks = &w[offset..offset + w[4] as usize];
    offset += w[4] as usize;
    let voxels = &w[offset..offset + w[5] as usize];
    let mut uniform = [0u32; 96];
    uniform[36..40].copy_from_slice(&[n * 16, n * 16, n * 16, n]);
    if w[6] == 16 {
        uniform[91] = 2;
    }
    let camera = buffer(
        device,
        "NAADF uniform",
        bytemuck::cast_slice(&uniform),
        wgpu::BufferUsages::UNIFORM,
    );
    let roots = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("canonical roots"),
        size: wgpu::Extent3d {
            width: n,
            height: n,
            depth_or_array_layers: n,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::R32Uint,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        roots.as_image_copy(),
        bytemuck::cast_slice(root),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(n * 4),
            rows_per_image: Some(n),
        },
        roots.size(),
    );
    let roots_view = roots.create_view(&Default::default());
    let height = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Uint,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let height_view = height.create_view(&Default::default());
    let blocks = buffer(
        device,
        "NAADF blocks",
        bytemuck::cast_slice(blocks),
        wgpu::BufferUsages::STORAGE,
    );
    let voxels = buffer(
        device,
        "NAADF leaves",
        bytemuck::cast_slice(voxels),
        wgpu::BufferUsages::STORAGE,
    );
    let palette = buffer(
        device,
        "unused pigment",
        &[0; 16],
        wgpu::BufferUsages::STORAGE,
    );
    let metrics = buffer(
        device,
        "disabled metrics",
        &[0; 4],
        wgpu::BufferUsages::STORAGE,
    );
    let rays_gpu = buffer(
        device,
        "rays",
        bytemuck::cast_slice(rays),
        wgpu::BufferUsages::STORAGE,
    );
    let bytes = rays.len() as u64 * 8;
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut entries = vec![];
    for binding in 0..10 {
        let ty = match binding {
            0 => wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            1 | 5 | 6 => wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Uint,
                view_dimension: if binding == 6 {
                    wgpu::TextureViewDimension::D2
                } else {
                    wgpu::TextureViewDimension::D3
                },
                multisampled: false,
            },
            _ => wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage {
                    read_only: binding != 7 && binding != 9,
                },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
        };
        entries.push(wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        });
    }
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &entries,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: camera.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&roots_view),
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
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&roots_view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&height_view),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: metrics.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: rays_gpu.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let source = concat!(
        "const NAADF_GROUP:u32=0u;const NAADF_BASE:u32=0u;\n",
        include_str!("../../../renderer/src/naadf/trace.wgsl"),
        r#"
struct InputRay{o:vec4<f32>,d:vec4<f32>};
@group(0) @binding(8) var<storage,read> input_rays:array<InputRay>;
@group(0) @binding(9) var<storage,read_write> output:array<vec2<u32>>;
@compute @workgroup_size(64) fn main(@builtin(global_invocation_id) i:vec3<u32>){
    if i.x>=arrayLength(&input_rays){return;}
    let r=input_rays[i.x];let h=trace_voxels(r.o.xyz,r.d.xyz,0.,true);
    var cell=0u;
    if h.found{let c=vec3<u32>(clamp(forward_cell(r.o.xyz+r.d.xyz*h.t,r.d.xyz),vec3(0),vec3<i32>(naadf.extent.xyz)-vec3(1)));cell=1u+c.x+naadf.extent.x*(c.y+naadf.extent.y*c.z);}
    output[i.x]=vec2(cell,bitcast<u32>(h.t));
}
"#
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("production NAADF control"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
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
    let times = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    warm(device, queue, &pipeline, &bind, rays.len())?;
    let mut samples = vec![];
    let mut mismatches = 0;
    let mut reconstruction_differences = 0;
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
            encoder.copy_buffer_to_buffer(&resolve, 0, &times, 0, 16);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, bytes);
        queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::wait_indefinitely())?;
        let wall = start.elapsed().as_secs_f64() * 1000. / 8.;
        if batch == 0 {
            let raw = read(device, &readback);
            for (index, (v, &e)) in raw.chunks_exact(8).zip(expected).enumerate() {
                let a = u32::from_le_bytes(v[..4].try_into().unwrap());
                if a != e {
                    reconstruction_differences += 1;
                }
                let at = f32::from_bits(u32::from_le_bytes(v[4..].try_into().unwrap()));
                let side = n * 16;
                let mut et = 0.0f64;
                if e != 0 {
                    let i = e - 1;
                    let c = [i % side, (i / side) % side, i / (side * side)];
                    for axis in 0..3 {
                        let o = f64::from(rays[index].o[axis]);
                        let d = f64::from(rays[index].d[axis]);
                        if d != 0. {
                            let a = (f64::from(c[axis]) - o) / d;
                            let b = (f64::from(c[axis] + 1) - o) / d;
                            et = et.max(a.min(b));
                        }
                    }
                }
                if (a == 0) != (e == 0) || (e != 0 && (f64::from(at) - et).abs() > 0.0001) {
                    if mismatches < 5 {
                        eprintln!("NAADF mismatch ray {index}: cell {a}/{e}, time {at}/{et}");
                    }
                    mismatches += 1;
                }
            }
        }
        let time = if timestamp {
            let raw = read(device, &times);
            (u64::from_le_bytes(raw[8..].try_into().unwrap())
                - u64::from_le_bytes(raw[..8].try_into().unwrap())) as f64
                * f64::from(queue.get_timestamp_period())
                / 1e6
                / 8.
        } else {
            wall
        };
        if batch >= 2 {
            samples.push(time);
        }
    }
    samples.sort_by(f64::total_cmp);
    let median = (samples[3] + samples[4]) * 0.5;
    eprintln!(
        "production NAADF compact{}: {median:.4} ms, {mismatches} oracle discrepancies",
        w[6]
    );
    Ok((format!("{{\"backend\":\"production-naadf-shadow-trace\",\"leaf_bits\":{},\"explicit_payload_bytes\":{},\"dispatch_p50_ms\":{median},\"mismatches\":{mismatches},\"reconstructed_cell_differences\":{reconstruction_differences},\"hit_distance_tolerance_native_cells\":0.0001,\"samples_ms\":{samples:?}}}",w[6],raw.len()-28),mismatches==0))
}
