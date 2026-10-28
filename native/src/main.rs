//! Native application shell. Window/input, GPU resources, streaming and camera
//! are separate modules; authoritative world code stays in the shared library.
mod benchmark;
mod camera;
mod gpu;
mod runtime;
mod streaming;
use gpu::Gpu;
use microvoxel::{generation::ProceduralTerrain, World};
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
    seed: u64,
    headless: bool,
    screenshot: Option<PathBuf>,
    frames: u32,
    tiles: usize,
    benchmark: bool,
    fly_test: bool,
    edit_save: Option<PathBuf>,
}
impl Options {
    fn read() -> microvoxel::Result<Self> {
        let mut options = Self {
            seed: 42,
            headless: false,
            screenshot: None,
            frames: 30,
            tiles: 384,
            benchmark: false,
            fly_test: false,
            edit_save: None,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--seed" => options.seed = args.next().ok_or("--seed needs an integer")?.parse()?,
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
                    println!("microvoxel-game [--seed 42] [--tiles 384] [--save edits.mvw]\n  --benchmark       Compare exact and coarse surfaces over the same 16 m patch\n  --headless        Render offscreen and exit after --frames (default 30)\n  --screenshot PATH Save a PNG (headless on completion, window on F12)\n  --fly-test        Exercise grid flight and LOD replacement in headless mode\nControls: click to capture mouse; WASD; Space/C; Shift boost; Esc release\nMouse wheel or +/-: speed; 1/2/3: 4/20/100 m/s; [ / ]: FOV\nR: random location; F11: fullscreen; F12: save PNG; close window to quit");
                    std::process::exit(0);
                }
                _ => return Err(format!("Unknown argument {arg}; use --help").into()),
            }
        }
        if !(64..=2048).contains(&options.tiles) || !(1..=10000).contains(&options.frames) {
            return Err("--tiles must be 64..2048; --frames must be 1..10000".into());
        }
        Ok(options)
    }
}
fn main() -> microvoxel::Result<()> {
    let options = Options::read()?;
    let start = Instant::now();
    println!("Preparing the 10 km world (seed {})...", options.seed);
    let mut world = World::new(ProceduralTerrain::new(options.seed));
    if let Some(path) = &options.edit_save {
        world.load_edits(path)?;
    }
    let world = Arc::new(world);
    println!("World ready in {:.2}s", start.elapsed().as_secs_f64());
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
    let gpu = pollster::block_on(Gpu::new(&instance, None, 1280, 720))?;
    let mut runtime = Runtime::new(gpu, world, options.tiles, 0);
    let deadline = Instant::now() + Duration::from_secs(180);
    let start = Instant::now();
    wait_for_cover(&mut runtime, deadline)?;
    println!(
        "Stitched cover ready in {:.2}s",
        start.elapsed().as_secs_f64()
    );
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
        // Let the normal 300 ms planning cadence observe the changed position.
        std::thread::sleep(Duration::from_millis(310));
        runtime.update(0.);
        wait_for_cover(&mut runtime, deadline)?;
        println!(
            "Grid flight: {:?} -> {:?}; replacement cover complete",
            before, runtime.camera.cell
        );
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
            .unwrap_or(std::path::Path::new("native_preview.png")),
    )?;
    Ok(())
}
fn wait_for_cover(runtime: &mut Runtime, deadline: Instant) -> microvoxel::Result<()> {
    while !runtime.settled() {
        runtime.update(0.);
        if let Some(error) = &runtime.last_error {
            return Err(error.clone().into());
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
    last_frame: Instant,
    last_title: Instant,
    frame_count: u32,
    nonce: u64,
    screenshot: bool,
    error: Option<String>,
}
impl App {
    fn capture(&mut self, grab: bool) {
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
            runtime.camera.clear();
        }
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
        runtime.update(dt);
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
        runtime.draw(&frame.texture.create_view(&Default::default()));
        frame.present();
        if self.screenshot {
            self.screenshot = false;
            let texture = runtime.gpu.offscreen();
            runtime.draw(&texture.create_view(&Default::default()));
            let path = self
                .options
                .screenshot
                .as_deref()
                .unwrap_or(std::path::Path::new("native_preview.png"));
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
                        .with_title("MicroVoxel — loading terrain | Click to fly")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280., 720.)),
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
            ));
            self.window = Some(window);
            self.surface = Some(surface);
            self.config = Some(config);
            self.last_frame = Instant::now();
            println!("Click scene to fly; WASD + mouse; Space/C; Shift boost; Esc release.\nWheel/+/- speed; 1/2/3 presets; [/] FOV; R random spot; F11 fullscreen; F12 PNG.");
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
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.capture(true),
            WindowEvent::MouseWheel { delta, .. } => {
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
                    if event.state == ElementState::Pressed && !event.repeat {
                        match key {
                            KeyCode::Escape => self.capture(false),
                            KeyCode::F11 => {
                                if let Some(window) = &self.window {
                                    window.set_fullscreen(if window.fullscreen().is_some() {
                                        None
                                    } else {
                                        Some(Fullscreen::Borderless(None))
                                    });
                                }
                            }
                            KeyCode::F12 => self.screenshot = true,
                            KeyCode::KeyR => {
                                self.nonce += 1;
                                if let Some(runtime) = &mut self.runtime {
                                    runtime.camera = camera::Camera::spawn(&self.world, self.nonce);
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
