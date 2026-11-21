//! Climate fBm, continental uplift and globally addressed drainage.
//! The original seeded recipe uses local D8 routing. Watershed adds a bounded
//! upstream halo, jittered drainage nodes and valley profiles shaped by incision.
//! Cached regions are an acceleration structure. Each has 256 candidate bins
//! and cache order never changes the world-space descriptors.
use super::{noise, FbmParams, FbmRecipe, TerrainColumn};
use crate::alpine::Value;
use crate::{voxel_size, Material};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::Rc,
};
pub const SEEDED_WORLD_SOURCE_VERSION: u32 = 8;
pub const SEEDED_WORLD_APPEARANCE_VERSION: u32 = 2;

const CLIMATE_CACHE_CAPACITY: usize = 4096;
const REGION_CACHE_CAPACITY: usize = 8;
const REGION_BINS_PER_AXIS: usize = 16;
const RUNOFF_HOPS: usize = 8;
pub(crate) const WATERSHED_MAX_WATER_DEPTH: f64 = 9.;
pub(crate) const WATERSHED_WATER_ABOVE_LAND: f64 = 0.7;

pub(crate) fn minimum_radius(p: FbmParams) -> f64 {
    if p.recipe.uses_watersheds() {
        3.
    } else {
        8.
    }
}

pub(crate) fn maximum_lake_radius(p: FbmParams) -> f64 {
    if p.recipe.uses_watersheds() {
        140.
    } else {
        100.
    }
}

pub(crate) fn global_height_range(p: FbmParams) -> [f64; 2] {
    let authored = if p.recipe == FbmRecipe::Highlands {
        [-650., 3880.]
    } else if p.recipe.uses_watersheds() {
        [-600., 2320.]
    } else {
        [-402., 1663.]
    };
    authored.map(|h| h * p.amplitude / 1000.)
}

/// Shared scalar and affine composition. Independent texture contributes most
/// on high slopes, with a second broad field breaking up lowland relief.
pub(crate) fn watershed_relief<V: Value>(
    continent: V,
    mountain: V,
    ridge: V,
    hills: V,
    fine: V,
    strata: V,
) -> V {
    let spine = ridge.mul(ridge).mul(ridge).scale(1520.);
    let highlands = mountain.mul(mountain).mul(spine.add(strata.scale(145.)));
    continent
        .scale(440.)
        .add(V::point(90.))
        .add(highlands)
        .add(hills.mul(V::point(50.).add(strata.scale(30.))))
        .add(fine.scale(5.))
}

