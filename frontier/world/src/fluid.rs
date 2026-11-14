//! Conservative, bounded native-cell water. Greenfield integer CA, not a pressure
//! or incompressible solver. Geometry is occupied whole cells even at mass 1.
//! Fixed activation schedules replay identically. Unloaded cells are sealed;
//! simulation elapsed time intentionally depends on the activation schedule.
use crate::Cell;
use std::collections::{BTreeMap, BTreeSet};

pub const FLUID_CAPACITY: u16 = 256;
pub const FLUID_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FluidConfig {
    pub max_loaded_cells: usize,
    /// Includes dormant cells and dry tombstones. Reject activation rather than
    /// evict authoritative history and accidentally regenerate drained water.
    pub max_stored_cells: usize,
    pub sleep_ticks: u16,
}
impl Default for FluidConfig {
    fn default() -> Self {
        Self {
            max_loaded_cells: 32_768,
            max_stored_cells: 1_048_576,
            sleep_ticks: 4,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct FluidRegion {
    pub min: Cell,
    pub max: Cell,
}
impl FluidRegion {
    pub fn new(min: Cell, max: Cell) -> Self {
        Self { min, max }
    }
    fn volume(self) -> Option<usize> {
        let d = [
            self.max.x as i128 - self.min.x as i128,
            self.max.y as i128 - self.min.y as i128,
            self.max.z as i128 - self.min.z as i128,
        ];
        if d.iter().any(|v| *v <= 0) {
            return None;
        }
        usize::try_from(d[0].checked_mul(d[1])?.checked_mul(d[2])?).ok()
    }
    fn cells(self) -> impl Iterator<Item = Cell> {
        (self.min.x..self.max.x).flat_map(move |x| {
            (self.min.y..self.max.y)
                .flat_map(move |y| (self.min.z..self.max.z).map(move |z| Cell { x, y, z }))
        })
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FluidError {
    InvalidRegion,
    Capacity,
    InvalidMass,
    InvalidCheckpoint,
}
impl std::fmt::Display for FluidError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for FluidError {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FluidDiagnostics {
    pub tick: u64,
    pub initial_mass: u64,
    pub additions: u64,
    pub removals: u64,
    pub resident_mass: u64,
    pub dormant_mass: u64,
    pub displaced_mass: u64,
    pub loaded_cells: usize,
    pub stored_cells: usize,
    pub awake_cells: usize,
}
impl FluidDiagnostics {
    pub fn conserved(self) -> bool {
        self.initial_mass as u128 + self.additions as u128
            == self.removals as u128
                + self.resident_mass as u128
                + self.dormant_mass as u128
                + self.displaced_mass as u128
    }
}
#[derive(Clone, Debug, Default)]
pub struct FluidStep {
    pub changed: Vec<Cell>,
    pub transfers: usize,
    pub visited_cells: usize,
}
#[derive(Clone, Debug)]
pub struct FluidWorld {
    config: FluidConfig,
    cells: BTreeMap<Cell, u16>,
    loaded: BTreeSet<Cell>,
    regions: BTreeSet<FluidRegion>,
    awake: BTreeMap<Cell, u16>,
    displaced: BTreeMap<Cell, u64>,
    dirty: BTreeSet<Cell>,
    tick: u64,
    initial_mass: u64,
    additions: u64,
    removals: u64,
}
fn offset(c: Cell, d: [i64; 3]) -> Option<Cell> {
    Some(Cell {
        x: c.x.checked_add(d[0])?,
        y: c.y.checked_add(d[1])?,
        z: c.z.checked_add(d[2])?,
    })
}
const NEIGHBORS: [[i64; 3]; 6] = [
    [0, -1, 0],
    [-1, 0, 0],
    [0, 0, -1],
    [0, 0, 1],
    [1, 0, 0],
    [0, 1, 0],
];
impl FluidWorld {
    pub fn new(config: FluidConfig) -> Self {
        Self {
            config,
            cells: BTreeMap::new(),
            loaded: BTreeSet::new(),
            regions: BTreeSet::new(),
            awake: BTreeMap::new(),
            displaced: BTreeMap::new(),
            dirty: BTreeSet::new(),
            tick: 0,
            initial_mass: 0,
            additions: 0,
            removals: 0,
        }
    }
    pub fn config(&self) -> FluidConfig {
        self.config
    }
    /// Raise persistent-history limits or adjust the bounded resident window.
    /// Shrinking below current authoritative residency is rejected atomically.
    pub fn set_config(&mut self, config: FluidConfig) -> Result<(), FluidError> {
        if self.loaded.len() > config.max_loaded_cells || self.cells.len() > config.max_stored_cells
        {
            return Err(FluidError::Capacity);
        }
        self.config = config;
        // A smaller settling window remains valid in the next checkpoint.
        for idle in self.awake.values_mut() {
            *idle = (*idle).min(config.sleep_ticks.max(1) - 1);
        }
        Ok(())
    }
    pub fn tick(&self) -> u64 {
        self.tick
    }
    pub fn mass(&self, c: Cell) -> Option<u16> {
        self.cells.get(&c).copied()
    }
    pub fn is_loaded(&self, c: Cell) -> bool {
        self.loaded.contains(&c)
    }
    /// Includes dry tombstones and dormant overrides; never infer regeneration
    /// from a zero mass value. Terrain remains independently authoritative.
    pub fn overrides(&self) -> impl Iterator<Item = (Cell, u16)> + '_ {
        self.cells.iter().map(|(c, m)| (*c, *m))
    }
    pub fn loaded_overrides(&self) -> impl Iterator<Item = (Cell, u16)> + '_ {
        self.loaded.iter().map(|c| (*c, self.cells[c]))
    }
    pub fn loaded_regions(&self) -> impl Iterator<Item = FluidRegion> + '_ {
        self.regions.iter().copied()
    }
    pub fn load_region(
        &mut self,
        region: FluidRegion,
        mut initial: impl FnMut(Cell) -> u16,
    ) -> Result<(), FluidError> {
        let n = region.volume().ok_or(FluidError::InvalidRegion)?;
        if self.regions.contains(&region) {
            return Ok(());
        }
        // Nonoverlap makes unload ownership unambiguous. Reject before calling source.
        if n > self.config.max_loaded_cells
            || self.loaded.len().saturating_add(n) > self.config.max_loaded_cells
        {
            return Err(FluidError::Capacity);
        }
        if region.cells().any(|c| self.loaded.contains(&c)) {
            return Err(FluidError::InvalidRegion);
        }
        let missing = region
            .cells()
            .filter(|c| !self.cells.contains_key(c))
            .count();
        if self.cells.len().saturating_add(missing) > self.config.max_stored_cells {
            return Err(FluidError::Capacity);
        }
        // Stage before mutation so invalid input cannot partially activate.
        let mut fresh = Vec::with_capacity(missing);
        for c in region.cells() {
            if !self.cells.contains_key(&c) {
                let m = initial(c);
                if m > FLUID_CAPACITY {
                    return Err(FluidError::InvalidMass);
                }
                fresh.push((c, m));
            }
        }
        for (c, m) in fresh {
            self.cells.insert(c, m);
            self.initial_mass += m as u64;
        }
        self.loaded.extend(region.cells());
        self.regions.insert(region);
        self.dirty.extend(region.cells());
        self.awake.extend(region.cells().map(|c| (c, 0)));
        // Internal neighbors are already awake. Only cross-boundary loaded
        // cells need a wake, avoiding seven tree lookups per activated cell.
        for c in region.cells().filter(|c| {
            c.x == region.min.x
                || c.x == region.max.x - 1
                || c.y == region.min.y
                || c.y == region.max.y - 1
                || c.z == region.min.z
                || c.z == region.max.z - 1
        }) {
            for d in NEIGHBORS {
                if let Some(n) = offset(c, d) {
                    let outside = n.x < region.min.x
                        || n.x >= region.max.x
                        || n.y < region.min.y
                        || n.y >= region.max.y
                        || n.z < region.min.z
                        || n.z >= region.max.z;
                    if outside && self.loaded.contains(&n) {
                        self.awake.insert(n, 0);
                    }
                }
            }
        }
        Ok(())
    }
    pub fn unload_region(&mut self, region: FluidRegion) {
        if self.regions.remove(&region) {
            for c in region.cells() {
                self.loaded.remove(&c);
                self.awake.remove(&c);
            }
            // No mass crossing the boundary; dormant history remains untouched.
        }
    }
    pub fn wake(&mut self, c: Cell) {
        if self.loaded.contains(&c) {
            self.awake.insert(c, 0);
        }
        for d in NEIGHBORS {
            if let Some(n) = offset(c, d) {
                if self.loaded.contains(&n) {
                    self.awake.insert(n, 0);
                }
            }
        }
    }
    /// Explicit source/sink, including edits which intentionally delete water.
    pub fn set_mass(&mut self, c: Cell, mass: u16) -> Result<(), FluidError> {
        if mass > FLUID_CAPACITY {
            return Err(FluidError::InvalidMass);
        }
        let old = *self.cells.get(&c).ok_or(FluidError::InvalidRegion)?;
        if mass >= old {
            self.additions += (mass - old) as u64;
        } else {
            self.removals += (old - mass) as u64;
        }
        self.cells.insert(c, mass);
        self.dirty.insert(c);
        self.wake(c);
        Ok(())
    }
    /// Placement preserves fluid as pending displaced mass until space is
    /// available in loaded adjacent cells. Save this state with the terrain edit.
    pub fn notify_edit(&mut self, c: Cell, solid: bool) {
        if solid {
            if let Some(m) = self.cells.get_mut(&c) {
                if *m > 0 {
                    *self.displaced.entry(c).or_default() += *m as u64;
                    *m = 0;
                    self.dirty.insert(c);
                }
            }
        }
        self.wake(c);
    }
    pub fn diagnostics(&self) -> FluidDiagnostics {
        let mut d = FluidDiagnostics {
            tick: self.tick,
            initial_mass: self.initial_mass,
            additions: self.additions,
            removals: self.removals,
            loaded_cells: self.loaded.len(),
            stored_cells: self.cells.len(),
            awake_cells: self.awake.len(),
            displaced_mass: self.displaced.values().sum(),
            ..Default::default()
        };
        for (c, m) in &self.cells {
            if self.loaded.contains(c) {
                d.resident_mass += *m as u64;
            } else {
                d.dormant_mass += *m as u64;
            }
        }
        d
    }
    pub fn step(&mut self, mut solid: impl FnMut(Cell) -> bool) -> FluidStep {
        self.tick = self.tick.saturating_add(1);
        // External edits and activation already woke their neighbors. Publish
        // their dirty cells without repeatedly waking unchanged dry interiors.
        let published_dirty = std::mem::take(&mut self.dirty);
        let mut changed = BTreeSet::new();
        // Handle newly solid cells even when callers only wake an edited region.
        let active: Vec<_> = self.awake.keys().copied().collect();
        for c in &active {
            if self.cells[c] > 0 && solid(*c) {
                self.notify_edit(*c, true);
                changed.insert(*c);
            }
        }
        // Deterministic bounded displacement. No mass is silently destroyed when
        // the six-cell neighborhood is full or absent.
        // Dormant pending mass is authoritative history, not active tick work.
        // Iterate the smaller ordered index and copy only resident entries.
        let pending: Vec<_> = if self.displaced.len() <= self.loaded.len() {
            self.displaced
                .keys()
                .filter(|c| self.loaded.contains(c))
                .copied()
                .collect()
        } else {
            self.loaded
                .iter()
                .filter(|c| self.displaced.contains_key(c))
                .copied()
                .collect()
        };
        for c in pending {
            if !self.loaded.contains(&c) {
                continue;
            }
            let mut remaining = self.displaced[&c];
            for n in std::iter::once(c).chain(NEIGHBORS.into_iter().filter_map(|d| offset(c, d))) {
                if remaining == 0 {
                    break;
                }
                if self.loaded.contains(&n) && !solid(n) {
                    let m = self.cells.get_mut(&n).unwrap();
                    let amount = remaining.min((FLUID_CAPACITY - *m) as u64);
                    if amount > 0 {
                        *m += amount as u16;
                        remaining -= amount;
                        changed.insert(n);
                    }
                }
            }
            if remaining == 0 {
                self.displaced.remove(&c);
            } else {
                self.displaced.insert(c, remaining);
            }
        }
        for c in changed.iter().copied().collect::<Vec<_>>() {
            self.wake(c);
        }
        // All proposals read tick-N mass. Faces have unique lower-coordinate
        // ownership; gravity is resolved first, then sorted horizontal faces.
        // Receiver/source budgets use tick-N capacity, so incoming mass cannot
        // cross a second face this tick. Integer horizontal remainder stays put.
        let mut proposals = Vec::new();
        let mut horizontal = BTreeSet::new();
        let candidates: Vec<_> = self.awake.keys().copied().collect();
        for &c in &candidates {
            if self.cells[&c] == 0 || solid(c) {
                continue;
            }
            if let Some(n) = offset(c, [0, -1, 0]) {
                if self.loaded.contains(&n) && !solid(n) {
                    let a = self.cells[&c].min(FLUID_CAPACITY - self.cells[&n]);
                    if a > 0 {
                        proposals.push((c, n, a));
                    }
                }
            }
            for d in [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]] {
                if let Some(n) = offset(c, d) {
                    if self.loaded.contains(&n) && !solid(n) {
                        horizontal.insert(if c < n { (c, n) } else { (n, c) });
                    }
                }
            }
        }
        for (a, b) in horizontal {
            let ma = self.cells[&a];
            let mb = self.cells[&b];
            let diff = ma.abs_diff(mb);
            if diff > 1 {
                let (s, t) = if ma > mb { (a, b) } else { (b, a) };
                proposals.push((s, t, (diff / 8).max(1)));
            }
        }
        let mut outgoing: BTreeMap<Cell, u16> = BTreeMap::new();
        let mut incoming: BTreeMap<Cell, u16> = BTreeMap::new();
        let mut transfers = 0;
        for (s, t, amount) in proposals {
            let a = amount
                .min(self.cells[&s] - outgoing.get(&s).copied().unwrap_or(0))
                .min(FLUID_CAPACITY - self.cells[&t] - incoming.get(&t).copied().unwrap_or(0));
            if a > 0 {
                *outgoing.entry(s).or_default() += a;
                *incoming.entry(t).or_default() += a;
                changed.insert(s);
                changed.insert(t);
                transfers += 1;
            }
        }
        for (c, a) in outgoing {
            *self.cells.get_mut(&c).unwrap() -= a;
        }
        for (c, a) in incoming {
            *self.cells.get_mut(&c).unwrap() += a;
        }
        // A stable region sleeps only after no legal transfer for this many ticks.
        let settle = self.config.sleep_ticks.max(1);
        self.awake.retain(|_, idle| {
            *idle = idle.saturating_add(1);
            *idle < settle
        });
        for &c in &changed {
            self.wake(c);
        }
        changed.extend(published_dirty);
        changed.append(&mut self.dirty);
        debug_assert!(self.diagnostics().conserved());
        FluidStep {
            changed: changed.into_iter().collect(),
            transfers,
            visited_cells: candidates.len(),
        }
    }
    /// Self-contained deterministic checkpoint. The runtime must envelope these
    /// bytes with seed/source identity and terrain edits in one atomic save.
    pub fn persist_bytes(&self) -> Vec<u8> {
        let mut out = b"FVFLUID1".to_vec();
        fn put(out: &mut Vec<u8>, n: u64) {
            out.extend_from_slice(&n.to_le_bytes());
        }
        fn cell(out: &mut Vec<u8>, c: Cell) {
            for n in [c.x, c.y, c.z] {
                out.extend_from_slice(&n.to_le_bytes());
            }
        }
        for n in [
            FLUID_SCHEMA_VERSION as u64,
            self.config.max_loaded_cells as u64,
            self.config.max_stored_cells as u64,
            self.config.sleep_ticks as u64,
            self.tick,
            self.initial_mass,
            self.additions,
            self.removals,
            self.cells.len() as u64,
        ] {
            put(&mut out, n);
        }
        for (&c, &m) in &self.cells {
            cell(&mut out, c);
            put(&mut out, m as u64);
        }
        put(&mut out, self.regions.len() as u64);
        for r in &self.regions {
            cell(&mut out, r.min);
            cell(&mut out, r.max);
        }
        put(&mut out, self.awake.len() as u64);
        for (&c, &idle) in &self.awake {
            cell(&mut out, c);
            put(&mut out, idle as u64);
        }
        put(&mut out, self.displaced.len() as u64);
        for (&c, &m) in &self.displaced {
            cell(&mut out, c);
            put(&mut out, m);
        }
        put(&mut out, self.dirty.len() as u64);
        for &c in &self.dirty {
            cell(&mut out, c);
        }
        out
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, FluidError> {
        struct Reader<'a>(&'a [u8]);
        impl Reader<'_> {
            fn n(&mut self) -> Result<u64, FluidError> {
                let (a, b) = self
                    .0
                    .split_at_checked(8)
                    .ok_or(FluidError::InvalidCheckpoint)?;
                self.0 = b;
                Ok(u64::from_le_bytes(a.try_into().unwrap()))
            }
            fn count(&mut self) -> Result<usize, FluidError> {
                usize::try_from(self.n()?).map_err(|_| FluidError::InvalidCheckpoint)
            }
            fn cell(&mut self) -> Result<Cell, FluidError> {
                Ok(Cell {
                    x: self.n()? as i64,
                    y: self.n()? as i64,
                    z: self.n()? as i64,
                })
            }
        }
        let err = FluidError::InvalidCheckpoint;
        if !bytes.starts_with(b"FVFLUID1") {
            return Err(err);
        }
        let mut r = Reader(&bytes[8..]);
        if r.n()? != FLUID_SCHEMA_VERSION as u64 {
            return Err(err);
        }
        let config = FluidConfig {
            max_loaded_cells: r.count()?,
            max_stored_cells: r.count()?,
            sleep_ticks: u16::try_from(r.n()?).map_err(|_| err.clone())?,
        };
        let mut world = Self::new(config);
        world.tick = r.n()?;
        world.initial_mass = r.n()?;
        world.additions = r.n()?;
        world.removals = r.n()?;
        let count = r.count()?;
        if count > config.max_stored_cells || count > r.0.len() / 32 {
            return Err(err);
        }
        for _ in 0..count {
            let c = r.cell()?;
            let m = r.n()?;
            if m > FLUID_CAPACITY as u64 || world.cells.insert(c, m as u16).is_some() {
                return Err(err);
            }
        }
        let regions = r.count()?;
        if regions > config.max_loaded_cells || regions > r.0.len() / 48 {
            return Err(err);
        }
        for _ in 0..regions {
            let region = FluidRegion::new(r.cell()?, r.cell()?);
            let n = region.volume().ok_or(err.clone())?;
            if n > config.max_loaded_cells
                || world.loaded.len().saturating_add(n) > config.max_loaded_cells
            {
                return Err(err);
            }
            for c in region.cells() {
                if !world.cells.contains_key(&c) || !world.loaded.insert(c) {
                    return Err(err);
                }
            }
            if !world.regions.insert(region) {
                return Err(err);
            }
        }
        let awake = r.count()?;
        if awake > world.loaded.len() || awake > r.0.len() / 32 {
            return Err(err);
        }
        for _ in 0..awake {
            let c = r.cell()?;
            let idle = r.n()?;
            if idle >= config.sleep_ticks.max(1) as u64
                || !world.loaded.contains(&c)
                || world.awake.insert(c, idle as u16).is_some()
            {
                return Err(err);
            }
        }
        let displaced = r.count()?;
        if displaced > world.cells.len() || displaced > r.0.len() / 32 {
            return Err(err);
        }
        for _ in 0..displaced {
            let c = r.cell()?;
            let m = r.n()?;
            if m == 0 || !world.cells.contains_key(&c) || world.displaced.insert(c, m).is_some() {
                return Err(err);
            }
        }
        let dirty = r.count()?;
        if dirty > world.cells.len() || dirty > r.0.len() / 24 {
            return Err(err);
        }
        for _ in 0..dirty {
            let c = r.cell()?;
            if !world.cells.contains_key(&c) || !world.dirty.insert(c) {
                return Err(err);
            }
        }
        // u128 validation avoids accepting malicious overflowing u64 totals.
        let total = world.cells.values().map(|m| *m as u128).sum::<u128>()
            + world.displaced.values().map(|m| *m as u128).sum::<u128>();
        if !r.0.is_empty()
            || total > u64::MAX as u128
            || total + world.removals as u128
                != world.initial_mass as u128 + world.additions as u128
        {
            return Err(err);
        }
        Ok(world)
    }
}
impl Default for FluidWorld {
    fn default() -> Self {
        Self::new(FluidConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c(x: i64, y: i64, z: i64) -> Cell {
        Cell { x, y, z }
    }
    fn region(min: [i64; 3], max: [i64; 3]) -> FluidRegion {
        FluidRegion::new(c(min[0], min[1], min[2]), c(max[0], max[1], max[2]))
    }
    #[test]
    fn sealed_box_conserves_every_tick_and_capacity() {
        let mut w = FluidWorld::default();
        w.load_region(region([-4, 0, -4], [4, 6, 4]), |p| {
            if p.y == 5 {
                256
            } else {
                0
            }
        })
        .unwrap();
        let initial = w.diagnostics().initial_mass;
        for _ in 0..250 {
            w.step(|_| false);
            assert!(w.diagnostics().conserved());
            assert_eq!(w.diagnostics().resident_mass, initial);
            assert!(w.overrides().all(|(_, m)| m <= 256));
        }
        assert!(w
            .overrides()
            .filter(|(p, _)| p.y == 0)
            .all(|(_, m)| m >= 255));
    }
    #[test]
    fn negative_chunk_boundary_sealed_until_both_load_and_wake() {
        let mut w = FluidWorld::default();
        let a = region([-2, 0, 0], [0, 1, 1]);
        let b = region([0, 0, 0], [2, 1, 1]);
        w.load_region(a, |_| 256).unwrap();
        for _ in 0..10 {
            w.step(|_| false);
        }
        assert_eq!(w.diagnostics().awake_cells, 0);
        assert_eq!(w.mass(c(0, 0, 0)), None);
        w.load_region(b, |_| 0).unwrap();
        for _ in 0..20 {
            w.step(|_| false);
        }
        assert!(w.mass(c(0, 0, 0)).unwrap() > 0);
        let before = w.mass(c(0, 0, 0));
        w.unload_region(b);
        for _ in 0..10 {
            w.step(|_| false);
        }
        assert_eq!(w.mass(c(0, 0, 0)), before);
        assert!(w.diagnostics().conserved());
    }
    #[test]
    fn dry_tombstones_survive_unload_restart_without_source_refill() {
        let mut w = FluidWorld::default();
        let r = region([0, 0, 0], [1, 2, 1]);
        w.load_region(r, |p| if p.y == 1 { 256 } else { 0 })
            .unwrap();
        w.step(|_| false);
        assert_eq!(w.mass(c(0, 1, 0)), Some(0));
        w.unload_region(r);
        let mut restored = FluidWorld::from_bytes(&w.persist_bytes()).unwrap();
        restored
            .load_region(r, |_| panic!("existing history must never regenerate"))
            .unwrap();
        assert_eq!(restored.mass(c(0, 1, 0)), Some(0));
        assert_eq!(restored.mass(c(0, 0, 0)), Some(256));
        assert!(restored.diagnostics().conserved());
    }
    #[test]
    fn solid_edit_displaces_without_losing_mass_and_releases_on_reopen() {
        let mut w = FluidWorld::default();
        let p = c(0, 0, 0);
        w.load_region(region([0, 0, 0], [1, 1, 1]), |_| 256)
            .unwrap();
        w.notify_edit(p, true);
        assert_eq!(w.mass(p), Some(0));
        for _ in 0..10 {
            w.step(|_| true);
        }
        assert_eq!(w.diagnostics().displaced_mass, 256);
        let r = region([0, 0, 0], [1, 1, 1]);
        w.unload_region(r);
        w.step(|_| false);
        assert_eq!(w.diagnostics().displaced_mass, 256);
        assert_eq!(w.mass(p), Some(0));
        let mut w = FluidWorld::from_bytes(&w.persist_bytes()).unwrap();
        w.load_region(r, |_| {
            panic!("displaced dormant history must not regenerate")
        })
        .unwrap();
        w.notify_edit(p, false);
        w.step(|_| false);
        assert_eq!(w.mass(p), Some(256));
        assert_eq!(w.diagnostics().displaced_mass, 0);
        assert!(w.diagnostics().conserved());
    }
    #[test]
    fn replay_generation_order_and_restart_are_identical() {
        let a = region([-4, 0, 0], [0, 3, 2]);
        let b = region([0, 0, 0], [4, 3, 2]);
        let init = |p: Cell| if p.y == 2 && p.x < 0 { 256 } else { 0 };
        let mut first = FluidWorld::default();
        let mut second = FluidWorld::default();
        first.load_region(a, init).unwrap();
        first.load_region(b, init).unwrap();
        second.load_region(b, init).unwrap();
        second.load_region(a, init).unwrap();
        for tick in 0..100 {
            if tick == 27 {
                second = FluidWorld::from_bytes(&second.persist_bytes()).unwrap();
            }
            first.step(|p| p.x == 1 && p.y == 0);
            second.step(|p| p.x == 1 && p.y == 0);
            assert_eq!(first.persist_bytes(), second.persist_bytes());
        }
    }
    #[test]
    fn sleep_and_source_wake_preserve_explicit_ledger() {
        let mut w = FluidWorld::default();
        w.load_region(region([0, 0, 0], [2, 1, 1]), |_| 128)
            .unwrap();
        for _ in 0..5 {
            w.step(|_| false);
        }
        assert_eq!(w.diagnostics().awake_cells, 0);
        assert_eq!(w.step(|_| false).visited_cells, 0);
        w.set_mass(c(0, 0, 0), 256).unwrap();
        assert!(w.step(|_| false).transfers > 0);
        w.set_mass(c(0, 0, 0), 0).unwrap();
        assert!(w.diagnostics().conserved());
        assert_eq!(w.diagnostics().additions, 128);
        assert!(w.diagnostics().removals > 0);
    }
    #[test]
    fn concurrent_faces_edits_and_checkpoint_keep_capacity_and_mass() {
        let mut w = FluidWorld::default();
        w.load_region(region([-6, 0, -6], [6, 5, 6]), |p| {
            ((p.x.wrapping_mul(73856093) ^ p.y.wrapping_mul(19349663) ^ p.z.wrapping_mul(83492791))
                .unsigned_abs()
                % 257) as u16
        })
        .unwrap();
        for tick in 0..150 {
            let p = c((tick % 12) - 6, 2, ((tick * 7) % 12) - 6);
            w.notify_edit(p, true);
            let step = w.step(|q| q == p || (q.y == 0 && q.x % 3 == 0));
            assert!(step.changed.contains(&p));
            assert!(w.diagnostics().conserved());
            assert!(w.overrides().all(|(_, m)| m <= 256));
            if tick % 19 == 0 {
                w = FluidWorld::from_bytes(&w.persist_bytes()).unwrap();
            }
        }
    }
    #[test]
    fn external_changes_are_published_even_when_no_flux_occurs() {
        let mut w = FluidWorld::default();
        let p = c(0, 0, 0);
        w.load_region(region([0, 0, 0], [1, 1, 1]), |_| 256)
            .unwrap();
        w.step(|_| false);
        w.notify_edit(p, true);
        let mut w = FluidWorld::from_bytes(&w.persist_bytes()).unwrap();
        assert_eq!(w.step(|_| true).changed, vec![p]);
        w.set_mass(p, 64).unwrap();
        assert_eq!(w.step(|_| false).changed, vec![p]);
    }
    #[test]
    fn bounds_and_malformed_checkpoint_reject_without_partial_activation() {
        let mut w = FluidWorld::new(FluidConfig {
            max_loaded_cells: 2,
            max_stored_cells: 3,
            sleep_ticks: 4,
        });
        assert_eq!(
            w.load_region(region([0, 0, 0], [3, 1, 1]), |_| panic!()),
            Err(FluidError::Capacity)
        );
        assert_eq!(
            w.load_region(region([0, 0, 0], [2, 1, 1]), |p| if p.x == 1 {
                257
            } else {
                0
            }),
            Err(FluidError::InvalidMass)
        );
        assert_eq!(w.diagnostics().stored_cells, 0);
        w.load_region(region([0, 0, 0], [2, 1, 1]), |_| 128)
            .unwrap();
        let old_config = w.config();
        assert_eq!(
            w.set_config(FluidConfig {
                max_loaded_cells: 1,
                ..old_config
            }),
            Err(FluidError::Capacity)
        );
        assert_eq!(w.config(), old_config);
        w.step(|_| false);
        w.set_config(FluidConfig {
            max_stored_cells: 16,
            sleep_ticks: 1,
            ..old_config
        })
        .unwrap();
        let bytes = w.persist_bytes();
        assert!(FluidWorld::from_bytes(&bytes).is_ok());
        for len in 0..bytes.len() {
            assert!(FluidWorld::from_bytes(&bytes[..len]).is_err());
        }
        let mut corrupt = bytes;
        corrupt.push(0);
        assert!(FluidWorld::from_bytes(&corrupt).is_err());
    }
}
