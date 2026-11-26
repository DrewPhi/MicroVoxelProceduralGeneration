use super::{
    cache::{Cache, Workers},
    classify,
    gpu::Gpu,
    millis, Options, Result,
};
use frontier_world::{FbmParams, FbmTerrain};
use glam::{DQuat, DVec3};
use std::{
    sync::{mpsc, Arc, Mutex},
    time::Instant,
};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

#[derive(Clone, Copy)]
struct Camera {
    eye: DVec3,
    yaw: f64,
    pitch: f64,
}
impl Camera {
    fn forward(self) -> DVec3 {
        DQuat::from_rotation_y(self.yaw) * DVec3::new(0., self.pitch.sin(), -self.pitch.cos())
    }
}

enum Status {
    Loading(String),
    Fatal(String),
}
struct Viewer {
    start: Camera,
    current: Arc<Mutex<Camera>>,
    window: Option<Arc<Window>>,
    options: Option<Options>,
    wake: mpsc::SyncSender<()>,
    receive: Option<mpsc::Receiver<()>>,
    status: EventLoopProxy<Status>,
    fast: bool,
}

pub fn run(options: Options) -> Result<()> {
    eprintln!(
        "WASD move, arrow keys look, Space/Ctrl climb/descend, Shift faster, Home reset, Esc exit"
    );
    let (eye, forward, _, _) = options.pose(0);
    let initial = Camera {
        eye,
        yaw: (-forward.x).atan2(-forward.z),
        pitch: forward.y.asin(),
    };
    let current = Arc::new(Mutex::new(initial));
    let event = EventLoop::with_user_event().build()?;
    let status = event.create_proxy();
    let (wake, receive) = mpsc::sync_channel(1);
    event.run_app(&mut Viewer {
        start: initial,
        current,
        window: None,
        options: Some(options),
        wake,
        receive: Some(receive),
        status,
        fast: false,
    })?;
    Ok(())
}

impl ApplicationHandler<Status> for Viewer {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let Some(options) = self.options.take() else {
            return;
        };
        let attributes = Window::default_attributes()
            .with_title("Frontier exact ground · preparing terrain")
            .with_inner_size(LogicalSize::new(
                options.width.max(640),
                options.height.max(360),
            ));
        let window = match el.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("{error}");
                el.exit();
                return;
            }
        };
        self.window = Some(window.clone());
        let current = self.current.clone();
        let status = self.status.clone();
        let receive = self.receive.take().unwrap();
        std::thread::spawn(move || {
            if let Err(error) = render_loop(options, window, current, receive, &status) {
                let _ = status.send_event(Status::Fatal(error.to_string()));
            }
        });
        let _ = self.wake.try_send(());
    }
    fn user_event(&mut self, el: &ActiveEventLoop, event: Status) {
        match event {
            Status::Loading(title) => {
                if let Some(win) = &self.window {
                    win.set_title(&title);
                }
            }
            Status::Fatal(reason) => {
                eprintln!("Exact preview: {reason}");
                el.exit();
            }
        }
    }
    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::ModifiersChanged(modifiers) => self.fast = modifiers.state().shift_key(),
            WindowEvent::Resized(_) => {
                let _ = self.wake.try_send(());
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let PhysicalKey::Code(key) = event.physical_key else {
                    return;
                };
                let mut view = self.current.lock().unwrap();
                let horizontal = view.forward().with_y(0.).normalize();
                let right = horizontal.cross(DVec3::Y);
                let step = if self.fast { 3.0 } else { 0.75 };
                match key {
                    KeyCode::Escape => {
                        el.exit();
                        return;
                    }
                    KeyCode::Home => *view = self.start,
                    KeyCode::KeyW => view.eye += horizontal * step,
                    KeyCode::KeyS => view.eye -= horizontal * step,
                    KeyCode::KeyD => view.eye += right * step,
                    KeyCode::KeyA => view.eye -= right * step,
                    KeyCode::Space => view.eye.y += step,
                    KeyCode::ControlLeft | KeyCode::ControlRight => view.eye.y -= step,
                    KeyCode::ArrowRight => view.yaw -= 0.04,
                    KeyCode::ArrowLeft => view.yaw += 0.04,
                    KeyCode::ArrowUp => view.pitch = (view.pitch + 0.04).min(1.48),
                    KeyCode::ArrowDown => view.pitch = (view.pitch - 0.04).max(-1.48),
                    _ => return,
                }
                let _ = self.wake.try_send(());
            }
            _ => (),
        }
    }
}

fn render_loop(
    o: Options,
    window: Arc<Window>,
    current: Arc<Mutex<Camera>>,
    receive: mpsc::Receiver<()>,
    status: &EventLoopProxy<Status>,
) -> Result<()> {
    let terrain = FbmTerrain::new(
        o.seed,
        FbmParams {
            recipe: o.recipe,
            ..Default::default()
        },
    )?;
    let workers = Workers::new(&terrain, o.workers, o.page_side, o.land_bound);
    let capacity = o.cache_mib * 1024 * 256;
    let mut cache = Cache::new(o.eye.to_array(), o.page_side, capacity, o.node_limit);
    let mut gpu = pollster::block_on(Gpu::new(
        o.width,
        o.height,
        o.node_limit,
        capacity,
        o.software,
        o.stackless,
        Some(window),
    ))?;
    gpu.bounds(&cache);
    let mut shown = false;
    while receive.recv().is_ok() {
        let start = Instant::now();
        let mut pose = *current.lock().unwrap();
        for round in 0..o.rounds {
            if receive.try_recv().is_ok() {
                pose = *current.lock().unwrap();
            }
            let eye = pose.eye.with_y(
                pose.eye
                    .y
                    .max(terrain.height_m(pose.eye.x, pose.eye.z) + 1.7),
            );
            if !cache.cover(eye.to_array(), o.far) {
                cache = Cache::new(eye.to_array(), o.page_side, capacity, o.node_limit);
                gpu.bounds(&cache);
            }
            let forward = pose.forward();
            let right = forward.cross(DVec3::Y).normalize();
            let view = o.view_from_pose(&cache, (eye, forward, right, right.cross(forward)));
            let (pixels, time) = gpu.render(&view)?;
            let (requests, pending, failed) = classify(&pixels, &cache)?;
            if pending == 0 && failed == 0 {
                gpu.present()?;
                if !shown {
                    eprintln!(
                        "Interactive first complete exact frame {:.1} ms",
                        millis(start)
                    );
                }
                shown = true;
                let _ = status.send_event(Status::Loading(format!(
                    "Frontier exact ground · {:.0} m view · {time:.1} ms GPU",
                    o.far
                )));
                break;
            }
            if failed > 0 {
                return Err(format!("{failed} visibility queries failed").into());
            }
            if round % 5 == 0 {
                let _ = status.send_event(Status::Loading(format!(
                    "Frontier exact ground · streaming {pending} pixels"
                )));
            }
            let data = cache.fill(&workers, &requests, o.batch.min(1024), o.prefetch);
            if data.jobs == 0 || round + 1 == o.rounds {
                let _ = status.send_event(Status::Loading(format!(
                    "Frontier exact ground · {pending} pixels awaiting pages"
                )));
                break;
            }
            gpu.publish(&cache, &data);
        }
    }
    Ok(())
}
