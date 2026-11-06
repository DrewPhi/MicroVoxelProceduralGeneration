//! Small egui overlay composited at output resolution after the terrain scene.
use crate::settings::{SettingsChanges, SettingsStats, VisualSettings};
use egui_wgpu::{Renderer, ScreenDescriptor};
use winit::{
    event::{ElementState, WindowEvent},
    keyboard::{KeyCode, PhysicalKey},
    window::Window,
};

pub struct SettingsUi {
    context: egui::Context,
    input: egui_winit::State,
    renderer: Renderer,
    settings: VisualSettings,
    loading_draft: VisualSettings,
    open: bool,
    paint_jobs: Vec<egui::ClippedPrimitive>,
    free_textures: Vec<egui::TextureId>,
    screen: ScreenDescriptor,
}

impl SettingsUi {
    pub fn new(
        window: &Window,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        settings: VisualSettings,
    ) -> Self {
        let context = egui::Context::default();
        context.set_visuals(egui::Visuals::dark());
        context.style_mut(|style| {
            style.spacing.item_spacing = egui::vec2(10., 8.);
            style.spacing.slider_width = 190.;
            style.visuals.window_fill = egui::Color32::from_rgb(18, 25, 31);
            style.visuals.panel_fill = egui::Color32::from_rgb(18, 25, 31);
        });
        let input = egui_winit::State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        Self {
            context,
            input,
            renderer: Renderer::new(device, format, None, 1, false),
            loading_draft: settings.clone(),
            settings,
            open: false,
            paint_jobs: Vec::new(),
            free_textures: Vec::new(),
            screen: ScreenDescriptor {
                size_in_pixels: [window.inner_size().width, window.inner_size().height],
                pixels_per_point: window.scale_factor() as f32,
            },
        }
    }

    pub fn menu_open(&self) -> bool {
        self.open
    }

    pub fn settings(&self) -> &VisualSettings {
        &self.settings
    }

