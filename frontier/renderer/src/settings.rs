//! Display and streaming controls; these never change authoritative voxels.
use crate::development_resources::ResourceBudgets;
pub use crate::development_resources::{MemorySnapshot, ResourceProfile};

#[derive(Clone, Debug, PartialEq)]
pub struct VisualSettings {
    pub beauty: bool,
    pub vegetation: bool,
    pub walking: bool,
    pub step_smoothing: bool,
    pub pixel_error: f64,
    pub view_distance: f32,
    pub render_scale: f32,
    pub msaa: u32,
    pub workers: usize,
    pub refinements: usize,
    pub cache_mib: usize,
    pub upload_mib: usize,
    pub resource_profile: ResourceProfile,
    /// Keeps the fixed Alpine baseline independent of the vegetation toggle.
    pub resource_alpine: bool,
    pub memory: MemorySnapshot,
    pub native_radius_m: f64,
    pub naadf_enabled: bool,
    pub naadf_follow: bool,
    pub naadf_gpu_distance: bool,
    pub visibility_benchmark: bool,
    pub naadf_auto_memory: bool,
    pub naadf_radius_voxels: u32,
    pub naadf_budget_mib: usize,
    pub vegetation_near_m: f64,
    pub(crate) vegetation_lod_curve: crate::lod_curve::LodCurve,
    pub(crate) terrain_lod_curve: crate::lod_curve::LodCurve,
    pub build_mib: usize,
    pub terrain_gpu_mib: usize,
    pub plant_cpu_mib: usize,
    pub plant_gpu_mib: usize,
    pub triangle_budget: usize,
    pub voxel_filter: bool,
    pub terrain_detail: bool,
    pub terrain_near_spacing: Option<f32>,
    pub terrain_near_cells: u32,
    pub terrain_far_detail: bool,
    pub terrain_depth_occlusion: bool,
    pub terrain_mid_detail: bool,
    pub terrain_mid_spacing: f32,
    pub terrain_mid_cells: Option<u32>,
    pub terrain_mid_strips: bool,
    pub terrain_mid_error_px: f32,
    pub terrain_mid_tile_error_m: f32,
    pub terrain_mid_boundary_ascent: bool,
    pub terrain_gray: bool,
    pub capture_terrain_distance: bool,
    pub atmosphere: bool,
    /// Bounded directional shadow map; enabled only for the Alpine adaptive scene.
    pub terrain_shadows: bool,
    pub natural_terrain_light: bool,
    pub cloud_phase: [f32; 4],
    pub edge_aa: f32,
    pub fog_density: f32,
    pub ground_bounce: f32,
    pub sunbeams: f32,
    pub streaming_haze: f32,
    pub fog_height_falloff: f32,
    /// Degrees above the horizon.
    pub sun_elevation: f32,
    /// Degrees clockwise about world up.
    pub sun_azimuth: f32,
    pub exposure: f32,
    pub cloud_coverage: f32,
    pub cloud_speed: f32,
    pub overcast: f32,
    pub day_night: bool,
    pub clock_start: f64,
    pub day_length: f64,
    pub time_speed: f32,
    pub celestial: Option<crate::celestial::Light>,
}

