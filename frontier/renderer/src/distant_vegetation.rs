//! Cached display-only Rowan proxies. Same seeded growth strokes/sprays as the
//! native tree, sampled directly on a coarse grid; never allocate native leaves.
//! Greenfield implementation inspired by multiscale forest representations
//! (Bruneton & Neyret 2012), not a port of their precomputed impostor renderer.
use super::{mesh_cells, VegetationMesh, VegetationVertex};
use frontier_world::{
    vegetation::{living_flora, plant_recipe},
    Cell, Material,
};
use std::collections::BTreeSet;

// Match canonical PlantModel's sRGB pigment -> linear radiance convention.
fn linear_rgb(rgb: [u8; 3]) -> [f32; 3] {
    rgb.map(|v| {
        let c = f32::from(v) / 255.;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    })
}

use crate::woodland_volume::{render_option, CrownDisplay};

pub(super) fn distant_tree_radius(default: f64) -> f64 {
    std::env::var("FRONTIER_VEGETATION_FAR_RADIUS_M")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(default)
        .clamp(220., 1536.)
}

#[derive(Clone, Copy, Default)]
pub(super) struct WoodlandPresentation {
    pub(super) crowns: CrownDisplay,
    sampled: bool,
    split_wood: bool,
}
impl WoodlandPresentation {
    pub(super) fn choose(living_woods: bool) -> Self {
        Self {
            crowns: CrownDisplay::choose(living_woods),
            sampled: match std::env::var("FRONTIER_WOODLAND_MID_MESH").as_deref() {
                Ok("sampled") => true,
                Ok(_) => false,
                Err(_) => living_woods,
            },
            split_wood: render_option("FRONTIER_WOODLAND_SPLIT_WOOD", living_woods),
        }
    }
}

pub(super) fn tree_baseline(distance: f64) -> u8 {
    if distance > 220. {
        16
    } else {
        8
    }
}
/// Experimental half-cell contour displacement estimate with a 25% safety
/// factor for foliage; wood keeps a separately halved display stride. These
/// estimates require image validation and are not exact silhouette guarantees.
pub(super) fn matched_tree_stride(distance: f64, projection: f64, pixel_error: f64) -> u8 {
    let pixels_per_cell = frontier_world::voxel_size() * projection / distance.max(1.);
    let mut stride = 4;
    for next in [8, 16] {
        let foliage_deviation = next as f64 * 0.5 * 1.25 * pixels_per_cell;
        let wood_deviation = (next / 2) as f64 * 0.5 * pixels_per_cell;
        if foliage_deviation <= pixel_error && wood_deviation <= pixel_error * 0.5 {
            stride = next;
        }
    }
    stride
}
pub(super) fn matched_transition_requested() -> bool {
    std::env::var("FRONTIER_VEGETATION_TREE_TRANSITION").is_ok_and(|v| v == "matched")
}
pub(super) fn distant_tree_mesh(
    seed: u64,
    variant: u8,
    cancel: impl Fn() -> bool,
) -> VegetationMesh {
    recipe_tree_mesh(seed, variant, 16, WoodlandPresentation::default(), cancel)
}
/// Same rooted recipe and pigments for intermediate and distant crowns. No
/// canonical leaf model is allocated at either display stride.
pub(super) fn recipe_tree_mesh(
    seed: u64,
    variant: u8,
    stride: u8,
    presentation: WoodlandPresentation,
    cancel: impl Fn() -> bool,
) -> VegetationMesh {
    if let Some(recipe) = living_flora::woody_recipe(seed, variant) {
        return woodland_recipe_mesh(recipe, variant, stride, presentation, &cancel);
    }
    let stride = i64::from(stride.clamp(8, 32));
    if cancel() {
        return VegetationMesh::default();
    }
    let recipe = plant_recipe::rowan_recipe(seed, variant);
    let mut wood = BTreeSet::new();
    for s in &recipe.wood {
        if cancel() {
            return VegetationMesh::default();
        }
        let from = s.from.map(f64::from);
        let delta: [f64; 3] = std::array::from_fn(|i| f64::from(s.to[i]) - from[i]);
        let count = (delta.iter().map(|v| v.abs()).fold(0., f64::max) / 4.)
            .ceil()
            .max(1.) as usize;
        for i in 0..=count {
            let p: [i64; 3] = std::array::from_fn(|a| {
                (from[a] + delta[a] * i as f64 / count as f64).floor() as i64
            });
            wood.insert(Cell {
                x: p[0],
                y: p[1],
                z: p[2],
            });
        }
    }
    let mut leaves = BTreeSet::new();
    for s in &recipe.foliage {
        if cancel() {
            return VegetationMesh::default();
        }
        let c = s.center.map(f64::from);
        let r = [s.rx, 1.8, s.rz];
        let low: [i64; 3] =
            std::array::from_fn(|i| ((c[i] - r[i]).floor() as i64).div_euclid(stride));
        let high: [i64; 3] =
            std::array::from_fn(|i| ((c[i] + r[i]).ceil() as i64).div_euclid(stride));
        for x in low[0]..=high[0] {
            for y in low[1]..=high[1] {
                for z in low[2]..=high[2] {
                    let q = [x, y, z];
                    // Closest point of each coarse cube to the spray centre: a
                    // conservative ellipsoid/cube intersection, not a centre sample
                    // that could erase a thin spray between samples.
                    let d: f64 = (0..3)
                        .map(|i| {
                            let nearest =
                                c[i].clamp((q[i] * stride) as f64, ((q[i] + 1) * stride) as f64);
                            ((nearest - c[i]) / r[i]).powi(2)
                        })
                        .sum();
                    if d <= 1. {
                        leaves.insert(Cell {
                            x: x * stride,
                            y: y * stride,
                            z: z * stride,
                        });
                    }
                }
            }
        }
    }
    let mut trunk = mesh_cells(
        wood.into_iter()
            .map(|c| (c, Material::Wood as u8, linear_rgb([128, 116, 96]))),
        (stride / 2) as u8,
        &cancel,
    );
    let canopy = mesh_cells(
        leaves
            .into_iter()
            .map(|c| (c, Material::Leaf as u8, linear_rgb([107, 140, 66]))),
        stride as u8,
        &cancel,
    );
    if cancel() {
        return VegetationMesh::default();
    }
    let base = trunk.vertices.len() as u32;
    trunk.vertices.extend(canopy.vertices);
    trunk
        .indices
        .extend(canopy.indices.into_iter().map(|i| i + base));
    trunk
}

