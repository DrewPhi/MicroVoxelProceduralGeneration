//! Grid-aligned terrain proxies and exact native vertical-run surface extraction.
//! Coarse terrain uses band-limited area summaries and a bounded shared hierarchy.
//! Only display data changes; authoritative cells and queries remain exact.
use super::{
    lod::{MeshKey, EXTENT, FINEST, SIDE},
    surface::{FaceCollector, Surface, AXES, NORMALS},
};
use crate::{
    generation::{Column, Generator, Run},
    grid::CellPos,
    materials::{MaterialId, Voxel},
    painting,
    world::World,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
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
    /// CPU preparation belongs on a mesh worker, not the frame/upload thread.
    pub fn prepare(&self, key: MeshKey) -> Vec<super::triangulation::Prepared> {
        let junctions =
            super::triangulation::Junctions::new(self.parts.iter().map(|p| (&p.surface, p.step)));
        let neighbors = key.neighbors.map(|level| 1i64 << (FINEST - level));
        self.parts
            .iter()
            .map(|part| {
                super::triangulation::prepare(
                    &part.surface,
                    part.step,
                    key.tile.edge(),
                    neighbors,
                    &junctions,
                )
            })
            .collect()
    }
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
/// Linear-light footprint colors, heights and coverage. Above 32 m these
/// summaries form an actual four-child pyramid; below it use bounded quadrature
/// with band-limited procedural noise, rather than visiting all native cells.
#[derive(Clone, Copy)]
struct Summary {
    column: Column,
    ground: f64,
    water: f64,
    bedrock: f64,
    wet: f64,
    weight: f64,
    colors: [[f32; 3]; 3], // surface, subsoil, rock, before lighting
}
#[derive(Default)]
pub struct SummaryCache {
    values: HashMap<(i64, i64, i64), Summary>,
    fifo: VecDeque<(i64, i64, i64)>,
    revision: u64,
}
const SUMMARY_LIMIT: usize = 65_536; // about 16 MiB payload + hash/FIFO overhead
pub type SharedSummaries = Arc<Mutex<SummaryCache>>;
fn linear(color: [u8; 3]) -> [f32; 3] {
    color.map(|v| (v as f32 / 255.).powf(2.2))
}
fn srgb(color: [f32; 3]) -> [u8; 3] {
    color.map(|v| (v.max(0.).powf(1. / 2.2) * 255.).round().clamp(0., 255.) as u8)
}
/// Each worker owns its profile cache. No shared mesh-generation lock is needed.
pub struct Sampler<G: Generator> {
    world: Arc<World<G>>,
    profiles: HashMap<(i64, i64, i64), Arc<Vec<Run>>>,
    fifo: VecDeque<(i64, i64, i64)>,
    revision: u64,
    summaries: SharedSummaries,
    lighting: HashMap<(i64, i64, i64), u8>,
}
impl<G: Generator> Sampler<G> {
    pub fn new(world: Arc<World<G>>) -> Self {
        Self::with_summaries(world, Arc::new(Mutex::new(SummaryCache::default())))
    }
    pub fn with_summaries(world: Arc<World<G>>, summaries: SharedSummaries) -> Self {
        let revision = world.revision();
        Self {
            world,
            profiles: HashMap::new(),
            fifo: VecDeque::new(),
            revision,
            summaries,
            lighting: HashMap::new(),
        }
    }
    pub fn world(&self) -> &World<G> {
        &self.world
    }
    pub(crate) fn profile(&mut self, x: i64, z: i64, step: i64) -> Arc<Vec<Run>> {
        if self.revision != self.world.revision() {
            let revision = self.revision;
            self.profiles
                .retain(|&(x, z, step), _| !self.world.region_changed_since(x, z, step, revision));
            self.fifo.retain(|k| self.profiles.contains_key(k));
            self.lighting.retain(|&(x, z, step), _| {
                !self
                    .world
                    .region_changed_since(x - step, z - step, step * 3, revision)
            });
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
            if let Some(summary) = self.summary(x, z, step) {
                let c = summary.column;
                let quantize = |v: i64| ((v + step / 2).div_euclid(step) * step).clamp(0, CEILING);
                let ground = quantize(summary.ground.round() as i64);
                let water = if summary.wet >= 0.5 {
                    // Enclose the true plane; vertices are clipped back to its
                    // native height, identically at every LOD.
                    ((summary.water.round() as i64 + step - 1).div_euclid(step) * step)
                        .clamp(0, CEILING)
                        .max(ground)
                } else {
                    ground
                };
                let surface_start = (ground - step).max(0);
                let bedrock = quantize(summary.bedrock.round() as i64).min(surface_start);
                let mut runs = Vec::new();
                for (start, end, material) in [
                    (0, bedrock, c.rock),
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
            } else {
                vec![Run {
                    start: 0,
                    end: CEILING,
                    material: MaterialId::AIR,
                }]
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
    fn summary(&self, x: i64, z: i64, step: i64) -> Option<Summary> {
        let x = x.div_euclid(step) * step;
        let z = z.div_euclid(step) * step;
        if !(0..EXTENT).contains(&x) || !(0..EXTENT).contains(&z) {
            return None;
        }
        let key = (x, z, step);
        {
            let mut cache = self.summaries.lock().unwrap();
            if cache.revision != self.world.revision() {
                let revision = cache.revision;
                cache.values.retain(|&(x, z, step), _| {
                    !self.world.region_changed_since(x, z, step, revision)
                });
                let retained: std::collections::HashSet<_> = cache.values.keys().copied().collect();
                cache.fifo.retain(|k| retained.contains(k));
                cache.revision = self.world.revision();
            }
            if let Some(s) = cache.values.get(&key) {
                return Some(*s);
            }
        }
        let half = (step / 2).max(1);
        let mut children = Vec::with_capacity(4);
        for dz in 0..2 {
            for dx in 0..2 {
                let sx = x + dx * half;
                let sz = z + dz * half;
                if sx >= EXTENT || sz >= EXTENT {
                    continue;
                }
                if step > 512 && step <= 2048 {
                    if let Some(child) = self.summary(sx, sz, half) {
                        children.push(child);
                    }
                } else {
                    let px = (sx + half / 2).min(EXTENT - 1);
                    let pz = (sz + half / 2).min(EXTENT - 1);
                    if let Some((c, environment)) = self.world.display_sample(px, pz, half) {
                        let mut colors = [[0.; 3]; 3];
                        for (index, mat) in [c.surface, c.subsoil, c.rock].into_iter().enumerate() {
                            let p = CellPos::new(px, c.ground - 1, pz);
                            colors[index] = linear(painting::base_color(
                                Voxel::new(
                                    mat,
                                    environment.map_or_else(
                                        || self.world.appearance_at(p, mat),
                                        |e| {
                                            painting::environment_appearance(
                                                p,
                                                self.world.identity().seed,
                                                mat,
                                                e,
                                            )
                                        },
                                    ),
                                ),
                                self.world.materials(),
                            ));
                        }
                        if half > 1 {
                            let p = CellPos::new(px, c.ground - 1, pz);
                            if let Some(coverage) = environment
                                .filter(|_| {
                                    !matches!(c.surface, MaterialId::GRAVEL | MaterialId::SILT)
                                })
                                .map(|e| e.surface_cover(c.rock))
                                .or_else(|| {
                                    if environment.is_none() {
                                        self.world.display_surface_cover(p)
                                    } else {
                                        None
                                    }
                                })
                            {
                                colors[0] = [0.; 3];
                                for (material, weight) in coverage {
                                    if weight <= 0. {
                                        continue;
                                    }
                                    let color = linear(painting::base_color(
                                        Voxel::new(
                                            material,
                                            environment.map_or_else(
                                                || self.world.appearance_at(p, material),
                                                |e| {
                                                    painting::environment_appearance(
                                                        p,
                                                        self.world.identity().seed,
                                                        material,
                                                        e,
                                                    )
                                                },
                                            ),
                                        ),
                                        self.world.materials(),
                                    ));
                                    for channel in 0..3 {
                                        colors[0][channel] += color[channel] * weight;
                                    }
                                }
                            }
                        }
                        children.push(Summary {
                            column: c,
                            ground: c.ground as f64,
                            water: c.water as f64,
                            bedrock: c.bedrock as f64,
                            wet: f64::from(c.water > c.ground),
                            weight: ((half.min(EXTENT - sx)) * (half.min(EXTENT - sz))) as f64,
                            colors,
                        });
                    }
                }
            }
        }
        let mut result = *children.first()?;
        let weight: f64 = children.iter().map(|s| s.weight).sum();
        let average =
            |f: fn(&Summary) -> f64| children.iter().map(|s| f(s) * s.weight).sum::<f64>() / weight;
        result.ground = average(|s| s.ground);
        result.bedrock = average(|s| s.bedrock);
        result.wet = average(|s| s.wet);
        result.weight = weight;
        let wet_weight: f64 = children.iter().map(|s| s.wet * s.weight).sum();
        result.water = if wet_weight > 0. {
            children
                .iter()
                .map(|s| s.water * s.wet * s.weight)
                .sum::<f64>()
                / wet_weight
        } else {
            result.ground
        };
        // Representative material IDs are display metadata; RGB retains all
        // sampled material coverage instead of repainting the whole coarse cell.
        for layer in 0..3 {
            for channel in 0..3 {
                result.colors[layer][channel] = children
                    .iter()
                    .map(|s| s.colors[layer][channel] as f64 * s.weight)
                    .sum::<f64>() as f32
                    / weight as f32;
            }
        }
        let mut cache = self.summaries.lock().unwrap();
        if !cache.values.contains_key(&key) {
            if cache.values.len() >= SUMMARY_LIMIT {
                if let Some(old) = cache.fifo.pop_front() {
                    cache.values.remove(&old);
                }
            }
            cache.fifo.push_back(key);
            cache.values.insert(key, result);
        }
        Some(result)
    }
    pub(crate) fn color(&self, p: CellPos, material: MaterialId, step: i64) -> [u8; 3] {
        if step > 1 && material != MaterialId::WATER {
            if let Some(s) = self.summary(p.x, p.z, step) {
                let layer = if material == s.column.surface {
                    0
                } else if material == s.column.subsoil {
                    1
                } else {
                    2
                };
                return srgb(s.colors[layer]);
            }
        }
        let voxel = if step == 1 {
            self.world.voxel_at(p).known().unwrap_or(Voxel::AIR)
        } else {
            Voxel::new(
                material,
                self.world
                    .storage()
                    .get(p)
                    .filter(|v| v.material == material)
                    .map_or_else(
                        || painting::appearance(p, self.world.identity().seed),
                        |v| v.appearance,
                    ),
            )
        };
        painting::base_color(voxel, self.world.materials())
    }
    /// A bounded worker cache of filtered slope illumination. Sampling is at
    /// least one meter wide, so native cells do not trigger fine-noise work.
    fn slope_illumination(&mut self, p: CellPos, step: i64) -> u8 {
        let spacing = step.max(16);
        let x = p.x.div_euclid(spacing) * spacing;
        let z = p.z.div_euclid(spacing) * spacing;
        let key = (x, z, spacing);
        if let Some(value) = self.lighting.get(&key) {
            return *value;
        }
        let height = |x, z| self.summary(x, z, spacing).map(|s| s.ground);
        let center = height(x, z).unwrap_or(p.y as f64);
        let dx = (height(x + spacing, z).unwrap_or(center)
            - height(x - spacing, z).unwrap_or(center))
            / (2 * spacing) as f64;
        let dz = (height(x, z + spacing).unwrap_or(center)
            - height(x, z - spacing).unwrap_or(center))
            / (2 * spacing) as f64;
        let length = (dx * dx + 1. + dz * dz).sqrt();
        let sun_length = (0.4_f64.powi(2) + 0.85_f64.powi(2) + 0.35_f64.powi(2)).sqrt();
        let diffuse = ((0.4 * dx + 0.85 + 0.35 * dz) / (length * sun_length)).clamp(0., 1.);
        let value = (diffuse * 63.).round() as u8;
        if self.lighting.len() >= CACHE_LIMIT {
            self.lighting.clear();
        }
        self.lighting.insert(key, value);
        value
    }
    /// One byte retains six bits of filtered illumination and two of near AO.
    /// Base RGB and geometry remain unchanged; actors retain their original AO.
    pub(crate) fn ambient(&mut self, p: CellPos, face: u32, step: i64) -> u8 {
        let slope = self.slope_illumination(p, step);
        if step >= 8 {
            return slope | (3 << 6);
        }
        let normal = NORMALS[face as usize];
        let [_, u, v] = AXES[face as usize];
        let mut outside = [p.x, p.y, p.z];
        for i in 0..3 {
            outside[i] += normal[i] as i64 * step;
        }
        let mut occupied = 0;
        for axis in [u, v] {
            for sign in [-1, 1] {
                let mut n = outside;
                n[axis] += sign * step;
                if n[0] < 0 || n[2] < 0 || n[0] >= EXTENT || n[2] >= EXTENT {
                    continue;
                }
                if material_at(&self.profile(n[0], n[2], step), n[1]) != MaterialId::AIR {
                    occupied += 1;
                }
            }
        }
        let visibility = 1. - occupied as f32 * 18. / (255. * (step as f32).sqrt());
        let ao = ((visibility - 0.7) * 10.).round().clamp(0., 3.) as u8;
        slope | (ao << 6)
    }
    pub(crate) fn display_water_level(&self, x: i64, z: i64, step: i64) -> i64 {
        self.summary(x, z, step)
            .map_or(0, |s| s.water.round() as i64)
    }
    /// Guaranteed solid base prism for conservative software occlusion.
    pub fn solid_top(&mut self, key: MeshKey) -> i64 {
        let [ox, oz] = key.tile.origin();
        let step = key.tile.step();
        let mut top = i64::MAX;
        for iz in 0..SIDE {
            for ix in 0..SIDE {
                let x = ox + ix * step;
                let z = oz + iz * step;
                if x >= EXTENT || z >= EXTENT {
                    continue;
                }
                let profile = self.profile(x, z, step);
                let mut end = 0;
                for run in profile.iter() {
                    if run.material == MaterialId::AIR {
                        break;
                    }
                    end = run.end;
                    if run.material == MaterialId::WATER && step > 1 {
                        end = end.min(self.display_water_level(x, z, step));
                    }
                }
                top = top.min(end);
            }
        }
        if top == i64::MAX {
            0
        } else {
            top
        }
    }
    pub fn mesh(&mut self, key: MeshKey) -> Result<TerrainMesh, String> {
        self.mesh_cancellable(key, || false)
    }
    pub fn mesh_cancellable(
        &mut self,
        key: MeshKey,
        cancelled: impl Fn() -> bool,
    ) -> Result<TerrainMesh, String> {
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
        let mut water_faces: std::collections::BTreeMap<(i64, i64), FaceCollector> =
            std::collections::BTreeMap::new();
        for iz in 0..SIDE {
            if cancelled() {
                return Err("mesh cancelled".into());
            }
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
                        let color = self.color(p, run.material, step);
                        let ambient = if run.material == MaterialId::WATER {
                            255
                        } else {
                            self.ambient(p, face, step)
                        };
                        let water_level = if run.material == MaterialId::WATER {
                            if step == 1 {
                                run.end
                            } else {
                                self.summary(x, z, step).unwrap().water.round() as i64
                            }
                        } else {
                            0
                        };
                        let collector = if water_level > 0 {
                            water_faces.entry((step, water_level)).or_default()
                        } else {
                            &mut faces
                        };
                        collector.add_shaded(
                            face,
                            [x / step, plane / step, z / step],
                            color,
                            ambient,
                        )?;
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
                                    let color =
                                        self.color(CellPos::new(p[0], y, p[2]), run.material, step);
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
                                    let water_level = if run.material == MaterialId::WATER {
                                        if step == 1 {
                                            run.end
                                        } else {
                                            self.summary(x, z, step).unwrap().water.round() as i64
                                        }
                                    } else {
                                        0
                                    };
                                    let collector = if water_level > 0 {
                                        water_faces.entry((substep, water_level)).or_default()
                                    } else if substep < step {
                                        &mut stitches
                                    } else {
                                        &mut faces
                                    };
                                    let ambient = if run.material == MaterialId::WATER {
                                        255
                                    } else {
                                        self.ambient(
                                            CellPos::new(
                                                if side < 2 { x } else { x + offset },
                                                y,
                                                if side < 2 { z + offset } else { z },
                                            ),
                                            (side + 2) as u32,
                                            step,
                                        )
                                    };
                                    collector.add_shaded(
                                        (side + 2) as u32,
                                        p.map(|v| v / substep),
                                        color,
                                        ambient,
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
        for part in &mut parts {
            for quad in &mut part.surface.quads {
                quad.slope_light = true;
            }
        }
        // Water groups share native height, even where their enclosing coarse
        // grid differs. Merge them into existing draw parts without extra draws.
        for ((spacing, level), collector) in water_faces {
            let mut surface = collector.finish(origin, spacing);
            for q in &mut surface.quads {
                q.water_level = level as u16;
            }
            let index = match parts.iter().position(|p| p.step == spacing) {
                Some(i) => i,
                None => {
                    parts.push(SurfacePart {
                        step: spacing,
                        surface: Surface::default(),
                    });
                    parts.len() - 1
                }
            };
            let target = &mut parts[index].surface;
            let offset = target.colors.len() as u32;
            for q in &mut surface.quads {
                q.color_offset += offset;
            }
            target.min_y = target.min_y.min(surface.min_y).min(level);
            target.max_y = target.max_y.max(surface.max_y);
            target.unit_faces += surface.unit_faces;
            target.colors.extend(surface.colors);
            target.quads.extend(surface.quads);
        }
        Ok(TerrainMesh { origin, parts })
    }
}
fn material_at(runs: &[Run], y: i64) -> MaterialId {
    runs.iter()
        .find(|r| r.start <= y && y < r.end)
        .map_or(MaterialId::AIR, |r| r.material)
}

#[cfg(test)]
mod tests {
    use super::super::lod::TileKey;
    use super::*;
    use crate::generation::{FlatTerrain, GeneratorIdentity};
    #[derive(Clone, Copy)]
    struct Cliffs;
    impl Generator for Cliffs {
        fn identity(&self) -> GeneratorIdentity {
            FlatTerrain {
                height: 64,
                water: 64,
                seed: 42,
            }
            .identity()
        }
        fn column_hint(&self, x: i64, z: i64) -> Option<Column> {
            let ground = 40 + (x % 13) * 2 + (z % 11) + if x % 17 < 5 { 19 } else { 0 };
            Some(Column {
                ground,
                water: ground.max(67),
                bedrock: ground - 5,
                surface: MaterialId::GRASS,
                subsoil: MaterialId::SOIL,
                rock: MaterialId::ROCK,
            })
        }
        fn voxel(&self, p: CellPos) -> Option<Voxel> {
            Some(Voxel::new(
                self.column_hint(p.x, p.z)?.material_at(p.y),
                crate::materials::Appearance(0),
            ))
        }
        fn runs(&self, x: i64, z: i64, r: std::ops::Range<i64>) -> Option<Vec<Run>> {
            Some(self.column_hint(x, z)?.runs(r))
        }
    }
    #[test]
    fn cliff_water_and_corner_boundaries_have_exactly_one_face_per_exposed_unit() {
        let world = Arc::new(World::new(Cliffs));
        let mut s = Sampler::new(world);
        let coarse = MeshKey {
            tile: TileKey {
                level: 12,
                x: 1,
                z: 1,
            },
            neighbors: [13, 12, 13, 12],
            revision: 0,
        };
        let keys = [
            coarse,
            MeshKey {
                tile: TileKey {
                    level: 13,
                    x: 1,
                    z: 2,
                },
                neighbors: [13, 12, 13, 13],
                revision: 0,
            },
            MeshKey {
                tile: TileKey {
                    level: 13,
                    x: 1,
                    z: 3,
                },
                neighbors: [13, 12, 13, 13],
                revision: 0,
            },
            MeshKey {
                tile: TileKey {
                    level: 13,
                    x: 2,
                    z: 1,
                },
                neighbors: [13, 13, 13, 12],
                revision: 0,
            },
            MeshKey {
                tile: TileKey {
                    level: 13,
                    x: 3,
                    z: 1,
                },
                neighbors: [13, 13, 13, 12],
                revision: 0,
            },
        ];
        let meshes: Vec<_> = keys.into_iter().map(|k| s.mesh(k).unwrap()).collect();
        for (axis, negative, positive) in [(0, 2, 3), (2, 4, 5)] {
            let mut coverage = std::collections::BTreeMap::new();
            for mesh in &meshes {
                for part in &mesh.parts {
                    for quad in &part.surface.quads {
                        if quad.face != negative && quad.face != positive {
                            continue;
                        }
                        let p = std::array::from_fn::<_, 3, _>(|i| {
                            mesh.origin[i] + quad.origin[i] as i64
                        });
                        if p[axis] != 64 {
                            continue;
                        }
                        let [_, u, v] = AXES[quad.face as usize];
                        for du in 0..quad.size[0] as i64 * part.step {
                            for dv in 0..quad.size[1] as i64 * part.step {
                                let mut cell = p;
                                cell[u] += du;
                                cell[v] += dv;
                                let along = cell[2 - axis];
                                if (64..128).contains(&along) {
                                    *coverage.entry((quad.face, cell[1], along)).or_insert(0) += 1;
                                }
                            }
                        }
                    }
                }
            }
            for along in 64..128 {
                for y in 0..128 {
                    let (cx, cz, fx, fz) = if axis == 0 {
                        (64, along, 63, along)
                    } else {
                        (along, 64, along, 63)
                    };
                    let c = material_at(&s.profile(cx, cz, 2), y) != MaterialId::AIR;
                    let f = material_at(&s.profile(fx, fz, 1), y) != MaterialId::AIR;
                    assert_eq!(
                        coverage.get(&(negative, y, along)).copied().unwrap_or(0),
                        usize::from(c && !f),
                        "coarse axis {axis} y {y} along {along}"
                    );
                    assert_eq!(
                        coverage.get(&(positive, y, along)).copied().unwrap_or(0),
                        usize::from(f && !c),
                        "fine axis {axis} y {y} along {along}"
                    );
                }
            }
        }
    }
    #[test]
    fn flat_lake_stays_on_the_same_native_plane_at_every_lod() {
        let world = Arc::new(World::new(FlatTerrain {
            height: 17,
            water: 67,
            seed: 42,
        }));
        let mut sampler = Sampler::new(world);
        for level in [13, 12, 10, 8, 6] {
            let key = MeshKey {
                tile: TileKey { level, x: 1, z: 1 },
                neighbors: [level; 4],
                revision: 0,
            };
            let mesh = sampler.mesh(key).unwrap();
            let mut tops = 0;
            for part in &mesh.parts {
                for q in &part.surface.quads {
                    if q.face == 0 {
                        let rendered = if q.water_level > 0 {
                            i64::from(q.origin[1]).min(q.water_level as i64)
                        } else {
                            i64::from(q.origin[1])
                        };
                        assert_eq!(rendered, 67, "LOD {level} water plane");
                        assert!(
                            !q.slope_light,
                            "water must not inherit submerged slope shading"
                        );
                        tops += 1;
                    }
                    if q.water_level > 0 && q.face >= 2 {
                        // Enclosing LOD walls above the real plane collapse,
                        // rather than producing dark ledges across a flat lake.
                        assert!(q.origin[1] as i64 >= 67);
                    }
                }
            }
            assert!(tops > 0);
        }
    }
    #[test]
    fn filtered_slope_light_is_shared_by_all_faces_and_keeps_native_ao() {
        let world = Arc::new(World::new(FlatTerrain {
            height: 64,
            water: 64,
            seed: 42,
        }));
        let mut sampler = Sampler::new(world);
        let p = CellPos::new(128, 63, 128);
        let values: Vec<_> = (0..6).map(|f| sampler.ambient(p, f, 8)).collect();
        assert!(values.iter().all(|v| *v == values[0]));
        assert!(values[0] > 200);
        assert_eq!(sampler.ambient(p, 0, 1) >> 6, 3);
    }
    #[test]
    fn coarse_land_and_water_never_request_native_voxels_or_appearance() {
        struct CoarseOnly {
            water: i64,
        }
        impl Generator for CoarseOnly {
            fn identity(&self) -> GeneratorIdentity {
                FlatTerrain {
                    height: 64,
                    water: 64,
                    seed: 42,
                }
                .identity()
            }
            fn voxel(&self, _: CellPos) -> Option<Voxel> {
                panic!("native voxel requested for coarse display")
            }
            fn appearance_at(&self, _: CellPos, _: MaterialId) -> crate::materials::Appearance {
                panic!("native appearance requested for coarse display")
            }
            fn display_sample(
                &self,
                _: i64,
                _: i64,
                _: i64,
            ) -> Option<(Column, Option<crate::generation::EnvironmentSample>)> {
                use crate::generation::{EnvironmentSample, Geology};
                Some((
                    Column {
                        ground: 64,
                        water: self.water.max(64),
                        bedrock: 60,
                        surface: MaterialId::GRASS,
                        subsoil: MaterialId::SOIL,
                        rock: MaterialId::ROCK,
                    },
                    Some(EnvironmentSample {
                        elevation_m: 4.,
                        soil_depth_m: 1.,
                        slope: 0.,
                        temperature_c: 14.,
                        cold_season_c: 7.,
                        warm_season_c: 21.,
                        rainfall_mm_year: 600.,
                        potential_evaporation_mm_year: 900.,
                        moisture: 0.5,
                        ground_wetness: 0.5,
                        atmospheric_humidity: 0.5,
                        orographic_exposure: 0.,
                        drainage_area_m2: 1.,
                        prevailing_wind: [1., 0.],
                        landform_weights: [1., 0., 0., 0., 0.],
                        geology: Geology::Granite,
                    }),
                ))
            }
        }
        for water in [64, 81] {
            let mut s = Sampler::new(Arc::new(World::new(CoarseOnly { water })));
            let key = MeshKey {
                tile: TileKey {
                    level: 8,
                    x: 1,
                    z: 1,
                },
                neighbors: [8; 4],
                revision: 0,
            };
            assert!(s.mesh(key).unwrap().faces() > 0);
            assert!(!s.ray_brick(key, || false).unwrap().words.is_empty());
        }
    }
    #[test]
    fn coarse_edit_cache_invalidates_locally_without_dense_voxel_storage() {
        let world = Arc::new(World::new(FlatTerrain {
            height: 64,
            water: 64,
            seed: 42,
        }));
        let summaries = Arc::new(Mutex::new(SummaryCache::default()));
        let sampler = Sampler::with_summaries(world.clone(), summaries.clone());
        sampler.summary(64, 64, 2).unwrap();
        let far = sampler.summary(128, 128, 2).unwrap();
        drop(sampler);
        let mut world = Arc::try_unwrap(world).ok().unwrap();
        world
            .apply(&[crate::Edit::set(
                CellPos::new(64, 63, 64),
                Voxel::new(MaterialId::GRASS, crate::materials::Appearance(65535)),
            )])
            .unwrap();
        let world = Arc::new(world);
        let sampler = Sampler::with_summaries(world.clone(), summaries.clone());
        sampler.summary(64, 64, 2).unwrap();
        let cache = summaries.lock().unwrap();
        assert_eq!(cache.values[&(128, 128, 2)].colors, far.colors);
        assert_eq!(world.storage().override_count(), 1);
    }
    #[test]
    fn surface_paint_edits_are_area_filtered_in_linear_light() {
        let mut world = World::new(crate::generation::FlatTerrain {
            height: 64,
            water: 64,
            seed: 42,
        });
        let mut expected = [0f32; 3];
        for (i, (x, z)) in [(64, 64), (65, 64), (64, 65), (65, 65)]
            .into_iter()
            .enumerate()
        {
            let voxel = Voxel::new(
                MaterialId::GRASS,
                crate::materials::Appearance(if i % 2 == 0 { 0 } else { 65535 }),
            );
            world
                .apply(&[crate::Edit::set(CellPos::new(x, 63, z), voxel)])
                .unwrap();
            let color = linear(painting::base_color(voxel, world.materials()));
            for c in 0..3 {
                expected[c] += color[c] / 4.;
            }
        }
        let s = Sampler::new(Arc::new(world));
        let summary = s.summary(64, 64, 2).unwrap();
        for c in 0..3 {
            assert!((summary.colors[0][c] - expected[c]).abs() < 1e-6);
        }
    }
    #[test]
    fn cancellation_and_filtered_colors_do_not_change_native_world() {
        let world = Arc::new(World::new(Cliffs));
        let before = world.voxel_at(CellPos::new(71, 51, 77));
        let mut s = Sampler::new(Arc::clone(&world));
        let key = MeshKey {
            tile: TileKey {
                level: 12,
                x: 1,
                z: 1,
            },
            neighbors: [12; 4],
            revision: 0,
        };
        assert_eq!(
            s.mesh_cancellable(key, || true).unwrap_err(),
            "mesh cancelled"
        );
        let summary = s.summary(64, 64, 2).unwrap();
        assert!(summary
            .colors
            .iter()
            .flatten()
            .all(|x| x.is_finite() && *x >= 0. && *x <= 1.));
        s.mesh(key).unwrap();
        assert_eq!(before, world.voxel_at(CellPos::new(71, 51, 77)));
    }
}
