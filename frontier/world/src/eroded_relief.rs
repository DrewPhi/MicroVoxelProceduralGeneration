//! Deterministic bedrock profiles and secondary drainage inside the primary
//! watershed graph. All distances are expressed in authored world meters.
use super::{smooth, SeededFields};
use crate::terrain::{hash, noise, FbmParams};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

pub(crate) const BEDROCK_HEIGHT_BOUND: f64 = 61.;
pub(crate) const PROVINCE_HEIGHT_BOUND: f64 = 86.;
pub(crate) const INCISION_HEIGHT_BOUND: f64 = 27.;
const CELLS: usize = 16;
const HALO: usize = 8;
const UPSTREAM_HOPS: usize = 5;
const NODE_SPACING: f64 = 330.;

/// Select a low route through the terrain around an existing drainage link.
/// The lateral strip tapers to zero at the two canonical graph endpoints.
pub(super) fn river_offsets(
    spacing: f64,
    a: [f64; 2],
    b: [f64; 2],
    stage: [f64; 2],
    metres: f64,
    migration: f64,
    preferred: impl Fn(f64) -> f64,
    elevation: impl Fn(f64, f64) -> f64,
) -> [f64; 17] {
    const STATIONS: usize = 9;
    const LANES: usize = 9;
    const CENTER: usize = LANES / 2;
    let dx = b[0] - a[0];
    let dz = b[1] - a[1];
    let length = dx.hypot(dz);
    let normal = [-dz / length, dx / length];
    let span = spacing * 0.235;
    let envelope = |t: f64| (std::f64::consts::PI * t).sin().powi(2);
    let lateral = |lane: usize| lane as f64 / CENTER as f64 - 1.;
    let mut costs = [f64::INFINITY; LANES];
    costs[CENTER] = 0.;
    let mut parents = [[CENTER; LANES]; STATIONS];
    for (i, parent) in parents.iter_mut().enumerate().skip(1) {
        let t = i as f64 / (STATIONS - 1) as f64;
        let center = [a[0] + dx * t, a[1] + dz * t];
        let nominal = stage[0] + (stage[1] - stage[0]) * t;
        let mut next = [f64::INFINITY; LANES];
        for lane in 0..LANES {
            if i + 1 == STATIONS && lane != CENTER {
                continue;
            }
            let offset = lateral(lane) * envelope(t) * span;
            let local = if i + 1 < STATIONS {
                let h = elevation(
                    center[0] + normal[0] * offset,
                    center[1] + normal[1] * offset,
                );
                let excess = (h - nominal) / metres.max(1e-6);
                let excavation = excess.max(0.) + (-excess).max(0.) * 1.65;
                // Slow perennial channels can migrate across their floodplain.
                // Fade that freedom out where the bend would cross a ridge
                // or leave the original valley floor.
                let freedom = migration * (1. - smooth((excavation - 16.) / 52.));
                let course = 18. + 106. * freedom;
                excavation + course * (lateral(lane) - preferred(t)).powi(2)
            } else {
                0.
            };
            for (previous, &cost) in costs.iter().enumerate() {
                let turn = lane.abs_diff(previous);
                if turn > 2 {
                    continue;
                }
                let through = cost + (turn * turn) as f64 * 9.5 + local;
                if through < next[lane] {
                    next[lane] = through;
                    parent[lane] = previous;
                }
            }
        }
        costs = next;
    }
    let mut lanes = [CENTER; STATIONS];
    for i in (1..STATIONS).rev() {
        lanes[i - 1] = parents[i][lanes[i]];
    }
    std::array::from_fn(|i| {
        let left = i / 2;
        let right = i.div_ceil(2);
        let u = (lateral(lanes[left]) + lateral(lanes[right])) * 0.5;
        u * envelope(i as f64 / 16.) * span
    })
}

