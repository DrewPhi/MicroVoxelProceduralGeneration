//! Repeatable real walking/sprinting/turning test, including visible detail lag.
use crate::{gpu::Gpu, runtime::Runtime, settings::Settings};
use microvoxel::{generation::Terrain, World};
use std::{
    fmt::Write as _,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::keyboard::KeyCode;

// Keep every diagnostic sample, including stalls >= 1 second. The HUD's rolling
// window is useful in play, but would silently drop part of a benchmark route.
#[derive(Default)]
struct Samples(Vec<f64>);
impl Samples {
    fn push(&mut self, duration: Duration) {
        self.0.push(duration.as_secs_f64() * 1000.);
    }
    fn percentiles(&self) -> [f64; 3] {
        if self.0.is_empty() {
            return [0.; 3];
        }
        let mut sorted = self.0.clone();
        sorted.sort_by(f64::total_cmp);
        [0.5, 0.95, 0.99].map(|p| sorted[((sorted.len() - 1) as f64 * p).round() as usize])
    }
}

// Linux RSS includes CPU allocations and some driver mappings, not all GPU
// allocations. Other platforms leave the CSV field empty rather than guessing.
fn resident_kib() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmRSS:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
}

pub fn run(world: Arc<World<Terrain>>) -> microvoxel::Result<()> {
    std::fs::create_dir_all("output/streaming")?;
    // Optional visual evidence is a separate run: screenshot readback changes
    // pacing, so do not compare its timings to the normal benchmark route.
    let capture = std::env::var_os("MICROVOXEL_STREAM_CAPTURE").is_some();
    if capture {
        std::fs::create_dir_all("output/streaming/motion")?;
    }
    for preset in [0, 1] {
        for ray in [false, true] {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
            let gpu = pollster::block_on(Gpu::new(&instance, None, 960, 540))?;
            let mut rt = Runtime::new(gpu, world.clone(), 512, 0)?;
            let settings = Settings {
                ray,
                memory_mib: if preset == 0 { 96 } else { 256 },
                ..Settings::preset(preset)
            };
            rt.apply_settings(settings);
            let initial = Instant::now();
            crate::wait_for_cover(&mut rt, Instant::now() + Duration::from_secs(120))?;
            let initial_secs = initial.elapsed().as_secs_f64();
            let texture = rt.gpu.offscreen();
            let view = texture.create_view(&Default::default());
            let mut frame = Samples::default();
            let mut update = Samples::default();
            let mut submit = Samples::default();
            let mut wait = Samples::default();
            let mut phases: [Samples; 3] = std::array::from_fn(|_| Samples::default());
            let mut peak_rss = resident_kib();
            let mut csv = String::from("tick,phase,update_ms,submit_ms,gpu_wait_ms,work_ms,native_ready,pending,resident_terrain_bytes,ray_arena_bytes,rss_kib\n");
            let mut gaps = [0usize; 3];
            let mut policy_misses = [0usize; 3];
            let mut longest = 0.;
            let mut gap_time = 0.;
            let origin = rt.player.root;
            let mut traveled = 0.;
            let mut before = origin;
            let mut pending_max = 0;
            for i in 0..600 {
                let start = Instant::now();
                let phase = if i < 240 {
                    0
                } else if i < 480 {
                    1
                } else {
                    2
                };
                rt.camera.keys.clear();
                if phase < 2 {
                    rt.camera.keys.insert(KeyCode::KeyW);
                }
                if phase == 1 {
                    rt.camera.keys.insert(KeyCode::ShiftLeft);
                }
                if phase == 2 {
                    rt.camera.yaw += std::f64::consts::PI / 120.;
                }
                let update_start = Instant::now();
                rt.update(1. / 60.);
                let update_time = update_start.elapsed();
                if let Some(e) = &rt.last_error {
                    return Err(e.clone().into());
                }
                if !rt.player.clear(&rt.world) {
                    return Err("player collided during streaming test".into());
                }
                let p = rt.player.root;
                traveled +=
                    ((p.x - before.x) as f64).hypot((p.z - before.z) as f64) * rt.gpu.grid.cell_m();
                before = p;
                let readiness = (0..9).fold([true; 2], |mut ready, sample| {
                    let (dx, dz) = if sample == 0 {
                        (0., 0.)
                    } else {
                        let a = (sample - 1) as f64 * std::f64::consts::TAU / 8.;
                        (a.cos() * 3., a.sin() * 3.)
                    };
                    let x = ((rt.camera.eye()[0] + dx) / rt.gpu.grid.cell_m()) as i64;
                    let z = ((rt.camera.eye()[2] + dz) / rt.gpu.grid.cell_m()) as i64;
                    ready[0] &= rt
                        .active
                        .at(x, z)
                        .is_some_and(|key| key.level == rt.gpu.grid.finest);
                    ready[1] &= rt
                        .desired
                        .at(x, z)
                        .is_some_and(|key| key.level == rt.gpu.grid.finest);
                    ready
                });
                let ready = readiness[0];
                if !ready && !readiness[1] {
                    policy_misses[phase] += 1;
                }
                if !ready {
                    gaps[phase] += 1;
                    gap_time += 1. / 60.;
                    longest = f64::max(longest, gap_time);
                } else {
                    gap_time = 0.;
                }
                pending_max = pending_max.max(rt.pending_jobs());
                let submit_start = Instant::now();
                rt.draw(&view);
                let submit_time = submit_start.elapsed();
                let wait_start = Instant::now();
                rt.gpu.device.poll(wgpu::Maintain::Wait);
                let wait_time = wait_start.elapsed();
                let work_time = update_time + submit_time + wait_time;
                update.push(update_time);
                submit.push(submit_time);
                wait.push(wait_time);
                frame.push(work_time);
                phases[phase].push(work_time);
                let rss = (i % 30 == 0).then(resident_kib).flatten();
                if let Some(rss) = rss {
                    peak_rss = Some(peak_rss.unwrap_or(0).max(rss));
                }
                writeln!(
                    csv,
                    "{i},{phase},{:.6},{:.6},{:.6},{:.6},{ready},{},{},{},{}",
                    update_time.as_secs_f64() * 1000.,
                    submit_time.as_secs_f64() * 1000.,
                    wait_time.as_secs_f64() * 1000.,
                    work_time.as_secs_f64() * 1000.,
                    rt.pending_jobs(),
                    rt.gpu.bytes,
                    rt.gpu.ray_allocated_bytes(),
                    rss.map_or(String::new(), |v| v.to_string())
                )?;
                if capture && [0, 120, 239, 360, 479, 540, 599].contains(&i) {
                    let mode = if ray { "ray" } else { "mesh" };
                    rt.gpu.screenshot(
                        &texture,
                        std::path::Path::new(&format!(
                            "output/streaming/motion/{mode}_{preset}_{i:03}.png"
                        )),
                    )?;
                }
                if let Some(rest) = Duration::from_secs_f64(1. / 60.).checked_sub(start.elapsed()) {
                    std::thread::sleep(rest);
                }
            }
            rt.clear_controls();
            let mode = if ray { "ray" } else { "mesh" };
            let mut report=format!("{mode} preset {preset}: initial {initial_secs:.2}s; traveled {traveled:.2}m; native-detail misses within 3m: walk {}/240, sprint {}/240, turn {}/120; longest {:.3}s; work ms p50/p95/p99 {:?}; max pending {pending_max}; resident {:.2} MiB; ray arena allocated {:.2} MiB; culled {}\n",gaps[0],gaps[1],gaps[2],longest,frame.percentiles(),rt.gpu.bytes as f64/1048576.,rt.gpu.ray_allocated_bytes() as f64/1048576.,rt.gpu.culled);
            writeln!(report,"CPU update ms {:?}; CPU draw/submit ms {:?}; GPU wait ms {:?}; work by walk/sprint/turn {:?}; sampled peak process RSS MiB {:?}. GPU wait is synchronization time, not isolated GPU execution time. Work excludes diagnostic collision/detail checks and pacing sleep; RSS is sampled every 30 ticks and includes earlier cases in this process.",update.percentiles(),submit.percentiles(),wait.percentiles(),phases.map(|p|p.percentiles()),peak_rss.map(|v|v as f64/1024.))?;
            writeln!(report,"Of these misses, frames with deliberately coarse desired detail (walk/sprint/turn): {policy_misses:?}")?;
            writeln!(
                report,
                "Maximum accepted planner latency {:.2} ms",
                rt.max_plan_time.as_secs_f64() * 1000.
            )?;
            print!("{report}");
            std::fs::write(format!("output/streaming/{mode}_{preset}.txt"), report)?;
            std::fs::write(format!("output/streaming/{mode}_{preset}.csv"), csv)?;
            // Switching backends must preserve visible coverage and eventually settle.
            rt.apply_settings(Settings {
                ray: !ray,
                ..settings
            });
            crate::wait_for_cover(&mut rt, Instant::now() + Duration::from_secs(120))?;
            rt.draw(&view);
            rt.gpu.device.poll(wgpu::Maintain::Wait);
            println!(
                "{mode}: switch to {} settled",
                if ray { "mesh" } else { "ray" }
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_keep_the_whole_route_and_long_stalls() {
        let mut samples = Samples::default();
        for _ in 0..600 {
            samples.push(Duration::from_millis(5));
        }
        for _ in 0..10 {
            samples.push(Duration::from_secs(2));
        }
        assert_eq!(samples.0.len(), 610);
        assert_eq!(samples.percentiles(), [5., 5., 2000.]);
    }
}
