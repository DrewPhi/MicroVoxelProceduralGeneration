//! Conservative rejection behind a single, guaranteed-solid convex box.
//!
//! A caller must prove every occluder box is solid in the current source revision.
//! For a camera outside a convex solid C, its shadow
//! `{ camera + lambda*(p-camera) : p in C, lambda >= 1 }` is convex: a convex
//! combination of two such points has lambda equal to the same combination of
//! their lambdas, and its normalized point remains in C. Therefore a candidate
//! AABB is hidden when ALL eight corners lie strictly behind the SAME box.
//! Corner hits on different boxes never establish this convexity argument.

pub const MAX_OCCLUDERS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}
impl Aabb {
    pub fn valid(self) -> bool {
        (0..3).all(|a| {
            self.min[a].is_finite() && self.max[a].is_finite() && self.min[a] <= self.max[a]
        })
    }
    fn corner(self, i: usize) -> [f64; 3] {
        std::array::from_fn(|a| {
            if i & (1 << a) == 0 {
                self.min[a]
            } else {
                self.max[a]
            }
        })
    }
    fn contains_origin(self) -> bool {
        (0..3).all(|a| self.min[a] <= 0.0 && self.max[a] >= 0.0)
    }
}
#[derive(Clone, Copy)]
struct Occluder {
    relative: Aabb,
    priority: f64,
}

/// A bounded, current-camera set. Reconstruct when the camera changes; source
/// boxes remain independently reusable. A rejected insertion never affects safety.
pub struct OcclusionSet {
    camera: [f64; 3],
    forward: [f64; 3],
    right: [f64; 3],
    up: [f64; 3],
    near_m: f64,
    tolerance_m: f64,
    subtraction_margin: f64,
    occluders: Vec<Occluder>,
}
impl OcclusionSet {
    pub fn new(camera: [f64; 3], forward: [f64; 3], near_m: f64, tolerance_m: f64) -> Option<Self> {
        if camera.iter().chain(&forward).any(|v| !v.is_finite())
            || !near_m.is_finite()
            || near_m <= 0.0
            || !tolerance_m.is_finite()
            || tolerance_m < 0.0
        {
            return None;
        }
        let forward = unit(forward)?;
        let reference = if forward[1].abs() < 0.9 {
            [0.0, 1.0, 0.0]
        } else {
            [1.0, 0.0, 0.0]
        };
        let right = unit(cross(reference, forward))?;
        let up = cross(forward, right);
        // Global subtraction error is charged before using camera-relative math.
        // It only shrinks possible rejection; no epsilon enlarges an occluder.
        let scale = camera.iter().fold(1.0_f64, |a, v| a.max(v.abs()));
        Some(Self {
            camera,
            forward,
            right,
            up,
            near_m,
            tolerance_m,
            subtraction_margin: scale * (64.0 * f64::EPSILON),
            occluders: vec![],
        })
    }
    pub fn len(&self) -> usize {
        self.occluders.len()
    }
    pub fn is_empty(&self) -> bool {
        self.occluders.is_empty()
    }

