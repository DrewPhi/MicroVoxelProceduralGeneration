//! Four physical pockets and one hand. Swaps/stowing never duplicate contents;
//! terrain pickup/drop commits as a batch and preserves each loose material.
use crate::{garden::Garden, Cell, Material, World, VOXEL_SIZE};
use std::collections::BTreeMap;
pub const HANDFUL: usize = 4;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Seed,
    Bowl,
    Sand(Vec<Material>),
}
impl Item {
    pub fn name(&self) -> String {
        match self {
            Self::Seed => "Black seed".into(),
            Self::Bowl => "Bowl".into(),
            Self::Sand(v) => format!("Handful ×{}", v.len()),
        }
    }
}
#[derive(Clone, Debug)]
pub struct Pockets {
    pub slots: [Option<Item>; 4],
    pub hand: Option<Item>,
    pub selected: usize,
    pub picked: u64,
    pub placed: u64,
    pub legacy: u64,
    pub edits: BTreeMap<Cell, Material>,
}
impl Default for Pockets {
    fn default() -> Self {
        Self {
            slots: [Some(Item::Seed), Some(Item::Bowl), None, None],
            hand: None,
            selected: 0,
            picked: 0,
            placed: 0,
            legacy: 0,
            edits: BTreeMap::new(),
        }
    }
}
fn up(c: Cell, n: i64) -> Cell {
    Cell { y: c.y + n, ..c }
}
impl Pockets {
    pub fn select(&mut self, i: usize) -> Result<(), &'static str> {
        let slot = self.slots.get_mut(i).ok_or("Unknown pocket")?;
        std::mem::swap(slot, &mut self.hand);
        self.selected = i;
        Ok(())
    }
    pub fn stow(&mut self) -> Result<(), &'static str> {
        if self.hand.is_none() {
            return Ok(());
        }
        let i = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or("Your pockets are full — put the handful down first")?;
        self.slots[i] = self.hand.take();
        self.selected = i;
        Ok(())
    }
    pub fn bowl(&self) -> bool {
        self.hand == Some(Item::Bowl)
    }
    pub fn carried(&self) -> usize {
        self.slots
            .iter()
            .chain(std::iter::once(&self.hand))
            .filter_map(|v| {
                if let Some(Item::Sand(c)) = v {
                    Some(c.len())
                } else {
                    None
                }
            })
            .sum()
    }
    pub fn conserved(&self) -> bool {
        match (
            self.picked.checked_add(self.legacy),
            self.placed.checked_add(self.carried() as u64),
        ) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }
    pub fn validate(&self, seeds: u32) -> bool {
        let all = self.slots.iter().chain(std::iter::once(&self.hand));
        let mut bowls = 0;
        let mut seed = 0;
        for item in all.flatten() {
            match item {
                Item::Bowl => bowls += 1,
                Item::Seed => seed += 1,
                Item::Sand(v) => {
                    if v.is_empty()
                        || v.len() > HANDFUL
                        || !v
                            .iter()
                            .all(|m| matches!(m, Material::Sand | Material::Soil))
                    {
                        return false;
                    }
                }
            }
        }
        bowls == 1 && seed == seeds && self.selected < 4 && self.conserved()
    }
    pub fn selection(world: &World, c: Cell) -> Vec<(Cell, Material)> {
        let mut out = Vec::new();
        for n in 0..HANDFUL {
            let c = up(c, -(n as i64));
            let m = world.material(c);
            if !matches!(m, Material::Sand | Material::Soil) {
                break;
            }
            out.push((c, m));
        }
        out
    }
    pub fn pickup(
        &mut self,
        world: &mut World,
        cells: &[(Cell, Material)],
    ) -> Result<(), &'static str> {
        if self.hand.is_some() {
            return Err("Put away or drop what is in your hands first");
        }
        if cells.is_empty()
            || cells.len() > HANDFUL
            || cells.iter().enumerate().any(|(i, (c, m))| {
                *c != up(cells[0].0, -(i as i64))
                    || !matches!(m, Material::Sand | Material::Soil)
                    || world.material(*c) != *m
            })
        {
            return Err("Sand changed; try gathering again");
        }
        let changes = cells
            .iter()
            .map(|&(c, _)| (c, Material::Air))
            .collect::<Vec<_>>();
        world.set_batch(changes.iter().copied());
        self.edits.extend(changes);
        self.picked += cells.len() as u64;
        self.hand = Some(Item::Sand(cells.iter().map(|c| c.1).collect()));
        debug_assert!(self.conserved());
        Ok(())
    }
    /// Bounded vertical settling onto actual support. No planting/covering shortcut.
    pub fn settle(
        world: &World,
        start: Cell,
        occupied: &BTreeMap<Cell, Material>,
    ) -> Result<Cell, &'static str> {
        let material = |c| {
            occupied
                .get(&c)
                .copied()
                .unwrap_or_else(|| world.material(c))
        };
        let mut c = start;
        for _ in 0..16 {
            if material(c) == Material::Air {
                break;
            }
            c = up(c, 1);
        }
        if material(c) != Material::Air {
            return Err("No room to drop this here");
        }
        for _ in 0..64 {
            let below = material(up(c, -1));
            if below == Material::Water {
                return Err("Choose dry ground");
            }
            if below != Material::Air {
                return Ok(c);
            }
            c = up(c, -1);
        }
        Err("Ground is too far below")
    }
    pub fn drop_sand(
        &mut self,
        world: &mut World,
        start: Cell,
        can_place: impl Fn(Cell) -> bool,
    ) -> Result<usize, &'static str> {
        let Some(Item::Sand(cells)) = &self.hand else {
            return Err("You are not holding a handful");
        };
        let mut changes = BTreeMap::new();
        for &m in cells {
            let c = Self::settle(world, start, &changes)?;
            if !can_place(c) {
                return Err("That would place sand inside you");
            }
            changes.insert(c, m);
        }
        let n = cells.len();
        world.set_batch(changes.iter().map(|(&c, &m)| (c, m)));
        self.edits.extend(changes);
        self.hand = None;
        self.placed += n as u64;
        debug_assert!(self.conserved());
        Ok(n)
    }
    pub fn drop_seed(
        &mut self,
        world: &mut World,
        garden: &mut Garden,
        start: Cell,
        can_place: impl Fn(Cell) -> bool,
    ) -> Result<(), &'static str> {
        if self.hand != Some(Item::Seed) {
            return Err("Select the black seed from your pocket");
        }
        let c = Self::settle(world, start, &BTreeMap::new())?;
        if !can_place(c) {
            return Err("Move back a little before placing the seed");
        }
        garden.plant_at(world, c)?;
        self.hand = None;
        debug_assert!(self.validate(garden.seeds));
        Ok(())
    }
    pub fn apply(&self, world: &mut World) {
        world.set_batch(self.edits.iter().map(|(&c, &m)| (c, m)));
    }
    pub fn migrate(world: &mut World, garden: &Garden) -> Result<Self, &'static str> {
        let mut p = Self::default();
        if garden.seeds == 0 {
            p.slots[0] = None;
        }
        for h in &garden.holes {
            p.edits.insert(h.seed, Material::Air);
            p.edits
                .insert(up(h.seed, 1), if h.covered { h.cap } else { Material::Air });
        }
        let mut loose = vec![Material::Sand; garden.sand as usize];
        loose.extend(std::iter::repeat_n(Material::Soil, garden.soil as usize));
        p.legacy = loose.len() as u64;
        for cells in loose.chunks(HANDFUL) {
            let item = Some(Item::Sand(cells.to_vec()));
            if let Some(i) = p.slots.iter().position(Option::is_none) {
                p.slots[i] = item;
            } else if p.hand.is_none() {
                p.hand = item;
            } else {
                // Preserve legacy inventory overflow as visible terrain beside its old hole.
                let anchor = garden
                    .holes
                    .first()
                    .ok_or("Legacy sand has no source hole")?
                    .seed;
                for &m in cells {
                    let x = anchor.x + 8 + (p.placed % 8) as i64;
                    let z = anchor.z + 8 + (p.placed / 8) as i64;
                    let y = (world.ground_height_m(x as f64 * VOXEL_SIZE, z as f64 * VOXEL_SIZE)
                        / VOXEL_SIZE) as i64;
                    let c = Self::settle(world, Cell { x, y, z }, &p.edits)?;
                    p.edits.insert(c, m);
                    p.placed += 1;
                }
            }
        }
        if !p.validate(garden.seeds) {
            return Err("Invalid migrated pocket inventory");
        }
        p.apply(world);
        Ok(p)
    }
    pub fn snapshot(&self) -> String {
        let mut s = format!(
            "POCKETS1 {} {} {} {}\n",
            self.selected, self.picked, self.placed, self.legacy
        );
        for (i, item) in self
            .slots
            .iter()
            .chain(std::iter::once(&self.hand))
            .enumerate()
        {
            match item {
                None => {}
                Some(Item::Seed) => s += &format!("I {i} S\n"),
                Some(Item::Bowl) => s += &format!("I {i} B\n"),
                Some(Item::Sand(v)) => {
                    s += &format!(
                        "I {i} H {}\n",
                        v.iter()
                            .map(|m| (*m as u8).to_string())
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                }
            }
        }
        for (c, m) in &self.edits {
            s += &format!("E {} {} {} {}\n", c.x, c.y, c.z, *m as u8);
        }
        s
    }
    pub fn restore(text: &str, seeds: u32) -> Result<Self, &'static str> {
        if text.len() > 4_000_000 {
            return Err("Pocket save too large");
        }
        let mut lines = text.lines();
        let h = lines
            .next()
            .ok_or("Missing pockets")?
            .split_whitespace()
            .collect::<Vec<_>>();
        let num = |s: &str| s.parse::<u64>().map_err(|_| "Invalid pocket count");
        if h.len() != 5 || h[0] != "POCKETS1" {
            return Err("Invalid pocket version");
        }
        let mut p = Self {
            slots: std::array::from_fn(|_| None),
            hand: None,
            selected: usize::try_from(num(h[1])?).map_err(|_| "Invalid slot")?,
            picked: num(h[2])?,
            placed: num(h[3])?,
            legacy: num(h[4])?,
            edits: BTreeMap::new(),
        };
        for line in lines {
            let v = line.split_whitespace().collect::<Vec<_>>();
            match v.first().copied() {
                Some("I") if v.len() >= 3 => {
                    let i = usize::try_from(num(v[1])?).map_err(|_| "Invalid slot")?;
                    let slot = if i == 4 {
                        &mut p.hand
                    } else {
                        p.slots.get_mut(i).ok_or("Invalid slot")?
                    };
                    if slot.is_some() {
                        return Err("Duplicate pocket");
                    }
                    *slot = Some(match v[2] {
                        "S" if v.len() == 3 => Item::Seed,
                        "B" if v.len() == 3 => Item::Bowl,
                        "H" if (4..=3 + HANDFUL).contains(&v.len()) => Item::Sand(
                            v[3..]
                                .iter()
                                .map(|v| match *v {
                                    "8" => Ok(Material::Sand),
                                    "2" => Ok(Material::Soil),
                                    _ => Err("Invalid loose material"),
                                })
                                .collect::<Result<Vec<_>, _>>()?,
                        ),
                        _ => return Err("Invalid pocket item"),
                    });
                }
                Some("E") if v.len() == 5 => {
                    let c = Cell {
                        x: v[1].parse().map_err(|_| "Invalid cell")?,
                        y: v[2].parse().map_err(|_| "Invalid cell")?,
                        z: v[3].parse().map_err(|_| "Invalid cell")?,
                    };
                    if [c.x, c.y, c.z]
                        .iter()
                        .any(|v| v.unsigned_abs() > 1u64 << 40)
                    {
                        return Err("Pocket edit outside range");
                    }
                    let m = match v[4] {
                        "0" => Material::Air,
                        "8" => Material::Sand,
                        "2" => Material::Soil,
                        _ => return Err("Invalid pocket edit"),
                    };
                    if p.edits.insert(c, m).is_some() {
                        return Err("Duplicate pocket edit");
                    }
                }
                _ => return Err("Invalid pocket record"),
            }
        }
        if !p.validate(seeds) {
            return Err("Pocket inventory conservation failed");
        }
        Ok(p)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{desert, FbmParams, FbmRecipe};
    fn source() -> (World, Cell) {
        let w = World::fbm(
            19,
            FbmParams {
                recipe: FbmRecipe::Desert,
                ..Default::default()
            },
        )
        .unwrap();
        let o = desert::oasis(19, 0, 0);
        let c = Cell::from_meters([o.x + 60., 0., o.z]);
        let y = (w.ground_height_m(c.x as f64 * VOXEL_SIZE, c.z as f64 * VOXEL_SIZE) / VOXEL_SIZE)
            as i64
            - 1;
        (w, Cell { y, ..c })
    }
    #[test]
    fn failed_stow_or_partial_placement_does_not_lose_the_handful() {
        let (mut w, c) = source();
        let mut p = Pockets::default();
        let cells = Pockets::selection(&w, c);
        p.pickup(&mut w, &cells).unwrap();
        let before = p.snapshot();
        let revision = w.revision();
        let count = std::cell::Cell::new(0);
        assert!(p
            .drop_sand(&mut w, Cell { x: c.x + 5, ..c }, |_| {
                count.set(count.get() + 1);
                count.get() < 3
            })
            .is_err());
        assert_eq!(p.snapshot(), before);
        assert_eq!(w.revision(), revision);
        p.slots[2] = Some(Item::Sand(vec![Material::Sand; 4]));
        p.slots[3] = Some(Item::Sand(vec![Material::Sand; 4]));
        p.legacy = 8;
        assert!(p.validate(1));
        let before = p.snapshot();
        assert!(p.stow().is_err());
        assert_eq!(p.snapshot(), before);
        p.picked = u64::MAX;
        p.legacy = 1;
        p.placed = u64::MAX;
        assert!(!p.conserved());
        assert!(Pockets::restore(&p.snapshot(), 1).is_err());
    }
    #[test]
    fn legacy_overflow_becomes_visible_sand_without_losing_material() {
        let (mut w, c) = source();
        let mut g = Garden::default();
        g.dig(&mut w, c).unwrap();
        g.sand = 30;
        g.soil = 5;
        let p = Pockets::migrate(&mut w, &g).unwrap();
        assert_eq!(p.legacy, 35);
        assert_eq!(p.carried() as u64 + p.placed, 35);
        assert!(p.placed > 0);
        assert!(p.validate(1));
        assert!(Pockets::restore(&p.snapshot(), 1).is_ok());
    }
    #[test]
    fn pockets_swap_stow_and_keep_one_bowl_and_seed() {
        let mut p = Pockets::default();
        p.select(1).unwrap();
        assert!(p.bowl());
        p.select(0).unwrap();
        assert_eq!(p.hand, Some(Item::Seed));
        p.stow().unwrap();
        assert!(p.validate(1));
        let r = Pockets::restore(&p.snapshot(), 1).unwrap();
        assert_eq!(r.snapshot(), p.snapshot());
        assert!(Pockets::restore(&p.snapshot().replace("I 0 B", "I 0 S"), 1).is_err());
    }
    #[test]
    fn charged_sized_pickup_and_drop_preserve_loose_voxels() {
        let (mut w, c) = source();
        let mut p = Pockets::default();
        let cells = Pockets::selection(&w, c);
        assert_eq!(cells.len(), 4);
        p.pickup(&mut w, &cells[..1]).unwrap();
        assert_eq!(p.carried(), 1);
        assert!(p.pickup(&mut w, &cells).is_err());
        p.drop_sand(&mut w, c, |_| true).unwrap();
        let cells = Pockets::selection(&w, c);
        p.pickup(&mut w, &cells).unwrap();
        assert_eq!(p.carried(), 4);
        p.stow().unwrap();
        assert!(p.validate(1));
        p.select(2).unwrap();
        assert_eq!(p.carried(), 4);
        assert_eq!(
            p.drop_sand(&mut w, Cell { x: c.x + 4, ..c }, |_| true),
            Ok(4)
        );
        assert!(p.conserved());
    }
    #[test]
    fn real_dropped_sand_buries_black_seed_and_save_restores_terrain() {
        let (mut w, c) = source();
        let mut p = Pockets::default();
        let mut g = Garden::default();
        let selection = Pockets::selection(&w, c);
        p.pickup(&mut w, &selection).unwrap();
        p.drop_sand(
            &mut w,
            Cell {
                x: c.x + 5,
                y: c.y + 1,
                ..c
            },
            |_| true,
        )
        .unwrap();
        p.select(0).unwrap();
        p.drop_seed(&mut w, &mut g, c, |_| true).unwrap();
        let seed = g.holes[0].seed;
        assert_eq!(w.material(seed), Material::Seed);
        assert!(g.plants[0].emergence >= 3);
        let pile = *p
            .edits
            .iter()
            .filter(|(point, m)| point.x == c.x + 5 && **m != Material::Air)
            .max_by_key(|(point, _)| point.y)
            .unwrap()
            .0;
        let cells = Pockets::selection(&w, pile);
        assert_eq!(cells.len(), 4);
        p.pickup(&mut w, &cells).unwrap();
        p.drop_sand(&mut w, up(seed, 1), |_| true).unwrap();
        assert_eq!(w.material(up(seed, 1)), Material::Sand);
        let mut water = crate::bowl::Bowl::default();
        water.moisture.insert(c, 12);
        for i in 0..12 {
            water.removed.insert(Cell {
                x: c.x + 100 + i,
                ..c
            });
        }
        water.scooped = 12;
        for _ in 0..100 {
            g.advance(&mut w, &mut water, 0.25, [0., 1., 0.]);
        }
        assert_eq!(g.plants[0].germination, 4);
        assert!(g.cells() > 10);
        assert!(water.conserved());
        let r = Pockets::restore(&p.snapshot(), g.seeds).unwrap();
        let (mut fresh, _) = source();
        r.apply(&mut fresh);
        g.apply_tissue(&mut fresh);
        assert_eq!(fresh.material(seed), Material::Seed);
        assert_eq!(fresh.material(up(seed, 1)), Material::Wood);
        assert!(r.conserved());
    }
}
