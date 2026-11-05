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
    let center = data.centers[data.level_count as usize - 1].map(|v| (v * 2.) as i64);
    let radius = (half_extent(data.level_count) * 2.) as i64;
    assert_eq!(
        area2,
        8 * (half_extent(data.level_count) * 2.).powi(2) as i64,
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
    for level in 0..data.level_count as usize - 1 {
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
    for levels in [MIN_LEVELS, MAX_LEVELS] {
        let mut cache = SampleCache::new();
        let mut offsets: Vec<std::collections::HashSet<(i32, i32)>> =
            (1..levels).map(|_| Default::default()).collect();
        // Binary 1023 sets every lower snapping bit, exercising each level's
        // zero/one offset in both axes, including all four diagonal combinations.
        for position in [
            [0., 0.],
            [1023., 0.],
            [0., 1023.],
            [1023., 1023.],
            [-1., -1.],
        ] {
            let data =
                TerrainData::generate_levels(&source, position, &mut cache, levels, || false)
                    .unwrap();
            for level in 1..levels as usize {
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
}

#[test]
fn view_distance_is_bounded_and_only_adds_coarser_levels() {
    assert_eq!(PAGE_BYTES, page_bytes(MIN_LEVELS));
    assert_eq!(
        (MIN_LEVELS..=MAX_LEVELS)
            .map(half_extent)
            .collect::<Vec<_>>(),
        vec![2048., 4096., 8192., 16384., 32768.]
    );
    let source = FbmTerrain::new(
        7,
        frontier_world::FbmParams {
            octaves: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(TerrainStreamer::with_levels(source.clone(), MIN_LEVELS - 1).is_err());
    assert!(TerrainStreamer::with_levels(source.clone(), MAX_LEVELS + 1).is_err());
    let mut cache = SampleCache::new();
    let near =
        TerrainData::generate_levels(&source, [-1., 1023.], &mut cache, MIN_LEVELS, || false)
            .unwrap();
    let far = TerrainData::generate_levels(&source, [-1., 1023.], &mut cache, MAX_LEVELS, || false)
        .unwrap();
    assert_eq!(near.vertices.as_ptr(), far.vertices.as_ptr());
    assert_eq!(near.indices.as_ptr(), far.indices.as_ptr());
    assert_eq!(near.ranges, far.ranges[..MIN_LEVELS as usize]);
    assert_eq!(
        bytemuck::cast_slice::<Sample, u8>(&near.samples),
        bytemuck::cast_slice::<Sample, u8>(&far.samples[..near.samples.len()])
    );
    for data in [&near, &far] {
        assert_eq!(
            (data.samples.len() * std::mem::size_of::<Sample>()) as u64,
            page_bytes(data.level_count)
        );
        assert_eq!(data.ranges.len(), data.level_count as usize);
        assert_eq!(data.vertices[0].position, [-32., -32.]);
    }
}
