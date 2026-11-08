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
