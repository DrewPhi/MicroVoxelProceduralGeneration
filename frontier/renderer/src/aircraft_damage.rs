//! Aircraft structural description and its mapping to actual model-local voxels.
use crate::aircraft_model::{da62, ModelKind};
use crate::{
    aircraft,
    aircraft_model::{self, Part},
    destruction,
};
use frontier_runtime::structure::{Joint, Load, Member, Node, Structure};
use glam::{DVec3, IVec3, Mat3, Mat4, Quat, Vec3};
use std::sync::{Arc, OnceLock};

use aircraft::airframe::{FIN_HEIGHT, FIN_ROOT, TAIL, WING};

pub const ENGINES: usize = 36;
pub const REAR_DOOR: usize = 38;
pub const COUNT: usize = 39;
pub const INTACT: u64 = (1 << ENGINES) - 1;
pub fn intact(kind: ModelKind) -> u64 {
    if kind == ModelKind::Da62 {
        (1 << COUNT) - 1
    } else {
        INTACT
    }
}
pub const TAILCONE: usize = 17;
pub const TAIL_FIRST: usize = 18;
pub const FIN_FIRST: usize = 26;
pub const NOSE: usize = 30;
pub const GEAR_FIRST: usize = 31;
const DOOR_FIRST: usize = 34;
const EDGE: f32 = 0.0625;

pub fn live(mask: u64, node: usize) -> bool {
    mask & (1 << node) != 0
}

pub fn section_node(index: usize) -> usize {
    match index {
        0..16 => 1 + index,
        16..24 => TAIL_FIRST + index - 16,
        _ => FIN_FIRST + index - 24,
    }
}

pub fn wing_flex(kind: ModelKind, structure: &Structure<COUNT>, active: u64) -> [f32; 2] {
    let model = asset_for(kind);
    let span = wing_span(kind);
    let dx = span / 8.;
    std::array::from_fn(|side| {
        let sign = if side == 0 { 1. } else { -1. };
        (0..8)
            .filter(|i| live(active, 1 + side * 8 + i))
            .map(|i| {
                let n = 1 + side * 8 + i;
                let load = structure.reaction(n);
                let node = model.structure[n];
                let joint = node.joint.unwrap();
                let moment = load.moment + (node.center - joint.anchor).cross(load.force);
                let u = i as f32 / 8.;
                let rigidity = if kind == ModelKind::Da62 { 2.3 } else { 1. }
                    * (1_580_000. * (1. - u).powi(2) + 185_000.);
                sign * moment.z / rigidity * (span - (i as f32 + 0.5) * dx) * dx
            })
            .sum::<f32>()
            .clamp(-0.18, 0.30)
    })
}

fn wing_span(kind: ModelKind) -> f32 {
    if kind == ModelKind::Da62 {
        da62::SPAN
    } else {
        WING.span
    }
}

pub fn flex_point(kind: ModelKind, mut p: Vec3, x: f32, flex: [f32; 2]) -> Vec3 {
    let span = ((x.abs() - 0.6) / (wing_span(kind) - 0.6)).clamp(0., 1.);
    p.y += flex[usize::from(x < 0.)] * span * span;
    p
}

fn strip(p: Vec3, root: usize, count: usize, span: f32) -> usize {
    root + usize::from(p.x < 0.) * count
        + ((p.x.abs() / span * count as f32) as usize).min(count - 1)
}

