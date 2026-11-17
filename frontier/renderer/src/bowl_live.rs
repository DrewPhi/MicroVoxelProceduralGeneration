//! First-person bowl interaction and small voxel view model. Droplets use the
//! world-space native grid; the overlay projects them with canonical occlusion.
use crate::{Camera, Result};
use frontier_world::{
    bowl::{Bowl, CAPACITY},
    garden::{Garden, Tissue},
    Cell, Material, World, VOXEL_SIZE,
};
use std::{path::PathBuf, time::Instant};
#[derive(Clone, Default)]
pub struct Overlay {
    pub text: String,
    pub fill: f32,
    pub faces: Vec<([[f32; 2]; 4], [u8; 3])>,
}
pub struct BowlLive {
    pub state: Bowl,
    pub garden: Garden,
    pub equipped: bool,
    pub pouring: bool,
    pub tilt: f32,
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
            let (water, plants) = body
                .split_once("\nGARDEN1 ")
                .map_or((body, None), |(water, plants)| {
                    (water, Some(format!("GARDEN1 {plants}")))
                });
            state = Bowl::restore(water, world.seed())?;
            if let Some(plants) = plants {
                garden = Garden::restore(&plants, world.seed())?;
            }
            if garden.water_accounted() != state.plant_transferred {
                return Err("plant/water save ledger mismatch".into());
            }
            state.apply(world);
            garden.apply(world);
        }
        Ok(Self {
            state,
            garden,
            equipped: true,
            pouring: false,
            tilt: 0.,
            message: "Find the shoreline and scoop some water".into(),
            path,
            identity,
            debt: 0.,
            clock: Instant::now(),
            saves: 0,
        })
    }
    pub fn scoop(&mut self, world: &mut World, camera: Camera) {
        match self.state.scoop(
            world,
            camera.position.to_array(),
            camera.direction().as_dvec3().to_array(),
        ) {
            Ok(n) => {
                eprintln!(
                    "bowl scoop accepted units={n} held={} conserved={}",
                    self.state.units,
                    self.state.conserved()
                );
                self.message = format!(
                    "Scooped {:.2} L from the pond",
                    n as f64 * VOXEL_SIZE.powi(3) * 1000.
                )
            }
            Err(e) => self.message = e.into(),
        }
    }
    pub fn planting_action(&mut self, world: &mut World, camera: Camera, action: u8) {
        if self.equipped {
            self.message = "Press B to put away the bowl and use your hands".into();
            return;
        }
        let hit = frontier_world::raycast_solid(
            world,
            camera.position.to_array(),
            camera.direction().as_dvec3().to_array(),
            2.2,
        );
        let result = if let Some(hit) = hit {
            if action == 0 {
                self.garden.dig(world, hit.cell)
            } else if let Some(i) = self.garden.nearest_hole(hit.cell) {
                if action == 1 {
                    self.garden.plant(world, i)
                } else {
                    self.garden.cover(world, i)
                }
            } else {
                Err("Aim at your planting hole")
            }
        } else {
            Err("Aim at nearby ground within 2.2 m")
        };
        self.message = match result {
            Ok(()) => {
                let message = match action {
                    0 => "Dug a little hole — aim at it and press P to place your seed",
                    1 => "Seed placed — right click to cover it with sand",
                    _ => "Seed covered — water this patch with the bowl",
                };
                eprintln!(
                    "garden action={action} seeds={} holes={} plants={}",
                    self.garden.seeds,
                    self.garden.holes.len(),
                    self.garden.plants.len()
                );
                message.into()
            }
            Err(e) => e.into(),
        };
    }
    pub fn update(
        &mut self,
        world: &mut World,
        camera: Camera,
        spill: bool,
        active: bool,
        sun: [f64; 3],
    ) {
        let elapsed = self.clock.elapsed().as_secs_f64();
        self.clock = Instant::now();
        if !active {
            self.pouring = false;
            return;
        }
        self.debt += elapsed;
        let pouring = self.equipped && self.pouring;
        let spill = self.equipped && spill;
        let target = if spill {
            1.35
        } else if pouring {
            0.85
        } else {
            0.
        };
        self.tilt += (target - self.tilt) * (elapsed as f32 * 10.).min(1.);
        for _ in 0..16 {
            if self.debt < 0.025 {
                break;
            }
            self.debt -= 0.025;
            self.state.emit(
                world,
                camera.position.to_array(),
                camera.direction().as_dvec3().to_array(),
                0.025,
                spill,
                pouring || spill,
            );
            self.state.advance(world, 0.025);
            let before = self.garden.cells();
            self.garden.advance(world, &mut self.state, 0.025, sun);
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
        }
        if (pouring || spill) && self.state.units == 0 {
            self.message = "Bowl empty — the water is now in the world".into();
        }
    }
    pub fn save(&mut self, seed: u64) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = format!(
            "{}\n{}{}",
            self.identity,
            self.state.snapshot(seed),
            self.garden.snapshot()
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
        out.push_str(&format!(",\"plant_transferred\":{},\"garden\":{{\"seeds\":{},\"holes\":{},\"plants\":{},\"germinated\":{},\"tissue_cells\":{},\"leaves\":{},\"water_stored\":{},\"transpired\":{},\"tick\":{},\"water_ledger_matches\":{}}}}}",self.state.plant_transferred,self.garden.seeds,self.garden.holes.len(),self.garden.plants.len(),self.garden.plants.iter().filter(|p|p.germination==4).count(),self.garden.cells(),self.garden.plants.iter().flat_map(|p|p.tissue.values()).filter(|&&t|t==Tissue::Leaf).count(),self.garden.plants.iter().map(|p|p.water).sum::<u32>(),self.garden.plants.iter().map(|p|p.transpired).sum::<u64>(),self.garden.tick,self.garden.water_accounted()==self.state.plant_transferred));
        out
    }
    pub fn overlay(&self, world: &World, camera: Camera, aspect: f32) -> Overlay {
        let mut out=Overlay{text:format!("WOODEN BOWL   {:.2} / {:.2} L\nLeft click: scoop • Hold right: pour • Hold Q: tip/spill • B: equip\n{}",self.state.units as f64*VOXEL_SIZE.powi(3)*1000.,CAPACITY as f64*VOXEL_SIZE.powi(3)*1000.,self.message),fill:self.state.units as f32/CAPACITY as f32,..Default::default()};
        if !self.equipped {
            out.text=format!("HANDS • {} seed • {} loose sand cells • bowl {:.2} L\nLeft click: dig • P: place seed • Right click: cover • B: bowl\n{}",self.garden.seeds,self.garden.sand+self.garden.soil,self.state.units as f64*VOXEL_SIZE.powi(3)*1000.,self.message);
        }
        if let Some(p) = self.garden.plants.first() {
            let stage = if p.germination < 4 {
                if !self.garden.holes[p.hole].covered {
                    "Seed needs covering"
                } else if p.water == 0 {
                    "Buried seed — waiting for water"
                } else {
                    "Seed germinating"
                }
            } else if p.water == 0 && p.hydration == 0 {
                "Seedling thirsty — water near its base"
            } else if p.energy < 0.85 {
                "Seedling low on energy — needs sunlight"
            } else {
                "Seedling growing"
            };
            out.text.push_str(&format!(
                "\n{stage} • {} tissue cells • energy {:.1} • {} lit leaves",
                p.tissue.len(),
                p.energy,
                p.lit_leaves
            ));
        }
        let matrix = camera.matrix(camera.position, aspect);
        // Small bounded particle count: nearest-solid occlusion comes from canonical
        // cells, not approximate LOD depths. Each displayed droplet is a whole cube.
        let mut drops = self.state.drops.iter().collect::<Vec<_>>();
        drops.sort_by(|a, b| {
            let dist =
                |c: Cell| glam::DVec3::from_array(c.center_m()).distance_squared(camera.position);
            dist(b.cell).total_cmp(&dist(a.cell))
        });
        for d in drops {
            let center = glam::DVec3::from_array(d.cell.center_m());
            let delta = center - camera.position;
            let distance = delta.length();
            if distance > 0.
                && frontier_world::raycast(
                    world,
                    camera.position.to_array(),
                    delta.to_array(),
                    (distance - VOXEL_SIZE).max(0.),
                )
                .is_some()
            {
                continue;
            }
            cube(
                d.cell,
                [40, 139, 177],
                |p| {
                    let rel = glam::DVec3::from_array(p) - camera.position;
                    let v = matrix * rel.as_vec3().extend(1.);
                    (v.w > 0.025).then_some([v.x / v.w, v.y / v.w])
                },
                &mut out.faces,
            );
        }
        if self.equipped {
            let mut voxels = Vec::new();
            for y in 0..3 {
                for z in -2i64..=2 {
                    for x in -2i64..=2 {
                        let r = x * x + z * z;
                        if r <= 4 && (y == 0 || r > 1) {
                            voxels.push((Cell { x, y, z }, [139, 83, 42]));
                        }
                    }
                }
            }
            let mut water = 0;
            for y in 1..3 {
                for z in -1i64..=1 {
                    for x in -1i64..=1 {
                        if x * x + z * z <= 1 && water < self.state.units {
                            water += 1;
                            voxels.push((Cell { x, y, z }, [43, 153, 188]));
                        }
                    }
                }
            }
            let occupied: std::collections::BTreeSet<_> = voxels.iter().map(|(c, _)| *c).collect();
            let rotation = glam::Mat3::from_rotation_x(self.tilt);
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
fn cube(
    c: Cell,
    color: [u8; 3],
    project: impl Fn([f64; 3]) -> Option<[f32; 2]>,
    out: &mut Vec<([[f32; 2]; 4], [u8; 3])>,
) {
    let points: [Option<[f32; 2]>; 8] = std::array::from_fn(|i| {
        project([
            (c.x as f64 + (i & 1) as f64) * VOXEL_SIZE,
            (c.y as f64 + ((i >> 1) & 1) as f64) * VOXEL_SIZE,
            (c.z as f64 + ((i >> 2) & 1) as f64) * VOXEL_SIZE,
        ])
    });
    // Complete cube faces; painter order is sufficient for this tiny view model.
    for (indices, shade) in [
        ([0, 1, 5, 4], 0.65),
        ([0, 4, 6, 2], 0.75),
        ([1, 3, 7, 5], 0.85),
        ([4, 5, 7, 6], 0.8),
        ([0, 2, 3, 1], 0.9),
        ([2, 6, 7, 3], 1.),
    ] {
        if let [Some(a), Some(b), Some(c), Some(d)] = indices.map(|i| points[i]) {
            let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            if area > 0. {
                out.push(([a, b, c, d], color.map(|v| (v as f32 * shade) as u8)));
            }
        }
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
        live.garden.dig(&mut world, top).unwrap();
        live.garden.plant(&mut world, 0).unwrap();
        live.garden.cover(&mut world, 0).unwrap();
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
        live.save(19).unwrap();
        let mut restored_world = World::fbm(19, p).unwrap();
        let restored = BowlLive::new(&mut restored_world, Some(path.clone())).unwrap();
        assert_eq!(restored.state.snapshot(19), live.state.snapshot(19));
        assert_eq!(restored.garden.snapshot(), live.garden.snapshot());
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
