//! Lightweight high-wing bush plane, authored in metres around its pilot.
//! The same depth test as terrain handles ridgelines and ground occlusion.
use crate::aircraft::airframe::{
    elevator_deflection, fin_section, AILERON_HINGE_Z, AILERON_TRAVEL, CONTROL_TIP,
    ELEVATOR_HINGE_Z, FIN_HEIGHT, FIN_ROOT, FLAP_SPLIT, FLAP_TRAVEL, PROP_Z, RUDDER_HINGE_Z,
    RUDDER_TRAVEL, TAIL, WING,
};
use crate::{aircraft::Presentation, mesh::Vertex, Camera};
use bytemuck::{Pod, Zeroable};
use glam::{IVec3, Mat4, Vec3};
use wgpu::util::DeviceExt;

const WHITE: [f32; 3] = [0.83, 0.86, 0.86];
const ACCENT: [f32; 3] = [0.025, 0.25, 0.36];
const PINSTRIPE: [f32; 3] = [0.77, 0.52, 0.19];
const BLACK: [f32; 3] = [0.035, 0.042, 0.047];
const METAL: [f32; 3] = [0.27, 0.31, 0.33];

fn quad(out: &mut Vec<Vertex>, p: [Vec3; 4], color: [f32; 3]) {
    for [a, b, c] in [[0, 1, 2], [0, 2, 3]] {
        let n = (p[b] - p[a]).cross(p[c] - p[a]).normalize_or_zero();
        if n == Vec3::ZERO {
            continue;
        }
        for i in [a, b, c] {
            out.push(Vertex {
                position: p[i].to_array(),
                normal: n.to_array(),
                color,
            });
        }
    }
}

fn loft(
    out: &mut Vec<Vertex>,
    sections: usize,
    sides: usize,
    point: impl Fn(usize, f32, f32) -> Vec3,
    color: impl Fn(usize, usize) -> [f32; 3],
) {
    for i in 0..sections - 1 {
        for j in 0..sides {
            let (s, c) = (std::f32::consts::TAU * j as f32 / sides as f32).sin_cos();
            let (s1, c1) =
                (std::f32::consts::TAU * ((j + 1) % sides) as f32 / sides as f32).sin_cos();
            quad(
                out,
                [
                    point(i, s, c),
                    point(i, s1, c1),
                    point(i + 1, s1, c1),
                    point(i + 1, s, c),
                ],
                color(i, j),
            );
        }
    }
}

fn tube(out: &mut Vec<Vertex>, a: Vec3, b: Vec3, radius: f32, color: [f32; 3]) {
    let axis = (b - a).normalize();
    let u = axis.any_orthonormal_vector();
    let v = axis.cross(u);
    loft(
        out,
        2,
        8,
        |i, s, c| {
            let center = if i == 0 { a } else { b };
            center + (u * c + v * s) * radius
        },
        |_, _| color,
    );
}

fn tire(out: &mut Vec<Vertex>, center: Vec3, radius: f32, width: f32) {
    let rings = [
        (-1., 0.43),
        (-0.79, 0.87),
        (-0.40, 1.),
        (0.40, 1.),
        (0.79, 0.87),
        (1., 0.43),
    ];
    loft(
        out,
        rings.len(),
        20,
        |i, s, c| {
            let (x, r) = rings[i];
            center + Vec3::new(x * width, s * r * radius, c * r * radius)
        },
        |i, _| if i == 2 { [0.016, 0.018, 0.020] } else { BLACK },
    );
    tube(
        out,
        center - Vec3::X * width,
        center + Vec3::X * width,
        radius * 0.43,
        METAL,
    );
    for end in [-1., 1.] {
        let hub = center + Vec3::X * (width + 0.014) * end;
        quad(
            out,
            [
                hub + Vec3::Y * radius * 0.44,
                hub + Vec3::Z * radius * 0.44,
                hub - Vec3::Y * radius * 0.44,
                hub - Vec3::Z * radius * 0.44,
            ],
            WHITE,
        );
    }
}

fn fairing(out: &mut Vec<Vertex>, hub: Vec3, width: f32, length: f32) {
    let rings = [
        (-1., 0.12),
        (-0.63, 0.60),
        (0., 1.),
        (0.66, 0.86),
        (1., 0.15),
    ];
    loft(
        out,
        rings.len(),
        16,
        |i, s, c| {
            let (z, scale) = rings[i];
            hub + Vec3::new(
                width * scale * c,
                (0.231 * scale * s).max(-0.055) + 0.075,
                z * length,
            )
        },
        |_, j| if (9..15).contains(&j) { ACCENT } else { WHITE },
    );
}

