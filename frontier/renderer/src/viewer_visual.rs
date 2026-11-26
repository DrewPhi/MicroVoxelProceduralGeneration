//! Interactive presentation controls. Canonical source and edits never depend on these.
use super::*;
use crate::settings::{SettingsStats, VisualSettings};
impl Viewer {
    pub(super) fn visual_input(&mut self, event: &WindowEvent) -> bool {
        if self.benchmark_input_lock {
            return false;
        }
        let Some(ui) = &mut self.settings_ui else {
            return false;
        };
        let was_open = ui.owns_cursor();
        let consumed = ui.handle_event(self.window.as_ref().unwrap(), event);
        let open = ui.owns_cursor();
        if open != was_open {
            self.keys.clear();
            self.grab(!open);
        }
        consumed
    }
    pub(super) fn update_visual_ui(
        &mut self,
        frame_ms: f64,
        frame: Option<&world_weather::Frame>,
        fraction: f32,
    ) -> Result<()> {
        let observation = crate::weather_panel::Observation {
            achieved: self.weather_clock.achieved,
            frame,
            fraction,
            seconds: self.weather_clock.seconds,
            solar_day: self.weather_clock.solar_day(),
            camera: self.display_camera(),
            aspect: self.width as f32 / self.height.max(1) as f32,
        };
        let (Some(ui), Some(gpu), Some(window)) = (&mut self.settings_ui, &self.gpu, &self.window)
        else {
            return Ok(());
        };
        self.display_frame_ms = if self.display_frame_ms == 0. {
            frame_ms
        } else {
            self.display_frame_ms * 0.9 + frame_ms * 0.1
        };
        let mut stats = SettingsStats {
            can_board: self.parked_aircraft.as_ref().is_some_and(|(_, pose)| {
                aircraft::Aircraft::can_board(&self.world, *pose, self.camera.position)
            }),
            flight: self
                .aircraft
                .as_ref()
                .map(|plane| plane.instruments(plane.exterior_pose().unwrap_or(self.camera))),
            flight_audio: self.flight_audio.enabled,
            flight_camera_motion: self
                .aircraft
                .as_ref()
                .is_some_and(|p| p.feedback.camera_motion),
            frame_ms: self.display_frame_ms,
            process_mib: self.resource_process_mib,
            available_mib: self.resource_available_mib,
            background_paused: self.resource_pressure,
            ..Default::default()
        };
        if let Some(probe) = &self.gpu_probe {
            stats.gpu_ms = probe.stats.summary().p50_ms;
            stats.gpu_full_ms = probe
                .full_supported
                .then(|| probe.full_stats.summary().p50_ms)
                .flatten();
        }
        if let Some(view) = &self.adaptive_view {
            stats.pending = view.pending_count;
            stats.gpu_mib = view.stats.allocated_bytes as f64 / 1048576.;
            stats.cache_mib = view.cache_bytes as f64 / 1048576.;
            stats.quality_ready = view.quality_ready;
        } else if let Some(rolling) = &gpu.rolling {
            stats.gpu_mib = rolling.gpu_bytes() as f64 / 1048576.;
            stats.quality_ready = true;
        }
        if let Some(experiment) = &gpu.naadf {
            stats.naadf_status =
                if experiment.ready() && experiment.bounds_for(self.camera).is_none() {
                    "Outside fixed NAADF region · rebuild here".into()
                } else {
                    format!("{} (stage {})", experiment.status, experiment.stage)
                };
            stats.naadf_gpu_mib = experiment.gpu_bytes() as f64 / 1048576.;
        }
        stats.plant_instances = gpu.vegetation_stats.selected_instances;
        stats.plant_pending = gpu.vegetation_stats.pending_jobs;
        stats.plant_gpu_mib = gpu
            .vegetation
            .as_ref()
            .map_or(0., |p| p.stats().resident_bytes as f64 / 1048576.);
        let was_open = ui.owns_cursor();
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let changes = ui.prepare(
            window,
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &stats,
            [self.width, self.height],
            &observation,
        );
        gpu.queue.submit(Some(encoder.finish()));
        let now_open = ui.owns_cursor();
        let mut settings = ui.settings().clone();
        if was_open != now_open {
            self.keys.clear();
            self.grab(!now_open);
        }
        if settings.day_night {
            settings.atmosphere = true;
        }
        if self.world.vegetation_enabled() != settings.vegetation {
            if settings.vegetation {
                frontier_world::vegetation::prewarm_models();
            }
            self.world = self
                .world
                .clone()
                .with_vegetation_enabled(settings.vegetation);
        }
        if self.visual_settings.walking != settings.walking {
            if settings.walking {
                if self.aircraft.is_some() && !self.leave_aircraft() {
                    settings.walking = false;
                    if let Some(ui) = &mut self.settings_ui {
                        ui.set_walking(false);
                    }
                }
            }
            self.walking.reset();
        }
        let previous = self.visual_settings.clone();
        self.visual_settings = settings;
        if changes.loading_applied {
            if let Err(error) = self.apply_visual_quality() {
                self.visual_settings = previous.clone();
                if let Some(ui) = &mut self.settings_ui {
                    ui.restore_after_error(previous, error.to_string());
                }
            }
        }
        if changes.toggle_airflow {
            if let Some(plane) = &mut self.aircraft {
                plane.toggle_airflow();
            }
        }
        if changes.toggle_flight_audio {
            self.flight_audio.enabled = !self.flight_audio.enabled;
        }
        if changes.toggle_flight_camera_motion {
            if let Some(plane) = &mut self.aircraft {
                plane.toggle_camera_motion(&mut self.camera, &self.world);
            }
        }
        if let Some(gpu) = &mut self.gpu {
            if changes.naadf_rebuild {
                gpu.naadf_serial = gpu.naadf_serial.wrapping_add(1);
            }
        }
        Ok(())
    }
    pub(super) fn update_resource_throughput(&mut self, frame_ms: f64) -> Result<()> {
        if self.resource_checked.elapsed().as_secs_f64() >= 1. {
            self.resource_checked = Instant::now();
            let memory = settings::MemorySnapshot::detect();
            self.resource_available_mib = memory.measured.then_some(memory.available_mib);
            self.resource_process_mib =
                std::fs::read_to_string("/proc/self/status")
                    .ok()
                    .and_then(|text| {
                        text.lines()
                            .find(|l| l.starts_with("VmRSS:"))
                            .and_then(|l| l.split_whitespace().nth(1))
                            .and_then(|v| v.parse::<f64>().ok())
                            .map(|kb| kb / 1024.)
                    });
            if memory.measured {
                let threshold = (memory.total_mib / 20).max(512);
                self.resource_pressure = memory.available_mib
                    < if self.resource_pressure {
                        threshold * 3 / 2
                    } else {
                        threshold
                    };
            }
        }
        let s = &self.visual_settings;
        // Preparation intentionally presents at 20 Hz and menu/pause frames
        // are not evidence of rendering pressure. Do not throttle their loaders.
        if s.resource_profile != settings::ResourceProfile::Constrained
            && self.adaptive_route_started
            && self.settings_ui.as_ref().is_none_or(|ui| !ui.menu_open())
            && !self.minimized
        {
            self.throughput.observe(frame_ms);
        }
        if let Some(gpu) = &mut self.gpu {
            gpu.vegetation_background_paused = self.resource_pressure;
        }
        if let Some(view) = &mut self.adaptive_view {
            view.set_background_paused(self.resource_pressure);
            // Preserve the explicit Constrained reference budgets even after a
            // previously selected Development profile has been throttled.
            let constrained = s.resource_profile == settings::ResourceProfile::Constrained;
            let budget = |value| {
                if constrained {
                    value
                } else {
                    self.throughput.budget(value)
                }
            };
            view.set_upload_budget(budget(s.upload_mib));
            view.set_throughput(budget(s.workers), budget(s.refinements))?;
        }
        Ok(())
    }
    pub(super) fn apply_visual_quality(&mut self) -> Result<()> {
        let s = &self.visual_settings;
        if let Some(view) = &mut self.adaptive_view {
            view.reconfigure(adaptive_stream::StreamSettings {
                pixel_error: s.pixel_error,
                native_radius_m: s.native_radius_m,
                cpu_cache_bytes: s.cache_mib * 1024 * 1024,
                build_budget_bytes: s.build_mib * 1024 * 1024,
                worker_count: s.workers,
                jobs_per_update: s.workers,
                refinements_per_transaction: s.refinements,
                ..view.stream_settings()
            })?;
            view.set_upload_budget(s.upload_mib);
        }
        let attachments_changed = self.sample_count != s.msaa
            || self
                .gpu
                .as_ref()
                .and_then(|g| g.visual_settings.as_ref())
                .is_some_and(|old| old.render_scale != s.render_scale);
        let gpu = self.gpu.as_mut().unwrap();
        if gpu
            .rolling
            .as_ref()
            .is_some_and(|rolling| self.sample_count != s.msaa || !rolling.fits_settings(s))
        {
            let mut replacement = rolling_terrain::RollingTerrain::new(gpu, &self.world, s)?;
            replacement.update(
                &gpu.device,
                &gpu.queue,
                &self.world,
                self.camera,
                &s.terrain_lod_curve,
            )?;
            gpu.rolling = Some(replacement);
        }
        self.camera.far = s.view_distance;
        self.sample_count = s.msaa;
        self.voxel_filter = s.voxel_filter;
        gpu.sample_count = s.msaa;
        gpu.voxel_filter = s.voxel_filter;
        if let Some(adaptive) = &mut gpu.adaptive {
            adaptive.set_sample_count_preserving_geometry(&gpu.device, s.msaa);
            adaptive.set_resource_budget_mib(s.terrain_gpu_mib as u64);
        }
        gpu.visual_settings = Some(s.clone());
        if attachments_changed {
            self.atmosphere = Some(atmosphere::Atmosphere::new_with_scene_format(
                &gpu.device,
                gpu.format,
                self.width,
                self.height,
                s.render_scale,
                s.msaa,
                gpu.scene_format,
            ));
            self.depth = Some(gpu.texture(self.width, self.height, true));
            self.multisample = gpu.multisample_texture(self.width, self.height);
        }
        Ok(())
    }
}
impl Gpu {
    pub(super) fn configure_visuals(&mut self, world: &World, settings: &VisualSettings) {
        self.alpine_appearance = world
            .fbm_terrain()
            .is_some_and(|s| s.params().recipe == frontier_world::FbmRecipe::Alpine);
        self.terrain_shadow_appearance = self.alpine_appearance
            || world
                .fbm_terrain()
                .is_some_and(|s| s.params().recipe.has_watershed_drainage());
        self.regional_appearance = world
            .fbm_terrain()
            .is_some_and(|s| s.params().recipe != frontier_world::FbmRecipe::Legacy);
        self.terrain_amplitude = world
            .fbm_terrain()
            .map_or(1000., |s| s.params().amplitude as f32);
        self.visual_settings = Some(settings.clone());
    }
}
