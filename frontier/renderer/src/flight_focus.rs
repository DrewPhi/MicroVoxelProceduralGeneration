//! Scheduling hints from the terrain a pilot is looking at. Full source bounds,
//! edge closure and ownership checks remain authoritative at publication.
use super::{split_refinement, Camera, Node, RefinementCandidate, TileKey};
use frontier_world::{voxel_size, World};
use glam::{DVec2, DVec3, Vec3Swizzles};
use std::collections::HashSet;

#[derive(Clone, Copy)]
struct Hit {
    column: [i64; 2],
    depth: f64,
    grade: f64,
}

#[derive(Default)]
pub(super) struct Focus {
    view: Option<(Camera, [u32; 2], f64)>,
    hits: Vec<Hit>,
    skyline_count: usize,
}

/// A foreground shelf can lie just below a farther summit on the same ray.
/// Either shelf can own the rendered silhouette while square roofs are coarse.
fn skyline_crests(
    end: f64,
    blocks: bool,
    angular_tolerance: f64,
    probes: &mut Vec<(f64, f64)>,
    mut slope: impl FnMut(f64) -> f64,
) -> [Option<f64>; 2] {
    let mut range = 64_f64.min(end);
    let mut best = (range, f64::NEG_INFINITY);
    probes.clear();
    loop {
        let elevation = slope(range);
        if blocks {
            probes.push((range, elevation));
        }
        if elevation > best.1 {
            best = (range, elevation);
        }
        if range >= end {
            break;
        }
        range = (range + (range * 0.045).clamp(32., 96.)).min(end);
    }
    if !blocks {
        return [Some(best.0), None];
    }
    // Choose a separate local maximum. Samples on the shoulder of the
    // winning ridge often outrank a genuine second ridge farther along.
    let separation = (best.0 * 0.10).max(120.);
    let mut alternate = None;
    for (i, &(r, e)) in probes.iter().enumerate() {
        if (r - best.0).abs() < separation
            || i > 0 && probes[i - 1].1 > e
            || i + 1 < probes.len() && probes[i + 1].1 > e
        {
            continue;
        }
        if alternate.is_none_or(|(_, previous)| e > previous) {
            alternate = Some((r, e));
        }
    }
    let mut refine = |(center, elevation): (f64, f64)| {
        let span = (center * 0.045).clamp(32., 96.);
        let mut crest = (center, elevation);
        for tap in [-3, -2, -1, 1, 2, 3] {
            let d = (center + f64::from(tap) * span * 0.25).clamp(1., end);
            let e = slope(d);
            if e > crest.1 {
                crest = (d, e);
            }
        }
        crest
    };
    let primary = refine(best);
    let Some(secondary) = alternate.map(refine) else {
        return [Some(primary.0), None];
    };
    if (primary.1 - secondary.1).abs() <= angular_tolerance {
        [Some(primary.0), Some(secondary.0)]
    } else {
        [
            Some(if primary.1 >= secondary.1 {
                primary.0
            } else {
                secondary.0
            }),
            None,
        ]
    }
}

// Four source samples estimate how much a displaced block edge can change
// visible elevation. Use the same physical radius on both sides of the hit.
fn terrain_grade(world: &World, point: DVec2, height: f64, radius: f64) -> f64 {
    [DVec2::X, -DVec2::X, DVec2::Y, -DVec2::Y]
        .into_iter()
        .map(|axis| {
            let q = point + axis * radius;
            (world.height_m(q.x, q.y) - height).abs() / radius
        })
        .fold(0., f64::max)
}

