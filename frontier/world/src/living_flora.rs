//! Seeded growth for Eroded habitats. Coordinates and pigments describe
//! canonical plant cells. The renderer also reads the woody growth recipe.
use super::{
    model, model_from_connected_cells, pigmented_voxel, plant_recipe::WoodyStroke, put, stroke,
    Cells, Material, PlantModel, PlantVoxel, Rng, Species,
};
use crate::Cell;
use std::f64::consts::TAU;

pub const CONIFER: u8 = 128;
pub const SHRUB: u8 = 136;
pub const ASPEN: u8 = 144;
pub const ALDER: u8 = 148;
pub const SAPLING: u8 = 152;
pub const GRASS: u8 = 128;
pub const FERN: u8 = 136;
pub const MOSS: u8 = 140;
pub const TUSSOCK: u8 = 144;
pub const CUSHION: u8 = 148;
pub const RUSH: u8 = 152;
pub const FLOWER: u8 = 128;
pub const WOODY_HEIGHT: i64 = 432;
pub const GROUND_HEIGHT: i64 = 24;

pub(super) fn uses_growth_recipe(species: Species, variant: u8) -> bool {
    match species {
        Species::Rowan => (CONIFER..SAPLING + 4).contains(&variant),
        Species::Grass => (GRASS..RUSH + 4).contains(&variant),
        Species::Edelweiss => (FLOWER..FLOWER + 16).contains(&variant),
    }
}

#[derive(Clone, Copy)]
pub struct LeafCluster {
    pub center: [i16; 3],
    pub radius: [f64; 3],
    pub pigment: [u8; 3],
}

pub struct WoodlandRecipe {
    pub wood: Vec<WoodyStroke>,
    pub foliage: Vec<LeafCluster>,
    pub bark: [u8; 3],
    pub trunk_strokes: usize,
    bark_pattern: BarkPattern,
    bloom: Option<[u8; 3]>,
}
#[derive(Clone, Copy)]
enum BarkPattern {
    Furrows,
    Aspen,
}
pub const fn is_conifer(variant: u8) -> bool {
    (variant >= CONIFER && variant < SHRUB) || is_sapling(variant)
}
pub const fn is_shrub(variant: u8) -> bool {
    variant >= SHRUB && variant < ASPEN
}
pub const fn is_sapling(variant: u8) -> bool {
    variant >= SAPLING && variant < SAPLING + 4
}

/// Shared distance templates reuse branch structure but preserve each
/// instance's seed-dependent height within the same maturity cohort.
pub fn shared_crown_height_scale(seed: u64, template_seed: u64, variant: u8) -> f32 {
    let range = if is_conifer(variant) {
        [0.88, 1.10]
    } else if (ASPEN..ALDER + 4).contains(&variant) {
        [0.87, 1.12]
    } else {
        return 1.;
    };
    let height = |seed| Rng(seed).between(range[0], range[1]);
    (height(seed) / height(template_seed)) as f32
}

fn cluster(center: [i64; 3], radius: [f64; 3], pigment: [u8; 3]) -> LeafCluster {
    LeafCluster {
        center: center.map(|v| v as i16),
        radius,
        pigment,
    }
}

// Traverse successive voxel face crossings in order. Separate simultaneous
// crossings into one axial step at a time so every branch stays connected.
fn branch_path(from: [i64; 3], to: [i64; 3]) -> impl Iterator<Item = [i64; 3]> {
    let length: [i64; 3] = std::array::from_fn(|i| (to[i] - from[i]).abs());
    let sign: [i64; 3] = std::array::from_fn(|i| (to[i] - from[i]).signum());
    let mut steps = [0; 3];
    let mut p = from;
    std::iter::once(from).chain(std::iter::from_fn(move || {
        let mut axis = None::<usize>;
        for i in 0..3 {
            if steps[i] < length[i]
                && axis
                    .is_none_or(|j| (2 * steps[i] + 1) * length[j] < (2 * steps[j] + 1) * length[i])
            {
                axis = Some(i);
            }
        }
        let axis = axis?;
        steps[axis] += 1;
        p[axis] += sign[axis];
        Some(p)
    }))
}

fn on_branch(from: [i64; 3], to: [i64; 3], t: f64) -> [i64; 3] {
    let length: i64 = (0..3).map(|i| (to[i] - from[i]).abs()).sum();
    branch_path(from, to)
        .nth((t * length as f64).round() as usize)
        .unwrap()
}