pub(super) fn bedrock(
    seed: u64,
    x: f64,
    z: f64,
    base: f64,
    upland: f64,
    strata: f64,
    moisture: f64,
) -> (f64, f64) {
    if base <= 4. {
        return (0., 0.);
    }
    let exposure = noise(x / 930., z / 960., seed ^ 0x5171);
    let fractures = noise(x / 93. + z / 190., z / 350. - x / 270., seed ^ 0x5172);
    let tilt = if seed & 1 == 0 { 1. } else { -1. };
    let thickness = 39. + 12. * strata + 4.5 * exposure;
    let along = tilt * 0.81 * x + 0.59 * z;
    let across = -0.59 * x + tilt * 0.81 * z;
    let fan = noise((along + 119. * strata) / 180., across / 480., seed ^ 0x5175);
    let branch = noise(
        along / 72. - 0.8 * exposure,
        across / 173. + 0.66 * fan,
        seed ^ 0x5176,
    );
    let phase = base + tilt * (0.039 * x + 0.020 * z) + 62. * strata + 12. * fractures + 17. * fan;
    let phase = phase / thickness;
    let t = phase.rem_euclid(1.);
    // Strong beds concentrate elevation change into a riser, with a
    // shallower slope through the softer interval between successive beds.
    let thickness_variance = smooth((fractures + 0.38) / 0.78);
    let width = 0.17 + 0.21 * thickness_variance;
    let riser = ((t - 0.5) / width + 0.5).clamp(0., 1.);
    let step = 0.23 * t + 0.77 * smooth(riser);
    let mountain = upland * upland * smooth((exposure + 0.42) / 0.88);
    let mesa = smooth((-moisture - 0.06) / 0.34) * smooth((strata + 0.16) / 0.58);
    let joint_mask = 0.32 + 0.68 * smooth((fractures + 0.43 + 0.30 * fan) / 0.98);
    let strength = mountain.max(0.82 * mesa) * smooth((base - 18.) / 45.);
    let erosion = 7.5 * fractures * smooth((exposure + 0.18) / 0.65);
    // Neighboring beds have different resistance. Their displacement goes
    // to zero at each contact, so changing resistance cannot introduce a seam.
    let resistance = if strength > 0. {
        let index = phase.floor();
        let cement = noise(
            along / 470. + 1.57 * index,
            across / 605. - 0.88 * index,
            seed ^ 0x51b2,
        );
        0.19 + 0.81 * smooth((cement + 0.35) / 0.73)
    } else {
        0.
    };
    let hard_face = strength * joint_mask * resistance * 4. * riser * (1. - riser);
    let joint = if hard_face > 0. {
        let joint = noise(along / 15. + 0.8 * branch, across / 31., seed ^ 0x5177);
        // The hard layer breaks into narrower faces along its rock joints.
        // Geometry and the rock-exposure material mask share that layer.
        hard_face * (4.8 * joint + 1.9 * smooth((joint + 0.28) / 0.42))
    } else {
        0.
    };
    let ledges = strength * joint_mask * (resistance * (step - t) * thickness + erosion) + joint;
    // Upwind faces retain shallower ribs through fractured rock, while
    // larger talus fans fill part of the cut below the exposed face.
    let ribs = smooth((0.20 - (fan + 0.30 * branch).abs()) / 0.25);
    let relief = strength * (38. * (fan + 0.10) + 21. * (ribs - 0.35) + 9. * branch);
    // Small lowland folds retain readable drainage divides without pushing
    // the ocean or broad lake basins above their water levels.
    let lowland =
        (1. - upland * upland) * smooth((base - 4.) / 26.) * (1. - smooth((base - 155.) / 185.));
    let folds = lowland * (13. * fan + 4. * branch);
    let displacement = (ledges + relief + folds).clamp(-BEDROCK_HEIGHT_BOUND, BEDROCK_HEIGHT_BOUND);
    (displacement, hard_face)
}

