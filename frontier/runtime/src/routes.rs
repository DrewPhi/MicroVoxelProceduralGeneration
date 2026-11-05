//! Deterministic camera routes for repeatable renderer benchmarks.

/// Camera pose in world-space meters and radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub position: [f64; 3],
    pub yaw: f64,
    pub pitch: f64,
}

/// Fixed benchmark paths. Sampling is based on absolute elapsed seconds, so
/// results do not depend on frame rate or the sequence of time steps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Route {
    Stationary,
    Fly,
    StopResume,
    TurnReturn,
}

impl Route {
    /// Sample the route from an initial camera pose at an explicit travel speed
    /// in meters per second. Invalid/negative time or speed is treated as zero.
    ///
    /// Flight follows the initial camera look direction, including pitch.
    /// `StopResume` flies for 2 seconds, waits in place from t=2 through t=4,
    /// then resumes at the same speed. `TurnReturn` flies for 2 seconds, turns
    /// 180° in place over 1 second, then flies back for 2 seconds.
    pub fn pose_at(
        self,
        seconds: f64,
        initial_position: [f64; 3],
        initial_yaw: f64,
        initial_pitch: f64,
        speed: f64,
    ) -> Pose {
        let t = if seconds.is_finite() {
            seconds.max(0.0)
        } else {
            0.0
        };
        let speed = if speed.is_finite() {
            speed.max(0.0)
        } else {
            0.0
        };
        let position = initial_position.map(|v| if v.is_finite() { v } else { 0.0 });
        let yaw = if initial_yaw.is_finite() {
            initial_yaw
        } else {
            0.0
        };
        let pitch = if initial_pitch.is_finite() {
            initial_pitch.clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2)
        } else {
            0.0
        };
        let mut pose = Pose {
            position,
            yaw,
            pitch,
        };
        let forward = [
            yaw.sin() * pitch.cos(),
            pitch.sin(),
            yaw.cos() * pitch.cos(),
        ];
        let distance = match self {
            Route::Stationary => 0.0,
            Route::Fly => speed * t,
            Route::StopResume => speed * (t - (t - 2.0).clamp(0.0, 2.0)),
            Route::TurnReturn => speed * t.min(2.0) - speed * (t - 3.0).clamp(0.0, 2.0),
        };
        let distance = if distance.is_finite() { distance } else { 0.0 };
        for (axis, component) in pose.position.iter_mut().zip(forward) {
            let moved = distance * component;
            *axis = if moved.is_finite() && (*axis + moved).is_finite() {
                *axis + moved
            } else {
                *axis
            };
        }
        match self {
            Route::TurnReturn if t >= 2.0 && t < 3.0 => {
                pose.yaw += std::f64::consts::PI * (t - 2.0);
            }
            Route::TurnReturn if t >= 3.0 => pose.yaw += std::f64::consts::PI,
            _ => {}
        }
        pose
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_are_deterministic_and_frame_step_independent() {
        let p = Route::Fly.pose_at(3.5, [1.0, 2.0, 3.0], 0.0, 0.0, 4.0);
        assert_eq!(p, Route::Fly.pose_at(3.5, [1.0, 2.0, 3.0], 0.0, 0.0, 4.0));
        assert_eq!(p.position, [1.0, 2.0, 17.0]);
    }

    #[test]
    fn stop_resume_has_stationary_interval_and_resumes_continuously() {
        let start = Route::StopResume.pose_at(2.0, [0.0; 3], 0.0, 0.0, 10.0);
        let stop = Route::StopResume.pose_at(3.0, [0.0; 3], 0.0, 0.0, 10.0);
        let resumed = Route::StopResume.pose_at(5.0, [0.0; 3], 0.0, 0.0, 10.0);
        assert_eq!(start.position, [0.0, 0.0, 20.0]);
        assert_eq!(stop.position, start.position);
        assert_eq!(resumed.position, [0.0, 0.0, 30.0]);
        assert_eq!(
            Route::StopResume
                .pose_at(4.0, [0.0; 3], 0.0, 0.0, 10.0)
                .position,
            start.position
        );
    }

    #[test]
    fn turn_return_goes_out_turns_and_returns() {
        let turned = Route::TurnReturn.pose_at(3.0, [0.0; 3], 0.0, 0.0, 5.0);
        let returned = Route::TurnReturn.pose_at(5.0, [0.0; 3], 0.0, 0.0, 5.0);
        assert_eq!(turned.position, [0.0, 0.0, 10.0]);
        assert_eq!(turned.yaw, std::f64::consts::PI);
        assert!((returned.position[2]).abs() < 1e-12);
        assert_eq!(returned.yaw, std::f64::consts::PI);
    }

    #[test]
    fn times_and_speeds_are_clamped_and_outputs_stay_finite() {
        let origin = Route::StopResume.pose_at(-1.0, [1.0, 2.0, 3.0], 0.2, 0.3, 10.0);
        assert_eq!(origin.position, [1.0, 2.0, 3.0]);
        let invalid = Route::Fly.pose_at(
            f64::NAN,
            [f64::INFINITY, 0.0, 0.0],
            f64::NAN,
            0.0,
            f64::INFINITY,
        );
        assert!(invalid.position.iter().all(|v| v.is_finite()));
        assert!(invalid.yaw.is_finite() && invalid.pitch.is_finite());
    }
}
