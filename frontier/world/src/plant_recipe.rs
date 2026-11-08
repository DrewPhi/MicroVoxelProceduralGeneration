//! Ordered growth strokes, before native-cell rasterization. This is a bounded
//! grass recipe experiment, not a tree-growth replacement or an asset format.
use super::{model, stroke, Cells, Material, PlantModel, Rng};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct GrassStroke {
    pub from: [i16; 3],
    pub to: [i16; 3],
    /// RGB8 plus stable Leaf material id in the high byte.
    pub pigment_material: u32,
}
impl GrassStroke {
    fn new(from: [i64; 3], to: [i64; 3], rgb: [u8; 3]) -> Self {
        Self {
            from: from.map(|x| i16::try_from(x).expect("bounded grass growth")),
            to: to.map(|x| i16::try_from(x).expect("bounded grass growth")),
            pigment_material: u32::from_le_bytes([rgb[0], rgb[1], rgb[2], 5]),
        }
    }
}
#[derive(Clone, Debug)]
pub struct GrassRecipe {
    pub seed: u64,
    pub strokes: Vec<GrassStroke>,
}
impl GrassRecipe {
    pub fn materialize(&self) -> PlantModel {
        let mut cells = Cells::new();
        for s in &self.strokes {
            let rgba = s.pigment_material.to_le_bytes();
            stroke(
                &mut cells,
                s.from.map(i64::from),
                s.to.map(i64::from),
                0,
                Material::Leaf,
                [rgba[0], rgba[1], rgba[2]],
            );
        }
        model(cells, self.seed)
    }
    pub fn payload_bytes(&self) -> usize {
        self.strokes.len() * std::mem::size_of::<GrassStroke>()
    }
}

pub fn grass_recipe(seed: u64, variant: u8) -> GrassRecipe {
    let mut rng = Rng(seed);
    let mut strokes = Vec::new();
    let height = [2, 3, 3, 4, 5, 6, 8, 10][variant as usize % 8];
    let base = [96u8, 128, 57];
    let fresh = [108u8, 142, 66];
    let dry = [147u8, 158, 90];
    for _ in 0..[5, 7, 9, 6, 10, 8, 7, 9][variant as usize % 8] {
        let a = rng.between(0., std::f64::consts::TAU);
        let dry_tip = rng.unit() < 0.30;
        // Rhizome-like low strokes spread the carpet; rare mature tufts rise above it.
        let lean = rng.between(2., if variant < 4 { 5. } else { 3. });
        let spread = if variant < 4 {
            rng.between(1., 3.5)
        } else {
            1.
        };
        let bx = (a.cos() * spread).round() as i64;
        let bz = (a.sin() * spread).round() as i64;
        strokes.push(GrassStroke::new([0, 0, 0], [bx, 0, bz], base));
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
            strokes.push(GrassStroke::new(p, q, color));
            p = q;
        }
    }
    GrassRecipe { seed, strokes }
}

