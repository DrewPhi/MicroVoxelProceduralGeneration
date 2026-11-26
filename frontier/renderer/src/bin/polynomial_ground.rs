//! Meshless native-column visibility for a terrain source with bounded patches.
mod polynomial_ground {
    pub mod grid;
    pub mod window;
}
use bytemuck::{Pod, Zeroable};
use frontier_world::{FbmParams, FbmRecipe, FbmTerrain};
use glam::{DQuat, DVec3};
use polynomial_ground::{
    grid::Lattice,
    window::{Display, Preview},
};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    time::Instant,
};
use wgpu::util::DeviceExt;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const TRACE: &str = include_str!("polynomial_ground/trace.wgsl");
const PITCH: f64 = 0.0625;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct View {
    eye: [f32; 4],
    forward: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    size: [u32; 4],
    origin: [i32; 4],
}

struct Options {
    width: u32,
    height: u32,
    interactive: bool,
    present_benchmark: bool,
    far: f64,
    pitch: u32,
    eye: DVec3,
    aim: DVec3,
    seed: u64,
    frames: u32,
    speed: f64,
    turn: f64,
    level_step: u32,
    sibling: bool,
    beams: bool,
    early_relief: bool,
    group_width: u32,
    roll_every: f64,
    relief: bool,
    solve: bool,
    output: PathBuf,
}

impl Options {
    fn read() -> Result<Self> {
        let mut result = Self {
            interactive: false,
            present_benchmark: false,
            width: 1280,
            height: 720,
            far: 6000.,
            pitch: 8,
            eye: DVec3::new(-6688., 1450., 6080.),
            aim: DVec3::new(-5350., 900., 8360.),
            seed: 1685576058315138427,
            frames: 90,
            speed: 80.,
            turn: 26.,
            level_step: 2,
            sibling: true,
            beams: true,
            early_relief: true,
            group_width: 8,
            roll_every: 0.,
            relief: true,
            solve: true,
            output: "output/polynomial-ground".into(),
        };
        let mut arguments = std::env::args().skip(1);
        while let Some(key) = arguments.next() {
            match key.as_str() {
                "--interactive" => result.interactive = true,
                "--present-benchmark" => result.present_benchmark = true,
                "--no-relief" => result.relief = false,
                "--walk-leaves" => result.solve = false,
                "--ascend-parent" => result.sibling = false,
                "--beams" => result.beams = true,
                "--no-beams" => result.beams = false,
                "--full-relief-eval" => result.early_relief = false,
                _ => {
                    let value = arguments.next().ok_or("missing option value")?;
                    match key.as_str() {
                        "--width" => result.width = value.parse()?,
                        "--height" => result.height = value.parse()?,
                        "--far" => result.far = value.parse()?,
                        "--control-spacing" => result.pitch = value.parse()?,
                        "--eye" => result.eye = position(&value)?,
                        "--target" => result.aim = position(&value)?,
                        "--seed" => result.seed = value.parse()?,
                        "--frames" => result.frames = value.parse()?,
                        "--speed" => result.speed = value.parse()?,
                        "--turn" => result.turn = value.parse()?,
                        "--level-step" => result.level_step = value.parse()?,
                        "--group-width" => result.group_width = value.parse()?,
                        "--roll-every" => result.roll_every = value.parse()?,
                        "--output" => result.output = value.into(),
                        _ => return Err(format!("unknown option {key}").into()),
                    }
                }
            }
        }
        if !(32..=1920).contains(&result.width)
            || !(32..=1080).contains(&result.height)
            || !(1..=7200).contains(&result.frames)
            || !(1.0..=7000.).contains(&result.far)
            || !(0.0..=140.).contains(&result.speed)
            || !(-360.0..=360.).contains(&result.turn)
            || ![4, 8].contains(&result.pitch)
            || !(1..=3).contains(&result.level_step)
            || ![4, 8, 16, 32].contains(&result.group_width)
            || !(0.0..=1000.).contains(&result.roll_every)
            || result.eye.abs().max_element() > 500_000.
            || !result.eye.is_finite()
            || !result.aim.is_finite()
            || (result.aim - result.eye).with_y(0.).length_squared() < 1.0e-8
        {
            return Err("benchmark options outside supported range".into());
        }
        Ok(result)
    }
    fn pose(&self, index: u32) -> (DVec3, DVec3, DVec3, DVec3) {
        let forward = (self.aim - self.eye).normalize();
        let progress = f64::from(index) / f64::from((self.frames - 1).max(1));
        let yaw = progress * self.turn.to_radians();
        let dir = DQuat::from_rotation_y(yaw) * forward;
        let straight = forward.with_y(0.).normalize();
        let distance = self.speed * f64::from(index) / 30.;
        let bank = (self.turn * 0.50).to_radians() * (progress * std::f64::consts::PI).sin();
        let right = DQuat::from_axis_angle(dir, bank) * dir.cross(DVec3::Y).normalize();
        (self.eye + straight * distance, dir, right, right.cross(dir))
    }
    fn view(
        &self,
        grid: &Lattice,
        (eye, forward, right, up): (DVec3, DVec3, DVec3, DVec3),
    ) -> Result<(View, DVec3)> {
        if !grid.contains(eye.to_array(), self.far) {
            return Err("flight left the prepared control lattice".into());
        }
        let [x, z] = grid.origin.map(|n| f64::from(n) * PITCH);
        let local = eye - DVec3::new(x, 0., z);
        let fov = 1. / 3f64.sqrt();
        let pixel_aspect = f64::from(self.width) / f64::from(self.height);
        Ok((
            View {
                eye: local.as_vec3().extend(self.far as f32).to_array(),
                forward: forward.as_vec3().extend(0.).to_array(),
                right: (right * fov * pixel_aspect).as_vec3().extend(0.).to_array(),
                up: (up * fov).as_vec3().extend(0.).to_array(),
                size: [self.width, self.height, grid.side as u32, grid.side.ilog2()],
                origin: [
                    grid.origin[0],
                    grid.origin[1],
                    (self.seed ^ (self.seed >> 32)) as i32,
                    0,
                ],
            },
            eye,
        ))
    }
}

