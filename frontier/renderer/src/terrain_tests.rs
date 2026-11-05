//! Independent planar manifold and cross-level seam oracles.
use super::*;
use std::collections::HashMap;
type Point = (i64, i64);
fn global_vertex(data: &TerrainData, index: u32) -> Point {
    let count = ((N * 2 + 1).pow(2)) as usize;
    let level = index as usize / count;
    let v = data.vertices[index as usize];
    let c = data.centers[level];
    (
        ((v.position[0] as f64 + c[0]) * 2.).round() as i64,
        ((v.position[1] as f64 + c[1]) * 2.).round() as i64,
    )
}
fn check_manifold(data: &TerrainData) {
    let mut area2 = 0i64;
    let mut edges: HashMap<(Point, Point), u32> = HashMap::new();
    for range in &data.ranges {
        for t in data.indices[range.start as usize..range.end as usize].chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| global_vertex(data, i));
            let signed = (b.1 - a.1) * (c.0 - a.0) - (b.0 - a.0) * (c.1 - a.1);
            assert!(signed >= 0, "down-facing triangle {a:?} {b:?} {c:?}");
            area2 += signed;
            if signed == 0 {
                continue;
            }
            for (a, b) in [(a, b), (b, c), (c, a)] {
                *edges
                    .entry(if a < b { (a, b) } else { (b, a) })
                    .or_default() += 1;
            }
        }
    }
    let center = data.centers[LEVELS as usize - 1].map(|v| (v * 2.) as i64);
    let radius = (HALF_EXTENT * 2.) as i64;
    assert_eq!(
        area2,
        8 * (HALF_EXTENT * 2.).powi(2) as i64,
        "coverage area differs from complete outer square"
    );
    for ((a, b), count) in edges {
        let outer = (a.0 == b.0 && (a.0 - center[0]).abs() == radius)
            || (a.1 == b.1 && (a.1 - center[1]).abs() == radius);
        assert_eq!(
            count,
            if outer { 1 } else { 2 },
            "open/overlapping edge {a:?} {b:?}"
        );
    }
}
fn check_shading_and_depth_seams(data: &TerrainData) {
    let count = ((N * 2 + 1).pow(2)) as usize;
    for level in 0..LEVELS as usize - 1 {
        let coarse: HashMap<_, _> = ((level + 1) * count..(level + 2) * count)
            .map(|i| (global_vertex(data, i as u32), i))
            .collect();
        let mut matched = 0;
        for i in level * count..(level + 1) * count {
            let v = data.vertices[i];
            if v.morph != 1. {
                continue;
            }
            let point = global_vertex(data, i as u32);
            let j = *coarse
                .get(&point)
                .expect("fine edge must terminate at a parent anchor");
            assert_eq!(
                data.vertices[j].morph, 0.,
                "displaced inner hole must stay outside parent morph annulus"
            );
            let fine = data.samples[i];
            let parent = data.samples[j];
            assert_eq!(
                fine.heights[1], parent.heights[0],
                "depth seam at {point:?}"
            );
            assert_eq!(
                fine.parent_normal, parent.normal,
                "normal seam at {point:?}"
            );
            assert_eq!(fine.parent_color, parent.color, "color seam at {point:?}");
            matched += 1;
        }
        assert!(matched > 0);
    }
}
#[test]
fn all_four_offset_parities_have_complete_manifold_and_identical_seams() {
    let source = frontier_world::FbmTerrain::new(
        91,
        frontier_world::FbmParams {
            octaves: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let mut cache = SampleCache::new();
    let mut offsets: [std::collections::HashSet<(i32, i32)>; 6] =
        std::array::from_fn(|_| Default::default());
    // Binary 127 sets every lower snapping bit, exercising each level's
    // zero/one offset in both axes, including all four diagonal combinations.
    for position in [[0., 0.], [127., 0.], [0., 127.], [127., 127.], [-1., -1.]] {
        let data = TerrainData::generate(&source, position, &mut cache, || false).unwrap();
        for level in 1..LEVELS as usize {
            let step = 0.5 * (1u32 << level) as f64;
            offsets[level - 1].insert((
                ((data.centers[level - 1][0] - data.centers[level][0]) / step) as i32,
                ((data.centers[level - 1][1] - data.centers[level][1]) / step) as i32,
            ));
        }
        check_manifold(&data);
        check_shading_and_depth_seams(&data);
    }
    for variants in offsets {
        assert_eq!(
            variants,
            [(0, 0), (0, 1), (1, 0), (1, 1)].into_iter().collect()
        );
    }
}
