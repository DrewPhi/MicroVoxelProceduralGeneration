//! Original voxel DA62, dimensioned from Diamond's three-view and cabin drawings.
use super::{
    rectangle, solid_vertex, turn,
    voxels::{self, Voxels},
    Geometry, Pane, Part, Rig,
};
use crate::aircraft::{airframe::elevator_deflection, Presentation};
use glam::{IVec3, Mat4, Vec2, Vec3};
use std::sync::OnceLock;

const CELL: f32 = 0.0625;
const DETAIL: f32 = 0.0078125;
const UPHOLSTERY: f32 = CELL / 4.;
pub(crate) const SPAN: f32 = 7.275;
pub(crate) const ENGINE: f32 = 1.75;
pub(crate) const PROP: Vec3 = Vec3::new(ENGINE, -0.010, 1.42);
const SILVER: [f32; 3] = [0.69, 0.73, 0.76];
const PEARL: [f32; 3] = [0.84, 0.88, 0.86];
const BLUE: [f32; 3] = [0.043, 0.129, 0.225];
const NAVY: [f32; 3] = [0.020, 0.049, 0.090];
const TEAL: [f32; 3] = [0.11, 0.37, 0.48];
const SHADOW: [f32; 3] = [0.021, 0.026, 0.031];
const PANEL: [f32; 3] = [0.051, 0.068, 0.079];
const METAL: [f32; 3] = [0.40, 0.46, 0.48];
const LEATHER: [f32; 3] = [0.62, 0.52, 0.42];
const SEAM: [f32; 3] = [0.33, 0.27, 0.21];
const INSERT: [f32; 3] = [0.75, 0.65, 0.53];

pub(crate) const PILOT_EYE: Vec3 = Vec3::new(0.323, 0.49, -0.45);

type Parts = [Voxels; Part::COUNT];

fn sample(lo: Vec3, hi: Vec3, edge: f32, mut paint: impl FnMut(IVec3, Vec3)) {
    let a = (lo / edge).floor().as_ivec3();
    let b = (hi / edge).ceil().as_ivec3();
    for z in a.z..b.z {
        for y in a.y..b.y {
            for x in a.x..b.x {
                let cell = IVec3::new(x, y, z);
                paint(cell, (cell.as_vec3() + Vec3::splat(0.5)) * edge);
            }
        }
    }
}

fn block(cells: &mut Voxels, lo: Vec3, hi: Vec3, edge: f32, color: [f32; 3]) {
    sample(lo, hi, edge, |key, p| {
        if p.cmpge(lo).all() && p.cmplt(hi).all() {
            cells.insert(key, color);
        }
    });
}

fn rod(cells: &mut Voxels, a: Vec3, b: Vec3, radius: f32, color: [f32; 3]) {
    let axis = b - a;
    sample(a.min(b) - radius, a.max(b) + radius, DETAIL, |key, p| {
        let t = ((p - a).dot(axis) / axis.length_squared()).clamp(0., 1.);
        if p.distance_squared(a + t * axis) <= radius * radius {
            cells.insert(key, color);
        }
    });
}

fn section<const N: usize>(table: &[(f32, [f32; N])], at: f32) -> [f32; N] {
    let hi = table
        .partition_point(|&(x, _)| x < at)
        .clamp(1, table.len() - 1);
    let (a, low) = table[hi - 1];
    let (b, high) = table[hi];
    let u = ((at - a) / (b - a)).clamp(0., 1.);
    std::array::from_fn(|i| low[i] + (high[i] - low[i]) * u)
}

// Station, center height, half beam, half height. The canopy and aft fuselage
// share a continuous shell, including the windshield rake into the nose.
const HULL: &[(f32, [f32; 3])] = &[
    (-5.770, [-0.1460, 0.060, 0.0340]),
    (-5.350, [0.0875, 0.184, 0.3475]),
    (-4.870, [0.0900, 0.210, 0.4200]),
    (-4.410, [0.1300, 0.245, 0.4200]),
    (-4.040, [0.1275, 0.278, 0.3625]),
    (-3.450, [0.1250, 0.350, 0.2800]),
    (-3.100, [0.1250, 0.423, 0.2800]),
    (-2.770, [0.1125, 0.495, 0.3105]),
    (-2.150, [0.1565, 0.598, 0.4155]),
    (-1.580, [0.1760, 0.667, 0.5190]),
    (-0.970, [0.1950, 0.698, 0.6050]),
    (-0.430, [0.1940, 0.709, 0.6410]),
    (-0.030, [0.1785, 0.709, 0.6565]),
    (0.340, [0.1470, 0.695, 0.6530]),
    (0.690, [0.0800, 0.669, 0.6300]),
    (0.890, [0.0295, 0.653, 0.5795]),
    (1.150, [-0.0300, 0.628, 0.4600]),
    (1.320, [-0.0265, 0.602, 0.4315]),
    (1.670, [-0.0635, 0.543, 0.4075]),
    (2.050, [-0.0845, 0.465, 0.3645]),
    (2.350, [-0.1145, 0.392, 0.3105]),
    (2.570, [-0.1195, 0.323, 0.2545]),
    (2.970, [-0.1255, 0.179, 0.1345]),
    (3.160, [-0.1455, 0.099, 0.0715]),
    (3.280, [-0.1575, 0.028, 0.0185]),
];

pub(crate) fn hull(p: Vec3, inset: f32) -> bool {
    if !(HULL[0].0 + inset..HULL[HULL.len() - 1].0 - inset).contains(&p.z) {
        return false;
    }
    let [y, width, height] = section(HULL, p.z);
    let shape = Vec2::new(
        p.x / (width - inset).max(0.012),
        (p.y - y) / (height - inset).max(0.012),
    );
    shape.length_squared() <= 1.
}

fn hull_gradient(p: Vec3) -> Vec3 {
    let profile_distance = |z| {
        let [y, width, height] = section(HULL, z);
        (p.x / width).powi(2) + ((p.y - y) / height).powi(2)
    };
    let [y, width, height] = section(HULL, p.z);
    let dz = CELL * 0.5;
    Vec3::new(
        p.x / width.powi(2),
        (p.y - y) / height.powi(2),
        (profile_distance(p.z + dz) - profile_distance(p.z - dz)) / (4. * dz),
    )
    .normalize_or_zero()
}