fn woodland_recipe_mesh(
    recipe: living_flora::WoodlandRecipe,
    variant: u8,
    stride: u8,
    presentation: WoodlandPresentation,
    cancel: &impl Fn() -> bool,
) -> VegetationMesh {
    let shrub = living_flora::is_shrub(variant);
    if !shrub && stride >= 32 {
        if let Some(volume) = presentation.crowns.step().and_then(|step| {
            crate::woodland_volume::WoodlandVolume::from_recipe(
                &recipe,
                step,
                presentation.crowns.atlas(),
                cancel,
            )
        }) {
            let mut mesh = VegetationMesh::default();
            append_box(
                &mut mesh,
                volume.low,
                volume.high,
                Material::Leaf,
                recipe.bark,
                cancel,
            );
            mesh.woodland_volume = Some(volume);
            return mesh;
        }
        return far_woodland_mesh(&recipe, variant, cancel);
    }
    if presentation.sampled {
        let step = if shrub {
            stride.min(5).max(3)
        } else if stride <= 4 {
            4
        } else {
            7
        };
        let Some(volume) = crate::woodland_volume::WoodlandVolume::from_recipe(
            &recipe,
            i32::from(step),
            None,
            cancel,
        ) else {
            return VegetationMesh::default();
        };
        if !shrub && presentation.split_wood {
            let Some(wood) = recipe.wood_cells(stride <= 4, cancel) else {
                return VegetationMesh::default();
            };
            let bark = linear_rgb(recipe.bark);
            let mut mesh = mesh_cells(
                wood.into_iter()
                    .map(|cell| (cell, Material::Wood as u8, bark)),
                if stride <= 4 { 2 } else { 3 },
                cancel,
            );
            let needles = mesh_cells(
                volume
                    .coarse_cells()
                    .filter(|&(_, material, _)| material == Material::Leaf as u8),
                step,
                cancel,
            );
            let offset = mesh.vertices.len() as u32;
            mesh.vertices.extend(needles.vertices);
            mesh.indices
                .extend(needles.indices.into_iter().map(|index| index + offset));
            return mesh;
        }
        return mesh_cells(volume.coarse_cells(), step, cancel);
    }
    if !shrub && compact_fans_enabled() {
        return fan_woodland_mesh(&recipe, stride, cancel);
    }
    let leaf_step = i64::from(if shrub { stride.min(8) } else { stride });
    let wood_step = (leaf_step / 2).max(1);
    let mut wood = BTreeSet::new();
    let segment_count = if !shrub && leaf_step >= 16 {
        recipe.trunk_strokes
    } else {
        recipe.wood.len()
    };
    for s in recipe.wood.iter().take(segment_count) {
        if cancel() {
            return VegetationMesh::default();
        }
        let delta = std::array::from_fn::<_, 3, _>(|i| f64::from(s.to[i] - s.from[i]));
        let segments = (delta.iter().map(|x| x.abs()).fold(0., f64::max) / wood_step as f64)
            .ceil()
            .max(1.) as usize;
        for segment in 0..=segments {
            let t = segment as f64 / segments as f64;
            let [x, y, z] =
                std::array::from_fn(|i| (f64::from(s.from[i]) + t * delta[i]).floor() as i64);
            wood.insert(Cell { x, y, z });
        }
    }
    let mut leaves = std::collections::BTreeMap::new();
    let pitch = leaf_step as f64;
    let mixed_pigment = if shrub || !living_flora::is_conifer(variant) {
        recipe.foliage.first().map_or([98, 126, 71], |f| f.pigment)
    } else if variant < living_flora::CONIFER + 4 {
        [77, 112, 69]
    } else {
        [87, 120, 65]
    };
    for group in &recipe.foliage {
        if cancel() {
            return VegetationMesh::default();
        }
        let center = group.center.map(f64::from);
        let min: [i64; 3] =
            std::array::from_fn(|a| ((center[a] - group.radius[a]) / pitch).floor() as i64);
        let max: [i64; 3] =
            std::array::from_fn(|a| ((center[a] + group.radius[a]) / pitch).floor() as i64);
        for x in min[0]..=max[0] {
            for y in min[1].max(0)..=max[1] {
                for z in min[2]..=max[2] {
                    let coords = [x, y, z];
                    let distance = (0..3)
                        .map(|a| {
                            let base = coords[a] as f64 * pitch;
                            ((center[a] - center[a].clamp(base, base + pitch)) / group.radius[a])
                                .powi(2)
                        })
                        .sum::<f64>();
                    if distance <= 1. {
                        let cell = Cell {
                            x: x * leaf_step,
                            y: y * leaf_step,
                            z: z * leaf_step,
                        };
                        let pigment = if leaf_step >= 8 {
                            mixed_pigment
                        } else {
                            group.pigment.map(|c| c / 8 * 8 + 4)
                        };
                        leaves.entry(cell).or_insert(pigment);
                    }
                }
            }
        }
    }
    let bark = linear_rgb(recipe.bark);
    let mut mesh = mesh_cells(
        wood.into_iter().map(|c| (c, Material::Wood as u8, bark)),
        wood_step as u8,
        cancel,
    );
    let needles = mesh_cells(
        leaves
            .into_iter()
            .map(|(c, pigment)| (c, Material::Leaf as u8, linear_rgb(pigment))),
        leaf_step as u8,
        cancel,
    );
    if cancel() {
        return VegetationMesh::default();
    }
    let first_leaf = mesh.vertices.len() as u32;
    mesh.vertices.extend(needles.vertices);
    mesh.indices
        .extend(needles.indices.into_iter().map(|i| i + first_leaf));
    mesh
}

