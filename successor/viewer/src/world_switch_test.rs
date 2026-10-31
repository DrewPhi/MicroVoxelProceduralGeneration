//! Actual GPU/session replacement test, independent of downloaded survey data.
use super::*;
use microvoxel::{
    render::{
        lod::{Grid, MeshKey, TileKey},
        terrain::Sampler,
    },
    GridScale,
};
#[test]
#[ignore = "requires a graphics adapter"]
fn menu_switches_survey_procedural_and_back_without_stale_gpu_tiles() {
    let path =
        std::env::temp_dir().join(format!("voxel-menu-world-{}.mvearth", std::process::id()));
    let mut bytes = b"MVEARTH1".to_vec();
    // Four meters square; enough room for the standing voxel player.
    for v in [16u32, 16, 1, 4] {
        bytes.extend(v.to_le_bytes());
    }
    for v in [0f64, 0., 0.] {
        bytes.extend(v.to_le_bytes());
    }
    for v in [8u32, 8] {
        bytes.extend(v.to_le_bytes());
    }
    for _ in 0..256 {
        bytes.extend(10f32.to_le_bytes());
    }
    for _ in 0..256 {
        bytes.extend(0x07e0u16.to_le_bytes());
    }
    std::fs::write(&path, bytes).unwrap();
    let world = Arc::new(World::new(Terrain::from_survey(&path).unwrap()));
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let mut gpu = pollster::block_on(Gpu::new(&instance, None, 1280, 720)).unwrap();
    gpu.grid = Grid::new(world.spec());
    let level = gpu.grid.finest;
    let key = MeshKey {
        corner_levels: None,
        tile: TileKey { level, x: 0, z: 0 },
        neighbors: [level; 4],
        revision: 0,
    };
    let cover = std::collections::BTreeSet::from([key]);
    let mut sampler = Sampler::new(Arc::clone(&world));
    gpu.upload(key, sampler.mesh(key).unwrap(), &cover).unwrap();
    gpu.upload_ray(key, sampler.ray_brick(key, || false).unwrap(), &cover)
        .unwrap();
    assert!(gpu.bytes > 0);
    let mut menu = menu::Menu::new();
    menu.open = true;
    menu.worlds
        .select_current(&worlds::Request::Survey(path.clone()));
    menu.settings.ray = true;
    let mut runtime = Runtime::new(gpu, Arc::clone(&world), 256, 0).unwrap();
    runtime.set_view(player_view::ViewMode::Rear);
    runtime.toggle_flight();
    runtime.camera.fov = 70f64.to_radians();
    let mut app = App {
        options: Options::defaults(),
        world,
        instance,
        window: None,
        surface: None,
        config: None,
        runtime: Some(runtime),
        grabbed: false,
        menu,
        world_load: None,
        frame_times: Default::default(),
        menu_stats: Vec::new(),
        stats_time: Instant::now(),
        last_frame: Instant::now(),
        last_title: Instant::now(),
        frame_count: 0,
        nonce: 0,
        screenshot: false,
        error: None,
    };
    let request = worlds::Request::Procedural {
        seed: 137,
        grid: GridScale::new(1, 20).unwrap(),
        world_scale: 10,
    };
    fn apply(app: &mut App, request: worlds::Request) {
        app.menu.worlds.pending = Some(request);
        app.menu.worlds.busy = true;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            app.world_controls().unwrap();
            if !app.menu.worlds.busy {
                break;
            }
            assert!(Instant::now() < deadline, "world loader did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    apply(&mut app, request);
    assert_eq!(app.world.identity().seed, 137);
    assert_eq!(app.world.spec().grid.ratio(), (1, 20));
    assert!(app.runtime.as_ref().unwrap().settings.ray);
    assert!(app.runtime.as_ref().unwrap().flying);
    assert_eq!(
        app.runtime.as_ref().unwrap().view,
        player_view::ViewMode::Rear
    );
    assert_eq!(app.runtime.as_ref().unwrap().camera.fov, 70f64.to_radians());
    assert!(app.runtime.as_ref().unwrap().gpu.tiles.is_empty());
    assert_eq!(app.runtime.as_ref().unwrap().gpu.bytes, 0);
    assert!(app.runtime.as_ref().unwrap().player.clear(&app.world));
    assert_eq!(app.world.spec().extent_m, 1000);
    assert_eq!(app.world.baseline().world_scale(), 10);
    assert_eq!(app.menu.worlds.world_scale, "10");
    // Capture the real World tab with the new seed and centimeter field.
    let scale = (1280f32 / 792.).min(720. / 672.);
    let ox = (1280. - 760. * scale) * 0.5;
    let oy = (720. - 640. * scale) * 0.5;
    app.menu
        .cursor(ox + 600. * scale, oy + 30. * scale, 1280, 720);
    app.menu.click(true);
    let runtime = app.runtime.as_mut().unwrap();
    let texture = runtime.gpu.offscreen();
    let view = texture.create_view(&Default::default());
    runtime.draw(&view);
    runtime.gpu.menu_renderer.draw(
        &runtime.gpu.device,
        &runtime.gpu.queue,
        &view,
        &app.menu,
        1280,
        720,
        &[],
    );
    std::fs::create_dir_all("output/menu").unwrap();
    runtime
        .gpu
        .screenshot(&texture, std::path::Path::new("output/menu/world.png"))
        .unwrap();
    apply(&mut app, worlds::Request::Survey(path.clone()));
    assert!(app.world.baseline().survey().is_some());
    assert_eq!(app.world.spec().grid.ratio(), (1, 4));
    assert!(app.runtime.as_ref().unwrap().gpu.tiles.is_empty());
    assert!(app.runtime.as_ref().unwrap().player.clear(&app.world));
    // A failed request must keep the current world/player/session alive.
    let namespace = app.world.cache_namespace();
    apply(
        &mut app,
        worlds::Request::Survey(path.with_extension("missing")),
    );
    assert_eq!(app.world.cache_namespace(), namespace);
    assert!(!app.menu.worlds.status.is_empty());
    std::fs::remove_file(path).unwrap();
}