fn hull_side(y: f32, z: f32) -> f32 {
    let [center, width, height] = section(HULL, z);
    width * (1. - ((y - center) / height).powi(2)).max(0.).sqrt()
}

const QUARTER_WINDOW: &[(f32, [f32; 2])] = &[
    (-1.936, [0.445, 0.465]),
    (-1.804, [0.341, 0.528]),
    (-1.542, [0.299, 0.590]),
    (-1.520, [0.320, 0.590]),
];
const PASSENGER_WINDOW: &[(f32, [f32; 2])] = &[
    (-1.236, [0.610, 0.631]),
    (-1.082, [0.341, 0.693]),
    (-0.907, [0.258, 0.714]),
    (-0.360, [0.196, 0.776]),
    (-0.295, [0.196, 0.465]),
];
const PILOT_WINDOW: &[(f32, [f32; 2])] = &[
    (-0.185, [0.714, 0.797]),
    (-0.098, [0.341, 0.797]),
    (0.012, [0.258, 0.797]),
    (0.449, [0.196, 0.756]),
    (0.559, [0.196, 0.548]),
    (0.624, [0.217, 0.258]),
];

fn side_window(p: Vec3, stations: &[(f32, [f32; 2])]) -> bool {
    if p.x.abs() < 0.20 || !(stations[0].0..stations[stations.len() - 1].0).contains(&p.z) {
        return false;
    }
    let [lo, hi] = section(stations, p.z);
    (lo..hi).contains(&p.y)
}

fn fuselage(solids: &mut Parts) -> [(Part, Voxels); 5] {
    // Groups follow independently hinged doors, the windscreen, and the
    // fixed aft windows. Each is sorted as one transparent draw.
    let mut glass = [
        Part::Body,
        Part::LeftDoor,
        Part::RightDoor,
        Part::RearDoor,
        Part::Body,
    ]
    .map(|part| (part, Voxels::new()));
    sample(
        Vec3::new(-0.75, -0.59, -5.94),
        Vec3::new(0.75, 0.89, 3.29),
        CELL,
        |cell, p| {
            if !hull(p, 0.) {
                return;
            }
            let occupied_cabin = (-3.05..1.57).contains(&p.z);
            // Keep every exterior boundary cell, including the pillars between panes.
            if occupied_cabin
                && [Vec3::X, Vec3::Y, Vec3::Z]
                    .iter()
                    .all(|axis| hull(p + *axis * CELL, 0.) && hull(p - *axis * CELL, 0.))
            {
                return;
            }
            let side = p.x.abs();
            let forward_door = (-0.26..0.61 + (0.68 - p.y).max(0.) * 0.32).contains(&p.z)
                && side > 0.24
                && p.y > -0.22;
            let rear_door = (-1.47..-0.27).contains(&p.z) && p.x > 0.23 && p.y > -0.27;
            let group = if forward_door {
                if p.x > 0. {
                    1
                } else {
                    2
                }
            } else if rear_door {
                3
            } else {
                0
            };
            let windscreen = (0.62 + (0.69 - p.y).max(0.) * 0.39..1.28).contains(&p.z)
                && p.y > 0.23 + (p.z - 0.76).max(0.) * 0.35;
            let front_side = side_window(p, PILOT_WINDOW);
            let passenger_side = side_window(p, PASSENGER_WINDOW);
            let quarter = side_window(p, QUARTER_WINDOW);
            if windscreen || front_side || passenger_side || quarter {
                glass[if group == 0 && !windscreen { 4 } else { group }]
                    .1
                    .insert(cell, SILVER);
                return;
            }

            let paint_line = -0.22 + 0.035 * (p.z - 0.45).max(0.) + 0.095 * (-p.z - 1.4).max(0.);
            let edge = p.y - paint_line;
            let tail_mark =
                (-4.55..-3.67).contains(&p.z) && (0.11..0.43).contains(&p.y) && side > 0.12;
            let mark_col = if p.x > 0. { -cell.z - 59 } else { cell.z + 72 };
            let mark_row = 6 - cell.y;
            // A small, two-digit fleet marking painted on the occupied tail cells.
            let digits = [0b1110111, 0b1000001, 0b1110111, 0b1010100, 0b1110111];
            let ink = tail_mark
                && (0..14).contains(&mark_col)
                && (0..5).contains(&mark_row)
                && digits[mark_row as usize] & (1 << (6 - mark_col / 2)) != 0;
            let color = if ink {
                PEARL
            } else if edge < -0.10 {
                SILVER
            } else if edge < -0.045 {
                PEARL
            } else if edge < 0.018 {
                TEAL
            } else if (p.y > 0.19 && occupied_cabin && side > 0.32)
                || (windscreen_frame(p) && p.y > 0.30)
            {
                NAVY
            } else {
                BLUE
            };
            solids[glass[group].0 as usize].insert(cell, color);
        },
    );
    glass
}

fn windscreen_frame(p: Vec3) -> bool {
    (0.44..1.65).contains(&p.z) && p.x.abs() > 0.50
}

// Spanwise station, leading edge Z, chord length, center height.
const WING: &[(f32, [f32; 3])] = &[
    (0., [0.90, 2.35, -0.205]),
    (0.63, [0.89, 2.34, -0.198]),
    (1.75, [0.74, 2.15, -0.157]),
    (3.75, [0.42, 1.72, -0.011]),
    (5.85, [0.04, 1.12, 0.17]),
    (6.88, [-0.16, 0.78, 0.285]),
    (7.12, [-0.29, 0.55, 0.412]),
    (SPAN, [-0.45, 0.32, 0.545]),
];
const TAIL: &[(f32, [f32; 3])] = &[
    (0., [-4.90, 1.04, 1.40]),
    (0.56, [-4.92, 1.00, 1.40]),
    (1.50, [-5.04, 0.73, 1.389]),
    (1.88, [-5.17, 0.50, 1.337]),
];

