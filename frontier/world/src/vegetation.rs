//! Seeded native vegetation. Placement reads only the immutable terrain source;
//! camera demand, edits and tile request order never influence plant identity.
use crate::{Cell, Material, World};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex, OnceLock},
};

// Version 2: ecological patches, sparse colonies and a bounded larger model bank.
// This intentionally changes canonical plant records/voxels; display AO does not.
pub const VEGETATION_SOURCE_VERSION: u32 = 2;
pub const TILE_SIZE_CELLS: i64 = 256;
pub const MAX_PLANT_RADIUS_CELLS: i64 = 96;
pub const MAX_PLANT_HEIGHT_CELLS: i64 = 176;
pub const fn variant_count(species: Species) -> u8 {
    match species {
        Species::Grass | Species::Rowan => 8,
        Species::Edelweiss => 3,
    }
}
const MODEL_SLOTS: usize = 24;
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
fn model(mut cells: Cells, seed: u64) -> PlantModel {
    // Retain only the rooted face-connected component after voxel rasterization.
    let root = Cell::default();
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([root]);
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
    cells.retain(|c, _| seen.contains(c));
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
        .map(|(cell, (material, rgb))| {
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
    let mut rng = Rng(seed);
    let mut v = Cells::new();
    let height = [3, 4, 5, 6, 6, 7, 8, 10][variant as usize % 8];
    let base = [96u8, 128, 57];
    let fresh = [108u8, 142, 66];
    let dry = [147u8, 158, 90];
    for _ in 0..[5, 7, 9, 6, 10, 8, 7, 9][variant as usize % 8] {
        let a = rng.between(0., std::f64::consts::TAU);
        let dry_tip = rng.unit() < 0.30;
        let lean = rng.between(1., if variant < 4 { 3. } else { 4. });
        let bx = (rng.between(-1., 1.)).round() as i64;
        let bz = (rng.between(-1., 1.)).round() as i64;
        stroke(&mut v, [0, 0, 0], [bx, 0, bz], 0, Material::Leaf, base);
        let h = (height - (rng.unit() * 4.) as i64).max(2);
        let mut p = [bx, 0, bz];
        for y in 1..h {
            let t = y as f64 / h as f64;
            let bend = 0.35 * t + 0.65 * t * t;
            let q = [
                bx + (a.cos() * lean * bend).round() as i64,
                y,
                bz + (a.sin() * lean * bend).round() as i64,
            ];
            // Whole native voxels carry pigment; these are static leaf colors,
            // not view-dependent highlights or sub-voxel texture detail.
            let growth = (t / 0.65).min(1.);
            let dry_amount = if dry_tip {
                ((t - 0.65) / 0.35).max(0.)
            } else {
                0.
            };
            let color = std::array::from_fn(|i| {
                let green = base[i] as f64 + (fresh[i] as f64 - base[i] as f64) * growth;
                (green + (dry[i] as f64 - green) * dry_amount).round() as u8
            });
            stroke(&mut v, p, q, 0, Material::Leaf, color);
            p = q;
        }
    }
    model(v, seed)
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
        let h = 5 + (variant % 2) as i64;
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
    let mut rng = Rng(seed);
    let h = [52., 65., 76., 84., 94., 104., 112., 88.][variant as usize % 8];
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
    let mut v = Cells::new();
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
            stroke(
                &mut v,
                transform(nodes[parents[i]], *sign),
                transform(nodes[i], *sign),
                r,
                Material::Wood,
                BARK,
            );
        }
    }
    // The split begins above this single filled bole, never at ground level.
    for y in 0..=base_height as i64 {
        let t = y as f64 / base_height;
        let radius = (2. + (base_radius - 2.) * (1. - t).powf(1.2)).round() as i64;
        let p = transform([0., y as f64, 0.], 0.);
        stroke(&mut v, p, p, radius, Material::Wood, BARK);
    }
    for i in 1..nodes.len() {
        if nodes[i][1] < h * 0.30 || mass[i] > 9 {
            continue;
        }
        let p = transform(nodes[i], 0.);
        let rx = 2 + (rng.unit() * 3.) as i64;
        let ry = 2;
        let rz = 2 + (rng.unit() * 3.) as i64;
        for x in -rx..=rx {
            for y in -ry..=ry {
                for z in -rz..=rz {
                    if (x as f64 / rx as f64).powi(2)
                        + (y as f64 / ry as f64).powi(2)
                        + (z as f64 / rz as f64).powi(2)
                        <= 1.
                    {
                        v.entry(Cell {
                            x: p[0] + x,
                            y: p[1] + y,
                            z: p[2] + z,
                        })
                        .or_insert((Material::Leaf, [100, 137, 64]));
                    }
                }
            }
        }
    }
    let mut exposed: Vec<_> = v
        .iter()
        .filter_map(|(p, (m, _))| {
            (*m == Material::Leaf && !v.contains_key(&Cell { y: p.y + 1, ..*p })).then_some(*p)
        })
        .collect();
    // Seeded ordering chooses a bounded number of well-separated sprays.
    exposed.sort_by_key(|p| hash(seed ^ 0x626c6f73736f6d, p.x ^ p.y.rotate_left(13), p.z));
    let mut centers: Vec<Cell> = Vec::new();
    let cap = (h * 0.6 * 2.) as usize;
    for p in exposed {
        if centers.len() >= cap {
            break;
        }
        if centers
            .iter()
            .any(|q| (p.x - q.x).pow(2) + (p.y - q.y).pow(2) + (p.z - q.z).pow(2) < 20)
        {
            continue;
        }
        centers.push(p);
        for (dx, dz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, -1)] {
            for dy in [0, 1, -1] {
                let q = Cell {
                    x: p.x + dx,
                    y: p.y + dy,
                    z: p.z + dz,
                };
                if v.get(&q).is_some_and(|(m, _)| *m == Material::Leaf)
                    && !v.contains_key(&Cell { y: q.y + 1, ..q })
                {
                    v.insert(q, (Material::Flower, WHITE));
                    break;
                }
            }
        }
    }
    model(v, seed)
}
pub fn model_for(record: &PlantRecord) -> &'static PlantModel {
    static MODELS: [OnceLock<PlantModel>; MODEL_SLOTS] = [const { OnceLock::new() }; MODEL_SLOTS];
    let variant = record.variant as usize % variant_count(record.species) as usize;
    let species = record.species;
    MODELS[species as usize * 8 + variant].get_or_init(|| {
        let seed = mix(0x726f77616e + species as u64 * 97 + variant as u64);
        match species {
            Species::Grass => generate_grass(seed, variant as u8),
            Species::Edelweiss => generate_edelweiss(seed, variant as u8),
            Species::Rowan => generate_rowan(seed, variant as u8),
        }
    })
}

