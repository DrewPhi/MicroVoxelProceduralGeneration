//! Display and streaming controls; these never change authoritative voxels.
#[derive(Clone, Debug, PartialEq)]
pub struct VisualSettings {
    pub pixel_error: f64,
    pub view_distance: f32,
    pub render_scale: f32,
    pub msaa: u32,
    pub workers: usize,
    pub refinements: usize,
    pub cache_mib: usize,
    pub upload_mib: usize,
    pub voxel_filter: bool,
    pub atmosphere: bool,
    /// Bounded directional shadow map; enabled only for the Alpine adaptive scene.
    pub terrain_shadows: bool,
    pub fog_density: f32,
    pub fog_height_falloff: f32,
    /// Degrees above the horizon.
    pub sun_elevation: f32,
    /// Degrees clockwise about world up.
    pub sun_azimuth: f32,
    pub exposure: f32,
    pub cloud_coverage: f32,
    pub cloud_speed: f32,
    pub overcast: f32,
}

impl Default for VisualSettings {
    fn default() -> Self {
        Self {
            pixel_error: 3.,
            view_distance: 20_000.,
            render_scale: 1.,
            msaa: 4,
            workers: std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(2)
                .min(4),
            refinements: 8,
            cache_mib: 512,
            upload_mib: 8,
            voxel_filter: true,
            atmosphere: false,
            terrain_shadows: false,
            fog_density: 0.000035,
            fog_height_falloff: 0.0012,
            sun_elevation: 35.,
            sun_azimuth: 135.,
            exposure: 1.,
            cloud_coverage: 0.48,
            cloud_speed: 0.0015,
            overcast: 0.15,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SettingsStats {
    pub frame_ms: f64,
    pub gpu_ms: Option<f64>,
    pub pending: usize,
    pub gpu_mib: f64,
    pub cache_mib: f64,
    /// Resident process memory, when the platform exposes it.
    pub process_mib: Option<f64>,
    pub quality_ready: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SettingsChanges {
    pub visual_changed: bool,
    pub loading_applied: bool,
}
