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
    /// Bound vertical displacement only where either related surface (or the
    /// segment joining them) can enter the viewport. `bounds` encloses the
    /// reference endpoints a; every related endpoint is a+t*view_up, |t|<=error_m.
    /// The certificate must cover both directed surface correspondences, just
    /// as for `projected_vertical_error_px`.
    ///
    /// Clip against the frustum expanded by the entire allowed vertical segment,
    /// not against the reference surface alone: an offscreen reference point can
    /// have an onscreen counterpart at a silhouette. For each side plane L>=0,
    /// a relevant reference satisfies L(a)>=-error_m*|L(view_up)|. Maximizing the
    /// transverse coordinate over the AABB gives a conservative minimum depth.
    /// The original displacement bound then applies to both complete endpoints,
    /// including an endpoint outside the viewport. If either endpoint could
    /// reach the near plane, it still returns infinity; depth is never clamped.
    ///
    /// This is an optional view-dependent quality bound, not a visibility test,
    /// occlusion proof, far-plane rejection, or certificate of topology.
    pub fn projected_visible_vertical_error_px(
        self,
        error_m: f64,
        bounds: ViewBounds,
        view_up: [f64; 3],
    ) -> f64 {
        let full = self.projected_vertical_error_px(error_m, bounds, view_up);
        if full.is_nan() {
            return f64::INFINITY;
        }
        if !self.valid()
            || !error_m.is_finite()
            || error_m < 0.0
            || view_up.iter().any(|v| !v.is_finite())
            || (0..3).any(|i| {
                !bounds.min[i].is_finite()
                    || !bounds.max[i].is_finite()
                    || bounds.min[i] > bounds.max[i]
            })
            || error_m == 0.0
        {
            return full;
        }
        // Slightly enlarge the viewport to match renderer frustum padding.
        // This only weakens clipping. Camera/raster quantization remains part
        // of the caller's projection contract, as for the whole-tile method.
        let tan_y = (self.vertical_fov_radians * 0.5).tan() * (1.0 + 1e-6);
        let tan_x = tan_y * self.width as f64 / self.height as f64;
        let mut depth = bounds.min[2];
        for (axis, slope) in [(0, tan_x), (1, tan_y)] {
            for sign in [-1.0, 1.0] {
                let support = if sign > 0.0 {
                    bounds.max[axis]
                } else {
                    -bounds.min[axis]
                };
                let radius = error_m * (sign * view_up[axis] + slope * view_up[2]).abs();
                // Outward absolute allowance also covers cancellation inside
                // L(view_up); a relative allowance on its result alone would
                // be insufficient when that result is close to zero.
                let magnitude =
                    support.abs() + error_m * (view_up[axis].abs() + slope * view_up[2].abs());
                if !slope.is_finite()
                    || slope <= 0.0
                    || !radius.is_finite()
                    || !magnitude.is_finite()
                {
                    return full;
                }
                let slack = 128.0 * f64::EPSILON * magnitude.max(1.0);
                let candidate = (-support - radius - slack) / slope;
                if !candidate.is_finite() {
                    return full;
                }
                let candidate = candidate - 128.0 * f64::EPSILON * candidate.abs().max(1.0);
                depth = depth.max(candidate);
            }
        }
        if depth > bounds.max[2] {
            // No relevant reference is possible under these relaxed planes.
            // Keep the whole-tile bound rather than using this API to cull it.
            return full;
        }
        let clipped = ViewBounds {
            min: [bounds.min[0], bounds.min[1], depth],
            ..bounds
        };
        self.projected_vertical_error_px(error_m, clipped, view_up)
            .min(full)
    }
    /// Visible vertical bound using the actual uploaded f32 local-to-clip matrix
    /// (column-major). Local +Y is canonical world up. Unlike an ideal camera
    /// basis, these coefficients include per-tile eye/origin and matrix rounding.
    ///
    /// An interval dot-product bound covers any ordering/fusion of four f32
    /// products and three sums, rounding in either direction and subnormal
    /// flushing. Input slack covers a float conversion plus native-quad extent
    /// subtraction/reconstruction; packed proxies directly retain f32 positions.
    /// Both reference and canonical counterparts are included in every interval.
    /// This bounds projected geometry, not raster sample/coverage quantization.
    pub fn projected_f32_visible_vertical_error_px(
        self,
        error_m: f64,
        local: ViewBounds,
        clip_columns: [[f32; 4]; 4],
    ) -> f64 {
        if !self.valid()
            || !error_m.is_finite()
            || error_m < 0.0
            || !valid_bounds(local)
            || clip_columns.iter().flatten().any(|v| !v.is_finite())
        {
            return f64::INFINITY;
        }
        if error_m == 0.0 {
            return 0.0;
        }
        let rows: [[f64; 4]; 4] =
            std::array::from_fn(|r| std::array::from_fn(|c| clip_columns[c][r] as f64));
        let mut related = local;
        related.min[1] = (related.min[1] - error_m).next_down();
        related.max[1] = (related.max[1] + error_m).next_up();
        if !valid_bounds(related) {
            return f64::INFINITY;
        }
        let maximum: [f64; 4] = std::array::from_fn(|i| {
            if i == 3 {
                1.0
            } else {
                related.min[i].abs().max(related.max[i].abs())
            }
        });
        if maximum[..3]
            .iter()
            .any(|&v| !v.is_finite() || v > f32::MAX as f64 * 0.25)
        {
            return f64::INFINITY;
        }
        // |anchor| and |endpoint| <= maximum; an extent is at most 2*maximum.
        // 16 eps covers conversion, extent subtraction and reconstruction with
        // any directed rounding, including cancellation. Homogeneous 1 is exact.
        let input_error: [f64; 4] = std::array::from_fn(|i| {
            if i == 3 {
                0.0
            } else {
                16.0 * f32::EPSILON as f64 * maximum[i] + 32.0 * f32::MIN_POSITIVE as f64
            }
        });
        let eps = f32::EPSILON as f64;
        let gamma = 16.0 * eps / (1.0 - 16.0 * eps);
        let mut roundoff = [0.0; 4];
        for r in 0..4 {
            let magnitude: f64 = (0..4)
                .map(|c| rows[r][c].abs() * (maximum[c] + input_error[c]))
                .sum();
            if !magnitude.is_finite() || magnitude > f32::MAX as f64 / (1.0 + gamma) {
                return f64::INFINITY;
            }
            let input: f64 = (0..4).map(|c| rows[r][c].abs() * input_error[c]).sum();
            let flushed_coefficients: f64 = (0..4)
                .filter(|&c| rows[r][c].abs() < f32::MIN_POSITIVE as f64)
                .map(|c| rows[r][c].abs() * (maximum[c] + input_error[c]))
                .sum();
            roundoff[r] = (gamma * magnitude
                + input
                + flushed_coefficients
                + 32.0 * f32::MIN_POSITIVE as f64)
                .next_up();
        }
        let Some(reference) = affine_bounds(local, [rows[0], rows[1], rows[3]]) else {
            return f64::INFINITY;
        };
        let up = [rows[0][1], rows[1][1], rows[3][1]];
        let mut depth = reference.min[2];
        for axis in 0..2 {
            for sign in [-1.0, 1.0] {
                let support = if sign > 0.0 {
                    reference.max[axis]
                } else {
                    -reference.min[axis]
                };
                let radius = error_m * (sign * up[axis] + up[2]).abs();
                let padding = roundoff[axis] + roundoff[3];
                let slack = 128.0
                    * f64::EPSILON
                    * (support.abs() + error_m * (up[axis].abs() + up[2].abs()) + padding).max(1.0);
                let candidate = -support - radius - padding - slack;
                if !candidate.is_finite() {
                    return f64::INFINITY;
                }
                depth = depth.max(candidate.next_down());
            }
        }
        // If relaxed side planes exclude the whole box, use its original depth
        // rather than turning this quality API into a visibility/culling test.
        if depth > reference.max[2] {
            depth = reference.min[2];
        }
        let minimum_w = (depth - error_m * up[2].abs()).next_down();
        // The actual GPU near test is clip Z>=0, not ideal view W>=near_m.
        // Bound Z-W+near over both endpoints, then include shader Z roundoff.
        let near_difference = rows[2][3] - rows[3][3] + self.near_m;
        let near_slack = near_difference.abs()
            + (0..3)
                .map(|c| (rows[2][c] - rows[3][c]).abs() * maximum[c])
                .sum::<f64>();
        let near_slack = (near_slack
            + 128.0
                * f64::EPSILON
                * (1.0
                    + rows[2]
                        .iter()
                        .zip(maximum)
                        .map(|(c, m)| c.abs() * m)
                        .sum::<f64>()
                    + rows[3]
                        .iter()
                        .zip(maximum)
                        .map(|(c, m)| c.abs() * m)
                        .sum::<f64>()))
        .next_up();
        if !minimum_w.is_finite()
            || minimum_w - roundoff[3] <= self.near_m
            || minimum_w <= self.near_m + near_slack + roundoff[2]
        {
            return f64::INFINITY;
        }
        let xy = [
            reference.min[0].abs().max(reference.max[0].abs()),
            reference.min[1].abs().max(reference.max[1].abs()),
        ];
        let pixels = [self.width as f64 * 0.5, self.height as f64 * 0.5];
        let geometric: [f64; 2] = std::array::from_fn(|i| {
            pixels[i] * error_m / minimum_w * (up[i].abs() + xy[i] / depth * up[2].abs())
        });
        // Each endpoint can move independently under f32 evaluation. Add both
        // directed ratio perturbations, using the complete unclipped endpoints.
        let endpoint: [f64; 2] = std::array::from_fn(|i| {
            pixels[i] * (roundoff[i] + (xy[i] + error_m * up[i].abs()) / minimum_w * roundoff[3])
                / (minimum_w - roundoff[3])
        });
        let result = geometric[0].hypot(geometric[1]) + 2.0 * endpoint[0].hypot(endpoint[1]);
        if !result.is_finite() {
            f64::INFINITY
        } else {
            result * (1.0 + 128.0 * f64::EPSILON) + 128.0 * f64::EPSILON
        }
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
fn valid_bounds(b: ViewBounds) -> bool {
    (0..3).all(|i| b.min[i].is_finite() && b.max[i].is_finite() && b.min[i] <= b.max[i])
}
/// Outward interval image of an AABB under three affine rows.
fn affine_bounds(b: ViewBounds, rows: [[f64; 4]; 3]) -> Option<ViewBounds> {
    let mut out = ViewBounds {
        min: [0.0; 3],
        max: [0.0; 3],
    };
    for r in 0..3 {
        let mut lo = rows[r][3];
        let mut hi = lo;
        let mut magnitude = lo.abs();
        for c in 0..3 {
            let a = rows[r][c] * b.min[c];
            let z = rows[r][c] * b.max[c];
            lo += a.min(z);
            hi += a.max(z);
            magnitude += a.abs().max(z.abs());
        }
        let slack = 64.0 * f64::EPSILON * magnitude.max(1.0);
        out.min[r] = (lo - slack).next_down();
        out.max[r] = (hi + slack).next_up();
    }
    valid_bounds(out).then_some(out)
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
    fn visible_vertical_bound_ignores_only_provably_offscreen_near_floor() {
        let p = projection(1080);
        let b = ViewBounds {
            min: [-8.0, -1.8, -1.0],
            max: [8.0, -1.6, 20.0],
        };
        let e = 0.001;
        let up = [0.0, 1.0, 0.0];
        assert!(p.projected_vertical_error_px(e, b, up).is_infinite());
        let visible = p.projected_visible_vertical_error_px(e, b, up);
        assert!(visible.is_finite() && visible < 1.0);
        // The visible floor is metres away, despite its box crossing the eye.
        let a = [0.0, -1.6, 3.0];
        let q = [0.0, -1.599, 3.0];
        let actual = projection_distance(p, a, q);
        assert!(actual <= visible);
        // Near points that can actually enter the image must still fail.
        let crossing = ViewBounds {
            min: [-0.01, -0.01, -1.0],
            max: [0.01, 0.01, 2.0],
        };
        assert!(p
            .projected_visible_vertical_error_px(e, crossing, up)
            .is_infinite());
    }
    #[test]
    fn visible_vertical_bound_retains_counterpart_only_silhouettes() {
        let p = projection(1080);
        let b = ViewBounds {
            min: [-0.1, -2.0, 0.01],
            max: [0.1, -2.0, 10.0],
        };
        let e = 1.9;
        let a = [0.0, -2.0, 0.2];
        let q = [0.0, -0.1, 0.2];
        let bound = p.projected_visible_vertical_error_px(e, b, [0.0, 1.0, 0.0]);
        assert!(bound.is_finite());
        assert!(projection_distance(p, a, q) <= bound);
        // Clipping a alone would yield about 513px, missing the native-only
        // silhouette by over an order of magnitude.
        assert!(bound > 8000.0);
        // A segment can span the viewport even when neither endpoint is inside.
        let spanning = ViewBounds {
            min: [-1.0, -1.0, 0.1],
            max: [-1.0, -1.0, 1.0],
        };
        let up = [1.0 / 2.0_f64.sqrt(), 1.0 / 2.0_f64.sqrt(), 0.0];
        let e = 2.0 * 2.0_f64.sqrt();
        let a = [-1.0, -1.0, 0.1];
        let q = [1.0, 1.0, 0.1];
        assert!(segment_enters_view(p, a, q));
        assert!(
            projection_distance(p, a, q) <= p.projected_visible_vertical_error_px(e, spanning, up)
        );
    }
    fn projection_distance(p: Projection, a: [f64; 3], q: [f64; 3]) -> f64 {
        let f = p.height as f64 / (2.0 * (p.vertical_fov_radians * 0.5).tan());
        f * (a[0] / a[2] - q[0] / q[2]).hypot(a[1] / a[2] - q[1] / q[2])
    }
    // Independent parametric segment/frustum intersection for test relevance.
    // It includes a segment crossing the viewport with both endpoints outside.
    fn segment_enters_view(p: Projection, a: [f64; 3], q: [f64; 3]) -> bool {
        let ty = (p.vertical_fov_radians * 0.5).tan();
        let tx = ty * p.width as f64 / p.height as f64;
        let planes = |v: [f64; 3]| {
            [
                v[2] - p.near_m,
                v[0] + tx * v[2],
                -v[0] + tx * v[2],
                v[1] + ty * v[2],
                -v[1] + ty * v[2],
            ]
        };
        let mut first = 0.0_f64;
        let mut last = 1.0_f64;
        for (a, q) in planes(a).into_iter().zip(planes(q)) {
            if a < 0.0 && q < 0.0 {
                return false;
            }
            if a < 0.0 {
                first = first.max(-a / (q - a));
            }
            if q < 0.0 {
                last = last.min(a / (a - q));
            }
        }
        first <= last
    }
    #[test]
    fn visible_vertical_bound_contains_adversarial_directed_segments() {
        let mut state = 19_u64;
        let mut random = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 11) as f64 / (1_u64 << 53) as f64
        };
        let mut relevant = 0;
        let mut near_crossings = 0;
        for case in 0..160 {
            let p = Projection {
                vertical_fov_radians: [0.1, 0.7, 1.5, 2.8][case % 4],
                width: [1920, 1080, 1, 8000][case % 4],
                height: 1080,
                near_m: [0.025, 0.001, 0.5][case % 3],
            };
            let min = [
                random() * 24.0 - 12.0,
                random() * 24.0 - 12.0,
                random() * 8.0 - 4.0,
            ];
            let b = ViewBounds {
                min,
                max: [
                    min[0] + random() * 20.0,
                    min[1] + random() * 20.0,
                    min[2] + random() * 30.0,
                ],
            };
            let raw = [
                random() * 2.0 - 1.0,
                random() * 2.0 - 1.0,
                random() * 2.0 - 1.0,
            ];
            let norm = raw[0].hypot(raw[1]).hypot(raw[2]);
            let up = raw.map(|x| x / norm);
            for e in [0.000001, 0.0625, 0.3, 3.0] {
                let bound = p.projected_visible_vertical_error_px(e, b, up);
                assert!(!bound.is_nan());
                assert!(bound <= p.projected_vertical_error_px(e, b, up));
                for x in [0.0, 0.5, 1.0] {
                    for y in [0.0, 0.5, 1.0] {
                        for z in [0.0, 0.5, 1.0] {
                            let a = std::array::from_fn(|i| {
                                b.min[i] + [x, y, z][i] * (b.max[i] - b.min[i])
                            });
                            for displacement in [-1.0, -0.125, 0.125, 1.0] {
                                let q = std::array::from_fn(|i| a[i] + displacement * e * up[i]);
                                if !segment_enters_view(p, a, q) {
                                    continue;
                                }
                                relevant += 1;
                                if a[2] <= p.near_m || q[2] <= p.near_m {
                                    near_crossings += 1;
                                    assert!(
                                        bound.is_infinite(),
                                        "near crossing case={case} a={a:?} q={q:?} bound={bound}"
                                    );
                                } else {
                                    // Evaluate the exact vertical relation algebraically;
                                    // subtracting two projected near-identical endpoints
                                    // would introduce cancellation unrelated to its error.
                                    let t = displacement * e;
                                    let f = p.height as f64
                                        / (2.0 * (p.vertical_fov_radians * 0.5).tan());
                                    let actual = f
                                        * t.abs()
                                        * (up[0] * a[2] - a[0] * up[2])
                                            .hypot(up[1] * a[2] - a[1] * up[2])
                                        / (a[2] * (a[2] + t * up[2]));
                                    assert!(actual <= bound, "case={case} actual={actual:.17} bound={bound:.17} a={a:?} q={q:?}");
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(relevant > 1000);
        assert!(near_crossings > 0);
    }
    #[test]
    fn visible_vertical_bound_fails_closed_on_invalid_inputs() {
        let p = projection(1080);
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            assert!(p
                .projected_visible_vertical_error_px(bad, bounds(10.0), [0.0, 1.0, 0.0])
                .is_infinite());
            if !bad.is_finite() {
                assert!(p
                    .projected_visible_vertical_error_px(0.1, bounds(10.0), [bad, 1.0, 0.0])
                    .is_infinite());
            }
        }
        assert!(p
            .projected_visible_vertical_error_px(
                0.1,
                ViewBounds {
                    min: [0.0, 0.0, 2.0],
                    max: [0.0, 0.0, 1.0]
                },
                [0.0, 1.0, 0.0]
            )
            .is_infinite());
        assert_eq!(
            p.projected_visible_vertical_error_px(0.0, bounds(-1.0), [0.0, 1.0, 0.0]),
            0.0
        );
    }
    fn test_clip_matrix(p: Projection, base: f32, pitch: f32) -> [[f32; 4]; 4] {
        let ty = (p.vertical_fov_radians as f32 * 0.5).tan();
        let fx = 1.0 / (ty * p.width as f32 / p.height as f32);
        let fy = 1.0 / ty;
        let (s, c) = pitch.sin_cos();
        let alpha = 300.0 / (300.0 - p.near_m as f32);
        let beta = -alpha * p.near_m as f32;
        let rows = [
            [fx, 0.0, 0.0, -fx * base],
            [0.0, fy * c, -fy * s, -fy * c * base],
            [0.0, alpha * s, alpha * c, beta - alpha * s * base],
            [0.0, s, c, -s * base],
        ];
        std::array::from_fn(|c| std::array::from_fn(|r| rows[r][c]))
    }
    fn shader_dot(matrix: [[f32; 4]; 4], point: [f32; 3], balanced: bool) -> [f64; 4] {
        std::array::from_fn(|r| {
            let products = [
                matrix[0][r] * point[0],
                matrix[1][r] * point[1],
                matrix[2][r] * point[2],
                matrix[3][r],
            ];
            if balanced {
                ((products[0] + products[1]) + (products[2] + products[3])) as f64
            } else {
                (((products[0] + products[1]) + products[2]) + products[3]) as f64
            }
        })
    }
    fn clip_segment_enters(a: [f64; 4], q: [f64; 4]) -> bool {
        let planes = |v: [f64; 4]| {
            [
                v[3] + v[0],
                v[3] - v[0],
                v[3] + v[1],
                v[3] - v[1],
                v[2],
                v[3] - v[2],
            ]
        };
        let mut first = 0.0_f64;
        let mut last = 1.0_f64;
        for (a, q) in planes(a).into_iter().zip(planes(q)) {
            if a < 0.0 && q < 0.0 {
                return false;
            }
            if a < 0.0 {
                first = first.max(-a / (q - a));
            }
            if q < 0.0 {
                last = last.min(a / (a - q));
            }
        }
        first <= last
    }
    #[test]
    fn f32_vertical_bound_covers_actual_shader_rounding_and_reconstruction() {
        let p = projection(1080);
        let mut finite_checks = 0;
        let mut relevant_checks = 0;
        for base in [0.0_f32, 100.0, 10000.0, 100000.0] {
            for pitch in [-0.3_f32, 0.0, 0.3] {
                let matrix = test_clip_matrix(p, base, pitch);
                let b = ViewBounds {
                    min: [base as f64 - 2.0, base as f64 - 2.0, -1.0],
                    max: [base as f64 + 2.0, base as f64 - 1.6, 30.0],
                };
                for e in [0.001, 0.0625, 0.3, 1.9] {
                    let bound = p.projected_f32_visible_vertical_error_px(e, b, matrix);
                    assert!(!bound.is_nan());
                    for x in [0.0, 0.5, 1.0] {
                        for y in [0.0, 0.5, 1.0] {
                            for z in [0.0, 0.5, 1.0] {
                                let a: [f32; 3] = std::array::from_fn(|i| {
                                    (b.min[i] + [x, y, z][i] * (b.max[i] - b.min[i])) as f32
                                });
                                for sign in [-1.0, 1.0] {
                                    let mut q = a;
                                    // Same two-step extent conversion/reconstruction as
                                    // the packed native path, including swallowed offsets.
                                    q[1] += (sign * e) as f32;
                                    for balanced in [false, true] {
                                        let a = shader_dot(matrix, a, balanced);
                                        let q = shader_dot(matrix, q, balanced);
                                        if !clip_segment_enters(a, q) {
                                            continue;
                                        }
                                        relevant_checks += 1;
                                        if a[2] <= 0.0 || q[2] <= 0.0 || a[3] <= 0.0 || q[3] <= 0.0
                                        {
                                            assert!(bound.is_infinite());
                                        } else {
                                            let actual = (p.width as f64
                                                * 0.5
                                                * (a[0] / a[3] - q[0] / q[3]))
                                                .hypot(
                                                    p.height as f64
                                                        * 0.5
                                                        * (a[1] / a[3] - q[1] / q[3]),
                                                );
                                            assert!(actual<=bound,"base={base} pitch={pitch} e={e} actual={actual} bound={bound}");
                                            finite_checks += usize::from(bound.is_finite());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(relevant_checks > 100);
        assert!(finite_checks > 100);
        let floor = ViewBounds {
            min: [-8.0, -1.8, -1.0],
            max: [8.0, -1.6, 20.0],
        };
        assert!(
            p.projected_f32_visible_vertical_error_px(0.001, floor, test_clip_matrix(p, 0.0, 0.0))
                < 1.0
        );
        assert_eq!(
            p.projected_f32_visible_vertical_error_px(0.0, floor, test_clip_matrix(p, 0.0, 0.0)),
            0.0
        );
    }
    #[test]
    fn f32_vertical_bound_rejects_bad_or_overflowing_transforms() {
        let p = projection(1080);
        let b = bounds(10.0);
        let mut m = test_clip_matrix(p, 0.0, 0.0);
        m[0][0] = f32::NAN;
        assert!(p
            .projected_f32_visible_vertical_error_px(0.1, b, m)
            .is_infinite());
        m[0][0] = f32::MAX;
        assert!(p
            .projected_f32_visible_vertical_error_px(0.1, b, m)
            .is_infinite());
        assert!(p
            .projected_f32_visible_vertical_error_px(-1.0, b, test_clip_matrix(p, 0.0, 0.0))
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
