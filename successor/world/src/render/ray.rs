//! Sparse column bricks for direct voxel ray traversal. No triangle extraction.
//! Vertical runs retain cavities; only potentially exposed native colors expand.
use super::{
    lod::{MeshKey, SIDE},
    terrain::Sampler,
};
use crate::{generation::Generator, grid::CellPos, materials::MaterialId};

pub const LEAF: u32 = 0x8000_0000;
#[derive(Clone, Debug)]
pub struct Brick {
    pub words: Vec<u32>,
    pub nodes: usize,
    pub runs: usize,
    pub bounds: [[i32; 3]; 2],
    pub solid_top: i64,
}
impl Brick {
    pub fn bytes(&self) -> usize {
        self.words.len() * 4
    }
    /// Relocate pointer words, never coordinates, flags or packed colors.
    pub fn relocated(&self, offset: u32) -> Vec<u32> {
        let mut words = self.words.clone();
        for i in 0..self.nodes {
            let p = i * 8;
            if words[p + 3] & LEAF != 0 {
                words[p + 3] = (words[p + 3] & !LEAF) + offset | LEAF;
            } else {
                words[p + 3] += offset;
                words[p + 7] += offset;
            }
        }
        for i in 0..self.runs {
            words[(self.nodes + i) * 8 + 3] += offset;
        }
        words
    }
}
struct Column {
    low: [i32; 3],
    high: [i32; 3],
    first: u32,
    count: u32,
}
fn build(columns: &mut [Column], nodes: &mut Vec<[u32; 8]>, depth: usize) -> u32 {
    let index = nodes.len() as u32;
    nodes.push([0; 8]);
    let low: [i32; 3] = std::array::from_fn(|a| columns.iter().map(|c| c.low[a]).min().unwrap());
    let high: [i32; 3] = std::array::from_fn(|a| columns.iter().map(|c| c.high[a]).max().unwrap());
    let (left, right) = if columns.len() == 1 {
        (columns[0].first | LEAF, columns[0].count)
    } else {
        let axis = if depth % 2 == 0 { 0 } else { 2 };
        let mid = columns.len() / 2;
        columns.select_nth_unstable_by_key(mid, |c| c.low[axis]);
        let (a, b) = columns.split_at_mut(mid);
        (build(a, nodes, depth + 1), build(b, nodes, depth + 1))
    };
    nodes[index as usize] = [
        low[0] as u32,
        low[1] as u32,
        low[2] as u32,
        left,
        high[0] as u32,
        high[1] as u32,
        high[2] as u32,
        right,
    ];
    index
}
impl<G: Generator> Sampler<G> {
    pub fn ray_brick(
        &mut self,
        key: MeshKey,
        cancelled: impl Fn() -> bool,
    ) -> Result<Brick, String> {
        if key.revision != self.world().revision() {
            return Err("stale world mesh request".into());
        }
        let [ox, oz] = key.tile.origin_in(self.grid());
        let step = key.tile.step_in(self.grid());
        let mut columns = Vec::with_capacity(1024);
        let mut runs = Vec::new();
        let mut colors = Vec::new();
        let mut solid_top = i64::MAX;
        for iz in 0..SIDE {
            if cancelled() {
                return Err("mesh cancelled".into());
            }
            for ix in 0..SIDE {
                let x = ox + ix * step;
                let z = oz + iz * step;
                if x >= self.grid().extent || z >= self.grid().extent {
                    continue;
                }
                let own = self.profile(x, z, step);
                // Only native side-color coverage needs neighboring profiles.
                // Coarse bricks have one filtered color per run; sampling their
                // neighbors creates unused summaries and contends on the cache.
                let neighbors = (step == 1).then(|| {
                    [(x - step, z), (x + step, z), (x, z - step), (x, z + step)]
                        .map(|(x, z)| self.profile(x, z, step))
                });
                let first = runs.len() as u32;
                let mut low = i64::MAX;
                let mut high = 0;
                let mut contiguous = 0;
                for run in own.iter().filter(|r| r.material != MaterialId::AIR) {
                    let mut end = run.end;
                    if run.material == MaterialId::WATER && step > 1 {
                        // The shared coarse water level is independent of display spacing.
                        end = self.display_water_level(x, z, step).min(end);
                    }
                    if end <= run.start {
                        continue;
                    }
                    if run.start <= contiguous {
                        contiguous = contiguous.max(end);
                    }
                    low = low.min(run.start);
                    high = high.max(end);
                    let mut exposed = end - 1;
                    if step == 1 && run.material != MaterialId::WATER {
                        for neighbor in neighbors.as_ref().unwrap() {
                            for air in neighbor.iter().filter(|r| r.material == MaterialId::AIR) {
                                if air.start < end && air.end > run.start {
                                    exposed = exposed.min(run.start.max(air.start));
                                }
                            }
                        }
                    }
                    let start_color = colors.len() as u32;
                    let count = if step == 1 { (end - exposed) as u32 } else { 1 };
                    if colors.len() + count as usize > 262144 {
                        return Err("ray brick color budget exceeded".into());
                    }
                    for y in 0..count {
                        let c = self.color(
                            CellPos::new(
                                x,
                                if step == 1 {
                                    exposed + y as i64
                                } else {
                                    end - 1
                                },
                                z,
                            ),
                            run.material,
                            step,
                        );
                        colors.push(u32::from_le_bytes([c[0], c[1], c[2], 255]));
                    }
                    let light = if run.material == MaterialId::WATER {
                        255
                    } else {
                        self.ambient(CellPos::new(x, end - 1, z), 0, step)
                    };
                    runs.push([
                        run.start as u32,
                        end as u32,
                        if step == 1 {
                            let c = self.color(CellPos::new(x, run.start, z), run.material, step);
                            u32::from_le_bytes([c[0], c[1], c[2], 255])
                        } else {
                            colors[start_color as usize]
                        },
                        start_color,
                        exposed as u32,
                        count,
                        // Display spacing is implicit in column bounds. Keep
                        // flags independent of world resolution/large LOD steps.
                        (light as u32) | (u32::from(run.material == MaterialId::WATER) << 8),
                        0,
                    ]);
                }
                solid_top = solid_top.min(contiguous);
                if runs.len() as u32 > first {
                    columns.push(Column {
                        low: [x as i32, low as i32, z as i32],
                        high: [
                            (x + step).min(self.grid().extent) as i32,
                            high as i32,
                            (z + step).min(self.grid().extent) as i32,
                        ],
                        first,
                        count: runs.len() as u32 - first,
                    });
                }
            }
        }
        if columns.is_empty() {
            return Ok(Brick {
                words: vec![],
                nodes: 0,
                runs: 0,
                bounds: [[ox as i32, 0, oz as i32]; 2],
                solid_top: 0,
            });
        }
        let mut nodes = Vec::new();
        build(&mut columns, &mut nodes, 0);
        let bounds = [
            [nodes[0][0] as i32, nodes[0][1] as i32, nodes[0][2] as i32],
            [nodes[0][4] as i32, nodes[0][5] as i32, nodes[0][6] as i32],
        ];
        let run_base = nodes.len() as u32 * 8;
        let color_base = run_base + runs.len() as u32 * 8;
        for node in &mut nodes {
            if node[3] & LEAF != 0 {
                node[3] = LEAF | (run_base + (node[3] & !LEAF) * 8);
            } else {
                node[3] *= 8;
                node[7] *= 8;
            }
        }
        for run in &mut runs {
            run[3] += color_base;
        }
        let node_count = nodes.len();
        let run_count = runs.len();
        let mut words = Vec::with_capacity(color_base as usize + colors.len());
        for n in nodes {
            words.extend(n);
        }
        for r in runs {
            words.extend(r);
        }
        words.extend(colors);
        Ok(Brick {
            words,
            nodes: node_count,
            runs: run_count,
            bounds,
            solid_top: solid_top.max(0),
        })
    }
}

