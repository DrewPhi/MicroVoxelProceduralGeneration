//! Screen-space representation decisions. All distances are camera-relative;
//! content summaries themselves never contain a camera or fixed near radius.

/// Conservative camera-space bounds: +Z is forward, +X right, +Y up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewBounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection {
    pub vertical_fov_radians: f64,
    pub width: u32,
    pub height: u32,
    pub near_m: f64,
}
impl Projection {
    fn valid(self) -> bool {
        self.width > 0
            && self.height > 0
            && self.near_m.is_finite()
            && self.near_m > 0.0
            && self.vertical_fov_radians.is_finite()
            && self.vertical_fov_radians > 0.0
            && self.vertical_fov_radians < std::f64::consts::PI
    }
    /// A heightfield certificate relates points at identical world X/Z.
    /// Project that vertical segment using the camera-space world-up vector,
    /// retaining depth-denominator safety without charging two image axes for
    /// arbitrary isotropic displacement. Volumetric certificates use the
    /// general Euclidean method below instead.
    pub fn projected_vertical_error_px(
        self,
        error_m: f64,
        bounds: ViewBounds,
        view_up: [f64; 3],
    ) -> f64 {
        if !self.valid()
            || !error_m.is_finite()
            || error_m < 0.0
            || view_up.iter().any(|v| !v.is_finite())
            || (0..3).any(|i| {
                !bounds.min[i].is_finite()
                    || !bounds.max[i].is_finite()
                    || bounds.min[i] > bounds.max[i]
            })
        {
            return f64::INFINITY;
        }
        if error_m == 0.0 {
            return 0.0;
        }
        let z = bounds.min[2];
        let dz = view_up[2].abs();
        if z - error_m * dz <= self.near_m {
            return f64::INFINITY;
        }
        let x = bounds.min[0].abs().max(bounds.max[0].abs());
        let y = bounds.min[1].abs().max(bounds.max[1].abs());
        let f = self.height as f64 / (2.0 * (self.vertical_fov_radians * 0.5).tan());
        let bound = f * error_m / (z - error_m * dz)
            * (view_up[0].abs() + x / z * dz).hypot(view_up[1].abs() + y / z * dz);
        // Round outward across the small sequence of f64 arithmetic operations.
        bound * (1.0 + 32.0 * f64::EPSILON) + 32.0 * f64::EPSILON
    }
    /// Conservative Euclidean displacement in output pixels. The geometric
    /// certificate must bound both directed surface distances. For perspective
    /// p=x/z, |delta p| <= e(1+|x|/z)/(z-e); apply to both image axes.
    /// If either surface can reach the near plane return infinity: dividing by
    /// a clamped distance would incorrectly certify a crossing/containing tile.
    pub fn projected_error_px(self, error_m: f64, bounds: ViewBounds) -> f64 {
        if !self.valid()
            || !error_m.is_finite()
            || error_m < 0.0
            || (0..3).any(|i| {
                !bounds.min[i].is_finite()
                    || !bounds.max[i].is_finite()
                    || bounds.min[i] > bounds.max[i]
            })
        {
            return f64::INFINITY;
        }
        if error_m == 0.0 {
            return 0.0;
        }
        let z = bounds.min[2];
        if z - error_m <= self.near_m {
            return f64::INFINITY;
        }
        let x = bounds.min[0].abs().max(bounds.max[0].abs());
        let y = bounds.min[1].abs().max(bounds.max[1].abs());
        // Square pixels: horizontal focal length equals vertical focal length;
        // aspect ratio is already encoded by the pixel viewport dimensions.
        let f = self.height as f64 / (2.0 * (self.vertical_fov_radians * 0.5).tan());
        let scale = f * error_m / (z - error_m);
        scale * (1.0 + x / z).hypot(1.0 + y / z)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Native,
    Proxy,
    Refine,
    Unresolved,
}
/// Request exact geometry at any distance if resolvable. Uncertified error or
/// unknown/volume topology cannot authorize a heightfield proxy. A caller may
/// keep an already valid parent as temporary coverage, but must expose that
/// unresolved quality rather than labeling it certified.
pub fn select_representation(
    projection: Projection,
    bounds: ViewBounds,
    certified_error_m: Option<f64>,
    heightfield_safe: bool,
    native_ready: bool,
    can_refine: bool,
    pixel_budget: f64,
) -> Selection {
    if heightfield_safe && pixel_budget.is_finite() && pixel_budget >= 0.0 {
        if let Some(error) = certified_error_m {
            if projection.projected_error_px(error, bounds) <= pixel_budget {
                return Selection::Proxy;
            }
        }
    }
    if native_ready {
        Selection::Native
    } else if can_refine {
        Selection::Refine
    } else {
        Selection::Unresolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn projection(height: u32) -> Projection {
        Projection {
            vertical_fov_radians: std::f64::consts::PI / 3.0,
            width: height * 16 / 9,
            height,
            near_m: 0.025,
        }
    }
    fn bounds(z: f64) -> ViewBounds {
        ViewBounds {
            min: [-1.0, -1.0, z],
            max: [1.0, 1.0, z + 2.0],
        }
    }
    #[test]
    fn resolution_fov_distance_drive_decision() {
        let b = bounds(100.0);
        let p = projection(1080);
        let e = p.projected_error_px(0.05, b);
        assert!((projection(2160).projected_error_px(0.05, b) - 2.0 * e).abs() < 1e-12);
        assert!(p.projected_error_px(0.05, bounds(200.0)) < e);
        let wider = Projection {
            vertical_fov_radians: 1.5,
            ..p
        };
        assert!(wider.projected_error_px(0.05, b) < e);
        assert_eq!(
            select_representation(p, b, Some(0.05), true, true, true, e * 1.1),
            Selection::Proxy
        );
        assert_eq!(
            select_representation(projection(2160), b, Some(0.05), true, true, true, e * 1.1),
            Selection::Native
        );
    }
    #[test]
    fn unknown_topology_and_near_crossing_never_certify_proxy() {
        let p = projection(1080);
        assert!(p.projected_error_px(0.1, bounds(0.05)).is_infinite());
        assert!(p.projected_error_px(0.1, bounds(-1.0)).is_infinite());
        assert_eq!(
            select_representation(p, bounds(10000.0), Some(0.0), false, true, true, 1.0),
            Selection::Native
        );
        assert_eq!(
            select_representation(p, bounds(10000.0), None, true, false, true, 1.0),
            Selection::Refine
        );
        assert_eq!(
            select_representation(p, bounds(10000.0), None, true, false, false, 1.0),
            Selection::Unresolved
        );
    }
    #[test]
    fn vertical_certificate_projects_tightly_and_handles_pitch() {
        let p = projection(1080);
        let b = ViewBounds {
            min: [-30.0, -4.0, 100.0],
            max: [30.0, 5.0, 120.0],
        };
        let e = 0.1;
        let vertical = p.projected_vertical_error_px(e, b, [0.0, 1.0, 0.0]);
        let f = 1080.0 / (2.0 * (p.vertical_fov_radians * 0.5).tan());
        assert!((vertical - f * e / 100.0).abs() < 1e-12);
        assert!(vertical < p.projected_error_px(e, b));
        for pitch in [-1.2_f64, -0.1, 0.0, 0.8] {
            let up = [0.0, pitch.cos(), pitch.sin()];
            let bound = p.projected_vertical_error_px(e, b, up);
            for sign in [-1.0, 1.0] {
                let a = [30.0, 5.0, 100.0];
                let q = [a[0], a[1] + sign * e * up[1], a[2] + sign * e * up[2]];
                let actual = f * (a[0] / a[2] - q[0] / q[2]).hypot(a[1] / a[2] - q[1] / q[2]);
                assert!(actual <= bound, "actual={actual:.17} bound={bound:.17}");
            }
        }
        assert!(p
            .projected_vertical_error_px(0.1, bounds(0.05), [0.0, 0.0, 1.0])
            .is_infinite());
    }
    #[test]
    fn perspective_bound_contains_displaced_points() {
        let p = projection(1080);
        let b = ViewBounds {
            min: [80.0, -8.0, 20.0],
            max: [82.0, 8.0, 25.0],
        };
        let e = 0.25;
        let bound = p.projected_error_px(e, b);
        let f = 1080.0 / (2.0 * (p.vertical_fov_radians / 2.0).tan());
        for delta in [
            [e, 0.0, 0.0],
            [0.0, e, 0.0],
            [0.0, 0.0, -e],
            [e / 3.0_f64.sqrt(); 3],
        ] {
            let a = [82.0, 8.0, 20.0];
            let q = [a[0] + delta[0], a[1] + delta[1], a[2] + delta[2]];
            let actual = f * (a[0] / a[2] - q[0] / q[2]).hypot(a[1] / a[2] - q[1] / q[2]);
            assert!(actual <= bound, "actual={actual:.17} bound={bound:.17}");
        }
    }
}
