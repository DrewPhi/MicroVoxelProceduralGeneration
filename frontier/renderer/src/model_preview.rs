//! A small studio that exercises the same aircraft mesh, displays and rig as flight.
use crate::{
    aircraft::{Instruments, Presentation},
    aircraft_model::{self, Model, Part, Rig},
    Camera, Result,
};
use glam::{DVec3, Mat4, Vec3};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use wgpu::util::DeviceExt;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

const VIEWS: [&str; 8] = [
    "orbit",
    "front",
    "side",
    "rear",
    "top",
    "underside",
    "pilot",
    "rear-seat",
];
const TARGET: Vec3 = Vec3::new(0., 0.1, -0.85);
const STUDIO_COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const FOV: f32 = 43.;
const FAR: f32 = 250.;
const DEFAULT_DISTANCE: f32 = 13.7;

#[derive(Clone)]
struct Studio {
    view: usize,
    yaw: f32,
    pitch: f32,
    distance: f32,
    pan: Vec3,
    orthographic: bool,
    interior_look: [f32; 2],
    interior_fov: f32,
    rig: Rig,
    isolate: Option<Part>,
    roll: f32,
    elevator: f32,
    rudder: f32,
    power: f32,
    wheel: f32,
    compression: f32,
    engine: bool,
    lighting: f32,
}
impl Default for Studio {
    fn default() -> Self {
        Self {
            view: 0,
            yaw: 42.,
            pitch: 20.,
            distance: DEFAULT_DISTANCE,
            pan: Vec3::ZERO,
            orthographic: false,
            interior_look: [0.; 2],
            interior_fov: 64.,
            rig: Rig::default(),
            isolate: None,
            roll: 0.,
            elevator: 0.,
            rudder: 0.,
            power: 0.,
            wheel: 0.,
            compression: 0.,
            engine: false,
            lighting: 44.,
        }
    }
}

impl Studio {
    fn camera(&self) -> (Camera, Option<f32>) {
        let mut camera = Camera {
            position: DVec3::ZERO,
            yaw: 0.,
            pitch: 0.,
            roll: 0.,
            fov_y: FOV,
            far: FAR,
        };
        if self.view >= 6 {
            camera.fov_y = self.interior_fov;
            if self.view == 6 {
                camera.position = DVec3::new(0.28, 0.41, -0.48);
                camera.pitch = -0.19;
            } else {
                camera.position = DVec3::new(-0.22, 0.49, -1.49);
                camera.pitch = -0.16;
                camera.yaw = 0.19;
            }
            camera.position += self.pan.as_dvec3();
            camera.yaw += self.interior_look[0].to_radians();
            camera.pitch = (camera.pitch + self.interior_look[1].to_radians()).clamp(-1.50, 1.50);
            return (camera, None);
        }
        let [yaw, pitch]: [f32; 2] = match self.view {
            1 => [0., 0.],
            2 => [90., 0.],
            3 => [180., 0.],
            4 => [0., 90.],
            5 => [0., -90.],
            _ => [self.yaw, self.pitch],
        };
        let yaw = yaw.to_radians();
        let pitch = pitch.to_radians();
        let outward = Vec3::new(
            yaw.sin() * pitch.cos(),
            pitch.sin(),
            yaw.cos() * pitch.cos(),
        );
        camera.position = (TARGET + self.pan + outward * self.distance).as_dvec3();
        camera.yaw = yaw + std::f32::consts::PI;
        camera.pitch = -pitch;
        let ortho = (self.orthographic || self.view != 0)
            .then(|| 2. * self.distance * (FOV.to_radians() * 0.5).tan());
        (camera, ortho)
    }

