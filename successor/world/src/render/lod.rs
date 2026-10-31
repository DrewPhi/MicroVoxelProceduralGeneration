//! A complete, non-overlapping quadtree cover. Adjacent leaves differ by at most
//! one level; local patches include all changed neighbor dependencies.
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
pub const SIDE: i64 = 32;
pub const ROOT: i64 = 262_144;
pub const FINEST: u8 = 13;
pub const EXTENT: i64 = 160_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    pub spec: crate::WorldSpec,
    pub finest: u8,
    pub root: i64,
    pub extent: i64,
}
impl Default for Grid {
    fn default() -> Self {
        Self::new(crate::WorldSpec::default())
    }
}
impl Grid {
    pub fn new(spec: crate::WorldSpec) -> Self {
        let extent = spec.extent_cells();
        let root = (extent.max(SIDE) as u64).next_power_of_two() as i64;
        Self {
            spec,
            finest: (root.trailing_zeros() - 5) as u8,
            root,
            extent,
        }
    }
    pub fn cell_m(self) -> f64 {
        self.spec.grid.meters()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TileKey {
    pub level: u8,
    pub x: u32,
    pub z: u32,
}
impl TileKey {
    pub fn step_in(self, grid: Grid) -> i64 {
        1i64 << grid.finest.saturating_sub(self.level)
    }
    pub fn edge_in(self, grid: Grid) -> i64 {
        SIDE * self.step_in(grid)
    }
    pub fn origin_in(self, grid: Grid) -> [i64; 2] {
        let e = self.edge_in(grid);
        [self.x as i64 * e, self.z as i64 * e]
    }
    pub fn step(self) -> i64 {
        1 << (FINEST - self.level)
    }
    pub fn edge(self) -> i64 {
        SIDE * self.step()
    }
    pub fn origin(self) -> [i64; 2] {
        [self.x as i64 * self.edge(), self.z as i64 * self.edge()]
    }
    pub fn children(self) -> impl Iterator<Item = Self> {
        (0..4).map(move |i| Self {
            level: self.level + 1,
            x: self.x * 2 + i % 2,
            z: self.z * 2 + i / 2,
        })
    }
    pub fn parent(self) -> Option<Self> {
        (self.level > 0).then(|| Self {
            level: self.level - 1,
            x: self.x / 2,
            z: self.z / 2,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub grid: Grid,
    pub tiles: BTreeSet<TileKey>,
}
impl Layout {
    pub fn root() -> Self {
        Self::root_in(Grid::default())
    }
    pub fn root_in(grid: Grid) -> Self {
        Self {
            grid,
            tiles: [TileKey {
                level: 0,
                x: 0,
                z: 0,
            }]
            .into(),
        }
    }
    pub fn at(&self, x: i64, z: i64) -> Option<TileKey> {
        if !(0..self.grid.extent).contains(&x) || !(0..self.grid.extent).contains(&z) {
            return None;
        }
        for level in (0..=self.grid.finest).rev() {
            let edge = self.grid.root >> level;
            let key = TileKey {
                level,
                x: (x / edge) as u32,
                z: (z / edge) as u32,
            };
            if self.tiles.contains(&key) {
                return Some(key);
            }
        }
        None
    }
    fn side_midpoints(&self, key: TileKey) -> [[i64; 2]; 4] {
        let [x, z] = key.origin_in(self.grid);
        let e = key.edge_in(self.grid);
        // A finite world's last tile may contain only a narrow corner. Its
        // nominal midpoint can be outside the world even on an interior side.
        let mx = x + e.min(self.grid.extent - x) / 2;
        let mz = z + e.min(self.grid.extent - z) / 2;
        [[x - 1, mz], [x + e, mz], [mx, z - 1], [mx, z + e]]
    }
    /// Side order: -X,+X,-Z,+Z. Sample the valid portion of each side;
    /// balance ensures its touching neighbors are at the same level.
    pub fn neighbors(&self, key: TileKey) -> [u8; 4] {
        self.side_midpoints(key)
            .map(|p| self.at(p[0], p[1]).map_or(key.level, |n| n.level))
    }
    /// Walk every leaf touching the clipped boundary, jumping across its span.
    /// Balanced input has at most four neighbors per parent side before a merge.
    /// Fixed midpoint/quarter probes can miss a fine leaf at a clipped corner.
    fn has_finer_neighbor(&self, parent: TileKey) -> bool {
        let [x, z] = parent.origin_in(self.grid);
        let e = parent.edge_in(self.grid);
        for (mut point, axis) in [
            ([x - 1, z], 1),
            ([x + e, z], 1),
            ([x, z - 1], 0),
            ([x, z + e], 0),
        ] {
            let end = (point[axis] + e).min(self.grid.extent);
            while point[axis] < end {
                let Some(neighbor) = self.at(point[0], point[1]) else {
                    break;
                };
                if neighbor.level > parent.level + 1 {
                    return true;
                }
                let next = neighbor.origin_in(self.grid)[axis] + neighbor.edge_in(self.grid);
                debug_assert!(next > point[axis]);
                point[axis] = next;
            }
        }
        self.diagonal_points(parent)
            .into_iter()
            .any(|[x, z]| self.at(x, z).is_some_and(|n| n.level > parent.level + 1))
    }
    fn diagonal_points(&self, key: TileKey) -> [[i64; 2]; 4] {
        let [x, z] = key.origin_in(self.grid);
        let edge = key.edge_in(self.grid);
        [
            [x - 1, z - 1],
            [x - 1, z + edge],
            [x + edge, z + edge],
            [x + edge, z - 1],
        ]
    }
    fn corners(&self, key: TileKey) -> [[i64; 2]; 4] {
        let [x, z] = key.origin_in(self.grid);
        let e = key.edge_in(self.grid);
        [[x, z], [x, z + e], [x + e, z + e], [x + e, z]]
    }
    fn corner_level_at(&self, [cx, cz]: [i64; 2], fallback: u8) -> u8 {
        let (cx, cz) = (cx.min(self.grid.extent), cz.min(self.grid.extent));
        [[cx - 1, cz - 1], [cx - 1, cz], [cx, cz - 1], [cx, cz]]
            .into_iter()
            .filter_map(|[x, z]| self.at(x, z))
            .map(|k| k.level)
            .min()
            .unwrap_or(fallback)
    }
    pub fn corner_levels(&self, key: TileKey) -> [u8; 4] {
        self.corners(key)
            .map(|p| self.corner_level_at(p, key.level))
    }
    pub fn split(&mut self, key: TileKey) {
        self.tiles.remove(&key);
        for child in key.children() {
            let [x, z] = child.origin_in(self.grid);
            if x < self.grid.extent && z < self.grid.extent {
                self.tiles.insert(child);
            }
        }
    }
    pub fn balance(&mut self) {
        loop {
            let mut split = BTreeSet::new();
            for &key in &self.tiles {
                for p in self
                    .side_midpoints(key)
                    .into_iter()
                    .chain(self.diagonal_points(key))
                {
                    if let Some(n) = self.at(p[0], p[1]) {
                        if n.level + 1 < key.level {
                            split.insert(n);
                        }
                    }
                }
            }
            if split.is_empty() {
                break;
            }
            for key in split {
                self.split(key);
            }
        }
    }
}
/// Display quality is independent of canonical voxel resolution. The error is
/// projected display-cell size, not a proven geometric terrain-error bound.
#[derive(Clone, Copy, Debug)]
pub struct Quality {
    pub pixels: f64,
    pub native_radius: f64,
    pub direction: [f64; 3],
    pub visibility: f64,
}
impl Default for Quality {
    fn default() -> Self {
        Self {
            pixels: 2.0,
            native_radius: 6.0,
            direction: [0., 0., -1.],
            visibility: 24000.,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Selection {
    pub native_policy_radius: f64,
    pub layout: Layout,
    pub requested_pixels: f64,
    pub achieved_pixels: f64,
    pub budget_limited: bool,
}
/// Backwards-compatible deterministic selection for tools.
pub fn select(eye: [f64; 3], height: u32, fov: f64, previous: &Layout, budget: usize) -> Layout {
    select_quality(
        eye,
        height,
        fov,
        previous,
        budget,
        Quality {
            pixels: 4.,
            native_radius: 4.,
            ..Quality::default()
        },
    )
    .layout
}
pub fn select_quality(
    eye: [f64; 3],
    height: u32,
    fov: f64,
    previous: &Layout,
    budget: usize,
    quality: Quality,
) -> Selection {
    select_adaptive(eye, height, fov, previous, budget, quality, |_| None)
}
/// Approximate terrain bounds in meters, and sampled height interpolation error.
/// Selection remains conservative in footprint even when terrain data is absent.
#[derive(Clone, Copy, Debug)]
pub struct TerrainError {
    pub low: f64,
    pub high: f64,
    pub deviation: f64,
    pub feature_scale: f64,
}
#[derive(Clone, Copy, PartialEq)]
struct Candidate {
    score: f64,
    key: TileKey,
}
impl Eq for Candidate {}
impl Ord for Candidate {
    fn cmp(&self, b: &Self) -> std::cmp::Ordering {
        self.score.total_cmp(&b.score).then(self.key.cmp(&b.key))
    }
}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, b: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(b))
    }
}
/// Best-first refinement. Admit the complete local 2:1 closure of a split;
/// expensive terrain in one direction no longer relaxes quality everywhere.
pub fn select_adaptive(
    eye: [f64; 3],
    height: u32,
    fov: f64,
    previous: &Layout,
    budget: usize,
    quality: Quality,
    mut terrain: impl FnMut(TileKey) -> Option<TerrainError>,
) -> Selection {
    let grid = previous.grid;
    let budget = budget.max(1);
    let focal = height as f64 / (2. * (fov * 0.5).tan());
    let requested = if quality.pixels.is_finite() {
        quality.pixels.clamp(1., 16.)
    } else {
        2.
    };
    let radius = if quality.native_radius.is_finite() {
        quality.native_radius.clamp(2., 32.)
    } else {
        6.
    };
    let radius = radius
        .min(
            SIDE as f64
                * grid.cell_m()
                * (budget.max(64) as f64 / 4. / std::f64::consts::PI).sqrt(),
        )
        .min(grid.cell_m() * focal);
    let mut prior = BTreeSet::new();
    for &key in &previous.tiles {
        let mut k = key;
        while let Some(parent) = k.parent() {
            prior.insert(parent);
            k = parent;
        }
    }
    let mut cache = BTreeMap::new();
    let mut evaluate = |key: TileKey| -> (f64, f64, bool) {
        if let Some(&value) = cache.get(&key) {
            return value;
        }
        let [x, z] = key.origin_in(grid);
        let edge = key.edge_in(grid) as f64 * grid.cell_m();
        let low = [x as f64 * grid.cell_m(), z as f64 * grid.cell_m()];
        let dx = (low[0] - eye[0]).max(0.).max(eye[0] - (low[0] + edge));
        let dz = (low[1] - eye[2]).max(0.).max(eye[2] - (low[1] + edge));
        let bounds = terrain(key);
        let vertical = bounds.map_or(0., |b| (b.low - eye[1]).max(0.).max(eye[1] - b.high));
        let distance = dx.hypot(dz).hypot(vertical).max(0.1);
        let native = distance < radius;
        let center = [
            low[0] + edge * 0.5 - eye[0],
            bounds.map_or(eye[1], |b| (b.low + b.high) * 0.5) - eye[1],
            low[1] + edge * 0.5 - eye[2],
        ];
        let length = (center[0] * center[0] + center[1] * center[1] + center[2] * center[2])
            .sqrt()
            .max(0.1);
        let facing = (center[0] * quality.direction[0]
            + center[1] * quality.direction[1]
            + center[2] * quality.direction[2])
            / length;
        // Soft priorities only: sampled terrain ranges never certify invisibility.
        let visible = facing + edge * 0.9 / length > 0.5;
        let importance = if visible || distance < radius * 2. {
            1.
        } else {
            0.16
        };
        let cell = key.step_in(grid) as f64 * grid.cell_m();
        let px = cell * focal / distance;
        let geometric = bounds.map_or(0., |b| b.deviation.min(edge) * 0.2 * focal / distance);
        let water = bounds.map_or(0., |b| {
            if b.feature_scale.is_finite()
                && b.feature_scale * focal / distance >= 1.5
                && cell > b.feature_scale * 0.5
            {
                px * 2.
            } else {
                0.
            }
        });
        let hysteresis = if prior.contains(&key) { 0.9 } else { 1.1 };
        let score = if native {
            1e9 + 1. / distance
        } else if dx.hypot(dz) > quality.visibility {
            0.
        } else {
            px.max(geometric).max(water) * importance / (requested * hysteresis)
        };
        let value = (score, px, visible);
        cache.insert(key, value);
        value
    };
    let mut layout = Layout::root_in(grid);
    let root = *layout.tiles.first().unwrap();
    let mut queue = BinaryHeap::from([Candidate {
        score: evaluate(root).0,
        key: root,
    }]);
    let mut limited = false;
    while let Some(candidate) = queue.pop() {
        let key = candidate.key;
        if candidate.score <= 1. {
            break;
        }
        if key.level >= grid.finest || !layout.tiles.contains(&key) {
            continue;
        }
        let mut closure = BTreeSet::new();
        let mut pending = vec![key];
        while let Some(k) = pending.pop() {
            if !closure.insert(k) {
                continue;
            }
            let [x, z] = k.origin_in(grid);
            let edge = k.edge_in(grid);
            for offset in [0, edge / 2, edge - 1] {
                for [nx, nz] in [
                    [x - 1, z + offset],
                    [x + edge, z + offset],
                    [x + offset, z - 1],
                    [x + offset, z + edge],
                ] {
                    if let Some(neighbor) = layout.at(nx, nz) {
                        if neighbor.level < k.level && !closure.contains(&neighbor) {
                            pending.push(neighbor);
                        }
                    }
                }
            }
            for [nx, nz] in layout.diagonal_points(k) {
                if let Some(neighbor) = layout.at(nx, nz) {
                    if neighbor.level < k.level && !closure.contains(&neighbor) {
                        pending.push(neighbor);
                    }
                }
            }
        }
        let cost: usize = closure
            .iter()
            .map(|k| {
                k.children()
                    .filter(|c| {
                        let [x, z] = c.origin_in(grid);
                        x < grid.extent && z < grid.extent
                    })
                    .count()
                    - 1
            })
            .sum();
        if layout.tiles.len() + cost > budget {
            limited = true;
            continue;
        }
        for &k in &closure {
            layout.split(k);
        }
        for k in closure {
            for child in k.children() {
                if child.level < grid.finest && layout.tiles.contains(&child) {
                    queue.push(Candidate {
                        score: evaluate(child).0,
                        key: child,
                    });
                }
            }
        }
    }
    let mut samples = Vec::new();
    for &key in &layout.tiles {
        if key.level < grid.finest {
            let (_, px, visible) = evaluate(key);
            if visible {
                samples.push(px);
            }
        }
    }
    samples.sort_by(f64::total_cmp);
    let achieved = if samples.is_empty() {
        requested
    } else {
        samples[((samples.len() - 1) as f64 * 0.95) as usize].max(requested)
    };
    Selection {
        native_policy_radius: radius,
        layout,
        requested_pixels: requested,
        achieved_pixels: achieved,
        budget_limited: limited,
    }
}
/// One balanced local step toward a desired cover. A patch includes the changed
/// neighbor mesh keys, so swapping it cannot leave stale boundary geometry.
/// Fine tiles outside the desired cover coarsen first to free residency.
pub fn next_patch(active: &Layout, desired: &Layout, eye: [f64; 3]) -> Option<Layout> {
    next_patch_budget(
        active,
        desired,
        eye,
        active.tiles.len().max(desired.tiles.len()) + 64,
    )
}
/// Refinement has first claim on spare residency; coarsen only to free space.
pub fn next_patch_budget(
    active: &Layout,
    desired: &Layout,
    eye: [f64; 3],
    budget: usize,
) -> Option<Layout> {
    let grid = active.grid;
    if active == desired {
        return None;
    }
    let mut merges = BTreeSet::new();
    for &key in &active.tiles {
        if let Some(parent) = key.parent() {
            let [x, z] = parent.origin_in(grid);
            if desired.at(x, z).is_some_and(|d| d.level <= parent.level)
                && parent
                    .children()
                    .filter(|c| {
                        let [x, z] = c.origin_in(grid);
                        x < grid.extent && z < grid.extent
                    })
                    .all(|c| active.tiles.contains(&c))
            {
                merges.insert(parent);
            }
        }
    }
    let distance = |key: TileKey| {
        let [x, z] = key.origin_in(grid);
        let e = key.edge_in(grid);
        ((x + e / 2) as f64 * grid.cell_m() - eye[0])
            .hypot((z + e / 2) as f64 * grid.cell_m() - eye[2])
    };
    let mut splits: Vec<_> = active
        .tiles
        .iter()
        .copied()
        .filter(|key| {
            let [x, z] = key.origin_in(grid);
            key.level < grid.finest
                && !desired.tiles.contains(key)
                && desired.at(x, z).is_some_and(|d| d.level > key.level)
        })
        .collect();
    splits.sort_by(|a, b| distance(*a).total_cmp(&distance(*b)));
    if active.tiles.len() + 3 <= budget {
        if let Some(key) = splits.first() {
            let mut next = active.clone();
            next.split(*key);
            next.balance();
            if next.tiles.len() <= budget {
                return Some(next);
            }
        }
    }
    let mut candidates: Vec<_> = merges.into_iter().collect();
    candidates.sort_by(|a, b| distance(*b).total_cmp(&distance(*a)));
    for parent in candidates {
        let too_fine = active.has_finer_neighbor(parent);
        if too_fine {
            continue;
        }
        let mut next = active.clone();
        for child in parent.children() {
            next.tiles.remove(&child);
        }
        next.tiles.insert(parent);
        // The neighbor check makes this merge balanced by construction. Avoid
        // cloning/rebalancing every rejected candidate on the rendering thread.
        return Some(next);
    }
    let mut splits: Vec<_> = active
        .tiles
        .iter()
        .copied()
        .filter(|key| {
            let [x, z] = key.origin_in(grid);
            key.level < grid.finest
                && !desired.tiles.contains(key)
                && desired.at(x, z).is_some_and(|d| d.level > key.level)
        })
        .collect();
    splits.sort_by(|a, b| distance(*a).total_cmp(&distance(*b)));
    splits.first().map(|key| {
        let mut next = active.clone();
        next.split(*key);
        next.balance();
        next
    })
}
/// Neighbor levels are part of mesh identity: a boundary changes when its LOD
/// neighbor changes, even if the tile's own level remains unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MeshKey {
    pub tile: TileKey,
    pub neighbors: [u8; 4],
    /// Coarsest incident leaf at (-X,-Z), (-X,+Z), (+X,+Z), (+X,-Z).
    /// Coarse layout keys include these. Native cells do not use corner filtering.
    pub corner_levels: Option<[u8; 4]>,
    pub revision: u64,
}
impl Layout {
    pub fn mesh_keys(&self, revision: u64) -> BTreeMap<TileKey, MeshKey> {
        let mut corners = BTreeMap::new();
        self.tiles
            .iter()
            .map(|&tile| {
                (
                    tile,
                    MeshKey {
                        corner_levels: if tile.level == self.grid.finest {
                            None
                        } else {
                            Some(self.corners(tile).map(|point| {
                                *corners
                                    .entry(point)
                                    .or_insert_with(|| self.corner_level_at(point, tile.level))
                            }))
                        },
                        tile,
                        neighbors: self.neighbors(tile),
                        revision,
                    },
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_mesh_identity_omits_unused_corner_filters() {
        let grid = Grid::default();
        let mut layout = Layout::root_in(grid);
        let root = *layout.tiles.iter().next().unwrap();
        let mut tile = root;
        while tile.level < grid.finest {
            layout.split(tile);
            tile = tile.children().next().unwrap();
        }
        let keys = layout.mesh_keys(7);
        assert_eq!(keys[&tile].corner_levels, None);
        assert!(keys
            .values()
            .filter(|k| k.tile.level < grid.finest)
            .all(|k| k.corner_levels.is_some()));
    }
    #[test]
    fn balance_and_merges_include_diagonal_neighbors() {
        let grid = Grid::default();
        let level = grid.finest - 3;
        let base: Vec<_> = (0..4)
            .map(|i| TileKey {
                level,
                x: i % 2,
                z: i / 2,
            })
            .collect();
        let desired = Layout {
            grid,
            tiles: base.iter().copied().collect(),
        };
        let mut active = desired.clone();
        for &tile in &base[1..] {
            active.split(tile);
        }
        active.split(base[3].children().next().unwrap());
        assert!(active
            .tiles
            .iter()
            .any(|&k| active.corner_levels(k).into_iter().any(|l| k.level > l + 1)));
        active.balance();
        for _ in 0..30 {
            for &tile in &active.tiles {
                assert!(active
                    .corner_levels(tile)
                    .into_iter()
                    .all(|l| tile.level <= l + 1));
            }
            if active == desired {
                return;
            }
            active = next_patch_budget(&active, &desired, [0., 10., 0.], 128).unwrap();
        }
        panic!("corner-balanced coarsening did not converge");
    }

    fn grid(extent: i64) -> Grid {
        let root = (extent.max(SIDE) as u64).next_power_of_two() as i64;
        Grid {
            extent,
            root,
            finest: (root.trailing_zeros() - 5) as u8,
            ..Grid::default()
        }
    }
    /// Independent full edge-overlap oracle, including finite clipped corners.
    fn assert_balanced(layout: &Layout) {
        for &a in &layout.tiles {
            assert!(
                layout
                    .neighbors(a)
                    .iter()
                    .all(|&level| level.abs_diff(a.level) <= 1),
                "mesh identity {a:?}"
            );
            let [ax, az] = a.origin_in(layout.grid);
            let ae = a.edge_in(layout.grid);
            let ar = (ax + ae).min(layout.grid.extent);
            let ab = (az + ae).min(layout.grid.extent);
            for &b in &layout.tiles {
                if a == b {
                    continue;
                }
                let [bx, bz] = b.origin_in(layout.grid);
                let be = b.edge_in(layout.grid);
                let br = (bx + be).min(layout.grid.extent);
                let bb = (bz + be).min(layout.grid.extent);
                let touches = ((ar == bx || br == ax) && az.max(bz) < ab.min(bb))
                    || ((ab == bz || bb == az) && ax.max(bx) < ar.min(br));
                assert!(
                    !touches || a.level.abs_diff(b.level) <= 1,
                    "unbalanced clipped neighbors {a:?} {b:?}, extent {}",
                    layout.grid.extent
                );
            }
        }
    }
    fn refine_at(grid: Grid, point: [i64; 2], depth: u8) -> Layout {
        let mut layout = Layout::root_in(grid);
        while layout.at(point[0], point[1]).unwrap().level < depth.min(grid.finest) {
            layout.split(layout.at(point[0], point[1]).unwrap());
            layout.balance();
        }
        layout
    }
    #[test]
    fn partial_corner_patch_splits_and_merges_always_preserve_true_edge_balance() {
        for extent in [96, 97, 1001, 131073, 147457, 160000] {
            let grid = grid(extent);
            let mut active = Layout::root_in(grid);
            for point in [
                [extent - 1, extent - 1],
                [extent * 3 / 4, extent * 3 / 4],
                [extent - 1, extent / 3],
                [extent / 3, extent - 1],
                [extent / 2, extent / 2],
            ] {
                let desired = refine_at(grid, point, 6);
                assert_balanced(&desired);
                let eye = [
                    point[0] as f64 * grid.cell_m(),
                    100.,
                    point[1] as f64 * grid.cell_m(),
                ];
                for _ in 0..1000 {
                    let Some(next) = next_patch(&active, &desired, eye) else {
                        break;
                    };
                    assert_balanced(&next);
                    active = next;
                }
                assert_eq!(active, desired, "patch convergence at extent {extent}");
            }
            let root = Layout::root_in(grid);
            for _ in 0..1000 {
                let Some(next) = next_patch(&active, &root, [0.; 3]) else {
                    break;
                };
                assert_balanced(&next);
                active = next;
            }
            assert_eq!(active, root);
        }
    }
    #[test]
    fn merge_sees_fine_neighbor_in_partial_corner_before_nominal_quarter() {
        let grid = grid(160000);
        let active = refine_at(grid, [131071, 140000], 3);
        let parent = TileKey {
            level: 1,
            x: 1,
            z: 1,
        };
        assert_balanced(&active);
        assert!(parent
            .children()
            .filter(|child| {
                let [x, z] = child.origin_in(grid);
                x < grid.extent && z < grid.extent
            })
            .all(|child| active.tiles.contains(&child)));
        // Both old nominal quarter probes fall outside this 28,928-cell strip.
        assert!(parent.origin_in(grid)[1] + parent.edge_in(grid) / 4 >= grid.extent);
        assert!(active.has_finer_neighbor(parent));
        let desired = Layout::root_in(grid);
        let next = next_patch(&active, &desired, [0.; 3]).unwrap();
        assert_balanced(&next);
    }
    #[test]
    fn narrow_corner_midpoint_stays_inside_each_valid_side() {
        let grid = grid(160000);
        let mut layout = refine_at(grid, [159999, 159999], 4);
        let corner = layout.at(159999, 159999).unwrap();
        let points = layout.side_midpoints(corner);
        assert!(points[0][1] < grid.extent && points[2][0] < grid.extent);
        // Deliberately create an imbalance around the clipped corner; balancing
        // must see the real neighbor despite the nominal midpoint being outside.
        let coarse = TileKey {
            level: 1,
            x: 1,
            z: 1,
        };
        layout.tiles.retain(|tile| {
            let [x, z] = tile.origin_in(grid);
            x < grid.root / 2 || z < grid.root / 2
        });
        layout.tiles.insert(coarse);
        layout.balance();
        assert_balanced(&layout);
    }
}
