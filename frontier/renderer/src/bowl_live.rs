//! Desert inventory, physical bowl interaction, and garden updates.
use crate::{avatar::Hands, vessel::Vessel, Camera, Result};
use frontier_world::{
    bowl::{Bowl, CAPACITY},
    garden::{Garden, GrowthControls, Tissue},
    pockets::{Item, Pockets, HANDFUL},
    Cell, Material, World, VOXEL_SIZE,
};
use std::{path::PathBuf, time::Instant};
#[derive(Clone, Default)]
pub struct Overlay {
    pub text: String,
    pub fill: f32,
    pub faces: Vec<([[f32; 2]; 4], [u8; 3])>,
    pub lines: Vec<[[f32; 2]; 2]>,
}
struct HandGesture {
    started: Instant,
    cells: Vec<(Cell, Material)>,
}
pub struct BowlLive {
    pub state: Bowl,
    pub garden: Garden,
    pub growth_controls: GrowthControls,
    pub pockets: Pockets,
    gesture: Option<HandGesture>,
    pub pouring: bool,
    scooping: bool,
    dip: f64,
    tilt: f64,
    pub vessel: Vessel,
    pub message: String,
    path: PathBuf,
    identity: String,
    debt: f64,
    clock: Instant,
    pub saves: u64,
}
impl BowlLive {
    pub fn new(world: &mut World, path: Option<PathBuf>) -> Result<Self> {
        let t = world.fbm_terrain().ok_or("bowl requires desert terrain")?;
        let p = t.params();
        let identity = format!(
            "DESERT_BOWL1 {} {} {} {}",
            world.seed(),
            t.source_version(),
            p.horizontal_scale.to_bits(),
            p.amplitude.to_bits()
        );
        let path = path.unwrap_or_else(|| {
            PathBuf::from(format!(
                "output/desert-game/{}-{}-{}.bowl",
                world.seed(),
                p.horizontal_scale.to_bits(),
                p.amplitude.to_bits()
            ))
        });
        let mut state = Bowl::default();
        let mut garden = Garden::default();
        if path.exists() {
            let s = std::fs::read_to_string(&path)?;
            let (id, body) = s.split_once('\n').ok_or("invalid bowl envelope")?;
            if id != identity {
                return Err("bowl save belongs to another terrain source".into());
            }
            let pocket_start = body.find("\nPOCKETS1 ");
            let (body, pocket_text) =
                pocket_start.map_or((body, None), |i| (&body[..i], Some(&body[i + 1..])));
            let garden_start = body.find("\nGARDEN");
            let (water, plants) =
                garden_start.map_or((body, None), |i| (&body[..i], Some(&body[i + 1..])));
            state = Bowl::restore(water, world.seed())?;
            if let Some(plants) = plants {
                garden = Garden::restore(plants, world.seed())?;
            }
            if garden.water_accounted() != state.plant_transferred {
                return Err("plant/water save ledger mismatch".into());
            }
            state.apply_damp(world);
            if let Some(text) = pocket_text {
                let pockets = Pockets::restore(text, garden.seeds)?;
                pockets.apply(world);
                state.apply_occupancy(world);
                garden.apply_tissue(world);
                return Ok(Self::with_state(state, garden, pockets, path, identity));
            }
            garden.apply(world);
            state.apply_occupancy(world);
        }
        let pockets = Pockets::migrate(world, &garden)?;
        garden.sand = 0;
        garden.soil = 0;
        garden.apply_tissue(world);
        Ok(Self::with_state(state, garden, pockets, path, identity))
    }
    fn with_state(
        state: Bowl,
        garden: Garden,
        pockets: Pockets,
        path: PathBuf,
        identity: String,
    ) -> Self {
        Self {
            state,
            garden,
            growth_controls: GrowthControls::default(),
            pockets,
            gesture: None,
            pouring: false,
            scooping: false,
            dip: 0.,
            tilt: 0.,
            vessel: Vessel::default(),
            message: "Find the shoreline and scoop some water".into(),
            path,
            identity,
            debt: 0.,
            clock: Instant::now(),
            saves: 0,
        }
    }
    pub(crate) fn holds_seed(&self) -> bool {
        self.pockets.hand == Some(Item::Seed)
    }
    pub(crate) fn seed_program(&self) -> frontier_world::growth::SeedProgram {
        self.garden.seed_program.clone().unwrap_or_default()
    }
    pub(crate) fn apply_seed_program(&mut self, program: frontier_world::growth::SeedProgram) {
        if self.holds_seed() {
            self.garden.seed_program = Some(program);
            self.message = "Seed programmed. Place it, cover it with sand, then water it.".into();
        }
    }
    pub fn select_pocket(&mut self, index: usize) {
        self.gesture = None;
        self.pouring = false;
        self.scooping = false;
        self.vessel = Vessel::default();
        self.message = match self.pockets.select(index) {
            Ok(()) => format!(
                "Hands: {}",
                self.pockets
                    .hand
                    .as_ref()
                    .map_or("empty".into(), Item::name)
            ),
            Err(e) => e.into(),
        };
    }
    pub fn stow(&mut self) {
        self.gesture = None;
        self.pouring = false;
        self.scooping = false;
        self.vessel = Vessel::default();
        self.message = match self.pockets.stow() {
            Ok(()) => "Item put in your pocket".into(),
            Err(e) => e.into(),
        };
    }
    pub fn toggle_bowl(&mut self) {
        if self.pockets.bowl() {
            self.stow();
        } else if let Some(i) = self
            .pockets
            .slots
            .iter()
            .position(|v| v == &Some(Item::Bowl))
        {
            self.select_pocket(i);
        }
    }
    fn hand_settings() -> frontier_world::edit::HandSettings {
        frontier_world::edit::HandSettings {
            volume_liters: HANDFUL as f64 * VOXEL_SIZE.powi(3) * 1000. + 0.0001,
            extent_m: 0.25,
            reach_m: 2.2,
            ..Default::default()
        }
    }
    pub fn hand_mouse(&mut self, world: &mut World, camera: Camera, left: bool, pressed: bool) {
        if pressed {
            eprintln!(
                "hand_pose [{},{},{},{},{}]",
                camera.position.x, camera.position.y, camera.position.z, camera.yaw, camera.pitch
            );
        }
        if self.pockets.bowl() {
            if left {
                self.scooping = pressed;
            } else {
                self.pouring = pressed;
            }
            return;
        }
        if left && pressed && self.pockets.hand.is_none() {
            if let Some(hit) = frontier_world::raycast_solid(
                world,
                camera.position.to_array(),
                camera.direction().as_dvec3().to_array(),
                2.2,
            ) {
                let cells = Pockets::selection(world, hit.cell)
                    .into_iter()
                    .take_while(|(c, _)| crate::player_edit::reachable(*c, camera, 2.2))
                    .collect::<Vec<_>>();
                if !cells.is_empty() {
                    self.gesture = Some(HandGesture {
                        started: Instant::now(),
                        cells,
                    });
                    self.message = "Hold to gather a larger handful, then release".into();
                } else {
                    self.message = "Empty hands can gather sand; select the bowl for water".into();
                }
            }
        } else if left && !pressed {
            if let Some(g) = self.gesture.take() {
                let hit = frontier_world::raycast_solid(
                    world,
                    camera.position.to_array(),
                    camera.direction().as_dvec3().to_array(),
                    2.2,
                );
                if hit.is_none_or(|h| h.cell != g.cells[0].0)
                    || g.cells
                        .iter()
                        .any(|(c, _)| !crate::player_edit::reachable(*c, camera, 2.2))
                {
                    self.message = "Gathering cancelled: aim or reach changed".into();
                    return;
                }
                let n = Self::hand_settings()
                    .count_at(g.started.elapsed().as_secs_f64())
                    .min(g.cells.len());
                if g.cells[..n]
                    .iter()
                    .any(|(c, _)| self.state.moisture.get(c).copied().unwrap_or(0) > 0)
                {
                    self.message = "That soil holds water — gather dry sand for now".into();
                    return;
                }
                self.message = match self.pockets.pickup(world, &g.cells[..n]) {
                    Ok(()) => {
                        for &(cell, _) in &g.cells[..n] {
                            self.state.terrain_changed(world, cell);
                        }
                        eprintln!(
                            "pockets pickup count={n} conserved={}",
                            self.pockets.conserved()
                        );
                        format!("Holding {n} voxels — right click/R to drop, E to pocket")
                    }
                    Err(e) => e.into(),
                };
            }
        } else if !left && pressed {
            self.gesture = None;
            let Some(hit) = frontier_world::raycast_solid(
                world,
                camera.position.to_array(),
                camera.direction().as_dvec3().to_array(),
                2.2,
            ) else {
                self.message = "Aim at nearby ground to place what you hold".into();
                return;
            };
            let start = Cell {
                x: hit.cell.x + i64::from(hit.normal[0]),
                y: hit.cell.y + i64::from(hit.normal[1]),
                z: hit.cell.z + i64::from(hit.normal[2]),
            };
            let can_place =
                |c| !crate::player_edit::intersects_player(c, camera.position.to_array());
            self.message = match self.pockets.hand.as_ref() {
                Some(Item::Seed) => {
                    match self
                        .pockets
                        .drop_seed(world, &mut self.garden, start, can_place)
                    {
                        Ok(()) => {
                            eprintln!("pockets seed placed black=true");
                            "Black seed placed — cover it by dropping sand on it".into()
                        }
                        Err(e) => e.into(),
                    }
                }
                Some(Item::Sand(_)) => {
                    let before = world.clone();
                    match self.pockets.drop_sand_with_water(
                        world,
                        start,
                        can_place,
                        Some(&mut self.state),
                    ) {
                        Ok(n) => {
                            for cell in world.changed_edit_cells(&before) {
                                self.state.terrain_changed(world, cell);
                            }
                            eprintln!(
                                "pockets drop count={n} conserved={}",
                                self.pockets.conserved()
                            );
                            format!("Dropped {n} sand voxels onto the ground")
                        }
                        Err(e) => e.into(),
                    }
                }
                _ => "Your hands are empty — select a pocket".into(),
            };
        }
    }
    pub fn update(
        &mut self,
        world: &mut World,
        hands: Hands,
        spill: bool,
        active: bool,
        sun: [f64; 3],
    ) {
        let elapsed = self.clock.elapsed().as_secs_f64();
        self.clock = Instant::now();
        if !active {
            self.pouring = false;
            self.scooping = false;
            self.gesture = None;
            return;
        }
        self.debt = (self.debt + elapsed).min(0.4);
        let equipped = self.pockets.bowl();
        let pouring = equipped && self.pouring;
        let scoop = equipped && self.scooping && !pouring && !spill;
        let target = if spill {
            1.9
        } else if pouring {
            0.95
        } else {
            0.
        };
        let response = (elapsed * 11.).min(1.);
        self.tilt += (target - self.tilt) * response;
        self.dip += (f64::from(scoop) - self.dip) * response;
        if equipped {
            self.vessel.update(
                hands.vessel(self.dip, self.tilt),
                elapsed.clamp(0.001, 0.12),
            );
        } else {
            self.vessel = Vessel::default();
        }
        let spilling_before = self.state.spilled;
        let mut filled = 0;
        let before = self.garden.cells();
        let instant = self.growth_controls.is_instant();
        let mut water_stepped = false;
        for _ in 0..16 {
            if self.debt < 0.025 {
                break;
            }
            self.debt -= 0.025;
            filled += self
                .vessel
                .step(world, &mut self.state, scoop, !pouring, 0.025);
            self.state.advance(world, 0.025);
            water_stepped = true;
            if !instant {
                self.garden.advance_with_controls(
                    world,
                    &mut self.state,
                    0.025,
                    sun,
                    self.growth_controls,
                );
            }
        }
        if instant && water_stepped {
            self.garden.advance_with_controls(
                world,
                &mut self.state,
                0.025,
                sun,
                self.growth_controls,
            );
        }
        if self.garden.cells() != before {
            eprintln!(
                "garden growth cells={} leaves={} water_transferred={}",
                self.garden.cells(),
                self.garden
                    .plants
                    .iter()
                    .flat_map(|p| p.tissue.values())
                    .filter(|&&t| t == Tissue::Leaf)
                    .count(),
                self.state.plant_transferred
            );
        }
        if filled > 0 {
            eprintln!(
                "bowl scoop accepted units={filled} held={} conserved={}",
                self.state.units,
                self.state.conserved()
            );
            self.message = "Water scooped from the pond".into();
        } else if scoop && self.state.units == 0 {
            self.message = "Hold Ctrl and look down to dip the rim into the pond".into();
        }
        if self.state.spilled > spilling_before {
            self.message = if self.state.units == 0 {
                "Spilled the bowl".into()
            } else {
                "Water spilled over the rim".into()
            };
        } else if pouring && self.state.units == 0 {
            self.message = "Bowl empty".into();
        }
    }
    pub fn save(&mut self, world: &World) -> Result<()> {
        let seed = world.seed();
        // Water may have recolored placed sand since its placement was recorded.
        // Persist its current terrain material; tissue is restored separately.
        for (cell, material) in &mut self.pockets.edits {
            let current = world.material(*cell);
            if matches!(current, Material::Air | Material::Sand | Material::Soil) {
                *material = current;
            }
        }
        if !self.pockets.validate(self.garden.seeds)
            || !self.state.conserved()
            || self.garden.water_accounted() != self.state.plant_transferred
        {
            return Err("game inventory/water ledger mismatch; save refused".into());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = format!(
            "{}\n{}{}{}",
            self.identity,
            self.state.snapshot(seed),
            self.garden.snapshot(),
            self.pockets.snapshot()
        );
        let temporary = self.path.with_extension("bowl.tmp");
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(temporary, &self.path)?;
        self.saves += 1;
        Ok(())
    }
    pub fn json(&self) -> String {
        let mut out=format!("{{\"units\":{},\"capacity\":{},\"litres\":{},\"scooped_units\":{},\"emitted_units\":{},\"spilled_units\":{},\"airborne\":{},\"absorbed_units\":{},\"damp_cells\":{},\"deposited_units\":{},\"conserved\":{},\"simulation_debt\":{},\"save_count\":{}}}",self.state.units,CAPACITY,self.state.units as f64*VOXEL_SIZE.powi(3)*1000.,self.state.scooped,self.state.emitted,self.state.spilled,self.state.drops.len(),self.state.moisture.values().sum::<u32>(),self.state.damp.len(),self.state.deposited.len(),self.state.conserved(),self.debt,self.saves);
        out.pop();
        out.push_str(&format!(
            ",\"growth_controls\":{{\"infinite_energy\":{infinite},\"infinite_water\":{infinite},\"speed_step\":{}}}}}",
            self.growth_controls.speed_step,
            infinite = self.growth_controls.infinite_resources
        ));
        out.pop();
        out.push_str(&format!(",\"plant_transferred\":{},\"garden\":{{\"seeds\":{},\"holes\":{},\"plants\":{},\"germinated\":{},\"tissue_cells\":{},\"leaves\":{},\"water_stored\":{},\"transpired\":{},\"tick\":{},\"water_ledger_matches\":{}}}}}",self.state.plant_transferred,self.garden.seeds,self.garden.holes.len(),self.garden.plants.len(),self.garden.plants.iter().filter(|p|p.germination==4).count(),self.garden.cells(),self.garden.plants.iter().flat_map(|p|p.tissue.values()).filter(|&&t|t==Tissue::Leaf).count(),self.garden.plants.iter().map(|p|p.water).sum::<u32>(),self.garden.plants.iter().map(|p|p.transpired).sum::<u64>(),self.garden.tick,self.garden.water_accounted()==self.state.plant_transferred));
        out.pop();
        out.push_str(&format!(",\"pockets\":{{\"hand\":{:?},\"slots\":{},\"picked_voxels\":{},\"placed_voxels\":{},\"carried_voxels\":{},\"handful_limit\":{},\"loose_conserved\":{},\"inventory_valid\":{}}}}}",self.pockets.hand.as_ref().map_or("Empty".into(),Item::name),format!("[{}]",self.pockets.slots.iter().map(|v|format!("{:?}",v.as_ref().map_or("Empty".into(),Item::name))).collect::<Vec<_>>().join(",")),self.pockets.picked,self.pockets.placed,self.pockets.carried(),HANDFUL,self.pockets.conserved(),self.pockets.validate(self.garden.seeds)));
        out
    }
    pub fn overlay(&self, camera: Camera, aspect: f32) -> Overlay {
        let mut out = Overlay {
            fill: self.state.units as f32 / CAPACITY as f32,
            ..Default::default()
        };
        let pockets = self
            .pockets
            .slots
            .iter()
            .enumerate()
            .map(|(i, v)| {
                format!(
                    "{}: {}",
                    i + 1,
                    v.as_ref().map_or("Empty".into(), Item::name)
                )
            })
            .collect::<Vec<_>>()
            .join("  |  ");
        let hands = self
            .pockets
            .hand
            .as_ref()
            .map_or("Empty".into(), Item::name);
        out.text=format!("Hands: {hands} • bowl {:.2} L\n{pockets}\n1–4 swap pocket • E stow • Empty hands: hold left to gather • Right/R: place/pour\n{}",self.state.units as f64*VOXEL_SIZE.powi(3)*1000.,self.message);
        if self.pockets.bowl() {
            out.text.push_str(
                "\nCtrl crouch • Hold left to dip • Hold right to pour • C change camera",
            );
        }
        if self.holds_seed() {
            out.text.push_str("\nG · Program seed");
        }
        if let Some(g) = &self.gesture {
            let n = Self::hand_settings()
                .count_at(g.started.elapsed().as_secs_f64())
                .min(g.cells.len());
            out.fill = n as f32 / HANDFUL as f32;
            out.text.push_str(&format!(
                "\nGathering {n}/{HANDFUL} voxels ({:.2} L max)",
                HANDFUL as f64 * VOXEL_SIZE.powi(3) * 1000.
            ));
        }
        if let Some(p) = self.garden.plants.first() {
            let infinite = self.growth_controls.infinite_resources;
            let stage = if p.germination < 4 {
                if !self.garden.holes[p.hole].covered {
                    "Seed needs covering"
                } else if !infinite && p.water == 0 {
                    "Buried seed — waiting for water"
                } else {
                    "Seed germinating"
                }
            } else if p.at_capacity() {
                "Plant reached the current engine safety limit"
            } else if !p.has_growth() {
                "Plant tips blocked — no open growth directions"
            } else if !infinite && p.water == 0 && p.hydration == 0 {
                "Plant thirsty — water near its base"
            } else if !infinite && p.energy < 0.85 {
                "Plant low on energy — needs sunlight"
            } else {
                "Recursive plant growing"
            };
            let resources = if infinite {
                "energy ∞ • water ∞".into()
            } else {
                format!("energy {:.1}", p.energy)
            };
            out.text.push_str(&format!(
                "\n{stage} • {} cells • {resources} • {} lit leaves • {} growing tips",
                p.tissue.len(),
                p.lit_leaves,
                p.active_tips()
            ));
        }
        let matrix = camera.matrix(camera.position, aspect);
        if let Some(g) = &self.gesture {
            let n = Self::hand_settings()
                .count_at(g.started.elapsed().as_secs_f64())
                .min(g.cells.len());
            for &(cell, _) in &g.cells[..n] {
                let corners: [Option<[f32; 2]>; 8] = std::array::from_fn(|i| {
                    let p = glam::DVec3::new(
                        (cell.x as f64 + (i & 1) as f64) * VOXEL_SIZE,
                        (cell.y as f64 + ((i >> 1) & 1) as f64) * VOXEL_SIZE,
                        (cell.z as f64 + ((i >> 2) & 1) as f64) * VOXEL_SIZE,
                    ) - camera.position;
                    let q = matrix * p.as_vec3().extend(1.);
                    (q.w > 0.025).then_some([q.x / q.w, q.y / q.w])
                });
                for i in 0..8 {
                    for axis in [1, 2, 4] {
                        if i & axis == 0 {
                            if let (Some(a), Some(b)) = (corners[i], corners[i | axis]) {
                                out.lines.push([a, b]);
                            }
                        }
                    }
                }
            }
        }

        if let Some(held) = &self.pockets.hand {
            let mut voxels = Vec::new();
            match held {
                Item::Bowl => return out,
                Item::Seed => voxels.push((Cell { x: 0, y: 1, z: 0 }, [12, 12, 12])),
                Item::Sand(cells) => {
                    for (i, m) in cells.iter().enumerate() {
                        let color = if *m == Material::Sand {
                            [205, 172, 108]
                        } else {
                            [125, 88, 55]
                        };
                        voxels.push((
                            Cell {
                                x: (i % 2) as i64,
                                y: 1,
                                z: (i / 2) as i64,
                            },
                            color,
                        ));
                    }
                }
            }
            let occupied: std::collections::BTreeSet<_> = voxels.iter().map(|(c, _)| *c).collect();
            let rotation = glam::Mat3::IDENTITY;
            let view = glam::Vec3::new(1., 2., 1.);
            let mut faces = Vec::new();
            for (c, color) in voxels {
                for (indices, normal) in [
                    ([0, 1, 5, 4], [0, -1, 0]),
                    ([0, 4, 6, 2], [-1, 0, 0]),
                    ([1, 3, 7, 5], [1, 0, 0]),
                    ([4, 5, 7, 6], [0, 0, 1]),
                    ([0, 2, 3, 1], [0, 0, -1]),
                    ([2, 6, 7, 3], [0, 1, 0]),
                ] {
                    let neighbor = Cell {
                        x: c.x + normal[0],
                        y: c.y + normal[1],
                        z: c.z + normal[2],
                    };
                    if occupied.contains(&neighbor) {
                        continue;
                    }
                    let n = rotation
                        * glam::Vec3::new(normal[0] as f32, normal[1] as f32, normal[2] as f32);
                    if n.dot(view) <= 0. {
                        continue;
                    }
                    let points = indices.map(|i| {
                        rotation
                            * glam::Vec3::new(
                                c.x as f32 + (i & 1) as f32,
                                c.y as f32 + ((i >> 1) & 1) as f32 - 1.,
                                c.z as f32 + ((i >> 2) & 1) as f32,
                            )
                    });
                    let center = points.iter().copied().sum::<glam::Vec3>() * 0.25;
                    let shade =
                        0.6 + 0.4 * n.dot(glam::Vec3::new(-0.2, 0.9, 0.4).normalize()).max(0.);
                    faces.push((
                        center.dot(view),
                        points.map(|q| {
                            [
                                0.63 + (q.x - q.z) * 0.027 / aspect * 1.7,
                                -0.65 + q.y * 0.045 - (q.x + q.z) * 0.027,
                            ]
                        }),
                        color.map(|v| (v as f32 * shade) as u8),
                    ));
                }
            }
            faces.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (_, points, color) in faces {
                out.faces.push((points, color));
            }
        }
        out
    }
}
#[cfg(test)]
mod garden_save_tests {
    use super::*;
    use frontier_world::{desert, FbmParams, FbmRecipe};
    #[test]
    fn combined_garden_save_restores_water_holes_growth_and_native_tissue() {
        let p = FbmParams {
            recipe: FbmRecipe::Desert,
            ..Default::default()
        };
        let mut world = World::fbm(19, p).unwrap();
        let path = std::env::temp_dir().join(format!(
            "frontier-garden-{}-{}.bowl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut live = BowlLive::new(&mut world, Some(path.clone())).unwrap();
        let o = desert::oasis(19, 0, 0);
        let x = o.x + 60.;
        let z = o.z;
        let top = Cell::from_meters([x, world.ground_height_m(x, z) - VOXEL_SIZE, z]);
        let cells = Pockets::selection(&world, top);
        live.pockets.pickup(&mut world, &cells).unwrap();
        live.pockets
            .drop_sand(
                &mut world,
                Cell {
                    x: top.x + 5,
                    ..top
                },
                |_| true,
            )
            .unwrap();
        live.pockets.select(0).unwrap();
        live.pockets
            .drop_seed(&mut world, &mut live.garden, top, |_| true)
            .unwrap();
        let pile = *live
            .pockets
            .edits
            .iter()
            .filter(|(c, m)| c.x == top.x + 5 && **m != Material::Air)
            .max_by_key(|(c, _)| c.y)
            .unwrap()
            .0;
        let cells = Pockets::selection(&world, pile);
        live.pockets.pickup(&mut world, &cells).unwrap();
        live.pockets.drop_sand(&mut world, top, |_| true).unwrap();
        live.state
            .scoop(&mut world, [o.x, o.stage + 1., o.z], [0., -1., 0.])
            .unwrap();
        let eye = [x, world.ground_height_m(x, z) + 1.7, z];
        for _ in 0..960 {
            live.state
                .emit(&world, eye, [0., -1., 0.], 0.025, false, true);
            live.state.advance(&mut world, 0.025);
            live.garden
                .advance(&mut world, &mut live.state, 0.025, [0., 1., 0.]);
        }
        assert!(live.garden.cells() > 6);
        assert!(live.state.conserved());
        live.save(&world).unwrap();
        let mut restored_world = World::fbm(19, p).unwrap();
        let restored = BowlLive::new(&mut restored_world, Some(path.clone())).unwrap();
        assert_eq!(restored.state.snapshot(19), live.state.snapshot(19));
        assert_eq!(restored.garden.snapshot(), live.garden.snapshot());
        for cell in live.pockets.edits.keys() {
            assert_eq!(restored_world.material(*cell), world.material(*cell));
        }
        assert_eq!(
            restored.garden.water_accounted(),
            restored.state.plant_transferred
        );
        for plant in &restored.garden.plants {
            for (&cell, &tissue) in &plant.tissue {
                assert_eq!(restored_world.material(cell), tissue.material());
            }
        }
        assert!(crate::live_water::immutable_surface_world(&restored_world)
            .unwrap()
            .edit_cells()
            .next()
            .is_none());
        std::fs::remove_file(path).unwrap();
    }
}