    fn plane(&self, time: f32) -> Presentation {
        Presentation {
            pose: Camera {
                position: DVec3::ZERO,
                yaw: 0.,
                pitch: 0.,
                roll: 0.,
                fov_y: FOV,
                far: FAR,
            },
            instruments: Instruments {
                speed_mps: 40.,
                altitude_m: 825.,
                climb_mps: 2.05,
                bank_degrees: self.roll * 15.,
                pitch_degrees: self.elevator * 10.,
                heading_degrees: 180.,
                stall_warning: false,
                flaps_degrees: self.rig.flaps * 30.,
                pitch_trim: 0.,
                angle_of_attack: 0.,
                wind: Vec3::new(3., 0.4, 4.),
                show_airflow: false,
                grounded: true,
                throttle: self.power,
            },
            controls: Vec3::new(self.roll, self.elevator, self.power),
            door: 0.,
            rudder: self.rudder,
            turn_rate: self.rudder * 0.04,
            prop_angle: if self.engine { time * 42. } else { 0.51 },
            engine_running: self.engine,
            wheel_turn: [self.wheel.to_radians(); 3],
            gear_compression: [self.compression; 3],
            radio_khz: 118_025,
            home_relative_radians: 0.50,
            home_distance_km: 0.52,
            hold: false,
            wing_flex: [0.; 2],
        }
    }

    fn panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("model")
            .resizable(false)
            .default_width(258.)
            .show(ctx, |ui| {
                ui.heading("Aircraft studio");
                ui.label("Skyhawk · 6.25 cm voxels");
                ui.add_space(8.);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label(egui::RichText::new("Camera").strong());
                    ui.horizontal_wrapped(|ui| {
                        for (i, name) in VIEWS.iter().enumerate() {
                            if ui.selectable_label(self.view == i, *name).clicked() {
                                self.view = i;
                                self.pan = Vec3::ZERO;
                                self.interior_look = [0.; 2];
                                self.interior_fov = 64.;
                                self.distance = DEFAULT_DISTANCE;
                            }
                        }
                    });
                    ui.checkbox(&mut self.orthographic, "Orthographic orbit");
                    ui.small("Drag to orbit · right drag to pan · scroll to zoom");
                    ui.add_space(8.);
                    ui.label(egui::RichText::new("Inspection").strong());
                    egui::ComboBox::from_id_salt("isolate")
                        .selected_text(self.isolate.map_or("Whole aircraft", Part::name))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.isolate, None, "Whole aircraft");
                            for part in Part::ALL {
                                ui.selectable_value(&mut self.isolate, Some(part), part.name());
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.rig.cutaway, 0., "Shell");
                        ui.selectable_value(&mut self.rig.cutaway, 1., "Left cut");
                        ui.selectable_value(&mut self.rig.cutaway, -1., "Right cut");
                    });
                    ui.checkbox(&mut self.rig.glass_off, "Hide glass");
                    ui.add(
                        egui::Slider::new(&mut self.lighting, -180.0..=180.).text("Light angle"),
                    );
                    ui.separator();
                    ui.label(egui::RichText::new("Articulation").strong());
                    for (value, label) in self.rig.doors.iter_mut().zip(["Left door", "Right door"])
                    {
                        ui.add(egui::Slider::new(value, 0.0..=1.).text(label));
                    }
                    for (value, label) in [
                        (&mut self.roll, "Ailerons"),
                        (&mut self.elevator, "Elevator"),
                        (&mut self.rudder, "Rudder"),
                    ] {
                        ui.add(egui::Slider::new(value, -1.0..=1.).text(label));
                    }
                    ui.add(egui::Slider::new(&mut self.rig.flaps, 0.0..=1.).text("Flaps"));
                    ui.add(egui::Slider::new(&mut self.power, 0.0..=1.).text("Throttle"));
                    ui.add(egui::Slider::new(&mut self.wheel, 0.0..=360.).text("Wheels"));
                    ui.add(
                        egui::Slider::new(&mut self.compression, 0.0..=0.13)
                            .text("Strut travel (m)"),
                    );
                    ui.checkbox(&mut self.engine, "Propeller running");
                    if ui.button("Reset preview").clicked() {
                        *self = Self::default();
                    }
                });
            });
    }
}