pub(super) fn province_relief(
    seed: u64,
    x: f64,
    z: f64,
    base: f64,
    uplift: f64,
    strata: f64,
    rain: f64,
) -> f64 {
    let mountain = smooth((uplift - 0.28) / 0.64) * smooth((base - 95.) / 250.);
    let coastal = smooth((base - 1.) / 13.) * (1. - smooth((base - 22.) / 56.));
    if mountain + coastal == 0. {
        return 0.;
    }
    let spine = noise(
        (x + 290. * strata) / 780.,
        (z + 440. * rain) / 890.,
        seed ^ 0x51b0,
    );
    let fork = noise(
        (x - 92. * spine) / 360.,
        (z + 111. * spine) / 310.,
        seed ^ 0x51b1,
    );
    // Humid uplift has tributary ridges and amphitheatre-shaped hollows.
    // Dry uplift retains wider structural benches under the same drainage.
    let arid = smooth((-rain - 0.025) / 0.31);
    let rib = smooth((0.31 - (spine + 0.39 * fork).abs()) / 0.30);
    let bowl = smooth((fork - 0.08) / 0.52) * smooth((-spine + 0.02) / 0.57);
    let mountain_relief = (1. - arid) * (66. * (rib - 0.43) - 42. * bowl)
        + arid * (70. * (smooth((spine + 0.12) / 0.26) - 0.5) + 16. * fork);
    // Coastal promontories and inlets share the underlying province warp.
    let coast_relief = (13. * spine + 5. * fork) * arid.mul_add(0.35, 0.65);
    (mountain * mountain_relief + coastal * coast_relief)
        .clamp(-PROVINCE_HEIGHT_BOUND, PROVINCE_HEIGHT_BOUND)
}

#[derive(Clone, Copy)]
struct Node {
    pos: [f64; 2],
    ground: f64,
    moisture: f64,
    wet: bool,
}

#[derive(Clone, Copy)]
struct Gully {
    start: [f64; 2],
    delta: [f64; 2],
    inverse_length_squared: f64,
    radius: f64,
    depth: f64,
    t0: f64,
    t1: f64,
}

impl Gully {
    fn cut(self, x: f64, z: f64) -> f64 {
        let [dx, dz] = [x - self.start[0], z - self.start[1]];
        let t =
            ((dx * self.delta[0] + dz * self.delta[1]) * self.inverse_length_squared).clamp(0., 1.);
        let x = dx - t * self.delta[0];
        let z = dz - t * self.delta[1];
        let distance2 = x * x + z * z;
        if distance2 >= self.radius * self.radius {
            return 0.;
        }
        let cross_section = distance2.sqrt() / self.radius;
        let shoulder = 1. - smooth(cross_section);
        let bed = 1. - smooth((cross_section - 0.06) / 0.49);
        let along = self.t0 + (self.t1 - self.t0) * t;
        // Incision grows from the divide and blends into the receiving
        // drainage or the common valley floor downstream.
        let longitudinal = smooth(along / 0.22) * (0.76 + 0.24 * smooth((1. - along) / 0.18));
        self.depth * longitudinal * (0.39 * shoulder + 0.61 * bed)
    }
}

struct Tile {
    bins: Vec<Vec<Gully>>,
    origin: [f64; 2],
    spacing: f64,
}

impl Tile {
    fn cut(&self, x: f64, z: f64) -> f64 {
        let bin = |coord: f64, base: f64| {
            ((coord - base) / self.spacing)
                .floor()
                .clamp(0., (CELLS - 1) as f64) as usize
        };
        self.bins[bin(z, self.origin[1]) * CELLS + bin(x, self.origin[0])]
            .iter()
            .map(|r| r.cut(x, z))
            .fold(0., f64::max)
    }

