//! Anchored coarse surfaces. Native cells and volumetric replacement regions
//! use the exact/run mesher; ordinary distant terrain uses connected samples.
use super::{Sampler, SurfacePart, TerrainMesh};
use crate::render::{
    lod::{MeshKey, SIDE},
    surface::{Quad, Surface},
};
use crate::{
    generation::Generator,
    grid::CellPos,
    materials::{MaterialId, Voxel},
    painting,
};
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Copy)]
pub(super) struct Sample {
    height: f32,
    color: u32,
    water: bool,
}

const VISUAL_NODE_LIMIT: usize = 16_384;
type NodeKey = (i64, i64, i64);
/// Per-worker sampled appearance/height cache. Never shared with canonical data.
#[derive(Default)]
pub(super) struct VisualNodes {
    values: HashMap<NodeKey, Sample>,
    fifo: VecDeque<NodeKey>,
    revision: Option<u64>,
}
impl VisualNodes {
    fn get(&self, key: &NodeKey) -> Option<&Sample> {
        self.values.get(key)
    }
    fn insert(&mut self, key: NodeKey, sample: Sample) {
        if self.values.contains_key(&key) {
            self.values.insert(key, sample);
            return;
        }
        while self.values.len() >= VISUAL_NODE_LIMIT {
            if let Some(old) = self.fifo.pop_front() {
                self.values.remove(&old);
            } else {
                break;
            }
        }
        self.values.insert(key, sample);
        self.fifo.push_back(key);
    }
    fn set_revision(&mut self, revision: u64) {
        if self.revision != Some(revision) {
            self.values.clear();
            self.fifo.clear();
            self.revision = Some(revision);
        }
    }
}

impl<G: Generator> Sampler<G> {
    /// Coherent raster path; the old mesher remains an explicit comparator.
    pub fn display_mesh(
        &mut self,
        key: MeshKey,
        cancelled: impl Fn() -> bool,
    ) -> Result<TerrainMesh, String> {
        if key.revision != self.world.revision() {
            return Err("stale world mesh request".into());
        }
        let grid = self.grid();
        let [x, z] = key.tile.origin_in(grid);
        let edge = key.tile.edge_in(grid);
        let step = key.tile.step_in(grid);
        if self.run_region(x, z, edge, step) {
            return self.mesh_cancellable(key, cancelled);
        }
        self.coherent_mesh(key, cancelled)
    }

    fn run_region(&self, x: i64, z: i64, edge: i64, step: i64) -> bool {
        if step == 1 {
            return true;
        }
        let halo = step * 2;
        let scale = self.grid().cell_m();
        self.world
            .region_changed_since(x - halo, z - halo, edge + halo * 2, 0)
            || self.world.has_volumetric_features(
                [(x - halo) as f64 * scale, (z - halo) as f64 * scale],
                (edge + halo * 2) as f64 * scale,
            )
    }

    fn visual_node(
        &self,
        x: i64,
        z: i64,
        step: i64,
        cache: &mut VisualNodes,
    ) -> Result<Sample, String> {
        if let Some(s) = cache.get(&(x, z, step)) {
            return Ok(*s);
        }
        let extent = self.grid().extent;
        let px = x.clamp(0, extent - 1);
        let pz = z.clamp(0, extent - 1);
        let (c, env) = self
            .world
            .display_sample(px, pz, step)
            .ok_or("unknown coarse terrain sample")?;
        let wet = c.water > c.ground;
        let material = if wet { MaterialId::WATER } else { c.surface };
        let p = CellPos::new(px, if wet { c.water - 1 } else { c.ground - 1 }, pz);
        let appearance = env.map_or_else(
            || self.world.display_appearance_at(p, material, step),
            |e| {
                painting::environment_appearance_m(
                    self.world.spec().grid.point(p),
                    self.world.identity().seed,
                    material,
                    e,
                )
            },
        );
        let rgb = painting::base_color(Voxel::new(material, appearance), self.world.materials());
        let mut linear = rgb.map(|v| (v as f32 / 255.).powf(2.2));
        if !wet {
            if let Some(cover) = self.world.display_coverage_at(p, step) {
                linear = [0.; 3];
                for (mat, weight) in cover {
                    if weight <= 0. {
                        continue;
                    }
                    let appearance = env.map_or_else(
                        || self.world.display_appearance_at(p, mat, step),
                        |e| {
                            painting::environment_appearance_m(
                                self.world.spec().grid.point(p),
                                self.world.identity().seed,
                                mat,
                                e,
                            )
                        },
                    );
                    let rgb =
                        painting::base_color(Voxel::new(mat, appearance), self.world.materials());
                    for i in 0..3 {
                        linear[i] += (rgb[i] as f32 / 255.).powf(2.2) * weight;
                    }
                }
            }
        }
        let rgb = linear.map(|v| (v.max(0.).powf(1. / 2.2) * 255.).round().clamp(0., 255.) as u8);
        let value = Sample {
            height: c.ground.max(c.water) as f32,
            color: u32::from_le_bytes([rgb[0], rgb[1], rgb[2], 255]),
            water: wet,
        };
        cache.insert((x, z, step), value);
        Ok(value)
    }