fn number(raw: &str, low: f32, high: f32) -> Result<f32> {
    let x: f32 = raw.parse()?;
    if !(low..=high).contains(&x) {
        return Err(format!("preview value must be in {low}..={high}: {raw}").into());
    }
    Ok(x)
}

pub(crate) fn run(mut args: impl Iterator<Item = String>) -> Result<()> {
    let mut studio = Studio::default();
    let mut capture: Option<PathBuf> = None;
    let mut target = None;
    let mut size = [1280, 800];
    while let Some(key) = args.next() {
        if key == "--inspect" {
            println!("{}", aircraft_model::inspect());
            return Ok(());
        }
        if key == "--help" {
            println!("--model-preview [--view orbit|front|side|rear|top|underside|pilot|rear-seat] [--capture FILE --width N --height N] [--inspect]\n\
            Orbit: --yaw DEG --pitch DEG --distance M --orthographic --target X,Y,Z\n\
            Inspection: --isolate PART --cutaway left|right --no-glass --light DEG\n\
            Posing: --door 0..1 --left-door 0..1 --right-door 0..1 --aileron -1..1 --elevator -1..1 --rudder -1..1 --flaps 0..1 --power 0..1 --wheel DEG --compression M --engine");
            return Ok(());
        }
        match key.as_str() {
            "--no-glass" => {
                studio.rig.glass_off = true;
                continue;
            }
            "--orthographic" => {
                studio.orthographic = true;
                continue;
            }
            "--engine" => {
                studio.engine = true;
                continue;
            }
            _ => {}
        }
        let value = args
            .next()
            .ok_or_else(|| format!("{key} requires a value"))?;
        match key.as_str() {
            "--capture" => capture = Some(value.into()),
            "--width" | "--height" => {
                let pixels = value.parse()?;
                if !(160..=2400).contains(&pixels) {
                    return Err("preview dimensions must be 160..=2400".into());
                }
                size[usize::from(key == "--height")] = pixels;
            }
            "--view" => {
                studio.view = VIEWS
                    .iter()
                    .position(|name| *name == value)
                    .ok_or("unknown model view")?
            }
            "--yaw" => studio.yaw = number(&value, -360., 360.)?,
            "--pitch" => studio.pitch = number(&value, -89., 89.)?,
            "--distance" => studio.distance = number(&value, 0.55, 80.)?,
            "--light" => studio.lighting = number(&value, -180., 180.)?,
            "--target" => {
                let v = value
                    .split(',')
                    .map(|x| number(x, -16., 16.))
                    .collect::<Result<Vec<_>>>()?;
                let v: [f32; 3] = v.try_into().map_err(|_| "model target requires x,y,z")?;
                target = Some(Vec3::from_array(v));
            }
            "--isolate" => {
                studio.isolate = Some(
                    Part::ALL
                        .into_iter()
                        .find(|p| p.name() == value)
                        .ok_or("unknown model part")?,
                )
            }
            "--cutaway" => {
                studio.rig.cutaway = match value.as_str() {
                    "left" => 1.,
                    "right" => -1.,
                    _ => return Err("cutaway requires left or right".into()),
                }
            }
            "--door" => studio.rig.doors = [number(&value, 0., 1.)?; 2],
            "--left-door" => studio.rig.doors[0] = number(&value, 0., 1.)?,
            "--right-door" => studio.rig.doors[1] = number(&value, 0., 1.)?,
            "--aileron" => studio.roll = number(&value, -1., 1.)?,
            "--elevator" => studio.elevator = number(&value, -1., 1.)?,
            "--rudder" => studio.rudder = number(&value, -1., 1.)?,
            "--flaps" => studio.rig.flaps = number(&value, 0., 1.)?,
            "--power" => studio.power = number(&value, 0., 1.)?,
            "--wheel" => studio.wheel = number(&value, 0., 360.)?,
            "--compression" => studio.compression = number(&value, 0., 0.13)?,
            _ => return Err(format!("unknown model preview argument {key}").into()),
        }
    }
    if let Some(target) = target {
        studio.pan = target - if studio.view < 6 { TARGET } else { Vec3::ZERO };
    }
    if let Some(path) = capture {
        let instance = wgpu::Instance::default();
        let gpu = pollster::block_on(StudioGpu::new(&instance, None))?.1;
        return gpu.capture(&studio, size, &path);
    }
    let event_loop = EventLoop::new()?;
    let mut app = Preview {
        studio,
        size,
        state: None,
        error: None,
    };
    event_loop.run_app(&mut app)?;
    app.error.map_or(Ok(()), |e| Err(e.into()))
}

