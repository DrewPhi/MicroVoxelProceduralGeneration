//! Grid-aligned terrain proxies and exact native vertical-run surface extraction.
//! Coarse cells use a bounded five-column median summary, not an exact aggregate
//! of every buried cell. Only the display changes; world data remains native.
use super::{
    lod::{MeshKey, EXTENT, FINEST, SIDE},
    surface::{FaceCollector, Surface},
};
use crate::{
    generation::{Generator, Run},
    grid::CellPos,
    materials::{MaterialId, Voxel},
    painting,
    world::World,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};
const CEILING: i64 = 32_768;
const CACHE_LIMIT: usize = 16_384;
#[derive(Clone, Debug)]
pub struct SurfacePart {
    pub step: i64,
    pub surface: Surface,
}
#[derive(Clone, Debug)]
pub struct TerrainMesh {
    pub origin: [i64; 3],
    pub parts: Vec<SurfacePart>,
}
impl TerrainMesh {
    pub fn bytes(&self) -> usize {
        self.parts.iter().map(|p| p.surface.bytes()).sum()
    }
    pub fn quads(&self) -> usize {
        self.parts.iter().map(|p| p.surface.quads.len()).sum()
    }
    pub fn faces(&self) -> usize {
        self.parts.iter().map(|p| p.surface.unit_faces).sum()
    }
    pub fn heights(&self) -> [i64; 2] {
        [
            self.parts
                .iter()
                .map(|p| p.surface.min_y)
                .min()
                .unwrap_or(0),
            self.parts
                .iter()
                .map(|p| p.surface.max_y)
                .max()
                .unwrap_or(0),
        ]
    }
}
/// Each worker owns its profile cache. No shared mesh-generation lock is needed.
pub struct Sampler<G: Generator> {
    world: Arc<World<G>>,
    profiles: HashMap<(i64, i64, i64), Arc<Vec<Run>>>,
    fifo: VecDeque<(i64, i64, i64)>,
    revision: u64,
}
impl<G: Generator> Sampler<G> {
    pub fn new(world: Arc<World<G>>) -> Self {
        let revision = world.revision();
        Self {
            world,
            profiles: HashMap::new(),
            fifo: VecDeque::new(),
            revision,
        }
    }
    pub fn world(&self) -> &World<G> {
        &self.world
    }
    fn profile(&mut self, x: i64, z: i64, step: i64) -> Arc<Vec<Run>> {
        if self.revision != self.world.revision() {
            self.profiles.clear();
            self.fifo.clear();
            self.revision = self.world.revision();
        }
        let x = x.div_euclid(step) * step;
        let z = z.div_euclid(step) * step;
        let key = (x, z, step);
        if let Some(r) = self.profiles.get(&key) {
            return Arc::clone(r);
        }
        let runs = if !(0..EXTENT).contains(&x) || !(0..EXTENT).contains(&z) {
            vec![Run {
                start: 0,
                end: CEILING,
                material: MaterialId::AIR,
            }]
        } else if step == 1 {
            self.world.column_runs(x, z, 0..CEILING).unwrap_or_else(|| {
                vec![Run {
                    start: 0,
                    end: CEILING,
                    material: MaterialId::AIR,
                }]
            })
        } else {
            let offsets = [
                (step / 2, step / 2),
                (step / 4, step / 4),
                (3 * step / 4, step / 4),
                (step / 4, 3 * step / 4),
                (3 * step / 4, 3 * step / 4),
            ];
            let mut samples: Vec<_> = offsets
                .into_iter()
                .filter_map(|(dx, dz)| {
                    self.world
                        .surface_column((x + dx).min(EXTENT - 1), (z + dz).min(EXTENT - 1))
                })
                .collect();
            samples.sort_by_key(|c| c.ground);
            if samples.is_empty() {
                vec![Run {
                    start: 0,
                    end: CEILING,
                    material: MaterialId::AIR,
                }]
            } else {
                let c = samples[samples.len() / 2];
                let quantize = |v: i64| ((v + step / 2).div_euclid(step) * step).clamp(0, CEILING);
                let ground = quantize(c.ground);
                let mut wet: Vec<_> = samples
                    .iter()
                    .filter(|c| c.water > c.ground)
                    .map(|c| c.water)
                    .collect();
                wet.sort_unstable();
                let water = if wet.len() > samples.len() / 2 {
                    quantize(wet[wet.len() / 2]).max(ground)
                } else {
                    ground
                };
                let surface_start = (ground - step).max(0);
                let bedrock = quantize(c.bedrock).min(surface_start);
                let mut runs = Vec::new();
                for (start, end, material) in [
                    (0, bedrock, MaterialId::ROCK),
                    (bedrock, surface_start, c.subsoil),
                    (surface_start, ground, c.surface),
                    (ground, water, MaterialId::WATER),
                    (water, CEILING, MaterialId::AIR),
                ] {
                    crate::generation::push_run(
                        &mut runs,
                        Run {
                            start,
                            end,
                            material,
                        },
                    );
                }
                runs
            }
        };
        let runs = Arc::new(runs);
        if self.profiles.len() >= CACHE_LIMIT {
            if let Some(old) = self.fifo.pop_front() {
                self.profiles.remove(&old);
            }
        }
        self.fifo.push_back(key);
        self.profiles.insert(key, Arc::clone(&runs));
        runs
    }
    fn color(&self, p: CellPos, material: MaterialId, native: bool) -> [u8; 3] {
        let voxel = if native {
            self.world.voxel_at(p).known().unwrap_or(Voxel::AIR)
        } else {
            Voxel::new(
                material,
                painting::appearance(p, self.world.identity().seed),
            )
        };
        painting::base_color(voxel, self.world.materials())
    }
    pub fn mesh(&mut self, key: MeshKey) -> Result<TerrainMesh, String> {
        if key.tile.level > FINEST
            || key.tile.x >= 1u32.checked_shl(key.tile.level as u32).unwrap_or(0)
            || key.tile.z >= 1u32.checked_shl(key.tile.level as u32).unwrap_or(0)
            || key
                .neighbors
                .iter()
                .any(|&n| n > FINEST || n.abs_diff(key.tile.level) > 1)
        {
            return Err("unbalanced LOD mesh request".into());
        }
        if key.revision != self.world.revision() {
            return Err("stale world mesh request".into());
        }
        let step = key.tile.step();
        let [ox, oz] = key.tile.origin();
        let origin = [ox, 0, oz];
        let mut faces = FaceCollector::default();
        let mut stitches = FaceCollector::default();
        for iz in 0..SIDE {
            for ix in 0..SIDE {
                let x = ox + ix * step;
                let z = oz + iz * step;
                if x >= EXTENT || z >= EXTENT {
                    continue;
                }
                let own = self.profile(x, z, step);
                for run in own.iter().filter(|r| r.material != MaterialId::AIR) {
                    // Only boundaries against air are visible; buried solid/fluid
                    // interfaces have no geometry. Native profiles retain cavities.
                    for (plane, cell_y, face, outside) in [
                        (run.end, run.end - 1, 0, run.end),
                        (run.start, run.start, 1, run.start - 1),
                    ] {
                        if plane == 0 || material_at(&own, outside) != MaterialId::AIR {
                            continue;
                        }
                        let p = CellPos::new(x, cell_y, z);
                        let color = self.color(p, run.material, step == 1);
                        faces.add(face, [x / step, plane / step, z / step], color)?;
                    }
                    for side in 0..4 {
                        let at_boundary = match side {
                            0 => ix == 0,
                            1 => ix == SIDE - 1,
                            2 => iz == 0,
                            _ => iz == SIDE - 1,
                        };
                        let neighbor_step = if at_boundary {
                            1i64 << (FINEST - key.neighbors[side])
                        } else {
                            step
                        };
                        let substep = step.min(neighbor_step);
                        let segments = step / substep;
                        for segment in 0..segments {
                            let offset = segment * substep;
                            let (nx, nz) = match side {
                                0 => (x - 1, z + offset),
                                1 => (x + step, z + offset),
                                2 => (x + offset, z - 1),
                                _ => (x + offset, z + step),
                            };
                            // Unknown territory is not air. Do not expose or scan
                            // the entire buried world boundary as a giant cliff.
                            if !(0..EXTENT).contains(&nx) || !(0..EXTENT).contains(&nz) {
                                continue;
                            }
                            let neighbor = self.profile(nx, nz, neighbor_step);
                            for air in neighbor.iter().filter(|r| r.material == MaterialId::AIR) {
                                let low = run.start.max(air.start);
                                let high = run.end.min(air.end);
                                if low >= high {
                                    continue;
                                }
                                for y in (low..high).step_by(substep as usize) {
                                    let mut p = [x, y, z];
                                    if side < 2 {
                                        p[2] += offset;
                                    } else {
                                        p[0] += offset;
                                    }
                                    let color = self.color(
                                        CellPos::new(p[0], y, p[2]),
                                        run.material,
                                        step == 1,
                                    );
                                    if side == 1 {
                                        p[0] += step;
                                    }
                                    if side == 3 {
                                        p[2] += step;
                                    }
                                    // A coarse face next to fine terrain is divided
                                    // at the fine spacing and uses the fine heights.
                                    // The opposite tile applies the identical profile
                                    // comparison, so together they close the boundary.
                                    let collector = if substep < step {
                                        &mut stitches
                                    } else {
                                        &mut faces
                                    };
                                    collector.add(
                                        (side + 2) as u32,
                                        p.map(|v| v / substep),
                                        color,
                                    )?;
                                }
                            }
                        }
                    }
                }
            }
        }
        let mut parts = vec![SurfacePart {
            step,
            surface: faces.finish(origin, step),
        }];
        if step > 1 {
            let surface = stitches.finish(origin, step / 2);
            if !surface.quads.is_empty() {
                parts.push(SurfacePart {
                    step: step / 2,
                    surface,
                });
            }
        }
        Ok(TerrainMesh { origin, parts })
    }
}
fn material_at(runs: &[Run], y: i64) -> MaterialId {
    runs.iter()
        .find(|r| r.start <= y && y < r.end)
        .map_or(MaterialId::AIR, |r| r.material)
}
