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
    let mut params = frontier_world::FbmParams::default();
    let mut route = frontier_runtime::routes::Route::Stationary;
    let mut uncapped = false;
    let mut warmup = 0;
    let mut report_path = None;
    let mut route_step: f64 = 1. / 60.;
    let mut route_speed: Option<f64> = None;
    let mut route_realtime = false;
    let mut terrain_static = false;
    let mut view_levels = 7;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--scene" => scene = args.next().ok_or("--scene requires fixture|fbm")?,
            "--camera" => camera_preset = args.next().ok_or("--camera requires valley|aerial")?,
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
            "--uncapped" => uncapped = true,
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
                println!("Frontier experimental FBM terrain (default) / exact native fixture\n--scene fbm|fbm-native|hybrid|fixture --seed 7 --scale 1800 --amplitude 1000 --octaves 8 --roughness 0.5 --camera valley|aerial\n--route static|fly|stop-resume|turn-return --route-step 0.0166667 --route-speed MPS --route-clock frame|realtime --seconds N --capture-on-exit PATH --terrain-static --view-levels 7..11 --uncapped --warmup N --report PATH\nFBM: moving background-generated nested GPU grids, bounded ±2048m around published center, surface-only, no editing/native handoff. --terrain-static freezes the initial coverage for comparison. WASD 35m/s; Shift 80m/s.\nHybrid: fixed 64m exact native heightfield interior, approximate 2m collar, static outer proxy; no caves, edits, moving ownership or certified proxy error. Default eye 1.7m above canonical top at origin.\nFrontier bounded native-voxel fixture\n--capture PATH [--width 1280 --height 720] or --frames N for a finite window run\nClick: capture mouse; WASD: free flight; Shift: fast; Space/Ctrl: vertical; Esc: release mouse; F12: PNG\nAim at screen center: left removes a cell, right places stone (8m reach); wait for each rebuild.\nThis reference has no streaming, distant LOD, collision or persistence.");
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
    }))
}