fn skin() -> [Vec<Vertex>; Part::COUNT] {
    let mut pieces = std::array::from_fn(|_| Vec::new());
    let mut out = Vec::new();
    // Leave the cabin open so the interior and the hinged doors own its sides.
    let tail = [
        (-5.62, 0.025, 0.055, 0.11),
        (-5.29, 0.16, 0.16, 0.07),
        (-4.20, 0.24, 0.24, 0.02),
        (-3.18, 0.37, 0.38, 0.02),
        (-2.51, 0.49, 0.47, 0.04),
        (-2.07, 0.61, 0.57, 0.10),
    ];
    let cowl = [
        (1.00, 0.622, 0.385, -0.16),
        (1.30, 0.61, 0.37, -0.14),
        (1.80, 0.56, 0.32, -0.13),
        (2.21, 0.48, 0.28, -0.12),
        (2.50, 0.34, 0.235, -0.10),
        (2.57, 0.12, 0.10, -0.08),
    ];
    for stations in [&tail[..], &cowl[..]] {
        loft(
            &mut out,
            stations.len(),
            24,
            |i, s, c| {
                let (z, w, h, y) = stations[i];
                Vec3::new(w * c, y + h * s, z)
            },
            |_, j| if (14..22).contains(&j) { ACCENT } else { WHITE },
        );
    }
    for side in [-1., 1.] {
        // Single streamlined lift strut and inset mounting plates.
        let strut = Vec3::new(side * 0.55, -0.39, -0.07);
        tube(
            &mut out,
            strut,
            Vec3::new(side * 3.53, 0.98, 0.21),
            0.067,
            WHITE,
        );
        let (gear, tire_part) = if side > 0. {
            (Part::LeftStrut, Part::LeftWheel)
        } else {
            (Part::RightStrut, Part::RightWheel)
        };
        let wheel = Vec3::new(side * 0.94, -1.07, -0.38);
        let gear_mesh = &mut pieces[gear as usize];
        tube(
            gear_mesh,
            Vec3::new(side * 0.50, -0.45, -0.49),
            Vec3::new(side * 0.79, -0.77, -0.40),
            0.071,
            WHITE,
        );
        tube(
            gear_mesh,
            Vec3::new(side * 0.79, -0.77, -0.40),
            wheel,
            0.064,
            METAL,
        );
        fairing(gear_mesh, wheel, 0.20, 0.39);
        tire(&mut pieces[tire_part as usize], wheel, 0.25, 0.105);
    }
    pieces[Part::Body as usize] = out;
    let out = &mut pieces[Part::NoseGear as usize];
    // Steerable nosewheel fork below the engine mount.
    for side in [-1., 1.] {
        tube(
            out,
            Vec3::new(side * 0.11, -0.42, 1.48),
            Vec3::new(side * 0.10, -1.14, 1.63),
            0.046,
            METAL,
        );
    }
    fairing(out, Vec3::new(0., -1.14, 1.63), 0.132, 0.28);
    tire(
        &mut pieces[Part::NoseWheel as usize],
        Vec3::new(0., -1.14, 1.63),
        0.18,
        0.085,
    );
    let out = &mut pieces[Part::Prop as usize];
    for j in 0..2 {
        let angle = 0.34 + j as f32 * std::f32::consts::PI;
        let r = Vec3::new(angle.cos(), angle.sin(), 0.);
        let t = Vec3::new(-r.y, r.x, 0.);
        let hub = Vec3::new(0., -0.06, 2.60);
        for (a, b, w0, w1, color) in [
            (0.10, 0.90, 0.07, 0.095, BLACK),
            (0.90, 1.04, 0.095, 0.058, WHITE),
        ] {
            quad(
                out,
                [
                    hub + r * a - t * w0,
                    hub + r * b - t * w1,
                    hub + r * b + t * w1,
                    hub + r * a + t * w0,
                ],
                color,
            );
        }
    }
    pieces
}

#[path = "aircraft_voxels.rs"]
pub(super) mod voxels;