/// Woody growth and selected ellipsoidal sprays, before any native cells exist.
/// Blossom selection still depends on the exposed canonical raster, so it is a
/// separately disclosed materialization phase rather than a GPU-growth claim.
#[derive(Clone, Copy, Debug)]
pub struct WoodyStroke {
    pub from: [i16; 3],
    pub to: [i16; 3],
    pub radius: u8,
}
impl WoodyStroke {
    pub(super) fn new(from: [i64; 3], to: [i64; 3], radius: i64) -> Self {
        Self {
            from: from.map(|x| i16::try_from(x).expect("bounded rowan wood")),
            to: to.map(|x| i16::try_from(x).expect("bounded rowan wood")),
            radius: radius as u8,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct FoliageSpray {
    pub center: [i16; 3],
    pub mass: u16,
    pub rx: f64,
    pub rz: f64,
}
#[derive(Clone, Debug)]
pub struct RowanRecipe {
    pub seed: u64,
    pub variant: u8,
    pub height: f64,
    pub wood: Vec<WoodyStroke>,
    pub foliage: Vec<FoliageSpray>,
}
pub fn rowan_recipe(seed: u64, variant: u8) -> RowanRecipe {
    super::build_rowan_recipe(seed, variant, &|| false).unwrap()
}
impl RowanRecipe {
    pub fn payload_bytes(&self) -> usize {
        self.wood.len() * std::mem::size_of::<WoodyStroke>()
            + self.foliage.len() * std::mem::size_of::<FoliageSpray>()
    }
    pub fn materialize(&self) -> PlantModel {
        self.materialize_cancellable(&|| false).unwrap()
    }
    pub(super) fn materialize_cancellable(&self, cancel: &impl Fn() -> bool) -> Option<PlantModel> {
        use super::{cache_wood_model, hash, variant_count, Cell, Species, BARK, WHITE};
        let seed = self.seed;
        let variant = self.variant;
        let h = self.height;
        let mut v = Cells::new();
        for (i, s) in self.wood.iter().enumerate() {
            if i % 128 == 0 && cancel() {
                return None;
            }
            stroke(
                &mut v,
                s.from.map(i64::from),
                s.to.map(i64::from),
                s.radius as i64,
                Material::Wood,
                BARK,
            );
        }
        for (n, s) in self.foliage.iter().enumerate() {
            if n % 128 == 0 && cancel() {
                return None;
            }
            let p = s.center.map(i64::from);
            let mass = s.mass as usize;
            let rx = s.rx;
            let rz = s.rz;
            let ry = 1.8;
            let young = (9 - mass) as f64 / 8.;
            for x in -(rx.ceil() as i64)..=rx.ceil() as i64 {
                for y in -2..=2 {
                    for z in -(rz.ceil() as i64)..=rz.ceil() as i64 {
                        if (x as f64 / rx).powi(2)
                            + (y as f64 / ry).powi(2)
                            + (z as f64 / rz).powi(2)
                            > 1.
                        {
                            continue;
                        }
                        // Branch age and position within one connected spray supply
                        // new-growth color. It is canonical pigment, not lighting
                        // or disconnected per-voxel color noise.
                        let freshness = 0.40 * young * (0.65 + 0.35 * (y as f64 / ry + 1.) * 0.5);
                        let mature = [100u8, 137, 64];
                        let new_growth = [143u8, 164, 76];
                        let pigment = std::array::from_fn(|k| {
                            (mature[k] as f64
                                + (new_growth[k] as f64 - mature[k] as f64) * freshness)
                                .round() as u8
                        });
                        v.entry(Cell {
                            x: p[0] + x,
                            y: p[1] + y,
                            z: p[2] + z,
                        })
                        .or_insert((Material::Leaf, pigment));
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
        if cancel() {
            return None;
        }
        let grown = model(v, seed);
        cache_wood_model((variant % variant_count(Species::Rowan), seed), &grown);
        Some(grown)
    }
}
#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    // Independent canonical source arithmetic frozen from 0a6ca71.
    fn original_grass(seed: u64, variant: u8) -> PlantModel {
        let mut rng = Rng(seed);
        let mut v = Cells::new();
        let height = [2, 3, 3, 4, 5, 6, 8, 10][variant as usize % 8];
        let base = [96u8, 128, 57];
        let fresh = [108u8, 142, 66];
        let dry = [147u8, 158, 90];
        for _ in 0..[5, 7, 9, 6, 10, 8, 7, 9][variant as usize % 8] {
            let a = rng.between(0., std::f64::consts::TAU);
            let dry_tip = rng.unit() < 0.30;
            // Rhizome-like low strokes spread the carpet; rare mature tufts rise above it.
            let lean = rng.between(2., if variant < 4 { 5. } else { 3. });
            let spread = if variant < 4 {
                rng.between(1., 3.5)
            } else {
                1.
            };
            let bx = (a.cos() * spread).round() as i64;
            let bz = (a.sin() * spread).round() as i64;
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

    #[test]
    fn recipe_matches_original_grass_for_120_seeds_all_variants() {
        for n in 0..120u64 {
            let seed = super::super::mix(n.wrapping_mul(0x9e3779b97f4a7c15));
            for variant in 0..8 {
                let a = grass_recipe(seed, variant).materialize();
                let b = original_grass(seed, variant);
                assert_eq!(a.min, b.min);
                assert_eq!(a.max, b.max);
                assert_eq!(a.voxels, b.voxels);
                for v in &b.voxels {
                    assert_eq!(a.voxel(v.cell), Some(*v));
                }
            }
        }
    }
    // Independent growth/raster arithmetic frozen from 0a6ca71.
    fn original_rowan(seed: u64, variant: u8, cancel: &impl Fn() -> bool) -> Option<PlantModel> {
        let mut rng = Rng(seed);
        let h = [52., 65., 76., 84., 94., 104., 112., 88.][variant as usize % 8]
            * rng.between(0.96, 1.04);
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
                && centers.iter().any(|q| {
                    (p[0] - q[0]).pow(2) + (p[1] - q[1]).pow(2) + (p[2] - q[2]).pow(2) < 13
                })
            {
                continue;
            }
            centers.push(p);
            let ry = 1.8;
            let young = (9 - mass) as f64 / 8.;
            for x in -(rx.ceil() as i64)..=rx.ceil() as i64 {
                for y in -2..=2 {
                    for z in -(rz.ceil() as i64)..=rz.ceil() as i64 {
                        if (x as f64 / rx).powi(2)
                            + (y as f64 / ry).powi(2)
                            + (z as f64 / rz).powi(2)
                            > 1.
                        {
                            continue;
                        }
                        // Branch age and position within one connected spray supply
                        // new-growth color. It is canonical pigment, not lighting
                        // or disconnected per-voxel color noise.
                        let freshness = 0.40 * young * (0.65 + 0.35 * (y as f64 / ry + 1.) * 0.5);
                        let mature = [100u8, 137, 64];
                        let new_growth = [143u8, 164, 76];
                        let pigment = std::array::from_fn(|k| {
                            (mature[k] as f64
                                + (new_growth[k] as f64 - mature[k] as f64) * freshness)
                                .round() as u8
                        });
                        v.entry(Cell {
                            x: p[0] + x,
                            y: p[1] + y,
                            z: p[2] + z,
                        })
                        .or_insert((Material::Leaf, pigment));
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
        if cancel() {
            return None;
        }
        let grown = model(v, seed);
        cache_wood_model((variant % variant_count(Species::Rowan), seed), &grown);
        Some(grown)
    }

    #[test]
    fn rowan_recipe_matches_original_growth_for_120_seeds() {
        for n in 0..120u64 {
            let seed = super::super::mix(n.wrapping_mul(0x9e3779b97f4a7c15));
            let variant = (n % 8) as u8;
            let recipe = rowan_recipe(seed, variant);
            let a = recipe.materialize();
            let b = original_rowan(seed, variant, &|| false).unwrap();
            assert_eq!(a.min, b.min);
            assert_eq!(a.max, b.max);
            assert_eq!(a.voxels, b.voxels);
            for v in &b.voxels {
                assert_eq!(a.voxel(v.cell), Some(*v));
            }
        }
    }
}