fn compact_fans_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("FRONTIER_WOODLAND_COMPACT_FANS").as_deref() == Ok("1"))
}
fn fan_woodland_mesh(
    recipe: &living_flora::WoodlandRecipe,
    stride: u8,
    cancel: &impl Fn() -> bool,
) -> VegetationMesh {
    let steps = if stride <= 4 { 17 } else { 10 };
    let root = &recipe.wood[..16];
    let (first_branch, top) =
        recipe
            .foliage
            .iter()
            .fold((i64::from(root[15].to[1]), 0), |(lo, hi), g| {
                let y = i64::from(g.center[1]);
                (lo.min(y), hi.max(y + 1))
            });
    let mut mesh = VegetationMesh::default();
    for (i, branch) in root.iter().enumerate().step_by(2) {
        let a = branch.from.map(i64::from);
        let b = root[i + 1].to.map(i64::from);
        let r = i64::from(branch.radius);
        append_box(
            &mut mesh,
            [a[0].min(b[0]) - r, a[1], a[2].min(b[2]) - r],
            [a[0].max(b[0]) + r + 1, b[1] + 1, a[2].max(b[2]) + r + 1],
            Material::Wood,
            recipe.bark,
            cancel,
        );
    }
    for band in 0..steps {
        if cancel() {
            return VegetationMesh::default();
        }
        let lo = first_branch + (top - first_branch) * band / steps;
        let hi = first_branch + (top - first_branch) * (band + 1) / steps;
        let stem = root
            .iter()
            .find(|s| i64::from(s.to[1]) >= (lo + hi) / 2)
            .unwrap_or(&root[15]);
        let stem = [f64::from(stem.from[0]), f64::from(stem.from[2])];
        // A small central sector retains the shoots on the trunk itself.
        let mut sectors = [CrownSector::default(); 5];
        for group in recipe
            .foliage
            .iter()
            .filter(|g| (lo..hi).contains(&i64::from(g.center[1])))
        {
            let x = f64::from(group.center[0]) - stem[0];
            let z = f64::from(group.center[2]) - stem[1];
            let sector = if x.abs().max(z.abs()) < 5. {
                4
            } else {
                usize::from(x > 0.) + 2 * usize::from(z > 0.)
            };
            sectors[sector].include(group);
        }
        for (sector, bounds) in sectors.into_iter().enumerate() {
            if bounds.count == 0 {
                continue;
            }
            let shrink = if sector == 4 {
                [0.92, 0.81, 0.92]
            } else {
                [0.84, 0.65, 0.84]
            };
            let center: [f64; 3] = std::array::from_fn(|i| (bounds.lo[i] + bounds.hi[i]) * 0.5);
            let half: [f64; 3] =
                std::array::from_fn(|i| (bounds.hi[i] - bounds.lo[i]) * 0.5 * shrink[i]);
            let pigment = bounds.pigment.map(|value| (value / bounds.count) as u8);
            append_box(
                &mut mesh,
                std::array::from_fn(|i| (center[i] - half[i]).floor() as i64),
                std::array::from_fn(|i| (center[i] + half[i]).ceil() as i64 + 1),
                Material::Leaf,
                pigment,
                cancel,
            );
        }
    }
    mesh
}

