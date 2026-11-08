//! Command-line configuration for the three explicit comparison scenes.
use super::{Result, HEIGHT, SEED, WIDTH};
use std::path::PathBuf;
pub struct Options {
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
fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Option<Options>> {
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
    let mut adaptive_build_mib = 512usize;
    let mut adaptive_refinements = 8usize;
    let mut adaptive_direct_native = false;
    let mut adaptive_disk_cache = None;
    let mut adaptive_disk_mib = 2048usize;
    let mut benchmark_input_lock = false;
    let mut native_bins = true;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--preset" => {
                if args.next().as_deref() != Some("alpine") {
                    return Err("--preset requires alpine".into());
                }
                params = frontier_world::FbmParams {
                    recipe: frontier_world::FbmRecipe::Alpine,
                    ..Default::default()
                };
                scene = "adaptive".into();
                capture_far = Some(6000.);
                view_levels = 11;
                pixel_error = 3.;
                camera_preset = "alpine".into();
                start_xz = [frontier_world::terrain::alpine_path_center_m(-250.), -250.];
                atmosphere = Some(true);
                visual.sun_elevation = 34.;
                visual.sun_azimuth = 100.;
                visual.exposure = 1.15;
                visual.fog_density = 0.000045;
                visual.cloud_coverage = 0.52;
                visual.cloud_speed = 0.;
                visual.overcast = 0.08;
                visual.terrain_shadows = true;
                visual.beauty = true;
                visual.vegetation = true;
                visual.walking = true;
            }
            "--sun-elevation"
            | "--sun-azimuth"
            | "--exposure"
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
            "--vegetation" | "--walk" | "--beauty" => {
                let value = match args.next().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err(format!("{arg} requires on|off").into()),
                };
                if arg == "--vegetation" {
                    visual.vegetation = value;
                } else if arg == "--beauty" {
                    visual.beauty = value;
                } else {
                    visual.walking = value;
                }
            }
            "--terrain-shadows" => {
                visual.terrain_shadows = match args.next().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err("--terrain-shadows requires on|off".into()),
                };
            }
            "--atmosphere" => {
                atmosphere = Some(match args.next().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err("--atmosphere requires on|off".into()),
                });
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
            }
            "--adaptive-direct-native" => {
                adaptive_direct_native = match args.next().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err("--adaptive-direct-native requires on|off".into()),
                };
            }
            "--adaptive-refinements" => {
                adaptive_refinements = args
                    .next()
                    .ok_or("--adaptive-refinements requires 1..32")?
                    .parse()?;
                if !(1..=32).contains(&adaptive_refinements) {
                    return Err("--adaptive-refinements requires 1..32".into());
                }
            }
            "--adaptive-workers" => {
                adaptive_workers = args
                    .next()
                    .ok_or("--adaptive-workers requires 1..16")?
                    .parse()?;
                if !(1..=16).contains(&adaptive_workers) {
                    return Err("--adaptive-workers requires 1..16".into());
                }
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
                native_bins = match args.next().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err("--native-bins requires on|off".into()),
                }
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
                    .ok_or("--camera requires valley|aerial|topology|cave|bridge")?
            }
            "--terrain-recipe" => {
                params.recipe =
                    match args.next().as_deref() {
                        Some("legacy") => frontier_world::FbmRecipe::Legacy,
                        Some("alpine") => frontier_world::FbmRecipe::Alpine,
                        Some("regional") => frontier_world::FbmRecipe::Regional,
                        Some("regional-unwarped") => frontier_world::FbmRecipe::RegionalUnwarped,
                        _ => return Err(
                            "--terrain-recipe requires legacy|regional|regional-unwarped|alpine"
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
            }
            "--seed" => seed = args.next().ok_or("--seed requires integer")?.parse()?,
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
                voxel_filter = match args.next().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err("--voxel-filter requires on|off".into()),
                }
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
            "--msaa" => msaa = args.next().ok_or("--msaa requires 1|4")?.parse()?,
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
                    _ => return Err("--route requires static|fly|stop-resume|turn-return".into()),
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
                println!("Visual preview: --preset alpine selects the Alpine meadow scene; later flags override preset values. --sun-elevation DEGREES --sun-azimuth DEGREES --exposure SCALE --fog-density PER_M --fog-height-falloff PER_M --cloud-coverage 0..1 --cloud-speed 0..0.1 --overcast 0..1 --terrain-shadows on|off (bounded Alpine adaptive geometry only). Alpine enables scene beauty, seeded vegetation and grounded walking; --beauty on|off toggles the lighting/atmosphere treatment; --vegetation on|off --walk on|off override them. F toggles walk/free flight; Space jumps while walking. Esc opens settings; --atmosphere on|off, --render-scale 0.5..1. Current Regional terrain is the default. Quality/resources apply together in the menu; atmosphere controls are live.\nAdaptive candidate: --scene adaptive|adaptive-topology --pixel-error 1 --adaptive-cache-mib 512 --adaptive-workers N (default up to four available CPU threads, 1..16 accepted, up to four active builds), --adaptive-build-mib 512 reservation budget. Selection batch: --adaptive-refinements 1..32 (default 8; independent of builder concurrency). Experimental nearest-leaf direct-native selection: --adaptive-direct-native on|off (default off). Optional derived disk cache: --adaptive-disk-cache PATH --adaptive-disk-mib 2048. Exact cave/bridge owner remains resident in adaptive-topology. Headless captures require settled quality; --capture-unresolved explicitly permits a coherent diagnostic image with failed quality marked in metadata. --adaptive-wait-quality starts benchmark clocks only after current coverage and the 0.8x internal error reserve settle (120s failure limit; reported quality still uses the requested bound); normal startup begins from coarse coverage. Adaptive window benchmarks accept --capture-far and explicit camera poses for matched clips.");
                println!("Prototype comparisons: --scene hybrid-topology --camera topology|cave|bridge; --renderer mesh|brick|brick-skip|brick-directional (fixture only); --surface-layout clipmap|quadtree (fbm only, quadtree freezes source pages); --teleport-test (hybrid only); --edit-test (fixture or hybrid-topology: add 1m cube at 2s, restore original materials 2s after publication); T teleports 512m or home. Single-sample captures also write .depth.f32. Explicit headless camera: --eye X,Y,Z --target X,Y,Z --fov-y 60 --msaa 1 --voxel-filter off --capture-linear (optional unclipped float32 render target); captures write .capture.json, .linear_rgb.f32 (raw float32 with --capture-linear, otherwise decoded 8-bit target), .coverage.f32 and .depth.f32.\nFrontier experimental FBM terrain (default) / exact native fixture\n--terrain-recipe regional|regional-unwarped|legacy|alpine (default regional); --start-xz X,Z; voxel-filter on and MSAA4 default\n--scene pages|pages-topology|adaptive|adaptive-topology|fbm|fbm-native|hybrid|hybrid-static|fixture --pixel-error 1 --seed 7 --scale 1800 --amplitude 1000 --octaves 8 --roughness 0.5 --camera valley|aerial\n--route static|fly|stop-resume|turn-return --route-step 0.0166667 --route-speed MPS --route-clock frame|realtime --seconds N --capture-on-exit PATH --terrain-static --view-levels 7..11 --msaa 1|4 --voxel-filter on|off --capture-route-time SECONDS --capture-coverage-shift METERS (headless hybrid diagnostic only) --uncapped --warmup N --report PATH\nPages: bounded uniform reusable GPU grid baseline; --page-spacing 1|2|4|8|16 meters, 24x24 worldaligned32cell pages. Default far300*spacing meters, hard coverage344*spacing; native/collar and fixed volume owner retained; only1m spacing matches collar samples. No certified error or CDLOD claim.\nFBM: moving background-generated nested GPU grids, bounded ±2048m around published center, surface-only, no editing/native handoff. --terrain-static freezes the initial coverage for comparison. WASD 35m/s; Shift 80m/s.\nHybrid: moving 64m exact native heightfield chunks, approximate 2m collar, GPU proxy with coherent ownership publication; hybrid-topology adds a fixed exact cave/bridge owner and local edits. No collision or certified proxy error. hybrid-static preserves the fixed CPU reference. Default eye 1.7m above canonical top at origin.\nFrontier bounded native-voxel fixture\n--capture PATH [--width 1280 --height 720] or --frames N for a finite window run\nClick: capture mouse; WASD: free flight; Shift: fast; Space/Ctrl: vertical; Esc: release mouse; F12: PNG\nAim at screen center: left removes a cell, right places stone (8m reach); wait for each rebuild.\nThis reference has no streaming, distant LOD, collision or persistence.");
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
    Ok(Some(Options {
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
}
