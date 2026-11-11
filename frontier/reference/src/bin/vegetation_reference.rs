//! Bounded offline meadow reference: exact native terrain plus all actual plant
//! cells. No display LOD, NAADF, enlarged tufts, billboards or runtime meshes.
use frontier_reference::{
    landscape::Landscape, normalized, srgb, vegetation::Vegetation, Camera, Hit,
};
use frontier_world::{FbmParams, FbmRecipe, World, VOXEL_SIZE};
use std::{
    error::Error,
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
fn vector(s: &str) -> Result<[f64; 3]> {
    Ok(s.split(',')
        .map(str::parse)
        .collect::<std::result::Result<Vec<f64>, _>>()?
        .try_into()
        .map_err(|_| "expected X,Y,Z")?)
}
fn png(path: PathBuf, w: usize, h: usize, data: &[u8]) -> Result<()> {
    let mut e = png::Encoder::new(BufWriter::new(File::create(path)?), w as u32, h as u32);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    e.write_header()?.write_image_data(data)?;
    Ok(())
}
struct PlantView {
    camera: Camera,
    forward: [f64; 3],
    right: [f64; 3],
    up: [f64; 3],
    tan_x: f64,
    tan_y: f64,
    shadow_margin: f64,
}
impl PlantView {
    fn new(camera: &Camera, width: usize, height: usize, shadow_margin: f64) -> Self {
        let (forward, right, up) = camera.basis();
        let tan_y = (camera.fov_y_degrees.to_radians() * 0.5).tan();
        Self {
            camera: camera.clone(),
            forward,
            right,
            up,
            tan_x: tan_y * width as f64 / height as f64,
            tan_y,
            shadow_margin,
        }
    }
    fn contains(&self, p: &frontier_world::vegetation::PlantRecord) -> bool {
        use frontier_world::vegetation::{Species, MAX_PLANT_HEIGHT_CELLS, MAX_PLANT_RADIUS_CELLS};
        let r = if p.species == Species::Rowan {
            MAX_PLANT_RADIUS_CELLS as f64 * VOXEL_SIZE
        } else {
            8. * VOXEL_SIZE
        };
        let h = MAX_PLANT_HEIGHT_CELLS as f64 * VOXEL_SIZE;
        let center = [
            p.root.x as f64 * VOXEL_SIZE,
            p.root.y as f64 * VOXEL_SIZE + h * 0.5,
            p.root.z as f64 * VOXEL_SIZE,
        ];
        // Include below-root sphere rasterization, cell extents, and every
        // potential occluder within the finite32m shadow-ray neighbourhood.
        let radius =
            (2. * r * r + (h * 0.5 + r).powi(2)).sqrt() + 2. * VOXEL_SIZE + self.shadow_margin;
        let delta: [f64; 3] = std::array::from_fn(|a| center[a] - self.camera.eye[a]);
        let dot = |v: [f64; 3]| (0..3).map(|a| delta[a] * v[a]).sum::<f64>();
        let z = dot(self.forward);
        let x = dot(self.right).abs();
        let y = dot(self.up).abs();
        z + radius >= self.camera.near
            && z - radius <= self.camera.far
            && x - z * self.tan_x <= radius * (1. + self.tan_x * self.tan_x).sqrt()
            && y - z * self.tan_y <= radius * (1. + self.tan_y * self.tan_y).sqrt()
    }
}
struct Scene {
    terrain: Landscape,
    plants: Vegetation,
}
impl Scene {
    fn ray(
        &self,
        o: [f64; 3],
        d: [f64; 3],
        near: f64,
        far: f64,
        vegetation: bool,
    ) -> Option<(Hit, [f32; 3])> {
        let ground = self.terrain.ray(o, d, near, far);
        if vegetation {
            if let Some(v) =
                self.plants
                    .ray(o, d, near, ground.as_ref().map_or(far, |h| h.distance))
            {
                if ground.as_ref().is_none_or(|g| v.hit.distance < g.distance) {
                    return Some((v.hit, v.color));
                }
            }
        }
        ground.map(|h| {
            let color = frontier_reference::material(h.material).color();
            (h, color)
        })
    }
    fn sample(
        &self,
        o: [f64; 3],
        d: [f64; 3],
        near: f64,
        far: f64,
        vegetation: bool,
        shadows: bool,
        legacy: bool,
    ) -> ([f64; 3], f64) {
        let Some((hit, color)) = self.ray(o, d, near, far, vegetation) else {
            if legacy {
                return (frontier_reference::SKY, 0.);
            }
            let h = (d[1] * 0.5 + 0.5).clamp(0., 1.);
            return (
                std::array::from_fn(|i| {
                    [0.63, 0.73, 0.80][i] * (1. - h) + [0.24, 0.46, 0.73][i] * h
                }),
                0.,
            );
        };
        let mut n = [0.; 3];
        n[hit.face / 2] = if hit.face % 2 == 0 { -1. } else { 1. };
        let sun = normalized([-0.5, 0.8, -0.3]);
        let cosine: f64 = n.iter().zip(sun).map(|(a, b)| a * b).sum();
        let p = std::array::from_fn(|i| o[i] + d[i] * hit.distance + n[i] * 0.00001);
        let visible =
            if shadows && cosine > 0. && self.ray(p, sun, 0.00001, 32., vegetation).is_some() {
                0.
            } else {
                1.
            };
        let leaf = hit.material == 5;
        let sky = 0.5 + 0.5 * n[1];
        if legacy {
            return (
                std::array::from_fn(|i| {
                    color[i] as f64
                        * ([0.12, 0.105, 0.085][i] * (1. - sky)
                            + [0.28, 0.33, 0.40][i] * sky
                            + [0.78, 0.70, 0.58][i] * cosine.max(0.))
                }),
                if hit.material >= 4 { 1. } else { 0. },
            );
        }
        let c = std::array::from_fn(|i| {
            color[i] as f64
                * ([0.17, 0.18, 0.16][i] * (1. - sky)
                    + [0.38, 0.44, 0.51][i] * sky
                    + [0.88, 0.80, 0.63][i] * cosine.max(0.) * visible
                    + if leaf { 0.08 } else { 0. })
        });
        (c, if hit.material >= 4 { 1. } else { 0. })
    }
}
fn jitter(mut x: u64) -> f64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    ((x ^ (x >> 31)) >> 11) as f64 / ((1u64 << 53) as f64)
}
fn main() -> Result<()> {
    let mut out = PathBuf::from("output/vegetation-reference");
    let (mut width, mut height, mut spp, mut threads) = (960usize, 540usize, 16usize, 8usize);
    let mut center = [-200., -220.];
    let mut extent = 512.;
    let mut eye = [-200., 10.7, -100.];
    let mut target = [-200., 25., -240.];
    let mut far = 200.;
    let mut plants = true;
    let mut shadows = true;
    let mut scan = false;
    let mut view_cull = false;
    let mut legacy = false;
    let mut terrain_cache: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--view-cull" => {
                view_cull = true;
                continue;
            }
            "--scan" => {
                scan = true;
                continue;
            }
            "--no-vegetation" => {
                plants = false;
                continue;
            }
            "--no-shadows" => {
                shadows = false;
                continue;
            }
            "--help" => {
                println!("vegetation_reference --output DIR --width 960 --height 540 --spp 16 --threads 8 --extent 512 --center X,Z --eye X,Y,Z --target X,Y,Z --far 200 [--no-vegetation] [--no-shadows] [--scan] [--view-cull] [--cache FILE] [--lighting reference|legacy]");
                return Ok(());
            }
            _ => {}
        }
        let v = args.next().ok_or("missing option value")?;
        match a.as_str() {
            "--output" => out = v.into(),
            "--cache" => terrain_cache = Some(v.into()),
            "--lighting" => {
                legacy = match v.as_str() {
                    "legacy" => true,
                    "reference" => false,
                    _ => return Err("lighting requires legacy|reference".into()),
                };
            }
            "--width" => width = v.parse()?,
            "--height" => height = v.parse()?,
            "--spp" => spp = v.parse()?,
            "--threads" => threads = v.parse()?,
            "--extent" => extent = v.parse()?,
            "--center" => {
                center = v
                    .split(',')
                    .map(str::parse)
                    .collect::<std::result::Result<Vec<f64>, _>>()?
                    .try_into()
                    .map_err(|_| "center needs X,Z")?
            }
            "--eye" => eye = vector(&v)?,
            "--target" => target = vector(&v)?,
            "--far" => far = v.parse()?,
            _ => return Err(format!("unknown argument {a}").into()),
        }
    }
    if legacy {
        shadows = false;
    }
    let lighting_name = if legacy {
        "matched legacy hemisphere and directional light; no shadows or atmosphere"
    } else {
        "reference hemisphere ambient plus directional sun; not matched game beauty"
    };
    let world = World::fbm(
        7,
        FbmParams {
            recipe: FbmRecipe::Alpine,
            ..Default::default()
        },
    )?
    .with_vegetation();
    if scan {
        eprintln!(
            "center height={}, standing eye_y={}",
            world.height_m(center[0], center[1]),
            world.height_m(center[0], center[1]) + 1.7
        );
        for dz in (-400..=400).step_by(100) {
            let z = center[1] + dz as f64;
            println!(
                "z={z}: {:?}",
                (-400..=400)
                    .step_by(100)
                    .map(|dx| {
                        let x = center[0] + dx as f64;
                        (x, (world.height_m(x, z) * 10.).round() / 10.)
                    })
                    .collect::<Vec<_>>()
            );
        }
        return Ok(());
    }
    let grid = (spp as f64).sqrt() as usize;
    let size = (extent / VOXEL_SIZE) as usize;
    if grid * grid != spp
        || spp == 0
        || threads == 0
        || width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || !size.is_power_of_two()
        || !(16..=32768).contains(&size)
        || !eye
            .into_iter()
            .chain(target)
            .chain(center)
            .chain([far, extent])
            .all(f64::is_finite)
        || far <= 0.
    {
        return Err(
            "invalid parameters; square spp, power-of-two native extent<=2048m required".into(),
        );
    }
    std::fs::create_dir_all(&out)?;
    let min = center.map(|v| (v / VOXEL_SIZE).floor() as i64 - size as i64 / 2);
    let max = min.map(|v| v + size as i64);
    let camera = Camera {
        eye,
        target,
        fov_y_degrees: 60.,
        near: 0.025,
        far,
    };
    // Fail rather than silently substitute missing vegetation at crop edges.
    for (x, y) in [
        (0., 0.),
        (width as f64, 0.),
        (0., height as f64),
        (width as f64, height as f64),
    ] {
        let (d, near, far) = camera.ray(x, y, width, height);
        for t in [near, far] {
            for (a, k) in [0, 2].into_iter().enumerate() {
                let p = eye[k] + d[k] * t;
                if p < min[a] as f64 * VOXEL_SIZE || p >= max[a] as f64 * VOXEL_SIZE {
                    return Err(
                        "camera footprint exits exact crop: enlarge extent or shift center".into(),
                    );
                }
            }
        }
    }
    let start = Instant::now();
    eprintln!("Building exact native terrain {size}x{size}...");
    let cache = terrain_cache.unwrap_or_else(|| out.join("terrain.bin"));
    let terrain = Landscape::build(
        world.fbm_terrain().unwrap().clone(),
        min,
        size,
        threads,
        Some(&cache),
    )?;
    let terrain_seconds = start.elapsed().as_secs_f64();
    // Shadow rays extend32m; enumerate an additional32m source halo for plants.
    eprintln!("Building actual native grass, edelweiss and Rowan...");
    let vegetation = if plants {
        let view = PlantView::new(&camera, width, height, if shadows { 32. } else { 0. });
        Vegetation::build_filtered(
            &world,
            min.map(|v| v - 512),
            max.map(|v| v + 512),
            threads,
            |p| !view_cull || view.contains(p),
        )
    } else {
        Vegetation::empty()
    };
    let plant_seconds = start.elapsed().as_secs_f64() - terrain_seconds;
    let counts = vegetation.counts;
    let voxels = vegetation.voxel_count;
    let payload = terrain.memory_bytes() + vegetation.memory_bytes();
    let scene = Scene {
        terrain,
        plants: vegetation,
    };
    let build_seconds = start.elapsed().as_secs_f64();
    eprintln!("Prepared {:?} plants, {voxels} plant voxels, {:.1}MiB payload in {build_seconds:.2}s; rendering {width}x{height}, {spp}spp...",counts,payload as f64/1048576.);
    let mut image = vec![[0f64; 4]; width * height];
    let rows = height.div_ceil(threads.min(height));
    let completed = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for (part, buffer) in image.chunks_mut(rows * width).enumerate() {
            let scene = &scene;
            let camera = &camera;
            let completed = &completed;
            scope.spawn(move || {
                for (i, pixel) in buffer.iter_mut().enumerate() {
                    let x = i % width;
                    let y = part * rows + i / width;
                    for sy in 0..grid {
                        for sx in 0..grid {
                            let key = ((y * width + x) as u64).wrapping_mul(0x9e3779b97f4a7c15)
                                ^ ((sy * grid + sx) as u64).wrapping_mul(0xd1b54a32d192ed03)
                                ^ 83;
                            let (d, n, f) = camera.ray(
                                x as f64 + (sx as f64 + jitter(key)) / grid as f64,
                                y as f64
                                    + (sy as f64 + jitter(key ^ 0xa0761d6478bd642f)) / grid as f64,
                                width,
                                height,
                            );
                            let (c, coverage) = scene.sample(eye, d, n, f, plants, shadows, legacy);
                            for a in 0..3 {
                                pixel[a] += c[a] / spp as f64;
                            }
                            pixel[3] += coverage / spp as f64;
                        }
                    }
                    if x + 1 == width {
                        let n = completed.fetch_add(1, Ordering::Relaxed) + 1;
                        if n % 32 == 0 {
                            eprintln!("rendered {n}/{height} rows");
                        }
                    }
                }
            });
        }
    });
    let render_seconds = start.elapsed().as_secs_f64() - build_seconds;
    let data: Vec<_> = image
        .iter()
        .flat_map(|p| p[..3].iter().map(|v| srgb(*v)))
        .collect();
    png(out.join("color.png"), width, height, &data)?;
    let coverage: Vec<_> = image.iter().flat_map(|p| [srgb(p[3]); 3]).collect();
    png(
        out.join("vegetation-coverage.png"),
        width,
        height,
        &coverage,
    )?;
    let mut raw = BufWriter::new(File::create(out.join("linear_rgb.f32"))?);
    for p in &image {
        for v in &p[..3] {
            raw.write_all(&(*v as f32).to_le_bytes())?;
        }
    }
    raw.flush()?;
    let peak = std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        * 1024;
    std::fs::write(out.join("manifest.json"),format!("{{\"schema\":\"native_vegetation_reference_v1\",\"seed\":7,\"terrain_recipe\":\"alpine\",\"native_voxel_m\":0.0625,\"width\":{width},\"height\":{height},\"spp\":{spp},\"sample_seed\":83,\"threads\":{threads},\"eye\":{eye:?},\"target\":{target:?},\"fov_y_degrees\":60,\"far\":{far},\"crop_min_cells\":{min:?},\"crop_max_cells\":{max:?},\"view_cull\":{view_cull},\"plant_counts_grass_flower_rowan\":{counts:?},\"plant_voxels\":{voxels},\"snapshot_payload_bytes\":{payload},\"peak_process_rss_bytes\":{peak},\"terrain_build_seconds\":{terrain_seconds},\"plant_build_seconds\":{plant_seconds},\"render_seconds\":{render_seconds},\"vegetation_primary\":{plants},\"shadows\":{shadows},\"shadow_distance_m\":32,\"lighting\":\"{lighting_name}\",\"geometry\":\"actual native terrain columns and native plant cells; no display LOD\",\"scope\":\"bounded exact primary visibility; optional32m local hard shadows, terrain shadows clipped to crop; not full-world lighting or real-time FPS benchmark\"}}\n"))?;
    eprintln!(
        "Finished: build {build_seconds:.2}s, render {render_seconds:.2}s, peak RSS {:.1}MiB -> {}",
        peak as f64 / 1048576.,
        out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn view_filter_keeps_all_sampled_visible_native_cells_and_shadow_occluders() {
        use frontier_world::{
            vegetation::{model_for, PlantRecord, Species},
            Cell,
        };
        let camera = Camera {
            eye: [0., 15., -30.],
            target: [0., 0., 80.],
            far: 450.,
            ..Default::default()
        };
        let view = PlantView::new(&camera, 1920, 1080, 32.);
        let (f, r, u) = camera.basis();
        let visible = |p: [f64; 3]| {
            let q: [f64; 3] = std::array::from_fn(|a| p[a] - camera.eye[a]);
            let dot = |v: [f64; 3]| (0..3).map(|a| q[a] * v[a]).sum::<f64>();
            let z = dot(f);
            z >= camera.near
                && z <= camera.far
                && dot(r).abs() <= z * view.tan_x
                && dot(u).abs() <= z * view.tan_y
        };
        let sun = normalized([-0.5, 0.8, -0.3]);
        let mut checked = 0;
        for species in [Species::Grass, Species::Edelweiss, Species::Rowan] {
            let mut p = PlantRecord {
                species,
                seed: 941,
                variant: 3,
                root: Cell::default(),
                rotation: 1,
            };
            let model = model_for(&p);
            for x in [-4000, -1000, 0, 1000, 4000] {
                for z in [-1000, 0, 1000, 4000, 8000] {
                    p.root = Cell { x, y: 0, z };
                    for voxel in model.voxels.iter().step_by(17) {
                        let c = p.world_cell(voxel.cell);
                        let point = [
                            c.x as f64 * VOXEL_SIZE,
                            c.y as f64 * VOXEL_SIZE,
                            c.z as f64 * VOXEL_SIZE,
                        ];
                        for distance in [0., 16., 32.] {
                            let receiver = std::array::from_fn(|a| point[a] - sun[a] * distance);
                            if visible(receiver) {
                                checked += 1;
                                assert!(view.contains(&p));
                            }
                        }
                    }
                }
            }
        }
        assert!(checked > 100);
    }
}
