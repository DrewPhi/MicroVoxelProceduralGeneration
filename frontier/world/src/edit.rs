//! Native gameplay edits. Inventory and expected-cell validation precede the
//! single canonical batch; renderer residency remains an explicit caller gate.
use crate::{Cell, Material, World, voxel_size};
use std::collections::{BTreeSet, VecDeque};

pub const MATERIALS: [Material; 10] = [
    Material::Air,
    Material::Grass,
    Material::Soil,
    Material::Stone,
    Material::Wood,
    Material::Leaf,
    Material::Flower,
    Material::Water,
    Material::Sand,
    Material::Snow,
];
#[derive(Clone, Copy, Debug)]
pub struct HandSettings {
    pub reach_m: f64,
    pub volume_liters: f64,
    pub extent_m: f64,
    pub quick_seconds: f64,
    pub full_seconds: f64,
}
impl Default for HandSettings {
    fn default() -> Self {
        Self {
            reach_m: 2.,
            volume_liters: 0.5,
            extent_m: 0.125,
            quick_seconds: 0.15,
            full_seconds: 0.9,
        }
    }
}
impl HandSettings {
    pub fn max_cells(self) -> usize {
        ((self.volume_liters / (1000. * voxel_size().powi(3))).floor() as usize)
            .max(1)
            .min(64)
    }
    pub fn count_at(self, elapsed: f64) -> usize {
        let fraction = ((elapsed - self.quick_seconds) / (self.full_seconds - self.quick_seconds))
            .clamp(0., 1.);
        1 + (fraction * (self.max_cells() - 1) as f64).floor() as usize
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Inventory {
    pub counts: [u64; 10],
    pub capacity: u64,
    pub selected: Material,
    pub sequence: u64,
}
impl Default for Inventory {
    fn default() -> Self {
        Self {
            counts: [0; 10],
            capacity: 64,
            selected: Material::Stone,
            sequence: 0,
        }
    }
}
impl Inventory {
    pub fn total(&self) -> u64 {
        self.counts.iter().sum()
    }
    pub fn available(&self) -> usize {
        self.capacity.saturating_sub(self.total()) as usize
    }
    pub fn select_slot(&mut self, slot: usize) {
        let solids = [
            Material::Grass,
            Material::Soil,
            Material::Stone,
            Material::Wood,
            Material::Sand,
            Material::Snow,
        ];
        if let Some(&m) = solids.get(slot) {
            self.selected = m;
        }
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        for v in self
            .counts
            .into_iter()
            .chain([self.capacity, self.selected as u64, self.sequence])
        {
            b.extend(v.to_le_bytes());
        }
        b
    }
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() != 13 * 8 {
            return None;
        }
        let values: Vec<_> = b
            .chunks_exact(8)
            .map(|x| u64::from_le_bytes(x.try_into().unwrap()))
            .collect();
        let counts: [u64; 10] = values[..10].try_into().ok()?;
        let selected = *MATERIALS.get(usize::try_from(values[11]).ok()?)?;
        let total = counts.iter().try_fold(0u64, |sum, x| sum.checked_add(*x))?;
        if total > values[10]
            || values[10] > 1_048_576
            || !selected.is_blocking()
            || MATERIALS
                .iter()
                .zip(counts)
                .any(|(m, n)| !m.is_blocking() && n != 0)
        {
            return None;
        }
        Some(Self {
            counts,
            capacity: values[10],
            selected,
            sequence: values[12],
        })
    }
}
fn key(c: Cell) -> (i64, i64, i64) {
    (c.x, c.y, c.z)
}
fn adjacent(c: Cell) -> Vec<Cell> {
    let mut out = Vec::new();
    for (dx, dy, dz) in [
        (-1, 0, 0),
        (0, -1, 0),
        (0, 0, -1),
        (0, 0, 1),
        (0, 1, 0),
        (1, 0, 0),
    ] {
        if let (Some(x), Some(y), Some(z)) = (
            c.x.checked_add(dx),
            c.y.checked_add(dy),
            c.z.checked_add(dz),
        ) {
            out.push(Cell { x, y, z });
        }
    }
    out
}
/// Compact face-connected BFS; coordinate order breaks equal-distance ties.
/// Every later cell connects through previously selected matching terrain.
pub fn select(
    world: &World,
    anchor: Cell,
    count: usize,
    settings: HandSettings,
    owns: impl Fn(Cell) -> bool,
) -> Vec<(Cell, Material)> {
    let material = world.material_without_fluid(anchor);
    if !material.is_blocking() || !owns(anchor) || count == 0 {
        return vec![];
    }
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([anchor]);
    let mut lo = [anchor.x, anchor.y, anchor.z];
    let mut hi = lo;
    while let Some(c) = queue.pop_front() {
        if !seen.insert(key(c)) || !owns(c) || world.material_without_fluid(c) != material {
            continue;
        }
        let p = [c.x, c.y, c.z];
        let next_lo: [i64; 3] = std::array::from_fn(|a| lo[a].min(p[a]));
        let next_hi: [i64; 3] = std::array::from_fn(|a| hi[a].max(p[a]));
        if (0..3).any(|a| {
            (next_hi[a] as i128 - next_lo[a] as i128 + 1) as f64 * voxel_size()
                > settings.extent_m + 1e-10
        }) {
            continue;
        }
        lo = next_lo;
        hi = next_hi;
        selected.push((c, material));
        if selected.len() == count.min(settings.max_cells()) {
            break;
        }
        queue.extend(adjacent(c));
    }
    selected
}
#[derive(Clone, Debug)]
pub struct EditTransaction {
    pub sequence: u64,
    pub expected: Vec<(Cell, Material)>,
    pub place: Option<Material>,
}
impl EditTransaction {
    pub fn pickup(inventory: &Inventory, expected: Vec<(Cell, Material)>) -> Self {
        Self {
            sequence: inventory.sequence.saturating_add(1),
            expected,
            place: None,
        }
    }
    pub fn placement(inventory: &Inventory, c: Cell, expected: Material) -> Self {
        Self {
            sequence: inventory.sequence.saturating_add(1),
            expected: vec![(c, expected)],
            place: Some(inventory.selected),
        }
    }
    pub fn apply(
        &self,
        world: &mut World,
        inventory: &mut Inventory,
        owns: impl Fn(Cell) -> bool,
    ) -> Result<Vec<Cell>, &'static str> {
        if inventory.sequence.checked_add(1) != Some(self.sequence) || self.expected.is_empty() {
            return Err("stale or empty edit transaction");
        }
        let mut seen = BTreeSet::new();
        for &(c, m) in &self.expected {
            if !seen.insert(key(c)) || !owns(c) || world.material_without_fluid(c) != m {
                return Err("terrain changed or native ownership unavailable");
            }
        }
        let mut next = inventory.clone();
        let changes: Vec<_> = if let Some(material) = self.place {
            if self.expected.len() != 1
                || !material.is_blocking()
                || next.counts[material as usize] == 0
                || self.expected[0].1 != Material::Air
            {
                return Err("placement needs owned material and empty terrain");
            }
            next.counts[material as usize] -= 1;
            vec![(self.expected[0].0, material)]
        } else {
            if self.expected.len() > inventory.available() {
                return Err("inventory full");
            }
            for &(_, m) in &self.expected {
                if !m.is_blocking() {
                    return Err("pickup requires solid terrain");
                }
                next.counts[m as usize] += 1;
                next.selected = m;
            }
            self.expected
                .iter()
                .map(|&(c, _)| (c, Material::Air))
                .collect()
        };
        next.sequence = self.sequence;
        world.set_batch(changes.iter().copied());
        *inventory = next;
        Ok(changes.into_iter().map(|(c, _)| c).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cells() -> (World, Cell, Cell) {
        let mut world = World::new(7);
        let a = Cell {
            x: -4097,
            y: 1000,
            z: -8193,
        };
        let b = Cell { x: a.x + 1, ..a };
        world.set_batch([(a, Material::Soil), (b, Material::Soil)]);
        (world, a, b)
    }
    #[test]
    fn physical_handful_and_deterministic_connected_selection() {
        let (world, a, b) = cells();
        let settings = HandSettings::default();
        assert_eq!(settings.max_cells(), 2);
        assert_eq!(settings.count_at(0.149), 1);
        assert_eq!(settings.count_at(0.9), 2);
        assert_eq!(
            select(&world, a, 2, settings, |_| true),
            vec![(a, Material::Soil), (b, Material::Soil)]
        );
        assert_eq!(
            select(&world, a, 2, settings, |c| c == a),
            vec![(a, Material::Soil)]
        );
    }
    #[test]
    fn transactions_are_atomic_finite_and_checkpoint_replay_safe() {
        let (mut world, a, b) = cells();
        let mut inventory = Inventory::default();
        inventory.capacity = 1;
        let txn =
            EditTransaction::pickup(&inventory, vec![(a, Material::Soil), (b, Material::Soil)]);
        let revision = world.revision();
        assert!(txn.apply(&mut world, &mut inventory, |_| true).is_err());
        assert_eq!(world.revision(), revision);
        assert_eq!(inventory.total(), 0);
        inventory.capacity = 2;
        txn.apply(&mut world, &mut inventory, |_| true).unwrap();
        assert_eq!(world.revision(), revision + 1);
        assert_eq!(inventory.counts[Material::Soil as usize], 2);
        assert!(txn.apply(&mut world, &mut inventory, |_| true).is_err());
        assert_eq!(
            Inventory::decode(&inventory.encode()),
            Some(inventory.clone())
        );
        EditTransaction::placement(&inventory, a, Material::Air)
            .apply(&mut world, &mut inventory, |_| true)
            .unwrap();
        assert_eq!(world.material(a), Material::Soil);
        assert_eq!(inventory.total(), 1);
    }
    #[test]
    fn underwater_solid_ray_has_two_metre_reach_and_nearest_solid_occlusion() {
        let (mut world, a, b) = cells();
        let water = Cell { x: a.x - 1, ..a };
        world.set(water, Material::Water);
        let origin = [
            (a.x as f64 - 1.5) * voxel_size(),
            (a.y as f64 + 0.5) * voxel_size(),
            (a.z as f64 + 0.5) * voxel_size(),
        ];
        assert_eq!(
            crate::raycast(&world, origin, [1., 0., 0.], 2.)
                .unwrap()
                .cell,
            water
        );
        assert_eq!(
            crate::raycast_solid(&world, origin, [1., 0., 0.], 2.)
                .unwrap()
                .cell,
            a
        );
        let far_origin = [origin[0] - 2., origin[1], origin[2]];
        assert!(crate::raycast_solid(&world, far_origin, [1., 0., 0.], 2.).is_none());
        // The solid at a prevents targeting the next matching cell through it.
        assert_ne!(
            crate::raycast_solid(&world, origin, [1., 0., 0.], 2.)
                .unwrap()
                .cell,
            b
        );
    }
    #[test]
    fn stale_preview_and_unresident_placement_leave_everything_unchanged() {
        let (mut world, a, b) = cells();
        let mut inventory = Inventory::default();
        let txn =
            EditTransaction::pickup(&inventory, vec![(a, Material::Soil), (b, Material::Soil)]);
        world.set(b, Material::Stone);
        assert!(txn.apply(&mut world, &mut inventory, |_| true).is_err());
        assert_eq!(world.material(a), Material::Soil);
        assert_eq!(inventory.total(), 0);
    }
}