#[derive(Clone, Copy)]
struct CrownSector {
    lo: [f64; 3],
    hi: [f64; 3],
    count: u32,
    pigment: [u32; 3],
}
impl Default for CrownSector {
    fn default() -> Self {
        Self {
            lo: [f64::INFINITY; 3],
            hi: [f64::NEG_INFINITY; 3],
            count: 0,
            pigment: [0; 3],
        }
    }
}
impl CrownSector {
    fn include(&mut self, group: &living_flora::LeafCluster) {
        for i in 0..3 {
            let center = f64::from(group.center[i]);
            self.lo[i] = self.lo[i].min(center - group.radius[i]);
            self.hi[i] = self.hi[i].max(center + group.radius[i]);
            self.pigment[i] += u32::from(group.pigment[i]);
        }
        self.count += 1;
    }
}

fn far_woodland_mesh(
    recipe: &living_flora::WoodlandRecipe,
    variant: u8,
    cancel: &impl Fn() -> bool,
) -> VegetationMesh {
    const TIERS: i64 = 6;
    let low = recipe
        .foliage
        .iter()
        .map(|g| i64::from(g.center[1]))
        .min()
        .unwrap_or(1);
    let high = recipe
        .foliage
        .iter()
        .map(|g| i64::from(g.center[1]) + g.radius[1].ceil() as i64)
        .max()
        .unwrap_or(low + TIERS);
    let interval = (high - low).max(TIERS);
    let radius = i64::from(recipe.wood[0].radius);
    let mut mesh = VegetationMesh::default();
    // The stem remains slender at distance. Envelope widths and crown start
    // are read from this particular recipe, retaining its different habit.
    append_box(
        &mut mesh,
        [-radius, 0, -radius],
        [radius + 1, low + interval / 2, radius + 1],
        Material::Wood,
        recipe.bark,
        cancel,
    );
    for tier in 0..TIERS {
        if cancel() {
            return VegetationMesh::default();
        }
        let bottom = low + interval * tier / TIERS;
        let top = low + interval * (tier + 1) / TIERS;
        let mut bounds = CrownSector::default();
        for leaves in recipe
            .foliage
            .iter()
            .filter(|g| (bottom - 3..=top + 2).contains(&i64::from(g.center[1])))
        {
            bounds.include(leaves);
        }
        if bounds.count == 0 {
            continue;
        }
        let width = if variant < living_flora::CONIFER + 4 {
            0.80
        } else {
            0.75
        };
        let [[x0, x1], [z0, z1]] = [0, 2].map(|i| {
            let [lo, hi] = [bounds.lo[i], bounds.hi[i]];
            let center = 0.5 * (lo + hi);
            let half = (hi - lo) * 0.5 * width + 1.;
            [
                (center - half).floor() as i64,
                (center + half).ceil() as i64,
            ]
        });
        let pitch = (top - bottom).max(1);
        // The shorter upper tiers partially separate as needle mass thins.
        let crown_top = top - if tier >= 2 { pitch / 8 } else { 0 };
        let light = 0.91 + tier as f64 * 0.023;
        let color = bounds.pigment.map(|c| (c / bounds.count) as u8);
        append_box(
            &mut mesh,
            [x0, bottom, z0],
            [x1, crown_top, z1],
            Material::Leaf,
            color.map(|c| (f64::from(c) * light) as u8),
            cancel,
        );
    }
    mesh
}