pub fn owner(part: Part, cell: IVec3) -> usize {
    let p = (cell.as_vec3() + Vec3::splat(0.5)) * EDGE;
    match part {
        Part::Prop => NOSE,
        Part::LeftDoor => DOOR_FIRST,
        Part::RightDoor => DOOR_FIRST + 1,
        Part::LeftFlap | Part::RightFlap | Part::LeftAileron | Part::RightAileron => {
            strip(p, 1, 8, WING.span)
        }
        Part::Elevator => strip(p, TAIL_FIRST, 4, TAIL.span),
        Part::Rudder => fin(p),
        Part::RightWheel | Part::RightStrut => GEAR_FIRST,
        Part::LeftWheel | Part::LeftStrut => GEAR_FIRST + 1,
        Part::NoseWheel | Part::NoseGear => GEAR_FIRST + 2,
        Part::Body => {
            let (chord, center, y) = WING.section((p.x.abs() / WING.span).min(1.));
            if p.x.abs() > 0.60
                && (p.y - y).abs() < 0.23
                && (p.z - center).abs() < chord * 0.5 + EDGE
            {
                strip(p, 1, 8, WING.span)
            } else if p.z > 1.19 {
                NOSE
            } else if p.z < -3.78 && p.y > 0.32 && p.x.abs() < 0.18 {
                fin(p)
            } else if p.z < -3.78 && (p.y - 0.22).abs() < 0.12 && p.x.abs() > 0.25 {
                strip(p, TAIL_FIRST, 4, TAIL.span)
            } else if p.z < -2.04 {
                TAILCONE
            } else {
                0
            }
        }
        _ => 0,
    }
}

pub(super) fn owner_for(kind: ModelKind, part: Part, cell: IVec3) -> usize {
    if kind == ModelKind::Skyhawk {
        return owner(part, cell);
    }
    da62_owner(part, (cell.as_vec3() + 0.5) * EDGE)
}

pub(super) fn da62_owner(part: Part, p: Vec3) -> usize {
    let x = p.x.abs();
    let side = usize::from(p.x < 0.);
    let tail = || strip(p, TAIL_FIRST, 4, da62::TAIL_SPAN);
    let fin = || FIN_FIRST + (((p.y - da62::FIN_ROOT) / da62::FIN_HEIGHT * 4.) as usize).min(3);
    match part {
        Part::Prop => ENGINES,
        Part::RightProp => ENGINES + 1,
        Part::LeftDoor => DOOR_FIRST,
        Part::RightDoor => DOOR_FIRST + 1,
        Part::RearDoor => REAR_DOOR,
        Part::LeftFlap | Part::RightFlap | Part::LeftAileron | Part::RightAileron => {
            strip(p, 1, 8, da62::SPAN)
        }
        Part::Elevator => tail(),
        Part::Rudder => fin(),
        Part::RightWheel | Part::RightStrut => GEAR_FIRST,
        Part::LeftWheel | Part::LeftStrut => GEAR_FIRST + 1,
        Part::NoseWheel | Part::NoseGear => GEAR_FIRST + 2,
        Part::Body => {
            if p.z < -4.70 && p.y >= 1.29 && x >= 0.095 {
                return tail();
            }
            if p.z < -3.65 && p.y > 0.39 && x < 0.16 {
                return fin();
            }
            if (1.37..2.13).contains(&x) && (-1.88..1.46).contains(&p.z) {
                let [lead, chord, height] = da62::surface(false, x / da62::SPAN);
                let outside_wing =
                    !(lead - chord..lead).contains(&p.z) || (p.y - height).abs() > 0.145;
                if (-0.47..0.34).contains(&p.y) && outside_wing {
                    return ENGINES + side;
                }
            }
            if x > 0.72 && p.z > -2. && p.z < 1.54 {
                return strip(p, 1, 8, da62::SPAN);
            }
            if p.z > 1.17 {
                return NOSE;
            }
            if p.z < -3.05 {
                return TAILCONE;
            }
            0
        }
        _ => 0,
    }
}

fn fin(p: Vec3) -> usize {
    FIN_FIRST + (((p.y - FIN_ROOT).max(0.) / FIN_HEIGHT * 4.) as usize).min(3)
}

pub struct Asset {
    pub structure: [Node; COUNT],
    pub order: [usize; COUNT - 1],
    pub probes: [Vec<Vec3>; COUNT],
    pub inertia: [Mat3; COUNT],
    cells: [Vec<(Part, IVec3, [f32; 3])>; COUNT],
}

