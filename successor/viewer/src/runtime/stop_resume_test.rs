//! Asynchronous world-switch, sprint, idle, resume regression.
use super::*;
#[test]
#[ignore = "real GPU stop/wait/resume streaming regression"]
fn stop_wait_resume_streaming() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let gpu = pollster::block_on(Gpu::new(&instance, None, 1280, 720)).unwrap();
    let seed = std::env::var("MICROVOXEL_STOP_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8957990822362167096);
    let mut rt = Runtime::new(gpu, Arc::new(World::new(Terrain::new(42))), 8192, 0).unwrap();
    rt.apply_settings(crate::settings::Settings::default());
    crate::wait_for_cover(&mut rt, Instant::now() + Duration::from_secs(120)).unwrap();
    let mut gpu = rt.into_gpu();
    gpu.clear_world();
    let mut rt = Runtime::new(gpu, Arc::new(World::new(Terrain::new(seed))), 8192, 0).unwrap();
    rt.apply_settings(crate::settings::Settings::default());
    let texture = rt.gpu.offscreen();
    let view = texture.create_view(&Default::default());
    let mut misses = [0usize; 3];
    let mut changes = 0;
    for tick in 0..3000 {
        let start = Instant::now();
        let phase = if tick < 600 {
            0
        } else if tick < 1800 {
            1
        } else {
            2
        };
        rt.camera.keys.clear();
        if phase != 1 {
            rt.camera.keys.insert(KeyCode::KeyW);
            rt.camera.keys.insert(KeyCode::ShiftLeft);
        }
        let idle_target = if tick == 1800 {
            rt.target.as_ref().map(|(_, keys)| keys.clone())
        } else {
            None
        };
        rt.update(1. / 60.);
        if let Some(old) = idle_target {
            assert!(
                rt.target.as_ref().is_none_or(|(_, keys)| *keys != old),
                "idle target survived movement restart"
            );
        }
        assert!(rt.last_error.is_none(), "{:?}", rt.last_error);
        let p = rt.player.root;
        let ready = (0..9).all(|i| {
            let a = (i as f64) * std::f64::consts::TAU / 8.;
            let (dx, dz) = if i == 8 {
                (0, 0)
            } else {
                (
                    (a.cos() * 3. / rt.gpu.grid.cell_m()).round() as i64,
                    (a.sin() * 3. / rt.gpu.grid.cell_m()).round() as i64,
                )
            };
            rt.active
                .at(p.x + dx, p.z + dz)
                .is_some_and(|k| k.level == rt.gpu.grid.finest)
        });
        if !ready {
            misses[phase] += 1;
        }
        rt.draw(&view);
        rt.gpu.device.poll(wgpu::Maintain::Poll);
        if tick % 60 == 0 {
            println!("tick {tick} phase {phase} native {ready} active {} desired {} target {:?} pending {} effective {} pressure {} live {:.1} committed {:.1}",rt.active.tiles.len(),rt.desired.tiles.len(),rt.target.as_ref().map(|(_,k)|k.difference(&rt.cover).count()),rt.pending_jobs(),rt.effective_budget,rt.memory_pressure,rt.gpu.bytes as f64/1048576.,rt.gpu.managed_committed_bytes() as f64/1048576.);
        }
        if rt.target.is_none() {
            changes += 1;
        }
        if let Some(rest) = Duration::from_secs_f64(1. / 60.).checked_sub(start.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    println!("stop/resume misses {misses:?}; free target frames {changes}");
    assert!(misses[2] < 120, "resumed native detail stalled: {misses:?}");
}