fn radial(p: [i64; 3], angle: f64, length: f64, rise: f64) -> [i64; 3] {
    [
        p[0] + (angle.cos() * length).round() as i64,
        (p[1] + rise.round() as i64).max(0),
        p[2] + (angle.sin() * length).round() as i64,
    ]
}

pub fn woody_recipe(seed: u64, variant: u8) -> Option<WoodlandRecipe> {
    if is_sapling(variant) {
        Some(conifer(seed, variant - SAPLING, true))
    } else if is_conifer(variant) {
        Some(conifer(seed, variant - CONIFER, false))
    } else if is_shrub(variant) {
        Some(shrub(seed, variant - SHRUB))
    } else if (ASPEN..ALDER + 4).contains(&variant) {
        Some(broadleaf(seed, variant - ASPEN))
    } else {
        None
    }
}

fn conifer(seed: u64, maturity: u8, young: bool) -> WoodlandRecipe {
    let mut rng = Rng(seed);
    let spruce = maturity < 4;
    let heights = if young {
        [70., 106., 133., 157.]
    } else {
        [189., 245., 302., 373.]
    };
    let h = heights[usize::from(maturity % 4)] * rng.between(0.88, 1.10);
    let crown_radius = h * rng.between(0.125, 0.161) * if young { 1.34 } else { 1. };
    let phase = rng.unit() * TAU;
    let shelter_heading = phase + f64::from((seed >> 34) as u16) / 65535. * TAU;
    let lean = rng.between(2., 13.);
    let center = |t: f64| radial([0, (t * h).round() as i64, 0], phase, lean * t * t, 0.);
    let mut wood = Vec::new();
    let trunk_radius = (0.028 * h).round().min(9.) as i64;
    for i in 0..16 {
        let t = f64::from(i) / 16.;
        let radius = (trunk_radius as f64 * (1. - t).powf(1.45)).round() as i64;
        wood.push(WoodyStroke::new(center(t), center(t + 1. / 16.), radius));
    }
    let trunk_at = |y: i64| {
        let i = ((y as f64 / h) * 16.).floor() as usize;
        // The boundaries were rounded to native cells when the trunk
        // segments were built. A neighboring segment can own a rounded edge.
        let a = center(i.min(15) as f64 / 16.);
        let b = center((i + 1).min(16) as f64 / 16.);
        let y = y.clamp(a[1], b[1]);
        branch_path(a, b).find(|p| p[1] == y).unwrap()
    };
    for k in 0..4 {
        wood.push(WoodyStroke::new(
            trunk_at(6),
            radial(
                [0; 3],
                phase + f64::from(k) * TAU / 4.,
                (trunk_radius * 2) as f64,
                0.,
            ),
            2,
        ));
    }
    let mut foliage = Vec::new();
    let start = if young {
        rng.between(0.10, 0.22)
    } else if spruce {
        rng.between(0.17, 0.32)
    } else {
        rng.between(0.37, 0.54)
    };
    let levels = (h / if young { 12. } else { 16. }).round() as u32;
    let mut heading = phase;
    for level in 0..levels {
        let t = start
            + (0.984 - start) * (f64::from(level) + rng.between(0.10, 0.87)) / f64::from(levels);
        let fraction = ((t - start) / (1. - start)).clamp(0., 1.);
        let profile = if spruce {
            (1. - fraction).powf(0.94) * (0.72 + 0.28 * (fraction * 7.).min(1.))
        } else {
            (0.59 + 0.41 * (fraction * 2.5).min(1.)) * (1. - fraction).powf(0.78)
        };
        let origin = trunk_at((t * h).round() as i64);
        let count = if profile > 0.40 { 5 } else { 4 };
        heading += 2.399_963 + rng.between(-0.22, 0.22);
        for k in 0..count {
            let angle = heading + f64::from(k) * TAU / f64::from(count) + rng.between(-0.16, 0.16);
            let aspect = (angle - shelter_heading).cos();
            let competition = 0.925 + 0.075 * aspect;
            let length = (competition * crown_radius * profile * rng.between(0.68, 1.22)).max(4.);
            let droop = if spruce { -0.20 } else { 0.05 };
            let middle = radial(origin, angle, length * 0.58, length * droop);
            let tip = radial(
                origin,
                angle + rng.between(-0.1, 0.1),
                length,
                length * (droop + 0.10),
            );
            let thickness = if length > 30. { 2 } else { 1 };
            wood.push(WoodyStroke::new(origin, middle, thickness));
            wood.push(WoodyStroke::new(middle, tip, 0));
            // Side shoots flatten into fans. A little lift and seed-dependent
            // gaps keep the tiers legible beneath the canopy.
            for side in [-1., 1.] {
                let fork = on_branch(origin, middle, rng.between(0.56, 0.87));
                let shoot = radial(
                    fork,
                    angle + side * rng.between(0.39, 0.71),
                    length * 0.38,
                    length * -0.04,
                );
                wood.push(WoodyStroke::new(fork, shoot, 0));
                add_needles(&mut foliage, fork, shoot, spruce, &mut rng);
            }
            add_needles(&mut foliage, middle, tip, spruce, &mut rng);
        }
    }
    foliage.push(cluster(
        trunk_at((0.979 * h).round() as i64),
        [3.4, 7.5, 3.4],
        [89, 124, 72],
    ));
    WoodlandRecipe {
        wood,
        foliage,
        bark: if spruce { [104, 81, 62] } else { [127, 96, 68] },
        trunk_strokes: 16,
        bark_pattern: BarkPattern::Furrows,
        bloom: None,
    }
}

