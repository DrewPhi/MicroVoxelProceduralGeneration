//! Same physical patch at four native grids, with bounded column caches.
use microvoxel::{
    generation::{Generator, ProceduralTerrain},
    GridScale,
};
use std::{fmt::Write as _, time::Instant};
fn main() -> microvoxel::Result<()> {
    let start = Instant::now();
    let source = ProceduralTerrain::new(42);
    let regional = start.elapsed().as_secs_f64() * 1000.;
    let mut report = String::from(
        "voxel_m,physical_edge_m,columns,query_ms,cached_columns,regional_bytes,checksum\n",
    );
    for denominator in [4, 16, 100, 1000] {
        let grid = GridScale::new(1, denominator)?;
        let generator = source.at_grid(grid);
        let edge = 0.5;
        let count = (edge / grid.meters()).round() as i64;
        let ox = grid.cells(4250.).unwrap();
        let oz = grid.cells(6250.).unwrap();
        let start = Instant::now();
        let mut checksum = 0u64;
        for z in 0..count {
            for x in 0..count {
                let c = generator
                    .column_hint(ox + x, oz + z)
                    .ok_or("unknown benchmark column")?;
                checksum = checksum.wrapping_add((c.ground as u64).rotate_left((x % 31) as u32));
            }
        }
        writeln!(
            report,
            "{},0.5,{}, {:.3},{},{},{}",
            grid.meters(),
            count * count,
            start.elapsed().as_secs_f64() * 1000.,
            generator.cached_columns(),
            generator.regional_memory_bytes(),
            checksum
        )?;
        assert!(generator.cached_columns() <= 32768);
    }
    std::fs::create_dir_all("output/foundation")?;
    std::fs::write("output/foundation/scales.csv", &report)?;
    println!("Shared regional build: {regional:.2} ms. Column queries cover 0.5 m x 0.5 m; this is not an interactive FPS benchmark.\n{report}");
    Ok(())
}
