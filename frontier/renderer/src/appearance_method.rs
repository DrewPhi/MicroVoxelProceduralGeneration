//! Reversible appearance-only experiments. Geometry and canonical voxels are shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    Baseline,
    Hierarchy,
    Visibility,
    Map,
}
impl Method {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Hierarchy => "hierarchy",
            Self::Visibility => "visibility",
            Self::Map => "map",
        }
    }
}
pub(crate) fn selected() -> Method {
    static VALUE: std::sync::OnceLock<Method> = std::sync::OnceLock::new();
    *VALUE.get_or_init(
        || match std::env::var("FRONTIER_LOD_APPEARANCE").as_deref() {
            Ok("hierarchy") => Method::Hierarchy,
            Ok("visibility") => Method::Visibility,
            Ok("map") => Method::Map,
            Ok("baseline") | Err(_) => Method::Baseline,
            Ok(value) => panic!(
                "unknown FRONTIER_LOD_APPEARANCE={value}; use baseline|hierarchy|visibility|map"
            ),
        },
    )
}

/// Optional display-only voxel grain amplitude; None retains the original shader.
pub(crate) fn voxel_grain() -> Option<f32> {
    static VALUE: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("FRONTIER_VOXEL_GRAIN")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite() && (0.0..=0.3).contains(v))
    })
}

/// Diagnostic output only; geometry/depth are unchanged.
pub(crate) fn debug_view() -> u32 {
    static VALUE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *VALUE.get_or_init(
        || match std::env::var("FRONTIER_APPEARANCE_DEBUG").as_deref() {
            Ok("lod") => 1,
            Ok("normals") => 2,
            Ok("flat") => 3,
            Ok("paint") => 4,
            Ok("blend") => 5,
            Ok("raw-light") => 6,
            Ok("material") => 7,
            Ok("profile") => 8,
            _ => 0,
        },
    )
}

/// Near proxies can mix macro-surface and microface response before minification.
/// 0 retains the prior full-profile rule,1 is balanced,2 is footprint-only debug.
pub(crate) fn near_blend() -> u32 {
    static VALUE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| match std::env::var("FRONTIER_NEAR_BLEND").as_deref() {
        Ok("balanced") => 1,
        Ok("footprint") => 2,
        _ => 0,
    })
}

/// Bounded directional map sampling. One is the original bilinear path.
pub(crate) fn map_anisotropy() -> u32 {
    static VALUE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *VALUE.get_or_init(
        || match std::env::var("FRONTIER_MAP_ANISOTROPY").as_deref() {
            Ok("2") => 2,
            Ok("4") => 4,
            _ => 1,
        },
    )
}

/// Preserve point-sampled rock contrast while filtering softer ground materials.
pub(crate) fn map_filter_soft_surfaces() -> bool {
    static VALUE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| std::env::var("FRONTIER_MAP_FILTER_SURFACES").as_deref() == Ok("soft"))
}
