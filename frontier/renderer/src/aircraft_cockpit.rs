//! The small hardware on the panel uses an eighth of the airframe's cell pitch.
use super::{joints, turn, voxels::Voxels, Part, Rig, BLACK, METAL};
use crate::{aircraft, Camera};
use glam::{IVec3, Mat4, Vec3};

pub(super) const EDGE: f32 = 0.0078125;
pub(super) const THROTTLE_TRAVEL: f32 = 0.069;
pub(super) const MIXTURE_TRAVEL: f32 = 0.048;
const YOKE_HUB: Vec3 = Vec3::new(0.28125, -0.230, 0.5625);
const YOKE_TRAVEL: f32 = 0.092;
const YOKE_ROLL: f32 = 0.60;
const PEDAL_TRAVEL: f32 = 0.045;
const MAT_TOP: f32 = -0.375 + EDGE;
const THROTTLE: Vec3 = Vec3::new(0.063, -0.372, 0.789);
const MIXTURE: Vec3 = Vec3::new(-0.051, -0.372, 0.789);
const IGNITION: Vec3 = Vec3::new(0.352, -0.220, 0.849);
const AVIONICS: Vec3 = Vec3::new(0.493, -0.224, 0.849);
const FLAPS: Vec3 = Vec3::new(-0.377, -0.224, 0.849);
const DARK: [f32; 3] = [0.016, 0.022, 0.027];
const LIGHT: [f32; 3] = [0.66, 0.72, 0.70];
const RED: [f32; 3] = [0.66, 0.046, 0.031];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Control {
    Throttle,
    LeftPower,
    RightPower,
    Gear,
    Mixture,
    Ignition,
    Avionics,
    Flaps,
}

impl Control {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::LeftPower => "Left power · drag",
            Self::RightPower => "Right power · drag",
            Self::Gear => "Landing gear · click",
            Self::Throttle => "Throttle · drag",
            Self::Mixture => "Mixture · drag",
            Self::Ignition => "Ignition · click",
            Self::Avionics => "Avionics · click",
            Self::Flaps => "Flap selector · click",
        }
    }
}

fn paint(cells: &mut Voxels, lo: Vec3, hi: Vec3, color: [f32; 3], inside: impl Fn(Vec3) -> bool) {
    let lo = (lo / EDGE).floor().as_ivec3();
    let hi = (hi / EDGE).ceil().as_ivec3();
    for z in lo.z..hi.z {
        for y in lo.y..hi.y {
            for x in lo.x..hi.x {
                let cell = IVec3::new(x, y, z);
                if inside((cell.as_vec3() + Vec3::splat(0.5)) * EDGE) {
                    cells.insert(cell, color);
                }
            }
        }
    }
}

fn block(cells: &mut Voxels, lo: Vec3, hi: Vec3, color: [f32; 3]) {
    paint(cells, lo, hi, color, |p| {
        p.cmpge(lo).all() && p.cmplt(hi).all()
    });
}

fn drum(cells: &mut Voxels, center: Vec3, radius: f32, depth: f32, color: [f32; 3]) {
    let half = Vec3::new(radius, radius, depth * 0.5);
    paint(cells, center - half, center + half, color, |p| {
        let d = p - center;
        d.truncate().length_squared() <= radius * radius && d.z.abs() <= half.z
    });
}

fn stem(cells: &mut Voxels, a: Vec3, b: Vec3, radius: f32, color: [f32; 3]) {
    let edge = b - a;
    paint(cells, a.min(b) - radius, a.max(b) + radius, color, |p| {
        let t = ((p - a).dot(edge) / edge.length_squared()).clamp(0., 1.);
        p.distance_squared(a + t * edge) <= radius * radius
    });
}

fn push_pull(cells: &mut Voxels, center: Vec3, radius: f32, color: [f32; 3], cap: [f32; 3]) {
    // Keep the far end inside the dash at both ends of travel.
    drum(cells, center + Vec3::Z * 0.084, 0.0088, 0.167, METAL);
    drum(cells, center + Vec3::Z * 0.012, radius * 0.72, 0.020, color);
    drum(cells, center, radius, 0.023, color);
    drum(cells, center - Vec3::Z * 0.012, radius * 0.77, 0.008, cap);
}