    /// Returns true for input owned by the menu. The caller still handles resize,
    /// close, focus and redraw events, and releases cursor grab while open.
    pub fn handle_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        if let WindowEvent::KeyboardInput { event, .. } = event {
            if event.physical_key == PhysicalKey::Code(KeyCode::Escape) {
                if event.state == ElementState::Pressed && !event.repeat {
                    self.open = !self.open;
                }
                return true;
            }
        }
        let response = self.input.on_window_event(window, event);
        self.open
            && (response.consumed
                || matches!(
                    event,
                    WindowEvent::KeyboardInput { .. }
                        | WindowEvent::MouseInput { .. }
                        | WindowEvent::MouseWheel { .. }
                        | WindowEvent::CursorMoved { .. }
                        | WindowEvent::Ime(_)
                ))
    }

    /// Build UI and upload its tiny draw buffers before the scene command buffer
    /// is submitted. Quality and streaming values commit only on the Apply button.
    pub fn prepare(
        &mut self,
        window: &Window,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        stats: &SettingsStats,
    ) -> SettingsChanges {
        let previous = self.settings.clone();
        let mut changes = SettingsChanges::default();
        let input = self.input.take_egui_input(window);
        let output = self.context.run(input, |ctx| {
            egui::Area::new(egui::Id::new("frontier telemetry"))
                .anchor(egui::Align2::LEFT_TOP, [14., 14.])
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        let fps = if stats.frame_ms > 0. {
                            1000. / stats.frame_ms
                        } else {
                            0.
                        };
                        ui.label(
                            egui::RichText::new(format!(
                                "{fps:.0} FPS  ·  {:.1} ms  ·  Esc settings",
                                stats.frame_ms
                            ))
                            .strong(),
                        );
                        ui.horizontal(|ui| {
                            if let Some(ms) = stats.gpu_ms {
                                ui.label(format!("Terrain GPU {ms:.1} ms  ·"));
                            }
                            ui.label(format!(
                                "{} pending  ·  {}",
                                stats.pending,
                                if stats.quality_ready {
                                    "quality ready"
                                } else {
                                    "refining terrain"
                                }
                            ));
                        });
                        let mut memory = format!(
                            "Terrain GPU {:.0} MiB  ·  cache {:.0} MiB",
                            stats.gpu_mib, stats.cache_mib
                        );
                        if let Some(rss) = stats.process_mib {
                            memory.push_str(&format!("  ·  process {rss:.0} MiB"));
                        }
                        ui.label(memory);
                        ui.small("Terrain GPU figures exclude atmosphere and UI.");
                    });
                });
            if !self.open {
                return;
            }
            ctx.layer_painter(egui::LayerId::new(
                egui::Order::Background,
                egui::Id::new("settings dim"),
            ))
            .rect_filled(ctx.screen_rect(), 0., egui::Color32::from_black_alpha(105));
            let available = ctx.screen_rect().height() - 50.;
            egui::Window::new("Frontier · visual settings")
                .collapsible(false)
                .resizable(false)
                .default_width(480.)
                .max_height(available.max(100.))
                .anchor(egui::Align2::CENTER_CENTER, [0., 0.])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Atmosphere changes are live.");
                        if ui.button("Resume · Esc").clicked() {
                            self.open = false;
                        }
                    });
                    egui::ScrollArea::vertical()
                        .max_height((available - 80.).max(80.))
                        .show(ui, |ui| {
                            ui.heading("Image quality");
                            ui.add(
                                egui::Slider::new(&mut self.loading_draft.pixel_error, 0.5..=12.)
                                    .logarithmic(true)
                                    .text("Terrain error (px)"),
                            )
                            .on_hover_text(
                                "Lower values request more terrain detail and may reduce FPS.",
                            );
                            ui.add(
                                egui::Slider::new(
                                    &mut self.loading_draft.view_distance,
                                    100.0..=40_000.0,
                                )
                                .logarithmic(true)
                                .text("View distance (m)"),
                            );
                            ui.add(
                                egui::Slider::new(&mut self.loading_draft.render_scale, 0.5..=1.)
                                    .text("Render scale"),
                            );
                            ui.horizontal(|ui| {
                                ui.label("MSAA");
                                for samples in [1, 4] {
                                    ui.selectable_value(
                                        &mut self.loading_draft.msaa,
                                        samples,
                                        if samples == 1 {
                                            "Off".to_owned()
                                        } else {
                                            format!("{samples}×")
                                        },
                                    );
                                }
                            });
                            ui.checkbox(
                                &mut self.loading_draft.voxel_filter,
                                "Filter distant voxel detail",
                            );
                            ui.separator();
                            ui.heading("Light & atmosphere");
                            let mut clear_air = !self.settings.atmosphere;
                            if ui
                                .checkbox(&mut clear_air, "Clear air · disable atmosphere")
                                .changed()
                            {
                                self.settings.atmosphere = !clear_air;
                            }
                            ui.add(
                                egui::Slider::new(&mut self.settings.sun_elevation, -5.0..=85.0)
                                    .text("Sun elevation (°)"),
                            );
                            ui.add(
                                egui::Slider::new(&mut self.settings.sun_azimuth, 0.0..=360.0)
                                    .text("Sun azimuth (°)"),
                            );
                            ui.add(
                                egui::Slider::new(&mut self.settings.exposure, 0.4..=2.5)
                                    .text("Exposure"),
                            );
                            ui.add_enabled_ui(self.settings.atmosphere, |ui| {
                                ui.add(
                                    egui::Slider::new(
                                        &mut self.settings.fog_density,
                                        0.0..=0.00015,
                                    )
                                    .max_decimals(6)
                                    .text("Haze density"),
                                );
                                ui.add(
                                    egui::Slider::new(
                                        &mut self.settings.fog_height_falloff,
                                        0.0001..=0.01,
                                    )
                                    .logarithmic(true)
                                    .text("Height falloff"),
                                );
                                ui.add(
                                    egui::Slider::new(&mut self.settings.cloud_coverage, 0.0..=1.0)
                                        .text("Cloud coverage"),
                                );
                                ui.add(
                                    egui::Slider::new(&mut self.settings.cloud_speed, 0.0..=0.1)
                                        .max_decimals(3)
                                        .text("Cloud speed"),
                                );
                                ui.add(
                                    egui::Slider::new(&mut self.settings.overcast, 0.0..=1.0)
                                        .text("Overcast"),
                                );
                            });
                            ui.separator();
                            ui.heading("Loading resources");
                            ui.label(
                                "More resources can reduce FPS. Quality and loading changes take effect on Apply.",
                            );
                            let max_workers = std::thread::available_parallelism()
                                .map(usize::from)
                                .unwrap_or(4)
                                .min(4);
                            ui.add(
                                egui::Slider::new(&mut self.loading_draft.workers, 1..=max_workers)
                                    .text("Workers"),
                            );
                            ui.add(
                                egui::Slider::new(&mut self.loading_draft.refinements, 1..=32)
                                    .logarithmic(true)
                                    .text("Refinements / batch"),
                            );
                            ui.add(
                                egui::Slider::new(&mut self.loading_draft.cache_mib, 128..=2048)
                                    .logarithmic(true)
                                    .text("Cache budget (MiB)"),
                            );
                            ui.add(
                                egui::Slider::new(&mut self.loading_draft.upload_mib, 1..=64)
                                    .logarithmic(true)
                                    .text("Upload / frame (MiB)"),
                            );
                            let dirty = self.settings.pixel_error != self.loading_draft.pixel_error
                                || self.settings.view_distance != self.loading_draft.view_distance
                                || self.settings.render_scale != self.loading_draft.render_scale
                                || self.settings.msaa != self.loading_draft.msaa
                                || self.settings.voxel_filter != self.loading_draft.voxel_filter
                                || self.settings.workers != self.loading_draft.workers
                                || self.settings.refinements != self.loading_draft.refinements
                                || self.settings.cache_mib != self.loading_draft.cache_mib
                                || self.settings.upload_mib != self.loading_draft.upload_mib;
                            if ui
                                .add_enabled(dirty, egui::Button::new("Apply quality & loading"))
                                .clicked()
                            {
                                self.settings.pixel_error = self.loading_draft.pixel_error;
                                self.settings.view_distance = self.loading_draft.view_distance;
                                self.settings.render_scale = self.loading_draft.render_scale;
                                self.settings.msaa = self.loading_draft.msaa;
                                self.settings.voxel_filter = self.loading_draft.voxel_filter;
                                self.settings.workers = self.loading_draft.workers;
                                self.settings.refinements = self.loading_draft.refinements;
                                self.settings.cache_mib = self.loading_draft.cache_mib;
                                self.settings.upload_mib = self.loading_draft.upload_mib;
                                changes.loading_applied = true;
                            }
                        });
                });
        });
        self.input
            .handle_platform_output(window, output.platform_output);
        self.screen = ScreenDescriptor {
            size_in_pixels: [window.inner_size().width, window.inner_size().height],
            pixels_per_point: output.pixels_per_point,
        };
        for (id, delta) in &output.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }
        // Surface loss or minimization may skip finish_frame; retain releases
        // until the next submitted presentation frame instead of leaking them.
        self.free_textures.extend(output.textures_delta.free);
        self.paint_jobs = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        // This UI uses no custom callbacks, so update_buffers produces no auxiliary command buffers.
        let callbacks =
            self.renderer
                .update_buffers(device, queue, encoder, &self.paint_jobs, &self.screen);
        debug_assert!(callbacks.is_empty());
        let mut visual_previous = previous;
        visual_previous.pixel_error = self.settings.pixel_error;
        visual_previous.view_distance = self.settings.view_distance;
        visual_previous.render_scale = self.settings.render_scale;
        visual_previous.msaa = self.settings.msaa;
        visual_previous.voxel_filter = self.settings.voxel_filter;
        visual_previous.workers = self.settings.workers;
        visual_previous.refinements = self.settings.refinements;
        visual_previous.cache_mib = self.settings.cache_mib;
        visual_previous.upload_mib = self.settings.upload_mib;
        changes.visual_changed = visual_previous != self.settings;
        changes
    }

    /// The target must be the resolved, single-sampled output view. Draw after
    /// upscaling the scene so text remains sharp at reduced render scale.
    pub fn render(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("settings overlay"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
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
        self.renderer
            .render(&mut pass.forget_lifetime(), &self.paint_jobs, &self.screen);
    }

    /// Call after submitting the frame that references these textures.
    pub fn finish_frame(&mut self) {
        for id in self.free_textures.drain(..) {
            self.renderer.free_texture(&id);
        }
    }
}