    fn coarse_height(
        &self,
        x: i64,
        z: i64,
        step: i64,
        cache: &mut VisualNodes,
    ) -> Result<f32, String> {
        if x.rem_euclid(step) == 0 && z.rem_euclid(step) == 0 {
            return Ok(self.visual_node(x, z, step, cache)?.height);
        }
        let ox = x.div_euclid(step) * step;
        let oz = z.div_euclid(step) * step;
        let u = (x - ox) as f32 / step as f32;
        let v = (z - oz) as f32 / step as f32;
        let a = self.visual_node(ox, oz, step, cache)?.height;
        let b = self.visual_node(ox + step, oz, step, cache)?.height;
        let c = self.visual_node(ox + step, oz + step, step, cache)?.height;
        let d = self.visual_node(ox, oz + step, step, cache)?.height;
        // Same diagonal as the GPU quad: (0,0) -> (1,1).
        Ok(if u >= v {
            a + (b - a) * u + (c - b) * v
        } else {
            a + (c - d) * u + (d - a) * v
        })
    }

    fn coarse_color(
        &self,
        x: i64,
        z: i64,
        step: i64,
        cache: &mut VisualNodes,
    ) -> Result<u32, String> {
        if x.rem_euclid(step) == 0 && z.rem_euclid(step) == 0 {
            return Ok(self.visual_node(x, z, step, cache)?.color);
        }
        let ox = x.div_euclid(step) * step;
        let oz = z.div_euclid(step) * step;
        let u = (x - ox) as f32 / step as f32;
        let v = (z - oz) as f32 / step as f32;
        let mut colors = [[0.; 3]; 4];
        for (i, (nx, nz)) in [
            (ox, oz),
            (ox + step, oz),
            (ox + step, oz + step),
            (ox, oz + step),
        ]
        .into_iter()
        .enumerate()
        {
            let packed = self.visual_node(nx, nz, step, cache)?.color.to_le_bytes();
            colors[i] = std::array::from_fn(|c| (packed[c] as f32 / 255.).powf(2.2));
        }
        let [a, b, c, d] = colors;
        let rgb: [u8; 3] = std::array::from_fn(|i| {
            let value = if u >= v {
                a[i] + (b[i] - a[i]) * u + (c[i] - b[i]) * v
            } else {
                a[i] + (c[i] - d[i]) * u + (d[i] - a[i]) * v
            };
            (value.max(0.).powf(1. / 2.2) * 255.)
                .round()
                .clamp(0., 255.) as u8
        });
        Ok(u32::from_le_bytes([rgb[0], rgb[1], rgb[2], 255]))
    }

    fn coherent_mesh(
        &mut self,
        key: MeshKey,
        cancelled: impl Fn() -> bool,
    ) -> Result<TerrainMesh, String> {
        let mut cache = std::mem::take(&mut self.visual_nodes);
        cache.set_revision(self.world.revision());
        let result = self.coherent_mesh_cached(key, cancelled, &mut cache);
        self.visual_nodes = cache;
        result
    }

