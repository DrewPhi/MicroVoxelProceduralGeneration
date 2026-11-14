//! Opt-in geometry-first cover pipeline. Geometry publishes with explicit
//! appearance debt; complete profile generations promote through AdaptiveView.
//! Inputs contain profiles, never an extra terrain mesh or NAADF volume.
use super::{finish, GeometryTile, TileKey};
use crate::{mesh::Mesh, spatial_appearance::Profile};
use frontier_world::{Cell, World};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    thread::JoinHandle,
};

pub(crate) fn enabled() -> bool {
    std::env::var("FRONTIER_DEFERRED_GROUND_COVER").as_deref() == Ok("1")
}

#[derive(Clone)]
pub(crate) struct Input {
    key: TileKey,
    profiles: Vec<Profile>,
    // First-use order before compaction/packing controls wrapper allocation.
    // Saving only unique IDs retains the synchronous table byte order.
    profile_order: Vec<u32>,
}
impl Input {
    pub(super) fn from_tile(tile: &GeometryTile) -> Self {
        let mut seen = HashSet::new();
        Self {
            key: tile.key,
            profiles: tile.face_profiles.clone(),
            profile_order: tile
                .face_profile_indices
                .iter()
                .copied()
                .filter(|id| seen.insert(*id))
                .collect(),
        }
    }
    pub(crate) fn bytes(&self) -> usize {
        self.profiles.capacity() * 80 + self.profile_order.capacity() * 4
    }
    pub(crate) fn resolve(
        self,
        world: &World,
        mut cancel: impl FnMut() -> bool,
    ) -> Result<Patch, String> {
        if cancel() {
            return Err("cancelled deferred cover".into());
        }
        if self.key.source_revision != world.revision() {
            return Err("obsolete deferred cover source revision".into());
        }
        let mut tile = finish(
            self.key,
            Mesh {
                vertices: vec![],
                indices: vec![],
                origin: Cell::default(),
                exposed_native_faces: 0,
            },
            0,
            0.,
            false,
        );
        tile.face_profiles = self.profiles.clone();
        tile.face_profile_indices = self.profile_order.clone();
        crate::ground_cover::decorate(world, &mut tile, &mut cancel)?;
        if cancel() {
            return Err("cancelled deferred cover".into());
        }
        let remap = self
            .profile_order
            .into_iter()
            .zip(tile.face_profile_indices)
            .collect();
        let shell = crate::ground_cover::shell_enabled()
            && tile
                .face_profiles
                .iter()
                .any(|profile| profile[0][3] == -6.);
        Ok(Patch {
            key: self.key,
            base_profiles: self.profiles,
            profiles: tile.face_profiles,
            remap,
            shell,
        })
    }
}

pub(crate) struct Patch {
    key: TileKey,
    base_profiles: Vec<Profile>,
    profiles: Vec<Profile>,
    remap: HashMap<u32, u32>,
    shell: bool,
}
impl Patch {
    pub(crate) fn bytes(&self) -> usize {
        self.base_profiles.capacity() * 80
            + self.profiles.capacity() * 80
            + self.remap.capacity() * std::mem::size_of::<(u32, u32)>()
    }
    /// Validate the entire source generation and every address before mutating.
    /// The caller must publish a complete new GPU table and references together.
    /// Never call on an admitted Arc: its lifetime guard charges immutable bytes.
    pub(crate) fn apply(self, tile: &mut GeometryTile) -> Result<(), String> {
        if tile.payload_lifetime.is_some()
            || tile.key != self.key
            || tile.exact
            || tile.gpu_profiles.is_some()
            || tile.face_profiles != self.base_profiles
        {
            return Err("deferred cover generation mismatch".into());
        }
        if tile
            .face_profile_indices
            .iter()
            .chain(tile.proxy_triangles.iter().map(|p| &p.profile_index))
            .any(|id| !self.remap.contains_key(id))
        {
            return Err("deferred cover profile address mismatch".into());
        }
        for id in &mut tile.face_profile_indices {
            *id = self.remap[id];
        }
        for triangle in &mut tile.proxy_triangles {
            triangle.profile_index = self.remap[&triangle.profile_index];
        }
        tile.face_profiles = self.profiles;
        if self.shell {
            tile.conservative_surface_bounds[1][1] += 1.;
        }
        tile.bytes = tile.mesh.vertices.capacity() * std::mem::size_of::<crate::mesh::Vertex>()
            + tile.mesh.indices.capacity() * 4
            + tile.face_profile_indices.capacity() * 4
            + tile.face_profiles.capacity() * 80
            + tile.proxy_triangles.capacity() * std::mem::size_of::<super::ProxyTriangle>()
            + tile.native_quads.capacity() * std::mem::size_of::<super::NativeQuad>();
        Ok(())
    }
}

