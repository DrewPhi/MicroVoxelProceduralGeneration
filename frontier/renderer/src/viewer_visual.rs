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
            ..Default::default()
        };
        if let Some(probe) = &self.gpu_probe {
            stats.gpu_ms = probe.stats.summary().p50_ms;
        }
        if let Some(view) = &self.adaptive_view {
            stats.pending = view.pending_count;
            stats.gpu_mib = view.stats.allocated_bytes as f64 / 1048576.;
            stats.cache_mib = view.cache_bytes as f64 / 1048576.;
            stats.quality_ready = view.quality_ready;
        }
        let was_open = ui.menu_open();
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let changes = ui.prepare(window, &gpu.device, &gpu.queue, &mut encoder, &stats);
        gpu.queue.submit(Some(encoder.finish()));
        let now_open = ui.menu_open();
        let settings = ui.settings().clone();
        if was_open && !now_open {
            self.keys.clear();
            self.grab(true);
        }
        self.visual_settings = settings;
        if changes.loading_applied {
            self.apply_visual_quality()?;
        }
        if let Some(gpu) = &mut self.gpu {
            gpu.visual_settings = Some(self.visual_settings.clone());
        }
        Ok(())
    }
    pub(super) fn apply_visual_quality(&mut self) -> Result<()> {
        let s = &self.visual_settings;
        // Explicit Apply resets derived demand, not authoritative terrain. Keep
        // disk-cache identity/source parameters and the current camera unchanged.
        self.adaptive_view.take();
        let mut view = adaptive_view::AdaptiveView::new(
            self.world.clone(),
            s.pixel_error,
            s.cache_mib * 1024 * 1024,
            true,
            s.workers,
            s.refinements,
            false,
            self.visual_build_mib * 1024 * 1024,
            self.visual_disk_cache
                .clone()
                .map(|path| adaptive_stream::DiskCacheConfig {
                    path,
                    budget_bytes: (self.visual_disk_mib * 1024 * 1024) as u64,
                    max_entry_bytes: 64 * 1024 * 1024,
                }),
        )?;
        view.set_upload_budget(s.upload_mib);
        self.adaptive_view = Some(view);
        self.camera.far = s.view_distance;
        self.sample_count = s.msaa;
        self.voxel_filter = s.voxel_filter;
        let gpu = self.gpu.as_mut().unwrap();
        gpu.sample_count = s.msaa;
        gpu.voxel_filter = s.voxel_filter;
        gpu.adaptive = Some(adaptive_gpu::AdaptiveGpu::new(
            &gpu.device,
            gpu.format,
            s.msaa,
        ));
        gpu.visual_settings = Some(s.clone());
        self.atmosphere = Some(atmosphere::Atmosphere::new(
            &gpu.device,
            gpu.format,
            self.width,
            self.height,
            s.render_scale,
            s.msaa,
        ));
        self.depth = Some(gpu.texture(self.width, self.height, true));
        self.multisample = gpu.multisample_texture(self.width, self.height);
        Ok(())
    }
}
impl Gpu {
    pub(super) fn configure_visuals(&mut self, world: &World, settings: &VisualSettings) {
        self.regional_appearance = world
            .fbm_terrain()
            .is_some_and(|s| s.params().recipe != frontier_world::FbmRecipe::Legacy);
        self.terrain_amplitude = world
            .fbm_terrain()
            .map_or(1000., |s| s.params().amplitude as f32);
        self.visual_settings = Some(settings.clone());
    }
}
