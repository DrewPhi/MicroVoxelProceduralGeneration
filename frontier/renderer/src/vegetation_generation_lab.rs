//! Headless-only procedural expansion experiment. Canonical CPU generation stays
//! authoritative. GPU readback and dense-grid allocation never enter gameplay.
use frontier_world::{
    vegetation::{
        plant_recipe::{grass_recipe, rowan_recipe},
        PlantModel,
    },
    Cell,
};
use std::{collections::BTreeMap, time::Instant};
use wgpu::util::DeviceExt;
const VOLUME: usize = 33 * 16 * 33;
const MODELS: usize = 16;
fn read(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Result<Vec<u8>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let data = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    Ok(data)
}
fn storage(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
fn address(p: [i16; 3]) -> usize {
    (p[0] as i32 + 16) as usize * 528 + p[1] as usize * 33 + (p[2] as i32 + 16) as usize
}
pub(crate) async fn run(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<String, String> {
    let mut rule_ms = 0.;
    let mut materialization_ms = 0.;
    let mut recipe_bytes = 0usize;
    let mut strokes = Vec::<[u32; 8]>::new();
    let mut seeds = Vec::<[u32; 2]>::new();
    let mut expected_pigments = vec![0u32; VOLUME * MODELS];
    let mut models = Vec::<PlantModel>::new();
    for n in 0..MODELS {
        let seed = (n as u64).wrapping_mul(0x9e3779b97f4a7c15) ^ 0xab174cc9ee231f91;
        let started = Instant::now();
        let recipe = grass_recipe(seed, (n % 8) as u8);
        rule_ms += started.elapsed().as_secs_f64() * 1000.;
        recipe_bytes += recipe.payload_bytes();
        seeds.push([seed as u32, (seed >> 32) as u32]);
        for s in &recipe.strokes {
            for p in [s.from, s.to] {
                if !(-16..=16).contains(&p[0])
                    || !(0..16).contains(&p[1])
                    || !(-16..=16).contains(&p[2])
                {
                    return Err("grass recipe exceeds bounded lab grid".into());
                }
            }
            strokes.push([
                s.from[0] as i32 as u32,
                s.from[1] as i32 as u32,
                s.from[2] as i32 as u32,
                n as u32,
                s.to[0] as i32 as u32,
                s.to[1] as i32 as u32,
                s.to[2] as i32 as u32,
                s.pigment_material,
            ]);
            // Independent CPU raster oracle, not uploaded to GPU.
            let mut p = s.from;
            loop {
                expected_pigments[n * VOLUME + address(p)] = s.pigment_material;
                if p == s.to {
                    break;
                }
                let axis = (0..3).max_by_key(|&i| (s.to[i] - p[i]).abs()).unwrap();
                p[axis] += (s.to[axis] - p[axis]).signum();
            }
        }
        let started = Instant::now();
        models.push(recipe.materialize());
        materialization_ms += started.elapsed().as_secs_f64() * 1000.;
    }
    // Tree experiment stays CPU-only: preserve the existing exposed-cell bloom
    // phase instead of pretending compact scaffold generation completes a tree.
    let mut rowan_rule_ms = 0.;
    let mut rowan_materialization_ms = 0.;
    let mut rowan_recipe_bytes = 0usize;
    let mut rowan_native_cells = 0usize;
    let mut rowan_wood_strokes = 0usize;
    let mut rowan_foliage_sprays = 0usize;
    for variant in 0..8 {
        let started = Instant::now();
        let recipe = rowan_recipe(0xaba715792a31 ^ variant as u64, variant);
        rowan_rule_ms += started.elapsed().as_secs_f64() * 1000.;
        rowan_recipe_bytes += recipe.payload_bytes();
        rowan_wood_strokes += recipe.wood.len();
        rowan_foliage_sprays += recipe.foliage.len();
        let started = Instant::now();
        let model = recipe.materialize();
        rowan_materialization_ms += started.elapsed().as_secs_f64() * 1000.;
        rowan_native_cells += model.voxels.len();
    }
    let initialization = Instant::now();
    let cells = VOLUME * MODELS;
    let native_cells: usize = models.iter().map(|m| m.voxels.len()).sum();
    let upload_bytes = strokes.len() * 32 + seeds.len() * 8 + 16;
    let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("recipe lab params"),
        contents: bytemuck::cast_slice(&[MODELS as u32, strokes.len() as u32, cells as u32, 0]),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let stroke_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("procedural grass strokes, not cells"),
        contents: bytemuck::cast_slice(&strokes),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let seed_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("recipe seed identity"),
        contents: bytemuck::cast_slice(&seeds),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let owners = storage(device, "bounded raster owners", (cells * 4) as u64);
    let pigments = storage(device, "bounded raster pigment", (cells * 4) as u64);
    let colors = storage(device, "bounded raster colors", (cells * 16) as u64);
    let size = (cells * 20) as u64;
    let download = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("headless recipe parity readback"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("bounded procedural grass expansion"),
        source: wgpu::ShaderSource::Wgsl(include_str!("vegetation_generation_lab.wgsl").into()),
    });
    // Both entry points use the explicit shared layout: auto layouts omit bindings
    // that are statically unused by one entry point.
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("recipe lab layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            storage_entry(1, true),
            storage_entry(2, true),
            storage_entry(3, false),
            storage_entry(4, false),
            storage_entry(5, false),
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("recipe lab pipelines"),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let raster = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("recipe native occupancy"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("raster"),
        compilation_options: Default::default(),
        cache: None,
    });
    let resolve = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("recipe pigment and color"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("resolve"),
        compilation_options: Default::default(),
        cache: None,
    });
    let buffers = [
        &params,
        &stroke_buffer,
        &seed_buffer,
        &owners,
        &pigments,
        &colors,
    ];
    let entries: Vec<_> = buffers
        .iter()
        .enumerate()
        .map(|(binding, b)| wgpu::BindGroupEntry {
            binding: binding as u32,
            resource: b.as_entire_binding(),
        })
        .collect();
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("recipe experiment"),
        layout: &layout,
        entries: &entries,
    });
    if let Some(error) = device.pop_error_scope().await {
        return Err(error.to_string());
    }
    let timestamp = device.features().contains(wgpu::Features::TIMESTAMP_QUERY);
    let queries = timestamp.then(|| {
        device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("recipe expansion timing"),
            ty: wgpu::QueryType::Timestamp,
            count: 4,
        })
    });
    let query_buffer = timestamp.then(|| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("recipe timestamps"),
            size: 32,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    });
    let query_read = timestamp.then(|| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("recipe time readback"),
            size: 32,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    });
    let initialization_ms = initialization.elapsed().as_secs_f64() * 1000.;
    let submit_started = Instant::now();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("recipe lab dispatch"),
    });
    encoder.clear_buffer(&owners, 0, None);
    for (pipeline, workgroups, start) in [
        (&raster, (strokes.len() as u32 + 63) / 64, 0),
        (&resolve, (cells as u32 + 63) / 64, 2),
    ] {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("procedural expansion stage"),
            timestamp_writes: queries.as_ref().map(|q| wgpu::ComputePassTimestampWrites {
                query_set: q,
                beginning_of_pass_write_index: Some(start),
                end_of_pass_write_index: Some(start + 1),
            }),
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(workgroups, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&pigments, 0, &download, 0, (cells * 4) as u64);
    encoder.copy_buffer_to_buffer(
        &colors,
        0,
        &download,
        (cells * 4) as u64,
        (cells * 16) as u64,
    );
    if let (Some(q), Some(buffer), Some(read)) = (&queries, &query_buffer, &query_read) {
        encoder.resolve_query_set(q, 0..4, buffer, 0);
        encoder.copy_buffer_to_buffer(buffer, 0, read, 0, 32);
    }
    queue.submit(Some(encoder.finish()));
    let output = read(device, &download)?;
    let submit_readback_ms = submit_started.elapsed().as_secs_f64() * 1000.;
    let gpu_times = if let Some(buffer) = &query_read {
        let data = read(device, buffer)?;
        let q: Vec<_> = data
            .chunks_exact(8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
            .collect();
        Some([
            (q[1] - q[0]) as f64 * queue.get_timestamp_period() as f64 / 1e6,
            (q[3] - q[2]) as f64 * queue.get_timestamp_period() as f64 / 1e6,
        ])
    } else {
        None
    };
    let words: Vec<u32> = output[..cells * 4]
        .chunks_exact(4)
        .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
        .collect();
    let pigment_mismatches = words
        .iter()
        .zip(&expected_pigments)
        .filter(|(a, b)| a != b)
        .count();
    let mut occupancy_mismatches = 0;
    let mut float_component_bit_mismatches = 0;
    let mut max_color_abs_error = 0f32;
    let mut nonfinite_color_components = 0usize;
    for (n, model) in models.iter().enumerate() {
        let oracle: BTreeMap<_, _> = model.voxels.iter().map(|v| (v.cell, v.color)).collect();
        for local in 0..VOLUME {
            let cell = Cell {
                x: (local / 528) as i64 - 16,
                y: ((local / 33) % 16) as i64,
                z: (local % 33) as i64 - 16,
            };
            let index = n * VOLUME + local;
            let expected = oracle.get(&cell);
            occupancy_mismatches += usize::from((words[index] != 0) != expected.is_some());
            if let Some(expected) = expected {
                for channel in 0..3 {
                    let offset = cells * 4 + index * 16 + channel * 4;
                    let actual = f32::from_le_bytes(output[offset..offset + 4].try_into().unwrap());
                    nonfinite_color_components += usize::from(!actual.is_finite());
                    float_component_bit_mismatches +=
                        usize::from(actual.to_bits() != expected[channel].to_bits());
                    max_color_abs_error =
                        max_color_abs_error.max((actual - expected[channel]).abs());
                }
            }
        }
    }
    let (raster_ms, color_ms) = gpu_times.map_or(("null".into(), "null".into()), |t| {
        (t[0].to_string(), t[1].to_string())
    });
    Ok(format!("{{\"models\":{MODELS},\"source_version\":{},\"cpu_rule_ms\":{rule_ms},\"cpu_materialization_ms\":{materialization_ms},\"recipe_bytes\":{recipe_bytes},\"gpu_recipe_upload_bytes\":{upload_bytes},\"native_cells\":{native_cells},\"native_voxel_struct_bytes\":{},\"bounded_grid_cells\":{cells},\"gpu_grid_bytes\":{},\"initialization_ms\":{initialization_ms},\"gpu_raster_ms\":{raster_ms},\"gpu_color_ms\":{color_ms},\"submit_and_readback_wall_ms\":{submit_readback_ms},\"pigment_mismatches\":{pigment_mismatches},\"occupancy_mismatches\":{occupancy_mismatches},\"float_component_bit_mismatches\":{float_component_bit_mismatches},\"max_color_abs_error\":{max_color_abs_error},\"nonfinite_color_components\":{nonfinite_color_components},\"promoted\":false,\"rowan_structure\":{{\"models\":8,\"rule_ms\":{rowan_rule_ms},\"materialization_including_blooms_ms\":{rowan_materialization_ms},\"recipe_bytes\":{rowan_recipe_bytes},\"native_cells\":{rowan_native_cells},\"native_voxel_struct_bytes\":{},\"wood_strokes\":{rowan_wood_strokes},\"foliage_sprays\":{rowan_foliage_sprays},\"gpu_expansion\":false}},\"scope\":\"headless grass recipe rasterization; descriptor payload excludes Vec headers/capacity; native bytes exclude lookup index; dense grid and readback excluded from gameplay\"}}",frontier_world::vegetation::VEGETATION_SOURCE_VERSION,native_cells*std::mem::size_of::<frontier_world::vegetation::PlantVoxel>(),cells*24,rowan_native_cells*std::mem::size_of::<frontier_world::vegetation::PlantVoxel>()))
}
fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn bounded_recipe_shader_validates() {
        let module =
            naga::front::wgsl::parse_str(include_str!("vegetation_generation_lab.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
}