fn joint(kind: ModelKind, node: usize, p: Vec3) -> Option<Joint> {
    let twin = kind == ModelKind::Da62;
    let (mut parent, mut anchor, axes, mut forces, mut moments, toughness) = match node {
        0 => return None,
        1..17 => {
            let index = (node - 1) % 8;
            let u = index as f32 / 8.;
            let side = p.x.signum();
            let (_, z, y) = WING.section(u);
            let taper = (1. - u).powf(1.45) + 0.035;
            (
                if index == 0 { 0 } else { node - 1 },
                Vec3::new(side * (u * WING.span).max(0.55), y, z),
                Mat3::IDENTITY,
                Vec3::new(29_000., 29_000., 21_000.) * taper,
                Vec3::new(12_000., 24_000., 39_000.) * taper,
                0.24,
            )
        }
        TAILCONE => (
            0,
            Vec3::new(0., 0.06, -2.04),
            Mat3::IDENTITY,
            Vec3::splat(28_000.),
            Vec3::splat(18_000.),
            0.23,
        ),
        TAIL_FIRST..FIN_FIRST => {
            let index = (node - TAIL_FIRST) % 4;
            let u = index as f32 / 4.;
            let (_, z, y) = TAIL.section(u);
            let taper = 1. - 0.82 * u;
            (
                if index == 0 { TAILCONE } else { node - 1 },
                Vec3::new(p.x.signum() * u * TAIL.span, y, z),
                Mat3::IDENTITY,
                Vec3::splat(8_000. * taper),
                Vec3::splat(3_300. * taper),
                0.18,
            )
        }
        FIN_FIRST..NOSE => {
            let u = (node - FIN_FIRST) as f32 / 4.;
            let taper = 1. - 0.82 * u;
            (
                if node == FIN_FIRST {
                    TAILCONE
                } else {
                    node - 1
                },
                Vec3::new(0., FIN_ROOT + u * FIN_HEIGHT, -4.9),
                Mat3::from_rotation_z(std::f32::consts::FRAC_PI_2),
                Vec3::splat(6_800. * taper),
                Vec3::splat(2_600. * taper),
                0.16,
            )
        }
        NOSE => (
            0,
            Vec3::new(0., -0.04, 1.19),
            Mat3::IDENTITY,
            Vec3::splat(66_000.),
            Vec3::splat(34_000.),
            0.21,
        ),
        GEAR_FIRST..DOOR_FIRST => (
            0,
            Vec3::new(p.x * 0.57, -0.48, p.z),
            Mat3::IDENTITY,
            Vec3::new(19_000., 49_000., 19_000.),
            Vec3::splat(12_000.),
            0.16,
        ),
        ENGINES..REAR_DOOR => (
            strip(p, 1, 8, da62::SPAN),
            da62::PROP.with_x(p.x),
            Mat3::IDENTITY,
            Vec3::splat(48_000.),
            Vec3::splat(27_000.),
            0.19,
        ),
        _ => (
            0,
            Vec3::new(p.x, 0.16, 0.35),
            Mat3::IDENTITY,
            Vec3::splat(9_500.),
            Vec3::splat(4_100.),
            0.14,
        ),
    };
    if twin {
        match node {
            1..17 => {
                let u = ((node - 1) % 8) as f32 / 8.;
                let [lead, chord, y] = da62::surface(false, u);
                anchor = Vec3::new(
                    p.x.signum() * (u * da62::SPAN).max(0.71),
                    y,
                    lead - 0.45 * chord,
                );
            }
            TAILCONE => anchor = Vec3::new(0., 0.04, -3.05),
            TAIL_FIRST..FIN_FIRST => {
                let u = ((node - TAIL_FIRST) % 4) as f32 / 4.;
                let [lead, chord, y] = da62::surface(true, u);
                anchor = Vec3::new(p.x.signum() * u * da62::TAIL_SPAN, y, lead - 0.45 * chord);
                if u == 0. {
                    parent = NOSE - 1;
                }
            }
            FIN_FIRST..NOSE => {
                anchor = Vec3::new(
                    0.,
                    da62::FIN_ROOT + ((node - FIN_FIRST) as f32 / 4.) * da62::FIN_HEIGHT,
                    -5.30,
                )
            }
            GEAR_FIRST..=32 => parent = ENGINES + usize::from(p.x < 0.),
            _ => {}
        }
        if node < ENGINES {
            forces *= 1.85;
            moments *= 2.15;
        }
    }
    Some(Joint {
        parent,
        anchor,
        axes,
        // Spars carry the bulk of the load. The lower-toughness skin can
        // yield first, transferring its load to the structural members.
        members: std::array::from_fn(|i| {
            let stiffness = [0.53, 0.32, 0.15][i];
            let strength = [0.57, 0.34, 0.105][i];
            Member {
                force: forces * strength,
                moment: moments * strength,
                stiffness,
                toughness: toughness * [1.15, 0.87, 0.43][i],
            }
        }),
    })
}

