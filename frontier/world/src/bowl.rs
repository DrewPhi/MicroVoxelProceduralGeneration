//! Finite bowl and local whole-cell droplets. No fluid source is manufactured.
//! Sand absorption stores water units separately from the visual damp footprint.
use crate::{raycast, Cell, Material, World, VOXEL_SIZE};
use std::collections::{BTreeMap, BTreeSet};
pub const CAPACITY: u32 = 8; // 1.953125 litres at the native resolution.
#[derive(Clone, Debug)]
pub struct Drop {
    pub cell: Cell,
    pub velocity: [f64; 3],
    credit: [f64; 3],
}
#[derive(Clone, Default, Debug)]
pub struct Bowl {
    pub units: u32,
    pub scooped: u64,
    pub emitted: u64,
    pub spilled: u64,
    pub moisture: BTreeMap<Cell, u32>,
    pub removed: BTreeSet<Cell>,
    pub deposited: BTreeSet<Cell>,
    pub damp: BTreeSet<Cell>,
    pub drops: Vec<Drop>,
    phase: f64,
    pub simulated: f64,
}
impl Bowl {
    pub fn conserved(&self) -> bool {
        self.scooped
            == u64::from(self.units)
                + self.drops.len() as u64
                + self.moisture.values().map(|v| u64::from(*v)).sum::<u64>()
                + self.deposited.len() as u64
    }
    pub fn scoop(
        &mut self,
        world: &mut World,
        eye: [f64; 3],
        dir: [f64; 3],
    ) -> Result<u32, &'static str> {
        let hit = raycast(world, eye, dir, 2.2).ok_or("Move closer and aim at pond water")?;
        if hit.material != Material::Water {
            return Err("Aim at water within 2.2 m");
        }
        if self.units == CAPACITY {
            return Err("Bowl is full");
        }
        let mut picked = Vec::new();
        // Take visible neighbors first, then the layer beneath. Every unit is real.
        for y in [0, -1, -2] {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let c = Cell {
                        x: hit.cell.x + dx,
                        y: hit.cell.y + y,
                        z: hit.cell.z + dz,
                    };
                    if picked.len() < (CAPACITY - self.units) as usize
                        && world.material(c) == Material::Water
                    {
                        picked.push(c);
                    }
                }
            }
        }
        for &c in &picked {
            if !self.deposited.remove(&c) {
                self.removed.insert(c);
            }
        }
        world.set_fluid_occupancy(picked.iter().map(|&c| (c, false)));
        // A previously deposited unit can be picked up again without new source mass.
        let recycled = picked.len() as u32;
        self.units += recycled;
        self.scooped = self.removed.len() as u64;
        debug_assert!(self.conserved());
        Ok(recycled)
    }
    pub fn emit(
        &mut self,
        world: &World,
        eye: [f64; 3],
        dir: [f64; 3],
        dt: f64,
        spill: bool,
        enabled: bool,
    ) {
        if !enabled || self.units == 0 {
            self.phase = 0.;
            return;
        }
        self.phase += dt.clamp(0., 0.1) * if spill { 6. } else { 10. };
        while self.phase >= 1. && self.units > 0 {
            self.phase -= 1.;
            // Camera-relative lip: right/up are normalized so looking down
            // does not move the stream back through the player's body.
            let length = dir[0].hypot(dir[2]);
            let right = if length > 1e-8 {
                [-dir[2] / length, 0., dir[0] / length]
            } else {
                [0.; 3]
            };
            let up = [
                -right[2] * dir[1],
                right[2] * dir[0] - right[0] * dir[2],
                right[0] * dir[1],
            ];
            let c = Cell::from_meters(std::array::from_fn(|a| {
                eye[a] + dir[a] * 0.45 + right[a] * 0.27 - up[a] * 0.14
            }));
            if world.material(c) != Material::Air || self.drops.iter().any(|d| d.cell == c) {
                continue;
            }
            self.units -= 1;
            self.emitted += 1;
            if spill {
                self.spilled += 1;
            }
            self.drops.push(Drop {
                cell: c,
                velocity: [dir[0] * 1.2, -0.7, dir[2] * 1.2],
                credit: [0.; 3],
            });
        }
    }
    fn absorb(&mut self, world: &mut World, at: Cell) {
        *self.moisture.entry(at).or_default() += 1;
        // Wet color is a temporary Soil-palette override, NOT fertility. Moisture is
        // authoritative above; no organic matter/nutrients are created by watering.
        let units = self.moisture[&at];
        let r = (1 + (units as f64).sqrt() as i64).min(4);
        let mut changes = Vec::new();
        for z in -r..=r {
            for x in -r..=r {
                if x * x + z * z > r * r {
                    continue;
                }
                let h = (world.ground_height_m(
                    (at.x + x) as f64 * VOXEL_SIZE,
                    (at.z + z) as f64 * VOXEL_SIZE,
                ) / VOXEL_SIZE) as i64;
                let c = Cell {
                    x: at.x + x,
                    y: h - 1,
                    z: at.z + z,
                };
                if world.material(c) == Material::Sand && (c.y - at.y).abs() <= 4 {
                    self.damp.insert(c);
                    changes.push(c);
                }
            }
        }
        world.set_batch(changes.into_iter().map(|c| (c, Material::Soil)));
    }
    pub fn advance(&mut self, world: &mut World, dt: f64) {
        // Numerical integration is continuous; authoritative occupied locations are
        // native cells. Swept face moves prevent jumping through thin terrain.
        let dt = dt.clamp(0., 0.025);
        self.simulated += dt;
        let mut active = std::mem::take(&mut self.drops);
        let mut occupied: BTreeSet<_> = active.iter().map(|d| d.cell).collect();
        for mut d in active.drain(..) {
            occupied.remove(&d.cell);
            d.velocity[1] -= 9.81 * dt;
            for a in 0..3 {
                d.credit[a] += d.velocity[a] * dt / VOXEL_SIZE;
            }
            let mut finished = false;
            for a in [0, 2, 1] {
                for _ in 0..32 {
                    if d.credit[a].abs() < 1. {
                        break;
                    }
                    let step = if d.credit[a] > 0. { 1 } else { -1 };
                    let mut next = d.cell;
                    match a {
                        0 => next.x += step,
                        1 => next.y += step,
                        _ => next.z += step,
                    }
                    if occupied.contains(&next) {
                        d.credit[a] = 0.;
                        d.velocity[a] = 0.;
                        break;
                    }
                    match world.material(next) {
                        Material::Air => {
                            d.cell = next;
                            d.credit[a] -= step as f64;
                        }
                        Material::Sand | Material::Soil => {
                            self.absorb(world, next);
                            finished = true;
                            break;
                        }
                        Material::Water => {
                            // Return to the waterbody at the first free cell in this column.
                            let mut top = next;
                            let mut found = false;
                            for _ in 0..128 {
                                if world.material(top) == Material::Air && !occupied.contains(&top)
                                {
                                    found = true;
                                    break;
                                }
                                top.y += 1;
                            }
                            if found {
                                world.set_fluid_occupancy([(top, true)]);
                                self.deposited.insert(top);
                                finished = true;
                            } else {
                                d.credit[a] = 0.;
                                d.velocity[a] = 0.;
                            }
                            break;
                        }
                        _ => {
                            if a == 1 && step < 0 && world.material(d.cell) == Material::Air {
                                world.set_fluid_occupancy([(d.cell, true)]);
                                self.deposited.insert(d.cell);
                                finished = true;
                                break;
                            }
                            d.credit[a] = 0.;
                            d.velocity[a] = 0.;
                        }
                    }
                }
                if finished {
                    break;
                }
            }
            if !finished {
                occupied.insert(d.cell);
                self.drops.push(d);
            }
        }
        debug_assert!(self.conserved());
    }
    pub fn apply(&self, world: &mut World) {
        world.set_fluid_occupancy(
            self.removed
                .iter()
                .map(|&c| (c, false))
                .chain(self.deposited.iter().map(|&c| (c, true))),
        );
        for &c in &self.damp {
            world.set(c, Material::Soil);
        }
    }
    pub fn snapshot(&self, seed: u64) -> String {
        let mut s = format!(
            "BOWL1 {seed} {} {} {} {} {} {}\n",
            self.units, self.scooped, self.emitted, self.spilled, self.phase, self.simulated
        );
        for &c in &self.removed {
            s += &format!("R {} {} {}\n", c.x, c.y, c.z);
        }
        for &c in &self.deposited {
            s += &format!("W {} {} {}\n", c.x, c.y, c.z);
        }
        for &c in &self.damp {
            s += &format!("D {} {} {}\n", c.x, c.y, c.z);
        }
        for (c, n) in &self.moisture {
            s += &format!("M {} {} {} {n}\n", c.x, c.y, c.z);
        }
        for d in &self.drops {
            s += &format!(
                "P {} {} {} {} {} {} {} {} {}\n",
                d.cell.x,
                d.cell.y,
                d.cell.z,
                d.velocity[0],
                d.velocity[1],
                d.velocity[2],
                d.credit[0],
                d.credit[1],
                d.credit[2]
            );
        }
        s
    }
    pub fn restore(s: &str, seed: u64) -> Result<Self, &'static str> {
        if s.len() > 4_000_000 {
            return Err("bowl save too large");
        }
        let mut lines = s.lines();
        let h: Vec<_> = lines
            .next()
            .ok_or("empty bowl save")?
            .split_whitespace()
            .collect();
        if h.len() != 8 || h[0] != "BOWL1" || h[1].parse::<u64>().ok() != Some(seed) {
            return Err("bowl save identity mismatch");
        }
        let number = |v: &str| v.parse::<u64>().map_err(|_| "invalid count");
        let real = |v: &str| {
            v.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or("invalid numeric field")
        };
        let mut b = Self {
            units: u32::try_from(number(h[2])?).map_err(|_| "count overflow")?,
            scooped: number(h[3])?,
            emitted: number(h[4])?,
            spilled: number(h[5])?,
            phase: real(h[6])?,
            simulated: real(h[7])?,
            ..Default::default()
        };
        if b.units > CAPACITY
            || !(0. ..1.).contains(&b.phase)
            || b.simulated < 0.
            || b.spilled > b.emitted
        {
            return Err("invalid bowl header");
        }
        for line in lines {
            let v: Vec<_> = line.split_whitespace().collect();
            if v.len() < 4 {
                return Err("short record");
            }
            let cell = Cell {
                x: v[1].parse().map_err(|_| "invalid cell")?,
                y: v[2].parse().map_err(|_| "invalid cell")?,
                z: v[3].parse().map_err(|_| "invalid cell")?,
            };
            if [cell.x, cell.y, cell.z]
                .into_iter()
                .any(|v| v.unsigned_abs() > (1u64 << 40))
            {
                return Err("bowl cell outside supported range");
            }
            let valid = match v[0] {
                "R" if v.len() == 4 => b.removed.insert(cell),
                "W" if v.len() == 4 => b.deposited.insert(cell),
                "D" if v.len() == 4 => b.damp.insert(cell),
                "M" if v.len() == 5 => {
                    let n = u32::try_from(number(v[4])?).map_err(|_| "moisture overflow")?;
                    n > 0 && b.moisture.insert(cell, n).is_none()
                }
                "P" if v.len() == 10 => {
                    let velocity = [real(v[4])?, real(v[5])?, real(v[6])?];
                    let credit = [real(v[7])?, real(v[8])?, real(v[9])?];
                    if b.drops.len() >= CAPACITY as usize || b.drops.iter().any(|d| d.cell == cell)
                    {
                        false
                    } else {
                        b.drops.push(Drop {
                            cell,
                            velocity,
                            credit,
                        });
                        true
                    }
                }
                _ => false,
            };
            if !valid {
                return Err("invalid or duplicate bowl record");
            }
        }
        if b.removed.len() as u64 != b.scooped || !b.conserved() {
            return Err("bowl ledger mismatch");
        }
        Ok(b)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{desert, FbmParams, FbmRecipe};
    fn fixture() -> (World, [f64; 3], [f64; 3]) {
        let p = FbmParams {
            recipe: FbmRecipe::Desert,
            ..Default::default()
        };
        let w = World::fbm(19, p).unwrap();
        let o = desert::oasis(19, 0, 0);
        (w, [o.x, o.stage + 1., o.z], [0., -1., 0.])
    }
    #[test]
    fn return_to_pond_and_rescoop_preserve_the_original_ledger() {
        let (mut w, eye, dir) = fixture();
        let mut b = Bowl::default();
        b.scoop(&mut w, eye, dir).unwrap();
        for _ in 0..200 {
            b.emit(&w, eye, dir, 0.025, false, true);
            b.advance(&mut w, 0.025);
        }
        assert!(b.drops.is_empty());
        assert_eq!(b.deposited.len(), CAPACITY as usize);
        assert!(b.conserved());
        let s = b.snapshot(19);
        let mut restored = Bowl::restore(&s, 19).unwrap();
        let n = restored.scoop(&mut w, eye, dir).unwrap();
        assert!(n > 0 && n <= CAPACITY);
        assert!(restored.conserved());
        assert_eq!(restored.scooped, CAPACITY as u64);
    }
    #[test]
    fn scoop_is_finite_and_pour_becomes_saved_moisture() {
        let (mut w, eye, dir) = fixture();
        let mut b = Bowl::default();
        assert_eq!(b.scoop(&mut w, eye, dir), Ok(CAPACITY));
        assert!(b.scoop(&mut w, eye, dir).is_err());
        let o = desert::oasis(19, 0, 0);
        let x = o.x + 60.;
        let z = o.z;
        let y = w.ground_height_m(x, z) + 1.7;
        let eye = [x, y, z];
        for _ in 0..200 {
            b.emit(&w, eye, dir, 0.025, false, true);
            b.advance(&mut w, 0.025);
        }
        assert_eq!(b.units, 0);
        assert!(b.drops.is_empty());
        assert_eq!(b.moisture.values().sum::<u32>(), CAPACITY);
        assert!(!b.damp.is_empty());
        assert!(b.conserved());
        let s = b.snapshot(19);
        let r = Bowl::restore(&s, 19).unwrap();
        assert_eq!(r.snapshot(19), s);
        let (mut fresh, _, _) = fixture();
        r.apply(&mut fresh);
        for c in &r.damp {
            assert_eq!(fresh.material(*c), Material::Soil);
        }
        assert!(Bowl::restore(&s, 20).is_err());
        assert!(Bowl::restore(&s.replace("BOWL1 19 0", "BOWL1 19 8"), 19).is_err());
    }
    #[test]
    fn blocked_or_distant_scoop_never_creates_water() {
        let (mut w, mut eye, dir) = fixture();
        eye[1] += 10.;
        let mut b = Bowl::default();
        assert!(b.scoop(&mut w, eye, dir).is_err());
        assert!(b.conserved());
        assert_eq!(b.units, 0);
    }
    #[test]
    fn tipped_bowl_counts_spills_and_restart_in_flight() {
        let (mut w, eye, dir) = fixture();
        let mut b = Bowl::default();
        b.scoop(&mut w, eye, dir).unwrap();
        b.emit(&w, eye, dir, 0.1, true, true);
        b.emit(&w, eye, dir, 0.1, true, true);
        assert_eq!(b.spilled, 1);
        assert_eq!(b.drops.len(), 1);
        let r = Bowl::restore(&b.snapshot(19), 19).unwrap();
        assert!(r.conserved());
        assert_eq!(r.drops[0].cell, b.drops[0].cell);
    }
}
