//! Deterministic edited cave/tower exercise at 32-cell and 64 m boundaries.
use crate::{camera::Camera, gpu::Gpu, runtime::Runtime};
use microvoxel::{
    generation::Terrain,
    grid::CellPos,
    materials::{Appearance, MaterialId, Voxel},
    player::Player,
    render::{
        lod::{MeshKey, TileKey},
        terrain::{Sampler, SharedSummaries, SummaryCache, TerrainMesh},
    },
    Edit, EditAction, World,
};
use std::{
    fmt::Write as _,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn key(world: &World<Terrain>, x: i64, z: i64, coarse: bool) -> MeshKey {
    let grid = microvoxel::render::lod::Grid::new(world.spec());
    let level = grid.finest - u8::from(coarse);
    let edge = 32i64 << u8::from(coarse);
    MeshKey {
        tile: TileKey {
            level,
            x: (x.div_euclid(edge)) as u32,
            z: (z.div_euclid(edge)) as u32,
        },
        neighbors: [level; 4],
        revision: world.revision(),
    }
}
fn top_planes(mesh: &TerrainMesh, x: i64, z: i64) -> Vec<i64> {
    let mut out = Vec::new();
    for part in &mesh.parts {
        for q in &part.surface.quads {
            if q.face != 0 {
                continue;
            }
            let ox = mesh.origin[0] + q.origin[0] as i64;
            let oz = mesh.origin[2] + q.origin[2] as i64;
            // +Y's U is Z and V is X.
            if x >= ox
                && x < ox + q.size[1] as i64 * part.step
                && z >= oz
                && z < oz + q.size[0] as i64 * part.step
            {
                out.push(mesh.origin[1] + q.origin[1] as i64);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}
fn mesh_at(
    world: Arc<World<Terrain>>,
    x: i64,
    z: i64,
    coarse: bool,
    cache: SharedSummaries,
) -> microvoxel::Result<TerrainMesh> {
    let k = key(&world, x, z, coarse);
    let mut sampler = Sampler::with_summaries(world, cache);
    Ok(sampler
        .display_mesh(k, || false)
        .map_err(|e| format!("mesh at ({x},{z}) coarse={coarse}: {e}"))?)
}
fn percentile(samples: &[f64], p: f64) -> f64 {
    let mut s = samples.to_vec();
    s.sort_by(f64::total_cmp);
    s[((s.len() - 1) as f64 * p).round() as usize]
}

pub fn run(source: Arc<World<Terrain>>) -> microvoxel::Result<()> {
    let baseline = Arc::new(World::with_materials(
        source.baseline().clone(),
        source.materials().clone(),
    ));
    let spec = baseline.spec();
    let page = spec.grid.cells(64.).ok_or("invalid 64 m page")?;
    let x = page - 1;
    let z = page - 1;
    if z + 48 >= spec.extent_cells() {
        return Err("edit diagnostic needs at least 128 m source extent".into());
    }
    let mut changing = Arc::new(World::with_materials(
        source.baseline().clone(),
        source.materials().clone(),
    ));
    let cache: SharedSummaries = Arc::new(Mutex::new(SummaryCache::default()));
    let mut lo = i64::MAX;
    let mut hi = i64::MIN;
    for dz in -15..=16 {
        for dx in -15..=16 {
            let ground = changing
                .surface_column(x + dx, z + dz)
                .ok_or("unknown cave site")?
                .ground;
            lo = lo.min(ground);
            hi = hi.max(ground);
        }
    }
    if lo < 16 {
        return Err("cave site too shallow".into());
    }
    let old_top = changing
        .surface_column(x, z)
        .ok_or("unknown center")?
        .ground;
    let center = CellPos::new(x, old_top, z);
    let baseline_native = mesh_at(baseline.clone(), x, z, false, cache.clone())?;
    let baseline_far = mesh_at(baseline.clone(), x, z, true, cache.clone())?;
    // Prime this world's cache at revision zero. The same world instance is
    // then edited, so a stale summary cannot hide behind a namespace change.
    let _warm_far = mesh_at(changing.clone(), x, z, true, cache.clone())?;
    if !top_planes(&baseline_native, x, z).contains(&old_top) {
        return Err("baseline native top face missing".into());
    }
    let mut actions = Vec::new();
    // A roofed horizontal cavity with a large open mouth. Both axes cross 64 m
    // page boundaries; the opening is large enough for the 2-cell far sample.
    for dz in -15..=16 {
        for dx in -15..=16 {
            let mouth = (-5..=6).contains(&dx) && (-5..=6).contains(&dz);
            let upper = if mouth { hi + 2 } else { lo - 5 };
            for y in lo - 8..upper {
                let p = CellPos::new(x + dx, y, z + dz);
                if changing.voxel_at(p).known() != Some(Voxel::AIR) {
                    actions.push(EditAction::Set(Edit::set(p, Voxel::AIR)));
                }
            }
        }
    }
    let tower_x = page - 1;
    let tower_z = page + 40;
    let tower_ground = changing
        .surface_column(tower_x, tower_z)
        .ok_or("unknown tower site")?
        .ground;
    let crossing = (tower_ground.div_euclid(page) + 1) * page;
    let tower_end = crossing + 9;
    if tower_end >= spec.ceiling_cells() {
        return Err("tower exceeds source ceiling".into());
    }
    let tower = Voxel::new(MaterialId::BASALT, Appearance(0x39e7));
    for dz in 0..3 {
        for dx in 0..3 {
            for y in tower_ground..tower_end {
                let p = CellPos::new(tower_x + dx, y, tower_z + dz);
                if changing.voxel_at(p).known() != Some(tower) {
                    actions.push(EditAction::Set(Edit::set(p, tower)));
                }
            }
        }
    }
    if actions.len() > 65_536 {
        return Err("edit fixture exceeds transaction limit".into());
    }
    let edit_start = Instant::now();
    let change = Arc::get_mut(&mut changing)
        .ok_or("world snapshot unexpectedly retained")?
        .apply_actions(&actions)?;
    let batch_ms = edit_start.elapsed().as_secs_f64() * 1000.;
    if change.changed_cells.is_empty() {
        return Err("fixture produced no edits".into());
    }
    let cave_probe = CellPos::new(x, old_top - 1, z);
    if changing.voxel_at(cave_probe).known() != Some(Voxel::AIR) {
        return Err("cave still contains source terrain".into());
    }
    if changing
        .voxel_at(CellPos::new(tower_x, crossing, tower_z))
        .known()
        != Some(tower)
    {
        return Err("tower does not cross vertical page".into());
    }
    let edited = changing;
    let near_start = Instant::now();
    let native = mesh_at(edited.clone(), x, z, false, cache.clone())?;
    let native_ms = near_start.elapsed().as_secs_f64() * 1000.;
    let far_start = Instant::now();
    let far = mesh_at(edited.clone(), x, z, true, cache.clone())?;
    let far_ms = far_start.elapsed().as_secs_f64() * 1000.;
    if top_planes(&native, x, z).contains(&old_top) {
        return Err("native mesh retained procedural top over cave mouth".into());
    }
    if top_planes(&far, x, z).contains(&old_top) {
        return Err("far mesh retained procedural top over cave mouth".into());
    }
    let tower_mesh = mesh_at(edited.clone(), tower_x, tower_z, false, cache.clone())?;
    if !top_planes(&tower_mesh, tower_x, tower_z).contains(&tower_end) {
        return Err("native mesh omitted tower top".into());
    }
    // Change 1000 deterministic existing cells in a bounded ring. Alternate
    // solid and Air and include both side of the page/chunk seams.
    let mut repeated = World::with_materials(source.baseline().clone(), source.materials().clone());
    repeated.apply_actions(&actions)?;
    let mut latencies = Vec::with_capacity(1000);
    for i in 0..1000u64 {
        let dx = ((i * 37) % 24) as i64 - 12;
        let dz = ((i * 53) % 24) as i64 - 12;
        let p = CellPos::new(x + dx, lo - 6, z + dz);
        let old = repeated
            .voxel_at(p)
            .known()
            .ok_or("unknown repeated edit cell")?;
        let new = if old.is_air() { tower } else { Voxel::AIR };
        let start = Instant::now();
        repeated.apply_actions(&[EditAction::Set(Edit {
            pos: p,
            value: new,
            expected: Some(old),
        })])?;
        latencies.push(start.elapsed().as_secs_f64() * 1000.);
    }
    let changed_snapshot = Arc::new(repeated);
    let updated = mesh_at(changed_snapshot.clone(), x, z, true, cache.clone())?;
    if updated.quads() == 0 {
        return Err("changed far mesh is empty".into());
    }
    let mut report = String::new();
    writeln!(
        report,
        "edited cave/tower: {} changed cells, {} override bricks, revision {}; batch {:.3} ms",
        change.changed_cells.len(),
        edited.storage().chunk_count(),
        edited.revision(),
        batch_ms
    )?;
    writeln!(report,"native mesh: {:.3} ms, {} quads; far replacement mesh: {:.3} ms, {} quads; baseline native/far quads: {}/{}",native_ms,native.quads(),far_ms,far.quads(),baseline_native.quads(),baseline_far.quads())?;
    writeln!(report,"1000 deterministic cell transactions: p50 {:.4} ms, p95 {:.4} ms, p99 {:.4} ms, max {:.4} ms; final revision {}",percentile(&latencies,0.5),percentile(&latencies,0.95),percentile(&latencies,0.99),latencies.iter().copied().fold(0.,f64::max),changed_snapshot.revision())?;
    writeln!(report,"site cave=({x},{z}), ground min/max={lo}/{hi}, tower=({tower_x},{tower_z}) y={tower_ground}..{tower_end}, 64m page edge={page}")?;
    std::fs::create_dir_all("output/editing")?;
    std::fs::write("output/editing/report.txt", &report)?;
    print!("{report}");
    if std::env::var_os("MICROVOXEL_EDIT_CAPTURE").is_some() {
        capture(
            baseline,
            &actions,
            &edited,
            CellPos::new(x, hi + 32, z - 96),
            center,
        )?;
    }
    Ok(())
}
fn capture(
    baseline: Arc<World<Terrain>>,
    actions: &[EditAction],
    expected: &World<Terrain>,
    eye: CellPos,
    target: CellPos,
) -> microvoxel::Result<()> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let gpu = pollster::block_on(Gpu::new(&instance, None, 960, 540))?;
    let mut camera = Camera::at(eye);
    camera.grid = baseline.spec().grid;
    camera.yaw = ((target.z - eye.z) as f64).atan2((target.x - eye.x) as f64);
    camera.pitch = ((target.y - eye.y) as f64)
        .atan2(((target.x - eye.x) as f64).hypot((target.z - eye.z) as f64));
    let mut player = Player::with_scale(baseline.identity().seed, eye, baseline.spec().grid)?;
    if !player.place_above(&baseline, eye.x, eye.z) {
        return Err("edit capture camera has no safe player placement".into());
    }
    let mut runtime = Runtime::with_spawn(gpu, baseline.clone(), 512, (Camera::at(eye), player))?;
    // with_spawn synchronizes the walking view. Install the requested fly view
    // afterwards, so both captures use precisely the same cave/tower framing.
    runtime.flying = true;
    runtime.camera = camera;
    runtime.apply_settings(crate::settings::Settings {
        transitions: false,
        native_radius: 32.,
        ..runtime.settings
    });
    crate::wait_for_cover(&mut runtime, Instant::now() + Duration::from_secs(120))?;
    if runtime
        .active
        .at(target.x, target.z)
        .is_none_or(|tile| tile.level != runtime.active.grid.finest)
    {
        return Err("edit capture did not achieve native detail at the cave mouth".into());
    }
    save_capture(&mut runtime, "baseline")?;
    let camera_before = (
        runtime.camera.cell,
        runtime.camera.yaw,
        runtime.camera.pitch,
        runtime.camera.fov,
    );
    let player_before = (
        runtime.player.root,
        runtime.player.pose,
        runtime.player.ticks,
        runtime.player.grounded,
        runtime.player.vertical_velocity,
    );
    let old_cover = runtime.cover.clone();
    let unchanged: Vec<_> = old_cover
        .iter()
        .copied()
        .filter(|k| {
            let [x, z] = k.tile.origin_in(runtime.active.grid);
            let halo = k.tile.step_in(runtime.active.grid) * 2;
            expected.region_revision(
                x - halo,
                z - halo,
                k.tile.edge_in(runtime.active.grid) + halo * 2,
            ) == k.revision
        })
        .collect();
    if unchanged.is_empty() || unchanged.len() == old_cover.len() {
        return Err("live edit fixture needs both affected and unaffected resident tiles".into());
    }
    let change = runtime.apply_cell_changes(actions)?;
    if change.changed_cells.len() != actions.len() || change.revision != expected.revision() {
        return Err("live transaction did not commit every expected cell exactly once".into());
    }
    for action in actions {
        let EditAction::Set(edit) = action else {
            return Err("unexpected reset in edit fixture".into());
        };
        if runtime.world.voxel_at(edit.pos).known() != Some(edit.value) {
            return Err(format!("live edit mismatch at {:?}", edit.pos).into());
        }
        if baseline.voxel_at(edit.pos).known() == Some(edit.value) {
            return Err("live edit mutated its retained baseline snapshot".into());
        }
    }
    if runtime.cover != old_cover || unchanged.iter().any(|k| !runtime.gpu.ready(k, false)) {
        return Err(
            "live transaction discarded the resident cover before replacements were ready".into(),
        );
    }
    crate::wait_for_cover(&mut runtime, Instant::now() + Duration::from_secs(120))?;
    if camera_before
        != (
            runtime.camera.cell,
            runtime.camera.yaw,
            runtime.camera.pitch,
            runtime.camera.fov,
        )
        || player_before
            != (
                runtime.player.root,
                runtime.player.pose,
                runtime.player.ticks,
                runtime.player.grounded,
                runtime.player.vertical_velocity,
            )
    {
        return Err("live edit reset the camera or player".into());
    }
    for k in &runtime.cover {
        let [x, z] = k.tile.origin_in(runtime.active.grid);
        let halo = k.tile.step_in(runtime.active.grid) * 2;
        if k.revision
            != runtime.world.region_revision(
                x - halo,
                z - halo,
                k.tile.edge_in(runtime.active.grid) + halo * 2,
            )
            || !runtime.gpu.ready(k, false)
        {
            return Err("settled edited cover retained an old or unready surface".into());
        }
    }
    if unchanged.iter().any(|k| !runtime.gpu.ready(k, false)) {
        return Err("live edit evicted unrelated resident terrain".into());
    }
    if runtime.gpu.transitioning() {
        return Err("edit capture still retains a fading old surface".into());
    }
    save_capture(&mut runtime, "edited")?;
    let report = format!("live runtime transaction: {} exact cells, {} unchanged resident tiles preserved; camera/player preserved; settled cover {} current regional keys\n",
        change.changed_cells.len(), unchanged.len(), runtime.cover.len());
    print!("{report}");
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open("output/editing/report.txt")?
        .write_all(report.as_bytes())?;
    Ok(())
}
fn save_capture(runtime: &mut Runtime, label: &str) -> microvoxel::Result<()> {
    let texture = runtime.gpu.offscreen();
    let view = texture.create_view(&Default::default());
    runtime.draw(&view);
    runtime.gpu.device.poll(wgpu::Maintain::Wait);
    runtime.gpu.screenshot(
        &texture,
        std::path::Path::new(&format!("output/editing/{label}.png")),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "expensive end-to-end terrain and mesh diagnostic"]
    fn cave_tower_revisions() {
        run(Arc::new(World::new(Terrain::new(5)))).unwrap();
    }
}