impl Focus {
    fn survey(&mut self, world: &World, camera: &Camera, viewport: [u32; 2], pixel_error: f64) {
        let aspect = f64::from(viewport[0]) / f64::from(viewport[1].max(1));
        self.view = Some((*camera, viewport, pixel_error));
        self.hits.clear();
        self.skyline_count = 0;
        let eye = camera.position;
        let low = eye.y - world.height_m(eye.x, eye.z) < 45.;
        let forward = camera.direction().as_dvec3();
        let right = forward
            .cross(camera.view_up().as_dvec3())
            .normalize_or_zero();
        let up = right.cross(forward);
        let half_height = (f64::from(camera.fov_y).to_radians() * 0.5).tan();
        let far = f64::from(camera.far).min(4500.);
        // Across the view, find the source column with the greatest elevation
        // angle. A horizontal ray can pass above every summit on this line.
        // Those summits still form the terrain's most visible boundary.
        let blocks = crate::adaptive_geometry::distant_voxels::Mode::current()
            == crate::adaptive_geometry::distant_voxels::Mode::Blocks;
        if low && !blocks {
            return;
        }
        let half_samples = if blocks { 16 } else { 5 };
        let view_fraction = if blocks { 0.99 } else { 0.95 };
        let angular_tolerance = 4. * half_height * pixel_error / f64::from(viewport[1].max(1));
        let mut probes = Vec::with_capacity(128);
        for i in -half_samples..=half_samples {
            let across =
                f64::from(i) / f64::from(half_samples) * view_fraction * aspect * half_height;
            let d = forward + right * across;
            let length = d.x.hypot(d.z);
            if length < 0.1 {
                continue;
            }
            let heading = d.xz() / length;
            let project = heading.dot(forward.xz()).max(0.5);
            // Clip distance is measured along the camera forward, whereas the
            // survey walks in horizontal metres. The corner of the frustum
            // reaches farther than its center, even with a level aircraft.
            let end = if blocks {
                (f64::from(camera.far) / project).min(10000.)
            } else {
                far
            };
            let terrain = |distance: f64| {
                let p = eye.xz() + heading * distance;
                let height = world.height_m(p.x, p.y);
                ((height - eye.y) / distance, p, height)
            };
            let crests = skyline_crests(end, blocks, angular_tolerance, &mut probes, |distance| {
                terrain(distance).0
            });
            for best in crests.into_iter().flatten() {
                let (_, p, h) = terrain(best);
                let relative = DVec3::new(p.x - eye.x, h - eye.y, p.y - eye.z);
                if low && (best < 100. || relative.y < 0.025 * best) {
                    continue;
                }
                let depth = relative.dot(forward);
                if blocks && (depth <= 0. || relative.dot(up).abs() > 1.10 * depth * half_height) {
                    continue;
                }
                let mut grade = 1.;
                if blocks {
                    // A coarse column on a cliff can shift its visible height by
                    // several times its horizontal sampling error.
                    let radius = (best * 0.008).clamp(6., 32.);
                    grade = terrain_grade(world, p, h, radius);
                }
                self.hits.push(Hit {
                    column: p.to_array().map(|v| (v / voxel_size()).floor() as i64),
                    depth: if blocks {
                        relative.dot(forward).max(1.)
                    } else {
                        best
                    },
                    grade,
                });
            }
        }
        self.skyline_count = self.hits.len();
        if low {
            // Collision, editable columns and the terrain underfoot already
            // have their own scheduling priority. Only promote distant crests.
            return;
        }
        let base = [
            (0., 0.05),
            (-0.65, 0.03),
            (0.65, 0.03),
            (0., -0.49),
            (-0.73, -0.48),
            (0.73, -0.48),
            (0., 0.38),
        ];
        let foreground = [
            (-0.90, -0.84),
            (-0.35, -0.78),
            (0.34, -0.78),
            (0.90, -0.84),
            (-0.38, -0.19),
            (0.38, -0.19),
            (-0.92, -0.20),
            (0.92, -0.20),
            (-0.39, 0.32),
            (0.39, 0.32),
            (-0.82, 0.62),
            (0.82, 0.62),
        ];
        for &(h, v) in base.iter().chain(foreground.iter().filter(|_| blocks)) {
            let direction =
                (forward + right * (h * aspect * half_height) + up * (v * half_height)).normalize();
            let above = |p: DVec3| p.y > world.height_m(p.x, p.z);
            let mut lo = 0.;
            let mut hi = 40_f64.min(far);
            loop {
                if !above(eye + direction * hi) {
                    for _ in 0..4 {
                        let mid = (lo + hi) * 0.5;
                        if above(eye + direction * mid) {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    let p = eye + direction * hi;
                    self.hits.push(Hit {
                        column: [p.x, p.z].map(|x| (x / voxel_size()).floor() as i64),
                        depth: if blocks {
                            hi * direction.dot(forward)
                        } else {
                            hi
                        },
                        grade: if blocks {
                            terrain_grade(
                                world,
                                p.xz(),
                                world.height_m(p.x, p.z),
                                (hi * 0.008).clamp(3., 24.),
                            )
                        } else {
                            1.
                        },
                    });
                    break;
                }
                if hi >= far {
                    break;
                }
                lo = hi;
                hi = (hi + (hi * 0.13).clamp(40., 180.)).min(far);
            }
        }
    }

    fn update(&mut self, world: &World, camera: &Camera, viewport: [u32; 2], pixel_error: f64) {
        if self.view.is_none_or(|(old, size, target)| {
            size != viewport
                || target != pixel_error
                || old.fov_y != camera.fov_y
                || old.far != camera.far
                || old.position.distance_squared(camera.position) > 48. * 48.
                || old.direction().dot(camera.direction()) < 0.997
                || (old.roll - camera.roll).abs() > 0.08
        }) {
            self.survey(world, camera, viewport, pixel_error);
        }
    }

    pub(super) fn seed_flight_view(
        &mut self,
        world: &World,
        camera: &Camera,
        viewport: [u32; 2],
        pixel_error: f64,
        roots: &mut Vec<Node>,
        max_nodes: usize,
    ) {
        let Some(root) = roots.first().copied() else {
            return;
        };
        if camera.position.y - world.height_m(camera.position.x, camera.position.z) < 45. {
            return;
        }
        self.update(world, camera, viewport, pixel_error);
        let pixel_angle =
            2. * (f64::from(camera.fov_y).to_radians() * 0.5).tan() / f64::from(viewport[1].max(1));
        self.split_coarse_crests(roots, root, pixel_angle, max_nodes);
    }

    fn split_coarse_crests(
        &self,
        nodes: &mut Vec<Node>,
        root: Node,
        pixel_angle: f64,
        max_nodes: usize,
    ) {
        // Each split replaces one coarse owner with four children that cover
        // exactly its area. Limit the cold-start work and use the same keys
        // that normal flight refinement would generate once coverage is live.
        let segments = root.width / root.step;
        for _ in 0..4 {
            if nodes.len().saturating_add(3) > max_nodes {
                break;
            }
            let mut best = None;
            for (i, node) in nodes.iter().enumerate() {
                if node.width <= root.width / 4 || node.step < 2 {
                    continue;
                }
                let pitch = f64::from(node.step) * voxel_size();
                let mut score = 0.;
                for hit in &self.hits[..self.skyline_count] {
                    if (0..2).all(|a| {
                        (node.min[a]..node.min[a] + i64::from(node.width)).contains(&hit.column[a])
                    }) {
                        let pixels = pitch / (pixel_angle * hit.depth);
                        score += (pixels - 16.).clamp(0., 128.);
                    }
                }
                if score > 0. && best.is_none_or(|(_, value)| score > value) {
                    best = Some((i, score));
                }
            }
            let Some((i, _)) = best else {
                break;
            };
            let parent = nodes.swap_remove(i);
            split_refinement(parent, Some(0), segments, nodes);
        }
    }

    pub(super) fn select(
        &mut self,
        world: &World,
        camera: &Camera,
        viewport: [u32; 2],
        pixel_error: f64,
        candidates: &[RefinementCandidate],
        budget: usize,
    ) -> Option<HashSet<TileKey>> {
        if candidates.is_empty() || budget == 0 {
            return None;
        }
        let pixel_angle =
            2. * (f64::from(camera.fov_y).to_radians() * 0.5).tan() / f64::from(viewport[1].max(1));
        self.update(world, camera, viewport, pixel_error);
        let blocks = crate::adaptive_geometry::distant_voxels::Mode::current()
            == crate::adaptive_geometry::distant_voxels::Mode::Blocks;
        let mut priorities = Vec::new();
        let mut skyline = None;
        let mut foreground = None;
        for c in candidates {
            let key = c.key;
            let side = i64::from(key.width_cells);
            let step = f64::from(key.step_cells) * voxel_size();
            let mut debt = [0_f64; 2];
            for (i, hit) in self.hits.iter().enumerate() {
                if (0..2).all(|a| hit.column[a] >= key.min[a] && hit.column[a] < key.min[a] + side)
                {
                    let limit = if blocks && i < self.skyline_count {
                        (hit.depth * pixel_angle * pixel_error.clamp(1., 7.)
                            / (0.75 + 0.45 * hit.grade.min(4.)))
                        .clamp(1., 40.)
                    } else if blocks {
                        let pixels = pixel_error.clamp(2., 9.);
                        (hit.depth * pixel_angle * pixels / (0.8 + 0.55 * hit.grade.min(4.)))
                            .clamp(0.5, 40.)
                    } else {
                        (hit.depth * 0.012).clamp(4., 48.)
                    };
                    let debt = &mut debt[usize::from(i >= self.skyline_count)];
                    *debt = debt.max(step / limit);
                }
            }
            if debt[0] > 1. && skyline.is_none_or(|(d, k)| debt[0] > d || (debt[0] == d && key < k))
            {
                skyline = Some((debt[0], key));
            }
            if debt[1] > 1.
                && foreground.is_none_or(|(d, k)| debt[1] > d || (debt[1] == d && key < k))
            {
                foreground = Some((debt[1], key));
            }
            let worst = debt[0].max(debt[1]);
            if worst > 1. {
                priorities.push((worst, key));
            }
        }
        if priorities.is_empty() {
            return None;
        }
        priorities.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let mut selected = HashSet::new();
        if blocks && budget >= 2 {
            if let Some((_, key)) = skyline {
                selected.insert(key);
            }
            // A nearby cliff may occupy half the screen while two distinct
            // ridges compete for the skyline. Give the cliff a turn when its
            // error is of the same order as the worst projected debt.
            if let Some((debt, key)) = foreground {
                if debt >= priorities[0].0 * 0.35 {
                    selected.insert(key);
                }
            }
        }
        for (_, key) in priorities {
            if selected.len() >= budget.min(2) {
                break;
            }
            selected.insert(key);
        }
        Some(selected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::FbmParams;
    #[test]
    fn cold_flight_splits_only_surveyed_owners_and_preserves_the_full_partition() {
        let root = Node {
            min: [-65536, -65536],
            width: 65536,
            step: 2048,
        };
        let other = Node {
            min: [0, -65536],
            ..root
        };
        let focus = Focus {
            hits: vec![Hit {
                column: [-2000, -8000],
                depth: 800.,
                grade: 1.,
            }],
            skyline_count: 1,
            ..Default::default()
        };
        for (limit, expected) in [(2, 2), (5, 5), (32, 8)] {
            let mut nodes = vec![root, other];
            focus.split_coarse_crests(&mut nodes, root, 0.001, limit);
            assert_eq!(nodes.len(), expected);
            assert!(nodes.contains(&other));
            assert_eq!(
                nodes.iter().map(|n| u64::from(n.width).pow(2)).sum::<u64>(),
                2 * u64::from(root.width).pow(2)
            );
            for x in 0..16 {
                for z in 0..8 {
                    let point = [-65536 + 4096 + x * 8192, -65536 + 4096 + z * 8192];
                    assert_eq!(
                        nodes
                            .iter()
                            .filter(|n| (0..2)
                                .all(|a| (n.min[a]..n.min[a] + i64::from(n.width))
                                    .contains(&point[a])))
                            .count(),
                        1
                    );
                }
            }
        }
    }

    #[test]
    fn skyline_survey_keeps_competing_ridges_in_separate_tiles() {
        let mut probes = Vec::new();
        let sample =
            |r: f64| (0.30 - ((r - 640.) / 900.).powi(2)).max(0.294 - ((r - 1720.) / 830.).powi(2));
        let [Some(a), Some(b)] = skyline_crests(3200., true, 0.01, &mut probes, sample) else {
            panic!("expected both crests");
        };
        assert!((a - 640.).abs() < 16. && (b - 1720.).abs() < 16.);
        let [Some(a), None] = skyline_crests(3200., true, 0.001, &mut probes, sample) else {
            panic!("expected the higher crest only");
        };
        assert!((a - 640.).abs() < 16.);
    }
    #[test]
    fn survey_prioritizes_visible_approach_and_releases_the_scheduler_on_foot() {
        let world = World::fbm(
            7,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(0., 250., 0.);
        camera.yaw = 0.;
        camera.pitch = -0.35;
        camera.roll = 0.;
        camera.far = 6000.;
        let key = TileKey {
            min: [-32768, 0],
            width_cells: 65536,
            step_cells: 2048,
            edge_steps: [2048; 4],
            source_revision: 0,
        };
        let other = TileKey {
            min: [-32768, -65536],
            ..key
        };
        let candidates = [other, key].map(|key| RefinementCandidate {
            key,
            distance_squared: 0.,
            error_px: f64::INFINITY,
        });
        let mut focus = Focus::default();
        assert_eq!(
            focus.select(&world, &camera, [1600, 1000], 5., &candidates, 2),
            Some(HashSet::from([key]))
        );
        camera.yaw = std::f32::consts::PI;
        assert_eq!(
            focus.select(&world, &camera, [1600, 1000], 5., &candidates, 2),
            Some(HashSet::from([other]))
        );
        camera.position.y = 2.;
        assert_eq!(
            focus.select(&world, &camera, [1600, 1000], 5., &candidates, 2),
            None
        );
    }
}
