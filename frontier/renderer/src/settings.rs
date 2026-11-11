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
    pub naadf_radius_m: u32,
    pub naadf_budget_mib: usize,
    pub vegetation_near_m: f64,
    pub build_mib: usize,
    pub terrain_gpu_mib: usize,
    pub plant_cpu_mib: usize,
    pub plant_gpu_mib: usize,
    pub triangle_budget: usize,
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
            naadf_radius_m: 64,
            naadf_budget_mib: 1024,
            vegetation_near_m: 0.,
            build_mib: 512,
            terrain_gpu_mib: 512,
            plant_cpu_mib: 160,
            plant_gpu_mib: 256,
            triangle_budget: 1_000_000,
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
}

impl VisualSettings {
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