const FIN: &[(f32, [f32; 3])] = &[
    (-0.20, [-5.72, -5.76, 0.060]),
    (0.12, [-4.04, -5.79, 0.110]),
    (0.43, [-3.70, -5.82, 0.115]),
    (0.56, [-4.31, -5.84, 0.105]),
    (0.68, [-4.46, -5.85, 0.096]),
    (1.07, [-4.71, -5.88, 0.075]),
    (1.39, [-4.92, -5.91, 0.052]),
    (1.46, [-5.01, -5.91, 0.050]),
];

pub(crate) fn surface(tail: bool, u: f32) -> [f32; 3] {
    section(
        if tail { TAIL } else { WING },
        u * if tail { 1.88 } else { SPAN },
    )
}

pub(crate) fn fin(y: f32) -> [f32; 3] {
    section(FIN, y)
}

pub(crate) const FIN_ROOT: f32 = 0.43;
pub(crate) const FIN_HEIGHT: f32 = 0.99;
pub(crate) const TAIL_SPAN: f32 = 1.88;
pub(crate) const MAINS: [Vec3; 2] = [
    Vec3::new(-1.38, -1.359375, -0.165),
    Vec3::new(1.38, -1.359375, -0.165),
];
pub(crate) const TREAD_NOSE: Vec3 = Vec3::new(0., -1.359375, 1.72);

fn lift_surfaces(solids: &mut Parts, fine: &mut Parts) {
    // Molded tip lenses and the fin cap extend fractions of an airframe cell.
    // Build their shells on the hardware grid, overlapping the native skin.
    let caps = &mut fine[Part::Body as usize];
    for side in [-1., 1.] {
        let color = if side > 0. {
            [0.98, 0.038, 0.028]
        } else {
            [0.025, 0.78, 0.29]
        };
        let x = SPAN * side;
        sample(
            Vec3::new(x - 0.065, 0.485, -0.78),
            Vec3::new(x + 0.065, 0.586, -0.40),
            DETAIL,
            |cell, p| {
                let span = p.x.abs();
                if !(7.214..SPAN).contains(&span) {
                    return;
                }
                let [lead, chord, y] = section(WING, span);
                let u = (lead - p.z) / chord;
                if (0.0..1.).contains(&u)
                    && (p.y - y - 0.013).abs() < 0.012 + 0.037 * (4. * u * (1. - u)).sqrt()
                {
                    caps.insert(cell, if u < 0.47 { color } else { BLUE });
                }
            },
        );
    }
    sample(
        Vec3::new(-0.082, 1.392, -5.87),
        Vec3::new(0.082, 1.462, -4.99),
        DETAIL,
        |cell, p| {
            let r = (p.x / 0.080).powi(2) + ((p.z + 5.43) / 0.44).powi(2);
            if r < 1. && p.y < 1.398 + 0.063 * (1. - r).sqrt() {
                caps.insert(cell, PEARL);
            }
        },
    );

    // Round the stabilizer root nose where the upper fin meets the T-tail.
    // The drawn leading edge falls between two native spanwise rows.
    sample(
        Vec3::new(-0.64, 1.31, -5.06),
        Vec3::new(0.64, 1.43, -4.837),
        DETAIL,
        |cell, p| {
            let r = (p.x / 0.64).powi(2)
                + ((p.z + 4.955) / 0.113).powi(2)
                + ((p.y - 1.365) / 0.055).powi(2);
            if r < 1. {
                caps.insert(cell, PEARL);
            }
        },
    );
    sample(
        Vec3::new(-7.28, -0.43, -1.55),
        Vec3::new(7.28, 0.60, 0.95),
        CELL,
        |cell, p| {
            let x = p.x.abs();
            if x > SPAN || (x < 0.75 && hull(p, 0.)) {
                return;
            }
            let [lead, chord, y] = section(WING, x);
            let u = (lead - p.z) / chord;
            if !(0. ..=1.).contains(&u) {
                return;
            }
            let foil = (u * (1. - u)).max(0.).sqrt();
            let mid = y + 0.060 * foil;
            let half = 0.031 + 0.084 * foil * chord * (1. - 0.53 * u);
            if (p.y - mid).abs() > half {
                return;
            }
            let left = p.x > 0.;
            let part = if u > 0.75 && (4.55..6.93).contains(&x) {
                if left {
                    Part::LeftAileron
                } else {
                    Part::RightAileron
                }
            } else if u > 0.78 && (0.68..4.52).contains(&x) {
                if left {
                    Part::LeftFlap
                } else {
                    Part::RightFlap
                }
            } else {
                Part::Body
            };
            let color = if x > 7.11 && u < 0.53 {
                if left {
                    [0.98, 0.038, 0.028]
                } else {
                    [0.025, 0.78, 0.29]
                }
            } else if (0.76..1.10).contains(&x) && (0.31..0.765).contains(&u) && p.y >= mid {
                [0.14, 0.17, 0.18] // Inboard boarding tread.
            } else if x > 6.89 || (x > 4.92 && (0.29..0.40).contains(&u)) {
                BLUE
            } else if x > 4.92 && (0.40..0.47).contains(&u) {
                TEAL
            } else if u < 0.075 && x > 0.69 {
                [0.10, 0.13, 0.15]
            } else if p.y > mid {
                PEARL
            } else {
                SILVER
            };
            solids[part as usize].insert(cell, color);
        },
    );
    sample(
        Vec3::new(-1.92, 1.26, -5.95),
        Vec3::new(1.92, 1.49, -4.73),
        CELL,
        |cell, p| {
            let x = p.x.abs();
            let [front, chord, y] = section(TAIL, x);
            let u = (front - p.z) / chord;
            if x <= 1.89
                && (0. ..=1.).contains(&u)
                && (p.y - y).abs() < 0.033 + 0.067 * (u * (1. - u)).sqrt()
            {
                let part = if u > 0.76 { Part::Elevator } else { Part::Body };
                solids[part as usize].insert(cell, if x > 1.64 { BLUE } else { PEARL });
            }
        },
    );
    sample(
        Vec3::new(-0.20, -0.20, -5.94),
        Vec3::new(0.20, 1.46, -3.70),
        CELL,
        |cell, p| {
            // Side-elevation stations: leading edge, rudder trailing edge, half thickness.
            if p.y > 1.45 {
                return;
            }
            let [leading, trailing, width] = section(FIN, p.y);
            let u = (leading - p.z) / (leading - trailing);
            let half = 0.034 + (width - 0.034) * 2. * (u * (1. - u)).max(0.).sqrt();
            if (trailing..leading).contains(&p.z) && p.x.abs() < half {
                let part = if u > 0.72 { Part::Rudder } else { Part::Body };
                let stripe = p.y - (0.49 + 0.62 * (p.z + 5.90));
                let color = if stripe.abs() < 0.058 {
                    PEARL
                } else if stripe > 0. {
                    BLUE
                } else {
                    SILVER
                };
                solids[part as usize].insert(cell, color);
            }
        },
    );
}