/// Perform the bounded one-time model construction during explicit loading,
/// before either display jobs or canonical walking queries need these models.
pub fn prewarm_models() {
    for species in [Species::Grass, Species::Edelweiss, Species::Rowan] {
        for variant in 0..variant_count(species) {
            model_for(&PlantRecord {
                species,
                variant,
                seed: 0,
                root: Cell::default(),
                rotation: 0,
            });
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct TileCache(Mutex<VecDeque<((i64, i64, Species), Arc<Vec<PlantRecord>>)>>);
impl TileCache {
    fn tile(&self, w: &World, x: i64, z: i64, species: Species) -> Arc<Vec<PlantRecord>> {
        let key = (x, z, species);
        {
            let cache = self.0.lock().unwrap();
            if let Some((_, r)) = cache.iter().find(|(k, _)| *k == key) {
                return r.clone();
            }
        }
        let records = Arc::new(place_tile(w, x, z, species));
        let mut cache = self.0.lock().unwrap();
        if cache.len() >= 128 {
            cache.pop_front();
        }
        cache.push_back((key, records.clone()));
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
fn ground_candidate(world_seed: u64, gx: i64, gz: i64) -> (i64, i64, u64, Option<Species>) {
    let seed = hash(world_seed ^ 0x6d6561646f77, gx, gz);
    let x = gx * 8 + 2 + (seed % 4) as i64;
    let z = gz * 8 + 2 + ((seed >> 16) % 4) as i64;
    let meadow = patch_field(world_seed ^ 0x7061746368, x, z, 192);
    let tuft = patch_field(world_seed ^ 0x74756674, x, z, 48);
    let colony = patch_field(world_seed ^ 0x666c6f776572, x, z, 64);
    let bloom_probability = 0.001 + 0.30 * ((colony - 0.65) / 0.35).max(0.).powi(2);
    let roll = |salt| (mix(seed ^ salt) >> 11) as f64 / ((1u64 << 53) as f64);
    let species = if roll(0x626c6f6f6d) < bloom_probability {
        Some(Species::Edelweiss)
    } else if roll(0x6772617373) < (0.08 + 0.85 * meadow + 0.25 * (tuft - 0.5)).clamp(0.04, 0.94) {
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
    let mut records = Vec::new();
    let (x, z, seed) = tree_candidate(w.seed, tx, tz);
    if species == Species::Rowan && tree_allowed(w.seed, tx, tz) {
        if let Some(y) = tree_supported(w, x, z) {
            records.push(PlantRecord {
                species: Species::Rowan,
                seed,
                root: Cell { x, y: y - 2, z },
                variant: (seed % variant_count(Species::Rowan) as u64) as u8,
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
            let (a, b, _) = tree_candidate(w.seed, tx + dx, tz + dz);
            if tree_allowed(w.seed, tx + dx, tz + dz) && tree_supported(w, a, b).is_some() {
                trees.push((a, b));
            }
        }
    }
    for gx in 0..32 {
        for gz in 0..32 {
            let (x, z, seed, chosen) = ground_candidate(w.seed, tx * 32 + gx, tz * 32 + gz);
            if chosen != Some(species) {
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
                        let tall = patch_field(w.seed ^ 0x686569676874, x, z, 64) > 0.57;
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
    /// Collision skips all soft foliage and queries only reusable rowan wood.
    pub fn blocking_material(&self, c: Cell) -> Material {
        if let Some(&m) = self.edits.get(&c) {
            return if m.is_blocking() { m } else { Material::Air };
        }
        let terrain = self.source(c);
        if terrain != Material::Air {
            return terrain;
        }
        self.vegetation_species_voxel(c, Species::Rowan)
            .filter(|v| v.material.is_blocking())
            .map_or(Material::Air, |v| v.material)
    }
}

#[cfg(test)]
mod tests {
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
    fn model_bank_has_distinct_heights_full_crowns_and_native_grass() {
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
            grass_heights.len() >= 6,
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
        assert!((1.4..2.7).contains(&density), "grass density {density}");
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
