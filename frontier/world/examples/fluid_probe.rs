//! Bounded live fluid CPU probe; no renderer/GPU claims.
use frontier_world::{
    fluid::{FluidConfig, FluidRegion, FluidWorld},
    Cell,
};
use std::time::Instant;
fn main() {
    for side in [16i64, 32, 48] {
        let region = FluidRegion::new(
            Cell::default(),
            Cell {
                x: side,
                y: side,
                z: side,
            },
        );
        let mut water = FluidWorld::new(FluidConfig {
            max_loaded_cells: side.pow(3) as usize,
            ..FluidConfig::default()
        });
        let start = Instant::now();
        water
            .load_region(region, |p| if p.y >= side - 2 { 256 } else { 0 })
            .unwrap();
        let activation_ms = start.elapsed().as_secs_f64() * 1000.;
        let mut ms = Vec::new();
        let mut active_ms = Vec::new();
        let mut max_visits = 0;
        let mut transfers = 0;
        for _ in 0..160 {
            let start = Instant::now();
            let step = water.step(|_| false);
            let elapsed = start.elapsed().as_secs_f64() * 1000.;
            ms.push(elapsed);
            if step.visited_cells > 0 {
                active_ms.push(elapsed);
            }
            max_visits = max_visits.max(step.visited_cells);
            transfers += step.transfers;
            assert!(water.diagnostics().conserved());
        }
        ms.sort_by(f64::total_cmp);
        active_ms.sort_by(f64::total_cmp);
        println!("cells={} activation_ms={activation_ms:.3} tick_p50_ms={:.3} tick_p95_ms={:.3} tick_p99_ms={:.3} active_p50_ms={:.3} active_p95_ms={:.3} tick_max_ms={:.3} max_visits={max_visits} transfers={transfers} awake={} conserved={}",
            side.pow(3), ms[80], ms[152], ms[158], active_ms[active_ms.len()/2], active_ms[active_ms.len()*95/100],ms[159], water.diagnostics().awake_cells, water.diagnostics().conserved());
    }
}