fn clone_geometry(tile: &GeometryTile) -> Result<GeometryTile, String> {
    if tile.gpu_profiles.is_some() || tile.exact || tile.naadf_owned {
        return Err("invalid deferred cover geometry owner".into());
    }
    Ok(GeometryTile {
        naadf_owned: tile.naadf_owned,
        cover_input: None,
        payload_lifetime: None,
        key: tile.key,
        mesh: Mesh {
            vertices: tile.mesh.vertices.clone(),
            indices: tile.mesh.indices.clone(),
            origin: tile.mesh.origin,
            exposed_native_faces: tile.mesh.exposed_native_faces,
        },
        native_quads: tile.native_quads.clone(),
        proxy_triangles: tile.proxy_triangles.clone(),
        native_bins: tile.native_bins.clone(),
        bounds: tile.bounds,
        conservative_surface_bounds: tile.conservative_surface_bounds,
        guaranteed_solid_box: tile.guaranteed_solid_box,
        sampled_columns: tile.sampled_columns,
        empirical_error_m: tile.empirical_error_m,
        certified_error_m: tile.certified_error_m,
        best_certified_vertical_error_m: tile.best_certified_vertical_error_m,
        global_error_m: tile.global_error_m,
        affine_error_m: tile.affine_error_m,
        affine_height_range_m: tile.affine_height_range_m,
        exact_height_range_m: tile.exact_height_range_m,
        affine_stats: tile.affine_stats,
        appearance: tile.appearance,
        face_profiles: tile.face_profiles.clone(),
        gpu_profiles: None,
        face_profile_indices: tile.face_profile_indices.clone(),
        exact: tile.exact,
        bytes: tile.bytes,
    })
}