fn mass(kind: ModelKind, node: usize) -> f32 {
    if kind == ModelKind::Da62 {
        return match node {
            0 => 0.,
            1..17 => [24., 31., 29., 19., 17., 12., 8., 5.][(node - 1) % 8],
            TAILCONE => 72.,
            TAIL_FIRST..FIN_FIRST => 4.4,
            FIN_FIRST..NOSE => 5.,
            NOSE => 58.,
            GEAR_FIRST..DOOR_FIRST => 22.,
            ENGINES..REAR_DOOR => 201.,
            _ => 12.,
        };
    }
    match node {
        0 => 0.,
        1..17 => [11., 21., 17., 10., 7., 6., 5., 4.][(node - 1) % 8],
        TAILCONE => 42.,
        TAIL_FIRST..FIN_FIRST => 2.4,
        FIN_FIRST..NOSE => 2.5,
        NOSE => 135.,
        GEAR_FIRST..DOOR_FIRST => 13.,
        _ => 9.,
    }
}

#[cfg(test)]
pub fn asset() -> &'static Asset {
    asset_for(ModelKind::Skyhawk)
}

pub fn asset_for(kind: ModelKind) -> &'static Asset {
    static ASSET: OnceLock<Asset> = OnceLock::new();
    static DIAMOND: OnceLock<Asset> = OnceLock::new();
    let cache = if kind == ModelKind::Da62 {
        &DIAMOND
    } else {
        &ASSET
    };
    cache.get_or_init(|| {
        let solids = aircraft_model::structural_cells(kind);
        let mut points: [Vec<Vec3>; COUNT] = std::array::from_fn(|_| Vec::new());
        let mut groups: [Vec<(Part, IVec3, [f32; 3])>; COUNT] = std::array::from_fn(|_| Vec::new());
        for part in Part::ALL {
            let mut cells: Vec<_> = solids[part as usize].keys().copied().collect();
            cells.sort_unstable_by_key(|p| (p.x, p.y, p.z));
            for cell in cells {
                let owner = owner_for(kind, part, cell);
                points[owner].push((cell.as_vec3() + Vec3::splat(0.5)) * EDGE);
                groups[owner].push((part, cell, solids[part as usize][&cell]));
            }
        }
        let structure = std::array::from_fn(|i| {
            if !live(intact(kind), i) {
                return Node {
                    center: Vec3::ZERO,
                    mass: 0.,
                    joint: None,
                };
            }
            assert!(!points[i].is_empty(), "empty structural assembly {i}");
            let center = points[i].iter().sum::<Vec3>() / points[i].len() as f32;
            Node {
                center,
                mass: mass(kind, i),
                joint: joint(kind, i, center),
            }
        });
        let inertia = std::array::from_fn(|i| {
            let node = structure[i];
            let voxel_mass = node.mass / points[i].len().max(1) as f32;
            points[i].iter().fold(
                Mat3::from_diagonal(Vec3::splat(node.mass * EDGE * EDGE / 6.)),
                |m, p| m + point_inertia(*p - node.center) * voxel_mass,
            )
        });
        let probes = std::array::from_fn(|i| {
            let xs = &points[i];
            support_points(xs)
        });
        let mut order = std::array::from_fn(|i| i + 1);
        if kind == ModelKind::Da62 {
            let depth = |mut i: usize| {
                let mut steps = 0;
                while let Some(joint) = structure[i].joint {
                    i = joint.parent;
                    steps += 1;
                    assert!(steps < COUNT, "cycle in DA62 attachments");
                }
                steps
            };
            order.sort_by_key(|&i| depth(i));
        }
        Asset {
            structure,
            order,
            probes,
            inertia,
            cells: groups,
        }
    })
}

