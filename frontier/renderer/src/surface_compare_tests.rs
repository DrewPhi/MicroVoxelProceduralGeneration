use super::*;
use frontier_world::{FbmParams, FbmTerrain};
fn data(center: [f64; 2]) -> TerrainData {
    TerrainData::at_center(
        &FbmTerrain::new(
            7,
            FbmParams {
                octaves: 1,
                ..Default::default()
            },
        )
        .unwrap(),
        center,
    )
}
#[test]
fn shader_validates() {
    let module = naga::front::wgsl::parse_str(SHADER)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(SHADER)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
}
#[test]
fn selection_covers_same_square_without_overlaps_and_balances() {
    for center in [[0., 0.], [13., -29.]] {
        let d = data(center);
        let layout = Layout::new(&d);
        for eye in [[0., 0.], [37., -101.], [1100., -700.], [5000., 5000.]] {
            let leaves = layout.select(eye);
            assert!(leaves.len() <= CAPACITY);
            let top_step = 1 << (layout.levels - 1);
            let side = 128 * top_step;
            assert_eq!(
                leaves.iter().map(|p| (p.side() as i64).pow(2)).sum::<i64>(),
                (side as i64).pow(2)
            );
            for (i, &p) in leaves.iter().enumerate() {
                assert!(layout.contains(p));
                for &q in &leaves[i + 1..] {
                    assert!(
                        !(overlap(p.x, p.x + p.side(), q.x, q.x + q.side())
                            && overlap(p.z, p.z + p.side(), q.z, q.z + q.side()))
                    );
                    if neighbors(p, q) {
                        assert!(p.level.abs_diff(q.level) <= 1);
                    }
                }
            }
            eprintln!(
                "quadtree eye={eye:?} {} patches {} triangles {} bytes shared pages",
                leaves.len(),
                leaves.len() * 512,
                d.samples.len() * 80
            );
        }
    }
}
fn point(p: Leaf, patch: Patch, x: u32, z: u32) -> ([f32; 2], bool) {
    let (mut x, mut z) = (x, z);
    let mut parent = false;
    if (x == 0 && patch.edges & 1 != 0) || (x == 16 && patch.edges & 2 != 0) {
        z -= z % 2;
        parent = true;
    }
    if (z == 0 && patch.edges & 4 != 0) || (z == 16 && patch.edges & 8 != 0) {
        x -= x % 2;
        parent = true;
    }
    if (x == 0 || x == 16) && (z == 0 || z == 16) {
        let corner = usize::from(x == 16) + usize::from(z == 16) * 2;
        parent |= patch.edges & (1 << (4 + corner)) != 0;
    }
    let step = 0.5 * (1 << p.level) as f32;
    (
        [
            patch.offset[0] + x as f32 * step,
            patch.offset[1] + z as f32 * step,
        ],
        parent,
    )
}
#[test]
fn shared_joins_match_positions_and_parent_heights_from_same_pages() {
    for center in [[0., 0.], [13., -29.]] {
        let d = data(center);
        let layout = Layout::new(&d);
        let leaves = layout.select([3.17, -11.2]);
        let patches = layout.patches(&leaves);
        let mut vertices =
            std::collections::HashMap::<[i32; 2], Vec<(f32, [f32; 4], [f32; 4])>>::new();
        let mut collapsed = 0;
        for (&p, &patch) in leaves.iter().zip(&patches) {
            for z in 0..=16 {
                for x in 0..=16 {
                    if x != 0 && x != 16 && z != 0 && z != 16 {
                        continue;
                    }
                    let (ground, parent) = point(p, patch, x, z);
                    collapsed += usize::from(parent);
                    let step = 0.5 * (1 << p.level) as f32;
                    let c = std::array::from_fn::<_, 2, _>(|a| {
                        d.centers[p.level as usize][a] - layout.origin[a]
                    });
                    let px = ((ground[0] as f64 - c[0]) / step as f64).round() as i32 + 64;
                    let pz = ((ground[1] as f64 - c[1]) / step as f64).round() as i32 + 64;
                    assert!((0..129).contains(&px) && (0..129).contains(&pz));
                    let s = d.samples[p.level as usize * PAGE + (pz * 129 + px) as usize];
                    vertices
                        .entry(ground.map(|v| (v * 2.) as i32))
                        .or_default()
                        .push(if parent {
                            (s.heights[1], s.parent_normal, s.parent_color)
                        } else {
                            (s.heights[0], s.normal, s.color)
                        });
                }
            }
        }
        assert!(collapsed > 0);
        for values in vertices.values() {
            for v in values {
                assert_eq!(*v, values[0]);
            }
        }
    }
}
