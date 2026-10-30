//! Reproducible display-quality captures plus a paced moving-camera stress run.
use crate::{
    runtime::Runtime,
    settings::{FrameTimes, Settings},
};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use winit::keyboard::KeyCode;
pub fn run(runtime: &mut Runtime, directory: &Path) -> microvoxel::Result<()> {
    std::fs::create_dir_all(directory)?;
    let texture = runtime.gpu.offscreen();
    let view = texture.create_view(&Default::default());
    for (name, quality, memory) in [
        ("low", 25., 128),
        ("balanced", 75., 128),
        ("high", 100., 256),
    ] {
        let settings = Settings {
            quality,
            memory_mib: memory,
            transitions: false,
            ..Settings::default()
        };
        let start = Instant::now();
        runtime.apply_settings(settings);
        crate::wait_for_cover(runtime, Instant::now() + Duration::from_secs(120))?;
        assert!(
            runtime.achieved_pixels + 1e-9 >= settings.pixels(),
            "quality intent must be planned before ready"
        );
        for _ in 0..3 {
            runtime.draw(&view);
            runtime.gpu.device.poll(wgpu::Maintain::Wait);
        }
        let mut times = FrameTimes::default();
        for _ in 0..60 {
            let t = Instant::now();
            runtime.draw(&view);
            runtime.gpu.device.poll(wgpu::Maintain::Wait);
            times.push(t.elapsed().as_secs_f64());
        }
        println!("LOD {name}: requested {:.2} px / visible P95 {:.2} px; {} tiles, {} native; {:.2} MiB mesh cache; frame ms {:?}; prepare+capture {:.2}s",
            settings.pixels(),runtime.achieved_pixels,runtime.cover.len(),runtime.cover.iter().filter(|k|k.tile.level==13).count(),runtime.gpu.bytes as f64/1048576.,times.percentiles(),start.elapsed().as_secs_f64());
        runtime
            .gpu
            .screenshot(&texture, &directory.join(format!("lod_{name}.png")))?;
    }
    // Check that reducing the cache leaves a valid complete resident cover.
    runtime.apply_settings(Settings {
        quality: 100.,
        memory_mib: 64,
        transitions: false,
        ..Settings::default()
    });
    crate::wait_for_cover(runtime, Instant::now() + Duration::from_secs(120))?;
    assert!(runtime
        .cover
        .iter()
        .all(|k| runtime.gpu.tiles.contains_key(k)));
    assert!(
        runtime.gpu.bytes <= 64 * 1024 * 1024,
        "shrinking must retire the old cover"
    );
    println!(
        "Constrained cache: {:.2} MiB / 64 MiB; visible P95 {:.2} px; limited {}",
        runtime.gpu.bytes as f64 / 1048576.,
        runtime.achieved_pixels,
        runtime.budget_limited
    );
    runtime.apply_settings(Settings {
        transitions: false,
        ..Settings::default()
    });
    crate::wait_for_cover(runtime, Instant::now() + Duration::from_secs(120))?;
    runtime.apply_settings(Settings::default());
    if !runtime.flying {
        runtime.toggle_flight();
    }
    runtime.camera.cell.y += 96;
    runtime.camera.pitch = 0.03;
    runtime.camera.speed = 20.;
    let original = runtime.player.root;
    let probe = microvoxel::CellPos::new(original.x, original.y - 1, original.z);
    let before = runtime.world.voxel_at(probe);
    let mut times = FrameTimes::default();
    let mut update_times = FrameTimes::default();
    let mut draw_times = FrameTimes::default();
    let mut saved = false;
    let mut transitions = 0;
    runtime.camera.keys.insert(KeyCode::KeyW);
    for frame in 0..180 {
        let t = Instant::now();
        runtime.camera.yaw += 0.002;
        runtime.update(1. / 60.);
        let drawing = Instant::now();
        update_times.push(t.elapsed().as_secs_f64());
        runtime.draw(&view);
        runtime.gpu.device.poll(wgpu::Maintain::Wait);
        draw_times.push(drawing.elapsed().as_secs_f64());
        if let Some(error) = &runtime.last_error {
            return Err(error.clone().into());
        }
        assert!(runtime
            .cover
            .iter()
            .all(|k| runtime.gpu.tiles.contains_key(k)));
        if runtime.gpu.transitioning() {
            transitions += 1;
            if !saved && frame > 30 {
                runtime
                    .gpu
                    .screenshot(&texture, &directory.join("lod_transition.png"))?;
                saved = true;
            }
        }
        let elapsed = t.elapsed();
        times.push(elapsed.as_secs_f64());
        if let Some(rest) = Duration::from_secs_f64(1. / 60.).checked_sub(elapsed) {
            std::thread::sleep(rest);
        }
    }
    runtime.clear_controls();
    assert_eq!(original, runtime.player.root);
    assert_eq!(before, runtime.world.voxel_at(probe));
    println!("Moving camera: 180 paced frames, work ms median/p95/p99 {:?}; {transitions} frames with LOD fade; {} queued jobs",times.percentiles(),runtime.pending_jobs());
    println!(
        "Moving update ms {:?}; synchronized draw ms {:?}",
        update_times.percentiles(),
        draw_times.percentiles()
    );
    crate::wait_for_cover(runtime, Instant::now() + Duration::from_secs(120))?;
    runtime.draw(&view);
    runtime
        .gpu
        .screenshot(&texture, &directory.join("lod_after_flight.png"))?;
    Ok(())
}
