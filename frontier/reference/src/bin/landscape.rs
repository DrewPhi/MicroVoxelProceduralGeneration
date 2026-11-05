use frontier_reference::{landscape::Landscape, shade, srgb, Camera, SKY};
use frontier_world::{FbmParams, FbmRecipe, FbmTerrain, VOXEL_SIZE};
use std::{
    error::Error,
    fs::{self, File},
    io::{BufWriter, Write},
    path::PathBuf,
    time::Instant,
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
fn vector(s: &str) -> Result<Vec<f64>> {
    Ok(s.split(',')
        .map(str::parse)
        .collect::<std::result::Result<Vec<_>, _>>()?)
}
fn hash(p: [u32; 3]) -> f32 {
    let mut h = p[0].wrapping_mul(1597334677)
        ^ p[1].wrapping_mul(3812015801)
        ^ p[2].wrapping_mul(2798796415);
    h = (h ^ (h >> 16)).wrapping_mul(2246822519);
    h = (h ^ (h >> 13)).wrapping_mul(3266489917);
    ((h ^ (h >> 16)) & 65535) as f32 / 65535.
}
fn field(cell: [u32; 3], shift: u32) -> f32 {
    let mask = u32::MAX >> shift;
    let q = [cell[0] >> shift, cell[2] >> shift];
    let t = [cell[0], cell[2]].map(|v| {
        let f = ((v & ((1 << shift) - 1)) as f32 + 0.5) / (1 << shift) as f32;
        f * f * (3. - 2. * f)
    });
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    lerp(
        lerp(
            hash([q[0], 19, q[1]]),
            hash([(q[0] + 1) & mask, 19, q[1]]),
            t[0],
        ),
        lerp(
            hash([q[0], 19, (q[1] + 1) & mask]),
            hash([(q[0] + 1) & mask, 19, (q[1] + 1) & mask]),
            t[0],
        ),
        t[1],
    )
}
fn tint(cell: [i64; 3]) -> [f64; 3] {
    let cell = cell.map(|v| v as u32);
    let b = field(cell, 9) - 0.5;
    let l = field(cell, 6) - 0.5;
    [
        1. + b * 0.24 + l * 0.10,
        1. + b * 0.15 + l * 0.08,
        1. + b * 0.07 + l * 0.06,
    ]
    .map(|v| v as f64)
}
fn jitter(mut x: u64) -> f64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    ((x ^ (x >> 31)) >> 11) as f64 * (1. / 9007199254740992.)
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
fn main() -> Result<()> {
    let (mut width, mut height, mut spp, mut seed) = (1280usize, 720usize, 16usize, 7u64);
    let mut sample_seed = 19u64;
    let mut recipe = FbmRecipe::Regional;
    let mut center = [-1156., 3469.];
    let mut extent = 1024.;
    let mut output = PathBuf::from("landscape-reference");
    let mut cache = None;
    let mut camera = Camera {
        eye: [-1156., 600., 3469.],
        target: [-1156., 150., 3519.],
        ..Camera::default()
    };
    let mut threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(12);
    let mut build_only = false;
    let mut paint = true;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--help" {
            println!("landscape --output DIR --cache FILE --center X,Z --extent 1024 --eye X,Y,Z --target X,Y,Z --width 1280 --height 720 --spp 16 --seed 7 --threads N --fov-y 60 --near .025 --far 20000 [--build-only] [--no-tint]");
            return Ok(());
        }
        if a == "--build-only" {
            build_only = true;
            continue;
        }
        if a == "--no-tint" {
            paint = false;
            continue;
        }
        let v = args.next().ok_or("missing value")?;
        match a.as_str() {
            "--output" => output = v.into(),
            "--cache" => cache = Some(PathBuf::from(v)),
            "--center" => center = vector(&v)?.try_into().map_err(|_| "center needs X,Z")?,
            "--extent" => extent = v.parse()?,
            "--eye" => camera.eye = vector(&v)?.try_into().map_err(|_| "eye needs X,Y,Z")?,
            "--target" => {
                camera.target = vector(&v)?.try_into().map_err(|_| "target needs X,Y,Z")?
            }
            "--width" => width = v.parse()?,
            "--height" => height = v.parse()?,
            "--spp" => spp = v.parse()?,
            "--seed" => seed = v.parse()?,
            "--terrain-recipe" => {
                recipe = match v.as_str() {
                    "legacy" => FbmRecipe::Legacy,
                    "regional" => FbmRecipe::Regional,
                    _ => return Err("recipe must be legacy|regional".into()),
                }
            }
            "--sample-seed" => sample_seed = v.parse()?,
            "--threads" => threads = v.parse()?,
            "--fov-y" => camera.fov_y_degrees = v.parse()?,
            "--near" => camera.near = v.parse()?,
            "--far" => camera.far = v.parse()?,
            _ => return Err(format!("unknown option {a}").into()),
        }
    }
    let grid = (spp as f64).sqrt() as usize;
    let size = (extent / VOXEL_SIZE) as usize;
    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || grid * grid != spp
        || spp == 0
        || spp > 65536
        || extent != size as f64 * VOXEL_SIZE
        || !center
            .iter()
            .chain(camera.eye.iter())
            .chain(camera.target.iter())
            .chain([camera.near, camera.far, camera.fov_y_degrees, extent].iter())
            .all(|v| v.is_finite())
        || camera.near <= 0.
        || camera.far <= camera.near
        || camera.fov_y_degrees <= 0.
        || camera.fov_y_degrees >= 179.
        || !camera
            .basis()
            .0
            .iter()
            .chain(camera.basis().1.iter())
            .all(|v| v.is_finite())
    {
        return Err("invalid image, extent or camera".into());
    }
    let min = center.map(|v| (v / VOXEL_SIZE).floor() as i64 - size as i64 / 2);
    let terrain = FbmTerrain::new(
        seed,
        FbmParams {
            recipe,
            ..Default::default()
        },
    )?;
    let source_version = terrain.source_version();
    let appearance_version = terrain.appearance_version();
    let recipe_name = format!("{recipe:?}");
    let center_h = terrain.height_m(center[0], center[1]);
    let mut range = [f64::INFINITY, f64::NEG_INFINITY];
    for z in 0..=32 {
        for x in 0..=32 {
            let h = terrain.height_m(
                (min[0] as f64 + size as f64 * x as f64 / 32.) * VOXEL_SIZE,
                (min[1] as f64 + size as f64 * z as f64 / 32.) * VOXEL_SIZE,
            );
            range[0] = range[0].min(h);
            range[1] = range[1].max(h);
        }
    }
    eprintln!("Center height {center_h}; sampled crop height range {range:?}. Building {size}² exact columns on {threads} threads...");
    let start = Instant::now();
    let scene = Landscape::build(terrain, min, size, threads, cache.as_deref())?;
    let build_seconds = start.elapsed().as_secs_f64();
    eprintln!(
        "Native columns ready in {build_seconds:.2}s; {} bytes",
        scene.memory_bytes()
    );
    if build_only {
        return Ok(());
    }
    fs::create_dir_all(&output)?;
    let mut pixels = vec![[0f32; 5]; width * height];
    let rows = height.div_ceil(threads.min(height));
    let render = Instant::now();
    std::thread::scope(|scope| {
        for (part, out) in pixels.chunks_mut(rows * width).enumerate() {
            let scene = &scene;
            let camera = &camera;
            scope.spawn(move || {
                for (i, p) in out.iter_mut().enumerate() {
                    let x = i % width;
                    let y = part * rows + i / width;
                    let mut rgb = [0.; 3];
                    let mut covered = 0.;
                    let mut depth = 0.;
                    for sy in 0..grid {
                        for sx in 0..grid {
                            let key = sample_seed
                                ^ ((y * width + x) as u64).wrapping_mul(0x9e3779b97f4a7c15)
                                ^ ((sy * grid + sx) as u64).wrapping_mul(0xd1b54a32d192ed03);
                            let (d, n, f) = camera.ray(
                                x as f64 + (sx as f64 + jitter(key)) / grid as f64,
                                y as f64
                                    + (sy as f64 + jitter(key ^ 0xa0761d6478bd642f)) / grid as f64,
                                width,
                                height,
                            );
                            let c = if let Some(hit) = scene.ray(camera.eye, d, n, f) {
                                covered += 1.;
                                depth += hit.distance;
                                let c = shade(hit);
                                let tint = if paint { tint(hit.cell) } else { [1.; 3] };
                                std::array::from_fn(|a| c[a] * tint[a])
                            } else {
                                SKY
                            };
                            for a in 0..3 {
                                rgb[a] += c[a];
                            }
                        }
                    }
                    for a in 0..3 {
                        p[a] = (rgb[a] / spp as f64) as f32;
                    }
                    p[3] = (covered / spp as f64) as f32;
                    p[4] = if covered > 0. {
                        (depth / covered) as f32
                    } else {
                        f32::NAN
                    };
                }
                eprintln!(
                    "Rendered rows {}..{}",
                    part * rows,
                    (part * rows + rows).min(height)
                );
            });
        }
    });
    let render_seconds = render.elapsed().as_secs_f64();
    let data: Vec<u8> = pixels
        .iter()
        .flat_map(|p| p[..3].iter().map(|v| srgb(*v as f64)))
        .collect();
    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(output.join("color.png"))?),
        width as u32,
        height as u32,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    encoder.write_header()?.write_image_data(&data)?;
    for (name, lo, hi) in [("linear_rgb", 0, 3), ("coverage", 3, 4), ("depth", 4, 5)] {
        let mut w = BufWriter::new(File::create(output.join(format!("{name}.f32")))?);
        for p in &pixels {
            for v in &p[lo..hi] {
                w.write_all(&v.to_le_bytes())?;
            }
        }
        w.flush()?;
    }
    let git_revision = git(&["rev-parse", "HEAD"]);
    let git_dirty = !git(&["status", "--porcelain"]).is_empty();
    let source_fingerprint = [
        include_str!("landscape.rs"),
        include_str!("../landscape.rs"),
        include_str!("../lib.rs"),
        include_str!("../../../world/src/lib.rs"),
        include_str!("../../../world/src/terrain.rs"),
    ]
    .iter()
    .flat_map(|s| s.bytes())
    .fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    });
    let cache_identity = scene.cache_identity;
    let ray_count = width as u64 * height as u64 * spp as u64;
    let miss_fraction = 1. - pixels.iter().map(|p| p[3] as f64).sum::<f64>() / pixels.len() as f64;
    fs::write(
        output.join("manifest.json"),
        format!(
            r#"{{"schema":"frontier-landscape-reference-v1","source":"FbmTerrain CPU f64","seed":{seed},"params":{{"horizontal_scale":1800,"amplitude":1000,"octaves":8,"roughness":0.5}},"terrain_recipe":"{recipe_name}","source_version":{source_version},"appearance_version":{appearance_version},"voxel_size_m":{VOXEL_SIZE},"lod":false,"geometry":"all native columns; conservative exact-max rejection hierarchy","crop_policy":"exposed canonical faces only; no crop caps; unbounded downward solid columns","native_min_xz":{min:?},"native_size_xz":{size},"width":{width},"height":{height},"spp":{spp},"eye":{:?},"target":{:?},"fov_y_degrees":{},"near":{},"far":{},"fog":false,"shadows":false,"terrain_tint":{paint},"tint":"same renderer 32m/4m coherent paint; fine grain zero","sampling":"deterministic jittered square stratification SplitMix64; box pixel filter; f64 linear accumulation","sample_seed":{sample_seed},"ray_count":{ray_count},"git_revision":"{git_revision}","git_dirty":{git_dirty},"source_fnv1a64":"{source_fingerprint:016x}","cache_identity_fnv1a64":"{cache_identity:016x}","raw_format":"little endian f32 top-left row-major","depth":"mean hit distance in meters; NaN on miss","build_seconds":{build_seconds},"render_seconds":{render_seconds},"snapshot_bytes":{},"threads":{threads},"miss_fraction":{miss_fraction}}}"#,
            camera.eye,
            camera.target,
            camera.fov_y_degrees,
            camera.near,
            camera.far,
            scene.memory_bytes()
        ),
    )?;
    eprintln!("Wrote {} in {render_seconds:.2}s", output.display());
    Ok(())
}