fn add_needles(
    foliage: &mut Vec<LeafCluster>,
    from: [i64; 3],
    to: [i64; 3],
    spruce: bool,
    rng: &mut Rng,
) {
    for t in [0.30, 0.63, 0.96] {
        let radius = rng.between(2.7, 5.4) * (1.18 - 0.32 * t);
        let young = rng.between(0., 1.) * t;
        let color = if spruce {
            [58. + 28. * young, 91. + 32. * young, 64. + 12. * young]
        } else {
            [73. + 31. * young, 104. + 34. * young, 57. + 14. * young]
        };
        foliage.push(cluster(
            on_branch(from, to, t),
            [
                radius,
                radius * rng.between(0.46, 0.80),
                radius * rng.between(0.72, 1.11),
            ],
            color.map(|c| c.round() as u8),
        ));
    }
}

fn broadleaf(seed: u64, variant: u8) -> WoodlandRecipe {
    let mut rng = Rng(seed);
    let alder = variant >= ALDER - ASPEN;
    let age = usize::from(variant % 4);
    let heights = if alder {
        [122., 168., 205., 247.]
    } else {
        [153., 210., 252., 298.]
    };
    let height = heights[age] * rng.between(0.87, 1.12);
    let phase = rng.unit() * TAU;
    let lean = rng.between(4., 17.);
    let width = (height
        * if alder {
            rng.between(0.21, 0.29)
        } else {
            rng.between(0.12, 0.18)
        })
    .min(58.);
    let tint = rng.between(0.0, 1.0);
    let leaves = if alder {
        [70. + 19. * tint, 103. + 24. * tint, 65. + 12. * tint]
    } else {
        [88. + 28. * tint, 121. + 26. * tint, 65. + 17. * tint]
    };
    let center = |t: f64| radial([0, (height * t).round() as i64, 0], phase, lean * t * t, 0.);
    let mut wood = Vec::new();
    let mut foliage = Vec::new();
    for band in 0..16 {
        let t = f64::from(band) / 16.;
        let radius = (height * 0.024 * (1. - t).powf(1.42)).round() as i64;
        wood.push(WoodyStroke::new(center(t), center(t + 1. / 16.), radius));
    }
    // Riparian alder commonly sends up several leaders from one rootstock.
    // Vary their reach and relative height independently of the main crown.
    let mut forks = Rng(seed ^ 0x616c6465725f666f);
    if alder && age > 0 && forks.unit() < 0.83 {
        let count = 1 + usize::from(forks.unit() > 0.64);
        for i in 0..count {
            let direction = phase + i as f64 * std::f64::consts::PI + forks.between(-0.62, 0.62);
            let root = center(0.0625);
            let elbow = radial(
                root,
                direction,
                width * forks.between(0.18, 0.28),
                height * 0.25,
            );
            let tip = radial(
                elbow,
                direction,
                width * forks.between(0.21, 0.39),
                height * forks.between(0.24, 0.41),
            );
            wood.push(WoodyStroke::new(root, elbow, 3));
            wood.push(WoodyStroke::new(elbow, tip, 2));
            for t in [0.41, 0.60, 0.83, 1.] {
                let fork = on_branch(elbow, tip, t);
                let r = (4.3 + 1.3 * age as f64) * forks.between(0.74, 1.27);
                foliage.push(cluster(
                    fork,
                    [r, 0.80 * r, 1.10 * r],
                    leaves.map(|c| (c * (0.84 + 0.13 * t)) as u8),
                ));
            }
        }
    }
    let trunk_strokes = wood.len();
    let branches = if alder { 9 } else { 13 };
    for limb in 0..branches {
        let fraction = f64::from(limb) / f64::from(branches);
        let t = if alder {
            0.13 + 0.60 * fraction
        } else {
            0.39 + 0.53 * fraction
        };
        let angle = phase + f64::from(limb) * 2.399_963 + rng.between(-0.32, 0.32);
        // Start at an actual trunk joint so climbing crowns retain rooted
        // woody paths even after cropping.
        let root = center((t * 16.).round() / 16.);
        let reach = width * (0.82 + 0.16 * (TAU * fraction).sin()) * (1. - 0.68 * fraction);
        let lift = (height * ((if alder { 0.62 } else { 0.31 }) - 0.17 * fraction))
            .min(height - root[1] as f64);
        let elbow = radial(root, angle, reach * 0.52, lift * 0.51);
        let tip = radial(
            elbow,
            angle + rng.between(-0.30, 0.3),
            reach * 0.48,
            lift * 0.49,
        );
        wood.push(WoodyStroke::new(root, elbow, if limb < 4 { 2 } else { 1 }));
        wood.push(WoodyStroke::new(elbow, tip, 1));
        for side in [-1., 1.] {
            let bud = on_branch(elbow, tip, rng.between(0.31, 0.76));
            let spray = radial(
                bud,
                angle + side * rng.between(0.65, 1.25),
                (reach * 0.30).clamp(3., 13.),
                rng.between(1., 9.),
            );
            wood.push(WoodyStroke::new(bud, spray, 0));
            for t in [0.32, 0.74, 1.] {
                let r = (4.5 + 1.2 * age as f64) * rng.between(0.74, 1.38);
                let light = rng.between(0.84, 1.15);
                foliage.push(cluster(
                    on_branch(bud, spray, t),
                    [r, r * 0.75, r * 0.93],
                    leaves.map(|c| (c * light).round() as u8),
                ));
            }
        }
        let r = (7. + 1.1 * age as f64) * rng.between(0.80, 1.24);
        foliage.push(cluster(tip, [r, r * 0.86, r], leaves.map(|c| c as u8)));
    }
    foliage.push(cluster(
        center(1.),
        [6.4, 8.2, 7.],
        leaves.map(|c| (c + 5.) as u8),
    ));
    WoodlandRecipe {
        wood,
        foliage,
        bark: if alder {
            [113, 99, 75]
        } else {
            [151, 150, 135]
        },
        trunk_strokes,
        bark_pattern: if alder {
            BarkPattern::Furrows
        } else {
            BarkPattern::Aspen
        },
        bloom: None,
    }
}