fn support_points(xs: &[Vec3]) -> Vec<Vec3> {
    let mut probes = Vec::new();
    for direction in [
        Vec3::X,
        -Vec3::X,
        Vec3::Y,
        -Vec3::Y,
        Vec3::Z,
        -Vec3::Z,
        Vec3::new(1., -1., 0.),
        Vec3::new(-1., -1., 0.),
        Vec3::new(0., -1., 1.),
        Vec3::new(0., -1., -1.),
    ] {
        if let Some(p) = xs
            .iter()
            .max_by(|a, b| a.dot(direction).total_cmp(&b.dot(direction)))
        {
            let point = *p + direction.signum() * (EDGE * 0.49) * direction.abs().min(Vec3::ONE);
            if !probes.contains(&point) {
                probes.push(point);
            }
        }
    }
    probes
}

pub fn point_inertia(r: Vec3) -> Mat3 {
    Mat3::from_diagonal(Vec3::splat(r.length_squared()))
        - Mat3::from_cols(r * r.x, r * r.y, r * r.z)
}

pub struct Damage {
    pub kind: ModelKind,
    pub active: u64,
    pub structure: Structure<COUNT>,
    pub fragments: Vec<destruction::Fragment>,
    pub impacts: Vec<destruction::Impact>,
    pub clearance: destruction::Clearance,
    pub flex: [f32; 2],
    pub peak_seen: f32,
}

impl Default for Damage {
    fn default() -> Self {
        Self {
            kind: ModelKind::Skyhawk,
            active: INTACT,
            structure: Structure::default(),
            fragments: Vec::new(),
            impacts: Vec::new(),
            clearance: Default::default(),
            flex: [0.; 2],
            peak_seen: 0.,
        }
    }
}

pub struct FragmentStructure {
    kind: ModelKind,
    mask: u64,
    structure: Structure<COUNT>,
    joints: [Mat4; Part::COUNT],
    root_mass: f32,
    root_center: Vec3,
    excluded: Vec<Chip>,
    flex: [f32; 2],
}

#[derive(Clone, Copy)]
struct Chip {
    node: usize,
    part: Part,
    cell: IVec3,
    color: [f32; 3],
    center: Vec3,
    mass: f32,
}

pub fn pose_joints(pose: aircraft::Presentation) -> [Mat4; Part::COUNT] {
    pose.kind.joints(pose, aircraft_model::Rig::flight(pose))
}

