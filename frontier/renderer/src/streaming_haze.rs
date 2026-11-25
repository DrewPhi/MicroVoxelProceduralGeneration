//! Only current published terrain with a refinement in flight can acquire haze.
use crate::adaptive_geometry::TileKey;
use crate::adaptive_gpu::VisibleOffender;
use glam::DVec3;

pub(crate) const REGIONS: usize = 8;

struct Region {
    key: TileKey,
    center: [f64; 2],
    half_size: f32,
    amount: f32,
    target: f32,
}

#[derive(Default)]
pub(crate) struct StreamingHaze {
    regions: Vec<Region>,
    last_frame: Option<(DVec3, f32)>,
}

impl StreamingHaze {
    pub(crate) fn prepare(
        &mut self,
        issues: Option<(&[Option<VisibleOffender>], f64)>,
        eye: DVec3,
        time: f32,
    ) -> ([[f32; 4]; REGIONS], usize) {
        let dt = self.last_frame.map_or(0., |(old_eye, old_time)| {
            if old_eye.distance(eye) > 120. || time < old_time {
                self.regions.clear();
                0.
            } else {
                (time - old_time).min(0.1)
            }
        });
        self.last_frame = Some((eye, time));
        for region in &mut self.regions {
            region.target = 0.;
        }
        if let Some((offenders, pixel_error)) = issues {
            for issue in offenders.iter().flatten() {
                let bounds = issue.proxy_bounds;
                let center = [
                    (bounds[0][0] + bounds[1][0]) * 0.5,
                    (bounds[0][2] + bounds[1][2]) * 0.5,
                ];
                let width = (bounds[1][0] - bounds[0][0]).max(bounds[1][2] - bounds[0][2]);
                if !(8. ..=256.).contains(&width) {
                    continue;
                }
                let excess = issue.projected_error_px.map_or(0.5, |error| {
                    ((error / pixel_error.max(0.1) - 1.) / 4.).clamp(0., 1.) as f32
                });
                let terrain_distance = (center[0] - eye.x).hypot(center[1] - eye.z);
                if terrain_distance < 24. || terrain_distance > 550. || excess <= 0. {
                    continue;
                }
                if let Some(region) = self.regions.iter_mut().find(|r| r.key == issue.key) {
                    region.target = excess;
                } else if self.regions.len() < REGIONS {
                    self.regions.push(Region {
                        key: issue.key,
                        center,
                        half_size: (width * 0.5) as f32,
                        amount: 0.,
                        target: excess,
                    });
                }
            }
        }
        for region in &mut self.regions {
            let seconds = if region.target > region.amount {
                0.38
            } else {
                0.25
            };
            region.amount += (region.target - region.amount) * (1. - (-dt / seconds).exp());
        }
        self.regions.retain(|r| r.target > 0. || r.amount > 0.005);
        let count = self.regions.len();
        (
            std::array::from_fn(|i| {
                self.regions.get(i).map_or([0.; 4], |region| {
                    [
                        (region.center[0] - eye.x) as f32,
                        (region.center[1] - eye.z) as f32,
                        region.half_size,
                        region.amount,
                    ]
                })
            }),
            count,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_published_errors_fade_in_and_clear_with_refinement() {
        let tile = VisibleOffender {
            key: TileKey {
                min: [0, 960],
                width_cells: 512,
                step_cells: 8,
                edge_steps: [8; 4],
                source_revision: 1,
            },
            proxy_bounds: [[-8., 0., 60.], [24., 23., 92.]],
            canonical_bounds: [[-8., 0., 60.], [24., 23., 92.]],
            cached_vertical_error_m: Some(2.),
            water_feature_error_m: 0.,
            projected_error_px: Some(24.),
        };
        let issues = [Some(tile)];
        let mut haze = StreamingHaze::default();
        assert_eq!(haze.prepare(None, DVec3::ZERO, 0.).1, 0);
        let (first, count) = haze.prepare(Some((&issues, 6.)), DVec3::ZERO, 0.1);
        assert_eq!(count, 1);
        assert!(first[0][3] > 0. && first[0][3] < 1.);
        let (finishing, count) = haze.prepare(None, DVec3::new(3., 0., 0.), 0.2);
        assert_eq!(count, 1);
        assert_eq!(finishing[0][0], first[0][0] - 3.);
        assert!(finishing[0][3] < first[0][3]);
        for i in 3..24 {
            haze.prepare(None, DVec3::ZERO, i as f32 * 0.1);
        }
        assert_eq!(haze.prepare(None, DVec3::ZERO, 2.4).1, 0);

        haze.prepare(Some((&issues, 6.)), DVec3::ZERO, 2.5);
        assert_eq!(haze.prepare(None, DVec3::new(0., 0., 600.), 2.6).1, 0);
    }
}