fn shrub(seed: u64, variant: u8) -> WoodlandRecipe {
    let mut rng = Rng(seed);
    let habit = variant / 2;
    let height = [20., 32., 18., 25.][habit as usize] * rng.between(0.82, 1.24);
    let width = height * rng.between(0.56, 0.82);
    let phase = rng.unit() * TAU;
    let leaves = [
        [74, 111, 64],
        [103, 136, 57],
        [118, 141, 103],
        [82, 109, 70],
    ][habit as usize];
    let bloom = [
        Some([213, 158, 176]),
        None,
        Some([237, 224, 176]),
        Some([173, 60, 68]),
    ][habit as usize];
    let mut wood = Vec::new();
    let mut foliage = Vec::new();
    let count = (rng.unit() * 4.).round() as u32 + 6;
    for k in 0..count {
        let angle = phase + f64::from(k) * 2.399_963;
        let bend = radial(
            [0; 3],
            angle,
            width * rng.between(0.15, 0.38),
            height * rng.between(0.25, 0.53),
        );
        let tip = radial(
            bend,
            angle + rng.between(-0.35, 0.35),
            width * rng.between(0.30, 0.70),
            height * rng.between(0.22, 0.45),
        );
        wood.push(WoodyStroke::new([0; 3], bend, 1));
        wood.push(WoodyStroke::new(bend, tip, 0));
        for t in [0.22, 0.48, 0.77, 1.] {
            let center = on_branch(bend, tip, t);
            let r = rng.between(3.3, 6.6);
            let tint = rng.between(0.88, 1.12);
            foliage.push(cluster(
                center,
                [r, r * 0.76, r * 1.11],
                leaves.map(|c| (f64::from(c) * tint) as u8),
            ));
        }
    }
    WoodlandRecipe {
        wood,
        foliage,
        bark: [107, 86, 67],
        trunk_strokes: 0,
        bark_pattern: BarkPattern::Furrows,
        bloom,
    }
}