fn texture(
    device: &wgpu::Device,
    size: [u32; 2],
    samples: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("model studio target"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

struct StudioGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    samples: u32,
    model: Model,
    floor: wgpu::RenderPipeline,
    floor_camera: wgpu::Buffer,
    floor_bind: wgpu::BindGroup,
}

impl StudioGpu {
    async fn new(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<(wgpu::Adapter, Self)> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: surface,
                force_fallback_adapter: false,
            })
            .await
            .ok_or("no GPU for model preview")?;
        let format = surface
            .and_then(|s| {
                s.get_capabilities(&adapter)
                    .formats
                    .into_iter()
                    .find(|f| f.is_srgb())
            })
            .unwrap_or(STUDIO_COLOR);
        let samples = if [format, wgpu::TextureFormat::Depth32Float]
            .into_iter()
            .all(|f| {
                adapter
                    .get_texture_format_features(f)
                    .flags
                    .contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4)
            }) {
            4
        } else {
            1
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await?;
        let model = Model::new(&device, format, samples);
        // A tiny generated chart exercises the real panel shader in the studio.
        model.set_preview_chart(&queue);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("studio lighting and floor"),
            source: wgpu::ShaderSource::Wgsl(include_str!("model_preview.wgsl").into()),
        });
        let floor = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("aircraft studio floor"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });
        let floor_camera = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("studio camera"),
            contents: &[0; 64],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let floor_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("studio camera"),
            layout: &floor.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: floor_camera.as_entire_binding(),
            }],
        });
        Ok((
            adapter,
            Self {
                device,
                queue,
                format,
                samples,
                model,
                floor,
                floor_camera,
                floor_bind,
            },
        ))
    }

    fn targets(&self, size: [u32; 2]) -> Targets {
        let view = |samples, format| {
            texture(
                &self.device,
                size,
                samples,
                format,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
            .create_view(&Default::default())
        };
        Targets {
            depth: view(self.samples, wgpu::TextureFormat::Depth32Float),
            msaa: (self.samples > 1).then(|| view(self.samples, self.format)),
        }
    }

    fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        studio: &Studio,
        output: &wgpu::TextureView,
        targets: &Targets,
        rect: [f32; 4],
        time: f32,
    ) {
        let [x, y, w, h] = rect;
        if w < 1. || h < 1. {
            return;
        }
        let aspect = w / h;
        let (camera, ortho) = studio.camera();
        let plane = studio.plane(time);
        let sun = Vec3::new(
            studio.lighting.to_radians().sin(),
            1.25,
            studio.lighting.to_radians().cos(),
        )
        .normalize();
        self.model.prepare_view(
            &self.queue,
            camera,
            plane,
            aspect,
            sun,
            time,
            ortho,
            studio.rig,
        );
        let matrix = if let Some(height) = ortho {
            let y = height * 0.5;
            let x = y * aspect;
            Mat4::orthographic_rh(-x, x, -y, y, 0.025, FAR)
                * Mat4::look_to_rh(
                    camera.position.as_vec3(),
                    camera.direction(),
                    camera.view_up(),
                )
        } else {
            camera.matrix(DVec3::ZERO, aspect)
        };
        self.queue.write_buffer(
            &self.floor_camera,
            0,
            bytemuck::bytes_of(&matrix.inverse().to_cols_array_2d()),
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("aircraft studio"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: targets.msaa.as_ref().unwrap_or(output),
                resolve_target: targets.msaa.as_ref().map(|_| output),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.025,
                        g: 0.032,
                        b: 0.044,
                        a: 1.,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_viewport(x, y, w, h, 0., 1.);
        if studio.view != 5 {
            pass.set_pipeline(&self.floor);
            pass.set_bind_group(0, &self.floor_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.model.draw_part(&mut pass, studio.isolate);
    }

    fn capture(&self, studio: &Studio, size: [u32; 2], path: &Path) -> Result<()> {
        let [w, h] = size;
        let output = texture(
            &self.device,
            size,
            1,
            self.format,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let view = output.create_view(&Default::default());
        let stride = (w * 4).next_multiple_of(256);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("model studio PNG"),
            size: u64::from(stride) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.render(
            &mut encoder,
            studio,
            &view,
            &self.targets(size),
            [0., 0., w as f32, h as f32],
            0.,
        );
        encoder.copy_texture_to_buffer(
            output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(h),
                },
            },
            output.size(),
        );
        self.queue.submit(Some(encoder.finish()));
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, move |status| {
            let _ = tx.send(status);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let mapped = slice.get_mapped_range();
        let rgb = mapped
            .chunks_exact(stride as usize)
            .flat_map(|row| {
                row[..w as usize * 4]
                    .chunks_exact(4)
                    .flat_map(|rgba| [rgba[0], rgba[1], rgba[2]])
            })
            .collect::<Vec<_>>();
        let mut png =
            png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?), w, h);
        png.set_color(png::ColorType::Rgb);
        png.set_depth(png::BitDepth::Eight);
        png.write_header()?.write_image_data(&rgb)?;
        Ok(())
    }
}

struct Targets {
    depth: wgpu::TextureView,
    msaa: Option<wgpu::TextureView>,
}
struct WindowState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: StudioGpu,
    targets: Targets,
    egui: egui::Context,
    input: egui_winit::State,
    painter: egui_wgpu::Renderer,
    start: Instant,
}
struct Preview {
    studio: Studio,
    size: [u32; 2],
    state: Option<WindowState>,
    error: Option<String>,
}