fn append_box(
    mesh: &mut VegetationMesh,
    low: [i64; 3],
    high: [i64; 3],
    material: Material,
    rgb: [u8; 3],
    cancel: &impl Fn() -> bool,
) {
    if cancel() || (0..3).any(|i| low[i] >= high[i]) {
        return;
    }
    let color = linear_rgb(rgb);
    let transmission = if material == Material::Leaf { 1. } else { 0. };
    let voxel = frontier_world::voxel_size() as f32;
    for axis in 0..3 {
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        for sign in [-1., 1.] {
            let mut normal = [0.; 3];
            normal[axis] = sign;
            let first = mesh.vertices.len() as u32;
            for [u1, v1] in [[false, false], [true, false], [true, true], [false, true]] {
                let mut p = low;
                if sign > 0. {
                    p[axis] = high[axis];
                }
                if u1 {
                    p[u] = high[u];
                }
                if v1 {
                    p[v] = high[v];
                }
                mesh.vertices.push(VegetationVertex {
                    position: p.map(|cell| cell as f32 * voxel),
                    normal,
                    color,
                    ao: 1.,
                    transmission,
                });
            }
            let indices = if sign > 0. {
                [0, 1, 2, 0, 2, 3]
            } else {
                [0, 2, 1, 0, 3, 2]
            };
            mesh.indices.extend(indices.map(|i| first + i));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn intermediate_recipe_is_repeatable_rooted_and_bounded_without_native_leaves() {
        for seed in [7, 941, 1274974721108113005] {
            for stride in [8, 16] {
                let a =
                    recipe_tree_mesh(seed, 3, stride, WoodlandPresentation::default(), || false);
                let b =
                    recipe_tree_mesh(seed, 3, stride, WoodlandPresentation::default(), || false);
                assert!(!a.indices.is_empty());
                assert!(a.indices.len() / 3 < 20_000);
                assert_eq!(a.indices, b.indices);
                assert_eq!(
                    bytemuck::cast_slice::<_, u8>(&a.vertices),
                    bytemuck::cast_slice::<_, u8>(&b.vertices)
                );
                assert!(a.vertices.iter().any(|v| v.position[1] <= 0.));
                assert!(a
                    .vertices
                    .iter()
                    .all(|v| v.position.iter().all(|p| p.is_finite())));
            }
        }
        assert!(
            recipe_tree_mesh(7, 3, 8, WoodlandPresentation::default(), || true)
                .indices
                .is_empty()
        );
    }
    #[test]
    fn projected_contour_policy_inserts_intermediate_lod_and_scales_with_resolution() {
        let projection = 1080. / (2. * (60f64.to_radians() / 2.).tan());
        for (distance, stride) in [(150., 4), (220., 4), (260., 8), (300., 8), (520., 16)] {
            assert_eq!(matched_tree_stride(distance, projection, 1.25), stride);
        }
        assert_eq!(matched_tree_stride(300., projection * 2., 1.25), 4);
        assert_eq!(matched_tree_stride(300., projection, 0.625), 4);
    }
    #[test]
    fn distant_recipes_are_bounded_repeatable_and_keep_rooted_wood() {
        for seed in [7, 941, 1274974721108113005] {
            let a = distant_tree_mesh(seed, 3, || false);
            let b = distant_tree_mesh(seed, 3, || false);
            assert!(!a.indices.is_empty());
            assert!(a.indices.len() / 3 < 5000);
            assert_eq!(a.indices, b.indices);
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&a.vertices),
                bytemuck::cast_slice::<_, u8>(&b.vertices)
            );
            assert!(a.vertices.iter().any(|v| v.position[1] <= 0.));
            assert!(a
                .vertices
                .iter()
                .all(|v| v.position.iter().all(|p| p.is_finite())));
        }
        assert!(distant_tree_mesh(7, 3, || true).indices.is_empty());
        assert_eq!(tree_baseline(220.), 8);
        assert_eq!(tree_baseline(220.001), 16);
    }
}