impl WoodlandRecipe {
    fn voxel_pigment(
        &self,
        seed: u64,
        cell: Cell,
        material: Material,
        pigment: [u8; 3],
    ) -> [u8; 3] {
        if material != Material::Wood {
            return pigment;
        }
        // Local coordinates keep bark scale and pattern anchored to the
        // branch through rotation, streaming and cropped native assembly.
        let strength = match self.bark_pattern {
            BarkPattern::Furrows => {
                let seam = super::hash(seed ^ 0x667572726f77, cell.x, cell.z);
                let fiber = super::hash(
                    seed ^ 0x6669626572,
                    cell.x - cell.z,
                    (cell.y + ((seam >> 9) & 7) as i64).div_euclid(14),
                );
                if seam & 7 < 2 {
                    82 + (fiber % 10) as u16
                } else {
                    96 + (fiber % 9) as u16
                }
            }
            BarkPattern::Aspen => {
                let scale = super::hash(
                    seed ^ 0x6c656e746963656c,
                    cell.x.div_euclid(4) + 97 * cell.z.div_euclid(4),
                    cell.y.div_euclid(2),
                );
                if scale % 19 == 0 {
                    58
                } else if scale % 7 == 0 {
                    86
                } else {
                    98 + (scale % 5) as u16
                }
            }
        };
        pigment.map(|value| (u16::from(value) * strength / 100) as u8)
    }

    /// Read the same rooted branch cells used by collision and the full model,
    /// without allocating any of the foliage or pigment index.
    pub fn wood_cells(&self, branches: bool, cancel: &impl Fn() -> bool) -> Option<Vec<Cell>> {
        let mut wood = Cells::new();
        let segments = if branches {
            self.wood.len()
        } else {
            self.trunk_strokes
        };
        self.rasterize_wood_region(&mut wood, [i64::MIN; 3], [i64::MAX; 3], segments, cancel)
            .then(|| wood.into_keys().collect())
    }

    pub(super) fn rasterize_wood(&self, cells: &mut Cells) {
        self.rasterize_wood_region(
            cells,
            [i64::MIN; 3],
            [i64::MAX; 3],
            self.wood.len(),
            &|| false,
        );
    }

    pub(super) fn materialize(self, seed: u64, cancel: &impl Fn() -> bool) -> Option<PlantModel> {
        let mut cells = Cells::new();
        if !self.rasterize_wood_region(
            &mut cells,
            [i64::MIN; 3],
            [i64::MAX; 3],
            self.wood.len(),
            cancel,
        ) || !self.rasterize_leaves(
            &mut cells,
            seed,
            [i64::MIN, 0, i64::MIN],
            [i64::MAX; 3],
            cancel,
        ) {
            return None;
        }
        // The growth paths anchor each limb and its leaf clusters to the
        // stem. Retain the independent component walk in tests.
        #[cfg(test)]
        assert!(super::rooted_component_reference(&cells)
            .into_iter()
            .all(|v| v));
        let painted = cells.into_iter().map(|(cell, (material, rgb))| {
            (
                cell,
                (material, self.voxel_pigment(seed, cell, material, rgb)),
            )
        });
        Some(model_from_connected_cells(painted, seed))
    }

    pub(super) fn materialize_clipped(
        self,
        seed: u64,
        min: [i64; 3],
        max: [i64; 3],
        cancel: &impl Fn() -> bool,
    ) -> Option<Vec<PlantVoxel>> {
        let mut cells = Cells::new();
        // Each path anchors on an earlier path to the root. Every ellipsoid
        // includes its center on one path and is face-connected to that center.
        // Clipping therefore cannot change which cells survive the canonical
        // rooted-component filter, even when a joining branch is out of bounds.
        if !self.rasterize_wood_region(&mut cells, min, max, self.wood.len(), cancel)
            || !self.rasterize_leaves(&mut cells, seed, min, max, cancel)
        {
            return None;
        }
        Some(
            cells
                .into_iter()
                .map(|(cell, (material, rgb))| {
                    pigmented_voxel(
                        cell,
                        material,
                        self.voxel_pigment(seed, cell, material, rgb),
                        seed,
                    )
                })
                .collect(),
        )
    }

