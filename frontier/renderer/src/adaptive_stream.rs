//! Bounded experimental selection over immutable canonical terrain tiles.
//! Missing coverage and unmet quality remain explicit; no fog or crop hides it.
use crate::{
    adaptive_geometry::{GeometryCache, GeometryTile, TileKey},
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
}
impl Default for StreamSettings {
    fn default() -> Self {
        Self {
            pixel_error: 2.0,
            cpu_cache_bytes: 512 * 1024 * 1024,
            max_nodes: 100_000,
            jobs_per_update: 4,
        }
    }
}
pub struct SelectionFrame {
    pub epoch: u64,
    pub tiles: Vec<Arc<GeometryTile>>,
    pub coverage_ready: bool,
    pub quality_ready: bool,
    /// Current partition also satisfies the stricter internal motion reserve.
    pub refinement_ready: bool,
    pub pending: usize,
    pub visited: usize,
    pub limit_reached: bool,
    pub cache: crate::adaptive_geometry::CacheStats,
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
        {
            return Err("invalid adaptive stream settings".into());
        }
        Ok(Self {
            cache: GeometryCache::new(world.clone(), settings.cpu_cache_bytes),
            world,
            settings,
            pending: VecDeque::new(),
            transaction: None,
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
        self.epoch = self.epoch.wrapping_add(1);
        self.coarsen_probes.clear();
        self.coarsen_requested = false;
        self.coarsen_targets.clear();
        let mut frame = SelectionFrame {
            epoch: self.epoch,
            tiles: vec![],
            coverage_ready: false,
            quality_ready: false,
            refinement_ready: false,
            pending: 0,
            visited: 0,
            limit_reached: false,
            cache: self.cache.stats(),
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
            return frame;
        }
        let Some(p) = camera_projection(camera, width, height) else {
            frame.limit_reached = true;
            return frame;
        };
        self.refresh_occlusion(camera);
        // Content is camera-independent. Finish the small replacement already
        // in flight while its complete footprint still covers the current view.
        // Replanning every moving epoch discarded nearly finished transactions.
        if let Some(keys) = self.transaction.take() {
            let bytes = keys
                .iter()
                .filter_map(|k| self.known_bytes.get(k))
                .copied()
                .fold(0usize, usize::saturating_add);
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
                        self.committed = next;
                        self.committed_pose = Some((*camera, width, height));
                        self.pending.clear();
                        self.refresh_occlusion(camera);
                        frame.tiles = self.committed.clone();
                        frame.coverage_ready = true;
                        self.assess_quality(&mut frame, camera, p);
                        frame.cache = self.cache.stats();
                        return frame;
                    }
                } else {
                    self.pending = missing;
                    self.transaction = Some(keys);
                    frame.tiles = self.committed.clone();
                    self.revalidate(&mut frame, camera, width, height);
                    frame.pending = self.pending.len();
                    frame.cache = self.cache.stats();
                    return frame;
                }
            }
            // A teleport, uncovered view or inadmissible coarsening invalidates
            // demand, never the immutable completed geometry in the cache.
            self.pending.clear();
        }
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
        let refine = select_refinements(
            urgent,
            self.settings.pixel_error,
            self.settings.jobs_per_update,
        );
        let mut nodes = vec![];
        if same {
            for tile in self.committed.clone() {
                if !self.tile_visible(&tile, camera, p) {
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
                        step: (k.step_cells / 2).max(1),
                    });
                }
            }
        } else {
            let Some(extents) = all_heading_horizontal_extents(camera, width, height) else {
                frame.limit_reached = true;
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
                    if !self.tile_visible(t, camera, p) {
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
                            step: (old.step / 2).max(1),
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
            return frame;
        }
        if !complete {
            self.transaction = Some(target_keys);
        }
        if complete {
            self.committed = next;
            self.committed_pose = Some((camera.clone(), width, height));
            self.refresh_occlusion(camera);
            frame.coverage_ready = true;
        } else {
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
        frame
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
                if !self.tile_visible(&tile, camera, p)
                    || self.projected_error(&tile, camera, p) <= self.settings.pixel_error * 0.65
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
    }
    fn projected_error(&self, tile: &GeometryTile, camera: &Camera, p: Projection) -> f64 {
        let e = tile.best_certified_vertical_error_m;
        e.map(|e| vertical_pixel_bound(e, tile.bounds, camera, p))
            .unwrap_or(f64::INFINITY)
    }
    pub fn prepare(&mut self, cancelled: impl FnMut() -> bool) -> Result<usize, String> {
        self.prepare_jobs(self.settings.jobs_per_update, cancelled)
    }
    fn prepare_one(&mut self, cancelled: impl FnMut() -> bool) -> Result<usize, String> {
        self.prepare_jobs(1, cancelled)
    }
    fn prepare_jobs(
        &mut self,
        limit: usize,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<usize, String> {
        let mut completed = 0;
        for _ in 0..limit {
            if cancelled() {
                break;
            }
            let Some(key) = self.pending.pop_front() else {
                break;
            };
            let tile = self.cache.get_or_build(key, &mut cancelled)?;
            self.known_bytes.insert(key, tile.bytes);
            let variants = self.variants.entry((key.min, key.width_cells)).or_default();
            if !variants.contains(&key) {
                variants.push(key);
            }
            completed += 1;
        }
        Ok(completed)
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
}
impl Worker {
    pub fn new(world: World, settings: StreamSettings) -> Result<Self, String> {
        let delivery = DeliveryPolicy {
            volume_owner: world.topology_base_cells().is_some(),
            pixel_error: settings.pixel_error,
        };
        let mut stream = AdaptiveStream::new(world, settings)?;
        let state = std::sync::Arc::new((
            std::sync::Mutex::new(WorkerState {
                request: None,
                epoch: 0,
                last_request: None,
                result: None,
                stop: false,
            }),
            std::sync::Condvar::new(),
        ));
        let shared = state.clone();
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
                if pending == 0 && (frame_quality || frame_limited) {
                    let mut s = shared.0.lock().unwrap();
                    if s.epoch == epoch {
                        s.request = None;
                    }
                    continue;
                }
                // Re-read demand between EVERY canonical job. Amortize the
                // full partition plan across a small batch only while the pose
                // epoch is unchanged; a turn never starts three obsolete jobs.
                let mut result = Ok(0usize);
                for _ in 0..settings.jobs_per_update {
                    {
                        let s = shared.0.lock().unwrap();
                        if s.stop || s.epoch != epoch {
                            break;
                        }
                    }
                    match stream.prepare_one(|| shared.0.lock().unwrap().stop) {
                        Ok(0) => break,
                        Ok(count) => {
                            if let Ok(total) = &mut result {
                                *total += count;
                            }
                        }
                        Err(error) => {
                            result = Err(error);
                            break;
                        }
                    }
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