// Low passes and individual summits interrupt the main crest. Smaller
// tributary ridges rise along its flanks. Relief in both bands fades across
// the foothills, leaving climate and drainage continuous into the lowlands.
pub(crate) fn highland_relief<V: Value>(
    continent: V,
    mountain: V,
    ridge: V,
    hills: V,
    fine: V,
    spur: V,
) -> V {
    let crest = ridge.mul(ridge);
    let summits = crest.mul(ridge).mul(V::point(1360.).add(hills.scale(820.)));
    let buttresses = spur.mul(spur).mul(crest).scale(420.);
    let relief = mountain.mul(mountain).mul(summits.add(buttresses));
    let foothills = mountain.mul(V::point(170.).add(hills.scale(230.)));
    continent
        .scale(480.)
        .add(V::point(96.))
        .add(relief)
        .add(foothills)
        .add(hills.scale(85.))
        .add(fine.mul(V::point(5.).add(mountain.mul(ridge).scale(72.))))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SeededBiome {
    Ocean,
    Coast,
    Desert,
    Plains,
    Tundra,
    Mountains,
    Woodland,
    Wetland,
    Steppe,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeededFields {
    pub continentalness: f64,
    pub temperature: f64,
    pub moisture: f64,
    pub mountain: f64,
    pub biome: SeededBiome,
    pub ground_height_m: f64,
    pub water_height_m: Option<f64>,
}
fn smooth(t: f64) -> f64 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}
fn fbm(seed: u64, x: f64, z: f64, wavelength: f64, count: u32, gain: f64, salt: u64) -> f64 {
    let (mut a, mut f, mut sum, mut norm) = (1., 1. / wavelength, 0., 0.);
    for i in 0..count {
        sum += a * noise(
            x * f + 13.7,
            z * f - 8.3,
            seed ^ salt.wrapping_add(i as u64 * 0x9e3779b9),
        );
        norm += a;
        a *= gain;
        f *= 2.;
    }
    sum / norm
}
#[derive(Clone, Copy)]
struct Land {
    h: f64,
    continental: f64,
    mountain: f64,
}
fn land(seed: u64, p: FbmParams, x: f64, z: f64) -> Land {
    // World scale changes wavelengths alone; amplitude changes meters of relief.
    let x = x * 1800. / p.horizontal_scale;
    let z = z * 1800. / p.horizontal_scale;
    let continental = fbm(seed, x, z, 48000., 3, 0.45, 0x5101);
    let province = fbm(seed, x, z, 22000., 3, 0.45, 0x5102);
    let mountain = smooth((province + 0.08) / 0.48);
    let ridge = 1. - fbm(seed, x, z, 3600., 4, 0.48, 0x5105).abs();
    let foothill = fbm(seed, x, z, 1800., 3, 0.45, 0x5106);
    let fine = fbm(seed, x, z, 90., p.octaves, 0.65 * p.roughness, 0x5107);
    let authored = if p.recipe == FbmRecipe::Highlands {
        let spur = 1.
            - fbm(
                seed,
                x + 410. * foothill,
                z - 260. * foothill,
                960.,
                4,
                0.52,
                0x5174,
            )
            .abs();
        highland_relief(continental, mountain, ridge, foothill, fine, spur)
    } else if p.recipe.uses_watersheds() {
        let strata = fbm(seed, x, z, 1050., 3, 0.45, 0x5121);
        watershed_relief(continental, mountain, ridge, foothill, fine, strata)
    } else {
        400. * continental
            + 65.
            + 1150. * mountain * mountain * ridge * ridge
            + 45. * foothill
            + 3. * fine
    };
    let h = p.amplitude / 1000. * authored;
    Land {
        h,
        continental,
        mountain,
    }
}
// Climate is a smooth, globally anchored 512m field. Fine native columns reuse
// the same four climate corners rather than repeating upwind noise evaluations.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ClimateKey {
    seed: u64,
    x: i64,
    z: i64,
}
#[derive(Default)]
struct ClimateCache {
    nodes: HashMap<ClimateKey, [f64; 2]>,
    last: Option<(ClimateKey, [[f64; 2]; 4])>,
}
thread_local! {
    static CLIMATE: RefCell<ClimateCache> = RefCell::new(ClimateCache::default());
}
fn climate_node(seed: u64, ix: i64, iz: i64) -> [f64; 2] {
    let x = ix as f64 * 512.;
    let z = iz as f64 * 512.;
    let temperature = (0.52 + 0.62 * fbm(seed, x, z, 38000., 3, 0.5, 0x5103)).clamp(0., 1.);
    let uplift = |x, z| {
        let m = smooth((fbm(seed, x, z, 22000., 3, 0.45, 0x5102) + 0.08) / 0.48);
        m * m
    };
    let local = uplift(x, z);
    let wind = match seed & 3 {
        0 => [1., 0.],
        1 => [0., 1.],
        2 => [-1., 0.],
        _ => [0., -1.],
    };
    let mut barrier: f64 = 0.;
    for distance in [2000., 4000., 8000., 16000.] {
        barrier = barrier.max(uplift(x - wind[0] * distance, z - wind[1] * distance) - local);
    }
    let moisture =
        (0.50 + 0.75 * fbm(seed, x, z, 27000., 3, 0.5, 0x5104) - 0.24 * barrier).clamp(0., 1.);
    [temperature, moisture]
}
fn climate(seed: u64, p: FbmParams, x: f64, z: f64) -> [f64; 2] {
    let x = x * 1800. / p.horizontal_scale / 512.;
    let z = z * 1800. / p.horizontal_scale / 512.;
    let ix = x.floor() as i64;
    let iz = z.floor() as i64;
    let key = ClimateKey { seed, x: ix, z: iz };
    let corners = CLIMATE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((old, values)) = cache.last {
            if old == key {
                return values;
            }
        }
        // Leave room for all four interpolation corners before inserting them.
        if cache.nodes.len() > CLIMATE_CACHE_CAPACITY - 4 {
            cache.nodes.clear();
        }
        let coords = [(ix, iz), (ix + 1, iz), (ix, iz + 1), (ix + 1, iz + 1)];
        let values = coords.map(|(x, z)| {
            *cache
                .nodes
                .entry(ClimateKey { seed, x, z })
                .or_insert_with(|| climate_node(seed, x, z))
        });
        cache.last = Some((key, values));
        values
    });
    let tx = smooth(x - x.floor());
    let tz = smooth(z - z.floor());
    [0, 1].map(|i| {
        let a = corners[0][i] + tx * (corners[1][i] - corners[0][i]);
        let b = corners[2][i] + tx * (corners[3][i] - corners[2][i]);
        a + tz * (b - a)
    })
}
// Graph nodes sample the same canonical landform at globally fixed coordinates;
// renderer spacing and cached query order never change their stages.
fn node(seed: u64, p: FbmParams, ix: i64, iz: i64) -> Node {
    let spacing = 2400. * p.horizontal_scale / 1800.;
    let mut x = ix as f64 * spacing;
    let mut z = iz as f64 * spacing;
    if p.recipe.uses_watersheds() {
        let unit = |salt| (super::hash(ix, iz, seed ^ salt) >> 11) as f64 / (1u64 << 53) as f64;
        x += (unit(0x5131) - 0.5) * 0.32 * spacing;
        z += (unit(0x5132) - 0.5) * 0.32 * spacing;
    }
    let l = land(seed, p, x, z);
    Node {
        x,
        z,
        stage: (l.h - 12. * p.amplitude / 1000.).max(0.),
        potential: l.h - 12. * p.amplitude / 1000.,
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Node {
    pub(crate) x: f64,
    pub(crate) z: f64,
    pub(crate) stage: f64,
    potential: f64,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Reach {
    pub(crate) a: Node,
    pub(crate) b: Node,
    pub(crate) width: f64,
    pub(crate) width_b: f64,
    pub(crate) depth: f64,
    grade: f64,
    valley_radius: f64,
    pub(crate) lake: bool,
    pub(crate) owner: [i64; 2],
    pub(crate) segment: u8,
}
impl Reach {
    /// Radius range at projections of this box onto the finite channel.
    /// A wet sample must intersect the segment's capsule, including its caps.
    pub(crate) fn core_radii(self, min: [f64; 2], max: [f64; 2]) -> Option<[f64; 2]> {
        if self.a.stage.max(self.b.stage) < 0. {
            return None;
        }
        let dx = self.b.x - self.a.x;
        let dz = self.b.z - self.a.z;
        let len2 = dx * dx + dz * dz;
        let mid = [0, 1].map(|a| (min[a] + max[a]) * 0.5);
        let half = [0, 1].map(|a| (max[a] - min[a]) * 0.5);
        let x = mid[0] - self.a.x;
        let z = mid[1] - self.a.z;
        // Include rounding in both the projection and separating-axis tests.
        let pad = 1e-8 * (1. + mid[0].abs() + mid[1].abs() + dx.abs() + dz.abs());
        let length = len2.sqrt();
        let (t0, t1) = if len2 > 0. {
            let mid_dot = x * dx + z * dz;
            let span = half[0] * dx.abs() + half[1] * dz.abs() + pad * length;
            (
                ((mid_dot - span) / len2).clamp(0., 1.),
                ((mid_dot + span) / len2).clamp(0., 1.),
            )
        } else {
            (0., 0.)
        };
        let radii = [t0, t1].map(|t| self.width + (self.width_b - self.width) * t);
        let radius = radii[0].max(radii[1]) + pad;
        if self.a.x.min(self.b.x) - radius > max[0]
            || self.a.x.max(self.b.x) + radius < min[0]
            || self.a.z.min(self.b.z) - radius > max[1]
            || self.a.z.max(self.b.z) + radius < min[1]
            || (x * dz - z * dx).abs() > half[0] * dz.abs() + half[1] * dx.abs() + radius * length
        {
            return None;
        }
        Some([
            (radii[0].min(radii[1]) - pad).max(self.width.min(self.width_b)),
            radius,
        ])
    }

    /// Compact river-valley conditioning stays inside one drainage-node halo.
    pub(crate) fn outer_radius(self) -> f64 {
        self.valley_radius
    }
}
#[derive(Debug)]
struct Region {
    reaches: Vec<Reach>,
    bins: Vec<Vec<usize>>,
    origin: [f64; 2],
    bin_size: f64,
}
impl Region {
    fn bin(&self, x: f64, z: f64) -> usize {
        let ix = ((x - self.origin[0]) / self.bin_size)
            .floor()
            .clamp(0., (REGION_BINS_PER_AXIS - 1) as f64) as usize;
        let iz = ((z - self.origin[1]) / self.bin_size)
            .floor()
            .clamp(0., (REGION_BINS_PER_AXIS - 1) as f64) as usize;
        iz * REGION_BINS_PER_AXIS + ix
    }
    fn candidates(&self, x: f64, z: f64) -> impl Iterator<Item = Reach> + '_ {
        self.bins[self.bin(x, z)].iter().map(|&i| self.reaches[i])
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    legacy_riverbanks: bool,
    recipe: FbmRecipe,
    seed: u64,
    scale: u64,
    amplitude: u64,
    octaves: u32,
    roughness: u64,
    x: i64,
    z: i64,
}
thread_local! {
    static REGIONS: RefCell<VecDeque<(Key, Rc<Region>)>> = const {
        RefCell::new(VecDeque::new())
    };
}

/// Rainfall accumulates along strictly descending links. For an interior node,
/// each iteration adds exactly one more upstream hop, so the finite halo gives
/// identical discharge to neighboring regions even when their caches differ.
fn accumulated_runoff(nodes: &[Node], side: usize, seed: u64, p: FbmParams) -> Vec<f64> {
    let mut links = Vec::with_capacity(nodes.len());
    let mut local = Vec::with_capacity(nodes.len());
    for (i, node) in nodes.iter().enumerate() {
        let [t, m] = climate(seed, p, node.x, node.z);
        local.push(if node.potential > 0. {
            0.035 + 1.1 * smooth((m - 0.23 - 0.12 * t) / 0.52)
        } else {
            0.
        });
        links.push(downstream(nodes, side, i));
    }
    let mut flow = local.clone();
    let mut next = local.clone();
    for _ in 0..RUNOFF_HOPS {
        next.copy_from_slice(&local);
        for (i, &j) in links.iter().enumerate() {
            if i != j {
                next[j] += flow[i];
            }
        }
        std::mem::swap(&mut next, &mut flow);
    }
    flow
}

fn downstream(nodes: &[Node], side: usize, i: usize) -> usize {
    let x = i % side;
    let z = i / side;
    if nodes[i].potential <= 0. || x == 0 || z == 0 || x + 1 == side || z + 1 == side {
        return i;
    }
    let mut best = i;
    for row in z - 1..=z + 1 {
        for col in x - 1..=x + 1 {
            let j = row * side + col;
            if nodes[j].potential < nodes[best].potential {
                best = j;
            }
        }
    }
    best
}

fn region(seed: u64, p: FbmParams, x: f64, z: f64) -> Rc<Region> {
    let spacing = 2400. * p.horizontal_scale / 1800.;
    let rx = (x / (spacing * 4.)).floor() as i64;
    let rz = (z / (spacing * 4.)).floor() as i64;
    let key = Key {
        legacy_riverbanks: p.legacy_riverbanks,
        recipe: p.recipe,
        seed,
        scale: p.horizontal_scale.to_bits(),
        amplitude: p.amplitude.to_bits(),
        octaves: p.octaves,
        roughness: p.roughness.to_bits(),
        x: rx,
        z: rz,
    };
    if let Some(hit) = REGIONS.with(|cache| {
        cache
            .borrow()
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| Rc::clone(v))
    }) {
        return hit;
    }
    // Build outside the cache borrow. A one-node halo includes every incident
    // edge affecting this region, including diagonals and negative boundaries.
    let watershed = p.recipe.uses_watersheds();
    // The row just outside the accumulated halo is needed to choose the
    // downstream link of its outermost contributors.
    let halo = if watershed { RUNOFF_HOPS + 1 } else { 0 };
    // A region covers four intervals and has five bounding nodes. A reach
    // owned one node beyond either end can drain back into it. In particular
    // a jittered endpoint may cross the positive boundary from the next region.
    let extra_owner = usize::from(watershed && !p.legacy_riverbanks);
    let side = 8 + extra_owner + 2 * halo;
    let base = [rx * 4 - 2 - halo as i64, rz * 4 - 2 - halo as i64];
    let mut nodes = Vec::with_capacity(side * side);
    for iz in base[1]..base[1] + side as i64 {
        for ix in base[0]..base[0] + side as i64 {
            nodes.push(node(seed, p, ix, iz));
        }
    }
    let flow = watershed.then(|| accumulated_runoff(&nodes, side, seed, p));
    let river_width = |index: usize| {
        let n = nodes[index];
        let authored = if let Some(flow) = &flow {
            (3. + 10. * flow[index].sqrt()).min(44.)
        } else {
            let [temperature, moisture] = climate(seed, p, n.x, n.z);
            let runoff = (moisture - 0.10 * temperature).clamp(0., 1.);
            8. + 16. * smooth((runoff - 0.15) / 0.60)
        };
        authored * p.horizontal_scale / 1800.
    };
    let mut reaches = Vec::new();
    for j in halo + 1..halo + 7 + extra_owner {
        for i in halo + 1..halo + 7 + extra_owner {
            let index = j * side + i;
            let a = nodes[index];
            if a.potential <= 0. {
                continue;
            }
            let next = downstream(&nodes, side, index);
            let b = nodes[next];
            let lake = next == index;
            let width = if lake {
                let radius = flow.as_ref().map_or(100., |flow| {
                    (38. + 21. * flow[index].sqrt()).min(maximum_lake_radius(p))
                });
                radius * p.horizontal_scale / 1800.
            } else {
                river_width(index)
            };
            let owner = [base[0] + i as i64, base[1] + j as i64];
            let depth = if lake {
                7.
            } else if watershed {
                (0.7 + 0.18 * width * 1800. / p.horizontal_scale).min(7.)
            } else {
                4.
            };
            if lake {
                let lake_end = if let Some(flow) = &flow {
                    // Extend the lake up its principal inlet. Its water stage
                    // remains exactly the shared sink stage.
                    let inlet = (j - 1..=j + 1)
                        .flat_map(|row| (i - 1..=i + 1).map(move |col| row * side + col))
                        .filter(|&k| k != index && downstream(&nodes, side, k) == index)
                        .max_by(|&a, &b| flow[a].total_cmp(&flow[b]));
                    inlet.map_or(b, |k| {
                        let dx = nodes[k].x - a.x;
                        let dz = nodes[k].z - a.z;
                        let extension = 1.35 * width / dx.hypot(dz);
                        Node {
                            x: a.x + dx * extension,
                            z: a.z + dz * extension,
                            ..a
                        }
                    })
                } else {
                    b
                };
                reaches.push(Reach {
                    a,
                    b: lake_end,
                    grade: 0.,
                    width,
                    width_b: width,
                    depth,
                    valley_radius: 3. * width,
                    lake,
                    owner,
                    segment: 0,
                });
            } else {
                // The source path is finite and independent of cache regions.
                // Every segment shares its exact endpoint/stage with the next.
                let dx = b.x - a.x;
                let dz = b.z - a.z;
                let length = dx.hypot(dz);
                // Two explicit bounded oscillations keep bends legible at the
                // kilometre node scale; low-amplitude FBM alone looked straight.
                // The envelope has zero value/slope at both shared nodes.
                // The bend, endpoint jitter and valley support together
                // span less than one grid interval beyond incident nodes.
                let phase = |salt| {
                    let bits = super::hash(owner[0], owner[1], seed ^ salt);
                    (bits >> 11) as f64 / ((1u64 << 53) as f64) * std::f64::consts::TAU
                };
                let phase1 = phase(0x5191);
                let phase2 = phase(0x5192);
                let point = |index: u8| {
                    if index == 0 {
                        return a;
                    }
                    if index == 16 {
                        return b;
                    }
                    let t = index as f64 / 16.;
                    let px = a.x + t * dx;
                    let pz = a.z + t * dz;
                    let bend = (std::f64::consts::PI * t).sin().powi(2);
                    let signal = 0.75 * (std::f64::consts::TAU * t + phase1).sin()
                        + 0.25 * (2. * std::f64::consts::TAU * t + phase2).sin();
                    let offset = 0.16 * spacing * bend * signal;
                    Node {
                        x: px - dz / length * offset,
                        z: pz + dx / length * offset,
                        stage: a.stage + t * (b.stage - a.stage),
                        potential: a.potential + t * (b.potential - a.potential),
                    }
                };
                let mut points: [Node; 17] = std::array::from_fn(|i| point(i as u8));
                let downstream_width = river_width(next);
                let valley_radius = watershed.then(|| {
                    // Deep channels through uplift need broader shoulders than
                    // the water itself. Look for incision along the entire edge,
                    // so adjacent segments do not abruptly narrow the valley.
                    let incision = points
                        .iter()
                        .step_by(4)
                        .map(|n| land(seed, p, n.x, n.z).h - n.stage)
                        .fold(0.0_f64, f64::max);
                    let scale = p.horizontal_scale / 1800.;
                    let authored_incision = incision / (p.amplitude / 1000.).max(1e-12);
                    (190. + 2.2 * authored_incision).clamp(190., 820.) * scale
                });
                let max_width: f64 = if watershed { 44. } else { 24. };
                let widths: [f64; 17] = std::array::from_fn(|i| {
                    if i == 0 {
                        return width;
                    }
                    if i == 16 {
                        return downstream_width;
                    }
                    let t = i as f64 / 16.;
                    let baseline = width + t * (downstream_width - width);
                    // Width varies continuously within an edge as well as
                    // between climates. Shared endpoints have exact node widths.
                    let pulse = 1.
                        + 0.14
                            * (std::f64::consts::PI * t).sin().powi(2)
                            * (std::f64::consts::TAU * t + phase2).sin();
                    (baseline * pulse).clamp(
                        minimum_radius(p) * p.horizontal_scale / 1800.,
                        max_width * p.horizontal_scale / 1800.,
                    )
                });
                if watershed && !p.legacy_riverbanks {
                    // An interpolated reach can cross a saddle below its graph
                    // stage. Fit one stage across its width, preserving the
                    // shared end nodes and a non-increasing downstream profile.
                    let mut upstream = a.stage;
                    for i in 1..16 {
                        let tx = points[i + 1].x - points[i - 1].x;
                        let tz = points[i + 1].z - points[i - 1].z;
                        let radius = widths[i] / tx.hypot(tz);
                        let point = &mut points[i];
                        let mut shoulder = land(seed, p, point.x, point.z).h;
                        for sign in [-1., 1.] {
                            let x = point.x - sign * tz * radius;
                            let z = point.z + sign * tx * radius;
                            shoulder = shoulder.min(land(seed, p, x, z).h);
                        }
                        point.stage = point
                            .stage
                            .min(upstream)
                            .min(shoulder - 0.7 * p.amplitude / 1000.)
                            .max(b.stage);
                        upstream = point.stage;
                    }
                }
                for segment in 0..16 {
                    let segment_width = widths[segment as usize];
                    let segment_width_b = widths[segment as usize + 1];
                    let widest = segment_width.max(segment_width_b);
                    let a = points[segment as usize];
                    let b = points[segment as usize + 1];
                    let length = (a.x - b.x).hypot(a.z - b.z);
                    reaches.push(Reach {
                        a,
                        b,
                        grade: if length > 0. {
                            ((a.stage - b.stage) / length).max(0.)
                        } else {
                            0.
                        },
                        width: segment_width,
                        width_b: segment_width_b,
                        depth,
                        valley_radius: valley_radius
                            .map_or(18. * widest, |radius| radius.max(14. * widest)),
                        lake,
                        owner,
                        segment,
                    });
                }
            }
        }
    }
    let origin = [rx as f64 * spacing * 4., rz as f64 * spacing * 4.];
    let bin_size = spacing / (REGION_BINS_PER_AXIS as f64 / 4.);
    let mut bins = vec![Vec::new(); REGION_BINS_PER_AXIS * REGION_BINS_PER_AXIS];
    for (i, r) in reaches.iter().enumerate() {
        let radius = r.outer_radius();
        let ax = ((r.a.x.min(r.b.x) - radius - origin[0]) / bin_size).floor() as i64;
        let bx = ((r.a.x.max(r.b.x) + radius - origin[0]) / bin_size).floor() as i64;
        let az = ((r.a.z.min(r.b.z) - radius - origin[1]) / bin_size).floor() as i64;
        let bz = ((r.a.z.max(r.b.z) + radius - origin[1]) / bin_size).floor() as i64;
        for iz in az.max(0)..=bz.min((REGION_BINS_PER_AXIS - 1) as i64) {
            for ix in ax.max(0)..=bx.min((REGION_BINS_PER_AXIS - 1) as i64) {
                bins[iz as usize * REGION_BINS_PER_AXIS + ix as usize].push(i);
            }
        }
    }
    let value = Rc::new(Region {
        reaches,
        bins,
        origin,
        bin_size,
    });
    REGIONS.with(|cache| {
        let mut c = cache.borrow_mut();
        let capacity = if watershed { 20 } else { REGION_CACHE_CAPACITY };
        while c.len() >= capacity {
            c.pop_front();
        }
        c.push_back((key, Rc::clone(&value)));
    });
    value
}
fn projection_t(r: Reach, x: f64, z: f64) -> f64 {
    let dx = r.b.x - r.a.x;
    let dz = r.b.z - r.a.z;
    let denom = dx * dx + dz * dz;
    if denom > 0. {
        (((x - r.a.x) * dx + (z - r.a.z) * dz) / denom).clamp(0., 1.)
    } else {
        0.
    }
}
fn project(r: Reach, x: f64, z: f64) -> (f64, f64) {
    let dx = r.b.x - r.a.x;
    let dz = r.b.z - r.a.z;
    let t = projection_t(r, x, z);
    (
        (x - r.a.x - t * dx).hypot(z - r.a.z - t * dz),
        r.a.stage + t * (r.b.stage - r.a.stage),
    )
}

/// Downstream unit tangent and fall per horizontal metre at a channel column.
/// The caller already knows the column is wet. A lake owns its surface at an inlet.
pub(crate) fn river_course(seed: u64, p: FbmParams, x: f64, z: f64) -> Option<([f64; 2], f64)> {
    let region = region(seed, p, x, z);
    let mut nearest = 1.;
    let mut result = None;
    for r in region.candidates(x, z) {
        let t = projection_t(r, x, z);
        let dx = r.b.x - r.a.x;
        let dz = r.b.z - r.a.z;
        let width = r.width + t * (r.width_b - r.width);
        let fraction = (x - r.a.x - t * dx).hypot(z - r.a.z - t * dz) / width;
        if fraction >= 1. || r.a.stage + t * (r.b.stage - r.a.stage) <= 0. {
            continue;
        }
        if r.lake {
            return None;
        }
        if fraction < nearest {
            let length = dx.hypot(dz);
            if length > 0. {
                nearest = fraction;
                result = Some(([dx / length, dz / length], r.grade));
            }
        }
    }
    result
}

#[derive(Clone, Copy)]
struct ChannelMargin {
    bank_distance: f64,
    stage: f64,
    // Distance in bankfull radii and hydraulic grade at the nearest reach.
    width_fraction: f64,
    grade: f64,
    lake: bool,
}

// Both the point evaluator and its outward-rounded certificate use the same
// seeded fields. The long band shapes bars; the short band cuts shallow coves.
pub(crate) const MAX_BANK_INSET: f64 = 0.59;

pub(crate) fn bank_inset<V: Value>(bar: V, cove: V) -> V {
    let t = V::point(0.49)
        .add(bar.scale(1.05))
        .add(cove.scale(0.42))
        .clamp01();
    t.mul(t)
        .mul(V::point(3.).add(t.scale(-2.)))
        .scale(MAX_BANK_INSET + 0.53)
        .add(V::point(-0.53))
}

pub(crate) const BAR_BAND: (f64, u64) = (58., 0xb421);
pub(crate) const COVE_BAND: (f64, u64) = (19., 0xb422);

struct Hydrography {
    land: Land,
    ground: f64,
    water: Option<f64>,
    riparian: bool,
    channel: Option<ChannelMargin>,
}

// Height probes need the same channel bed and hydraulic stage as material
// queries. Tracking the nearest green bank is only necessary for biome paint.
fn hydrography<const BANKS: bool>(seed: u64, p: FbmParams, x: f64, z: f64) -> Hydrography {
    let l = land(seed, p, x, z);
    let mut ground = l.h;
    let mut water = if ground < 0. { Some(0.) } else { None };
    let watershed = p.recipe.uses_watersheds();
    let relief = p.amplitude / 1000.;
    let land_water_ceiling = (l.h + WATERSHED_WATER_ABOVE_LAND * relief).max(0.);
    let mut nearest_bank: Option<ChannelMargin> = None;
    if p.amplitude > 0. {
        let region = region(seed, p, x, z);
        let mut weighted_ground = ground;
        let mut original_weighted_ground = ground;
        let mut total_weight = 1.;
        let mut river_stage = f64::NEG_INFINITY;
        let mut lake_stage: Option<f64> = None;
        let mut near_lake = false;
        let mut inset = None;
        let mut profile_distance = |d: f64, width: f64| {
            if !p.has_shaped_banks() || d >= 1.8 * width {
                return d;
            }
            let inset = *inset.get_or_insert_with(|| {
                let sx = x * 1800. / p.horizontal_scale;
                let sz = z * 1800. / p.horizontal_scale;
                let band = |(w, salt)| noise(sx / w, sz / w, seed ^ salt);
                bank_inset(band(BAR_BAND), band(COVE_BAND))
            });
            let r = d / width;
            let bar = inset.max(0.) * (1. - smooth((r - 1.) / 0.8));
            // A scoured cove returns to bank height at the original outer
            // water radius. Its visible waterline always runs into solid ground.
            let cove = inset.min(0.) * (1. - smooth((r - 0.66) / 0.34));
            d + d.min(width).min(32. * p.horizontal_scale / 1800.) * (bar + cove)
        };
        for r in region.candidates(x, z) {
            let (d, stage) = project(r, x, z);
            let outer = r.outer_radius();
            let t = projection_t(r, x, z);
            let width = r.width + t * (r.width_b - r.width);
            if d >= outer || stage < 0. {
                continue;
            }
            near_lake |= r.lake;
            let amplitude = p.amplitude / 1000.;
            let depth = r.depth * amplitude;
            let bank_height = 0.7 * amplitude;
            let bank_d = profile_distance(d, width);
            let bed_profile = |radius: f64| {
                let height = if radius <= width {
                    stage - depth + (depth + bank_height) * smooth(radius / width)
                } else {
                    stage
                        + bank_height
                        + (l.h - stage - bank_height) * smooth((radius - width) / (outer - width))
                };
                if watershed || (stage == 0. && l.h < 0.) {
                    height.min(l.h)
                } else {
                    height
                }
            };
            let profile = bed_profile(bank_d);
            let taper = 1. - smooth(d / outer);
            let weight = outer * outer / (d * d).max(width * width * 1e-8) * taper * taper;
            weighted_ground += weight * profile;
            if p.has_shaped_banks() {
                let original = if bank_d == d { profile } else { bed_profile(d) };
                original_weighted_ground += weight * original;
            }
            total_weight += weight;
            if BANKS && watershed && stage > 0. {
                // Use the locally capped water level and a channel-scaled floodplain.
                // Compare the final combined bed against this stage after blending.
                let bank_width = (width * 4.5).max(28. * p.horizontal_scale / 1800.);
                let distance = d / bank_width;
                if distance < 1. && nearest_bank.is_none_or(|best| distance < best.bank_distance) {
                    nearest_bank = Some(ChannelMargin {
                        bank_distance: distance,
                        stage: stage.min(land_water_ceiling),
                        width_fraction: bank_d / width,
                        grade: r.grade,
                        lake: r.lake,
                    });
                }
            }
            if d < width {
                if r.lake && profile < stage {
                    lake_stage = Some(lake_stage.map_or(stage, |old| old.min(stage)));
                }
                river_stage = river_stage.max(stage);
            }
        }
        ground = weighted_ground / total_weight;
        let original_ground = if p.has_shaped_banks() {
            original_weighted_ground / total_weight
        } else {
            ground
        };
        if let Some(stage) = lake_stage {
            // Tributaries can lift the blended shore above the lake level.
            // Flat lake priority applies only to its actual wet interior.
            if stage > ground && (!watershed || p.legacy_riverbanks) {
                river_stage = stage;
            }
        }
        if watershed && !p.legacy_riverbanks && river_stage > ground.max(0.) {
            // At a confluence the blended bed can sit below either reach's
            // nominal bank. Limit each reach to its own shrinking cross-section
            // before taking their union. Sea level is continuous offshore.
            let mut supported_stage = 0.0_f64;
            for r in region.candidates(x, z) {
                let t = projection_t(r, x, z);
                let width = r.width + t * (r.width_b - r.width);
                let (d, stage) = project(r, x, z);
                if d < width && stage > supported_stage {
                    let depth = r.depth * relief;
                    let bank_d = profile_distance(d, width);
                    let head = (depth - (depth + 0.7 * relief) * smooth(bank_d / width)).max(0.);
                    let mut waterline = stage.min(ground + head);
                    if p.has_shaped_banks() {
                        // Sediment can redirect the shoreline while the old hydraulic
                        // ceiling still controls shared wet columns at confluences.
                        let old_head = (depth - (depth + 0.7 * relief) * smooth(d / width)).max(0.);
                        waterline = waterline
                            .min(original_ground + old_head)
                            .min((original_ground + WATERSHED_MAX_WATER_DEPTH * relief).max(0.));
                    }
                    supported_stage = supported_stage.max(waterline);
                }
            }
            river_stage = river_stage.min(supported_stage);
            if near_lake {
                // Draw tributaries down to the receiving basin before the bed
                // crosses the lake level. A binary lake-priority switch here
                // leaves an abrupt water wall on the first shallow voxel.
                let upstream = river_stage;
                for lake in region
                    .candidates(x, z)
                    .filter(|r| r.lake && r.a.stage < upstream)
                {
                    let (d, stage) = project(lake, x, z);
                    let radius = lake.outer_radius();
                    if d < radius {
                        let away = smooth(((d - lake.width) / (radius - lake.width)).clamp(0., 1.));
                        river_stage = river_stage.min(stage + (upstream - stage) * away);
                    }
                }
            }
        }
        if river_stage > ground {
            water = Some(water.map_or(river_stage, |w: f64| w.max(river_stage)));
        }
    }
    if watershed {
        // A graph reach can cross a low saddle between its two nodes. Keep
        // the local water in the valley instead of suspending the nominal
        // interpolated stage above the surrounding terrain.
        water = water.map(|stage| {
            stage
                .min(land_water_ceiling)
                .min((ground + WATERSHED_MAX_WATER_DEPTH * relief).max(0.))
        });
    }
    let riparian = nearest_bank.is_some_and(|bank| {
        let stage = bank.stage;
        stage > 0. && (stage - 2. * relief..=stage + 7. * relief).contains(&ground)
    });
    Hydrography {
        land: l,
        ground,
        water: water.filter(|w| *w > ground),
        riparian,
        channel: nearest_bank,
    }
}

fn quantized_tops(ground: f64, water: Option<f64>) -> [i64; 2] {
    let pitch = voxel_size();
    let bed = (ground / pitch).floor() as i64;
    [
        bed,
        water
            .map_or(bed, |stage| (stage / pitch).floor() as i64)
            .max(bed),
    ]
}

pub(super) fn column_tops(seed: u64, p: FbmParams, cx: i64, cz: i64) -> [i64; 2] {
    let x = (cx as f64 + 0.5) * voxel_size();
    let z = (cz as f64 + 0.5) * voxel_size();
    let profile = hydrography::<false>(seed, p, x, z);
    quantized_tops(profile.ground, profile.water)
}

pub(super) fn fields(seed: u64, p: FbmParams, x: f64, z: f64) -> SeededFields {
    site_fields(seed, p, x, z).0
}

fn site_fields(seed: u64, p: FbmParams, x: f64, z: f64) -> (SeededFields, Option<ChannelMargin>) {
    let Hydrography {
        land: l,
        ground,
        water,
        riparian,
        channel,
    } = hydrography::<true>(seed, p, x, z);
    let [temperature, moisture] = climate(seed, p, x, z);
    let effective_temperature = temperature - 0.00019 * ground.max(0.);
    let biome = if ground < -4. {
        SeededBiome::Ocean
    } else if ground >= 0. && effective_temperature >= 0.29 && riparian {
        SeededBiome::Wetland
    } else if ground < 10. {
        SeededBiome::Coast
    } else if effective_temperature < 0.29 {
        SeededBiome::Tundra
    } else if l.mountain > 0.62 && ground > 350. * p.amplitude / 1000. {
        SeededBiome::Mountains
    } else if moisture < 0.39 && effective_temperature > 0.48 {
        SeededBiome::Desert
    } else if p.recipe.uses_watersheds() && moisture >= 0.56 {
        SeededBiome::Woodland
    } else if p.recipe.uses_watersheds() && moisture < 0.44 {
        SeededBiome::Steppe
    } else {
        SeededBiome::Plains
    };
    (
        SeededFields {
            continentalness: l.continental,
            temperature: effective_temperature,
            moisture,
            mountain: l.mountain,
            biome,
            ground_height_m: ground,
            water_height_m: water,
        },
        channel,
    )
}
pub(super) fn column(seed: u64, p: FbmParams, x: f64, z: f64) -> TerrainColumn {
    let (fields, channel) = site_fields(seed, p, x, z);
    column_from_fields(seed, p, x, z, fields, channel)
}

// Shallow sand bars break into stony riffles as the hydraulic grade rises.
// Deposits cross the waterline only in low, connected patches near a reach.
fn river_sediment(
    seed: u64,
    p: FbmParams,
    x: f64,
    z: f64,
    fields: SeededFields,
    channel: ChannelMargin,
    wet: bool,
) -> Option<Material> {
    let relief = p.amplitude / 1000.;
    let elevation = fields.ground_height_m - channel.stage;
    if channel.width_fraction > 1.18
        || (!wet && (elevation < -0.08 * relief || elevation > 0.58 * relief))
    {
        return None;
    }
    let scale = 1800. / p.horizontal_scale;
    let (x, z) = (x * scale, z * scale);
    let bar =
        0.70 * noise(x / 24., z / 24., seed ^ 0x51f1) + 0.30 * noise(x / 7., z / 7., seed ^ 0x51f2);
    if !wet && (bar < -0.04 || elevation > (0.24 + 0.38 * bar) * relief) {
        return None;
    }
    let grains = noise(x / 1.8, z / 1.8, seed ^ 0x51f3);
    let grade = if channel.lake {
        0.
    } else {
        // Grade is dimensionless. Normalize for the recipe's horizontal and
        // vertical scaling so equivalent valleys expose the same deposits.
        channel.grade / (scale * relief).max(1e-9)
    };
    let energy = smooth((grade - 0.012) / 0.13);
    let stone = 0.83 * energy + 0.66 * bar + 0.18 * grains;
    // A fast reach packs more cobbles together. Small pockets between them
    // retain sand even at high grades. One cobble spans several native cells.
    let cobble = stone > 0.20
        && noise(x / 0.44, z / 0.44, seed ^ 0x51f4) > (0.68 - 1.25 * stone).max(-0.55);
    let sand = bar + 0.19 * grains + 0.55 * energy;
    Some(if cobble {
        Material::Stone
    } else if sand > 0.08
        || (sand > -0.32 && sand + 0.20 * noise(x / 0.65, z / 0.65, seed ^ 0x51f5) > -0.12)
    {
        Material::Sand
    } else {
        Material::Soil
    })
}

fn column_from_fields(
    seed: u64,
    p: FbmParams,
    x: f64,
    z: f64,
    f: SeededFields,
    channel: Option<ChannelMargin>,
) -> TerrainColumn {
    let [ground_top, visible_top] = quantized_tops(f.ground_height_m, f.water_height_m);
    let mut surface = match f.biome {
        SeededBiome::Ocean | SeededBiome::Coast => Material::Sand,
        SeededBiome::Woodland | SeededBiome::Wetland => Material::Grass,
        SeededBiome::Steppe => {
            let x = x * 1800. / p.horizontal_scale;
            let z = z * 1800. / p.horizontal_scale;
            // Even dry steppe has grass. Reserve connected bare patches for
            // the driest ground where broad and local cover both thin out.
            let cover = 0.7 * noise(x / 125., z / 125., seed ^ 0x5123)
                + 0.3 * noise(x / 41., z / 41., seed ^ 0x5125);
            if f.moisture + 0.18 * cover < 0.20 {
                Material::Soil
            } else {
                Material::Grass
            }
        }
        SeededBiome::Tundra => {
            if f.temperature < 0.18 {
                Material::Snow
            } else if f.moisture < 0.42 {
                Material::Soil
            } else {
                Material::Grass
            }
        }
        SeededBiome::Mountains => {
            if f.temperature < 0.36 {
                Material::Snow
            } else if p.recipe.uses_watersheds() && f.moisture >= 0.46 {
                let x = x * 1800. / p.horizontal_scale;
                let z = z * 1800. / p.horizontal_scale;
                let patch = noise(x / 190., z / 190., seed ^ 0x5124);
                if f.temperature + 0.12 * patch > 0.41 && patch > -0.08 {
                    Material::Grass
                } else {
                    Material::Stone
                }
            } else {
                Material::Stone
            }
        }
        SeededBiome::Desert | SeededBiome::Plains => {
            // Shift only the moisture transition belt with connected meter-scale
            // turf/sand tongues. No hash dithering or per-face color variation.
            let shift = if (f.moisture - 0.39).abs() < 0.075 {
                let nx = x * 1800. / p.horizontal_scale;
                let nz = z * 1800. / p.horizontal_scale;
                0.075
                    * (0.75 * noise(nx / 180. + 3.1, nz / 180. - 7.3, seed ^ 0x51a1)
                        + 0.25 * noise(nx / 45. - 8.9, nz / 45. + 4.7, seed ^ 0x51a2))
            } else {
                0.
            };
            if f.temperature > 0.48 && f.moisture < 0.39 + shift {
                Material::Sand
            } else {
                Material::Grass
            }
        }
    };
    if p.recipe == FbmRecipe::Highlands
        && f.mountain > 0.32
        && f.ground_height_m > 440. * p.amplitude / 1000.
    {
        // Broken snowfields follow the local snow line. Exposed ribs and rock
        // interrupt the blanket of snow as the wind scours the upper slopes.
        let scale = 1800. / p.horizontal_scale;
        let nx = x * scale;
        let nz = z * scale;
        let shelter = fbm(seed, nx, nz, 480., 3, 0.48, 0x5177);
        let snow_line = 0.22 + 0.14 * shelter;
        if f.temperature < snow_line && shelter > -0.40 {
            surface = Material::Snow;
        } else if f.temperature < 0.35 || shelter < -0.34 {
            surface = Material::Stone;
        }
    }
    if visible_top > ground_top {
        surface = if matches!(
            f.biome,
            SeededBiome::Ocean | SeededBiome::Coast | SeededBiome::Desert
        ) {
            Material::Sand
        } else {
            Material::Soil
        };
    }
    let deposit = if p.alluvial_deposits && !p.legacy_riverbanks {
        channel.and_then(|c| river_sediment(seed, p, x, z, f, c, visible_top > ground_top))
    } else {
        None
    };
    let surface = deposit.unwrap_or(surface);
    TerrainColumn {
        ground_top,
        visible_top,
        ground_surface: surface,
        ground_subsurface: deposit.unwrap_or(if surface == Material::Sand {
            Material::Sand
        } else {
            Material::Soil
        }),
    }
}

/// Subpixel terrain albedo at a canonical column. The same field is sampled
/// by streamed mountain meshes and the flight horizon at shared positions.
pub(super) fn landscape(
    seed: u64,
    p: FbmParams,
    cx: i64,
    cz: i64,
    vegetation: bool,
) -> (TerrainColumn, [f32; 3]) {
    let x = (cx as f64 + 0.5) * voxel_size();
    let z = (cz as f64 + 0.5) * voxel_size();
    let (f, channel) = site_fields(seed, p, x, z);
    let col = column_from_fields(seed, p, x, z, f, channel);
    let mat = col.top_material();
    let scale = 1800. / p.horizontal_scale;
    let (nx, nz) = (x * scale, z * scale);
    let stone = || noise(nx / 420., nz / 420., seed ^ 0x517a);
    let mix = |a: [f32; 3], b: [f32; 3], u: f64| {
        std::array::from_fn(|i| a[i] + (b[i] - a[i]) * u.clamp(0., 1.) as f32)
    };
    let rgb = match mat {
        Material::Grass => {
            let turf = noise(nx / 240., nz / 240., seed ^ 0x5179);
            let lush = smooth((f.moisture - 0.25) * 2.4 + 0.22 * turf);
            let meadow = mix([0.36, 0.33, 0.125], [0.135, 0.28, 0.09], lush);
            let canopy = if vegetation {
                crate::vegetation::landscape_canopy(seed, cx, cz, f)
            } else {
                0.
            };
            mix(meadow, [0.086, 0.145, 0.077], canopy)
        }
        Material::Stone if p.recipe == FbmRecipe::Highlands => {
            // Sample a stratigraphic section along folded beds. Independent
            // units vary in thickness and pinch out along strike.
            let province = noise(nx / 1750., nz / 1750., seed ^ 0xa76d);
            let weather = stone();
            let relief = f.ground_height_m / (p.amplitude / 1000.).max(0.01);
            let depth = relief + nx * 0.14 - nz * 0.09 + 68. * weather + 145. * province;
            let strike = (nx * 0.54 + nz * 0.84) / 1660.;
            let formation = noise(depth / 155., strike, seed ^ 0xda01);
            let beds = noise(depth / 36., strike * 2.2, seed ^ 0xda02);
            let strata = smooth(0.55 + province * 0.9);
            let granite = mix(
                [0.25, 0.285, 0.315],
                [0.43, 0.405, 0.365],
                0.55 + weather * 0.7,
            );
            let sandstone = mix(
                [0.31, 0.30, 0.28],
                [0.51, 0.425, 0.32],
                0.54 + formation * 0.86 + weather * 0.28,
            );
            let shale = smooth((0.44 - formation - beds * 0.52) * 1.18);
            mix(
                granite,
                mix(sandstone, [0.235, 0.257, 0.268], shale * 0.73),
                strata,
            )
        }
        Material::Snow if p.recipe == FbmRecipe::Highlands => {
            // Wind-scoured firn and fresher drifts have distinct albedos.
            let drift = noise(
                (nx + nz * 0.32) / 275.,
                (nz - nx * 0.32) / 94.,
                seed ^ 0xda03,
            );
            mix(
                [0.72, 0.80, 0.87],
                [0.925, 0.946, 0.968],
                smooth(0.68 + drift * 0.60),
            )
        }
        Material::Stone => mix(
            [0.32, 0.345, 0.37],
            [0.46, 0.38, 0.295],
            0.48 + stone() * 0.85,
        ),
        Material::Sand => mix(
            [0.45, 0.39, 0.27],
            [0.65, 0.55, 0.35],
            0.56 + stone() * 0.75,
        ),
        _ => mat.color(),
    };
    (col, rgb)
}

/// Analytical pre-quantization envelope; ordinary f64 endpoints are not an
/// outward-rounded certificate. No sampled extrema are used.
/// Gradient noise has Euclidean Lipschitz constant <=16: corner gradients have
/// norm one, quintic fade derivative <=1.875, corner differences <=2sqrt(2),
/// and output scaling is 1.6; clamping cannot increase that bound.
pub(super) fn height_bounds(seed: u64, p: FbmParams, min: [f64; 2], max: [f64; 2]) -> [f64; 2] {
    let a = p.amplitude / 1000.;
    let global = global_height_range(p);
    if p.recipe.uses_watersheds() {
        // The affine path supplies the local conservative bounds for this recipe.
        return global;
    }
    let x = (min[0] + max[0]) * 0.5;
    let z = (min[1] + max[1]) * 0.5;
    let radius = ((max[0] - min[0]) * 0.5).hypot((max[1] - min[1]) * 0.5);
    let scale = 1800. / p.horizontal_scale;
    let band = |w: f64, count: u32, gain: f64, salt: u64| {
        let (mut weight, mut norm, mut sum, mut uncertainty, mut f) = (1., 0., 0., 0., 1. / w);
        for i in 0..count {
            let n = noise(
                x * scale * f + 13.7,
                z * scale * f - 8.3,
                seed ^ salt.wrapping_add(i as u64 * 0x9e3779b9),
            );
            let e = 16. * radius * scale * f;
            sum += weight * n;
            uncertainty += weight * e;
            norm += weight;
            weight *= gain;
            f *= 2.;
        }
        [
            ((sum - uncertainty) / norm).max(-1.),
            ((sum + uncertainty) / norm).min(1.),
        ]
    };
    let c = band(48000., 3, 0.45, 0x5101);
    let province = band(22000., 3, 0.45, 0x5102);
    let m = province.map(|v| smooth((v + 0.08) / 0.48));
    let r = band(3600., 4, 0.48, 0x5105);
    let abslo = if r[0] <= 0. && r[1] >= 0. {
        0.
    } else {
        r[0].abs().min(r[1].abs())
    };
    let ridge = [1. - r[0].abs().max(r[1].abs()), 1. - abslo];
    let hills = band(1800., 3, 0.45, 0x5106);
    let fine = band(90., p.octaves, 0.65 * p.roughness, 0x5107);
    let base = [0, 1].map(|i| {
        a * (400. * c[i]
            + 65.
            + 1150. * m[i] * m[i] * ridge[i] * ridge[i]
            + 45. * hills[i]
            + 3. * fine[i])
    });
    let mut result = [base[0], base[1].max(0.)];
    let spacing = 2400. * p.horizontal_scale / 1800.;
    if max[0] - min[0] > spacing * 4. || max[1] - min[1] > spacing * 4. {
        return global;
    }
    for cx in [min[0], max[0]] {
        for cz in [min[1], max[1]] {
            let region = region(seed, p, cx, cz);
            for &r in &region.reaches {
                let (distance, stage) = project(r, x, z);
                let dlo = (distance - radius).max(0.);
                if dlo >= r.outer_radius() || r.a.stage.max(r.b.stage) < 0. {
                    continue;
                }
                let length = (r.a.x - r.b.x).hypot(r.a.z - r.b.z);
                let e = if length > 0. {
                    (r.a.stage - r.b.stage).abs() * radius / length
                } else {
                    0.
                };
                let slo = (stage - e).max(r.a.stage.min(r.b.stage)).max(0.);
                let shi = (stage + e).min(r.a.stage.max(r.b.stage)).max(0.);
                // Every conditioned height is a convex combination of the
                // original source and compact valley profiles. This diagnostic
                // hull intentionally drops weights; certified callers use AA.
                let depth = r.depth * a;
                let profile_lo = if dlo < r.width.max(r.width_b) {
                    slo - depth
                } else {
                    slo + 0.7 * a
                };
                result[0] = result[0].min(profile_lo.min(base[0]));
                result[1] = result[1].max(base[1].max(shi + 0.7 * a));
            }
        }
    }
    [result[0].max(global[0]), result[1].min(global[1])]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::{FbmRecipe, FbmTerrain};
    fn params() -> FbmParams {
        FbmParams {
            recipe: FbmRecipe::SeededWorld,
            ..Default::default()
        }
    }

    #[test]
    fn feature_boxes_follow_diagonal_channels_and_their_tapered_caps() {
        let node = |x, z| Node {
            x,
            z,
            stage: 5.,
            potential: 5.,
        };
        let reach = Reach {
            a: node(-240., -80.),
            b: node(160., 220.),
            width: 8.,
            width_b: 28.,
            depth: 2.,
            grade: 0.,
            valley_radius: 60.,
            lake: false,
            owner: [0; 2],
            segment: 0,
        };
        assert!(reach.core_radii([-230., 160.], [-212., 180.]).is_none());
        for i in 0..=20 {
            let t = f64::from(i) / 20.;
            let radius = reach.width + t * (reach.width_b - reach.width);
            for j in 0..16 {
                let a = f64::from(j) * std::f64::consts::TAU / 16.;
                let x = reach.a.x + t * 400. + radius * a.cos();
                let z = reach.a.z + t * 300. + radius * a.sin();
                let projected = projection_t(reach, x, z);
                let actual = reach.width + projected * (reach.width_b - reach.width);
                if project(reach, x, z).0 <= actual {
                    let [lo, hi] = reach
                        .core_radii([x - 0.5, z - 2.], [x + 0.5, z + 2.])
                        .unwrap();
                    assert!(lo <= actual && actual <= hi);
                }
            }
        }
        let [lo, hi] = reach.core_radii([154., 218.], [160., 220.]).unwrap();
        assert!(lo > 27. && hi >= 28.);
        let lake = Reach {
            b: reach.a,
            width_b: reach.width,
            ..reach
        };
        assert!(lake.core_radii([-232., -80.], [-232., -80.]).is_some());
    }

    #[test]
    fn drainage_cache_distinguishes_recipes() {
        let seeded = params();
        let watershed = FbmParams {
            recipe: FbmRecipe::Watershed,
            ..seeded
        };
        let original = region(997, seeded, -1., 1.);
        let other = region(997, watershed, -1., 1.);
        assert!(!Rc::ptr_eq(&original, &other));
        assert!(Rc::ptr_eq(&original, &region(997, seeded, -1., 1.)));
    }

    #[test]
    fn watershed_keeps_dry_riverbanks_green_and_steppe_grassy() {
        let p = FbmParams {
            recipe: FbmRecipe::Watershed,
            ..params()
        };
        let mut dry_grassy_bank = 0;
        let mut steppe_grass = 0;
        let mut steppe_soil = 0;
        let mut desert_sand = 0;
        for z in -60..=60 {
            for x in -60..=60 {
                let (x, z) = (64. * f64::from(x) + 5., 64. * f64::from(z) + 7.);
                let f = fields(19, p, x, z);
                let c = column(19, p, x, z);
                match (f.biome, c.top_material()) {
                    (SeededBiome::Wetland, Material::Grass) if f.moisture < 0.40 => {
                        dry_grassy_bank += 1;
                    }
                    (SeededBiome::Steppe, Material::Grass) => steppe_grass += 1,
                    (SeededBiome::Steppe, Material::Soil) => steppe_soil += 1,
                    (SeededBiome::Desert, Material::Sand) => desert_sand += 1,
                    _ => {}
                }
            }
        }
        assert!(dry_grassy_bank > 20, "dry grassy banks {dry_grassy_bank}");
        assert!(steppe_grass > steppe_soil && steppe_soil > 0);
        assert!(desert_sand > 100);
    }

    #[test]
    fn alluvial_source_sorts_native_beds_and_keeps_the_previous_water_stage() {
        let params = FbmParams {
            shaped_banks: false,
            recipe: FbmRecipe::Watershed,
            ..Default::default()
        };
        let fresh = FbmTerrain::new(997, params).unwrap();
        let old = FbmTerrain::new(
            997,
            FbmParams {
                alluvial_deposits: false,
                ..params
            },
        )
        .unwrap();
        assert_eq!((old.source_version(), fresh.source_version()), (2, 3));
        for (center, dominant) in [
            ([-791., 987.], Material::Sand),
            ([-1194., 1223.], Material::Stone),
        ] {
            let mut wet = 0;
            let mut deposits = 0;
            let mut exposed_bank = 0;
            for dz in -28..=28 {
                for dx in -28..=28 {
                    let [x, z] = [center[0] + f64::from(dx), center[1] + f64::from(dz)]
                        .map(|v| (v / voxel_size()).floor() as i64);
                    let a = fresh.column_cells(x, z);
                    let b = old.column_cells(x, z);
                    assert_eq!((a.ground_top, a.visible_top), (b.ground_top, b.visible_top));
                    if a.visible_top > a.ground_top {
                        assert_eq!(b.ground_surface, Material::Soil);
                        assert_eq!(a.material(a.ground_top), Material::Water);
                        assert_eq!(a.material(a.visible_top), Material::Air);
                        wet += 1;
                        deposits += usize::from(a.material(a.ground_top - 1) == dominant);
                    } else if a.ground_surface != b.ground_surface {
                        exposed_bank += 1;
                    }
                }
            }
            assert!(wet > 100 && deposits > wet / 2 && exposed_bank > 0);
        }
    }

    #[test]
    fn bars_and_coves_move_both_banks_within_the_existing_hydraulic_ceiling() {
        let params = FbmParams {
            recipe: FbmRecipe::Watershed,
            ..params()
        };
        let source = super::super::FbmTerrain::new(997, params).unwrap();
        let earlier = super::super::FbmTerrain::new(
            997,
            FbmParams {
                shaped_banks: false,
                ..params
            },
        )
        .unwrap();
        assert_eq!((earlier.source_version(), source.source_version()), (3, 4));
        let mut bars = 0;
        let mut coves = 0;
        let mut shared_water = 0;
        for (x, z) in [(-791., 987.), (-1194., 1223.)] {
            for dz in -55..=55 {
                for dx in -55..=55 {
                    let cell = crate::Cell::from_meters([x + f64::from(dx), 0., z + f64::from(dz)]);
                    let before = earlier.column_cells(cell.x, cell.z);
                    let after = source.column_cells(cell.x, cell.z);
                    let was_wet = before.visible_top > before.ground_top;
                    let wet = after.visible_top > after.ground_top;
                    if was_wet && wet {
                        shared_water += 1;
                        assert!(
                            after.visible_top <= before.visible_top,
                            "raised stage at {cell:?}"
                        );
                    } else if was_wet {
                        bars += 1;
                        assert_ne!(after.material(after.visible_top - 1), Material::Water);
                    } else if wet {
                        coves += 1;
                        assert_eq!(after.material(after.ground_top), Material::Water);
                        assert_ne!(after.material(after.ground_top - 1), Material::Water);
                    }
                }
            }
        }
        assert!(
            bars > 60 && coves > 60 && shared_water > 1000,
            "{bars} {coves} {shared_water}"
        );
    }

    #[test]
    fn sampled_river_course_follows_the_downstream_reach_and_ends_at_lakes() {
        let p = FbmParams { recipe: FbmRecipe::Watershed, ..params() };
        let terrain = super::super::FbmTerrain::new(997, p).unwrap();
        let mut rivers = 0;
        let mut lakes = 0;
        for reach in &region(997, p, 0., 0.).reaches {
            let x = (reach.a.x + reach.b.x) * 0.5;
            let z = (reach.a.z + reach.b.z) * 0.5;
            if !(0. ..9600.).contains(&x) || !(0. ..9600.).contains(&z) {
                continue;
            }
            let course = terrain.river_course(x, z);
            if reach.lake {
                assert!(course.is_none());
                lakes += 1;
            } else if let Some(([cx, cz], grade)) = course {
                let dx = reach.b.x - reach.a.x;
                let dz = reach.b.z - reach.a.z;
                let length = dx.hypot(dz);
                assert!((cx * dx + cz * dz) / length > 0.99999);
                assert!((grade - (reach.a.stage - reach.b.stage) / length).abs() < 1e-9);
                rivers += 1;
            }
        }
        assert!(rivers > 24 && lakes > 0, "{rivers} rivers, {lakes} lakes");
    }

    #[test]
    fn watershed_reaches_agree_across_halos_and_vary_with_runoff() {
        let p = FbmParams {
            recipe: FbmRecipe::Watershed,
            ..params()
        };
        let mut common = 0;
        let mut min_width = f64::INFINITY;
        let mut max_width: f64 = 0.;
        for seed in [19, 83, 40961] {
            for z in [-9601., -1., 1., 9601.] {
                let west = region(seed, p, -1., z);
                let east = region(seed, p, 1., z);
                for r in &west.reaches {
                    if let Some(other) = east
                        .reaches
                        .iter()
                        .find(|b| b.owner == r.owner && b.segment == r.segment)
                    {
                        common += 1;
                        let fingerprint = |r: &Reach| {
                            [
                                r.a.x, r.a.z, r.a.stage, r.b.x, r.b.z, r.b.stage, r.width,
                                r.width_b, r.depth,
                            ]
                            .map(f64::to_bits)
                        };
                        assert_eq!(fingerprint(r), fingerprint(other));
                    }
                    assert!(r.b.stage <= r.a.stage);
                    if !r.lake {
                        min_width = min_width.min(r.width);
                        max_width = max_width.max(r.width);
                    }
                }
            }
        }
        assert!(common > 100);
        assert!(max_width > min_width * 2., "{min_width}..{max_width}");
    }
    #[test]
    fn perched_river_regressions_and_saved_source_replay() {
        for (seed, recipe, x, z, dx, dz, old_lip) in [
            (673, FbmRecipe::Highlands, -34728, -3328, -1, 0, 144),
            (997, FbmRecipe::Watershed, 520, 5657, 0, -1, 39),
        ] {
            let current = FbmParams {
                recipe,
                ..Default::default()
            };
            let saved = FbmParams {
                legacy_riverbanks: true,
                ..current
            };
            let at = |p, dx: i64, dz: i64| column_tops(seed, p, x + dx, z + dz);
            // Exercise shared thread-local caches in both source orders.
            for p in [saved, current, saved, current] {
                let bank = at(p, dx, dz);
                let river = at(p, 0, 0);
                if p.legacy_riverbanks {
                    assert_eq!(bank[0], bank[1]);
                    assert_eq!(river[1] - bank[1], old_lip);
                } else {
                    assert!(river[1].abs_diff(bank[1]) <= 2);
                }
            }
        }
    }
    #[test]
    fn watershed_banks_agree_across_drainage_region_boundary() {
        let p = FbmParams {
            recipe: FbmRecipe::Highlands,
            ..Default::default()
        };
        let x = -1991.65625;
        let z = 0.;
        let collect = |side| {
            let region = region(673, p, x, side);
            let mut list = Vec::new();
            for r in region.candidates(x, side) {
                let (d, stage) = project(r, x, z);
                if d < r.outer_radius() {
                    list.push((r.owner, r.segment, r.lake, d, stage, r.width, r.width_b));
                }
            }
            list.sort_by_key(|r| (r.0, r.1));
            list
        };
        assert_eq!(collect(-0.03), collect(0.03));
    }
    #[test]
    fn deterministic_columns_survive_negative_boundaries_and_cache_eviction() {
        let p = params();
        let a = FbmTerrain::new(7, p).unwrap();
        let b = FbmTerrain::new(7, p).unwrap();
        let locations = [
            (-153601, -1),
            (-153600, 0),
            (-1, -1),
            (0, 0),
            (1, 1),
            (153599, -153600),
            (153600, -153601),
        ];
        let expected: Vec<_> = locations
            .iter()
            .map(|&(x, z)| a.column_cells(x, z))
            .collect();
        for i in 0..16 {
            let _ = a.height_m(i as f64 * 19201. - 100000., i as f64 * -19003.);
        }
        for (&(x, z), &column) in locations.iter().zip(&expected).rev() {
            assert_eq!(column, b.column_cells(x, z));
            assert!(column.visible_top >= column.ground_top);
            assert_eq!(column.material(column.visible_top), Material::Air);
            assert_eq!(
                column.material(column.ground_top - 1),
                column.ground_surface
            );
            assert_eq!(column.material(column.ground_top - 13), Material::Stone);
        }
        let other = FbmTerrain::new(8, p).unwrap();
        assert!(locations
            .iter()
            .any(|&(x, z)| a.column_cells(x, z) != other.column_cells(x, z)));
    }
    #[test]
    fn node_stages_are_shared_monotone_and_sink_lakes_are_level() {
        let p = params();
        let mut wet = 0;
        let mut lakes = 0;
        for x in -5..=5 {
            for z in -5..=5 {
                let region = region(7, p, x as f64 * 2400., z as f64 * 2400.);
                for &r in &region.reaches {
                    assert!(r.a.stage >= r.b.stage);
                    if r.a.stage < 0. {
                        continue;
                    }
                    for t in [0., 0.25, 0.5, 0.75, 1.] {
                        let x = r.a.x + (r.b.x - r.a.x) * t;
                        let z = r.a.z + (r.b.z - r.a.z) * t;
                        let f = fields(7, p, x, z);
                        let stage = r.a.stage + (r.b.stage - r.a.stage) * t;
                        assert!(
                            f.ground_height_m < stage,
                            "channel lost at {x},{z}; stage={stage}, fields={f:?}, reach={r:?}"
                        );
                        let water = f.water_height_m.unwrap_or_else(||panic!("continuous wet channel lost at {x},{z}: stage={stage}, fields={f:?}, reach={r:?}"));
                        if water < stage - 1e-9 {
                            assert!(
                                super::region(7, p, x, z)
                                    .candidates(x, z)
                                    .any(|lake| lake.lake && lake.a.stage == water),
                                "river stage fell without a shared sink lake at {x},{z}: {f:?}"
                            );
                        }
                        wet += 1;
                    }
                    if r.lake {
                        lakes += 1;
                        // Sink center and a short radius use the exact same stage.
                        let f = fields(7, p, r.a.x + 1., r.a.z);
                        assert_eq!(f.water_height_m, Some(r.a.stage));
                    }
                }
            }
        }
        assert!(wet > 0 && lakes > 0);
    }
    #[test]
    fn positive_river_reaches_sea_level_without_a_dry_coastal_gap() {
        let p = params();
        // Held-out regression: the destination's raw potential is -71.77m.
        // Previously skipping negative interpolated stages cut this river off
        // before the endpoint, even though it drains into the ocean.
        let region = region(7, p, -18000., -20400.);
        let mut path: Vec<_> = region
            .reaches
            .iter()
            .filter(|r| r.owner == [-8, -9])
            .copied()
            .collect();
        path.sort_by_key(|r| r.segment);
        assert_eq!(path.len(), 16);
        assert!(path[0].a.stage > 0. && path[15].b.potential < 0.);
        assert_eq!(path[15].b.stage, 0.);
        let mut previous = f64::INFINITY;
        for r in path {
            for t in [0., 0.25, 0.5, 0.75, 0.95, 0.99, 1.] {
                let x = r.a.x + t * (r.b.x - r.a.x);
                let z = r.a.z + t * (r.b.z - r.a.z);
                let stage = r.a.stage + t * (r.b.stage - r.a.stage);
                assert!(stage <= previous + 1e-9);
                previous = stage;
                let f = fields(7, p, x, z);
                assert!(
                    f.ground_height_m < stage,
                    "dry coast at segment={} t={t}: {f:?}",
                    r.segment
                );
                assert!(f.water_height_m.unwrap() >= stage - 1e-9);
                let c = column(7, p, x, z);
                assert!(
                    c.visible_top > c.ground_top,
                    "native dry coast at t={t}: {c:?}"
                );
            }
        }
    }
    #[test]
    fn variable_width_reaches_share_node_widths_and_stay_in_search_halo() {
        let p = params();
        let mut narrowest = f64::INFINITY;
        let mut widest: f64 = 0.;
        let mut changes = 0;
        let mut nodes = std::collections::HashMap::new();
        for seed in [7, 19, 83] {
            for (x, z) in [(-9600., -9600.), (0., 0.), (9600., 9600.)] {
                let region = region(seed, p, x, z);
                for r in region.reaches.iter().filter(|r| !r.lake) {
                    for width in [r.width, r.width_b] {
                        assert!((8. ..=24.).contains(&width));
                        narrowest = narrowest.min(width);
                        widest = widest.max(width);
                    }
                    if (r.width - r.width_b).abs() > 0.1 {
                        changes += 1;
                    }
                    assert!(r.outer_radius() + 0.16 * 2400. < 0.5 * 2400.);
                    for (endpoint, width, is_node) in [
                        (r.a, r.width, r.segment == 0),
                        (r.b, r.width_b, r.segment == 15),
                    ] {
                        if !is_node {
                            continue;
                        }
                        let key = (seed, endpoint.x.to_bits(), endpoint.z.to_bits());
                        if let Some(previous) = nodes.insert(key, width) {
                            assert_eq!(previous, width);
                        }
                    }
                    // Native projected centerline remains wet despite varying
                    // profiles and overlaps; widths never repaint solid water.
                    for t in [0., 0.25, 0.5, 0.75, 1.] {
                        let x = r.a.x + t * (r.b.x - r.a.x);
                        let z = r.a.z + t * (r.b.z - r.a.z);
                        let q = column(seed, p, x, z);
                        assert!(
                            q.visible_top > q.ground_top,
                            "dry seed={seed} owner={:?} segment={} t={t}",
                            r.owner,
                            r.segment
                        );
                    }
                }
            }
        }
        assert!(
            changes > 100,
            "variable width failed to vary along edges: {changes}"
        );
        assert!(
            widest > narrowest * 1.3,
            "width range too uniform {narrowest}..{widest}"
        );
    }
    #[test]
    fn meandering_edges_preserve_global_endpoints_stages_and_descriptor_identity() {
        let p = params();
        let owner = [-8, -9];
        let extract = |r: &Region| {
            let mut path: Vec<_> = r
                .reaches
                .iter()
                .filter(|v| v.owner == owner)
                .copied()
                .collect();
            path.sort_by_key(|v| v.segment);
            path
        };
        let original = region(7, p, -18000., -20400.);
        let path = extract(&original);
        assert_eq!(path.len(), 16);
        assert_eq!([path[0].a.x, path[0].a.z], [-19200., -21600.]);
        assert_eq!([path[15].b.x, path[15].b.z], [-16800., -19200.]);
        let a = path[0].a;
        let b = path[15].b;
        let dx = b.x - a.x;
        let dz = b.z - a.z;
        let length = dx.hypot(dz);
        let mut widest: f64 = 0.;
        for (i, r) in path.iter().enumerate() {
            assert_eq!(r.segment as usize, i);
            assert!(r.a.stage >= r.b.stage);
            if i > 0 {
                let previous = path[i - 1].b;
                assert_eq!(
                    [previous.x, previous.z, previous.stage],
                    [r.a.x, r.a.z, r.a.stage]
                );
            }
            let distance = ((r.a.x - a.x) * dz - (r.a.z - a.z) * dx).abs() / length;
            assert!(distance <= 0.16 * 2400. + 1e-8);
            widest = widest.max(distance);
        }
        assert!(widest > 60., "river remained straight: {widest}");
        let min = [
            path.iter()
                .map(|r| r.a.x.min(r.b.x))
                .fold(f64::INFINITY, f64::min)
                - 50.,
            path.iter()
                .map(|r| r.a.z.min(r.b.z))
                .fold(f64::INFINITY, f64::min)
                - 50.,
        ];
        let max = [
            path.iter()
                .map(|r| r.a.x.max(r.b.x))
                .fold(f64::NEG_INFINITY, f64::max)
                + 50.,
            path.iter()
                .map(|r| r.a.z.max(r.b.z))
                .fold(f64::NEG_INFINITY, f64::max)
                + 50.,
        ];
        let descriptors: Vec<_> = reaches_intersecting(7, p, min, max)
            .into_iter()
            .filter(|r| r.owner == owner)
            .collect();
        assert_eq!(
            descriptors.len(),
            16,
            "segment descriptors were collapsed by owner"
        );
        for i in 0..16 {
            assert_eq!(descriptors.iter().filter(|r| r.segment == i).count(), 1);
        }
        for i in 0..12 {
            let _ = region(7, p, i as f64 * 19200., -30000.);
        }
        let repeated = extract(&region(7, p, -18000., -20400.));
        for (a, b) in path.iter().zip(repeated) {
            assert_eq!(
                [
                    a.a.x.to_bits(),
                    a.a.z.to_bits(),
                    a.a.stage.to_bits(),
                    a.b.x.to_bits(),
                    a.b.z.to_bits(),
                    a.b.stage.to_bits()
                ],
                [
                    b.a.x.to_bits(),
                    b.a.z.to_bits(),
                    b.a.stage.to_bits(),
                    b.b.x.to_bits(),
                    b.b.z.to_bits(),
                    b.b.stage.to_bits()
                ]
            );
        }
    }
    #[test]
    fn sea_level_reach_caps_do_not_raise_seabed_rings_or_islands() {
        let p = params();
        let mut checked = 0;
        // Known negative-potential destination of an above-sea river.
        for ix in -10..=10 {
            for iz in -10..=10 {
                let x = -16800. + ix as f64 * 32.;
                let z = -19200. + iz as f64 * 32.;
                let base = land(7, p, x, z).h;
                if base >= -1. {
                    continue;
                }
                let local = region(7, p, x, z);
                let active: Vec<_> = local
                    .candidates(x, z)
                    .filter(|&r| project(r, x, z).0 < r.outer_radius())
                    .collect();
                if active.is_empty() || active.iter().any(|&r| project(r, x, z).1 != 0.) {
                    continue;
                }
                let f = fields(7, p, x, z);
                assert!(
                    f.ground_height_m <= base + 1e-10,
                    "sea bank raised from {base} to {f:?}"
                );
                assert_eq!(f.water_height_m, Some(0.));
                assert_eq!(column(7, p, x, z).visible_top, 0);
                checked += 1;
            }
        }
        assert!(
            checked > 10,
            "fixture missed sea-level influence caps: {checked}"
        );
    }
    #[test]
    fn seed19_river_valley_has_broad_shoulders_instead_of_a_narrow_wall() {
        let p = params();
        let anchor = [-896., 256.];
        let reaches =
            reaches_intersecting(19, p, anchor.map(|v| v - 600.), anchor.map(|v| v + 600.));
        let r = reaches
            .iter()
            .filter(|r| !r.lake)
            .min_by(|a, b| {
                project(**a, anchor[0], anchor[1])
                    .0
                    .total_cmp(&project(**b, anchor[0], anchor[1]).0)
            })
            .copied()
            .unwrap();
        let dx = r.b.x - r.a.x;
        let dz = r.b.z - r.a.z;
        let length = dx.hypot(dz);
        let t = (((anchor[0] - r.a.x) * dx + (anchor[1] - r.a.z) * dz) / (length * length))
            .clamp(0., 1.);
        let center = [r.a.x + t * dx, r.a.z + t * dz];
        let mut broad = 0.;
        let mut steepest: f64 = 0.;
        for sign in [-1., 1.] {
            for distance in [0., 14., 28., 42., 56., 112., 200., 252.] {
                let x = center[0] - dz / length * distance * sign;
                let z = center[1] + dx / length * distance * sign;
                let ground = fields(19, p, x, z).ground_height_m;
                let base = land(19, p, x, z).h;
                if distance == 56. {
                    broad = f64::max(broad, (ground - base).abs());
                }
                if (14. ..=112.).contains(&distance) {
                    let next = fields(19, p, x - dz / length * sign, z + dx / length * sign)
                        .ground_height_m;
                    steepest = steepest.max((next - ground).abs());
                }
            }
        }
        assert!(broad > 0.5, "valley lost its broad shoulder: {broad}");
        assert!(steepest < 2., "narrow bank wall remains: {steepest}m/m");
    }
    #[test]
    fn wet_beds_are_sediment_and_ecotone_paint_is_native_coherent() {
        let p = params();
        let mut wet = 0;
        let mut changes = 0;
        for ix in -12..=12 {
            for iz in -12..=12 {
                let x = ix as f64 * 2400.;
                let z = iz as f64 * 2400.;
                let a = column(19, p, x, z);
                if a.visible_top > a.ground_top {
                    wet += 1;
                    assert!(matches!(a.ground_surface, Material::Sand | Material::Soil));
                }
                let x = x + 317.25;
                let z = z - 189.75;
                let a = column(19, p, x, z);
                let b = column(19, p, x + voxel_size(), z);
                changes += usize::from(a.ground_surface != b.ground_surface);
            }
        }
        assert!(wet > 100, "wet coverage {wet}");
        assert!(changes < 12, "native paint became speckled: {changes}");
    }
    #[test]
    fn climate_survey_contains_biomes_and_water_is_local_on_land() {
        let p = params();
        let mut biomes = [0; 6];
        let (mut land_count, mut wet_land) = (0, 0);
        for x in -24..=24 {
            for z in -24..=24 {
                let f = fields(7, p, x as f64 * 4871. + 117., z as f64 * 4933. - 239.);
                biomes[f.biome as usize] += 1;
                if f.ground_height_m > 10. {
                    land_count += 1;
                    wet_land += usize::from(f.water_height_m.is_some());
                }
            }
        }
        assert!(biomes.iter().all(|&n| n > 0), "biomes {biomes:?}");
        assert!(
            wet_land * 5 < land_count,
            "wet land {wet_land}/{land_count}"
        );
    }
    #[test]
    fn analytical_ranges_contain_survey_heights_including_channels() {
        let p = params();
        for seed in [7, 113] {
            for i in -12..=12 {
                let x = i as f64 * 2400.;
                let z = (i % 5) as f64 * 2400.;
                for radius in [0.03125, 3., 41., 300.] {
                    let bounds =
                        height_bounds(seed, p, [x - radius, z - radius], [x + radius, z + radius]);
                    for dx in [-radius, 0., radius] {
                        for dz in [-radius, 0., radius] {
                            let f = fields(seed, p, x + dx, z + dz);
                            assert!(
                                f.ground_height_m >= bounds[0] - 1e-8
                                    && f.ground_height_m <= bounds[1] + 1e-8,
                                "{bounds:?} vs {f:?}"
                            );
                            if let Some(w) = f.water_height_m {
                                assert!(w <= bounds[1] + 1e-8);
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn horizontal_scale_preserves_physical_amplitude() {
        let p = params();
        let scaled = FbmParams {
            horizontal_scale: 3600.,
            ..p
        };
        for (x, z) in [(-9321., 2712.), (17., -5.), (2400., 4800.)] {
            let a = fields(7, p, x, z);
            let b = fields(7, scaled, x * 2., z * 2.);
            assert_eq!(a, b);
        }
    }
}

/// Conservative descriptor enumeration for boxes no larger than one graph region.
/// Larger requests must use the source global envelope instead.
pub(crate) fn reaches_intersecting(
    seed: u64,
    p: FbmParams,
    min: [f64; 2],
    max: [f64; 2],
) -> Vec<Reach> {
    let spacing = 2400. * p.horizontal_scale / 1800.;
    if max[0] - min[0] > spacing * 4. || max[1] - min[1] > spacing * 4. {
        return Vec::new();
    }
    let intersects = |r: &Reach| {
        let radius = r.outer_radius();
        r.a.x.min(r.b.x) - radius <= max[0]
            && r.a.x.max(r.b.x) + radius >= min[0]
            && r.a.z.min(r.b.z) - radius <= max[1]
            && r.a.z.max(r.b.z) + radius >= min[1]
    };
    // Most affine patches and water-feature queries fit inside one bin.
    // Its entries already retain original reach order and contain no duplicates.
    let region_width = spacing * 4.;
    if (0..2).all(|i| (min[i] / region_width).floor() == (max[i] / region_width).floor()) {
        let local = region(seed, p, min[0], min[1]);
        if local.bin(min[0], min[1]) == local.bin(max[0], max[1]) {
            return local
                .candidates(min[0], min[1])
                .filter(intersects)
                .collect();
        }
    }
    let mut out: Vec<Reach> = Vec::new();
    let mut visited = Vec::with_capacity(4);
    for x in [min[0], max[0]] {
        for z in [min[1], max[1]] {
            let region = region(seed, p, x, z);
            if visited.iter().any(|previous| Rc::ptr_eq(previous, &region)) {
                continue;
            }
            for &r in region.reaches.iter().filter(|r| intersects(r)) {
                if !out
                    .iter()
                    .any(|old| old.owner == r.owner && old.segment == r.segment)
                {
                    out.push(r);
                }
            }
            visited.push(region);
        }
    }
    out
}

#[cfg(test)]
mod climate_tests {
    use super::*;
    #[test]
    fn macro_barriers_dry_the_lee_and_climate_has_no_cell_seams() {
        let mut dry = 0;
        for i in -16..=16 {
            let ix = i * 13;
            let iz = i * 7 + 3;
            let [_, moisture] = climate_node(7, ix, iz);
            let baseline = (0.50
                + 0.75
                    * fbm(
                        7,
                        ix as f64 * 512.,
                        iz as f64 * 512.,
                        27000.,
                        3,
                        0.5,
                        0x5104,
                    ))
            .clamp(0., 1.);
            assert!(moisture <= baseline + 1e-14);
            dry += usize::from(moisture < baseline - 0.01);
        }
        assert!(dry > 0);
        let p = FbmParams::default();
        for x in [-1024., -512., 0., 512., 1024.] {
            let a = climate(7, p, x - 1e-5, 37.);
            let b = climate(7, p, x + 1e-5, 37.);
            assert!((a[0] - b[0]).abs() < 1e-8 && (a[1] - b[1]).abs() < 1e-8);
        }
    }
}