    fn rasterize_wood_region(
        &self,
        cells: &mut Cells,
        min: [i64; 3],
        max: [i64; 3],
        segments: usize,
        cancel: &impl Fn() -> bool,
    ) -> bool {
        for (index, segment) in self.wood.iter().take(segments).enumerate() {
            if index % 24 == 0 && cancel() {
                return false;
            }
            let a = segment.from.map(i64::from);
            let b = segment.to.map(i64::from);
            let r = i64::from(segment.radius);
            let pad = [r, 0, r];
            if (0..3).any(|i| a[i].max(b[i]) + pad[i] < min[i] || a[i].min(b[i]) - pad[i] >= max[i])
            {
                continue;
            }
            for [x, y, z] in branch_path(a, b) {
                if !(min[1]..max[1]).contains(&y) {
                    continue;
                }
                let xs = (-r).max(min[0].saturating_sub(x))..(r + 1).min(max[0].saturating_sub(x));
                let zs = (-r).max(min[2].saturating_sub(z))..(r + 1).min(max[2].saturating_sub(z));
                for dx in xs {
                    for dz in zs.clone() {
                        if dx * dx + dz * dz <= r * r {
                            put(cells, [x + dx, y, z + dz], Material::Wood, self.bark);
                        }
                    }
                }
            }
        }
        !cancel()
    }

    fn rasterize_leaves(
        &self,
        cells: &mut Cells,
        seed: u64,
        min: [i64; 3],
        max: [i64; 3],
        cancel: &impl Fn() -> bool,
    ) -> bool {
        for (index, leaves) in self.foliage.iter().enumerate() {
            if index % 24 == 0 && cancel() {
                return false;
            }
            let center = leaves.center.map(i64::from);
            let radius = leaves.radius.map(|v| v.ceil() as i64);
            let [xs, ys, zs] = std::array::from_fn(|axis| {
                (-radius[axis]).max(min[axis].saturating_sub(center[axis]))
                    ..(radius[axis] + 1).min(max[axis].saturating_sub(center[axis]))
            });
            let [rx, ry, rz] = leaves.radius;
            for x in xs {
                let x2 = (x as f64 / rx).powi(2);
                for y in ys.clone() {
                    let xy = x2 + (y as f64 / ry).powi(2);
                    if xy > 1. {
                        continue;
                    }
                    for z in zs.clone() {
                        let d = xy + (z as f64 / rz).powi(2);
                        if d > 1. || center[1] + y < 0 {
                            continue;
                        }
                        let cell = Cell {
                            x: center[0] + x,
                            y: center[1] + y,
                            z: center[2] + z,
                        };
                        let blossom = self.bloom.filter(|_| {
                            d > 0.77
                                && y > 0
                                && super::hash(seed, cell.x ^ cell.y.rotate_left(7), cell.z) & 63
                                    == 0
                        });
                        cells.entry(cell).or_insert_with(|| {
                            blossom.map_or((Material::Leaf, leaves.pigment), |rgb| {
                                (Material::Flower, rgb)
                            })
                        });
                    }
                }
            }
        }
        !cancel()
    }
}

pub(super) fn grass(seed: u64, variant: u8) -> PlantModel {
    if variant >= RUSH {
        return wet_rush(seed, variant - RUSH);
    }
    if variant >= CUSHION {
        return alpine_cushion(seed, variant - CUSHION);
    }
    if variant >= TUSSOCK {
        return dry_tussock(seed, variant - TUSSOCK);
    }
    if variant >= MOSS {
        return moss(seed, variant - MOSS);
    }
    if variant >= FERN {
        return fern(seed, variant - FERN);
    }
    let mut rng = Rng(seed);
    let mut cells = Cells::new();
    let form = variant - GRASS;
    let profile = [3., 4.5, 6.5, 8., 10., 12., 15., 17.][form as usize];
    let base = if form < 4 {
        [66u8, 98, 42]
    } else {
        [81, 112, 52]
    };
    let tip = if form >= 6 {
        [151u8, 150, 85]
    } else {
        [127, 156, 73]
    };
    let rhythm = rng.unit() * TAU;
    for i in 0..(5 + form % 3) {
        let angle = rhythm + f64::from(i) * 2.399_963 + rng.between(-0.3, 0.3);
        let h = (profile * rng.between(0.56, 1.16)).round() as i64;
        let root = radial([0; 3], angle, rng.between(0., 2.2), 0.);
        stroke(&mut cells, [0; 3], root, 0, Material::Leaf, base);
        let lean = rng.between(1.6, 5.1);
        let mut last = root;
        for y in 1..=h {
            let t = y as f64 / h as f64;
            let next = radial(root, angle, lean * t.powf(1.75), y as f64);
            let pigment = std::array::from_fn(|k| {
                (f64::from(base[k]) * (1. - t) + f64::from(tip[k]) * t).round() as u8
            });
            stroke(&mut cells, last, next, 0, Material::Leaf, pigment);
            last = next;
        }
        if form >= 5 && i % 3 == 0 {
            let seed_head = [160, 129, 71];
            stroke(
                &mut cells,
                last,
                radial(last, angle, 1., 2.),
                0,
                Material::Leaf,
                seed_head,
            );
        }
    }
    model(cells, seed)
}

