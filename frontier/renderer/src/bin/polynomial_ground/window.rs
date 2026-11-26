use crate::{run, Options, Result};
use glam::DVec3;
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

type Pose = (DVec3, DVec3, DVec3, DVec3);

pub fn play(options: Options) -> Result<()> {
    let event_loop = EventLoop::<String>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    event_loop.run_app(&mut App {
        options: Some(options),
        proxy,
        shared: Arc::new(Shared::default()),
    })?;
    Ok(())
}

#[derive(Default)]
struct Shared {
    keys: Mutex<HashSet<KeyCode>>,
    close: AtomicBool,
    capture: AtomicBool,
}

struct App {
    options: Option<Options>,
    proxy: EventLoopProxy<String>,
    shared: Arc<Shared>,
}

impl ApplicationHandler<String> for App {
    fn resumed(&mut self, events: &ActiveEventLoop) {
        let Some(options) = self.options.take() else {
            return;
        };
        let window = match events.create_window(
            Window::default_attributes()
                .with_title("Frontier · preparing native terrain")
                .with_inner_size(PhysicalSize::new(options.width, options.height)),
        ) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("{error}");
                events.exit();
                return;
            }
        };
        let initial = (options.aim - options.eye).normalize();
        let preview = Preview {
            window,
            shared: self.shared.clone(),
            start: options.eye,
            eye: options.eye,
            yaw: initial.x.atan2(initial.z),
            pitch: initial.y.asin(),
            initial_yaw: initial.x.atan2(initial.z),
            initial_pitch: initial.y.asin(),
            update: Instant::now(),
        };
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let outcome = pollster::block_on(run(options, Some(preview)));
            let message = outcome.err().map_or(String::new(), |e| e.to_string());
            let _ = proxy.send_event(message);
        });
    }

    fn user_event(&mut self, events: &ActiveEventLoop, message: String) {
        if !message.is_empty() {
            eprintln!("{message}");
        }
        events.exit();
    }

    fn window_event(&mut self, events: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.shared.close.store(true, Ordering::Relaxed);
                events.exit();
            }
            WindowEvent::Focused(false) => self.shared.keys.lock().unwrap().clear(),
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(key) = event.physical_key else {
                    return;
                };
                if event.state == ElementState::Pressed {
                    match key {
                        KeyCode::Escape => {
                            self.shared.close.store(true, Ordering::Relaxed);
                            events.exit();
                        }
                        KeyCode::F12 if !event.repeat => {
                            self.shared.capture.store(true, Ordering::Relaxed)
                        }
                        _ => {
                            self.shared.keys.lock().unwrap().insert(key);
                        }
                    }
                } else {
                    self.shared.keys.lock().unwrap().remove(&key);
                }
            }
            _ => (),
        }
    }
}

pub(crate) struct Preview {
    pub window: Arc<Window>,
    shared: Arc<Shared>,
    start: DVec3,
    eye: DVec3,
    yaw: f64,
    pitch: f64,
    initial_yaw: f64,
    initial_pitch: f64,
    update: Instant,
}

impl Preview {
    pub fn closing(&self) -> bool {
        self.shared.close.load(Ordering::Relaxed)
    }

    pub fn capture(&self) -> bool {
        self.shared.capture.swap(false, Ordering::Relaxed)
    }

    pub fn pose(&mut self, speed: f64) -> Pose {
        let now = Instant::now();
        let dt = (now - self.update).as_secs_f64().min(0.05);
        self.update = now;
        let keys = self.shared.keys.lock().unwrap();
        let axis = |plus, minus| {
            f64::from(u8::from(keys.contains(&plus))) - f64::from(u8::from(keys.contains(&minus)))
        };
        if keys.contains(&KeyCode::Home) {
            self.eye = self.start;
            self.yaw = self.initial_yaw;
            self.pitch = self.initial_pitch;
        }
        self.yaw += axis(KeyCode::ArrowLeft, KeyCode::ArrowRight) * dt;
        self.pitch =
            (self.pitch + axis(KeyCode::ArrowUp, KeyCode::ArrowDown) * dt).clamp(-1.53, 1.53);
        let forward = DVec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.cos() * self.pitch.cos(),
        );
        let right = forward.cross(DVec3::Y).normalize();
        let motion = forward * axis(KeyCode::KeyW, KeyCode::KeyS)
            + right * axis(KeyCode::KeyD, KeyCode::KeyA)
            + DVec3::Y * axis(KeyCode::Space, KeyCode::ControlLeft);
        let fast = if keys.contains(&KeyCode::ShiftLeft) {
            2.
        } else {
            1.
        };
        self.eye += motion.normalize_or_zero() * dt * speed * fast;
        (self.eye, forward, right, right.cross(forward))
    }
}

pub(crate) struct Display {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    binding: wgpu::BindGroup,
}

impl Display {
    pub fn new(
        surface: wgpu::Surface<'static>,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        preview: &Preview,
        camera: &wgpu::Buffer,
        pixels: &wgpu::Buffer,
    ) -> Result<Self> {
        let size = preview.window.inner_size();
        let mut config = surface
            .get_default_config(adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface has no supported format")?;
        let caps = surface.get_capabilities(adapter);
        // The compute pass produces display-encoded bytes. A linear swap
        // chain view can copy them without an extra gamma conversion.
        if let Some(&format) = caps.formats.iter().find(|format| !format.is_srgb()) {
            config.format = format;
        }
        config.present_mode = wgpu::PresentMode::AutoNoVsync;
        surface.configure(device, &config);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ground preview presentation"),
            source: wgpu::ShaderSource::Wgsl(include_str!("display.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &std::collections::HashMap::from([(
                        "SRGB".into(),
                        f64::from(config.format.is_srgb()),
                    )]),
                    ..Default::default()
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pixels.as_entire_binding(),
                },
            ],
        });
        Ok(Self {
            surface,
            config,
            pipeline,
            binding,
        })
    }

    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        window: &Window,
        output: &mut wgpu::CommandEncoder,
    ) -> Result<Option<wgpu::SurfaceTexture>> {
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(None);
        }
        if self.config.width != size.width || self.config.height != size.height {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(device, &self.config);
        }
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(device, &self.config);
                return Ok(None);
            }
            Err(wgpu::SurfaceError::Timeout | wgpu::SurfaceError::Other) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let target = frame.texture.create_view(&Default::default());
        {
            let mut pass = output.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.binding, &[]);
            pass.draw(0..3, 0..1);
        }
        Ok(Some(frame))
    }
}
