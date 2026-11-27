//! Small egui overlay composited at output resolution after the terrain scene.
use crate::lod_curve::{distance_at, LodCurve, DISTANCES, LAST_KNOT, TERRAIN_DISTANCES};
use crate::settings::{ResourceProfile, SettingsChanges, SettingsStats, VisualSettings};
use egui_wgpu::{Renderer, ScreenDescriptor};
use winit::{
    event::{ElementState, WindowEvent},
    keyboard::{KeyCode, PhysicalKey},
    window::Window,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum LodEditor {
    Vegetation,
    Terrain,
}

fn distance_curve_editor(
    ui: &mut egui::Ui,
    curve: &mut LodCurve,
    drag: &mut Option<(f32, LodCurve)>,
    distances: &[f32; DISTANCES.len()],
) {
    let (outer, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 245.),
        egui::Sense::click_and_drag(),
    );
    let plot = egui::Rect::from_min_max(
        outer.min + egui::vec2(102., 13.),
        outer.max - egui::vec2(22., 33.),
    );
    let knot_at = |x: f32| ((x - plot.left()) / plot.width()).clamp(0., 1.) * LAST_KNOT;
    let value_at = |y: f32| ((plot.center().y - y) / (0.5 * plot.height())).clamp(-1., 1.);
    let point_at = |knot: f32, value: f32| {
        egui::pos2(
            plot.left() + plot.width() * knot / LAST_KNOT,
            plot.center().y - 0.5 * plot.height() * value,
        )
    };
    if response.drag_started() {
        if let Some(origin) = ui
            .input(|input| input.pointer.press_origin())
            .filter(|p| plot.contains(*p))
        {
            *drag = Some((knot_at(origin.x), curve.clone()));
        }
    }
    if response.dragged() || response.drag_stopped() {
        if let (Some((knot, start)), Some(pointer)) =
            (drag.as_ref(), response.interact_pointer_pos())
        {
            *curve = start.clone();
            curve.bend(*knot, value_at(pointer.y));
        }
    } else if response.clicked() {
        if let Some(pointer) = response
            .interact_pointer_pos()
            .filter(|p| plot.contains(*p))
        {
            curve.bend(knot_at(pointer.x), value_at(pointer.y));
        }
    }
    if response.drag_stopped() {
        *drag = None;
    }

    let painter = ui.painter();
    let accent = egui::Color32::from_rgb(125, 220, 178);
    let grid = ui.visuals().widgets.noninteractive.bg_stroke;
    let font = egui::FontId::proportional(12.);
    let label = ui.visuals().text_color();
    painter.rect_filled(plot, 3., ui.visuals().extreme_bg_color);
    for (value, text) in [(1., "Max visible"), (0., "Automatic"), (-1., "Coarsest")] {
        let left = point_at(0., value);
        painter.line_segment([left, point_at(LAST_KNOT, value)], grid);
        painter.text(
            left - egui::vec2(8., 0.),
            egui::Align2::RIGHT_CENTER,
            text,
            font.clone(),
            label,
        );
    }
    for knot in [0., 5., 10., 14., 19., 22., LAST_KNOT] {
        let bottom = point_at(knot, -1.);
        painter.line_segment([bottom, point_at(knot, 1.)], grid);
        painter.text(
            bottom + egui::vec2(0., 8.),
            egui::Align2::CENTER_TOP,
            format!("{:.0} m", distance_at(knot, distances)),
            font.clone(),
            label,
        );
    }
    let line = (0..=156)
        .map(|i| {
            let knot = i as f32 * LAST_KNOT / 156.;
            point_at(knot, curve.sample(knot))
        })
        .collect();
    painter.add(egui::Shape::line(line, egui::Stroke::new(2.5_f32, accent)));

    let active = drag.as_ref().map(|(knot, _)| *knot).or_else(|| {
        response
            .hover_pos()
            .filter(|p| plot.contains(*p))
            .map(|p| knot_at(p.x))
    });
    if let Some(knot) = active {
        let point = point_at(knot, curve.sample(knot));
        painter.line_segment([point_at(knot, 1.), point_at(knot, -1.)], grid);
        painter.circle_filled(point, 5., accent);
        let right = point.x < plot.right() - 75.;
        painter.text(
            egui::pos2(point.x + if right { 8. } else { -8. }, plot.top() + 7.),
            if right {
                egui::Align2::LEFT_TOP
            } else {
                egui::Align2::RIGHT_TOP
            },
            format!("{:.0} m", distance_at(knot, distances)),
            font,
            accent,
        );
    }
}