    fn add(&mut self, a: [f64; 2], b: [f64; 2], profile: [f64; 4]) {
        let [radius, depth, t0, t1] = profile;
        let delta = [b[0] - a[0], b[1] - a[1]];
        let rill = Gully {
            start: a,
            delta,
            inverse_length_squared: (delta[0] * delta[0] + delta[1] * delta[1]).recip(),
            radius,
            depth,
            t0,
            t1,
        };
        let bin =
            |value: f64, axis: usize| ((value - self.origin[axis]) / self.spacing).floor() as i32;
        let [x0, z0] = [
            bin(a[0].min(b[0]) - radius, 0),
            bin(a[1].min(b[1]) - radius, 1),
        ];
        let [x1, z1] = [
            bin(a[0].max(b[0]) + radius, 0),
            bin(a[1].max(b[1]) + radius, 1),
        ];
        for z in z0.max(0)..=z1.min(CELLS as i32 - 1) {
            for x in x0.max(0)..=x1.min(CELLS as i32 - 1) {
                self.bins[z as usize * CELLS + x as usize].push(rill);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Key {
    seed: u64,
    terrain: FbmParams,
    origin: [i64; 2],
}

#[derive(Default)]
struct Cache {
    current: Option<(Key, Rc<Tile>)>,
    recent: VecDeque<(Key, Rc<Tile>)>,
}

thread_local! {
    static TILES: RefCell<Cache> = RefCell::new(Cache::default());
}

pub(super) fn incision(
    seed: u64,
    terrain: FbmParams,
    x: f64,
    z: f64,
    primary: impl Fn(f64, f64) -> SeededFields,
) -> f64 {
    let spacing = NODE_SPACING * terrain.horizontal_scale / 1800.;
    let width = (CELLS as f64) * spacing;
    let origin = [(x / width).floor() as i64, (z / width).floor() as i64];
    let key = Key {
        seed,
        terrain,
        origin,
    };
    if let Some(value) = TILES.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((k, tile)) = &cache.current {
            if *k == key {
                return Some(tile.cut(x, z));
            }
        }
        let tile = cache
            .recent
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, t)| Rc::clone(t))?;
        let cut = tile.cut(x, z);
        cache.current = Some((key, tile));
        Some(cut)
    }) {
        return value;
    }
    let tile = Rc::new(build(key, spacing, primary));
    let cut = tile.cut(x, z);
    TILES.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.recent.len() == 24 {
            cache.recent.pop_front();
        }
        cache.recent.push_back((key, Rc::clone(&tile)));
        cache.current = Some((key, tile));
    });
    cut
}