fn engines(solids: &mut Parts, fine: &mut Parts) {
    const NACELLE: &[(f32, [f32; 3])] = &[
        (-1.82, [-0.152, 0.055, 0.064]),
        (-1.42, [-0.123, 0.20, 0.162]),
        (-0.87, [-0.109, 0.306, 0.245]),
        (-0.21, [-0.066, 0.354, 0.306]),
        (0.63, [-0.012, 0.328, 0.283]),
        (1.19, [0.017, 0.282, 0.244]),
        (1.48, [0.021, 0.207, 0.188]),
    ];
    for (side, prop) in [(1., Part::Prop), (-1., Part::RightProp)] {
        let x = side * ENGINE;
        sample(
            Vec3::new(x - 0.37, -0.48, -1.92),
            Vec3::new(x + 0.37, 0.34, 1.43),
            CELL,
            |cell, p| {
                let p = p + Vec3::new(0., 0.028, 0.07);
                let [y, w, h] = section(NACELLE, p.z);
                if Vec2::new((p.x - x) / w, (p.y - y) / h).length_squared() <= 1. {
                    // Paired recessed cooling openings under the spinner.
                    let intake =
                        Vec2::new(((p.x - x).abs() - 0.143) / 0.077, (p.y + 0.075) / 0.068)
                            .length_squared();
                    if p.z > 1.24 && intake < 1. {
                        return;
                    }
                    let color = if p.z > 1.17 && intake < 1.25 {
                        SHADOW
                    } else if p.y > y + 0.025 {
                        BLUE
                    } else if p.y > y - 0.030 {
                        TEAL
                    } else {
                        SILVER
                    };
                    solids[Part::Body as usize].insert(cell, color);
                }
            },
        );
        let hub = PROP.with_x(x);
        let blades = &mut fine[prop as usize];
        sample(
            hub + Vec3::new(-0.98, -0.98, -0.041),
            hub + Vec3::new(0.98, 0.98, 0.40),
            DETAIL,
            |cell, p| {
                let q = p - hub;
                let radius = q.truncate().length();
                let cone = (0.171 * (1. - q.z / 0.35).max(0.).powf(0.70)).max(0.006);
                if q.z >= -0.022 && q.z < 0.35 && radius < cone {
                    blades.insert(cell, if q.z > 0.18 { SILVER } else { BLUE });
                    return;
                }
                for blade in 0..3 {
                    let a = std::f32::consts::TAU * blade as f32 / 3. + 0.12;
                    let axis = Vec2::new(a.sin(), a.cos());
                    let r = q.truncate().dot(axis);
                    let lateral = q.truncate().dot(Vec2::new(axis.y, -axis.x));
                    let sweep = -0.084 * (r - 0.32).max(0.).powi(2);
                    let half = 0.035 + 0.047 * (1.0 - ((r - 0.46) / 0.56).powi(2)).max(0.);
                    if (0.111..0.978).contains(&r)
                        && (lateral - sweep).abs() < half
                        && (q.z - 0.018 - lateral * 0.55).abs() < 0.010
                    {
                        blades.insert(cell, if r > 0.909 { PEARL } else { SHADOW });
                        break;
                    }
                }
            },
        );
        // Landing lamp and stepped cooling exhausts.
        let cabin = &mut fine[Part::Body as usize];
        let lamp = Vec3::new(0.91 * side, -0.118, 0.838);
        block(
            cabin,
            lamp - Vec3::new(0.115, 0.033, 0.012),
            lamp + Vec3::new(0.115, 0.033, 0.021),
            DETAIL,
            SHADOW,
        );
        block(
            cabin,
            lamp - Vec3::new(0.088, 0.021, 0.023),
            lamp + Vec3::new(0.088, 0.021, 0.001),
            DETAIL,
            [0.74, 0.74, 0.56],
        );
        let exhaust = Vec3::new(x + side * 0.21, -0.345, 0.90);
        rod(
            cabin,
            exhaust,
            exhaust + Vec3::new(0., -0.047, -0.24),
            0.036,
            METAL,
        );
        rod(
            cabin,
            exhaust + Vec3::new(0., -0.045, -0.20),
            exhaust + Vec3::new(0., -0.056, -0.26),
            0.029,
            SHADOW,
        );
    }
}

const NOSE_STRUT: Vec3 = Vec3::new(0., -0.34, 1.41);
const NOSE_WHEEL: Vec3 = Vec3::new(0., -1.17, 1.72);

