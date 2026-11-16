//! Opt-in local fluid driver. Authoritative fluid history is never a render cache.
use crate::{Camera, Result};
use frontier_world::{
    edit::{EditTransaction, Inventory},
    fluid::{FluidConfig, FluidRegion, FluidWorld, FLUID_CAPACITY},
    Cell, Material, World, VOXEL_SIZE,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::Instant,
};

// One metre at the authoritative 6.25 cm voxel resolution.
const REGION_EDGE_CELLS: i64 = 16;
const REGIONS_PER_FRAME: usize = 2;
const STEP_SECONDS: f64 = 0.1;

/// Shared by the simulation allocation and CLI history-budget validation.
pub(crate) fn active_cell_capacity(radius: u32) -> usize {
    (radius as usize * 2 + 1).pow(3) * (REGION_EDGE_CELLS as usize).pow(3)
}

/// One immutable far-renderer source, constructed once at viewer startup. Native
/// rendering, picking, collision and persistence continue using the live world.
pub fn immutable_surface_world(world: &World) -> Result<World> {
    let source = world
        .fbm_terrain()
        .ok_or("live water requires FBM terrain")?;
    Ok(World::fbm(world.seed(), source.params())?
        .with_vegetation_enabled(world.vegetation_enabled()))
}

pub struct LiveWater {
    pub fluid: FluidWorld,
    pub inventory: Inventory,
    path: Option<PathBuf>,
    active_radius: i64,
    clock: Instant,
    debt: f64,
    reported: Instant,
    pub demo_center: Option<Cell>,
    demo_open_at: Option<u64>,
    pub last_step_ms: f64,
    pub activation_error: Option<String>,
}
fn solid(world: &World, c: Cell) -> bool {
    !matches!(
        world.material_without_fluid(c),
        Material::Air | Material::Water
    )
}
fn region(p: [i64; 3]) -> FluidRegion {
    FluidRegion::new(
        Cell {
            x: p[0] * REGION_EDGE_CELLS,
            y: p[1] * REGION_EDGE_CELLS,
            z: p[2] * REGION_EDGE_CELLS,
        },
        Cell {
            x: (p[0] + 1) * REGION_EDGE_CELLS,
            y: (p[1] + 1) * REGION_EDGE_CELLS,
            z: (p[2] + 1) * REGION_EDGE_CELLS,
        },
    )
}
impl LiveWater {
    pub fn new(
        world: &mut World,
        path: Option<PathBuf>,
        active_radius: u32,
        history_cells: usize,
    ) -> Result<Self> {
        let now = Instant::now();
        let mut live = Self {
            fluid: FluidWorld::new(FluidConfig {
                max_loaded_cells: active_cell_capacity(active_radius),
                max_stored_cells: history_cells,
                sleep_ticks: 4,
            }),
            inventory: Inventory::default(),
            path,
            active_radius: active_radius as i64,
            clock: now,
            debt: 0.,
            reported: now,
            demo_center: None,
            demo_open_at: None,
            last_step_ms: 0.,
            activation_error: None,
        };
        if let Some(path) = &live.path {
            if path.exists() {
                let bytes = std::fs::read(path)?;
                live.restore(world, &bytes)?;
                eprintln!(
                    "live water restored {} cells, tick {}",
                    live.fluid.diagnostics().stored_cells,
                    live.fluid.tick()
                );
            }
        }
        Ok(live)
    }
    /// At most two one-metre regions per frame. Absent neighbors are sealed.
    fn activate(&mut self, world: &World, camera: Camera) -> Result<()> {
        let feet = Cell::from_meters([
            camera.position.x,
            camera.position.y - 1.7,
            camera.position.z,
        ]);
        let center = self
            .demo_center
            .filter(|c| (c.x - feet.x).abs() < 96 && (c.z - feet.z).abs() < 96)
            .unwrap_or(feet);
        let p = [
            center.x.div_euclid(REGION_EDGE_CELLS),
            center.y.div_euclid(REGION_EDGE_CELLS),
            center.z.div_euclid(REGION_EDGE_CELLS),
        ];
        let mut desired = BTreeSet::new();
        for z in -self.active_radius..=self.active_radius {
            for x in -self.active_radius..=self.active_radius {
                for y in -self.active_radius..=self.active_radius {
                    desired.insert(region([p[0] + x, p[1] + y, p[2] + z]));
                }
            }
        }
        let old: Vec<_> = self.fluid.loaded_regions().collect();
        for r in old {
            if !desired.contains(&r) {
                self.fluid.unload_region(r);
            }
        }
        let old: BTreeSet<_> = self.fluid.loaded_regions().collect();
        // Nearest regions first gives immediate local response during activation.
        let mut missing: Vec<_> = desired.difference(&old).copied().collect();
        missing.sort_by_key(|r| {
            (r.min.x / REGION_EDGE_CELLS - p[0]).abs()
                + (r.min.y / REGION_EDGE_CELLS - p[1]).abs()
                + (r.min.z / REGION_EDGE_CELLS - p[2]).abs()
        });
        for r in missing.into_iter().take(REGIONS_PER_FRAME) {
            let mut columns = BTreeMap::new();
            for x in r.min.x..r.max.x {
                for z in r.min.z..r.max.z {
                    columns.insert((x, z), world.column(x, z));
                }
            }
            self.fluid.load_region(r, |c| {
                if columns[&(c.x, c.z)].material(c.y) == Material::Water {
                    FLUID_CAPACITY
                } else {
                    0
                }
            })?;
        }
        Ok(())
    }
    fn publish(&self, world: &mut World, cells: impl IntoIterator<Item = Cell>) {
        let values: Vec<_> = cells
            .into_iter()
            .filter_map(|c| self.fluid.mass(c).map(|m| (c, m > 0)))
            .collect();
        world.set_fluid_occupancy(values);
    }
    pub fn wait_for_render(&mut self) {
        self.clock = Instant::now();
        self.debt = 0.;
    }
    pub fn update(&mut self, world: &mut World, camera: Camera) -> Result<()> {
        let elapsed = self.clock.elapsed().as_secs_f64();
        self.clock = Instant::now();
        self.debt = (self.debt + elapsed).min(0.5);
        if let Err(e) = self.activate(world, camera) {
            let message = e.to_string();
            if self.activation_error.as_ref() != Some(&message) {
                eprintln!("live water activation paused: {message}; dormant history retained");
            }
            self.activation_error = Some(message);
        } else {
            self.activation_error = None;
        }
        if self
            .demo_open_at
            .is_some_and(|tick| self.fluid.tick() >= tick)
        {
            self.open_dam(world);
        }
        if self.debt >= STEP_SECONDS {
            if self.fluid.tick() == 0 {
                eprintln!("water simulation started after initial native publication");
            }
            let start = Instant::now();
            let mut columns = BTreeMap::new();
            let step = self.fluid.step(|c| {
                let m = columns
                    .entry((c.x, c.z))
                    .or_insert_with(|| world.column(c.x, c.z))
                    .canonical_material_without_fluid(c.y);
                !matches!(m, Material::Air | Material::Water)
            });
            drop(columns);
            self.publish(world, step.changed);
            self.last_step_ms = start.elapsed().as_secs_f64() * 1000.;
            self.debt -= STEP_SECONDS;
        }
        if self.reported.elapsed().as_secs() >= 5 {
            let d = self.fluid.diagnostics();
            eprintln!("water tick={} step={:.2}ms resident={} dormant={} displaced={} loaded={} awake={} conserved={}",d.tick,self.last_step_ms,d.resident_mass,d.dormant_mass,d.displaced_mass,d.loaded_cells,d.awake_cells,d.conserved());
            self.reported = Instant::now();
        }
        Ok(())
    }
    pub fn edit(&mut self, world: &mut World, c: Cell, material: Material) {
        world.set(c, material);
        self.fluid.notify_edit(c, solid(world, c));
        self.publish(world, [c]);
    }
    pub fn apply_player_transaction(
        &mut self,
        world: &mut World,
        transaction: &EditTransaction,
        owns: impl Fn(Cell) -> bool,
    ) -> std::result::Result<(), &'static str> {
        if transaction.place.is_some()
            && transaction.expected.iter().any(|&(c, _)| {
                self.fluid.mass(c).is_some_and(|m| m > 0) || world.material(c) == Material::Water
            })
        {
            return Err("Placement into water needs supported displacement");
        }
        let changed = transaction.apply(world, &mut self.inventory, owns)?;
        for c in &changed {
            self.fluid.notify_edit(*c, solid(world, *c));
        }
        self.publish(world, changed);
        Ok(())
    }
    /// A sealed two-chamber native-cell basin, with a removable central dam.
    /// Initial water is an explicitly counted addition, never an infinite source.
    pub fn demo(&mut self, world: &mut World, camera: &mut Camera) -> Result<()> {
        let x = (camera.position.x / VOXEL_SIZE).floor() as i64;
        let z = (camera.position.z / VOXEL_SIZE).floor() as i64;
        let y = (world.ground_height_m(camera.position.x, camera.position.z) / VOXEL_SIZE).round()
            as i64
            + 2;
        // Anchor a 2x1x1m basin on a metre grid for reproducible activation.
        let c = Cell {
            x: x.div_euclid(16) * 16,
            y: y.div_euclid(16) * 16 + 16,
            z: z.div_euclid(16) * 16,
        };
        for dx in -16..=16 {
            for dz in -8..=8 {
                for dy in -1..=12 {
                    let p = Cell {
                        x: c.x + dx,
                        y: c.y + dy,
                        z: c.z + dz,
                    };
                    let wall = dy == -1 || dx.abs() == 16 || dz.abs() == 8 || dx == 0;
                    world.set(p, if wall { Material::Stone } else { Material::Air });
                }
            }
        }
        // Activate the complete demo footprint before adding mass.
        let mut demo_camera = *camera;
        demo_camera.position = glam::DVec3::new(
            c.x as f64 * VOXEL_SIZE,
            (c.y as f64 * VOXEL_SIZE) + 1.7,
            c.z as f64 * VOXEL_SIZE,
        );
        self.demo_center = Some(c);
        self.demo_open_at = Some(self.fluid.tick() + 20);
        for _ in 0..14 {
            self.activate(world, demo_camera)?;
        }
        let mut cells = Vec::new();
        for dx in -15..0 {
            for dz in -7..8 {
                for dy in 0..8 {
                    let p = Cell {
                        x: c.x + dx,
                        y: c.y + dy,
                        z: c.z + dz,
                    };
                    self.fluid.set_mass(p, FLUID_CAPACITY)?;
                    cells.push(p);
                }
            }
        }
        self.publish(world, cells);
        camera.position = glam::DVec3::new(
            c.x as f64 * VOXEL_SIZE,
            c.y as f64 * VOXEL_SIZE + 2.2,
            c.z as f64 * VOXEL_SIZE - 2.,
        );
        camera.yaw = 0.;
        camera.pitch = -0.7;
        eprintln!("water demo basin at {:?}; dam opens at tick 20 (G opens early); left/right mouse remove/place terrain; P adds one water cell",c);
        Ok(())
    }
    pub fn open_dam(&mut self, world: &mut World) {
        self.demo_open_at = None;
        if let Some(c) = self.demo_center {
            eprintln!("water demo dam opened at tick {}", self.fluid.tick());
            for dz in -7..8 {
                for dy in 0..=12 {
                    self.edit(
                        world,
                        Cell {
                            x: c.x,
                            y: c.y + dy,
                            z: c.z + dz,
                        },
                        Material::Air,
                    );
                }
            }
        }
    }
    pub fn add_water(&mut self, world: &mut World, c: Cell) -> Result<()> {
        if self.fluid.is_loaded(c) && !solid(world, c) {
            self.fluid.set_mass(c, FLUID_CAPACITY)?;
            self.publish(world, [c]);
        }
        Ok(())
    }
    fn identity(world: &World) -> Vec<u64> {
        let t = world.fbm_terrain().expect("live water requires FBM");
        let p = t.params();
        vec![
            world.seed(),
            p.recipe as u64,
            t.source_version() as u64,
            p.horizontal_scale.to_bits(),
            p.amplitude.to_bits(),
            p.octaves as u64,
            p.roughness.to_bits(),
            world.vegetation_enabled() as u64,
        ]
    }
    pub fn save(&self, world: &World) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let bytes = self.checkpoint(world);
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("water-save.tmp");
        let mut f = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut f, &bytes)?;
        f.sync_all()?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
    fn checkpoint(&self, world: &World) -> Vec<u8> {
        let mut b = b"FVWATER3".to_vec();
        for v in Self::identity(world) {
            b.extend(v.to_le_bytes());
        }
        let edits: Vec<_> = world.terrain_edits().collect();
        b.extend((edits.len() as u64).to_le_bytes());
        for (c, m) in edits {
            for v in [c.x, c.y, c.z] {
                b.extend(v.to_le_bytes());
            }
            b.push(m as u8);
        }
        b.push(self.demo_center.is_some() as u8);
        if let Some(c) = self.demo_center {
            for v in [c.x, c.y, c.z] {
                b.extend(v.to_le_bytes());
            }
        }
        b.extend(self.demo_open_at.unwrap_or(u64::MAX).to_le_bytes());
        b.extend(self.inventory.encode());
        b.extend(self.fluid.persist_bytes());
        b
    }
    fn restore(&mut self, world: &mut World, b: &[u8]) -> Result<()> {
        let mut pos = 8;
        let legacy = b.get(..8) == Some(b"FVWATER2");
        if !legacy && b.get(..8) != Some(b"FVWATER3") {
            return Err("invalid water save header".into());
        }
        fn read(b: &[u8], p: &mut usize) -> Result<u64> {
            let v = u64::from_le_bytes(
                b.get(*p..*p + 8)
                    .ok_or("truncated water save")?
                    .try_into()?,
            );
            *p += 8;
            Ok(v)
        }
        for expected in Self::identity(world) {
            if read(b, &mut pos)? != expected {
                return Err("water save source/seed mismatch".into());
            }
        }
        let n = read(b, &mut pos)?;
        if n > 1_048_576 {
            return Err("water save terrain edit limit exceeded".into());
        }
        let materials = [
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
        let mut edits = Vec::new();
        for _ in 0..n {
            let c = Cell {
                x: read(b, &mut pos)? as i64,
                y: read(b, &mut pos)? as i64,
                z: read(b, &mut pos)? as i64,
            };
            let id = *b.get(pos).ok_or("truncated material")? as usize;
            pos += 1;
            edits.push((c, *materials.get(id).ok_or("invalid saved material")?));
        }
        let demo = *b.get(pos).ok_or("truncated demo flag")?;
        pos += 1;
        self.demo_center = match demo {
            0 => None,
            1 => Some(Cell {
                x: read(b, &mut pos)? as i64,
                y: read(b, &mut pos)? as i64,
                z: read(b, &mut pos)? as i64,
            }),
            _ => return Err("invalid demo flag".into()),
        };
        let open_at = read(b, &mut pos)?;
        self.demo_open_at = (open_at != u64::MAX).then_some(open_at);
        let inventory = if legacy {
            Inventory::default()
        } else {
            let bytes = b.get(pos..pos + 104).ok_or("truncated inventory")?;
            pos += 104;
            Inventory::decode(bytes).ok_or("invalid saved inventory")?
        };
        let mut fluid = FluidWorld::from_bytes(&b[pos..])?;
        let config = self.fluid.config();
        // Residency is a runtime window, while overrides (including dry
        // tombstones) are authoritative history. A smaller restart window must
        // deactivate the old window before its capacity can be applied.
        if fluid.diagnostics().loaded_cells > config.max_loaded_cells {
            let regions: Vec<_> = fluid.loaded_regions().collect();
            for region in regions {
                fluid.unload_region(region);
            }
        }
        fluid.set_config(config)?;
        world.set_batch(edits);
        let occupancy: Vec<_> = fluid
            .overrides()
            .filter_map(|(c, m)| {
                let wet = m > 0;
                (wet || world.material_without_fluid(c) == Material::Water).then_some((c, wet))
            })
            .collect();
        world.set_fluid_occupancy(occupancy);
        self.fluid = fluid;
        self.inventory = inventory;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_checkpoint_replays_terrain_fluid_and_dry_tombstones() {
        let mut world = World::fbm(19, Default::default()).unwrap();
        let mut live = LiveWater::new(&mut world, None, 1, 1_048_576).unwrap();
        let y = (world.height_m(0., 0.) / VOXEL_SIZE) as i64 + 20;
        let r = FluidRegion::new(
            Cell { x: 0, y, z: 0 },
            Cell {
                x: 3,
                y: y + 3,
                z: 1,
            },
        );
        live.fluid.load_region(r, |_| 0).unwrap();
        let source = Cell {
            x: 0,
            y: y + 2,
            z: 0,
        };
        live.fluid.set_mass(source, FLUID_CAPACITY).unwrap();
        live.publish(&mut world, [source]);
        live.edit(&mut world, Cell { x: 2, y, z: 0 }, Material::Stone);
        let step = live.fluid.step(|c| solid(&world, c));
        live.publish(&mut world, step.changed);
        let saved = live.checkpoint(&world);
        let mut other = World::fbm(19, Default::default()).unwrap();
        let mut restored = LiveWater::new(&mut other, None, 1, 1_048_576).unwrap();
        restored.restore(&mut other, &saved).unwrap();
        assert_eq!(live.fluid.persist_bytes(), restored.fluid.persist_bytes());
        for (c, _) in live.fluid.overrides() {
            assert_eq!(world.material(c), other.material(c));
        }
        assert_eq!(other.material(Cell { x: 2, y, z: 0 }), Material::Stone);
        assert_eq!(other.material(source), Material::Air);
        for _ in 0..8 {
            let a = live.fluid.step(|c| solid(&world, c));
            live.publish(&mut world, a.changed);
            let b = restored.fluid.step(|c| solid(&other, c));
            restored.publish(&mut other, b.changed);
            assert_eq!(live.fluid.persist_bytes(), restored.fluid.persist_bytes());
        }
        assert!(restored.fluid.diagnostics().conserved());
        let mut wrong = World::fbm(20, Default::default()).unwrap();
        assert!(restored.restore(&mut wrong, &saved).is_err());
    }
    #[test]
    fn player_pickup_inventory_sequence_and_world_share_checkpoint_and_legacy_migrates() {
        let mut world = World::fbm(19, Default::default()).unwrap();
        let mut live = LiveWater::new(&mut world, None, 1, 1_048_576).unwrap();
        let cell = Cell {
            x: -4097,
            y: 16000,
            z: -8193,
        };
        world.set(cell, Material::Soil);
        let pickup = EditTransaction::pickup(&live.inventory, vec![(cell, Material::Soil)]);
        let revision = world.revision();
        live.apply_player_transaction(&mut world, &pickup, |_| true)
            .unwrap();
        assert_eq!(world.revision(), revision + 1);
        assert_eq!(live.inventory.total(), 1);
        let saved = live.checkpoint(&world);
        let mut other = World::fbm(19, Default::default()).unwrap();
        let mut restored = LiveWater::new(&mut other, None, 1, 1_048_576).unwrap();
        restored.restore(&mut other, &saved).unwrap();
        assert_eq!(restored.inventory, live.inventory);
        assert_eq!(other.material(cell), Material::Air);
        assert!(restored
            .apply_player_transaction(&mut other, &pickup, |_| true)
            .is_err());
        let place = EditTransaction::placement(&restored.inventory, cell, Material::Air);
        restored
            .apply_player_transaction(&mut other, &place, |_| true)
            .unwrap();
        assert_eq!(restored.inventory.total(), 0);
        assert_eq!(restored.inventory.sequence, 2);
        assert_eq!(other.material(cell), Material::Soil);
        // V2 contained the same terrain/fluid payload but no inventory block.
        let mut legacy = saved;
        legacy[..8].copy_from_slice(b"FVWATER2");
        let at = legacy.len() - live.fluid.persist_bytes().len() - 104;
        legacy.drain(at..at + 104);
        let mut old_world = World::fbm(19, Default::default()).unwrap();
        let mut migrated = LiveWater::new(&mut old_world, None, 1, 1_048_576).unwrap();
        migrated.restore(&mut old_world, &legacy).unwrap();
        assert_eq!(migrated.inventory, Inventory::default());
        assert_eq!(old_world.material(cell), Material::Air);
    }
    #[test]
    fn wet_player_placement_rejects_without_mutating_inventory_terrain_or_water() {
        let mut world = World::fbm(19, Default::default()).unwrap();
        let mut live = LiveWater::new(&mut world, None, 1, 1_048_576).unwrap();
        let cell = Cell {
            x: -4097,
            y: 16000,
            z: -8193,
        };
        live.fluid
            .load_region(
                FluidRegion::new(
                    cell,
                    Cell {
                        x: cell.x + 1,
                        y: cell.y + 1,
                        z: cell.z + 1,
                    },
                ),
                |_| FLUID_CAPACITY,
            )
            .unwrap();
        live.publish(&mut world, [cell]);
        live.inventory.counts[Material::Stone as usize] = 1;
        let before = live.checkpoint(&world);
        let txn = EditTransaction::placement(&live.inventory, cell, Material::Air);
        assert!(live
            .apply_player_transaction(&mut world, &txn, |_| true)
            .is_err());
        assert_eq!(before, live.checkpoint(&world));
        assert!(live.fluid.diagnostics().conserved());
    }
    #[test]
    fn smaller_restart_window_preserves_dormant_mass_and_dry_tombstones() {
        let mut world = World::fbm(19, Default::default()).unwrap();
        let mut live = LiveWater::new(&mut world, None, 2, 1_048_576).unwrap();
        let first = region([-14, 100, -2]);
        let wet = first.min;
        let dry = Cell {
            x: wet.x + 1,
            ..wet
        };
        let displaced = Cell {
            x: wet.x + 2,
            ..wet
        };
        world.set(wet, Material::Air);
        world.set(displaced, Material::Air);
        world.set(dry, Material::Water);
        // 28 metre regions exceed radius one's 27-region capacity.
        for x in -14..14 {
            live.fluid
                .load_region(region([x, 100, -2]), |c| {
                    if c == wet || c == dry || c == displaced {
                        FLUID_CAPACITY
                    } else {
                        0
                    }
                })
                .unwrap();
        }
        live.fluid.set_mass(dry, 0).unwrap();
        live.edit(&mut world, displaced, Material::Stone);
        live.publish(&mut world, [wet, dry]);
        let before = live.fluid.diagnostics();
        assert!(before.loaded_cells > active_cell_capacity(1));
        let overrides: Vec<_> = live.fluid.overrides().collect();
        let saved = live.checkpoint(&world);
        let mut other = World::fbm(19, Default::default()).unwrap();
        let mut restored = LiveWater::new(&mut other, None, 1, 1_048_576).unwrap();
        restored.restore(&mut other, &saved).unwrap();
        let after = restored.fluid.diagnostics();
        assert_eq!(after.loaded_cells, 0);
        assert_eq!(after.resident_mass, 0);
        assert_eq!(
            after.dormant_mass,
            before.resident_mass + before.dormant_mass
        );
        assert_eq!(after.displaced_mass, before.displaced_mass);
        assert_eq!(after.initial_mass, before.initial_mass);
        assert_eq!(after.additions, before.additions);
        assert_eq!(after.removals, before.removals);
        assert_eq!(after.tick, before.tick);
        assert_eq!(after.stored_cells, before.stored_cells);
        assert!(after.conserved());
        assert_eq!(overrides, restored.fluid.overrides().collect::<Vec<_>>());
        assert_eq!(other.material(wet), Material::Water);
        assert_eq!(other.material(dry), Material::Air);
        assert_eq!(other.material(displaced), Material::Stone);
        restored
            .fluid
            .load_region(first, |_| FLUID_CAPACITY)
            .unwrap();
        assert_eq!(restored.fluid.mass(dry), Some(0));
        assert_eq!(restored.fluid.mass(wet), Some(FLUID_CAPACITY));
        assert_eq!(overrides, restored.fluid.overrides().collect::<Vec<_>>());
        assert!(restored.fluid.diagnostics().conserved());
    }
    #[test]
    fn demo_dam_opens_and_real_conserved_mass_crosses_into_empty_chamber() {
        let mut world = World::fbm(
            19,
            frontier_world::FbmParams {
                octaves: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let mut live = LiveWater::new(&mut world, None, 1, 1_048_576).unwrap();
        let mut camera = Camera::fixture();
        camera.position.x = 0.;
        camera.position.z = 0.;
        live.demo(&mut world, &mut camera).unwrap();
        let center = live.demo_center.unwrap();
        let right_mass = |fluid: &FluidWorld| {
            fluid
                .overrides()
                .filter(|(c, _)| {
                    c.x > center.x
                        && c.x < center.x + 16
                        && c.z > center.z - 8
                        && c.z < center.z + 8
                })
                .map(|(_, m)| m as u64)
                .sum::<u64>()
        };
        assert_eq!(right_mass(&live.fluid), 0);
        assert_eq!(world.material(center), Material::Stone);
        let far = immutable_surface_world(&world).unwrap();
        assert_eq!(far.edit_cells().count(), 0);
        let _view = crate::adaptive_view::AdaptiveView::new(
            far,
            5.,
            16 * 1048576,
            true,
            1,
            1,
            false,
            64 * 1048576,
            None,
        )
        .unwrap();
        for _ in 0..24 {
            live.debt = STEP_SECONDS;
            live.update(&mut world, camera).unwrap();
        }
        assert_eq!(world.material_without_fluid(center), Material::Air);
        assert_eq!(
            world.material_without_fluid(Cell {
                y: center.y + 12,
                ..center
            }),
            Material::Air
        );
        assert!(right_mass(&live.fluid) > 0);
        assert!(live.fluid.diagnostics().conserved());
        let (wet, _) = live
            .fluid
            .overrides()
            .find(|(c, m)| c.x > center.x && *m > 0)
            .unwrap();
        assert_eq!(world.material(wet), Material::Water);
        assert_eq!(world.column(wet.x, wet.z).material(wet.y), Material::Water);
    }
}
