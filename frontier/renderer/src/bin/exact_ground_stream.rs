//! Experimental verified first-hit rendering from streamed canonical ground.
#[path = "exact_ground_stream/cache.rs"]
mod cache;
#[path = "exact_ground_stream/gpu.rs"]
mod gpu;
#[path = "exact_ground_stream/window.rs"]
mod window;
use bytemuck::{Pod, Zeroable};
use cache::{Cache, NATIVE_PITCH, ROOT_LOG};
use frontier_world::{FbmParams, FbmRecipe, FbmTerrain};
use glam::{DQuat, DVec3};
use std::{
    error::Error,
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::Instant,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct View {
    eye: [f32; 4],
    forward: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    size: [u32; 4],
    camera_cell: [i32; 4],
}
impl View {
    fn direction(self, x: u32, y: u32) -> DVec3 {
        let xyz = |v: [f32; 4]| DVec3::new(v[0].into(), v[1].into(), v[2].into());
        let u = (f64::from(x) + 0.5) * 2. / f64::from(self.size[0]) - 1.;
        let v = (f64::from(y) + 0.5) * 2. / f64::from(self.size[1]) - 1.;
        (xyz(self.forward) + u * xyz(self.right) - v * xyz(self.up)).normalize()
    }
}

struct Options {
    width: u32,
    height: u32,
    far: f64,
    eye: DVec3,
    target: DVec3,
    seed: u64,
    recipe: FbmRecipe,
    page_side: u32,
    cache_mib: u32,
    node_limit: usize,
    workers: usize,
    land_bound: bool,
    prefetch: bool,
    stackless: bool,
    batch: usize,
    rounds: u32,
    frames: u32,
    step: f64,
    speed: f64,
    bank: f64,
    audit: u32,
    software: bool,
    capture_motion: bool,
    interactive: bool,
    output: PathBuf,
}
impl Options {
    fn read() -> Result<Option<Self>> {
        let mut o = Self {
            width: 320,
            height: 180,
            far: 6000.,
            eye: DVec3::new(-6688., 435.75, 6080.),
            target: DVec3::new(-6547.9551, 463.5846, 6220.0449),
            seed: 1685576058315138427,
            recipe: FbmRecipe::Eroded,
            page_side: 16,
            cache_mib: 96,
            node_limit: 1 << 20,
            workers: 8,
            land_bound: true,
            prefetch: false,
            stackless: false,
            batch: 65536,
            rounds: 64,
            frames: 1,
            step: 1. / 60.,
            speed: 80.,
            bank: 0.,
            audit: 0,
            software: false,
            capture_motion: false,
            interactive: false,
            output: "output/exact-ground-stream".into(),
        };
        let mut args = std::env::args().skip(1);
        while let Some(name) = args.next() {
            match name.as_str() {
                "--help" => {
                    println!("exact_ground_stream [--interactive] [--seed N --recipe eroded|legacy|alpine --eye X,Y,Z --target X,Y,Z --far 6000] [--width 320 --height 180 --page-side 16 --cache-mib 96 --node-limit 1048576 --workers 8 --batch 65536 --rounds 64 --frames 1 --step .0166667 --speed 80 --bank 0 --audit 0 --capture-motion --allow-software --output DIR] [--full-bounds --stackless --prefetch-siblings]");
                    return Ok(None);
                }
                "--allow-software" => o.software = true,
                "--capture-motion" => o.capture_motion = true,
                "--interactive" => o.interactive = true,
                "--full-bounds" => o.land_bound = false,
                "--prefetch-siblings" => o.prefetch = true,
                "--stackless" => o.stackless = true,
                _ => {
                    let val = args.next().ok_or_else(|| format!("{name} needs a value"))?;
                    match name.as_str() {
                        "--width" => o.width = val.parse()?,
                        "--height" => o.height = val.parse()?,
                        "--far" => o.far = val.parse()?,
                        "--eye" => o.eye = position(&val)?,
                        "--target" => o.target = position(&val)?,
                        "--seed" => o.seed = val.parse()?,
                        "--recipe" => {
                            o.recipe = match val.as_str() {
                                "eroded" => FbmRecipe::Eroded,
                                "legacy" => FbmRecipe::Legacy,
                                "alpine" => FbmRecipe::Alpine,
                                _ => return Err("unsupported recipe".into()),
                            }
                        }
                        "--page-side" => o.page_side = val.parse()?,
                        "--cache-mib" => o.cache_mib = val.parse()?,
                        "--node-limit" => o.node_limit = val.parse()?,
                        "--workers" => o.workers = val.parse()?,
                        "--batch" => o.batch = val.parse()?,
                        "--rounds" => o.rounds = val.parse()?,
                        "--frames" => o.frames = val.parse()?,
                        "--step" => o.step = val.parse()?,
                        "--speed" => o.speed = val.parse()?,
                        "--bank" => o.bank = val.parse::<f64>()?.to_radians(),
                        "--audit" => o.audit = val.parse()?,
                        "--output" => o.output = val.into(),
                        _ => return Err(format!("unknown flag {name}").into()),
                    }
                }
            }
        }
        if !(32..=1920).contains(&o.width)
            || !(32..=1080).contains(&o.height)
            || !(1. ..=7000.).contains(&o.far)
            || !(0. ..=130.).contains(&o.speed)
            || !(0.001..=2.).contains(&o.step)
            || o.eye.abs().max_element() > 1_000_000.
            || (o.target - o.eye).with_y(0.).length_squared() < 1e-8
            || !(0. ..=1.6).contains(&o.bank.abs())
            || ![4, 8, 16, 32, 64].contains(&o.page_side)
            || !(1..=96).contains(&o.cache_mib)
            || !(5..=(1 << 22)).contains(&o.node_limit)
            || !(1..=32).contains(&o.workers)
            || !(1..=600_000).contains(&o.batch)
            || !(1..=512).contains(&o.rounds)
            || !(1..=3600).contains(&o.frames)
            || o.audit > 32
        {
            return Err("benchmark options out of range".into());
        }
        Ok(Some(o))
    }
    fn pose(&self, frame: u32) -> (DVec3, DVec3, DVec3, DVec3) {
        let base = (self.target - self.eye).normalize();
        let flight = base.with_y(0.).normalize();
        let time = f64::from(frame) * self.step;
        let progress = f64::from(frame) / f64::from(self.frames.saturating_sub(1).max(1));
        let yaw = self.bank * 1.6 * progress;
        let forward = DQuat::from_rotation_y(yaw) * base;
        let roll =
            DQuat::from_axis_angle(forward, self.bank * (std::f64::consts::PI * progress).sin());
        let right = roll * forward.cross(DVec3::Y).normalize();
        let up = right.cross(forward).normalize();
        (self.eye + flight * (self.speed * time), forward, right, up)
    }
    fn view(&self, frame: u32, cache: &Cache) -> View {
        self.view_from_pose(cache, self.pose(frame))
    }

    fn view_from_pose(
        &self,
        cache: &Cache,
        (mut eye, forward, right, up): (DVec3, DVec3, DVec3, DVec3),
    ) -> View {
        let x = eye.x / NATIVE_PITCH - cache.origin[0] as f64;
        let z = eye.z / NATIVE_PITCH - cache.origin[1] as f64;
        let whole = [x.floor() as i32, z.floor() as i32];
        eye.x = x - f64::from(whole[0]);
        eye.z = z - f64::from(whole[1]);
        let tan = 0.5773502691896258;
        View {
            eye: eye.as_vec3().extend(self.far as f32).to_array(),
            forward: forward.as_vec3().extend(0.).to_array(),
            right: (right * tan * f64::from(self.width) / f64::from(self.height))
                .as_vec3()
                .extend(0.)
                .to_array(),
            up: (up * tan).as_vec3().extend(0.).to_array(),
            size: [self.width, self.height, ROOT_LOG, self.page_side.ilog2()],
            camera_cell: [whole[0], whole[1], 0, 0],
        }
    }
}
fn position(text: &str) -> Result<DVec3> {
    let fields = text
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<Vec<f64>, _>>()?;
    if fields.len() != 3 || !fields.iter().all(|x| x.is_finite()) {
        return Err("expected three finite X,Y,Z coordinates".into());
    }
    Ok(DVec3::new(fields[0], fields[1], fields[2]))
}
fn millis(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.
}
fn metrics(values: &mut [f64]) -> [f64; 3] {
    values.sort_by(f64::total_cmp);
    [0.50, 0.95, 0.99]
        .map(|q| values[((values.len() as f64 * q).ceil() as usize).saturating_sub(1)])
}

fn classify(hits: &[[u32; 2]], cache: &Cache) -> Result<(Vec<(u32, u32)>, usize, usize)> {
    let mut counts = vec![0u32; cache.nodes.len()];
    let mut pending = 0;
    let mut failed = 0;
    for [bits, key] in hits.iter().copied() {
        match f32::from_bits(bits) {
            -2. => {
                let need = counts
                    .get_mut(key as usize)
                    .ok_or("unresolved node outside live tree")?;
                *need += 1;
                pending += 1;
            }
            -3. => failed += 1,
            _ => (),
        }
    }
    let mut requests: Vec<_> = counts
        .into_iter()
        .enumerate()
        .filter(|&(_, n)| n > 0)
        .map(|(id, n)| (id as u32, n))
        .collect();
    requests.sort_unstable_by_key(|&(id, count)| (std::cmp::Reverse(count), id));
    Ok((requests, pending, failed))
}

fn image(path: &Path, view: View, hits: &[[u32; 2]]) -> Result<()> {
    let [width, height, _, _] = view.size;
    let sun = DVec3::new(-0.54, 0.66, -0.52).normalize();
    let normal = [DVec3::Y, -DVec3::X, DVec3::X, -DVec3::Z, DVec3::Z];
    let mut rgb = Vec::with_capacity(hits.len() * 3);
    for (index, &[bits, mat]) in hits.iter().enumerate() {
        let d = f32::from_bits(bits);
        let color = if d >= 0. {
            let n = normal[(mat >> 8) as usize];
            let light = (0.39 + 0.16 * n.y + 0.52 * n.dot(sun).max(0.)) as f32;
            cache::albedo(mat & 255).map(|v| v * light)
        } else if d == -1. {
            [0.55, 0.68, 0.84]
        } else {
            let x = index % width as usize;
            let y = index / width as usize;
            if ((x / 8) ^ (y / 8)) & 1 == 0 {
                [0.85, 0.01, 0.45]
            } else {
                [0.025, 0.009, 0.02]
            }
        };
        rgb.extend(color.map(|v| {
            let v = if v <= 0.0031308 {
                12.92 * v
            } else {
                1.055 * v.powf(1. / 2.4) - 0.055
            };
            (v.clamp(0., 1.) * 255.).round() as u8
        }));
    }
    let mut enc = png::Encoder::new(BufWriter::new(File::create(path)?), width, height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgb)?;
    Ok(())
}

fn cpu_hit(terrain: &FbmTerrain, origin: DVec3, d: DVec3, limit: f64) -> Option<f64> {
    let mut start = 0.025;
    let cell_size = NATIVE_PITCH;
    let s = origin + d * start;
    let mut cell = [
        (s.x / cell_size).floor() as i64,
        (s.z / cell_size).floor() as i64,
    ];
    let sign = [d.x, d.z].map(|v| if v > 0. { 1i64 } else { -1 });
    loop {
        let a = [0, 1].map(|axis| {
            let o = [origin.x, origin.z][axis];
            let v = [d.x, d.z][axis];
            if v == 0. {
                f64::INFINITY
            } else {
                ((cell[axis] + i64::from(sign[axis] > 0)) as f64 * cell_size - o) / v
            }
        });
        let end = a[0].min(a[1]).min(limit).max(start);
        if end > start {
            let y = terrain.column_cells(cell[0], cell[1]).ground_top as f64 * cell_size;
            if origin.y + d.y * start <= y {
                return Some(start);
            }
            if d.y < 0. && origin.y + d.y * end <= y {
                return Some((y - origin.y) / d.y);
            }
        }
        if end >= limit {
            return None;
        }
        for axis in 0..2 {
            if a[axis] <= end {
                cell[axis] += sign[axis];
            }
        }
        start = end;
    }
}
fn audit(
    o: &Options,
    terrain: &FbmTerrain,
    eye: DVec3,
    view: View,
    hits: &[[u32; 2]],
) -> (usize, f64) {
    let mut bad = 0;
    let mut max: f64 = 0.;
    for row in 0..o.audit {
        for col in 0..o.audit {
            let x = (2 * col + 1) * o.width / (2 * o.audit);
            let y = (2 * row + 1) * o.height / (2 * o.audit);
            let depth = f32::from_bits(hits[(x + y * o.width) as usize][0]);
            let target = cpu_hit(terrain, eye, view.direction(x, y), o.far);
            if let Some(t) = target.filter(|_| depth >= 0.) {
                max = max.max((f64::from(depth) - t).abs());
            } else if depth != -1. || target.is_some() {
                bad += 1;
            }
        }
    }
    (bad, max)
}

fn main() -> Result<()> {
    let Some(o) = Options::read()? else {
        return Ok(());
    };
    if o.interactive {
        return window::run(o);
    }
    let params = FbmParams {
        recipe: o.recipe,
        ..Default::default()
    };
    let terrain = FbmTerrain::new(o.seed, params)?;
    let workers = cache::Workers::new(&terrain, o.workers, o.page_side, o.land_bound);
    let capacity_words = o.cache_mib * 1024 * 256;
    let mut cache = Cache::new(o.eye.to_array(), o.page_side, capacity_words, o.node_limit);
    fs::create_dir_all(&o.output)?;
    let mut trace = BufWriter::new(File::create(o.output.join("rounds.csv"))?);
    let mut report = BufWriter::new(File::create(o.output.join("frames.csv"))?);
    writeln!(trace,"frame,round,pending,failed,requested,submitted_jobs,nodes,pages,gpu_ms,readback_wall_ms,build_ms")?;
    writeln!(report,"frame,pending,failed,rounds,ready_wall_ms,ready_gpu_ms,nodes,pages,branch_jobs,page_jobs,root_resets,ground_clearance_m,farthest_hit_m,hits_past_1000m,hits_past_3000m")?;
    let gpu = pollster::block_on(gpu::Gpu::new(
        o.width,
        o.height,
        o.node_limit,
        capacity_words,
        o.software,
        o.stackless,
        None,
    ))?;
    gpu.bounds(&cache);
    let capture = o.output.join("frames");
    if o.capture_motion {
        fs::create_dir_all(&capture)?;
    }
    let mut wall_times = Vec::new();
    let mut gpu_times = Vec::new();
    let mut final_bad = 0;
    let mut final_error = 0.;
    let mut unresolved_frames = 0;
    let mut root_resets = 0;
    for frame in 0..o.frames {
        let start = Instant::now();
        let eye = o.pose(frame).0;
        if !cache.cover(eye.to_array(), o.far) {
            cache = Cache::new(eye.to_array(), o.page_side, capacity_words, o.node_limit);
            gpu.bounds(&cache);
            root_resets += 1;
        }
        let view = o.view(frame, &cache);
        for round in 0..o.rounds {
            let timer = Instant::now();
            let (hits, gpu_ms) = gpu.render(&view)?;
            let read_ms = millis(timer);
            let (requests, pending, failed) = classify(&hits, &cache)?;
            let ready = pending == 0 && failed == 0;
            let mut jobs = 0;
            let mut build_ms = 0.;
            if !requests.is_empty() && round + 1 < o.rounds && failed == 0 {
                let timer = Instant::now();
                let data = cache.fill(&workers, &requests, o.batch, o.prefetch);
                build_ms = millis(timer);
                jobs = data.jobs;
                if jobs > 0 {
                    gpu.publish(&cache, &data);
                }
            }
            writeln!(
                trace,
                "{frame},{round},{pending},{failed},{},{jobs},{},{},{gpu_ms},{read_ms},{build_ms}",
                requests.len(),
                cache.nodes.len(),
                cache.pages
            )?;
            if ready || jobs == 0 || failed > 0 {
                let wall = millis(start);
                if ready {
                    wall_times.push(wall);
                    gpu_times.push(gpu_ms);
                } else {
                    unresolved_frames += 1;
                }
                let ground = terrain
                    .column_cells(
                        (eye.x / NATIVE_PITCH).floor() as i64,
                        (eye.z / NATIVE_PITCH).floor() as i64,
                    )
                    .ground_top as f64
                    * NATIVE_PITCH;
                let clearance = eye.y - ground;
                let mut farthest = 0f32;
                let mut beyond = [0u32; 2];
                for &[bits, _] in &hits {
                    let depth = f32::from_bits(bits);
                    farthest = farthest.max(depth);
                    beyond[0] += u32::from(depth > 1000.);
                    beyond[1] += u32::from(depth > 3000.);
                }
                writeln!(
                    report,
                    "{frame},{pending},{failed},{},{wall},{gpu_ms},{},{},{},{},{root_resets},{clearance},{farthest},{},{}",
                    round + 1,
                    cache.nodes.len(),
                    cache.pages,
                    cache.branch_jobs,
                    cache.page_jobs,
                    beyond[0], beyond[1]
                )?;
                if frame == 0 || frame + 1 == o.frames {
                    let name = if frame == 0 { "first.png" } else { "last.png" };
                    image(&o.output.join(name), view, &hits)?;
                    eprintln!("frame {frame}: {pending} pending, {failed} failed, {} rounds, {:.1} ms wall, {:.2} ms final GPU, {} pages, {} nodes",round+1,wall,gpu_ms,cache.pages,cache.nodes.len());
                }
                if o.capture_motion {
                    image(&capture.join(format!("{frame:04}.png")), view, &hits)?;
                }
                if frame + 1 == o.frames {
                    if ready && o.audit > 0 {
                        (final_bad, final_error) = audit(&o, &terrain, eye, view, &hits);
                    }
                    let raw: Vec<_> = hits.iter().map(|hit| hit[0]).collect();
                    fs::write(o.output.join("depth.f32"), bytemuck::cast_slice(&raw))?;
                }
                break;
            }
        }
        trace.flush()?;
        report.flush()?;
        if frame == 0 && unresolved_frames > 0 {
            break;
        }
    }
    let timing = |data: &mut Vec<f64>| {
        if data.is_empty() {
            "null".into()
        } else {
            format!("{:?}", metrics(data))
        }
    };
    let summary = format!(
        r#"{{"adapter":{:?},"driver":{:?},"seed":{},"recipe":"{:?}","voxel_m":{},"width":{},"height":{},"horizon_m":{},"frame_count_requested":{},"frame_count_ready":{},"unresolved_frames":{unresolved_frames},"pages":{},"nodes":{},"page_side":{},"resident_page_bytes":{},"root_resets":{root_resets},"gpu_page_capacity_bytes":{},"gpu_bound_capacity_bytes":{},"ready_wall_ms_p50_p95_p99":{},"ready_gpu_ms_p50_p95_p99":{},"audited_rays":{},"audit_coverage_errors":{final_bad},"audit_max_depth_error_m":{final_error}}}"#,
        gpu.info.name,
        gpu.info.driver_info,
        o.seed,
        o.recipe,
        NATIVE_PITCH,
        o.width,
        o.height,
        o.far,
        o.frames,
        wall_times.len(),
        cache.pages,
        cache.nodes.len(),
        o.page_side,
        u64::from(cache.words) * 4,
        u64::from(o.cache_mib) * 1024 * 1024,
        o.node_limit * std::mem::size_of::<cache::Node>(),
        timing(&mut wall_times),
        timing(&mut gpu_times),
        if unresolved_frames == 0 {
            o.audit.pow(2)
        } else {
            0
        }
    );
    fs::write(o.output.join("report.json"), &summary)?;
    println!("{summary}");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn shader_validates() {
        for source in [
            include_str!("exact_ground_stream/trace.wgsl"),
            include_str!("exact_ground_stream/display.wgsl"),
        ] {
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
}