    /// `bounds` must be wholly canonical solid, with no edited/cave/volume holes.
    /// Shrinking and near-plane exclusion make the proof valid for rendered rays.
    pub fn insert_solid(&mut self, bounds: Aabb) -> bool {
        let Some((relative, margin)) = self.relative(bounds, false) else {
            return false;
        };
        // A containing box cannot occlude a camera and is especially unsafe when
        // the GPU near clip removes its front surface.
        let original = Aabb {
            min: std::array::from_fn(|a| bounds.min[a] - self.camera[a]),
            max: std::array::from_fn(|a| bounds.max[a] - self.camera[a]),
        };
        if original.contains_origin()
            || (0..8).any(|i| dot(original.corner(i), self.forward) <= self.near_m + margin * 4.)
            || (0..3).any(|a| relative.min[a] >= relative.max[a])
        {
            return false;
        }
        let mut front = f64::INFINITY;
        let mut projected_min = [f64::INFINITY; 2];
        let mut projected_max = [f64::NEG_INFINITY; 2];
        for i in 0..8 {
            let p = relative.corner(i);
            let z = dot(p, self.forward);
            // Require the whole shrunken box beyond the actual view near plane.
            // Occluders intersecting that plane are ignored, never capped.
            if !z.is_finite() || z <= self.near_m + margin {
                return false;
            }
            front = front.min(z);
            for (a, basis) in [self.right, self.up].into_iter().enumerate() {
                let q = dot(p, basis) / z;
                if !q.is_finite() {
                    return false;
                }
                projected_min[a] = projected_min[a].min(q);
                projected_max[a] = projected_max[a].max(q);
            }
        }
        if self.occluders.iter().any(|c| c.relative == relative) {
            return false;
        }
        // This priority affects retention only, never the visibility proof.
        // Prefer frontmost boxes with broad angular coverage, capped at 64.
        let area = ((projected_max[0] - projected_min[0]) * (projected_max[1] - projected_min[1]))
            .min(1e12);
        let priority = area / (1.0 + front);
        if !priority.is_finite() || priority <= 0.0 {
            return false;
        }
        let next = Occluder { relative, priority };
        if self.occluders.len() == MAX_OCCLUDERS {
            let worst = self
                .occluders
                .iter()
                .enumerate()
                .max_by(|a, b| compare_occluders(a.1, b.1))
                .unwrap()
                .0;
            if compare_occluders(&next, &self.occluders[worst]) != std::cmp::Ordering::Less {
                return false;
            }
            self.occluders[worst] = next;
        } else {
            self.occluders.push(next);
        }
        self.occluders.sort_by(compare_occluders);
        true
    }

    /// The candidate must enclose all possible current source geometry, including
    /// its certified approximation error. Unknown geometry needs global bounds.
    pub fn occludes(&self, candidate: Aabb) -> bool {
        let Some((candidate, margin)) = self.relative(candidate, true) else {
            return false;
        };
        if candidate.contains_origin() {
            return false;
        }
        self.occluders.iter().any(|occluder| {
            (0..8).all(|i| segment_enters_before(candidate.corner(i), occluder.relative, margin))
        })
    }