/// CPU reference traversal used for GPU-format validation and edited-brick tests.
/// Inputs and returned distance are in native cell units.
pub fn trace(
    brick: &Brick,
    origin: [f64; 3],
    direction: [f64; 3],
) -> Option<([i64; 3], [u8; 3], f64)> {
    if brick.nodes == 0 {
        return None;
    }
    let span = |low: [f64; 3], high: [f64; 3]| {
        let mut near = f64::NEG_INFINITY;
        let mut far = f64::INFINITY;
        for i in 0..3 {
            if direction[i].abs() < 1e-12 {
                if origin[i] < low[i] || origin[i] >= high[i] {
                    return None;
                }
            } else {
                let a = (low[i] - origin[i]) / direction[i];
                let b = (high[i] - origin[i]) / direction[i];
                near = near.max(a.min(b));
                far = far.min(a.max(b));
            }
        }
        (far >= near.max(0.)).then_some((near, far))
    };
    let d = &brick.words;
    let mut stack = vec![0usize];
    let mut closest = f64::INFINITY;
    let mut result = None;
    while let Some(n) = stack.pop() {
        let low = [d[n] as f64, d[n + 1] as f64, d[n + 2] as f64];
        let high = [d[n + 4] as f64, d[n + 5] as f64, d[n + 6] as f64];
        let Some((near, _)) = span(low, high) else {
            continue;
        };
        if near > closest {
            continue;
        }
        if d[n + 3] & LEAF == 0 {
            stack.extend([d[n + 3] as usize, d[n + 7] as usize]);
            continue;
        }
        for i in 0..d[n + 7] {
            let p = (d[n + 3] & !LEAF) as usize + i as usize * 8;
            if let Some((near, far)) = span(
                [low[0], d[p] as f64, low[2]],
                [high[0], d[p + 1] as f64, high[2]],
            ) {
                let t = if near < 0. { far } else { near };
                if t < closest {
                    closest = t;
                    let cell = std::array::from_fn(|i| {
                        (origin[i] + direction[i] * (t + 1e-6)).floor() as i64
                    });
                    let offset = (cell[1] - d[p + 4] as i64).clamp(0, d[p + 5] as i64 - 1) as usize;
                    let packed = if cell[1] < d[p + 4] as i64 {
                        d[p + 2]
                    } else {
                        d[d[p + 3] as usize + offset]
                    };
                    let rgb = packed.to_le_bytes();
                    result = Some((cell, [rgb[0], rgb[1], rgb[2]], t));
                }
            }
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        generation::FlatTerrain,
        materials::{Appearance, Voxel},
        render::lod::TileKey,
        Edit, World,
    };
    use std::sync::Arc;
    #[test]
    fn exact_native_rays_preserve_caves_and_painted_voxels() {
        let mut world = World::new(FlatTerrain {
            height: 16,
            water: 16,
            seed: 42,
        });
        let hole = CellPos::new(40, 15, 40);
        let paint = CellPos::new(40, 14, 40);
        world
            .apply(&[
                Edit::set(hole, Voxel::AIR),
                Edit::set(paint, Voxel::new(MaterialId::ROCK, Appearance(31415))),
            ])
            .unwrap();
        let expected =
            crate::painting::base_color(world.voxel_at(paint).known().unwrap(), world.materials());
        let mut s = Sampler::new(Arc::new(world));
        let key = MeshKey {
            corner_levels: None,
            tile: TileKey {
                level: 13,
                x: 1,
                z: 1,
            },
            neighbors: [13; 4],
            revision: 1,
        };
        let brick = s.ray_brick(key, || false).unwrap();
        let (cell, color, distance) = trace(&brick, [40.5, 30., 40.5], [0., -1., 0.]).unwrap();
        assert_eq!(cell, [40, 14, 40]);
        assert_eq!(color, expected);
        assert!((distance - 15.).abs() < 1e-6);
        assert!(trace(&brick, [80., 30., 80.], [0., -1., 0.]).is_none());
    }
    #[test]
    fn cave_ceiling_retains_its_own_native_color() {
        let mut world = World::new(FlatTerrain {
            height: 128,
            water: 128,
            seed: 42,
        });
        let ceiling = CellPos::new(40, 64, 40);
        world
            .apply(&[
                Edit::set(CellPos::new(40, 63, 40), Voxel::AIR),
                Edit::set(ceiling, Voxel::new(MaterialId::ROCK, Appearance(12345))),
            ])
            .unwrap();
        let color = crate::painting::base_color(
            world.voxel_at(ceiling).known().unwrap(),
            world.materials(),
        );
        let mut sampler = Sampler::new(Arc::new(world));
        let key = MeshKey {
            corner_levels: None,
            tile: TileKey {
                level: 13,
                x: 1,
                z: 1,
            },
            neighbors: [13; 4],
            revision: 1,
        };
        let brick = sampler.ray_brick(key, || false).unwrap();
        let (cell, rgb, _) = trace(&brick, [40.5, 63.5, 40.5], [0., 1., 0.]).unwrap();
        assert_eq!(cell, [40, 64, 40]);
        assert_eq!(rgb, color);
    }
    #[test]
    fn sparse_runs_do_not_expand_the_solid_interior_and_cancellation_works() {
        let mut s = Sampler::new(Arc::new(World::new(FlatTerrain {
            height: 30000,
            water: 30000,
            seed: 42,
        })));
        let key = MeshKey {
            corner_levels: None,
            tile: TileKey {
                level: 13,
                x: 1,
                z: 1,
            },
            neighbors: [13; 4],
            revision: 0,
        };
        assert_eq!(s.ray_brick(key, || true).unwrap_err(), "mesh cancelled");
        let brick = s.ray_brick(key, || false).unwrap();
        assert!(brick.bytes() < 200000);
        let moved = brick.relocated(131072);
        assert_eq!(moved.len(), brick.words.len());
        assert_eq!(brick.words[0], moved[0]);
        assert_eq!(moved[3], brick.words[3] + 131072);
    }
}
