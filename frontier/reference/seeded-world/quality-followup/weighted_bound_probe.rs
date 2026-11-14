extern crate frontier_world;
use frontier_world::affine_bounds::{bound_native_height, BoundBudget, ColumnBox};
use frontier_world::{FbmParams, FbmRecipe, FbmTerrain, VOXEL_SIZE};
fn main() {
    let t = FbmTerrain::new(
        19,
        FbmParams {
            recipe: FbmRecipe::SeededWorld,
            ..Default::default()
        },
    )
    .unwrap();
    for min in [[-17920, 3072], [-17408, 2560], [-14848, 4096]] {
        let mut error = 0f64;
        let mut range = [f64::INFINITY, f64::NEG_INFINITY];
        for z in (min[1]..min[1] + 512).step_by(64) {
            for x in (min[0]..min[0] + 512).step_by(64) {
                let b = bound_native_height(
                    &t,
                    ColumnBox {
                        min: [x, z],
                        size_cells: [64; 2],
                    },
                    BoundBudget::default(),
                )
                .unwrap();
                range[0] = range[0].min(b.height_range[0]);
                range[1] = range[1].max(b.height_range[1]);
                let point = |x: i64, z: i64| {
                    [
                        (x as f64) * VOXEL_SIZE,
                        t.column_cells(x, z).visible_top as f64 * VOXEL_SIZE,
                        z as f64 * VOXEL_SIZE,
                    ]
                };
                let center = point(x + 32, z + 32);
                let corners = [
                    point(x, z),
                    point(x, z + 64),
                    point(x + 64, z + 64),
                    point(x + 64, z),
                ];
                for i in 0..4 {
                    error = error.max(
                        b.triangle_error([center, corners[i], corners[(i + 1) % 4]])
                            .unwrap(),
                    );
                }
            }
        }
        println!(
            "min={min:?} native-source-center={} affine_error={error:.9} range={range:?}",
            t.column_cells(min[0] + 256, min[1] + 256).visible_top
        );
    }
}
