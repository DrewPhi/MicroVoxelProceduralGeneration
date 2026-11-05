//! Command-line configuration for the three explicit comparison scenes.
use super::{Result, HEIGHT, SEED, WIDTH};
use std::path::PathBuf;
pub struct Options {
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
    pub msaa: u32,
    pub capture_route_time: Option<f64>,
    pub capture_coverage_shift: f64,
    pub voxel_filter: bool,
    pub renderer: String,
    pub surface_layout: String,
    pub teleport_test: bool,
    pub edit_test: bool,
}
pub fn parse() -> Result<Option<Options>> {
    let mut capture = None;
    let mut capture_on_exit = None;
    let mut seconds: Option<f64> = None;
    let mut width = WIDTH;
    let mut height = HEIGHT;
    let mut frame_limit = None;
    let mut scene = String::from("fbm");
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
    let mut msaa = 4;
    let mut capture_route_time: Option<f64> = None;
    let mut capture_coverage_shift: f64 = 0.;
    // User selected the appearance transfer as the preferred default.
    let mut voxel_filter = true;
    let mut renderer = String::from("mesh");
    let mut surface_layout = String::from("clipmap");
    let mut teleport_test = false;
    let mut edit_test = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
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
                params.recipe = match args.next().as_deref() {
                    Some("legacy") => frontier_world::FbmRecipe::Legacy,
                    Some("regional") => frontier_world::FbmRecipe::Regional,
                    Some("regional-unwarped") => frontier_world::FbmRecipe::RegionalUnwarped,
                    _ => {
                        return Err(
                            "--terrain-recipe requires legacy|regional|regional-unwarped".into(),
                        )
                    }
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
                    .ok_or("--renderer requires mesh|brick|brick-skip")?
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
                println!("Prototype comparisons: --scene hybrid-topology --camera topology|cave|bridge; --renderer mesh|brick|brick-skip (fixture only); --surface-layout clipmap|quadtree (fbm only, quadtree freezes source pages); --teleport-test (hybrid only); --edit-test (fixture or hybrid-topology: add 1m cube at 2s, restore original materials 2s after publication); T teleports 512m or home. Single-sample captures also write .depth.f32.\nFrontier experimental FBM terrain (default) / exact native fixture\n--terrain-recipe regional|regional-unwarped|legacy (default regional); --start-xz X,Z; voxel-filter on and MSAA4 default\n--scene fbm|fbm-native|hybrid|hybrid-static|fixture --seed 7 --scale 1800 --amplitude 1000 --octaves 8 --roughness 0.5 --camera valley|aerial\n--route static|fly|stop-resume|turn-return --route-step 0.0166667 --route-speed MPS --route-clock frame|realtime --seconds N --capture-on-exit PATH --terrain-static --view-levels 7..11 --msaa 1|4 --voxel-filter on|off --capture-route-time SECONDS --capture-coverage-shift METERS (headless hybrid diagnostic only) --uncapped --warmup N --report PATH\nFBM: moving background-generated nested GPU grids, bounded ±2048m around published center, surface-only, no editing/native handoff. --terrain-static freezes the initial coverage for comparison. WASD 35m/s; Shift 80m/s.\nHybrid: moving 64m exact native heightfield chunks, approximate 2m collar, GPU proxy with coherent ownership publication; hybrid-topology adds a fixed exact cave/bridge owner and local edits. No collision or certified proxy error. hybrid-static preserves the fixed CPU reference. Default eye 1.7m above canonical top at origin.\nFrontier bounded native-voxel fixture\n--capture PATH [--width 1280 --height 720] or --frames N for a finite window run\nClick: capture mouse; WASD: free flight; Shift: fast; Space/Ctrl: vertical; Esc: release mouse; F12: PNG\nAim at screen center: left removes a cell, right places stone (8m reach); wait for each rebuild.\nThis reference has no streaming, distant LOD, collision or persistence.");
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
        && (capture.is_none() || !matches!(scene.as_str(), "hybrid" | "hybrid-topology"))
    {
        return Err(
            "--capture-coverage-shift is only for hybrid headless publication comparisons".into(),
        );
    }
    if capture_route_time.is_some_and(|t| !t.is_finite() || t < 0.) {
        return Err("--capture-route-time requires finite nonnegative seconds".into());
    }
    if msaa != 1 && msaa != 4 {
        return Err("--msaa requires 1|4".into());
    }
    if !["mesh", "brick", "brick-skip"].contains(&renderer.as_str()) {
        return Err("--renderer requires mesh|brick|brick-skip".into());
    }
    if renderer != "mesh" && scene != "fixture" {
        return Err("brick comparison currently supports --scene fixture".into());
    }
    if edit_test && !["fixture", "hybrid-topology"].contains(&scene.as_str()) {
        return Err("--edit-test requires fixture or hybrid-topology scene".into());
    }
    if edit_test && capture.is_some() {
        return Err(
            "--edit-test requires a window run; use --capture-on-exit with --seconds".into(),
        );
    }
    if teleport_test && !["hybrid", "hybrid-topology"].contains(&scene.as_str()) {
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
    if start_xz != [0., 0.] && !matches!(scene.as_str(), "fbm" | "hybrid") {
        return Err("--start-xz supports fbm and hybrid scenes only".into());
    }
    params.validate()?;
    if capture.is_some() && report_path.is_some() {
        return Err(
            "--report requires a window benchmark (--frames N); --capture writes only a PNG".into(),
        );
    }
    Ok(Some(Options {
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
        msaa,
        capture_route_time,
        capture_coverage_shift,
        voxel_filter,
        renderer,
        surface_layout,
        teleport_test,
        edit_test,
    }))
}