pub struct SettingsUi {
    context: egui::Context,
    input: egui_winit::State,
    renderer: Renderer,
    settings: VisualSettings,
    rolling: bool,
    loading_draft: VisualSettings,
    open: bool,
    lod_editor: Option<LodEditor>,
    lod_curve_drag: Option<(f32, LodCurve)>,
    paint_jobs: Vec<egui::ClippedPrimitive>,
    free_textures: Vec<egui::TextureId>,
    screen: ScreenDescriptor,
    started: std::time::Instant,
    settings_error: Option<String>,
    player_overlay: crate::player_edit::Overlay,
    radar: crate::weather_panel::Panel,
}

impl SettingsUi {
    pub fn new(
        window: &Window,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        settings: VisualSettings,
        rolling: bool,
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
            started: std::time::Instant::now(),
            settings_error: None,
            player_overlay: Default::default(),
            radar: Default::default(),
            context,
            input,
            renderer: Renderer::new(device, format, None, 1, false),
            loading_draft: settings.clone(),
            settings,
            rolling,
            open: false,
            lod_editor: None,
            lod_curve_drag: None,
            paint_jobs: Vec::new(),
            free_textures: Vec::new(),
            screen: ScreenDescriptor {
                size_in_pixels: [window.inner_size().width, window.inner_size().height],
                pixels_per_point: window.scale_factor() as f32,
            },
        }
    }

    pub fn set_player_overlay(&mut self, overlay: crate::player_edit::Overlay) {
        self.player_overlay = overlay;
    }
    pub fn restore_after_error(&mut self, previous: VisualSettings, message: String) {
        self.loading_draft = previous.clone();
        self.settings = previous;
        self.settings_error = Some(message);
        self.open = true;
    }
    pub fn menu_open(&self) -> bool {
        self.open
    }

    pub fn radar_open(&self) -> bool {
        self.radar.open
    }

    pub fn owns_cursor(&self) -> bool {
        self.open || self.radar.open
    }

    pub fn set_walking(&mut self, walking: bool) {
        self.settings.walking = walking;
        self.loading_draft.walking = walking;
    }

    pub fn settings(&self) -> &VisualSettings {
        &self.settings
    }

    /// Returns true for input owned by the menu. The caller still handles resize,
    /// close, focus and redraw events, and releases cursor grab while open.
    pub fn handle_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        if let WindowEvent::KeyboardInput { event, .. } = event {
            if event.physical_key == PhysicalKey::Code(KeyCode::F4) {
                if event.state == ElementState::Pressed && !event.repeat {
                    self.radar.open = !self.radar.open;
                }
                return true;
            }
            if event.physical_key == PhysicalKey::Code(KeyCode::Escape) {
                if event.state == ElementState::Pressed && !event.repeat {
                    if self.lod_editor.take().is_some() {
                        self.lod_curve_drag = None;
                    } else if self.radar.open && !self.open {
                        self.radar.open = false;
                    } else {
                        self.open = !self.open;
                    }
                }
                return true;
            }
        }
        let response = self.input.on_window_event(window, event);
        if response.consumed
            && matches!(
                event,
                WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. }
            )
        {
            return true;
        }
        self.owns_cursor()
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
        dimensions: [u32; 2],
        observation: &crate::weather_panel::Observation<'_>,
    ) -> SettingsChanges {
        let previous = self.settings.clone();
        let mut changes = SettingsChanges::default();
        // The standard convenience adapter queries X11 geometry every frame.
        // Feed its public RawInput the event-maintained output dimensions instead.
        let native_scale = window.scale_factor() as f32;
        let points = egui::vec2(dimensions[0] as f32, dimensions[1] as f32)
            / (native_scale * self.context.zoom_factor());
        let raw = self.input.egui_input_mut();
        raw.time = Some(self.started.elapsed().as_secs_f64());
        raw.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, points));
        raw.viewport_id = egui::ViewportId::ROOT;
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(native_scale);
        let input = raw.take();
        let output = self.context.run(input, |ctx| {
            if let Some(flight) = stats.flight.filter(|_| !self.open) {
                crate::flight_instruments::show(ctx, flight);
            }
            if !self.open && !self.radar.open {
                if let (Some(pointer), Some(control)) = (stats.cockpit_pointer, stats.cockpit_control) {
                    let screen = ctx.screen_rect();
                    let anchor = screen.min + egui::vec2(pointer.x * screen.width(), pointer.y * screen.height());
                    egui::Area::new(egui::Id::new("cockpit control hint"))
                        .fixed_pos(anchor + egui::vec2(16., 18.)).interactable(false)
                        .show(ctx, |ui| {
                            egui::Frame::popup(ui.style()).show(ui, |ui| ui.small(control.label()));
                        });
                }
            }
            if !self.player_overlay.text.is_empty() && !self.open {
                egui::Area::new(egui::Id::new("player inventory"))
                    .anchor(egui::Align2::CENTER_BOTTOM,[0.,-18.]).interactable(false).show(ctx,|ui| {
                        egui::Frame::popup(ui.style()).show(ui,|ui| {
                            ui.label(&self.player_overlay.text);
                            ui.add(egui::ProgressBar::new(self.player_overlay.charge).desired_width(240.));
                        });
                    });
                let painter=ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground,egui::Id::new("native pickup preview")));
                let rect=ctx.screen_rect();
                let project=|p:[f32;2]| egui::pos2(rect.left()+(p[0]+1.)*0.5*rect.width(),rect.top()+(1.-p[1])*0.5*rect.height());
                for line in &self.player_overlay.lines {
                    painter.line_segment([project(line[0]),project(line[1])],egui::Stroke::new(1.5,egui::Color32::YELLOW));
                }
                painter.circle_stroke(rect.center(),3.,egui::Stroke::new(1.,egui::Color32::WHITE));
            }
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
                        if stats.can_board && !self.open { ui.label("V · Board aircraft"); }
                        ui.horizontal(|ui| {
                            if let Some(ms) = stats.gpu_full_ms {
                                ui.label(format!("Render GPU {ms:.1} ms  ·"));
                            } else if let Some(ms) = stats.gpu_ms {
                                ui.label(format!("Opaque GPU {ms:.1} ms  ·"));
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
                        if let Some(available) = stats.available_mib { ui.small(format!("Available system RAM: {available} MiB")); }
                        if stats.background_paused {
                            ui.colored_label(egui::Color32::YELLOW, "Low available RAM · background builds and uploads paused");
                        }
                        ui.small("Render GPU includes shadows/atmosphere/UI when supported; excludes CPU and prior uploads.");
                        if self.settings.vegetation {
                            ui.label(format!("Plants: {} instances · {} pending · {:.1} MiB GPU", stats.plant_instances, stats.plant_pending, stats.plant_gpu_mib));
                        }
                    });
                });
            if observation.frame.is_some() || self.settings.day_night {
                egui::Area::new(egui::Id::new("world clock"))
                    .anchor(egui::Align2::RIGHT_TOP, [-14., 12.]).show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(observation.clock_label());
                                if observation.frame.is_some() && ui.button("Weather radar · F4").clicked() {
                                    self.radar.open = !self.radar.open;
                                }
                            });
                        });
                    });
            }
            self.radar.show(ctx, observation, &mut self.settings);
            if !self.open {
                return;
            }
            ctx.layer_painter(egui::LayerId::new(
                egui::Order::Background,
                egui::Id::new("settings dim"),
            ))
            .rect_filled(ctx.screen_rect(), 0., egui::Color32::from_black_alpha(105));
            if let Some(editor) = self.lod_editor {
                let (title, distances, curve, current) = match editor {
                    LodEditor::Vegetation => (
                        "Vegetation LOD by distance", &DISTANCES,
                        &mut self.loading_draft.vegetation_lod_curve,
                        &mut self.settings.vegetation_lod_curve,
                    ),
                    LodEditor::Terrain => (
                        "Terrain LOD by distance", &TERRAIN_DISTANCES,
                        &mut self.loading_draft.terrain_lod_curve,
                        &mut self.settings.terrain_lod_curve,
                    ),
                };
                let mut open = true;
                egui::Window::new(title)
                    .open(&mut open)
                    .anchor(egui::Align2::CENTER_CENTER, [0., 0.])
                    .collapsible(false)
                    .default_width(620.)
                    .show(ctx, |ui| {
                        distance_curve_editor(ui, curve, &mut self.lod_curve_drag, distances);
                        ui.small("Drag vertically to blend between automatic, finer and coarser detail.");
                        ui.small(match editor {
                            LodEditor::Vegetation => "Native vegetation radius takes priority nearby. Plant draw ranges and rendering budgets still apply.",
                            LodEditor::Terrain => "Terrain detail follows the screen projection and available cache layers. Exact voxels and NAADF keep their own settings.",
                        });
                        ui.horizontal(|ui| {
                            if ui.button("Reset to automatic").clicked() {
                                *curve = LodCurve::default();
                            }
                            if ui.add_enabled(curve != current, egui::Button::new("Apply")).clicked() {
                                *current = curve.clone();
                            }
                        });
                    });
                if !open {
                    self.lod_editor = None;
                    self.lod_curve_drag = None;
                }
                return;
            }
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
                        if ui.button("Vegetation LOD curve").clicked() {
                            self.loading_draft.vegetation_lod_curve =
                                self.settings.vegetation_lod_curve.clone();
                            self.lod_editor = Some(LodEditor::Vegetation);
                        }
                        if self.rolling && !self.settings.native_lattice && ui.button("Terrain LOD curve").clicked() {
                            self.loading_draft.terrain_lod_curve =
                                self.settings.terrain_lod_curve.clone();
                            self.lod_editor = Some(LodEditor::Terrain);
                        }
                        if ui.button("Resume · Esc").clicked() {
                            self.open = false;
                        }
                    });
                    if let Some(flight) = stats.flight {
                        ui.horizontal(|ui| {
                            changes.toggle_airflow = ui.selectable_label(flight.show_airflow, "Show airflow · Y").clicked();
                            changes.toggle_flight_audio = ui.selectable_label(stats.flight_audio, "Flight sound").clicked();
                            changes.toggle_flight_camera_motion = ui.selectable_label(stats.flight_camera_motion, "Camera inertia · J").clicked();
                        });
                    }
                    if let Some(offset) = stats.cockpit_eye_offset {
                        ui.collapsing("Cockpit camera", |ui| {
                            let mut cm = offset * 100.;
                            let min = crate::aircraft::COCKPIT_OFFSET_MIN * 100.;
                            let max = crate::aircraft::COCKPIT_OFFSET_MAX * 100.;
                            let mut changed = false;
                            for (axis, label) in ["Side offset", "Height", "Fore / aft"].into_iter().enumerate() {
                                changed |= ui.add(egui::Slider::new(&mut cm[axis], min[axis]..=max[axis])
                                    .fixed_decimals(1).suffix(" cm").text(label)).changed();
                            }
                            if changed {
                                changes.cockpit_eye_offset = Some(cm / 100.);
                            }
                            if ui.button("Reset pilot view").clicked() {
                                changes.cockpit_eye_offset = Some(glam::Vec3::ZERO);
                            }
                        });
                    }
                    if let Some(message) = &self.settings_error {
                        ui.colored_label(egui::Color32::YELLOW, format!("Settings were not applied: {message}"));
                    }
                    egui::ScrollArea::vertical()
                        .max_height((available - 80.).max(80.))
                        .show(ui, |ui| {
                            ui.heading("World time");
                            crate::weather_panel::time_controls(ui, &mut self.settings);
                            if observation.frame.is_some() && ui.button("Open weather radar").clicked() {
                                self.radar.open = true;
                                self.open = false;
                            }
                            ui.heading("Image quality");
                            if !self.rolling {
                                ui.add(
                                egui::Slider::new(&mut self.loading_draft.pixel_error, 0.5..=12.)
                                    .logarithmic(true)
                                    .text("Terrain error (px)"),
                            )
                            .on_hover_text(
                                "Lower values request more terrain detail and may reduce FPS.",
                            );
                            }
                            let max_distance = if self.settings.native_lattice {
                                crate::native_lattice::MAX_FAR
                            } else if self.rolling {
                                crate::rolling_terrain::MAX_FAR
                            } else {
                                40_000.
                            };
                            ui.add(
                                egui::Slider::new(
                                    &mut self.loading_draft.view_distance,
                                    100.0..=max_distance,
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
                            ui.add_enabled(!self.rolling, egui::Checkbox::new(
                                &mut self.loading_draft.voxel_filter,
                                "Filter distant voxel detail",
                            ));
                            ui.separator();
                            ui.heading("Meadow & movement");
                            ui.checkbox(&mut self.settings.vegetation, "Procedural vegetation");
                            ui.checkbox(&mut self.settings.walking, "Walk on ground (F toggles flight)");
                            ui.checkbox(&mut self.settings.step_smoothing, "Ease camera over voxel steps (visual only)");
                            ui.separator();
                            ui.heading("Light & atmosphere");
                            ui.checkbox(&mut self.settings.beauty, "Meadow lighting, foliage occlusion and sky");
                            ui.checkbox(&mut self.settings.terrain_shadows, "Terrain shadows");
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
                            ui.add(egui::Slider::new(&mut self.settings.ground_bounce, 0.0..=2.0).text("Ground bounce"));
                            ui.add_enabled_ui(self.settings.atmosphere, |ui| {
                                ui.add(egui::Slider::new(&mut self.settings.sunbeams, 0.0..=2.0).text("Sunbeams"));
                                ui.add(egui::Slider::new(&mut self.settings.streaming_haze, 0.0..=1.0).text("Streaming haze"));
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
                            ui.heading("NAADF experiment");
                            ui.checkbox(&mut self.loading_draft.naadf_enabled, "Use exact voxel rays in test region");
                            ui.checkbox(&mut self.loading_draft.naadf_follow, "Follow player");
                            let max_voxels = if self.rolling {
                                crate::rolling_terrain::MAX_NAADF_RADIUS
                            } else {
                                crate::naadf::stream::MAX_RADIUS_VOXELS
                            };
                            ui.add(egui::Slider::new(
                                &mut self.loading_draft.naadf_radius_voxels, 1..=max_voxels,
                            ).text("NAADF radius (voxels)"));
                            ui.checkbox(&mut self.loading_draft.naadf_auto_memory,"Use available RAM for NAADF (leave system headroom)");
                            if !self.rolling {
                                ui.checkbox(&mut self.settings.visibility_benchmark,"Diagnostic matched white lighting");
                            }
                            ui.add(egui::Slider::new(&mut self.loading_draft.naadf_budget_mib, 16..=16384).logarithmic(true).text("NAADF build estimate ceiling (MiB)"));
                            if let Ok(roots) = crate::naadf::stream::radius_roots(self.loading_draft.naadf_radius_voxels) {
                                let radius_m = f64::from(roots) * crate::naadf::stream::root_size_m();
                                ui.small(format!("{radius_m:.2} m after rounding up to whole roots."));
                                let top = (roots as f64 * 2.).powi(3) * 4. / 1048576.;
                                ui.small(format!("Top grid {top:.1} MiB GPU, with at least {:.1} MiB for CPU and build copies plus voxel payloads.", top * 3.));
                            }
                            ui.small("Radius is the cube half-extent. Follow mode builds incoming strips as you move. Distant terrain keeps its renderer.");
                            ui.label(format!("{} · {:.1} MiB NAADF GPU",stats.naadf_status,stats.naadf_gpu_mib));
                            if ui.button("Rebuild NAADF here").clicked() {
                                changes.naadf_rebuild=true;
                                self.settings.naadf_auto_memory=self.loading_draft.naadf_auto_memory;
                                self.settings.naadf_enabled=self.loading_draft.naadf_enabled;
                                self.settings.naadf_follow=self.loading_draft.naadf_follow;
                                self.settings.naadf_radius_voxels=self.loading_draft.naadf_radius_voxels;
                                self.settings.naadf_budget_mib=self.loading_draft.naadf_budget_mib;
                                changes.loading_applied=true;
                            }
                            ui.separator();
                            ui.heading("Vegetation");
                            if ui.add(egui::Slider::new(
                                &mut self.loading_draft.vegetation_near_m, 0.0..=128.0,
                            ).text("Native vegetation radius (m)")).changed() {
                                self.loading_draft.resource_profile = ResourceProfile::Custom;
                            }
                            ui.small("Inside this radius, use full-detail plant geometry when the budget allows. 0 uses projected detail alone.");
                            ui.separator();
                            ui.heading("Performance & memory");
                            ui.label("Development targets smooth 30–60 FPS. Budgets grow lazily as content is needed.");
                            let profile_before = self.loading_draft.resource_profile;
                            ui.horizontal(|ui| {
                                for profile in [ResourceProfile::Development, ResourceProfile::Constrained, ResourceProfile::Custom] {
                                    ui.selectable_value(&mut self.loading_draft.resource_profile, profile, profile.label());
                                }
                            });
                            if profile_before != self.loading_draft.resource_profile {
                                self.loading_draft.apply_resource_profile(self.loading_draft.resource_profile, self.loading_draft.resource_alpine);
                            }
                            if ui.button("Refresh available RAM").on_hover_text("Re-measures free capacity. Close other large apps first; current game residency reduces the new recommendation.").clicked() {
                                self.loading_draft.memory = crate::settings::MemorySnapshot::detect();
                                self.loading_draft.apply_resource_profile(self.loading_draft.resource_profile, self.loading_draft.resource_alpine);
                            }
                            let memory = self.loading_draft.memory;
                            ui.small(format!("RAM available {} / {} MiB{} · suggested working budgets {} MiB",
                                memory.available_mib, memory.total_mib,
                                if memory.measured { "" } else { " (portable estimate)" }, memory.working_mib()));
                            ui.small("Integrated GPU memory shares RAM. CPU + GPU budgets count together; OS, drivers and temporary replacements need headroom. These are working-set limits, not a whole-process cap.");
                            let manual_before = self.loading_draft.clone();
                            let max_workers = std::thread::available_parallelism().map(usize::from).unwrap_or(4).min(16);
                            ui.add(egui::Slider::new(&mut self.loading_draft.workers, 1..=max_workers).text("Workers"));
                            ui.add(egui::Slider::new(&mut self.loading_draft.refinements, 1..=32).text("Refinements / batch"));
                            ui.add(egui::Slider::new(&mut self.loading_draft.native_radius_m, 0.0..=64.0).text("Minimum native radius (m)"));
                            ui.small(format!("Native voxel edge: {} cm. Larger radii need more memory.", frontier_world::voxel_size() * 100.));
                            let max_mib = memory.working_mib().max(2048);
                            for (field, label) in [
                                (&mut self.loading_draft.cache_mib, "Terrain CPU cache (MiB)"),
                                (&mut self.loading_draft.build_mib, "Build memory (MiB)"),
                                (&mut self.loading_draft.terrain_gpu_mib, "Terrain GPU (MiB)"),
                                (&mut self.loading_draft.plant_cpu_mib, "Plant CPU cache (MiB)"),
                                (&mut self.loading_draft.plant_gpu_mib, "Plant GPU cache (MiB)"),
                            ] { ui.add(egui::Slider::new(field, 0..=max_mib).logarithmic(true).text(label)); }
                            ui.add(egui::Slider::new(&mut self.loading_draft.triangle_budget, 100_000..=32_000_000).logarithmic(true).text("Plant triangles"));
                            ui.add(egui::Slider::new(&mut self.loading_draft.upload_mib, 1..=64).text("Upload / frame (MiB)"));
                            if manual_before != self.loading_draft { self.loading_draft.resource_profile = ResourceProfile::Custom; }
                            let combined = self.loading_draft.working_budgets().total_mib().saturating_add(if self.loading_draft.naadf_enabled {self.loading_draft.naadf_budget_mib}else{0});
                            ui.label(format!("Combined configured working budgets: {combined} MiB"));
                            if combined > memory.working_mib() {
                                ui.colored_label(egui::Color32::YELLOW, "Above available-memory recommendation: risk of paging and repeated evictions.");
                            }
                            let dirty = self.settings.naadf_auto_memory != self.loading_draft.naadf_auto_memory || self.settings.naadf_enabled != self.loading_draft.naadf_enabled
                                || self.settings.naadf_follow != self.loading_draft.naadf_follow
                                || self.settings.naadf_radius_voxels != self.loading_draft.naadf_radius_voxels
                                || self.settings.naadf_budget_mib != self.loading_draft.naadf_budget_mib
                                || self.settings.pixel_error != self.loading_draft.pixel_error
                                || self.settings.view_distance != self.loading_draft.view_distance
                                || self.settings.render_scale != self.loading_draft.render_scale
                                || self.settings.msaa != self.loading_draft.msaa
                                || self.settings.voxel_filter != self.loading_draft.voxel_filter
                                || self.settings.workers != self.loading_draft.workers
                                || self.settings.refinements != self.loading_draft.refinements
                                || self.settings.cache_mib != self.loading_draft.cache_mib
                                || self.settings.upload_mib != self.loading_draft.upload_mib
                                || self.settings.working_budgets() != self.loading_draft.working_budgets()
                                || self.settings.resource_profile != self.loading_draft.resource_profile
                                || self.settings.native_radius_m != self.loading_draft.native_radius_m
                                || self.settings.vegetation_near_m != self.loading_draft.vegetation_near_m
                                || self.settings.triangle_budget != self.loading_draft.triangle_budget;
                            if ui
                                .add_enabled(dirty, egui::Button::new("Apply quality & loading"))
                                .clicked()
                            {
                                self.settings_error = None;
                                self.settings.naadf_auto_memory=self.loading_draft.naadf_auto_memory;
                                self.settings.naadf_enabled=self.loading_draft.naadf_enabled;
                                self.settings.naadf_follow=self.loading_draft.naadf_follow;
                                self.settings.naadf_radius_voxels=self.loading_draft.naadf_radius_voxels;
                                self.settings.naadf_budget_mib=self.loading_draft.naadf_budget_mib;
                                self.settings.pixel_error = self.loading_draft.pixel_error;
                                self.settings.view_distance = self.loading_draft.view_distance;
                                self.settings.render_scale = self.loading_draft.render_scale;
                                self.settings.msaa = self.loading_draft.msaa;
                                self.settings.voxel_filter = self.loading_draft.voxel_filter;
                                self.settings.workers = self.loading_draft.workers;
                                self.settings.refinements = self.loading_draft.refinements;
                                self.settings.cache_mib = self.loading_draft.cache_mib;
                                self.settings.upload_mib = self.loading_draft.upload_mib;
                                self.settings.resource_profile = self.loading_draft.resource_profile;
                                self.settings.resource_alpine = self.loading_draft.resource_alpine;
                                self.settings.memory = self.loading_draft.memory;
                                self.settings.native_radius_m = self.loading_draft.native_radius_m;
                                self.settings.vegetation_near_m = self.loading_draft.vegetation_near_m;
                                self.settings.build_mib = self.loading_draft.build_mib;
                                self.settings.terrain_gpu_mib = self.loading_draft.terrain_gpu_mib;
                                self.settings.plant_cpu_mib = self.loading_draft.plant_cpu_mib;
                                self.settings.plant_gpu_mib = self.loading_draft.plant_gpu_mib;
                                self.settings.triangle_budget = self.loading_draft.triangle_budget;
                                changes.loading_applied = true;
                            }
                        });
                });
        });
        self.input
            .handle_platform_output(window, output.platform_output);
        self.screen = ScreenDescriptor {
            size_in_pixels: dimensions,
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
        visual_previous.resource_profile = self.settings.resource_profile;
        visual_previous.resource_alpine = self.settings.resource_alpine;
        visual_previous.memory = self.settings.memory;
        visual_previous.naadf_auto_memory = self.settings.naadf_auto_memory;
        visual_previous.naadf_enabled = self.settings.naadf_enabled;
        visual_previous.naadf_follow = self.settings.naadf_follow;
        visual_previous.naadf_radius_voxels = self.settings.naadf_radius_voxels;
        visual_previous.naadf_budget_mib = self.settings.naadf_budget_mib;
        visual_previous.native_radius_m = self.settings.native_radius_m;
        visual_previous.vegetation_near_m = self.settings.vegetation_near_m;
        visual_previous.build_mib = self.settings.build_mib;
        visual_previous.terrain_gpu_mib = self.settings.terrain_gpu_mib;
        visual_previous.plant_cpu_mib = self.settings.plant_cpu_mib;
        visual_previous.plant_gpu_mib = self.settings.plant_gpu_mib;
        visual_previous.triangle_budget = self.settings.triangle_budget;
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
