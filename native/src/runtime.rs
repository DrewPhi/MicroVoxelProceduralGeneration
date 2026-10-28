//! Coordinates immutable mesh jobs and GPU-resident covers. A complete cover is
//! swapped atomically, so an unfinished child never leaves a hole in its parent.
use crate::{camera::Camera, gpu::Gpu, streaming::Workers};
use microvoxel::{
    generation::ProceduralTerrain,
    queries::{raycast, Trace},
    render::lod::{self, Layout, MeshKey},
    World,
};
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};

pub struct Runtime {
    pub gpu: Gpu,
    pub camera: Camera,
    pub world: Arc<World<ProceduralTerrain>>,
    workers: Workers,
    pub active: Layout,
    pub cover: BTreeSet<MeshKey>,
    target: Option<(Layout, BTreeSet<MeshKey>)>,
    pending: BTreeSet<MeshKey>,
    last_plan: Instant,
    last_cell: microvoxel::CellPos,
    last_fov: f64,
    last_height: u32,
    pub tile_budget: usize,
    pub completed: usize,
    pub mesh_time: Duration,
    pub visible: usize,
    pub last_error: Option<String>,
}
impl Runtime {
    pub fn new(
        gpu: Gpu,
        world: Arc<World<ProceduralTerrain>>,
        tile_budget: usize,
        spawn_nonce: u64,
    ) -> Self {
        let camera = Camera::spawn(&world, spawn_nonce);
        let height = gpu.height;
        let cell = camera.cell;
        let fov = camera.fov;
        let workers = Workers::new(Arc::clone(&world));
        println!(
            "{} mesh workers; 128 MiB GPU mesh budget; tile budget {tile_budget}",
            workers.count
        );
        let active = Layout::root();
        let keys = active.mesh_keys(world.revision()).into_values().collect();
        Self {
            gpu,
            camera,
            world,
            workers,
            active: active.clone(),
            cover: BTreeSet::new(),
            target: Some((active, keys)),
            pending: BTreeSet::new(),
            last_plan: Instant::now() - Duration::from_secs(1),
            last_cell: cell,
            last_fov: fov,
            last_height: height,
            tile_budget,
            completed: 0,
            mesh_time: Duration::ZERO,
            visible: 0,
            last_error: None,
        }
    }
    pub fn loading(&self) -> (usize, usize) {
        self.target
            .as_ref()
            .map_or((self.cover.len(), self.cover.len()), |(_, keys)| {
                (
                    keys.iter()
                        .filter(|k| self.gpu.tiles.contains_key(k))
                        .count(),
                    keys.len(),
                )
            })
    }
    pub fn settled(&self) -> bool {
        self.target.is_none() && self.active.tiles.len() > 1
    }
    pub fn update(&mut self, dt: f64) {
        self.camera.update(&self.world, dt.min(0.25));
        if self.last_error.is_some() {
            return;
        }
        let mut protected = self.cover.clone();
        if let Some((_, keys)) = &self.target {
            protected.extend(keys);
        }
        let upload_start = Instant::now();
        for _ in 0..2 {
            let Ok(result) = self.workers.results.try_recv() else {
                break;
            };
            self.pending.remove(&result.key);
            if !protected.contains(&result.key) {
                continue;
            }
            self.completed += 1;
            self.mesh_time += result.elapsed;
            match result
                .mesh
                .and_then(|mesh| self.gpu.upload(result.key, mesh, &protected))
            {
                Ok(()) => {}
                Err(message) => {
                    eprintln!("Mesh {:?}: {message}", result.key.tile);
                    self.last_error = Some(message);
                    return;
                }
            }
            if upload_start.elapsed() > Duration::from_millis(3) {
                break;
            }
        }
        if let Some((_, keys)) = &self.target {
            if keys.iter().all(|k| self.gpu.tiles.contains_key(k)) {
                let (layout, keys) = self.target.take().unwrap();
                self.active = layout;
                self.cover = keys;
                println!(
                    "Installed {} stitched LOD tiles, {} native; {:.2} MiB resident",
                    self.cover.len(),
                    self.cover
                        .iter()
                        .filter(|k| k.tile.level == lod::FINEST)
                        .count(),
                    self.gpu.bytes as f64 / (1024. * 1024.)
                );
            }
        }
        let moved = self.camera.cell != self.last_cell
            || self.camera.fov != self.last_fov
            || self.gpu.height != self.last_height;
        if self.target.is_none()
            && (self.cover.len() <= 1
                || moved && self.last_plan.elapsed() > Duration::from_millis(300))
        {
            let layout = lod::select(
                self.camera.eye(),
                self.gpu.height,
                self.camera.fov,
                &self.active,
                self.tile_budget,
            );
            if layout != self.active {
                let keys = layout
                    .mesh_keys(self.world.revision())
                    .into_values()
                    .collect();
                self.target = Some((layout, keys));
            }
            self.last_plan = Instant::now();
            self.last_cell = self.camera.cell;
            self.last_fov = self.camera.fov;
            self.last_height = self.gpu.height;
        }
        if let Some((_, keys)) = &self.target {
            let eye = self.camera.eye();
            let mut requests: Vec<_> = keys
                .iter()
                .filter(|k| !self.gpu.tiles.contains_key(k) && !self.pending.contains(k))
                .copied()
                .collect();
            requests.sort_by(|a, b| {
                let distance = |k: &MeshKey| {
                    let [x, z] = k.tile.origin();
                    let e = k.tile.edge();
                    ((x + e / 2) as f64 / 16. - eye[0]).hypot((z + e / 2) as f64 / 16. - eye[2])
                };
                distance(a).total_cmp(&distance(b))
            });
            for key in requests {
                if !self.workers.submit(key) {
                    break;
                }
                self.pending.insert(key);
            }
        }
    }
    pub fn draw(&mut self, view: &wgpu::TextureView) {
        let selection = match raycast(&self.world, self.camera.eye(), self.camera.direction(), 12.)
        {
            Trace::Hit(hit)
                if self
                    .active
                    .at(hit.cell.x, hit.cell.z)
                    .is_some_and(|tile| tile.level == lod::FINEST) =>
            {
                Some([hit.cell.x, hit.cell.y, hit.cell.z])
            }
            _ => None,
        };
        self.visible = self.gpu.render(view, &self.camera, &self.cover, selection);
    }
    pub fn stats(&self, fps: f64) -> String {
        let (ready, total) = self.loading();
        let level = self.cover.iter().map(|k| k.tile.level).max().unwrap_or(0);
        let spacing = 100. * (1i64 << (lod::FINEST - level)) as f64 / 16.;
        format!("MicroVoxel | {fps:.0} FPS | Speed {:.1} m/s | FOV {:.0}° | {spacing:.2} cm | {} / {} tiles | {:.1} MiB | Loaded {ready}/{total}{}",self.camera.speed,self.camera.fov.to_degrees(),self.visible,self.cover.len(),self.gpu.bytes as f64/(1024.*1024.),if self.last_error.is_some(){" | ERROR: see terminal"}else{""})
    }
    pub fn report(&self) {
        let faces: usize = self
            .cover
            .iter()
            .filter_map(|k| self.gpu.tiles.get(k))
            .map(|t| t.faces)
            .sum();
        let quads: usize = self
            .cover
            .iter()
            .filter_map(|k| self.gpu.tiles.get(k))
            .map(|t| t.quads)
            .sum();
        println!("Active surface: {faces} cell faces -> {quads} merged rectangles ({:.1}x fewer); GPU cache {:.2} MiB; {} mesh jobs, summed worker time {:.2}s",faces as f64/quads.max(1) as f64,self.gpu.bytes as f64/(1024.*1024.),self.completed,self.mesh_time.as_secs_f64());
    }
}