fn dry_tussock(seed: u64, age: u8) -> PlantModel {
    let mut rng = Rng(seed);
    let mut cells = Cells::new();
    let phase = rng.unit() * TAU;
    let roots = [77, 90, 47];
    let leaves = [
        [132, 133, 69],
        [153, 142, 83],
        [166, 142, 81],
        [147, 125, 72],
    ][age as usize];
    for blade in 0..(6 + age) {
        let angle = phase + f64::from(blade) * 2.399_963 + rng.between(-0.22, 0.22);
        let length = rng.between(2., 4.8) + f64::from(age) * 0.22;
        let height = rng.between(3., 5.8) + f64::from(age) * 0.89;
        let middle = radial([0; 3], angle, length * 0.33, height * 0.72);
        let tip = radial([0; 3], angle, length, height);
        for p in branch_path([0; 3], middle) {
            put(&mut cells, p, Material::Leaf, roots);
        }
        for p in branch_path(middle, tip) {
            put(&mut cells, p, Material::Leaf, leaves);
        }
        if blade % 4 == 0 && age > 1 {
            put(
                &mut cells,
                [tip[0], tip[1] + 1, tip[2]],
                Material::Leaf,
                [194, 158, 93],
            );
        }
    }
    model(cells, seed)
}

fn alpine_cushion(seed: u64, age: u8) -> PlantModel {
    let mut rng = Rng(seed);
    let mut cells = Cells::new();
    let phase = rng.unit() * TAU;
    let basal = [[76, 100, 69], [80, 113, 90], [108, 128, 77], [91, 117, 71]][age as usize];
    for stem in 0..(5 + age) {
        let angle = phase + f64::from(stem) * 2.399_963;
        let middle = radial([0; 3], angle, rng.between(1., 2.3), 1.);
        let tip = radial(
            [0; 3],
            angle,
            rng.between(3., 4.6) + f64::from(age) * 0.4,
            1.5,
        );
        for p in branch_path([0; 3], middle).chain(branch_path(middle, tip)) {
            put(&mut cells, p, Material::Leaf, basal);
            if rng.unit() > 0.69 {
                put(
                    &mut cells,
                    [p[0], p[1] + 1, p[2]],
                    Material::Leaf,
                    basal.map(|v| v + 14),
                );
            }
        }
    }
    model(cells, seed)
}

fn wet_rush(seed: u64, age: u8) -> PlantModel {
    let mut rng = Rng(seed);
    let mut cells = Cells::new();
    let phase = rng.unit() * TAU;
    let base = [55, 90, 48];
    let tips = [97, 130, 72];
    for stem in 0..6 {
        let angle = phase + f64::from(stem) * 2.399_963;
        let reach = rng.between(1.3, 5.9);
        let height = rng.between(6.5, 13.8) + f64::from(age) * 1.55;
        let knee = radial([0; 3], angle, reach * 0.25, height * 0.56);
        let tip = radial([0; 3], angle, reach, height);
        for p in branch_path([0; 3], knee) {
            put(&mut cells, p, Material::Leaf, base);
        }
        for p in branch_path(knee, tip) {
            put(&mut cells, p, Material::Leaf, tips);
        }
        if stem % 2 == 0 {
            for offset in 1..=2 {
                put(
                    &mut cells,
                    [tip[0], tip[1] + offset, tip[2]],
                    Material::Leaf,
                    [116, 84, 52],
                );
            }
        }
    }
    model(cells, seed)
}

fn moss(seed: u64, age: u8) -> PlantModel {
    let mut rng = Rng(seed);
    let mut cells = Cells::new();
    let angle = rng.unit() * TAU;
    let green = [[80, 95, 51], [85, 107, 57], [72, 101, 69], [92, 113, 65]][age as usize];
    for branch in 0..6 {
        let a = angle + f64::from(branch) * 2.399_963;
        let tip = radial([0; 3], a, rng.between(2.2, 5.9), 0.);
        for p in branch_path([0; 3], tip) {
            let tint = rng.between(0.83, 1.11);
            let paint = green.map(|v| (f64::from(v) * tint) as u8);
            put(&mut cells, p, Material::Leaf, paint);
            if rng.unit() > 0.63 {
                put(&mut cells, [p[0], p[1] + 1, p[2]], Material::Leaf, paint);
            }
        }
    }
    model(cells, seed)
}

