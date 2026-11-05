use frontier_reference::*;
use frontier_world::{World, SOURCE_VERSION, VOXEL_SIZE};
use std::{
    error::Error,
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
// RGB, coverage, depth min/max/mean/variance, six normals, four materials.
fn vec3(s: &str) -> Result<[f64; 3]> {
    let v = s
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<Vec<f64>, _>>()?;
    if v.len() != 3 {
        return Err("expected X,Y,Z".into());
    }
    Ok([v[0], v[1], v[2]])
}
fn png(path: &Path, w: usize, h: usize, data: &[u8]) -> Result<()> {
    let mut e = png::Encoder::new(BufWriter::new(File::create(path)?), w as u32, h as u32);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    e.write_header()?.write_image_data(data)?;
    Ok(())
}
fn git(args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}
fn jitter(mut x: u64) -> f64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    ((x ^ (x >> 31)) >> 11) as f64 * (1. / 9007199254740992.)
}
fn source_fingerprint(canonical_only: bool) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    let sources = [
        include_str!("lib.rs"),
        include_str!("main.rs"),
        include_str!("../../world/src/lib.rs"),
        include_str!("../../world/src/terrain.rs"),
    ];
    for source in if canonical_only {
        &sources[2..]
    } else {
        &sources[..]
    } {
        for b in source.as_bytes() {
            hash = (hash ^ *b as u64).wrapping_mul(0x100000001b3);
        }
    }
    hash
}
fn write_centers(
    scene: &Snapshot,
    camera: &Camera,
    width: usize,
    height: usize,
    threads: usize,
    accelerated: bool,
    output: &Path,
    world: &World,
) -> Result<()> {
    let mut values = vec![[0f32, f32::NAN]; width * height];
    let (forward, right, up) = camera.basis();
    let h = (camera.fov_y_degrees.to_radians() * 0.5).tan();
    let rows_per_thread = height.div_ceil(threads.min(height));
    std::thread::scope(|scope| {
        for (chunk, rows) in values.chunks_mut(width * rows_per_thread).enumerate() {
            scope.spawn(move || {
                for (ly, row) in rows.chunks_mut(width).enumerate() {
                    for (x, p) in row.iter_mut().enumerate() {
                        let dx = (2. * (x as f64 + 0.5) / width as f64 - 1.) * h * width as f64
                            / height as f64;
                        let dy = (1.
                            - 2. * ((chunk * rows_per_thread + ly) as f64 + 0.5) / height as f64)
                            * h;
                        let d = normalized(std::array::from_fn(|a| {
                            forward[a] + right[a] * dx + up[a] * dy
                        }));
                        let cosine = dot(forward, d);
                        if let Some(hit) = scene.ray(
                            camera.eye,
                            d,
                            camera.near / cosine,
                            camera.far / cosine,
                            accelerated,
                        ) {
                            *p = [1., hit.distance as f32];
                        }
                    }
                }
            });
        }
    });
    for (channel, name) in ["center_coverage.f32", "center_depth.f32"]
        .iter()
        .enumerate()
    {
        let mut f = BufWriter::new(File::create(output.join(name))?);
        for p in &values {
            f.write_all(&p[channel].to_le_bytes())?;
        }
        f.flush()?;
    }
    let seed = world.seed();
    let edit_revision = world.revision();
    let hash = source_fingerprint(false);
    let canonical_hash = source_fingerprint(true);
    let dirty = !git(&["status", "--porcelain"]).is_empty();
    fs::write(
        output.join("center_manifest.json"),
        format!(
            r#"{{"schema":"frontier-reference-centers-v1","source":"World::new","scene":"fixture","source_version":{SOURCE_VERSION},"seed":{seed},"edit_revision":{edit_revision},"voxel_size_m":{VOXEL_SIZE},"native_min_cell":{FIXTURE_MIN:?},"native_size_cells":{FIXTURE_SIZE:?},"source_fnv1a64":"{hash:016x}","canonical_source_fnv1a64":"{canonical_hash:016x}","git_dirty":{dirty},"width":{width},"height":{height},"eye":{:?},"target":{:?},"fov_y_degrees":{},"near":{},"far":{},"sampling":"pixel center (0.5,0.5)","depth":"ray distance meters, NaN on miss","raw_format":"little endian f32, top-left row-major","git_revision":"{}"}}"#,
            camera.eye,
            camera.target,
            camera.fov_y_degrees,
            camera.near,
            camera.far,
            git(&["rev-parse", "HEAD"])
        ),
    )?;
    eprintln!("Wrote pixel-center coverage/depth");
    Ok(())
}
fn main() -> Result<()> {
    let started = Instant::now();
    let (mut width, mut height, mut spp, mut seed) = (1920usize, 1080usize, 64usize, 7u64);
    let mut sample_seed = 0u64;
    let mut output = PathBuf::from("reference-output");
    let mut camera = Camera::default();
    let mut threads = std::thread::available_parallelism().map_or(1, usize::from);
    let mut accelerate = true;
    let mut centers_only = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--help" {
            println!("frontier-reference --output DIR --width 1920 --height 1080 --spp 64|256 --seed 7 --sample-seed 0 --camera fixture|cave|bridge --eye X,Y,Z --target X,Y,Z --fov-y 60 --near .025 --far 20000 --threads N [--plain] [--centers-only]");
            return Ok(());
        }
        if arg == "--centers-only" {
            centers_only = true;
            continue;
        }
        if arg == "--plain" {
            accelerate = false;
            continue;
        }
        let v = args.next().ok_or("missing option value")?;
        match arg.as_str() {
            "--output" => output = v.into(),
            "--width" => width = v.parse()?,
            "--height" => height = v.parse()?,
            "--spp" => spp = v.parse()?,
            "--seed" => seed = v.parse()?,
            "--sample-seed" => sample_seed = v.parse()?,
            "--threads" => threads = v.parse()?,
            "--eye" => camera.eye = vec3(&v)?,
            "--target" => camera.target = vec3(&v)?,
            "--fov" | "--fov-y" => camera.fov_y_degrees = v.parse()?,
            "--near" => camera.near = v.parse()?,
            "--far" => camera.far = v.parse()?,
            "--camera" => match v.as_str() {
                "fixture" => camera = Camera::default(),
                "cave" => {
                    camera.eye = [-3., 3., -7.];
                    camera.target = [-3., 3., 6.];
                }
                "bridge" => {
                    camera.eye = [4.5, 2.8, -2.5];
                    camera.target = [4.5, 4.1, -1.5];
                }
                _ => return Err("unknown camera preset".into()),
            },
            _ => return Err(format!("unknown option {arg}").into()),
        }
    }
    let grid = (spp as f64).sqrt() as usize;
    if grid * grid != spp || spp == 0 || spp > 65536 {
        return Err("spp must be a positive square (64 or 256 recommended)".into());
    }
    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || threads == 0
        || camera.fov_y_degrees <= 0.
        || camera.fov_y_degrees >= 179.
        || camera.near <= 0.
        || camera.far <= camera.near
        || ![camera.near, camera.far, camera.fov_y_degrees]
            .iter()
            .chain(camera.eye.iter())
            .chain(camera.target.iter())
            .all(|v| v.is_finite())
    {
        return Err("invalid image/camera/thread parameters".into());
    }
    let (forward, right, up) = camera.basis();
    if !forward.iter().chain(right.iter()).all(|v| v.is_finite()) {
        return Err("camera needs distinct eye/target and nonvertical view".into());
    }
    fs::create_dir_all(&output)?;
    let world = World::new(seed);
    eprintln!("Building independent canonical snapshot...");
    let build_started = Instant::now();
    let scene = Snapshot::from_world(&world, FIXTURE_MIN, FIXTURE_SIZE);
    let build_seconds = build_started.elapsed().as_secs_f64();
    eprintln!(
        "Snapshot {:.2}s, {} bytes; rendering {}x{} at {} spp on {} threads",
        build_seconds,
        scene.memory_bytes(),
        width,
        height,
        spp,
        threads
    );
    write_centers(
        &scene, &camera, width, height, threads, accelerate, &output, &world,
    )?;
    if centers_only {
        return Ok(());
    }
    let render_started = Instant::now();
    let mut pixels = vec![[0f32; 18]; width * height];
    let completed = AtomicUsize::new(0);
    let h = (camera.fov_y_degrees.to_radians() * 0.5).tan();
    let aspect = width as f64 / height as f64;
    let rows_per_thread = height.div_ceil(threads.min(height));
    std::thread::scope(|scope| {
        for (chunk, rows) in pixels.chunks_mut(width * rows_per_thread).enumerate() {
            let scene = &scene;
            let camera = &camera;
            let completed = &completed;
            scope.spawn(move || {
                for (ly, row) in rows.chunks_mut(width).enumerate() {
                    let y = chunk * rows_per_thread + ly;
                    for (x, out) in row.iter_mut().enumerate() {
                        let mut rgb = [0f64; 3];
                        let mut count = 0usize;
                        let (mut mean, mut m2, mut min, mut max) =
                            (0f64, 0f64, f64::INFINITY, f64::NEG_INFINITY);
                        let mut normals = [0u32; 6];
                        let mut materials = [0u32; 4];
                        for sy in 0..grid {
                            for sx in 0..grid {
                                let key = sample_seed
                                    ^ ((y * width + x) as u64).wrapping_mul(0x9e3779b97f4a7c15)
                                    ^ ((sy * grid + sx) as u64).wrapping_mul(0xd1b54a32d192ed03);
                                let px = x as f64 + (sx as f64 + jitter(key)) / grid as f64;
                                let py = y as f64
                                    + (sy as f64 + jitter(key ^ 0xa0761d6478bd642f)) / grid as f64;
                                let dx = (2. * px / width as f64 - 1.) * h * aspect;
                                let dy = (1. - 2. * py / height as f64) * h;
                                let d = normalized(std::array::from_fn(|a| {
                                    forward[a] + right[a] * dx + up[a] * dy
                                }));
                                let cosine = dot(forward, d);
                                let color = if let Some(hit) = scene.ray(
                                    camera.eye,
                                    d,
                                    camera.near / cosine,
                                    camera.far / cosine,
                                    accelerate,
                                ) {
                                    count += 1;
                                    let delta = hit.distance - mean;
                                    mean += delta / count as f64;
                                    m2 += delta * (hit.distance - mean);
                                    min = min.min(hit.distance);
                                    max = max.max(hit.distance);
                                    normals[hit.face] += 1;
                                    materials[hit.material as usize - 1] += 1;
                                    shade(hit)
                                } else {
                                    SKY
                                };
                                for a in 0..3 {
                                    rgb[a] += color[a];
                                }
                            }
                        }
                        out[0..3].copy_from_slice(&rgb.map(|v| (v / spp as f64) as f32));
                        out[3] = count as f32 / spp as f32;
                        out[4..8].copy_from_slice(&if count > 0 {
                            [
                                min as f32,
                                max as f32,
                                mean as f32,
                                (m2 / count as f64) as f32,
                            ]
                        } else {
                            [f32::NAN; 4]
                        });
                        out[8..14].copy_from_slice(&normals.map(|v| v as f32 / spp as f32));
                        out[14..18].copy_from_slice(&materials.map(|v| v as f32 / spp as f32));
                    }
                    let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                    if done % (height / 20).max(1) == 0 || done == height {
                        eprintln!(
                            "render {done}/{height} rows ({:.1}s)",
                            render_started.elapsed().as_secs_f64()
                        );
                    }
                }
            });
        }
    });
    let render_seconds = render_started.elapsed().as_secs_f64();
    for (name, lo, hi) in [
        ("linear_rgb", 0, 3),
        ("coverage", 3, 4),
        ("depth", 4, 8),
        ("normals", 8, 14),
        ("materials", 14, 18),
    ] {
        let mut f = BufWriter::new(File::create(output.join(format!("{name}.f32")))?);
        for p in &pixels {
            for v in &p[lo..hi] {
                f.write_all(&v.to_le_bytes())?;
            }
        }
        f.flush()?;
    }
    let depth_min = pixels
        .iter()
        .filter(|p| p[3] > 0.)
        .map(|p| p[4])
        .fold(f32::INFINITY, f32::min);
    let depth_max = pixels
        .iter()
        .filter(|p| p[3] > 0.)
        .map(|p| p[5])
        .fold(f32::NEG_INFINITY, f32::max);
    for name in ["color", "coverage", "depth", "normals", "materials"] {
        let mut data = Vec::with_capacity(width * height * 3);
        for p in &pixels {
            let c = match name {
                "color" => [p[0], p[1], p[2]].map(|v| srgb(v as f64)),
                "coverage" => [(p[3] * 255.).round() as u8; 3],
                "depth" => {
                    let t = if p[3] > 0. {
                        1. - (p[6] - depth_min) / (depth_max - depth_min).max(1e-6)
                    } else {
                        0.
                    };
                    [(t * 255.).round() as u8; 3]
                }
                "normals" => {
                    let colors = [
                        [1., 0., 0.],
                        [0.4, 0., 0.],
                        [0., 1., 0.],
                        [0., 0.4, 0.],
                        [0., 0., 1.],
                        [0., 0., 0.4],
                    ];
                    std::array::from_fn(|a| {
                        srgb((0..6).map(|i| p[8 + i] as f64 * colors[i][a]).sum())
                    })
                }
                _ => std::array::from_fn(|a| {
                    srgb(
                        (0..4)
                            .map(|i| p[14 + i] as f64 * material((i + 1) as u8).color()[a] as f64)
                            .sum(),
                    )
                }),
            };
            data.extend(c);
        }
        png(&output.join(format!("{name}.png")), width, height, &data)?;
    }
    let hash = source_fingerprint(false);
    let canonical_hash = source_fingerprint(true);
    let revision = git(&["rev-parse", "HEAD"]);
    let dirty = !git(&["status", "--porcelain"]).is_empty();
    let rss = fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
        })
        .map(|kb| kb * 1024);
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cpu = cpuinfo
        .lines()
        .find_map(|l| l.strip_prefix("model name\t: "))
        .unwrap_or("unknown")
        .replace(['"', '\\'], "_");
    let ray_count = width as u64 * height as u64 * spp as u64;
    let manifest = format!(
        r#"{{
 "schema":"frontier-reference-v1", "source":"World::new", "source_version":{SOURCE_VERSION}, "seed":{seed}, "edit_revision":{}, "scene":"fixture",
 "git_revision":"{revision}", "git_dirty":{dirty}, "source_fnv1a64":"{hash:016x}", "canonical_source_fnv1a64":"{canonical_hash:016x}",
 "width":{width}, "height":{height}, "spp":{spp}, "threads":{threads}, "accelerated":{accelerate}, "voxel_size_m":{VOXEL_SIZE}, "lod":false,
 "eye":{:?}, "target":{:?}, "up":[0,1,0], "fov_y_degrees":{}, "near":{}, "far":{},
 "native_min_cell":{:?}, "native_size_cells":{:?}, "crop_policy":"canonical exposed faces only; no crop caps",
 "sampling":"deterministic jittered square stratification (SplitMix64)", "sample_seed":{sample_seed}, "pixel_filter":"box", "exposure":1.0, "ray_count":{ray_count}, "accumulation":"f64 linear then f32 storage", "png_transfer":"IEC 61966-2-1 sRGB",
 "lighting":{{"sun_direction":[-0.5,0.8,-0.3],"sun_color":[0.78,0.70,0.58],"ambient_ground":[0.12,0.105,0.085],"ambient_sky":[0.28,0.33,0.40],"sky_clear_linear":[0.34,0.57,0.80],"shadows":false,"fog":false}},
 "raw_format":"little-endian IEEE754 f32; top-left origin row-major interleaved",
 "files":{{"linear_rgb.f32":["R","G","B"],"coverage.f32":["hit fraction"],"depth.f32":["min distance m","max distance m","mean distance m","population variance m2"],"normals.f32":["-X","+X","-Y","+Y","-Z","+Z"],"materials.f32":["Grass","Soil","Stone","Wood"]}},
 "histogram_normalization":"counts/spp; sum equals coverage", "depth_conditioning":"hits only; NaN for no hits",
 "build_seconds":{build_seconds}, "render_seconds":{render_seconds}, "total_seconds":{}, "snapshot_bytes":{}, "peak_rss_bytes":{}, "cpu":"{cpu}"
}}
"#,
        world.revision(),
        camera.eye,
        camera.target,
        camera.fov_y_degrees,
        camera.near,
        camera.far,
        FIXTURE_MIN,
        FIXTURE_SIZE,
        started.elapsed().as_secs_f64(),
        scene.memory_bytes(),
        rss.map(|v| v.to_string()).unwrap_or_else(|| "null".into())
    );
    fs::write(output.join("manifest.json"), manifest)?;
    eprintln!("Done: {} ({render_seconds:.2}s render)", output.display());
    Ok(())
}