fn gear(fine: &mut Parts) {
    for (leg, wheel, foot, anchor, radius) in [
        (
            Part::NoseGear,
            Part::NoseWheel,
            NOSE_WHEEL,
            NOSE_STRUT,
            0.190,
        ),
        (
            Part::LeftStrut,
            Part::LeftWheel,
            Vec3::new(1.38, -1.139, -0.165),
            Vec3::new(1.32, -0.30, -0.14),
            0.221,
        ),
        (
            Part::RightStrut,
            Part::RightWheel,
            Vec3::new(-1.38, -1.139, -0.165),
            Vec3::new(-1.32, -0.30, -0.14),
            0.221,
        ),
    ] {
        if foot.x != 0. {
            // The outer main-leg door covers the oleo down to the tire shoulder.
            let side = foot.x.signum();
            let panel_x = foot.x + side * 0.066;
            sample(
                Vec3::new(panel_x - 0.014, -0.973, -0.400),
                Vec3::new(panel_x + 0.014, -0.455, 0.048),
                DETAIL,
                |cell, p| {
                    let t = (p.y + 0.973) / 0.518;
                    if p.z >= -0.33 - 0.07 * t && p.z < 0.022 + 0.022 * t {
                        let color = if (p.x - panel_x) * side > 0. {
                            SILVER
                        } else {
                            METAL
                        };
                        fine[leg as usize].insert(cell, color);
                    }
                },
            );
        }
        rod(
            &mut fine[leg as usize],
            anchor,
            foot + Vec3::Y * 0.13,
            0.049,
            METAL,
        );
        rod(
            &mut fine[leg as usize],
            anchor + Vec3::Z * 0.12,
            foot + Vec3::new(0., 0.23, 0.01),
            0.032,
            PANEL,
        );
        for side in [-1., 1.] {
            rod(
                &mut fine[leg as usize],
                foot + Vec3::Y * 0.22,
                foot + Vec3::X * (0.087 * side),
                0.029,
                METAL,
            );
        }
        let half = Vec3::new(0.077, radius, radius);
        sample(foot - half, foot + half, DETAIL, |cell, p| {
            let d = p - foot;
            let r = Vec2::new(d.y, d.z).length();
            let bevel = (1. - ((d.x.abs() / half.x - 0.58).max(0.) / 0.48).powi(2)).max(0.);
            if r < radius * bevel {
                let tread = r > radius * 0.93 && (d.x / 0.019).round().abs() as i32 % 2 == 0;
                let color = if r < radius * 0.35 {
                    if d.x.abs() > 0.058 {
                        SILVER
                    } else {
                        METAL
                    }
                } else if tread {
                    [0.033, 0.040, 0.042]
                } else {
                    SHADOW
                };
                fine[wheel as usize].insert(cell, color);
            }
        });
    }
}

// The floor rises toward the narrow aft luggage bay.
fn floor_rise(z: f32) -> f32 {
    ((-z - 0.47) * 0.125).clamp(0., 0.195)
}

fn seat(cells: &mut Voxels, x: f32, z: f32, width: f32, tall: f32) {
    let lo = Vec3::new(x - width * 0.5, -0.456, z - 0.37);
    let hi = Vec3::new(x + width * 0.5, tall, z + 0.30);
    let offset = Vec3::Y * floor_rise(z);
    sample(lo + offset, hi + offset, UPHOLSTERY, |cell, world| {
        if !hull(world, 0.040) {
            return;
        }
        let p = world - offset;
        let u = (p.x - x) / (width * 0.50);
        let v = (p.z - z - 0.015) / 0.28;
        let shoulder = ((p.y + 0.20) / (tall + 0.20)).clamp(0., 1.);
        let back_z = z - 0.145 - 0.19 * (p.y + 0.18);
        let bolster = u.abs().powi(3);
        let seat_top = -0.222 + 0.047 * bolster - 0.022 * v.powi(2);
        let cushion = u.powi(4) + v.powi(4) < 0.95 && (-0.352..seat_top).contains(&p.y);
        let back_width = 0.96 - 0.19 * shoulder.powi(2);
        let back = (u / back_width).powi(4) + ((p.z - back_z - 0.038 * bolster) / 0.069).powi(4)
            < 1.
            && (-0.25..tall - 0.133 - 0.035 * u.powi(4)).contains(&p.y);
        let head = (u / 0.60).powi(4)
            + ((p.y - tall + 0.080) / 0.080).powi(4)
            + ((p.z - back_z + 0.013) / 0.072).powi(4)
            < 1.;
        let runners =
            (-0.45..-0.346).contains(&p.y) && v.abs() < 0.79 && (0.62..0.75).contains(&u.abs());
        if !(cushion || back || head || runners) {
            return;
        }
        let color = if runners {
            METAL
        } else if !head && (0.675..0.727).contains(&u.abs()) {
            SEAM
        } else if !head && u.abs() < 0.675 && (cushion || p.z > back_z) {
            let seam = if cushion { v * 2.7 } else { (p.y + 0.22) * 8. };
            if (seam - seam.round()).abs() < 0.046 {
                LEATHER
            } else {
                INSERT
            }
        } else {
            LEATHER
        };
        cells.insert(cell, color);
    });
}

fn display(x: f32, y: f32, width: f32, height: f32, z: f32) -> [Vec3; 4] {
    [
        Vec3::new(x + width * 0.5, y + height * 0.5, z),
        Vec3::new(x - width * 0.5, y + height * 0.5, z),
        Vec3::new(x - width * 0.5, y - height * 0.5, z),
        Vec3::new(x + width * 0.5, y - height * 0.5, z),
    ]
}

