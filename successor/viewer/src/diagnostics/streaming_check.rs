//! Repeatable real walking/sprinting/turning test, including visible detail lag.
use crate::{gpu::Gpu, runtime::Runtime, settings::Settings};
use microvoxel::{generation::Terrain, World};
use std::{
    fmt::Write as _,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::keyboard::KeyCode;

#[derive(Debug, PartialEq)]
struct Config {
    size: [u32; 2],
    ticks: usize,
    renderers: Vec<bool>,
    presets: Vec<usize>,
    output: PathBuf,
}
impl Config {
    fn from_env() -> Result<Self, String> {
        Self::parse(|name| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(_) => Err(format!("{name} must contain valid UTF-8")),
        })
    }
    fn parse(mut get: impl FnMut(&str) -> Result<Option<String>, String>) -> Result<Self, String> {
        let mut config = Self {
            size: [960, 540],
            ticks: 600,
            renderers: vec![false, true],
            presets: vec![0, 1],
            output: PathBuf::from("output/streaming"),
        };
        if let Some(value) = get("MICROVOXEL_STREAM_SIZE")? {
            let invalid = || {
                "MICROVOXEL_STREAM_SIZE must be WIDTHxHEIGHT with positive u32 dimensions (for example 1920x1080)".to_string()
            };
            let (w, h) = value.split_once('x').ok_or_else(invalid)?;
            config.size = [
                w.parse().map_err(|_| invalid())?,
                h.parse().map_err(|_| invalid())?,
            ];
            if config.size.contains(&0) {
                return Err(invalid());
            }
        }
        if let Some(value) = get("MICROVOXEL_STREAM_TICKS")? {
            config.ticks = value.parse().map_err(|_| {
                "MICROVOXEL_STREAM_TICKS must be an integer of at least 5".to_string()
            })?;
            if config.ticks < 5 {
                return Err("MICROVOXEL_STREAM_TICKS must be an integer of at least 5".into());
            }
        }
        if let Some(value) = get("MICROVOXEL_STREAM_RENDERER")? {
            config.renderers = match value.as_str() {
                "mesh" => vec![false],
                "ray" => vec![true],
                "both" => vec![false, true],
                _ => return Err("MICROVOXEL_STREAM_RENDERER must be mesh, ray, or both".into()),
            };
        }
        if let Some(value) = get("MICROVOXEL_STREAM_PRESET")? {
            config.presets = match value.as_str() {
                "0" => vec![0],
                "1" => vec![1],
                "2" => vec![2],
                _ => return Err("MICROVOXEL_STREAM_PRESET must be 0, 1, or 2".into()),
            };
        }
        if let Some(value) = get("MICROVOXEL_STREAM_OUTPUT")? {
            if value.is_empty() {
                return Err("MICROVOXEL_STREAM_OUTPUT must be a nonempty directory path".into());
            }
            config.output = PathBuf::from(value);
        }
        Ok(config)
    }
    fn phase_counts(&self) -> [usize; 3] {
        let walk = (self.ticks as u128 * 2 / 5) as usize;
        let end_sprint = (self.ticks as u128 * 4 / 5) as usize;
        [walk, end_sprint - walk, self.ticks - end_sprint]
    }
    fn phase(&self, tick: usize) -> usize {
        let counts = self.phase_counts();
        if tick < counts[0] {
            0
        } else if tick < counts[0] + counts[1] {
            1
        } else {
            2
        }
    }
    fn capture_ticks(&self) -> Vec<usize> {
        let [walk, sprint, turn] = self.phase_counts();
        let end_sprint = walk + sprint;
        let mut ticks = vec![
            0,
            self.ticks / 5,
            walk - 1,
            (self.ticks as u128 * 3 / 5) as usize,
            end_sprint - 1,
            end_sprint + turn / 2,
            self.ticks - 1,
        ];
        ticks.sort_unstable();
        ticks.dedup();
        ticks
    }
}

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