pub(super) fn model() -> [Voxels; Part::COUNT] {
    let mut parts = std::array::from_fn(|_| Voxels::new());
    let fascia = &mut parts[Part::Cabin as usize];
    for center in [THROTTLE, MIXTURE] {
        drum(fascia, center.with_z(0.867), 0.024, 0.022, METAL);
        drum(fascia, center.with_z(0.853), 0.017, 0.010, BLACK);
    }
    drum(fascia, IGNITION.with_z(0.862), 0.036, 0.038, DARK);
    drum(fascia, IGNITION - Vec3::Z * 0.012, 0.025, 0.009, METAL);
    for x in [-0.047, 0.047] {
        drum(
            fascia,
            IGNITION + Vec3::new(x, 0., -0.003),
            0.006,
            0.018,
            LIGHT,
        );
    }
    for pos in [AVIONICS, FLAPS] {
        block(
            fascia,
            pos - Vec3::new(0.044, 0.046, 0.008),
            pos + Vec3::new(0.044, 0.046, 0.030),
            DARK,
        );
    }
    for i in 0..4 {
        let a = FLAPS + Vec3::new(0.027, 0.025 - i as f32 * 0.016, -0.011);
        block(fascia, a, a + Vec3::new(0.013, EDGE, EDGE), METAL);
    }
    // Breaker caps along the lower panel rail.
    for side in [-1., 1.] {
        let x = side * YOKE_HUB.x;
        block(
            fascia,
            Vec3::new(x - 0.172, -0.375, -0.08),
            Vec3::new(x + 0.172, MAT_TOP, 0.69),
            [0.052, 0.060, 0.064],
        );
        let collar = Vec3::new(x, YOKE_HUB.y, 0.874);
        drum(fascia, collar, 0.033, 0.021, DARK);
        drum(fascia, collar - Vec3::Z * 0.014, 0.027, 0.012, METAL);
        for i in 0..7 {
            let x = side * (0.206 + 0.038 * i as f32);
            drum(fascia, Vec3::new(x, -0.343, 0.870), 0.0105, 0.016, METAL);
            drum(fascia, Vec3::new(x, -0.343, 0.859), 0.008, 0.008, DARK);
        }
    }

    push_pull(
        &mut parts[Part::Throttle as usize],
        THROTTLE,
        0.031,
        DARK,
        BLACK,
    );
    push_pull(
        &mut parts[Part::Mixture as usize],
        MIXTURE,
        0.026,
        RED,
        [0.82, 0.081, 0.045],
    );
    let key = &mut parts[Part::Ignition as usize];
    drum(key, IGNITION - Vec3::Z * 0.021, 0.012, 0.027, METAL);
    block(
        key,
        IGNITION + Vec3::new(-0.009, -0.025, -0.044),
        IGNITION + Vec3::new(0.009, 0.025, -0.026),
        DARK,
    );
    block(
        key,
        IGNITION + Vec3::new(-0.009, 0.012, -0.051),
        IGNITION + Vec3::new(0.009, 0.023, -0.044),
        LIGHT,
    );

    let rocker = &mut parts[Part::Avionics as usize];
    block(
        rocker,
        AVIONICS - Vec3::new(0.022, 0.033, 0.029),
        AVIONICS + Vec3::new(0.022, 0.033, 0.006),
        [0.53, 0.09, 0.046],
    );
    block(
        rocker,
        AVIONICS + Vec3::new(-0.013, 0.015, -0.036),
        AVIONICS + Vec3::new(0.013, 0.023, -0.028),
        LIGHT,
    );

    let flap = &mut parts[Part::FlapSelector as usize];
    stem(
        flap,
        FLAPS,
        FLAPS + Vec3::new(0., 0.024, -0.045),
        0.008,
        METAL,
    );
    block(
        flap,
        FLAPS + Vec3::new(-0.024, 0.016, -0.068),
        FLAPS + Vec3::new(0.024, 0.035, -0.047),
        LIGHT,
    );

    for seat in [-1., 1.] {
        for (part, offset) in [(Part::LeftPedal, 0.082), (Part::RightPedal, -0.082)] {
            let pedal = &mut parts[part as usize];
            let foot = Vec3::new(seat * YOKE_HUB.x + offset, -0.368, 0.785);
            stem(
                pedal,
                foot,
                foot + Vec3::new(0., 0.044, -0.018),
                0.008,
                METAL,
            );
            let pad = foot + Vec3::new(0., 0.046, -0.021);
            block(
                pedal,
                pad - Vec3::new(0.034, 0.021, 0.014),
                pad + Vec3::new(0.034, 0.025, 0.009),
                DARK,
            );
            for y in [-0.012, 0.002, 0.016] {
                let lo = pad + Vec3::new(-0.024, y, -0.020);
                block(pedal, lo, lo + Vec3::new(0.048, EDGE, EDGE), METAL);
            }
        }
    }
    for (part, sign) in [(Part::Yoke, 1.), (Part::CopilotYoke, -1.)] {
        let yoke = &mut parts[part as usize];
        let hub = YOKE_HUB.with_x(YOKE_HUB.x * sign);
        // The rod continues into its panel collar at either pitch limit.
        drum(yoke, hub + Vec3::Z * 0.212, 0.019, 0.438, METAL);
        drum(yoke, hub + Vec3::Z * 0.046, 0.034, 0.080, DARK);
        block(
            yoke,
            hub - Vec3::new(0.052, 0.036, 0.037),
            hub + Vec3::new(0.052, 0.029, 0.027),
            BLACK,
        );
        for side in [-1., 1.] {
            let root = hub + Vec3::new(side * 0.03, -0.016, -0.012);
            let bend = hub + Vec3::new(side * 0.118, -0.029, -0.018);
            let grip = hub + Vec3::new(side * 0.143, 0.023, -0.025);
            let tip = hub + Vec3::new(side * 0.136, 0.085, -0.019);
            stem(yoke, root, bend, 0.022, DARK);
            stem(yoke, bend, grip, 0.021, BLACK);
            stem(yoke, grip, tip, 0.023, BLACK);
            drum(
                yoke,
                tip - Vec3::Z * 0.021,
                0.009,
                0.008,
                if side > 0. { RED } else { METAL },
            );
        }
        block(
            yoke,
            hub + Vec3::new(-0.022, -0.007, -0.045),
            hub + Vec3::new(0.022, 0.006, -0.037),
            METAL,
        );
    }
    parts
}

