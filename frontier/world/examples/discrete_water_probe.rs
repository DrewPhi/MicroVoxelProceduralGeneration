//! Reproducible solver-only workload; not a game-frame benchmark.
use frontier_world::{discrete_water::DiscreteWater, Cell};
use std::time::Instant;
fn main() -> Result<(), String> {
    let c = |x, y, z| Cell { x, y, z };
    let mut water = DiscreteWater::new(c(0, 0, 0), [48, 40, 48])?;
    for z in 0..48 {
        for y in 0..40 {
            for x in 0..48 {
                let cell = c(x, y, z);
                if x == 0 || x == 47 || y == 0 || z == 0 || z == 47 {
                    water.set_solid(cell, true)?;
                } else if x < 24 && y < 8 {
                    water.set_water(cell, true)?;
                }
            }
        }
    }
    for x in 18..26 {
        for z in 21..25 {
            water.set_source(c(x, 30, z), true)?;
        }
    }
    let mut times = Vec::new();
    let mut sweeps = 0;
    let mut substeps = 0;
    let mut simulated = 0.;
    let mut deferred = 0.;
    for _ in 0..160 {
        let start = Instant::now();
        let stats = water.step(0.025);
        times.push(start.elapsed().as_secs_f64() * 1000.);
        sweeps += stats.pressure_sweeps;
        substeps += stats.substeps;
        simulated += stats.simulated_dt;
        deferred += stats.deferred_dt;
    }
    times.sort_by(f64::total_cmp);
    let bytes = water.snapshot();
    let hash = bytes.iter().fold(0xcbf29ce484222325u64, |h, &byte| {
        (h ^ byte as u64).wrapping_mul(0x100000001b3)
    });
    println!("{{\"fixture\":\"48x40x48-dam-32-sources\",\"voxel_m\":0.0625,\"mass\":{},\"tick\":{},\"sweeps\":{},\"substeps\":{},\"simulated_seconds\":{},\"deferred_seconds\":{},\"p50_ms\":{},\"p95_ms\":{},\"p99_ms\":{},\"snapshot_bytes\":{},\"snapshot_hash\":\"{hash:016x}\"}}",
        water.mass(), water.tick(), sweeps, substeps, simulated, deferred,
        times[80], times[152], times[158], bytes.len());
    Ok(())
}
