//! Native application shell. Window/input, GPU resources, streaming and camera
//! are separate modules; authoritative world code stays in the shared library.
mod camera;
mod capture;
mod diagnostics;
mod environment;
mod gpu;
mod lod_planner;
mod maps;
mod menu;
mod overview;
mod player_view;
mod presentation;
mod raycaster;
mod runtime;
mod settings;
mod streaming;
mod visibility;
use diagnostics::{benchmark, character_preview, lod_check, ray_study, streaming_check};
use gpu::Gpu;
use microvoxel::{
    generation::{debug::DebugLayer, ClimatePreset, ProceduralTerrain, TerrainConfig},
    World,
};
use runtime::Runtime;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Fullscreen, Window, WindowId},
};

#[derive(Clone)]
struct Options {
    character_preview: Option<PathBuf>,
    seed: u64,
    climate: ClimatePreset,
    wind: Option<f64>,
    moisture: Option<f64>,
    debug_maps: Option<PathBuf>,
    map: Option<DebugLayer>,
    isometric: bool,
    view: player_view::ViewMode,
    fly: bool,
    player_test: bool,
    headless: bool,
    screenshot: Option<PathBuf>,
    frames: u32,
    tiles: usize,
    benchmark: bool,
    fly_test: bool,
    lod_test: bool,
    edit_save: Option<PathBuf>,
    display: settings::Settings,
    display_explicit: bool,
    camera: Option<capture::Viewpoint>,
    size: [u32; 2],
    no_hud: bool,
    camera_ground: bool,
    streaming_test: bool,
    ray_study: bool,
}
impl Options {
    fn read() -> microvoxel::Result<Self> {
        let mut options = Self {
            character_preview: None,
            seed: 42,
            climate: ClimatePreset::Temperate,
            wind: None,
            moisture: None,
            debug_maps: None,
            map: None,
            isometric: false,
            view: player_view::ViewMode::First,
            fly: false,
            player_test: false,
            headless: false,
            screenshot: None,
            frames: 30,
            tiles: 8192,
            benchmark: false,
            fly_test: false,
            lod_test: false,
            edit_save: None,
            display: settings::Settings::default(),
            display_explicit: false,
            camera: None,
            size: [1280, 720],
            no_hud: false,
            camera_ground: false,
            streaming_test: false,
            ray_study: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--character-preview" => {
                    options.character_preview = Some(
                        args.next()
                            .ok_or("--character-preview needs a directory")?
                            .into(),
                    )
                }
                "--camera" => {
                    options.camera = Some(
                        args.next()
                            .ok_or("--camera needs X,Y,Z,YAW,PITCH")?
                            .parse()?,
                    )
                }
                "--size" => {
                    options.size = capture::size(&args.next().ok_or("--size needs WIDTHxHEIGHT")?)?
                }
                "--no-hud" => options.no_hud = true,
                "--quality" => {
                    options.display.quality =
                        args.next().ok_or("--quality needs 0..100")?.parse()?;
                    options.display_explicit = true;
                }
                "--native-radius" => {
                    options.display.native_radius =
                        args.next().ok_or("--native-radius needs meters")?.parse()?;
                    options.display_explicit = true;
                }
                "--terrain-memory" => {
                    options.display.memory_mib =
                        args.next().ok_or("--terrain-memory needs MiB")?.parse()?;
                    options.display_explicit = true;
                }
                "--haze" => {
                    options.display.haze = args.next().ok_or("--haze needs 0..2")?.parse()?;
                    options.display_explicit = true;
                }
                "--no-effects" => {
                    options.display.ambient = false;
                    options.display.shadows = false;
                    options.display.antialias = false;
                    options.display.transitions = false;
                    options.display_explicit = true;
                }
                "--seed" => options.seed = args.next().ok_or("--seed needs an integer")?.parse()?,
                "--climate" => {
                    options.climate = args.next().ok_or("--climate needs a preset")?.parse()?
                }
                "--wind" => {
                    options.wind = Some(args.next().ok_or("--wind needs degrees")?.parse()?)
                }
                "--moisture" => {
                    options.moisture = Some(args.next().ok_or("--moisture needs 0..1")?.parse()?)
                }
                "--debug-maps" => {
                    options.debug_maps =
                        Some(args.next().ok_or("--debug-maps needs a directory")?.into())
                }
                "--map" => {
                    options.map = Some(args.next().ok_or("--map needs a layer name")?.parse()?)
                }
                "--isometric" => options.isometric = true,
                "--view" => {
                    options.view = match args
                        .next()
                        .ok_or("--view needs first, rear or front")?
                        .as_str()
                    {
                        "first" => player_view::ViewMode::First,
                        "rear" => player_view::ViewMode::Rear,
                        "front" => player_view::ViewMode::Front,
                        _ => return Err("--view needs first, rear or front".into()),
                    }
                }
                "--fly" => options.fly = true,
                "--player-test" => {
                    options.player_test = true;
                    options.headless = true;
                }
                "--lod-test" => {
                    options.lod_test = true;
                    options.headless = true;
                }
                "--camera-ground" => options.camera_ground = true,
                "--ray-study" => {
                    options.ray_study = true;
                    options.headless = true;
                }
                "--stream-test" => {
                    options.streaming_test = true;
                    options.headless = true;
                }
                "--renderer" => {
                    options.display.ray = match args.next().as_deref() {
                        Some("ray") => true,
                        Some("mesh") => false,
                        _ => return Err("--renderer needs mesh or ray".into()),
                    };
                    options.display_explicit = true;
                }
                "--headless" => options.headless = true,
                "--screenshot" => {
                    options.screenshot =
                        Some(args.next().ok_or("--screenshot needs a path")?.into())
                }
                "--frames" => {
                    options.frames = args.next().ok_or("--frames needs a count")?.parse()?
                }
                "--tiles" => options.tiles = args.next().ok_or("--tiles needs a count")?.parse()?,
                "--save" => {
                    options.edit_save = Some(args.next().ok_or("--save needs a path")?.into())
                }
                "--benchmark" => options.benchmark = true,
                "--fly-test" => options.fly_test = true,
                "--help" | "-h" => {
                    println!("microvoxel-game [--seed 42] [--tiles 8192] [--save edits.mvw]\n  --character-preview DIR  Export procedural character PNGs and animations, then exit\n  --view first|rear|front  Initial player camera\n  --fly             Start in debug flight (F6 returns to player)\n  --player-test     Offscreen walking/jumping/camera smoke test\n  --benchmark       Compare exact and coarse surfaces over the same 16 m patch\n  --isometric       Save a full-region isometric PNG and exit (uses --screenshot)\n  --headless        Render offscreen and exit after --frames (default 30)\n  --screenshot PATH Save a PNG (headless on completion, window on F12)\n  --fly-test        Exercise grid flight and LOD replacement in headless mode\nControls: click to capture mouse; WASD; Space jump; Shift sprint; Esc settings\nF5: first/rear/front view; F6: debug flight (Space/C vertical, Shift boost)\nMouse wheel or +/-: flight speed; 1/2/3: 4/20/100 m/s; [ / ]: FOV\nR: random location; F11: fullscreen; F12: save PNG; close window to quit");
                    println!("Diagnostics: --ray-study profiles column/SVO/DAG geometry and attributes.\nDiagnostics: --stream-test compares both backends while walking/sprinting/turning.\nRenderer: --renderer mesh|ray (also selectable in Esc menu).\nCapture: --camera-ground places --camera X/Z on the ground (Y ignored); combines with --view rear.\nDisplay: --lod-test (quality PNGs + moving-camera stress check)\n  --quality 0..100 --native-radius 2..32 --terrain-memory 64..1024 --haze 0..2 --no-effects\nEsc: settings menu with sliders and editable numbers. Window preferences persist; headless defaults stay deterministic.");
                    println!("Capture: --size 2560x1440 --camera X,Y,Z,YAW,PITCH --no-hud\n  Coordinates in meters; camera angles in degrees. Explicit camera enables flight.");
                    println!("Environment: --climate temperate|continental|arid|subpolar|tropical\n  --wind DEGREES    Prevailing wind direction (towards +X = 0, +Z = 90)\n  --moisture 0..1   Override incoming climatic moisture\n  --debug-maps DIR  Export 12 PNG diagnostic maps and exit (unless --headless/--benchmark)\n  --map LAYER       Start in a diagnostic view (e.g. biomes, rainfall, landforms)\n  M                Cycle diagnostic maps and back to 3D terrain");
                    std::process::exit(0);
                }
                _ => return Err(format!("Unknown argument {arg}; use --help").into()),
            }
        }
        if !(64..=8192).contains(&options.tiles) || !(1..=10000).contains(&options.frames) {
            return Err("--tiles must be 64..8192; --frames must be 1..10000".into());
        }
        options.display.validate();
        Ok(options)
    }
}
fn main() -> microvoxel::Result<()> {
    let options = Options::read()?;
    if let Some(directory) = &options.character_preview {
        return character_preview::export(options.seed, directory);
    }
    let start = Instant::now();
    println!("Preparing the 10 km world (seed {})...", options.seed);
    let mut config = TerrainConfig::for_climate(options.climate);
    if let Some(wind) = options.wind {
        config.wind_degrees = wind;
    }
    if let Some(moisture) = options.moisture {
        config.boundary_moisture = moisture;
    }
    let generator = ProceduralTerrain::with_config(options.seed, config)?;
    println!("Climate {}, wind towards {:.1} degrees, boundary moisture {:.2}; regional data {:.2} MiB\n{}",config.climate,config.wind_degrees,config.boundary_moisture,generator.regional_memory_bytes() as f64/1048576.,generator.environment_statistics());
    if let Some(directory) = &options.debug_maps {
        environment::export(&generator, directory)?;
        if !options.headless && !options.benchmark && !options.isometric {
            return Ok(());
        }
    }
    let mut world = World::new(generator);
    if let Some(path) = &options.edit_save {
        world.load_edits(path)?;
    }
    let world = Arc::new(world);
    if options.ray_study {
        return ray_study::run(world);
    }
    if options.streaming_test {
        return streaming_check::run(world);
    }
    println!("World ready in {:.2}s", start.elapsed().as_secs_f64());
    if options.isometric {
        return overview::render(
            world,
            options
                .screenshot
                .as_deref()
                .unwrap_or(std::path::Path::new("isometric_10km.png")),
        );
    }
    if options.benchmark {
        return benchmark::run(world);
    }
    if options.headless {
        return headless(options, world);
    }
    let event_loop = EventLoop::new()?;
    let mut app = App {
        options,
        world,
        instance: wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default()),
        window: None,
        surface: None,
        config: None,
        runtime: None,
        grabbed: false,
        menu: menu::Menu::new(),
        frame_times: settings::FrameTimes::default(),
        menu_stats: Vec::new(),
        stats_time: Instant::now(),
        last_frame: Instant::now(),
        last_title: Instant::now(),
        frame_count: 0,
        nonce: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos() as u64,
        screenshot: false,
        error: None,
    };
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.error {
        return Err(error.into());
    }
    Ok(())
}
fn headless(options: Options, world: Arc<World<ProceduralTerrain>>) -> microvoxel::Result<()> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let gpu = pollster::block_on(Gpu::new(&instance, None, options.size[0], options.size[1]))?;
    let mut runtime = Runtime::new(gpu, world, options.tiles, 0)?;
    runtime.apply_settings(options.display);
    runtime.set_view(options.view);
    if options.fly || options.fly_test {
        runtime.toggle_flight();
    }
    runtime.gpu.hud_visible = !options.no_hud;
    if options.camera.is_some() {
        runtime.prepare_still_capture();
    }
    if let Some(camera) = options.camera {
        camera.apply(&mut runtime);
        if options.camera_ground {
            camera.ground(&mut runtime)?;
        }
    }
    let deadline =
        Instant::now() + Duration::from_secs(if options.camera.is_some() { 600 } else { 180 });
    let start = Instant::now();
    wait_for_cover(&mut runtime, deadline)?;
    println!(
        "Stitched cover ready in {:.2}s",
        start.elapsed().as_secs_f64()
    );
    println!("Terrain quality: requested {:.2} px, effective {:.2} px; native radius {:.1} m; cache {:.1}/{} MiB",runtime.settings.pixels(),runtime.achieved_pixels,runtime.settings.native_radius,runtime.gpu.bytes as f64/1048576.,runtime.settings.memory_mib);
    if options.lod_test {
        lod_check::run(&mut runtime, std::path::Path::new("output/lod"))?;
    }
    if options.player_test {
        player_smoke(&mut runtime, &options)?;
        wait_for_cover(&mut runtime, deadline)?;
    }
    if options.fly_test {
        let before = runtime.camera.cell;
        runtime.camera.keys.insert(KeyCode::KeyW);
        for _ in 0..60 {
            runtime.update(1. / 60.);
        }
        runtime.camera.clear();
        if runtime.camera.cell == before {
            return Err("flight smoke test did not advance a voxel".into());
        }
        // Let the normal 200 ms planning cadence observe the changed position.
        std::thread::sleep(Duration::from_millis(310));
        runtime.update(0.);
        wait_for_cover(&mut runtime, deadline)?;
        println!(
            "Grid flight: {:?} -> {:?}; replacement cover complete",
            before, runtime.camera.cell
        );
    }
    if let Some(layer) = options.map {
        runtime.set_environment_map(Some(layer))?;
    }
    let texture = runtime.gpu.offscreen();
    let view = texture.create_view(&Default::default());
    for _ in 0..3 {
        runtime.draw(&view);
        runtime.gpu.device.poll(wgpu::Maintain::Wait);
    }
    let start = Instant::now();
    for _ in 0..options.frames {
        runtime.draw(&view);
        runtime.gpu.device.poll(wgpu::Maintain::Wait);
    }
    println!(
        "{} synchronized offscreen frames: {:.2} ms/frame; {} visible tiles; adapter {}",
        options.frames,
        start.elapsed().as_secs_f64() * 1000. / options.frames as f64,
        runtime.visible,
        runtime.gpu.adapter_name
    );
    runtime.report();
    runtime.gpu.screenshot(
        &texture,
        options
            .screenshot
            .as_deref()
            .unwrap_or(std::path::Path::new("output/native_preview.png")),
    )?;
    Ok(())
}
fn player_smoke(runtime: &mut Runtime, options: &Options) -> microvoxel::Result<()> {
    if runtime.flying {
        return Err("--player-test cannot be combined with --fly or --fly-test".into());
    }
    let start = Instant::now();
    for _ in 0..60 {
        runtime.update(1. / 60.);
        if !runtime.player.clear(&runtime.world) {
            return Err("spawn/landing overlap".into());
        }
    }
    let before = runtime.player.root;
    runtime.camera.keys.insert(KeyCode::KeyW);
    for _ in 0..120 {
        runtime.update(1. / 60.);
        if !runtime.player.clear(&runtime.world) {
            return Err("walking overlap".into());
        }
    }
    runtime.clear_controls();
    if runtime.player.root.x == before.x && runtime.player.root.z == before.z {
        return Err("player did not walk".into());
    }
    for _ in 0..180 {
        if runtime.player.grounded {
            break;
        }
        runtime.update(1. / 60.);
    }
    if !runtime.player.grounded {
        return Err("player did not land before jump test".into());
    }
    let ground = runtime.player.root.y;
    let mut peak = ground;
    runtime.camera.keys.insert(KeyCode::Space);
    runtime.update(1. / 60.);
    runtime.camera.keys.remove(&KeyCode::Space);
    for _ in 0..120 {
        runtime.update(1. / 60.);
        peak = peak.max(runtime.player.root.y);
        if !runtime.player.clear(&runtime.world) {
            return Err("jump overlap".into());
        }
    }
    if peak <= ground {
        return Err("player did not jump".into());
    }
    println!("Player smoke: {:?} -> {:?}; jump rose {:.2} m; grounded {}; 301 fixed ticks and collision checks in {:.2} ms",before,runtime.player.root,(peak-ground) as f64/16.,runtime.player.grounded,start.elapsed().as_secs_f64()*1000.);
    let parent = options
        .screenshot
        .as_deref()
        .and_then(|p| p.parent())
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("output"));
    std::fs::create_dir_all(parent)?;
    for (view, name) in [
        (player_view::ViewMode::First, "first"),
        (player_view::ViewMode::Rear, "rear"),
        (player_view::ViewMode::Front, "front"),
    ] {
        runtime.set_view(view);
        runtime.update(1. / 60.);
        runtime.refresh_terrain();
        wait_for_cover(runtime, Instant::now() + Duration::from_secs(60))?;
        let texture = runtime.gpu.offscreen();
        runtime.draw(&texture.create_view(&Default::default()));
        runtime
            .gpu
            .screenshot(&texture, &parent.join(format!("player_{name}.png")))?;
    }
    let old_pitch = runtime.camera.pitch;
    runtime.set_view(player_view::ViewMode::First);
    for (pitch, name) in [(-60f64.to_radians(), "down"), (60f64.to_radians(), "up")] {
        runtime.camera.pitch = pitch;
        for _ in 0..8 {
            runtime.update(1. / 60.);
        }
        if !runtime.player.clear(&runtime.world) {
            return Err("look pose overlaps terrain".into());
        }
        runtime.refresh_terrain();
        wait_for_cover(runtime, Instant::now() + Duration::from_secs(60))?;
        let texture = runtime.gpu.offscreen();
        runtime.draw(&texture.create_view(&Default::default()));
        runtime
            .gpu
            .screenshot(&texture, &parent.join(format!("player_look_{name}.png")))?;
    }
    runtime.camera.pitch = old_pitch;
    for _ in 0..8 {
        runtime.update(1. / 60.);
    }
    runtime.set_view(options.view);
    Ok(())
}
fn wait_for_cover(runtime: &mut Runtime, deadline: Instant) -> microvoxel::Result<()> {
    let mut report = Instant::now();
    while !runtime.settled() {
        runtime.update(0.);
        if let Some(error) = &runtime.last_error {
            return Err(error.clone().into());
        }
        if report.elapsed() > Duration::from_secs(5) {
            let (ready, total) = runtime.loading();
            println!(
                "Preparing terrain: {ready}/{total} tiles; {} jobs; {:.1} MiB cached",
                runtime.pending_jobs(),
                runtime.gpu.bytes as f64 / 1048576.
            );
            report = Instant::now();
        }
        if Instant::now() > deadline {
            return Err("timed out preparing a stitched LOD cover".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}
struct App {
    options: Options,
    world: Arc<World<ProceduralTerrain>>,
    instance: wgpu::Instance,
    window: Option<Arc<Window>>,
    surface: Option<wgpu::Surface<'static>>,
    config: Option<wgpu::SurfaceConfiguration>,
    runtime: Option<Runtime>,
    grabbed: bool,
    menu: menu::Menu,
    frame_times: settings::FrameTimes,
    menu_stats: Vec<String>,
    stats_time: Instant,
    last_frame: Instant,
    last_title: Instant,
    frame_count: u32,
    nonce: u64,
    screenshot: bool,
    error: Option<String>,
}
impl App {
    fn capture(&mut self, grab: bool) {
        let grab = grab
            && !self.menu.open
            && self
                .runtime
                .as_ref()
                .is_some_and(|r| r.debug_layer.is_none());
        let Some(window) = &self.window else {
            return;
        };
        if grab {
            let result = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            if let Err(e) = result {
                eprintln!("Mouse capture: {e}");
                return;
            }
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
        }
        self.grabbed = grab;
        window.set_cursor_visible(!grab);
        if let Some(runtime) = &mut self.runtime {
            runtime.clear_controls();
        }
    }
    fn toggle_menu(&mut self) {
        self.menu.open = !self.menu.open;
        if !self.menu.open {
            if let Some(settings) = self.menu.ready(true) {
                if let Some(runtime) = &mut self.runtime {
                    runtime.apply_settings(settings);
                }
                if let Err(e) = settings.save() {
                    eprintln!("Display preferences: {e}");
                }
            }
        }
        self.capture(!self.menu.open);
    }
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: impl ToString) {
        self.error = Some(error.to_string());
        event_loop.exit();
    }
    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let Some(runtime) = &mut self.runtime else {
            return;
        };
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f64();
        self.last_frame = now;
        self.frame_times.push(dt);
        if let Some(settings) = self.menu.ready(false) {
            runtime.apply_settings(settings);
            if let Err(e) = settings.save() {
                eprintln!("Display preferences: {e}");
            }
        }
        runtime.update(if self.menu.open { 0. } else { dt });
        if self.menu.open
            && (self.stats_time.elapsed() > Duration::from_millis(250)
                || self.menu_stats.is_empty())
        {
            let [median, p95, p99] = self.frame_times.percentiles();
            self.menu_stats = vec![
                format!(
                    "FRAME MS: MED {:.1} / P95 {:.1} / P99 {:.1}",
                    median, p95, p99
                ),
                format!(
                    "TARGET {:.2} PX / EFFECTIVE {:.2} PX {}",
                    runtime.settings.pixels(),
                    runtime.achieved_pixels,
                    if runtime.budget_limited {
                        "BUDGET LIMITED"
                    } else {
                        ""
                    }
                ),
                format!(
                    "TILES {} / NATIVE {} / JOBS {} / CULL {}",
                    runtime.cover.len(),
                    runtime
                        .cover
                        .iter()
                        .filter(|k| k.tile.level == microvoxel::render::lod::FINEST)
                        .count(),
                    runtime.pending_jobs(),
                    runtime.gpu.culled
                ),
                format!(
                    "DATA {:.1}/{} MIB / ARENA {:.0} MIB {}",
                    runtime.gpu.bytes as f64 / 1048576.,
                    runtime.settings.memory_mib,
                    runtime.gpu.ray_allocated_bytes() as f64 / 1048576.,
                    if runtime.memory_pressure {
                        "MEMORY PRESSURE"
                    } else {
                        ""
                    }
                ),
            ];
            self.stats_time = Instant::now();
        }
        let surface = self.surface.as_ref().unwrap();
        let config = self.config.as_ref().unwrap();
        let frame = match surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                surface.configure(&runtime.gpu.device, config);
                return;
            }
            Err(wgpu::SurfaceError::Timeout) => return,
            Err(error) => {
                self.fail(event_loop, error);
                return;
            }
        };
        let target = frame.texture.create_view(&Default::default());
        runtime.draw(&target);
        if self.menu.open {
            runtime.gpu.menu_renderer.draw(
                &runtime.gpu.device,
                &runtime.gpu.queue,
                &target,
                &self.menu,
                runtime.gpu.width,
                runtime.gpu.height,
                &self.menu_stats,
            );
        }
        frame.present();
        if self.screenshot {
            self.screenshot = false;
            let texture = runtime.gpu.offscreen();
            let target = texture.create_view(&Default::default());
            runtime.draw(&target);
            if self.menu.open {
                runtime.gpu.menu_renderer.draw(
                    &runtime.gpu.device,
                    &runtime.gpu.queue,
                    &target,
                    &self.menu,
                    runtime.gpu.width,
                    runtime.gpu.height,
                    &self.menu_stats,
                );
            }
            let path = self
                .options
                .screenshot
                .as_deref()
                .unwrap_or(std::path::Path::new("output/native_preview.png"));
            if let Err(error) = runtime.gpu.screenshot(&texture, path) {
                eprintln!("Screenshot: {error}");
            }
        }
        self.frame_count += 1;
        if self.last_title.elapsed() > Duration::from_millis(500) {
            let fps = self.frame_count as f64 / self.last_title.elapsed().as_secs_f64();
            self.window.as_ref().unwrap().set_title(&runtime.stats(fps));
            self.last_title = Instant::now();
            self.frame_count = 0;
        }
    }
}
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let result = (|| -> microvoxel::Result<()> {
            let window = Arc::new(
                event_loop.create_window(
                    Window::default_attributes()
                        .with_title("MicroVoxel — loading terrain | Click to walk")
                        .with_inner_size(winit::dpi::LogicalSize::new(
                            self.options.size[0] as f64,
                            self.options.size[1] as f64,
                        )),
                )?,
            );
            let size = window.inner_size();
            let surface = self.instance.create_surface(Arc::clone(&window))?;
            let gpu = pollster::block_on(Gpu::new(
                &self.instance,
                Some(&surface),
                size.width,
                size.height,
            ))?;
            let config = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: gpu.format,
                width: size.width.max(1),
                height: size.height.max(1),
                present_mode: wgpu::PresentMode::AutoVsync,
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: vec![],
            };
            surface.configure(&gpu.device, &config);
            self.runtime = Some(Runtime::new(
                gpu,
                Arc::clone(&self.world),
                self.options.tiles,
                self.nonce,
            )?);
            if self.options.display_explicit {
                self.menu.settings = self.options.display;
            }
            self.runtime
                .as_mut()
                .unwrap()
                .apply_settings(self.menu.settings);
            self.runtime.as_mut().unwrap().set_view(self.options.view);
            if self.options.fly {
                self.runtime.as_mut().unwrap().toggle_flight();
            }
            self.runtime.as_mut().unwrap().gpu.hud_visible = !self.options.no_hud;
            if let Some(camera) = self.options.camera {
                camera.apply(self.runtime.as_mut().unwrap());
                if self.options.camera_ground {
                    camera.ground(self.runtime.as_mut().unwrap())?;
                }
            }
            if let Some(layer) = self.options.map {
                self.runtime
                    .as_mut()
                    .unwrap()
                    .set_environment_map(Some(layer))?;
            }
            self.window = Some(window);
            self.surface = Some(surface);
            self.config = Some(config);
            self.last_frame = Instant::now();
            println!("Click scene to walk; WASD + mouse; Space jump; Shift sprint; Esc settings.\nF5 camera views; F6 debug flight (Space/C vertical).\nWheel/+/- speed; 1/2/3 presets; [/] FOV; R random spot; M environment maps; F11 fullscreen; F12 PNG.");
            Ok(())
        })();
        if let Err(error) = result {
            self.fail(event_loop, error);
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                if let Some(runtime) = &self.runtime {
                    runtime.report();
                }
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    if let (Some(runtime), Some(config), Some(surface)) =
                        (&mut self.runtime, &mut self.config, &self.surface)
                    {
                        runtime.gpu.resize(size.width, size.height);
                        config.width = size.width;
                        config.height = size.height;
                        surface.configure(&runtime.gpu.device, config);
                    }
                }
            }
            WindowEvent::Focused(false) => self.capture(false),
            WindowEvent::CursorMoved { position, .. } if self.menu.open => {
                if let Some(runtime) = &self.runtime {
                    self.menu.cursor(
                        position.x as f32,
                        position.y as f32,
                        runtime.gpu.width,
                        runtime.gpu.height,
                    );
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                if self.menu.open {
                    if self.menu.click(state == ElementState::Pressed) {
                        self.toggle_menu();
                    }
                } else if state == ElementState::Pressed {
                    self.capture(true);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.menu.open {
                    return;
                }
                if let Some(runtime) = &mut self.runtime {
                    let amount = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y as f64,
                        MouseScrollDelta::PixelDelta(p) => p.y / 40.,
                    };
                    runtime.camera.speed =
                        (runtime.camera.speed * 1.25f64.powf(amount)).clamp(0.5, 300.);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(key) = event.physical_key {
                    if key == KeyCode::Escape
                        && event.state == ElementState::Pressed
                        && !event.repeat
                    {
                        self.toggle_menu();
                        return;
                    }
                    if self.menu.open {
                        if event.state == ElementState::Pressed {
                            if key == KeyCode::F12 {
                                self.screenshot = true;
                            } else {
                                self.menu.key(key, event.text.as_deref());
                            }
                        }
                        return;
                    }
                    if event.state == ElementState::Pressed && !event.repeat {
                        match key {
                            KeyCode::Escape => {}
                            KeyCode::F11 => {
                                if let Some(window) = &self.window {
                                    window.set_fullscreen(if window.fullscreen().is_some() {
                                        None
                                    } else {
                                        Some(Fullscreen::Borderless(None))
                                    });
                                }
                            }
                            KeyCode::Space => {
                                if self.grabbed {
                                    if let Some(runtime) = &mut self.runtime {
                                        if !runtime.flying {
                                            runtime.player.request_jump();
                                        }
                                    }
                                }
                            }
                            KeyCode::F12 => self.screenshot = true,
                            KeyCode::F5 => {
                                if let Some(runtime) = &mut self.runtime {
                                    runtime.cycle_view();
                                }
                            }
                            KeyCode::F6 => {
                                if let Some(runtime) = &mut self.runtime {
                                    runtime.toggle_flight();
                                }
                            }
                            KeyCode::KeyM => {
                                self.capture(false);
                                if let Some(runtime) = &mut self.runtime {
                                    if let Err(error) = runtime.cycle_environment_map() {
                                        eprintln!("Map: {error}");
                                    }
                                }
                            }
                            KeyCode::KeyR => {
                                self.nonce += 1;
                                if let Some(runtime) = &mut self.runtime {
                                    if let Err(error) = runtime.respawn(self.nonce) {
                                        eprintln!("Respawn: {error}");
                                    }
                                }
                            }
                            _ => {
                                if let Some(runtime) = &mut self.runtime {
                                    match key {
                                        KeyCode::Equal => {
                                            runtime.camera.speed =
                                                (runtime.camera.speed * 2.).min(300.)
                                        }
                                        KeyCode::Minus => {
                                            runtime.camera.speed =
                                                (runtime.camera.speed / 2.).max(0.5)
                                        }
                                        KeyCode::Digit1 => runtime.camera.speed = 4.,
                                        KeyCode::Digit2 => runtime.camera.speed = 20.,
                                        KeyCode::Digit3 => runtime.camera.speed = 100.,
                                        KeyCode::BracketLeft => {
                                            runtime.camera.fov = (runtime.camera.fov
                                                - 5f64.to_radians())
                                            .max(45f64.to_radians())
                                        }
                                        KeyCode::BracketRight => {
                                            runtime.camera.fov = (runtime.camera.fov
                                                + 5f64.to_radians())
                                            .min(110f64.to_radians())
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    if let Some(runtime) = &mut self.runtime {
                        if event.state == ElementState::Released {
                            runtime.camera.keys.remove(&key);
                        } else if self.grabbed {
                            runtime.camera.keys.insert(key);
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }
    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if self.grabbed {
            if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
                if let Some(runtime) = &mut self.runtime {
                    runtime.camera.rotate(dx, dy);
                }
            }
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            if !window.is_minimized().unwrap_or(false) {
                window.request_redraw();
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(8),
        ));
    }
}
