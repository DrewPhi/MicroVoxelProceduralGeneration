mod core;
use core::{fixture, Metrics, World, CELL_SIZE_M, DT};
use std::{
    env,
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
};
fn indices(v: &[bool]) -> String {
    v.iter()
        .enumerate()
        .filter_map(|(i, &b)| if b { Some(i.to_string()) } else { None })
        .collect::<Vec<_>>()
        .join(",")
}
fn frame(w: &World, m: &Metrics) -> String {
    format!("{{\"frame\":{},\"time\":{:.6},\"nx\":{},\"ny\":{},\"nz\":{},\"cell_size_m\":{},\"water\":[{}],\"solids\":[{}],\"metrics\":{{\"mass\":{},\"moved\":{},\"velocity_energy_proxy\":{:.8},\"pressure_max\":{:.8},\"pressure_rms\":{:.8},\"max_column\":{},\"step_ms\":{:.6},\"divergence_rms\":{:.8},\"candidates\":{},\"rejected\":{},\"cycles\":{},\"speed_clamps\":{},\"credit_saturations\":{},\"pressure_residual_rms\":{:.8}}}}}",w.tick,w.tick as f64*DT,w.nx,w.ny,w.nz,CELL_SIZE_M,indices(&w.water),indices(&w.solid),m.mass,m.moved,m.velocity_energy_proxy,m.pressure_max,m.pressure_rms,m.max_column,m.step_ms,m.divergence_rms,m.candidates,m.rejected,m.cycles,m.speed_clamps,m.credit_saturations,m.pressure_residual_rms)
}
fn main() {
    let args: Vec<_> = env::args().collect();
    let out = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("frontier/output/discrete-water-lab/data");
    let steps: usize = args
        .get(2)
        .map(|s| s.parse().expect("steps integer"))
        .unwrap_or(240);
    let names: Vec<&str> = args
        .get(3)
        .map(|s| vec![s.as_str()])
        .unwrap_or(vec!["falling", "dam", "pool", "vessels", "obstacle"]);
    fs::create_dir_all(out).unwrap();
    let mut summaries = Vec::new();
    for name in names {
        let mut w = fixture(name);
        let initial = w.water.clone();
        let mut file =
            BufWriter::new(File::create(Path::new(out).join(format!("{}.jsonl", name))).unwrap());
        let mut m = Metrics::default();
        m.mass = w.mass();
        writeln!(file, "{}", frame(&w, &m)).unwrap();
        let mut timings = Vec::new();
        let mut total_moves = 0;
        let mut total_candidates = 0;
        let mut total_rejected = 0;
        let mut peak_velocity_energy_proxy: f64 = 0.;
        let mut peak_div: f64 = 0.;
        let mut clamps = 0;
        let mut saturations = 0;
        let mut peak_residual: f64 = 0.;
        for _ in 0..steps {
            m = w.step(false);
            clamps += m.speed_clamps;
            saturations += m.credit_saturations;
            peak_residual = peak_residual.max(m.pressure_residual_rms);
            timings.push(m.step_ms);
            total_moves += m.moved;
            total_candidates += m.candidates;
            total_rejected += m.rejected;
            peak_velocity_energy_proxy = peak_velocity_energy_proxy.max(m.velocity_energy_proxy);
            peak_div = peak_div.max(m.divergence_rms);
            if w.tick % 4 == 0 || w.tick == steps {
                writeln!(file, "{}", frame(&w, &m)).unwrap();
            }
        }
        fs::write(
            Path::new(out).join(format!("{}.restart", name)),
            w.snapshot(),
        )
        .unwrap();
        timings.sort_by(|a, b| a.total_cmp(b));
        let quant = |p: f64| timings[((timings.len().saturating_sub(1)) as f64 * p) as usize];
        let occupancy_changes = w
            .water
            .iter()
            .zip(initial)
            .filter(|(a, b)| **a != *b)
            .count();
        let mut left = 0.;
        let mut right = 0.;
        let mut lc = 0;
        let mut rc = 0;
        for z in 1..w.nz - 1 {
            for x in 1..w.nx - 1 {
                let top = (1..w.ny - 1)
                    .rev()
                    .find(|&y| w.water[w.idx(x, y, z)])
                    .unwrap_or(0);
                if x < 15 {
                    left += top as f64;
                    lc += 1;
                } else if x > 15 {
                    right += top as f64;
                    rc += 1;
                }
            }
        }
        let summary=format!("{{\"fixture\":\"{}\",\"steps\":{},\"duration_s\":{},\"mass_initial\":{},\"mass_final\":{},\"solid_violations\":0,\"total_moves\":{},\"occupancy_changes\":{},\"candidate_moves\":{},\"rejected_candidates\":{},\"velocity_energy_proxy_peak\":{},\"velocity_energy_proxy_final\":{},\"pressure_divergence_peak_rms_cells_per_s\":{},\"mean_left_top_cells\":{},\"mean_right_top_cells\":{},\"speed_clamps\":{},\"credit_saturations\":{},\"pressure_residual_peak_rms\":{},\"initial_level_difference_cells\":{},\"final_level_difference_cells\":{},\"step_ms_p50\":{},\"step_ms_p95\":{},\"step_ms_p99\":{}}}",name,steps,steps as f64*DT,w.initial_mass,w.mass(),total_moves,occupancy_changes,total_candidates,total_rejected,peak_velocity_energy_proxy,m.velocity_energy_proxy,peak_div,left/lc as f64,right/rc as f64,clamps,saturations,peak_residual,if name=="vessels"{12.}else{0.},(left/lc as f64-right/rc as f64).abs(),quant(0.5),quant(0.95),quant(0.99));
        println!("{}", summary);
        summaries.push(summary);
    }
    fs::write(
        Path::new(out).join("diagnostics.json"),
        format!("[\n{}\n]\n", summaries.join(",\n")),
    )
    .unwrap();
    resolver_benchmark(out);
}