struct Resident {
    grid: Arc<Lattice>,
    vertices: wgpu::Buffer,
    roofs: wgpu::Buffer,
}
impl Resident {
    fn prepare(
        device: &wgpu::Device,
        terrain: &FbmTerrain,
        origin: [i32; 2],
        side: usize,
        cell_width: i32,
        relief: bool,
        previous: Option<&Lattice>,
    ) -> Self {
        let grid = Arc::new(Lattice::generate(
            terrain, origin, side, cell_width, relief, previous,
        ));
        let upload = |label, contents| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
        let vertices = upload("canonical lattice", bytemuck::cast_slice(&grid.vertices));
        let roofs = upload("traversal bounds", bytemuck::cast_slice(&grid.roofs));
        Self {
            grid,
            vertices,
            roofs,
        }
    }

    fn origin_at(eye: DVec3, side: usize, cell_width: i32) -> [i32; 2] {
        [eye.x, eye.z].map(|p| {
            ((p / (f64::from(cell_width) * PITCH)).floor() as i32 - side as i32 / 2) * cell_width
        })
    }
}

fn position(text: &str) -> Result<DVec3> {
    let values = text
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<Vec<f64>, _>>()?;
    if values.len() != 3 {
        return Err("expected X,Y,Z".into());
    }
    Ok(DVec3::from_slice(&values))
}

fn main() -> Result<()> {
    let options = Options::read()?;
    if options.interactive || options.present_benchmark {
        polynomial_ground::window::play(options)
    } else {
        pollster::block_on(run(options, None))
    }
}