impl WindowState {
    fn new(el: &ActiveEventLoop, size: [u32; 2]) -> Result<Self> {
        let window = Arc::new(
            el.create_window(
                Window::default_attributes()
                    .with_title("Frontier aircraft studio")
                    .with_inner_size(winit::dpi::PhysicalSize::new(size[0], size[1]))
                    .with_min_inner_size(winit::dpi::LogicalSize::new(690, 420)),
            )?,
        );
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let (adapter, gpu) = pollster::block_on(StudioGpu::new(&instance, Some(&surface)))?;
        let w = window.inner_size().width.max(1);
        let h = window.inner_size().height.max(1);
        let mut config = surface
            .get_default_config(&adapter, w, h)
            .ok_or("no preview surface configuration")?;
        config.format = gpu.format;
        config.present_mode = wgpu::PresentMode::Fifo;
        surface.configure(&gpu.device, &config);
        let targets = gpu.targets([w, h]);
        let egui = egui::Context::default();
        egui.set_visuals(egui::Visuals::dark());
        egui.style_mut(|style| {
            style.visuals.panel_fill = egui::Color32::from_rgb(20, 26, 37);
            style.visuals.override_text_color = Some(egui::Color32::from_rgb(214, 224, 231));
            style.spacing.slider_width = 94.;
            style.spacing.item_spacing.y = 7.;
        });
        let input = egui_winit::State::new(
            egui.clone(),
            egui::ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(4096),
        );
        let painter = egui_wgpu::Renderer::new(&gpu.device, gpu.format, None, 1, false);
        Ok(Self {
            window,
            surface,
            config,
            gpu,
            targets,
            egui,
            input,
            painter,
            start: Instant::now(),
        })
    }

    fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.gpu.device, &self.config);
        self.targets = self.gpu.targets([w, h]);
    }

    fn draw(&mut self, studio: &mut Studio) -> Result<()> {
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.gpu.device, &self.config);
                return Ok(());
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let raw = self.input.take_egui_input(&self.window);
        let mut viewport = egui::Rect::ZERO;
        let output = self.egui.run(raw, |ctx| {
            studio.panel(ctx);
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    let (r, response) =
                        ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
                    viewport = r;
                    let delta = ui.input(|i| i.pointer.delta());
                    if response.dragged_by(egui::PointerButton::Primary) && studio.view < 6 {
                        if studio.view != 0 {
                            let old = studio.camera().0;
                            studio.yaw = old.yaw.to_degrees() - 180.;
                            studio.pitch = -old.pitch.to_degrees();
                            studio.orthographic = true;
                            studio.view = 0;
                        }
                        studio.yaw = (studio.yaw - delta.x * 0.36).rem_euclid(360.);
                        studio.pitch = (studio.pitch + delta.y * 0.36).clamp(-89., 89.);
                    }
                    if studio.view >= 6 && response.dragged_by(egui::PointerButton::Primary) {
                        studio.interior_look[0] -= delta.x * 0.23;
                        studio.interior_look[1] =
                            (studio.interior_look[1] + delta.y * 0.23).clamp(-85., 85.);
                    }
                    if response.dragged_by(egui::PointerButton::Secondary) {
                        let (camera, _) = studio.camera();
                        let right = camera.direction().cross(camera.view_up()).normalize();
                        studio.pan += (-right * delta.x + camera.view_up() * delta.y)
                            * if studio.view < 6 {
                                studio.distance * 0.0016
                            } else {
                                0.0015
                            };
                    }
                    if response.hovered() {
                        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                        studio.distance =
                            (studio.distance * (-0.0025 * scroll).exp()).clamp(0.55, 65.);
                    }
                });
        });
        self.input
            .handle_platform_output(&self.window, output.platform_output);
        let ppp = output.pixels_per_point;
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: ppp,
        };
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        let x = (viewport.min.x * ppp).clamp(0., self.config.width as f32);
        let y = (viewport.min.y * ppp).clamp(0., self.config.height as f32);
        let w = (viewport.width() * ppp).min(self.config.width as f32 - x);
        let h = (viewport.height() * ppp).min(self.config.height as f32 - y);
        self.gpu.render(
            &mut encoder,
            studio,
            &view,
            &self.targets,
            [x, y, w, h],
            self.start.elapsed().as_secs_f32(),
        );
        for (id, delta) in &output.textures_delta.set {
            self.painter
                .update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
        }
        let jobs = self.egui.tessellate(output.shapes, ppp);
        let callbacks = self.painter.update_buffers(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &jobs,
            &screen,
        );
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("studio controls"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        self.painter
            .render(&mut pass.forget_lifetime(), &jobs, &screen);
        self.gpu
            .queue
            .submit(callbacks.into_iter().chain([encoder.finish()]));
        frame.present();
        for id in &output.textures_delta.free {
            self.painter.free_texture(id);
        }
        Ok(())
    }
}

impl ApplicationHandler for Preview {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match WindowState::new(el, self.size) {
            Ok(state) => {
                state.window.request_redraw();
                self.state = Some(state);
            }
            Err(e) => {
                self.error = Some(e.to_string());
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else {
            return;
        };
        let response = state.input.on_window_event(&state.window, &event);
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                state.resize(size.width, size.height);
                state.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = state.draw(&mut self.studio) {
                    self.error = Some(e.to_string());
                    el.exit();
                }
            }
            _ if response.repaint => state.window.request_redraw(),
            _ => {}
        }
    }
}