fn fern(seed: u64, age: u8) -> PlantModel {
    let mut rng = Rng(seed);
    let mut cells = Cells::new();
    let phase = rng.unit() * TAU;
    let base = [63, 105, 63];
    let tips = [110, 150, 80];
    for spoke in 0..(5 + age / 2) {
        let angle = phase + f64::from(spoke) * 2.399_963 + rng.between(-0.19, 0.19);
        let reach = rng.between(3.7, 5.5) + f64::from(age) * 0.24;
        let lift = rng.between(7.5, 11.5) + f64::from(age);
        let mut previous = [0; 3];
        for i in 1..=8 {
            let t = f64::from(i) / 8.;
            let p = radial(
                [0; 3],
                angle,
                reach * t.powf(1.8),
                lift * (1.53 * t - 0.63 * t * t),
            );
            for cell in branch_path(previous, p) {
                put(&mut cells, cell, Material::Leaf, base);
            }
            previous = p;
            if (2..=7).contains(&i) {
                for side in [-1., 1.] {
                    let reach = (1.1 + 2.45 * (std::f64::consts::PI * t).sin()) * (1. - 0.35 * t);
                    let mut leaflet = radial(p, angle + side * 1.46, reach, 0.45);
                    // Ground-cover extraction owns a fixed eight-cell halo.
                    leaflet[0] = leaflet[0].clamp(-8, 8);
                    leaflet[2] = leaflet[2].clamp(-8, 8);
                    for cell in branch_path(p, leaflet) {
                        put(&mut cells, cell, Material::Leaf, tips);
                    }
                }
            }
        }
    }
    model(cells, seed)
}

pub(super) fn flower(seed: u64, variant: u8) -> PlantModel {
    let mut rng = Rng(seed);
    let mut cells = Cells::new();
    let form = (variant - FLOWER) / 4;
    let color = [
        [226, 183, 74],
        [180, 106, 183],
        [222, 99, 75],
        [232, 229, 211],
    ][form as usize];
    let light = [
        [249, 218, 119],
        [216, 150, 212],
        [249, 144, 99],
        [249, 247, 231],
    ][form as usize];
    let green = [75, 113, 59];
    let phase = rng.unit() * TAU;
    let heads = if form == 1 || form == 2 {
        1
    } else {
        1 + variant % 3
    };
    for k in 0..heads {
        let a = phase + f64::from(k) * 2.399_963;
        let height = rng.between(5., if form == 1 { 14. } else { 10. });
        let stem = radial([0; 3], a, if k == 0 { 1. } else { 3. }, height);
        for cell in branch_path([0; 3], stem) {
            put(&mut cells, cell, Material::Leaf, green);
        }
        for t in [0.21, 0.43] {
            let joint = on_branch([0; 3], stem, t);
            stroke(
                &mut cells,
                joint,
                radial(joint, a + t * TAU, 2., 1.),
                0,
                Material::Leaf,
                green,
            );
        }
        if form == 1 {
            // The flowering spike opens from the broad lower rings upward.
            for level in 0..5 {
                let center = [stem[0], stem[1] + level, stem[2]];
                put(&mut cells, center, Material::Flower, color);
                let petals = if level < 3 { 3 } else { 2 };
                for k in 0..petals {
                    let angle = a + k as f64 * TAU / petals as f64 + level as f64 * 0.8;
                    stroke(
                        &mut cells,
                        center,
                        radial(center, angle, if level < 2 { 2. } else { 1. }, 0.),
                        0,
                        Material::Flower,
                        light,
                    );
                }
            }
        } else {
            for k in 0..if form == 2 { 5 } else { 7 } {
                let angle = a + f64::from(k) * TAU / if form == 2 { 5. } else { 7. };
                let radius = if form == 2 { 3.0 } else { 2.2 };
                let petal = radial(stem, angle, radius, if form == 2 { 1. } else { 0. });
                stroke(&mut cells, stem, petal, 0, Material::Flower, light);
                let flare = radial(petal, angle + 1.35, 1., 0.);
                put(&mut cells, flare, Material::Flower, color);
            }
            put(
                &mut cells,
                stem,
                Material::Flower,
                if form == 2 {
                    [67, 45, 38]
                } else {
                    [202, 149, 58]
                },
            );
        }
    }
    model(cells, seed)
}
