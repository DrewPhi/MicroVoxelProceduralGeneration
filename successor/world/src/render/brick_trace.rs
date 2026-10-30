//! Bounded sparse-brick CPU ray traversal candidate. Coordinates are continuous
//! canonical cell coordinates; visual LOD never participates in these reads.
use crate::{
    generation::Generator,
    grid::{Bounds, CellPos, ChunkPos, CHUNK_VOLUME},
    materials::MaterialId,
    snapshot::WorldSnapshot,
};
use std::collections::BTreeMap;
const WORDS: usize = CHUNK_VOLUME / 64;
pub const MAX_INDEXED_BRICKS: usize = 256;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trace {
    Hit { pos: CellPos, material: MaterialId },
    Miss,
    Unknown(CellPos),
    Incomplete,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildError {
    InvalidBounds,
    TooLarge,
}
impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for BuildError {}
struct Occupancy {
    solid: Box<[u64; WORDS]>,
    unknown: Box<[u64; WORDS]>,
    materials: Box<[MaterialId; CHUNK_VOLUME]>,
}
impl Occupancy {
    fn new() -> Self {
        Self {
            solid: Box::new([0; WORDS]),
            unknown: Box::new([0; WORDS]),
            materials: Box::new([MaterialId::AIR; CHUNK_VOLUME]),
        }
    }
    fn set(&mut self, p: CellPos, material: Option<MaterialId>) {
        let i = p.local_index() as usize;
        let bit = 1u64 << (i % 64);
        if let Some(material) = material {
            self.materials[i] = material;
            if material != MaterialId::AIR {
                self.solid[i / 64] |= bit;
            }
        } else {
            self.unknown[i / 64] |= bit;
        }
    }
    fn get(&self, p: CellPos) -> Trace {
        let i = p.local_index() as usize;
        let bit = 1u64 << (i % 64);
        if self.unknown[i / 64] & bit != 0 {
            Trace::Unknown(p)
        } else if self.solid[i / 64] & bit != 0 {
            Trace::Hit {
                pos: p,
                material: self.materials[i],
            }
        } else {
            Trace::Miss
        }
    }
}
pub struct BrickTrace {
    bounds: Bounds,
    bricks: BTreeMap<ChunkPos, Occupancy>,
}
impl BrickTrace {
    /// Material runs fill each column where the source provides it. Unknown
    /// columns fall back to exact cells so a partial source stays explicit.
    pub fn build<G: Generator>(
        snapshot: &WorldSnapshot<G>,
        bounds: Bounds,
    ) -> Result<Self, BuildError> {
        let volume = bounds.volume().ok_or(BuildError::InvalidBounds)?;
        if volume == 0 {
            return Err(BuildError::InvalidBounds);
        }
        if volume > MAX_INDEXED_BRICKS * CHUNK_VOLUME {
            return Err(BuildError::TooLarge);
        }
        let mut bricks = BTreeMap::new();
        for z in bounds.min.z..bounds.max.z {
            for x in bounds.min.x..bounds.max.x {
                let runs = snapshot
                    .column_runs(x, z, bounds.min.y..bounds.max.y)
                    .map_err(|_| BuildError::TooLarge)?;
                if let Some(runs) = runs {
                    for run in runs {
                        for y in run.start..run.end {
                            let p = CellPos::new(x, y, z);
                            bricks
                                .entry(p.chunk())
                                .or_insert_with(Occupancy::new)
                                .set(p, Some(run.material));
                        }
                    }
                } else {
                    for y in bounds.min.y..bounds.max.y {
                        let p = CellPos::new(x, y, z);
                        bricks
                            .entry(p.chunk())
                            .or_insert_with(Occupancy::new)
                            .set(p, snapshot.voxel_at(p).known().map(|v| v.material));
                    }
                }
                if bricks.len() > MAX_INDEXED_BRICKS {
                    return Err(BuildError::TooLarge);
                }
            }
        }
        Ok(Self { bounds, bricks })
    }
    /// Max distance is in cell edges. A finite positive step budget is required.
    /// Leaving indexed bounds or reaching the step budget returns Incomplete.
    pub fn trace(
        &self,
        origin: [f64; 3],
        direction: [f64; 3],
        max_distance: f64,
        max_steps: usize,
    ) -> Trace {
        if !origin.iter().chain(direction.iter()).all(|v| v.is_finite())
            || !max_distance.is_finite()
            || max_distance < 0.
            || max_steps == 0
        {
            return Trace::Incomplete;
        }
        let norm = direction.iter().map(|v| v * v).sum::<f64>().sqrt();
        if !norm.is_finite() || norm == 0. {
            return Trace::Incomplete;
        }
        let dir = direction.map(|v| v / norm);
        let mut cell = [0i64; 3];
        let mut step = [0i64; 3];
        let mut next = [f64::INFINITY; 3];
        let mut delta = [f64::INFINITY; 3];
        for axis in 0..3 {
            // f64 cannot identify adjacent cells beyond 2^53.
            if origin[axis].abs() >= (1u64 << 53) as f64 {
                return Trace::Incomplete;
            }
            cell[axis] = origin[axis].floor() as i64;
            if dir[axis] > 0. {
                step[axis] = 1;
                next[axis] = (cell[axis] as f64 + 1. - origin[axis]) / dir[axis];
                delta[axis] = 1. / dir[axis];
            } else if dir[axis] < 0. {
                step[axis] = -1;
                next[axis] = (origin[axis] - cell[axis] as f64) / -dir[axis];
                delta[axis] = 1. / -dir[axis];
            }
        }
        for _ in 0..max_steps {
            let p = CellPos::new(cell[0], cell[1], cell[2]);
            if !self.bounds.contains(p) {
                return Trace::Incomplete;
            }
            let Some(brick) = self.bricks.get(&p.chunk()) else {
                return Trace::Unknown(p);
            };
            match brick.get(p) {
                Trace::Miss => {}
                result => return result,
            }
            let axis = if next[0] <= next[1] && next[0] <= next[2] {
                0
            } else if next[1] <= next[2] {
                1
            } else {
                2
            };
            let distance = next[axis];
            if distance > max_distance {
                return Trace::Miss;
            }
            // A ray through an edge/corner only enters the diagonal cell.
            for a in 0..3 {
                if next[a] <= distance {
                    let Some(n) = cell[a].checked_add(step[a]) else {
                        return Trace::Incomplete;
                    };
                    cell[a] = n;
                    next[a] += delta[a];
                }
            }
        }
        Trace::Incomplete
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        generation::GeneratorIdentity,
        materials::{Appearance, Voxel},
        world::World,
    };
    struct Cave;
    impl Generator for Cave {
        fn identity(&self) -> GeneratorIdentity {
            GeneratorIdentity {
                seed: 1,
                version: 1,
                definition: 1,
            }
        }
        fn voxel(&self, p: CellPos) -> Option<Voxel> {
            if p.x.abs() > 20 || p.y.abs() > 20 || p.z.abs() > 20 {
                return None;
            }
            let solid = p.y <= 0 && !(p.y == 0 && (-3..=3).contains(&p.x) && p.z == 0);
            Some(if solid {
                Voxel::new(MaterialId::ROCK, Appearance(7))
            } else {
                Voxel::AIR
            })
        }
    }
    #[test]
    fn cave_negative_cells_and_explicit_unknown() {
        let world = World::new(Cave);
        let snap = world.read_snapshot();
        let bounds = Bounds {
            min: CellPos::new(-24, -4, -2),
            max: CellPos::new(24, 4, 3),
        };
        let grid = BrickTrace::build(&snap, bounds).unwrap();
        assert_eq!(
            grid.trace([-2.5, 0.5, 0.5], [1., 0., 0.], 5., 100),
            Trace::Miss
        );
        assert_eq!(
            grid.trace([-10.5, -1.5, 0.5], [1., 0., 0.], 2., 100),
            Trace::Hit {
                pos: CellPos::new(-11, -2, 0),
                material: MaterialId::ROCK
            }
        );
        assert_eq!(
            grid.trace([-23.5, 2.5, 0.5], [1., 0., 0.], 10., 100),
            Trace::Unknown(CellPos::new(-24, 2, 0))
        );
        assert_eq!(
            grid.trace([0.5, 2.5, 0.5], [0., 0., 0.], 10., 100),
            Trace::Incomplete
        );
        assert_eq!(
            grid.trace([0.5, 2.5, 0.5], [1., 0., 0.], 10., 1),
            Trace::Incomplete
        );
        assert_eq!(
            grid.trace([0.5, 2.5, 0.5], [1., 0., 0.], 2., 100),
            Trace::Miss
        );
        for y in [-2, 0, 2] {
            let expected = (-5..=3)
                .find_map(|x| {
                    let p = CellPos::new(x, y, 1);
                    world
                        .voxel_at(p)
                        .known()
                        .filter(|v| !v.is_air())
                        .map(|v| Trace::Hit {
                            pos: p,
                            material: v.material,
                        })
                })
                .unwrap_or(Trace::Miss);
            assert_eq!(
                grid.trace([-4.5, y as f64 + 0.5, 1.5], [1., 0., 0.], 8., 100),
                expected
            );
        }
    }
    #[test]
    fn corner_crossing_does_not_hit_touch_only_cells() {
        let world = World::new(Cave);
        let grid = BrickTrace::build(
            &world.read_snapshot(),
            Bounds {
                min: CellPos::new(-4, -2, -4),
                max: CellPos::new(5, 3, 5),
            },
        )
        .unwrap();
        // Starts in air, then crosses x/y simultaneously into a solid diagonal.
        assert_eq!(
            grid.trace([-0.5, 1.5, 1.5], [1., -1., 0.], 4., 20),
            Trace::Hit {
                pos: CellPos::new(0, 0, 1),
                material: MaterialId::ROCK
            }
        );
    }
}
