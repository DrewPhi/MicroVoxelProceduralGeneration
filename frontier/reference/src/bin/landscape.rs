#[path = "../daylight.rs"]
mod daylight;
use frontier_reference::{
    landscape::{Landscape, LandscapeRays, MultiScaleLandscape, SparseLandscape},
    material, shade, srgb, Camera, Hit, SKY,
};
use frontier_world::{voxel_size, FbmParams, FbmRecipe, FbmTerrain};
use std::{
    error::Error,
    fs::{self, File},
    io::{BufWriter, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
/// Independent CPU implementation of the terrain lighting contract. Legacy is
/// the default, including its historical zero-grain landscape tint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum LightingProfile {
    #[default]
    Legacy,
    Regional,
    Daylight,
}
#[derive(Clone, Copy, Debug)]
struct Lighting {
    profile: LightingProfile,
    sun_elevation: f64,
    sun_azimuth: f64,
    overcast: f64,
}
impl Default for Lighting {
    fn default() -> Self {
        Self {
            profile: LightingProfile::Legacy,
            sun_elevation: 35.,
            sun_azimuth: 135.,
            overcast: 0.15,
        }
    }
}
impl Lighting {
    fn validate(self) -> Result<Self> {
        if !self.sun_elevation.is_finite()
            || !self.sun_azimuth.is_finite()
            || !self.overcast.is_finite()
            || !(-90. ..=90.).contains(&self.sun_elevation)
            || !(0. ..=1.).contains(&self.overcast)
        {
            return Err(
                "lighting needs finite sun angles, elevation in [-90,90], overcast in [0,1]".into(),
            );
        }
        Ok(self)
    }
    fn name(self) -> &'static str {
        match self.profile {
            LightingProfile::Legacy => "legacy",
            LightingProfile::Regional => "regional",
            LightingProfile::Daylight => "daylight",
        }
    }
    fn sun_direction(self) -> [f64; 3] {
        if self.profile == LightingProfile::Legacy {
            let length = (0.5f64 * 0.5 + 0.8 * 0.8 + 0.3 * 0.3).sqrt();
            return [-0.5 / length, 0.8 / length, -0.3 / length];
        }
        let elevation = self.sun_elevation.to_radians();
        let azimuth = self.sun_azimuth.to_radians();
        [
            azimuth.sin() * elevation.cos(),
            elevation.sin(),
            azimuth.cos() * elevation.cos(),
        ]
    }
    fn regional_response(self, normal: [f64; 3]) -> [f64; 3] {
        self.regional_response_visible(normal, 1.)
    }
    fn regional_response_visible(self, normal: [f64; 3], visibility: f64) -> [f64; 3] {
        let elevation = self.sun_elevation.to_radians();
        let azimuth = self.sun_azimuth.to_radians();
        let sun = [
            azimuth.sin() * elevation.cos(),
            elevation.sin(),
            azimuth.cos() * elevation.cos(),
        ];
        let cosine = normal
            .iter()
            .zip(sun)
            .map(|(n, s)| n * s)
            .sum::<f64>()
            .max(0.);
        let sky = 0.5 + 0.5 * normal[1];
        let low = [0.15, 0.14, 0.115];
        let high = [0.29, 0.36, 0.44];
        let direct = [0.83, 0.75, 0.60];
        std::array::from_fn(|a| {
            (low[a] + (high[a] - low[a]) * sky) * (1. + self.overcast * 0.3)
                + direct[a] * cosine * (1. - 0.72 * self.overcast) * visibility
        })
    }
    fn shade(self, hit: Hit) -> [f64; 3] {
        if self.profile == LightingProfile::Legacy {
            return shade(hit);
        }
        let mut normal = [0.; 3];
        normal[hit.face / 2] = if hit.face % 2 == 0 { -1. } else { 1. };
        let light = self.regional_response(normal);
        let color = material(hit.material).color();
        std::array::from_fn(|a| color[a] as f64 * light[a])
    }
    fn shade_visibility(self, hit: Hit, visibility: f64) -> [f64; 3] {
        if visibility == 1. {
            return self.shade(hit);
        }
        let mut normal = [0.; 3];
        normal[hit.face / 2] = if hit.face % 2 == 0 { -1. } else { 1. };
        let color = material(hit.material).color();
        if self.profile == LightingProfile::Regional {
            let light = self.regional_response_visible(normal, visibility);
            return std::array::from_fn(|a| color[a] as f64 * light[a]);
        }
        let lit = shade(hit);
        let cosine = normal
            .iter()
            .zip(self.sun_direction())
            .map(|(n, s)| n * s)
            .sum::<f64>()
            .max(0.);
        let direct = [0.78, 0.70, 0.58];
        std::array::from_fn(|a| lit[a] - color[a] as f64 * direct[a] * cosine * (1. - visibility))
    }
}