async fn run(o: Options, mut preview: Option<Preview>) -> Result<()> {
    let wall_start = Instant::now();
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..Default::default()
    });
    let surface = preview
        .as_ref()
        .map(|p| instance.create_surface(p.window.clone()))
        .transpose()?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: surface.as_ref(),
            ..Default::default()
        })
        .await
        .ok_or("no Vulkan adapter")?;
    let info = adapter.get_info();
    eprintln!("{info:?}");
    if info.device_type == wgpu::DeviceType::Cpu {
        return Err("requires a hardware GPU".into());
    }
    let side = (((2. * o.far + 128.) / f64::from(o.pitch)).ceil() as usize)
        .max(32)
        .next_power_of_two();
    let vertex_bytes = (8 * (side + 1).pow(2)) as u32;
    let limits = adapter.limits();
    if vertex_bytes > limits.max_storage_buffer_binding_size
        || u64::from(vertex_bytes) > limits.max_buffer_size
    {
        return Err("control lattice exceeds adapter's binding size".into());
    }
    let (device, queue) = adapter
        .request_device(
            &wgpu::DeviceDescriptor {
                required_features: wgpu::Features::TIMESTAMP_QUERY,
                required_limits: wgpu::Limits {
                    max_storage_buffer_binding_size: vertex_bytes
                        .max(wgpu::Limits::default().max_storage_buffer_binding_size),
                    max_buffer_size: u64::from(vertex_bytes)
                        .max(wgpu::Limits::default().max_buffer_size),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await?;
    let terrain = FbmTerrain::new(
        o.seed,
        FbmParams {
            recipe: FbmRecipe::Eroded,
            ..Default::default()
        },
    )?;
    let cell_width = o.pitch as i32 * 16;
    let origin = Resident::origin_at(o.eye, side, cell_width);
    let start = Instant::now();
    let mut resident =
        Resident::prepare(&device, &terrain, origin, side, cell_width, o.relief, None);
    let generation_ms = start.elapsed().as_secs_f64() * 1000.;
    eprintln!(
        "control lattice {}², generated in {:.2} ms",
        side + 1,
        generation_ms
    );
    let (view, _) = o.view(&resident.grid, o.pose(0))?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("polynomial ground visibility"),
        source: wgpu::ShaderSource::Wgsl(TRACE.into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &std::array::from_fn::<_, 6, _>(|i| wgpu::BindGroupLayoutEntry {
            binding: i as u32,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: if i == 0 {
                    wgpu::BufferBindingType::Uniform
                } else {
                    wgpu::BufferBindingType::Storage { read_only: i < 3 }
                },
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
        label: None,
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("visibility"),
        compilation_options: wgpu::PipelineCompilationOptions {
            constants: &std::collections::HashMap::from([
                ("RELIEF".into(), f64::from(o.relief)),
                ("LEVEL_STEP".into(), f64::from(o.level_step)),
                ("SOLVE".into(), f64::from(o.solve)),
                ("SIBLING".into(), f64::from(o.sibling)),
                ("BEAMS".into(), f64::from(o.beams)),
                ("EARLY_RELIEF".into(), f64::from(o.early_relief)),
                ("GROUP_WIDTH".into(), f64::from(o.group_width)),
                ("GROUP_HEIGHT".into(), f64::from(64 / o.group_width)),
                ("WIDTH".into(), f64::from(cell_width)),
                (
                    "INTERPOLANT_BITS".into(),
                    f64::from((cell_width as u32 * 2).ilog2()),
                ),
            ]),
            ..Default::default()
        },
        cache: None,
    });
    let prefix_pipeline = o.beams.then(|| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("conservative beam prefixes"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("find_prefix"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &std::collections::HashMap::from([(
                    "WIDTH".into(),
                    f64::from(cell_width),
                )]),
                ..Default::default()
            },
            cache: None,
        })
    });
    use wgpu::BufferUsages as U;
    let storage = |label, contents, usage| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents,
            usage,
        })
    };
    let camera = storage(
        "camera",
        bytemuck::bytes_of(&view),
        U::UNIFORM | U::COPY_DST,
    );
    let buffer = |label, size, usage| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage,
            mapped_at_creation: false,
        })
    };
    let pixel_bytes = 8 * u64::from(o.width) * u64::from(o.height);
    let pixels = buffer(
        "display pixels and hit distances",
        pixel_bytes,
        U::STORAGE | U::COPY_SRC,
    );
    let failed = buffer(
        "failed visibility queries",
        4,
        U::STORAGE | U::COPY_SRC | U::COPY_DST,
    );
    let prefix = buffer(
        "beam prefixes",
        4 * u64::from(o.width.div_ceil(8) * o.height.div_ceil(8)),
        U::STORAGE,
    );
    let timing = buffer("timestamps", 16, U::QUERY_RESOLVE | U::COPY_SRC);
    let readback = buffer("readback", 24 + pixel_bytes, U::MAP_READ | U::COPY_DST);
    let timestamps = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: None,
        ty: wgpu::QueryType::Timestamp,
        count: 2,
    });
    let bind = |r: &Resident| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[&camera, &r.vertices, &r.roofs, &pixels, &failed, &prefix]
                .into_iter()
                .enumerate()
                .map(|(i, b)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: b.as_entire_binding(),
                })
                .collect::<Vec<_>>(),
        })
    };
    let mut bindings = bind(&resident);
    let mut display = surface
        .map(|s| {
            Display::new(
                s,
                &adapter,
                &device,
                preview.as_ref().unwrap(),
                &camera,
                &pixels,
            )
        })
        .transpose()?;
    let mut title_clock = Instant::now();
    let mut report = if o.interactive {
        None
    } else {
        fs::create_dir_all(&o.output)?;
        let mut file = fs::File::create(o.output.join("frames.csv"))?;
        writeln!(file, "frame,gpu_ms,wall_ms,failed,coverage_updates")?;
        Some(file)
    };
    let mut times = Vec::new();
    let mut failed_total = 0u64;
    let mut audit = [0.; 2];
    let mut far_hit = 0f32;
    let mut first_ms = 0.;
    let mut presented_frames = 0;
    let available = side as f64 * f64::from(o.pitch) / 2. - o.far - 8.;
    let automatic = (available * 0.55).clamp(8., 1024.);
    let roll_span = if o.roll_every > 0. {
        o.roll_every.min(automatic)
    } else {
        automatic
    };
    let mut pending: Option<mpsc::Receiver<(Resident, f64)>> = None;
    let mut updates = 0;
    let mut update_ms = Vec::new();
    let mut new_vertices = 0;
    let frames = if o.interactive { u32::MAX } else { o.frames };
    for frame in 0..frames {
        let start = Instant::now();
        if preview.as_ref().is_some_and(Preview::closing) {
            return Ok(());
        }
        let pose = if o.interactive {
            preview.as_mut().unwrap().pose(o.speed)
        } else {
            o.pose(frame)
        };
        let eye = pose.0;
        if let Some(rx) = &pending {
            match rx.try_recv() {
                Ok((new, elapsed)) => {
                    pending = None;
                    if new.grid.contains(eye.to_array(), o.far) {
                        updates += 1;
                        new_vertices += new.grid.generated;
                        update_ms.push(elapsed);
                        eprintln!(
                            "coverage publication {frame}, {:.2} ms background, {} new vertices",
                            elapsed, new.grid.generated
                        );
                        bindings = bind(&new);
                        resident = new;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => (),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("lattice worker stopped".into())
                }
            }
        }
        let desired = Resident::origin_at(eye, side, cell_width);
        let shift = desired
            .into_iter()
            .zip(resident.grid.origin)
            .map(|(a, b)| f64::from((a - b).abs()) * PITCH)
            .fold(0., f64::max);
        if pending.is_none() && shift >= roll_span && frame + 1 < frames {
            let (tx, rx) = mpsc::sync_channel(1);
            let device = device.clone();
            let terrain = terrain.clone();
            let previous = resident.grid.clone();
            let relief = o.relief;
            std::thread::spawn(move || {
                let start = Instant::now();
                let data = Resident::prepare(
                    &device,
                    &terrain,
                    desired,
                    side,
                    cell_width,
                    relief,
                    Some(&previous),
                );
                let _ = tx.send((data, start.elapsed().as_secs_f64() * 1000.));
            });
            pending = Some(rx);
        }
        if !resident.grid.contains(eye.to_array(), o.far) && o.interactive {
            std::thread::sleep(std::time::Duration::from_millis(8));
            continue;
        }
        let (view, eye) = o.view(&resident.grid, pose)?;
        let capture = if o.interactive {
            preview.as_ref().unwrap().capture()
        } else {
            frame == 0 || frame + 1 == o.frames
        };
        queue.write_buffer(&camera, 0, bytemuck::bytes_of(&view));
        let mut e = device.create_command_encoder(&Default::default());
        e.clear_buffer(&failed, 0, None);
        if let Some(pipeline) = &prefix_pipeline {
            let mut pass = e.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                    query_set: &timestamps,
                    beginning_of_pass_write_index: Some(0),
                    end_of_pass_write_index: None,
                }),
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups(o.width.div_ceil(64), o.height.div_ceil(64), 1);
        }
        {
            let mut pass = e.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                    query_set: &timestamps,
                    beginning_of_pass_write_index: (!o.beams).then_some(0),
                    end_of_pass_write_index: Some(1),
                }),
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups(
                o.width.div_ceil(o.group_width),
                o.height.div_ceil(64 / o.group_width),
                1,
            );
        }
        e.resolve_query_set(&timestamps, 0..2, &timing, 0);
        e.copy_buffer_to_buffer(&timing, 0, &readback, 0, 16);
        e.copy_buffer_to_buffer(&failed, 0, &readback, 16, 4);
        if capture {
            e.copy_buffer_to_buffer(&pixels, 0, &readback, 24, pixel_bytes);
        }
        let present = if let (Some(display), Some(preview)) = (&mut display, &preview) {
            display.draw(&device, &preview.window, &mut e)?
        } else {
            None
        };
        queue.submit([e.finish()]);
        if let Some(frame) = present {
            frame.present();
            presented_frames += 1;
        }
        let slice = readback.slice(..if capture { 24 + pixel_bytes } else { 24 });
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let bytes = slice.get_mapped_range();
        let stamp: &[u64] = bytemuck::cast_slice(&bytes[..16]);
        let gpu = (stamp[1] - stamp[0]) as f64 * f64::from(queue.get_timestamp_period()) * 1.0e-6;
        let failed = u32::from_ne_bytes(bytes[16..20].try_into()?);
        let wall = start.elapsed().as_secs_f64() * 1000.;
        if let Some(report) = &mut report {
            writeln!(report, "{frame},{gpu},{wall},{failed},{updates}")?;
        }
        if let Some(preview) = &preview {
            if title_clock.elapsed().as_millis() >= 300 {
                preview.window.set_title(&format!(
                    "Frontier native ground · {:.0} FPS · {:.0} m · WASD and arrows",
                    1000. / wall,
                    o.far
                ));
                title_clock = Instant::now();
            }
        }
        if frame == 0 {
            first_ms = wall_start.elapsed().as_secs_f64() * 1000.;
        }
        if frame >= 8 && !o.interactive {
            times.push([gpu, wall]);
        }
        failed_total += u64::from(failed);
        if capture {
            let words: &[[u32; 2]] = bytemuck::cast_slice(&bytes[24..]);
            if o.interactive {
                fs::create_dir_all(&o.output)?;
                image(&o.output.join("preview.png"), o.width, o.height, words)?;
                drop(bytes);
                readback.unmap();
                continue;
            }
            let prefix = if frame == 0 { "first" } else { "last" };
            image(
                &o.output.join(format!("{prefix}.png")),
                o.width,
                o.height,
                words,
            )?;
            let depths: Vec<_> = words.iter().map(|p| p[0]).collect();
            fs::write(
                o.output.join(format!("{prefix}.depth")),
                bytemuck::cast_slice(&depths),
            )?;
            far_hit = far_hit.max(
                words
                    .iter()
                    .map(|p| f32::from_bits(p[0]))
                    .fold(0., f32::max),
            );
            let [bad, error] = audit_image(&o, &resident.grid, &view, eye, words);
            audit[0] += bad;
            audit[1] = audit[1].max(error);
        }
        drop(bytes);
        readback.unmap();
    }
    let metrics = |axis: usize| {
        let mut samples: Vec<_> = times.iter().map(|t| t[axis]).collect();
        samples.sort_by(f64::total_cmp);
        if samples.is_empty() {
            return [0.; 3];
        }
        [0.5, 0.95, 0.99].map(|p| samples[(p * samples.len() as f64).ceil() as usize - 1])
    };
    let summary = format!(
        r#"{{"adapter":{:?},"seed":{},"width":{},"height":{},"horizon_m":{},"control_spacing_m":{},"farthest_hit_m":{},"speed":{},"turn_deg":{},"frames":{},"measured_frames":{},"gpu_ms_p50_p95_p99":{:?},"wall_ms_p50_p95_p99":{:?},"total_failed":{},"audit_coverage_errors":{},"audit_max_depth_m":{},"generation_ms":{},"first_ms":{},"resident_bytes":{},"coverage_updates":{},"new_control_vertices":{},"worst_background_update_ms":{},"presented_frames":{}}}"#,
        info.name,
        o.seed,
        o.width,
        o.height,
        o.far,
        o.pitch,
        far_hit,
        o.speed,
        o.turn,
        o.frames,
        times.len(),
        metrics(0),
        metrics(1),
        failed_total,
        audit[0],
        audit[1],
        generation_ms,
        first_ms,
        8 * resident.grid.vertices.len() + 4 * resident.grid.roofs.len(),
        updates,
        new_vertices,
        update_ms.into_iter().fold(0., f64::max),
        presented_frames,
    );
    println!("{summary}");
    fs::write(o.output.join("report.json"), summary)?;
    Ok(())
}

