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
        let was_open = ui.menu_open();
        let consumed = ui.handle_event(self.window.as_ref().unwrap(), event);
        let open = ui.menu_open();
        if open != was_open {
            self.keys.clear();
            self.grab(!open);
        }
        consumed
    }
    pub(super) fn update_visual_ui(&mut self, frame_ms: f64) -> Result<()> {
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
        }
        stats.plant_instances = gpu.vegetation_stats.selected_instances;
        stats.plant_pending = gpu.vegetation_stats.pending_jobs;
        stats.plant_gpu_mib = gpu
            .vegetation
            .as_ref()
            .map_or(0., |p| p.stats().resident_bytes as f64 / 1048576.);
        let was_open = ui.menu_open();
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let changes = ui.prepare(
            window,
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &stats,
            [self.width, self.height],
        );
        gpu.queue.submit(Some(encoder.finish()));
        let now_open = ui.menu_open();
        let settings = ui.settings().clone();
        if was_open && !now_open {
            self.keys.clear();
            self.grab(true);
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
        if let Some(gpu) = &mut self.gpu {
            gpu.visual_settings = Some(self.visual_settings.clone());
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
        self.camera.far = s.view_distance;
        self.sample_count = s.msaa;
        self.voxel_filter = s.voxel_filter;
        let gpu = self.gpu.as_mut().unwrap();
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
        self.regional_appearance = world
            .fbm_terrain()
            .is_some_and(|s| s.params().recipe != frontier_world::FbmRecipe::Legacy);
        self.terrain_amplitude = world
            .fbm_terrain()
            .map_or(1000., |s| s.params().amplitude as f32);
        self.visual_settings = Some(settings.clone());
    }
}
