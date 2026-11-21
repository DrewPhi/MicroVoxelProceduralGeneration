//! Scheduling hints from the terrain a pilot is looking at. Full source bounds,
//! edge closure and ownership checks remain authoritative at publication.
use super::{Camera, RefinementCandidate, TileKey};
use frontier_world::{voxel_size, World};
use glam::DVec3;
use std::collections::HashSet;

#[derive(Default)]
pub(super) struct Focus {
    view: Option<(Camera, f32)>,
    hits: Vec<([i64; 2], f64)>,
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
            let mut best_slope = f64::NEG_INFINITY;
            let mut best = None;
            let mut distance = 64.;
            while distance <= far {
                let x = eye.x + distance * d.x / length;
                let z = eye.z + distance * d.z / length;
                let h = world.height_m(x, z);
                let slope = (h - eye.y) / distance;
                if slope > best_slope {
                    best_slope = slope;
                    best = Some(([x, z], distance));
                }
                distance += (distance * 0.045).clamp(32., 96.);
            }
            if let Some((p, range)) = best {
                self.hits
                    .push((p.map(|x| (x / voxel_size()).floor() as i64), range));
            }
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
                    self.hits
                        .push(([p.x, p.z].map(|x| (x / voxel_size()).floor() as i64), hi));
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
        aspect: f32,
        candidates: &[RefinementCandidate],
        budget: usize,
    ) -> Option<HashSet<TileKey>> {
        if candidates.is_empty() || budget == 0 {
            return None;
        }
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
            for (i, (p, range)) in self.hits.iter().enumerate() {
                if (0..2).all(|a| p[a] >= key.min[a] && p[a] < key.min[a] + side) {
                    let limit = if blocks {
                        (range * 0.006).clamp(2., 32.)
                    } else {
                        (range * 0.012).clamp(4., 48.)
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
            focus.select(&world, &camera, 1.6, &candidates, 2),
            Some(HashSet::from([key]))
        );
        camera.yaw = std::f32::consts::PI;
        assert_eq!(
            focus.select(&world, &camera, 1.6, &candidates, 2),
            Some(HashSet::from([other]))
        );
        camera.position.y = 2.;
        assert_eq!(focus.select(&world, &camera, 1.6, &candidates, 2), None);
    }
}
