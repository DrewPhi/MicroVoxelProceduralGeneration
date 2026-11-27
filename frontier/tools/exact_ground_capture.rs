//! Build with the production frontier_world crate. The output contains exact
//! ground heights followed by a one-metre grid of canonical ground albedo.
use frontier_world::{FbmParams, FbmRecipe, FbmTerrain};
use std::{
    fs::File,
    io::{BufWriter, Write},
    time::Instant,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let [action, seed, extras @ ..] = arguments.as_slice() else {
        return Err("expected survey SEED or export SEED ORIGIN_X_M ORIGIN_Z_M CELLS FILE".into());
    };
    let seed = seed.parse()?;
    let terrain = FbmTerrain::new(
        seed,
        FbmParams {
            recipe: FbmRecipe::Eroded,
            ..FbmParams::default()
        },
    )?;
    let column = |x: i64, z: i64| terrain.column_cells(x, z);
    match (action.as_str(), extras) {
        ("survey", []) => {
            let mut views = Vec::new();
            for z in (-8192..=8192).step_by(256) {
                for x in (-8192..=8192).step_by(256) {
                    let c = column(x * 16, z * 16);
                    if c.visible_top > c.ground_top || c.ground_top < 16 * 80 {
                        continue;
                    }
                    let y = c.ground_top as f64 / 16.;
                    for [dx, dz] in [
                        [1, 0],
                        [0, 1],
                        [-1, 0],
                        [0, -1],
                        [1, 1],
                        [1, -1],
                        [-1, 1],
                        [-1, -1],
                    ] {
                        let ahead = |d, side| {
                            column((x + dx * d - dz * side) * 16, (z + dz * d + dx * side) * 16)
                                .ground_top as f64
                                / 16.
                        };
                        let near = ahead(48, 0);
                        let bottom = ahead(105, 0);
                        let far = ahead(158, 0);
                        let shoulder = ahead(120, 60).max(ahead(120, -60));
                        let score = (shoulder - bottom).clamp(0., 120.)
                            + (far - bottom).clamp(0., 105.) * 0.65
                            + (y - bottom).clamp(0., 55.) * 0.55
                            - (near - y - 14.).max(0.) * 2.
                            - (y - bottom - 150.).max(0.);
                        views.push((
                            score,
                            [x as f64, y + 15., z as f64],
                            [
                                (x + dx * 150) as f64,
                                bottom * 0.50 + far * 0.50,
                                (z + dz * 150) as f64,
                            ],
                        ));
                    }
                }
            }
            views.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
            for (score, eye, target) in views.into_iter().take(10) {
                println!("{score:.2} eye={eye:?} target={target:?}");
            }
        }
        ("export", [x, z, cells, path]) => {
            let x: i64 = x.parse::<i64>()? * 16;
            let z: i64 = z.parse::<i64>()? * 16;
            let cells: usize = cells.parse()?;
            if ![2048, 4096, 8192].contains(&cells) {
                return Err("expected 2048, 4096 or 8192 cells".into());
            }
            let start = Instant::now();
            let mut heights = vec![0f32; cells * cells];
            let rows = cells.div_ceil(
                std::thread::available_parallelism()
                    .map_or(1, usize::from)
                    .min(8),
            );
            std::thread::scope(|scope| {
                for (part, data) in heights.chunks_mut(rows * cells).enumerate() {
                    let column = &column;
                    scope.spawn(move || {
                        for (i, height) in data.iter_mut().enumerate() {
                            let c = column(
                                x + (i % cells) as i64,
                                z + (part * rows + i / cells) as i64,
                            );
                            *height = c.ground_top as f32 / 16.;
                        }
                    });
                }
            });
            let mut file = BufWriter::new(File::create(path)?);
            for h in heights {
                file.write_all(&h.to_le_bytes())?;
            }
            for iz in (8..cells).step_by(16) {
                for ix in (8..cells).step_by(16) {
                    let c = column(x + ix as i64, z + iz as i64);
                    let rgb = c
                        .ground_surface
                        .color()
                        .map(|v| (v.clamp(0., 1.) * 255.).round() as u32);
                    let packed = (rgb[0] | rgb[1] << 8 | rgb[2] << 16) as f32;
                    file.write_all(&packed.to_le_bytes())?;
                }
            }
            file.flush()?;
            eprintln!(
                "Exported {cells}² exact columns and albedo in {:.3} s",
                start.elapsed().as_secs_f64()
            );
        }
        _ => {
            return Err(
                "expected survey SEED or export SEED ORIGIN_X_M ORIGIN_Z_M CELLS FILE".into(),
            )
        }
    }
    Ok(())
}