fn cabin(solids: &mut Parts, fine: &mut Parts) -> [(u32, [Vec3; 4]); 6] {
    let cabin = &mut solids[Part::Cabin as usize];
    sample(
        Vec3::new(-0.66, -0.50, -2.97),
        Vec3::new(0.66, -0.17, 1.15),
        CELL,
        |cell, p| {
            if hull(p, 0.025) && (-0.50..-0.37).contains(&(p.y - floor_rise(p.z))) {
                cabin.insert(cell, PANEL);
            }
        },
    );
    block(
        cabin,
        Vec3::new(-0.16, -0.39, -0.44),
        Vec3::new(0.16, -0.18, 0.93),
        CELL,
        PANEL,
    );
    sample(
        Vec3::new(-0.47, -0.23, -2.82),
        Vec3::new(0.47, -0.10, -2.50),
        CELL,
        |cell, p| {
            if hull(p, 0.060) {
                cabin.insert(cell, SEAM);
            }
        },
    );
    let hardware = &mut fine[Part::Cabin as usize];
    // Matte fascia and separate glareshield follow the sides of the canopy.
    sample(
        Vec3::new(-0.612, -0.28, 0.77),
        Vec3::new(0.612, 0.392, 1.04),
        DETAIL,
        |cell, p| {
            let breadth = 0.60 - 0.18 * (p.y - 0.17).max(0.);
            let cap = p.y > 0.303;
            let profile = 0.351 - 0.148 * (p.x.abs() / 0.63).powi(3);
            if p.x.abs() < breadth && p.y < profile && (p.z > 0.862 || cap) {
                hardware.insert(cell, if cap { SHADOW } else { PANEL });
            }
        },
    );
    for x in [-0.27, 0.27] {
        block(
            hardware,
            Vec3::new(x - 0.172, 0.003, 0.849),
            Vec3::new(x + 0.172, 0.283, 0.885),
            DETAIL,
            SHADOW,
        );
        for side in [-1., 1.] {
            for y in [0.041, 0.072, 0.104, 0.136, 0.168, 0.200, 0.232] {
                let lo = Vec3::new(x + side * 0.154 - 0.008, y, 0.837);
                block(
                    hardware,
                    lo,
                    lo + Vec3::new(0.016, 0.012, 0.014),
                    DETAIL,
                    METAL,
                );
            }
        }
        for i in 0..8 {
            let lo = Vec3::new(x - 0.115 + i as f32 * 0.033, 0.015, 0.837);
            block(
                hardware,
                lo,
                lo + Vec3::new(0.018, 0.010, 0.015),
                DETAIL,
                [0.30, 0.36, 0.39],
            );
        }
    }
    for x in [-0.50, -0.43, -0.36, 0.36, 0.43, 0.50] {
        let lo = Vec3::new(x - 0.014, -0.104, 0.837);
        block(
            hardware,
            lo,
            lo + Vec3::new(0.028, 0.027, 0.045),
            DETAIL,
            [0.38, 0.43, 0.42],
        );
    }
    // Recessed lower switch banks, landing-gear handle, annunciators and vents.
    for side in [-1., 1.] {
        let x = side * 0.415;
        block(
            hardware,
            Vec3::new(x - 0.122, -0.206, 0.838),
            Vec3::new(x + 0.122, -0.137, 0.869),
            DETAIL,
            SHADOW,
        );
        for i in 0..6 {
            let center = Vec3::new(x - 0.089 + i as f32 * 0.036, -0.178, 0.838);
            let color = if i == 0 && side > 0. {
                [0.73, 0.10, 0.054]
            } else {
                METAL
            };
            rod(
                hardware,
                center,
                center + Vec3::new(0., 0.010, -0.024),
                0.009,
                color,
            );
        }
        let vent = Vec3::new(side * 0.503, 0.143, 0.851);
        rod(hardware, vent, vent - Vec3::Z * 0.013, 0.052, METAL);
        rod(
            hardware,
            vent - Vec3::Z * 0.013,
            vent - Vec3::Z * 0.021,
            0.044,
            SHADOW,
        );
        for y in [-0.024, -0.008, 0.008, 0.024] {
            rod(
                hardware,
                vent + Vec3::new(-0.031, y, -0.026),
                vent + Vec3::new(0.031, y, -0.026),
                0.005,
                PANEL,
            );
        }
    }
    let gear_handle = Vec3::new(0.175, -0.130, 0.862);
    rod(
        hardware,
        gear_handle,
        gear_handle - Vec3::Z * 0.061,
        0.013,
        METAL,
    );
    rod(
        hardware,
        gear_handle + Vec3::new(-0.015, 0., -0.061),
        gear_handle + Vec3::new(0.015, 0., -0.061),
        0.024,
        PEARL,
    );
    for x in [0.135, 0.175, 0.215] {
        let lamp = Vec3::new(x, -0.062, 0.859);
        rod(
            hardware,
            lamp,
            lamp - Vec3::Z * 0.013,
            0.010,
            [0.12, 0.61, 0.24],
        );
    }
    // Center console uses two single-engine power controls and a keypad.
    block(
        hardware,
        Vec3::new(-0.138, -0.105, 0.475),
        Vec3::new(0.138, -0.069, 0.83),
        DETAIL,
        SHADOW,
    );
    for x in [-0.050, 0.050] {
        block(
            hardware,
            Vec3::new(x - 0.019, -0.067, 0.51),
            Vec3::new(x + 0.019, -0.059, 0.70),
            DETAIL,
            METAL,
        );
    }
    for row in 0..4 {
        for col in 0..6 {
            let lo = Vec3::new(
                -0.087 + 0.030 * col as f32,
                -0.064,
                0.245 + 0.031 * row as f32,
            );
            block(
                hardware,
                lo,
                lo + Vec3::new(0.018, 0.016, 0.019),
                DETAIL,
                METAL,
            );
        }
    }
    block(
        hardware,
        Vec3::new(-0.115, -0.102, 0.21),
        Vec3::new(0.115, -0.066, 0.41),
        DETAIL,
        PANEL,
    );
    for (part, side) in [(Part::Throttle, 1.), (Part::Mixture, -1.)] {
        let lever = &mut fine[part as usize];
        let p = Vec3::new(0.050 * side, -0.080, 0.61);
        rod(lever, p, p + Vec3::new(0., 0.10, -0.015), 0.016, METAL);
        rod(
            lever,
            p + Vec3::new(-0.029, 0.104, -0.015),
            p + Vec3::new(0.029, 0.104, -0.015),
            0.023,
            SHADOW,
        );
        block(
            lever,
            p + Vec3::new(-0.015, 0.123, -0.024),
            p + Vec3::new(0.015, 0.131, -0.005),
            DETAIL,
            PEARL,
        );
    }
    for (part, side) in [(Part::Yoke, 1.), (Part::CopilotYoke, -1.)] {
        let stick = &mut fine[part as usize];
        let p = Vec3::new(0.323 * side, -0.35, 0.213);
        rod(stick, p, p + Vec3::new(0., 0.185, -0.040), 0.025, METAL);
        rod(
            stick,
            p + Vec3::new(0., 0.162, -0.034),
            p + Vec3::new(-0.012 * side, 0.295, -0.066),
            0.028,
            SHADOW,
        );
        rod(
            stick,
            p + Vec3::new(-0.012 * side, 0.293, -0.066),
            p + Vec3::new(0.024 * side, 0.314, -0.064),
            0.020,
            SHADOW,
        );
        block(
            stick,
            p + Vec3::new(-0.019, 0.307, -0.087),
            p + Vec3::new(-0.003, 0.327, -0.071),
            DETAIL,
            [0.66, 0.056, 0.031],
        );
        for (pedal, offset) in [(Part::LeftPedal, 0.079), (Part::RightPedal, -0.079)] {
            let center = Vec3::new(p.x + offset, -0.324, 0.758);
            block(
                &mut fine[pedal as usize],
                center - Vec3::new(0.034, 0.046, 0.018),
                center + Vec3::new(0.034, 0.052, 0.019),
                DETAIL,
                PANEL,
            );
            for y in [-0.023, 0., 0.023] {
                let lo = center + Vec3::new(-0.025, y, -0.028);
                block(
                    &mut fine[pedal as usize],
                    lo,
                    lo + Vec3::new(0.050, DETAIL, 0.014),
                    DETAIL,
                    METAL,
                );
            }
        }
    }
    // Exterior flush handles are inset into a dark escutcheon on each door.
    // Use the stepped shell face at this station so hardware touches the skin.
    for (part, side, z) in [
        (Part::LeftDoor, 1., -0.13),
        (Part::RightDoor, -1., -0.13),
        (Part::RearDoor, 1., -1.32),
    ] {
        let skin = &mut fine[part as usize];
        sample(
            Vec3::new(-0.74, 0.095, z - 0.090),
            Vec3::new(0.74, 0.162, z + 0.090),
            DETAIL,
            |cell, p| {
                let row = ((p / CELL).floor() + 0.5) * CELL;
                let face = (hull_side(row.y, row.z) / CELL + 0.5).floor() * CELL;
                let depth = p.x * side - face;
                let u = (p.z - z) / 0.088;
                let v = (p.y - 0.129) / 0.031;
                if u.powi(4) + v.powi(4) < 1. && (0.0..0.015).contains(&depth) {
                    skin.insert(
                        cell,
                        if u.abs() < 0.80 && (-0.35..0.45).contains(&v) {
                            METAL
                        } else {
                            NAVY
                        },
                    );
                }
            },
        );
    }
    // Fine handles and door cards follow their hinges.
    for (part, side, z) in [
        (Part::LeftDoor, 1., 0.02),
        (Part::RightDoor, -1., 0.02),
        (Part::RearDoor, 1., -1.18),
    ] {
        let door = &mut fine[part as usize];
        let x = hull_side(0.048, z) - 0.090;
        sample(
            Vec3::new(-0.72, -0.192, z - 0.22),
            Vec3::new(0.72, 0.15, z + 0.32),
            DETAIL,
            |cell, p| {
                let inner = hull_side(p.y, p.z) - p.x * side;
                if (0.055..0.095).contains(&inner) {
                    door.insert(cell, if p.y > 0.072 { SEAM } else { LEATHER });
                }
            },
        );
        let grip = Vec3::new(x * side, 0.048, z);
        rod(
            door,
            grip + Vec3::new(-0.032 * side, 0., -0.065),
            grip + Vec3::new(-0.032 * side, 0., 0.125),
            0.016,
            SHADOW,
        );
        for end in [-0.065, 0.125] {
            let mount = grip + Vec3::Z * end;
            rod(door, mount, mount - Vec3::X * (0.032 * side), 0.015, METAL);
        }
    }
    [
        (7, display(0.27, 0.150, 0.268, 0.200, 0.845)),
        (8, display(-0.27, 0.150, 0.268, 0.200, 0.845)),
        (2, display(0., 0.127, 0.080, 0.080, 0.858)),
        (10, display(0.415, -0.044, 0.195, 0.027, 0.857)),
        (11, display(-0.415, -0.044, 0.195, 0.027, 0.857)),
        (12, display(0.175, -0.026, 0.096, 0.023, 0.857)),
    ]
}