    fn relative(&self, bounds: Aabb, expand: bool) -> Option<(Aabb, f64)> {
        if !bounds.valid() {
            return None;
        }
        let min: [f64; 3] = std::array::from_fn(|a| bounds.min[a] - self.camera[a]);
        let max: [f64; 3] = std::array::from_fn(|a| bounds.max[a] - self.camera[a]);
        let scale = min.iter().chain(&max).fold(1.0_f64, |a, v| a.max(v.abs()));
        let margin =
            self.tolerance_m.max(1e-9) + self.subtraction_margin + scale * (64.0 * f64::EPSILON);
        let sign = if expand { 1.0 } else { -1.0 };
        let relative = Aabb {
            min: std::array::from_fn(|a| min[a] - sign * margin),
            max: std::array::from_fn(|a| max[a] + sign * margin),
        };
        if !margin.is_finite() || !relative.valid() {
            return None;
        }
        Some((relative, margin))
    }
}
fn compare_occluders(a: &Occluder, b: &Occluder) -> std::cmp::Ordering {
    let mut order = b.priority.total_cmp(&a.priority);
    for axis in 0..3 {
        order = order.then_with(|| a.relative.min[axis].total_cmp(&b.relative.min[axis]));
    }
    for axis in 0..3 {
        order = order.then_with(|| a.relative.max[axis].total_cmp(&b.relative.max[axis]));
    }
    order
}
fn segment_enters_before(endpoint: [f64; 3], bounds: Aabb, margin: f64) -> bool {
    let mut enter = 0.0_f64;
    let mut exit = 1.0_f64;
    for a in 0..3 {
        let d = endpoint[a];
        if d == 0.0 {
            if bounds.min[a] > 0.0 || bounds.max[a] < 0.0 {
                return false;
            }
            continue;
        }
        let a0 = bounds.min[a] / d;
        let a1 = bounds.max[a] / d;
        enter = enter.max(a0.min(a1));
        exit = exit.min(a0.max(a1));
        if !enter.is_finite() || !exit.is_finite() || enter > exit {
            return false;
        }
    }
    let distance = endpoint[0].hypot(endpoint[1]).hypot(endpoint[2]);
    if !distance.is_finite() || distance <= margin {
        return false;
    }
    // Strictly before the candidate, with an extra world-distance safety gap.
    enter > 0.0 && enter <= exit && enter < 1.0 - margin / distance
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] * b[0] + a[1] * b[1]) + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn unit(v: [f64; 3]) -> Option<[f64; 3]> {
    let length = v[0].hypot(v[1]).hypot(v[2]);
    if !length.is_finite() || length <= 0.0 {
        return None;
    }
    Some(v.map(|v| v / length))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn b(min: [f64; 3], max: [f64; 3]) -> Aabb {
        Aabb { min, max }
    }
    fn set() -> OcclusionSet {
        OcclusionSet::new([0.; 3], [0., 0., 1.], 0.1, 1e-7).unwrap()
    }
    // Independent face-plane intersection oracle, not the production slab test.
    fn oracle(camera: [f64; 3], end: [f64; 3], bounds: Aabb) -> bool {
        for axis in 0..3 {
            for plane in [bounds.min[axis], bounds.max[axis]] {
                let d = end[axis] - camera[axis];
                if d == 0. {
                    continue;
                }
                let t = (plane - camera[axis]) / d;
                if t <= 0. || t >= 1. {
                    continue;
                }
                let p: [f64; 3] = std::array::from_fn(|a| camera[a] + t * (end[a] - camera[a]));
                if (0..3).all(|a| a == axis || p[a] >= bounds.min[a] && p[a] <= bounds.max[a]) {
                    return true;
                }
            }
        }
        false
    }
    #[test]
    fn one_box_shadows_entire_candidate_verified_by_dense_independent_oracle() {
        let solid = b([-2., -2., 2.], [2., 2., 3.]);
        let candidate = b([-1., -1., 8.], [1., 1., 9.]);
        let mut s = set();
        assert!(s.insert_solid(solid));
        assert!(s.occludes(candidate));
        for z in 0..9 {
            for y in 0..9 {
                for x in 0..9 {
                    let p = std::array::from_fn(|a| {
                        candidate.min[a]
                            + (candidate.max[a] - candidate.min[a]) * [x, y, z][a] as f64 / 8.
                    });
                    assert!(oracle([0.; 3], p, solid));
                }
            }
        }
    }
    #[test]
    fn visible_sliver_and_mixed_box_corner_coverage_never_reject() {
        let mut s = set();
        s.insert_solid(b([-2., -2., 2.], [2., 2., 3.]));
        assert!(!s.occludes(b([-1., -1., 8.], [9., 1., 9.])));
        // Each corner is hidden by one box, but a central slit remains visible.
        let left = b([-2., -2., 2.], [-0.01, 2., 3.]);
        let right = b([0.01, -2., 2.], [2., 2., 3.]);
        let candidate = b([-1., -1., 8.], [1., 1., 9.]);
        let mut split = set();
        split.insert_solid(left);
        split.insert_solid(right);
        assert!((0..8).all(|i| oracle([0.; 3], candidate.corner(i), left)
            || oracle([0.; 3], candidate.corner(i), right)));
        assert!(!oracle([0.; 3], [0., 0., 8.], left) && !oracle([0.; 3], [0., 0., 8.], right));
        assert!(!split.occludes(candidate));
    }
    #[test]
    fn camera_inside_near_clipping_and_tangent_cases_stay_visible() {
        let mut s = set();
        assert!(!s.insert_solid(b([-1.; 3], [1.; 3])));
        assert!(!s.insert_solid(b([-1., -1., 0.01], [1., 1., 0.05])));
        assert!(!s.insert_solid(b([-1., -1., 0.05], [1., 1., 0.2])));
        assert!(s.insert_solid(b([-2., -2., 2.], [2., 2., 3.])));
        assert!(!s.occludes(b([-1., -1., 0.05], [1., 1., 8.])));
        assert!(!s.occludes(b([-1.; 3], [1.; 3])));
        // Its right front corner only grazes the original occluder's right edge.
        assert!(!s.occludes(b([7.9, -0.1, 8.], [8., 0.1, 8.1])));
    }
    #[test]
    fn camera_relative_large_world_and_rotated_near_plane_are_conservative() {
        let camera = [1e9, -1e9, 1e9];
        let shift = |p: [f64; 3]| std::array::from_fn(|a| p[a] + camera[a]);
        let mut s = OcclusionSet::new(camera, [0., 0., 1.], 0.1, 1e-7).unwrap();
        assert!(s.insert_solid(b(shift([-2., -2., 2.]), shift([2., 2., 3.]))));
        assert!(s.occludes(b(shift([-1., -1., 8.]), shift([1., 1., 9.]))));
        let mut rotated = OcclusionSet::new([0.; 3], [1., 0., 0.], 0.1, 1e-7).unwrap();
        assert!(rotated.insert_solid(b([2., -2., -2.], [3., 2., 2.])));
        assert!(rotated.occludes(b([8., -1., -1.], [9., 1., 1.])));
        assert!(!rotated.insert_solid(b([0.05, -2., -2.], [0.2, 2., 2.])));
    }
    #[test]
    fn camera_motion_and_turn_recompute_visibility_without_hidden_history() {
        let solid = b([-2., -2., 2.], [2., 2., 3.]);
        let candidate = b([-1., -1., 8.], [1., 1., 9.]);
        let mut first = set();
        assert!(first.insert_solid(solid));
        assert!(first.occludes(candidate));
        let mut moved = OcclusionSet::new([10., 0., 0.], [-10., 0., 8.5], 0.1, 1e-7).unwrap();
        assert!(moved.insert_solid(solid));
        assert!(!moved.occludes(candidate));
        let mut turned = OcclusionSet::new([0.; 3], [0., 0., -1.], 0.1, 1e-7).unwrap();
        assert!(!turned.insert_solid(solid));
        assert!(!turned.occludes(candidate));
    }
    #[test]
    fn equal_priority_retention_is_independent_of_insertion_order() {
        let mut boxes = vec![b([-100., -100., 0.2], [100., 100., 0.3])];
        for i in 1..=50 {
            let x = i as f64;
            boxes.push(b([x, -1., 2.], [x + 1., 1., 3.]));
            boxes.push(b([-x - 1., -1., 2.], [-x, 1., 3.]));
        }
        let mut forward = set();
        let mut reverse = set();
        for &bounds in &boxes {
            forward.insert_solid(bounds);
        }
        for &bounds in boxes.iter().rev() {
            reverse.insert_solid(bounds);
        }
        assert_eq!(forward.len(), MAX_OCCLUDERS);
        assert_eq!(
            forward
                .occluders
                .iter()
                .map(|o| o.relative)
                .collect::<Vec<_>>(),
            reverse
                .occluders
                .iter()
                .map(|o| o.relative)
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn bounded_retention_invalid_inputs_and_solid_front_surface_are_not_culled() {
        assert!(OcclusionSet::new([f64::NAN, 0., 0.], [0., 0., 1.], 0.1, 0.).is_none());
        assert!(OcclusionSet::new([0.; 3], [0.; 3], 0.1, 0.).is_none());
        let mut s = set();
        assert!(!s.insert_solid(b([f64::NAN; 3], [1.; 3])));
        let solid = b([-2., -2., 2.], [2., 2., 3.]);
        s.insert_solid(solid);
        assert!(!s.occludes(solid));
        assert!(!s.occludes(b([f64::NAN; 3], [1.; 3])));
        for i in 0..100 {
            let z = 4. + i as f64;
            s.insert_solid(b([-2., -2., z], [2., 2., z + 0.5]));
        }
        assert!(s.len() <= MAX_OCCLUDERS);
    }
}
