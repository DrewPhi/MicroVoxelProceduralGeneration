//! Climate fBm, continental uplift and globally addressed drainage.
//! The original seeded recipe uses local D8 routing. Watershed adds a bounded
//! upstream halo, jittered drainage nodes and valley profiles shaped by incision.
//! Cached regions are an acceleration structure. Each has 256 candidate bins
//! and cache order never changes the world-space descriptors.
use super::{noise, noise_gradient, FbmParams, FbmRecipe, TerrainColumn};
use crate::alpine::Value;
use crate::{voxel_size, Material};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::Rc,
};
#[path = "eroded_relief.rs"]
pub(crate) mod eroded_relief;
#[path = "eroded_gpu_data.rs"]
pub(super) mod eroded_gpu_data;
pub const SEEDED_WORLD_SOURCE_VERSION: u32 = 8;
pub const SEEDED_WORLD_APPEARANCE_VERSION: u32 = 2;

const CLIMATE_CACHE_CAPACITY: usize = 4096;
const REGION_CACHE_CAPACITY: usize = 8;
const REGION_BINS_PER_AXIS: usize = 16;
const RUNOFF_HOPS: usize = 8;
pub(crate) const WATERSHED_MAX_WATER_DEPTH: f64 = 9.;
pub(crate) const WATERSHED_WATER_ABOVE_LAND: f64 = 0.7;

pub(crate) fn minimum_radius(p: FbmParams) -> f64 {
    if p.recipe.has_watershed_drainage() {
        3.
    } else {
        8.
    }
}

pub(crate) fn maximum_lake_radius(p: FbmParams) -> f64 {
    if p.recipe.has_watershed_drainage() {
        140.
    } else {
        100.
    }
}

