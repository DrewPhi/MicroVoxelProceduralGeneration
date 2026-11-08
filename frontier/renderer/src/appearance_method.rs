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
static VALUE: std::sync::OnceLock<Method> = std::sync::OnceLock::new();

fn resolve(value: Option<&str>, default: Method) -> Method {
    match value {
        Some("hierarchy") => Method::Hierarchy,
        Some("visibility") => Method::Visibility,
        Some("map") => Method::Map,
        Some("baseline") => Method::Baseline,
        None => default,
        Some(value) => {
            panic!("unknown FRONTIER_LOD_APPEARANCE={value}; use baseline|hierarchy|visibility|map")
        }
    }
}

fn from_environment(default: Method) -> Method {
    resolve(
        std::env::var("FRONTIER_LOD_APPEARANCE").ok().as_deref(),
        default,
    )
}

/// Application policy is selected once, before creating any terrain products.
/// Keep baseline file-format fixtures independent of the viewer's preferred look.
pub(crate) fn initialize_application() {
    VALUE.get_or_init(|| from_environment(Method::Map));
}

pub(crate) fn selected() -> Method {
    *VALUE.get_or_init(|| from_environment(Method::Baseline))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn meadow_sampling_policy_preserves_explicit_overrides_and_fixture_defaults() {
        assert_eq!(resolve_anisotropy(None, 2), 2);
        assert_eq!(resolve_anisotropy(None, 1), 1);
        for (value, expected) in [("1", 1), ("2", 2), ("4", 4), ("invalid", 1)] {
            assert_eq!(resolve_anisotropy(Some(value), 2), expected);
        }
        assert!(resolve_soft_surfaces(None, true));
        assert!(!resolve_soft_surfaces(None, false));
        assert!(resolve_soft_surfaces(Some("soft"), false));
        assert!(!resolve_soft_surfaces(Some("all"), true));
    }
    #[test]
    fn application_preference_preserves_explicit_method_overrides() {
        assert_eq!(resolve(None, Method::Map), Method::Map);
        for (value, expected) in [
            ("baseline", Method::Baseline),
            ("map", Method::Map),
            ("hierarchy", Method::Hierarchy),
            ("visibility", Method::Visibility),
        ] {
            assert_eq!(resolve(Some(value), Method::Map), expected);
        }
    }
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

static MAP_ANISOTROPY: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
static MAP_SOFT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
fn resolve_anisotropy(value: Option<&str>, default: u32) -> u32 {
    match value {
        None => default,
        Some("2") => 2,
        Some("4") => 4,
        _ => 1,
    }
}
fn resolve_soft_surfaces(value: Option<&str>, default: bool) -> bool {
    match value {
        None => default,
        Some("soft") => true,
        _ => false,
    }
}
/// Call for the beauty meadow scene before generating terrain products. The
/// generic accessor defaults stay frozen for Legacy and file-format fixtures.
/// Rock nodes retain point profiles; the two GPU taps apply only to elongated
/// pixel footprints and never increase map storage or affect terrain geometry.
pub(crate) fn initialize_meadow_application() {
    crate::appearance_map::initialize_meadow_application();
    MAP_ANISOTROPY.get_or_init(|| {
        resolve_anisotropy(std::env::var("FRONTIER_MAP_ANISOTROPY").ok().as_deref(), 2)
    });
    MAP_SOFT.get_or_init(|| {
        resolve_soft_surfaces(
            std::env::var("FRONTIER_MAP_FILTER_SURFACES")
                .ok()
                .as_deref(),
            true,
        )
    });
}
/// Bounded directional map sampling. One is the original bilinear path.
pub(crate) fn map_anisotropy() -> u32 {
    *MAP_ANISOTROPY.get_or_init(|| {
        resolve_anisotropy(std::env::var("FRONTIER_MAP_ANISOTROPY").ok().as_deref(), 1)
    })
}

/// Preserve point-sampled rock contrast while filtering softer ground materials.
pub(crate) fn map_filter_soft_surfaces() -> bool {
    *MAP_SOFT.get_or_init(|| {
        resolve_soft_surfaces(
            std::env::var("FRONTIER_MAP_FILTER_SURFACES")
                .ok()
                .as_deref(),
            false,
        )
    })
}
