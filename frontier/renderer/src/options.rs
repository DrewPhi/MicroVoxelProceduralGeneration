//! Command-line configuration for the three explicit comparison scenes.
use super::{Result, HEIGHT, SEED, WIDTH};
use std::path::PathBuf;
pub struct Options {
    pub bowl_save: Option<PathBuf>,
    pub pond_test: bool,
    pub live_water: bool,
    pub discrete_water: bool,
    pub procedural_water: bool,
    pub moving_water: bool,
    pub water_river_rate: Option<u64>,
    pub player_edit: bool,
    pub hand: frontier_world::edit::HandSettings,
    pub inventory_capacity: u64,
    pub water_active_radius: u32,
    pub water_history_cells: usize,
    pub water_save: Option<PathBuf>,
    pub water_demo: bool,
    pub atmosphere: Option<bool>,
    pub visual: crate::settings::VisualSettings,
    pub render_scale: f32,
    pub capture: Option<PathBuf>,
    pub capture_on_exit: Option<PathBuf>,
    pub seconds: Option<f64>,
    pub width: u32,
    pub height: u32,
    pub frame_limit: Option<u64>,
    pub scene: String,
    pub camera_preset: String,
    pub seed: u64,
    pub start_xz: [f64; 2],
    pub params: frontier_world::FbmParams,
    pub route: frontier_runtime::routes::Route,
    pub uncapped: bool,
    pub warmup: u64,
    pub report_path: Option<PathBuf>,
    pub route_step: f64,
    pub route_speed: Option<f64>,
    pub route_realtime: bool,
    pub terrain_static: bool,
    pub view_levels: u32,
    pub page_spacing: u32,
    pub msaa: u32,
    pub capture_route_time: Option<f64>,
    pub capture_coverage_shift: f64,
    pub voxel_filter: bool,
    pub renderer: String,
    pub surface_layout: String,
    pub teleport_test: bool,
    pub edit_test: bool,
    pub eye: Option<[f64; 3]>,
    pub target: Option<[f64; 3]>,
    pub fov_y: f32,
    pub capture_linear: bool,
    pub capture_far: Option<f32>,
    pub pixel_error: f64,
    pub capture_unresolved: bool,
    pub adaptive_wait_quality: bool,
    pub adaptive_cache_mib: usize,
    pub adaptive_workers: usize,
    pub adaptive_refinements: usize,
    pub adaptive_direct_native: bool,
    pub adaptive_build_mib: usize,
    pub adaptive_disk_cache: Option<std::path::PathBuf>,
    pub adaptive_disk_mib: usize,
    pub benchmark_input_lock: bool,
    pub native_bins: bool,
}
pub fn parse() -> Result<Option<Options>> {
    parse_args(std::env::args().skip(1))
}

