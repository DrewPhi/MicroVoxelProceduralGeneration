//! Conservative air bounds and an all-air mip hierarchy. No terrain approximation.
use super::scene::Scene;
/// Upper bound for a quadratic on [-1,1]^3. Rounded f32 coefficients are the
/// rendering source; padding includes the native voxel-center envelope.
fn upper(q: &[f32; 12]) -> f64 {
    q[0] as f64
        + q[1..4].iter().map(|v| (*v as f64).abs()).sum::<f64>()
        + q[4..7].iter().map(|v| (*v as f64).max(0.)).sum::<f64>()
        + q[7..10].iter().map(|v| (*v as f64).abs()).sum::<f64>()
        + q[10] as f64
        + 0.0001 * (1. + q[..10].iter().map(|v| (*v as f64).abs()).sum::<f64>())
}
/// First 40 u32: nine (offset,x,y,z) descriptors and level count at 36.
/// Each following node is 1 only when its entire region is provably air.
pub fn build(s: &Scene) -> Vec<u32> {
    let packed = s.packed();
    let mut dims = s.patches_per_axis;
    let count = dims.iter().product::<usize>();
    let n = s.quadratics_per_patch;
    let mut nodes = vec![0u32; 40];
    for patch in packed.chunks_exact(n) {
        let air = if s.is_spline() {
            patch.iter().all(|q| upper(q) < 0.)
        } else {
            patch.iter().any(|q| upper(q) < 0.)
        };
        nodes.push(u32::from(air));
    }
    debug_assert_eq!(nodes.len(), 40 + count);
    let mut offset = 40;
    let mut level = 0;
    loop {
        nodes[level * 4] = offset as u32;
        for a in 0..3 {
            nodes[level * 4 + a + 1] = dims[a] as u32;
        }
        level += 1;
        if dims == [1; 3] {
            break;
        }
        let next = dims.map(|v| v.div_ceil(2));
        let next_offset = nodes.len();
        for y in 0..next[1] {
            for z in 0..next[2] {
                for x in 0..next[0] {
                    let mut air = true;
                    for dy in 0..2 {
                        for dz in 0..2 {
                            for dx in 0..2 {
                                let [cx, cy, cz] = [x * 2 + dx, y * 2 + dy, z * 2 + dz];
                                if cx < dims[0] && cy < dims[1] && cz < dims[2] {
                                    air &= nodes[offset + (cy * dims[2] + cz) * dims[0] + cx] != 0;
                                }
                            }
                        }
                    }
                    nodes.push(u32::from(air));
                }
            }
        }
        offset = next_offset;
        dims = next;
    }
    nodes[36] = level as u32;
    nodes
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn odd_sized_hierarchy_never_marks_a_mixed_parent_as_air() {
        let q = [-2., 0., 0., 0., 0., 0., 0., 0., 0., 0.];
        let mut s: Scene = serde_json::from_value(serde_json::json!({
            "version":1,"representation":"min-of-implicit-quadrics","solid":"min(Q)>0",
            "feature_order":[],"patch_order":"y,z,x","patches_per_axis":[3,5,2],
            "quadratics_per_patch":1,"patch_edge_m":8.,"origin_cells":[0,0,0],
            "voxel_m":0.0625,"dimensions":[384,640,256],"normalization":"",
            "coefficients":vec![vec![vec![vec![q];3];2];5]
        }))
        .unwrap();
        let air = build(&s);
        assert!(air[40..].iter().all(|&v| v == 1));
        s.coefficients[4][1][2][0][0] = 2.;
        let mixed = build(&s);
        assert_eq!(mixed[40 + (4 * 2 + 1) * 3 + 2], 0);
        let root = mixed[(mixed[36] as usize - 1) * 4] as usize;
        assert_eq!(mixed[root], 0);
        assert_eq!(mixed[40], 1);
    }
    #[test]
    fn bound_contains_sampled_quadratic_values() {
        let mut seed = 71u64;
        for _ in 0..1000 {
            let mut rng = || {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((seed >> 32) as u32 as f64 / u32::MAX as f64) * 2. - 1.
            };
            let q: [f32; 12] =
                std::array::from_fn(|i| if i < 10 { rng() as f32 * 10. } else { 0. });
            let c = std::array::from_fn(|i| q[i] as f64);
            for _ in 0..32 {
                assert!(super::super::scene::value(&c, [rng(), rng(), rng()]) <= upper(&q));
            }
        }
    }
}