fn build(key: Key, spacing: f64, primary: impl Fn(f64, f64) -> SeededFields) -> Tile {
    let side = CELLS + 2 * HALO;
    let start = key.origin.map(|n| n * CELLS as i64 - HALO as i64);
    let origin = key.origin.map(|n| n as f64 * CELLS as f64 * spacing);
    let scale = key.terrain.amplitude / 1000.;
    let nodes = (0..side * side)
        .map(|i| {
            let ix = start[0] + (i % side) as i64;
            let iz = start[1] + (i / side) as i64;
            let displacement = |salt| {
                let fraction = (hash(ix, iz, key.seed ^ salt) >> 11) as f64 / ((1u64 << 53) as f64);
                0.24 * (fraction - 0.5)
            };
            let pos = [
                (ix as f64 + displacement(0x5181)) * spacing,
                (iz as f64 + displacement(0x5182)) * spacing,
            ];
            let f = primary(pos[0], pos[1]);
            Node {
                pos,
                ground: f.ground_height_m,
                moisture: f.moisture,
                wet: f.water_height_m.is_some(),
            }
        })
        .collect::<Vec<_>>();
    let mut next = vec![None; side * side];
    let mut local = vec![0.; side * side];
    for iz in 1..side - 1 {
        for ix in 1..side - 1 {
            let i = iz * side + ix;
            let a = nodes[i];
            if a.wet || a.ground < 3. * scale {
                continue;
            }
            local[i] = 0.14 + 0.7 * smooth((a.moisture - 0.18) / 0.52);
            let mut best = (0.013 * scale, i);
            for dz in iz - 1..=iz + 1 {
                for dx in ix - 1..=ix + 1 {
                    let j = dz * side + dx;
                    let b = nodes[j];
                    if i == j || a.ground <= b.ground {
                        continue;
                    }
                    let length = (b.pos[0] - a.pos[0]).hypot(b.pos[1] - a.pos[1]);
                    let grade = (a.ground - b.ground) / length;
                    if grade > best.0 {
                        best = (grade, j);
                    }
                }
            }
            if best.1 != i {
                next[i] = Some(best);
            }
        }
    }
    let mut runoff = local.clone();
    let mut scratch = local.clone();
    for _ in 0..UPSTREAM_HOPS {
        scratch.copy_from_slice(&local);
        for (i, link) in next.iter().enumerate() {
            if let Some((_, receiver)) = *link {
                scratch[receiver] += runoff[i];
            }
        }
        std::mem::swap(&mut scratch, &mut runoff);
    }

    let mut tile = Tile {
        bins: vec![Vec::new(); CELLS * CELLS],
        origin,
        spacing,
    };
    for iz in HALO - 2..HALO + CELLS + 2 {
        for ix in HALO - 2..HALO + CELLS + 2 {
            let index = iz * side + ix;
            let Some((slope, receiver)) = next[index] else {
                continue;
            };
            let strength = smooth((runoff[index] - 0.25) / 1.35);
            if strength < 0.018 {
                continue;
            }
            let a = nodes[index].pos;
            let b = nodes[receiver].pos;
            let dx = b[0] - a[0];
            let dz = b[1] - a[1];
            let dist = dx.hypot(dz);
            let across = [-dz / dist * 0.16 * spacing, dx / dist * 0.16 * spacing];
            let centre = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
            let left = primary(centre[0] + across[0], centre[1] + across[1]).ground_height_m;
            let right = primary(centre[0] - across[0], centre[1] - across[1]).ground_height_m;
            let turn = ((right - left) / (26. * scale).max(1e-6)).clamp(-1., 1.);
            let middle = [centre[0] + across[0] * turn, centre[1] + across[1] * turn];
            let radius = spacing * (0.095 + 0.18 * strength);
            let grade = smooth(slope / (0.26 * scale).max(1e-6));
            let depth = ((2.5 + 22. * grade) * strength.sqrt()).min(INCISION_HEIGHT_BOUND) * scale;
            tile.add(a, middle, [radius, depth, 0., 0.5]);
            tile.add(middle, b, [radius, depth, 0.5, 1.]);

            if grade * strength < 0.19 {
                continue;
            }
            // A steep enough flank sheds rain into a narrower headwater
            // tributary. Ground and wet-stage checks keep sides and confluences
            // tied to the drainage already present in neighboring tiles.
            let placement = hash(
                start[0] + ix as i64,
                start[1] + iz as i64,
                key.seed ^ 0x519e,
            );
            let side = if placement & 1 == 0 { -1. } else { 1. };
            let reach = 1.75 + 0.69 * f64::from((placement >> 8) as u8) / 255.;
            let start = [
                centre[0] - 0.16 * dx + across[0] * side * reach,
                centre[1] - 0.16 * dz + across[1] * side * reach,
            ];
            let mouth = [
                0.68 * middle[0] + 0.32 * b[0],
                0.68 * middle[1] + 0.32 * b[1],
            ];
            let head = primary(start[0], start[1]);
            let end = primary(mouth[0], mouth[1]);
            let fall = head.ground_height_m - end.ground_height_m;
            if head.water_height_m.is_none() && fall > 3. * scale {
                let grade = smooth(fall / (38. * scale).max(1e-6));
                let bed_width = spacing * (0.046 + 0.057 * grade);
                tile.add(
                    start,
                    mouth,
                    [bed_width, depth * (0.28 + 0.33 * grade), 0., 1.],
                );
            }
        }
    }
    tile
}

#[cfg(test)]
mod tests {
    use super::river_offsets;

    #[test]
    fn drainage_path_follows_lower_ground_within_its_corridor() {
        let offsets = river_offsets(
            2400.,
            [0., 0.],
            [2400., 0.],
            [20., 0.],
            1.,
            0.,
            |_| 0.,
            |x, z| {
                let trough = 420. * (std::f64::consts::PI * x / 2400.).sin().powi(2);
                20. * (1. - x / 2400.) + (z - trough).powi(2) / 470.
            },
        );
        assert_eq!(offsets[0], 0.);
        assert!(offsets[16].abs() < 1e-10);
        assert!(offsets[8] > 280.);
        assert!(offsets.iter().all(|v| v.abs() <= 2400. * 0.235));
    }

    #[test]
    fn a_lowland_course_can_bend_on_both_sides_of_the_same_floodplain() {
        let offsets = river_offsets(
            2400.,
            [0., 0.],
            [2400., 0.],
            [10., 0.],
            1.,
            1.,
            |t| -(std::f64::consts::TAU * (t - 0.5)).sin(),
            |x, _| 10. * (1. - x / 2400.),
        );
        assert_eq!(offsets[0], 0.);
        assert!(offsets[16].abs() < 1e-10);
        assert!(offsets[..8].iter().any(|&offset| offset > 145.));
        assert!(offsets[9..].iter().any(|&offset| offset < -145.));
    }
}
