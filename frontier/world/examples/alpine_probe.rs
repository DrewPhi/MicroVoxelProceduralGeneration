//! Reproducible default-scale Alpine camera/path/source inspection.
use frontier_world::{FbmParams, FbmRecipe, FbmTerrain};
fn main() {
    let seed = std::env::args()
        .skip(1)
        .find_map(|s| s.parse().ok())
        .unwrap_or(7);
    let terrain = FbmTerrain::new(
        seed,
        FbmParams {
            recipe: FbmRecipe::Alpine,
            ..Default::default()
        },
    )
    .unwrap();
    if std::env::args().any(|s| s == "--json") {
        println!("{{\"seed\":{seed},\"camera_candidates\":[");
        for (i, (name, x, z, eye_offset)) in [
            (
                "meadow",
                frontier_world::terrain::alpine_path_center_m(-250.),
                -250.,
                1.7,
            ),
            (
                "path-forward",
                frontier_world::terrain::alpine_path_center_m(-100.),
                -100.,
                1.7,
            ),
            (
                "overlook",
                frontier_world::terrain::alpine_path_center_m(150.),
                150.,
                5.,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let y = terrain.height_m(x, z) + eye_offset;
            println!(
                "{}{{\"name\":\"{name}\",\"eye\":[{x},{y},{z}],\"target\":[0,650,3300]}}",
                if i > 0 { "," } else { "" }
            );
        }
        println!("],\"terrain_stats\":[");
        for (i, (name, x0, x1, z0, z1)) in [
            ("meadow", -500, 500, -500, 500),
            ("backdrop", -2000, 2000, 2500, 4500),
        ]
        .into_iter()
        .enumerate()
        {
            let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
            for z in (z0..=z1).step_by(25) {
                for x in (x0..=x1).step_by(25) {
                    let h = terrain.height_m(x as f64, z as f64);
                    low = low.min(h);
                    high = high.max(h);
                }
            }
            println!("{}{{\"name\":\"{name}\",\"sampled_min_m\":{low},\"sampled_max_m\":{high},\"step_m\":25,\"conservative\":false}}", if i>0 {","} else {""});
        }
        println!("]}}");
        return;
    }
    println!(
        "Alpine seed={seed} source={} appearance={}",
        terrain.source_version(),
        terrain.appearance_version()
    );
    for [x, z] in [[0., -250.], [-180., -100.], [250., 150.], [0., 3300.]] {
        let h = terrain.height_m(x, z);
        println!(
            "camera ground x={x:.3} z={z:.3} y={h:.3} eye_y={:.3}",
            h + 1.7
        );
    }
    for z in [-400., -250., 0., 250., 500., 900., 1400.] {
        let x = frontier_world::terrain::alpine_path_center_m(z);
        let h = terrain.height_m(x, z);
        println!(
            "path x={x:.3} z={z:.3} y={h:.3} material={:?}",
            terrain.surface_material(x, z, h)
        );
    }
    for (name, x0, x1, z0, z1) in [
        ("meadow", -500, 500, -500, 500),
        ("backdrop", -2000, 2000, 2500, 4500),
    ] {
        let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
        for z in (z0..=z1).step_by(25) {
            for x in (x0..=x1).step_by(25) {
                let h = terrain.height_m(x as f64, z as f64);
                low = low.min(h);
                high = high.max(h);
            }
        }
        println!(
            "{name} sampled 25m range=[{low:.3},{high:.3}] (empirical; not a conservative bound)"
        );
    }
}