#[path = "aircraft_cabin.rs"]
mod cabin;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum Part {
    Body,
    Prop,
    Yoke,
    LeftDoor,
    RightDoor,
    LeftAileron,
    RightAileron,
    Elevator,
    Throttle,
    Rudder,
    LeftPedal,
    RightPedal,
    NoseGear,
    LeftStrut,
    RightStrut,
    LeftWheel,
    RightWheel,
    NoseWheel,
    Cabin,
    CopilotYoke,
    LeftFlap,
    RightFlap,
}
impl Part {
    pub(super) const COUNT: usize = 22;
    pub(crate) const ALL: [Self; Self::COUNT] = [
        Self::Body,
        Self::Prop,
        Self::Yoke,
        Self::LeftDoor,
        Self::RightDoor,
        Self::LeftAileron,
        Self::RightAileron,
        Self::Elevator,
        Self::Throttle,
        Self::Rudder,
        Self::LeftPedal,
        Self::RightPedal,
        Self::NoseGear,
        Self::LeftStrut,
        Self::RightStrut,
        Self::LeftWheel,
        Self::RightWheel,
        Self::NoseWheel,
        Self::Cabin,
        Self::CopilotYoke,
        Self::LeftFlap,
        Self::RightFlap,
    ];
    pub(crate) fn name(self) -> &'static str {
        [
            "airframe",
            "propeller",
            "pilot-yoke",
            "left-door",
            "right-door",
            "left-aileron",
            "right-aileron",
            "elevator",
            "throttle",
            "rudder",
            "left-pedals",
            "right-pedals",
            "nose-strut",
            "left-strut",
            "right-strut",
            "left-wheel",
            "right-wheel",
            "nose-wheel",
            "cabin",
            "copilot-yoke",
            "left-flap",
            "right-flap",
        ][self as usize]
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ModelVertex {
    position: [f32; 3],
    color: [f32; 3],
    normal: [f32; 3],
    part: u32,
    display: u32,
    uv: [f32; 2],
}

struct Pane {
    first: u32,
    center: Vec3,
    part: Part,
}

struct DamagedMesh {
    mask: u64,
    buffer: wgpu::Buffer,
    prop: std::ops::Range<u32>,
    opaque: u32,
    pane_starts: Vec<u32>,
}
struct Geometry {
    parts: [std::ops::Range<u32>; Part::COUNT],
    volumes: [usize; Part::COUNT],
    vertices: Vec<ModelVertex>,
    opaque_count: u32,
    prop: std::ops::Range<u32>,
    panes: Vec<Pane>,
    solids: [voxels::Voxels; Part::COUNT],
}

fn rectangle(out: &mut Vec<ModelVertex>, part: Part, display: u32, points: [Vec3; 4]) {
    let normal = (points[1] - points[0])
        .cross(points[2] - points[0])
        .normalize()
        .to_array();
    let uv = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
    for index in [0, 1, 2, 0, 2, 3] {
        out.push(ModelVertex {
            position: points[index].to_array(),
            normal,
            color: [0.; 3],
            part: part as u32,
            display,
            uv: uv[index],
        });
    }
}

fn build_surfaces(solids: &mut [voxels::Voxels; Part::COUNT]) {
    // Each pair shares one tapered cross section. Ownership changes behind
    // the hinge, leaving a one-cell slot and closed tips on both wings.
    let edge = cabin::EDGE;
    for (planform, is_tail) in [(&WING, false), (&TAIL, true)] {
        let limit = (planform.span / edge).ceil() as i32;
        for x in -limit..limit {
            let u = ((x as f32 + 0.5) * edge).abs() / planform.span;
            if u > 1. {
                continue;
            }
            let (chord, center, height) = planform.section(u);
            let lo = ((center - chord * 0.5) / edge).floor() as i32;
            let hi = ((center + chord * 0.5) / edge).ceil() as i32;
            let hinge_cell = (planform.hinge / edge).floor() as i32;
            for z in lo..hi {
                let along = (((z as f32 + 0.5) * edge) - (center - chord * 0.5)) / chord;
                if !(0.0..=1.0).contains(&along) {
                    continue;
                }
                let movable = u > planform.control_root && u < CONTROL_TIP;
                if movable && z == hinge_cell {
                    continue;
                }
                let part = if movable && z < hinge_cell {
                    if is_tail {
                        Part::Elevator
                    } else if u < FLAP_SPLIT {
                        if x >= 0 {
                            Part::LeftFlap
                        } else {
                            Part::RightFlap
                        }
                    } else if x >= 0 {
                        Part::LeftAileron
                    } else {
                        Part::RightAileron
                    }
                } else {
                    Part::Body
                };
                let radius = chord * 0.092 * (4. * along * (1. - along)).sqrt();
                let lower = ((height - radius) / edge).floor() as i32;
                let upper = ((height + radius) / edge).ceil() as i32;
                for y in lower..upper {
                    let paint = if u > 0.95 { ACCENT } else { WHITE };
                    solids[part as usize].insert(IVec3::new(x, y, z), paint);
                }
            }
        }
    }
    // Rudder and fin share the same tapered section and registration stripe.
    let hinge = (RUDDER_HINGE_Z / edge).floor() as i32;
    let fin_root = (FIN_ROOT / edge) as i32;
    let fin_cells = (FIN_HEIGHT / edge) as i32;
    for y in fin_root..fin_root + fin_cells {
        let fraction = (y - fin_root) as f32 / fin_cells as f32;
        let (front, rear) = fin_section(fraction);
        let front = (front / edge) as i32;
        let rear = (rear / edge) as i32;
        for z in rear..front {
            if z == hinge {
                continue;
            }
            let part = if z < hinge { Part::Rudder } else { Part::Body };
            let paint = if y >= 17 {
                ACCENT
            } else if (16..17).contains(&y) {
                BLACK
            } else {
                WHITE
            };
            let half_width = if fraction < 0.36 && z > hinge { 2 } else { 1 };
            for x in -half_width..half_width {
                solids[part as usize].insert(IVec3::new(x, y, z), paint);
            }
        }
    }
}

fn geometry() -> &'static Geometry {
    static MESH: std::sync::OnceLock<Geometry> = std::sync::OnceLock::new();
    MESH.get_or_init(|| {
        let skin = skin();
        let mut cabin = cabin::build();
        let solids = &mut cabin.solids;
        // Explicit cabin paint and trim have priority at the fuselage joins.
        for (part, skin) in solids.iter_mut().zip(&skin) {
            if skin.is_empty() {
                continue;
            }
            let explicit = std::mem::take(part);
            *part = voxels::rasterize(skin);
            part.extend(explicit);
        }
        for (part, hub, radius, width) in [
            (Part::LeftStrut, Vec3::new(0.94, -1.07, -0.38), 0.25, 0.126),
            (
                Part::RightStrut,
                Vec3::new(-0.94, -1.07, -0.38),
                0.25,
                0.126,
            ),
            (Part::NoseGear, Vec3::new(0., -1.14, 1.63), 0.18, 0.090),
        ] {
            solids[part as usize].retain(|cell, _| {
                let off = (cell.as_vec3() + Vec3::splat(0.5)) * cabin::EDGE - hub;
                off.x.abs() > width || off.y.hypot(off.z) > radius + 0.025
            });
        }
        build_surfaces(solids);
        cabin::block(
            &mut solids[Part::Body as usize],
            [86, 16, -4],
            [88, 18, 0],
            [0.98, 0.04, 0.025],
        );
        cabin::block(
            &mut solids[Part::Body as usize],
            [-88, 16, -4],
            [-86, 18, 0],
            [0.02, 0.9, 0.27],
        );
        cabin::block(
            &mut solids[Part::Body as usize],
            [-1, 22, -85],
            [1, 23, -83],
            [0.95, 0.055, 0.035],
        );
        cabin::block(
            &mut solids[Part::Body as usize],
            [4, -8, 33],
            [6, -6, 38],
            METAL,
        );
        let body = &mut solids[Part::Body as usize];
        // Ram-air inlets and a bright spinner ahead of the orange engine cowling.
        for x in [-6, 3] {
            cabin::block(body, [x, -4, 38], [x + 3, -2, 41], BLACK);
            cabin::block(body, [x, -2, 38], [x + 3, -1, 40], METAL);
        }
        for (z, half) in [(41, 3), (42, 2), (43, 1)] {
            cabin::block(body, [-half, -1 - half, z], [half, -1 + half, z + 1], WHITE);
        }
        for side in [-1, 1] {
            // Fuel caps and inspection plates on the wing, plus a landing lamp.
            let x = side * 26;
            cabin::block(body, [x, 16, 3], [x + 3, 17, 5], METAL);
            cabin::block(body, [x + 1, 17, 3], [x + 2, 18, 5], BLACK);
        }
        cabin::block(body, [63, 15, 8], [68, 17, 9], METAL);
        cabin::block(body, [64, 15, 9], [67, 17, 10], [0.94, 0.91, 0.72]);
        // VHF whip raked aft in native cells.
        for y in 14..21 {
            let z = -17 - (y - 14) / 2;
            cabin::block(body, [-1, y, z], [0, y + 1, z + 1], METAL);
        }

        // Carry the same painted stripe across the sheet-metal joins.
        let skin = &mut solids[Part::Body as usize];
        for (p, paint) in skin.iter_mut() {
            if p.z >= 17 && p.z < 38 && p.x.abs() > 4 {
                if (-7..-4).contains(&p.y) {
                    *paint = ACCENT;
                }
                if p.y == -4 {
                    *paint = PINSTRIPE;
                }
            }
        }

        // The doors own their shared boundary when closed. Within a part,
        // one occupancy cell has one paint and internal faces are culled.
        for part in [Part::LeftDoor, Part::RightDoor] {
            let door = std::mem::take(&mut solids[part as usize]);
            for cell in door.keys() {
                solids[Part::Body as usize].remove(cell);
            }
            solids[part as usize] = door;
        }
        let mut vertices = Vec::new();
        let volumes = std::array::from_fn(|part| solids[part].len());
        let mut parts = std::array::from_fn(|_| 0..0);
        for (part, solid) in solids.iter().enumerate() {
            let first = vertices.len() as u32;
            vertices.extend(
                voxels::mesh(solid.clone())
                    .into_iter()
                    .map(|v| ModelVertex {
                        position: v.position,
                        color: v.color,
                        normal: v.normal,
                        part: part as u32,
                        display: 0,
                        uv: [0.; 2],
                    }),
            );
            parts[part] = first..vertices.len() as u32;
        }
        // Cabin displays are contiguous with its opaque voxel range only when
        // drawn in the separate display range, so the part picker can show both.
        for (kind, points) in cabin.displays {
            rectangle(&mut vertices, Part::Cabin, kind, points);
        }
        let opaque_count = vertices.len() as u32;
        let mut panes: Vec<_> = cabin
            .panes
            .into_iter()
            .map(|(part, points)| {
                let first = vertices.len() as u32;
                rectangle(&mut vertices, part, 0, points);
                Pane {
                    first,
                    center: points.iter().sum::<Vec3>() * 0.25,
                    part,
                }
            })
            .collect();
        let prop_center = Vec3::new(0., -0.06, 2.60);
        let radius = 1.06;
        let first = vertices.len() as u32;
        rectangle(
            &mut vertices,
            Part::Body,
            9,
            [
                prop_center + Vec3::new(-radius, radius, 0.),
                prop_center + Vec3::new(radius, radius, 0.),
                prop_center + Vec3::new(radius, -radius, 0.),
                prop_center + Vec3::new(-radius, -radius, 0.),
            ],
        );
        panes.push(Pane {
            first,
            center: prop_center,
            part: Part::Body,
        });
        Geometry {
            prop: parts[Part::Prop as usize].clone(),
            parts,
            volumes,
            vertices,
            opaque_count,
            panes,
            solids: std::mem::take(solids),
        }
    })
}

