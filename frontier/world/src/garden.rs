//! Seed-to-seedling prototype: local whole-unit water uptake and a deterministic
//! branching grammar. Native tissue is authoritative; display LOD never grows it.
use crate::{bowl::Bowl, Cell, Material, World, VOXEL_SIZE};
use std::collections::{BTreeMap, BTreeSet};
const LIMIT: usize = 32;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tissue {
    Seed = 0,
    Root = 1,
    Stem = 2,
    Leaf = 3,
}
impl Tissue {
    pub fn material(self) -> Material {
        if self == Self::Seed {
            Material::Seed
        } else if self == Self::Leaf {
            Material::Leaf
        } else {
            Material::Wood
        }
    }
    fn cost(self) -> f64 {
        match self {
            Self::Leaf => 0.65,
            Self::Root => 0.7,
            _ => 0.85,
        }
    }
    fn maintenance(self) -> f64 {
        match self {
            Self::Leaf => 0.008,
            Self::Root => 0.004,
            Self::Stem => 0.001,
            Self::Seed => 0.,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Hole {
    pub seed: Cell,
    pub cap: Material,
    pub base: Material,
    pub covered: bool,
}
#[derive(Clone, Copy, Debug)]
struct Step {
    cell: Cell,
    parent: Cell,
    tissue: Tissue,
}
#[derive(Clone, Debug)]
pub struct Plant {
    pub hole: usize,
    pub age: u64,
    pub germination: u8,
    pub energy: f64,
    pub water: u32,
    pub hydration: u8,
    pub cursor: usize,
    pub transpired: u64,
    pub uptake: u64,
    pub lit_leaves: u32,
    pub tissue: BTreeMap<Cell, Tissue>,
    pub emergence: u8,
    program: Vec<Step>,
}
#[derive(Clone, Debug)]
pub struct Garden {
    pub seeds: u32,
    pub sand: u32,
    pub soil: u32,
    pub holes: Vec<Hole>,
    pub plants: Vec<Plant>,
    pub tick: u64,
    phase: f64,
}
impl Default for Garden {
    fn default() -> Self {
        Self {
            seeds: 1,
            sand: 0,
            soil: 0,
            holes: Vec::new(),
            plants: Vec::new(),
            tick: 0,
            phase: 0.,
        }
    }
}
fn offset(c: Cell, d: [i64; 3]) -> Cell {
    Cell {
        x: c.x + d[0],
        y: c.y + d[1],
        z: c.z + d[2],
    }
}
/// Rewriting rule: extend the main tip, grow a short rotating lateral tip,
/// attach a three-leaf fan, repeat. All offsets/parents are native neighbors.
fn program(seed: u64, anchor: Cell, emergence: u8) -> Vec<Step> {
    let mut result = Vec::new();
    let mut seen = BTreeSet::from([anchor]);
    let mut push = |d: [i64; 3], parent: [i64; 3], tissue| {
        let cell = offset(anchor, d);
        if seen.insert(cell) {
            result.push(Step {
                cell,
                parent: offset(anchor, parent),
                tissue,
            });
        }
    };
    push([0, -1, 0], [0, 0, 0], Tissue::Root);
    push([-1, -1, 0], [0, -1, 0], Tissue::Root);
    push([1, -1, 0], [0, -1, 0], Tissue::Root);
    let emergence = i64::from(emergence);
    for y in 1..=emergence {
        push([0, y, 0], [0, y - 1, 0], Tissue::Stem);
    }
    push([1, emergence, 0], [0, emergence, 0], Tissue::Leaf);
    push([-1, emergence, 0], [0, emergence, 0], Tissue::Leaf);
    let rotation = (seed ^ (anchor.x as u64).wrapping_mul(31) ^ anchor.z as u64) % 4;
    for segment in 0..6i64 {
        let y = emergence + 1 + segment;
        push([0, y, 0], [0, y - 1, 0], Tissue::Stem);
        let dir = match (segment + rotation as i64) % 4 {
            0 => [1, 0],
            1 => [0, 1],
            2 => [-1, 0],
            _ => [0, -1],
        };
        let length = 1 + segment / 2;
        for i in 1..=length {
            push(
                [dir[0] * i, y, dir[1] * i],
                [dir[0] * (i - 1), y, dir[1] * (i - 1)],
                Tissue::Stem,
            );
        }
        let tip = [dir[0] * length, y, dir[1] * length];
        for d in [[-dir[1], 0, dir[0]], [dir[1], 0, -dir[0]], [0, 1, 0]] {
            push(
                [tip[0] + d[0], tip[1] + d[1], tip[2] + d[2]],
                tip,
                Tissue::Leaf,
            );
        }
    }
    result
}
impl Garden {
    pub fn dig(&mut self, world: &mut World, hit: Cell) -> Result<(), &'static str> {
        if self.holes.len() >= LIMIT {
            return Err("Planting patch limit reached");
        }
        let seed = offset(hit, [0, -1, 0]);
        let top = (world.ground_height_m(hit.x as f64 * VOXEL_SIZE, hit.z as f64 * VOXEL_SIZE)
            / VOXEL_SIZE) as i64
            - 1;
        if hit.y != top
            || self
                .holes
                .iter()
                .any(|h| h.seed.x == hit.x && h.seed.z == hit.z)
        {
            return Err("Aim at untouched sand to dig a little hole");
        }
        let cap = world.material(hit);
        let base = world.material(seed);
        if ![cap, base]
            .into_iter()
            .all(|m| matches!(m, Material::Sand | Material::Soil))
            || world.material(offset(hit, [0, 1, 0])) != Material::Air
        {
            return Err("Choose dry sand above the waterline");
        }
        for m in [cap, base] {
            if m == Material::Sand {
                self.sand += 1
            } else {
                self.soil += 1
            }
        }
        self.holes.push(Hole {
            seed,
            cap,
            base,
            covered: false,
        });
        world.set_batch([(hit, Material::Air), (seed, Material::Air)]);
        Ok(())
    }
    pub fn plant(&mut self, world: &mut World, index: usize) -> Result<(), &'static str> {
        if self.seeds == 0 {
            return Err("Your single seed is already planted");
        }
        let hole = self.holes.get(index).ok_or("Aim at your planting hole")?;
        if self.plants.iter().any(|p| p.hole == index)
            || hole.covered
            || world.material(hole.seed) != Material::Air
            || world.material(offset(hole.seed, [0, 1, 0])) != Material::Air
        {
            return Err("The seed needs an open planting hole");
        }
        let anchor = hole.seed;
        world.set(anchor, Material::Seed);
        self.seeds -= 1;
        self.plants.push(Plant {
            hole: index,
            age: 0,
            germination: 0,
            energy: 10.,
            water: 0,
            hydration: 0,
            cursor: 0,
            transpired: 0,
            uptake: 0,
            lit_leaves: 0,
            tissue: BTreeMap::from([(anchor, Tissue::Seed)]),
            emergence: 2,
            program: program(world.seed(), anchor, 2),
        });
        Ok(())
    }
    /// A loose seed settles onto dry soil; burial is detected from real cover.
    pub fn plant_at(&mut self, world: &mut World, anchor: Cell) -> Result<(), &'static str> {
        if self.seeds != 1 {
            return Err("Your seed is already planted");
        }
        if self.holes.len() >= LIMIT
            || world.material(anchor) != Material::Air
            || world.material(offset(anchor, [0, 1, 0])) != Material::Air
            || !matches!(
                world.material(offset(anchor, [0, -1, 0])),
                Material::Sand | Material::Soil
            )
        {
            return Err("The seed needs dry soil and open space");
        }
        let soil = world.material(offset(anchor, [0, -1, 0]));
        self.holes.push(Hole {
            seed: anchor,
            cap: soil,
            base: soil,
            covered: false,
        });
        let i = self.holes.len() - 1;
        self.plant(world, i)?;
        let ground = (world
            .ground_height_m(anchor.x as f64 * VOXEL_SIZE, anchor.z as f64 * VOXEL_SIZE)
            / VOXEL_SIZE) as i64;
        let emergence = (ground - anchor.y).clamp(1, 6) as u8;
        let p = self.plants.last_mut().unwrap();
        p.emergence = emergence;
        p.program = program(world.seed(), anchor, emergence);
        Ok(())
    }
    pub fn apply_tissue(&self, world: &mut World) {
        world.set_batch(
            self.plants
                .iter()
                .flat_map(|p| p.tissue.iter().map(|(&c, &t)| (c, t.material()))),
        );
    }
    pub fn cover(&mut self, world: &mut World, index: usize) -> Result<(), &'static str> {
        let h = self
            .holes
            .get_mut(index)
            .ok_or("Aim at your planting hole")?;
        if h.covered {
            return Err("This hole is already covered");
        }
        let c = offset(h.seed, [0, 1, 0]);
        if world.material(c) != Material::Air {
            return Err("The hole is obstructed");
        }
        let stock = if h.cap == Material::Sand {
            &mut self.sand
        } else {
            &mut self.soil
        };
        if *stock == 0 {
            return Err("No loose sand to cover the seed");
        }
        *stock -= 1;
        h.covered = true;
        world.set(c, h.cap);
        Ok(())
    }
    pub fn nearest_hole(&self, target: Cell) -> Option<usize> {
        self.holes
            .iter()
            .enumerate()
            .filter(|(_, h)| {
                (h.seed.x - target.x).abs() <= 3
                    && (h.seed.z - target.z).abs() <= 3
                    && (h.seed.y - target.y).abs() <= 4
            })
            .min_by_key(|(_, h)| (h.seed.x - target.x).pow(2) + (h.seed.z - target.z).pow(2))
            .map(|(i, _)| i)
    }
    fn uptake(p: &mut Plant, anchor: Cell, bowl: &mut Bowl) {
        if p.water >= 2 {
            return;
        }
        let cell = bowl
            .moisture
            .iter()
            .find(|(c, n)| {
                **n > 0
                    && (c.x - anchor.x).abs() <= 6
                    && (c.z - anchor.z).abs() <= 6
                    && (c.y - anchor.y).abs() <= 6
            })
            .map(|(c, _)| *c);
        if let Some(c) = cell {
            let n = bowl.moisture.get_mut(&c).unwrap();
            *n -= 1;
            if *n == 0 {
                bowl.moisture.remove(&c);
            }
            bowl.plant_transferred += 1;
            p.water += 1;
            p.uptake += 1;
        }
    }
    pub fn advance(&mut self, world: &mut World, bowl: &mut Bowl, dt: f64, sun: [f64; 3]) {
        if !dt.is_finite() || dt < 0. {
            return;
        }
        self.phase += dt.min(0.25);
        if self.phase < 1. {
            return;
        }
        self.phase -= 1.;
        self.tick += 1;
        let length = sun.iter().map(|v| v * v).sum::<f64>().sqrt();
        let sun = sun.map(|v| {
            if length > 0. && v.is_finite() {
                v / length
            } else {
                0.
            }
        });
        for p in &mut self.plants {
            p.age += 1;
            let h = &mut self.holes[p.hole];
            let cap = offset(h.seed, [0, 1, 0]);
            if p.germination < 4 {
                h.covered = matches!(world.material(cap), Material::Sand | Material::Soil);
                if !h.covered || !matches!(world.material(cap), Material::Sand | Material::Soil) {
                    p.germination = 0;
                    continue;
                }
                Self::uptake(p, h.seed, bowl);
                if p.water == 0 {
                    p.germination = 0;
                    continue;
                }
                p.germination += 1;
                if p.germination == 4 {
                    p.energy -= 1.2;
                }
                continue;
            }
            Self::uptake(p, h.seed, bowl);
            if p.age % 20 == 0 {
                if p.water > 0 {
                    p.water -= 1;
                    p.transpired += 1;
                } else {
                    p.hydration = 0;
                }
            }
            p.lit_leaves = 0;
            if sun[1] > 0. && (p.water > 0 || p.hydration > 0) {
                for (&c, &t) in &p.tissue {
                    if t != Tissue::Leaf {
                        continue;
                    }
                    let center = c.center_m();
                    let exit = VOXEL_SIZE * 0.5001 / sun.iter().map(|v| v.abs()).fold(0., f64::max);
                    let origin = std::array::from_fn(|a| center[a] + sun[a] * exit);
                    if crate::raycast(world, origin, sun, 2.).is_some() {
                        continue;
                    }
                    let blocked = [8., 32., 128.].into_iter().any(|distance| {
                        let x = center[0] + sun[0] * distance;
                        let z = center[2] + sun[2] * distance;
                        world.height_m(x, z) > center[1] + sun[1] * distance
                    });
                    if !blocked {
                        p.lit_leaves += 1;
                    }
                }
                p.energy = (p.energy + p.lit_leaves as f64 * 0.55 * sun[1]).min(30.);
            }
            p.energy = (p.energy - p.tissue.values().map(|t| t.maintenance()).sum::<f64>()).max(0.);
            if p.cursor >= p.program.len() {
                continue;
            }
            let next = p.program[p.cursor];
            if !p.tissue.contains_key(&next.parent) {
                p.cursor += 1;
                continue;
            }
            let m = world.material(next.cell);
            let allowed = m == Material::Air
                || ((next.tissue == Tissue::Root
                    || (next.tissue == Tissue::Stem
                        && next.cell.y <= h.seed.y + i64::from(p.emergence)))
                    && matches!(m, Material::Sand | Material::Soil));
            if !allowed {
                p.cursor += 1;
                continue;
            }
            if p.energy < next.tissue.cost() || (p.hydration == 0 && p.water == 0) {
                continue;
            }
            if p.hydration == 0 {
                p.water -= 1;
                p.transpired += 1;
                p.hydration = 4;
            }
            p.energy -= next.tissue.cost();
            p.hydration -= 1;
            p.cursor += 1;
            p.tissue.insert(next.cell, next.tissue);
            world.set(next.cell, next.tissue.material());
        }
        debug_assert!(bowl.conserved());
        debug_assert!(self.water_accounted() == bowl.plant_transferred);
    }
    pub fn water_accounted(&self) -> u64 {
        self.plants
            .iter()
            .map(|p| u64::from(p.water) + p.transpired)
            .sum()
    }
    pub fn cells(&self) -> usize {
        self.plants.iter().map(|p| p.tissue.len()).sum()
    }
    pub fn apply(&self, world: &mut World) {
        let mut edits = Vec::new();
        for h in &self.holes {
            edits.push((h.seed, Material::Air));
            edits.push((
                offset(h.seed, [0, 1, 0]),
                if h.covered { h.cap } else { Material::Air },
            ));
        }
        for p in &self.plants {
            edits.extend(p.tissue.iter().map(|(&c, &t)| (c, t.material())));
        }
        world.set_batch(edits);
    }
    pub fn snapshot(&self) -> String {
        let mut s = format!(
            "GARDEN2 {} {} {} {} {}\n",
            self.seeds, self.sand, self.soil, self.tick, self.phase
        );
        for h in &self.holes {
            s += &format!(
                "H {} {} {} {} {} {}\n",
                h.seed.x,
                h.seed.y,
                h.seed.z,
                h.cap as u8,
                h.base as u8,
                u8::from(h.covered)
            );
        }
        for (i, p) in self.plants.iter().enumerate() {
            s += &format!(
                "P {} {} {} {} {} {} {} {} {} {}\n",
                p.hole,
                p.age,
                p.germination,
                p.energy,
                p.water,
                p.hydration,
                p.cursor,
                p.transpired,
                p.uptake,
                p.emergence
            );
            for (c, t) in &p.tissue {
                s += &format!("C {i} {} {} {} {}\n", c.x, c.y, c.z, *t as u8);
            }
        }
        s
    }
    pub fn restore(s: &str, seed: u64) -> Result<Self, &'static str> {
        if s.len() > 2_000_000 {
            return Err("garden save too large");
        }
        let mut lines = s.lines();
        let h: Vec<_> = lines
            .next()
            .ok_or("missing garden")?
            .split_whitespace()
            .collect();
        let num = |v: &str| v.parse::<u64>().map_err(|_| "invalid garden count");
        let real = |v: &str| {
            v.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or("invalid garden number")
        };
        if h.len() != 6 || !matches!(h[0], "GARDEN1" | "GARDEN2") {
            return Err("invalid garden version");
        }
        let mut g = Self {
            seeds: u32::try_from(num(h[1])?).map_err(|_| "overflow")?,
            sand: u32::try_from(num(h[2])?).map_err(|_| "overflow")?,
            soil: u32::try_from(num(h[3])?).map_err(|_| "overflow")?,
            tick: num(h[4])?,
            phase: real(h[5])?,
            ..Default::default()
        };
        if g.tick > (1u64 << 53)
            || g.seeds > 1
            || !(0. ..1.).contains(&g.phase)
            || g.sand > 64
            || g.soil > 64
        {
            return Err("invalid garden header");
        }
        for line in lines {
            let v: Vec<_> = line.split_whitespace().collect();
            match v.first().copied() {
                Some("H") if v.len() == 7 => {
                    if g.holes.len() >= LIMIT {
                        return Err("too many holes");
                    }
                    let c = Cell {
                        x: v[1].parse().map_err(|_| "invalid cell")?,
                        y: v[2].parse().map_err(|_| "invalid cell")?,
                        z: v[3].parse().map_err(|_| "invalid cell")?,
                    };
                    if [c.x, c.y, c.z]
                        .into_iter()
                        .any(|v| v.unsigned_abs() > 1u64 << 40)
                        || g.holes.iter().any(|h| h.seed.x == c.x && h.seed.z == c.z)
                    {
                        return Err("duplicate or distant hole");
                    }
                    let mat = |v| match v {
                        "8" => Ok(Material::Sand),
                        "2" => Ok(Material::Soil),
                        _ => Err("invalid planting soil"),
                    };
                    let covered = match v[6] {
                        "0" => false,
                        "1" => true,
                        _ => return Err("invalid cover"),
                    };
                    g.holes.push(Hole {
                        seed: c,
                        cap: mat(v[4])?,
                        base: mat(v[5])?,
                        covered,
                    });
                }
                Some("P")
                    if (h[0] == "GARDEN1" && v.len() == 10)
                        || (h[0] == "GARDEN2" && v.len() == 11) =>
                {
                    let i = usize::try_from(num(v[1])?).map_err(|_| "invalid hole")?;
                    let hole = g.holes.get(i).ok_or("missing hole")?;
                    if g.plants.len() >= LIMIT || g.plants.iter().any(|p| p.hole == i) {
                        return Err("duplicate plant");
                    }
                    let p = Plant {
                        hole: i,
                        age: num(v[2])?,
                        germination: u8::try_from(num(v[3])?).map_err(|_| "invalid germination")?,
                        energy: real(v[4])?,
                        water: u32::try_from(num(v[5])?).map_err(|_| "invalid water")?,
                        hydration: u8::try_from(num(v[6])?).map_err(|_| "invalid hydration")?,
                        cursor: usize::try_from(num(v[7])?).map_err(|_| "invalid cursor")?,
                        transpired: num(v[8])?,
                        uptake: num(v[9])?,
                        lit_leaves: 0,
                        tissue: BTreeMap::new(),
                        emergence: if v.len() == 11 {
                            u8::try_from(num(v[10])?).map_err(|_| "bad emergence")?
                        } else {
                            2
                        },
                        program: program(
                            seed,
                            hole.seed,
                            if v.len() == 11 {
                                u8::try_from(num(v[10])?).map_err(|_| "bad emergence")?
                            } else {
                                2
                            },
                        ),
                    };
                    if !(1..=6).contains(&p.emergence)
                        || p.age > g.tick
                        || p.germination > 4
                        || !(0. ..=30.).contains(&p.energy)
                        || p.water > 2
                        || p.hydration > 4
                        || p.cursor > p.program.len()
                        || p.transpired.checked_add(u64::from(p.water)) != Some(p.uptake)
                    {
                        return Err("invalid plant state");
                    }
                    g.plants.push(p);
                }
                Some("C") if v.len() == 6 => {
                    let i = usize::try_from(num(v[1])?).map_err(|_| "invalid plant")?;
                    let p = g.plants.get_mut(i).ok_or("missing plant")?;
                    let c = Cell {
                        x: v[2].parse().map_err(|_| "invalid cell")?,
                        y: v[3].parse().map_err(|_| "invalid cell")?,
                        z: v[4].parse().map_err(|_| "invalid cell")?,
                    };
                    let t = match v[5] {
                        "0" => Tissue::Seed,
                        "1" => Tissue::Root,
                        "2" => Tissue::Stem,
                        "3" => Tissue::Leaf,
                        _ => return Err("invalid tissue"),
                    };
                    let valid = if t == Tissue::Seed {
                        c == g.holes[p.hole].seed
                    } else {
                        p.program[..p.cursor]
                            .iter()
                            .any(|n| n.cell == c && n.tissue == t)
                    };
                    if !valid || p.tissue.insert(c, t).is_some() {
                        return Err("invalid tissue placement");
                    }
                }
                _ => return Err("invalid garden record"),
            }
        }
        if g.seeds as usize + g.plants.len() != 1 {
            return Err("seed conservation failed");
        }
        let mut occupied = BTreeSet::new();
        for p in &g.plants {
            let anchor = g.holes[p.hole].seed;
            if p.tissue.get(&anchor) != Some(&Tissue::Seed) {
                return Err("missing seed");
            }
            for (&c, &t) in &p.tissue {
                if !occupied.insert(c) {
                    return Err("overlapping tissue");
                }
                if t != Tissue::Seed {
                    let step = p
                        .program
                        .iter()
                        .find(|n| n.cell == c)
                        .ok_or("unknown tissue")?;
                    if !p.tissue.contains_key(&step.parent) {
                        return Err("unsupported tissue");
                    }
                }
            }
        }
        Ok(g)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{desert, FbmParams, FbmRecipe};
    fn scene() -> (World, Garden, Bowl, Cell) {
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
        let top = Cell {
            y: (w.ground_height_m(c.x as f64 * VOXEL_SIZE, c.z as f64 * VOXEL_SIZE) / VOXEL_SIZE)
                as i64
                - 1,
            ..c
        };
        (w, Garden::default(), Bowl::default(), top)
    }
    fn supply(b: &mut Bowl, c: Cell, n: u32) {
        b.moisture.insert(c, n);
        for i in 0..n {
            b.removed.insert(offset(c, [100 + i as i64, 0, 0]));
        }
        b.scooped = b.removed.len() as u64;
    }
    #[test]
    fn shade_and_finite_water_constrain_growth_without_creating_resources() {
        let (mut w, mut g, mut b, top) = scene();
        g.dig(&mut w, top).unwrap();
        g.plant(&mut w, 0).unwrap();
        g.cover(&mut w, 0).unwrap();
        supply(&mut b, top, 16);
        let (mut dark_world, mut dark, mut dark_water) = (w.clone(), g.clone(), b.clone());
        for _ in 0..160 {
            g.advance(&mut w, &mut b, 0.25, [0., 1., 0.]);
            dark.advance(&mut dark_world, &mut dark_water, 0.25, [0., -1., 0.]);
        }
        assert!(g.cells() > dark.cells());
        assert!(dark.plants[0].energy < 0.85);
        assert!(b.conserved() && dark_water.conserved());
        let (mut w, mut g, mut b, top) = scene();
        g.dig(&mut w, top).unwrap();
        g.plant(&mut w, 0).unwrap();
        g.cover(&mut w, 0).unwrap();
        supply(&mut b, top, 1);
        for _ in 0..120 {
            g.advance(&mut w, &mut b, 0.25, [0., 1., 0.]);
        }
        let cells = g.cells();
        for _ in 0..120 {
            g.advance(&mut w, &mut b, 0.25, [0., 1., 0.]);
        }
        assert_eq!(g.cells(), cells);
        assert_eq!(g.water_accounted(), 1);
        assert!(b.conserved());
    }
    #[test]
    fn unburied_or_dry_seed_waits_and_single_seed_is_conserved() {
        let (mut w, mut g, mut b, top) = scene();
        g.dig(&mut w, top).unwrap();
        g.plant(&mut w, 0).unwrap();
        assert!(g.plant(&mut w, 0).is_err());
        for _ in 0..24 {
            g.advance(&mut w, &mut b, 0.25, [0., 1., 0.]);
        }
        assert_eq!(g.cells(), 1);
        g.cover(&mut w, 0).unwrap();
        for _ in 0..40 {
            g.advance(&mut w, &mut b, 0.25, [0., 1., 0.]);
        }
        assert_eq!(g.cells(), 1);
        assert_eq!(g.seeds, 0);
        assert!(b.conserved());
    }
    #[test]
    fn watered_seed_grows_connected_tissue_and_restart_is_exact() {
        let (mut w, mut g, mut b, top) = scene();
        g.dig(&mut w, top).unwrap();
        g.plant(&mut w, 0).unwrap();
        g.cover(&mut w, 0).unwrap();
        supply(&mut b, top, 20);
        for _ in 0..240 {
            g.advance(&mut w, &mut b, 0.25, [0., 1., 0.]);
        }
        assert!(g.cells() > 15);
        assert!(g.plants[0].lit_leaves > 0);
        assert!(g.plants[0].energy >= 0.);
        assert_eq!(g.water_accounted(), b.plant_transferred);
        assert!(b.conserved());
        let s = g.snapshot();
        let mut other = Garden::restore(&s, 19).unwrap();
        assert_eq!(other.snapshot(), s);
        let mut w2 = w.clone();
        let mut b2 = b.clone();
        for _ in 0..40 {
            g.advance(&mut w, &mut b, 0.25, [0., 1., 0.]);
            other.advance(&mut w2, &mut b2, 0.25, [0., 1., 0.]);
        }
        assert_eq!(other.snapshot(), g.snapshot());
        assert_eq!(b.snapshot(19), b2.snapshot(19));
        for p in &g.plants {
            for (c, t) in &p.tissue {
                assert_eq!(w.material(*c), t.material());
            }
        }
    }
    #[test]
    fn support_and_obstacle_rules_never_overwrite_stone() {
        let (mut w, mut g, mut b, top) = scene();
        g.dig(&mut w, top).unwrap();
        g.plant(&mut w, 0).unwrap();
        g.cover(&mut w, 0).unwrap();
        let blocked = offset(g.holes[0].seed, [0, 3, 0]);
        w.set(blocked, Material::Stone);
        supply(&mut b, top, 10);
        for _ in 0..180 {
            g.advance(&mut w, &mut b, 0.25, [0., 1., 0.]);
        }
        assert_eq!(w.material(blocked), Material::Stone);
        assert!(!g.plants[0].tissue.contains_key(&offset(blocked, [0, 1, 0])));
        assert!(Garden::restore(&g.snapshot(), 19).is_ok());
    }
    #[test]
    fn old_bowl_saves_migrate_without_losing_water() {
        let b = Bowl::default();
        let s = b.snapshot(19);
        let fields: Vec<_> = s.trim().split_whitespace().collect();
        let legacy = format!("BOWL1 {}\n", fields[1..8].join(" "));
        let r = Bowl::restore(&legacy, 19).unwrap();
        assert!(r.conserved());
        assert_eq!(r.plant_transferred, 0);
    }
}
