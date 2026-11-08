//! Bounded, frozen-content architectural experiments. No game source replacement.
//! Native sparse bricks, coarse solid cells and axis-averaged coverage aggregates
//! are compared against rasterized faces from exactly the same seeded model.
use bytemuck::{Pod, Zeroable};
use frontier_world::{vegetation, Cell};
use glam::{Mat4, Vec3};
use std::{collections::BTreeMap, path::Path, time::Instant};
use wgpu::util::DeviceExt;
const SHADER: &str = include_str!("vegetation_lab.wgsl");
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Header {
    low: u32,
    high: u32,
    base: u32,
    pad: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    inverse: [f32; 16],
    matrix: [f32; 16],
    eye: [f32; 4],
    low: [f32; 4],
    high: [f32; 4],
    grid: [u32; 4],
    image: [u32; 4],
}
struct Snapshot {
    headers: Vec<Header>,
    values: Vec<[f32; 4]>,
    low: [f32; 3],
    high: [f32; 3],
    grid: [u32; 3],
    build_ms: f64,
}
fn snapshot(model: &vegetation::PlantModel, stride: i64, coverage: bool) -> Snapshot {
    let now = Instant::now();
    let mut groups: BTreeMap<Cell, Vec<&vegetation::PlantVoxel>> = BTreeMap::new();
    for v in &model.voxels {
        groups
            .entry(Cell {
                x: v.cell.x.div_euclid(stride),
                y: v.cell.y.div_euclid(stride),
                z: v.cell.z.div_euclid(stride),
            })
            .or_default()
            .push(v);
    }
    let min =
        [model.min.x, model.min.y, model.min.z].map(|v| v.div_euclid(stride).div_euclid(4) * 4);
    let end = [model.max.x, model.max.y, model.max.z]
        .map(|v| (v - 1).div_euclid(stride).div_euclid(4) * 4 + 4);
    let grid = std::array::from_fn(|d| ((end[d] - min[d]) / 4) as u32);
    assert!(
        grid.iter().product::<u32>() <= 1_000_000,
        "bounded lab brick extent"
    );
    let mut buckets: BTreeMap<usize, BTreeMap<usize, [f32; 4]>> = BTreeMap::new();
    for (c, vs) in groups {
        let q = [c.x - min[0], c.y - min[1], c.z - min[2]];
        let brick = (q[0] / 4 + grid[0] as i64 * (q[1] / 4 + grid[1] as i64 * (q[2] / 4))) as usize;
        let slot = ((q[0] % 4) * 16 + (q[1] % 4) * 4 + q[2] % 4) as usize;
        let mut color = [0f64; 3];
        for v in &vs {
            for d in 0..3 {
                color[d] += v.color[d] as f64;
            }
        }
        // Average projected occupied area of three axes. A display aggregate,
        // not a new semitransparent native material or an exact visibility bound.
        let mut projections = [
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
        ];
        for v in &vs {
            let p = [
                v.cell.x.rem_euclid(stride),
                v.cell.y.rem_euclid(stride),
                v.cell.z.rem_euclid(stride),
            ];
            for d in 0..3 {
                projections[d].insert((p[(d + 1) % 3], p[(d + 2) % 3]));
            }
        }
        let alpha = if coverage {
            projections.iter().map(|p| p.len() as f32).sum::<f32>()
                / (3. * (stride * stride) as f32)
        } else {
            1.
        };
        let n = vs.len() as f64;
        buckets.entry(brick).or_default().insert(
            slot,
            [
                (color[0] / n) as f32,
                (color[1] / n) as f32,
                (color[2] / n) as f32,
                alpha,
            ],
        );
    }
    let mut headers = vec![
        Header {
            low: 0,
            high: 0,
            base: 0,
            pad: 0
        };
        grid.iter().product::<u32>() as usize
    ];
    let mut values = Vec::new();
    for (brick, slots) in buckets {
        let h = &mut headers[brick];
        h.base = values.len() as u32;
        for (slot, value) in slots {
            if slot < 32 {
                h.low |= 1 << slot;
            } else {
                h.high |= 1 << (slot - 32);
            }
            values.push(value);
        }
    }
    Snapshot {
        headers,
        values,
        low: min.map(|v| v as f32 * stride as f32 * 0.0625),
        high: end.map(|v| v as f32 * stride as f32 * 0.0625),
        grid,
        build_ms: now.elapsed().as_secs_f64() * 1000.,
    }
}
fn json_string(value: &str) -> String {
    let mut result = String::from("\"");
    for c in value.chars() {
        match c {
            '\"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            c if c <= '\u{1f}' => result.push_str(&format!("\\u{:04x}", c as u32)),
            c => result.push(c),
        }
    }
    result.push('\"');
    result
}
fn save_png(
    path: &Path,
    data: &[[f32; 4]],
    w: u32,
    h: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(
        &data
            .iter()
            .flat_map(|p| {
                p[..3]
                    .iter()
                    .map(|v| (v.max(0.).powf(1. / 2.2).min(1.) * 255.).round() as u8)
            })
            .collect::<Vec<_>>(),
    )?;
    Ok(())
}
fn read(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Vec<u8> {
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv().unwrap().unwrap();
    let data = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    data
}
fn input(
    device: &wgpu::Device,
    label: &str,
    bytes: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytes,
        usage,
    })
}
/// Finite headless experiment, deliberately independent of live streaming.
pub fn run(output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(output)?;
    pollster::block_on(run_gpu(
        output,
        std::env::var("FRONTIER_VEGETATION_LAB_STRICT").as_deref() == Ok("1"),
    ))
}
const WARMUP: usize = 2;
const REPEATS: usize = 7;
struct Measurement {
    gpu_ms: Option<f64>,
    wall_ms: f64,
}
fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}
/// Each sample completes before the next. GPU timestamps exclude host readback;
/// wall samples include submission and synchronization, and are not game FPS.
fn measure(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    timestamps: bool,
    mut encode: impl FnMut(&mut wgpu::CommandEncoder, Option<&wgpu::QuerySet>),
) -> Measurement {
    for _ in 0..WARMUP {
        let mut encoder = device.create_command_encoder(&Default::default());
        encode(&mut encoder, None);
        queue.submit(Some(encoder.finish()));
        device.poll(wgpu::Maintain::Wait);
    }
    let queries = timestamps.then(|| {
        device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("warm lab samples"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        })
    });
    let resolve = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("lab timestamp resolve"),
        size: 256,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let download = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("lab timestamp readback"),
        size: 16,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut gpu = Vec::new();
    let mut wall = Vec::new();
    for _ in 0..REPEATS {
        let started = Instant::now();
        let mut encoder = device.create_command_encoder(&Default::default());
        encode(&mut encoder, queries.as_ref());
        if let Some(q) = &queries {
            encoder.resolve_query_set(q, 0..2, &resolve, 0);
            encoder.copy_buffer_to_buffer(&resolve, 0, &download, 0, 16);
        }
        queue.submit(Some(encoder.finish()));
        device.poll(wgpu::Maintain::Wait);
        wall.push(started.elapsed().as_secs_f64() * 1000.);
        if queries.is_some() {
            let bytes = read(device, &download);
            let ticks: &[u64] = bytemuck::cast_slice(&bytes);
            gpu.push(
                ticks[1].saturating_sub(ticks[0]) as f64 * queue.get_timestamp_period() as f64
                    / 1e6,
            );
        }
    }
    Measurement {
        gpu_ms: (!gpu.is_empty()).then(|| median(&mut gpu)),
        wall_ms: median(&mut wall),
    }
}
struct Image {
    color: Vec<[f32; 4]>,
    depth: Vec<f32>,
    mean_steps: f64,
    edge_distance: Vec<f32>,
}
fn texture(device: &wgpu::Device, width: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frozen vegetation lab target"),
        size: wgpu::Extent3d {
            width,
            height: width,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
fn metrics(image: &Image, reference: &Image) -> (f64, f64) {
    let n = image.color.len() as f64;
    let rmse = (image
        .color
        .iter()
        .zip(&reference.color)
        .flat_map(|(a, b)| (0..3).map(move |d| (a[d] as f64 - b[d] as f64).powi(2)))
        .sum::<f64>()
        / (n * 3.))
        .sqrt();
    let coverage = image
        .color
        .iter()
        .zip(&reference.color)
        .map(|(a, b)| (a[3] - b[3]).abs() as f64)
        .sum::<f64>()
        / n;
    (rmse, coverage)
}
fn report(
    label: &str,
    distance: f32,
    stride: i64,
    spp: u32,
    resident_bytes: usize,
    build_ms: f64,
    upload_cpu_ms: f64,
    upload_wall_ms: f64,
    timing: &Measurement,
    image: &Image,
    reference: &Image,
) -> String {
    let (rmse, coverage) = metrics(image, reference);
    format!("{{\"distance_heights\":{distance},\"method\":\"{label}\",\"stride\":{stride},\"spp\":{spp},\"resident_bytes\":{resident_bytes},\"build_ms\":{build_ms},\"upload_encode_cpu_ms\":{upload_cpu_ms},\"upload_submit_and_wait_wall_ms\":{upload_wall_ms},\"gpu_median_ms\":{},\"submit_and_wait_median_ms\":{},\"linear_rgb_rmse_vs_native16\":{rmse},\"coverage_mae_vs_native16\":{coverage},\"mean_steps\":{}}}",timing.gpu_ms.map_or("null".into(),|v|v.to_string()),timing.wall_ms,image.mean_steps)
}
/// Compare every pixel except rays explicitly within six micrometres of a
/// physical native-cell edge. Raster top-left rules and floating DDA ties can
/// select different adjacent faces there. Neighborhood color/depth similarity
/// does not control exclusion; every discrepancy is counted and reported.
/// This one-sided classifier is diagnostic, not a certified raster precision
/// bound: an occluded foreground raster edge need not be near the ray's hit.
/// A failed check leaves the experiment unpromoted and strict mode fails.
struct ParityReport {
    passed: bool,
    json: String,
}
fn parity(native: &Image, raster: &Image, width: usize, require_hits: bool) -> ParityReport {
    const EDGE_EPSILON: f32 = 0.0001; // native-cell units;0.00000625m
    let mut checked = 0usize;
    let mut checked_hits = 0usize;
    let mut differences = 0usize;
    let mut all_coverage_differences = 0usize;
    let mut all_rgb_differences = 0usize;
    let mut all_depth_differences = 0usize;
    let mut excluded = 0usize;
    let mut excluded_mismatches = Vec::new();
    let mut failures = Vec::new();
    let mut max_depth = 0f32;
    let mut max_rgb = 0f32;
    for i in 0..native.color.len() {
        let hit = raster.color[i][3] > 0.5;
        let native_hit = native.color[i][3] > 0.5;
        let coverage_difference = native_hit != hit;
        let depth = (native.depth[i] - raster.depth[i]).abs();
        let rgb = (0..3)
            .map(|d| (native.color[i][d] - raster.color[i][d]).abs())
            .fold(0., f32::max);
        all_coverage_differences += coverage_difference as usize;
        all_rgb_differences += (rgb > 0.0002) as usize;
        all_depth_differences += (depth > 0.00002) as usize;
        let mismatch = coverage_difference || depth > 0.00002 || rgb > 0.0002;
        let edge = native.edge_distance[i];
        let detail = || {
            format!("{{\"pixel\":[{},{}],\"edge_distance_native_cells\":{edge},\"rgb_abs_error\":{rgb},\"depth_abs_error\":{depth},\"coverage_mismatch\":{coverage_difference}}}",i%width,i/width)
        };
        if native_hit && edge < EDGE_EPSILON {
            excluded += 1;
            if mismatch && excluded_mismatches.len() < 32 {
                excluded_mismatches.push(detail());
            }
            continue;
        }
        checked += 1;
        checked_hits += hit as usize;
        max_depth = max_depth.max(depth);
        max_rgb = max_rgb.max(rgb);
        if mismatch {
            differences += 1;
            if failures.len() < 16 {
                failures.push(detail());
            }
        }
    }
    let required_hits_present = !require_hits || checked_hits > 0;
    let passed = differences == 0 && required_hits_present;
    let json=format!("{{\"passed\":{passed},\"required_hit_pixels_present\":{required_hits_present},\"nonedge_pixels\":{checked},\"nonedge_hit_pixels\":{checked_hits},\"nonedge_mismatches\":{differences},\"failures\":[{}],\"excluded_native_edge_pixels\":{excluded},\"native_edge_epsilon_cells\":{EDGE_EPSILON},\"excluded_mismatches\":[{}],\"all_pixel_coverage_mismatches\":{all_coverage_differences},\"all_pixel_rgb_mismatches\":{all_rgb_differences},\"all_pixel_depth_mismatches\":{all_depth_differences},\"max_nonedge_depth_error\":{max_depth},\"max_nonedge_rgb_error\":{max_rgb},\"depth_tolerance\":0.00002,\"rgb_tolerance\":0.0002,\"edge_classifier_scope\":\"diagnostic native first-hit tangential edge clearance; does not classify occluded raster foreground edges or fixed-subpixel raster quantization\"}}",failures.join(","),excluded_mismatches.join(","));
    ParityReport { passed, json }
}

async fn run_gpu(output: &Path, strict: bool) -> Result<(), Box<dyn std::error::Error>> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..Default::default()
    });
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: None,
            force_fallback_adapter: false,
            power_preference: wgpu::PowerPreference::HighPerformance,
        })
        .await
        .ok_or("no GPU")?;
    let features = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
    let (device, queue) = adapter
        .request_device(
            &wgpu::DeviceDescriptor {
                label: Some("vegetation representation lab"),
                required_features: features,
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
            },
            None,
        )
        .await?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("vegetation lab"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("sparse vegetation lab"),
        layout: None,
        module: &shader,
        entry_point: Some("trace_image"),
        compilation_options: Default::default(),
        cache: None,
    });
    let attributes = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x3];
    let raster = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("matched native face raster baseline"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<crate::vegetation::VegetationVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba32Float,
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
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });
    let width = 256u32;
    let pixels = (width * width) as usize;
    let mut reports = Vec::new();
    let mut checks = Vec::new();
    let mut all_parity_passed = true;
    let generated = Instant::now();
    let model = vegetation::generate_rowan(123, 3);
    let generation_ms = generated.elapsed().as_secs_f64() * 1000.;
    let mesh_started = Instant::now();
    let mesh = crate::vegetation_mesher::mesh_cells_scaled(
        model
            .voxels
            .iter()
            .map(|v| (v.cell, v.material as u8, v.color)),
        1,
        1,
        || false,
    );
    let mesh_build_ms = mesh_started.elapsed().as_secs_f64() * 1000.;
    let upload = Instant::now();
    let vertices = input(
        &device,
        "native seeded face vertices",
        bytemuck::cast_slice(&mesh.vertices),
        wgpu::BufferUsages::VERTEX,
    );
    let indices = input(
        &device,
        "native seeded face indices",
        bytemuck::cast_slice(&mesh.indices),
        wgpu::BufferUsages::INDEX,
    );
    let mesh_upload_cpu_ms = upload.elapsed().as_secs_f64() * 1000.;
    queue.submit([]);
    device.poll(wgpu::Maintain::Wait);
    let mesh_upload_wall_ms = upload.elapsed().as_secs_f64() * 1000.;
    let raster_bytes = mesh.vertices.len()
        * std::mem::size_of::<crate::vegetation::VegetationVertex>()
        + mesh.indices.len() * 4;
    let color = texture(&device, width, wgpu::TextureFormat::Rgba32Float);
    let depth = texture(&device, width, wgpu::TextureFormat::Depth32Float);
    let color_view = color.create_view(&Default::default());
    let depth_view = depth.create_view(&Default::default());
    for distance in [2f32, 8., 32.] {
        let center = Vec3::new(
            (model.min.x + model.max.x) as f32 * 0.03125,
            (model.min.y + model.max.y) as f32 * 0.03125,
            (model.min.z + model.max.z) as f32 * 0.03125,
        );
        let height = (model.max.y - model.min.y) as f32 * 0.0625;
        let eye = center + Vec3::new(0.65, 0.38, 1.).normalize() * height * distance;
        let matrix = Mat4::perspective_rh(50f32.to_radians(), 1., 0.025, 1000.)
            * Mat4::look_at_rh(eye, center, Vec3::Y);
        let mut reference: Option<Image> = None;
        let mut native: Option<Image> = None;
        let mut raster_uniform = None;
        for (label, stride, coverage, spp) in [
            ("reference16", 1, false, 16),
            ("native-bricks", 1, false, 1),
            ("solid4", 4, false, 1),
            ("coverage4", 4, true, 1),
            ("solid8", 8, false, 1),
            ("coverage8", 8, true, 1),
        ] {
            let snap = snapshot(&model, stride, coverage);
            let params = Params {
                inverse: matrix.inverse().to_cols_array(),
                matrix: matrix.to_cols_array(),
                eye: [eye.x, eye.y, eye.z, 0.],
                low: [snap.low[0], snap.low[1], snap.low[2], 0.],
                high: [snap.high[0], snap.high[1], snap.high[2], 0.],
                grid: [snap.grid[0], snap.grid[1], snap.grid[2], spp],
                image: [width, width, if coverage { 2 } else { 0 }, stride as u32],
            };
            let started = Instant::now();
            let uniform = input(
                &device,
                "lab camera",
                bytemuck::bytes_of(&params),
                wgpu::BufferUsages::UNIFORM,
            );
            if label == "native-bricks" {
                raster_uniform = Some(uniform.clone());
            }
            let headers = input(
                &device,
                "sparse brick masks",
                bytemuck::cast_slice(&snap.headers),
                wgpu::BufferUsages::STORAGE,
            );
            let values = input(
                &device,
                "ranked native colors",
                bytemuck::cast_slice(&snap.values),
                wgpu::BufferUsages::STORAGE,
            );
            let upload_cpu_ms = started.elapsed().as_secs_f64() * 1000.;
            queue.submit([]);
            device.poll(wgpu::Maintain::Wait);
            let upload_wall_ms = started.elapsed().as_secs_f64() * 1000.;
            let result = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("lab pixels"),
                size: pixels as u64 * 32,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let back = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("lab readback"),
                size: pixels as u64 * 32,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("frozen local plants"),
                layout: &compute.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: headers.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: values.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: result.as_entire_binding(),
                    },
                ],
            });
            let timing = measure(
                &device,
                &queue,
                features.contains(wgpu::Features::TIMESTAMP_QUERY),
                |encoder, queries| {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("bounded local traversal"),
                        timestamp_writes: queries.map(|q| wgpu::ComputePassTimestampWrites {
                            query_set: q,
                            beginning_of_pass_write_index: Some(0),
                            end_of_pass_write_index: Some(1),
                        }),
                    });
                    pass.set_pipeline(&compute);
                    pass.set_bind_group(0, &bind, &[]);
                    pass.dispatch_workgroups(width.div_ceil(8), width.div_ceil(8), 1);
                },
            );
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_buffer_to_buffer(&result, 0, &back, 0, pixels as u64 * 32);
            queue.submit(Some(encoder.finish()));
            let bytes = read(&device, &back);
            let raw: &[[f32; 8]] = bytemuck::cast_slice(&bytes);
            let image = Image {
                color: raw.iter().map(|p| [p[0], p[1], p[2], p[3]]).collect(),
                depth: raw.iter().map(|p| p[4]).collect(),
                mean_steps: raw.iter().map(|p| p[5] as f64).sum::<f64>() / pixels as f64,
                edge_distance: raw.iter().map(|p| p[6]).collect(),
            };
            save_png(
                &output.join(format!("distance{distance}-{label}.png")),
                &image.color,
                width,
                width,
            )?;
            let r = reference.as_ref().unwrap_or(&image);
            reports.push(report(
                label,
                distance,
                stride,
                spp,
                snap.headers.len() * 16 + snap.values.len() * 16,
                snap.build_ms,
                upload_cpu_ms,
                upload_wall_ms,
                &timing,
                &image,
                r,
            ));
            if label == "reference16" {
                reference = Some(image);
            } else if label == "native-bricks" {
                native = Some(image);
            }
        }
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("matched raster camera"),
            layout: &raster.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: raster_uniform.as_ref().unwrap().as_entire_binding(),
            }],
        });
        let timing = measure(
            &device,
            &queue,
            features.contains(wgpu::Features::TIMESTAMP_QUERY),
            |encoder, queries| {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("native face raster baseline"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.34,
                                g: 0.57,
                                b: 0.8,
                                a: 0.,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: queries.map(|q| wgpu::RenderPassTimestampWrites {
                        query_set: q,
                        beginning_of_pass_write_index: Some(0),
                        end_of_pass_write_index: Some(1),
                    }),
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&raster);
                pass.set_bind_group(0, &bind, &[]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.indices.len() as u32, 0, 0..1);
            },
        );
        let downloads: Vec<_> = [16usize, 4]
            .into_iter()
            .map(|stride| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("raster lab readback"),
                    size: (pixels * stride) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let mut encoder = device.create_command_encoder(&Default::default());
        for ((target, buffer), stride) in
            [&color, &depth].into_iter().zip(&downloads).zip([16u32, 4])
        {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: target,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(width * stride),
                        rows_per_image: Some(width),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height: width,
                    depth_or_array_layers: 1,
                },
            );
        }
        queue.submit(Some(encoder.finish()));
        let color_bytes = read(&device, &downloads[0]);
        let depth_bytes = read(&device, &downloads[1]);
        let image = Image {
            color: bytemuck::cast_slice::<_, [f32; 4]>(&color_bytes).to_vec(),
            depth: bytemuck::cast_slice::<_, f32>(&depth_bytes).to_vec(),
            mean_steps: 0.,
            edge_distance: Vec::new(),
        };
        save_png(
            &output.join(format!("distance{distance}-raster-native.png")),
            &image.color,
            width,
            width,
        )?;
        let check = parity(
            native.as_ref().unwrap(),
            &image,
            width as usize,
            distance == 2.,
        );
        all_parity_passed &= check.passed;
        checks.push(format!(
            "{{\"distance_heights\":{distance},\"check\":{}}}",
            check.json
        ));
        reports.push(report(
            "raster-native",
            distance,
            1,
            1,
            raster_bytes,
            mesh_build_ms,
            mesh_upload_cpu_ms,
            mesh_upload_wall_ms,
            &timing,
            &image,
            reference.as_ref().unwrap(),
        ));
    }
    let generation_report = crate::vegetation_generation_lab::run(&device, &queue)
        .await
        .map_err(|e| format!("GPU generation lab: {e}"))?;
    let parity_diagnostic = if all_parity_passed {
        "passed measured native/raster criterion with explicit native-only edge exclusions"
    } else {
        "strict native/raster criteria failed; unresolved raster subpixel/foreground edge ownership or traversal differences; prototype not promoted"
    };
    std::fs::write(output.join("report.json"),format!("{{\"schema\":\"vegetation_local_lab_v2\",\"adapter\":{},\"seed\":123,\"variant\":3,\"voxel_m\":0.0625,\"width\":256,\"height\":256,\"generation_ms\":{generation_ms},\"warmup_dispatches\":{WARMUP},\"timed_samples\":{REPEATS},\"scope\":\"frozen single-tree kernels, simplified matched Lambert lighting, no game FPS or streaming inference; sparse storage excludes output/readback targets; upload wall timings include host submission/synchronization; raster mesh build/upload is shared across distances and repeated in rows for comparison; reference16 is a finite 4x4-sample estimate, not ground truth\",\"runs\":[{}],\"native_raster_parity\":[{}],\"native_raster_parity_passed\":{all_parity_passed},\"promoted\":false,\"parity_diagnostic\":{},\"procedural_generation\":{generation_report}}}",json_string(&adapter.get_info().name),reports.join(","),checks.join(","),json_string(parity_diagnostic)))?;
    if strict && !all_parity_passed {
        return Err(format!(
            "native sparse/raster parity failed; full diagnostic report saved to {}",
            output.join("report.json").display()
        )
        .into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shader_valid() {
        let module = naga::front::wgsl::parse_str(SHADER).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
    #[test]
    fn ranked_bricks_preserve_every_native_color_and_empty_address() {
        for model in [
            vegetation::generate_grass(7, 3),
            vegetation::generate_rowan(123, 3),
        ] {
            let snap = snapshot(&model, 1, false);
            let expected: BTreeMap<_, _> = model
                .voxels
                .iter()
                .map(|v| (v.cell, v.color.map(f32::to_bits)))
                .collect();
            let mut decoded = BTreeMap::new();
            let low = snap.low.map(|v| (v * 16.) as i64);
            for (brick, header) in snap.headers.iter().enumerate() {
                let bx = brick as u32 % snap.grid[0];
                let by = brick as u32 / snap.grid[0] % snap.grid[1];
                let bz = brick as u32 / (snap.grid[0] * snap.grid[1]);
                for slot in 0..64u32 {
                    let mask = if slot < 32 { header.low } else { header.high };
                    let bit = slot % 32;
                    if mask & (1 << bit) == 0 {
                        continue;
                    }
                    let rank = if slot < 32 {
                        0
                    } else {
                        header.low.count_ones()
                    } + (mask & ((1u32 << bit) - 1)).count_ones();
                    let value = snap.values[(header.base + rank) as usize];
                    assert_eq!(value[3], 1.);
                    let cell = Cell {
                        x: low[0] + (bx * 4 + slot / 16) as i64,
                        y: low[1] + (by * 4 + slot / 4 % 4) as i64,
                        z: low[2] + (bz * 4 + slot % 4) as i64,
                    };
                    assert!(decoded
                        .insert(
                            cell,
                            [value[0].to_bits(), value[1].to_bits(), value[2].to_bits()]
                        )
                        .is_none());
                }
            }
            assert_eq!(decoded, expected);
            let coarse = snapshot(&model, 4, true);
            assert!(coarse.values.iter().all(|v| v[3] > 0. && v[3] <= 1.));
        }
    }
    #[test]
    fn parity_excludes_only_measured_native_edges() {
        let raster = Image {
            color: vec![[1., 1., 1., 1.]; 3],
            depth: vec![0.5; 3],
            edge_distance: Vec::new(),
            mean_steps: 0.,
        };
        let mut native = Image {
            color: vec![[0., 0., 0., 1.], [0., 0., 0., 1.], [1., 1., 1., 1.]],
            depth: vec![0.5; 3],
            edge_distance: vec![0.00001, 0.001, 0.25],
            mean_steps: 0.,
        };
        // Neighboring matching pixels cannot hide an actual nonedge mismatch.
        assert!(!parity(&native, &raster, 3, true).passed);
        native.color[1] = raster.color[1];
        let check = parity(&native, &raster, 3, true);
        assert!(check.passed);
        let report = check.json;
        assert!(report.contains("\"excluded_native_edge_pixels\":1"));
        assert!(report.contains("\"all_pixel_rgb_mismatches\":1"));
        assert!(report.contains("\"nonedge_hit_pixels\":2"));
    }
    #[test]
    #[ignore = "requires explicitly selected hardware GPU; executes finite warmed raster/ray lab"]
    fn native_lab_gpu_parity() {
        let output =
            std::env::temp_dir().join(format!("frontier-vegetation-lab-{}", std::process::id()));
        std::fs::create_dir_all(&output).unwrap();
        pollster::block_on(run_gpu(&output, true)).unwrap();
    }
}