pub(super) fn rig(joints: &mut [Mat4; Part::COUNT], pose: aircraft::Presentation) {
    for (part, side) in [(Part::Yoke, 1.), (Part::CopilotYoke, -1.)] {
        joints[part as usize] = yoke_transform(side, pose.controls.x, pose.controls.y);
    }
    for (part, side) in [(Part::LeftPedal, -1.), (Part::RightPedal, 1.)] {
        joints[part as usize] =
            Mat4::from_translation(Vec3::Z * (PEDAL_TRAVEL * pose.rudder * side));
    }
    joints[Part::Throttle as usize] =
        Mat4::from_translation(Vec3::NEG_Z * (1. - pose.controls.z) * THROTTLE_TRAVEL);
    joints[Part::Mixture as usize] =
        Mat4::from_translation(Vec3::NEG_Z * (1. - pose.mixture) * MIXTURE_TRAVEL);
    joints[Part::Ignition as usize] = turn(
        IGNITION,
        Mat4::from_rotation_z(if pose.ignition { -0.52 } else { 0.76 }),
    );
    joints[Part::Avionics as usize] = turn(
        AVIONICS,
        Mat4::from_rotation_x(if pose.avionics { -0.27 } else { 0.27 }),
    );
    joints[Part::FlapSelector as usize] =
        turn(FLAPS, Mat4::from_rotation_x(-1.15 * pose.flap_selector));
}

fn yoke_transform(side: f32, bank: f32, pitch: f32) -> Mat4 {
    Mat4::from_translation(Vec3::NEG_Z * (YOKE_TRAVEL * pitch))
        * turn(
            YOKE_HUB.with_x(YOKE_HUB.x * side),
            Mat4::from_rotation_z(YOKE_ROLL * bank),
        )
}

