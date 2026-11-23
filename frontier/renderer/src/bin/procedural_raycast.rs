//! Standalone meshless Alpine experiment. Produces PNGs, raw ray distances,
//! optional per-pixel work counters and reproducible GPU timing reports.
use bytemuck::{Pod, Zeroable};
use frontier_world::{raycast, FbmParams, FbmRecipe, FbmTerrain, World};
use glam::{DVec3, Vec3};
use std::{error::Error, fs, io::BufWriter, path::PathBuf, time::Instant};
use wgpu::util::DeviceExt;

const SIDE: u32 = 256;
const BLOCK: f64 = 80.;
const BOUNDS_BYTES: u64 = (256 * 256 + 64 * 64 + 16 * 16) * 4;
const SHADER: &str = concat!(
    include_str!("alpine_source.wgsl"),
    "\n",
    include_str!("procedural_raycast.wgsl")
);
type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct View {
    eye: [f32; 4],
    forward: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    grid: [i32; 4],
    params: [u32; 4],
    image: [u32; 4],
    controls: [f32; 4],
    lattice: [[i32; 4]; 9],
}

struct Options {
    eye: DVec3,
    target: DVec3,
    width: u32,
    height: u32,
    far: f64,
    fov: f64,
    seed: u64,
    mode: u32,
    shading: u32,
    tolerance: f32,
    haze: f32,
    frames: u32,
    warmup: u32,
    tile: u32,
    oracle: u32,
    turn: f64,
    approach: f64,
    instrument: bool,
    capture_motion: bool,
    gradient_cache: bool,
    repair: bool,
    numerical: bool,
    output: PathBuf,
}
fn vector(s: &str) -> Result<DVec3> {
    let values = s
        .split(',')
        .map(str::parse::<f64>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    match values.as_slice() {
        &[x, y, z] => Ok(DVec3::new(x, y, z)),
        _ => Err("expected x,y,z".into()),
    }
}
fn options() -> Result<Option<Options>> {
    let mut o = Options {
        eye: DVec3::new(-43.625, 75., -289.5625),
        target: DVec3::new(0., 490., 3300.),
        width: 640,
        height: 360,
        far: 6000.,
        fov: 60.,
        seed: 7,
        mode: 3,
        shading: 1,
        tolerance: 0.35,
        haze: 0.,
        frames: 6,
        warmup: 2,
        tile: 0,
        oracle: 0,
        turn: 0.,
        approach: 0.,
        instrument: false,
        capture_motion: false,
        gradient_cache: false,
        repair: true,
        numerical: false,
        output: "output/continuous-rays".into(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(name) = args.next() {
        if name == "--help" {
            println!("procedural_raycast --output DIR --width 640 --height 360 --eye X,Y,Z --target X,Y,Z --far 6000 --fov-y 60 --seed 7 --mode eager|lazy|band-range|clip --shading geometry|macro|filtered --tolerance-px 0.35 --haze 0 --frames 6 --warmup 2 --tile 0 --turn-deg 0 --approach-m 0 --capture-motion --instrument --gradient-cache --no-repair --numerical-fallback --oracle 0");
            return Ok(None);
        }
        match name.as_str() {
            "--capture-motion" => {
                o.capture_motion = true;
                continue;
            }
            "--instrument" => {
                o.instrument = true;
                continue;
            }
            "--gradient-cache" => {
                o.gradient_cache = true;
                continue;
            }
            "--no-repair" => {
                o.repair = false;
                continue;
            }
            "--numerical-fallback" => {
                o.numerical = true;
                continue;
            }
            _ => {}
        }
        let value = args.next().ok_or_else(|| format!("{name} needs a value"))?;
        match name.as_str() {
            "--eye" => o.eye = vector(&value)?,
            "--target" => o.target = vector(&value)?,
            "--width" => o.width = value.parse()?,
            "--height" => o.height = value.parse()?,
            "--far" => o.far = value.parse()?,
            "--fov-y" => o.fov = value.parse()?,
            "--seed" => o.seed = value.parse()?,
            "--frames" => o.frames = value.parse()?,
            "--warmup" => o.warmup = value.parse()?,
            "--tolerance-px" => o.tolerance = value.parse()?,
            "--haze" => o.haze = value.parse()?,
            "--tile" => o.tile = value.parse()?,
            "--oracle" => o.oracle = value.parse()?,
            "--turn-deg" => o.turn = value.parse()?,
            "--approach-m" => o.approach = value.parse()?,
            "--output" => o.output = value.into(),
            "--shading" => {
                o.shading = match value.as_str() {
                    "geometry" => 2,
                    "macro" => 0,
                    "filtered" => 1,
                    _ => return Err("invalid shading".into()),
                }
            }
            "--mode" => {
                o.mode = match value.as_str() {
                    "eager" => 0,
                    "lazy" => 1,
                    "band-range" => 2,
                    "clip" => 3,
                    _ => return Err("invalid mode".into()),
                }
            }
            _ => return Err(format!("unknown option {name}").into()),
        }
    }
    let d = (o.target - o.eye).normalize();
    if o.width == 0
        || o.width > 1920
        || o.height == 0
        || o.height > 1080
        || o.frames == 0
        || o.frames > 240
        || o.warmup > 30
        || o.oracle > 512
        || o.tile != 0 && (!(32..=1920).contains(&o.tile) || o.tile % 8 != 0)
        || !d.is_finite()
        || d.y.abs() > 0.9999
        || !o.eye.is_finite()
        || o.eye.abs().max_element() > 8192.
        || !(1. ..=6000.).contains(&o.far)
        || !(20. ..=75.).contains(&o.fov)
        || !(0.05..=2.).contains(&o.tolerance)
        || !(0. ..=0.01).contains(&o.haze)
        || !(-120. ..=120.).contains(&o.turn)
        || !(0. ..=1200.).contains(&o.approach)
    {
        return Err("unsupported dimensions, clip, camera or experiment setting".into());
    }
    Ok(Some(o))
}
impl Options {
    fn atlas(&self) -> [[i32; 4]; 9] {
        let center = self.eye + (self.target - self.eye).normalize() * self.far * 0.36;
        let mut offset = 0;
        [750., 240., 1800., 850., 180., 480., 45., 22.5, 11.25].map(|wave| {
            let side = (SIDE as f64 * BLOCK / wave).ceil().min(512.) as i32 + 2;
            let entry = [
                (center.x / wave + 12.7).floor() as i32 - side / 2,
                (center.z / wave - 29.1).floor() as i32 - side / 2,
                side,
                offset,
            ];
            offset += side * side;
            entry
        })
    }

    fn view(&self, alpha: f64) -> View {
        let d = (self.target - self.eye).normalize();
        let eye = self.eye + d * (self.approach * alpha);
        let forward =
            (glam::DQuat::from_rotation_y(self.turn.to_radians() * alpha.sin()) * d).as_vec3();
        let right = forward.cross(Vec3::Y).normalize();
        let up = right.cross(forward).normalize();
        let y = (self.fov.to_radians() * 0.5).tan() as f32;
        let pixel = 2. * y / self.height as f32;
        View {
            eye: eye.as_vec3().extend(self.far as f32).to_array(),
            forward: forward.extend(0.).to_array(),
            right: (right * (y * self.width as f32 / self.height as f32))
                .extend(0.)
                .to_array(),
            up: (up * y).extend(0.).to_array(),
            grid: [
                (self.eye.x / BLOCK).floor() as i32 - SIDE as i32 / 2,
                (self.eye.z / BLOCK).floor() as i32 - SIDE as i32 / 2,
                0,
                0,
            ],
            params: [
                self.seed as u32,
                (self.seed >> 32) as u32,
                self.mode,
                self.shading,
            ],
            image: [self.width, self.height, 0, 0],
            controls: [pixel * self.tolerance, pixel, self.haze, self.width as f32],
            lattice: self.atlas(),
        }
    }
}
fn read(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Result<Vec<u8>> {
    let (send, recv) = std::sync::mpsc::sync_channel(1);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |status| {
            let _ = send.send(status);
        });
    device.poll(wgpu::Maintain::Wait);
    recv.recv()??;
    let bytes = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    Ok(bytes)
}
fn read_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}
fn stats(mut values: Vec<f64>) -> [f64; 3] {
    values.sort_by(f64::total_cmp);
    if values.is_empty() {
        return [0.; 3];
    }
    [0.50, 0.95, 0.99].map(|p| values[((values.len() - 1) as f64 * p).round() as usize])
}
fn png(path: PathBuf, w: u32, h: u32, data: &[[f32; 4]]) -> Result<()> {
    let rgb: Vec<u8> = data
        .iter()
        .flat_map(|p| {
            p[..3].iter().map(|&x| {
                let y = if x <= 0.0031308 {
                    12.92 * x
                } else {
                    1.055 * x.powf(1. / 2.4) - 0.055
                };
                (y.clamp(0., 1.) * 255.).round() as u8
            })
        })
        .collect();
    let mut enc = png::Encoder::new(BufWriter::new(fs::File::create(path)?), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgb)?;
    Ok(())
}
fn oracle(o: &Options, view: &View, data: &[[f32; 4]]) -> Result<String> {
    if o.oracle == 0 {
        return Ok("null".into());
    }
    let p = FbmParams {
        recipe: FbmRecipe::Alpine,
        ..Default::default()
    };
    let t = FbmTerrain::new(o.seed, p)?;
    let world = World::fbm(o.seed, p)?;
    let side = (f64::from(o.oracle).sqrt().floor() as u32).max(1);
    let xyz = |v: [f32; 4]| DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64);
    let mut errors = Vec::new();
    let mut native = Vec::new();
    let (mut mismatches, mut native_mask, mut unresolved) = (0, 0, 0);
    for row in 0..side {
        for col in 0..side {
            let x = ((2 * col + 1) * o.width / (2 * side)).min(o.width - 1);
            let y = ((2 * row + 1) * o.height / (2 * side)).min(o.height - 1);
            let ix = (y * o.width + x) as usize;
            let u = 2. * (x as f64 + 0.5) / o.width as f64 - 1.;
            let v = 2. * (y as f64 + 0.5) / o.height as f64 - 1.;
            let d = (xyz(view.forward) + u * xyz(view.right) - v * xyz(view.up)).normalize();
            let eye = xyz(view.eye);
            let forward = d.dot(xyz(view.forward));
            let near = 0.025 / forward;
            let far = o.far / forward;
            let gap = |s: f64| {
                let point = eye + d * s;
                point.y - t.continuous_height_m(point.x, point.z)
            };
            // Independently sample the full f64 source at 10 cm spacing, then
            // bisect the first sampled bracket. This is a numeric oracle,
            // with a separate exact DDA comparison for stepped geometry.
            let mut l = near;
            let mut reference = None;
            if gap(l) <= 0. {
                reference = Some(l);
            }
            while reference.is_none() && l < far {
                let r = (l + 0.1).min(far);
                if gap(r) <= 0. {
                    let mut r = r;
                    for _ in 0..24 {
                        let m = (r + l) * 0.5;
                        if gap(m) > 0. {
                            l = m;
                        } else {
                            r = m;
                        }
                    }
                    reference = Some(r);
                    break;
                }
                l = r;
            }
            let exact = raycast(
                &world,
                (eye + d * near).to_array(),
                d.to_array(),
                far - near,
            )
            .map(|hit| hit.distance + near);
            let got = f64::from(data[ix][3]);
            if got == -2. {
                unresolved += 1;
                continue;
            }
            if let Some(reference) = reference.filter(|_| got >= 0.) {
                errors.push((got - reference).abs());
            }
            mismatches += u32::from((got >= 0.) != reference.is_some());
            if let Some(exact) = exact.filter(|_| got >= 0.) {
                native.push((got - exact).abs());
            }
            native_mask += u32::from((got >= 0.) != exact.is_some());
        }
    }
    let worst = errors.iter().copied().fold(0., f64::max);
    Ok(format!("{{\"rays\":{},\"continuous_mask_mismatches\":{mismatches},\"native_mask_mismatches\":{native_mask},\"unresolved\":{unresolved},\"continuous_error_m_p50_p95_p99\":{:?},\"continuous_max_error_m\":{worst},\"native_error_m_p50_p95_p99\":{:?}}}", side*side,stats(errors),stats(native)))
}

fn main() -> Result<()> {
    let begin = Instant::now();
    let Some(o) = options()? else {
        return Ok(());
    };
    fs::create_dir_all(&o.output)?;
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..Default::default()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok_or("no Vulkan adapter")?;
    let info = adapter.get_info();
    eprintln!("Adapter {:?}", info);
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("continuous Alpine"),
            required_features: wgpu::Features::TIMESTAMP_QUERY,
            required_limits: wgpu::Limits::default(),
            memory_hints: Default::default(),
        },
        None,
    ))?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("continuous Alpine bounds and ray solver"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let bytes = u64::from(o.width) * u64::from(o.height) * 16;
    let storage = |size, label| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    };
    let view = o.view(0.);
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("view"),
        contents: bytemuck::bytes_of(&view),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let bounds = storage(BOUNDS_BYTES, "bounds-only cache");
    let hits = storage(bytes, "RGB and ray distance");
    let counters = storage(if o.instrument { bytes } else { 16 }, "optional counters");
    let atlas_bytes = if o.gradient_cache {
        let last = view.lattice[8];
        (last[3] + last[2] * last[2]) as u64 * 4
    } else {
        4
    };
    let gradients = storage(atlas_bytes, "seeded gradient atlas");
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &std::array::from_fn::<_, 5, _>(|i| wgpu::BindGroupLayoutEntry {
            binding: i as u32,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: if i == 0 {
                    wgpu::BufferBindingType::Uniform
                } else {
                    wgpu::BufferBindingType::Storage { read_only: false }
                },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }),
    });
    let pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let pipeline = |name, repair, numeric| {
        let mut constants = std::collections::HashMap::new();
        if name == "render" || name == "build" {
            constants.insert("USE_ATLAS".to_owned(), f64::from(o.gradient_cache));
        }
        if name == "render" {
            constants.extend([
                ("INSTRUMENT".to_owned(), f64::from(o.instrument)),
                ("MODE".to_owned(), f64::from(o.mode)),
                ("REPAIR".to_owned(), f64::from(repair)),
                ("NUMERICAL".to_owned(), f64::from(numeric)),
            ]);
        }
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(name),
            layout: Some(&pipe_layout),
            module: &shader,
            entry_point: Some(name),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &constants,
                ..Default::default()
            },
            cache: None,
        })
    };
    let atlas = o
        .gradient_cache
        .then(|| pipeline("fill_atlas", false, false));
    let builders =
        ["build", "reduce_medium", "reduce_coarse"].map(|name| pipeline(name, false, false));
    let render = pipeline("render", false, false);
    let repair = o.repair.then(|| pipeline("render", true, false));
    let numerical = o.numerical.then(|| pipeline("render", false, true));
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[&uniform, &bounds, &hits, &counters, &gradients]
            .into_iter()
            .enumerate()
            .map(|(i, b)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: b.as_entire_binding(),
            })
            .collect::<Vec<_>>(),
    });
    let encoder = || device.create_command_encoder(&Default::default());
    let mut cmd = encoder();
    if let Some(atlas) = &atlas {
        let mut pass = cmd.begin_compute_pass(&Default::default());
        pass.set_pipeline(atlas);
        pass.set_bind_group(0, &bind, &[]);
        let side = view.lattice.iter().map(|l| l[2]).max().unwrap_or(1) as u32;
        pass.dispatch_workgroups((side * side).div_ceil(64), 9, 1);
    }
    for (pipe, groups) in builders.iter().zip([32, 8, 2]) {
        let mut pass = cmd.begin_compute_pass(&Default::default());
        pass.set_pipeline(pipe);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(groups, groups, 1);
    }
    let bound_start = Instant::now();
    queue.submit([cmd.finish()]);
    device.poll(wgpu::Maintain::Wait);
    let bound_ms = bound_start.elapsed().as_secs_f64() * 1000.;
    let tile = if o.tile == 0 {
        o.width.max(o.height)
    } else {
        o.tile
    };
    let tiles_x = o.width.div_ceil(tile);
    let tiles_y = o.height.div_ceil(tile);
    let query_count = tiles_x * tiles_y * 2;
    let query = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: None,
        ty: wgpu::QueryType::Timestamp,
        count: query_count,
    });
    let resolve = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(query_count) * 8,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let timing = read_buffer(&device, u64::from(query_count) * 8);
    let mut gpu_times = Vec::new();
    let mut frame_times = Vec::new();
    let mut first_frame_ms = 0.;
    let mut final_view = view;
    let mut first_image = None;
    let motion = o.turn != 0. || o.approach != 0.;
    let motion_dir = o.output.join("frames");
    if o.capture_motion {
        fs::create_dir_all(&motion_dir)?;
    }
    let image_read = read_buffer(&device, bytes);
    for frame in 0..(o.frames + o.warmup) {
        let tic = Instant::now();
        let fraction =
            frame.saturating_sub(o.warmup) as f64 / o.frames.saturating_sub(1).max(1) as f64;
        final_view = o.view(fraction);
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let x = tx * tile;
                let y = ty * tile;
                let mut tile_view = final_view;
                tile_view.image[2] = x;
                tile_view.image[3] = y;
                tile_view.controls[3] = (o.width - x).min(tile) as f32;
                queue.write_buffer(&uniform, 0, bytemuck::bytes_of(&tile_view));
                let mut cmd = encoder();
                let at = (ty * tiles_x + tx) * 2;
                {
                    let mut pass = cmd.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("continuous primary rays"),
                        timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                            query_set: &query,
                            beginning_of_pass_write_index: Some(at),
                            end_of_pass_write_index: Some(at + 1),
                        }),
                    });
                    pass.set_bind_group(0, &bind, &[]);
                    for pipeline in std::iter::once(&render).chain(&repair).chain(&numerical) {
                        pass.set_pipeline(pipeline);
                        pass.dispatch_workgroups(
                            (o.width - x).min(tile).div_ceil(8),
                            (o.height - y).min(tile).div_ceil(8),
                            1,
                        );
                    }
                }
                queue.submit([cmd.finish()]);
            }
        }
        let mut cmd = encoder();
        cmd.resolve_query_set(&query, 0..query_count, &resolve, 0);
        cmd.copy_buffer_to_buffer(&resolve, 0, &timing, 0, u64::from(query_count) * 8);
        let save_motion_frame = o.capture_motion && frame >= o.warmup;
        let save_first = motion && frame == o.warmup;
        if save_motion_frame || save_first {
            cmd.copy_buffer_to_buffer(&hits, 0, &image_read, 0, bytes);
        }
        queue.submit([cmd.finish()]);
        let data = read(&device, &timing)?;
        let ticks = bytemuck::cast_slice::<u8, u64>(&data)
            .chunks_exact(2)
            .map(|p| p[1].saturating_sub(p[0]))
            .sum::<u64>();
        let dt = ticks as f64 * queue.get_timestamp_period() as f64 / 1e6;
        if frame == 0 {
            first_frame_ms = begin.elapsed().as_secs_f64() * 1000.;
            eprintln!(
                "First frame {:0.2} ms GPU, {:0.1} ms from process",
                dt, first_frame_ms
            );
        }
        if frame >= o.warmup {
            gpu_times.push(dt);
            frame_times.push(tic.elapsed().as_secs_f64() * 1000.);
        }
        if save_motion_frame || save_first {
            let image = read(&device, &image_read)?;
            if save_motion_frame {
                png(
                    motion_dir.join(format!("{:04}.png", frame - o.warmup)),
                    o.width,
                    o.height,
                    bytemuck::cast_slice(&image),
                )?;
            }
            if save_first {
                first_image = Some(image);
            }
        }
    }
    let mut cmd = encoder();
    cmd.copy_buffer_to_buffer(&hits, 0, &image_read, 0, bytes);
    let count_read = o.instrument.then(|| {
        let read = read_buffer(&device, bytes);
        cmd.copy_buffer_to_buffer(&counters, 0, &read, 0, bytes);
        read
    });
    queue.submit([cmd.finish()]);
    let data = read(&device, &image_read)?;
    let mut pixels = bytemuck::cast_slice::<u8, [f32; 4]>(&data).to_vec();
    let mut numerical_pixels = 0usize;
    for p in &mut pixels {
        if p[3] >= 32768. {
            p[3] -= 32768.;
            numerical_pixels += 1;
        } else if p[3] == -3. {
            p[3] = -1.;
            numerical_pixels += 1;
        }
    }
    if let Some(first) = first_image {
        png(
            o.output.join("start.png"),
            o.width,
            o.height,
            bytemuck::cast_slice(&first),
        )?;
    }
    png(o.output.join("terrain.png"), o.width, o.height, &pixels)?;
    let depths: Vec<_> = pixels.iter().map(|p| p[3]).collect();
    let missed = depths.iter().filter(|&&t| t == -2.).count();
    let terrain = depths.iter().filter(|&&t| t >= 0.).count();
    fs::write(
        o.output.join("ray-distance.f32"),
        bytemuck::cast_slice(&depths),
    )?;
    let count_json = count_read
        .map(|buffer| {
            let data = read(&device, &buffer)?;
            let counters = bytemuck::cast_slice::<u8, [u32; 4]>(&data);
            let columns = std::array::from_fn::<_, 4, _>(|axis| {
                stats(counters.iter().map(|p| p[axis] as f64).collect())
            });
            Ok::<_, Box<dyn Error>>(format!("{columns:?}"))
        })
        .transpose()?
        .unwrap_or_else(|| "null".into());
    let check = oracle(&o, &final_view, &pixels)?;
    let gpu = stats(gpu_times);
    let frame = stats(frame_times);
    let rss = fs::read_to_string("/proc/self/status").ok().and_then(|s| {
        s.lines()
            .find(|l| l.starts_with("VmHWM:"))
            .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
    });
    let report=format!(
        "{{\"adapter\":{:?},\"driver\":{:?},\"seed\":{},\"eye\":{:?},\"target\":{:?},\"fov_y\":{},\"far_m\":{},\"width\":{},\"height\":{},\"mode\":{},\"shading\":{},\"tile\":{},\"frames\":{},\"warmup\":{},\"turn_deg\":{},\"approach_m\":{},\"instrument\":{},\"capture_motion\":{},\"repair\":{},\"numerical_fallback\":{},\"gpu_ms_p50_p95_p99\":{gpu:?},\"frame_ms_p50_p95_p99\":{frame:?},\"bounds_build_wall_ms\":{bound_ms},\"process_first_frame_wall_ms\":{first_frame_ms},\"bounds_bytes\":{BOUNDS_BYTES},\"gradient_atlas_bytes\":{atlas_bytes},\"numerical_fallback_pixels\":{numerical_pixels},\"output_gpu_bytes\":{bytes},\"rss_peak_kib\":{},\"terrain_pixels\":{terrain},\"unresolved_pixels\":{missed},\"counter_names\":[\"leaf_intervals\",\"full_height_evaluations\",\"detailed_bands_bounded\",\"hierarchy_steps\"],\"counters_p50_p95_p99\":{count_json},\"oracle\":{check}}}",
        info.name,info.driver_info,o.seed,o.eye.to_array(),o.target.to_array(),o.fov,o.far,o.width,o.height,o.mode,o.shading,o.tile,o.frames,o.warmup,o.turn,o.approach,o.instrument,o.capture_motion,o.repair,o.numerical,rss.map_or("null".to_owned(),|n|n.to_string())
    );
    fs::write(o.output.join("report.json"), &report)?;
    println!("{report}");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn bounds_and_trace_compute_shader_validate() {
        let source = super::SHADER;
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
    }
}
