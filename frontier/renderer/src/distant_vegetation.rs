//! Cached display-only Rowan proxies. Same seeded growth strokes/sprays as the
//! native tree, sampled directly on a coarse grid; never allocate native leaves.
//! Greenfield implementation inspired by multiscale forest representations
//! (Bruneton & Neyret 2012), not a port of their precomputed impostor renderer.
use super::{mesh_cells, VegetationMesh};
use frontier_world::{vegetation::plant_recipe, Cell, Material};
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

pub(super) fn distant_tree_radius() -> f64 {
    std::env::var("FRONTIER_VEGETATION_FAR_RADIUS_M")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(768.)
        .clamp(220., 1024.)
}
pub(super) fn tree_baseline(distance: f64) -> u8 {
    if distance > 220. {
        16
    } else {
        8
    }
}
pub(super) fn distant_tree_mesh(
    seed: u64,
    variant: u8,
    cancel: impl Fn() -> bool,
) -> VegetationMesh {
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
        let low: [i64; 3] = std::array::from_fn(|i| ((c[i] - r[i]).floor() as i64).div_euclid(16));
        let high: [i64; 3] = std::array::from_fn(|i| ((c[i] + r[i]).ceil() as i64).div_euclid(16));
        for x in low[0]..=high[0] {
            for y in low[1]..=high[1] {
                for z in low[2]..=high[2] {
                    let q = [x, y, z];
                    // Closest point of each coarse cube to the spray centre: a
                    // conservative ellipsoid/cube intersection, not a centre sample
                    // that could erase a thin spray between samples.
                    let d: f64 = (0..3)
                        .map(|i| {
                            let nearest = c[i].clamp((q[i] * 16) as f64, ((q[i] + 1) * 16) as f64);
                            ((nearest - c[i]) / r[i]).powi(2)
                        })
                        .sum();
                    if d <= 1. {
                        leaves.insert(Cell {
                            x: x * 16,
                            y: y * 16,
                            z: z * 16,
                        });
                    }
                }
            }
        }
    }
    let mut trunk = mesh_cells(
        wood.into_iter()
            .map(|c| (c, Material::Wood as u8, linear_rgb([128, 116, 96]))),
        8,
        &cancel,
    );
    let canopy = mesh_cells(
        leaves
            .into_iter()
            .map(|c| (c, Material::Leaf as u8, linear_rgb([107, 140, 66]))),
        16,
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

#[cfg(test)]
mod tests {
    use super::*;
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
