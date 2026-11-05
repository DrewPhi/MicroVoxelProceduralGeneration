//! Command-line configuration for the three explicit comparison scenes.
use super::{Result, HEIGHT, SEED, WIDTH};
use std::path::PathBuf;
pub struct Options {
    pub capture: Option<PathBuf>,
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
}
pub fn parse() -> Result<Option<Options>> {
    let mut capture = None;
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
            "--warmup" => warmup = args.next().ok_or("--warmup requires frames")?.parse()?,
            "--report" => {
                report_path = Some(PathBuf::from(
                    args.next().ok_or("--report requires JSON path")?,
                ))
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
                println!("Frontier experimental FBM terrain (default) / exact native fixture\n--scene fbm|fbm-native|fixture --seed 7 --scale 1800 --amplitude 1000 --octaves 8 --roughness 0.5 --camera valley|aerial\n--route static|fly|stop-resume|turn-return --route-step 0.0166667 --uncapped --warmup N --report PATH\nFBM: immutable fixed-center nested GPU grids, bounded ±2048m, surface-only, no editing/streaming/native handoff. WASD 35m/s; Shift 240m/s.\nFrontier bounded native-voxel fixture\n--capture PATH [--width 1280 --height 720] or --frames N for a finite window run\nClick: capture mouse; WASD: free flight; Shift: fast; Space/Ctrl: vertical; Esc: release mouse; F12: PNG\nAim at screen center: left removes a cell, right places stone (8m reach); wait for each rebuild.\nThis reference has no streaming, distant LOD, collision or persistence.");
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
    params.validate()?;
    if capture.is_some() && report_path.is_some() {
        return Err(
            "--report requires a window benchmark (--frames N); --capture writes only a PNG".into(),
        );
    }
    Ok(Some(Options {
        capture,
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
    }))
}
