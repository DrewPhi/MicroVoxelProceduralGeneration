use frontier_world::{desert, FbmParams, FbmRecipe, FbmTerrain};
fn main() {
    for seed in [7, 19, 42, 91, 1337] {
        let o = desert::oasis(seed, 0, 0);
        let t = FbmTerrain::new(
            seed,
            FbmParams {
                recipe: FbmRecipe::Desert,
                ..Default::default()
            },
        )
        .unwrap();
        let high = (0..16)
            .map(|a| {
                let angle = a as f64 * std::f64::consts::TAU / 16.;
                let x = o.x + angle.sin() * 700.;
                let z = o.z + angle.cos() * 700.;
                (t.height_m(x, z), x, z)
            })
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap();
        println!("seed={seed} oasis={o:?} backdrop={high:?}");
    }
}