pub(super) fn solid_parts() -> &'static [voxels::Voxels; Part::COUNT] {
    &geometry().solids
}

pub(super) fn fragment_part(
    cells: voxels::Voxels,
    part: Part,
    transform: Mat4,
    center: Vec3,
    flex: [f32; 2],
    out: &mut Vec<crate::destruction::Vertex>,
) {
    out.extend(voxels::mesh(cells).into_iter().map(|v| {
        let p = Vec3::from_array(v.position);
        let moved = transform.transform_point3(p);
        let wing = match part {
            Part::Body => p.x.abs() > 0.60 && (0.57..1.42).contains(&p.y) && p.z.abs() < 0.96,
            Part::LeftAileron | Part::RightAileron | Part::LeftFlap | Part::RightFlap => true,
            _ => false,
        };
        let point = if wing {
            crate::aircraft_damage::flex_point(moved, p.x, flex)
        } else {
            moved
        };
        crate::destruction::Vertex {
            position: (point - center).to_array(),
            normal: transform
                .transform_vector3(Vec3::from_array(v.normal))
                .normalize_or_zero()
                .to_array(),
            color: v.color,
        }
    }));
}

fn turn(pivot: Vec3, rotation: Mat4) -> Mat4 {
    Mat4::from_translation(pivot) * rotation * Mat4::from_translation(-pivot)
}