impl Default for VisualSettings {
    fn default() -> Self {
        Self {
            beauty: false,
            vegetation: false,
            walking: false,
            step_smoothing: true,
            pixel_error: 3.,
            view_distance: 20_000.,
            render_scale: 1.,
            msaa: 4,
            workers: std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(2)
                .saturating_sub(2)
                .clamp(1, 16),
            refinements: 8,
            cache_mib: 512,
            upload_mib: 2,
            resource_profile: ResourceProfile::Constrained,
            resource_alpine: false,
            memory: MemorySnapshot::detect(),
            native_radius_m: 0.,
            naadf_enabled: false,
            naadf_follow: true,
            naadf_gpu_distance: false,
            visibility_benchmark: false,
            naadf_auto_memory: true,
            naadf_radius_voxels: 1024,
            naadf_budget_mib: 1024,
            vegetation_near_m: 0.,
            vegetation_lod_curve: Default::default(),
            terrain_lod_curve: Default::default(),
            build_mib: 512,
            terrain_gpu_mib: 512,
            plant_cpu_mib: 160,
            plant_gpu_mib: 256,
            triangle_budget: 1_000_000,
            voxel_filter: true,
            terrain_detail: false,
            terrain_near_spacing: None,
            terrain_near_cells: 512,
            terrain_far_detail: false,
            terrain_depth_occlusion: false,
            terrain_mid_detail: false,
            terrain_mid_spacing: 1.5,
            terrain_mid_cells: None,
            terrain_mid_strips: false,
            terrain_mid_error_px: 0.,
            terrain_mid_tile_error_m: 0.,
            terrain_mid_boundary_ascent: false,
            terrain_gray: false,
            capture_terrain_distance: false,
            atmosphere: false,
            terrain_shadows: false,
            natural_terrain_light: false,
            cloud_phase: [0.; 4],
            edge_aa: 0.,
            fog_density: 0.000035,
            ground_bounce: 0.65,
            sunbeams: 0.8,
            streaming_haze: 0.32,
            fog_height_falloff: 0.0012,
            sun_elevation: 35.,
            sun_azimuth: 135.,
            exposure: 1.,
            cloud_coverage: 0.48,
            cloud_speed: 0.0015,
            overcast: 0.15,
            day_night: false,
            clock_start: 13. * 3600.,
            day_length: frontier_runtime::weather::SOLAR_DAY_SECONDS,
            time_speed: 1.,
            celestial: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SettingsStats {
    pub can_board: bool,
    pub flight: Option<crate::aircraft::Instruments>,
    pub flight_audio: bool,
    pub flight_camera_motion: bool,
    pub available_mib: Option<usize>,
    pub naadf_status: String,
    pub naadf_gpu_mib: f64,
    pub background_paused: bool,
    pub plant_instances: usize,
    pub plant_pending: usize,
    pub plant_gpu_mib: f64,
    pub frame_ms: f64,
    pub gpu_ms: Option<f64>,
    pub gpu_full_ms: Option<f64>,
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
    pub naadf_rebuild: bool,
    pub toggle_airflow: bool,
    pub toggle_flight_audio: bool,
    pub toggle_flight_camera_motion: bool,
}

impl VisualSettings {
    pub fn light_direction(&self) -> glam::Vec3 {
        self.celestial.map_or_else(
            || crate::atmosphere::sun_direction(self.sun_elevation, self.sun_azimuth),
            crate::celestial::Light::terrain_direction,
        )
    }

    pub fn terrain_light_direction(&self) -> glam::Vec3 {
        if self.beauty || self.celestial.is_some() {
            self.light_direction()
        } else {
            glam::Vec3::new(-0.5, 0.8, -0.3).normalize()
        }
    }

    /// Reuse the startup/explicit-refresh snapshot: resident engine memory must
    /// not make repeated profile application shrink its own budgets.
    pub fn apply_resource_profile(&mut self, profile: ResourceProfile, alpine: bool) {
        self.resource_profile = profile;
        self.resource_alpine = alpine;
        if profile == ResourceProfile::Custom {
            return;
        }
        let budgets = if profile == ResourceProfile::Development {
            ResourceBudgets::development(self.memory)
        } else {
            ResourceBudgets::constrained(alpine)
        };
        self.cache_mib = budgets.terrain_cpu_mib;
        self.build_mib = budgets.build_mib;
        self.terrain_gpu_mib = budgets.terrain_gpu_mib;
        self.plant_cpu_mib = budgets.plant_cpu_mib;
        self.plant_gpu_mib = budgets.plant_gpu_mib;
        self.triangle_budget = if profile == ResourceProfile::Development {
            4_000_000
        } else {
            1_000_000
        };
        self.native_radius_m = if profile == ResourceProfile::Development {
            4.
        } else {
            0.
        };
        self.vegetation_near_m = if profile == ResourceProfile::Development {
            12.
        } else {
            0.
        };
        self.upload_mib = if profile == ResourceProfile::Development {
            8
        } else {
            2
        };
    }
    pub fn working_budgets(&self) -> ResourceBudgets {
        ResourceBudgets {
            terrain_cpu_mib: self.cache_mib,
            build_mib: self.build_mib,
            terrain_gpu_mib: self.terrain_gpu_mib,
            plant_cpu_mib: self.plant_cpu_mib,
            plant_gpu_mib: self.plant_gpu_mib,
        }
    }
}