fn distance_grain(distance: f64) -> f32 {
    let t = ((distance - 12.) / 28.).clamp(0., 1.);
    (1. - t * t * (3. - 2. * t)) as f32
}
fn vector(s: &str) -> Result<Vec<f64>> {
    Ok(s.split(',')
        .map(str::parse)
        .collect::<std::result::Result<Vec<_>, _>>()?)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PixelCrop {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}
impl PixelCrop {
    fn parse(value: &str) -> Result<Self> {
        let values: Vec<usize> = value
            .split(',')
            .map(str::parse)
            .collect::<std::result::Result<_, _>>()?;
        let [x, y, width, height]: [usize; 4] = values
            .try_into()
            .map_err(|_| "crop needs X,Y,WIDTH,HEIGHT")?;
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }
    fn validate(self, width: usize, height: usize) -> Result<Self> {
        if self.width == 0
            || self.height == 0
            || self.x.checked_add(self.width).is_none_or(|x| x > width)
            || self.y.checked_add(self.height).is_none_or(|y| y > height)
        {
            return Err("pixel crop must be nonempty and inside full viewport".into());
        }
        Ok(self)
    }
    fn pixel(self, x: usize, y: usize) -> [usize; 2] {
        [self.x + x, self.y + y]
    }
}
// Neighbouring primary rays and their shadow rays reuse the same native pages.
fn pixel_tiles(width: usize, height: usize) -> impl Iterator<Item = (usize, usize)> {
    const TILE: usize = 16;
    (0..height).step_by(TILE).flat_map(move |top| {
        (0..width).step_by(TILE).flat_map(move |left| {
            (top..height.min(top + TILE))
                .flat_map(move |y| (left..width.min(left + TILE)).map(move |x| (x, y)))
        })
    })
}

fn sample_position(
    x: usize,
    y: usize,
    width: usize,
    grid: usize,
    sx: usize,
    sy: usize,
    seed: u64,
) -> [f64; 2] {
    let key = seed
        ^ ((y * width + x) as u64).wrapping_mul(0x9e3779b97f4a7c15)
        ^ ((sy * grid + sx) as u64).wrapping_mul(0xd1b54a32d192ed03);
    [
        x as f64 + (sx as f64 + jitter(key)) / grid as f64,
        y as f64 + (sy as f64 + jitter(key ^ 0xa0761d6478bd642f)) / grid as f64,
    ]
}
/// Conservative completeness diagnostic only; does not alter native traversal.
fn ray_within_native_footprint(
    min: [i64; 2],
    size: usize,
    eye: [f64; 3],
    d: [f64; 3],
    near: f64,
    far: f64,
) -> bool {
    [near, far].into_iter().all(|t| {
        [0, 2].into_iter().enumerate().all(|(i, axis)| {
            let position = eye[axis] + t * d[axis];
            let lo = min[i] as f64 * voxel_size();
            let hi = (min[i] as f64 + size as f64) * voxel_size();
            position >= lo && position < hi
        })
    })
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
fn shaded_tint(cell: [i64; 3], eye: [f64; 3], profile: LightingProfile) -> [f64; 3] {
    if profile == LightingProfile::Legacy {
        return tint(cell);
    }
    let distance = cell
        .iter()
        .zip(eye)
        .map(|(&c, e)| {
            let delta = (c as f64 + 0.5) * voxel_size() - e;
            delta * delta
        })
        .sum::<f64>()
        .sqrt();
    let grain = distance_grain(distance);
    if grain == 0. {
        return tint(cell);
    }
    let cell = cell.map(|v| v as u32);
    let broad = field(cell, 9) - 0.5;
    let local = field(cell, 6) - 0.5;
    let speckle = (hash(cell) - 0.5) * 0.055 * grain;
    [
        1. + broad * 0.24 + local * 0.10 + speckle,
        1. + broad * 0.15 + local * 0.08 + speckle,
        1. + broad * 0.07 + local * 0.06 + speckle,
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
    let mut pixel_crop = None;
    let mut recipe = FbmRecipe::Regional;
    let mut lighting = Lighting::default();
    let mut bank = 0f64;
    let mut center = [-1156., 3469.];
    let mut extent = None;
    let mut output = PathBuf::from("landscape-reference");
    let mut cache = None;
    let mut sample_stride = 1usize;
    let mut native_extent = 128f64;
    let mut plant_extent = 0f64;
    let mut shadows = false;
    let mut shadow_distance = 4096f64;
    let mut camera = Camera {
        eye: [-1156., 600., 3469.],
        target: [-1156., 150., 3519.],
        ..Camera::default()
    };
    let mut threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(12);
    let mut build_only = false;
    let mut sparse = false;
    let mut paint = true;
    let mut args = frontier_world::voxel_grid_args(std::env::args().skip(1))?;
    while let Some(a) = args.next() {
        if a == "--help" {
            println!("landscape --voxel-size METERS --output DIR --cache FILE --terrain-recipe legacy|regional|alpine|seeded-world|watershed|highlands --sample-stride 1 [--sparse] --native-extent 128 [--plant-extent METERS] [--shadows] --shadow-distance 4096 --lighting legacy|regional|daylight --sun-elevation 35 --sun-azimuth 135 --overcast .15 --center X,Z --extent 1024 --eye X,Y,Z --target X,Y,Z --width 1280 --height 720 --spp 16 --seed 7 --threads N --fov-y 60 --bank DEGREES --near .025 --far 20000 [--crop X,Y,WIDTH,HEIGHT] [--build-only] [--no-tint] (lighting defaults to legacy; daylight adds native water optics, sky visibility and aerial perspective)");
            return Ok(());
        }
        if a == "--shadows" {
            shadows = true;
            continue;
        }
        if a == "--sparse" {
            sparse = true;
            continue;
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
            "--sample-stride" => sample_stride = v.parse()?,
            "--plant-extent" => plant_extent = v.parse()?,
            "--native-extent" => native_extent = v.parse()?,
            "--shadow-distance" => shadow_distance = v.parse()?,
            "--center" => center = vector(&v)?.try_into().map_err(|_| "center needs X,Z")?,
            "--extent" => extent = Some(v.parse::<f64>()?),
            "--eye" => camera.eye = vector(&v)?.try_into().map_err(|_| "eye needs X,Y,Z")?,
            "--target" => {
                camera.target = vector(&v)?.try_into().map_err(|_| "target needs X,Y,Z")?
            }
            "--width" => width = v.parse()?,
            "--height" => height = v.parse()?,
            "--crop" => pixel_crop = Some(PixelCrop::parse(&v)?),
            "--spp" => spp = v.parse()?,
            "--seed" => seed = v.parse()?,
            "--terrain-recipe" => {
                recipe = match v.as_str() {
                    "legacy" => FbmRecipe::Legacy,
                    "regional" => FbmRecipe::Regional,
                    "alpine" => FbmRecipe::Alpine,
                    "seeded-world" | "world" => FbmRecipe::SeededWorld,
                    "highlands" => FbmRecipe::Highlands,
                    "watershed" => FbmRecipe::Watershed,
                    _ => return Err(
                        "recipe must be legacy|regional|alpine|seeded-world|watershed|highlands"
                            .into(),
                    ),
                }
            }
            "--sample-seed" => sample_seed = v.parse()?,
            "--lighting" => {
                lighting.profile = match v.as_str() {
                    "legacy" => LightingProfile::Legacy,
                    "regional" => LightingProfile::Regional,
                    "daylight" => LightingProfile::Daylight,
                    _ => return Err("lighting must be legacy|regional|daylight".into()),
                }
            }
            "--sun-elevation" => lighting.sun_elevation = v.parse()?,
            "--sun-azimuth" => lighting.sun_azimuth = v.parse()?,
            "--overcast" => lighting.overcast = v.parse()?,
            "--threads" => threads = v.parse()?,
            "--bank" => bank = v.parse()?,
            "--fov-y" => camera.fov_y_degrees = v.parse()?,
            "--near" => camera.near = v.parse()?,
            "--far" => camera.far = v.parse()?,
            _ => return Err(format!("unknown option {a}").into()),
        }
    }
    let lighting = lighting.validate()?;
    let grid = (spp as f64).sqrt() as usize;
    let extent = extent.unwrap_or_else(|| 16384. * voxel_size());
    let size = (extent / voxel_size()).round() as usize;
    if !plant_extent.is_finite()
        || !(0. ..=extent).contains(&plant_extent)
        || plant_extent > 0. && lighting.profile != LightingProfile::Daylight
    {
        return Err(
            "plant extent must fit the reference footprint and uses daylight lighting".into(),
        );
    }
    if !sample_stride.is_power_of_two()
        || sample_stride > 256
        || !native_extent.is_finite()
        || native_extent < 0.
        || !shadow_distance.is_finite()
        || shadow_distance <= 0.
        || width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || grid * grid != spp
        || spp == 0
        || spp > 65536
        || (extent - size as f64 * voxel_size()).abs() > extent.abs() * f64::EPSILON * 4.
        || !center
            .iter()
            .chain(camera.eye.iter())
            .chain(camera.target.iter())
            .chain([camera.near, camera.far, camera.fov_y_degrees, extent].iter())
            .all(|v| v.is_finite())
        || !bank.is_finite()
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
    let crop = pixel_crop
        .unwrap_or(PixelCrop {
            x: 0,
            y: 0,
            width,
            height,
        })
        .validate(width, height)?;
    let (viewport_width, viewport_height) = (width, height);
    let (width, height) = (crop.width, crop.height);
    let spacing = voxel_size() * sample_stride as f64;
    let sample_size = size / sample_stride;
    if sparse && (sample_stride != 1 || cache.is_some() || build_only) {
        return Err("sparse traversal builds native pages on demand; use stride 1 with no dense cache or build-only option".into());
    }
    if size % sample_stride != 0
        || !sample_size.is_power_of_two()
        || sample_size < 16
        || (!sparse && sample_size > 32768)
        || (sparse && !(64..=1 << 23).contains(&sample_size))
    {
        return Err("extent / sample spacing must be a power of two in 16..32768 columns".into());
    }
    let sample_min = center.map(|v| (v / spacing).floor() as i64 - sample_size as i64 / 2);
    let min = sample_min.map(|v| v * sample_stride as i64);
    let native_size = if sample_stride > 1 {
        (native_extent / voxel_size()) as usize
    } else {
        0
    };
    if sample_stride > 1
        && native_extent > 0.
        && (!native_size.is_power_of_two()
            || native_size < 16
            || native_size > 32768
            || native_extent != native_size as f64 * voxel_size()
            || native_extent >= extent)
    {
        return Err(
            "native extent must be a power of two native footprint, smaller than the full extent"
                .into(),
        );
    }
    let native_min = [camera.eye[0], camera.eye[2]]
        .map(|v| (v / voxel_size()).floor() as i64 - native_size as i64 / 2);
    if native_size > 0
        && (0..2).any(|a| {
            native_min[a] < min[a] || native_min[a] + native_size as i64 > min[a] + size as i64
        })
    {
        return Err("native footprint around eye must fit entirely inside far footprint".into());
    }
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
                (min[0] as f64 + size as f64 * x as f64 / 32.) * voxel_size(),
                (min[1] as f64 + size as f64 * z as f64 / 32.) * voxel_size(),
            );
            range[0] = range[0].min(h);
            range[1] = range[1].max(h);
        }
    }
    eprintln!("Center height {center_h}; sampled crop height range {range:?}. Building {sample_size}² columns at {spacing}m plus {native_size}² native columns on {threads} threads...");
    let start = Instant::now();
    let near = if native_size > 0 {
        Some(Landscape::build(
            terrain.clone(),
            native_min,
            native_size,
            threads,
            None,
        )?)
    } else {
        None
    };
    let (landscape, cache_identity, near_cache_identity): (Box<dyn LandscapeRays>, u64, u64) =
        if sparse {
            (
                Box::new(SparseLandscape::new(terrain.clone(), min, size)?),
                0,
                0,
            )
        } else {
            let dense = MultiScaleLandscape {
                far: Landscape::build_sampled(
                    terrain.clone(),
                    sample_min,
                    sample_size,
                    sample_stride,
                    threads,
                    cache.as_deref(),
                )?,
                near,
            };
            let ids = (
                dense.far.cache_identity,
                dense.near.as_ref().map_or(0, |s| s.cache_identity),
            );
            (Box::new(dense), ids.0, ids.1)
        };
    let plant_width = (plant_extent / voxel_size()).ceil() as i64;
    let plant_min = [0, 1].map(|a| min[a] + (size as i64 - plant_width) / 2);
    let plants = if plant_width > 0 {
        let world = frontier_world::World::fbm(seed, terrain.params())?.with_vegetation();
        frontier_reference::vegetation::Vegetation::build(
            &world,
            plant_min,
            plant_min.map(|v| v + plant_width),
            threads,
        )
    } else {
        frontier_reference::vegetation::Vegetation::empty()
    };
    let scene = daylight::Scene {
        landscape: &*landscape,
        plants: &plants,
    };
    let plant_voxels = plants.voxel_count;
    let snapshot_bytes = landscape.memory_bytes() + plants.memory_bytes();
    let build_seconds = start.elapsed().as_secs_f64();
    eprintln!(
        "Native columns ready in {build_seconds:.2}s; {} bytes",
        snapshot_bytes
    );
    if build_only {
        return Ok(());
    }
    fs::create_dir_all(&output)?;
    let mut pixels = vec![[0f32; 6]; width * height];
    // Shallow water and grazing skyline rays can cost much more than the
    // foreground. Hand out short strips while retaining each worker's caches.
    let workers = threads.min(height);
    let rows = height.div_ceil(workers).clamp(1, 8);
    let strips = std::sync::Mutex::new(pixels.chunks_mut(rows * width).enumerate());
    let render = Instant::now();
    let boundary_misses = AtomicU64::new(0);
    let rays = camera.raster_rays(viewport_width, viewport_height, bank);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let strips = &strips;
            let scene = &scene;
            let camera = &camera;
            let rays = &rays;
            let boundary_misses = &boundary_misses;
            let terrain = &terrain;
            scope.spawn(move || {
                let mut daylight = (lighting.profile == LightingProfile::Daylight).then(|| {
                    daylight::Daylight::new(terrain, lighting.sun_direction(), lighting.overcast)
                });
                let mut incomplete = 0u64;
                loop {
                    let next = strips.lock().unwrap().next();
                    let Some((part, out)) = next else { break };
                    for (ix, iy) in pixel_tiles(width, out.len() / width) {
                        let p = &mut out[iy * width + ix];
                        let [x, y] = crop.pixel(ix, part * rows + iy);
                        let mut rgb = [0.; 3];
                        let mut water_hits = 0u32;
                        let mut covered = 0.;
                        let mut depth = 0.;
                        for sy in 0..grid {
                            for sx in 0..grid {
                                let [px, py] = sample_position(
                                    x,
                                    y,
                                    viewport_width,
                                    grid,
                                    sx,
                                    sy,
                                    sample_seed,
                                );
                                let (d, n, f) = rays(px, py);
                                let c = if let Some((hit, plant)) = scene.ray(camera.eye, d, n, f) {
                                    covered += 1.;
                                    water_hits += u32::from(hit.material == 7);
                                    depth += hit.distance;
                                    let sample = ((y * viewport_width + x) as u64)
                                        .wrapping_mul(0x9e3779b97f4a7c15)
                                        ^ sample_seed
                                        ^ ((sy * grid + sx) as u64)
                                            .wrapping_mul(0xd1b54a32d192ed03);
                                    let sun = daylight.as_ref().map_or_else(
                                        || lighting.sun_direction(),
                                        |l| {
                                            l.sun_sample(
                                                jitter(sample ^ 0x9ab3ea6f),
                                                jitter(sample ^ 0x17312f6a),
                                            )
                                        },
                                    );
                                    let normal_sign = if hit.face % 2 == 0 { -1. } else { 1. };
                                    let visibility = if shadows
                                        && (sun[hit.face / 2] * normal_sign > 0.
                                            || (daylight.is_some() && hit.material == 7))
                                    {
                                        let mut origin = std::array::from_fn(|a| {
                                            camera.eye[a] + d[a] * hit.distance
                                        });
                                        origin[hit.face / 2] += normal_sign * 0.001;
                                        if scene.ray(origin, sun, 0.001, shadow_distance).is_some()
                                        {
                                            0.
                                        } else {
                                            1.
                                        }
                                    } else {
                                        1.
                                    };
                                    let c = match &mut daylight {
                                        Some(light) => light.shade(
                                            scene,
                                            hit,
                                            plant,
                                            camera.eye,
                                            d,
                                            visibility,
                                            [
                                                jitter(sample ^ 0x81b71dd5),
                                                jitter(sample ^ 0x72c28641),
                                            ],
                                            camera.far,
                                            if shadows { shadow_distance } else { 0. },
                                        ),
                                        None => lighting.shade_visibility(hit, visibility),
                                    };
                                    let tint = if paint
                                        && plant.is_none()
                                        && (daylight.is_none() || hit.material != 7)
                                    {
                                        shaded_tint(hit.cell, camera.eye, lighting.profile)
                                    } else {
                                        [1.; 3]
                                    };
                                    std::array::from_fn(|a| c[a] * tint[a])
                                } else {
                                    if !ray_within_native_footprint(min, size, camera.eye, d, n, f)
                                    {
                                        incomplete += 1;
                                    }
                                    daylight.as_ref().map_or(SKY, |light| light.sky(d))
                                };
                                for a in 0..3 {
                                    rgb[a] += c[a];
                                }
                            }
                        }
                        for a in 0..3 {
                            p[a] = (rgb[a] / spp as f64) as f32;
                        }
                        p[5] = water_hits as f32 / spp as f32;
                        p[3] = (covered / spp as f64) as f32;
                        p[4] = if covered > 0. {
                            (depth / covered) as f32
                        } else {
                            f32::NAN
                        };
                    }
                }
                boundary_misses.fetch_add(incomplete, Ordering::Relaxed);
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
    let water_output =
        (lighting.profile == LightingProfile::Daylight).then_some(("water_coverage", 5, 6));
    for (name, lo, hi) in [("linear_rgb", 0, 3), ("coverage", 3, 4), ("depth", 4, 5)]
        .into_iter()
        .chain(water_output)
    {
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
        include_str!("../sparse_landscape.rs"),
        include_str!("../daylight.rs"),
        include_str!("../vegetation.rs"),
        include_str!("../../../world/src/vegetation.rs"),
        include_str!("../../../world/src/seeded_world.rs"),
        include_str!("../lib.rs"),
        include_str!("../../../world/src/lib.rs"),
        include_str!("../../../world/src/terrain.rs"),
        include_str!("../../../world/src/alpine.rs"),
    ]
    .iter()
    .flat_map(|s| s.bytes())
    .fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    });
    let snapshot_bytes = landscape.memory_bytes() + plants.memory_bytes();
    let approximate = sample_stride > 1;
    let geometry = if sparse {
        "native columns on demand; conservative affine rejection and exact native page intersection"
    } else if approximate {
        "approximate sampled far columns plus disjoint exact native near columns; max hierarchy bounds sampled columns only, not canonical terrain"
    } else {
        "all native columns; conservative exact-max rejection hierarchy"
    };
    let crop_policy = if approximate {
        "native square owns ray segments inside its footprint; sampled columns own outside; no artificial crop caps; source features between samples are not certified"
    } else {
        "exposed canonical faces only; no crop caps; unbounded downward solid columns"
    };
    let ray_count = width as u64 * height as u64 * spp as u64;
    let miss_fraction = 1. - pixels.iter().map(|p| p[3] as f64).sum::<f64>() / pixels.len() as f64;
    let finite_domain_miss_samples = boundary_misses.load(Ordering::Relaxed);
    let finite_domain_miss_fraction = finite_domain_miss_samples as f64 / ray_count as f64;
    let pixel_crop = [crop.x, crop.y, crop.width, crop.height];
    let lighting_name = lighting.name();
    let sun_elevation = lighting.sun_elevation;
    let sun_azimuth = lighting.sun_azimuth;
    let overcast = lighting.overcast;
    let daylight = lighting.profile == LightingProfile::Daylight;
    let shadow_method = if daylight {
        "finite solar disk visibility per covered subpixel, 1mm face offset; occluders beyond the footprint or shadow distance omitted"
    } else {
        "one hard directional visibility ray per covered subpixel, 1mm face offset; same disjoint geometry; occluders beyond finite footprint or shadow distance omitted"
    };
    let light_model = if daylight {
        "source landscape pigment, native-face sun, cosine-weighted sky visibility, reflected terrain and native refracted bed rays"
    } else {
        "independent CPU f64 face lighting; canonical material palette"
    };
    let tint_description = match lighting.profile {
        LightingProfile::Legacy => "same renderer 32m/4m coherent paint; fine grain zero",
        LightingProfile::Daylight | LightingProfile::Regional => "32m/4m coherent paint plus cell hash grain fading over 12..40m from eye to native cell center; f64 distance, f32 paint",
    };
    let native_voxel_m = voxel_size();
    fs::write(
        output.join("manifest.json"),
        format!(
            r#"{{"schema":"frontier-landscape-reference-v1","source":"FbmTerrain CPU f64","seed":{seed},"params":{{"horizontal_scale":1800,"amplitude":1000,"octaves":8,"roughness":0.5}},"terrain_recipe":"{recipe_name}","source_version":{source_version},"appearance_version":{appearance_version},"voxel_size_m":{native_voxel_m},"lod":{approximate},"sparse_native_traversal":{sparse},"approximate_artistic_target":{approximate},"geometry":"{geometry}","crop_policy":"{crop_policy}","horizontal_sample_spacing_m":{spacing},"sample_stride_native_cells":{sample_stride},"sample_grid_min_xz":{sample_min:?},"sample_grid_size_xz":{sample_size},"exact_near_min_xz":{native_min:?},"exact_near_size_xz":{native_size},"near_cache_identity_fnv1a64":"{near_cache_identity:016x}","native_min_xz":{min:?},"native_size_xz":{size},"width":{width},"height":{height},"viewport_width":{viewport_width},"viewport_height":{viewport_height},"pixel_crop":{pixel_crop:?},"spp":{spp},"eye":{:?},"target":{:?},"fov_y_degrees":{},"roll_degrees":{bank},"near":{},"far":{},"fog":{daylight},"shadows":{shadows},"shadow_distance_m":{shadow_distance},"plant_extent_m":{plant_extent},"plant_min_xz":{plant_min:?},"plant_width_cells":{plant_width},"plant_voxels":{plant_voxels},"water_coverage_output":{daylight},"shadow_method":"{shadow_method}","terrain_tint":{paint},"tint":"{tint_description}","lighting":{{"profile":"{lighting_name}","contract_version":1,"implementation":"{light_model}","sun_elevation_degrees":{sun_elevation},"sun_azimuth_degrees":{sun_azimuth},"overcast":{overcast},"settings_apply_to":"regional and daylight profiles; legacy retains fixed normalized [-0.5,0.8,-0.3] sun and original hemisphere"}},"atmosphere":{daylight},"exposure":1,"postprocessed":false,"sampling":"deterministic jittered square stratification SplitMix64; box pixel filter; f64 linear accumulation","sample_seed":{sample_seed},"ray_count":{ray_count},"git_revision":"{git_revision}","git_dirty":{git_dirty},"source_fnv1a64":"{source_fingerprint:016x}","cache_identity_fnv1a64":"{cache_identity:016x}","raw_format":"little endian f32 top-left row-major","depth":"mean hit distance in meters; NaN on miss","build_seconds":{build_seconds},"render_seconds":{render_seconds},"snapshot_bytes":{},"threads":{threads},"miss_fraction":{miss_fraction},"finite_domain_miss_samples":{finite_domain_miss_samples},"finite_domain_miss_fraction":{finite_domain_miss_fraction},"miss_policy":"sky on all misses; missed rays leaving the finite native footprint are incomplete oracle coverage, never a geometry pass"}}"#,
            camera.eye,
            camera.target,
            camera.fov_y_degrees,
            camera.near,
            camera.far,
            snapshot_bytes
        ),
    )?;
    eprintln!("Wrote {} in {render_seconds:.2}s", output.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn close(actual: [f64; 3], expected: [f64; 3]) {
        for (a, e) in actual.into_iter().zip(expected) {
            assert!((a - e).abs() < 1e-12, "{actual:?} != {expected:?}");
        }
    }
    #[test]
    fn regional_sun_overhead_and_overcast_preserve_known_face_response() {
        let clear = Lighting {
            profile: LightingProfile::Regional,
            sun_elevation: 90.,
            sun_azimuth: 0.,
            overcast: 0.,
        };
        close(clear.regional_response([0., 1., 0.]), [1.12, 1.11, 1.04]);
        close(clear.regional_response([0., -1., 0.]), [0.15, 0.14, 0.115]);
        close(clear.regional_response([1., 0., 0.]), [0.22, 0.25, 0.2775]);
        let cloudy = Lighting {
            overcast: 1.,
            ..clear
        };
        close(
            cloudy.regional_response([0., 1., 0.]),
            [0.6094, 0.678, 0.74],
        );
        close(
            cloudy.regional_response([0., -1., 0.]),
            [0.195, 0.182, 0.1495],
        );
    }
    #[test]
    fn regional_sun_azimuth_rotates_illumination_without_changing_hemisphere() {
        let east = Lighting {
            profile: LightingProfile::Regional,
            sun_elevation: 0.,
            sun_azimuth: 90.,
            overcast: 0.,
        };
        close(east.regional_response([1., 0., 0.]), [1.05, 1., 0.8775]);
        close(east.regional_response([-1., 0., 0.]), [0.22, 0.25, 0.2775]);
        let north = Lighting {
            sun_azimuth: 0.,
            ..east
        };
        close(
            north.regional_response([0., 0., 1.]),
            east.regional_response([1., 0., 0.]),
        );
    }
    #[test]
    fn default_lighting_and_tint_retain_legacy_bits_for_every_face() {
        let legacy = Lighting::default();
        for face in 0..6 {
            for material in 1..=4 {
                let hit = Hit {
                    cell: [-17, 1503, 31],
                    distance: 1.,
                    face,
                    material,
                };
                assert_eq!(
                    legacy.shade(hit).map(f64::to_bits),
                    shade(hit).map(f64::to_bits)
                );
                assert_eq!(
                    shaded_tint(hit.cell, [0., 93.95, 0.], legacy.profile),
                    tint(hit.cell)
                );
            }
        }
    }
    #[test]
    fn regional_grain_uses_cell_center_distance_and_fades_to_existing_far_tint() {
        assert_eq!(distance_grain(0.), 1.);
        assert_eq!(distance_grain(12.), 1.);
        assert_eq!(distance_grain(26.), 0.5);
        assert_eq!(distance_grain(40.), 0.);
        assert_eq!(distance_grain(1000.), 0.);
        let cell = [-17, 1503, 31];
        let center = cell.map(|c| (c as f64 + 0.5) * voxel_size());
        let near = shaded_tint(cell, center, LightingProfile::Regional);
        let far = shaded_tint(
            cell,
            [center[0] + 40., center[1], center[2]],
            LightingProfile::Regional,
        );
        assert_eq!(far, tint(cell));
        assert_ne!(near, far);
        let delta = near[0] - far[0];
        assert!((delta - (near[1] - far[1])).abs() < 2e-7);
        assert!((delta - (near[2] - far[2])).abs() < 2e-7);
        assert!(delta.abs() <= 0.0275);
    }
    #[test]
    fn invalid_lighting_settings_fail_before_building_native_columns() {
        assert!(Lighting::default().validate().is_ok());
        for value in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            assert!(Lighting {
                overcast: value,
                ..Lighting::default()
            }
            .validate()
            .is_err());
        }
        assert!(Lighting {
            sun_elevation: 91.,
            ..Lighting::default()
        }
        .validate()
        .is_err());
        assert!(Lighting {
            sun_azimuth: f64::NAN,
            ..Lighting::default()
        }
        .validate()
        .is_err());
    }
    #[test]
    fn cropped_samples_keep_full_viewport_rays_and_sample_indices() {
        let (width, height, grid, seed) = (1920, 1080, 8, 19);
        let camera = Camera::default();
        let full = PixelCrop {
            x: 0,
            y: 0,
            width,
            height,
        };
        for crop in [
            full,
            PixelCrop {
                x: 928,
                y: 508,
                width: 64,
                height: 64,
            },
        ] {
            for local in [
                [0, 0],
                [crop.width - 1, crop.height - 1],
                [crop.width / 2, crop.height / 2],
            ] {
                let [x, y] = crop.pixel(local[0], local[1]);
                assert_eq!(full.pixel(x, y), [x, y]);
                for sy in 0..grid {
                    for sx in 0..grid {
                        let pixel_index = y * width + x;
                        let sample_index = sy * grid + sx;
                        let key = seed
                            ^ (pixel_index as u64).wrapping_mul(0x9e3779b97f4a7c15)
                            ^ (sample_index as u64).wrapping_mul(0xd1b54a32d192ed03);
                        let expected = [
                            x as f64 + (sx as f64 + jitter(key)) / grid as f64,
                            y as f64 + (sy as f64 + jitter(key ^ 0xa0761d6478bd642f)) / grid as f64,
                        ];
                        let actual = sample_position(x, y, width, grid, sx, sy, seed);
                        assert_eq!(actual.map(f64::to_bits), expected.map(f64::to_bits));
                        assert_eq!(
                            camera.ray(actual[0], actual[1], width, height),
                            camera.ray(expected[0], expected[1], width, height)
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn malformed_or_out_of_viewport_pixel_crops_are_rejected() {
        for value in ["1,2,3", "1,2,3,4,5", "-1,0,1,1", "x,0,1,1"] {
            assert!(PixelCrop::parse(value).is_err());
        }
        for value in [
            "0,0,0,1", "0,0,1,0", "8,0,1,1", "0,4,1,1", "7,0,2,1", "0,3,1,2",
        ] {
            assert!(PixelCrop::parse(value).unwrap().validate(8, 4).is_err());
        }
        assert!(PixelCrop {
            x: usize::MAX,
            y: 0,
            width: 1,
            height: 1
        }
        .validate(8, 4)
        .is_err());
        assert!(PixelCrop::parse("0,0,8,4").unwrap().validate(8, 4).is_ok());
    }
    #[test]
    fn finite_domain_misses_are_marked_incomplete_without_changing_traversal() {
        let inside = [0., 10., 0.];
        assert!(ray_within_native_footprint(
            [-16, -16],
            32,
            inside,
            [0., -1., 0.],
            0.025,
            20.
        ));
        assert!(!ray_within_native_footprint(
            [-16, -16],
            32,
            inside,
            [0., 0., -1.],
            0.025,
            20.
        ));
        assert!(!ray_within_native_footprint(
            [-16, -16],
            32,
            [0., 10., 2.],
            [0., 0., -1.],
            0.025,
            2.5
        ));
    }
    #[test]
    fn hard_shadows_remove_direct_light_and_preserve_hemisphere() {
        let regional = Lighting {
            profile: LightingProfile::Regional,
            sun_elevation: 90.,
            sun_azimuth: 0.,
            overcast: 0.,
        };
        close(
            regional.regional_response_visible([0., 1., 0.], 0.),
            [0.29, 0.36, 0.44],
        );
        for profile in [LightingProfile::Legacy, LightingProfile::Regional] {
            let lighting = Lighting {
                profile,
                ..regional
            };
            for face in 0..6 {
                let hit = Hit {
                    cell: [0; 3],
                    distance: 1.,
                    face,
                    material: 1,
                };
                assert_eq!(lighting.shade_visibility(hit, 1.), lighting.shade(hit));
                let shaded = lighting.shade_visibility(hit, 0.);
                let lit = lighting.shade(hit);
                for a in 0..3 {
                    assert!(shaded[a] >= 0. && shaded[a] <= lit[a] + 1e-12);
                }
            }
        }
    }
}
