//! Bounded experimental selection over immutable canonical terrain tiles.
//! Missing coverage and unmet quality remain explicit; no fog or crop hides it.
use crate::{
    adaptive_geometry::{GeometryCache, GeometryTile, TileKey},
    build_memory::{self, BuildMemoryStats},
    Camera,
};
use frontier_runtime::{
    occlusion::{Aabb, OcclusionSet},
    representation::{Projection, ViewBounds},
};
use frontier_world::{World, VOXEL_SIZE};
use glam::DVec3;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

/// Internal convergence reserve; the reported quality contract is unchanged.
pub(crate) const REFINEMENT_RESERVE: f64 = 0.8;

#[derive(Clone, Copy, Debug)]
pub struct StreamSettings {
    pub pixel_error: f64,
    pub cpu_cache_bytes: usize,
    pub max_nodes: usize,
    pub jobs_per_update: usize,
    /// Bounded simultaneous canonical builds; one keeps the serial control path.
    pub worker_count: usize,
    /// Separate from retained cache: builders and externally held evicted tiles.
    pub build_budget_bytes: usize,
    /// Experimental: skip intermediate proxy steps for one urgent native leaf.
    pub direct_native_urgent: bool,
}
impl Default for StreamSettings {
    fn default() -> Self {
        Self {
            pixel_error: 2.0,
            cpu_cache_bytes: 512 * 1024 * 1024,
            max_nodes: 100_000,
            jobs_per_update: 4,
            worker_count: 1,
            build_budget_bytes: 512 * 1024 * 1024,
            direct_native_urgent: false,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PlanningStats {
    pub calls: u64,
    pub full_calls: u64,
    pub retained_calls: u64,
    pub total_ms: f64,
    pub max_ms: f64,
    pub prepared_batches_without_plan: u64,
}
pub struct SelectionFrame {
    pub epoch: u64,
    pub preparation: Option<PreparationCycle>,
    pub publication_transaction: Option<PublicationTransaction>,
    pub planning: PlanningStats,
    pub tiles: Vec<Arc<GeometryTile>>,
    pub coverage_ready: bool,
    pub quality_ready: bool,
    /// Current partition also satisfies the stricter internal motion reserve.
    pub refinement_ready: bool,
    pub pending: usize,
    pub visited: usize,
    pub limit_reached: bool,
    pub cache: crate::adaptive_geometry::CacheStats,
    pub build_memory: BuildMemoryStats,
    /// None means at least one visible tile has an unbounded projection.
    pub max_visible_error_px: Option<f64>,
    pub visible_unresolved_count: usize,
    pub visible_uncertified_count: usize,
    pub global_bound_clusters: usize,
    pub local_certified_clusters: usize,
    pub affine_bound_clusters: usize,
    pub occluded_clusters: usize,
    pub occluder_boxes: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Node {
    min: [i64; 2],
    width: u32,
    step: u32,
}
#[derive(Clone, Copy)]
struct RefinementCandidate {
    key: TileKey,
    distance_squared: f64,
    error_px: f64,
}
/// Representation-choice heuristic, not proof that intermediate proxies fail.
/// Native construction still establishes its own certificate and passes the
/// normal builder admission and coherent edge-closure checks.
fn direct_native_candidate(
    candidates: &[RefinementCandidate],
    enabled: bool,
    pixel_error: f64,
    finest_cells: u32,
) -> Option<TileKey> {
    enabled
        .then(|| {
            candidates
                .iter()
                .filter(|c| {
                    c.key.width_cells <= finest_cells.min(128)
                        && c.key.step_cells > 1
                        && (!c.error_px.is_finite() || c.error_px > pixel_error)
                })
                .min_by(|a, b| {
                    a.distance_squared
                        .total_cmp(&b.distance_squared)
                        .then_with(|| a.key.cmp(&b.key))
                })
                .map(|c| c.key)
        })
        .flatten()
}
fn refined_leaf_step(key: TileKey, direct_native: Option<TileKey>) -> u32 {
    if direct_native == Some(key) {
        1
    } else {
        (key.step_cells / 2).max(1)
    }
}
/// Requested failures always precede reserve work. Protect nearest unbounded
/// work, but give one slot to a finite large error so distance alone cannot
/// starve a coarse owner covering an important distant part of the image.
fn select_refinements(
    mut candidates: Vec<RefinementCandidate>,
    pixel_error: f64,
    budget: usize,
) -> HashSet<TileKey> {
    candidates.sort_by(|a, b| {
        a.distance_squared
            .total_cmp(&b.distance_squared)
            .then_with(|| a.key.cmp(&b.key))
    });
    let mut selected = HashSet::new();
    if budget == 0 {
        return selected;
    }
    if let Some(candidate) = candidates.iter().find(|c| !c.error_px.is_finite()) {
        selected.insert(candidate.key);
    }
    if selected.len() < budget {
        if let Some(candidate) = candidates
            .iter()
            .filter(|c| c.error_px.is_finite() && c.error_px > pixel_error)
            .max_by(|a, b| {
                a.error_px
                    .total_cmp(&b.error_px)
                    .then_with(|| b.key.cmp(&a.key))
            })
        {
            selected.insert(candidate.key);
        }
    }
    for requested in [true, false] {
        for candidate in &candidates {
            if selected.len() >= budget {
                return selected;
            }
            let fails_requested =
                !candidate.error_px.is_finite() || candidate.error_px > pixel_error;
            if fails_requested == requested {
                selected.insert(candidate.key);
            }
        }
    }
    selected
}
#[derive(Clone, Copy)]
struct PredictionCandidate {
    key: TileKey,
    need_fraction: f64,
    error_px: f64,
}
fn select_with_prediction(
    current: Vec<RefinementCandidate>,
    predicted: Vec<PredictionCandidate>,
    pixel_error: f64,
    budget: usize,
) -> (HashSet<TileKey>, Option<TileKey>) {
    let requested_failed = current
        .iter()
        .any(|c| !c.error_px.is_finite() || c.error_px > pixel_error);
    let may_predict =
        !requested_failed && !predicted.is_empty() && (current.is_empty() || budget > 1);
    let mut selected = select_refinements(
        current.clone(),
        pixel_error,
        budget.saturating_sub(usize::from(may_predict)),
    );
    if may_predict {
        if let Some(next) = predicted
            .iter()
            .filter(|c| !selected.contains(&c.key))
            .min_by(|a, b| {
                a.need_fraction
                    .total_cmp(&b.need_fraction)
                    .then_with(|| b.error_px.total_cmp(&a.error_px))
                    .then_with(|| a.key.cmp(&b.key))
            })
        {
            selected.insert(next.key);
            return (selected, Some(next.key));
        }
    }
    (select_refinements(current, pixel_error, budget), None)
}
fn footprints_overlap(a: TileKey, b: TileKey) -> bool {
    (0..2).all(|axis| {
        (a.min[axis] as i128) < b.min[axis] as i128 + b.width_cells as i128
            && (b.min[axis] as i128) < a.min[axis] as i128 + a.width_cells as i128
    })
}
#[derive(Clone, Copy)]
struct RejectedCoarsen {
    error_m: f64,
    bounds: [[f64; 3]; 2],
}
pub struct AdaptiveStream {
    world: World,
    cache: GeometryCache,
    settings: StreamSettings,
    pending: VecDeque<TileKey>,
    /// Retain a bounded coherent replacement across nearby camera epochs.
    transaction: Option<Vec<TileKey>>,
    transaction_known_bytes: usize,
    pending_publication: Option<PublicationTransaction>,
    committed_publication: Option<PublicationTransaction>,
    next_publication_id: u64,
    planning: PlanningStats,
    epoch: u64,
    known_bytes: HashMap<TileKey, usize>,
    variants: HashMap<([i64; 2], u32), Vec<TileKey>>,
    coarsen_probes: Vec<TileKey>,
    coarsen_requested: bool,
    coarsen_targets: HashSet<([i64; 2], u32)>,
    rejected_coarsen: HashMap<([i64; 2], u32, u32), RejectedCoarsen>,
    committed: Vec<Arc<GeometryTile>>,
    committed_pose: Option<(Camera, u32, u32)>,
    occlusion: Option<(Camera, OcclusionSet)>,
    /// Measured-latency extrapolation; never changes current-view quality.
    prediction: Option<Camera>,
    predicted_footprint: Option<TileKey>,
    build_memory: BuildMemoryStats,
    build_refused: HashSet<TileKey>,
    #[cfg(test)]
    diagnostic_target: Vec<TileKey>,
    #[cfg(test)]
    finest_cells: u32,
}
impl AdaptiveStream {
    pub fn new(world: World, settings: StreamSettings) -> Result<Self, String> {
        if world.fbm_terrain().is_none()
            || world.edit_cells().any(|c| !world.volume_owned(c))
            || (world.revision() != 0 && world.topology_base_cells().is_none())
        {
            return Err(
                "adaptive stream requires FBM with edits confined to explicit volume ownership"
                    .into(),
            );
        }
        if !settings.pixel_error.is_finite()
            || settings.pixel_error < 0.0
            || settings.max_nodes == 0
            || settings.jobs_per_update == 0
            || !(1..=16).contains(&settings.worker_count)
            || settings.build_budget_bytes == 0
        {
            return Err("invalid adaptive stream settings".into());
        }
        Ok(Self {
            cache: GeometryCache::new(world.clone(), settings.cpu_cache_bytes),
            world,
            settings,
            pending: VecDeque::new(),
            transaction: None,
            transaction_known_bytes: 0,
            pending_publication: None,
            committed_publication: None,
            next_publication_id: 0,
            planning: PlanningStats::default(),
            epoch: 0,
            known_bytes: HashMap::new(),
            variants: HashMap::new(),
            coarsen_probes: vec![],
            coarsen_requested: false,
            coarsen_targets: HashSet::new(),
            rejected_coarsen: HashMap::new(),
            committed: vec![],
            committed_pose: None,
            occlusion: None,
            prediction: None,
            predicted_footprint: None,
            build_memory: BuildMemoryStats {
                budget_bytes: settings.build_budget_bytes,
                ..Default::default()
            },
            build_refused: HashSet::new(),
            #[cfg(test)]
            diagnostic_target: vec![],
            #[cfg(test)]
            finest_cells: 128,
        })
    }
    fn finest_cells(&self) -> u32 {
        #[cfg(test)]
        {
            self.finest_cells
        }
        #[cfg(not(test))]
        {
            128
        }
    }
    #[cfg(test)]
    pub(crate) fn set_test_finest_cells(&mut self, cells: u32) {
        assert!(cells == 64 || cells == 128);
        assert!(self.committed.is_empty() && self.transaction.is_none());
        self.finest_cells = cells;
    }
    pub(crate) fn plan(&mut self, camera: &Camera, width: u32, height: u32) -> SelectionFrame {
        let started = std::time::Instant::now();
        let mut frame = self.plan_partition(camera, width, height);
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        self.planning.calls += 1;
        self.planning.total_ms += elapsed;
        self.planning.max_ms = self.planning.max_ms.max(elapsed);
        frame.planning = self.planning;
        frame
    }
    fn plan_partition(&mut self, camera: &Camera, width: u32, height: u32) -> SelectionFrame {
        self.epoch = self.epoch.wrapping_add(1);
        self.coarsen_probes.clear();
        self.coarsen_requested = false;
        self.coarsen_targets.clear();
        let mut frame = SelectionFrame {
            epoch: self.epoch,
            preparation: None,
            publication_transaction: None,
            planning: self.planning,
            tiles: vec![],
            coverage_ready: false,
            quality_ready: false,
            refinement_ready: false,
            pending: 0,
            visited: 0,
            limit_reached: false,
            cache: self.cache.stats(),
            build_memory: self.build_memory,
            max_visible_error_px: Some(0.0),
            visible_unresolved_count: 0,
            visible_uncertified_count: 0,
            global_bound_clusters: 0,
            local_certified_clusters: 0,
            affine_bound_clusters: 0,
            occluded_clusters: 0,
            occluder_boxes: 0,
        };
        if width == 0
            || height == 0
            || !camera.far.is_finite()
            || camera.far <= 0.0
            || !camera.position.is_finite()
            || camera.position.abs().max_element() / VOXEL_SIZE > (1_u64 << 52) as f64
            || camera.far as f64 / VOXEL_SIZE > (1_u64 << 29) as f64
        {
            frame.limit_reached = true;
            self.finish_build_memory(&mut frame);
            return frame;
        }
        let Some(p) = camera_projection(camera, width, height) else {
            frame.limit_reached = true;
            self.finish_build_memory(&mut frame);
            return frame;
        };
        self.refresh_occlusion(camera);
        // Content is camera-independent. Finish the small replacement already
        // in flight while its complete footprint still covers the current view.
        // Replanning every moving epoch discarded nearly finished transactions.
        if let Some(keys) = self.transaction.take() {
            let bytes = self.transaction_known_bytes;
            if complete_key_guard(
                keys.iter().copied(),
                camera,
                width,
                height,
                self.world.topology_base_cells().is_some(),
            ) && bytes <= self.cache.mesh_budget_bytes()
            {
                let mut next = Vec::with_capacity(keys.len());
                let mut missing = VecDeque::new();
                for &key in &keys {
                    if let Some(tile) = self.cache.get(key) {
                        next.push(tile);
                    } else {
                        missing.push_back(key);
                    }
                }
                if missing.is_empty() {
                    // A turn can expose previously hidden coarsening. The actual
                    // closed-edge certificate must still pass at the new pose.
                    if self.coarsening_certified(&next, camera, p) {
                        self.planning.retained_calls += 1;
                        self.committed_publication = self.pending_publication.take();
                        self.committed = next;
                        self.committed_pose = Some((*camera, width, height));
                        self.pending.clear();
                        self.refresh_occlusion(camera);
                        frame.tiles = self.committed.clone();
                        frame.coverage_ready = true;
                        self.assess_quality(&mut frame, camera, p);
                        frame.cache = self.cache.stats();
                        self.finish_build_memory(&mut frame);
                        return frame;
                    }
                } else {
                    self.planning.retained_calls += 1;
                    self.pending = missing;
                    self.transaction = Some(keys);
                    frame.tiles = self.committed.clone();
                    self.revalidate(&mut frame, camera, width, height);
                    frame.pending = self.pending.len();
                    frame.cache = self.cache.stats();
                    self.finish_build_memory(&mut frame);
                    return frame;
                }
            }
            // A teleport, uncovered view or inadmissible coarsening invalidates
            // demand, never the immutable completed geometry in the cache.
            self.pending.clear();
            self.pending_publication = None;
        }
        self.planning.full_calls += 1;
        let same = self
            .committed_pose
            .as_ref()
            .map(|(c, w, h)| same_pose(c, camera) && *w == width && *h == height)
            .unwrap_or(false);
        // Refine a bounded local region, keeping the rest of the coherent
        // partition unchanged. A global refinement wave can consume the cache
        // before any near-native geometry becomes publishable.
        let urgent: Vec<_> = self
            .committed
            .iter()
            .filter_map(|t| {
                if !self.tile_visible(t, camera, p) {
                    return None;
                }
                let error_px = self.projected_error(t, camera, p);
                if error_px <= self.settings.pixel_error * REFINEMENT_RESERVE {
                    return None;
                }
                let distance_squared = (0..3)
                    .map(|a| {
                        let c = camera.position[a];
                        let d = if c < t.bounds[0][a] {
                            t.bounds[0][a] - c
                        } else if c > t.bounds[1][a] {
                            c - t.bounds[1][a]
                        } else {
                            0.0
                        };
                        d * d
                    })
                    .sum::<f64>();
                Some(RefinementCandidate {
                    key: t.key,
                    distance_squared,
                    error_px,
                })
            })
            .collect();
        let requested_passes = urgent
            .iter()
            .all(|c| c.error_px.is_finite() && c.error_px <= self.settings.pixel_error);
        let predicted = if requested_passes {
            self.prediction
                .as_ref()
                .map(|predicted| {
                    self.committed
                        .iter()
                        .filter_map(|tile| {
                            if !self.frustum_visible(tile, predicted, p) {
                                return None;
                            }
                            let error_px = self.projected_error(tile, predicted, p);
                            (error_px > self.settings.pixel_error * REFINEMENT_RESERVE).then(|| {
                                PredictionCandidate {
                                    key: tile.key,
                                    need_fraction: self
                                        .predicted_need_fraction(tile, camera, predicted, p),
                                    error_px,
                                }
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            vec![]
        };
        // One time-to-need slot may precede reserve-only work; requested
        // failures retain every slot. Closure and builder-byte admission still
        // bound the actual transaction, including necessary neighbor edges.
        let direct_native = direct_native_candidate(
            &urgent,
            self.settings.direct_native_urgent,
            self.settings.pixel_error,
            self.finest_cells(),
        );
        let (refine, predicted_footprint) = select_with_prediction(
            urgent,
            predicted,
            self.settings.pixel_error,
            self.settings.jobs_per_update,
        );
        // The experiment never adds work outside the existing priority budget.
        let direct_native = direct_native.filter(|key| refine.contains(key));
        self.predicted_footprint = predicted_footprint;
        let mut nodes = vec![];
        if same {
            for tile in self.committed.clone() {
                if !self.tile_visible(&tile, camera, p) && !self.prediction_protects(tile.key) {
                    let key = self
                        .cached_coarse(
                            tile.key.min,
                            tile.key.width_cells,
                            tile.key.step_cells,
                            camera,
                            p,
                        )
                        .unwrap_or(tile.key);
                    nodes.push(Node {
                        min: key.min,
                        width: key.width_cells,
                        step: key.step_cells,
                    });
                    continue;
                }
                let mut k = tile.key;
                if let Some(coarse) =
                    self.cached_coarse(k.min, k.width_cells, k.step_cells, camera, p)
                {
                    k = coarse;
                }
                if k != tile.key || self.accepts(&tile, camera, p) || !refine.contains(&tile.key) {
                    nodes.push(Node {
                        min: k.min,
                        width: k.width_cells,
                        step: k.step_cells,
                    });
                } else if k.width_cells > self.finest_cells() {
                    split(
                        Node {
                            min: k.min,
                            width: k.width_cells,
                            step: k.step_cells,
                        },
                        &mut nodes,
                    );
                } else {
                    nodes.push(Node {
                        min: k.min,
                        width: k.width_cells,
                        step: refined_leaf_step(k, direct_native),
                    });
                }
            }
        } else {
            let Some(extents) = all_heading_horizontal_extents(camera, width, height) else {
                frame.limit_reached = true;
                self.finish_build_memory(&mut frame);
                return frame;
            };
            let far = extents.map(|v| (v / VOXEL_SIZE).ceil() as i64);
            let root = (far[0].max(far[1]).max(512) as u64)
                .next_power_of_two()
                .min(1 << 20) as i64;
            let cell = [
                (camera.position.x / VOXEL_SIZE).floor() as i64,
                (camera.position.z / VOXEL_SIZE).floor() as i64,
            ];
            let start = [
                (cell[0] - far[0]).div_euclid(root) * root,
                (cell[1] - far[1]).div_euclid(root) * root,
            ];
            let counts = [0, 1].map(|a| ((cell[a] + far[a] - start[a]) / root + 1) as u128);
            if counts[0] * counts[1] > self.settings.max_nodes as u128 {
                frame.limit_reached = true;
                self.finish_build_memory(&mut frame);
                return frame;
            }
            let mut x = start[0];
            while x <= cell[0] + far[0] {
                let mut z = start[1];
                while z <= cell[1] + far[1] {
                    nodes.push(Node {
                        min: [x, z],
                        width: root as u32,
                        step: (root as u32 / 8).max(1),
                    });
                    z += root;
                }
                x += root;
            }
        }
        if !same && !self.committed.is_empty() {
            // Reuse spatial detail after every camera change. Starting over at
            // coarse roots each frame would erase near voxels during movement.
            let existing: HashMap<_, _> = self
                .committed
                .iter()
                .map(|t| ((t.key.min, t.key.width_cells), t.clone()))
                .collect();
            let mut ancestors = HashSet::new();
            for t in &self.committed {
                let mut w = t.key.width_cells;
                while w < (1 << 20) {
                    w *= 2;
                    let min = t.key.min.map(|v| v.div_euclid(w as i64) * w as i64);
                    ancestors.insert((min, w));
                }
            }
            let mut stack = nodes;
            nodes = vec![];
            while let Some(n) = stack.pop() {
                if let Some(t) = existing.get(&(n.min, n.width)) {
                    if !self.tile_visible(t, camera, p) && !self.prediction_protects(t.key) {
                        let key = self
                            .cached_coarse(
                                t.key.min,
                                t.key.width_cells,
                                t.key.step_cells,
                                camera,
                                p,
                            )
                            .unwrap_or(t.key);
                        nodes.push(Node {
                            min: key.min,
                            width: key.width_cells,
                            step: key.step_cells,
                        });
                        continue;
                    }
                    let coarse = self.cached_coarse(n.min, n.width, t.key.step_cells, camera, p);
                    let old = Node {
                        min: n.min,
                        width: n.width,
                        step: coarse.map(|k| k.step_cells).unwrap_or(t.key.step_cells),
                    };
                    if coarse.is_some() || self.accepts(t, camera, p) || !refine.contains(&t.key) {
                        nodes.push(old)
                    } else if n.width > self.finest_cells() {
                        split(old, &mut nodes)
                    } else {
                        nodes.push(Node {
                            step: refined_leaf_step(t.key, direct_native),
                            ..old
                        })
                    }
                } else if ancestors.contains(&(n.min, n.width)) {
                    split(n, &mut stack)
                } else {
                    nodes.push(n)
                }
            }
        }
        // Coarsen complete spatial quartets only after a parent certificate
        // passes the stricter hysteresis threshold. Unrelated regions stay put.
        loop {
            let mut groups: HashMap<([i64; 2], u32), Vec<usize>> = HashMap::new();
            for (i, n) in nodes.iter().enumerate() {
                if n.width < (1 << 20) {
                    let width = n.width * 2;
                    let min = n.min.map(|v| v.div_euclid(width as i64) * width as i64);
                    groups.entry((min, width)).or_default().push(i);
                }
            }
            let mut remove = HashSet::new();
            let mut parents = vec![];
            for ((min, width), children) in groups {
                if children.len() != 4 {
                    continue;
                }
                if let Some(k) = self.cached_coarse(min, width, 0, camera, p) {
                    remove.extend(children);
                    parents.push(Node {
                        min,
                        width,
                        step: k.step_cells,
                    });
                }
            }
            if parents.is_empty() {
                break;
            }
            nodes = nodes
                .into_iter()
                .enumerate()
                .filter_map(|(i, n)| (!remove.contains(&i)).then_some(n))
                .chain(parents)
                .collect();
        }
        // Ownership is an integer spatial partition at every distance. Split
        // intersecting nodes before adjacency; never cap a cave with a surface.
        if self.world.topology_base_cells().is_some() {
            let mut stack = nodes;
            nodes = vec![];
            while let Some(n) = stack.pop() {
                let end = [n.min[0] + n.width as i64, n.min[1] + n.width as i64];
                let overlap = n.min[0] < 128 && end[0] > -128 && n.min[1] < 128 && end[1] > -128;
                let contained =
                    n.min[0] >= -128 && end[0] <= 128 && n.min[1] >= -128 && end[1] <= 128;
                if contained {
                    continue;
                }
                if overlap {
                    split(n, &mut stack)
                } else {
                    nodes.push(n)
                }
            }
        }
        // Keep complete coarse roots, including offscreen regions. This is a
        // conservative coverage guard for camera movement during CPU work;
        // only visible nodes are refined and GPU frustum culling handles draws.
        // Balance the logical partition before selecting immutable geometry keys.
        loop {
            if nodes.len() > self.settings.max_nodes {
                frame.limit_reached = true;
                break;
            }
            let pairs = adjacency(&nodes);
            let mut divide = HashSet::new();
            for (a, _, b, _) in pairs {
                if nodes[a].width > nodes[b].width.saturating_mul(2) {
                    divide.insert(a);
                } else if nodes[b].width > nodes[a].width.saturating_mul(2) {
                    divide.insert(b);
                }
            }
            if divide.is_empty() {
                break;
            }
            let mut next = vec![];
            for (i, n) in nodes.into_iter().enumerate() {
                if divide.contains(&i) {
                    split(n, &mut next)
                } else {
                    next.push(n)
                }
            }
            nodes = next;
        }
        frame.visited = nodes.len();
        if frame.limit_reached {
            frame.tiles = self.committed.clone();
            frame.coverage_ready = same;
            if !same {
                self.revalidate(&mut frame, camera, width, height);
            }
            self.assess_quality(&mut frame, camera, p);
            self.finish_build_memory(&mut frame);
            return frame;
        }
        // Second pass: each physical shared edge has one canonical sample stride,
        // including the coarse edge split across two balanced fine neighbors.
        let pairs = adjacency(&nodes);
        let mut edges: Vec<[u32; 4]> = nodes.iter().map(|n| [n.step; 4]).collect();
        if self.world.topology_base_cells().is_some() {
            for (i, n) in nodes.iter().enumerate() {
                let end = [n.min[0] + n.width as i64, n.min[1] + n.width as i64];
                if n.min[1] < 128 && end[1] > -128 {
                    if n.min[0] == 128 {
                        edges[i][0] = 1
                    }
                    if end[0] == -128 {
                        edges[i][2] = 1
                    }
                }
                if n.min[0] < 128 && end[0] > -128 {
                    if n.min[1] == 128 {
                        edges[i][3] = 1
                    }
                    if end[1] == -128 {
                        edges[i][1] = 1
                    }
                }
            }
        }
        loop {
            let mut changed = false;
            for &(a, ea, b, eb) in &pairs {
                let step = edges[a][ea].min(edges[b][eb]);
                if edges[a][ea] != step || edges[b][eb] != step {
                    changed = true;
                    edges[a][ea] = step;
                    edges[b][eb] = step;
                }
            }
            if !changed {
                break;
            }
        }
        #[cfg(test)]
        self.diagnostic_target.clear();
        let mut target_keys = Vec::with_capacity(nodes.len());
        let mut wanted = VecDeque::new();
        let mut next = vec![];
        let mut complete = true;
        for (i, n) in nodes.iter().enumerate() {
            let key = TileKey {
                min: n.min,
                width_cells: n.width,
                step_cells: n.step,
                edge_steps: edges[i],
                source_revision: self.world.revision(),
            };
            target_keys.push(key);
            #[cfg(test)]
            self.diagnostic_target.push(key);
            if let Some(tile) = self.cache.get(key) {
                next.push(tile)
            } else {
                wanted.push_back(key);
                complete = false;
            }
        }
        let required_bytes = nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                self.known_bytes.get(&TileKey {
                    min: n.min,
                    width_cells: n.width,
                    step_cells: n.step,
                    edge_steps: edges[i],
                    source_revision: self.world.revision(),
                })
            })
            .copied()
            .fold(0usize, usize::saturating_add);
        let mesh_budget = self.cache.mesh_budget_bytes();
        if required_bytes > mesh_budget {
            // A coherent replacement cannot fit: retain coarse coverage and
            // report an explicit resource/quality deficit instead of thrashing.
            self.pending.clear();
            frame.limit_reached = true;
            frame.tiles = self.committed.clone();
            frame.coverage_ready = same;
            frame.cache = self.cache.stats();
            if !same {
                self.revalidate(&mut frame, camera, width, height);
            }
            self.assess_quality(&mut frame, camera, p);
            self.finish_build_memory(&mut frame);
            return frame;
        }
        // Low-priority probes regenerate evicted coarse content before it may
        // replace distant native geometry. Certificates still gate publication.
        if complete && required_bytes < mesh_budget * 4 / 5 {
            wanted.extend(self.coarsen_probes.iter().copied());
        }
        // Keep still-required jobs in order; camera movement does not change content.
        let desired: HashSet<_> = wanted.iter().copied().collect();
        self.pending.retain(|k| desired.contains(k));
        let mut queued: HashSet<_> = self.pending.iter().copied().collect();
        self.pending
            .extend(wanted.into_iter().filter(|k| queued.insert(*k)));
        if complete && self.coarsen_requested && !self.coarsening_certified(&next, camera, p) {
            // Cached candidates were checked before neighbor closure. Verify
            // the actual replacement edge keys before allowing any degradation.
            self.pending.clear();
            frame.tiles = self.committed.clone();
            frame.coverage_ready = same;
            self.revalidate(&mut frame, camera, width, height);
            self.assess_quality(&mut frame, camera, p);
            frame.cache = self.cache.stats();
            self.finish_build_memory(&mut frame);
            return frame;
        }
        if !complete {
            self.next_publication_id = self.next_publication_id.wrapping_add(1);
            self.pending_publication = Some(PublicationTransaction {
                id: self.next_publication_id,
                started: std::time::Instant::now(),
            });
            self.transaction_known_bytes = required_bytes;
            self.transaction = Some(target_keys);
        } else {
            // Repeated planning can run before the render side polls. Preserve
            // the token for an unchanged cover until that cover is replaced.
            if target_keys.len() != self.committed.len()
                || target_keys
                    .iter()
                    .zip(&self.committed)
                    .any(|(key, tile)| *key != tile.key)
            {
                self.next_publication_id = self.next_publication_id.wrapping_add(1);
                self.committed_publication = Some(PublicationTransaction {
                    id: self.next_publication_id,
                    started: std::time::Instant::now(),
                });
            }
            self.committed = next;
            self.committed_pose = Some((camera.clone(), width, height));
            self.refresh_occlusion(camera);
            frame.coverage_ready = true;
        }
        if !complete {
            frame.coverage_ready = same;
        }
        frame.tiles = self.committed.clone();
        self.assess_quality(&mut frame, camera, p);
        if self.known_bytes.len() > self.settings.max_nodes.saturating_mul(2).max(1024) {
            let keep: HashSet<_> = self
                .pending
                .iter()
                .copied()
                .chain(self.committed.iter().map(|t| t.key))
                .chain(self.transaction.iter().flatten().copied())
                .collect();
            self.known_bytes.retain(|k, _| keep.contains(k));
            let footprints: HashSet<_> = keep.iter().map(|k| (k.min, k.width_cells)).collect();
            self.rejected_coarsen
                .retain(|(min, width, _), _| footprints.contains(&(*min, *width)));
            self.variants.retain(|_, keys| {
                keys.retain(|k| self.known_bytes.contains_key(k));
                !keys.is_empty()
            });
        }
        frame.pending = self.pending.len();
        frame.cache = self.cache.stats();
        if !frame.coverage_ready {
            self.revalidate(&mut frame, camera, width, height);
        }
        self.finish_build_memory(&mut frame);
        frame
    }
    fn finish_build_memory(&mut self, frame: &mut SelectionFrame) {
        frame.publication_transaction = self.committed_publication;
        let pending: HashSet<_> = self.pending.iter().copied().collect();
        self.build_refused.retain(|key| pending.contains(key));
        let stats = self.cache.stats();
        self.build_memory.evicted_arc_bytes = stats.live_evicted_payload_bytes;
        frame.build_memory = self.build_memory;
        frame.limit_reached |= self
            .pending
            .iter()
            .any(|key| self.build_refused.contains(key));
    }
    fn revalidate(
        &mut self,
        frame: &mut SelectionFrame,
        camera: &Camera,
        width: u32,
        height: u32,
    ) -> bool {
        if !complete_coverage_guard(
            &frame.tiles,
            camera,
            width,
            height,
            self.world.topology_base_cells().is_some(),
        ) {
            return false;
        }
        let Some(p) = camera_projection(camera, width, height) else {
            return false;
        };
        frame.coverage_ready = true;
        self.refresh_occlusion(camera);
        self.assess_quality(frame, camera, p);
        true
    }
    fn assess_quality(&self, frame: &mut SelectionFrame, camera: &Camera, p: Projection) {
        let (mut max_error, mut finite) = (0.0_f64, true);
        frame.visible_unresolved_count = 0;
        frame.visible_uncertified_count = 0;
        frame.global_bound_clusters = 0;
        frame.local_certified_clusters = 0;
        frame.affine_bound_clusters = 0;
        frame.occluded_clusters = frame
            .tiles
            .iter()
            .filter(|t| self.frustum_visible(t, camera, p) && self.is_occluded(t, camera))
            .count();
        frame.occluder_boxes = self
            .occlusion
            .as_ref()
            .filter(|(pose, _)| same_pose(pose, camera))
            .map(|(_, s)| s.len())
            .unwrap_or(0);
        for tile in frame
            .tiles
            .iter()
            .filter(|t| self.tile_visible(t, camera, p))
        {
            let error = self.projected_error(tile, camera, p);
            if error.is_finite() {
                max_error = max_error.max(error)
            } else {
                finite = false;
            }
            if !error.is_finite() || error > self.settings.pixel_error {
                frame.visible_unresolved_count += 1;
            }
            match proof_kind(tile) {
                1 => frame.local_certified_clusters += 1,
                2 => frame.affine_bound_clusters += 1,
                3 => frame.global_bound_clusters += 1,
                4 => frame.visible_uncertified_count += 1,
                _ => {}
            }
        }
        frame.max_visible_error_px = finite.then_some(max_error);
        frame.quality_ready = frame.coverage_ready && frame.visible_unresolved_count == 0;
        frame.refinement_ready = frame.coverage_ready
            && finite
            && max_error <= self.settings.pixel_error * REFINEMENT_RESERVE;
    }
    fn coarsening_certified(
        &mut self,
        next: &[Arc<GeometryTile>],
        camera: &Camera,
        p: Projection,
    ) -> bool {
        let prior: HashMap<_, _> = self
            .committed
            .iter()
            .map(|t| ((t.key.min, t.key.width_cells), t.key.step_cells))
            .collect();
        let mut prior_ancestors = HashSet::new();
        for tile in &self.committed {
            let mut width = tile.key.width_cells;
            while width < (1 << 20) {
                width *= 2;
                let min = tile
                    .key
                    .min
                    .map(|v| v.div_euclid(width as i64) * width as i64);
                prior_ancestors.insert((min, width));
            }
        }
        let mut safe = true;
        for tile in next {
            let n = tile.key;
            let is_coarsening = prior
                .get(&(n.min, n.width_cells))
                .map(|&step| step < n.step_cells)
                .unwrap_or(false)
                || prior_ancestors.contains(&(n.min, n.width_cells));
            if is_coarsening
                && self.frustum_visible(tile, camera, p)
                && self.projected_error(tile, camera, p) > self.settings.pixel_error * 0.65
            {
                let error_m = tile
                    .best_certified_vertical_error_m
                    .unwrap_or(f64::INFINITY);
                self.rejected_coarsen.insert(
                    (n.min, n.width_cells, n.step_cells),
                    RejectedCoarsen {
                        error_m,
                        bounds: tile.bounds,
                    },
                );
                safe = false;
            }
        }
        safe
    }
    fn cached_coarse(
        &mut self,
        min: [i64; 2],
        width: u32,
        step: u32,
        camera: &Camera,
        p: Projection,
    ) -> Option<TileKey> {
        let mut keys = self.variants.get(&(min, width))?.clone();
        keys.sort_by_key(|k| std::cmp::Reverse(k.step_cells));
        for k in keys {
            if k.step_cells <= step {
                continue;
            }
            if let Some(rejected) = self
                .rejected_coarsen
                .get(&(min, width, k.step_cells))
                .copied()
            {
                let mut expanded = rejected.bounds;
                expanded[0][1] -= rejected.error_m;
                expanded[1][1] += rejected.error_m;
                if !outside_frustum(
                    world_bounds_to_view(expanded, camera),
                    p,
                    conservative_clip_depth(camera).unwrap_or(f64::INFINITY),
                ) && vertical_pixel_bound(rejected.error_m, rejected.bounds, camera, p)
                    > self.settings.pixel_error * 0.65
                {
                    continue;
                }
                self.rejected_coarsen.remove(&(min, width, k.step_cells));
            }
            if let Some(tile) = self.cache.get(k) {
                if (!self.tile_visible(&tile, camera, p)
                    || self.projected_error(&tile, camera, p) <= self.settings.pixel_error * 0.65)
                    && (!self.prediction_protects(tile.key)
                        || self.prediction.as_ref().is_none_or(|predicted| {
                            !self.frustum_visible(&tile, predicted, p)
                                || self.projected_error(&tile, predicted, p)
                                    <= self.settings.pixel_error * 0.65
                        }))
                {
                    let footprint = (min, width);
                    if !self.coarsen_targets.contains(&footprint)
                        && self.coarsen_targets.len() >= self.settings.jobs_per_update
                    {
                        return None;
                    }
                    self.coarsen_targets.insert(footprint);
                    self.coarsen_requested = true;
                    return Some(k);
                }
            } else if self.coarsen_probes.len() < self.settings.jobs_per_update {
                let cx = (min[0] as f64 + width as f64 * 0.5) * VOXEL_SIZE;
                let cz = (min[1] as f64 + width as f64 * 0.5) * VOXEL_SIZE;
                if (camera.position.x - cx).hypot(camera.position.z - cz)
                    > width as f64 * VOXEL_SIZE * 8.0
                {
                    self.coarsen_probes.push(k);
                }
            }
        }
        None
    }
    fn refresh_occlusion(&mut self, camera: &Camera) {
        let Some(mut set) = OcclusionSet::new(
            camera.position.to_array(),
            camera.direction().as_dvec3().to_array(),
            0.025,
            VOXEL_SIZE * 2.0,
        ) else {
            self.occlusion = None;
            return;
        };
        for tile in &self.committed {
            if let Some(solid) = tile.guaranteed_solid_box {
                set.insert_solid(solid);
            }
        }
        self.occlusion = Some((camera.clone(), set));
    }
    fn is_occluded(&self, tile: &GeometryTile, camera: &Camera) -> bool {
        self.occlusion
            .as_ref()
            .filter(|(pose, _)| same_pose(pose, camera))
            .map(|(_, set)| {
                set.occludes(Aabb {
                    min: tile.conservative_surface_bounds[0],
                    max: tile.conservative_surface_bounds[1],
                })
            })
            .unwrap_or(false)
    }
    fn frustum_visible(&self, tile: &GeometryTile, camera: &Camera, p: Projection) -> bool {
        !outside_frustum(
            world_bounds_to_view(tile.conservative_surface_bounds, camera),
            p,
            conservative_clip_depth(camera).unwrap_or(f64::INFINITY),
        )
    }
    fn tile_visible(&self, tile: &GeometryTile, camera: &Camera, p: Projection) -> bool {
        self.frustum_visible(tile, camera, p) && !self.is_occluded(tile, camera)
    }
    fn accepts(&self, tile: &GeometryTile, camera: &Camera, p: Projection) -> bool {
        self.projected_error(tile, camera, p) <= self.settings.pixel_error * REFINEMENT_RESERVE
            && (!self.prediction_protects(tile.key)
                || self.prediction.as_ref().is_none_or(|predicted| {
                    !self.frustum_visible(tile, predicted, p)
                        || self.projected_error(tile, predicted, p)
                            <= self.settings.pixel_error * REFINEMENT_RESERVE
                }))
    }
    fn prediction_protects(&self, key: TileKey) -> bool {
        self.predicted_footprint
            .is_some_and(|footprint| footprints_overlap(key, footprint))
    }
    fn predicted_need_fraction(
        &self,
        tile: &GeometryTile,
        camera: &Camera,
        predicted: &Camera,
        p: Projection,
    ) -> f64 {
        // Fixed metadata-only samples estimate priority, never certify quality.
        // Projection need can be nonmonotonic during turns and near-plane crossings.
        let yaw_delta = (predicted.yaw - camera.yaw + std::f32::consts::PI)
            .rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        for sample in 1..=8 {
            let fraction = sample as f64 / 8.0;
            let mut pose = camera.clone();
            pose.position = camera.position.lerp(predicted.position, fraction);
            pose.yaw += yaw_delta * fraction as f32;
            pose.pitch += (predicted.pitch - camera.pitch) * fraction as f32;
            if self.frustum_visible(tile, &pose, p)
                && self.projected_error(tile, &pose, p) > self.settings.pixel_error
            {
                return fraction;
            }
        }
        1.0
    }
    fn projected_error(&self, tile: &GeometryTile, camera: &Camera, p: Projection) -> f64 {
        let e = tile.best_certified_vertical_error_m;
        e.map(|e| vertical_pixel_bound(e, tile.bounds, camera, p))
            .unwrap_or(f64::INFINITY)
    }
    pub fn prepare(&mut self, cancelled: impl Fn() -> bool + Sync) -> Result<usize, String> {
        let mut completed = 0;
        while completed < self.settings.jobs_per_update {
            let count = self.prepare_jobs(self.settings.jobs_per_update - completed, &cancelled)?;
            if count == 0 {
                break;
            }
            completed += count;
        }
        Ok(completed)
    }
    /// Finish the already selected small coherent replacement without scanning
    /// the full cover between bounded builder batches. The cancellation callback
    /// checks current demand before every batch and during CPU construction.
    fn prepare_demand(&mut self, cancelled: impl Fn() -> bool + Sync) -> Result<usize, String> {
        let mut completed = 0;
        let mut remaining = self.settings.jobs_per_update;
        loop {
            if cancelled() {
                break;
            }
            let count = self.prepare_jobs(remaining, &cancelled)?;
            if count == 0 {
                break;
            }
            completed += count;
            remaining = remaining.saturating_sub(count);
            if self.transaction.is_some()
                && self.transaction_known_bytes > self.cache.mesh_budget_bytes()
            {
                break;
            }
            if remaining == 0 {
                if self.transaction.is_none()
                    || self.pending.is_empty()
                    || self.transaction_known_bytes > self.cache.mesh_budget_bytes()
                {
                    break;
                }
                self.planning.prepared_batches_without_plan += 1;
                remaining = self.settings.jobs_per_update;
            }
        }
        Ok(completed)
    }
    fn prepare_jobs(
        &mut self,
        limit: usize,
        cancelled: impl Fn() -> bool + Sync,
    ) -> Result<usize, String> {
        // Never queue payloads behind occupied workers. Only canonical keys sit
        // in `pending`; the synchronous bounded batch is drained before planning.
        let limit = limit.min(self.settings.worker_count).min(4);
        if cancelled() || limit == 0 {
            return Ok(0);
        }
        let cache_stats = self.cache.stats();
        let evicted = cache_stats.live_evicted_payload_bytes;
        let mut reserved = evicted;
        let mut keys = Vec::with_capacity(limit);
        for &key in self.pending.iter().take(limit) {
            let Some(bytes) = build_memory::reservation(key) else {
                if self.build_refused.insert(key) {
                    self.build_memory.refused_jobs += 1;
                }
                break;
            };
            if reserved.saturating_add(bytes) > self.settings.build_budget_bytes {
                // A temporarily full batch is smaller; an inadmissible first
                // job becomes an explicit resource deficit, preserving coverage.
                if keys.is_empty() && self.build_refused.insert(key) {
                    self.build_memory.refused_jobs += 1;
                }
                break;
            }
            reserved += bytes;
            keys.push(key);
        }
        self.build_memory.evicted_arc_bytes = evicted;
        self.build_memory.last_batch_jobs = keys.len();
        if keys.is_empty() {
            return Ok(0);
        }
        self.build_memory.reserved_bytes = reserved;
        self.build_memory.peak_reserved_bytes = self.build_memory.peak_reserved_bytes.max(reserved);
        let result = if self.settings.worker_count == 1 {
            self.cache
                .get_or_build(keys[0], &cancelled)
                .map(|tile| vec![tile])
        } else {
            self.cache.prepare_parallel(&keys, keys.len(), &cancelled)
        };
        self.build_memory.reserved_bytes = 0;
        let tiles = match result {
            Ok(tiles) => tiles,
            Err(error) => {
                // Parallel cancellation may still admit completed products.
                // Record their exact bytes once without removing pending keys.
                for &key in &keys {
                    if let Some(tile) = self.cache.get(key) {
                        self.record_prepared_tile(&tile);
                    }
                }
                return Err(error);
            }
        };
        // A cancelled/failed batch keeps every key pending. Successfully built
        // content stays reusable in the cache, and the next plan deduplicates it.
        for tile in &tiles {
            self.record_prepared_tile(tile);
        }
        for _ in 0..tiles.len() {
            self.pending.pop_front();
        }
        Ok(tiles.len())
    }
    fn record_prepared_tile(&mut self, tile: &GeometryTile) {
        let old_bytes = self.known_bytes.insert(tile.key, tile.bytes).unwrap_or(0);
        if self.transaction.is_some() {
            self.transaction_known_bytes = self
                .transaction_known_bytes
                .saturating_sub(old_bytes)
                .saturating_add(tile.bytes);
        }
        let variants = self
            .variants
            .entry((tile.key.min, tile.key.width_cells))
            .or_default();
        if !variants.contains(&tile.key) {
            variants.push(tile.key);
        }
    }
    pub fn cache_stats(&self) -> crate::adaptive_geometry::CacheStats {
        self.cache.stats()
    }
}
fn proof_kind(tile: &GeometryTile) -> u8 {
    if tile.exact {
        return 0;
    }
    let Some(best) = tile.best_certified_vertical_error_m else {
        return 4;
    };
    if tile.certified_error_m.map(|e| e <= best).unwrap_or(false) {
        1
    } else if tile.affine_error_m.map(|e| e <= best).unwrap_or(false) {
        2
    } else {
        3
    }
}
fn complete_coverage_guard(
    tiles: &[Arc<GeometryTile>],
    camera: &Camera,
    width: u32,
    height: u32,
    volume_owner: bool,
) -> bool {
    complete_key_guard(
        tiles.iter().map(|t| t.key),
        camera,
        width,
        height,
        volume_owner,
    )
}
/// Keys must come from an already verified, nonoverlapping complete partition.
/// This validates current view coverage without source generation or allocation.
pub(crate) fn published_partition_covers(
    keys: &HashSet<TileKey>,
    has_volume: bool,
    camera: &Camera,
    width: u32,
    height: u32,
) -> bool {
    complete_key_guard(keys.iter().copied(), camera, width, height, has_volume)
}
fn complete_key_guard(
    keys: impl Iterator<Item = TileKey> + Clone,
    camera: &Camera,
    width: u32,
    height: u32,
    volume_owner: bool,
) -> bool {
    if keys.clone().next().is_none() {
        return false;
    }
    let min = [0, 1].map(|a| keys.clone().map(|k| k.min[a]).min().unwrap());
    let max = [0, 1].map(|a| {
        keys.clone()
            .map(|k| k.min[a] + k.width_cells as i64)
            .max()
            .unwrap()
    });
    let area: i128 = keys.clone().map(|k| (k.width_cells as i128).pow(2)).sum();
    let mut expected = (max[0] - min[0]) as i128 * (max[1] - min[1]) as i128;
    if volume_owner {
        let dx = (max[0].min(128) - min[0].max(-128)).max(0) as i128;
        let dz = (max[1].min(128) - min[1].max(-128)).max(0) as i128;
        expected -= dx * dz;
    }
    let Some(extents) = frustum_horizontal_extents(camera, width, height) else {
        return false;
    };
    let far = extents.map(|v| v / VOXEL_SIZE);
    let cell = [
        camera.position.x / VOXEL_SIZE,
        camera.position.z / VOXEL_SIZE,
    ];
    area == expected
        && (0..2).all(|a| cell[a] - far[a] >= min[a] as f64 && cell[a] + far[a] <= max[a] as f64)
}
pub(crate) fn cached_bounds_visible(
    bounds: [[f64; 3]; 2],
    camera: &Camera,
    width: u32,
    height: u32,
) -> bool {
    camera_projection(camera, width, height)
        .map(|p| {
            !outside_frustum(
                world_bounds_to_view(bounds, camera),
                p,
                conservative_clip_depth(camera).unwrap_or(f64::INFINITY),
            )
        })
        .unwrap_or(true)
}
pub(crate) fn projected_cached_vertical_error(
    error_m: f64,
    bounds: [[f64; 3]; 2],
    camera: &Camera,
    width: u32,
    height: u32,
) -> f64 {
    camera_projection(camera, width, height)
        .map(|p| vertical_pixel_bound(error_m, bounds, camera, p))
        .unwrap_or(f64::INFINITY)
}
fn vertical_pixel_bound(
    error_m: f64,
    bounds: [[f64; 3]; 2],
    camera: &Camera,
    p: Projection,
) -> f64 {
    let forward = camera.direction().as_dvec3().normalize();
    let right = forward.cross(DVec3::Y).normalize();
    let up = right.cross(forward);
    p.projected_vertical_error_px(
        error_m,
        world_bounds_to_view(bounds, camera),
        [right.y, up.y, forward.y],
    )
}
/// GPU far clip is a forward-depth plane, not a radial sphere. Bound its
/// corners in world X/Z using the support function of the camera-space box.
fn camera_projection(camera: &Camera, width: u32, height: u32) -> Option<Projection> {
    if width == 0
        || height == 0
        || !camera.fov_y.is_finite()
        || camera.fov_y <= 0.0
        || camera.fov_y >= 180.0
        || conservative_clip_depth(camera).is_none()
    {
        return None;
    }
    let matrix = glam::Mat4::perspective_rh(
        camera.fov_y.to_radians(),
        width as f32 / height as f32,
        0.025,
        camera.far,
    );
    if !matrix.x_axis.x.is_finite()
        || !matrix.y_axis.y.is_finite()
        || matrix.x_axis.x <= 0.0
        || matrix.y_axis.y <= 0.0
    {
        return None;
    }
    Some(Projection {
        vertical_fov_radians: 2.0 * (1.0 / matrix.y_axis.y as f64).atan(),
        width,
        height,
        near_m: 0.025,
    })
}
fn conservative_clip_depth(camera: &Camera) -> Option<f64> {
    if !camera.far.is_finite() || camera.far <= 0.025 {
        return None;
    }
    // Match glam's f32 depth coefficients. Their rounding can move the actual
    // GPU far plane beyond the requested distance, especially at long range.
    let r = camera.far / (0.025_f32 - camera.far);
    let far = (r * 0.025_f32) as f64 / (r as f64 + 1.0);
    if !far.is_finite() || far <= 0.0 {
        None
    } else {
        Some(far.max(camera.far as f64) * (1.0 + 1e-6))
    }
}
fn all_heading_horizontal_extents(camera: &Camera, width: u32, height: u32) -> Option<[f64; 2]> {
    camera_projection(camera, width, height)?;
    let matrix = glam::Mat4::perspective_rh(
        camera.fov_y.to_radians(),
        width as f32 / height as f32,
        0.025,
        camera.far,
    );
    let tx = 1.0 / matrix.x_axis.x as f64;
    let ty = 1.0 / matrix.y_axis.y as f64;
    let radius = conservative_clip_depth(camera)? * (1.0 + tx * tx + ty * ty).sqrt() * (1.0 + 1e-6);
    (radius.is_finite() && radius > 0.0).then_some([radius; 2])
}
fn frustum_horizontal_extents(camera: &Camera, width: u32, height: u32) -> Option<[f64; 2]> {
    if width == 0
        || height == 0
        || !camera.fov_y.is_finite()
        || camera.fov_y <= 0.0
        || camera.fov_y >= 180.0
    {
        return None;
    }
    let f = camera.direction().as_dvec3().normalize();
    let r = f.cross(DVec3::Y).normalize();
    let u = r.cross(f);
    let projection = glam::Mat4::perspective_rh(
        camera.fov_y.to_radians(),
        width as f32 / height as f32,
        0.025,
        camera.far,
    );
    let tx = 1.0 / projection.x_axis.x as f64;
    let ty = 1.0 / projection.y_axis.y as f64;
    if !tx.is_finite() || !ty.is_finite() || tx <= 0.0 || ty <= 0.0 {
        return None;
    }
    let far = conservative_clip_depth(camera)?;
    let extents =
        [0, 2].map(|a| far * (f[a].abs() + tx * r[a].abs() + ty * u[a].abs()) * (1.0 + 1e-6));
    if extents
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0 || *v / VOXEL_SIZE > (1_u64 << 50) as f64)
    {
        None
    } else {
        Some(extents)
    }
}
fn same_pose(a: &Camera, b: &Camera) -> bool {
    a.position == b.position
        && a.yaw == b.yaw
        && a.pitch == b.pitch
        && a.far == b.far
        && a.fov_y == b.fov_y
}
fn split(n: Node, out: &mut Vec<Node>) {
    let half = n.width / 2;
    for dz in [0, half] {
        for dx in [0, half] {
            out.push(Node {
                min: [n.min[0] + dx as i64, n.min[1] + dz as i64],
                width: half,
                step: (half / 8).max(1),
            });
        }
    }
}
/// Interval sweep of positive/negative edges on each shared coordinate plane.
fn adjacency(nodes: &[Node]) -> Vec<(usize, usize, usize, usize)> {
    use std::collections::BTreeMap;
    type Edge = (i64, i64, usize, usize, bool);
    let mut groups: BTreeMap<(u8, i64), Vec<Edge>> = BTreeMap::new();
    for (i, n) in nodes.iter().enumerate() {
        let [x, z] = n.min;
        let w = n.width as i64;
        for (axis, plane, start, edge, positive) in [
            (0, x, z, 0, false),
            (0, x + w, z, 2, true),
            (1, z, x, 3, false),
            (1, z + w, x, 1, true),
        ] {
            groups
                .entry((axis, plane))
                .or_default()
                .push((start, start + w, i, edge, positive));
        }
    }
    let mut pairs = vec![];
    for list in groups.values() {
        let mut a: Vec<_> = list.iter().filter(|e| e.4).collect();
        let mut b: Vec<_> = list.iter().filter(|e| !e.4).collect();
        a.sort_by_key(|e| e.0);
        b.sort_by_key(|e| e.0);
        let (mut i, mut j) = (0, 0);
        while i < a.len() && j < b.len() {
            let (x, y) = (a[i], b[j]);
            if x.0 < y.1 && y.0 < x.1 {
                pairs.push((x.2, x.3, y.2, y.3));
            }
            if x.1 <= y.1 {
                i += 1
            } else {
                j += 1
            }
        }
    }
    pairs
}
fn world_bounds_to_view(bounds: [[f64; 3]; 2], camera: &Camera) -> ViewBounds {
    let forward = camera.direction().as_dvec3().normalize();
    let right = forward.cross(DVec3::Y).normalize();
    let up = right.cross(forward);
    let mut result = ViewBounds {
        min: [f64::INFINITY; 3],
        max: [f64::NEG_INFINITY; 3],
    };
    for x in 0..2 {
        for y in 0..2 {
            for z in 0..2 {
                let d = DVec3::new(bounds[x][0], bounds[y][1], bounds[z][2]) - camera.position;
                let v = [d.dot(right), d.dot(up), d.dot(forward)];
                for a in 0..3 {
                    result.min[a] = result.min[a].min(v[a]);
                    result.max[a] = result.max[a].max(v[a]);
                }
            }
        }
    }
    result
}
fn outside_frustum(b: ViewBounds, p: Projection, far: f64) -> bool {
    let ty = (p.vertical_fov_radians * 0.5).tan() * (1.0 + 1e-6);
    let tx = ty * p.width as f64 / p.height as f64;
    // Interval plane maxima: conservative even for AABBs spanning the camera.
    b.max[2] < p.near_m
        || b.min[2] > far
        || b.max[0] + tx * b.max[2] < 0.0
        || -b.min[0] + tx * b.max[2] < 0.0
        || b.max[1] + ty * b.max[2] < 0.0
        || -b.min[1] + ty * b.max[2] < 0.0
}
/// Diagnostic only: invert the actual f32 clip matrix in f64, enclosing all
/// eight near/far corners. Generous primary-scale padding covers matrix and
/// vertex rounding; no production visibility decision uses this experiment.
#[cfg(test)]
fn diagnostic_world_frustum_box(camera: &Camera, width: u32, height: u32) -> Option<[[f64; 3]; 2]> {
    camera_projection(camera, width, height)?;
    let matrix = camera.matrix(camera.position, width as f32 / height as f32);
    let inverse = glam::DMat4::from_cols_array(&matrix.to_cols_array().map(f64::from)).inverse();
    if !inverse.is_finite() {
        return None;
    }
    let mut bounds = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
    for x in [-1.0, 1.0] {
        for y in [-1.0, 1.0] {
            for z in [0.0, 1.0] {
                let h = inverse * glam::DVec4::new(x, y, z, 1.0);
                if !h.is_finite() || h.w <= 0.0 {
                    return None;
                }
                let v = camera.position + h.truncate() / h.w;
                for a in 0..3 {
                    bounds[0][a] = bounds[0][a].min(v[a]);
                    bounds[1][a] = bounds[1][a].max(v[a]);
                }
            }
        }
    }
    let pad = 0.01
        + conservative_clip_depth(camera)? * 1e-5
        + camera.position.abs().max_element() * 128.0 * f64::EPSILON;
    for a in 0..3 {
        bounds[0][a] -= pad;
        bounds[1][a] += pad;
    }
    Some(bounds)
}
#[cfg(test)]
fn diagnostic_boxes_disjoint(a: [[f64; 3]; 2], b: [[f64; 3]; 2]) -> bool {
    (0..3).any(|axis| a[1][axis] < b[0][axis] || a[0][axis] > b[1][axis])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direct_native_experiment_selects_only_one_nearest_requested_native_leaf() {
        let candidate = |x, width, step, distance, error| RefinementCandidate {
            key: TileKey {
                min: [x, 0],
                width_cells: width,
                step_cells: step,
                edge_steps: [step; 4],
                source_revision: 0,
            },
            distance_squared: distance,
            error_px: error,
        };
        let near = candidate(0, 128, 16, 4.0, 8.0);
        let far = candidate(128, 128, 16, 16.0, 80.0);
        let reserve = candidate(256, 128, 16, 0.0, 3.5);
        let large = candidate(512, 256, 32, 0.0, f64::INFINITY);
        let native = candidate(768, 128, 1, 0.0, f64::INFINITY);
        let candidates = [far, reserve, large, native, near];
        let selected = direct_native_candidate(&candidates, true, 4.0, 128);
        assert_eq!(selected, Some(near.key));
        assert_eq!(refined_leaf_step(near.key, selected), 1);
        assert_eq!(refined_leaf_step(far.key, selected), 8);
        assert_eq!(refined_leaf_step(reserve.key, selected), 8);
        assert_eq!(direct_native_candidate(&candidates, false, 4.0, 128), None);
        assert_eq!(direct_native_candidate(&candidates, true, 4.0, 64), None);
        assert_eq!(direct_native_candidate(&[reserve], true, 4.0, 128), None);
        let unbounded = candidate(-128, 128, 16, 1.0, f64::INFINITY);
        assert_eq!(
            direct_native_candidate(&[near, unbounded], true, 4.0, 128),
            Some(unbounded.key)
        );
        assert!(!StreamSettings::default().direct_native_urgent);
    }
    #[test]
    fn direct_native_planning_keeps_old_certificate_until_native_is_built() {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let mut stream = AdaptiveStream::new(
            world.clone(),
            StreamSettings {
                direct_native_urgent: true,
                pixel_error: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        let key = TileKey {
            min: [0, 0],
            width_cells: 128,
            step_cells: 16,
            edge_steps: [16; 4],
            source_revision: world.revision(),
        };
        let tile = stream.cache.get_or_build(key, || false).unwrap();
        assert!(!tile.exact);
        let certificate = tile.best_certified_vertical_error_m;
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(4.0, world.height_m(4.0, 4.0) + 1.7, 4.0);
        camera.far = 1.0;
        stream.committed = vec![tile.clone()];
        stream.committed_pose = Some((camera, 320, 180));
        let frame = stream.plan(&camera, 320, 180);
        assert!(stream
            .transaction
            .as_ref()
            .unwrap()
            .iter()
            .any(|key| key.step_cells == 1));
        assert_eq!(frame.tiles.len(), 1);
        assert!(Arc::ptr_eq(&frame.tiles[0], &tile));
        assert_eq!(frame.tiles[0].best_certified_vertical_error_m, certificate);
        assert!(!frame.tiles[0].exact);
        assert!(!frame.quality_ready);
        assert_eq!(
            frame.cache.builds, 1,
            "planning cannot fabricate native geometry"
        );
    }
    #[test]
    fn bounded_priority_advances_near_and_far_requested_errors_before_reserve() {
        fn candidate(x: i64, distance: f64, error: f64) -> RefinementCandidate {
            RefinementCandidate {
                key: TileKey {
                    min: [x, 0],
                    width_cells: 128,
                    step_cells: 8,
                    edge_steps: [8; 4],
                    source_revision: 0,
                },
                distance_squared: distance * distance,
                error_px: error,
            }
        }
        let near = candidate(0, 5.0, 5.0);
        let far = candidate(128, 304.0, 500.0);
        let unbounded = candidate(256, 1.0, f64::INFINITY);
        let requested = candidate(384, 10.0, 6.0);
        let reserve = candidate(512, 0.5, 3.5);
        let selected = select_refinements(vec![reserve, far, requested, unbounded, near], 4.0, 4);
        assert_eq!(selected.len(), 4);
        assert!(
            selected.contains(&near.key) && selected.contains(&far.key),
            "near and far requested errors must both advance"
        );
        assert!(
            selected.contains(&unbounded.key),
            "near-plane/unknown work keeps a protected slot"
        );
        assert!(selected.contains(&requested.key));
        assert!(
            !selected.contains(&reserve.key),
            "reserve-only work cannot displace a requested-quality failure"
        );
        let tied = candidate(-128, 400.0, 500.0);
        let selected = select_refinements(vec![far, tied], 4.0, 1);
        assert_eq!(
            selected,
            HashSet::from([tied.key]),
            "largest-error ties use canonical keys, independent of input order"
        );
        assert_eq!(
            select_refinements(vec![far, unbounded], 4.0, 1),
            HashSet::from([unbounded.key])
        );
    }
    #[test]
    fn diagnostic_frustum_box_encloses_clip_interior_and_rejects_behind_heading() {
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(0.0, 94.8875, 0.0);
        camera.far = 300.0;
        camera.pitch = -0.05;
        camera.yaw = 0.0;
        let bounds = diagnostic_world_frustum_box(&camera, 1920, 1080).unwrap();
        assert!(bounds[0][2] > 0.0);
        assert!(diagnostic_boxes_disjoint(
            [[-8.0, -1000.0, -8.0], [8.0, 1000.0, 0.0]],
            bounds
        ));
        for yaw in [0.0, 0.7, 1.5, 3.14] {
            for pitch in [-1.4, -0.05, 0.8] {
                camera.yaw = yaw;
                camera.pitch = pitch;
                let bounds = diagnostic_world_frustum_box(&camera, 1920, 1080).unwrap();
                let matrix = camera.matrix(camera.position, 1920.0 / 1080.0);
                let inverse =
                    glam::DMat4::from_cols_array(&matrix.to_cols_array().map(f64::from)).inverse();
                for x in [-1.0, -0.5, 0.0, 0.5, 1.0] {
                    for y in [-1.0, 0.0, 1.0] {
                        for z in [0.0, 0.25, 0.75, 1.0] {
                            let h = inverse * glam::DVec4::new(x, y, z, 1.0);
                            let v = camera.position + h.truncate() / h.w;
                            assert!((0..3).all(|a| v[a] >= bounds[0][a] && v[a] <= bounds[1][a]));
                        }
                    }
                }
            }
        }
    }
    /// CPU-only wall-clock reproduction; run without simultaneous GPU timing.
    #[test]
    #[ignore]
    fn diagnose_turn_return_stationary_tail() {
        use std::{
            hash::{Hash, Hasher},
            time::{Duration, Instant},
        };
        fn key_hash(keys: impl Iterator<Item = TileKey>) -> u64 {
            let mut keys: Vec<_> = keys.collect();
            keys.sort();
            let mut h = std::collections::hash_map::DefaultHasher::new();
            keys.hash(&mut h);
            h.finish()
        }
        let width: u32 = std::env::var("FRONTIER_DIAG_WIDTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1920);
        let height = width * 9 / 16;
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let mut camera = Camera::fixture();
        camera.set_explicit_pose(
            Some([0.0, 94.8875, 0.0]),
            Some([-0.00000015, 94.83756238, -0.99875236]),
        );
        camera.far = 300.0;
        let initial = camera;
        let mut stream = AdaptiveStream::new(
            world.clone(),
            StreamSettings {
                pixel_error: 4.0,
                ..StreamSettings::default()
            },
        )
        .unwrap();
        let leaf: u32 = std::env::var("FRONTIER_DIAG_LEAF_CELLS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(128);
        stream.set_test_finest_cells(leaf);
        eprintln!("DIAG leaf_cells={leaf}");
        let warm = Instant::now();
        loop {
            let frame = stream.plan(&camera, width, height);
            if frame.quality_ready || frame.limit_reached {
                eprintln!(
                    "DIAG warm={:.3} ready={} limited={} builds={}",
                    warm.elapsed().as_secs_f64(),
                    frame.quality_ready,
                    frame.limit_reached,
                    frame.cache.builds
                );
                break;
            }
            assert!(warm.elapsed() < Duration::from_secs(90), "warmup timed out");
            stream.prepare(|| false).unwrap();
        }
        let started = Instant::now();
        let (mut second, mut commits, mut target_changes, mut last_commit, mut last_target) =
            (0_u64, 0_u64, 0_u64, 0, 0);
        while started.elapsed() < Duration::from_secs(12) {
            let t = started.elapsed().as_secs_f64();
            let pose = frontier_runtime::routes::Route::TurnReturn.pose_at(
                t,
                initial.position.to_array(),
                initial.yaw as f64,
                initial.pitch as f64,
                3.0,
            );
            camera.position = DVec3::from_array(pose.position);
            camera.position.y = camera
                .position
                .y
                .max(world.height_m(camera.position.x, camera.position.z) + 1.7);
            camera.yaw = pose.yaw as f32;
            camera.pitch = pose.pitch as f32;
            let frame = stream.plan(&camera, width, height);
            let commit = key_hash(frame.tiles.iter().map(|t| t.key));
            let target = key_hash(stream.diagnostic_target.iter().copied());
            commits += u64::from(commit != last_commit);
            target_changes += u64::from(target != last_target);
            last_commit = commit;
            last_target = target;
            if t as u64 >= second {
                eprintln!("DIAG t={t:.2} coverage={} ready={} limited={} tiles={} native={} pending={} commit={commit:x} target={target:x} commits={commits} target_changes={target_changes} builds={} generated={} evictions={} bytes={} error={:?} violations={}",frame.coverage_ready,frame.quality_ready,frame.limit_reached,frame.tiles.len(),frame.tiles.iter().filter(|t|t.exact).count(),frame.pending,frame.cache.builds,frame.cache.generated_columns,frame.cache.evictions,frame.cache.resident_bytes,frame.max_visible_error_px,frame.visible_unresolved_count);
                let p = camera_projection(&camera, width, height).unwrap();
                let frustum = diagnostic_world_frustum_box(&camera, width, height).unwrap();
                let mut violations: Vec<_> = frame
                    .tiles
                    .iter()
                    .filter(|tile| stream.tile_visible(tile, &camera, p))
                    .map(|tile| (stream.projected_error(tile, &camera, p), tile))
                    .filter(|(error, _)| !error.is_finite() || *error > stream.settings.pixel_error)
                    .collect();
                violations.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.key.cmp(&b.1.key)));
                let rejected = violations
                    .iter()
                    .filter(|(_, tile)| {
                        diagnostic_boxes_disjoint(tile.conservative_surface_bounds, frustum)
                    })
                    .count();
                let remaining_unbounded = violations
                    .iter()
                    .filter(|(error, tile)| {
                        !error.is_finite()
                            && !diagnostic_boxes_disjoint(tile.conservative_surface_bounds, frustum)
                    })
                    .count();
                eprintln!("FRUSTUM t={t:.2} box={frustum:?} violations={} outside_world_box={rejected} remaining_unbounded={remaining_unbounded}",violations.len());
                if t >= 5.0 {
                    for (error, tile) in violations.iter().take(8) {
                        eprintln!("OFFENDER t={t:.2} key={:?} error_px={error} vertical_m={:?} proxy={:?} canonical={:?} view={:?} outside_world_box={}",tile.key,tile.best_certified_vertical_error_m,tile.bounds,tile.conservative_surface_bounds,world_bounds_to_view(tile.bounds,&camera),diagnostic_boxes_disjoint(tile.conservative_surface_bounds,frustum));
                    }
                }
                second = t as u64 + 1;
            }
            stream.prepare(|| false).unwrap();
        }
    }
    #[test]
    fn retained_transactions_commit_distinct_geometry_during_continuous_demand() {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(0.0, world.height_m(0.0, 0.0) + 1.7, 0.0);
        camera.far = 3.0;
        let mut stream = AdaptiveStream::new(
            world,
            StreamSettings {
                pixel_error: 0.0,
                ..StreamSettings::default()
            },
        )
        .unwrap();
        let mut distinct = HashSet::new();
        let mut retained = None;
        for _ in 0..160 {
            camera.yaw += 0.0001;
            camera.position.x += 0.00001;
            let frame = stream.plan(&camera, 160, 90);
            if frame.coverage_ready {
                assert!(complete_coverage_guard(
                    &frame.tiles,
                    &camera,
                    160,
                    90,
                    false
                ));
                let mut keys: Vec<_> = frame.tiles.iter().map(|t| t.key).collect();
                keys.sort();
                distinct.insert(keys);
            }
            if let (Some(before), Some(after)) = (&retained, &stream.transaction) {
                // Same valid footprint cannot change in the middle of a batch.
                assert_eq!(before, after);
            }
            retained = stream.transaction.clone();
            stream.prepare_jobs(1, || false).unwrap();
            if distinct.len() >= 3 {
                break;
            }
        }
        assert!(
            distinct.len() >= 3,
            "completed source work must become distinct coherent partitions during motion"
        );
    }
    #[test]
    fn coarse_guard_contains_all_headings_and_reserve_is_separate() {
        let mut camera = Camera::fixture();
        camera.far = 300.0;
        let extents = all_heading_horizontal_extents(&camera, 1920, 1080).unwrap();
        for yaw in 0..72 {
            camera.yaw = yaw as f32 * std::f32::consts::TAU / 72.0;
            for pitch in [-1.5, -0.5, 0.0, 0.5, 1.5] {
                camera.pitch = pitch;
                let view = frustum_horizontal_extents(&camera, 1920, 1080).unwrap();
                assert!(view[0] <= extents[0] && view[1] <= extents[1]);
            }
        }
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let mut stream = AdaptiveStream::new(world.clone(), StreamSettings::default()).unwrap();
        let tile = stream
            .cache
            .get_or_build(
                TileKey {
                    min: [0, 0],
                    width_cells: 128,
                    step_cells: 16,
                    edge_steps: [16; 4],
                    source_revision: world.revision(),
                },
                || false,
            )
            .unwrap();
        camera.position = DVec3::new(4.0, tile.bounds[1][1] + 100.0, 4.0);
        camera.pitch = -1.4;
        let p = camera_projection(&camera, 1920, 1080).unwrap();
        let error = stream.projected_error(&tile, &camera, p);
        assert!(error.is_finite() && error > 0.0);
        stream.settings.pixel_error = error / 0.9;
        let mut frame = stream.plan(&camera, 1920, 1080);
        frame.tiles = vec![tile];
        frame.coverage_ready = true;
        stream.assess_quality(&mut frame, &camera, p);
        assert!(
            frame.quality_ready,
            "the requested error contract is satisfied"
        );
        assert!(
            !frame.refinement_ready,
            "reserve completion is a separate stricter condition"
        );
    }
    #[test]
    fn offscreen_coarsening_has_a_bounded_transaction_budget() {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let mut stream = AdaptiveStream::new(world.clone(), StreamSettings::default()).unwrap();
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(0.0, 10000.0, 0.0);
        camera.far = 1.0;
        let p = camera_projection(&camera, 320, 180).unwrap();
        let mut accepted = 0;
        for i in 0..8 {
            let key = TileKey {
                min: [i * 16, 0],
                width_cells: 16,
                step_cells: 2,
                edge_steps: [2; 4],
                source_revision: world.revision(),
            };
            stream.cache.get_or_build(key, || false).unwrap();
            stream.variants.insert((key.min, 16), vec![key]);
            accepted += usize::from(stream.cached_coarse(key.min, 16, 1, &camera, p).is_some());
        }
        assert_eq!(accepted, stream.settings.jobs_per_update);
        assert_eq!(stream.coarsen_targets.len(), accepted);
    }
    #[test]
    fn coarse_roots_publish_before_quality_refinement() {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(0.0, world.height_m(0.0, 0.0) + 1.7, 0.0);
        camera.far = 10.0;
        let mut stream = AdaptiveStream::new(world, StreamSettings::default()).unwrap();
        let cold = stream.plan(&camera, 1920, 1080);
        assert!(!cold.coverage_ready);
        assert!(cold.pending > 0);
        while !stream.pending.is_empty() {
            stream.prepare(|| false).unwrap();
        }
        let coarse = stream.plan(&camera, 1920, 1080);
        assert!(coarse.coverage_ready);
        assert!(!coarse.tiles.is_empty());
        for tile in &coarse.tiles {
            assert!(tile.key.step_cells > 1);
            assert!(tile.key.edge_steps.iter().all(|&e| e > 1));
        }
        let refining = stream.plan(&camera, 1920, 1080);
        assert!(refining.coverage_ready);
    }
    #[test]
    fn coverage_uses_actual_f32_gpu_far_plane() {
        let mut camera = Camera::fixture();
        for far in [300.0_f32, 20000.0, 100000.0] {
            camera.far = far;
            let projection =
                glam::Mat4::perspective_rh(camera.fov_y.to_radians(), 1920.0 / 1080.0, 0.025, far);
            let gpu_far = projection.w_axis.z as f64 / (projection.z_axis.z as f64 + 1.0);
            assert!(conservative_clip_depth(&camera).unwrap() >= gpu_far);
        }
        camera.far = 10000000.0;
        assert!(conservative_clip_depth(&camera).is_none());
        assert!(frustum_horizontal_extents(&camera, 1920, 1080).is_none());
    }
    #[test]
    fn depth_clip_far_corners_fit_root_coverage_support() {
        let mut camera = Camera::fixture();
        camera.far = 300.0;
        for yaw in [-2.1_f32, -0.8, 0.0, 0.8, 2.1] {
            for pitch in [-0.7_f32, 0.0, 0.7] {
                camera.yaw = yaw;
                camera.pitch = pitch;
                let extents = frustum_horizontal_extents(&camera, 1920, 1080).unwrap();
                let f = camera.direction().as_dvec3().normalize();
                let r = f.cross(DVec3::Y).normalize();
                let u = r.cross(f);
                let ty = (camera.fov_y as f64 * std::f64::consts::PI / 360.0).tan();
                let tx = ty * 1920.0 / 1080.0;
                for sx in [-1.0, 1.0] {
                    for sy in [-1.0, 1.0] {
                        let corner = (f + r * (tx * sx) + u * (ty * sy)) * camera.far as f64;
                        assert!(corner.x.abs() <= extents[0]);
                        assert!(corner.z.abs() <= extents[1]);
                    }
                }
            }
        }
        camera.yaw = std::f32::consts::FRAC_PI_4;
        camera.pitch = 0.0;
        assert!(frustum_horizontal_extents(&camera, 1920, 1080)
            .unwrap()
            .iter()
            .any(|&e| e > camera.far as f64 * 1.3));
    }
    #[test]
    fn accepted_cached_proxy_coarsens_native_geometry_with_hysteresis() {
        let world = World::fbm(
            7,
            frontier_world::FbmParams {
                amplitude: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(4.0, 1.0, 4.0);
        camera.far = 10.0;
        let mut stream = AdaptiveStream::new(world, StreamSettings::default()).unwrap();
        let exact = TileKey {
            min: [0, 0],
            width_cells: 128,
            step_cells: 1,
            edge_steps: [1; 4],
            source_revision: 0,
        };
        let proxy = TileKey {
            step_cells: 16,
            edge_steps: [16; 4],
            ..exact
        };
        stream.pending.extend([exact, proxy]);
        stream.prepare(|| false).unwrap();
        stream.committed = vec![stream.cache.get(exact).unwrap()];
        stream.committed_pose = Some((camera.clone(), 320, 180));
        let frame = stream.plan(&camera, 320, 180);
        assert!(frame.coverage_ready);
        assert_eq!(frame.tiles.len(), 1);
        assert_eq!(frame.tiles[0].key.step_cells, 16);
    }
    #[test]
    fn actual_neighbor_edge_certificate_gates_coarsening_publication() {
        let world = World::fbm(
            7,
            frontier_world::FbmParams {
                amplitude: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(4.0, 1.0, -100.0);
        camera.yaw = 0.0;
        camera.pitch = 0.0;
        camera.far = 300.0;
        let mut stream = AdaptiveStream::new(
            world.clone(),
            StreamSettings {
                pixel_error: 0.5,
                ..Default::default()
            },
        )
        .unwrap();
        let exact = TileKey {
            min: [0, 0],
            width_cells: 128,
            step_cells: 1,
            edge_steps: [1; 4],
            source_revision: 0,
        };
        stream.committed = vec![Arc::new(
            crate::adaptive_geometry::build(&world, exact, || false).unwrap(),
        )];
        let coarse = TileKey {
            step_cells: 16,
            edge_steps: [16; 4],
            ..exact
        };
        let mut replacement = crate::adaptive_geometry::build(&world, coarse, || false).unwrap();
        // Simulate a measured residual introduced by the actual closure key,
        // even though the previously admitted candidate had a smaller bound.
        replacement.certified_error_m = Some(1.0);
        replacement.best_certified_vertical_error_m = Some(1.0);
        replacement.affine_error_m = None;
        replacement.global_error_m = None;
        let p = Projection {
            vertical_fov_radians: std::f64::consts::PI / 3.0,
            width: 320,
            height: 180,
            near_m: 0.025,
        };
        let replacement = Arc::new(replacement);
        assert!(!stream.coarsening_certified(&[replacement.clone()], &camera, p));
        assert!(stream.rejected_coarsen.contains_key(&(
            coarse.min,
            coarse.width_cells,
            coarse.step_cells
        )));
        camera.position.z = -10000.0;
        camera.far = 20000.0;
        assert!(stream.coarsening_certified(&[replacement], &camera, p));
    }
    #[test]
    fn coherent_mesh_budget_reserves_source_cache_and_stops_thrashing() {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(0.0, 10000.0, 0.0);
        camera.far = 1.0;
        let mut stream = AdaptiveStream::new(world, StreamSettings::default()).unwrap();
        stream.plan(&camera, 320, 180);
        while !stream.pending.is_empty() {
            stream.prepare(|| false).unwrap();
        }
        let cover = stream.plan(&camera, 320, 180);
        assert!(cover.coverage_ready);
        let mesh_bytes = cover.tiles.iter().map(|t| t.bytes).sum::<usize>();
        // A new fixed-budget cache must reserve source capacity even when a
        // caller still owns the previously published complete mesh partition.
        stream = AdaptiveStream::new(
            stream.world.clone(),
            StreamSettings {
                cpu_cache_bytes: mesh_bytes,
                ..StreamSettings::default()
            },
        )
        .unwrap();
        stream.committed = cover.tiles;
        stream.committed_pose = Some((camera, 320, 180));
        for tile in &stream.committed {
            stream.known_bytes.insert(tile.key, tile.bytes);
        }
        let limited = stream.plan(&camera, 320, 180);
        assert!(limited.coverage_ready);
        assert!(limited.limit_reached);
        assert_eq!(limited.pending, 0);
        let builds = limited.cache.builds;
        assert_eq!(stream.prepare(|| false).unwrap(), 0);
        assert_eq!(stream.cache.stats().builds, builds);
    }
    #[test]
    fn moving_camera_reuses_native_spatial_leaf() {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(4.0, world.height_m(4.0, 4.0) + 1.7, 4.0);
        camera.far = 1.0;
        let mut stream = AdaptiveStream::new(world, StreamSettings::default()).unwrap();
        let key = TileKey {
            min: [0, 0],
            width_cells: 128,
            step_cells: 1,
            edge_steps: [1; 4],
            source_revision: 0,
        };
        let exact = stream.cache.get_or_build(key, || false).unwrap();
        stream.committed = vec![exact];
        stream.committed_pose = Some((camera.clone(), 320, 180));
        camera.position.x += 0.01;
        let moving = stream.plan(&camera, 320, 180);
        assert!(stream.pending.iter().all(|k| k.width_cells < 512));
        assert!(moving.tiles.iter().any(|t| t.exact && t.key == key));
    }
    #[test]
    fn interval_adjacency_finds_split_coarse_edge() {
        let nodes = [
            Node {
                min: [0, 0],
                width: 256,
                step: 32,
            },
            Node {
                min: [256, 0],
                width: 128,
                step: 16,
            },
            Node {
                min: [256, 128],
                width: 128,
                step: 1,
            },
        ];
        let pairs = adjacency(&nodes);
        assert!(pairs.contains(&(0, 2, 1, 0)));
        assert!(pairs.contains(&(0, 2, 2, 0)));
    }
    #[test]
    fn camera_crossing_box_is_not_rejected() {
        let p = Projection {
            vertical_fov_radians: 1.0,
            width: 1920,
            height: 1080,
            near_m: 0.025,
        };
        assert!(!outside_frustum(
            ViewBounds {
                min: [-10., -10., -10.],
                max: [10., 10., 10.]
            },
            p,
            100.
        ));
        assert!(outside_frustum(
            ViewBounds {
                min: [100., 0., 1.],
                max: [101., 1., 2.]
            },
            p,
            100.
        ));
    }
}

/// Latest-state worker: request replaces queued demand, and published results
/// are epoch-tagged. CPU generation never runs on the requesting/render thread.
struct DeliveryPolicy {
    volume_owner: bool,
    pixel_error: f64,
}
impl DeliveryPolicy {
    /// Metadata only: no world/source-column generation on the render thread.
    /// Occlusion is intentionally omitted here; GPU recomputes it for this pose.
    fn revalidate(
        &self,
        frame: &mut SelectionFrame,
        camera: &Camera,
        width: u32,
        height: u32,
    ) -> bool {
        if !complete_coverage_guard(&frame.tiles, camera, width, height, self.volume_owner) {
            return false;
        }
        let Some(p) = camera_projection(camera, width, height) else {
            return false;
        };
        frame.coverage_ready = true;
        frame.visible_unresolved_count = 0;
        frame.visible_uncertified_count = 0;
        frame.global_bound_clusters = 0;
        frame.local_certified_clusters = 0;
        frame.affine_bound_clusters = 0;
        frame.occluded_clusters = 0;
        frame.occluder_boxes = 0;
        let (mut maximum, mut finite) = (0.0_f64, true);
        for tile in &frame.tiles {
            if outside_frustum(
                world_bounds_to_view(tile.conservative_surface_bounds, camera),
                p,
                conservative_clip_depth(camera).unwrap_or(f64::INFINITY),
            ) {
                continue;
            }
            let error_m = tile.best_certified_vertical_error_m;
            let error = error_m
                .map(|e| vertical_pixel_bound(e, tile.bounds, camera, p))
                .unwrap_or(f64::INFINITY);
            if error.is_finite() {
                maximum = maximum.max(error)
            } else {
                finite = false;
            }
            if !error.is_finite() || error > self.pixel_error {
                frame.visible_unresolved_count += 1;
            }
            match proof_kind(tile) {
                1 => frame.local_certified_clusters += 1,
                2 => frame.affine_bound_clusters += 1,
                3 => frame.global_bound_clusters += 1,
                4 => frame.visible_uncertified_count += 1,
                _ => {}
            }
        }
        frame.max_visible_error_px = finite.then_some(maximum);
        frame.quality_ready = frame.visible_unresolved_count == 0;
        frame.refinement_ready = finite && maximum <= self.pixel_error * REFINEMENT_RESERVE;
        true
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct PreparationCycle {
    pub id: u64,
    pub started: std::time::Instant,
}
/// One small coherent cover replacement, independent of whole-view quality.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PublicationTransaction {
    pub id: u64,
    pub started: std::time::Instant,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct PredictionStats {
    pub horizon_ms: Option<f64>,
    pub horizon_from_publication: bool,
    pub publication_samples: usize,
    pub generation_samples: usize,
    pub active: bool,
    pub quality_cycle_samples: usize,
    pub quality_cycle_p95_ms: Option<f64>,
}
pub(crate) struct DiskCacheConfig {
    pub path: std::path::PathBuf,
    pub budget_bytes: u64,
    pub max_entry_bytes: usize,
}
pub struct Worker {
    delivery: DeliveryPolicy,
    state: std::sync::Arc<(std::sync::Mutex<WorkerState>, std::sync::Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}
struct WorkerState {
    request: Option<(u64, Camera, u32, u32)>,
    epoch: u64,
    last_request: Option<(Camera, u32, u32)>,
    result: Option<Result<SelectionFrame, String>>,
    stop: bool,
    motion: MotionPrediction,
    active_preparation: Option<PreparationCycle>,
    next_preparation_id: u64,
    publication_ms: VecDeque<f64>,
    transaction_publication_ms: VecDeque<f64>,
    last_transaction_publication_id: u64,
    generation_ms: VecDeque<f64>,
}
/// Small bounded observation window; no assumed generation speed or fixed
/// meter collar. A teleport resets velocity rather than predicting through it.
#[derive(Default)]
struct MotionPrediction {
    previous: Option<(Camera, std::time::Instant)>,
    predicted: Option<Camera>,
}
fn latency_p95(samples: &VecDeque<f64>) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted: Vec<_> = samples.iter().copied().collect();
    sorted.sort_by(f64::total_cmp);
    Some(sorted[(sorted.len() * 95).div_ceil(100) - 1])
}
impl MotionPrediction {
    fn observe(&mut self, camera: &Camera, now: std::time::Instant, horizon_ms: Option<f64>) {
        self.predicted = None;
        if let (Some((previous, at)), Some(horizon_ms)) = (&self.previous, horizon_ms) {
            let dt = now.duration_since(*at).as_secs_f64();
            let delta = camera.position - previous.position;
            // A jump outside the previous view's far extent is a destination
            // change. Its coherent root demand supersedes speculative motion.
            if dt > 0.0 && dt <= 1.0 && delta.length() <= previous.far as f64 {
                let ratio = horizon_ms / 1000.0 / dt;
                let mut predicted = camera.clone();
                predicted.position += delta * ratio;
                let yaw_delta = (camera.yaw - previous.yaw + std::f32::consts::PI)
                    .rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI;
                predicted.yaw += yaw_delta * ratio as f32;
                predicted.pitch = (camera.pitch + (camera.pitch - previous.pitch) * ratio as f32)
                    .clamp(-1.55, 1.55);
                if predicted.position.is_finite() && predicted.yaw.is_finite() {
                    self.predicted = Some(predicted);
                }
            }
        }
        self.previous = Some((camera.clone(), now));
    }
}
impl Worker {
    pub fn new(world: World, settings: StreamSettings) -> Result<Self, String> {
        Self::new_with_cache(world, settings, None)
    }
    pub(crate) fn new_with_cache(
        world: World,
        settings: StreamSettings,
        disk: Option<DiskCacheConfig>,
    ) -> Result<Self, String> {
        let delivery = DeliveryPolicy {
            volume_owner: world.topology_base_cells().is_some(),
            pixel_error: settings.pixel_error,
        };
        let mut stream = AdaptiveStream::new(world, settings)?;
        if let Some(disk) = disk {
            stream
                .cache
                .enable_disk_cache(disk.path, disk.budget_bytes, disk.max_entry_bytes)
                .map_err(|e| format!("adaptive disk cache: {e}"))?;
        }
        let state = std::sync::Arc::new((
            std::sync::Mutex::new(WorkerState {
                request: None,
                epoch: 0,
                last_request: None,
                result: None,
                stop: false,
                motion: MotionPrediction::default(),
                active_preparation: None,
                next_preparation_id: 0,
                publication_ms: VecDeque::new(),
                transaction_publication_ms: VecDeque::new(),
                last_transaction_publication_id: 0,
                generation_ms: VecDeque::new(),
            }),
            std::sync::Condvar::new(),
        ));
        let shared = state.clone();
        let volume_owner = delivery.volume_owner;
        let thread = std::thread::Builder::new()
            .name("adaptive-terrain".into())
            .spawn(move || loop {
                let (epoch, camera, width, height) = {
                    let (lock, wake) = &*shared;
                    let mut s = lock.lock().unwrap();
                    while s.request.is_none() && !s.stop {
                        s = wake.wait(s).unwrap();
                    }
                    if s.stop {
                        break;
                    }
                    s.request.as_ref().unwrap().clone()
                };
                stream.prediction = shared.0.lock().unwrap().motion.predicted.clone();
                let planning_started = std::time::Instant::now();
                let mut frame = stream.plan(&camera, width, height);
                frame.epoch = epoch;
                let pending = frame.pending;
                let frame_quality = frame.refinement_ready;
                let frame_limited = frame.limit_reached;
                {
                    let mut s = shared.0.lock().unwrap();
                    if s.stop {
                        break;
                    }
                    if !frame.refinement_ready && s.active_preparation.is_none() {
                        s.next_preparation_id = s.next_preparation_id.wrapping_add(1);
                        s.active_preparation = Some(PreparationCycle {
                            id: s.next_preparation_id,
                            started: planning_started,
                        });
                    }
                    frame.preparation = s.active_preparation;
                    if s.epoch == epoch {
                        s.result = Some(Ok(frame));
                    } else if let Some((latest, camera, width, height)) = s.request.clone() {
                        // Validate complete spatial coverage against CURRENT
                        // demand before relabeling a valid canonical partition.
                        if stream.revalidate(&mut frame, &camera, width, height) {
                            frame.epoch = latest;
                            s.result = Some(Ok(frame));
                        }
                    }
                }
                if (pending == 0 && (frame_quality || frame_limited))
                    || (frame_limited && !stream.build_refused.is_empty())
                {
                    let mut s = shared.0.lock().unwrap();
                    if s.epoch == epoch {
                        s.request = None;
                    }
                    continue;
                }
                // Re-read demand before each bounded batch. Camera movement
                // changes priority without discarding reusable content. A new
                // view outside this transaction's complete footprint cancels
                // CPU work at the builder's cancellation checkpoints.
                let keys = stream.transaction.clone().unwrap_or_default();
                let checked_epoch = std::sync::atomic::AtomicU64::new(epoch);
                let obsolete = std::sync::atomic::AtomicBool::new(false);
                let cancel = || {
                    use std::sync::atomic::Ordering;
                    if obsolete.load(Ordering::Relaxed) {
                        return true;
                    }
                    let s = shared.0.lock().unwrap();
                    if s.stop {
                        return true;
                    }
                    if checked_epoch.load(Ordering::Relaxed) != s.epoch {
                        if let Some((_, camera, width, height)) = &s.request {
                            if keys.is_empty()
                                || !complete_key_guard(
                                    keys.iter().copied(),
                                    camera,
                                    *width,
                                    *height,
                                    volume_owner,
                                )
                            {
                                obsolete.store(true, Ordering::Relaxed);
                                return true;
                            }
                        }
                        checked_epoch.store(s.epoch, Ordering::Relaxed);
                    }
                    false
                };
                let generation_started = std::time::Instant::now();
                // An epoch change alone cannot starve canonical work. The
                // callback permits retained transactions only while their full
                // footprint covers CURRENT demand, and preempts uncovered views.
                let mut result = stream.prepare_demand(&cancel);
                if result.is_err() && cancel() {
                    result = Ok(0);
                }
                if matches!(result, Ok(n) if n > 0) {
                    let mut s = shared.0.lock().unwrap();
                    if s.generation_ms.len() == 64 {
                        s.generation_ms.pop_front();
                    }
                    s.generation_ms
                        .push_back(generation_started.elapsed().as_secs_f64() * 1000.0);
                }
                if let Err(e) = result {
                    let mut s = shared.0.lock().unwrap();
                    if s.epoch == epoch && !s.stop {
                        s.result = Some(Err(e));
                        s.request = None;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            delivery,
            state,
            thread: Some(thread),
        })
    }
    pub(crate) fn request(&self, camera: Camera, width: u32, height: u32) -> u64 {
        let mut s = self.state.0.lock().unwrap();
        if let Some((old, w, h)) = &s.last_request {
            if *w == width
                && *h == height
                && old.position == camera.position
                && old.yaw == camera.yaw
                && old.pitch == camera.pitch
                && old.far == camera.far
                && old.fov_y == camera.fov_y
            {
                return s.epoch;
            }
        }
        let now = std::time::Instant::now();
        // Whole-view cold convergence remains separate: it can take seconds
        // and does not estimate one incremental replacement's publication time.
        let horizon =
            latency_p95(&s.transaction_publication_ms).or_else(|| latency_p95(&s.generation_ms));
        s.motion.observe(&camera, now, horizon);
        s.last_request = Some((camera.clone(), width, height));
        s.epoch = s.epoch.wrapping_add(1);
        let epoch = s.epoch;
        s.request = Some((epoch, camera, width, height));
        // A ready partition produced between render frames must survive the
        // next pose update. Relabel only after CURRENT coverage/quality checks.
        if let Some(result) = s.result.take() {
            if let Ok(mut frame) = result {
                if self.delivery.revalidate(&mut frame, &camera, width, height) {
                    frame.epoch = epoch;
                    s.result = Some(Ok(frame));
                }
            }
        }
        self.state.1.notify_one();
        epoch
    }
    pub(crate) fn prediction_stats(&self) -> PredictionStats {
        let s = self.state.0.lock().unwrap();
        let publication = latency_p95(&s.transaction_publication_ms);
        PredictionStats {
            horizon_ms: publication.or_else(|| latency_p95(&s.generation_ms)),
            horizon_from_publication: publication.is_some(),
            publication_samples: s.transaction_publication_ms.len(),
            generation_samples: s.generation_ms.len(),
            active: s.motion.predicted.is_some(),
            quality_cycle_samples: s.publication_ms.len(),
            quality_cycle_p95_ms: latency_p95(&s.publication_ms),
        }
    }
    /// The render side acknowledges only the exact cover that became current.
    /// Acknowledgement does not require global requested/reserve quality, and
    /// camera relabeling never resets the transaction's original timestamp.
    pub(crate) fn record_transaction_publication(&self, transaction: PublicationTransaction) {
        let now = std::time::Instant::now();
        let Some(elapsed) = now.checked_duration_since(transaction.started) else {
            return;
        };
        let mut s = self.state.0.lock().unwrap();
        if transaction.id <= s.last_transaction_publication_id {
            return;
        }
        s.last_transaction_publication_id = transaction.id;
        if s.transaction_publication_ms.len() == 64 {
            s.transaction_publication_ms.pop_front();
        }
        s.transaction_publication_ms
            .push_back(elapsed.as_secs_f64() * 1000.0);
    }
    /// Acknowledge a complete preparation cycle only after CURRENT GPU error
    /// assessment passes the reserve. Camera relabeling never changes its start.
    /// Old or repeated acknowledgements cannot add samples or close a new cycle.
    pub(crate) fn record_preparation(&self, cycle: PreparationCycle) {
        let mut s = self.state.0.lock().unwrap();
        if s.active_preparation
            .is_some_and(|active| active.id == cycle.id)
        {
            let elapsed = s
                .active_preparation
                .take()
                .unwrap()
                .started
                .elapsed()
                .as_secs_f64()
                * 1000.0;
            if s.publication_ms.len() == 64 {
                s.publication_ms.pop_front();
            }
            s.publication_ms.push_back(elapsed);
        }
    }
    pub fn poll(&self) -> Option<Result<SelectionFrame, String>> {
        self.state.0.lock().unwrap().result.take()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        {
            let mut s = self.state.0.lock().unwrap();
            s.stop = true;
            self.state.1.notify_one();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod worker_tests {
    use super::*;
    #[test]
    fn retained_drain_skips_cover_scans_and_publishes_the_same_coherent_partition() {
        let world = World::fbm(
            7,
            frontier_world::FbmParams {
                amplitude: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(0.0, 1.7, 0.0);
        camera.far = 10.0;
        let settings = StreamSettings {
            jobs_per_update: 1,
            worker_count: 2,
            ..Default::default()
        };
        let mut drained = AdaptiveStream::new(world.clone(), settings).unwrap();
        let cold = drained.plan(&camera, 320, 180);
        assert!(cold.pending > 1);
        let token = drained.pending_publication.unwrap();
        assert_eq!(drained.prepare_demand(|| false).unwrap(), cold.pending);
        assert!(
            drained.committed.is_empty(),
            "CPU preparation cannot publish ownership"
        );
        assert_eq!(drained.planning.calls, 1);
        assert!(drained.planning.prepared_batches_without_plan > 0);
        let completed = drained.plan(&camera, 320, 180);
        assert!(completed.coverage_ready);
        assert_eq!(completed.publication_transaction.unwrap().id, token.id);
        assert_eq!(completed.planning.calls, 2);
        assert_eq!(completed.planning.full_calls, 1);
        assert_eq!(completed.planning.retained_calls, 1);
        let bytes: usize = completed.tiles.iter().map(|tile| tile.bytes).sum();
        assert_eq!(drained.transaction_known_bytes, bytes);
        let mut batched = AdaptiveStream::new(world, settings).unwrap();
        let mut frame = batched.plan(&camera, 320, 180);
        while !frame.coverage_ready {
            batched.prepare(|| false).unwrap();
            frame = batched.plan(&camera, 320, 180);
        }
        assert!(frame.planning.calls > completed.planning.calls);
        let keys =
            |frame: &SelectionFrame| frame.tiles.iter().map(|t| t.key).collect::<HashSet<_>>();
        assert_eq!(keys(&frame), keys(&completed));
        assert_eq!(frame.max_visible_error_px, completed.max_visible_error_px);
        let repeated = drained.plan(&camera, 320, 180);
        assert_eq!(keys(&repeated), keys(&completed));
        let repeated_token = repeated.publication_transaction.unwrap();
        assert_eq!(repeated_token.id, token.id);
        assert_eq!(repeated_token.started, token.started);
    }
    #[test]
    fn prepared_size_updates_are_idempotent_inside_a_retained_transaction() {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let mut stream = AdaptiveStream::new(world.clone(), StreamSettings::default()).unwrap();
        let key = TileKey {
            min: [0, 0],
            width_cells: 16,
            step_cells: 4,
            edge_steps: [4; 4],
            source_revision: 0,
        };
        let tile = crate::adaptive_geometry::build(&world, key, || false).unwrap();
        stream.transaction = Some(vec![key]);
        stream.record_prepared_tile(&tile);
        stream.record_prepared_tile(&tile);
        assert_eq!(stream.transaction_known_bytes, tile.bytes);
        assert_eq!(stream.variants[&(key.min, key.width_cells)].len(), 1);
    }
    #[test]
    fn prediction_gets_one_deadline_slot_ahead_of_reserve_but_never_requested_work() {
        let key = |x| TileKey {
            min: [x, 0],
            width_cells: 16,
            step_cells: 4,
            edge_steps: [4; 4],
            source_revision: 0,
        };
        let current: Vec<_> = (0..4)
            .map(|i| RefinementCandidate {
                key: key(i * 16),
                distance_squared: i as f64,
                error_px: 4.5,
            })
            .collect();
        let predicted = vec![
            PredictionCandidate {
                key: key(128),
                need_fraction: 0.75,
                error_px: 500.0,
            },
            PredictionCandidate {
                key: key(144),
                need_fraction: 0.25,
                error_px: 6.0,
            },
        ];
        let (selected, footprint) =
            select_with_prediction(current.clone(), predicted.clone(), 5.0, 4);
        assert_eq!(selected.len(), 4);
        assert_eq!(footprint, Some(key(144)));
        assert!(
            selected.contains(&key(0))
                && selected.contains(&key(16))
                && selected.contains(&key(32))
        );
        assert!(!selected.contains(&key(48)));
        let mut failing = current;
        failing[0].error_px = f64::INFINITY;
        let (selected, footprint) = select_with_prediction(failing, predicted, 5.0, 4);
        assert_eq!(selected.len(), 4);
        assert_eq!(footprint, None);
        assert!(!selected.contains(&key(144)));
    }
    #[test]
    fn prediction_protection_is_limited_to_the_selected_spatial_footprint() {
        let tile = TileKey {
            min: [-128, -128],
            width_cells: 128,
            step_cells: 16,
            edge_steps: [16; 4],
            source_revision: 0,
        };
        assert!(footprints_overlap(
            tile,
            TileKey {
                width_cells: 64,
                ..tile
            }
        ));
        assert!(footprints_overlap(
            tile,
            TileKey {
                min: [-256, -256],
                width_cells: 256,
                ..tile
            }
        ));
        assert!(!footprints_overlap(
            tile,
            TileKey {
                min: [0, -128],
                ..tile
            }
        ));
        assert!(!footprints_overlap(
            tile,
            TileKey {
                min: [128, 128],
                ..tile
            }
        ));
    }
    #[test]
    fn measured_horizon_uses_exact_nearest_rank_percentile() {
        assert_eq!(latency_p95(&VecDeque::new()), None);
        let samples = (1..=20).rev().map(|v| v as f64).collect();
        assert_eq!(latency_p95(&samples), Some(19.0));
        assert_eq!(latency_p95(&VecDeque::from([12.0])), Some(12.0));
    }
    #[test]
    fn prediction_uses_measured_velocity_turn_and_latency_then_resets_on_jump() {
        let now = std::time::Instant::now();
        let mut motion = MotionPrediction::default();
        let mut camera = Camera::fixture();
        camera.far = 300.0;
        motion.observe(&camera, now, Some(200.0));
        assert!(motion.predicted.is_none());
        camera.position.x += 1.5;
        camera.position.y += 0.5;
        camera.yaw += 0.1;
        motion.observe(
            &camera,
            now + std::time::Duration::from_millis(100),
            Some(200.0),
        );
        let prediction = motion.predicted.as_ref().unwrap();
        assert!((prediction.position.x - camera.position.x - 3.0).abs() < 1e-9);
        assert!((prediction.position.y - camera.position.y - 1.0).abs() < 1e-9);
        assert!((prediction.yaw - camera.yaw - 0.2).abs() < 1e-5);
        camera.position.x += 1000.0;
        motion.observe(
            &camera,
            now + std::time::Duration::from_millis(200),
            Some(200.0),
        );
        assert!(motion.predicted.is_none());
        motion.observe(&camera, now + std::time::Duration::from_millis(300), None);
        assert!(motion.predicted.is_none());
    }
    #[test]
    fn preparation_latency_survives_camera_epochs_and_is_sampled_once() {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let worker = Worker::new(world, StreamSettings::default()).unwrap();
        let cycle = PreparationCycle {
            id: 17,
            started: std::time::Instant::now() - std::time::Duration::from_millis(100),
        };
        {
            let mut state = worker.state.0.lock().unwrap();
            state.active_preparation = Some(cycle);
            state.next_preparation_id = cycle.id;
            state.epoch = 99;
        }
        worker.record_preparation(cycle);
        worker.record_preparation(cycle);
        let stats = worker.prediction_stats();
        assert_eq!(stats.quality_cycle_samples, 1);
        assert!(stats.quality_cycle_p95_ms.unwrap() >= 100.0);
        assert_eq!(stats.publication_samples, 0);
        assert!(!stats.horizon_from_publication);
        assert!(stats.horizon_ms.is_none());
        let next = PreparationCycle {
            id: 18,
            started: std::time::Instant::now(),
        };
        worker.state.0.lock().unwrap().active_preparation = Some(next);
        worker.record_preparation(cycle);
        assert_eq!(
            worker
                .state
                .0
                .lock()
                .unwrap()
                .active_preparation
                .unwrap()
                .id,
            next.id
        );
        assert_eq!(worker.prediction_stats().quality_cycle_samples, 1);
    }
    #[test]
    fn transaction_publication_is_sampled_once_independent_of_quality_and_epochs() {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let worker = Worker::new(world, StreamSettings::default()).unwrap();
        let transaction = PublicationTransaction {
            id: 17,
            started: std::time::Instant::now() - std::time::Duration::from_millis(100),
        };
        {
            let mut state = worker.state.0.lock().unwrap();
            state.epoch = 99;
            state.generation_ms.push_back(5.0);
        }
        assert_eq!(worker.prediction_stats().horizon_ms, Some(5.0));
        worker.record_transaction_publication(transaction);
        worker.record_transaction_publication(transaction);
        worker.record_transaction_publication(PublicationTransaction {
            id: 16,
            ..transaction
        });
        let stats = worker.prediction_stats();
        assert_eq!(stats.publication_samples, 1);
        assert!(stats.horizon_from_publication);
        assert!(stats.horizon_ms.unwrap() >= 100.0);
        assert_eq!(stats.quality_cycle_samples, 0);
        worker.record_transaction_publication(PublicationTransaction {
            id: 18,
            ..transaction
        });
        assert_eq!(worker.prediction_stats().publication_samples, 2);
    }
    #[test]
    fn single_job_memory_refusal_keeps_coarse_state_and_reports_limit() {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let mut camera = Camera::fixture();
        camera.position.y = 100.0;
        camera.far = 10.0;
        let mut stream = AdaptiveStream::new(
            world,
            StreamSettings {
                worker_count: 4,
                build_budget_bytes: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let initial = stream.plan(&camera, 320, 180);
        assert!(initial.pending > 0);
        assert_eq!(stream.prepare(|| false).unwrap(), 0);
        let refused = stream.plan(&camera, 320, 180);
        assert!(refused.limit_reached);
        assert!(!refused.quality_ready);
        assert_eq!(refused.build_memory.refused_jobs, 1);
        assert_eq!(refused.cache.builds, 0);
    }
    #[test]
    fn cancelled_preparation_preserves_pending_canonical_keys() {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let mut stream = AdaptiveStream::new(
            world,
            StreamSettings {
                worker_count: 4,
                ..Default::default()
            },
        )
        .unwrap();
        let key = TileKey {
            min: [0, 0],
            width_cells: 16,
            step_cells: 2,
            edge_steps: [2; 4],
            source_revision: 0,
        };
        stream.pending.push_back(key);
        assert_eq!(stream.prepare(|| true).unwrap(), 0);
        assert_eq!(stream.pending, VecDeque::from([key]));
        assert_eq!(stream.cache_stats().builds, 0);
    }
    #[test]
    fn continuous_sixty_hz_motion_publishes_current_geometry() {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(0.0, world.height_m(0.0, 0.0) + 1.7, 0.0);
        camera.far = 10.0;
        let worker = Worker::new(
            world,
            StreamSettings {
                pixel_error: 0.5,
                ..Default::default()
            },
        )
        .unwrap();
        worker.request(camera.clone(), 320, 180);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(frame) = worker.poll() {
                if frame.unwrap().coverage_ready {
                    break;
                }
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let mut published = 0;
        let mut epochs = HashSet::new();
        let mut geometry = HashSet::new();
        // Keep demand moving until structural publication progress is seen.
        // CPU load may delay bounded tile jobs; fixed wall-time publication
        // counts would test the host scheduler rather than epoch correctness.
        let motion_deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        let mut requests = 0;
        while (requests < 70 || published < 3 || geometry.len() < 2)
            && std::time::Instant::now() < motion_deadline
        {
            requests += 1;
            camera.position.x += 0.001;
            camera.yaw += 0.0001;
            let epoch = worker.request(camera.clone(), 320, 180);
            if let Some(result) = worker.poll() {
                let frame = result.unwrap();
                assert_eq!(frame.epoch, epoch);
                if frame.coverage_ready {
                    published += 1;
                    epochs.insert(frame.epoch);
                    let mut keys = frame
                        .tiles
                        .iter()
                        .map(|t| (t.key.min, t.key.width_cells, t.key.step_cells))
                        .collect::<Vec<_>>();
                    keys.sort_unstable();
                    geometry.insert(keys);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        assert!(requests >= 70, "motion window ended early");
        assert!(published >= 3, "published={published}");
        assert!(epochs.len() >= 3);
        assert!(geometry.len() > 1, "geometry did not evolve during motion");
    }
    #[test]
    fn moving_demand_keeps_canonical_work_and_return_cache() {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(0.0, world.height_m(0.0, 0.0) + 1.7, 0.0);
        camera.far = 10.0;
        let worker = Worker::new(world, StreamSettings::default()).unwrap();
        let mut epoch = 0;
        // Continuous turning supersedes demand while bounded source jobs finish.
        for _ in 0..12 {
            camera.yaw += 0.01;
            epoch = worker.request(camera.clone(), 320, 180);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let frame = loop {
            if let Some(result) = worker.poll() {
                let f = result.unwrap();
                assert_eq!(f.epoch, epoch);
                if f.coverage_ready && f.cache.builds > 0 {
                    break f;
                }
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        assert!(frame.cache.hits > 0);
        // Repeated unchanged camera demand preserves its epoch and valid cache.
        assert_eq!(worker.request(camera, 320, 180), epoch);
    }
    #[test]
    fn latest_state_worker_rejects_superseded_publication_and_deduplicates() {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let worker = Worker::new(world, StreamSettings::default()).unwrap();
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(0.0, 10000.0, 0.0);
        camera.far = 1.0;
        worker.request(camera.clone(), 1920, 1080);
        camera.yaw += 0.1;
        worker.request(camera.clone(), 1920, 1080);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let frame = loop {
            if let Some(result) = worker.poll() {
                let frame = result.unwrap();
                if frame.coverage_ready {
                    break frame;
                }
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        assert_eq!(frame.epoch, 2);
        assert!(frame.coverage_ready);
        assert_eq!(frame.pending, 0);
        worker.request(camera, 1920, 1080);
        assert_eq!(worker.state.0.lock().unwrap().epoch, 2);
    }
}