pub(crate) fn global_height_range(p: FbmParams) -> [f64; 2] {
    let authored = if p.recipe == FbmRecipe::Eroded {
        [-938., 2863.]
    } else if p.recipe == FbmRecipe::Landforms {
        [-750., 2680.]
    } else if p.recipe.has_landscape_relief() {
        [-730., 2660.]
    } else if p.recipe.has_watershed_drainage() {
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

/// Shared by the canonical height function and its conservative bounds. All
/// bands are in [-1, 1]. Relief is added before the drainage graph conditions it.
pub(crate) fn landscape_relief<V: Value>(
    base: V,
    mountain: V,
    ridge: V,
    strata: V,
    moisture: V,
    ribs: V,
    cuts: V,
) -> V {
    let ramp = |v: V, low: f64, high: f64| {
        let t = v.add(V::point(-low)).scale(1. / (high - low)).clamp01();
        t.smooth_unit()
    };
    let upland = mountain.mul(mountain);
    let shoulders = ramp(ridge, 0.52, 0.88);
    // A warped ribs field and a narrower cut put ravines inside the shoulders
    // of the parent range. Quiet valley floors keep their broad relief.
    let flutes = ribs.add(cuts.scale(0.30));
    let ravine = ramp(flutes, 0.13, 0.52);
    let crest = ramp(ribs.add(strata.scale(0.32)), 0.03, 0.47);
    let alpine = shoulders.mul(crest.scale(94.).add(ravine.scale(-108.)));
    // In dry provinces, steep ledges and long aprons survive even outside the
    // mountains. Transition continuously into fluvial grasslands.
    let dry = ramp(moisture.scale(-1.), 0.06, 0.36);
    let platform = strata.add(ribs.scale(0.22));
    let bench = ramp(platform, -0.12, 0.04)
        .scale(37.)
        .add(ramp(platform, 0.26, 0.40).scale(27.))
        .add(platform.scale(-14.));
    let banks = ramp(cuts.add(strata.scale(0.26)), 0.10, 0.45);
    base.add(upland.mul(alpine))
        .add(dry.mul(bench))
        .add(V::point(1.).add(upland.scale(-1.)).mul(banks).scale(-10.))
}

/// Canonical meter-scale relief. The two rotated noise coordinates stretch
/// rock joints along strike, and the smaller band breaks up their edges.
pub(crate) fn jointed_relief<V: Value>(ridge: V, upland: V, strata: V, joints: V, debris: V) -> V {
    let ramp = |x: V, start: f64, width: f64| {
        let t = x.add(V::point(-start)).scale(1. / width).clamp01();
        t.smooth_unit()
    };
    let exposure = ramp(ridge, 0.45, 0.38).mul(ramp(strata, -0.38, 0.63));
    let fracture = joints.add(debris.scale(0.24));
    let ledges = ramp(fracture, -0.23, 0.20)
        .scale(2.8)
        .add(ramp(fracture, 0.21, 0.19).scale(4.2))
        .add(V::point(-3.5));
    let talus = debris.mul(debris).scale(0.85);
    let strength = exposure.mul(upland.mul(upland).scale(0.80).add(V::point(0.20)));
    strength.mul(ledges.add(talus)).add(debris.scale(0.24))
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
    fbm_sample::<false>(seed, x, z, wavelength, count, gain, salt)[0]
}
fn fbm_sample<const GRADIENT: bool>(
    seed: u64,
    x: f64,
    z: f64,
    wavelength: f64,
    count: u32,
    gain: f64,
    salt: u64,
) -> [f64; 3] {
    let (mut a, mut f, mut sum, mut norm) = (1., 1. / wavelength, 0., 0.);
    let mut grade = [0.; 2];
    for i in 0..count {
        let octave_seed = seed ^ salt.wrapping_add(i as u64 * 0x9e3779b9);
        let (u, v) = (x * f + 13.7, z * f - 8.3);
        if GRADIENT {
            let [value, du, dv] = noise_gradient(u, v, octave_seed);
            sum += a * value;
            grade[0] += a * du * f;
            grade[1] += a * dv * f;
        } else {
            sum += a * noise(u, v, octave_seed);
        }
        norm += a;
        a *= gain;
        f *= 2.;
    }
    [sum / norm, grade[0] / norm, grade[1] / norm]
}
#[derive(Clone, Copy)]
struct Land {
    h: f64,
    continental: f64,
    mountain: f64,
    outcrop: f64,
    parent_grade: [f64; 2],
}
fn land(seed: u64, p: FbmParams, x: f64, z: f64) -> Land {
    // World scale changes wavelengths alone; amplitude changes meters of relief.
    let x = x * 1800. / p.horizontal_scale;
    let z = z * 1800. / p.horizontal_scale;
    let continental = fbm(seed, x, z, 48000., 3, 0.45, 0x5101);
    let province = fbm(seed, x, z, 22000., 3, 0.45, 0x5102);
    let mountain = smooth((province + 0.08) / 0.48);
    let [ridge_noise, ridge_dx, ridge_dz] = if p.recipe == FbmRecipe::Eroded {
        fbm_sample::<true>(seed, x, z, 3600., 4, 0.48, 0x5105)
    } else {
        [fbm(seed, x, z, 3600., 4, 0.48, 0x5105), 0., 0.]
    };
    let ridge = 1. - ridge_noise.abs();
    let [foothill, hill_dx, hill_dz] = if p.recipe == FbmRecipe::Eroded {
        fbm_sample::<true>(seed, x, z, 1800., 3, 0.45, 0x5106)
    } else {
        [fbm(seed, x, z, 1800., 3, 0.45, 0x5106), 0., 0.]
    };
    // The parent ridge controls slope and aspect across snowfields. Metre-
    // scale rock joints and the sub-voxel profile would fragment the snowline
    // if their full gradient were used as the support slope.
    let ridge_scale = -ridge_noise.signum() * 4560. * (mountain * ridge).powi(2);
    let grade_scale = (p.amplitude / 1000.) * (1800. / p.horizontal_scale);
    let parent_grade = [
        grade_scale * (ridge_scale * ridge_dx + 50. * hill_dx),
        grade_scale * (ridge_scale * ridge_dz + 50. * hill_dz),
    ];
    let fine = fbm(seed, x, z, 90., p.octaves, 0.65 * p.roughness, 0x5107);
    let mut outcrop = 0.;
    let authored = if p.recipe.has_watershed_drainage() {
        let strata = fbm(seed, x, z, 1050., 3, 0.45, 0x5121);
        let base = watershed_relief(continental, mountain, ridge, foothill, fine, strata);
        if p.recipe.has_landscape_relief() {
            let moisture = fbm(seed, x, z, 27000., 3, 0.5, 0x5104);
            let ribs = fbm(seed, x, z, 620., 2, 0.47, 0x5130);
            let cuts = fbm(seed, x, z, 180., 2, 0.40, 0x5131);
            let base = landscape_relief(base, mountain, ridge, strata, moisture, ribs, cuts);
            if p.recipe.has_jointed_relief() {
                let joints = noise(x / 23. + z / 30., z / 8.5 - x / 11., seed ^ 0x5140);
                let debris = noise(x / 3.1, z / 3.1, seed ^ 0x5141);
                let rock = base + jointed_relief(ridge, mountain, strata, joints, debris);
                if p.recipe == FbmRecipe::Eroded {
                    let rock = rock
                        + eroded_relief::province_relief(
                            seed, x, z, rock, mountain, strata, moisture,
                        );
                    let (displacement, exposure) =
                        eroded_relief::bedrock(seed, x, z, rock, mountain, strata, moisture);
                    outcrop = exposure;
                    rock + displacement
                } else {
                    rock
                }
            } else {
                base
            }
        } else {
            base
        }
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
        outcrop,
        parent_grade,
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
    if p.recipe.has_watershed_drainage() {
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Node {
    pub(crate) x: f64,
    pub(crate) z: f64,
    pub(crate) stage: f64,
    potential: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Reach {
    pub(crate) a: Node,
    pub(crate) b: Node,
    pub(crate) width: f64,
    pub(crate) width_b: f64,
    pub(crate) depth: f64,
    valley_radius: f64,
    pub(crate) lake: bool,
    pub(crate) wet: bool,
    pub(crate) owner: [i64; 2],
    pub(crate) segment: u8,
}
impl Reach {
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
    fn candidates(&self, x: f64, z: f64) -> impl Iterator<Item = Reach> + '_ {
        let ix = ((x - self.origin[0]) / self.bin_size)
            .floor()
            .clamp(0., (REGION_BINS_PER_AXIS - 1) as f64) as usize;
        let iz = ((z - self.origin[1]) / self.bin_size)
            .floor()
            .clamp(0., (REGION_BINS_PER_AXIS - 1) as f64) as usize;
        self.bins[iz * REGION_BINS_PER_AXIS + ix]
            .iter()
            .map(|&i| self.reaches[i])
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    seed: u64,
    recipe: FbmRecipe,
    scale: u64,
    amplitude: u64,
    octaves: u32,
    roughness: u64,
    x: i64,
    z: i64,
}
struct RegionCache {
    last: Option<(Key, Rc<Region>)>,
    recent: VecDeque<(Key, Rc<Region>)>,
}

thread_local! {
    static REGIONS: RefCell<RegionCache> = const {
        RefCell::new(RegionCache { last: None, recent: VecDeque::new() })
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
        seed,
        recipe: p.recipe,
        scale: p.horizontal_scale.to_bits(),
        amplitude: p.amplitude.to_bits(),
        octaves: p.octaves,
        roughness: p.roughness.to_bits(),
        x: rx,
        z: rz,
    };
    if let Some(hit) = REGIONS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((k, value)) = &cache.last {
            if *k == key {
                return Some(Rc::clone(value));
            }
        }
        let hit = cache
            .recent
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, value)| Rc::clone(value))?;
        cache.last = Some((key, Rc::clone(&hit)));
        Some(hit)
    }) {
        return hit;
    }
    // Build outside the cache borrow. A one-node halo includes every incident
    // edge affecting this region, including diagonals and negative boundaries.
    let watershed = p.recipe.has_watershed_drainage();
    // The row just outside the accumulated halo is needed to choose the
    // downstream link of its outermost contributors.
    // A node two cells outside can drain to a jittered node near the side;
    // the reach's shoulders can then cross it. Include those contributors and
    // leave the same runoff halo around this enlarged descriptor set.
    let extra = usize::from(p.recipe.has_landscape_relief());
    let halo = if watershed {
        RUNOFF_HOPS + 1 + extra
    } else {
        0
    };
    let side = 8 + 2 * halo;
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
    for j in halo + 1 - extra..halo + 7 + extra {
        for i in halo + 1 - extra..halo + 7 + extra {
            let index = j * side + i;
            let a = nodes[index];
            if a.potential <= 0. {
                continue;
            }
            let next = downstream(&nodes, side, index);
            let b = nodes[next];
            let lake = next == index;
            // The river network carves drainage even where discharge is too
            // low for perennial water. This decision is shared by all
            // segments of a reach, independent of render resolution.
            let wet = !p.recipe.has_landscape_relief()
                || flow.as_ref().is_some_and(|runoff| runoff[index] >= 0.30);
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
                    width,
                    width_b: width,
                    depth,
                    valley_radius: 3. * width,
                    lake,
                    wet,
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
                let signal = |t: f64| {
                    0.75 * (std::f64::consts::TAU * t + phase1).sin()
                        + 0.25 * (2. * std::f64::consts::TAU * t + phase2).sin()
                };
                let fitted = (p.recipe == FbmRecipe::Eroded).then(|| {
                    let metres = p.amplitude / 1000.;
                    let gradient = (a.stage - b.stage).abs() * (p.horizontal_scale / 1800.)
                        / (length * metres).max(1e-9);
                    let migration = if wet {
                        1. - smooth((gradient - 0.006) / 0.033)
                    } else {
                        0.
                    };
                    eroded_relief::river_offsets(
                        spacing,
                        [a.x, a.z],
                        [b.x, b.z],
                        [a.stage, b.stage],
                        metres,
                        migration,
                        |t| ((0.68 + 0.55 * migration) * signal(t)).clamp(-1., 1.),
                        |x, z| land(seed, p, x, z).h,
                    )
                });
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
                    let offset = fitted.as_ref().map_or_else(
                        || 0.16 * spacing * bend * signal(t),
                        |offsets| offsets[usize::from(index)],
                    );
                    Node {
                        x: px - dz / length * offset,
                        z: pz + dx / length * offset,
                        stage: a.stage + t * (b.stage - a.stage),
                        potential: a.potential + t * (b.potential - a.potential),
                    }
                };
                let points: [Node; 17] = std::array::from_fn(|i| point(i as u8));
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
                for segment in 0..16 {
                    let segment_width = widths[segment as usize];
                    let segment_width_b = widths[segment as usize + 1];
                    let widest = segment_width.max(segment_width_b);
                    reaches.push(Reach {
                        a: points[segment as usize],
                        b: points[segment as usize + 1],
                        width: segment_width,
                        width_b: segment_width_b,
                        depth,
                        valley_radius: valley_radius
                            .map_or(18. * widest, |radius| radius.max(14. * widest)),
                        lake,
                        wet,
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
        while c.recent.len() >= capacity {
            c.recent.pop_front();
        }
        c.recent.push_back((key, Rc::clone(&value)));
        c.last = Some((key, Rc::clone(&value)));
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
pub(super) fn climate_surface(seed: u64, p: FbmParams, x: f64, z: f64) -> [f64; 3] {
    let [temperature, moisture] = climate(seed, p, x, z);
    [land(seed, p, x, z).h, temperature, moisture]
}

pub(super) fn fields(seed: u64, p: FbmParams, x: f64, z: f64) -> SeededFields {
    fields_with_relief(seed, p, x, z, true)
}

fn fields_with_relief(seed: u64, p: FbmParams, x: f64, z: f64, secondary: bool) -> SeededFields {
    evaluate(seed, p, x, z, secondary).fields
}

struct TerrainState {
    fields: SeededFields,
    outcrop: f64,
    cut: f64,
    floodplain: f64,
    parent_grade: [f64; 2],
}

fn evaluate(seed: u64, p: FbmParams, x: f64, z: f64, secondary: bool) -> TerrainState {
    let l = land(seed, p, x, z);
    let mut ground = l.h;
    let mut water = if ground < 0. { Some(0.) } else { None };
    let watershed = p.recipe.has_watershed_drainage();
    let relief = p.amplitude / 1000.;
    let land_water_ceiling = (l.h + WATERSHED_WATER_ABOVE_LAND * relief).max(0.);
    let mut nearest_bank: Option<(f64, f64)> = None;
    if p.amplitude > 0. {
        let region = region(seed, p, x, z);
        let mut weighted_ground = ground;
        let mut total_weight = 1.;
        let mut river_stage = f64::NEG_INFINITY;
        let mut lake_stage: Option<f64> = None;
        for r in region.candidates(x, z) {
            let (d, stage) = project(r, x, z);
            let outer = r.outer_radius();
            let t = projection_t(r, x, z);
            let width = r.width + t * (r.width_b - r.width);
            if d >= outer || stage < 0. {
                continue;
            }
            let amplitude = p.amplitude / 1000.;
            let depth = r.depth * amplitude;
            let bank_height = 0.7 * amplitude;
            let profile = if d <= width {
                stage - depth + (depth + bank_height) * smooth(d / width)
            } else {
                let t = (d - width) / (outer - width);
                let shoulder = if p.recipe == FbmRecipe::Eroded {
                    // Low flood terraces and a higher valley shoulder form
                    // from the same continuous transverse profile as the bed.
                    0.56 * smooth(t)
                        + 0.19 * smooth((t - 0.10) / 0.14)
                        + 0.25 * smooth((t - 0.57) / 0.36)
                } else {
                    smooth(t)
                };
                stage + bank_height + (l.h - stage - bank_height) * shoulder
            };
            // Sea-level reach caps may cut the seabed, never raise a ring.
            let profile = if p.recipe.has_watershed_drainage() || (stage == 0. && l.h < 0.) {
                profile.min(l.h)
            } else {
                profile
            };
            let taper = 1. - smooth(d / outer);
            let weight = outer * outer / (d * d).max(width * width * 1e-8) * taper * taper;
            weighted_ground += weight * profile;
            total_weight += weight;
            if watershed && r.wet && stage > 0. {
                // Use the locally capped water level and a channel-scaled floodplain.
                // Compare the final combined bed against this stage after blending.
                let bank_width = (width * 4.5).max(28. * p.horizontal_scale / 1800.);
                let distance = d / bank_width;
                if distance < 1. && nearest_bank.is_none_or(|(best, _)| distance < best) {
                    nearest_bank = Some((distance, stage.min(land_water_ceiling)));
                }
            }
            if r.wet && d < width {
                if r.lake && profile < stage {
                    lake_stage = Some(lake_stage.map_or(stage, |old| old.min(stage)));
                }
                river_stage = river_stage.max(stage);
            }
        }
        ground = weighted_ground / total_weight;
        if let Some(stage) = lake_stage {
            // Tributaries can lift the blended shore above the lake level.
            // Flat lake priority applies only to its actual wet interior.
            if stage > ground {
                river_stage = stage;
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
    let mut incision = 0.;
    if secondary && p.recipe == FbmRecipe::Eroded && relief > 0. && water.is_none() {
        // Primary channels already have continuous riverbed profiles. Blend
        // a tributary into the floodplain while preserving the wet stage.
        let floodplain = nearest_bank.map_or(1., |(distance, _)| smooth((distance - 0.30) / 0.65));
        let exposure = smooth((ground / relief - 3.) / 14.);
        if floodplain * exposure > 0. {
            let cut = eroded_relief::incision(seed, p, x, z, |a, b| {
                fields_with_relief(seed, p, a, b, false)
            });
            incision = floodplain * exposure * cut;
            ground -= incision;
        }
    }
    let [temperature, moisture] = climate(seed, p, x, z);
    let effective_temperature = temperature - 0.00019 * ground.max(0.);
    let riparian = nearest_bank.is_some_and(|(_, stage)| {
        stage > 0. && (stage - 2. * relief..=stage + 7. * relief).contains(&ground)
    });
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
    } else if p.recipe.has_watershed_drainage() && moisture >= 0.56 {
        SeededBiome::Woodland
    } else if p.recipe.has_watershed_drainage() && moisture < 0.44 {
        SeededBiome::Steppe
    } else {
        SeededBiome::Plains
    };
    let floodplain = nearest_bank.map_or(0., |(distance, stage)| {
        (1. - smooth(distance))
            * (1. - smooth(((ground - stage).abs() - 1.5 * relief) / (11. * relief).max(0.01)))
    });
    TerrainState {
        fields: SeededFields {
            continentalness: l.continental,
            temperature: effective_temperature,
            moisture,
            mountain: l.mountain,
            biome,
            ground_height_m: ground,
            water_height_m: water.filter(|w| *w > ground),
        },
        outcrop: l.outcrop,
        cut: incision / relief.max(0.001),
        floodplain,
        parent_grade: l.parent_grade,
    }
}
/// Shared canopy probability for native tree candidates and distant cover.
pub(crate) fn landscape_tree_likelihood(seed: u64, f: SeededFields, x: f64, z: f64) -> f64 {
    if f.water_height_m.is_some()
        || matches!(
            f.biome,
            SeededBiome::Ocean | SeededBiome::Coast | SeededBiome::Desert
        )
    {
        return 0.;
    }
    let groves = noise(x / 260., z / 260., seed ^ 0x5134);
    let edges = noise(x / 74., z / 74., seed ^ 0x5135);
    let shelter = groves + 0.46 * edges + 0.7 * (f.moisture - 0.5);
    let treeline = smooth((f.temperature - 0.26) / 0.14);
    let moisture = smooth((f.moisture - 0.34) / 0.24);
    let riparian = if f.biome == SeededBiome::Wetland {
        0.22
    } else {
        0.
    };
    treeline * (riparian + (1. - riparian) * moisture * smooth((shelter + 0.15) / 0.50))
}

pub(super) fn column(seed: u64, p: FbmParams, x: f64, z: f64) -> TerrainColumn {
    column_from_fields(seed, p, x, z, fields(seed, p, x, z))
}

pub(super) fn eroded_column(
    seed: u64,
    p: FbmParams,
    x: f64,
    z: f64,
    sample_cover: bool,
) -> (TerrainColumn, f64, SeededFields) {
    let state = evaluate(seed, p, x, z, true);
    let f = state.fields;
    let ground_top = (f.ground_height_m / voxel_size()).floor() as i64;
    let visible_top = f
        .water_height_m
        .map_or(ground_top, |w| (w / voxel_size()).floor() as i64)
        .max(ground_top);
    let nx = x * 1800. / p.horizontal_scale;
    let nz = z * 1800. / p.horizontal_scale;
    // Connected patches model differences in soil retention and exposure.
    // The larger controls come from drainage, bedrock and lapse temperature.
    let soil = 0.68 * noise(nx / 164., nz / 164., seed ^ 0x5173)
        + 0.32 * noise(nx / 39., nz / 39., seed ^ 0x5174);
    let [dx, dz] = state.parent_grade;
    let parent_grade = dx.hypot(dz);
    let grade = parent_grade * (1. - 0.85 * state.floodplain);
    let bare_grade = smooth((grade - 0.67) / 0.78);
    // The canonical midlatitude snowpack lasts longer on poleward slopes.
    // Weight the aspect by slope so plains have no preferred exposure.
    let snow_shelter = -dz / (0.42 + parent_grade);
    let rock = state.outcrop + 0.62 * smooth((state.cut - 3.5) / 12.) + 0.65 * bare_grade;
    let cold_retention = 0.10 * (1. - smooth((f.temperature - 0.29) / 0.23));
    let local_moisture = f.moisture + 0.12 * soil + 0.28 * state.floodplain + cold_retention;
    let snow_line = 0.218 + 0.143 * smooth((f.moisture - 0.33) / 0.34) + 0.050 * soil
        - 0.096 * smooth(state.outcrop)
        - 0.245 * bare_grade
        + 0.065 * snow_shelter;
    let highland = smooth((f.mountain - 0.43) / 0.41)
        * smooth((f.ground_height_m / (p.amplitude / 1000.).max(0.001) - 200.) / 570.);
    let sand_line = 0.263 + 0.094 * smooth((f.temperature - 0.48) / 0.27);
    let surface = if visible_top > ground_top {
        if matches!(
            f.biome,
            SeededBiome::Ocean | SeededBiome::Coast | SeededBiome::Desert
        ) {
            Material::Sand
        } else {
            Material::Soil
        }
    } else if f.temperature < snow_line {
        Material::Snow
    } else if rock + highland * (0.45 - 0.29 * soil)
        > 0.42 + 0.34 * smooth((local_moisture - 0.27) / 0.43)
    {
        Material::Stone
    } else if f.ground_height_m < 6. + 7. * soil && f.biome == SeededBiome::Coast
        || f.temperature > 0.43 && local_moisture < sand_line
    {
        Material::Sand
    } else if local_moisture < 0.267 + 0.082 * highland || rock > 0.47 + 0.17 * soil {
        Material::Soil
    } else {
        Material::Grass
    };
    let cover = if sample_cover && surface == Material::Grass && visible_top == ground_top {
        let broad = noise(nx / 415., nz / 440., seed ^ 0x5134);
        let edge = noise(nx / 95., nz / 110., seed ^ 0x5135);
        let climate = smooth((local_moisture - 0.29) / 0.31);
        let warmth = smooth((f.temperature - 0.245) / 0.14);
        let canopy = smooth((0.76 * broad + 0.33 * edge + climate * 0.45 + 0.2) / 0.72);
        let refuge = smooth((local_moisture - 0.29) / 0.24);
        let shelter = (1. - smooth((rock - 0.18) / 0.52)) * refuge;
        (shelter * warmth * (1.42 * canopy + 0.56 * state.floodplain)).min(0.95)
    } else {
        0.
    };
    let ground_subsurface = match surface {
        Material::Sand | Material::Stone => surface,
        _ if rock > 0.51 => Material::Stone,
        _ => Material::Soil,
    };
    (
        TerrainColumn {
            ground_top,
            visible_top,
            ground_surface: surface,
            ground_subsurface,
        },
        cover,
        SeededFields {
            moisture: local_moisture,
            ..f
        },
    )
}

pub(super) fn column_from_fields(
    seed: u64,
    p: FbmParams,
    x: f64,
    z: f64,
    f: SeededFields,
) -> TerrainColumn {
    let ground_top = (f.ground_height_m / voxel_size()).floor() as i64;
    let visible_top = f
        .water_height_m
        .map_or(ground_top, |w| (w / voxel_size()).floor() as i64)
        .max(ground_top);
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
            } else if p.recipe.has_watershed_drainage() && f.moisture >= 0.46 {
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
    if p.recipe.has_landscape_relief() && visible_top == ground_top {
        let sx = x * 1800. / p.horizontal_scale;
        let sz = z * 1800. / p.horizontal_scale;
        if matches!(
            f.biome,
            SeededBiome::Desert | SeededBiome::Mountains | SeededBiome::Tundra
        ) {
            let seam = noise(
                sx / 115. + f.ground_height_m * 0.017,
                sz / 142.,
                seed ^ 0x5133,
            );
            if f.biome == SeededBiome::Desert {
                let strata = fbm(seed, sx, sz, 1050., 3, 0.45, 0x5121);
                if strata + 0.14 * seam > 0.22 {
                    surface = Material::Stone;
                }
            } else {
                // Snow collects unevenly, and the lower slopes can carry
                // connected turf even within the mountain province.
                let warmth = f.temperature - 0.15 * seam;
                let grass_line = 0.34 - 0.36 * (f.moisture - 0.47).clamp(-0.22, 0.26);
                surface = if warmth < 0.256 {
                    Material::Snow
                } else if warmth > grass_line && (f.moisture > 0.31 || seam < -0.35) {
                    Material::Grass
                } else {
                    Material::Stone
                };
            }
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
    TerrainColumn {
        ground_top,
        visible_top,
        ground_surface: surface,
        ground_subsurface: if surface == Material::Sand {
            Material::Sand
        } else {
            Material::Soil
        },
    }
}

/// Analytical pre-quantization envelope; ordinary f64 endpoints are not an
/// outward-rounded certificate. No sampled extrema are used.
/// Gradient noise has Euclidean Lipschitz constant <=16: corner gradients have
/// norm one, quintic fade derivative <=1.875, corner differences <=2sqrt(2),
/// and output scaling is 1.6; clamping cannot increase that bound.
pub(super) fn height_bounds(seed: u64, p: FbmParams, min: [f64; 2], max: [f64; 2]) -> [f64; 2] {
    let a = p.amplitude / 1000.;
    let global = global_height_range(p);
    if p.recipe.has_watershed_drainage() {
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
    fn slope_sampling_preserves_ridge_values() {
        for seed in [19, 83, 70231, u64::MAX] {
            for i in -40..40 {
                for j in -32..32 {
                    let x = f64::from(i) * 137.862;
                    let z = f64::from(j) * 289.119;
                    for (wavelength, octaves, gain, salt) in
                        [(3600., 4, 0.48, 0x5105), (1800., 3, 0.45, 0x5106)]
                    {
                        let scalar = fbm(seed, x, z, wavelength, octaves, gain, salt);
                        let [differentiated, _, _] =
                            fbm_sample::<true>(seed, x, z, wavelength, octaves, gain, salt);
                        assert_eq!(scalar.to_bits(), differentiated.to_bits());
                    }
                }
            }
        }
    }

    #[test]
    fn landscapes_bounds_and_cover_are_consistent_across_seeds_and_recipes() {
        use crate::affine_bounds::{bound_native_height, BoundBudget, ColumnBox};

        let cases = [
            FbmRecipe::Landscapes,
            FbmRecipe::Landforms,
            FbmRecipe::Eroded,
        ]
        .into_iter()
        .flat_map(|recipe| [19, 83, 70231].map(|seed| (recipe, seed)));
        for (recipe, seed) in cases {
            let p = FbmParams { recipe, ..params() };
            let baseline = FbmParams {
                recipe: FbmRecipe::SeededWorld,
                ..p
            };
            let terrain = FbmTerrain::new(seed, p).unwrap();
            for x in [-9600., -27., 0., 1011.5, 7028., 9612.] {
                for z in [-9601., -1., 805., 9601.] {
                    let cell = crate::Cell::from_meters([x, 0., z]);
                    if z == -9601. {
                        let a = fields(seed, p, x, -9600. - 0.00001);
                        let b = fields(seed, p, x, -9600. + 0.00001);
                        assert!(
                            (a.ground_height_m - b.ground_height_m).abs() < 0.0002,
                            "discontinuous ground at a drainage cache boundary: {a:?} {b:?}"
                        );
                    }
                    let first = fields(seed, p, x, z);
                    // A cache queried by several recipes must not reuse an
                    // incompatible graph just because seed and location match.
                    let sampled_regions = region(seed, p, x, z);
                    let other_regions = region(seed, baseline, x, z);
                    assert!(!Rc::ptr_eq(&sampled_regions, &other_regions));
                    assert_eq!(first, fields(seed, p, x, z));

                    let box_ = ColumnBox {
                        min: [cell.x, cell.z],
                        size_cells: [32, 32],
                    };
                    let bound =
                        bound_native_height(&terrain, box_, BoundBudget::default()).unwrap();
                    for dx in [0, 16, 31] {
                        for dz in [0, 16, 31] {
                            let x = cell.x + dx;
                            let z = cell.z + dz;
                            let (column, cover) = terrain.column_with_tree_cover(x, z);
                            assert_eq!(column, terrain.column_cells(x, z));
                            assert!((0. ..=1.).contains(&cover));
                            if column.visible_top > column.ground_top
                                || column.ground_surface != Material::Grass
                            {
                                assert_eq!(cover, 0.);
                            }
                            let top = column.visible_top as f64 * voxel_size();
                            let [xm, _, zm] = crate::Cell { x, y: 0, z }.center_m();
                            assert!(
                                (bound.height_range[0]..=bound.height_range[1]).contains(&top),
                                "seed {seed} {xm} {zm} {bound:?} missed {top}; land {} fields {:?}",
                                land(seed, p, xm, zm).h,
                                fields(seed, p, xm, zm)
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn secondary_drainage_cuts_dry_ground_and_crosses_cache_edges() {
        let p = FbmParams {
            recipe: FbmRecipe::Eroded,
            ..params()
        };
        let mut substantial_cuts = 0;
        for seed in [19, 83, 997, 70231] {
            for i in -6..6 {
                let x = f64::from(i) * 417.;
                for seam in [-5280., 0., 5280.] {
                    let d = 0.00001;
                    let a = fields(seed, p, x, seam - d);
                    let b = fields(seed, p, x, seam + d);
                    assert!(
                        (a.ground_height_m - b.ground_height_m).abs() < 0.004,
                        "seed {seed}, seam {x}, {seam}: {a:?} {b:?}"
                    );
                }
            }
            for z in (-240..2400).step_by(61) {
                for x in (-180..2300).step_by(67) {
                    let (x, z) = (f64::from(x), f64::from(z));
                    let primary = fields_with_relief(seed, p, x, z, false);
                    let final_fields = fields(seed, p, x, z);
                    let cut = primary.ground_height_m - final_fields.ground_height_m;
                    assert!((0. ..=eroded_relief::INCISION_HEIGHT_BOUND).contains(&cut));
                    assert_eq!(primary.water_height_m, final_fields.water_height_m);
                    substantial_cuts += usize::from(cut > 3.);
                }
            }
        }
        assert!(
            substantial_cuts > 30,
            "observed {substantial_cuts} substantial cuts"
        );
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
    reaches_with_coverage(seed, p, min, max)
        .into_iter()
        .map(|(reach, _)| reach)
        .collect()
}

/// The flag states that this exact descriptor is defined in every region
/// covered by the query. A union member can still be a possible contributor.
pub(crate) fn reaches_with_coverage(
    seed: u64,
    p: FbmParams,
    min: [f64; 2],
    max: [f64; 2],
) -> Vec<(Reach, bool)> {
    let spacing = 2400. * p.horizontal_scale / 1800.;
    if max[0] - min[0] > spacing * 4. || max[1] - min[1] > spacing * 4. {
        return Vec::new();
    }
    let mut out: Vec<(Reach, u8)> = Vec::new();
    let mut visited = Vec::with_capacity(4);
    for x in [min[0], max[0]] {
        for z in [min[1], max[1]] {
            let region = region(seed, p, x, z);
            if visited.iter().any(|previous| Rc::ptr_eq(previous, &region)) {
                continue;
            }
            for &r in &region.reaches {
                let radius = r.outer_radius();
                if r.a.x.min(r.b.x) - radius > max[0]
                    || r.a.x.max(r.b.x) + radius < min[0]
                    || r.a.z.min(r.b.z) - radius > max[1]
                    || r.a.z.max(r.b.z) + radius < min[1]
                {
                    continue;
                }
                if let Some((_, count)) = out.iter_mut().find(|(old, _)| *old == r) {
                    *count += 1;
                } else {
                    out.push((r, 1));
                }
            }
            visited.push(region);
        }
    }
    out.into_iter()
        .map(|(reach, count)| (reach, usize::from(count) == visited.len()))
        .collect()
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