fn image(path: &Path, width: u32, height: u32, pixels: &[[u32; 2]]) -> Result<()> {
    let rgb: Vec<_> = pixels
        .iter()
        .flat_map(|pixel| pixel[1].to_le_bytes()[..3].to_vec())
        .collect();
    let mut encoder = png::Encoder::new(fs::File::create(path)?, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&rgb)?;
    Ok(())
}

fn audit_image(
    o: &Options,
    grid: &Lattice,
    view: &View,
    eye: DVec3,
    pixels: &[[u32; 2]],
) -> [f64; 2] {
    let vector = |v: [f32; 4]| DVec3::new(v[0].into(), v[1].into(), v[2].into());
    let mut bad = 0.;
    let mut error = 0f64;
    for z in 0..8 {
        for x in 0..8 {
            let x = (2 * x + 1) * o.width / 16;
            let z = (2 * z + 1) * o.height / 16;
            let sx = (f64::from(x) + 0.5) * 2. / f64::from(o.width) - 1.;
            let sy = (f64::from(z) + 0.5) * 2. / f64::from(o.height) - 1.;
            let ray =
                (vector(view.forward) + sx * vector(view.right) - sy * vector(view.up)).normalize();
            let got = f32::from_bits(pixels[(z * o.width + x) as usize][0]) as f64;
            let want = reference(o, grid, eye, ray);
            if (got >= 0.) != (want >= 0.) {
                bad += 1.;
            }
            if got >= 0. && want >= 0. {
                error = error.max((got - want).abs());
            }
        }
    }
    [bad, error]
}