pub(crate) enum Product {
    Patch(Patch),
    Tile(GeometryTile),
}
pub(crate) struct Ticket(Arc<AtomicBool>);
impl Ticket {
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
struct Job {
    input: Input,
    base: Option<Arc<GeometryTile>>,
    cancel: Arc<AtomicBool>,
}
pub(crate) struct Completion {
    pub(crate) key: TileKey,
    pub(crate) result: Result<Product, String>,
    pub(crate) cpu_ms: f64,
}
/// One appearance thread, one queued input and at most two outstanding jobs.
/// Completed outputs retain their admission slots until polled. Admission
/// has an explicit per-product payload cap. Urgent terrain never uses this slot.
/// The caller owns demand priority and must cancel tickets for obsolete regions.
#[derive(Default)]
struct Counters {
    queued: AtomicUsize,
    active: AtomicUsize,
    outstanding: AtomicUsize,
    completed: AtomicUsize,
    cancelled: AtomicUsize,
    discarded_completions: AtomicUsize,
    payload_refusals: AtomicUsize,
}
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Stats {
    pub(crate) queued: usize,
    pub(crate) active: usize,
    pub(crate) outstanding: usize,
    pub(crate) completed: usize,
    pub(crate) cancelled: usize,
    pub(crate) discarded_completions: usize,
    pub(crate) payload_refusals: usize,
}
pub(crate) struct Worker {
    send: Option<mpsc::SyncSender<Job>>,
    receive: mpsc::Receiver<Completion>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    max_payload_bytes: usize,
    counters: Arc<Counters>,
}
impl Worker {
    pub(crate) fn new(world: World, max_payload_bytes: usize) -> Result<Self, String> {
        let (send, jobs) = mpsc::sync_channel::<Job>(1);
        let (results, receive) = mpsc::sync_channel(2);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let counters = Arc::new(Counters::default());
        let count = counters.clone();
        let thread = std::thread::Builder::new()
            .name("terrain-cover".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    count.queued.fetch_sub(1, Ordering::Relaxed);
                    if stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    count.active.store(1, Ordering::Relaxed);
                    let key = job.input.key;
                    let started = std::time::Instant::now();
                    let result = job
                        .input
                        .resolve(&world, || {
                            stopped.load(Ordering::Relaxed) || job.cancel.load(Ordering::Relaxed)
                        })
                        .and_then(|patch| {
                            if patch.bytes() > max_payload_bytes {
                                return Err("deferred cover result exceeds payload cap".into());
                            }
                            if let Some(base) = job.base {
                                let mut tile = clone_geometry(&base)?;
                                patch.apply(&mut tile)?;
                                if tile.bytes > max_payload_bytes {
                                    return Err("deferred cover tile exceeds payload cap".into());
                                }
                                if let Some(lifetime) = &base.payload_lifetime {
                                    lifetime.total.fetch_add(tile.bytes, Ordering::Relaxed);
                                    tile.payload_lifetime = Some(super::PayloadLifetime {
                                        total: lifetime.total.clone(),
                                        bytes: tile.bytes,
                                    });
                                }
                                Ok(Product::Tile(tile))
                            } else {
                                Ok(Product::Patch(patch))
                            }
                        });
                    // At most two admitted jobs share a two-slot result queue.
                    // The send cannot block; a disconnected consumer is counted.
                    if result
                        .as_ref()
                        .is_err_and(|error| error.contains("exceeds payload cap"))
                    {
                        count.payload_refusals.fetch_add(1, Ordering::Relaxed);
                    }
                    if job.cancel.load(Ordering::Relaxed) || stopped.load(Ordering::Relaxed) {
                        count.cancelled.fetch_add(1, Ordering::Relaxed);
                    }
                    if results
                        .try_send(Completion {
                            key,
                            result,
                            cpu_ms: started.elapsed().as_secs_f64() * 1000.,
                        })
                        .is_err()
                    {
                        count.discarded_completions.fetch_add(1, Ordering::Relaxed);
                        count.outstanding.fetch_sub(1, Ordering::Relaxed);
                    }
                    count.completed.fetch_add(1, Ordering::Relaxed);
                    count.active.store(0, Ordering::Relaxed);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            send: Some(send),
            receive,
            stop,
            thread: Some(thread),
            max_payload_bytes,
            counters,
        })
    }
    pub(crate) fn submit(&self, input: Input) -> Result<Ticket, Input> {
        self.submit_inner(input, None)
    }
    pub(crate) fn submit_tile(&self, tile: Arc<GeometryTile>) -> Result<Ticket, ()> {
        if tile.bytes > self.max_payload_bytes {
            self.counters
                .payload_refusals
                .fetch_add(1, Ordering::Relaxed);
            return Err(());
        }
        let input = tile.cover_input.as_ref().ok_or(())?.clone();
        self.submit_inner(input, Some(tile)).map_err(|_| ())
    }
    fn submit_inner(&self, input: Input, base: Option<Arc<GeometryTile>>) -> Result<Ticket, Input> {
        if input.bytes() > self.max_payload_bytes {
            self.counters
                .payload_refusals
                .fetch_add(1, Ordering::Relaxed);
            return Err(input);
        }
        if self
            .counters
            .outstanding
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < 2).then_some(n + 1)
            })
            .is_err()
        {
            return Err(input);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.counters.queued.fetch_add(1, Ordering::Relaxed);
        match self.send.as_ref().unwrap().try_send(Job {
            input,
            base,
            cancel: cancel.clone(),
        }) {
            Ok(()) => Ok(Ticket(cancel)),
            Err(mpsc::TrySendError::Full(job) | mpsc::TrySendError::Disconnected(job)) => {
                self.counters.queued.fetch_sub(1, Ordering::Relaxed);
                self.counters.outstanding.fetch_sub(1, Ordering::Relaxed);
                Err(job.input)
            }
        }
    }
    pub(crate) fn poll(&self) -> Option<Completion> {
        let completion = self.receive.try_recv().ok()?;
        self.counters.outstanding.fetch_sub(1, Ordering::Relaxed);
        Some(completion)
    }
    pub(crate) fn max_payload_bytes(&self) -> usize {
        self.max_payload_bytes
    }
    pub(crate) fn stats(&self) -> Stats {
        let c = &self.counters;
        Stats {
            queued: c.queued.load(Ordering::Relaxed),
            active: c.active.load(Ordering::Relaxed),
            outstanding: c.outstanding.load(Ordering::Relaxed),
            completed: c.completed.load(Ordering::Relaxed),
            cancelled: c.cancelled.load(Ordering::Relaxed),
            discarded_completions: c.discarded_completions.load(Ordering::Relaxed),
            payload_refusals: c.payload_refusals.load(Ordering::Relaxed),
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.send.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn world() -> World {
        World::fbm(
            7,
            frontier_world::FbmParams {
                recipe: frontier_world::FbmRecipe::Alpine,
                ..Default::default()
            },
        )
        .unwrap()
        .with_vegetation()
    }
    fn key(world: &World) -> TileKey {
        TileKey {
            min: [-928, -768],
            width_cells: 32,
            step_cells: 8,
            edge_steps: [8; 4],
            source_revision: world.revision(),
        }
    }
    #[test]
    fn worker_retains_bounded_completions_and_reports_payload_refusal() {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let make = || Input {
            key: key(&world),
            profiles: vec![[[0.; 4]; 5]],
            profile_order: vec![0],
        };
        let worker = Worker::new(world.clone(), 1024).unwrap();
        let _first = worker.submit(make()).ok().expect("first bounded admission");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let second = loop {
            if let Ok(ticket) = worker.submit(make()) {
                break ticket;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert!(worker.submit(make()).is_err());
        assert_eq!(worker.stats().outstanding, 2);
        let mut completions = 0;
        while completions < 2 {
            if let Some(done) = worker.poll() {
                assert!(matches!(done.result, Ok(Product::Patch(_))));
                completions += 1;
            } else {
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
        assert_eq!(worker.stats().outstanding, 0);
        assert_eq!(worker.stats().discarded_completions, 0);
        second.cancel();
        let tiny = Worker::new(world.clone(), 0).unwrap();
        assert!(tiny.submit(make()).is_err());
        assert_eq!(tiny.stats().payload_refusals, 1);
    }

    #[test]
    fn geometry_first_settles_to_synchronous_profiles_and_exact_geometry() {
        let world = world();
        let key = key(&world);
        let mut control = super::super::build_cached_staged(
            &world,
            key,
            || false,
            &mut super::super::SourceSamples::new(0),
            None,
        )
        .unwrap();
        let (mut staged, input) =
            super::super::build_geometry_first(&world, key, || false).unwrap();
        let input = input.expect("fixture must exercise deferred cover");
        let bounds = staged.bounds;
        let certified = staged.best_certified_vertical_error_m;
        let patch = input.resolve(&world, || false).unwrap();
        patch.apply(&mut staged).unwrap();
        assert_eq!(staged.bounds, bounds);
        assert_eq!(staged.best_certified_vertical_error_m, certified);
        assert_eq!(staged.face_profiles, control.face_profiles);
        assert_eq!(staged.face_profile_indices, control.face_profile_indices);
        assert_eq!(staged.mesh.indices, control.mesh.indices);
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&staged.mesh.vertices),
            bytemuck::cast_slice::<_, u8>(&control.mesh.vertices)
        );
        assert_eq!(
            staged.conservative_surface_bounds,
            control.conservative_surface_bounds
        );
        super::super::pack_proxy_triangles(&mut staged).unwrap();
        super::super::pack_proxy_triangles(&mut control).unwrap();
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&staged.proxy_triangles),
            bytemuck::cast_slice::<_, u8>(&control.proxy_triangles)
        );
    }
    #[test]
    fn patch_rejects_cancelled_or_changed_generation_without_partial_mutation() {
        let world = world();
        let (mut staged, input) =
            super::super::build_geometry_first(&world, key(&world), || false).unwrap();
        assert!(Input::from_tile(&staged).resolve(&world, || true).is_err());
        let patch = input.unwrap().resolve(&world, || false).unwrap();
        staged.key.source_revision = staged.key.source_revision.wrapping_add(1);
        let profiles = staged.face_profiles.clone();
        let indices = staged.face_profile_indices.clone();
        assert!(patch.apply(&mut staged).is_err());
        assert_eq!(staged.face_profiles, profiles);
        assert_eq!(staged.face_profile_indices, indices);
    }
    #[test]
    fn patch_can_promote_already_packed_geometry_without_changing_positions() {
        let world = world();
        let (mut staged, input) =
            super::super::build_geometry_first(&world, key(&world), || false).unwrap();
        super::super::pack_proxy_triangles(&mut staged).unwrap();
        assert!(
            !staged.proxy_triangles.is_empty(),
            "fixture must exercise packed proxies"
        );
        let before: Vec<_> = staged
            .proxy_triangles
            .iter()
            .map(|t| (t.positions, t.normal, t.color))
            .collect();
        input
            .unwrap()
            .resolve(&world, || false)
            .unwrap()
            .apply(&mut staged)
            .unwrap();
        assert_eq!(
            before,
            staged
                .proxy_triangles
                .iter()
                .map(|t| (t.positions, t.normal, t.color))
                .collect::<Vec<_>>()
        );
        let mut control = super::super::build_cached_staged(
            &world,
            key(&world),
            || false,
            &mut super::super::SourceSamples::new(0),
            None,
        )
        .unwrap();
        super::super::pack_proxy_triangles(&mut control).unwrap();
        assert_eq!(staged.face_profiles, control.face_profiles);
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&staged.proxy_triangles),
            bytemuck::cast_slice::<_, u8>(&control.proxy_triangles)
        );
    }
}