impl FragmentStructure {
    fn create(
        &self,
        mask: u64,
        origin: DVec3,
        orientation: Quat,
        cg_world: DVec3,
        velocity: Vec3,
        angular: Vec3,
    ) -> destruction::Fragment {
        let asset = asset_for(self.kind);
        let mass_of = |i: usize| {
            if i == 0 {
                self.root_mass
            } else {
                asset.structure[i].mass
            }
        };
        let members = || (0..COUNT).filter(|&i| live(mask, i));
        let center_of = |i: usize| {
            if i == 0 {
                self.root_center
            } else {
                asset.structure[i].center
            }
        };
        let excluded = || self.excluded.iter().filter(|chip| live(mask, chip.node));
        let mass =
            members().map(mass_of).sum::<f32>() - excluded().map(|chip| chip.mass).sum::<f32>();
        let center = (members().map(|i| center_of(i) * mass_of(i)).sum::<Vec3>()
            - excluded().map(|chip| chip.center * chip.mass).sum::<Vec3>())
            / mass;
        let inertia = members().fold(Mat3::from_diagonal(Vec3::splat(mass * 0.010)), |m, i| {
            let r = center_of(i) - center;
            let local = if i == 0 {
                Mat3::from_diagonal(Vec3::new(2., 1.7, 0.52) * mass_of(i))
            } else {
                asset.inertia[i]
            };
            m + local + point_inertia(r) * mass_of(i)
        }) - excluded().fold(Mat3::ZERO, |m, chip| {
            m + point_inertia(chip.center - center) * chip.mass
        });
        let mut positions = Vec::new();
        let mut vertices = Vec::new();
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        let mut parts: [std::collections::HashMap<IVec3, [f32; 3]>; Part::COUNT] =
            std::array::from_fn(|_| std::collections::HashMap::new());
        for i in members() {
            for &(part, cell, paint) in &asset.cells[i] {
                if !self
                    .excluded
                    .iter()
                    .any(|chip| chip.part == part && chip.cell == cell)
                {
                    parts[part as usize].insert(cell, paint);
                }
            }
        }
        for part in Part::ALL {
            let cells = std::mem::take(&mut parts[part as usize]);
            if cells.is_empty() {
                continue;
            }
            let joint = self.joints[part as usize];
            let mut coords: Vec<_> = cells.keys().copied().collect();
            coords.sort_unstable_by_key(|p| (p.x, p.y, p.z));
            for p in coords {
                let rest = (p.as_vec3() + Vec3::splat(0.5)) * EDGE;
                let moved = joint.transform_point3(rest);
                let point = if (1..17).contains(&owner_for(self.kind, part, p)) {
                    flex_point(self.kind, moved, rest.x, self.flex)
                } else {
                    moved
                } - center;
                lo = lo.min(point);
                hi = hi.max(point);
                positions.push(point);
            }
            // Coarse occupancy represents the contact shape of fine-only gear and
            // propellers. Render those parts from their own detail lattice.
            let cells = if self.kind == ModelKind::Da62 {
                let original = &aircraft_model::solid_parts_for(self.kind)[part as usize];
                cells
                    .into_iter()
                    .filter(|(p, _)| original.contains_key(p))
                    .collect()
            } else {
                cells
            };
            aircraft_model::fragment_part(
                self.kind,
                cells,
                part,
                joint,
                center,
                self.flex,
                &mut vertices,
            );
        }
        if self.kind == ModelKind::Da62 {
            aircraft_model::da62_fragment_details(
                mask,
                &self.joints,
                center,
                self.flex,
                &mut vertices,
            );
        }
        // Build contact extrema for each occupied segment, preserving the
        // leading and trailing edges of long, slender fragments.
        let mut probes = Vec::new();
        let size = hi - lo;
        let axis = (0..3).max_by(|&a, &b| size[a].total_cmp(&size[b])).unwrap();
        let width = (hi[axis] - lo[axis]).max(EDGE);
        let count = (width / 0.75).ceil().clamp(1., 12.) as usize;
        let mut groups = vec![Vec::new(); count];
        for p in positions {
            let group = (((p[axis] - lo[axis]) / width * count as f32) as usize).min(count - 1);
            groups[group].push(p);
        }
        for points in &groups {
            probes.extend(support_points(points));
        }
        let world_center = origin + (orientation * center).as_dvec3();
        destruction::Fragment {
            pose: destruction::Pose {
                center: world_center,
                orientation,
            },
            velocity: velocity + angular.cross((world_center - cg_world).as_vec3()),
            angular,
            shape: Arc::new(destruction::Shape {
                pressure_centers: destruction::pressure_centers(&vertices),
                vertices,
                probes,
                mass,
                center,
                size: hi - lo + Vec3::splat(EDGE),
                inverse_inertia: inertia.inverse(),
            }),
            source: Some(Self {
                kind: self.kind,
                mask,
                structure: self.structure.clone(),
                joints: self.joints,
                root_mass: self.root_mass,
                root_center: self.root_center,
                excluded: excluded().copied().collect(),
                flex: self.flex,
            }),
            sleep: 0.,
            clock: 0.,
            clearance: Default::default(),
        }
    }