fn reference(o: &Options, grid: &Lattice, eye: DVec3, ray: DVec3) -> f64 {
    let mut t = 0.025;
    let start = eye + t * ray;
    let mut cell = [start.x, start.z].map(|q| (q / PITCH).floor() as i32);
    let [ox, oz] = [eye.x, eye.z];
    let [dx, dz] = [ray.x, ray.z];
    let dir = [dx, dz].map(|v| if v > 0. { 1 } else { -1 });
    let seed = (o.seed ^ (o.seed >> 32)) as u32;
    while t < o.far {
        let sides = [(ox, dx), (oz, dz)];
        let times: [f64; 2] = std::array::from_fn(|i| {
            let (a, d) = sides[i];
            if d == 0. {
                f64::INFINITY
            } else {
                (f64::from(cell[i] + i32::from(dir[i] > 0)) * PITCH - a) / d
            }
        });
        let end = times[0].min(times[1]).min(o.far).max(t);
        if end > t {
            let h = f64::from(grid.height(cell, seed, o.relief)) * PITCH;
            if eye.y + t * ray.y <= h {
                return t;
            }
            if ray.y < 0. && eye.y + end * ray.y <= h {
                return ((h - eye.y) / ray.y).clamp(t, end);
            }
        }
        if end >= o.far {
            break;
        }
        for i in 0..2 {
            if times[i] <= end {
                cell[i] += dir[i];
            }
        }
        t = end;
    }
    -1.
}

#[cfg(test)]
mod tests {
    #[test]
    fn compute_and_presentation_shaders_validate() {
        for source in [super::TRACE, include_str!("polynomial_ground/display.wgsl")] {
            let module = naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
        }
    }
}