pub(super) fn geometry() -> &'static Geometry {
    static MESH: OnceLock<Geometry> = OnceLock::new();
    MESH.get_or_init(|| {
        let mut solids = std::array::from_fn(|_| Voxels::new());
        let mut fine = std::array::from_fn(|_| Voxels::new());
        let glass = fuselage(&mut solids);
        lift_surfaces(&mut solids, &mut fine);
        engines(&mut solids, &mut fine);
        gear(&mut fine);
        let displays = cabin(&mut solids, &mut fine);
        let mut seats = Voxels::new();
        for x in [-0.327, 0.327] {
            seat(&mut seats, x, -0.34, 0.48, 0.43);
        }
        for x in [-0.365, 0., 0.365] {
            seat(&mut seats, x, -1.35, 0.365, 0.40);
        }
        for x in [-0.215, 0.215] {
            seat(&mut seats, x, -2.14, 0.40, 0.26);
        }
        let mut volumes = std::array::from_fn(|i| solids[i].len() + fine[i].len());
        volumes[Part::Cabin as usize] += seats.len();
        let mut vertices = Vec::new();
        let mut parts = std::array::from_fn(|_| 0..0);
        let mut fine_parts = std::array::from_fn(|_| 0..0);
        for i in 0..Part::COUNT {
            let first = vertices.len() as u32;
            vertices.extend(
                voxels::mesh_at(solids[i].clone(), CELL)
                    .into_iter()
                    .map(|v| solid_vertex(v, i)),
            );
            let detail = vertices.len() as u32;
            if i == Part::Cabin as usize {
                vertices.extend(
                    voxels::mesh_at(std::mem::take(&mut seats), UPHOLSTERY)
                        .into_iter()
                        .map(|v| solid_vertex(v, i)),
                );
            }
            vertices.extend(
                voxels::mesh_at(std::mem::take(&mut fine[i]), DETAIL)
                    .into_iter()
                    .map(|v| solid_vertex(v, i)),
            );
            fine_parts[i] = detail..vertices.len() as u32;
            parts[i] = first..vertices.len() as u32;
        }
        let displays_start = vertices.len() as u32;
        for (id, face) in displays {
            rectangle(&mut vertices, Part::Cabin, id, face);
        }
        let opaque_count = vertices.len() as u32;
        let mut panes = Vec::new();
        for (part, cells) in glass {
            if cells.is_empty() {
                continue;
            }
            let center = cells
                .keys()
                .map(|p| (p.as_vec3() + Vec3::splat(0.5)) * CELL)
                .sum::<Vec3>()
                / cells.len() as f32;
            let first = vertices.len() as u32;
            for cell in cells.keys() {
                let center = (cell.as_vec3() + Vec3::splat(0.5)) * CELL;
                for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                    for sign in [-1., 1.] {
                        let n = axis * sign;
                        if hull(center + n * CELL, 0.) {
                            continue;
                        }
                        let face = center + n * (CELL * 0.5);
                        let a = Vec3::new(axis.z, axis.x, axis.y) * (CELL * 0.5);
                        let b = n.cross(a);
                        let start = vertices.len();
                        rectangle(
                            &mut vertices,
                            part,
                            0,
                            [face - a - b, face + a - b, face + a + b, face - a + b],
                        );
                        for v in &mut vertices[start..] {
                            v.normal = hull_gradient(Vec3::from_array(v.position)).to_array();
                        }
                    }
                }
            }
            let count = vertices.len() as u32 - first;
            if count > 0 {
                panes.push(Pane {
                    first,
                    count,
                    center,
                    part,
                    prop_disc: false,
                });
            }
        }
        Geometry {
            prop: parts[Part::Prop as usize].clone(),
            parts,
            fine_parts,
            displays_start,
            volumes,
            vertices,
            opaque_count,
            panes,
            solids,
        }
    })
}

