//! Native viewer for the fitted continuous min-of-quadrics scene.
#[path = "../analytic/envelopes.rs"]
mod envelopes;
#[path = "../analytic/gpu.rs"]
mod gpu;
#[path = "../analytic/scene.rs"]
mod scene;
#[path = "../analytic/traversal.rs"]
mod traversal;
use glam::Vec3;
use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Instant};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, ElementState, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window},
};
struct Camera {
    eye: Vec3,
    yaw: f32,
    pitch: f32,
    speed: f32,
    walking: bool,
}
impl Camera {
    fn new(s: &scene::Scene) -> Self {
        let e = Vec3::from_array(s.extent().map(|v| v as f32));
        let eye = e * Vec3::new(1.35, 1.15, 1.35);
        let d = (e * 0.5 - eye).normalize();
        Self {
            eye,
            yaw: d.z.atan2(d.x),
            pitch: d.y.asin(),
            speed: 8.,
            walking: false,
        }
    }
    fn toggle_walk(&mut self, s: &scene::Scene, mode: u32) {
        if self.walking {
            self.walking = false;
            return;
        }
        let e = s.extent();
        let x = self.eye.x.clamp(0.01, e[0] as f32 - 0.01);
        let z = self.eye.z.clamp(0.01, e[2] as f32 - 0.01);
        if let Some(h) = s.ground_height(x as f64, z as f64, mode >= 4) {
            self.eye = Vec3::new(x, h as f32 + 1.7, z);
            self.walking = true;
            self.speed = 4.3;
        }
    }
    fn walk(&mut self, s: &scene::Scene, dt: f32, keys: &HashSet<KeyCode>, mode: u32) {
        let f = Vec3::new(self.yaw.cos(), 0., self.yaw.sin());
        let r = f.cross(Vec3::Y);
        let mut direction = Vec3::ZERO;
        for (k, v) in [
            (KeyCode::KeyW, f),
            (KeyCode::KeyS, -f),
            (KeyCode::KeyD, r),
            (KeyCode::KeyA, -r),
        ] {
            if keys.contains(&k) {
                direction += v;
            }
        }
        let distance = dt
            * self.speed.min(10.)
            * if keys.contains(&KeyCode::ShiftLeft) {
                1.6
            } else {
                1.
            };
        let steps = (distance / 0.15).ceil().max(1.) as usize;
        let delta = direction.normalize_or_zero() * distance / steps as f32;
        for _ in 0..steps {
            let next = self.eye + delta;
            if let Some(h) = s.ground_height(next.x as f64, next.z as f64, mode >= 4) {
                let target = h as f32 + 1.7;
                // Surface-walking preview: reject cliffs, allow small voxel steps.
                if direction == Vec3::ZERO || (target - self.eye.y).abs() <= delta.length() + 0.15 {
                    self.eye = Vec3::new(next.x, target, next.z);
                }
            }
        }
    }
    fn frame(&self, s: &scene::Scene, w: u32, h: u32, mode: u32) -> gpu::Frame {
        let f = Vec3::new(
            self.pitch.cos() * self.yaw.cos(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.sin(),
        );
        let r = f.cross(Vec3::Y).normalize();
        let u = r.cross(f);
        let e = s.extent().map(|v| v as f32);
        gpu::Frame {
            eye: self.eye.extend((65f32.to_radians() / 2.).tan()).to_array(),
            forward: f.extend(w as f32 / h as f32).to_array(),
            right: r.extend(0.).to_array(),
            up: u.extend(s.voxel_m as f32).to_array(),
            grid: [
                s.patches_per_axis[0] as u32,
                s.patches_per_axis[1] as u32,
                s.patches_per_axis[2] as u32,
                s.quadratics_per_patch as u32,
            ],
            scene: [e[0], e[1], e[2], s.patch_edge_m as f32],
            options: [w, h, mode, 1],
        }
    }
    fn move_keys(&mut self, keys: &HashSet<KeyCode>, dt: f32) {
        let f = Vec3::new(
            self.pitch.cos() * self.yaw.cos(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.sin(),
        );
        let r = f.cross(Vec3::Y).normalize();
        let mut v = Vec3::ZERO;
        for (k, d) in [
            (KeyCode::KeyW, f),
            (KeyCode::KeyS, -f),
            (KeyCode::KeyD, r),
            (KeyCode::KeyA, -r),
            (KeyCode::Space, Vec3::Y),
            (KeyCode::ControlLeft, -Vec3::Y),
        ] {
            if keys.contains(&k) {
                v += d;
            }
        }
        self.eye += v.normalize_or_zero()
            * dt
            * self.speed
            * if keys.contains(&KeyCode::ShiftLeft) {
                4.
            } else {
                1.
            };
    }
}
struct App {
    scene: scene::Scene,
    camera: Camera,
    gpu: Option<gpu::Gpu>,
    window: Option<Arc<Window>>,
    surface: Option<wgpu::Surface<'static>>,
    config: Option<wgpu::SurfaceConfiguration>,
    keys: HashSet<KeyCode>,
    captured: bool,
    menu: bool,
    cursor: [f64; 2],
    slider_drag: Option<usize>,
    initial_traversal: u32,
    last: Instant,
    title: Instant,
    mode: u32,
    frames: u32,
}
impl App {
    fn set_slider(&mut self, row: usize, x: f64) {
        let value = ((x - 36.) / 368.).clamp(0., 1.) as f32;
        if let Some(g) = &mut self.gpu {
            if row == 8 {
                g.temporal_strength = value;
            } else if row == 9 {
                g.face_smoothing = value;
            } else {
                g.spatial_samples = 1 + (value * 3.).round() as u32;
            }
        }
    }

    fn grab(&mut self, on: bool) {
        if let Some(w) = &self.window {
            if on {
                self.captured = w
                    .set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| w.set_cursor_grab(CursorGrabMode::Confined))
                    .is_ok();
            } else {
                let _ = w.set_cursor_grab(CursorGrabMode::None);
                self.captured = false;
            }
            w.set_cursor_visible(!self.captured);
        }
    }
}
impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let w = Arc::new(
            el.create_window(
                Window::default_attributes()
                    .with_title("Analytic terrain — click to fly; Esc opens rendering options")
                    .with_inner_size(winit::dpi::PhysicalSize::new(1280, 720)),
            )
            .unwrap(),
        );
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(w.clone()).unwrap();
        let size = w.inner_size();
        let mut g = match pollster::block_on(gpu::Gpu::new(
            &instance,
            Some(&surface),
            &self.scene,
            size.width,
            size.height,
        )) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("{e}");
                el.exit();
                return;
            }
        };
        g.traversal_mode = self.initial_traversal;
        let c = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: g.format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&g.device, &c);
        self.window = Some(w);
        self.surface = Some(surface);
        self.config = Some(c);
        self.gpu = Some(g);
        self.last = Instant::now();
    }
    fn device_event(&mut self, _: &ActiveEventLoop, _: winit::event::DeviceId, event: DeviceEvent) {
        if self.captured {
            if let DeviceEvent::MouseMotion { delta } = event {
                self.camera.yaw += delta.0 as f32 * 0.002;
                self.camera.pitch = (self.camera.pitch - delta.1 as f32 * 0.002).clamp(-1.55, 1.55);
            }
        }
    }
    fn window_event(
        &mut self,
        el: &ActiveEventLoop,
        _: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Focused(false) => {
                self.keys.clear();
                self.slider_drag = None;
                self.grab(false);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } => {
                if self.menu {
                    let [x, y] = self.cursor;
                    if (436.0..806.0).contains(&x) && (64.0..196.0).contains(&y) {
                        if let Some(g) = &mut self.gpu {
                            g.traversal_mode = ((y - 20.) / 44.) as u32 - 1;
                        }
                    }
                    if (20.0..420.0).contains(&x) && (64.0..548.0).contains(&y) {
                        let row = ((y - 20.) / 44.) as usize;
                        if row >= 1 && row <= 5 {
                            self.mode = [0, 3, 4, 1, 2][row - 1];
                        } else if row == 6 {
                            self.camera.toggle_walk(&self.scene, self.mode);
                        } else if row == 7 {
                            if let Some(g) = &mut self.gpu {
                                g.aa_enabled = !g.aa_enabled;
                            }
                        } else if row == 8 || row == 9 || row == 10 {
                            self.slider_drag = Some(row);
                            self.set_slider(row, x);
                        } else if row == 11 {
                            self.menu = false;
                            self.grab(true);
                        }
                    }
                } else {
                    self.grab(true);
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                ..
            } => self.slider_drag = None,
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = [position.x, position.y];
                if self.menu {
                    if let Some(row) = self.slider_drag {
                        self.set_slider(row, position.x);
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let y = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.,
                };
                self.camera.speed = (self.camera.speed * 1.2f32.powf(y)).clamp(0.1, 1000.);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(k) = event.physical_key {
                    if event.state == ElementState::Pressed {
                        self.keys.insert(k);
                        if !event.repeat {
                            match k {
                                KeyCode::Escape => {
                                    self.menu = !self.menu;
                                    self.slider_drag = None;
                                    self.keys.clear();
                                    self.grab(!self.menu);
                                }
                                KeyCode::KeyT => {
                                    if let Some(g) = &mut self.gpu {
                                        g.aa_enabled = !g.aa_enabled;
                                    }
                                }
                                KeyCode::KeyG => self.camera.toggle_walk(&self.scene, self.mode),
                                KeyCode::Home => self.camera = Camera::new(&self.scene),
                                KeyCode::Digit1 => self.mode = 0,
                                KeyCode::Digit2 => self.mode = 1,
                                KeyCode::Digit3 => self.mode = 2,
                                KeyCode::Digit4 => self.mode = 3,
                                KeyCode::Digit5 => self.mode = 4,
                                KeyCode::Digit6 => self.mode = 5,
                                KeyCode::F12 => {
                                    if let Some(g) = &self.gpu {
                                        println!(
                                            "Capture: {:?}",
                                            g.capture(std::path::Path::new(
                                                "output/analytic/camera.png"
                                            ))
                                        );
                                    }
                                }
                                _ => {}
                            }
                        }
                    } else {
                        self.keys.remove(&k);
                    }
                }
            }
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    if let (Some(g), Some(c), Some(s)) =
                        (&mut self.gpu, &mut self.config, &self.surface)
                    {
                        c.width = size.width;
                        c.height = size.height;
                        s.configure(&g.device, c);
                        g.resize(size.width, size.height);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.last).as_secs_f32().min(0.1);
                self.last = now;
                if !self.menu {
                    if self.camera.walking {
                        self.camera.walk(&self.scene, dt, &self.keys, self.mode);
                    } else {
                        self.camera.move_keys(&self.keys, dt);
                    }
                }
                if let (Some(g), Some(s)) = (&mut self.gpu, &self.surface) {
                    match s.get_current_texture() {
                        Ok(t) => {
                            let view = t.texture.create_view(&Default::default());
                            let mut frame =
                                self.camera.frame(&self.scene, g.width, g.height, self.mode);
                            frame.right[3] = if self.menu { 1. } else { 0. };
                            g.draw(&frame, Some(&view));
                            t.present();
                        }
                        Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                            s.configure(&g.device, self.config.as_ref().unwrap())
                        }
                        Err(wgpu::SurfaceError::OutOfMemory) => el.exit(),
                        Err(_) => {}
                    }
                    self.frames += 1;
                    if self.title.elapsed().as_secs_f32() > 0.5 {
                        let fps = self.frames as f32 / self.title.elapsed().as_secs_f32();
                        self.window.as_ref().unwrap().set_title(&format!("Analytic | {fps:.0} FPS | GPU {:.2} ms | CPU {:.2} ms | visits {:.1}, Q {:.1}, bounds {:.1} | speed {:.1} m/s | T AA | G walk/fly | Esc options | mode {} | F12 capture",g.compute_ms.unwrap_or(0.),g.cpu_encode_ms,g.average_patches,g.average_quads,g.average_envelopes,self.camera.speed,["continuous","patch heatmap","quadratic heatmap","snapped approximate","analytic voxels","voxel reference"][self.mode as usize]));
                        self.frames = 0;
                        self.title = Instant::now();
                    }
                }
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}
fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let val = |key: &str| {
        args.iter()
            .position(|a| a == key)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if args.iter().any(|a| a == "--help") {
        println!("analytic [--scene terrain.json] [--headless --size 640x360 --frames 60 --capture output/analytic/preview.png --verify] [--camera x,y,z,yaw,pitch] [--mode shaded|snapped|voxels|patches|quadratics]\n--walk: start on ground with 1.7 m eye height; --speed m/s; G: walk/fly; --traversal grid|hierarchy|envelopes; T: toggle anti-aliasing; --no-aa: disable in headless captures; Click: capture mouse; WASD fly; Space/Ctrl up/down; Shift boost; wheel speed; Esc menu; Home reset; F12 screenshot; 1/2/3 diagnostic views.");
        return Ok(());
    }
    let path = val("--scene").unwrap_or_else(|| "output/fit-occupancy/terrain.json".into());
    let scene = scene::Scene::load(&path)?;
    println!(
        "Scene: {:?} m, global origin {:?} m",
        scene.extent(),
        scene.origin()
    );
    let mut camera = Camera::new(&scene);
    if let Some(c) = val("--camera") {
        let a: Vec<f32> = c
            .split(',')
            .map(|n| n.parse().map_err(|_| "Invalid camera".to_string()))
            .collect::<Result<_, _>>()?;
        if a.len() != 5 || a.iter().any(|x| !x.is_finite()) {
            return Err("Camera requires x,y,z,yaw,pitch in local meters/degrees".into());
        }
        camera.eye = Vec3::new(a[0], a[1], a[2]);
        camera.yaw = a[3].to_radians();
        camera.pitch = a[4].to_radians().clamp(-1.55, 1.55);
    }
    if let Some(speed) = val("--speed") {
        camera.speed = speed.parse().map_err(|_| "Invalid speed")?;
        if !camera.speed.is_finite() || camera.speed <= 0. || camera.speed > 1000. {
            return Err("Speed must be greater than zero and at most 1000 m/s".into());
        }
    }
    let mode = match val("--mode").as_deref() {
        Some("voxels") => 4,
        Some("voxel-reference") => 5,
        Some("snapped") => 3,
        Some("patches") => 1,
        Some("quadratics") => 2,
        _ => 0,
    };
    if args.iter().any(|a| a == "--walk") {
        camera.toggle_walk(&scene, mode);
    }
    let initial_traversal = match val("--traversal").as_deref() {
        None | Some("grid") => 0,
        Some("hierarchy") => 1,
        Some("envelopes") => 2,
        Some("hardware") => {
            return Err("Hardware procedural traversal unavailable in wgpu 24".into())
        }
        _ => return Err("Traversal must be grid, hierarchy or envelopes".into()),
    };
    if args.iter().any(|a| a == "--headless") {
        let size = val("--size").unwrap_or_else(|| "640x360".into());
        let (w, h) = size.split_once('x').ok_or("Size must be WIDTHxHEIGHT")?;
        let w: u32 = w.parse().map_err(|_| "Invalid width")?;
        let h: u32 = h.parse().map_err(|_| "Invalid height")?;
        if w == 0 || h == 0 || w > 8192 || h > 8192 {
            return Err("Size must be 1..8192".into());
        }
        let frames: u32 = val("--frames")
            .unwrap_or_else(|| "60".into())
            .parse()
            .map_err(|_| "Invalid frame count")?;
        let instance = wgpu::Instance::default();
        let start = Instant::now();
        let mut g = pollster::block_on(gpu::Gpu::new(&instance, None, &scene, w, h))?;
        g.aa_enabled = !args.iter().any(|a| a == "--no-aa" || a == "--verify");
        g.spatial_samples = val("--samples")
            .unwrap_or_else(|| "1".into())
            .parse()
            .map_err(|_| "Invalid sample count")?;
        if !(1..=4).contains(&g.spatial_samples) {
            return Err("Samples must be 1..4".into());
        }
        if args.iter().any(|a| a == "--verify") {
            g.spatial_samples = 1;
        }
        g.traversal_mode = initial_traversal;
        let setup = start.elapsed().as_secs_f64();
        let motion: Vec<f32> = val("--camera-step")
            .unwrap_or_else(|| "0,0,0".into())
            .split(',')
            .map(|v| v.parse().map_err(|_| "Invalid camera step"))
            .collect::<Result<_, _>>()?;
        if motion.len() != 3 || motion.iter().any(|v| !v.is_finite()) {
            return Err("Camera step requires x,y,z meters/frame".into());
        }
        let step = Vec3::new(motion[0], motion[1], motion[2]);
        let mut f = camera.frame(&scene, w, h, mode);
        let start = Instant::now();
        for i in 0..frames.max(1) {
            if i > 0 {
                camera.eye += step;
            }
            f = camera.frame(&scene, w, h, mode);
            g.draw(&f, None);
            g.device.poll(wgpu::Maintain::Wait);
        }
        g.poll();
        let sync_ms = start.elapsed().as_secs_f64() * 1000. / frames.max(1) as f64;
        let capture =
            PathBuf::from(val("--capture").unwrap_or_else(|| "output/analytic/preview.png".into()));
        g.capture(&capture)?;
        let mut mismatches = 0;
        let mut hits = 0;
        let mut max_error = 0f64;
        let mut checked = 0;
        if args.iter().any(|a| a == "--verify") {
            let probes = g.read_probes()?;
            let o = camera.eye.to_array().map(f64::from);
            for (iy, row) in probes.chunks(w.div_ceil(32) as usize).enumerate() {
                for (ix, p) in row.iter().enumerate() {
                    let x = ix as f32 * 32. + 0.5;
                    let y = iy as f32 * 32. + 0.5;
                    let d = (Vec3::from_array(f.forward[..3].try_into().unwrap())
                        + Vec3::from_array(f.right[..3].try_into().unwrap())
                            * ((2. * x / w as f32 - 1.) * f.eye[3] * f.forward[3])
                        + Vec3::from_array(f.up[..3].try_into().unwrap())
                            * ((1. - 2. * y / h as f32) * f.eye[3]))
                        .normalize();
                    let cpu = if mode >= 4 {
                        scene.reference_voxels(o, d.to_array().map(f64::from))
                    } else {
                        scene.reference(o, d.to_array().map(f64::from))
                    };
                    checked += 1;
                    match (cpu, p[0] >= 0.) {
                        (Some(t), true) => {
                            hits += 1;
                            let e = (t - p[0] as f64).abs();
                            max_error = max_error.max(e);
                            if e > 0.002 {
                                mismatches += 1;
                            }
                        }
                        (None, false) => {}
                        _ => mismatches += 1,
                    }
                }
            }
        }
        let report = serde_json::json!({"traversal":g.traversal_mode,"spatial_samples":g.spatial_samples,"anti_aliasing":g.aa_enabled,"camera_end":camera.eye.to_array(),"adapter":g.adapter_name,"scene":path,"render_mode":mode,"voxel_cm":scene.voxel_m*100.,"size":[w,h],"frames":frames,"setup_seconds":setup,"synchronized_frame_ms":sync_ms,"gpu_compute_ms":g.compute_ms,"cpu_encode_ms":g.cpu_encode_ms,"envelope_bytes":g.envelope_bytes,"bounds_build_ms":g.bounds_build_ms,"coefficient_bytes":g.coefficient_bytes,"average_traversal_visits":g.average_patches,"average_envelope_tests":g.average_envelopes,"average_quadratic_tests":g.average_quads,"verification":{"rays":checked,"hits":hits,"mismatches":mismatches,"max_hit_error_m":max_error},"capture":capture});
        let out = capture.with_extension("json");
        std::fs::write(out, serde_json::to_vec_pretty(&report).unwrap())
            .map_err(|e| e.to_string())?;
        println!("{report}");
        if mismatches > 0 {
            return Err("GPU/CPU reference mismatch (tolerance 2 mm); see report".into());
        }
        return Ok(());
    }
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut app = App {
        scene,
        camera,
        gpu: None,
        window: None,
        surface: None,
        config: None,
        keys: HashSet::new(),
        captured: false,
        menu: false,
        cursor: [0.; 2],
        slider_drag: None,
        initial_traversal,
        last: Instant::now(),
        title: Instant::now(),
        mode,
        frames: 0,
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod walking_tests {
    use super::*;
    #[test]
    fn walking_tracks_ground_and_ignores_camera_pitch() {
        // Analytic plane fixture, independent of downloaded assets.
        let dir = std::env::temp_dir().join(format!("analytic-walk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("scene.json");
        let model = serde_json::json!({"version":1,"representation":"min-of-implicit-quadrics","solid":"min(Q)>0","feature_order":["1","x","y","z","xx","yy","zz","xy","xz","yz"],"patch_order":"y,z,x","patches_per_axis":[1,1,1],"quadratics_per_patch":1,"patch_edge_m":8.,"origin_cells":[0,0,0],"voxel_m":0.0625,"dimensions":[128,128,128],"normalization":"local = 2*(world_m-patch_min_m)/patch_edge_m - 1","coefficients":[[[[[0.,0.25,-1.,0.,0.,0.,0.,0.,0.,0.]]]]]});
        std::fs::write(&file, model.to_string()).unwrap();
        let s = scene::Scene::load(file.to_str().unwrap()).unwrap();
        assert_eq!(s.ground_height(4., 4., false), Some(4.));
        assert_eq!(s.ground_height(-1., 4., false), None);
        let mut c = Camera::new(&s);
        c.eye = Vec3::new(4., 7., 4.);
        c.yaw = 0.;
        c.pitch = 1.5;
        c.toggle_walk(&s, 0);
        assert!(c.walking);
        assert!((c.eye.y - 5.7).abs() < 1e-5);
        c.walk(&s, 0.1, &HashSet::from([KeyCode::KeyW]), 0);
        assert!((c.eye.x - 4.43).abs() < 1e-5);
        assert!(
            (c.eye.y - (s.ground_height(c.eye.x as f64, 4., false).unwrap() as f32 + 1.7)).abs()
                < 1e-5
        );
        c.toggle_walk(&s, 0);
        assert!(!c.walking);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
