//! Finite bowl and local whole-cell droplets. No fluid source is manufactured.
//! Sand absorption stores water units separately from the visual damp footprint.
use crate::{raycast, Cell, Material, World, VOXEL_SIZE};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
const FLOW_CELLS_PER_TICK: usize = 64;
const SIDES: [[i64; 3]; 4] = [[1, 0, 0], [0, 0, 1], [-1, 0, 0], [0, 0, -1]];
const UP: [i64; 3] = [0, 1, 0];
const DOWN: [i64; 3] = [0, -1, 0];

fn offset(c: Cell, [x, y, z]: [i64; 3]) -> Cell {
    Cell {
        x: c.x + x,
        y: c.y + y,
        z: c.z + z,
    }
}
// Bounds hostile save inputs and the renderer's quadratic parcel work. Live
// intake is governed by space at the vessel opening.
const MAX_SAVED_UNITS: u32 = 256;
const MAX_DROPS: usize = 128;
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
    /// Whole units transferred to plant storage/transpiration, never lost.
    pub plant_transferred: u64,
    pub moisture: BTreeMap<Cell, u32>,
    pub removed: BTreeSet<Cell>,
    pub deposited: BTreeSet<Cell>,
    pub damp: BTreeSet<Cell>,
    pub drops: Vec<Drop>,
    phase: f64,
    pub simulated: f64,
    flow: VecDeque<Cell>,
    queued_flow: BTreeSet<Cell>,
}
impl Bowl {
    pub fn conserved(&self) -> bool {
        self.scooped
            == u64::from(self.units)
                + self.drops.len() as u64
                + self.moisture.values().map(|v| u64::from(*v)).sum::<u64>()
                + self.deposited.len() as u64
                + self.plant_transferred
    }
    pub fn scoop(
        &mut self,
        world: &mut World,
        eye: [f64; 3],
        dir: [f64; 3],
        limit: u32,
    ) -> Result<u32, &'static str> {
        let hit = raycast(world, eye, dir, 2.2).ok_or("Move closer and aim at pond water")?;
        if hit.material != Material::Water {
            return Err("Aim at water within 2.2 m");
        }
        self.scoop_at(world, hit.cell, limit)
    }

    pub fn scoop_at(
        &mut self,
        world: &mut World,
        contact: Cell,
        limit: u32,
    ) -> Result<u32, &'static str> {
        if world.material(contact) != Material::Water {
            return Err("Lower the rim into the pond");
        }
        let count = limit.min(MAX_SAVED_UNITS.saturating_sub(self.units)) as usize;
        if count == 0 {
            return Ok(0);
        }
        let mut picked = Vec::new();
        let mut frontier = VecDeque::from([contact]);
        let mut visited = BTreeSet::from([contact]);
        while let Some(cell) = frontier.pop_front() {
            if world.material(cell) != Material::Water {
                continue;
            }
            picked.push(cell);
            if picked.len() == count {
                break;
            }
            for delta in SIDES.into_iter().chain([DOWN, UP]) {
                let neighbor = offset(cell, delta);
                if visited.insert(neighbor) {
                    frontier.push_back(neighbor);
                }
            }
        }
        for &c in &picked {
            if !self.deposited.remove(&c) {
                self.removed.insert(c);
            }
        }
        world.set_fluid_occupancy(picked.iter().map(|&c| (c, false)));
        let units = picked.len() as u32;
        self.units += units;
        self.scooped = self.removed.len() as u64;
        for cell in picked.into_iter().rev() {
            self.relax_scoop(world, cell);
            self.wake_water_near(cell);
        }
        debug_assert!(self.conserved());
        Ok(units)
    }

    /// Spread each scoop vacancy over the connected surface. A same-level
    /// transfer across an occupied path is a local pressure adjustment. Its
    /// donor is one whole surface voxel and the net deficit is saved.
    fn relax_scoop(&mut self, world: &mut World, cell: Cell) {
        if world.material(cell) != Material::Air {
            return;
        }
        let above = offset(cell, UP);
        if world.material(above) == Material::Water {
            self.transfer(world, above, cell);
            return;
        }
        let turn = ((cell.x ^ cell.z) as usize) & 3;
        let mut visited = BTreeSet::from([cell]);
        let mut frontier = VecDeque::from([(cell, 0)]);
        let mut donor = None;
        while let Some((current, distance)) = frontier.pop_front() {
            if distance > 0 && world.material(offset(current, UP)) == Material::Air {
                donor = Some(current);
                if distance >= 5 {
                    break;
                }
            }
            for side in 0..4 {
                let next = offset(current, SIDES[(side + turn) & 3]);
                if visited.insert(next) && world.material(next) == Material::Water {
                    frontier.push_back((next, distance + 1));
                }
            }
            if visited.len() >= 96 {
                break;
            }
        }
        if let Some(from) = donor {
            self.transfer(world, from, cell);
        }
    }

    pub fn terrain_changed(&mut self, world: &mut World, cell: Cell) {
        // The newly exposed cell may be level with shallow water. Let the same
        // connected pressure adjustment fill it, then step the local flow.
        self.relax_scoop(world, cell);
        self.wake_water_near(cell);
    }

    pub fn wake_water_near(&mut self, cell: Cell) {
        self.enqueue_flow(cell);
        for delta in SIDES.into_iter().chain([UP, DOWN]) {
            self.enqueue_flow(offset(cell, delta));
        }
    }

    fn enqueue_flow(&mut self, cell: Cell) {
        if self.queued_flow.insert(cell) {
            self.flow.push_back(cell);
        }
    }

    /// The queue contains vacancies. Water may fall into one, spill into a
    /// lower opening, or refill it under a column of water. Level open surfaces
    /// sleep; there is no continual scan of the procedural pond.
    fn settle_water(&mut self, world: &mut World) {
        let mut changes = BTreeMap::new();
        for _ in 0..self.flow.len().min(FLOW_CELLS_PER_TICK) {
            let Some(to) = self.flow.pop_front() else {
                break;
            };
            self.queued_flow.remove(&to);
            let material = |cell| match changes.get(&cell) {
                Some(true) => Material::Water,
                Some(false) => Material::Air,
                None => world.material(cell),
            };
            if material(to) != Material::Air {
                continue;
            }
            let above = offset(to, UP);
            let below = material(offset(to, DOWN));
            let from = if material(above) == Material::Water {
                Some(above)
            } else {
                SIDES.iter().copied().map(|d| offset(to, d)).find(|&side| {
                    material(side) == Material::Water
                        && (below == Material::Air
                            || (below == Material::Water
                                && material(offset(side, UP)) == Material::Water))
                })
            };
            if let Some(from) = from {
                self.record_transfer(world, from, to);
                changes.extend([(from, false), (to, true)]);
                self.wake_water_near(from);
                self.wake_water_near(to);
            }
        }
        world.set_fluid_occupancy(changes);
    }

    fn transfer(&mut self, world: &mut World, from: Cell, to: Cell) {
        self.record_transfer(world, from, to);
        world.set_fluid_occupancy([(from, false), (to, true)]);
    }

    fn record_transfer(&mut self, world: &World, from: Cell, to: Cell) {
        if !self.deposited.remove(&from) {
            self.removed.insert(from);
        }
        // A wet override is still needed if the player has excavated away
        // the original source at this address.
        if world.material_without_fluid(to) != Material::Water || !self.removed.remove(&to) {
            self.deposited.insert(to);
        }
        self.scooped = self.removed.len() as u64;
    }
    /// Reserve a landing cell for each displaced whole unit before changing
    /// either the terrain or the liquid. A blocked placement is atomic.
    pub(crate) fn displace_for_placement(
        &mut self,
        world: &mut World,
        solids: &BTreeMap<Cell, Material>,
    ) -> Result<(), &'static str> {
        let mut displaced = Vec::new();
        let mut reserved = BTreeSet::new();
        for &from in solids.keys() {
            if world.material(from) != Material::Water {
                continue;
            }
            let mut to = from;
            let mut destination = None;
            for _ in 0..96 {
                to = offset(to, UP);
                if solids.contains_key(&to) || reserved.contains(&to) {
                    continue;
                }
                match world.material(to) {
                    Material::Air => {
                        destination = Some(to);
                        break;
                    }
                    Material::Water => (),
                    _ => break,
                }
            }
            let to = destination.ok_or("There is no open space for the displaced water")?;
            reserved.insert(to);
            displaced.push((from, to));
        }
        for &(from, to) in &displaced {
            self.record_transfer(world, from, to);
            self.wake_water_near(from);
            self.wake_water_near(to);
        }
        world.set_fluid_occupancy(
            displaced
                .into_iter()
                .flat_map(|(from, to)| [(from, false), (to, true)]),
        );
        debug_assert!(self.conserved());
        Ok(())
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
            self.emit_at(world, c, [dir[0] * 1.2, -0.7, dir[2] * 1.2], spill);
        }
    }

    pub fn emit_at(
        &mut self,
        world: &World,
        cell: Cell,
        velocity: [f64; 3],
        accidental: bool,
    ) -> bool {
        if self.units == 0
            || self.drops.len() >= MAX_DROPS
            || world.material(cell) != Material::Air
            || self.drops.iter().any(|drop| drop.cell == cell)
        {
            return false;
        }
        self.units -= 1;
        self.emitted += 1;
        self.spilled += u64::from(accidental);
        self.drops.push(Drop {
            cell,
            velocity,
            credit: [0.; 3],
        });
        true
    }
    /// Commit an escaped unit at the cell it actually reached, including when
    /// the vessel is still over the pond or pours directly into nearby sand.
    pub fn release_at(
        &mut self,
        world: &mut World,
        cell: Cell,
        velocity: [f64; 3],
        accidental: bool,
    ) -> bool {
        if self.units == 0 {
            return false;
        }
        match world.material(cell) {
            Material::Air => return self.emit_at(world, cell, velocity, accidental),
            Material::Sand | Material::Soil => self.absorb(world, cell),
            Material::Water => {
                let mut top = offset(cell, UP);
                for _ in 0..128 {
                    match world.material(top) {
                        Material::Air if !self.drops.iter().any(|d| d.cell == top) => {
                            world.set_fluid_occupancy([(top, true)]);
                            self.deposited.insert(top);
                            self.wake_water_near(top);
                            self.units -= 1;
                            self.emitted += 1;
                            self.spilled += u64::from(accidental);
                            return true;
                        }
                        Material::Water => top = offset(top, UP),
                        _ => return false,
                    }
                }
                return false;
            }
            _ => return false,
        }
        self.units -= 1;
        self.emitted += 1;
        self.spilled += u64::from(accidental);
        true
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
                                self.wake_water_near(top);
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
                                self.wake_water_near(d.cell);
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
        self.settle_water(world);
        debug_assert!(self.conserved());
    }
    pub fn apply(&self, world: &mut World) {
        self.apply_damp(world);
        self.apply_occupancy(world);
    }

    pub fn apply_damp(&self, world: &mut World) {
        world.set_batch(self.damp.iter().map(|&cell| (cell, Material::Soil)));
    }

    pub fn apply_occupancy(&self, world: &mut World) {
        world.set_fluid_occupancy(
            self.removed
                .iter()
                .filter(|c| !self.deposited.contains(c))
                .map(|&c| (c, false))
                .chain(self.deposited.iter().map(|&c| (c, true))),
        );
    }
    pub fn snapshot(&self, seed: u64) -> String {
        let mut s = format!(
            "BOWL2 {seed} {} {} {} {} {} {} {}\n",
            self.units,
            self.scooped,
            self.emitted,
            self.spilled,
            self.phase,
            self.simulated,
            self.plant_transferred
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
        for &c in &self.flow {
            s += &format!("F {} {} {}\n", c.x, c.y, c.z);
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
        if !((h.len() == 8 && h[0] == "BOWL1") || (h.len() == 9 && h[0] == "BOWL2"))
            || h[1].parse::<u64>().ok() != Some(seed)
        {
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
            plant_transferred: if h.len() == 9 { number(h[8])? } else { 0 },
            scooped: number(h[3])?,
            emitted: number(h[4])?,
            spilled: number(h[5])?,
            phase: real(h[6])?,
            simulated: real(h[7])?,
            ..Default::default()
        };
        if b.units > MAX_SAVED_UNITS
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
                "F" if v.len() == 4 && !b.queued_flow.contains(&cell) => {
                    b.enqueue_flow(cell);
                    true
                }
                "M" if v.len() == 5 => {
                    let n = u32::try_from(number(v[4])?).map_err(|_| "moisture overflow")?;
                    n > 0 && b.moisture.insert(cell, n).is_none()
                }
                "P" if v.len() == 10 => {
                    let velocity = [real(v[4])?, real(v[5])?, real(v[6])?];
                    let credit = [real(v[7])?, real(v[8])?, real(v[9])?];
                    if b.drops.len() >= MAX_DROPS || b.drops.iter().any(|d| d.cell == cell) {
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
        if b.removed.len() as u64 != b.scooped || b.plant_transferred > b.scooped || !b.conserved()
        {
            return Err("bowl ledger mismatch");
        }
        Ok(b)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{desert, FbmParams, FbmRecipe};
    const SAMPLE_UNITS: u32 = 20;
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
    fn rejects_impossible_transferred_water_before_ledger_arithmetic() {
        let b = Bowl::default();
        let s = b.snapshot(19);
        let mut fields = s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        fields[8] = u64::MAX.to_string();
        assert!(Bowl::restore(&fields.join(" "), 19).is_err());
    }
    #[test]
    fn return_to_pond_and_rescoop_preserve_the_original_ledger() {
        let (mut w, eye, dir) = fixture();
        let mut b = Bowl::default();
        b.scoop(&mut w, eye, dir, SAMPLE_UNITS).unwrap();
        for _ in 0..200 {
            b.emit(&w, eye, dir, 0.025, false, true);
            b.advance(&mut w, 0.025);
        }
        assert!(b.drops.is_empty());
        assert_eq!(b.units, 0);
        assert_eq!(b.emitted, u64::from(SAMPLE_UNITS));
        assert!(b.moisture.is_empty());
        assert_eq!(b.removed.len(), b.deposited.len());
        assert!(b.conserved());
        let s = b.snapshot(19);
        let mut restored = Bowl::restore(&s, 19).unwrap();
        let n = restored.scoop(&mut w, eye, dir, SAMPLE_UNITS).unwrap();
        assert!(n > 0 && n <= SAMPLE_UNITS);
        assert!(restored.conserved());
        assert_eq!(
            restored.removed.len() - restored.deposited.len(),
            n as usize
        );
    }
    #[test]
    fn scoop_is_finite_and_pour_becomes_saved_moisture() {
        let (mut w, eye, dir) = fixture();
        let mut b = Bowl::default();
        assert_eq!(b.scoop(&mut w, eye, dir, SAMPLE_UNITS), Ok(SAMPLE_UNITS));
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
        assert_eq!(b.moisture.values().sum::<u32>(), SAMPLE_UNITS);
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
        assert!(Bowl::restore(&s.replace("BOWL2 19 0", "BOWL2 19 8"), 19).is_err());
    }
    #[test]
    fn scoop_refills_the_visible_hole_with_finite_water() {
        let (mut world, eye, dir) = fixture();
        let mut bowl = Bowl::default();
        let hit = raycast(&world, eye, dir, 2.2).unwrap().cell;
        assert_eq!(
            bowl.scoop(&mut world, eye, dir, SAMPLE_UNITS),
            Ok(SAMPLE_UNITS)
        );
        assert_eq!(world.material(hit), Material::Water);
        assert_eq!(
            bowl.removed.len() - bowl.deposited.len(),
            SAMPLE_UNITS as usize
        );
        assert!(bowl.conserved());
    }

    #[test]
    fn breaking_a_bank_moves_whole_water_into_lower_ground_and_survives_reload() {
        let mut world = World::new(44);
        let bank = Cell { x: 1, y: 201, z: 0 };
        let below = offset(bank, DOWN);
        let source = Cell { x: 0, ..bank };
        let mut scaffold = vec![
            (bank, Material::Stone),
            (offset(below, DOWN), Material::Stone),
            (source, Material::Water),
            (offset(source, UP), Material::Water),
            (offset(source, DOWN), Material::Stone),
        ];
        for side in SIDES {
            scaffold.push((offset(below, side), Material::Stone));
            if side != SIDES[0] {
                scaffold.push((offset(source, side), Material::Stone));
                scaffold.push((offset(offset(source, UP), side), Material::Stone));
            }
        }
        world.set_batch(scaffold.iter().copied());
        let mut bowl = Bowl::default();
        assert_eq!(world.material(below), Material::Air);
        world.set(bank, Material::Air);
        bowl.terrain_changed(&mut world, bank);
        for _ in 0..4 {
            bowl.advance(&mut world, 0.025);
        }
        assert_eq!(world.material(below), Material::Water);
        assert_eq!(bowl.removed.len(), bowl.deposited.len());
        assert!(bowl.conserved());

        let saved = bowl.snapshot(world.seed());
        let restored = Bowl::restore(&saved, world.seed()).unwrap();
        let mut reloaded = World::new(world.seed());
        reloaded.set_batch(scaffold);
        reloaded.set(bank, Material::Air);
        restored.apply(&mut reloaded);
        assert_eq!(reloaded.material(below), Material::Water);
        assert_eq!(restored.snapshot(world.seed()), saved);
    }

    #[test]
    fn blocked_or_distant_scoop_never_creates_water() {
        let (mut w, mut eye, dir) = fixture();
        eye[1] += 10.;
        let mut b = Bowl::default();
        assert!(b.conserved());
        assert_eq!(b.units, 0);
    }
    #[test]
    fn tipped_bowl_counts_spills_and_restart_in_flight() {
        let (mut w, eye, dir) = fixture();
        let mut b = Bowl::default();
        b.scoop(&mut w, eye, dir, SAMPLE_UNITS).unwrap();
        b.emit(&w, eye, dir, 0.1, true, true);
        b.emit(&w, eye, dir, 0.1, true, true);
        assert_eq!(b.spilled, 1);
        assert_eq!(b.drops.len(), 1);
        let r = Bowl::restore(&b.snapshot(19), 19).unwrap();
        assert!(r.conserved());
        assert_eq!(r.drops[0].cell, b.drops[0].cell);
    }
}
