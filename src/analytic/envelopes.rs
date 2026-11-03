//! Conservative quadratic bounds over groups of source patches; see
//! agent_context/QUADRATIC_ENVELOPES.md for the construction and proof.
use super::scene::{gradient, value, Scene};
type Polynomial = [f64; 10];

/// Analytic cube upper bound, not a sampling estimate.
fn cube_upper(q: &Polynomial) -> f64 {
    q[0] + q[1..4].iter().map(|v| v.abs()).sum::<f64>()
        + q[4..7].iter().map(|v| v.max(0.)).sum::<f64>()
        + q[7..10].iter().map(|v| v.abs()).sum::<f64>()
}
/// Q(a*u+b), using a uniform scalar coordinate scale.
fn transform(q: &Polynomial, a: f64, b: [f64; 3]) -> Polynomial {
    let mut out = *q;
    out[0] = value(q, b);
    let g = gradient(q, b);
    for i in 0..3 {
        out[1 + i] = a * g[i];
    }
    for i in 4..10 {
        out[i] *= a * a;
    }
    out
}
fn parent(children: &[(Polynomial, [f64; 3])]) -> Polynomial {
    let mut p = [0.; 10];
    for (q, octant) in children {
        let mapped = transform(q, 2., octant.map(|x| 1. - 2. * x));
        for i in 0..10 {
            p[i] += mapped[i] / children.len() as f64;
        }
    }
    let mut correction = f64::NEG_INFINITY;
    let mut magnitude = 1. + p.iter().map(|v| v.abs()).sum::<f64>();
    for (q, octant) in children {
        let in_child = transform(&p, 0.5, octant.map(|x| x - 0.5));
        let residual = std::array::from_fn(|i| q[i] - in_child[i]);
        correction = correction.max(cube_upper(&residual));
        magnitude += q.iter().map(|v| v.abs()).sum::<f64>();
    }
    p[0] += correction + 1e-10 * magnitude;
    p
}
fn pack(q: &Polynomial) -> [f32; 12] {
    let mut out = [0.; 12];
    for i in 0..10 {
        out[i] = q[i] as f32;
    }
    let rounding = q
        .iter()
        .zip(out)
        .map(|(a, b)| (a - b as f64).abs())
        .sum::<f64>();
    let guard = 0.0001 * (1. + q.iter().map(|v| v.abs()).sum::<f64>());
    out[0] = (q[0] + rounding + guard) as f32;
    // Runtime first_possible() adds this second guard to the ray equation.
    out[10] = guard as f32;
    out
}
/// Parent entries use the same y,z,x ordering as the existing air hierarchy.
/// The first stored entry corresponds to air descriptor level 1. Leaf bounds
/// remain temporary CPU data and are not uploaded.
pub fn build(scene: &Scene) -> Vec<[f32; 12]> {
    let packed = scene.packed();
    let mut level: Vec<Polynomial> = packed
        .chunks_exact(scene.quadratics_per_patch)
        .map(|patch| {
            if scene.is_spline() {
                let maximum = patch
                    .iter()
                    .map(|q| {
                        let c = std::array::from_fn(|i| q[i] as f64);
                        cube_upper(&c) + q[10] as f64
                    })
                    .fold(f64::NEG_INFINITY, f64::max);
                let mut bound = [0.; 10];
                bound[0] = maximum;
                bound
            } else {
                let chosen = patch
                    .iter()
                    .min_by(|a, b| {
                        let aq = std::array::from_fn(|i| a[i] as f64);
                        let bq = std::array::from_fn(|i| b[i] as f64);
                        (cube_upper(&aq) + a[10] as f64)
                            .total_cmp(&(cube_upper(&bq) + b[10] as f64))
                    })
                    .unwrap();
                let mut q = std::array::from_fn(|i| chosen[i] as f64);
                q[0] += chosen[10] as f64;
                q
            }
        })
        .collect();
    let mut dims = scene.patches_per_axis;
    let mut output = Vec::new();
    while dims != [1; 3] {
        let next = dims.map(|v| v.div_ceil(2));
        let mut parents = Vec::with_capacity(next.iter().product());
        for y in 0..next[1] {
            for z in 0..next[2] {
                for x in 0..next[0] {
                    let mut children = Vec::with_capacity(8);
                    for dy in 0..2 {
                        for dz in 0..2 {
                            for dx in 0..2 {
                                let [cx, cy, cz] = [2 * x + dx, 2 * y + dy, 2 * z + dz];
                                if cx < dims[0] && cy < dims[1] && cz < dims[2] {
                                    children.push((
                                        level[(cy * dims[2] + cz) * dims[0] + cx],
                                        [dx as f64, dy as f64, dz as f64],
                                    ));
                                }
                            }
                        }
                    }
                    let p = parent(&children);
                    output.push(pack(&p));
                    parents.push(p);
                }
            }
        }
        level = parents;
        dims = next;
    }
    if output.is_empty() {
        output.push([0.; 12]);
    }
    output
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn odd_hierarchy_envelopes_bound_voxel_centers_at_every_parent_level() {
        let coefficients: Vec<_> = (0..5)
            .map(|y| {
                (0..2)
                    .map(|z| {
                        (0..3)
                            .map(|x| {
                                (0..2)
                                    .map(|branch| {
                                        std::array::from_fn::<_, 10, _>(|i| {
                                            ((y * 47 + z * 23 + x * 11 + branch * 7 + i * 3) as f64)
                                                .sin()
                                                * 4.
                                        })
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let s: Scene = serde_json::from_value(serde_json::json!({
            "version":1,"representation":"min-of-implicit-quadrics","solid":"min(Q)>0",
            "feature_order":[],"patch_order":"y,z,x","patches_per_axis":[3,5,2],
            "quadratics_per_patch":2,"patch_edge_m":2.,"origin_cells":[0,0,0],
            "voxel_m":0.125,"dimensions":[48,80,32],"normalization":"",
            "coefficients":coefficients
        }))
        .unwrap();
        let air = super::super::traversal::build(&s);
        let bounds = build(&s);
        let source = s.packed();
        let first = air[4] as usize;
        assert_eq!(bounds.len(), air.len() - first);
        for y in 0..5 {
            for z in 0..2 {
                for x in 0..3 {
                    for sample in 0..50 {
                        let p: [f64; 3] =
                            std::array::from_fn(|a| ((sample * 19 + a * 7) as f64).sin() * 0.99);
                        let center = p.map(|v| ((((v + 1.) * 8.).floor() + 0.5) / 8.) - 1.);
                        let field = source
                            [((y * 2 + z) * 3 + x) * 2..((y * 2 + z) * 3 + x + 1) * 2]
                            .iter()
                            .map(|q| value(&std::array::from_fn(|i| q[i] as f64), center))
                            .fold(f64::INFINITY, f64::min);
                        for level in 1..air[36] as usize {
                            let width = 1usize << level;
                            let node = [x / width, y / width, z / width];
                            let desc = level * 4;
                            let index = air[desc] as usize
                                + (node[1] * air[desc + 3] as usize + node[2])
                                    * air[desc + 1] as usize
                                + node[0]
                                - first;
                            let u = std::array::from_fn(|a| {
                                2. * (([x, y, z][a] % width) as f64 + (p[a] + 1.) * 0.5)
                                    / width as f64
                                    - 1.
                            });
                            let q = std::array::from_fn(|i| bounds[index][i] as f64);
                            assert!(field <= value(&q, u));
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn affine_mapping_is_exact_for_random_quadratics() {
        for n in 0..100 {
            let q = std::array::from_fn(|i| ((n * 17 + i * 31) as f64).sin() * 10.);
            let b = [-0.5, 0.5, -0.5];
            let a = 0.5;
            let mapped = transform(&q, a, b);
            for i in 0..100 {
                let u = std::array::from_fn(|k| ((i * 7 + k * 13) as f64).sin());
                let v = std::array::from_fn(|k| u[k] * a + b[k]);
                assert!((value(&q, v) - value(&mapped, u)).abs() < 1e-12);
            }
        }
    }
    #[test]
    fn parent_and_packed_parent_dominate_children_including_corners() {
        for seed in 0..100 {
            let children: Vec<_> = (0..8)
                .map(|k| {
                    let q =
                        std::array::from_fn(|i| ((seed * 71 + k * 17 + i * 23) as f64).sin() * 30.);
                    (
                        q,
                        [(k & 1) as f64, ((k >> 1) & 1) as f64, ((k >> 2) & 1) as f64],
                    )
                })
                .collect();
            let p = parent(&children);
            let gpu = pack(&p);
            let rounded = std::array::from_fn(|i| gpu[i] as f64);
            for (q, octant) in &children {
                for i in 0..100 {
                    let v = if i < 8 {
                        [
                            (i & 1) as f64 * 2. - 1.,
                            ((i >> 1) & 1) as f64 * 2. - 1.,
                            ((i >> 2) & 1) as f64 * 2. - 1.,
                        ]
                    } else {
                        std::array::from_fn(|k| ((i * 7 + k * 13) as f64).sin())
                    };
                    let u = std::array::from_fn(|k| 0.5 * v[k] + octant[k] - 0.5);
                    assert!(value(q, v) <= value(&p, u) + 1e-9);
                    assert!(value(q, v) <= value(&rounded, u));
                }
            }
        }
    }
}
