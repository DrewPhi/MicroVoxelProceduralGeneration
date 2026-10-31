//! Physical-space streaming margins, independent of native voxel count.
pub fn prediction(
    eye: [f64; 3],
    velocity: [f64; 3],
    direction: [f64; 3],
    tile_edge: f64,
    radius: f64,
) -> [f64; 3] {
    let speed = velocity[0].hypot(velocity[2]);
    let heading = if speed > 0.2 {
        [velocity[0] / speed, velocity[2] / speed]
    } else {
        let length = direction[0].hypot(direction[2]).max(0.001);
        [direction[0] / length, direction[2] / length]
    };
    let lead = if speed > 0.2 {
        (speed + tile_edge)
            .min(radius.max(tile_edge) * 0.75)
            .min(12.)
    } else {
        (tile_edge * 1.5).min(radius * 0.5)
    };
    [
        eye[0] + heading[0] * lead,
        eye[1],
        eye[2] + heading[1] * lead,
    ]
}
pub fn warm_budget(memory_mib: usize) -> usize {
    (memory_mib * 1048576 / 32).clamp(1048576, 8 * 1048576)
}
pub fn transition_seconds(tile_edge: f64, speed: f64) -> f32 {
    (tile_edge / speed.max(0.5) * 0.3).clamp(0.025, 0.16) as f32
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forecast_uses_travel_before_camera_and_is_physically_bounded() {
        let p = prediction([10., 20., 30.], [5., 0., 0.], [-1., 0., 0.], 2., 6.);
        assert!(p[0] > 10. && p[0] <= 14.5);
        assert_eq!(p[1], 20.);
        assert_eq!(p[2], 30.);
        let idle = prediction([0.; 3], [0.; 3], [0., 0., 1.], 0.64, 8.);
        assert!(idle[2] > 0. && idle[2] < 1.);
        assert_eq!(prediction([0.; 3], [0.; 3], [0., 1., 0.], 2., 6.), [0.; 3]);
    }
    #[test]
    fn warm_memory_and_fades_remain_bounded_at_tiny_resolutions() {
        assert_eq!(warm_budget(256), 8 * 1048576);
        assert!(warm_budget(96) < warm_budget(256));
        assert_eq!(transition_seconds(2., 0.), 0.16);
        assert!(transition_seconds(0.64, 5.8) < 0.04);
        assert_eq!(transition_seconds(0.032, 100.), 0.025);
    }
}