fn record_gpu_sample(
    latest: Option<crate::gpu_timing::Sample>,
    last_frame: &mut Option<u64>,
    samples: &mut Samples,
) -> Option<crate::gpu_timing::Sample> {
    let sample = latest.filter(|s| Some(s.frame) != *last_frame)?;
    *last_frame = Some(sample.frame);
    samples.0.push(sample.pass_ms);
    Some(sample)
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
    let config = Config::from_env()?;
    std::fs::create_dir_all(&config.output)?;
    let phase_counts = config.phase_counts();
    let capture_ticks = config.capture_ticks();
    // Optional visual evidence is a separate run: screenshot readback changes
    // pacing, so do not compare its timings to the normal benchmark route.
    let capture = std::env::var_os("MICROVOXEL_STREAM_CAPTURE").is_some();
    if capture {
        std::fs::create_dir_all(config.output.join("motion"))?;
    }
    for &preset in &config.presets {
        for &ray in &config.renderers {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
            let gpu =
                pollster::block_on(Gpu::new(&instance, None, config.size[0], config.size[1]))?;
            let mut rt = Runtime::new(gpu, world.clone(), 512, 0)?;
            let settings = Settings {
                ray,
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
            let mut gpu_pass = Samples::default();
            rt.gpu.timing.poll(&rt.gpu.device);
            let mut last_gpu_frame = rt.gpu.timing.latest().map(|s| s.frame);
            let initial_dropped = rt.gpu.timing.dropped_samples();
            let initial_mapping_failures = rt.gpu.timing.map_failures();
            let fixed_visibility = rt.gpu.visibility_limit;
            let mut peak_committed = rt.gpu.managed_committed_bytes();
            let mut sampled_coverage_misses = 0usize;
            let mut phases: [Samples; 3] = std::array::from_fn(|_| Samples::default());
            let mut peak_rss = resident_kib();
            let mut csv = String::from("tick,phase,update_ms,submit_ms,gpu_wait_ms,work_ms,native_ready,pending,resident_terrain_bytes,ray_arena_bytes,rss_kib,managed_committed_bytes,visibility_m,gpu_timestamp_frame,gpu_pass_ms,sampled_cover_ready\n");
            let mut gaps = [0usize; 3];
            let mut policy_misses = [0usize; 3];
            let mut longest = 0.;
            let mut gap_time = 0.;
            let origin = rt.player.root;
            let mut traveled = 0.;
            let mut before = origin;
            let mut pending_max = 0;
            let route_start = Instant::now();
            for i in 0..config.ticks {
                let start = Instant::now();
                let phase = config.phase(i);
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
                if rt.gpu.visibility_limit.to_bits() != fixed_visibility.to_bits() {
                    return Err(format!(
                        "streaming horizon changed at tick {i}: {fixed_visibility} to {}",
                        rt.gpu.visibility_limit
                    )
                    .into());
                }
                if rt.cover.is_empty() || rt.cover.iter().any(|key| !rt.gpu.ready(key, ray)) {
                    return Err(format!(
                        "published cover contains a non-resident tile at tick {i}"
                    )
                    .into());
                }
                peak_committed = peak_committed.max(rt.gpu.managed_committed_bytes());
                let p = rt.player.root;
                traveled +=
                    ((p.x - before.x) as f64).hypot((p.z - before.z) as f64) * rt.gpu.grid.cell_m();
                before = p;
                let readiness = (0..9).fold([true; 3], |mut ready, sample| {
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
                    ready[2] &= rt.active.at(x, z).is_some_and(|tile| {
                        rt.cover
                            .iter()
                            .any(|key| key.tile == tile && rt.gpu.ready(key, ray))
                    });
                    ready[1] &= rt
                        .desired
                        .at(x, z)
                        .is_some_and(|key| key.level == rt.gpu.grid.finest);
                    ready
                });
                let ready = readiness[0];
                if !readiness[2] {
                    sampled_coverage_misses += 1;
                }
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
                rt.gpu.timing.poll(&rt.gpu.device);
                let gpu_sample =
                    record_gpu_sample(rt.gpu.timing.latest(), &mut last_gpu_frame, &mut gpu_pass);
                let committed = rt.gpu.managed_committed_bytes();
                peak_committed = peak_committed.max(committed);
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
                    "{i},{phase},{:.6},{:.6},{:.6},{:.6},{ready},{},{},{},{},{committed},{fixed_visibility},{},{},{}",
                    update_time.as_secs_f64() * 1000.,
                    submit_time.as_secs_f64() * 1000.,
                    wait_time.as_secs_f64() * 1000.,
                    work_time.as_secs_f64() * 1000.,
                    rt.pending_jobs(),
                    rt.gpu.bytes,
                    rt.gpu.ray_allocated_bytes(),
                    rss.map_or(String::new(), |v| v.to_string()),
                    gpu_sample.map_or(String::new(),|s|s.frame.to_string()),
                    gpu_sample.map_or(String::new(),|s|format!("{:.6}",s.pass_ms)),
                    readiness[2]
                )?;
                if capture && capture_ticks.contains(&i) {
                    let mode = if ray { "ray" } else { "mesh" };
                    rt.gpu.screenshot(
                        &texture,
                        &config
                            .output
                            .join("motion")
                            .join(format!("{mode}_{preset}_{i:03}.png")),
                    )?;
                }
                if let Some(rest) = Duration::from_secs_f64(1. / 60.).checked_sub(start.elapsed()) {
                    std::thread::sleep(rest);
                }
            }
            rt.clear_controls();
            let mode = if ray { "ray" } else { "mesh" };
            let mut report=format!("{mode} preset {preset} ({} MiB budget): initial {initial_secs:.2}s; traveled {traveled:.2}m; native-detail misses within 3m: walk {}/{}, sprint {}/{}, turn {}/{}; longest {:.3}s; work ms p50/p95/p99 {:?}; max pending {pending_max}; resident {:.2} MiB; ray arena allocated {:.2} MiB; culled {}\n",settings.memory_mib,gaps[0],phase_counts[0],gaps[1],phase_counts[1],gaps[2],phase_counts[2],longest,frame.percentiles(),rt.gpu.bytes as f64/1048576.,rt.gpu.ray_allocated_bytes() as f64/1048576.,rt.gpu.culled);
            writeln!(report,"CPU update ms {:?}; CPU draw/submit ms {:?}; GPU wait ms {:?}; work by walk/sprint/turn {:?}; sampled peak process RSS MiB {:?}. GPU wait is synchronization time, not isolated GPU execution time. Work excludes diagnostic collision/detail checks and pacing sleep; RSS is sampled every 30 ticks and includes earlier cases in this process.",update.percentiles(),submit.percentiles(),wait.percentiles(),phases.map(|p|p.percentiles()),peak_rss.map(|v|v as f64/1024.))?;
            writeln!(report,"Route: {} ticks ({:.2}s simulated, {:.2}s elapsed), {}x{}; maximum work {:.3}ms; fixed horizon {:.1}m; sampled peak managed terrain/voxel buffer capacity {:.2}MiB (driver memory not measured). GPU pass timestamp samples {} p50/p95/p99 {:?}; supported {}; dropped {}; mapping failures {}. GPU timestamps measure terrain-pass execution, excluding presentation/readback. Capture enabled: {capture}.",config.ticks,config.ticks as f64/60.,route_start.elapsed().as_secs_f64(),config.size[0],config.size[1],frame.0.iter().copied().fold(0.,f64::max),fixed_visibility,peak_committed as f64/1048576.,gpu_pass.0.len(),gpu_pass.percentiles(),rt.gpu.timing.available(),rt.gpu.timing.dropped_samples()-initial_dropped,rt.gpu.timing.map_failures()-initial_mapping_failures)?;
            writeln!(report,"Published-cover GPU residency and fixed horizon checked every tick; nine-point logical cover misses {sampled_coverage_misses}/{}. These checks do not establish raster boundary closure or zero visible holes.",config.ticks)?;
            writeln!(report,"Of these misses, frames with deliberately coarse desired detail (walk/sprint/turn): {policy_misses:?}")?;
            writeln!(
                report,
                "Maximum accepted planner latency {:.2} ms",
                rt.max_plan_time.as_secs_f64() * 1000.
            )?;
            print!("{report}");
            std::fs::write(config.output.join(format!("{mode}_{preset}.txt")), report)?;
            std::fs::write(config.output.join(format!("{mode}_{preset}.csv")), csv)?;
            if config.renderers.len() > 1 {
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
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamp_reports_exclude_warmup_and_repeated_latest_values() {
        let mut samples = Samples::default();
        let mut last = Some(32);
        assert!(record_gpu_sample(None, &mut last, &mut samples).is_none());
        for _ in 0..20 {
            assert!(record_gpu_sample(
                Some(crate::gpu_timing::Sample {
                    frame: 32,
                    pass_ms: 99.
                }),
                &mut last,
                &mut samples
            )
            .is_none());
        }
        assert!(record_gpu_sample(
            Some(crate::gpu_timing::Sample {
                frame: 64,
                pass_ms: 2.5
            }),
            &mut last,
            &mut samples
        )
        .is_some());
        for _ in 0..20 {
            record_gpu_sample(
                Some(crate::gpu_timing::Sample {
                    frame: 64,
                    pass_ms: 2.5,
                }),
                &mut last,
                &mut samples,
            );
        }
        assert_eq!(samples.0, [2.5]);
    }
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

#[cfg(test)]
mod config_tests {
    use super::*;
    fn parse(entries: &[(&str, &str)]) -> Result<Config, String> {
        Config::parse(|name| {
            Ok(entries
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string()))
        })
    }
    #[test]
    fn defaults_preserve_route_cases_phases_and_capture_indices() {
        let config = parse(&[]).unwrap();
        assert_eq!(config.size, [960, 540]);
        assert_eq!(config.ticks, 600);
        assert_eq!(config.renderers, [false, true]);
        assert_eq!(config.presets, [0, 1]);
        assert_eq!(config.output, PathBuf::from("output/streaming"));
        assert_eq!(config.phase_counts(), [240, 240, 120]);
        assert_eq!(config.capture_ticks(), [0, 120, 239, 360, 479, 540, 599]);
        assert_eq!(
            [
                config.phase(0),
                config.phase(239),
                config.phase(240),
                config.phase(479),
                config.phase(480),
                config.phase(599)
            ],
            [0, 0, 1, 1, 2, 2]
        );
    }
    #[test]
    fn requested_1080p_two_minute_single_case_scales_denominators_and_captures() {
        let config = parse(&[
            ("MICROVOXEL_STREAM_SIZE", "1920x1080"),
            ("MICROVOXEL_STREAM_TICKS", "7200"),
            ("MICROVOXEL_STREAM_RENDERER", "mesh"),
            ("MICROVOXEL_STREAM_PRESET", "2"),
            ("MICROVOXEL_STREAM_OUTPUT", "/tmp/stream-test"),
        ])
        .unwrap();
        assert_eq!(config.size, [1920, 1080]);
        assert_eq!(config.ticks, 7200);
        assert_eq!(config.phase_counts(), [2880, 2880, 1440]);
        assert_eq!(config.renderers, [false]);
        assert_eq!(config.presets, [2]);
        assert_eq!(config.output, PathBuf::from("/tmp/stream-test"));
        assert_eq!(
            config.capture_ticks(),
            [0, 1440, 2879, 4320, 5759, 6480, 7199]
        );
        assert_eq!(
            parse(&[("MICROVOXEL_STREAM_RENDERER", "ray")])
                .unwrap()
                .renderers,
            [true]
        );
    }
    #[test]
    fn invalid_environment_values_identify_the_variable() {
        for (name, value) in [
            ("MICROVOXEL_STREAM_SIZE", "1920X1080"),
            ("MICROVOXEL_STREAM_SIZE", "0x540"),
            ("MICROVOXEL_STREAM_SIZE", "1920x1080x1"),
            ("MICROVOXEL_STREAM_TICKS", "0"),
            ("MICROVOXEL_STREAM_TICKS", "4"),
            ("MICROVOXEL_STREAM_TICKS", "lots"),
            ("MICROVOXEL_STREAM_RENDERER", "software"),
            ("MICROVOXEL_STREAM_PRESET", "3"),
            ("MICROVOXEL_STREAM_OUTPUT", ""),
        ] {
            assert!(
                parse(&[(name, value)]).unwrap_err().contains(name),
                "{name}={value}"
            );
        }
    }
    #[test]
    fn odd_and_minimum_tick_counts_keep_every_tick_in_one_nonempty_phase() {
        for ticks in [5, 6, 7, 601, 7201] {
            let config = parse(&[("MICROVOXEL_STREAM_TICKS", &ticks.to_string())]).unwrap();
            let mut actual = [0; 3];
            for tick in 0..ticks {
                actual[config.phase(tick)] += 1;
            }
            assert_eq!(actual, config.phase_counts());
            assert!(actual.iter().all(|&n| n > 0));
            assert!(config.capture_ticks().iter().all(|&tick| tick < ticks));
        }
    }
}