/// Studio check for the exposed yokes, pedals and neighboring control knobs.
/// The shaft is intentionally inside its corresponding socket at the dash.
pub(super) fn clearance_json() -> String {
    let parts = model();
    let bounds = |part: Part| {
        parts[part as usize].keys().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(lo, hi), cell| {
                let corner = cell.as_vec3() * EDGE;
                (lo.min(corner), hi.max(corner + EDGE))
            },
        )
    };
    let (left_lo, left_hi) = bounds(Part::LeftPedal);
    let (right_lo, right_hi) = bounds(Part::RightPedal);
    let pedal_nearest = left_lo.z.min(right_lo.z) - PEDAL_TRAVEL;
    let pedal_highest = left_hi.y.max(right_hi.y);
    let knob_nearest = (bounds(Part::Throttle).0.z - THROTTLE_TRAVEL)
        .min(bounds(Part::Mixture).0.z - MIXTURE_TRAVEL);
    let camera = Camera {
        position: aircraft::PILOT_EYE.as_dvec3(),
        pitch: -aircraft::PILOT_VIEW_DOWN,
        yaw: 0.,
        roll: 0.,
        fov_y: 60.,
        far: 5.,
    };
    let projection = camera.matrix(glam::DVec3::ZERO, 4. / 3.);
    let mut floor = f32::INFINITY;
    let mut sidewall = f32::INFINITY;
    let mut exposed = f32::INFINITY;
    let mut frame = f32::INFINITY;
    for (part, side) in [(Part::Yoke, 1.), (Part::CopilotYoke, -1.)] {
        for bank_step in -20..=20 {
            for pitch in [-1., 1.] {
                let transform = yoke_transform(side, bank_step as f32 / 20., pitch);
                for cell in parts[part as usize]
                    .keys()
                    .filter(|p| p.z as f32 * EDGE < 0.875)
                {
                    for corner in 0..8 {
                        let local = (cell.as_vec3()
                            + IVec3::new(corner & 1, (corner >> 1) & 1, corner >> 2).as_vec3())
                            * EDGE;
                        let point = transform.transform_point3(local);
                        floor = floor.min(point.y - MAT_TOP);
                        sidewall = sidewall.min(0.5 - point.x.abs());
                        // Across the grips there is longitudinal clearance.
                        // Behind them, the steering shaft passes above the pedals.
                        exposed = exposed.min(if local.z <= YOKE_HUB.z + 0.03125 {
                            knob_nearest.min(pedal_nearest) - point.z
                        } else {
                            point.y - pedal_highest
                        });
                        let clip = projection * point.extend(1.);
                        frame = frame
                            .min(1. - (clip.truncate() / clip.w).abs().truncate().max_element());
                    }
                }
            }
        }
    }
    // Reserve half an angular sample to account for points between samples.
    let angular_margin = 0.2 * YOKE_ROLL / 40.;
    let pedal_dash = 0.875 - left_hi.z.max(right_hi.z) - PEDAL_TRAVEL;
    format!(
        r#"{{"floor_m":{},"side_trim_m":{},"pedals_and_knobs_m":{},"pedal_dash_m":{},"frame_margin_ndc_at_4_3":{}}}"#,
        floor - angular_margin,
        sidewall - angular_margin,
        exposed - angular_margin,
        pedal_dash,
        frame,
    )
}

pub(super) fn pick(
    camera: Camera,
    direction: Vec3,
    pose: aircraft::Presentation,
) -> Option<Control> {
    let inverse = aircraft::axes(pose.pose).transpose();
    let eye = inverse * (camera.position - pose.pose.position).as_vec3();
    let ray = inverse * direction;
    let transforms = joints(pose, Rig::default());
    [
        (Control::Throttle, Part::Throttle, THROTTLE, 0.037),
        (Control::Mixture, Part::Mixture, MIXTURE, 0.033),
        (
            Control::Ignition,
            Part::Ignition,
            IGNITION - Vec3::Z * 0.033,
            0.033,
        ),
        (
            Control::Avionics,
            Part::Avionics,
            AVIONICS - Vec3::Z * 0.022,
            0.037,
        ),
        (
            Control::Flaps,
            Part::FlapSelector,
            FLAPS + Vec3::new(0., 0.026, -0.055),
            0.032,
        ),
    ]
    .into_iter()
    .filter_map(|(control, part, origin, radius)| {
        let center = transforms[part as usize].transform_point3(origin) - eye;
        let along = center.dot(ray);
        let distance = (center - ray * along).length_squared();
        (along > 0. && distance <= radius * radius).then_some((control, along, distance))
    })
    .min_by(|a, b| a.2.total_cmp(&b.2).then(a.1.total_cmp(&b.1)))
    .map(|(control, _, _)| control)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_push_pull_controls_are_authored_on_the_fine_lattice() {
        let parts = model();
        for part in [
            Part::Throttle,
            Part::Mixture,
            Part::Avionics,
            Part::Ignition,
            Part::FlapSelector,
        ] {
            let cells = &parts[part as usize];
            assert!(cells.len() > 20);
            assert!(cells.keys().any(|p| p.x.rem_euclid(8) != 0));
        }
    }
}