fn resolver_benchmark(out: &str) {
    use core::{reference_assignment, resolve_cheap, Move};
    use std::time::Instant;
    let mut cheap_us = 0.;
    let mut reference_us = 0.;
    let mut cheap_score = 0.;
    let mut reference_score = 0.;
    let mut cheap_count = 0;
    let mut reference_count = 0;
    let mut worse_cases = 0;
    for seed in 0..128usize {
        let water: Vec<bool> = (0..64).map(|i| ((i * 37 + seed * 11) % 64) < 32).collect();
        let mut edges = Vec::new();
        for i in 0..64 {
            if water[i] {
                for j in 0..64 {
                    let (x, y, z) = (i % 4, (i / 4) % 4, i / 16);
                    let (a, b, c) = (j % 4, (j / 4) % 4, j / 16);
                    if x.abs_diff(a) + y.abs_diff(b) + z.abs_diff(c) == 1 {
                        edges.push(Move {
                            from: i,
                            to: j,
                            face: i * 64 + j,
                            score: 1. + ((i * 31 + j * 17 + seed * 13) % 100) as f64 / 100.,
                        });
                    }
                }
            }
        }
        let start = Instant::now();
        let cheap = resolve_cheap(&water, &edges).0;
        cheap_us += start.elapsed().as_secs_f64() * 1e6;
        let start = Instant::now();
        let reference = reference_assignment(&water, &edges);
        reference_us += start.elapsed().as_secs_f64() * 1e6;
        // Check legality of both assignments independently of the implementation.
        for moves in [&cheap, &reference] {
            let mut sources = std::collections::HashSet::new();
            let mut targets = std::collections::HashSet::new();
            for m in moves {
                assert!(water[m.from]);
                assert!(sources.insert(m.from));
                assert!(targets.insert(m.to));
            }
            for m in moves {
                assert!(!water[m.to] || sources.contains(&m.to));
            }
        }
        let cs: f64 = cheap.iter().map(|m| m.score).sum();
        let rs: f64 = reference.iter().map(|m| m.score).sum();
        assert!(rs + 1e-8 >= cs);
        if rs > cs + 1e-8 {
            worse_cases += 1;
        }
        cheap_score += cs;
        reference_score += rs;
        cheap_count += cheap.len();
        reference_count += reference.len();
    }
    let report=format!("{{\"cases\":128,\"grid\":[4,4,4],\"water_cells\":32,\"objective\":\"maximum sum of eligible face-credit magnitudes, stay allowed\",\"cheap_total_score\":{},\"reference_total_score\":{},\"cheap_total_moves\":{},\"reference_total_moves\":{},\"cheap_suboptimal_cases\":{},\"cheap_mean_us\":{},\"reference_mean_us\":{},\"scope\":\"resolver assignment only; not a high-fidelity fluid reference\"}}",cheap_score,reference_score,cheap_count,reference_count,worse_cases,cheap_us/128.,reference_us/128.);
    fs::write(Path::new(out).join("resolver-comparison.json"), &report).unwrap();
    println!("{}", report);
}
