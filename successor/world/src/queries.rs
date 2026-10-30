//! Queries always inspect canonical cells, never display meshes or LOD samples.
use crate::{
    generation::Generator,
    grid::CellPos,
    materials::Voxel,
    world::{Lookup, World},
};
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub cell: CellPos,
    pub voxel: Voxel,
    pub distance: f64,
    pub normal: [i8; 3],
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Trace {
    Hit(RayHit),
    Miss,
    Unknown(CellPos),
}

pub fn raycast<G: Generator>(
    world: &World<G>,
    origin: [f64; 3],
    direction: [f64; 3],
    reach: f64,
) -> Trace {
    let length = direction.iter().map(|n| n * n).sum::<f64>().sqrt();
    if !length.is_finite()
        || length <= 0.0
        || !reach.is_finite()
        || !(0.0..=4096.0).contains(&reach)
    {
        return Trace::Miss;
    }
    let dir = direction.map(|n| n / length);
    let Some(mut cell) = world
        .spec()
        .grid
        .position(std::array::from_fn(|i| origin[i] + dir[i] * 1e-8))
    else {
        return Trace::Miss;
    };
    let step = dir.map(|v| {
        if v > 0.0 {
            1
        } else if v < 0.0 {
            -1
        } else {
            0
        }
    });
    let delta = dir.map(|v| {
        if v == 0.0 {
            f64::INFINITY
        } else {
            world.spec().grid.meters() / v.abs()
        }
    });
    let c = [cell.x, cell.y, cell.z];
    let mut next = std::array::from_fn::<_, 3, _>(|i| {
        if dir[i] == 0.0 {
            f64::INFINITY
        } else {
            ((c[i] + i64::from(step[i] > 0)) as f64 * world.spec().grid.meters() - origin[i])
                / dir[i]
        }
    });
    let mut distance = 0.0;
    let mut normal = [0i8; 3];
    // The old fixed iteration count silently shortened long rays on finer grids.
    // This bound follows the maximum number of crossed axis planes, not meters.
    let crossings = (reach / world.spec().grid.meters() * dir.iter().map(|v| v.abs()).sum::<f64>())
        .ceil() as u64
        + 4;
    for _ in 0..crossings {
        if distance > reach {
            return Trace::Miss;
        }
        match world.voxel_at(cell) {
            Lookup::Unknown => return Trace::Unknown(cell),
            Lookup::Known(v) if !v.is_air() => {
                return Trace::Hit(RayHit {
                    cell,
                    voxel: v,
                    distance,
                    normal,
                })
            }
            _ => {}
        }
        let boundary = next.iter().copied().fold(f64::INFINITY, f64::min);
        if !boundary.is_finite() || boundary > reach {
            return Trace::Miss;
        }
        let mut change = [0; 3];
        normal = [0; 3];
        for axis in 0..3 {
            if next[axis] <= boundary + 1e-10 {
                change[axis] = step[axis];
                normal[axis] = -step[axis] as i8;
                next[axis] += delta[axis];
            }
        }
        let Some(next_cell) = cell.offset(change) else {
            return Trace::Miss;
        };
        cell = next_cell;
        distance = boundary.max(0.0);
    }
    Trace::Miss
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sweep {
    pub position: CellPos,
    pub blocked: bool,
    pub visited: usize,
}
/// Point-camera movement, not rigid-body physics. Every crossed cell and corner
/// neighbor is checked, so a large displacement cannot tunnel through a wall.
pub fn sweep_point<G: Generator>(world: &World<G>, start: CellPos, delta: [i64; 3]) -> Sweep {
    let mut result = Sweep {
        position: start,
        blocked: false,
        visited: 0,
    };
    if delta.iter().any(|n| n.unsigned_abs() > 2048) {
        result.blocked = true;
        return result;
    }
    let count = delta.iter().map(|n| n.abs()).max().unwrap_or(0);
    if count == 0 {
        return result;
    }
    let round = |n: i128, d: i128| {
        if n >= 0 {
            (n + d / 2) / d
        } else {
            -((-n + d / 2) / d)
        }
    };
    for i in 1..=count {
        let offset = delta.map(|n| round(n as i128 * i as i128, count as i128) as i64);
        let Some(next) = start.offset(offset) else {
            result.blocked = true;
            break;
        };
        let change = [
            next.x - result.position.x,
            next.y - result.position.y,
            next.z - result.position.z,
        ];
        for mask in 1..8 {
            let candidate_delta = std::array::from_fn(|axis| {
                if mask & (1 << axis) != 0 {
                    change[axis]
                } else {
                    0
                }
            });
            if candidate_delta == [0, 0, 0] {
                continue;
            }
            let Some(p) = result.position.offset(candidate_delta) else {
                result.blocked = true;
                return result;
            };
            result.visited += 1;
            if !matches!(world.voxel_at(p),Lookup::Known(v) if v.is_air()) {
                result.blocked = true;
                return result;
            }
        }
        result.position = next;
    }
    result
}