/// Consume a boolean switch while preserving the flag-specific diagnostic.
fn parse_on_off(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<bool> {
    match args.next().as_deref() {
        Some("on") => Ok(true),
        Some("off") => Ok(false),
        _ => Err(format!("{flag} requires on|off").into()),
    }
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Option<Options>> {
    use crate::settings::ResourceProfile;
    let mut bowl_save = None;
    let mut pond_test = false;
    let mut live_water = false;
    let mut discrete_water = false;
    let mut procedural_water = false;
    let mut moving_water = false;
    let mut water_river_rate = None;
    let mut player_edit = None;
    let mut hand = frontier_world::edit::HandSettings::default();
    let mut inventory_capacity = 64u64;
    let mut water_active_radius = 1u32;
    let mut water_history_cells = 1_048_576usize;
    let mut water_save = None;
    let mut water_demo = false;
    let mut explicit_naadf = false;
    let mut explicit_msaa = false;
    let mut seeded_preset = false;
    let mut explicit_start = false;
    let mut resource_profile = None;
    let mut resource_overrides: Vec<(String, usize)> = Vec::new();
    let mut native_radius = None;
    let mut vegetation_near = None;
    let mut atmosphere = None;
    let mut visual = crate::settings::VisualSettings::default();
    let mut render_scale = 1.0f32;
    let mut capture = None;
    let mut capture_on_exit = None;
    let mut seconds: Option<f64> = None;
    let mut width = WIDTH;
    let mut height = HEIGHT;
    let mut frame_limit = None;
    let mut scene = String::from("adaptive");
    let mut camera_preset = String::from("valley");
    let mut seed = SEED;
    let mut params = frontier_world::FbmParams {
        recipe: frontier_world::FbmRecipe::Regional,
        ..Default::default()
    };
    let mut start_xz = [0f64; 2];
    let mut route = frontier_runtime::routes::Route::Stationary;
    let mut uncapped = false;
    let mut warmup = 0;
    let mut report_path = None;
    let mut route_step: f64 = 1. / 60.;
    let mut route_speed: Option<f64> = None;
    let mut route_realtime = false;
    let mut terrain_static = false;
    let mut view_levels = 7;
    let mut page_spacing = 1u32;
    let mut msaa = 4;
    let mut capture_route_time: Option<f64> = None;
    let mut capture_coverage_shift: f64 = 0.;
    // User selected the appearance transfer as the preferred default.
    let mut voxel_filter = true;
    let mut renderer = String::from("mesh");
    let mut surface_layout = String::from("clipmap");
    let mut teleport_test = false;
    let mut edit_test = false;
    let mut eye = None;
    let mut target = None;
    let mut fov_y = 60f32;
    let mut capture_linear = false;
    let mut capture_far: Option<f32> = None;
    let mut pixel_error = 1.0f64;
    let mut capture_unresolved = false;
    let mut adaptive_wait_quality = false;
    let mut adaptive_cache_mib = 512usize;
    let mut adaptive_workers = std::thread::available_parallelism().map_or(1, |n| n.get().min(4));
    let mut adaptive_build_mib: usize;
    let mut adaptive_refinements = 8usize;
    let mut adaptive_direct_native = false;
    let mut adaptive_disk_cache = None;
    let mut adaptive_disk_mib = 2048usize;
    let mut benchmark_input_lock = false;
    let mut native_bins = true;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--water-model" => {
                match args.next().as_deref() {
                    Some("ca") => {
                        discrete_water = false;
                        procedural_water = false;
                        moving_water = false;
                    }
                    Some("discrete") => {
                        discrete_water = true;
                        procedural_water = false;
                        moving_water = false;
                    }
                    Some("procedural") => {
                        discrete_water = false;
                        procedural_water = true;
                        moving_water = false;
                    }
                    Some("moving") => {
                        discrete_water = false;
                        procedural_water = false;
                        moving_water = true;
                    }
                    _ => return Err("--water-model requires ca|discrete|procedural|moving".into()),
                }
                live_water = true;
                if discrete_water || procedural_water || moving_water {
                    water_demo = true;
                }
            }
            "--water-river-rate" => {
                let rate: u64 = args.next()
                    .ok_or("--water-river-rate requires an integer native-cell rate per second")?
                    .parse()?;
                if rate > 1_000_000 {
                    return Err("--water-river-rate requires 0..1000000 native cells per second".into());
                }
                water_river_rate = Some(rate);
            }
            "--water-active-radius" => {
                water_active_radius = args.next()
                    .ok_or("water active radius requires 1..2 metres")?.parse()?;
                if !(1..=2).contains(&water_active_radius) {
                    return Err("--water-active-radius requires 1..2 metres".into());
                }
            }
            "--water-history-cells" => {
                water_history_cells = args.next()
                    .ok_or("water history requires cell count")?.parse()?;
                if !(32768..=16_777_216).contains(&water_history_cells) {
                    return Err("--water-history-cells requires 32768..16777216".into());
                }
            }
            "--player-edit" => player_edit=Some(parse_on_off(&mut args,&arg)?),
            "--pickup-reach" => hand.reach_m=args.next().ok_or("pickup reach requires metres")?.parse()?,
            "--hand-volume-liters" => hand.volume_liters=args.next().ok_or("hand volume requires litres")?.parse()?,
            "--hand-extent" => hand.extent_m=args.next().ok_or("hand extent requires metres")?.parse()?,
            "--pickup-quick-ms" => hand.quick_seconds=args.next().ok_or("quick threshold requires milliseconds")?.parse::<f64>()?/1000.,
            "--pickup-full-ms" => hand.full_seconds=args.next().ok_or("full charge requires milliseconds")?.parse::<f64>()?/1000.,
            "--inventory-capacity" => inventory_capacity=args.next().ok_or("inventory requires voxel capacity")?.parse()?,
            "--live-water" => live_water = parse_on_off(&mut args, &arg)?,
            "--water-save" => {
                water_save = Some(PathBuf::from(args.next().ok_or("--water-save requires PATH")?));
            }
            "--bowl-save" => bowl_save=Some(PathBuf::from(args.next().ok_or("--bowl-save requires a path")?)),
            "--pond-test" => pond_test = true,
            "--water-demo" => {
                live_water = true;
                water_demo = true;
            }
            "--visibility-benchmark" => visual.visibility_benchmark = true,
            "--naadf" => {
                explicit_naadf = true;
                visual.naadf_enabled = parse_on_off(&mut args, &arg)?;
            }
            "--naadf-gpu-distance" => visual.naadf_gpu_distance = parse_on_off(&mut args, &arg)?,
            "--naadf-follow" => visual.naadf_follow = parse_on_off(&mut args, &arg)?,
            "--naadf-radius" => {
                explicit_naadf = true;
                let radius: u32 = args.next().ok_or("NAADF radius requires metres")?.parse()?;
                if !(1..=256).contains(&radius) {
                    return Err("--naadf-radius requires integer 1..256 metres".into());
                }
                visual.naadf_radius_m = radius;
                visual.naadf_enabled = true;
            }
            "--naadf-budget-mib" => {
                let budget: usize = args.next().ok_or("NAADF budget requires MiB")?.parse()?;
                if !(16..=16384).contains(&budget) {
                    return Err("--naadf-budget-mib requires 16..16384".into());
                }
                visual.naadf_budget_mib = budget;
                visual.naadf_auto_memory = false;
            }
            "--resource-profile" => {
                resource_profile = Some(match args.next().as_deref() {
                    Some("development") => ResourceProfile::Development,
                    Some("constrained") => ResourceProfile::Constrained,
                    Some("custom") => ResourceProfile::Custom,
                    _ => return Err("--resource-profile requires development|constrained|custom".into()),
                });
            }
            "--native-radius" | "--vegetation-near" => {
                let value: f64 = args.next().ok_or("detail distance requires meters")?.parse()?;
                if !value.is_finite() || !(0.0..=128.0).contains(&value) { return Err("detail distance requires finite 0..128 meters".into()); }
                if arg == "--native-radius" { native_radius = Some(value); } else { vegetation_near = Some(value); }
            }
            "--terrain-cache-mib" | "--build-memory-mib" | "--terrain-gpu-mib"
            | "--plant-cpu-mib" | "--plant-gpu-mib" | "--triangle-budget" | "--upload-mib" => {
                let value: usize = args.next().ok_or("resource flag requires an integer")?.parse()?;
                if value == 0 || value.checked_mul(1024*1024).is_none() { return Err("resource flag requires a positive representable value".into()); }
                if arg == "--upload-mib" && value > 64 { return Err("--upload-mib requires 1..64".into()); }
                resource_overrides.push((arg, value));
            }
            "--preset" => {
                let preset = args.next().ok_or("--preset requires alpine|seeded-world|desert")?;
                seeded_preset = matches!(preset.as_str(), "seeded-world" | "world");
                if seeded_preset {
                    params = frontier_world::FbmParams {
                        recipe: frontier_world::FbmRecipe::SeededWorld,
                        ..Default::default()
                    };
                    scene = "adaptive".into();
                    camera_preset = "seeded-world".into();
                    if !explicit_start { start_xz = [0., 0.]; }
                    capture_far = Some(6000.);
                    view_levels = 11;
                    pixel_error = 5.;
                    adaptive_disk_cache = Some(PathBuf::from("output/cache/seeded-world-terrain"));
                    atmosphere = Some(true);
                    visual.sun_elevation = 28.;
                    visual.sun_azimuth = 22.;
                    visual.exposure = 1.20;
                    visual.fog_density = 0.000025;
                    visual.cloud_coverage = 0.24;
                    visual.cloud_speed = 0.;
                    visual.overcast = 0.08;
                    visual.terrain_shadows = true;
                    visual.beauty = true;
                    visual.vegetation = false;
                    visual.walking = true;
                    continue;
                }
                let (recipe, cache) = match preset.as_str() {
                    "alpine" => {
                        start_xz = [frontier_world::terrain::alpine_path_center_m(-250.), -250.];
                        (frontier_world::FbmRecipe::Alpine, "output/cache/alpine-terrain")
                    }
                    "desert" => (frontier_world::FbmRecipe::Desert, "output/cache/desert-v2"),
                    _ => return Err("--preset requires alpine|seeded-world|desert".into()),
                };
                params = frontier_world::FbmParams { recipe, ..Default::default() };
                scene = "adaptive".into();
                capture_far = Some(6000.);
                view_levels = 11;
                pixel_error = 3.;
                camera_preset = preset;
                adaptive_cache_mib = 640;
                adaptive_disk_cache = Some(PathBuf::from(cache));
                atmosphere = Some(true);
                visual.apply_polished_presentation();
                visual.vegetation = recipe == frontier_world::FbmRecipe::Alpine;
                visual.walking = true;
            }
            "--sun-elevation"
            | "--sun-azimuth"
            | "--exposure"
            | "--ground-bounce"
            | "--sunbeams"
            | "--streaming-haze"
            | "--fog-density"
            | "--fog-height-falloff"
            | "--cloud-coverage"
            | "--cloud-speed"
            | "--overcast" => {
                let value: f32 = args
                    .next()
                    .ok_or("visual flag requires a number")?
                    .parse()?;
                let (field, minimum, maximum) = match arg.as_str() {
                    "--sun-elevation" => (&mut visual.sun_elevation, -5., 85.),
                    "--sun-azimuth" => (&mut visual.sun_azimuth, 0., 360.),
                    "--exposure" => (&mut visual.exposure, 0., 8.),
                    "--ground-bounce" => (&mut visual.ground_bounce, 0., 2.),
                    "--sunbeams" => (&mut visual.sunbeams, 0., 2.),
                    "--streaming-haze" => (&mut visual.streaming_haze, 0., 1.),
                    "--fog-density" => (&mut visual.fog_density, 0., 0.01),
                    "--fog-height-falloff" => (&mut visual.fog_height_falloff, 0., 1.),
                    "--cloud-coverage" => (&mut visual.cloud_coverage, 0., 1.),
                    "--cloud-speed" => (&mut visual.cloud_speed, 0., 0.1),
                    _ => (&mut visual.overcast, 0., 1.),
                };
                if !value.is_finite() || !(minimum..=maximum).contains(&value) {
                    return Err(
                        format!("{arg} requires a finite number in {minimum}..={maximum}").into(),
                    );
                }
                *field = value;
            }
            "--vegetation" | "--walk" | "--beauty" | "--step-smoothing" => {
                let value = parse_on_off(&mut args, &arg)?;
                if arg == "--vegetation" {
                    visual.vegetation = value;
                } else if arg == "--step-smoothing" {
                    visual.step_smoothing = value;
                } else if arg == "--beauty" {
                    visual.beauty = value;
                } else {
                    visual.walking = value;
                }
            }
            "--terrain-shadows" => {
                visual.terrain_shadows = parse_on_off(&mut args, &arg)?;
            }
            "--atmosphere" => {
                atmosphere = Some(parse_on_off(&mut args, &arg)?);
            }
            "--render-scale" => {
                render_scale = args
                    .next()
                    .ok_or("--render-scale requires 0.5..1")?
                    .parse()?;
                if !render_scale.is_finite() || !(0.5..=1.0).contains(&render_scale) {
                    return Err("--render-scale requires 0.5..1".into());
                }
            }

            "--adaptive-build-mib" => {
                adaptive_build_mib = args
                    .next()
                    .ok_or("--adaptive-build-mib requires positive MiB")?
                    .parse::<usize>()?;
                if adaptive_build_mib == 0 || adaptive_build_mib.checked_mul(1024 * 1024).is_none()
                {
                    return Err("--adaptive-build-mib requires positive representable MiB".into());
                }
                resource_overrides.push((arg.clone(), adaptive_build_mib));
            }
            "--adaptive-direct-native" => {
                adaptive_direct_native = parse_on_off(&mut args, &arg)?;
            }
            "--adaptive-refinements" => {
                adaptive_refinements = args
                    .next()
                    .ok_or("--adaptive-refinements requires 1..32")?
                    .parse()?;
                if !(1..=32).contains(&adaptive_refinements) {
                    return Err("--adaptive-refinements requires 1..32".into());
                }
                resource_overrides.push((arg.clone(), adaptive_refinements));
            }
            "--adaptive-workers" => {
                adaptive_workers = args
                    .next()
                    .ok_or("--adaptive-workers requires 1..16")?
                    .parse()?;
                if !(1..=16).contains(&adaptive_workers) {
                    return Err("--adaptive-workers requires 1..16".into());
                }
                resource_overrides.push((arg.clone(), adaptive_workers));
            }
            "--no-adaptive-disk-cache" => {
                adaptive_disk_cache = None;
            }
            "--adaptive-disk-cache" => {
                let path = args
                    .next()
                    .ok_or("--adaptive-disk-cache requires a directory")?;
                if path.is_empty() {
                    return Err("disk cache directory cannot be empty".into());
                }
                adaptive_disk_cache = Some(std::path::PathBuf::from(path));
            }
            "--adaptive-disk-mib" => {
                adaptive_disk_mib = args
                    .next()
                    .ok_or("--adaptive-disk-mib requires positive MiB")?
                    .parse::<usize>()?;
                if adaptive_disk_mib == 0 || adaptive_disk_mib.checked_mul(1024 * 1024).is_none() {
                    return Err("--adaptive-disk-mib requires positive representable MiB".into());
                }
            }
            "--native-bins" => {
                native_bins = parse_on_off(&mut args, &arg)?
            }
            "--benchmark-input-lock" => benchmark_input_lock = true,
            "--adaptive-cache-mib" => {
                adaptive_cache_mib = args
                    .next()
                    .ok_or("--adaptive-cache-mib requires positive MiB")?
                    .parse()?;
                if adaptive_cache_mib == 0 || adaptive_cache_mib.checked_mul(1024 * 1024).is_none()
                {
                    return Err("--adaptive-cache-mib requires positive representable MiB".into());
                }
                resource_overrides.push((arg.clone(), adaptive_cache_mib));
            }
            "--capture-unresolved" => capture_unresolved = true,
            "--adaptive-wait-quality" => adaptive_wait_quality = true,
            "--pixel-error" => {
                pixel_error = args
                    .next()
                    .ok_or("--pixel-error requires finite pixels >=0")?
                    .parse()?;
                if !pixel_error.is_finite() || pixel_error < 0. {
                    return Err("--pixel-error requires finite pixels >=0".into());
                }
            }
            "--eye" | "--target" => {
                let value = args.next().ok_or("camera vector requires X,Y,Z")?;
                let values = value
                    .split(',')
                    .map(str::parse::<f64>)
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                if values.len() != 3 || values.iter().any(|v| !v.is_finite()) {
                    return Err("camera vector requires three finite coordinates".into());
                }
                let vector = [values[0], values[1], values[2]];
                if arg == "--eye" {
                    eye = Some(vector);
                } else {
                    target = Some(vector);
                }
            }
            "--capture-linear" => capture_linear = true,
            "--capture-far" => {
                capture_far = Some(
                    args.next()
                        .ok_or("--capture-far requires meters")?
                        .parse()?,
                )
            }
            "--fov" | "--fov-y" => {
                fov_y = args.next().ok_or("--fov-y requires degrees")?.parse()?
            }
            "--page-spacing" => {
                page_spacing = args
                    .next()
                    .ok_or("--page-spacing requires 1|2|4|8|16 meters")?
                    .parse()?;
            }
            "--scene" => {
                scene = args
                    .next()
                    .ok_or("--scene requires fixture|fbm|hybrid|hybrid-topology")?
            }
            "--camera" => {
                camera_preset = args
                    .next()
                    .ok_or("--camera requires valley|aerial|seeded-world|topology|cave|bridge")?
            }
            "--terrain-recipe" => {
                params.recipe =
                    match args.next().as_deref() {
                        Some("legacy") => frontier_world::FbmRecipe::Legacy,
                        Some("alpine") => frontier_world::FbmRecipe::Alpine,
                        Some("seeded-world" | "world") => frontier_world::FbmRecipe::SeededWorld,
                        Some("desert") => frontier_world::FbmRecipe::Desert,
                        Some("regional") => frontier_world::FbmRecipe::Regional,
                        Some("regional-unwarped") => frontier_world::FbmRecipe::RegionalUnwarped,
                        _ => return Err(
                            "--terrain-recipe requires legacy|regional|regional-unwarped|alpine|seeded-world"
                                .into(),
                        ),
                    }
            }
            "--start-xz" => {
                let value = args.next().ok_or("--start-xz requires X,Z meters")?;
                let (x, z) = value
                    .split_once(',')
                    .ok_or("--start-xz requires X,Z meters")?;
                start_xz = [x.parse()?, z.parse()?];
                explicit_start = true;
            }
            "--seed" => {
                let value = args.next().ok_or("--seed requires integer|random")?;
                seed = if value == "random" {
                    use std::io::Read;
                    let mut bytes = [0u8; 8];
                    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
                    let resolved = u64::from_ne_bytes(bytes);
                    eprintln!("Resolved random world seed: {resolved}");
                    resolved
                } else { value.parse()? };
            }
            "--scale" => {
                params.horizontal_scale = args.next().ok_or("--scale requires meters")?.parse()?
            }
            "--amplitude" => {
                params.amplitude = args.next().ok_or("--amplitude requires meters")?.parse()?
            }
            "--octaves" => {
                params.octaves = args.next().ok_or("--octaves requires integer")?.parse()?
            }
            "--roughness" => {
                params.roughness = args.next().ok_or("--roughness requires gain")?.parse()?
            }
            "--surface-layout" => {
                surface_layout = args
                    .next()
                    .ok_or("--surface-layout requires clipmap|quadtree")?
            }
            "--renderer" => {
                renderer = args
                    .next()
                    .ok_or("--renderer requires mesh|brick|brick-skip|brick-directional")?
            }
            "--teleport-test" => teleport_test = true,
            "--edit-test" => edit_test = true,
            "--uncapped" => uncapped = true,
            "--voxel-filter" => {
                voxel_filter = parse_on_off(&mut args, &arg)?
            }
            "--capture-coverage-shift" => {
                capture_coverage_shift = args
                    .next()
                    .ok_or("--capture-coverage-shift requires x meters")?
                    .parse()?;
            }
            "--capture-route-time" => {
                capture_route_time = Some(
                    args.next()
                        .ok_or("--capture-route-time requires seconds")?
                        .parse()?,
                )
            }
            "--msaa" => {explicit_msaa=true; msaa = args.next().ok_or("--msaa requires 1|4")?.parse()?;},
            "--terrain-static" => terrain_static = true,
            "--view-levels" => {
                view_levels = args.next().ok_or("--view-levels requires 7..11")?.parse()?
            }
            "--route-clock" => {
                route_realtime = match args.next().as_deref() {
                    Some("realtime") => true,
                    Some("frame") => false,
                    _ => return Err("--route-clock requires realtime|frame".into()),
                }
            }
            "--warmup" => warmup = args.next().ok_or("--warmup requires frames")?.parse()?,
            "--report" => {
                report_path = Some(PathBuf::from(
                    args.next().ok_or("--report requires JSON path")?,
                ))
            }
            "--route-speed" => {
                route_speed = Some(
                    args.next()
                        .ok_or("--route-speed requires meters per second")?
                        .parse()?,
                )
            }
            "--route-step" => {
                route_step = args
                    .next()
                    .ok_or("--route-step requires seconds per frame")?
                    .parse()?
            }
            "--route" => {
                route = match args.next().as_deref() {
                    Some("static") => frontier_runtime::routes::Route::Stationary,
                    Some("fly") => frontier_runtime::routes::Route::Fly,
                    Some("stop-resume") => frontier_runtime::routes::Route::StopResume,
                    Some("turn-return") => frontier_runtime::routes::Route::TurnReturn,
                    Some("walk-stop-resume") => frontier_runtime::routes::Route::WalkStopResume,
                    Some("walk-circuit") => frontier_runtime::routes::Route::WalkCircuit,
                    Some("walk-turn-return") => frontier_runtime::routes::Route::WalkTurnReturn,
                    _ => return Err("--route requires static|fly|stop-resume|turn-return|walk-stop-resume|walk-turn-return|walk-circuit".into()),
                }
            }
            "--seconds" => {
                seconds = Some(args.next().ok_or("--seconds requires duration")?.parse()?)
            }
            "--capture-on-exit" => {
                capture_on_exit = Some(PathBuf::from(
                    args.next().ok_or("--capture-on-exit requires PNG path")?,
                ))
            }
            "--capture" => {
                capture = Some(PathBuf::from(
                    args.next().ok_or("--capture requires a PNG path")?,
                ))
            }
            "--width" => width = args.next().ok_or("--width requires an integer")?.parse()?,
            "--height" => height = args.next().ok_or("--height requires an integer")?.parse()?,
            "--frames" => {
                frame_limit = Some(
                    args.next()
                        .ok_or("--frames requires an integer")?
                        .parse::<u64>()?
                        .max(1),
                )
            }
            "--help" | "-h" => {
                println!("NAADF experiment: --naadf on|off --naadf-follow on|off --naadf-gpu-distance on|off --naadf-radius 1..256 (integer metres, enables experiment) --naadf-budget-mib 16..16384. Exact region follows the player by default; --naadf-follow off freezes it. Initial visibility prototype, no NAADF GI.\nResource profiles: --resource-profile development|constrained|custom. Interactive default: development; finite comparisons retain constrained baseline. Overrides: --native-radius METERS --vegetation-near METERS --terrain-cache-mib N --build-memory-mib N --terrain-gpu-mib N --plant-cpu-mib N --plant-gpu-mib N --triangle-budget N --upload-mib 1..64. Existing --adaptive-workers and --adaptive-refinements set throughput. CPU and shared GPU working budgets leave RAM headroom; capacities allocate lazily.");
                println!("Player pickup defaults on with live water: --player-edit on|off. L press previews, release picks up; quick <150ms takes one voxel, charge to 900ms takes up to two. R places owned selected material; 1..6 selects Grass/Soil/Stone/Wood/Sand/Snow. --pickup-reach 2 --hand-volume-liters 0.5 --hand-extent 0.125 --pickup-quick-ms 150 --pickup-full-ms 900 --inventory-capacity 64. Preview outlines exact selected native cells; wet placement is rejected pending supported displacement. --player-edit off enables explicit creative/debug L remove/R Stone. Inventory, sequence, terrain and fluid share the save.");
                println!("Water models (opt-in): --water-model ca|discrete|procedural|moving.\n  ca: original local cellular automaton; --live-water on selects this path.\n  discrete: supplied whole-voxel waterfall/basin/outlet demo on a worker.\n  procedural: fixed 1 m generated river-bank tile with finite coarse neighbors and open boundaries.\n  moving: connected 2 m detailed window following the player, a 4 m local footprint and exact dormant history.\nMoving river flow: --water-river-rate 0..1000000 attempts native cells per simulated second at each eligible upstream/downstream continuation (default 128 for new runs; 0 selects finite water). A saved rate persists unless explicitly overridden. With positive input/output, downstream removal may lead accepted supply by at most 16 cells per channel. Blocked supply can stall accepted flow. Lakes remain finite.\nLive saves: --water-save PATH atomically saves terrain, inventory and model-specific water state on clean exit; reload with the same model/source/seed. Requires --scene adaptive, --naadf on, --naadf-follow on and --naadf-gpu-distance off.\nCA controls: --water-demo builds a two-chamber basin; G opens its dam early, P adds one counted water cell in creative mode. --water-active-radius 1|2 selects a 3 m|5 m active cube (default 1); --water-history-cells 32768..16777216 sets persistent capacity (default 1048576, at least the active cube). Unloaded CA borders are sealed.\nComparison renderer settings: --naadf-radius 8 --width 1920 --height 1080 selects an 8 m radius/16 m cube at 1080p. Ordinary NAADF radius defaults remain 64 m; water physics dimensions are independent. See LIVE_WATER.md for launch examples and experimental limits.");
                println!("Distant terrain defaults to coarse voxels with compact seed appearance in adaptive worlds. Set FRONTIER_DISTANT_TERRAIN=surface for the heightfield comparison or =voxy for the detailed-map voxel comparison. Set FRONTIER_VOXY_GPU_BATCH=1 to test optional GPU batching.");
                println!("Visual preview: --preset desert selects seeded dunes, rocky basins and sparse static oasis ponds with no vegetation; --preset alpine selects the Alpine meadow scene; --preset seeded-world (alias world) selects continuous seeded climate/biome terrain, static water, no vegetation, and a bounded dry walking spawn. --seed INTEGER|random resolves a reproducible seed (default 7). Later flags override preset values. --sun-elevation DEGREES --sun-azimuth DEGREES --exposure SCALE --ground-bounce 0..2 --sunbeams 0..2 --streaming-haze 0..1 --fog-density PER_M --fog-height-falloff PER_M --cloud-coverage 0..1 --cloud-speed 0..0.1 --overcast 0..1 --terrain-shadows on|off (bounded published terrain in current playable adaptive scenes). Alpine enables scene beauty, seeded vegetation and grounded walking; --beauty on|off toggles the lighting/atmosphere treatment; --vegetation on|off --walk on|off override them. F toggles walk/free flight; Space jumps while walking. Esc opens settings; --atmosphere on|off, --render-scale 0.5..1. Current Regional terrain is the default. Quality/resources apply together in the menu; atmosphere controls are live.\nAdaptive candidate: --scene adaptive|adaptive-topology --pixel-error 1 --adaptive-cache-mib 512 --adaptive-workers N (default up to four available CPU threads, 1..16 accepted, up to four active builds), --adaptive-build-mib 512 reservation budget. Selection batch: --adaptive-refinements 1..32 (default 8; independent of builder concurrency). Experimental nearest-leaf direct-native selection: --adaptive-direct-native on|off (default off). Optional derived disk cache: --adaptive-disk-cache PATH --adaptive-disk-mib 2048. Exact cave/bridge owner remains resident in adaptive-topology. Headless captures require settled quality; --capture-unresolved explicitly permits a coherent diagnostic image with failed quality marked in metadata. --adaptive-wait-quality starts benchmark clocks only after current coverage and the 0.8x internal error reserve settle (120s failure limit; reported quality still uses the requested bound); normal startup begins from coarse coverage. Adaptive window benchmarks accept --capture-far and explicit camera poses for matched clips.");
                println!("Prototype comparisons: --scene hybrid-topology --camera topology|cave|bridge; --renderer mesh|brick|brick-skip|brick-directional (fixture only); --surface-layout clipmap|quadtree (fbm only, quadtree freezes source pages); --teleport-test (hybrid only); --edit-test (fixture or hybrid-topology: add 1m cube at 2s, restore original materials 2s after publication); T teleports 512m or home. Single-sample captures also write .depth.f32. Explicit headless camera: --eye X,Y,Z --target X,Y,Z --fov-y 60 --msaa 1 --voxel-filter off --capture-linear (optional unclipped float32 render target); captures write .capture.json, .linear_rgb.f32 (raw float32 with --capture-linear, otherwise decoded 8-bit target), .coverage.f32 and .depth.f32.\nFrontier experimental FBM terrain (default) / exact native fixture\n--terrain-recipe regional|regional-unwarped|legacy|alpine|seeded-world|desert (default regional); --start-xz X,Z; voxel-filter on and MSAA4 default\n--scene pages|pages-topology|adaptive|adaptive-topology|fbm|fbm-native|hybrid|hybrid-static|fixture --pixel-error 1 --seed 7 --scale 1800 --amplitude 1000 --octaves 8 --roughness 0.5 --camera valley|aerial\n--route static|fly|stop-resume|turn-return --route-step 0.0166667 --route-speed MPS --route-clock frame|realtime --seconds N --capture-on-exit PATH --terrain-static --view-levels 7..11 --msaa 1|4 --voxel-filter on|off --capture-route-time SECONDS --capture-coverage-shift METERS (headless hybrid diagnostic only) --uncapped --warmup N --report PATH\nPages: bounded uniform reusable GPU grid baseline; --page-spacing 1|2|4|8|16 meters, 24x24 worldaligned32cell pages. Default far300*spacing meters, hard coverage344*spacing; native/collar and fixed volume owner retained; only1m spacing matches collar samples. No certified error or CDLOD claim.\nFBM: moving background-generated nested GPU grids, bounded ±2048m around published center, surface-only, no editing/native handoff. --terrain-static freezes the initial coverage for comparison. WASD 35m/s; Shift 80m/s.\nHybrid: moving 64m exact native heightfield chunks, approximate 2m collar, GPU proxy with coherent ownership publication; hybrid-topology adds a fixed exact cave/bridge owner and local edits. No collision or certified proxy error. hybrid-static preserves the fixed CPU reference. Default eye 1.7m above canonical top at origin.\nFrontier bounded native-voxel fixture\n--capture PATH [--width 1280 --height 720] or --frames N for a finite window run\nClick: capture mouse; WASD: free flight; Shift: fast; Space/Ctrl: vertical; Esc: release mouse; F12: PNG\nAim at screen center: left removes a cell, right places stone (8m reach); wait for each rebuild.\nThis reference has no streaming, distant LOD, collision or persistence.");
                return Ok(None);
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err("dimensions must be in 1..=8192".into());
    }
    if !route_step.is_finite() || route_step <= 0. {
        return Err("--route-step must be finite and positive".into());
    }
    if seconds.is_some_and(|s| !s.is_finite() || s <= 0.) {
        return Err("--seconds must be finite and positive".into());
    }
    if route_speed.is_some_and(|s| !s.is_finite() || s <= 0.) {
        return Err("--route-speed must be finite and positive".into());
    }
    if !(super::terrain::MIN_LEVELS..=super::terrain::MAX_LEVELS).contains(&view_levels) {
        return Err("--view-levels requires 7..11".into());
    }
    if !capture_coverage_shift.is_finite() || capture_coverage_shift.abs() > 24. {
        return Err("--capture-coverage-shift requires finite x meters within ±24".into());
    }
    if capture_coverage_shift != 0.
        && (capture.is_none()
            || !matches!(
                scene.as_str(),
                "hybrid" | "hybrid-topology" | "pages" | "pages-topology"
            ))
    {
        return Err(
            "--capture-coverage-shift is only for hybrid headless publication comparisons".into(),
        );
    }
    if capture_route_time.is_some_and(|t| !t.is_finite() || t < 0.) {
        return Err("--capture-route-time requires finite nonnegative seconds".into());
    }
    if ![1, 2, 4, 8, 16].contains(&page_spacing) {
        return Err("--page-spacing requires 1|2|4|8|16 meters".into());
    }
    if msaa != 1 && msaa != 4 {
        return Err("--msaa requires 1|4".into());
    }
    if !["mesh", "brick", "brick-skip", "brick-directional"].contains(&renderer.as_str()) {
        return Err("--renderer requires mesh|brick|brick-skip|brick-directional".into());
    }
    if !explicit_naadf {
        visual.naadf_enabled = scene == "adaptive";
    }
    if visual.naadf_enabled && !explicit_msaa {
        msaa = 1;
    }
    if visual.naadf_enabled && scene != "adaptive" {
        return Err("NAADF requires --scene adaptive (or --preset alpine/desert)".into());
    }
    if renderer != "mesh" && scene != "fixture" {
        return Err("brick comparison currently supports --scene fixture".into());
    }
    if edit_test
        && ![
            "fixture",
            "hybrid-topology",
            "adaptive-topology",
            "pages-topology",
        ]
        .contains(&scene.as_str())
    {
        return Err("--edit-test requires fixture or hybrid-topology scene".into());
    }
    if edit_test && capture.is_some() {
        return Err(
            "--edit-test requires a window run; use --capture-on-exit with --seconds".into(),
        );
    }
    if teleport_test
        && !["hybrid", "hybrid-topology", "pages", "pages-topology"].contains(&scene.as_str())
    {
        return Err("--teleport-test requires moving hybrid scene".into());
    }
    if !["clipmap", "quadtree"].contains(&surface_layout.as_str()) {
        return Err("--surface-layout requires clipmap|quadtree".into());
    }
    if surface_layout == "quadtree" {
        if scene != "fbm" {
            return Err("quadtree comparison requires --scene fbm".into());
        }
        terrain_static = true;
    }
    if start_xz.iter().any(|v| !v.is_finite() || v.abs() > 1e9) {
        return Err("--start-xz requires finite coordinates within ±1 billion meters".into());
    }
    if start_xz != [0., 0.]
        && !matches!(
            scene.as_str(),
            "fbm" | "hybrid" | "adaptive" | "adaptive-topology" | "pages" | "pages-topology"
        )
    {
        return Err("--start-xz supports fbm and hybrid scenes only".into());
    }
    if eye.is_some() != target.is_some() {
        return Err("--eye and --target must be provided together".into());
    }
    if let (Some(a), Some(b)) = (eye, target) {
        let d = glam::DVec3::from_array(b) - glam::DVec3::from_array(a);
        if !d.is_finite()
            || !d.length().is_finite()
            || d.length() < 1e-8
            || d.cross(glam::DVec3::Y).length() < 1e-8
        {
            return Err("camera direction must be nonzero and not parallel to Y".into());
        }
    }
    if !fov_y.is_finite() || !(1.0..179.0).contains(&fov_y) {
        return Err("--fov-y must be in 1..179 degrees".into());
    }
    // Finite, input-locked comparisons need the same explicit camera/clip as
    // the headless reference. Ordinary interactive terrain launches retain their
    // preset-camera validation.
    let fixed_terrain_benchmark = benchmark_input_lock
        && seconds.is_some()
        && capture_on_exit.is_some()
        && matches!(
            scene.as_str(),
            "pages" | "pages-topology" | "hybrid" | "hybrid-topology" | "fbm" | "fixture"
        );
    if eye.is_some()
        && scene != "fixture"
        && capture.is_none()
        && !scene.starts_with("adaptive")
        && !fixed_terrain_benchmark
    {
        return Err("explicit terrain camera requires --capture, or a finite --benchmark-input-lock --seconds --capture-on-exit comparison".into());
    }
    if capture_far.is_some_and(|v| !v.is_finite() || v <= 0.025)
        || (capture_far.is_some()
            && capture.is_none()
            && !scene.starts_with("adaptive")
            && !fixed_terrain_benchmark)
    {
        return Err("--capture-far requires finite distance >0.025m and a headless capture or finite input-locked benchmark".into());
    }
    if capture_linear && (capture.is_none() || msaa != 1) {
        return Err("--capture-linear requires --capture and --msaa 1".into());
    }
    if visual.vegetation && scene != "adaptive" {
        return Err("--vegetation on currently requires --scene adaptive".into());
    }
    params.validate()?;
    if seeded_preset
        && !explicit_start
        && eye.is_none()
        && params.recipe == frontier_world::FbmRecipe::SeededWorld
    {
        start_xz = seeded_spawn(seed, params)?;
        eprintln!(
            "Seeded world seed={seed}, dry spawn=({:.1},{:.1})m",
            start_xz[0], start_xz[1]
        );
    }
    if camera_preset == "desert" && !explicit_start && eye.is_none() {
        let o = frontier_world::desert::oasis(seed, 0, 0);
        let scale = params.horizontal_scale / 1800.;
        start_xz = [
            (o.x - o.radius * 1.35) * scale,
            (o.z - o.radius * 0.65) * scale,
        ];
    }
    if capture.is_some() && report_path.is_some() {
        return Err(
            "--report requires a window benchmark (--frames N); --capture writes only a PNG".into(),
        );
    }
    if capture_unresolved && (capture.is_none() || !scene.starts_with("adaptive")) {
        return Err("--capture-unresolved requires --capture with adaptive scene".into());
    }
    if adaptive_wait_quality && (capture.is_some() || !scene.starts_with("adaptive")) {
        return Err("--adaptive-wait-quality requires an adaptive window benchmark".into());
    }
    // Finite captures/benchmarks keep their historic resource baseline. Normal
    // interactive starts can use available RAM; no image quality is changed here.
    let deterministic = capture.is_some()
        || capture_on_exit.is_some()
        || frame_limit.is_some()
        || seconds.is_some()
        || report_path.is_some()
        || benchmark_input_lock;
    let profile = resource_profile.unwrap_or(if deterministic {
        ResourceProfile::Constrained
    } else {
        ResourceProfile::Development
    });
    let alpine = crate::scene_policy::polished(params.recipe);
    visual.apply_resource_profile(profile, alpine);
    if profile == ResourceProfile::Custom {
        visual.cache_mib = adaptive_cache_mib;
    }
    visual.workers = adaptive_workers;
    visual.refinements = adaptive_refinements;
    for (flag, value) in &resource_overrides {
        match flag.as_str() {
            "--terrain-cache-mib" | "--adaptive-cache-mib" => visual.cache_mib = *value,
            "--build-memory-mib" | "--adaptive-build-mib" => visual.build_mib = *value,
            "--terrain-gpu-mib" => visual.terrain_gpu_mib = *value,
            "--plant-cpu-mib" => visual.plant_cpu_mib = *value,
            "--plant-gpu-mib" => visual.plant_gpu_mib = *value,
            "--triangle-budget" => visual.triangle_budget = *value,
            "--upload-mib" => visual.upload_mib = *value,
            "--adaptive-workers" => visual.workers = *value,
            "--adaptive-refinements" => visual.refinements = *value,
            _ => unreachable!(),
        }
    }
    if let Some(radius) = native_radius {
        visual.native_radius_m = radius;
    }
    if let Some(distance) = vegetation_near {
        visual.vegetation_near_m = distance;
    }
    if !resource_overrides.is_empty() || native_radius.is_some() || vegetation_near.is_some() {
        visual.resource_profile = ResourceProfile::Custom;
    }
    adaptive_cache_mib = visual.cache_mib;
    adaptive_build_mib = visual.build_mib;
    adaptive_workers = visual.workers;
    adaptive_refinements = visual.refinements;
    let player_edit = player_edit.unwrap_or(live_water);
    if player_edit && !live_water {
        return Err("--player-edit on currently requires --live-water on for atomic terrain/fluid/inventory saves".into());
    }
    if !hand.reach_m.is_finite()
        || !(0.0625..=8.).contains(&hand.reach_m)
        || !hand.volume_liters.is_finite()
        || !(0.001..=16.).contains(&hand.volume_liters)
        || !hand.extent_m.is_finite()
        || !(0.0625..=0.5).contains(&hand.extent_m)
        || !hand.quick_seconds.is_finite()
        || !(0.01..=2.).contains(&hand.quick_seconds)
        || !hand.full_seconds.is_finite()
        || hand.full_seconds <= hand.quick_seconds
        || hand.full_seconds > 10.
        || !(1..=1_048_576).contains(&inventory_capacity)
    {
        return Err(
            "invalid pickup reach/hand volume/extent/charge time or inventory capacity".into(),
        );
    }
    if live_water
        && !discrete_water
        && !procedural_water
        && !moving_water
        && water_history_cells < crate::live_water::active_cell_capacity(water_active_radius)
    {
        return Err("--water-history-cells must fit the complete active region; increase history or lower active radius".into());
    }
    if (procedural_water || moving_water) && params.recipe != frontier_world::FbmRecipe::SeededWorld
    {
        return Err("--water-model procedural|moving requires Seeded World terrain; use --preset seeded-world".into());
    }
    if live_water && (scene != "adaptive" || !visual.naadf_enabled) {
        return Err("live water requires --scene adaptive and --naadf on".into());
    }
    if water_save.is_some() && !live_water {
        return Err("--water-save requires --live-water on".into());
    }
    if live_water && (visual.naadf_gpu_distance || !visual.naadf_follow) {
        return Err("live water requires --naadf-follow on --naadf-gpu-distance off".into());
    }
    if live_water && capture.is_some() {
        return Err(
            "live water uses window ticks; use --frames/--seconds with --capture-on-exit".into(),
        );
    }
    Ok(Some(Options {
        bowl_save,
        pond_test,
        live_water,
        discrete_water,
        procedural_water,
        moving_water,
        water_river_rate,
        player_edit,
        hand,
        inventory_capacity,
        water_active_radius,
        water_history_cells,
        water_save,
        water_demo,
        atmosphere,
        visual,
        render_scale,
        capture,
        capture_on_exit,
        seconds,
        width,
        height,
        frame_limit,
        scene,
        camera_preset,
        seed,
        start_xz,
        params,
        route,
        uncapped,
        warmup,
        report_path,
        route_step,
        route_speed,
        route_realtime,
        terrain_static,
        view_levels,
        page_spacing,
        msaa,
        capture_route_time,
        capture_coverage_shift,
        voxel_filter,
        renderer,
        surface_layout,
        teleport_test,
        edit_test,
        eye,
        target,
        fov_y,
        capture_linear,
        capture_far,
        pixel_error,
        capture_unresolved,
        adaptive_wait_quality,
        adaptive_cache_mib,
        adaptive_workers,
        adaptive_refinements,
        adaptive_direct_native,
        adaptive_build_mib,
        adaptive_disk_cache,
        adaptive_disk_mib,
        benchmark_input_lock,
        native_bins,
    }))
}

/// Bounded deterministic search, only used by the opt-in world preset. Check a
/// dry 4m neighborhood and a gentle local slope; no authored path or landmark.
fn seeded_spawn(seed: u64, params: frontier_world::FbmParams) -> Result<[f64; 2]> {
    let terrain = frontier_world::FbmTerrain::new(seed, params)?;
    let mut fallback = None;
    for spacing in [128., 512., 8192.] {
        // Once local grass has been exhausted, keep the nearby dry desert or
        // stone spawn rather than crossing continents to force a green biome.
        if spacing == 8192. {
            if let Some(point) = fallback {
                return Ok(point);
            }
        }
        for ring in 0i32..=16 {
            for iz in -ring..=ring {
                for ix in -ring..=ring {
                    if ix.abs().max(iz.abs()) != ring {
                        continue;
                    }
                    let x = ix as f64 * spacing;
                    let z = iz as f64 * spacing;
                    let p = frontier_world::Cell::from_meters([x, 0., z]);
                    let center = terrain.column_cells(p.x, p.z);
                    if center.visible_top != center.ground_top {
                        continue;
                    }
                    let h = center.ground_top as f64 * frontier_world::VOXEL_SIZE;
                    let safe =
                        [(-4., 0.), (4., 0.), (0., -4.), (0., 4.)]
                            .into_iter()
                            .all(|(dx, dz)| {
                                let p = frontier_world::Cell::from_meters([x + dx, 0., z + dz]);
                                let c = terrain.column_cells(p.x, p.z);
                                c.visible_top == c.ground_top
                                    && (c.ground_top as f64 * frontier_world::VOXEL_SIZE - h).abs()
                                        <= 1.5
                            });
                    if safe {
                        if center.ground_surface == frontier_world::Material::Grass
                            || spacing == 8192.
                        {
                            return Ok([x, z]);
                        }
                        fallback.get_or_insert([x, z]);
                    }
                }
            }
        }
    }
    Err("no dry walkable spawn found within bounded 131072m search; choose --start-xz X,Z".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn timed(scene: &str) -> Vec<String> {
        [
            "--scene",
            scene,
            "--eye",
            "0,2,-3",
            "--target",
            "0,2,0",
            "--capture-far",
            "300",
            "--benchmark-input-lock",
            "--seconds",
            "1",
            "--capture-on-exit",
            "/tmp/phasea-option-test.png",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }
    #[test]
    fn desert_and_alpine_share_the_full_rendering_baseline() {
        let parse = |preset: &str| {
            parse_args(
                ["--preset", preset, "--frames", "1"]
                    .into_iter()
                    .map(str::to_string),
            )
            .unwrap()
            .unwrap()
        };
        let alpine = parse("alpine");
        let mut desert = parse("desert");
        assert_eq!(desert.pixel_error, alpine.pixel_error);
        assert_eq!(desert.view_levels, alpine.view_levels);
        assert_eq!(desert.capture_far, alpine.capture_far);
        assert_eq!(desert.msaa, alpine.msaa);
        assert_eq!(desert.voxel_filter, alpine.voxel_filter);
        assert_eq!(desert.adaptive_cache_mib, alpine.adaptive_cache_mib);
        assert!(desert.visual.terrain_shadows);
        // Content and the detected memory snapshot may differ, rendering must not.
        assert!(!desert.visual.vegetation);
        assert!(alpine.visual.vegetation);
        desert.visual.vegetation = alpine.visual.vegetation;
        desert.visual.memory = alpine.visual.memory;
        assert_eq!(desert.visual, alpine.visual);
        assert!(crate::scene_policy::polished(desert.params.recipe));
        assert!(crate::scene_policy::polished(alpine.params.recipe));
        assert!(!crate::scene_policy::polished(
            frontier_world::FbmRecipe::Legacy
        ));
    }
    #[test]
    fn desert_preset_spawns_on_dry_ground_beside_a_seeded_pond() {
        let o = parse_args(
            ["--preset", "desert", "--seed", "7", "--frames", "1"]
                .into_iter()
                .map(str::to_string),
        )
        .unwrap()
        .unwrap();
        assert_eq!(o.params.recipe, frontier_world::FbmRecipe::Desert);
        assert!(!o.visual.vegetation);
        assert!(o.visual.walking);
        let t = frontier_world::FbmTerrain::new(o.seed, o.params).unwrap();
        let [x, z] = o.start_xz;
        assert_eq!(t.height_m(x, z), t.ground_height_m(x, z));
    }
    #[test]
    fn seeded_preset_uses_dry_ground_and_respects_later_overrides() {
        let parse = |args: &[&str]| {
            parse_args(args.iter().map(|s| s.to_string()))
                .unwrap()
                .unwrap()
        };
        let world = parse(&["--preset", "seeded-world", "--frames", "1"]);
        assert_eq!(world.seed, 7);
        assert_eq!(world.params.recipe, frontier_world::FbmRecipe::SeededWorld);
        assert_eq!(world.camera_preset, "seeded-world");
        assert!(world.visual.naadf_enabled && world.visual.walking && world.visual.beauty);
        assert_eq!(world.visual.naadf_radius_m, 64);
        assert_eq!(world.pixel_error, 5.);
        assert_eq!(world.capture_far, Some(6000.));
        assert!(!world.visual.vegetation);
        let terrain = frontier_world::FbmTerrain::new(world.seed, world.params).unwrap();
        let cell = frontier_world::Cell::from_meters([world.start_xz[0], 0., world.start_xz[1]]);
        let column = terrain.column_cells(cell.x, cell.z);
        assert_eq!(column.ground_top, column.visible_top);
        let explicit = parse(&[
            "--preset",
            "world",
            "--seed",
            "19",
            "--start-xz",
            "123,-456",
            "--naadf",
            "off",
            "--beauty",
            "off",
            "--walk",
            "off",
            "--fog-density",
            "0",
            "--camera",
            "aerial",
        ]);
        assert_eq!(explicit.seed, 19);
        assert_eq!(explicit.camera_preset, "aerial");
        assert_eq!(explicit.start_xz, [123., -456.]);
        assert!(
            !explicit.visual.naadf_enabled && !explicit.visual.beauty && !explicit.visual.walking
        );
        assert_eq!(explicit.visual.fog_density, 0.);
        let recipe = parse(&["--terrain-recipe", "seeded-world"]);
        assert!(!recipe.visual.walking);
        assert_eq!(recipe.start_xz, [0., 0.]);
    }
    #[test]
    fn adaptive_defaults_to_hybrid_with_explicit_overrides() {
        let parse = |args: &[&str]| parse_args(args.iter().map(|s| s.to_string()));
        assert!(
            parse(&["--scene", "adaptive"])
                .unwrap()
                .unwrap()
                .visual
                .naadf_enabled
        );
        let default = parse(&[]).unwrap().unwrap();
        assert!(default.visual.naadf_enabled);
        assert_eq!(default.msaa, 1);
        assert_eq!(default.visual.naadf_radius_m, 64);
        let off = parse(&["--preset", "alpine", "--naadf", "off"])
            .unwrap()
            .unwrap();
        assert!(!off.visual.naadf_enabled);
        assert_eq!(off.msaa, 4);
        let reference = parse(&["--scene", "hybrid"]).unwrap().unwrap();
        assert!(!reference.visual.naadf_enabled);
        let explicit = parse(&["--preset", "alpine", "--msaa", "4"])
            .unwrap()
            .unwrap();
        assert_eq!(explicit.msaa, 4);
        let options = parse(&[
            "--preset",
            "alpine",
            "--naadf-radius",
            "128",
            "--naadf-budget-mib",
            "1024",
        ])
        .unwrap()
        .unwrap();
        assert!(options.visual.naadf_enabled);
        assert_eq!(options.visual.naadf_radius_m, 128);
        assert_eq!(options.visual.naadf_budget_mib, 1024);
        for args in [
            vec!["--naadf-radius", "0"],
            vec!["--naadf-radius", "257"],
            vec!["--naadf-budget-mib", "15"],
            vec!["--scene", "hybrid", "--naadf", "on"],
        ] {
            assert!(parse(&args).is_err());
        }
    }
    #[test]
    fn resource_profiles_preserve_finite_comparisons_and_manual_overrides() {
        use crate::settings::ResourceProfile;
        let args = ["--preset", "alpine", "--frames", "1"];
        let constrained = parse_args(args.into_iter().map(str::to_string))
            .unwrap()
            .unwrap();
        assert_eq!(
            constrained.visual.resource_profile,
            ResourceProfile::Constrained
        );
        assert_eq!(constrained.visual.cache_mib, 640);
        assert_eq!(constrained.visual.build_mib, 512);
        assert_eq!(constrained.visual.terrain_gpu_mib, 512);
        assert_eq!(constrained.visual.upload_mib, 2);
        let args = [
            "--resource-profile",
            "development",
            "--adaptive-cache-mib",
            "900",
            "--plant-cpu-mib",
            "300",
            "--native-radius",
            "12",
        ];
        let custom = parse_args(args.into_iter().map(str::to_string))
            .unwrap()
            .unwrap();
        assert_eq!(custom.visual.resource_profile, ResourceProfile::Custom);
        assert_eq!(custom.visual.cache_mib, 900);
        assert_eq!(custom.adaptive_cache_mib, 900);
        assert_eq!(custom.visual.plant_cpu_mib, 300);
        assert_eq!(custom.visual.native_radius_m, 12.);
        for args in [
            ["--native-radius", "NaN"],
            ["--resource-profile", "unknown"],
            ["--upload-mib", "0"],
        ] {
            assert!(parse_args(args.into_iter().map(str::to_string)).is_err());
        }
    }
    #[test]
    fn alpine_preset_and_visual_overrides_are_reproducible() {
        let defaults = parse_args(std::iter::empty()).unwrap().unwrap();
        assert_eq!(defaults.params.recipe, frontier_world::FbmRecipe::Regional);
        assert!(!defaults.visual.terrain_shadows);
        assert!(!defaults.visual.vegetation);
        assert!(!defaults.visual.walking);
        assert!(!defaults.visual.beauty);
        let args = [
            "--preset",
            "alpine",
            "--sun-elevation",
            "31",
            "--fog-density",
            "0",
            "--terrain-shadows",
            "off",
        ];
        let alpine = parse_args(args.into_iter().map(str::to_string))
            .unwrap()
            .unwrap();
        assert_eq!(alpine.params.recipe, frontier_world::FbmRecipe::Alpine);
        assert_eq!(
            alpine.start_xz,
            [frontier_world::terrain::alpine_path_center_m(-250.), -250.]
        );
        assert_eq!(alpine.capture_far, Some(6000.));
        assert!(alpine.visual.vegetation);
        assert!(alpine.visual.walking);
        assert!(alpine.visual.beauty);
        let disabled = parse_args(
            [
                "--preset",
                "alpine",
                "--vegetation",
                "off",
                "--walk",
                "off",
                "--beauty",
                "off",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap()
        .unwrap();
        assert!(!disabled.visual.vegetation && !disabled.visual.walking);
        assert!(!disabled.visual.beauty);
        assert_eq!(alpine.visual.sun_elevation, 31.);
        assert_eq!(alpine.visual.fog_density, 0.);
        assert!(!alpine.visual.terrain_shadows);
        for args in [
            ["--exposure", "NaN"],
            ["--cloud-coverage", "1.1"],
            ["--sun-elevation", "90"],
            ["--terrain-shadows", "maybe"],
        ] {
            assert!(parse_args(args.into_iter().map(str::to_string)).is_err());
        }
    }
    #[test]
    fn finite_locked_benchmarks_accept_the_reference_pose_and_far() {
        for scene in [
            "pages",
            "pages-topology",
            "hybrid",
            "hybrid-topology",
            "fbm",
            "fixture",
        ] {
            let parsed = parse_args(timed(scene).into_iter()).unwrap().unwrap();
            assert_eq!(parsed.eye, Some([0., 2., -3.]));
            assert_eq!(parsed.capture_far, Some(300.));
        }
    }
    #[test]
    fn ordinary_terrain_launches_keep_camera_guards_and_linear_requires_float_target() {
        for flag in ["--benchmark-input-lock", "--seconds", "--capture-on-exit"] {
            let mut args = timed("pages");
            let i = args.iter().position(|v| v == flag).unwrap();
            args.remove(i);
            if flag != "--benchmark-input-lock" {
                args.remove(i);
            }
            assert!(parse_args(args.into_iter()).is_err());
        }
        let mut args = timed("fixture");
        args.push("--capture-linear".into());
        assert!(parse_args(args.into_iter()).is_err());
        let args = [
            "--scene",
            "pages",
            "--capture",
            "/tmp/phasea-option-test.png",
            "--capture-linear",
            "--msaa",
            "1",
            "--eye",
            "0,2,-3",
            "--target",
            "0,2,0",
            "--capture-far",
            "300",
        ];
        assert!(parse_args(args.into_iter().map(str::to_string)).is_ok());
        let mut args = timed("pages");
        let i = args.iter().position(|v| v == "300").unwrap();
        args[i] = "NaN".into();
        assert!(parse_args(args.into_iter()).is_err());
    }
    #[test]
    fn live_water_requires_supported_native_publication_and_is_opt_in() {
        let parse = |args: &[&str]| {
            parse_args(args.iter().map(|s| s.to_string()))
                .unwrap()
                .unwrap()
        };
        assert!(!parse(&[]).live_water);
        let live = parse(&["--preset", "seeded-world", "--water-demo"]);
        assert!(live.live_water && live.water_demo);
        let discrete = parse(&["--preset", "seeded-world", "--water-model", "discrete"]);
        assert!(discrete.live_water && discrete.discrete_water && discrete.water_demo);
        let ca = parse(&["--preset", "seeded-world", "--water-model", "ca"]);
        assert!(ca.live_water && !ca.discrete_water);
        let procedural = parse(&["--preset", "seeded-world", "--water-model", "procedural"]);
        assert!(procedural.live_water && procedural.procedural_water && !procedural.discrete_water);
        let moving = parse(&[
            "--preset",
            "seeded-world",
            "--water-model",
            "moving",
            "--water-river-rate",
            "0",
        ]);
        assert!(moving.live_water && moving.moving_water && moving.water_river_rate == Some(0));
        assert!(parse_args(
            ["--live-water", "on", "--naadf", "off"]
                .into_iter()
                .map(str::to_string)
        )
        .is_err());
        assert!(parse_args(
            ["--water-save", "unused.save"]
                .into_iter()
                .map(str::to_string)
        )
        .is_err());
    }
}
