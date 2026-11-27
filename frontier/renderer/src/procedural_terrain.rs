//! Experimental Alpine ray renderer. The GPU cache stores upper bounds only.
use crate::{settings::VisualSettings, Camera};
use frontier_world::FbmTerrain;
use glam::{DVec3, Vec3};
use std::{cell::Cell, time::Instant};

pub(super) fn tiled_hot_probe_requested() -> bool {
    std::env::var_os("FRONTIER_PROCEDURAL_HOT_PROBE").is_some_and(|v| v == "1")
        && std::env::var_os("FRONTIER_PROCEDURAL_FINE_CACHE").is_some_and(|v| v == "2")
        && std::env::var_os("FRONTIER_PROCEDURAL_SWEEP").is_none_or(|v| v == "0")
}

const SIDE: i32 = 512;
const BLOCK: f64 = 32.0;
const WAVELENGTHS: [f64; 9] = [750., 240., 1800., 850., 180., 480., 45., 22.5, 11.25];
const FINE_COUNTER_OFFSET: u64 = 4096 * 4096 * 2;
const SWEEP_PIXELS: u32 = 1920 * 1080;
const BEAM_COUNT: u32 = 512 * 512;
const BYTES: u64 = 4 * (SIDE * SIDE + 128 * 128 + 32 * 32) as u64;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct View {
    projection: [f32; 16],
    eye: [f32; 4],
    forward: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    sun: [f32; 4],
    settings: [f32; 4],
    tile: [f32; 4],
    grid: [i32; 2],
    seed: [u32; 2],
    flags: [u32; 4],
    lattice: [[i32; 4]; 9],
    output: [u32; 4],
    cache: [u32; 4],
}

pub(super) struct ProceduralTerrain {
    compute: wgpu::ComputePipeline,
    build_bind: wgpu::BindGroup,
    reductions: [(wgpu::ComputePipeline, wgpu::BindGroup); 2],
    lattice_fill: (wgpu::ComputePipeline, wgpu::BindGroup),
    lattice_bands: [(i32, i32); 9],
    lattice_bytes: u64,
    fine: wgpu::Buffer,
    fine_mode: u32,
    fine_capacity: [u32; 2],
    beam: (wgpu::ComputePipeline, wgpu::BindGroup),
    beam_enabled: bool,
    sweep: Option<(wgpu::ComputePipeline, wgpu::BindGroup)>,
    sweep_draws: Cell<u64>,
    sweep_rows: u32,
    last_beam_view: Cell<Option<(Camera, [u32; 2])>>,
    pipeline: wgpu::RenderPipeline,
    hot_pipeline: Option<wgpu::RenderPipeline>,
    hot_draws: Cell<u64>,
    warm_draws: Cell<u32>,
    render_bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    last_grid: Cell<Option<[i32; 2]>>,
    source: FbmTerrain,
    diagnostic: bool,
    tile: [u32; 3],
    started: Instant,
    startup_encode_ms: Cell<Option<f64>>,
    builds: Cell<u64>,
}