    fn coherent_mesh_cached(
        &mut self,
        key: MeshKey,
        cancelled: impl Fn() -> bool,
        cache: &mut VisualNodes,
    ) -> Result<TerrainMesh, String> {
        let grid = self.grid();
        let step = key.tile.step_in(grid);
        let edge = key.tile.edge_in(grid);
        let [ox, oz] = key.tile.origin_in(grid);
        let mut surface = Surface {
            min_y: i64::MAX,
            ..Default::default()
        };
        let neighbors = key.neighbors.map(|l| 1i64 << (grid.finest - l));
        let spacing = |x: i64, z: i64| -> i64 {
            let mut spacing = step;
            if x == ox {
                spacing = spacing.max(neighbors[0]);
            }
            if x == ox + edge {
                spacing = spacing.max(neighbors[1]);
            }
            if z == oz {
                spacing = spacing.max(neighbors[2]);
            }
            if z == oz + edge {
                spacing = spacing.max(neighbors[3]);
            }
            spacing
        };
        for iz in 0..SIDE {
            if cancelled() {
                return Err("mesh cancelled".into());
            }
            for ix in 0..SIDE {
                let x = ox + ix * step;
                let z = oz + iz * step;
                if x >= grid.extent || z >= grid.extent {
                    continue;
                }
                let coords = [[x, z], [x, z + step], [x + step, z + step], [x + step, z]];
                let mut heights = [0f32; 4];
                let mut parents = [0f32; 4];
                let mut corner_colors = [0u32; 4];
                for i in 0..4 {
                    let [nx, nz] = coords[i];
                    let spacing = spacing(nx, nz);
                    heights[i] = self.coarse_height(nx, nz, spacing, cache)?;
                    corner_colors[i] = self.coarse_color(nx, nz, spacing, cache)?;
                    parents[i] = self.coarse_height(coords[i][0], coords[i][1], step * 2, cache)?;
                    surface.min_y = surface.min_y.min(heights[i].min(parents[i]).floor() as i64);
                    surface.max_y = surface.max_y.max(heights[i].max(parents[i]).ceil() as i64);
                }
                let s = self.visual_node(x + step / 2, z + step / 2, step, cache)?;
                let offset = surface.colors.len() as u32;
                surface.colors.push(s.color);
                surface.colors.extend(heights.map(f32::to_bits));
                surface.colors.extend(parents.map(f32::to_bits));
                surface.colors.extend(corner_colors);
                surface.quads.push(Quad {
                    origin: [(x - ox) as i32, 0, (z - oz) as i32],
                    face: 6,
                    size: [1, 1],
                    color_offset: offset,
                    water_level: if s.water { s.height as u32 } else { 0 },
                    constant_color: true,
                    slope_light: false,
                });
                surface.unit_faces += 1;
            }
        }
        if surface.quads.is_empty() {
            surface.min_y = 0;
        }
        // A native or volumetric replacement neighbor owns its stepped edge.
        // Join it with bounded segments at the finer of the two display grids.
        let mut closure = Surface {
            min_y: i64::MAX,
            max_y: i64::MIN,
            ..Default::default()
        };
        let mut run_neighbors = HashMap::new();
        for side in 0..4 {
            let neighbor_step = neighbors[side];
            let span = step.min(neighbor_step);
            for q in &surface.quads {
                let x = q.origin[0] as i64;
                let z = q.origin[2] as i64;
                let on_edge = match side {
                    0 => x == 0,
                    1 => x + step == edge,
                    2 => z == 0,
                    _ => z + step == edge,
                };
                if !on_edge {
                    continue;
                }
                let indices = match side {
                    0 => [0, 1],
                    1 => [3, 2],
                    2 => [0, 3],
                    _ => [1, 2],
                };
                let h = indices
                    .map(|i| f32::from_bits(surface.colors[q.color_offset as usize + 1 + i]));
                for cell in (0..step).step_by(span as usize) {
                    if cancelled() {
                        return Err("mesh cancelled".into());
                    }
                    let (lx, lz) = match side {
                        0 => (0, z + cell),
                        1 => (edge, z + cell),
                        2 => (x + cell, 0),
                        _ => (x + cell, edge),
                    };
                    let (nx, nz) = match side {
                        0 => (ox - 1, oz + lz),
                        1 => (ox + edge, oz + lz),
                        2 => (ox + lx, oz - 1),
                        _ => (ox + lx, oz + edge),
                    };
                    if nx < 0 || nz < 0 || nx >= grid.extent || nz >= grid.extent {
                        continue;
                    }
                    let neighbor_edge = SIDE * neighbor_step;
                    let neighbor_origin = (
                        nx.div_euclid(neighbor_edge) * neighbor_edge,
                        nz.div_euclid(neighbor_edge) * neighbor_edge,
                    );
                    let run_neighbor = *run_neighbors
                        .entry((neighbor_origin, neighbor_step))
                        .or_insert_with(|| {
                            self.run_region(
                                neighbor_origin.0,
                                neighbor_origin.1,
                                neighbor_edge,
                                neighbor_step,
                            )
                        });
                    if !run_neighbor {
                        continue;
                    }
                    let runs = self.profile(nx, nz, neighbor_step);
                    let Some(top) = runs.iter().rev().find(|r| r.material != MaterialId::AIR)
                    else {
                        continue;
                    };
                    let native = top.end as f32;
                    let a = h[0] + (h[1] - h[0]) * cell as f32 / step as f32;
                    let b = h[0] + (h[1] - h[0]) * (cell + span) as f32 / step as f32;
                    if native == a && native == b {
                        continue;
                    }
                    let rgb = self.color(
                        CellPos::new(nx, top.end - 1, nz),
                        top.material,
                        neighbor_step,
                    );
                    let crossing = if (a - native) * (b - native) < 0. {
                        Some((native - a) / (b - a))
                    } else {
                        None
                    };
                    let ranges = if let Some(t) = crossing {
                        vec![(0., t), (t, 1.)]
                    } else {
                        vec![(0., 1.)]
                    };
                    for (t0, t1) in ranges {
                        let h0 = a + (b - a) * t0;
                        let h1 = a + (b - a) * t1;
                        let offset = closure.colors.len() as u32;
                        closure
                            .colors
                            .push(u32::from_le_bytes([rgb[0], rgb[1], rgb[2], 255]));
                        // The two final floats delimit a fractional crossing
                        // inside this native cell, avoiding a bow-tie quad.
                        closure
                            .colors
                            .extend([native, native, h0, h1, t0, t1].map(f32::to_bits));
                        closure.min_y = closure.min_y.min(native.min(h0).min(h1).floor() as i64);
                        closure.max_y = closure.max_y.max(native.max(h0).max(h1).ceil() as i64);
                        // The viewer culls back faces; emit both windings so a
                        // join remains visible whichever surface is higher.
                        for reverse in [0, 4] {
                            closure.quads.push(Quad {
                                origin: [lx as i32, 0, lz as i32],
                                face: 7,
                                size: [span as u32, side as u32 + 1 + reverse],
                                color_offset: offset,
                                water_level: 0,
                                constant_color: true,
                                slope_light: false,
                            });
                            closure.unit_faces += 1;
                        }
                    }
                }
            }
        }
        let mut parts = vec![SurfacePart { step, surface }];
        if !closure.quads.is_empty() {
            parts.push(SurfacePart {
                step: 1,
                surface: closure,
            });
        }
        Ok(TerrainMesh {
            grid,
            origin: [ox, 0, oz],
            parts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::lod::{Grid, TileKey};
    use crate::{generation::FlatTerrain, World};
    use std::sync::Arc;
    #[test]
    fn coarse_surface_is_bounded_and_native_remains_exact() {
        let world = Arc::new(World::new(FlatTerrain {
            height: 32,
            water: 0,
            seed: 42,
        }));
        let grid = Grid::new(world.spec());
        let mut sampler = Sampler::new(world);
        let coarse = MeshKey {
            tile: TileKey {
                level: grid.finest - 3,
                x: 1,
                z: 1,
            },
            neighbors: [grid.finest - 3; 4],
            revision: 0,
        };
        let mesh = sampler.display_mesh(coarse, || false).unwrap();
        assert_eq!(mesh.quads(), 1024);
        assert!(mesh.parts[0].surface.quads.iter().all(|q| q.face == 6));
        assert_eq!(mesh.prepare(coarse)[0].regular, 1024);
        let key = MeshKey {
            tile: TileKey {
                level: grid.finest,
                x: 1,
                z: 1,
            },
            neighbors: [grid.finest; 4],
            revision: 0,
        };
        assert!(sampler
            .display_mesh(key, || false)
            .unwrap()
            .parts
            .iter()
            .flat_map(|p| &p.surface.quads)
            .all(|q| q.face < 6));
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use crate::{
        generation::{Column, FlatTerrain, GeneratorIdentity},
        render::lod::{Grid, TileKey},
        world::Edit,
        World,
    };
    use std::{collections::BTreeMap, ops::Range, sync::Arc};

    struct Curved {
        complex: bool,
    }
    impl Generator for Curved {
        fn identity(&self) -> GeneratorIdentity {
            GeneratorIdentity {
                seed: 3,
                version: 1,
                definition: 9,
            }
        }
        fn has_volumetric_features(&self, _: [f64; 2], _: f64) -> bool {
            self.complex
        }
        fn column_hint(&self, x: i64, z: i64) -> Option<Column> {
            self.display_column(x, z, 1)
        }
        fn display_column(&self, x: i64, z: i64, step: i64) -> Option<Column> {
            let h = 32 + (x * x).rem_euclid(23) + (z * z).rem_euclid(19) + step;
            Some(Column {
                ground: h,
                water: if (x / 8 + z / 8).rem_euclid(3) == 0 {
                    h + 9
                } else {
                    h
                },
                bedrock: 0,
                surface: MaterialId::GRASS,
                subsoil: MaterialId::SOIL,
                rock: MaterialId::ROCK,
            })
        }
        fn voxel(&self, p: CellPos) -> Option<Voxel> {
            let m = self.column_hint(p.x, p.z)?.material_at(p.y);
            Some(if m == MaterialId::AIR {
                Voxel::AIR
            } else {
                Voxel::new(m, self.appearance_at(p, m))
            })
        }
        fn runs(&self, x: i64, z: i64, range: Range<i64>) -> Option<Vec<crate::generation::Run>> {
            Some(self.column_hint(x, z)?.runs(range))
        }
    }
    fn nodes(mesh: &TerrainMesh) -> BTreeMap<(i64, i64), f32> {
        let mut out = BTreeMap::new();
        for part in &mesh.parts {
            for q in &part.surface.quads {
                assert_eq!(q.face, 6);
                let x = mesh.origin[0] + q.origin[0] as i64;
                let z = mesh.origin[2] + q.origin[2] as i64;
                for (i, (x, z)) in [
                    (x, z),
                    (x, z + part.step),
                    (x + part.step, z + part.step),
                    (x + part.step, z),
                ]
                .into_iter()
                .enumerate()
                {
                    let h = f32::from_bits(part.surface.colors[q.color_offset as usize + 1 + i]);
                    if let Some(old) = out.insert((x, z), h) {
                        assert_eq!(old, h, "same-tile node must agree");
                    }
                }
            }
        }
        out
    }
    fn node_colors(mesh: &TerrainMesh) -> BTreeMap<(i64, i64), u32> {
        let mut out = BTreeMap::new();
        for part in &mesh.parts {
            for q in &part.surface.quads {
                let x = mesh.origin[0] + q.origin[0] as i64;
                let z = mesh.origin[2] + q.origin[2] as i64;
                for (i, p) in [
                    (x, z),
                    (x, z + part.step),
                    (x + part.step, z + part.step),
                    (x + part.step, z),
                ]
                .into_iter()
                .enumerate()
                {
                    let rgb = part.surface.colors[q.color_offset as usize + 9 + i];
                    if let Some(old) = out.insert(p, rgb) {
                        assert_eq!(old, rgb, "shared node color differs inside tile");
                    }
                }
            }
        }
        out
    }
    fn blend_color(a: u32, b: u32, t: f32) -> u32 {
        let a = a.to_le_bytes();
        let b = b.to_le_bytes();
        let rgb: [u8; 3] = std::array::from_fn(|i| {
            let a = (a[i] as f32 / 255.).powf(2.2);
            let b = (b[i] as f32 / 255.).powf(2.2);
            ((a + (b - a) * t).max(0.).powf(1. / 2.2) * 255.)
                .round()
                .clamp(0., 255.) as u8
        });
        u32::from_le_bytes([rgb[0], rgb[1], rgb[2], 255])
    }
    #[test]
    fn two_to_one_edges_share_exact_height_and_filtered_colors_in_all_four_directions() {
        let world = Arc::new(World::new(Curved { complex: false }));
        let grid = Grid::new(world.spec());
        let mut sampler = Sampler::new(world);
        let fine_level = grid.finest - 2;
        let coarse_level = fine_level - 1;
        for (side, fine_tile, coarse_tile) in [
            (
                0,
                TileKey {
                    level: fine_level,
                    x: 2,
                    z: 0,
                },
                TileKey {
                    level: coarse_level,
                    x: 0,
                    z: 0,
                },
            ),
            (
                1,
                TileKey {
                    level: fine_level,
                    x: 1,
                    z: 0,
                },
                TileKey {
                    level: coarse_level,
                    x: 1,
                    z: 0,
                },
            ),
            (
                2,
                TileKey {
                    level: fine_level,
                    x: 0,
                    z: 2,
                },
                TileKey {
                    level: coarse_level,
                    x: 0,
                    z: 0,
                },
            ),
            (
                3,
                TileKey {
                    level: fine_level,
                    x: 0,
                    z: 1,
                },
                TileKey {
                    level: coarse_level,
                    x: 0,
                    z: 1,
                },
            ),
        ] {
            let mut fine = MeshKey {
                tile: fine_tile,
                neighbors: [fine_level; 4],
                revision: 0,
            };
            fine.neighbors[side] = coarse_level;
            let mut coarse = MeshKey {
                tile: coarse_tile,
                neighbors: [coarse_level; 4],
                revision: 0,
            };
            coarse.neighbors[side ^ 1] = fine_level;
            let fine_mesh = sampler.display_mesh(fine, || false).unwrap();
            let coarse_mesh = sampler.display_mesh(coarse, || false).unwrap();
            let fine_nodes = nodes(&fine_mesh);
            let coarse_nodes = nodes(&coarse_mesh);
            let fine_colors = node_colors(&fine_mesh);
            let coarse_colors = node_colors(&coarse_mesh);
            let [ox, oz] = fine.tile.origin_in(grid);
            let edge = fine.tile.edge_in(grid);
            let plane = if side < 2 {
                ox + if side == 1 { edge } else { 0 }
            } else {
                oz + if side == 3 { edge } else { 0 }
            };
            for (&(x, z), &h) in &fine_nodes {
                if (if side < 2 { x } else { z }) != plane {
                    continue;
                }
                let t = if side < 2 { z } else { x };
                let step = coarse.tile.step_in(grid);
                let lo = t.div_euclid(step) * step;
                let a = if side < 2 { (plane, lo) } else { (lo, plane) };
                let expected = if t == lo {
                    coarse_nodes[&a]
                } else {
                    let b = if side < 2 {
                        (plane, lo + step)
                    } else {
                        (lo + step, plane)
                    };
                    coarse_nodes[&a]
                        + (coarse_nodes[&b] - coarse_nodes[&a]) * (t - lo) as f32 / step as f32
                };
                assert_eq!(h, expected, "side={side} node=({x},{z})");
                let expected_color = if t == lo {
                    coarse_colors[&a]
                } else {
                    let b = if side < 2 {
                        (plane, lo + step)
                    } else {
                        (lo + step, plane)
                    };
                    blend_color(
                        coarse_colors[&a],
                        coarse_colors[&b],
                        (t - lo) as f32 / step as f32,
                    )
                };
                assert_eq!(
                    fine_colors[&(x, z)],
                    expected_color,
                    "water/ground color seam side={side} node=({x},{z})"
                );
            }
        }
    }
    #[test]
    fn display_builds_leave_canonical_queries_and_revision_unchanged() {
        let world = Arc::new(World::new(Curved { complex: false }));
        let points: Vec<_> = (30..70).map(|x| CellPos::new(x, 48, 55)).collect();
        let before: Vec<_> = points.iter().map(|&p| world.voxel_at(p)).collect();
        let revision = world.revision();
        let grid = Grid::new(world.spec());
        let mut sampler = Sampler::new(world.clone());
        for step in 1..=4 {
            let level = grid.finest - step;
            let key = MeshKey {
                tile: TileKey { level, x: 0, z: 0 },
                neighbors: [level; 4],
                revision,
            };
            sampler.display_mesh(key, || false).unwrap();
        }
        assert_eq!(
            before,
            points
                .iter()
                .map(|&p| world.voxel_at(p))
                .collect::<Vec<_>>()
        );
        assert_eq!(revision, world.revision());
    }
    #[test]
    fn smooth_path_rejects_stale_world_requests() {
        let mut world = World::new(FlatTerrain {
            height: 32,
            water: 0,
            seed: 1,
        });
        world
            .apply(&[Edit::set(CellPos::new(10000, 31, 10000), Voxel::AIR)])
            .unwrap();
        let grid = Grid::new(world.spec());
        let level = grid.finest - 2;
        let key = MeshKey {
            tile: TileKey { level, x: 0, z: 0 },
            neighbors: [level; 4],
            revision: 0,
        };
        let mut sampler = Sampler::new(Arc::new(world));
        assert_eq!(
            sampler.display_mesh(key, || false).unwrap_err(),
            "stale world mesh request"
        );
    }
    #[test]
    fn smooth_edge_closes_against_adjacent_edited_coarse_replacement() {
        let mut world = World::new(Curved { complex: false });
        world
            .apply(&[Edit::set(CellPos::new(150, 31, 170), Voxel::AIR)])
            .unwrap();
        let world = Arc::new(world);
        let grid = Grid::new(world.spec());
        let level = grid.finest - 2;
        let key = MeshKey {
            tile: TileKey { level, x: 2, z: 1 },
            neighbors: [level; 4],
            revision: world.revision(),
        };
        let mut sampler = Sampler::new(world);
        assert!(!sampler.run_region(256, 128, 128, 4));
        assert!(sampler.run_region(128, 128, 128, 4));
        let mesh = sampler.display_mesh(key, || false).unwrap();
        assert!(mesh.parts[0].surface.quads.iter().all(|q| q.face == 6));
        let closure = &mesh.parts[1];
        assert!(!closure.surface.quads.is_empty());
        for q in &closure.surface.quads {
            assert_eq!(q.face, 7);
            assert_eq!(q.origin[0], 0);
            assert_eq!(q.size[0], 4);
            assert!([1, 5].contains(&q.size[1]));
            let z = mesh.origin[2] + q.origin[2] as i64;
            let profile = sampler.profile(255, z, 4);
            let top = profile
                .iter()
                .rev()
                .find(|r| r.material != MaterialId::AIR)
                .unwrap();
            assert_eq!(
                f32::from_bits(closure.surface.colors[q.color_offset as usize + 1]),
                top.end as f32
            );
        }
        assert!(closure.surface.quads.len() <= 32 * 4);
    }
    #[test]
    fn edited_and_volumetric_regions_keep_run_geometry() {
        let mut edited = World::new(FlatTerrain {
            height: 32,
            water: 0,
            seed: 42,
        });
        edited
            .apply(&[Edit::set(CellPos::new(64, 31, 64), Voxel::AIR)])
            .unwrap();
        let edited = Arc::new(edited);
        let grid = Grid::new(edited.spec());
        let level = grid.finest - 2;
        let key = MeshKey {
            tile: TileKey { level, x: 0, z: 0 },
            neighbors: [level; 4],
            revision: edited.revision(),
        };
        let mut sampler = Sampler::new(edited);
        let mesh = sampler.display_mesh(key, || false).unwrap();
        assert!(!mesh.parts.is_empty());
        assert!(mesh
            .parts
            .iter()
            .flat_map(|p| &p.surface.quads)
            .all(|q| q.face < 6));
        let mut sampler = Sampler::new(Arc::new(World::new(Curved { complex: true })));
        let mesh = sampler
            .display_mesh(MeshKey { revision: 0, ..key }, || false)
            .unwrap();
        assert!(!mesh.parts.is_empty());
        assert!(mesh
            .parts
            .iter()
            .flat_map(|p| &p.surface.quads)
            .all(|q| q.face < 6));
    }
}

#[cfg(test)]
mod closure_tests {
    use super::*;
    use crate::{
        generation::{Column, FlatTerrain, GeneratorIdentity},
        render::lod::{Grid, TileKey},
        World,
    };
    use std::{collections::BTreeMap, sync::Arc};
    struct Sloped;
    impl Generator for Sloped {
        fn identity(&self) -> GeneratorIdentity {
            GeneratorIdentity {
                seed: 2,
                version: 1,
                definition: 5,
            }
        }
        fn voxel(&self, p: CellPos) -> Option<Voxel> {
            Some(if p.y < self.column_hint(p.x, p.z)?.ground {
                Voxel::new(MaterialId::ROCK, self.appearance_at(p, MaterialId::ROCK))
            } else {
                Voxel::AIR
            })
        }
        fn column_hint(&self, x: i64, z: i64) -> Option<Column> {
            self.display_column(x, z, 1)
        }
        fn display_column(&self, x: i64, z: i64, step: i64) -> Option<Column> {
            let ground = 32 + (x + 3 * z).rem_euclid(7) + step;
            Some(Column {
                ground,
                water: ground,
                bedrock: 0,
                surface: MaterialId::ROCK,
                subsoil: MaterialId::ROCK,
                rock: MaterialId::ROCK,
            })
        }
        fn runs(
            &self,
            x: i64,
            z: i64,
            r: std::ops::Range<i64>,
        ) -> Option<Vec<crate::generation::Run>> {
            Some(self.column_hint(x, z)?.runs(r))
        }
    }
    #[test]
    fn native_closure_covers_each_edge_with_exact_native_and_smooth_heights() {
        let world = Arc::new(World::new(Sloped));
        let grid = Grid::new(world.spec());
        let level = grid.finest - 1;
        let key = MeshKey {
            tile: TileKey { level, x: 2, z: 2 },
            neighbors: [grid.finest; 4],
            revision: 0,
        };
        let mut sampler = Sampler::new(world.clone());
        let mesh = sampler.display_mesh(key, || false).unwrap();
        let closure = &mesh.parts[1];
        assert_eq!(closure.step, 1);
        let mut cells = BTreeMap::<(usize, i32, i32), Vec<(f32, f32)>>::new();
        let mut cache = VisualNodes::default();
        for q in &closure.surface.quads {
            assert_eq!(q.face, 7);
            assert!(q.constant_color);
            assert_eq!(q.water_level, 0);
            let side = ((q.size[1] - 1) % 4) as usize;
            let x = mesh.origin[0] + q.origin[0] as i64;
            let z = mesh.origin[2] + q.origin[2] as i64;
            let (nx, nz) = match side {
                0 => (x - 1, z),
                1 => (x, z),
                2 => (x, z - 1),
                _ => (x, z),
            };
            let expected = world.baseline().column_hint(nx, nz).unwrap().ground as f32;
            let data =
                &closure.surface.colors[q.color_offset as usize..q.color_offset as usize + 7];
            let h: Vec<_> = data[1..].iter().map(|&v| f32::from_bits(v)).collect();
            assert_eq!(h[0], expected);
            assert_eq!(h[1], expected);
            assert!(h[4] < h[5] && h[4] >= 0. && h[5] <= 1.);
            assert!(
                (h[2] - expected) * (h[3] - expected) >= -0.00001,
                "split crossing must not be a bow tie"
            );
            let step = key.tile.step_in(grid);
            let t = if side < 2 { z } else { x };
            let base = t.div_euclid(step) * step;
            let p0 = if side < 2 { (x, base) } else { (base, z) };
            let p1 = if side < 2 {
                (x, base + step)
            } else {
                (base + step, z)
            };
            let a = sampler.coarse_height(p0.0, p0.1, step, &mut cache).unwrap();
            let b = sampler.coarse_height(p1.0, p1.1, step, &mut cache).unwrap();
            for (i, fraction) in [h[4], h[5]].into_iter().enumerate() {
                let expected = a + (b - a) * ((t - base) as f32 + fraction) / step as f32;
                assert!((h[2 + i] - expected).abs() < 0.00001);
            }
            if q.size[1] <= 4 {
                cells
                    .entry((side, q.origin[0], q.origin[2]))
                    .or_default()
                    .push((h[4], h[5]));
            }
        }
        assert_eq!(cells.len(), 4 * key.tile.edge_in(grid) as usize);
        for intervals in cells.values_mut() {
            intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
            assert_eq!(intervals[0].0, 0.);
            assert_eq!(intervals.last().unwrap().1, 1.);
            for pair in intervals.windows(2) {
                assert_eq!(pair[0].1, pair[1].0);
            }
        }
        let prepared = mesh.prepare(key);
        assert_eq!(prepared[1].regular as usize, closure.surface.quads.len());
        assert!(closure.surface.quads.len() <= 4 * 64 * 4);
    }
    #[test]
    fn matching_flat_native_edges_need_no_closure() {
        let world = Arc::new(World::new(FlatTerrain {
            height: 32,
            water: 0,
            seed: 1,
        }));
        let grid = Grid::new(world.spec());
        let level = grid.finest - 1;
        let key = MeshKey {
            tile: TileKey { level, x: 2, z: 2 },
            neighbors: [grid.finest; 4],
            revision: 0,
        };
        let mut sampler = Sampler::new(world);
        let mesh = sampler.display_mesh(key, || false).unwrap();
        assert_eq!(mesh.parts.len(), 1);
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::{
        generation::{Column, FlatTerrain, GeneratorIdentity, Run},
        render::lod::{Grid, TileKey},
        World,
    };
    use std::{
        cell::Cell,
        ops::Range,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Instant,
    };
    struct Counted {
        calls: Arc<AtomicUsize>,
        flat: FlatTerrain,
    }
    impl Generator for Counted {
        fn identity(&self) -> GeneratorIdentity {
            self.flat.identity()
        }
        fn voxel(&self, p: CellPos) -> Option<Voxel> {
            self.flat.voxel(p)
        }
        fn column_hint(&self, x: i64, z: i64) -> Option<Column> {
            self.flat.column_hint(x, z)
        }
        fn display_column(&self, x: i64, z: i64, _: i64) -> Option<Column> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.flat.column_hint(x, z)
        }
        fn runs(&self, x: i64, z: i64, r: Range<i64>) -> Option<Vec<Run>> {
            self.flat.runs(x, z, r)
        }
    }
    fn source() -> (Arc<World<Counted>>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(World::new(Counted {
                calls: calls.clone(),
                flat: FlatTerrain {
                    height: 32,
                    water: 0,
                    seed: 42,
                },
            })),
            calls,
        )
    }
    fn key(grid: Grid, x: u32) -> MeshKey {
        let level = grid.finest - 3;
        MeshKey {
            tile: TileKey { level, x, z: 1 },
            neighbors: [level; 4],
            revision: 0,
        }
    }
    #[test]
    fn persistent_nodes_reuse_repeated_and_neighbor_tiles_and_survive_cancellation() {
        let (world, calls) = source();
        let grid = Grid::new(world.spec());
        let mut sampler = Sampler::new(world.clone());
        let a = key(grid, 1);
        let b = key(grid, 2);
        let first = sampler.display_mesh(a, || false).unwrap();
        let first_calls = calls.load(Ordering::Relaxed);
        assert!(first_calls > 0);
        let repeated = sampler.display_mesh(a, || false).unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), first_calls);
        assert_eq!(format!("{first:?}"), format!("{repeated:?}"));
        let before = calls.load(Ordering::Relaxed);
        let neighbor = sampler.display_mesh(b, || false).unwrap();
        let neighbor_calls = calls.load(Ordering::Relaxed) - before;
        let mut fresh = Sampler::new(world.clone());
        let before = calls.load(Ordering::Relaxed);
        let reference = fresh.display_mesh(b, || false).unwrap();
        let cold_calls = calls.load(Ordering::Relaxed) - before;
        assert!(
            neighbor_calls < cold_calls,
            "adjacent tile must reuse shared anchored nodes"
        );
        assert_eq!(format!("{neighbor:?}"), format!("{reference:?}"));
        let mut fresh = Sampler::new(world);
        let checks = Cell::new(0);
        let result = fresh.display_mesh(a, || {
            checks.set(checks.get() + 1);
            checks.get() > 1
        });
        assert_eq!(result.unwrap_err(), "mesh cancelled");
        assert!(!fresh.visual_nodes.values.is_empty());
        let before = calls.load(Ordering::Relaxed);
        fresh.display_mesh(a, || false).unwrap();
        assert!(calls.load(Ordering::Relaxed) - before < first_calls);
        assert!(fresh.visual_nodes.values.len() <= VISUAL_NODE_LIMIT);
        eprintln!("source evaluations first={first_calls}, repeated=0, adjacent={neighbor_calls}, cold-adjacent={cold_calls}");
    }
    #[test]
    fn fifo_nodes_remain_bounded_and_revision_changes_invalidate() {
        let mut cache = VisualNodes::default();
        cache.set_revision(1);
        let value = Sample {
            height: 32.,
            color: u32::MAX,
            water: false,
        };
        for x in 0..VISUAL_NODE_LIMIT + 5 {
            cache.insert((x as i64, 0, 8), value);
        }
        assert_eq!(cache.values.len(), VISUAL_NODE_LIMIT);
        assert_eq!(cache.fifo.len(), VISUAL_NODE_LIMIT);
        assert!(cache.get(&(0, 0, 8)).is_none());
        assert!(cache.get(&((VISUAL_NODE_LIMIT + 4) as i64, 0, 8)).is_some());
        cache.insert((5, 0, 8), value);
        assert_eq!(cache.fifo.len(), VISUAL_NODE_LIMIT);
        cache.set_revision(1);
        assert_eq!(cache.values.len(), VISUAL_NODE_LIMIT);
        cache.set_revision(2);
        assert!(cache.values.is_empty() && cache.fifo.is_empty());
    }
    #[test]
    #[ignore = "manual matched release visual-node-cache benchmark"]
    fn repeated_neighbor_build_timings() {
        let (world, calls) = source();
        let grid = Grid::new(world.spec());
        let mut sampler = Sampler::new(world.clone());
        let start = Instant::now();
        for _ in 0..10 {
            for x in 1..=4 {
                let mut cache = VisualNodes::default();
                std::hint::black_box(
                    sampler
                        .coherent_mesh_cached(key(grid, x), || false, &mut cache)
                        .unwrap(),
                );
            }
        }
        let cold = start.elapsed();
        let cold_calls = calls.load(Ordering::Relaxed);
        let mut sampler = Sampler::new(world);
        let start = Instant::now();
        let before = calls.load(Ordering::Relaxed);
        for _ in 0..10 {
            for x in 1..=4 {
                std::hint::black_box(sampler.display_mesh(key(grid, x), || false).unwrap());
            }
        }
        let warm = start.elapsed();
        let warm_calls = calls.load(Ordering::Relaxed) - before;
        eprintln!("40 builds cold={cold:?} persistent={warm:?} speedup={:.2} sourcecalls={cold_calls}/{warm_calls} retainednodes={}",cold.as_secs_f64()/warm.as_secs_f64(),sampler.visual_nodes.values.len());
        assert!(warm_calls < cold_calls);
    }
}