pub(super) fn joints(pose: Presentation, rig: Rig) -> [Mat4; Part::COUNT] {
    let mut result = [Mat4::IDENTITY; Part::COUNT];
    let [roll, pitch, _] = pose.controls.to_array();
    for (i, side) in [1., -1.].into_iter().enumerate() {
        let (door, prop, stick, lever, aileron, flap, strut, wheel) = if i == 0 {
            (
                Part::LeftDoor,
                Part::Prop,
                Part::Yoke,
                Part::Throttle,
                Part::LeftAileron,
                Part::LeftFlap,
                Part::LeftStrut,
                Part::LeftWheel,
            )
        } else {
            (
                Part::RightDoor,
                Part::RightProp,
                Part::CopilotYoke,
                Part::Mixture,
                Part::RightAileron,
                Part::RightFlap,
                Part::RightStrut,
                Part::RightWheel,
            )
        };
        result[door as usize] = turn(
            Vec3::new(0.239 * side, 0.762, -0.16),
            Mat4::from_rotation_z(1.41 * side * rig.doors[i]),
        );
        let angle = if i == 0 {
            pose.prop_angle
        } else {
            pose.right_prop_angle
        };
        result[prop as usize] = turn(PROP.with_x(side * ENGINE), Mat4::from_rotation_z(angle));
        result[stick as usize] = turn(
            Vec3::new(0.323 * side, -0.35, 0.213),
            Mat4::from_rotation_z(roll * 0.24) * Mat4::from_rotation_x(pitch * -0.21),
        );
        let power = if i == 0 {
            pose.controls.z
        } else {
            pose.right_power
        };
        result[lever as usize] = turn(
            Vec3::new(0.050 * side, -0.080, 0.61),
            Mat4::from_rotation_x((power - 0.5) * 0.90),
        );
        result[aileron as usize] = turn(
            Vec3::new(side * 5.72, 0.24, -0.80),
            Mat4::from_rotation_x(-0.28 * side * roll),
        );
        result[flap as usize] = turn(
            Vec3::new(side * 2.59, -0.087, -0.94),
            Mat4::from_rotation_x(-0.48 * rig.flaps),
        );
        let pivot = Vec3::new(1.32 * side, -0.30, -0.14);
        let tuck = rig.gear.powi(3);
        let retract = Mat4::from_translation(Vec3::new(0.36 * side, 0.291, 0.22) * tuck)
            * turn(pivot, Mat4::from_rotation_x(1.40 * rig.gear));
        let suspension =
            Vec3::Y * (pose.gear_compression[if i == 0 { 1 } else { 0 }] * (1. - rig.gear));
        let retract = retract * Mat4::from_translation(suspension);
        result[strut as usize] = retract;
        result[wheel as usize] = retract
            * turn(
                Vec3::new(1.38 * side, -1.139, -0.165),
                Mat4::from_rotation_x(pose.wheel_turn[if i == 0 { 1 } else { 0 }]),
            );
    }
    result[Part::RearDoor as usize] = turn(
        Vec3::new(0.24, 0.69, -1.70),
        Mat4::from_rotation_z(1.40 * rig.rear_door),
    );
    result[Part::Elevator as usize] = turn(
        Vec3::new(0., 1.42, -5.65),
        Mat4::from_rotation_x(elevator_deflection(
            (pitch + pose.instruments.pitch_trim).clamp(-1., 1.),
        )),
    );
    result[Part::Rudder as usize] = turn(
        Vec3::new(0., 0.82, -5.60),
        Mat4::from_rotation_y(0.27 * pose.rudder),
    );
    for (part, side) in [(Part::LeftPedal, 1.), (Part::RightPedal, -1.)] {
        result[part as usize] = Mat4::from_translation(Vec3::Z * (0.036 * side * pose.rudder));
    }
    let nose = Mat4::from_translation(Vec3::new(0., 0.03, 0.045) * rig.gear.powi(3))
        * turn(NOSE_STRUT, Mat4::from_rotation_x(-1.45 * rig.gear))
        * turn(
            NOSE_WHEEL,
            Mat4::from_rotation_y(pose.rudder * 0.45 * (1. - rig.gear)),
        );
    let nose = nose * Mat4::from_translation(Vec3::Y * pose.gear_compression[2] * (1. - rig.gear));
    result[Part::NoseGear as usize] = nose;
    result[Part::NoseWheel as usize] =
        nose * turn(NOSE_WHEEL, Mat4::from_rotation_x(pose.wheel_turn[2]));
    result
}