    fn detach(
        &mut self,
        mask: u64,
        origin: DVec3,
        orientation: Quat,
        cg_world: DVec3,
        velocity: Vec3,
        angular: Vec3,
        chips_out: &mut Vec<destruction::Fragment>,
    ) -> destruction::Fragment {
        let asset = asset_for(self.kind);
        let node = [0]
            .into_iter()
            .chain(asset.order)
            .find(|&i| live(mask, i))
            .unwrap();
        let mut chips = Vec::new();
        let mut direction = Vec3::ZERO;
        let mut speed = 0.;
        if let Some(joint) = asset.structure[node].joint {
            let reaction = self.structure.reaction(node);
            let axis = (asset.structure[node].center - joint.anchor).normalize_or_zero();
            direction = (reaction.moment.cross(axis) + reaction.force * 0.3).normalize_or_zero();
            if direction != Vec3::ZERO {
                let stress = reaction.force.length() + reaction.moment.length();
                let count = (stress / 14_000.).ceil().clamp(1., 4.) as usize;
                speed = (stress * 0.000075).sqrt().clamp(0.65, 3.5);
                let mut candidates = Vec::new();
                for &(part, cell, color) in &asset.cells[node] {
                    let cells = &aircraft_model::structural_cells(self.kind)[part as usize];
                    if self
                        .excluded
                        .iter()
                        .any(|c| c.part == part && c.cell == cell)
                    {
                        continue;
                    }
                    // The thin, exposed strip on the pulling side of the
                    // ruptured joint provides the ejectable voxels.
                    if [IVec3::X, IVec3::Y, IVec3::Z]
                        .into_iter()
                        .all(|d| cells.contains_key(&(cell + d)) && cells.contains_key(&(cell - d)))
                    {
                        continue;
                    }
                    let rest = (cell.as_vec3() + Vec3::splat(0.5)) * EDGE;
                    let moved = self.joints[part as usize].transform_point3(rest);
                    let center = if (1..17).contains(&node) {
                        flex_point(self.kind, moved, rest.x, self.flex)
                    } else {
                        moved
                    };
                    let delta = center - joint.anchor;
                    let score = delta.dot(axis).abs() - 0.18 * delta.dot(direction);
                    candidates.push((
                        score,
                        Chip {
                            node,
                            part,
                            cell,
                            color,
                            center,
                            mass: asset.structure[node].mass / asset.cells[node].len() as f32,
                        },
                    ));
                }
                candidates.sort_unstable_by(|(a, p), (b, q)| {
                    a.total_cmp(b).then_with(|| {
                        (p.part as usize, p.cell.to_array())
                            .cmp(&(q.part as usize, q.cell.to_array()))
                    })
                });
                chips.extend(candidates.into_iter().take(count).map(|(_, c)| c));
                self.excluded.extend(chips.iter().copied());
            }
        }
        let mut body = self.create(mask, origin, orientation, cg_world, velocity, angular);
        let mut recoil = Vec3::ZERO;
        let mut torque = Vec3::ZERO;
        for (index, chip) in chips.into_iter().enumerate() {
            let center = origin + (orientation * chip.center).as_dvec3();
            let ejection = orientation * direction * (speed * (0.74 + 0.12 * index as f32));
            recoil += ejection * chip.mass;
            torque += (center - body.pose.center)
                .as_vec3()
                .cross(ejection * chip.mass);
            chips_out.push(destruction::Fragment::voxel(
                destruction::Pose {
                    center,
                    orientation,
                },
                velocity + angular.cross((center - cg_world).as_vec3()) + ejection,
                angular,
                EDGE,
                chip.mass,
                chip.color,
            ));
        }
        body.velocity -= recoil / body.shape.mass;
        body.angular -=
            orientation * (body.shape.inverse_inertia * (orientation.conjugate() * torque));
        body
    }
}

