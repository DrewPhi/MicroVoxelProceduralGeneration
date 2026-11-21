//! Scheduling hints from the terrain a pilot is looking at. Full source bounds,
//! edge closure and ownership checks remain authoritative at publication.
use super::{Camera, RefinementCandidate, TileKey};
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
    view: Option<(Camera, f32)>,
    hits: Vec<Hit>,
    skyline_count: usize,
}

impl Focus {
    fn survey(&mut self, world: &World, camera: &Camera, aspect: f32) {
        self.view = Some((*camera, aspect));
        self.hits.clear();
        self.skyline_count = 0;
        let eye = camera.position;
        if eye.y - world.height_m(eye.x, eye.z) < 45. {
            return;
        }
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
        let half_samples = if blocks { 8 } else { 5 };
        for i in -half_samples..=half_samples {
            let across =
                f64::from(i) / f64::from(half_samples) * 0.95 * f64::from(aspect) * half_height;
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
            let mut range = 64_f64.min(end);
            let mut best = range;
            let mut best_slope = f64::NEG_INFINITY;
            loop {
                let (slope, _, _) = terrain(range);
                if slope > best_slope {
                    best_slope = slope;
                    best = range;
                }
                if range >= end {
                    break;
                }
                range = (range + (range * 0.045).clamp(32., 96.)).min(end);
            }
            if blocks {
                // Refine around the angular maximum. A narrow crest between
                // the coarse probes can belong to the neighboring terrain tile.
                let span = (best * 0.045).clamp(32., 96.);
                let center = best;
                for tap in [-3, -2, -1, 1, 2, 3] {
                    let distance = (center + f64::from(tap) * span * 0.25).clamp(1., end);
                    let (slope, _, _) = terrain(distance);
                    if slope > best_slope {
                        best_slope = slope;
                        best = distance;
                    }
                }
            }
            let (_, p, h) = terrain(best);
            let relative = DVec3::new(p.x - eye.x, h - eye.y, p.y - eye.z);
            let mut grade = 1.;
            if blocks {
                // A coarse column on a cliff can shift its visible height by
                // several times its horizontal sampling error.
                let radius = (best * 0.008).clamp(6., 32.);
                grade = [DVec2::X, -DVec2::X, DVec2::Y, -DVec2::Y]
                    .into_iter()
                    .map(|axis| {
                        let q = p + axis * radius;
                        (world.height_m(q.x, q.y) - h).abs() / radius
                    })
                    .fold(0.0, f64::max);
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
        self.skyline_count = self.hits.len();
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
        ];
        for &(h, v) in base.iter().chain(foreground.iter().filter(|_| blocks)) {
            let direction =
                (forward + right * (h * f64::from(aspect) * half_height) + up * (v * half_height))
                    .normalize();
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
                        depth: hi,
                        grade: 1.,
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
        let aspect = viewport[0] as f32 / viewport[1].max(1) as f32;
        let pixel_angle =
            2. * (f64::from(camera.fov_y).to_radians() * 0.5).tan() / f64::from(viewport[1].max(1));
        if self.view.is_none_or(|(old, a)| {
            a != aspect
                || old.fov_y != camera.fov_y
                || old.far != camera.far
                || old.position.distance_squared(camera.position) > 48. * 48.
                || old.direction().dot(camera.direction()) < 0.997
                || (old.roll - camera.roll).abs() > 0.08
        }) {
            self.survey(world, camera, aspect);
        }
        let blocks = crate::adaptive_geometry::distant_voxels::Mode::current()
            == crate::adaptive_geometry::distant_voxels::Mode::Blocks;
        let mut priorities = Vec::new();
        let mut skyline = None;
        for c in candidates {
            let key = c.key;
            let side = i64::from(key.width_cells);
            let step = f64::from(key.step_cells) * voxel_size();
            let mut debt = [0_f64; 2];
            for (i, hit) in self.hits.iter().enumerate() {
                if (0..2).all(|a| hit.column[a] >= key.min[a] && hit.column[a] < key.min[a] + side)
                {
                    let limit = if blocks && i < self.skyline_count {
                        (hit.depth * pixel_angle * pixel_error / (0.75 + 0.45 * hit.grade.min(4.)))
                            .clamp(1., 40.)
                    } else if blocks {
                        (hit.depth * 0.006).clamp(2., 32.)
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
        }
        for (_, key) in priorities {
            selected.insert(key);
            if selected.len() >= budget.min(2) {
                break;
            }
        }
        Some(selected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::FbmParams;
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
