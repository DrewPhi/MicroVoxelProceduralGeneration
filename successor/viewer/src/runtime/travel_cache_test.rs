//! First traversal, reversal and revisiting the same native cells.
//! MICROVOXEL_CACHE_ATLAS=1 uses saved seed 42 at world scale 10.
use super::*;
#[test]
#[ignore = "requires real GPU and paced travel; optional centimeter case via env"]
fn predictive_detail_and_backtracking() {
    let atlas = std::env::var_os("MICROVOXEL_CACHE_ATLAS").is_some();
    let cm = std::env::var_os("MICROVOXEL_CACHE_CM").is_some();
    let spec = microvoxel::WorldSpec {
        grid: if cm {
            microvoxel::GridScale::new(1, 50).unwrap()
        } else {
            Default::default()
        },
        ..Default::default()
    };
    let terrain = if atlas {
        Terrain::prepare_atlas(42, spec.grid, 10).expect("saved seed 42 atlas")
    } else {
        Terrain::with_spec(42, microvoxel::generation::TerrainConfig::default(), spec).unwrap()
    };
    println!(
        "Travel source: {}; voxel {:.4} m",
        if atlas {
            "atlas seed 42, scale 10"
        } else {
            "procedural seed 42"
        },
        spec.grid.meters()
    );
    let world = Arc::new(World::new(terrain));
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let gpu = pollster::block_on(Gpu::new(&instance, None, 1280, 720)).unwrap();
    let mut rt = Runtime::new(gpu, world, 8192, 0).unwrap();
    rt.apply_settings(crate::settings::Settings {
        native_radius: if cm { 32. } else { 6. },
        ..Default::default()
    });
    crate::wait_for_cover(&mut rt, Instant::now() + Duration::from_secs(120)).unwrap();
    let texture = rt.gpu.offscreen();
    let view = texture.create_view(&Default::default());
    let mut misses = [0usize; 4];
    let mut counts = [0usize; 4];
    let mut max_frame = 0f64;
    let mut last_phase = 0;
    for tick in 0..1680 {
        let start = Instant::now();
        let phase = if tick < 240 {
            0
        } else if tick < 840 {
            1
        } else if tick < 1440 {
            2
        } else {
            3
        };
        if phase != last_phase {
            println!("phase {last_phase} completed: gen {}, resident reuse {}, prefetch {} built / {} hits",rt.completed,rt.cache_hits,rt.prefetch_builds,rt.prefetch_hits);
            last_phase = phase;
        }
        rt.camera.keys.clear();
        if phase == 1 {
            rt.camera.keys.insert(KeyCode::KeyW);
        }
        if phase == 2 {
            rt.camera.keys.insert(KeyCode::KeyS);
        }
        rt.update(1. / 60.);
        assert!(rt.last_error.is_none(), "{:?}", rt.last_error);
        let p = rt.player.root;
        let ready = (0..9).all(|i| {
            let a = i as f64 * std::f64::consts::TAU / 8.;
            let (dx, dz) = if i == 8 {
                (0, 0)
            } else {
                (
                    (a.cos() * 3. / spec.grid.meters()).round() as i64,
                    (a.sin() * 3. / spec.grid.meters()).round() as i64,
                )
            };
            rt.active
                .at(p.x + dx, p.z + dz)
                .is_some_and(|k| k.level == rt.gpu.grid.finest)
        });
        if !ready {
            misses[phase] += 1;
        }
        counts[phase] += 1;
        assert!(rt.cover.iter().all(|k| rt.gpu.ready(k, rt.settings.ray)));
        rt.draw(&view);
        rt.gpu.device.poll(wgpu::Maintain::Poll);
        max_frame = max_frame.max(start.elapsed().as_secs_f64() * 1000.);
        if let Some(rest) = Duration::from_secs_f64(1. / 60.).checked_sub(start.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    println!("counts {counts:?}, native misses {misses:?}, max work {max_frame:.2} ms");
    rt.report();
    assert!(
        rt.cache_hits > 0,
        "backtracking did not reuse resident terrain"
    );
    if rt.prefetch_enabled {
        assert!(
            rt.prefetch_builds > 0,
            "forecast did not produce usable cached meshes"
        );
        assert!(
            rt.prefetch_hits > 0,
            "no forecast meshes were reused by visible terrain"
        );
    }
    if !cm {
        assert_eq!(
            misses, [0; 4],
            "normal-resolution travel lost native detail"
        );
    } else {
        // Max-radius 2 cm is a stress measurement, not a full-detail promise.
        // Coverage/residency and error assertions above still apply every frame.
        println!("Stress result: outbound {:.1}% / return {:.1}% frames had complete 3 m native coverage",
            100. * (1. - misses[1] as f64 / counts[1] as f64),
            100. * (1. - misses[2] as f64 / counts[2] as f64));
    }
}