pub fn separate(
    mask: u64,
    damage: &mut Damage,
    pose: aircraft::Presentation,
    cg: Vec3,
    mass: f32,
    velocity: Vec3,
    omega: Vec3,
) {
    let axes = aircraft::axes(pose.pose);
    let orientation = Quat::from_mat3(&axes);
    let origin = pose.pose.position;
    let cg_world = origin + (axes * cg).as_dvec3();
    let members = asset_for(pose.kind)
        .structure
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(i, _)| live(damage.active, *i));
    let root_mass = mass - members.clone().map(|(_, n)| n.mass).sum::<f32>();
    let root_center =
        (cg * mass - members.map(|(_, n)| n.center * n.mass).sum::<Vec3>()) / root_mass;
    let mut source = FragmentStructure {
        kind: pose.kind,
        mask,
        structure: damage.structure.clone(),
        joints: pose_joints(pose),
        root_mass,
        root_center,
        excluded: Vec::new(),
        flex: pose.wing_flex,
    };
    let body = source.detach(
        mask,
        origin,
        orientation,
        cg_world,
        velocity,
        axes * omega,
        &mut damage.fragments,
    );
    damage.fragments.push(body);
}

pub fn fracture_fragment(
    body: &mut destruction::Fragment,
    hit: destruction::Impact,
    spawned: &mut Vec<destruction::Fragment>,
) {
    let Some(source) = &mut body.source else {
        return;
    };
    if source.mask.count_ones() <= 1 {
        return;
    }
    let orientation = body.pose.orientation;
    let point = orientation.conjugate() * (hit.contact.point - body.pose.center).as_vec3()
        + body.shape.center;
    let impulse = orientation.conjugate() * hit.impulse;
    let asset = asset_for(source.kind);
    let node = (0..COUNT)
        .filter(|&i| live(source.mask, i))
        .min_by(|&a, &b| {
            asset.structure[a]
                .center
                .distance_squared(point)
                .total_cmp(&asset.structure[b].center.distance_squared(point))
        })
        .unwrap();
    const DURATION: f32 = 0.018;
    let mut loads = [Load::default(); COUNT];
    for (i, n) in asset
        .structure
        .iter()
        .enumerate()
        .filter(|(i, _)| live(source.mask, *i))
    {
        let mass = if i == 0 { source.root_mass } else { n.mass };
        loads[i].force -= impulse * (mass / body.shape.mass / DURATION);
    }
    loads[node].apply(impulse / DURATION, point - asset.structure[node].center);
    let mut components = Vec::new();
    if !source.structure.solve_in_order(
        &asset.structure,
        source.mask,
        &loads,
        DURATION,
        &mut components,
        asset.order.iter().copied(),
    ) {
        return;
    }
    let origin = body.pose.center - (orientation * body.shape.center).as_dvec3();
    let mut masks = components.into_iter();
    let root = masks
        .next()
        .expect("at least one component survives a fracture");
    for mask in masks {
        let fragment = source.detach(
            mask,
            origin,
            orientation,
            body.pose.center,
            body.velocity,
            body.angular,
            spawned,
        );
        spawned.push(fragment);
    }
    let mut first = source.create(
        root,
        origin,
        orientation,
        body.pose.center,
        body.velocity,
        body.angular,
    );
    first.clock = body.clock;
    *body = first;
}