impl ProceduralTerrain {
    pub(super) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        source: &FbmTerrain,
    ) -> crate::Result<Self> {
        let tile = capture_tile()?;
        let started = Instant::now();
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("procedural terrain view"),
            size: size_of::<View>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bounds = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("procedural terrain upper bounds"),
            size: BYTES,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let metres = f64::from(SIDE) * BLOCK * 1800. / source.params().horizontal_scale;
        let mut words = 0_i32;
        let use_lattice = std::env::var_os("FRONTIER_PROCEDURAL_LATTICE").is_some_and(|v| v == "1");
        let lattice_bands = WAVELENGTHS.map(|wave| {
            let side = (metres / wave).ceil() as i32;
            let side = if use_lattice && side <= 1528 {
                side + 8
            } else {
                0
            };
            let offset = words;
            words += side * side;
            (side, offset)
        });
        let lattice_bytes = 4 * words.max(1) as u64;
        let lattice = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("procedural terrain gradient lattice"),
            size: lattice_bytes,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let fine_mode = match std::env::var("FRONTIER_PROCEDURAL_FINE_CACHE").as_deref() {
            Ok("1") => 1,
            Ok("2") => 2,
            _ => 0,
        };
        let wide = std::env::var_os("FRONTIER_PROCEDURAL_WIDE_CACHE").is_some_and(|v| v == "1");
        let fine_capacity = if wide {
            [1 << 24, 1 << 23]
        } else {
            [1 << 22, 1 << 22]
        };
        let fine_bytes = match fine_mode {
            1 => FINE_COUNTER_OFFSET + 16 + u64::from(fine_capacity[0]) * 8,
            2 => FINE_COUNTER_OFFSET + 16 + u64::from(fine_capacity[0] + fine_capacity[1]) * 8,
            _ => 4,
        };
        if u64::from(device.limits().max_storage_buffer_binding_size) < fine_bytes {
            return Err(
                "the procedural fine cache exceeds this device's storage binding limit".into(),
            );
        }
        let fine = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("procedural terrain fine interval cache"),
            size: fine_bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let beam_enabled = std::env::var_os("FRONTIER_PROCEDURAL_BEAMS").is_some_and(|v| v == "1");
        let beam = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("procedural terrain shared empty prefixes"),
            size: if beam_enabled {
                u64::from(BEAM_COUNT) * 4
            } else {
                4
            },
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let sweep_mode = match std::env::var("FRONTIER_PROCEDURAL_SWEEP").as_deref() {
            Ok("1") => 1,
            Ok("2") => 2,
            _ => 0,
        };
        let sweep_enabled = sweep_mode != 0;
        let sweep_rows = std::env::var("FRONTIER_PROCEDURAL_SWEEP_ROWS")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|rows| (8..=256).contains(rows))
            .unwrap_or(8);
        let hits = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("procedural terrain swept hits"),
            size: if sweep_enabled {
                u64::from(SWEEP_PIXELS) * 16
            } else {
                16
            },
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Alpine procedural terrain"),
            source: wgpu::ShaderSource::Wgsl(include_str!("procedural_terrain.wgsl").into()),
        });
        let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Alpine upper bounds"),
            layout: None,
            module: &shader,
            entry_point: Some("build"),
            compilation_options: Default::default(),
            cache: None,
        });
        let traversal_bindings = [0, 2, 4, 5, 7, 9].map(|binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: if binding == 0 {
                    wgpu::BufferBindingType::Uniform
                } else {
                    wgpu::BufferBindingType::Storage {
                        read_only: binding != 5,
                    }
                },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        let traversal_bind_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("terrain traversal buffers"),
                entries: &traversal_bindings,
            });
        let traversal_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain traversal pipelines"),
            bind_group_layouts: &[&traversal_bind_layout],
            push_constant_ranges: &[],
        });
        let create_pipeline = |mode| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Alpine native column rays"),
                layout: Some(&traversal_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fragment"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &std::collections::HashMap::from([(
                            "FINE_MODE".to_owned(),
                            f64::from(mode),
                        )]),
                        ..Default::default()
                    },
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
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
            })
        };
        let pipeline = create_pipeline(fine_mode);
        let hot_pipeline = tiled_hot_probe_requested().then(|| create_pipeline(3_u32));
        let bind = |layout, index, lattice_index| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("procedural terrain view and bounds"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: index,
                        resource: bounds.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: lattice_index,
                        resource: lattice.as_entire_binding(),
                    },
                ],
            })
        };
        let render_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("procedural terrain traversal resources"),
            layout: &traversal_bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: bounds.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: lattice.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: fine.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: beam.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: hits.as_entire_binding(),
                },
            ],
        });
        let reductions = ["reduce_128", "reduce_512"].map(|entry| {
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(entry),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 1,
                    resource: bounds.as_entire_binding(),
                }],
            });
            (pipeline, bind)
        });
        let fill = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fill procedural terrain gradient lattice"),
            layout: None,
            module: &shader,
            entry_point: Some("fill_lattice"),
            compilation_options: Default::default(),
            cache: None,
        });
        let fill_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gradient lattice output"),
            layout: &fill.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: lattice.as_entire_binding(),
                },
            ],
        });
        let beam_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("procedural terrain ray packets"),
            layout: None,
            module: &shader,
            entry_point: Some("beam_prefix"),
            compilation_options: Default::default(),
            cache: None,
        });
        let beam_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("procedural terrain packet inputs and output"),
            layout: &beam_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: bounds.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: beam.as_entire_binding(),
                },
            ],
        });
        let sweep = sweep_enabled.then(|| {
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("procedural terrain column sweep"),
                layout: None,
                module: &shader,
                entry_point: Some(if sweep_mode == 2 {
                    "sweep_visible_columns"
                } else {
                    "sweep_columns"
                }),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &{
                        let mut constants = std::collections::HashMap::from([(
                            "SWEEP_HEIGHT".to_owned(),
                            f64::from(sweep_rows),
                        )]);
                        if sweep_mode == 1 {
                            constants.insert("FINE_MODE".to_owned(), f64::from(fine_mode));
                        }
                        constants
                    },
                    ..Default::default()
                },
                cache: None,
            });
            let mut sources = vec![
                (0, &uniform),
                (2, &bounds),
                (4, &lattice),
                (7, &beam),
                (8, &hits),
            ];
            if sweep_mode == 1 {
                sources.push((5, &fine));
            }
            let buffers = sources
                .into_iter()
                .map(|(binding, buffer)| wgpu::BindGroupEntry {
                    binding,
                    resource: buffer.as_entire_binding(),
                })
                .collect::<Vec<_>>();
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("procedural terrain column sweep I/O"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &buffers,
            });
            (pipeline, bind)
        });
        Ok(Self {
            sweep,
            sweep_draws: Cell::new(0),
            sweep_rows,
            hot_pipeline,
            warm_draws: Cell::new(0),
            hot_draws: Cell::new(0),
            beam: (beam_pipeline, beam_bind),
            beam_enabled,
            last_beam_view: Cell::new(None),
            lattice_fill: (fill, fill_bind),
            lattice_bands,
            lattice_bytes,
            tile,
            reductions,
            build_bind: bind(&compute.get_bind_group_layout(0), 1, 4),
            render_bind,
            fine,
            fine_mode,
            fine_capacity,
            compute,
            pipeline,
            uniform,
            last_grid: Cell::new(None),
            diagnostic: std::env::var_os("FRONTIER_PROCEDURAL_HIT_CAPTURE")
                .is_some_and(|v| v == "1"),
            source: source.clone(),
            started,
            startup_encode_ms: Cell::new(None),
            builds: Cell::new(0),
        })
    }

    pub(super) fn prepare(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: Camera,
        size: [u32; 2],
        visuals: Option<&VisualSettings>,
    ) {
        let center = [camera.position.x, camera.position.z];
        let grid = self
            .last_grid
            .get()
            .filter(|grid| {
                center.iter().zip(grid).all(|(&eye, &min)| {
                    let grid_center = (min + SIDE / 2) as f64 * BLOCK;
                    (eye - grid_center).abs() < BLOCK * 8.
                })
            })
            .unwrap_or_else(|| center.map(|eye| (eye / BLOCK).floor() as i32 - SIDE / 2));
        let groups = size.map(|n| n.div_ceil(4));
        let beam = self.beam_enabled && groups[0].saturating_mul(groups[1]) <= BEAM_COUNT;
        let sweep = self
            .sweep
            .as_ref()
            .filter(|_| camera.pitch == 0. && size[0].saturating_mul(size[1]) <= SWEEP_PIXELS);
        let aspect = size[0] as f32 / size[1] as f32;
        let forward = camera.direction();
        let tangent = (camera.fov_y.to_radians() * 0.5).tan();
        let right = forward.cross(Vec3::Y).normalize();
        let up = right.cross(forward);
        let (elevation, azimuth) =
            visuals.map_or((25.0_f32, 23.0_f32), |v| (v.sun_elevation, v.sun_azimuth));
        let (se, ce) = elevation.to_radians().sin_cos();
        let (sa, ca) = azimuth.to_radians().sin_cos();
        let p = self.source.params();
        let seed = self.source.seed();
        let mut relative = camera;
        relative.position = DVec3::ZERO;
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&View {
                projection: relative.matrix(DVec3::ZERO, aspect).to_cols_array(),
                eye: camera.position.as_vec3().extend(camera.far).to_array(),
                forward: forward.extend(0.).to_array(),
                right: (right * tangent * aspect).extend(0.).to_array(),
                up: (up * tangent).extend(0.).to_array(),
                sun: [sa * ce, se, ca * ce, 0.],
                tile: {
                    let [x, y, side] = self.tile.map(|v| v as f32);
                    [
                        1. / side,
                        (2. * x + 1.) / side - 1.,
                        1. - (2. * y + 1.) / side,
                        0.,
                    ]
                },
                settings: [
                    (1800. / p.horizontal_scale) as f32,
                    (p.amplitude / 1000.) as f32,
                    p.roughness as f32,
                    0.,
                ],
                grid,
                seed: [seed as u32, (seed >> 32) as u32],
                flags: [
                    p.octaves,
                    u32::from(self.diagnostic),
                    u32::from(self.lattice_bytes > 4),
                    self.fine_mode,
                ],
                output: [
                    size[0],
                    size[1],
                    u32::from(beam),
                    u32::from(sweep.is_some()),
                ],
                cache: [self.fine_capacity[0], self.fine_capacity[1], 0, 0],
                lattice: std::array::from_fn(|i| {
                    let (side, offset) = self.lattice_bands[i];
                    let scale = BLOCK * 1800. / p.horizontal_scale / WAVELENGTHS[i];
                    let [x, z] = grid.map(|v| f64::from(v) * scale);
                    let z = if i == 2 { z.min(0.) } else { z };
                    [
                        (x + 12.7).floor() as i32 - 2,
                        (z - 29.1).floor() as i32 - 2,
                        side,
                        offset,
                    ]
                }),
            }),
        );
        if self.last_grid.replace(Some(grid)) != Some(grid) {
            self.builds.set(self.builds.get() + 1);
            self.warm_draws.set(0);
            if self.fine_mode != 0 {
                encoder.clear_buffer(&self.fine, 0, None);
            }
            if self.lattice_bytes > 4 {
                let count = self
                    .lattice_bands
                    .iter()
                    .map(|&(side, _)| (side * side) as u32)
                    .max()
                    .unwrap();
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("fill procedural terrain gradient lattice"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.lattice_fill.0);
                pass.set_bind_group(0, &self.lattice_fill.1, &[]);
                pass.dispatch_workgroups(count.div_ceil(64), 9, 1);
            }
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("procedural terrain bounds"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.compute);
            pass.set_bind_group(0, &self.build_bind, &[]);
            pass.dispatch_workgroups(SIDE as u32 / 8, SIDE as u32 / 8, 1);
            drop(pass);
            for ((pipeline, bind), size) in self.reductions.iter().zip([16, 4]) {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("reduce procedural terrain bounds"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind, &[]);
                pass.dispatch_workgroups(size, size, 1);
            }
        }
        if beam && self.last_beam_view.replace(Some((camera, size))) != Some((camera, size)) {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("trace shared terrain ray prefixes"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.beam.0);
            pass.set_bind_group(0, &self.beam.1, &[]);
            pass.dispatch_workgroups(groups[0].div_ceil(8), groups[1].div_ceil(8), 1);
        }
        if let Some((pipeline, bind)) = sweep {
            self.sweep_draws.set(self.sweep_draws.get() + 1);
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sweep level camera columns"),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.dispatch_workgroups(
                size[0].div_ceil(32),
                size[1].div_ceil(self.sweep_rows * 2),
                1,
            );
        }
        if self.startup_encode_ms.get().is_none() {
            self.startup_encode_ms
                .set(Some(self.started.elapsed().as_secs_f64() * 1000.));
        }
    }

    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, [width, height]: [u32; 2]) {
        let warm = self.warm_draws.get();
        let columns = width.div_ceil(320);
        let rows = height.div_ceil(180);
        let tiles = columns * rows;
        if let Some(hot) = self.hot_pipeline.as_ref() {
            if warm >= (tiles * 2).max(12) {
                pass.set_pipeline(hot);
                self.hot_draws.set(self.hot_draws.get() + 1);
            } else {
                let tile = warm % tiles;
                let x = (tile % columns) * 320;
                let y = (tile / columns) * 180;
                pass.set_scissor_rect(x, y, (width - x).min(320), (height - y).min(180));
                pass.set_pipeline(&self.pipeline);
                self.warm_draws.set(warm + 1);
            }
        } else {
            pass.set_pipeline(&self.pipeline);
        }
        pass.set_bind_group(0, &self.render_bind, &[]);
        pass.draw(0..3, 0..1);
    }

    pub(super) fn capture_cache_counters(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> crate::Result<Option<[u32; 4]>> {
        if self.fine_mode == 0 {
            return Ok(None);
        }
        let counter_bytes = 16;
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("procedural terrain cache counters"),
            size: counter_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.fine, FINE_COUNTER_OFFSET, &read, 0, counter_bytes);
        queue.submit(Some(encoder.finish()));
        let slice = read.slice(..);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        device.poll(wgpu::Maintain::Wait);
        receiver.recv()??;
        let data = slice.get_mapped_range();
        let words =
            std::array::from_fn(|i| u32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap()));
        Ok(Some(words))
    }

    pub(super) fn json(&self) -> String {
        format!(
            "{{\"gpu_bound_bytes\":{BYTES},\"gpu_lattice_bytes\":{},\"gpu_fine_cache_bytes\":{},\"beams_requested\":{},\"hot_probe_draws\":{},\"sweep_draws\":{},\"sweep_rows\":{},\"bounds_builds\":{},\"init_to_first_encode_ms\":{},\"diagnostic\":{},\"tile\":{:?}}}",
            self.lattice_bytes,
            self.fine.size(),
            self.beam_enabled,
            self.hot_draws.get(),
            self.sweep_draws.get(),
            self.sweep_rows,
            self.builds.get(),
            self.startup_encode_ms.get().map_or("null".into(), |v| v.to_string()),
            self.diagnostic,
            self.tile
        )
    }
}

// An offscreen diagnostic may submit a full-size image in independent tiles.
// Tiles have the same pixel ray as a single full-frame render at that aspect.
fn capture_tile() -> crate::Result<[u32; 3]> {
    let Some(value) = std::env::var_os("FRONTIER_PROCEDURAL_TILE") else {
        return Ok([0, 0, 1]);
    };
    let parsed = value
        .to_str()
        .ok_or("non-UTF-8 procedural tile")?
        .split(',')
        .map(str::parse)
        .collect::<Result<Vec<u32>, _>>()?;
    let [column, row, side]: [u32; 3] = parsed
        .try_into()
        .map_err(|_| "procedural tile requires column,row,tiles-per-side")?;
    if !(1..=16).contains(&side) || column >= side || row >= side {
        return Err("procedural tile index outside capture grid".into());
    }
    Ok([column, row, side])
}
