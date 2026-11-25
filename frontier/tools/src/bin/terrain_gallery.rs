//! CPU heightfield comparison, not an engine rendering benchmark.
//! cargo run --release -p frontier-tools --bin terrain_gallery -- output/regional-terrain/cpu
use frontier_world::{FbmParams, FbmRecipe, FbmTerrain};
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::PathBuf,
    time::Instant,
};

const N: usize = 257;
fn percentile(v: &mut [f64], q: f64) -> f64 {
    v.sort_unstable_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * q).round() as usize]
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "output/regional-terrain/cpu".into()),
    );
    fs::create_dir_all(&out)?;
    fs::write(out.join("method.txt"), format!(
        "Pure CPU canonical heightmap preview; not engine rendering or frame timing.\nParameters: {:?} (recipe varies).\nSeeds: 7,19,83. Each square centered at (0,0), 257x257 samples.\nSpans: 8000m (31.25m grid), 2000m (7.8125m grid).\nSlope: atan(hypot(h(x+2)-h(x-2),h(z+2)-h(z-2))/4).\nLaplacian: (h(x+2)+h(x-2)+h(z+2)+h(z-2)-4h)/4 in inverse meters.\nDerivatives include native 0.0625m quantization.\nElapsed evaluation includes five height queries/sample and row allocation, excludes CSV IO and sorting; single run, no frame-time claim.\n", FbmParams::default()
    ))?;
    let mut metrics = BufWriter::new(File::create(out.join("metrics.csv"))?);
    writeln!(metrics, "recipe,source_version,seed,span_m,grid_spacing_m,samples,height_min_m,height_max_m,slope_p50_deg,slope_p95_deg,slope_max_deg,abs_laplacian_p95_per_m,eval_ms,eval_ns_per_height,peak_x_m,peak_z_m")?;
    for (name, recipe) in [
        ("legacy", FbmRecipe::Legacy),
        ("regional", FbmRecipe::Regional),
        ("regional_unwarped", FbmRecipe::RegionalUnwarped),
    ] {
        for seed in [7, 19, 83] {
            let terrain = FbmTerrain::new(
                seed,
                FbmParams {
                    recipe,
                    ..Default::default()
                },
            )?;
            for span in [8000.0, 2000.0] {
                let spacing = span / (N - 1) as f64;
                // Five canonical height queries per sample, fixed 2m derivative radius.
                // Time only evaluation, excluding CSV output and sorting.
                let start = Instant::now();
                let mut rows = Vec::with_capacity(N * N);
                for iz in 0..N {
                    for ix in 0..N {
                        let x = ix as f64 * spacing - span / 2.0;
                        let z = iz as f64 * spacing - span / 2.0;
                        let h = terrain.height_m(x, z);
                        let xp = terrain.height_m(x + 2.0, z);
                        let xm = terrain.height_m(x - 2.0, z);
                        let zp = terrain.height_m(x, z + 2.0);
                        let zm = terrain.height_m(x, z - 2.0);
                        let slope = ((xp - xm).hypot(zp - zm) / 4.0).atan().to_degrees();
                        let laplacian = (xp + xm + zp + zm - 4.0 * h) / 4.0;
                        rows.push([x, z, h, slope, laplacian]);
                    }
                }
                let elapsed = start.elapsed();
                let mut csv = BufWriter::new(File::create(
                    out.join(format!("{name}_{seed}_{}m.csv", span as u32)),
                )?);
                writeln!(csv, "x_m,z_m,height_m,slope_deg,laplacian_per_m")?;
                for r in &rows {
                    writeln!(csv, "{},{},{},{:.6},{:.6}", r[0], r[1], r[2], r[3], r[4])?;
                }
                let low = rows.iter().map(|r| r[2]).fold(f64::INFINITY, f64::min);
                let peak = rows.iter().max_by(|a, b| a[2].total_cmp(&b[2])).unwrap();
                let mut slopes: Vec<_> = rows.iter().map(|r| r[3]).collect();
                let p50 = percentile(&mut slopes, 0.5);
                let p95 = percentile(&mut slopes, 0.95);
                let max = *slopes.last().unwrap();
                let mut curvature: Vec<_> = rows.iter().map(|r| r[4].abs()).collect();
                let curvature95 = percentile(&mut curvature, 0.95);
                writeln!(metrics, "{name},{},{seed},{span},{spacing},{},{low},{},{p50},{p95},{max},{curvature95},{:.3},{:.1},{},{}", terrain.source_version(), rows.len(), peak[2], elapsed.as_secs_f64()*1000.0, elapsed.as_nanos() as f64 / (rows.len()*5) as f64, peak[0], peak[1])?;
                println!("{name} seed={seed} span={span:.0}m height={low:.1}..{:.1} slope p50/p95={p50:.1}/{p95:.1}deg eval={:.0}ms peak=({},{})", peak[2], elapsed.as_secs_f64()*1000.0, peak[0], peak[1]);
            }
        }
    }
    println!("CPU canonical quantized height samples; slopes/Laplacians use +/-2m differences. Timing includes five height queries/sample and row allocation, excludes IO; not frame time. Output: {}", out.display());
    Ok(())
}