pub(super) fn joints(pose: Presentation, rig: Rig) -> [Mat4; Part::COUNT] {
    let [bank, pitch, throttle] = pose.controls.to_array();
    let mut joints = [Mat4::IDENTITY; Part::COUNT];
    joints[Part::Prop as usize] = turn(
        Vec3::new(0., -0.06, PROP_Z),
        Mat4::from_rotation_z(pose.prop_angle),
    );
    joints[Part::Yoke as usize] = Mat4::from_translation(Vec3::new(0., 0., -0.105 * pitch))
        * turn(cabin::YOKE_PIVOT, Mat4::from_rotation_z(0.60 * bank));
    for (part, side) in [(Part::LeftDoor, 1.), (Part::RightDoor, -1.)] {
        let pivot = Vec3::new(cabin::DOOR_PIVOT.x * side, 0., cabin::DOOR_PIVOT.z);
        let amount = if side > 0. {
            rig.doors[0]
        } else {
            rig.doors[1]
        };
        let swing = amount * amount * (3. - 2. * amount);
        joints[part as usize] = turn(pivot, Mat4::from_rotation_y(-side * 1.23 * swing));
    }
    joints[Part::LeftAileron as usize] = turn(
        Vec3::new(3.9, 1.02, AILERON_HINGE_Z),
        Mat4::from_axis_angle(
            Vec3::new(1., WING.rise / WING.span, 0.).normalize(),
            -AILERON_TRAVEL * bank,
        ),
    );
    joints[Part::RightAileron as usize] = turn(
        Vec3::new(-3.9, 1.02, AILERON_HINGE_Z),
        Mat4::from_axis_angle(
            Vec3::new(1., -WING.rise / WING.span, 0.).normalize(),
            AILERON_TRAVEL * bank,
        ),
    );
    joints[Part::Elevator as usize] = turn(
        Vec3::new(0., 0.23, ELEVATOR_HINGE_Z),
        Mat4::from_rotation_x(elevator_deflection(
            (pitch + pose.instruments.pitch_trim).clamp(-1., 1.),
        )),
    );
    joints[Part::CopilotYoke as usize] = Mat4::from_translation(Vec3::new(0., 0., -0.105 * pitch))
        * turn(
            Vec3::new(
                -cabin::YOKE_PIVOT.x,
                cabin::YOKE_PIVOT.y,
                cabin::YOKE_PIVOT.z,
            ),
            Mat4::from_rotation_z(0.60 * bank),
        );
    for side in [-1., 1.] {
        let part = if side > 0. {
            Part::LeftFlap
        } else {
            Part::RightFlap
        };
        joints[part as usize] = turn(
            Vec3::new(side * 1.75, 0.91, AILERON_HINGE_Z),
            Mat4::from_axis_angle(
                Vec3::new(1., side * WING.rise / WING.span, 0.).normalize(),
                -FLAP_TRAVEL * rig.flaps,
            ),
        );
    }
    joints[Part::Throttle as usize] = turn(
        Vec3::new(-0.094, -0.29, 0.406),
        Mat4::from_rotation_x(0.37 * throttle),
    );
    joints[Part::Rudder as usize] = turn(
        Vec3::new(0., 0., RUDDER_HINGE_Z),
        Mat4::from_rotation_y(RUDDER_TRAVEL * pose.rudder),
    );
    for (part, side) in [(Part::LeftPedal, -1.), (Part::RightPedal, 1.)] {
        joints[part as usize] = Mat4::from_translation(Vec3::Z * 0.075 * pose.rudder * side);
    }
    for (index, (strut, wheel, pivot)) in [
        (
            Part::LeftStrut,
            Part::LeftWheel,
            Vec3::new(0.94, -1.07, -0.38),
        ),
        (
            Part::RightStrut,
            Part::RightWheel,
            Vec3::new(-0.94, -1.07, -0.38),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let compression = pose.gear_compression[1 - index];
        joints[strut as usize] = turn(
            Vec3::new(0., -0.44, 0.),
            Mat4::from_scale(Vec3::new(1., 1. - compression / 0.63, 1.)),
        );
        joints[wheel as usize] = Mat4::from_translation(Vec3::Y * compression)
            * turn(pivot, Mat4::from_rotation_x(pose.wheel_turn[1 - index]));
    }
    let compression = pose.gear_compression[2];
    let nose = Vec3::new(0., -1.14, 1.63);
    let steer = if pose.instruments.grounded {
        -0.48 * pose.rudder
    } else {
        0.
    };
    let steering = turn(nose, Mat4::from_rotation_y(steer));
    joints[Part::NoseGear as usize] = steering
        * turn(
            Vec3::new(0., -0.42, 0.),
            Mat4::from_scale(Vec3::new(1., 1. - compression / 0.72, 1.)),
        );
    joints[Part::NoseWheel as usize] = Mat4::from_translation(Vec3::Y * compression)
        * steering
        * turn(nose, Mat4::from_rotation_x(pose.wheel_turn[2]));
    joints
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Rig {
    pub doors: [f32; 2],
    pub flaps: f32,
    pub cutaway: f32,
    pub glass_off: bool,
}

pub(crate) fn inspect() -> String {
    let mesh = geometry();
    let parts = Part::ALL.map(|part| {
        let range = mesh.parts[part as usize].clone();
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for v in &mesh.vertices[range.start as usize..range.end as usize] {
            let p = Vec3::from_array(v.position);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let bounds = if range.is_empty() {
            "null".into()
        } else {
            format!(r#"{{"min":{:?},"max":{:?}}}"#, lo.to_array(), hi.to_array())
        };
        format!(
            r#"{{"name":"{}","voxels":{},"triangles":{},"bounds_m":{}}}"#,
            part.name(),
            mesh.volumes[part as usize],
            range.len() / 3,
            bounds
        )
    });
    format!(
        r#"{{"model":"skyhawk","voxel_size_m":0.0625,"axes":"+X port, +Y up, +Z nose","triangles":{},"parts":[{}]}}"#,
        mesh.vertices.len() / 3,
        parts.join(",")
    )
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct View {
    matrix: [[f32; 4]; 4],
    joints: [[[f32; 4]; 4]; Part::COUNT],
    sun: [f32; 4],
    eye: [f32; 4],
    flight: [f32; 4],
    nav: [f32; 4],
    radio: [f32; 4],
    chart: [f32; 4],
    motion: [f32; 4],
    wind: [f32; 4],
    weather_display: [f32; 4],
    weather_profile: [f32; 4],
    inspection: [f32; 4],
    shadow_matrix: [[f32; 4]; 4],
}

fn radar_extent() -> wgpu::Extent3d {
    use frontier_runtime::weather::radar::{HEIGHT, WIDTH};
    wgpu::Extent3d {
        width: WIDTH,
        height: HEIGHT,
        depth_or_array_layers: 1,
    }
}

const CHART_SIZE: u32 = 64;
const SAMPLE_SPACING: f64 = 32.;

pub(crate) struct Model {
    pub samples: u32,
    chart: wgpu::Texture,
    chart_center: Option<[f64; 2]>,
    weather_chart: wgpu::Texture,
    weather_image: Option<std::sync::Arc<frontier_runtime::weather::radar::Image>>,
    weather_display: [f32; 4],
    weather_profile: [f32; 4],
    solid: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
    glass: Option<wgpu::RenderPipeline>,
    markers: wgpu::RenderPipeline,
    airflow: Option<crate::airflow_preview::Preview>,
    group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    vertices: wgpu::Buffer,
    damaged: Option<DamagedMesh>,
    pane_order: std::cell::RefCell<Vec<usize>>,
    prop_blur: std::cell::Cell<bool>,
}
impl Model {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voxel aircraft and avionics"),
            source: wgpu::ShaderSource::Wgsl(include_str!("aircraft_model.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("aircraft pose and instruments"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    count: None,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    count: None,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    count: None,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("aircraft materials"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let attributes = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x3,3=>Uint32,4=>Uint32,5=>Float32x2];
        let marker_attributes = wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4,2=>Float32x4];
        let pipeline = |entry: &str, transparent: bool| {
            let marker = entry == "fs_marker";
            let vertex_layout = [if marker {
                wgpu::VertexBufferLayout {
                    array_stride: 48,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &marker_attributes,
                }
            } else {
                wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<ModelVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                }
            }];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(if marker { "vs_marker" } else { "vs" }),
                    compilation_options: Default::default(),
                    buffers: &vertex_layout,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: transparent.then_some(wgpu::BlendState {
                            color: wgpu::BlendState::ALPHA_BLENDING.color,
                            // The opaque terrain owns the atmosphere fade in alpha.
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::Zero,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: !transparent,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                multiview: None,
                cache: None,
            })
        };
        let shadow = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voxel aircraft shadow caster"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: size_of::<ModelVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                }],
            },
            fragment: None,
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let mesh = geometry();
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("voxel aircraft mesh"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aircraft pose and instruments"),
            size: std::mem::size_of::<View>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let chart = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("north-up terrain chart"),
            size: wgpu::Extent3d {
                width: CHART_SIZE,
                height: CHART_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let weather_chart = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("weather instrument raster"),
            size: radar_extent(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let weather_view = weather_chart.create_view(&Default::default());
        let chart_view = chart.create_view(&Default::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("aircraft pose and instruments"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&chart_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&weather_view),
                },
            ],
        });
        Self {
            chart,
            chart_center: None,
            weather_chart,
            weather_image: None,
            weather_display: [0.; 4],
            weather_profile: [0.; 4],
            solid: pipeline("fs_opaque", false),
            shadow,
            markers: pipeline("fs_marker", false),
            airflow: None,
            // Raw float terrain diagnostics use a non-blendable attachment.
            glass: (format != wgpu::TextureFormat::Rgba32Float).then(|| pipeline("fs_glass", true)),
            group,
            uniform,
            vertices,
            damaged: None,
            samples,
            pane_order: std::cell::RefCell::new((0..mesh.panes.len()).collect()),
            prop_blur: Default::default(),
        }
    }

    pub(super) fn set_structure(&mut self, device: &wgpu::Device, mask: u64) {
        use crate::aircraft_damage::{live, owner, INTACT};
        if mask == INTACT {
            self.damaged = None;
            return;
        }
        if self.damaged.as_ref().is_some_and(|m| m.mask == mask) {
            return;
        }
        let original = geometry();
        let mut vertices = Vec::new();
        let mut prop = 0..0;
        for part in Part::ALL {
            let first = vertices.len() as u32;
            let cells = original.solids[part as usize]
                .iter()
                .filter(|(p, _)| live(mask, owner(part, **p)))
                .map(|(&p, &paint)| (p, paint))
                .collect();
            vertices.extend(voxels::mesh(cells).into_iter().map(|v| ModelVertex {
                position: v.position,
                color: v.color,
                normal: v.normal,
                part: part as u32,
                display: 0,
                uv: [0.; 2],
            }));
            if part == Part::Prop {
                prop = first..vertices.len() as u32;
            }
        }
        vertices.extend_from_slice(
            &original.vertices[original.parts[Part::RightFlap as usize].end as usize
                ..original.opaque_count as usize],
        );
        let opaque = vertices.len() as u32;
        let pane_starts = original
            .panes
            .iter()
            .map(|pane| {
                let first = vertices.len() as u32;
                vertices.extend_from_slice(
                    &original.vertices[pane.first as usize..pane.first as usize + 6],
                );
                first
            })
            .collect();
        self.damaged = Some(DamagedMesh {
            mask,
            prop,
            opaque,
            pane_starts,
            buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("fractured voxel airframe"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            }),
        });
    }

    pub(crate) fn update_weather_radar(
        &mut self,
        queue: &wgpu::Queue,
        image: Option<std::sync::Arc<frontier_runtime::weather::radar::Image>>,
        time: f64,
    ) {
        use frontier_runtime::weather::radar::Mode;
        let Some(image) = image else {
            self.weather_display = [0.; 4];
            self.weather_image = None;
            return;
        };
        if self
            .weather_image
            .as_ref()
            .is_none_or(|old| !std::sync::Arc::ptr_eq(old, &image))
        {
            let extent = radar_extent();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.weather_chart,
                    mip_level: 0,
                    origin: Default::default(),
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&image.pixels),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(extent.width * 4),
                    rows_per_image: Some(extent.height),
                },
                extent,
            );
            self.weather_image = Some(image.clone());
        }
        self.weather_profile = [
            image.profile_feet[0] as f32,
            image.profile_feet[1] as f32,
            0.,
            0.,
        ];
        self.weather_display = [
            image.controls.mode as u32 as f32,
            image.controls.range_nm() as f32,
            if image.controls.mode == Mode::Wind {
                image.layer_feet as f32
            } else {
                f32::from(image.controls.elevation)
            },
            image
                .product_time
                .map_or(-1., |t| (time - t).max(0.) as f32),
        ];
    }

    pub(crate) fn update_airflow(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        flow: Option<crate::aircraft::Airflow<'_>>,
    ) {
        if let Some(flow) = flow {
            self.airflow
                .get_or_insert_with(|| crate::airflow_preview::Preview::new(device))
                .update(queue, flow);
        } else if let Some(preview) = &mut self.airflow {
            preview.clear();
        }
    }

    pub(crate) fn set_preview_chart(&self, queue: &wgpu::Queue) {
        let mut pixels = Vec::with_capacity((CHART_SIZE * CHART_SIZE) as usize);
        for z in 0..CHART_SIZE {
            for x in 0..CHART_SIZE {
                let bend = 32. + 4. * (z as f32 * 0.18).sin();
                let water = (x as f32 - bend).abs() < 2.;
                let hill = (x as f32 * 0.21).sin() * (z as f32 * 0.12).cos();
                pixels.push([
                    370. + hill * 85. + x as f32 * 7.,
                    if water { 3. } else { 0. },
                ]);
            }
        }
        queue.write_texture(
            self.chart.as_image_copy(),
            bytemuck::cast_slice(&pixels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(CHART_SIZE * 8),
                rows_per_image: None,
            },
            self.chart.size(),
        );
    }

    /// Cache a world-aligned patch. The aircraft scrolls continuously inside it.
    pub(crate) fn update_chart(
        &mut self,
        world: &frontier_world::World,
        queue: &wgpu::Queue,
        position: glam::DVec3,
    ) {
        let center = [position.x, position.z].map(|v| (v / 512.).round() * 512.);
        if self
            .chart_center
            .is_some_and(|c| (position.x - c[0]).abs() < 320. && (position.z - c[1]).abs() < 320.)
        {
            return;
        }
        let mut samples = Vec::with_capacity((CHART_SIZE * CHART_SIZE) as usize);
        for row in 0..CHART_SIZE {
            for col in 0..CHART_SIZE {
                let offset = |i| (f64::from(i) - f64::from(CHART_SIZE / 2)) * SAMPLE_SPACING;
                let cell = frontier_world::Cell::from_meters([
                    center[0] + offset(col),
                    0.,
                    center[1] + offset(row),
                ]);
                let column = world.column(cell.x, cell.z);
                samples.push([
                    column.ground_height_m() as f32,
                    (column.height_m() - column.ground_height_m()) as f32,
                ]);
            }
        }
        queue.write_texture(
            self.chart.as_image_copy(),
            bytemuck::cast_slice(&samples),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(CHART_SIZE * 8),
                rows_per_image: None,
            },
            self.chart.size(),
        );
        self.chart_center = Some(center);
    }

    pub(crate) fn prepare(
        &self,
        queue: &wgpu::Queue,
        camera: Camera,
        plane: Presentation,
        aspect: f32,
        sun: Vec3,
        time: f32,
    ) {
        self.prepare_view(
            queue,
            camera,
            plane,
            aspect,
            sun,
            time,
            None,
            Rig {
                doors: [plane.door; 2],
                flaps: plane.instruments.flaps_degrees / FLAP_TRAVEL.to_degrees(),
                ..Default::default()
            },
        );
    }

    pub(crate) fn prepare_view(
        &self,
        queue: &wgpu::Queue,
        camera: Camera,
        plane: Presentation,
        aspect: f32,
        sun: Vec3,
        time: f32,
        orthographic: Option<f32>,
        rig: Rig,
    ) {
        let axes = crate::aircraft::axes(plane.pose);
        self.prop_blur
            .set(plane.engine_running && self.glass.is_some());
        let eye = axes.transpose() * (camera.position - plane.pose.position).as_vec3();
        let forward = axes.transpose() * camera.direction();
        let joints = joints(plane, rig);
        let panes = &geometry().panes;
        let depth = |i: usize| {
            (joints[panes[i].part as usize].transform_point3(panes[i].center) - eye).dot(forward)
        };
        self.pane_order
            .borrow_mut()
            .sort_by(|&p, &q| depth(q).total_cmp(&depth(p)));
        let meters = plane.instruments;
        let center = self.chart_center.unwrap_or_default();
        let matrix = if let Some(height) = orthographic {
            let top = height * 0.5;
            let right = top * aspect;
            Mat4::orthographic_rh(-right, right, -top, top, 0.025, camera.far)
                * Mat4::look_to_rh(
                    (camera.position - plane.pose.position).as_vec3(),
                    camera.direction(),
                    camera.view_up(),
                )
        } else {
            camera.matrix(plane.pose.position, aspect)
        };
        let data = View {
            shadow_matrix: Mat4::IDENTITY.to_cols_array_2d(),
            inspection: [
                rig.cutaway,
                f32::from(rig.glass_off),
                plane.wing_flex[0],
                plane.wing_flex[1],
            ],
            motion: [plane.turn_rate, meters.throttle, 0., 0.],
            wind: meters.wind.extend(0.).to_array(),
            weather_display: self.weather_display,
            weather_profile: self.weather_profile,
            chart: [
                (plane.pose.position.x - center[0]) as f32,
                (plane.pose.position.z - center[1]) as f32,
                SAMPLE_SPACING as f32,
                1280.,
            ],
            matrix: (matrix * Mat4::from_mat3(axes)).to_cols_array_2d(),
            joints: joints.map(|joint| joint.to_cols_array_2d()),
            sun: (axes.transpose() * sun).extend(time).to_array(),
            eye: eye.extend(0.).to_array(),
            flight: [
                meters.speed_mps * 3.6,
                meters.altitude_m as f32,
                meters.bank_degrees.to_radians(),
                meters.pitch_degrees.to_radians(),
            ],
            nav: [
                meters.heading_degrees,
                meters.climb_mps,
                plane.home_relative_radians,
                plane.home_distance_km,
            ],
            radio: [
                plane.radio_khz as f32,
                if plane.hold { 1. } else { 0. },
                if meters.stall_warning { 1. } else { 0. },
                if meters.grounded { 1. } else { 0. },
            ],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&data));
    }

    pub(crate) fn draw_shadow(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::TextureView,
        projection: Mat4,
    ) {
        queue.write_buffer(
            &self.uniform,
            std::mem::offset_of!(View, shadow_matrix) as u64,
            bytemuck::cast_slice(&projection.to_cols_array()),
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("aircraft shadow depth"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        let (vertices, opaque) = self.damaged.as_ref().map_or_else(
            || (&self.vertices, geometry().opaque_count),
            |mesh| (&mesh.buffer, mesh.opaque),
        );
        pass.set_pipeline(&self.shadow);
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.draw(0..opaque, 0..1);
    }

    pub(crate) fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        self.draw_part(pass, None);
    }

    pub(crate) fn draw_part<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, part: Option<Part>) {
        let mesh = geometry();
        let damage = self.damaged.as_ref().filter(|_| part.is_none());
        let buffer = damage.map_or(&self.vertices, |m| &m.buffer);
        let opaque = damage.map_or(mesh.opaque_count, |m| m.opaque);
        let prop = damage.map_or(&mesh.prop, |m| &m.prop);
        pass.set_pipeline(&self.solid);
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_vertex_buffer(0, buffer.slice(..));
        if let Some(part) = part {
            if part as usize != Part::Prop as usize || !self.prop_blur.get() {
                pass.draw(mesh.parts[part as usize].clone(), 0..1);
            }
            if part as usize == Part::Cabin as usize {
                pass.draw(
                    mesh.parts[Part::RightFlap as usize].end..mesh.opaque_count,
                    0..1,
                );
            }
        } else if self.prop_blur.get() {
            pass.draw(0..prop.start, 0..1);
            pass.draw(prop.end..opaque, 0..1);
        } else {
            pass.draw(0..opaque, 0..1);
        }
        if let Some(airflow) = self.airflow.as_ref().filter(|_| part.is_none()) {
            airflow.draw(pass, &self.markers);
        }
        if let Some(glass) = &self.glass {
            pass.set_pipeline(glass);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.set_bind_group(0, &self.group, &[]);
            for &index in self.pane_order.borrow().iter() {
                let is_prop = index + 1 == mesh.panes.len();
                let owner = if is_prop {
                    Part::Prop
                } else {
                    mesh.panes[index].part
                };
                if part.is_some_and(|part| part as usize != owner as usize)
                    || (is_prop && !self.prop_blur.get())
                    || damage.is_some_and(|m| {
                        !crate::aircraft_damage::live(
                            m.mask,
                            crate::aircraft_damage::owner(
                                owner,
                                (mesh.panes[index].center / cabin::EDGE).floor().as_ivec3(),
                            ),
                        )
                    })
                {
                    continue;
                }
                let first = damage.map_or(mesh.panes[index].first, |m| m.pane_starts[index]);
                pass.draw(first..first + 6, 0..1);
            }
        }
    }
}

#[cfg(test)]
mod shader_tests {
    #[test]
    fn aircraft_avionics_and_airflow_shader_validates() {
        let source = include_str!("aircraft_model.wgsl");
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
    }
}
