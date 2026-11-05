//! Greenfield bounded native-voxel playtest viewer; no streaming or distant LOD yet.
pub mod mesh;
use frontier_world::{Cell, Material, World, VOXEL_SIZE};
use glam::{DVec3, Mat4, Vec3};
use std::{
    collections::HashSet,
    error::Error,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use wgpu::util::DeviceExt;
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

pub const SEED: u64 = 7;
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const SHADER: &str = include_str!("voxel.wgsl");
type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Camera {
    position: DVec3,
    yaw: f32,
    pitch: f32,
}
impl Camera {
    fn fixture() -> Self {
        let position = DVec3::new(11., 9., -15.);
        let direction = (DVec3::new(-1., 3., 1.) - position).normalize();
        Self {
            position,
            yaw: direction.x.atan2(direction.z) as f32,
            pitch: direction.y.asin() as f32,
        }
    }
    fn direction(&self) -> Vec3 {
        Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.cos() * self.pitch.cos(),
        )
    }
    fn matrix(&self, origin: DVec3, aspect: f32) -> Mat4 {
        let eye = (self.position - origin).as_vec3();
        Mat4::perspective_rh(60_f32.to_radians(), aspect, 0.025, 500.)
            * Mat4::look_to_rh(eye, self.direction(), Vec3::Y)
    }
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    origin: DVec3,
    format: wgpu::TextureFormat,
}
impl Gpu {
    async fn new(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        format: Option<wgpu::TextureFormat>,
        mesh: &mesh::Mesh,
    ) -> Result<Self> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: surface,
                force_fallback_adapter: false,
            })
            .await
            .ok_or("no compatible GPU adapter (try Vulkan software ICD for headless capture)")?;
        eprintln!("adapter: {:?}", adapter.get_info());
        let format = format.unwrap_or_else(|| {
            surface
                .map(|s| {
                    s.get_capabilities(&adapter)
                        .formats
                        .into_iter()
                        .find(|f| f.is_srgb())
                        .unwrap_or(wgpu::TextureFormat::Bgra8UnormSrgb)
                })
                .unwrap_or(wgpu::TextureFormat::Rgba8UnormSrgb)
        });
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("Frontier device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                },
                None,
            )
            .await?;
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera relative matrix"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native voxel"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native voxel"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("native voxel"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<mesh::Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fixture vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fixture indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Ok(Self {
            device,
            queue,
            pipeline,
            uniform,
            bind_group,
            vertices,
            indices,
            index_count: mesh.indices.len() as u32,
            origin: DVec3::new(
                mesh.origin.x as f64,
                mesh.origin.y as f64,
                mesh.origin.z as f64,
            ) * VOXEL_SIZE,
            format,
        })
    }
    fn replace_mesh(&mut self, mesh: &mesh::Mesh) {
        self.vertices = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("edited vertices"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        self.indices = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("edited indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        self.index_count = mesh.indices.len() as u32;
    }
    fn texture(&self, width: u32, height: u32, depth: bool) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(if depth { "depth" } else { "capture" }),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if depth {
                wgpu::TextureFormat::Depth32Float
            } else {
                self.format
            },
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | if depth {
                    wgpu::TextureUsages::empty()
                } else {
                    wgpu::TextureUsages::COPY_SRC
                },
            view_formats: &[],
        })
    }
    fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        camera: &Camera,
        width: u32,
        height: u32,
    ) {
        self.queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::cast_slice(
                &camera
                    .matrix(self.origin, width as f32 / height as f32)
                    .to_cols_array(),
            ),
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native fixture"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.34,
                        g: 0.57,
                        b: 0.80,
                        a: 1.,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
    fn capture(&self, camera: &Camera, path: &Path, width: u32, height: u32) -> Result<()> {
        let texture = self.texture(width, height, false);
        let depth = self.texture(width, height, true);
        let row = (width * 4 + 255) & !255;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture readback"),
            size: row as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.draw(
            &mut encoder,
            &texture.create_view(&Default::default()),
            &depth.create_view(&Default::default()),
            camera,
            width,
            height,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let mapped = buffer.slice(..).get_mapped_range();
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for scanline in mapped.chunks_exact(row as usize) {
            pixels.extend_from_slice(&scanline[..(width * 4) as usize]);
        }
        if self.format == wgpu::TextureFormat::Bgra8UnormSrgb
            || self.format == wgpu::TextureFormat::Bgra8Unorm
        {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        let mut png = png::Encoder::new(std::fs::File::create(path)?, width, height);
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        png.write_header()?.write_image_data(&pixels)?;
        eprintln!("captured {} ({}x{})", path.display(), width, height);
        Ok(())
    }
}

const ARENA_MIN: Cell = Cell {
    x: -128,
    y: -32,
    z: -128,
};
const ARENA_SIZE: [usize; 3] = [256, 224, 256];
fn fixture_mesh(world: &World) -> mesh::Mesh {
    mesh::extract(|p| world.material(p), ARENA_MIN, ARENA_SIZE)
}
fn inside_arena(cell: Cell) -> bool {
    (ARENA_MIN.x..ARENA_MIN.x + ARENA_SIZE[0] as i64).contains(&cell.x)
        && (ARENA_MIN.y..ARENA_MIN.y + ARENA_SIZE[1] as i64).contains(&cell.y)
        && (ARENA_MIN.z..ARENA_MIN.z + ARENA_SIZE[2] as i64).contains(&cell.z)
}
/// Picking always queries the canonical source. A pending rebuild permits no second edit.
fn edit_target(
    world: &World,
    origin: [f64; 3],
    direction: [f64; 3],
    place: bool,
    pending: bool,
) -> Option<Cell> {
    if pending {
        return None;
    }
    let hit = frontier_world::raycast(world, origin, direction, 8.)?;
    let cell = if place {
        if hit.normal == [0; 3] {
            return None;
        }
        Cell {
            x: hit.cell.x + hit.normal[0] as i64,
            y: hit.cell.y + hit.normal[1] as i64,
            z: hit.cell.z + hit.normal[2] as i64,
        }
    } else {
        hit.cell
    };
    inside_arena(cell).then_some(cell)
}
struct PendingEdit {
    receiver: std::sync::mpsc::Receiver<(mesh::Mesh, u64, std::time::Duration)>,
    started: Instant,
}
impl PendingEdit {
    fn start(snapshot: World, min: Cell, size: [usize; 3]) -> Self {
        let revision = snapshot.revision();
        let (sender, receiver) = std::sync::mpsc::channel();
        let started = Instant::now();
        std::thread::spawn(move || {
            let mesh = mesh::extract(|p| snapshot.material(p), min, size);
            let _ = sender.send((mesh, revision, started.elapsed()));
        });
        Self { receiver, started }
    }
}

struct Viewer {
    mesh: Option<mesh::Mesh>,
    window: Option<Arc<Window>>,
    surface: Option<wgpu::Surface<'static>>,
    gpu: Option<Gpu>,
    config: Option<wgpu::SurfaceConfiguration>,
    depth: Option<wgpu::Texture>,
    camera: Camera,
    keys: HashSet<KeyCode>,
    grabbed: bool,
    last_frame: Instant,
    fps_start: Instant,
    frames: u32,
    capture_number: u32,
    error: Option<String>,
    width: u32,
    height: u32,
    frame_limit: Option<u64>,
    total_frames: u64,
    stats: frontier_runtime::FrameStats,
    world: World,
    pending_edit: Option<PendingEdit>,
}
impl Viewer {
    fn edit(&mut self, place: bool) {
        let Some(cell) = edit_target(
            &self.world,
            self.camera.position.to_array(),
            self.camera.direction().as_dvec3().to_array(),
            place,
            self.pending_edit.is_some(),
        ) else {
            return;
        };
        self.world.set(
            cell,
            if place {
                Material::Stone
            } else {
                Material::Air
            },
        );
        let snapshot = self.world.clone();
        let revision = snapshot.revision();
        self.pending_edit = Some(PendingEdit::start(snapshot, ARENA_MIN, ARENA_SIZE));
        eprintln!(
            "edit {:?} at {:?}: revision {} rebuilding; further edits wait",
            if place { "place" } else { "remove" },
            cell,
            revision
        );
    }
    fn poll_edit(&mut self) {
        let Some(pending) = &self.pending_edit else {
            return;
        };
        match pending.receiver.try_recv() {
            Ok((mesh, revision, build_time)) => {
                if revision == self.world.revision() {
                    if let Some(gpu) = &mut self.gpu {
                        gpu.replace_mesh(&mesh);
                    }
                    eprintln!(
                        "published edit revision {}: build {:.3}s, input-to-upload {:.3}s",
                        revision,
                        build_time.as_secs_f64(),
                        pending.started.elapsed().as_secs_f64()
                    );
                } else {
                    eprintln!("discarded obsolete edit mesh revision {}", revision);
                }
                self.pending_edit = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                eprintln!("edit mesh worker disconnected");
                self.pending_edit = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => (),
        }
    }
    fn grab(&mut self, grabbed: bool) {
        if let Some(window) = &self.window {
            if grabbed {
                self.grabbed = window
                    .set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined))
                    .is_ok();
            } else {
                let _ = window.set_cursor_grab(CursorGrabMode::None);
                self.grabbed = false;
            }
            window.set_cursor_visible(!self.grabbed);
        }
    }
    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let (Some(gpu), Some(surface), Some(config)) = (&self.gpu, &self.surface, &mut self.config)
        else {
            return;
        };
        config.width = width;
        config.height = height;
        surface.configure(&gpu.device, config);
        self.depth = Some(gpu.texture(width, height, true));
    }
    fn render(&mut self) -> Result<()> {
        self.poll_edit();
        let now = Instant::now();
        let interval = now.duration_since(self.last_frame);
        if self.total_frames > 0 {
            self.stats.record(interval.as_secs_f64() * 1000.);
        }
        let dt = interval.as_secs_f32().min(0.1);
        self.last_frame = now;
        if self.grabbed {
            let forward = Vec3::new(self.camera.yaw.sin(), 0., self.camera.yaw.cos());
            let right = forward.cross(Vec3::Y);
            let mut movement = Vec3::ZERO;
            for (key, direction) in [
                (KeyCode::KeyW, forward),
                (KeyCode::KeyS, -forward),
                (KeyCode::KeyD, right),
                (KeyCode::KeyA, -right),
                (KeyCode::Space, Vec3::Y),
                (KeyCode::ControlLeft, -Vec3::Y),
                (KeyCode::ControlRight, -Vec3::Y),
            ] {
                if self.keys.contains(&key) {
                    movement += direction;
                }
            }
            let speed = if self.keys.contains(&KeyCode::ShiftLeft)
                || self.keys.contains(&KeyCode::ShiftRight)
            {
                15.
            } else {
                3.
            };
            self.camera.position += movement.normalize_or_zero().as_dvec3() * (dt * speed) as f64;
        }
        let (Some(gpu), Some(surface), Some(config), Some(depth)) =
            (&self.gpu, &self.surface, &self.config, &self.depth)
        else {
            return Ok(());
        };
        let window = self.window.as_ref().unwrap();
        if window.inner_size().width == 0 || window.inner_size().height == 0 {
            return Ok(());
        }
        let frame = match surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                surface.configure(&gpu.device, config);
                return Ok(());
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        gpu.draw(
            &mut encoder,
            &frame.texture.create_view(&Default::default()),
            &depth.create_view(&Default::default()),
            &self.camera,
            config.width,
            config.height,
        );
        gpu.queue.submit(Some(encoder.finish()));
        frame.present();
        self.frames += 1;
        self.total_frames += 1;
        let elapsed = now.duration_since(self.fps_start).as_secs_f64();
        if elapsed >= 0.5 {
            window.set_title(&format!("Frontier native fixture | {:.0} FPS | {} triangles | center aim: L remove / R stone | {} | Esc release / F12", self.frames as f64 / elapsed, gpu.index_count / 3, if self.pending_edit.is_some() { "edit building" } else { "ready" }));
            self.frames = 0;
            self.fps_start = now;
        }
        Ok(())
    }
}
impl ApplicationHandler for Viewer {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let setup = (|| -> Result<()> {
            let window = Arc::new(
                event_loop.create_window(
                    Window::default_attributes()
                        .with_title("Frontier native fixture — click to capture mouse")
                        .with_inner_size(winit::dpi::PhysicalSize::new(self.width, self.height)),
                )?,
            );
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
            let surface = instance.create_surface(window.clone())?;
            let gpu = pollster::block_on(Gpu::new(
                &instance,
                Some(&surface),
                None,
                self.mesh.as_ref().unwrap(),
            ))?;
            let size = window.inner_size();
            let config = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: gpu.format,
                width: size.width.max(1),
                height: size.height.max(1),
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: vec![],
            };
            surface.configure(&gpu.device, &config);
            self.depth = Some(gpu.texture(config.width, config.height, true));
            self.config = Some(config);
            self.gpu = Some(gpu);
            self.surface = Some(surface);
            self.window = Some(window);
            self.mesh = None;
            self.last_frame = Instant::now();
            Ok(())
        })();
        if let Err(error) = setup {
            self.error = Some(error.to_string());
            event_loop.exit();
        }
    }
    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if self.grabbed {
            if let DeviceEvent::MouseMotion { delta } = event {
                self.camera.yaw -= delta.0 as f32 * 0.002;
                self.camera.pitch = (self.camera.pitch - delta.1 as f32 * 0.002).clamp(-1.55, 1.55);
            }
        }
    }
    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.resize(size.width, size.height),
            WindowEvent::Focused(false) => {
                self.keys.clear();
                self.grab(false);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                if !self.grabbed {
                    self.grab(true);
                } else if button == MouseButton::Left || button == MouseButton::Right {
                    self.edit(button == MouseButton::Right);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(key) = event.physical_key {
                    if event.state == ElementState::Pressed {
                        self.keys.insert(key);
                        if key == KeyCode::Escape {
                            self.grab(false);
                        }
                        if key == KeyCode::F12 && !event.repeat {
                            let path = PathBuf::from(format!(
                                "frontier-capture-{:03}.png",
                                self.capture_number
                            ));
                            self.capture_number += 1;
                            if let (Some(gpu), Some(config)) = (&self.gpu, &self.config) {
                                if let Err(error) =
                                    gpu.capture(&self.camera, &path, config.width, config.height)
                                {
                                    eprintln!("capture failed: {error}");
                                }
                            }
                        }
                    } else {
                        self.keys.remove(&key);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.render() {
                    self.error = Some(error.to_string());
                    event_loop.exit();
                }
                if self
                    .frame_limit
                    .is_some_and(|limit| self.total_frames >= limit)
                {
                    event_loop.exit();
                }
            }
            _ => (),
        }
    }
    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

/// Runs the interactive fixture or captures it without a window using `--capture PATH`.
/// Camera movement never rebuilds geometry; edits rebuild on one bounded background worker.
pub fn run() -> Result<()> {
    let mut capture = None;
    let mut width = WIDTH;
    let mut height = HEIGHT;
    let mut frame_limit = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
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
                println!("Frontier bounded native-voxel fixture\n--capture PATH [--width 1280 --height 720] or --frames N for a finite window run\nClick: capture mouse; WASD: free flight; Shift: fast; Space/Ctrl: vertical; Esc: release mouse; F12: PNG\nAim at screen center: left removes a cell, right places stone (8m reach); wait for each rebuild.\nThis reference has no streaming, distant LOD, collision or persistence.");
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err("dimensions must be in 1..=8192".into());
    }
    let world = World::new(SEED);
    let started = Instant::now();
    eprintln!(
        "extracting bounded 16x14x16m fixture at native {}m cells (startup only)",
        VOXEL_SIZE
    );
    let mesh = fixture_mesh(&world);
    eprintln!(
        "mesh: {} native exposed faces -> {} greedy quads, {} vertices in {:.2}s",
        mesh.exposed_native_faces,
        mesh.indices.len() / 6,
        mesh.vertices.len(),
        started.elapsed().as_secs_f64()
    );
    if let Some(path) = capture {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let gpu = pollster::block_on(Gpu::new(&instance, None, None, &mesh))?;
        return gpu.capture(&Camera::fixture(), &path, width, height);
    }
    let now = Instant::now();
    let mut viewer = Viewer {
        mesh: Some(mesh),
        window: None,
        surface: None,
        gpu: None,
        config: None,
        depth: None,
        camera: Camera::fixture(),
        keys: HashSet::new(),
        grabbed: false,
        last_frame: now,
        fps_start: now,
        frames: 0,
        capture_number: 0,
        error: None,
        width,
        height,
        frame_limit,
        total_frames: 0,
        stats: frontier_runtime::FrameStats::new(10000),
        world,
        pending_edit: None,
    };
    EventLoop::new()?.run_app(&mut viewer)?;
    eprintln!(
        "frame intervals (includes presentation; not isolated GPU timing): {:?}",
        viewer.stats.summary()
    );
    if let Some(error) = viewer.error {
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn asynchronous_edit_snapshot_preserves_revision_and_changes_geometry() {
        use super::*;
        let mut world = World::new(SEED);
        let target = Cell { x: 0, y: 176, z: 0 };
        world.set(target, Material::Stone);
        let before = PendingEdit::start(world.clone(), target, [1; 3])
            .receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(before.1, world.revision());
        assert_eq!(before.0.indices.len(), 36);
        world.set(target, Material::Air);
        let expected_revision = world.revision();
        let pending = PendingEdit::start(world.clone(), target, [1; 3]);
        // A later mutation cannot change the immutable worker snapshot.
        world.set(target, Material::Wood);
        let after = pending
            .receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(after.1, expected_revision);
        assert_ne!(after.1, world.revision()); // publication guard rejects this snapshot
        assert!(after.0.indices.is_empty());
    }
    #[test]
    fn edits_pick_native_cell_and_adjacent_air_and_reject_pending() {
        use super::*;
        let mut world = World::new(SEED);
        // Isolated fixture cell in the air above the mountain.
        let target = Cell { x: 0, y: 176, z: 0 };
        world.set(target, Material::Stone);
        let origin = [-0.1, 11.03125, 0.03125];
        let direction = [1., 0., 0.];
        assert_eq!(
            edit_target(&world, origin, direction, false, false),
            Some(target)
        );
        assert_eq!(
            edit_target(&world, origin, direction, true, false),
            Some(Cell { x: -1, ..target })
        );
        assert_eq!(edit_target(&world, origin, direction, false, true), None);
        assert_eq!(edit_target(&world, origin, direction, true, true), None);
    }
    #[test]
    fn edit_bounds_are_half_open_and_do_not_place_outside_mesh() {
        use super::*;
        assert!(inside_arena(ARENA_MIN));
        assert!(!inside_arena(Cell { x: 128, y: 0, z: 0 }));
        assert!(!inside_arena(Cell {
            x: -129,
            y: 0,
            z: 0
        }));
        assert!(!inside_arena(Cell { x: 0, y: 192, z: 0 }));
        let mut world = World::new(SEED);
        world.set(
            Cell {
                x: -128,
                y: 176,
                z: 0,
            },
            Material::Stone,
        );
        assert_eq!(
            edit_target(&world, [-8.1, 11.03125, 0.03125], [1., 0., 0.], true, false),
            None
        );
        assert_eq!(
            edit_target(
                &world,
                [-8.1, 11.03125, 0.03125],
                [1., 0., 0.],
                false,
                false
            ),
            Some(Cell {
                x: -128,
                y: 176,
                z: 0
            })
        );
    }
    #[test]
    fn wgsl_is_valid() {
        let module = naga::front::wgsl::parse_str(super::SHADER).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }
    #[test]
    fn local_camera_is_stable_at_large_origins() {
        let mut camera = super::Camera::fixture();
        let original = camera.matrix(glam::DVec3::ZERO, 1.);
        let offset = glam::DVec3::new(1e9, -1e9, 1e9);
        camera.position += offset;
        assert!(original.abs_diff_eq(camera.matrix(offset, 1.), 1e-5));
    }
}
