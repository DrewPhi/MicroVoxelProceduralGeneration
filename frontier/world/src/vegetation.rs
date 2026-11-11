//! Seeded native vegetation. Placement reads only the immutable terrain source;
//! camera demand, edits and tile request order never influence plant identity.
use crate::{Cell, Material, World};
#[path = "plant_recipe.rs"]
pub mod plant_recipe;
#[cfg(test)]
use std::collections::BTreeSet;
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
};

// Version 3: per-record seeded growth, low spreading ground cover and habitat cohorts.
// This intentionally changes canonical plant records/voxels; display AO does not.
pub const VEGETATION_SOURCE_VERSION: u32 = 3;
pub const TILE_SIZE_CELLS: i64 = 256;
pub const MAX_PLANT_RADIUS_CELLS: i64 = 96;
pub const MAX_PLANT_HEIGHT_CELLS: i64 = 176;
pub const fn variant_count(species: Species) -> u8 {
    match species {
        Species::Grass | Species::Rowan => 8,
        Species::Edelweiss => 3,
    }
}
const MODEL_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MODEL_CACHE_ENTRIES: usize = 512;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Species {
    Grass,
    Edelweiss,
    Rowan,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlantRecord {
    pub species: Species,
    pub seed: u64,
    pub root: Cell,
    pub variant: u8,
    pub rotation: u8,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlantVoxel {
    pub cell: Cell,
    pub material: Material,
    pub color: [f32; 3],
}
#[derive(Clone, Debug)]
pub struct PlantModel {
    pub voxels: Vec<PlantVoxel>,
    pub min: Cell,
    /// Exclusive upper bound.
    pub max: Cell,
    index: BTreeMap<Cell, usize>,
}
impl PlantModel {
    pub fn voxel(&self, cell: Cell) -> Option<PlantVoxel> {
        self.index.get(&cell).map(|&i| self.voxels[i])
    }
}
impl PlantRecord {
    pub fn world_cell(&self, c: Cell) -> Cell {
        let (x, z) = rotate(c.x, c.z, self.rotation);
        Cell {
            x: self.root.x + x,
            y: self.root.y + c.y,
            z: self.root.z + z,
        }
    }
    pub fn local_cell(&self, c: Cell) -> Cell {
        let (x, z) = rotate(c.x - self.root.x, c.z - self.root.z, 4 - self.rotation % 4);
        Cell {
            x,
            y: c.y - self.root.y,
            z,
        }
    }
}
fn rotate(x: i64, z: i64, r: u8) -> (i64, i64) {
    match r % 4 {
        0 => (x, z),
        1 => (-z, x),
        2 => (-x, -z),
        _ => (z, -x),
    }
}
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}
fn hash(seed: u64, x: i64, z: i64) -> u64 {
    mix(seed
        ^ (x as u64).wrapping_mul(0x9e3779b97f4a7c15)
        ^ (z as u64).wrapping_mul(0xd6e8feb86659fd93))
}
struct Rng(u64);
impl Rng {
    fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        (mix(self.0) >> 11) as f64 / ((1u64 << 53) as f64)
    }
    fn between(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.unit()
    }
}
type Cells = BTreeMap<Cell, (Material, [u8; 3])>;
const LEAF: [u8; 3] = [103, 132, 65];
const SILVER: [u8; 3] = [144, 162, 130];
const WHITE: [u8; 3] = [237, 235, 213];
const BARK: [u8; 3] = [128, 116, 96];
fn put(v: &mut Cells, p: [i64; 3], m: Material, rgb: [u8; 3]) {
    v.insert(
        Cell {
            x: p[0],
            y: p[1],
            z: p[2],
        },
        (m, rgb),
    );
}
fn stroke(v: &mut Cells, mut a: [i64; 3], b: [i64; 3], radius: i64, m: Material, rgb: [u8; 3]) {
    loop {
        for x in -radius..=radius {
            for z in -radius..=radius {
                if x * x + z * z <= radius * radius {
                    put(v, [a[0] + x, a[1], a[2] + z], m, rgb);
                }
            }
        }
        if a == b {
            break;
        }
        let axis = (0..3).max_by_key(|&i| (b[i] - a[i]).abs()).unwrap();
        a[axis] += (b[axis] - a[axis]).signum();
    }
}
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
// A padded index volume makes the ordinary bounded plant flood six array
// accesses per cell. Sparse/very widely separated inputs fall back to binary
// search in canonical cell order; neither path allocates the coordinate gap.
fn rooted_component(cells: &[(Cell, (Material, [u8; 3]))]) -> Vec<bool> {
    let mut seen = vec![false; cells.len()];
    let Ok(root) = cells.binary_search_by_key(&Cell::default(), |v| v.0) else {
        return seen;
    };
    let mut min = [i64::MAX; 3];
    let mut max = [i64::MIN; 3];
    for &(c, _) in cells {
        for (d, value) in [c.x, c.y, c.z].into_iter().enumerate() {
            min[d] = min[d].min(value);
            max[d] = max[d].max(value);
        }
    }
    let size = (0..3)
        .map(|d| max[d].checked_sub(min[d])?.checked_add(3)?.try_into().ok())
        .collect::<Option<Vec<usize>>>();
    if let Some(size) = size {
        if let Some(volume) = size[0]
            .checked_mul(size[1])
            .and_then(|v| v.checked_mul(size[2]))
        {
            if volume <= 8 * 1024 * 1024 && volume <= cells.len().saturating_mul(64) {
                let yz = size[1] * size[2];
                let address = |c: Cell| {
                    (c.x - min[0] + 1) as usize * yz
                        + (c.y - min[1] + 1) as usize * size[2]
                        + (c.z - min[2] + 1) as usize
                };
                let mut occupied = vec![0u32; volume];
                for (i, &(c, _)) in cells.iter().enumerate() {
                    occupied[address(c)] = i as u32 + 1;
                }
                let mut queue = Vec::with_capacity(cells.len());
                let start = address(Cell::default());
                queue.push(start);
                occupied[start] = 0;
                seen[root] = true;
                let mut head = 0;
                while head < queue.len() {
                    let p = queue[head];
                    head += 1;
                    // The empty one-cell border makes each neighbor in bounds.
                    for q in [p + yz, p - yz, p + size[2], p - size[2], p + 1, p - 1] {
                        let id = occupied[q];
                        if id != 0 {
                            occupied[q] = 0; // Mark on enqueue, never enqueue twice.
                            seen[id as usize - 1] = true;
                            queue.push(q);
                        }
                    }
                }
                return seen;
            }
        }
    }
    let mut queue = Vec::with_capacity(cells.len());
    queue.push(root);
    seen[root] = true;
    let mut head = 0;
    while head < queue.len() {
        let c = cells[queue[head]].0;
        head += 1;
        for (x, y, z) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            let (Some(x), Some(y), Some(z)) =
                (c.x.checked_add(x), c.y.checked_add(y), c.z.checked_add(z))
            else {
                continue;
            };
            if let Ok(i) = cells.binary_search_by_key(&Cell { x, y, z }, |v| v.0) {
                if !seen[i] {
                    seen[i] = true;
                    queue.push(i);
                }
            }
        }
    }
    seen
}
#[cfg(test)]
fn rooted_component_reference(cells: &Cells) -> Vec<bool> {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([Cell::default()]);
    while let Some(c) = queue.pop_front() {
        if !cells.contains_key(&c) || !seen.insert(c) {
            continue;
        }
        for (x, y, z) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            queue.push_back(Cell {
                x: c.x + x,
                y: c.y + y,
                z: c.z + z,
            });
        }
    }
    cells.keys().map(|c| seen.contains(c)).collect()
}
fn model(cells: Cells, seed: u64) -> PlantModel {
    // Retain only the rooted face-connected component after voxel rasterization.
    let cells: Vec<_> = cells.into_iter().collect();
    let connected = rooted_component(&cells);
    // Every generated plant in the world test suite checks the old filter on
    // the actual pre-filter rasterization, including discarded crown voxels.
    #[cfg(test)]
    assert_eq!(
        connected,
        rooted_component_reference(&cells.iter().copied().collect())
    );
    let mut min = Cell {
        x: i64::MAX,
        y: i64::MAX,
        z: i64::MAX,
    };
    let mut max = Cell {
        x: i64::MIN,
        y: i64::MIN,
        z: i64::MIN,
    };
    let voxels: Vec<_> = cells
        .into_iter()
        .zip(connected)
        .filter(|(_, keep)| *keep)
        .map(|((cell, (material, rgb)), _)| {
            min.x = min.x.min(cell.x);
            min.y = min.y.min(cell.y);
            min.z = min.z.min(cell.z);
            max.x = max.x.max(cell.x + 1);
            max.y = max.y.max(cell.y + 1);
            max.z = max.z.max(cell.z + 1);
            // Broad pigment variation follows the plant, with only a quiet native-cell grain.
            let patch = patch_field(seed, cell.x + cell.y / 2, cell.z, 6) as f32;
            let grain = (hash(seed, cell.x ^ cell.y.rotate_left(13), cell.z) % 101) as f32 / 100.;
            let variation = 0.94 + 0.09 * patch + 0.025 * grain;
            PlantVoxel {
                cell,
                material,
                color: rgb.map(|c| srgb_to_linear((c as f32 / 255. * variation).min(1.))),
            }
        })
        .collect();
    let index = voxels
        .iter()
        .enumerate()
        .map(|(i, v)| (v.cell, i))
        .collect();
    PlantModel {
        voxels,
        min,
        max,
        index,
    }
}
/// Pure bounded native model generation; no authored arrangements or asset files.
pub fn generate_grass(seed: u64, variant: u8) -> PlantModel {
    plant_recipe::grass_recipe(seed, variant).materialize()
}
pub fn generate_edelweiss(seed: u64, variant: u8) -> PlantModel {
    let mut rng = Rng(seed);
    let mut v = Cells::new();
    for k in 0..6 {
        let a = k as f64 * std::f64::consts::TAU / 6. + 0.2;
        stroke(
            &mut v,
            [0, 0, 0],
            [
                (a.cos() * 2.).round() as i64,
                0,
                (a.sin() * 2.).round() as i64,
            ],
            0,
            Material::Leaf,
            SILVER,
        );
    }
    let heads = if variant % 3 == 2 { 3 } else { 1 };
    for head in 0..heads {
        let ox = if head == 0 {
            0
        } else if head == 1 {
            3
        } else {
            -3
        };
        let oz = if head == 0 { 0 } else { 2 };
        let h = 5 + (variant % 2) as i64 + (rng.unit() * 3.).floor() as i64;
        stroke(&mut v, [0, 0, 0], [ox, h, oz], 0, Material::Leaf, LEAF);
        let angle = rng.between(-0.3, 0.3);
        for k in 0..8 {
            let a = angle + k as f64 * std::f64::consts::TAU / 8.;
            let r = if k % 2 == 0 { 3. } else { 2. };
            stroke(
                &mut v,
                [ox, h, oz],
                [
                    ox + (a.cos() * r).round() as i64,
                    h,
                    oz + (a.sin() * r).round() as i64,
                ],
                0,
                Material::Flower,
                WHITE,
            );
        }
        put(&mut v, [ox, h + 1, oz], Material::Flower, [218, 178, 68]);
    }
    model(v, seed)
}
fn norm(v: [f64; 3]) -> [f64; 3] {
    let l = v.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-9);
    v.map(|x| x / l)
}
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum()
}
/// Bounded local space-colonization growth, connected strokes, overlapping fine
/// leaf sprays, exposed blossom clusters and a filled curved, flared bole.
/// Algorithm references (greenfield implementation, no external source copied):
/// https://www.algorithmicbotany.org/papers/colonization.egwnp2007.html
/// https://www.algorithmicbotany.org/papers/selforg.sig2009.html
/// The latter motivates light/space competition; this bounded prototype uses
/// attractor competition rather than claiming its full biological simulation.
pub fn generate_rowan(seed: u64, variant: u8) -> PlantModel {
    grow_rowan(seed, variant, &|| false).unwrap()
}
fn grow_rowan(seed: u64, variant: u8, cancel: &impl Fn() -> bool) -> Option<PlantModel> {
    build_rowan_recipe(seed, variant, cancel)?.materialize_cancellable(cancel)
}
pub(super) fn build_rowan_recipe(
    seed: u64,
    variant: u8,
    cancel: &impl Fn() -> bool,
) -> Option<plant_recipe::RowanRecipe> {
    let mut rng = Rng(seed);
    let h =
        [52., 65., 76., 84., 94., 104., 112., 88.][variant as usize % 8] * rng.between(0.96, 1.04);
    // The attractor volume supplies asymmetric, layered crowns; growth still
    // follows the existing space-colonization algorithm, never an authored mesh.
    let width = rng.between(0.27, 0.39);
    let depth = rng.between(0.25, 0.36);
    let crown_lift = rng.between(0.60, 0.69);
    let crown_depth = rng.between(0.28, 0.35);
    let bias = [rng.between(-0.08, 0.08), rng.between(-0.06, 0.06)];
    let mut attract = Vec::new();
    while attract.len() < 650 {
        let q = [
            rng.between(-1., 1.),
            rng.between(-1., 1.),
            rng.between(-1., 1.),
        ];
        if q.iter().map(|x| x * x).sum::<f64>() < 1. {
            // Crown tilt varies with height; a low-frequency lobe gives uneven
            // shoulders without thinning out the approved full leaf sprays.
            let lobe = 1. + 0.16 * (q[1] * 4. + q[2] * 2.).sin();
            attract.push([
                (q[0] * width * lobe + bias[0] * (q[1] + 1.)) * h,
                (crown_lift + q[1] * crown_depth) * h,
                (q[2] * depth + bias[1] * (q[1] + 1.)) * h,
            ]);
        }
    }
    let mut nodes: Vec<[f64; 3]> = (0..(h * 0.33) as usize / 2)
        .map(|i| [0., i as f64 * 2., 0.])
        .collect();
    let mut parents: Vec<usize> = (0..nodes.len()).map(|i| i.saturating_sub(1)).collect();
    for _ in 0..110 {
        if cancel() {
            return None;
        }
        let mut sums = vec![[0.; 3]; nodes.len()];
        let mut used = vec![false; nodes.len()];
        attract.retain(|p| {
            let (i, d) = nodes
                .iter()
                .enumerate()
                .map(|(i, q)| (i, distance(*p, *q)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            if d < 6.25 {
                return false;
            }
            if d < (h * 0.40).powi(2) {
                let n = norm(std::array::from_fn(|k| p[k] - nodes[i][k]));
                for k in 0..3 {
                    sums[i][k] += n[k];
                }
                used[i] = true;
            }
            true
        });
        let old = nodes.len();
        for i in 0..old {
            if !used[i] || nodes.len() >= 1400 {
                continue;
            }
            let mut d = norm(sums[i]);
            d[1] += 0.035;
            let d = norm(d);
            let q = std::array::from_fn(|k| (nodes[i][k] + d[k] * 2.5).round());
            if q[1] < h * 0.25 || nodes.iter().any(|p| *p == q) {
                continue;
            }
            nodes.push(q);
            parents.push(i);
        }
        if nodes.len() == old || attract.is_empty() {
            break;
        }
    }
    let mut mass = vec![1usize; nodes.len()];
    for i in (1..nodes.len()).rev() {
        mass[parents[i]] += mass[i];
    }
    let mut wood = Vec::new();
    let mut foliage = Vec::new();
    // Trunk character has an independent DNA stream: crown growth cannot
    // accidentally change the lean, curl or fork of a seeded tree.
    let mut dna = Rng(mix(seed ^ 0x7472756e6b646e62));
    let character = dna.unit().powf(1.5);
    let angle = dna.between(0., std::f64::consts::TAU);
    let lean = dna.between(-0.16, 0.16) * (0.3 + character) * h;
    let bend = dna.between(0.015, 0.14) * (0.35 + character) * h;
    let curl = dna.between(-0.065, 0.065) * character * h;
    let fork = dna.unit() < 0.32;
    let spread = dna.between(0.035, 0.075) * h;
    let join = h * 0.32;
    let base_height = (join * 0.30).round().max(4.);
    let base_radius = (h * 0.04).round().max(3.);
    let transform = |p: [f64; 3], sign: f64| {
        let t = (p[1] / join).clamp(0., 1.);
        let arc = lean * t + bend * (std::f64::consts::PI * t).sin();
        let u = ((p[1] - base_height) / (join - base_height)).clamp(0., 1.);
        let split = if fork {
            sign * spread * (std::f64::consts::PI * u).sin().max(0.).powf(1.4)
        } else {
            0.
        };
        let lateral =
            curl * (std::f64::consts::TAU * t).sin() * (std::f64::consts::PI * t).sin() + split;
        [
            (p[0] + arc * angle.cos() - lateral * angle.sin()).round() as i64,
            p[1].round() as i64,
            (p[2] + arc * angle.sin() + lateral * angle.cos()).round() as i64,
        ]
    };
    for i in 1..nodes.len() {
        let r = ((mass[i] as f64).sqrt() * 0.16).floor().min(3.) as i64;
        for sign in if fork { &[-1., 1.][..] } else { &[0.][..] } {
            wood.push(plant_recipe::WoodyStroke::new(
                transform(nodes[parents[i]], *sign),
                transform(nodes[i], *sign),
                r,
            ));
        }
    }
    // The split begins above this single filled bole, never at ground level.
    for y in 0..=base_height as i64 {
        let t = y as f64 / base_height;
        let radius = (2. + (base_radius - 2.) * (1. - t).powf(1.2)).round() as i64;
        let p = transform([0., y as f64, 0.], 0.);
        wood.push(plant_recipe::WoodyStroke::new(p, p, radius));
    }
    // Keep the complete woody scaffold and every terminal spray. Neighboring
    // intermediate growth competes for space instead of packing a continuous
    // leaf sheet along each 2.5-cell branch step. Tip-first ordering also gives
    // coherent young pigment precedence where successive sprays overlap.
    let mut sprays = Vec::new();
    for i in 1..nodes.len() {
        if nodes[i][1] < h * 0.30 || mass[i] > 9 {
            continue;
        }
        let p = transform(nodes[i], 0.);
        let rx = (2. + (rng.unit() * 3.).floor()) * 0.88;
        let rz = (2. + (rng.unit() * 3.).floor()) * 0.88;
        sprays.push((mass[i], i, p, rx, rz));
    }
    sprays.sort_by_key(|&(mass, i, _, _, _)| (mass != 1, i));
    let mut centers: Vec<[i64; 3]> = Vec::new();
    for (n, (mass, _, p, rx, rz)) in sprays.into_iter().enumerate() {
        if n % 128 == 0 && cancel() {
            return None;
        }
        if mass != 1
            && centers
                .iter()
                .any(|q| (p[0] - q[0]).pow(2) + (p[1] - q[1]).pow(2) + (p[2] - q[2]).pow(2) < 13)
        {
            continue;
        }
        centers.push(p);
        foliage.push(plant_recipe::FoliageSpray {
            center: p.map(|x| i16::try_from(x).expect("bounded rowan spray")),
            mass: mass as u16,
            rx,
            rz,
        });
    }
    Some(plant_recipe::RowanRecipe {
        seed,
        variant,
        height: h,
        wood,
        foliage,
    })
}

const WOOD_CACHE_BYTES: usize = 16 * 1024 * 1024;
const WOOD_CACHE_ENTRIES: usize = 256;
type WoodKey = (u8, u64);
struct WoodOccupancy {
    // Local canonical bounds are < 200 cells; six bytes per occupied wood cell
    // retain exact identity without pigments, soft foliage or a tree-map index.
    cells: Box<[[i16; 3]]>,
}
impl WoodOccupancy {
    fn from_model(model: &PlantModel) -> Self {
        let cells = model
            .voxels
            .iter()
            .filter(|v| v.material == Material::Wood)
            .map(|v| {
                [v.cell.x, v.cell.y, v.cell.z]
                    .map(|c| i16::try_from(c).expect("bounded local rowan cell"))
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        // PlantModel's canonical BTreeMap traversal already orders x,y,z.
        Self { cells }
    }
    fn contains(&self, c: Cell) -> bool {
        let (Ok(x), Ok(y), Ok(z)) = (i16::try_from(c.x), i16::try_from(c.y), i16::try_from(c.z))
        else {
            return false;
        };
        self.cells.binary_search(&[x, y, z]).is_ok()
    }
    fn bytes(&self) -> usize {
        self.cells.len() * std::mem::size_of::<[i16; 3]>() + std::mem::size_of::<Self>()
    }
}
#[derive(Default)]
struct WoodCache {
    entries: VecDeque<(WoodKey, Arc<WoodOccupancy>)>,
    bytes: usize,
}
impl WoodCache {
    fn get(&mut self, key: WoodKey) -> Option<Arc<WoodOccupancy>> {
        let i = self.entries.iter().position(|e| e.0 == key)?;
        let entry = self.entries.remove(i).unwrap();
        let result = entry.1.clone();
        self.entries.push_back(entry);
        Some(result)
    }
    fn insert(&mut self, key: WoodKey, wood: Arc<WoodOccupancy>) -> Arc<WoodOccupancy> {
        if let Some(existing) = self.get(key) {
            return existing;
        }
        let bytes = wood.bytes();
        while self.bytes.saturating_add(bytes) > WOOD_CACHE_BYTES
            || self.entries.len() >= WOOD_CACHE_ENTRIES
        {
            if let Some((_, old)) = self.entries.pop_front() {
                self.bytes -= old.bytes();
            } else {
                break;
            }
        }
        if bytes <= WOOD_CACHE_BYTES {
            self.bytes += bytes;
            self.entries.push_back((key, wood.clone()));
        }
        wood
    }
}
fn wood_cache() -> &'static Mutex<WoodCache> {
    static CACHE: OnceLock<Mutex<WoodCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(WoodCache::default()))
}
fn cache_wood_model(key: WoodKey, model: &PlantModel) -> Arc<WoodOccupancy> {
    if let Some(wood) = wood_cache().lock().unwrap().get(key) {
        return wood;
    }
    let wood = Arc::new(WoodOccupancy::from_model(model));
    wood_cache().lock().unwrap().insert(key, wood)
}
fn wood_for_with(
    record: &PlantRecord,
    generate: impl FnOnce() -> Arc<PlantModel>,
) -> Arc<WoodOccupancy> {
    let key = (record.variant % variant_count(Species::Rowan), record.seed);
    if let Some(wood) = wood_cache().lock().unwrap().get(key) {
        return wood;
    }
    // First-ever cold queries still construct canonical growth. Display growth
    // populates this much smaller cache before its full foliage model can evict.
    cache_wood_model(key, &generate())
}
fn wood_for(record: &PlantRecord) -> Arc<WoodOccupancy> {
    wood_for_with(record, || model_for(record))
}

type ModelKey = (Species, u8, u64);
#[derive(Default)]
struct ModelCache {
    entries: VecDeque<(ModelKey, Arc<PlantModel>, usize)>,
    bytes: usize,
}
fn model_cache(species: Species) -> &'static Mutex<ModelCache> {
    static WOOD: OnceLock<Mutex<ModelCache>> = OnceLock::new();
    static SOFT: OnceLock<Mutex<ModelCache>> = OnceLock::new();
    // Carpet churn cannot evict the more expensive woody growth.
    let cache = if species == Species::Rowan {
        &WOOD
    } else {
        &SOFT
    };
    cache.get_or_init(|| Mutex::new(ModelCache::default()))
}
/// Cache generated content, never a finite bank of replacement shapes. The full
/// seed is canonical identity; root/rotation only transform that local growth.
pub fn model_for(record: &PlantRecord) -> Arc<PlantModel> {
    model_for_cancellable(record, || false).unwrap()
}
pub fn model_for_cancellable(
    record: &PlantRecord,
    cancel: impl Fn() -> bool,
) -> Option<Arc<PlantModel>> {
    if cancel() {
        return None;
    }
    let key = (
        record.species,
        record.variant % variant_count(record.species),
        record.seed,
    );
    {
        let mut cache = model_cache(record.species).lock().unwrap();
        if let Some(i) = cache.entries.iter().position(|e| e.0 == key) {
            let entry = cache.entries.remove(i).unwrap();
            let result = entry.1.clone();
            cache.entries.push_back(entry);
            return Some(result);
        }
    }
    // Growth runs outside the lock so streaming cannot block native queries.
    let generated = match key.0 {
        Species::Grass => generate_grass(key.2, key.1),
        Species::Edelweiss => generate_edelweiss(key.2, key.1),
        Species::Rowan => grow_rowan(key.2, key.1, &cancel)?,
    };
    if cancel() {
        return None;
    }
    let generated = Arc::new(generated);
    // Includes conservative BTreeMap allocation overhead, not just voxel payload.
    let bytes = generated.voxels.capacity() * std::mem::size_of::<PlantVoxel>()
        + generated.voxels.len() * 96;
    let mut cache = model_cache(record.species).lock().unwrap();
    let budget = if record.species == Species::Rowan {
        MODEL_CACHE_BYTES
    } else {
        8 * 1024 * 1024
    };
    if let Some(entry) = cache.entries.iter().find(|e| e.0 == key) {
        return Some(entry.1.clone());
    }
    while cache.bytes.saturating_add(bytes) > budget || cache.entries.len() >= MODEL_CACHE_ENTRIES {
        if let Some((_, _, old)) = cache.entries.pop_front() {
            cache.bytes -= old;
        } else {
            break;
        }
    }
    if bytes <= budget {
        cache.bytes += bytes;
        cache.entries.push_back((key, generated.clone(), bytes));
    }
    Some(generated)
}
/// Per-record models are prepared on demand by the bounded streaming worker.
/// Retained for callers that used the previous fixed-bank loading hook.
pub fn prewarm_models() {}

#[derive(Debug, Default)]
struct PlacementPages {
    ground: VecDeque<((i64, i64, Species), Arc<Vec<PlantRecord>>)>,
    trees: HashMap<(i64, i64), Arc<Vec<PlantRecord>>>,
    tree_order: VecDeque<(i64, i64)>,
}
#[derive(Debug, Default)]
pub(crate) struct TileCache(Mutex<PlacementPages>);
impl TileCache {
    fn tile(&self, w: &World, x: i64, z: i64, species: Species) -> Arc<Vec<PlantRecord>> {
        let key = (x, z, species);
        {
            let cache = self.0.lock().unwrap();
            if species == Species::Rowan {
                if let Some(r) = cache.trees.get(&(x, z)) {
                    return r.clone();
                }
            } else if let Some((_, r)) = cache.ground.iter().find(|(k, _)| *k == key) {
                return r.clone();
            }
        }
        let records = Arc::new(place_tile(w, x, z, species));
        let mut cache = self.0.lock().unwrap();
        if species == Species::Rowan {
            // Supported-tree stencils are shared by small ground probes. Keep
            // their tiny 0/1-record pages separate from large grass pages.
            if !cache.trees.contains_key(&(x, z)) {
                while cache.trees.len() >= 4096 {
                    if let Some(old) = cache.tree_order.pop_front() {
                        cache.trees.remove(&old);
                    }
                }
                cache.trees.insert((x, z), records.clone());
                cache.tree_order.push_back((x, z));
            }
        } else {
            if cache.ground.len() >= 128 {
                cache.ground.pop_front();
            }
            cache.ground.push_back((key, records.clone()));
        }
        records
    }
}
fn tree_candidate(seed: u64, x: i64, z: i64) -> (i64, i64, u64) {
    let h = hash(seed ^ 0x74726565, x, z);
    (
        x * 256 + 16 + (h % 224) as i64,
        z * 256 + 16 + ((h >> 16) % 224) as i64,
        h,
    )
}
fn tree_habitat(seed: u64, x: i64, z: i64, roll: u64) -> bool {
    let broad = patch_field(seed ^ 0x776f6f646c616e64, x, z, 768);
    let edge = patch_field(seed ^ 0x67726f7665, x, z, 320);
    let density = (0.06 + 1.8 * (broad - 0.30).max(0.) + 0.35 * (edge - 0.5)).clamp(0.03, 0.94);
    (roll >> 11) as f64 / ((1u64 << 53) as f64) < density
}
fn tree_allowed(seed: u64, x: i64, z: i64) -> bool {
    let (px, pz, h) = tree_candidate(seed, x, z);
    if !tree_habitat(seed, px, pz, h) {
        return false;
    }
    for dx in -1..=1 {
        for dz in -1..=1 {
            if dx == 0 && dz == 0 {
                continue;
            }
            let (qx, qz, qh) = tree_candidate(seed, x + dx, z + dz);
            if tree_habitat(seed, qx, qz, qh)
                && qh < h
                && (px - qx).pow(2) + (pz - qz).pow(2) < 132 * 132
            {
                return false;
            }
        }
    }
    true
}
fn supported(w: &World, x: i64, z: i64, radius: i64, max_grade: i64) -> Option<i64> {
    let h = w.height_cells(x, z);
    if w.source(Cell { x, y: h - 1, z }) != Material::Grass {
        return None;
    }
    for (dx, dz) in [(radius, 0), (-radius, 0), (0, radius), (0, -radius)] {
        let y = w.height_cells(x + dx, z + dz);
        if (y - h).abs() > max_grade
            || w.source(Cell {
                x: x + dx,
                y: y - 1,
                z: z + dz,
            }) != Material::Grass
            || w.volume_owned(Cell {
                x: x + dx,
                y,
                z: z + dz,
            })
        {
            return None;
        }
    } // Terrain paths/rock and explicit topology ownership are excluded.
    if w.volume_owned(Cell { x, y: h, z }) {
        return None;
    }
    Some(h)
}
// Globally addressed value noise, smooth across tiles and the negative origin.
fn patch_field(seed: u64, x: i64, z: i64, scale: i64) -> f64 {
    let gx = x.div_euclid(scale);
    let gz = z.div_euclid(scale);
    let smooth = |t: f64| t * t * (3. - 2. * t);
    let u = smooth(x.rem_euclid(scale) as f64 / scale as f64);
    let v = smooth(z.rem_euclid(scale) as f64 / scale as f64);
    let sample = |a, b| (hash(seed, a, b) >> 11) as f64 / ((1u64 << 53) as f64);
    let a = sample(gx, gz) * (1. - u) + sample(gx + 1, gz) * u;
    let b = sample(gx, gz + 1) * (1. - u) + sample(gx + 1, gz + 1) * u;
    a * (1. - v) + b * v
}
/// Expected birth probability before the canonical per-candidate random roll.
/// Display estimators may average this field without promoting a single rare
/// blossom into a whole coarse population. Placement uses this same function.
pub fn flower_birth_probability(world_seed: u64, x: i64, z: i64) -> f64 {
    let colony = patch_field(world_seed ^ 0x666c6f776572, x, z, 64);
    0.001 + 0.30 * ((colony - 0.65) / 0.35).max(0.).powi(2)
}
fn ground_candidate(world_seed: u64, gx: i64, gz: i64) -> (i64, i64, u64, Option<Species>) {
    let seed = hash(world_seed ^ 0x6d6561646f77, gx, gz);
    let x = gx * 8 + 2 + (seed % 4) as i64;
    let z = gz * 8 + 2 + ((seed >> 16) % 4) as i64;
    let meadow = patch_field(world_seed ^ 0x7061746368, x, z, 192);
    let tuft = patch_field(world_seed ^ 0x74756674, x, z, 48);
    let bloom_probability = flower_birth_probability(world_seed, x, z);
    let roll = |salt| (mix(seed ^ salt) >> 11) as f64 / ((1u64 << 53) as f64);
    let species = if roll(0x626c6f6f6d) < bloom_probability {
        Some(Species::Edelweiss)
    } else if roll(0x6772617373) < (0.72 + 0.35 * meadow + 0.12 * (tuft - 0.5)).clamp(0.65, 0.98) {
        Some(Species::Grass)
    } else {
        None
    };
    (x, z, seed, species)
}
fn tree_supported(w: &World, x: i64, z: i64) -> Option<i64> {
    let h = supported(w, x, z, 6, 2)?;
    for dx in -6..=6 {
        for dz in -6..=6 {
            if dx * dx + dz * dz > 36 {
                continue;
            }
            let y = w.height_cells(x + dx, z + dz);
            if (y - h).abs() > 2
                || w.source(Cell {
                    x: x + dx,
                    y: y - 1,
                    z: z + dz,
                }) != Material::Grass
            {
                return None;
            }
        }
    }
    Some(h)
}
fn place_tile(w: &World, tx: i64, tz: i64, species: Species) -> Vec<PlantRecord> {
    place_tile_region(w, tx, tz, species, None)
}
// Identical source rules with early root-range rejection. Coarse display probes
// must not evaluate habitat/support for all 1024 candidates of a 16 m placement tile.
fn place_tile_region(
    w: &World,
    tx: i64,
    tz: i64,
    species: Species,
    bounds: Option<([i64; 2], [i64; 2])>,
) -> Vec<PlantRecord> {
    let mut records = Vec::new();
    let (x, z, seed) = tree_candidate(w.seed, tx, tz);
    if species == Species::Rowan && tree_allowed(w.seed, tx, tz) {
        if let Some(y) = tree_supported(w, x, z) {
            records.push(PlantRecord {
                species: Species::Rowan,
                seed,
                root: Cell { x, y: y - 2, z },
                // Broad resource availability sets maturity; local seed varies individuals.
                variant: ((patch_field(w.seed ^ 0x6d6174757265, x, z, 512) * 6.
                    + (mix(seed ^ 77) % 3) as f64)
                    .floor() as u8)
                    .min(7),
                rotation: ((seed >> 8) % 4) as u8,
            });
        }
    }
    if species == Species::Rowan {
        return records;
    }
    // Resolve supported neighbors once per tile: unsupported candidates cannot
    // cast ecological shade or clear a phantom hole in the meadow.
    let mut trees = Vec::new();
    for dx in -1..=1 {
        for dz in -1..=1 {
            if let Some(cache) = &w.vegetation {
                // Reuse canonical supported tree records across neighboring
                // bounded probes instead of repeating their support stencil.
                trees.extend(
                    cache
                        .tile(w, tx + dx, tz + dz, Species::Rowan)
                        .iter()
                        .map(|r| (r.root.x, r.root.z)),
                );
            } else {
                let (a, b, _) = tree_candidate(w.seed, tx + dx, tz + dz);
                if tree_allowed(w.seed, tx + dx, tz + dz) && tree_supported(w, a, b).is_some() {
                    trees.push((a, b));
                }
            }
        }
    }
    let grid = |axis: usize, tile: i64| {
        bounds
            .map(|(min, max)| {
                (
                    (min[axis].div_euclid(8) - tile * 32).clamp(0, 32),
                    ((max[axis] - 1).div_euclid(8) + 1 - tile * 32).clamp(0, 32),
                )
            })
            .unwrap_or((0, 32))
    };
    let (x0, x1) = grid(0, tx);
    let (z0, z1) = grid(1, tz);
    for gx in x0..x1 {
        for gz in z0..z1 {
            let (x, z, seed, chosen) = ground_candidate(w.seed, tx * 32 + gx, tz * 32 + gz);
            if chosen != Some(species) {
                continue;
            }
            if bounds
                .is_some_and(|(min, max)| x < min[0] || z < min[1] || x >= max[0] || z >= max[1])
            {
                continue;
            }
            let nearest = trees
                .iter()
                .map(|(a, b)| (a - x).pow(2) + (b - z).pow(2))
                .min()
                .unwrap_or(i64::MAX);
            if nearest <= 14 * 14 {
                continue;
            }
            let shade = (nearest as f64).sqrt();
            let retention = if species == Species::Edelweiss {
                0.12
            } else {
                0.38
            };
            let canopy_keep = retention + (1. - retention) * ((shade - 20.) / 44.).clamp(0., 1.);
            if (mix(seed ^ 0x7368616465) >> 11) as f64 / ((1u64 << 53) as f64) > canopy_keep {
                continue;
            }
            if let Some(y) = supported(w, x, z, 3, 2) {
                records.push(PlantRecord {
                    species,
                    seed,
                    root: Cell { x, y, z },
                    variant: if species == Species::Grass {
                        // Height cohorts follow the same ecological field, so short
                        // meadow mats and taller clumps coexist without a lattice.
                        let tall = patch_field(w.seed ^ 0x686569676874, x, z, 64) > 0.63
                            && mix(seed ^ 0x74616c6c) % 5 == 0;
                        ((seed >> 24) % 4) as u8 + if tall { 4 } else { 0 }
                    } else {
                        ((seed >> 24) % variant_count(species) as u64) as u8
                    },
                    rotation: ((seed >> 32) % 4) as u8,
                });
            }
        }
    }
    records.sort_by_key(|r| r.root.x);
    records
}
impl World {
    pub fn with_vegetation(mut self) -> Self {
        self.vegetation = Some(Arc::default());
        self
    }
    pub fn with_vegetation_enabled(mut self, enabled: bool) -> Self {
        self.vegetation = enabled.then(|| self.vegetation.clone().unwrap_or_default());
        self
    }
    pub fn vegetation_enabled(&self) -> bool {
        self.vegetation.is_some()
    }
    /// Root positions in a half-open native x/z rectangle. Expand by the maximum
    /// model radius when requesting all plants intersecting a display region.
    pub fn vegetation_records(&self, min: [i64; 2], max: [i64; 2]) -> Vec<PlantRecord> {
        [Species::Grass, Species::Edelweiss, Species::Rowan]
            .into_iter()
            .flat_map(|s| self.vegetation_records_species(min, max, s))
            .collect()
    }
    pub fn vegetation_records_species(
        &self,
        min: [i64; 2],
        max: [i64; 2],
        species: Species,
    ) -> Vec<PlantRecord> {
        let Some(cache) = &self.vegetation else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if min[0] >= max[0] || min[1] >= max[1] {
            return out;
        }
        for tx in min[0].div_euclid(256)..=(max[0] - 1).div_euclid(256) {
            for tz in min[1].div_euclid(256)..=(max[1] - 1).div_euclid(256) {
                out.extend(
                    cache
                        .tile(self, tx, tz, species)
                        .iter()
                        .copied()
                        .filter(|r| {
                            r.root.x >= min[0]
                                && r.root.x < max[0]
                                && r.root.z >= min[1]
                                && r.root.z < max[1]
                        }),
                );
            }
        }
        out
    }
    /// Canonical grass/flower roots in a small rectangle without populating
    /// entire 16 m placement pages. Same support, shade and seed rules as pages.
    pub fn vegetation_ground_records_in(
        &self,
        min: [i64; 2],
        max: [i64; 2],
        species: Species,
    ) -> Vec<PlantRecord> {
        if species == Species::Rowan {
            return self.vegetation_records_species(min, max, species);
        }
        if self.vegetation.is_none() || min[0] >= max[0] || min[1] >= max[1] {
            return Vec::new();
        }
        let mut records = Vec::new();
        for tx in min[0].div_euclid(256)..=(max[0] - 1).div_euclid(256) {
            for tz in min[1].div_euclid(256)..=(max[1] - 1).div_euclid(256) {
                records.extend(place_tile_region(self, tx, tz, species, Some((min, max))));
            }
        }
        records
    }
    pub fn vegetation_voxel(&self, c: Cell) -> Option<PlantVoxel> {
        for species in [Species::Rowan, Species::Grass, Species::Edelweiss] {
            if let Some(v) = self.vegetation_species_voxel(c, species) {
                return Some(v);
            }
        }
        None
    }
    fn vegetation_species_voxel(&self, c: Cell, species: Species) -> Option<PlantVoxel> {
        let cache = self.vegetation.as_ref()?;
        let radius = if species == Species::Rowan {
            MAX_PLANT_RADIUS_CELLS
        } else {
            8
        };
        for tx in (c.x - radius).div_euclid(256)..=(c.x + radius).div_euclid(256) {
            for tz in (c.z - radius).div_euclid(256)..=(c.z + radius).div_euclid(256) {
                let records = cache.tile(self, tx, tz, species);
                // Ground candidates are sorted by x grid coordinate: limit lookup to the
                // few neighboring 1m buckets rather than scanning a tile's whole meadow.
                let lo = records.partition_point(|r| r.root.x < c.x - radius - 10);
                for r in records[lo..]
                    .iter()
                    .take_while(|r| r.root.x <= c.x + radius + 10)
                {
                    let local = r.local_cell(c);
                    if local.y < 0
                        || local.y > MAX_PLANT_HEIGHT_CELLS
                        || local.x.abs() > radius
                        || local.z.abs() > radius
                    {
                        continue;
                    }
                    if let Some(mut voxel) = model_for(r).voxel(local) {
                        voxel.cell = c;
                        return Some(voxel);
                    }
                }
            }
        }
        None
    }
    /// Collision retains exact canonical wood in a compact cache independent of foliage.
    pub fn blocking_material(&self, c: Cell) -> Material {
        if let Some(&m) = self.edits.get(&c) {
            return if m.is_blocking() { m } else { Material::Air };
        }
        let terrain = self.source(c);
        if terrain != Material::Air {
            return terrain;
        }
        let Some(cache) = &self.vegetation else {
            return Material::Air;
        };
        let radius = MAX_PLANT_RADIUS_CELLS;
        for tx in (c.x - radius).div_euclid(256)..=(c.x + radius).div_euclid(256) {
            for tz in (c.z - radius).div_euclid(256)..=(c.z + radius).div_euclid(256) {
                for record in cache.tile(self, tx, tz, Species::Rowan).iter() {
                    let local = record.local_cell(c);
                    if local.y < 0
                        || local.y > MAX_PLANT_HEIGHT_CELLS
                        || local.x.abs() > radius
                        || local.z.abs() > radius
                    {
                        continue;
                    }
                    if wood_for(record).contains(local) {
                        return Material::Wood;
                    }
                }
            }
        }
        Material::Air
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rooted_component_preserves_sparse_disconnected_and_absent_roots() {
        for cells in [
            vec![],
            vec![Cell { x: 1, y: 0, z: 0 }],
            vec![
                Cell::default(),
                Cell { x: 1, y: 0, z: 0 },
                Cell { x: 2, y: 0, z: 0 },
                Cell { x: 2, y: 1, z: 1 },
                Cell { x: 0, y: 2, z: 0 },
            ],
            vec![
                Cell::default(),
                Cell { x: 0, y: -1, z: 0 },
                Cell {
                    x: 1_000_000_000_000,
                    y: 0,
                    z: -1_000_000_000_000,
                },
            ],
        ] {
            let cells: Cells = cells
                .into_iter()
                .map(|c| (c, (Material::Leaf, [127; 3])))
                .collect();
            let sorted: Vec<_> = cells.iter().map(|(&c, &paint)| (c, paint)).collect();
            assert_eq!(
                rooted_component(&sorted),
                rooted_component_reference(&cells)
            );
        }
    }
    #[test]
    fn generated_models_check_original_component_filter_on_every_candidate() {
        // model() compares each pre-filter rasterization with the original
        // BTreeMap/BTreeSet filter under cfg(test). The pigment/voxel/index
        // construction below that comparison remains the original arithmetic.
        for seed in [0, 17, 97, u64::MAX] {
            for variant in [0, 3, 7] {
                for model in [
                    generate_grass(seed, variant),
                    generate_edelweiss(seed, variant),
                    generate_rowan(seed, variant),
                ] {
                    assert!(model.voxels.windows(2).all(|v| v[0].cell < v[1].cell));
                    for v in &model.voxels {
                        assert_eq!(model.voxel(v.cell), Some(*v));
                    }
                }
            }
        }
    }
    #[test]
    #[ignore = "CPU-only release rooted-filter benchmark; --release --ignored --nocapture"]
    fn rooted_component_microbenchmark() {
        use std::{hint::black_box, time::Instant};
        for variant in [0, 3, 7] {
            let plant = generate_rowan(123, variant);
            let cells: Cells = plant
                .voxels
                .iter()
                .map(|v| (v.cell, (v.material, [127; 3])))
                .collect();
            let sorted: Vec<_> = cells.iter().map(|(&c, &p)| (c, p)).collect();
            assert_eq!(
                rooted_component(&sorted),
                rooted_component_reference(&cells)
            );
            let mut old = Vec::new();
            let mut new = Vec::new();
            for i in 0..15 {
                let mut reference = || {
                    let t = Instant::now();
                    black_box(rooted_component_reference(&cells));
                    old.push(t.elapsed());
                };
                let mut indexed = || {
                    let t = Instant::now();
                    black_box(rooted_component(&sorted));
                    new.push(t.elapsed());
                };
                if i % 2 == 0 {
                    reference();
                    indexed();
                } else {
                    indexed();
                    reference();
                }
            }
            old.sort();
            new.sort();
            eprintln!("rowan rooted filter seed=123 variant={variant} cells={} reference_us={} indexed_us={} speedup={:.2}",cells.len(),old[7].as_micros(),new[7].as_micros(),old[7].as_secs_f64()/new[7].as_secs_f64());
        }
    }

    use super::*;
    fn record(species: Species, variant: u8) -> PlantRecord {
        PlantRecord {
            species,
            variant,
            seed: 17,
            root: Cell {
                x: -13,
                y: 42,
                z: -257,
            },
            rotation: 0,
        }
    }
    #[test]
    fn compact_wood_matches_seeded_rotated_canonical_models() {
        for i in 0..8u64 {
            let record = PlantRecord {
                seed: mix(0x776f6f645f746573 ^ i),
                ..record(Species::Rowan, i as u8)
            };
            let model = model_for(&record);
            assert!(
                model.min.x >= -MAX_PLANT_RADIUS_CELLS && model.max.x <= MAX_PLANT_RADIUS_CELLS
            );
            assert!(
                model.min.z >= -MAX_PLANT_RADIUS_CELLS && model.max.z <= MAX_PLANT_RADIUS_CELLS
            );
            assert!(model.max.y <= MAX_PLANT_HEIGHT_CELLS);
            let cells: Cells = model
                .voxels
                .iter()
                .map(|v| (v.cell, (v.material, [0; 3])))
                .collect();
            assert_eq!(
                self::model(cells, 0).voxels.len(),
                model.voxels.len(),
                "seeded crown remains rooted"
            );
            let wood = wood_for(&record);
            assert!(wood.cells.windows(2).all(|p| p[0] < p[1]));
            for rotation in 0..4 {
                let rotated = PlantRecord { rotation, ..record };
                for v in &model.voxels {
                    let c = rotated.local_cell(rotated.world_cell(v.cell));
                    assert_eq!(wood.contains(c), v.material == Material::Wood);
                }
            }
            for j in 0..256 {
                let h = mix(record.seed ^ j);
                let c = Cell {
                    x: (h % 193) as i64 - 96,
                    y: ((h >> 16) % 177) as i64,
                    z: ((h >> 32) % 193) as i64 - 96,
                };
                assert_eq!(
                    wood.contains(c),
                    model.voxel(c).is_some_and(|v| v.material == Material::Wood)
                );
            }
            assert!(!wood.contains(Cell {
                x: i64::MAX,
                ..Cell::default()
            }));
        }
    }
    #[test]
    fn compact_wood_survives_full_model_eviction_without_generation() {
        let record = PlantRecord {
            seed: 0xf473c12adead0201,
            ..record(Species::Rowan, 5)
        };
        let expected = model_for(&record);
        let key = (Species::Rowan, 5, record.seed);
        {
            let mut cache = model_cache(Species::Rowan).lock().unwrap();
            if let Some(i) = cache.entries.iter().position(|e| e.0 == key) {
                cache.bytes -= cache.entries.remove(i).unwrap().2;
            }
            assert!(!cache.entries.iter().any(|e| e.0 == key));
        }
        let wood = wood_for_with(&record, || {
            panic!("wood hit must not regenerate full foliage")
        });
        assert_eq!(
            wood.cells.len(),
            expected
                .voxels
                .iter()
                .filter(|v| v.material == Material::Wood)
                .count()
        );
    }
    #[test]
    fn compact_wood_cache_enforces_entry_and_byte_bounds() {
        let mut cache = WoodCache::default();
        for seed in 0..300 {
            cache.insert(
                (0, seed),
                Arc::new(WoodOccupancy {
                    cells: vec![[0, 0, 0]].into_boxed_slice(),
                }),
            );
            assert!(cache.entries.len() <= WOOD_CACHE_ENTRIES);
        }
        assert_eq!(cache.entries.len(), WOOD_CACHE_ENTRIES);
        let held = cache.get((0, 299)).unwrap();
        for seed in 300..400 {
            cache.insert(
                (0, seed),
                Arc::new(WoodOccupancy {
                    cells: vec![[0, 0, 0]; 40_000].into_boxed_slice(),
                }),
            );
            assert!(cache.bytes <= WOOD_CACHE_BYTES && cache.entries.len() <= WOOD_CACHE_ENTRIES);
        }
        assert!(
            cache.entries.len() < 100,
            "byte budget must evict even below entry cap"
        );
        assert!(
            held.contains(Cell::default()),
            "eviction cannot mutate retained canonical content"
        );
        assert_eq!(
            cache.bytes,
            cache.entries.iter().map(|e| e.1.bytes()).sum::<usize>()
        );
    }
    #[test]
    fn compact_wood_collision_preserves_authoritative_edits() {
        let mut w = World::new(37).with_vegetation();
        let records = w.vegetation_records_species([-512, -512], [512, 512], Species::Rowan);
        let c = records
            .iter()
            .find_map(|p| {
                model_for(p)
                    .voxels
                    .iter()
                    .filter(|v| v.material == Material::Wood)
                    .map(|v| p.world_cell(v.cell))
                    .find(|c| w.source(*c) == Material::Air)
            })
            .expect("test region has exposed canonical wood");
        assert_eq!(w.blocking_material(c), Material::Wood);
        for (edit, blocking) in [
            (Material::Air, Material::Air),
            (Material::Flower, Material::Air),
            (Material::Stone, Material::Stone),
        ] {
            w.set(c, edit);
            assert_eq!(w.blocking_material(c), blocking);
        }
    }
    #[test]
    fn record_seed_drives_growth_and_cache_reuse_is_content_exact() {
        for species in [Species::Grass, Species::Edelweiss, Species::Rowan] {
            let a = record(species, 2);
            let b = PlantRecord {
                seed: a.seed + 1,
                ..a
            };
            let first = model_for(&a);
            assert!(Arc::ptr_eq(&first, &model_for(&a)));
            let second = model_for(&b);
            assert_ne!(first.voxels, second.voxels, "seed must affect {species:?}");
            assert_ne!(
                first.voxels.iter().map(|v| v.cell).collect::<Vec<_>>(),
                second.voxels.iter().map(|v| v.cell).collect::<Vec<_>>(),
                "seed must change growth geometry, not only pigment: {species:?}"
            );
            assert!(model_for_cancellable(&b, || true).is_none());
        }
    }
    #[test]
    fn growth_cohorts_have_distinct_heights_full_crowns_and_native_grass() {
        let mut signatures = BTreeSet::new();
        let mut grass_heights = BTreeSet::new();
        for variant in 0..variant_count(Species::Rowan) {
            let m = model_for(&record(Species::Rowan, variant));
            signatures.insert((
                m.max.y,
                m.max.x - m.min.x,
                m.max.z - m.min.z,
                m.voxels.len(),
            ));
            let leaves = m
                .voxels
                .iter()
                .filter(|v| v.material == Material::Leaf)
                .count();
            assert!(leaves > 1800, "full crown, variant {variant}: {leaves}");
        }
        assert_eq!(signatures.len(), 8);
        for variant in 0..variant_count(Species::Grass) {
            let m = model_for(&record(Species::Grass, variant));
            grass_heights.insert(m.max.y);
            assert!(m.max.x - m.min.x >= 2 && m.max.z - m.min.z >= 2);
        }
        assert!(
            grass_heights.len() >= 5,
            "short/tall native tufts: {grass_heights:?}"
        );
    }
    #[test]
    fn ecological_fields_produce_openings_and_localized_flower_colonies() {
        let seed = 37;
        let mut flower_patches = Vec::new();
        let mut tree_patches = Vec::new();
        for tx in -8..8 {
            for tz in -8..8 {
                let mut flowers = 0;
                for gx in 0..32 {
                    for gz in 0..32 {
                        if ground_candidate(seed, tx * 32 + gx, tz * 32 + gz).3
                            == Some(Species::Edelweiss)
                        {
                            flowers += 1;
                        }
                    }
                }
                flower_patches.push(flowers);
                let mut trees = 0;
                for dx in 0..4 {
                    for dz in 0..4 {
                        trees += usize::from(tree_allowed(seed, tx * 4 + dx, tz * 4 + dz));
                    }
                }
                tree_patches.push(trees);
            }
        }
        flower_patches.sort();
        assert!(flower_patches[0] <= 1);
        assert!(flower_patches[255] > 3 * flower_patches[128].max(1));
        assert!(*tree_patches.iter().min().unwrap() <= 2);
        assert!(*tree_patches.iter().max().unwrap() >= 9);
        let w = World::new(seed).with_vegetation();
        for r in w.vegetation_records([-512, -512], [512, 512]) {
            assert!(r.variant < variant_count(r.species));
        }
    }
    #[test]
    fn coherent_meadow_and_colonies_cross_tile_borders() {
        let seed = 37;
        let mut grass = 0;
        let mut flowers = 0;
        for gx in -128..128 {
            for gz in -128..128 {
                let a = ground_candidate(seed, gx, gz);
                let tx = gx.div_euclid(32);
                let tz = gz.div_euclid(32);
                assert_eq!(
                    a,
                    ground_candidate(
                        seed,
                        tx * 32 + gx.rem_euclid(32),
                        tz * 32 + gz.rem_euclid(32)
                    )
                );
                assert!((gx * 8..gx * 8 + 8).contains(&a.0));
                assert!((gz * 8..gz * 8 + 8).contains(&a.1));
                match a.3 {
                    Some(Species::Grass) => grass += 1,
                    Some(Species::Edelweiss) => flowers += 1,
                    _ => {}
                }
            }
        }
        let area = 256. * 256. * 0.25;
        let density = grass as f64 / area;
        let blooms = flowers as f64 / area;
        assert!((3.1..3.95).contains(&density), "grass density {density}");
        assert!(
            (0.005..0.08).contains(&blooms),
            "edelweiss density {blooms}"
        );
        for x in [-513, -512, -257, -256, -1, 0, 255, 256, 511, 512] {
            for z in -260..260 {
                let a = patch_field(seed, x, z, 128);
                let b = patch_field(seed, x + 1, z, 128);
                assert!((a - b).abs() < 0.012);
            }
        }
        let w = World::new(seed).with_vegetation();
        for r in w.vegetation_records_species([-1024, -1024], [1024, 1024], Species::Rowan) {
            assert_eq!(r.root.y, w.height_cells(r.root.x, r.root.z) - 2);
            for dx in -4..=4 {
                for dz in -4..=4 {
                    if dx * dx + dz * dz <= 16 {
                        assert!(w
                            .source(Cell {
                                x: r.root.x + dx,
                                y: r.root.y - 1,
                                z: r.root.z + dz
                            })
                            .is_blocking());
                    }
                }
            }
        }
    }
    #[test]
    fn native_models_are_connected_bounded_and_repeatable() {
        for species in [Species::Grass, Species::Edelweiss, Species::Rowan] {
            for variant in 0..variant_count(species) {
                let r = record(species, variant);
                let m = model_for(&r);
                assert!(!m.voxels.is_empty());
                assert!(m.voxel(Cell::default()).is_some());
                assert!(m.min.x >= -MAX_PLANT_RADIUS_CELLS && m.max.x <= MAX_PLANT_RADIUS_CELLS);
                assert!(m.min.z >= -MAX_PLANT_RADIUS_CELLS && m.max.z <= MAX_PLANT_RADIUS_CELLS);
                assert!(m.max.y <= MAX_PLANT_HEIGHT_CELLS);
                let cells: Cells = m
                    .voxels
                    .iter()
                    .map(|v| (v.cell, (v.material, [0; 3])))
                    .collect();
                assert_eq!(model(cells, 0).voxels.len(), m.voxels.len());
                for rotation in 0..4 {
                    let r = PlantRecord { rotation, ..r };
                    for v in &m.voxels {
                        assert_eq!(r.local_cell(r.world_cell(v.cell)), v.cell);
                        assert_eq!(m.voxel(v.cell), Some(*v));
                    }
                }
                if species == Species::Rowan {
                    let target = [52., 65., 76., 84., 94., 104., 112., 88.][variant as usize];
                    assert!((target * 0.72..=target * 1.08 + 5.).contains(&(m.max.y as f64)));
                    assert!(m.voxels.len() > 3000);
                    let flowers: Vec<_> = m
                        .voxels
                        .iter()
                        .filter(|v| v.material == Material::Flower)
                        .collect();
                    assert!(flowers.len() <= (target * 1.2) as usize * 7);
                    for flower in flowers {
                        assert!(m
                            .voxel(Cell {
                                y: flower.cell.y + 1,
                                ..flower.cell
                            })
                            .is_none());
                    }
                    for x in -3..=3 {
                        for z in -3..=3 {
                            if x * x + z * z <= 9 {
                                assert_eq!(
                                    m.voxel(Cell { x, y: 0, z }).unwrap().material,
                                    Material::Wood
                                );
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(generate_grass(71, 1).voxels, generate_grass(71, 1).voxels);
        assert_eq!(
            generate_edelweiss(17, 2).voxels,
            generate_edelweiss(17, 2).voxels
        );
    }
    #[test]
    fn negative_tile_borders_and_request_order() {
        let w = World::new(37).with_vegetation();
        let all = w.vegetation_records([-512, -512], [512, 512]);
        assert!(!all.is_empty());
        let mut split = Vec::new();
        for (lo, hi) in [
            ([-512, -512], [0, 0]),
            ([0, -512], [512, 0]),
            ([-512, 0], [0, 512]),
            ([0, 0], [512, 512]),
        ] {
            split.extend(w.vegetation_records(lo, hi));
        }
        let key = |r: &PlantRecord| (r.species, r.root, r.seed);
        let mut a = all.clone();
        a.sort_by_key(key);
        split.sort_by_key(key);
        assert_eq!(a, split);
        let mut reverse = World::new(37)
            .with_vegetation()
            .vegetation_records([0, 0], [512, 512]);
        reverse.extend(
            World::new(37)
                .with_vegetation()
                .vegetation_records([-512, -512], [0, 0]),
        );
        for r in all {
            let h = w.height_cells(r.root.x, r.root.z);
            assert_eq!(w.source(Cell { y: h - 1, ..r.root }), Material::Grass);
            assert!(w
                .source(Cell {
                    y: r.root.y - 1,
                    ..r.root
                })
                .is_blocking());
        }
    }
    #[test]
    fn occupancy_edits_and_disabled_terrain_regression() {
        let plain = World::new(37);
        let mut w = plain.clone().with_vegetation();
        let records = w.vegetation_records([-256, -256], [512, 512]);
        let mut checked = 0;
        for r in records {
            for v in &model_for(&r).voxels {
                let c = r.world_cell(v.cell);
                if plain.material(c) != Material::Air {
                    continue;
                }
                let canonical = w.vegetation_voxel(c).unwrap();
                assert_eq!(w.material(c), canonical.material);
                assert_eq!(w.column(c.x, c.z).material(c.y), plain.material(c));
                assert_eq!(
                    w.blocking_material(c).is_blocking(),
                    canonical.material.is_blocking()
                );
                checked += 1;
                if checked >= 200 {
                    break;
                }
            }
            if checked >= 200 {
                break;
            }
        }
        assert!(checked > 0);
        let r = w.vegetation_records([-256, -256], [512, 512])[0];
        let c = r.world_cell(model_for(&r).voxels.last().unwrap().cell);
        w.set(c, Material::Air);
        assert_eq!(w.material(c), Material::Air);
        assert_eq!(w.blocking_material(c), Material::Air);
        let off = plain
            .clone()
            .with_vegetation()
            .with_vegetation_enabled(false);
        assert!(off.vegetation_records([-512, -512], [512, 512]).is_empty());
        for x in -20..20 {
            for z in -10..10 {
                let h = plain.height_cells(x, z);
                for y in h - 2..h + 12 {
                    let c = Cell { x, y, z };
                    assert_eq!(plain.material(c), off.material(c));
                    assert_eq!(plain.column(x, z).material(y), w.column(x, z).material(y));
                }
            }
        }
    }
    #[test]
    fn tree_clearance_crosses_negative_tile_borders() {
        for seed in 0..8 {
            let w = World::new(seed).with_vegetation();
            let records =
                w.vegetation_records_species([-1024, -1024], [1024, 1024], Species::Rowan);
            for (i, a) in records.iter().enumerate() {
                for b in &records[i + 1..] {
                    assert!(
                        (a.root.x - b.root.x).pow(2) + (a.root.z - b.root.z).pow(2) >= 132 * 132
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod region_probe_tests {
    use super::*;
    #[test]
    fn bounded_ground_source_is_identical_to_filtered_canonical_pages() {
        let w = World::fbm(
            7,
            crate::FbmParams {
                recipe: crate::FbmRecipe::Alpine,
                ..Default::default()
            },
        )
        .unwrap()
        .with_vegetation();
        for min in [[-930, -770], [-8, -8], [240, 248]] {
            let max = min.map(|v| v + 32);
            for species in [Species::Grass, Species::Edelweiss] {
                assert_eq!(
                    w.vegetation_ground_records_in(min, max, species),
                    w.vegetation_records_species(min, max, species)
                );
            }
        }
    }
}
