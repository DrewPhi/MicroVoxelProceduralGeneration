//! Free voxel fragments and their interaction with canonical terrain. World
//! positions stay in f64; collision offsets and rigid-body state use metres.
use bytemuck::{Pod, Zeroable};
use frontier_world::{Cell, Material, RayHit, World};
use glam::{DVec3, Mat3, Quat, Vec3};
use std::{collections::BTreeSet, sync::Arc};

pub const GRAVITY: f32 = 9.80665;
const CONTACT_SKIN: f64 = 0.002;

#[derive(Default)]
pub struct Clearance {
    // Inner tile plus an overlap for moving and rotating bodies.
    tile: Option<([i64; 2], u64, f64)>,
}

impl Clearance {
    pub fn sky(&mut self, world: &World, from: DVec3, to: DVec3, radius: f64) -> bool {
        use frontier_world::{
            affine_bounds::{bound_native_height, ColumnBox},
            vegetation,
        };
        let Some(terrain) = world
            .fbm_terrain()
            .filter(|_| world.topology_base_cells().is_none())
        else {
            return false;
        };
        let edge = frontier_world::voxel_size();
        let side = (64. / edge).ceil() as i64;
        let margin = (16. / edge).ceil() as i64;
        let center = Cell::from_meters(from.to_array());
        let tile = [center.x.div_euclid(side), center.z.div_euclid(side)];
        let min = tile.map(|t| t * side - margin);
        let max = min.map(|v| v + side + margin * 2);
        let inside = from.min(to) - radius;
        let outside = from.max(to) + radius;
        if inside.x < min[0] as f64 * edge
            || inside.z < min[1] as f64 * edge
            || outside.x > max[0] as f64 * edge
            || outside.z > max[1] as f64 * edge
        {
            return false;
        }
        if self
            .tile
            .is_none_or(|(pos, revision, _)| pos != tile || revision != world.revision())
        {
            let reach = if world.vegetation_enabled() {
                vegetation::MAX_PLANT_RADIUS_CELLS + 2
            } else {
                1
            };
            let columns = ColumnBox {
                min: min.map(|v| v - reach),
                size_cells: [(side + 2 * (margin + reach)) as u32; 2],
            };
            let mut roof = bound_native_height(terrain, columns, Default::default())
                .map_or(f64::INFINITY, |bound| bound.height_range[1]);
            if world.vegetation_enabled() {
                roof += (vegetation::MAX_PLANT_HEIGHT_CELLS + 3) as f64 * edge;
            }
            if roof.is_finite() {
                let edits = world.edit_cells_in_bounds(
                    Cell {
                        x: min[0],
                        y: (roof / edge).floor() as i64,
                        z: min[1],
                    },
                    Cell {
                        x: max[0],
                        y: i64::MAX,
                        z: max[1],
                    },
                );
                for cell in edits {
                    if world.material(cell).is_blocking() {
                        roof = roof.max((cell.y as f64 + 1.) * edge);
                    }
                }
            }
            self.tile = Some((tile, world.revision(), roof + edge));
        }
        inside.y > self.tile.unwrap().2
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

pub struct Shape {
    pub vertices: Vec<Vertex>,
    pub probes: Vec<Vec3>,
    pub center: Vec3,
    pub size: Vec3,
    pub mass: f32,
    pub inverse_inertia: Mat3,
    pub pressure_centers: [Vec3; 6],
}

/// Project exposed faces onto each principal direction once at creation.
/// The centers of those projections provide pressure arms during flight.
pub fn pressure_centers(vertices: &[Vertex]) -> [Vec3; 6] {
    let mut areas = [0_f32; 6];
    let mut centers = [Vec3::ZERO; 6];
    for face in vertices.chunks_exact(3) {
        let [a, b, c] = [0, 1, 2].map(|i| Vec3::from_array(face[i].position));
        let center = (a + b + c) / 3.;
        let vector = (b - a).cross(c - a).abs() * 0.5;
        let normal = Vec3::from_array(face[0].normal);
        for axis in 0..3 {
            let index = axis * 2 + usize::from(normal[axis] < 0.);
            areas[index] += vector[axis];
            centers[index] += center * vector[axis];
        }
    }
    std::array::from_fn(|i| centers[i] / areas[i].max(1e-8))
}

#[derive(Clone, Copy)]
pub struct Pose {
    pub center: DVec3,
    pub orientation: Quat,
}

#[derive(Clone, Copy)]
pub struct Contact {
    pub hit: RayHit,
    pub point: DVec3,
    pub normal: Vec3,
    pub fraction: f64,
    pub local: Vec3,
    pub node: usize,
}

/// Point sweeps on the occupied boundary. Long travel keeps the same solid
/// voxel query as near travel. Nodes can supply separate contact offsets.
pub fn sweep(
    world: &World,
    before: Pose,
    after: Pose,
    probes: impl IntoIterator<Item = (usize, Vec3)>,
) -> Option<Contact> {
    let mut earliest: Option<Contact> = None;
    for (node, local) in probes {
        let lo = before.center + (before.orientation * local).as_dvec3();
        let hi = after.center + (after.orientation * local).as_dvec3();
        let delta = hi - lo;
        let length = delta.length();
        if length < 1e-7 {
            continue;
        }
        let Some(hit) =
            frontier_world::raycast_solid(world, lo.to_array(), delta.to_array(), length)
        else {
            continue;
        };
        let point = lo + delta * (hit.distance / length);
        let normal = Vec3::from_array(hit.normal.map(|n| n as f32));
        let normal = if normal == Vec3::ZERO {
            -(delta / length).as_vec3()
        } else {
            normal
        };
        let fraction = (hit.distance / length).clamp(0., 1.);
        if earliest.is_none_or(|e| fraction < e.fraction) && delta.dot(normal.as_dvec3()) < 0. {
            earliest = Some(Contact {
                hit,
                point,
                normal,
                fraction,
                local,
                node,
            });
        }
    }
    earliest
}

#[derive(Clone, Copy)]
pub struct Impact {
    pub contact: Contact,
    pub energy: f32,
    pub impulse: Vec3,
}

/// Resolve the normal impulse at the point and limit friction by Coulomb's
/// law. Angular velocity is stored in world axes during free flight.
pub fn collide(
    mass: f32,
    inverse_inertia: Mat3,
    orientation: Quat,
    center: DVec3,
    velocity: &mut Vec3,
    angular: &mut Vec3,
    contact: Contact,
) -> Impact {
    let r = (contact.point - center).as_vec3();
    let axes = Mat3::from_quat(orientation);
    let inverse = axes * inverse_inertia * axes.transpose();
    let v = *velocity + angular.cross(r);
    let n = contact.normal;
    let vn = v.dot(n).min(0.);
    let tangent = v - n * v.dot(n);
    let mass_along = |direction: Vec3| {
        (mass.recip() + (inverse * r.cross(direction)).cross(r).dot(direction)).recip()
    };
    let effective_mass = mass_along(n);
    let bounce = if vn < -2. {
        if contact.hit.material == Material::Stone {
            0.21
        } else {
            0.10
        }
    } else {
        0.
    };
    let impulse_n = -(1. + bounce) * vn * effective_mass;
    let t = tangent.normalize_or_zero();
    let tangent_mass = mass_along(t);
    let impulse_t = (tangent.length() * tangent_mass).min(impulse_n * 0.53);
    let impulse = n * impulse_n - t * impulse_t;
    *velocity += impulse / mass;
    *angular += inverse * r.cross(impulse);
    Impact {
        contact,
        energy: 0.5 * effective_mass * vn * vn * (1. - bounce * bounce)
            + impulse_t * (tangent.length() - 0.5 * impulse_t / tangent_mass),
        impulse,
    }
}

fn toughness(material: Material) -> f32 {
    match material {
        Material::Snow => 38_000.,
        Material::Sand => 108_000.,
        Material::Grass | Material::Soil => 194_000.,
        Material::Wood => 1_800_000.,
        Material::Stone => 12_000_000.,
        _ => f32::INFINITY,
    }
}

/// Excavate a connected patch below the actual contact. Each removed voxel
/// spends fracture work scaled by its physical volume and remaining support.
pub fn excavate(world: &mut World, impact: Impact, span: f32) -> usize {
    let edge = frontier_world::voxel_size();
    let material = impact.contact.hit.material;
    let mut work = impact.energy
        * match material {
            Material::Snow | Material::Sand => 0.33,
            Material::Grass | Material::Soil => 0.21,
            _ => 0.055,
        };
    if work < toughness(material) * edge.powi(3) as f32 {
        return 0;
    }
    let origin = impact.contact.hit.cell;
    let normal = impact.contact.normal;
    let radius = (0.18 + span * 0.38).clamp(0.24, 1.35);
    let offset = |a: Cell, b: Cell| {
        Vec3::new((a.x - b.x) as f32, (a.y - b.y) as f32, (a.z - b.z) as f32) * edge as f32
    };
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();
    let mut candidates = vec![(origin, 0_f32)];
    seen.insert(origin);
    while let Some((index, _)) = candidates
        .iter()
        .enumerate()
        .min_by(|a, b| a.1 .1.total_cmp(&b.1 .1))
    {
        let (cell, _) = candidates.swap_remove(index);
        let material = world.material_without_fluid(cell);
        // A stone footing carries more load than a thin exposed surface lip.
        let support = neighbors(cell)
            .into_iter()
            .filter(|c| !selected.contains(c) && world.material_without_fluid(*c).is_blocking())
            .count() as f32;
        let cost = toughness(material) * edge.powi(3) as f32 * (0.7 + 0.10 * support);
        if !material.is_blocking() || work < cost {
            continue;
        }
        selected.push(cell);
        work -= cost;
        if selected.len() == 192 {
            break;
        }
        for next in neighbors(cell) {
            let delta = offset(next, origin);
            let depth = -delta.dot(normal);
            let tangent = (delta + normal * depth).length();
            let distance = (tangent * tangent + 3.2 * depth * depth).sqrt();
            if distance <= radius && depth >= -(edge as f32) && seen.insert(next) {
                candidates.push((next, distance));
            }
        }
    }
    let count = selected.len();
    world.set_batch(selected.into_iter().map(|c| (c, Material::Air)));
    count
}

fn neighbors(cell: Cell) -> [Cell; 6] {
    [
        Cell {
            x: cell.x.saturating_sub(1),
            ..cell
        },
        Cell {
            x: cell.x.saturating_add(1),
            ..cell
        },
        Cell {
            y: cell.y.saturating_sub(1),
            ..cell
        },
        Cell {
            y: cell.y.saturating_add(1),
            ..cell
        },
        Cell {
            z: cell.z.saturating_sub(1),
            ..cell
        },
        Cell {
            z: cell.z.saturating_add(1),
            ..cell
        },
    ]
}

pub struct Fragment {
    pub shape: Arc<Shape>,
    pub pose: Pose,
    pub velocity: Vec3,
    pub angular: Vec3,
    pub source: Option<crate::aircraft_damage::FragmentStructure>,
    pub(crate) sleep: f32,
    pub(crate) clock: f32,
    pub(crate) clearance: Clearance,
}

impl Fragment {
    pub fn voxel(
        pose: Pose,
        velocity: Vec3,
        angular: Vec3,
        edge: f32,
        mass: f32,
        color: [f32; 3],
    ) -> Self {
        let half = edge * 0.5;
        let mut vertices = Vec::with_capacity(36);
        for (n, u, v) in [
            (Vec3::X, Vec3::Y, Vec3::Z),
            (Vec3::Y, Vec3::Z, Vec3::X),
            (Vec3::Z, Vec3::X, Vec3::Y),
        ] {
            for sign in [-1., 1.] {
                let n = n * sign;
                let points = [n - u - v, n + u - v, n + u + v, n - u + v];
                vertices.extend([0, 1, 2, 0, 2, 3].map(|i| Vertex {
                    position: (points[i] * half).to_array(),
                    normal: n.to_array(),
                    color,
                }));
            }
        }
        let probes = [-half, half]
            .into_iter()
            .flat_map(|x| {
                [-half, half]
                    .into_iter()
                    .flat_map(move |y| [-half, half].into_iter().map(move |z| Vec3::new(x, y, z)))
            })
            .collect();
        Self {
            pose,
            velocity,
            angular,
            shape: Arc::new(Shape {
                vertices,
                probes,
                center: Vec3::ZERO,
                size: Vec3::splat(edge),
                mass,
                inverse_inertia: Mat3::from_diagonal(Vec3::splat(6. / (mass * edge * edge))),
                pressure_centers: [Vec3::ZERO; 6],
            }),
            source: None,
            sleep: 0.,
            clock: 0.,
            clearance: Default::default(),
        }
    }

    pub fn advance(
        &mut self,
        world: &mut World,
        dt: f32,
        wind: &impl Fn(DVec3) -> Vec3,
    ) -> Option<Impact> {
        let relative = wind(self.pose.center) - self.velocity;
        self.velocity.y -= GRAVITY * dt;
        // The projected cuboid faces provide drag area in the incident flow.
        // Integrating quadratic damping analytically also handles light chips.
        let orientation = self.pose.orientation;
        let body_speed = orientation.conjugate() * relative;
        let size = self.shape.size;
        let area = Vec3::new(size.y * size.z, size.x * size.z, size.x * size.y);
        let decay = area * body_speed.abs() * (0.47 * dt / self.shape.mass);
        let delta = body_speed - body_speed / (Vec3::ONE + decay);
        self.velocity += orientation * delta;
        let spin = orientation.conjugate() * self.angular;
        let mut pressure = Vec3::ZERO;
        for axis in 0..3 {
            let face = axis * 2 + usize::from(body_speed[axis] >= 0.);
            let mut impulse = Vec3::ZERO;
            impulse[axis] = self.shape.mass * delta[axis];
            pressure += self.shape.pressure_centers[face].cross(impulse);
        }
        let inverse_moment = Vec3::new(
            self.shape.inverse_inertia.x_axis.x,
            self.shape.inverse_inertia.y_axis.y,
            self.shape.inverse_inertia.z_axis.z,
        );
        self.angular = orientation
            * ((spin + self.shape.inverse_inertia * pressure)
                / (Vec3::ONE
                    + inverse_moment * spin.abs() * area * (size.length_squared() * 0.21 * dt)));

        let target = Pose {
            center: self.pose.center + self.velocity.as_dvec3() * f64::from(dt),
            orientation: (Quat::from_scaled_axis(self.angular * dt) * orientation).normalize(),
        };
        let travel = if self.clearance.sky(
            world,
            self.pose.center,
            target.center,
            f64::from(self.shape.size.length()),
        ) {
            None
        } else {
            sweep(
                world,
                self.pose,
                target,
                self.shape.probes.iter().map(|p| (0, *p)),
            )
        };
        let Some(contact) = travel else {
            self.pose = target;
            return None;
        };
        self.pose.center = self.pose.center.lerp(target.center, contact.fraction)
            + contact.normal.as_dvec3() * CONTACT_SKIN;
        self.pose.orientation = orientation.slerp(target.orientation, contact.fraction as f32);
        let impact = collide(
            self.shape.mass,
            self.shape.inverse_inertia,
            self.pose.orientation,
            self.pose.center,
            &mut self.velocity,
            &mut self.angular,
            contact,
        );
        if impact.energy < 15. && contact.normal.y > 0.45 {
            self.velocity *= (-dt * 9.).exp();
            self.angular *= (-dt * 15.).exp();
        }
        Some(impact)
    }
}

#[derive(Default)]
pub struct Scene {
    pub bodies: Vec<Fragment>,
    pub removed_cells: usize,
    last_revision: Option<u64>,
}

impl Scene {
    pub fn report(&self) -> String {
        let bodies: Vec<_> = self
            .bodies
            .iter()
            .map(|body| {
                format!(
                    "{{\"position_m\":{:?},\"mass_kg\":{},\"speed_mps\":{},\"settled\":{}}}",
                    body.pose.center.to_array(),
                    body.shape.mass,
                    body.velocity.length(),
                    body.sleep >= 1.2,
                )
            })
            .collect();
        format!(
            "{{\"removed_terrain_cells\":{},\"fragments\":[{}]}}",
            self.removed_cells,
            bodies.join(",")
        )
    }

    pub fn receive(&mut self, world: &mut World, damage: &mut crate::aircraft_damage::Damage) {
        for impact in damage.impacts.drain(..) {
            let span = crate::aircraft_damage::asset().probes[impact.contact.node]
                .iter()
                .map(|p| p.distance(impact.contact.local))
                .fold(0_f32, f32::max);
            self.removed_cells += excavate(world, impact, span);
        }
        self.bodies.append(&mut damage.fragments);
    }

    pub fn advance(
        &mut self,
        world: &mut World,
        eye: DVec3,
        dt: f32,
        wind: impl Fn(DVec3) -> Vec3,
    ) {
        if self.last_revision != Some(world.revision()) {
            self.last_revision = Some(world.revision());
            for body in &mut self.bodies {
                if body.sleep >= 1.2 {
                    body.sleep = 0.;
                }
            }
        }
        let mut spawned = Vec::new();
        for body in &mut self.bodies {
            if body.sleep >= 1.2 {
                continue;
            }
            body.clock += dt;
            let remote = body.pose.center.distance_squared(eye) > 350. * 350.;
            let approach = body.pose.center + body.velocity.as_dvec3() / 30.;
            let clear = remote
                && body.clearance.sky(
                    world,
                    body.pose.center,
                    approach,
                    f64::from(body.shape.size.length()) + 1.,
                );
            let step = if clear { 1. / 30. } else { 1. / 120. };
            while body.clock >= step {
                body.clock -= step;
                // Limit the arc swept per substep for fast-spinning fragments.
                let substeps = (body.angular.length() * step / 0.12).ceil().clamp(1., 6.) as usize;
                let tick = step / substeps as f32;
                for _ in 0..substeps {
                    let before = body.pose;
                    if let Some(hit) = body.advance(world, tick, &wind) {
                        self.removed_cells += excavate(world, hit, body.shape.size.max_element());
                        if hit.energy > 280. {
                            crate::aircraft_damage::fracture_fragment(body, hit, &mut spawned);
                        }
                        // Use actual motion around a contact. A one-point
                        // impulse can leave residual angular velocity while
                        // a small cubical shard is visibly staying in place.
                        let quiet = hit.energy < body.shape.mass * 0.25
                            && before.center.distance(body.pose.center) < f64::from(tick * 0.71)
                            && body.velocity.length() < 0.75;
                        let supported = quiet
                            && (hit.contact.normal.y > 0.45
                                || supported_below(world, hit.contact.point));
                        if supported {
                            body.sleep += tick;
                        } else if !quiet {
                            body.sleep = 0.;
                        }
                    } else if body.velocity.length() > 0.75 {
                        body.sleep = 0.;
                    }
                }
                if body.sleep >= 1.2 {
                    body.velocity = Vec3::ZERO;
                    body.angular = Vec3::ZERO;
                    break;
                }
            }
        }
        self.bodies.extend(spawned);
    }
}

fn supported_below(world: &World, point: DVec3) -> bool {
    let edge = frontier_world::voxel_size();
    frontier_world::raycast_solid(
        world,
        (point + DVec3::Y * edge).to_array(),
        [0., -1., 0.],
        edge + 0.016,
    )
    .is_some_and(|hit| hit.normal == [0, 1, 0])
}
