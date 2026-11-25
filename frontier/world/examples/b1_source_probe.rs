//! Reproduce with: cargo run --release -p frontier-world --example b1_source_probe
//! CPU height-page experiment only. This is not adaptive renderer readiness.
use frontier_world::{
    visual_source::{generate_visual, VisualRequest},
    FbmParams, FbmRecipe, World, VOXEL_SIZE,
};
use std::{hint::black_box, time::Instant};
fn main() {
    println!("{{\"experiment\":\"B1_height_source_only\",\"native_voxel_m\":{VOXEL_SIZE},\"requested_quality_readiness_ms\":null,\"visual_quality_pass\":null,\"motion_quality_pass\":null,\"results\":[");
    let mut first = true;
    for recipe in [
        FbmRecipe::Legacy,
        FbmRecipe::Regional,
        FbmRecipe::RegionalUnwarped,
    ] {
        let world = World::fbm(
            7,
            FbmParams {
                recipe,
                ..Default::default()
            },
        )
        .unwrap();
        let base = VisualRequest {
            min: [-16384, -16384],
            samples: [129, 129],
            spacing_cells: 256,
            source_revision: world.fbm_terrain().unwrap().source_version(),
            edit_revision: world.revision(),
            retained_octaves: 8,
            max_samples: 129 * 129,
        };
        let oracle = generate_visual(&world, base).unwrap();
        for retained in 1..=8 {
            let request = VisualRequest {
                retained_octaves: retained,
                ..base
            };
            black_box(generate_visual(&world, request).unwrap());
            let mut timings = Vec::new();
            for _ in 0..9 {
                let start = Instant::now();
                black_box(generate_visual(&world, request).unwrap());
                timings.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            timings.sort_by(f64::total_cmp);
            let page = generate_visual(&world, request).unwrap();
            let mut max: f64 = 0.0;
            let mut squared = 0.0;
            for (h, exact) in page.heights_m.iter().zip(&oracle.heights_m) {
                let e = (h - exact).abs();
                max = max.max(e);
                squared += e * e;
            }
            if !first {
                println!(",");
            }
            first = false;
            print!("{{\"recipe\":\"{recipe:?}\",\"seed\":7,\"source_revision\":{},\"edit_revision\":0,\"retained_octaves\":{retained},\"sample_count\":{},\"elevation_noise_evaluations\":{},\"oracle_elevation_noise_evaluations\":{},\"evaluation_reduction_ratio\":{},\"ideal_sample_error_bound_m\":{},\"certified_float_bound\":false,\"sample_max_error_m\":{max},\"sample_rmse_m\":{},\"source_page_ms_min\":{},\"source_page_ms_median\":{},\"source_page_ms_max\":{},\"exact_fallback\":{},\"sample_one_voxel_gate\":{}}}",
                request.source_revision, page.heights_m.len(), page.elevation_noise_evaluations, oracle.elevation_noise_evaluations,
                oracle.elevation_noise_evaluations as f64 / page.elevation_noise_evaluations as f64, page.ideal_sample_error_bound_m,
                (squared / page.heights_m.len() as f64).sqrt(), timings[0], timings[4], timings[8], page.exact_fallback,
                max <= VOXEL_SIZE);
        }
    }
    println!("\n]}}");
}
