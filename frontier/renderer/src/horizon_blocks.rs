//! Exposed faces of the nested flight horizon. Coordinates are relative to
//! the patch origin in units of the finest horizon column.
use super::{Sample, LEVELS, SIDE, SPACING};
use std::ops::Range;

const CELLS: i32 = 2 * (SIDE as i32 - 1);
const HALF: i32 = CELLS / 2;
const CHUNK: i32 = 32;
pub(super) const UNIT: f32 = SPACING as f32 / (2 << (LEVELS - 1)) as f32;

#[derive(Clone, Copy, Default, PartialEq)]
struct Column {
    y: f32,
    rgb: u32,
    slope: u32,
    water: u32,
    depth: u32,
}

pub(super) struct Batch {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub faces: [Range<u32>; 5],
}

pub(super) struct Mesh {
    pub faces: Vec<[u32; 4]>,
    pub batches: Vec<Batch>,
}

// Each half-open square selects its finest available ring. Continuous world
// indexing also covers the negative sides of a ring and its four corners.
fn address(x: i32, z: i32) -> Option<(usize, usize)> {
    for level in 0..LEVELS {
        let x = x.div_euclid(1 << level) + HALF;
        let z = z.div_euclid(1 << level) + HALF;
        if (0..CELLS).contains(&x) && (0..CELLS).contains(&z) {
            return Some((level, (z * CELLS + x) as usize));
        }
    }
    None
}

fn hole(level: usize, x: i32, z: i32) -> bool {
    level > 0
        && [x, z]
            .into_iter()
            .all(|v| (-HALF / 2..HALF / 2).contains(&v))
}

fn columns(samples: &[Sample], voxel: f32) -> Vec<Column> {
    let mut result = vec![Column::default(); LEVELS * (CELLS * CELLS) as usize];
    for level in 0..LEVELS {
        let vertical_step = ((UNIT * (1 << level) as f32 / 64.) / voxel).round().max(1.) * voxel;
        for z in 0..CELLS {
            for x in 0..CELLS {
                if hole(level, x - HALF, z - HALF) {
                    continue;
                }
                let i = level * SIDE * SIDE + (z / 2) as usize * SIDE + (x / 2) as usize;
                let u = 0.25 + 0.5 * (x % 2) as f32;
                let v = 0.25 + 0.5 * (z % 2) as f32;
                let at = |k: usize| {
                    let a = samples[i][k] + u * (samples[i + 1][k] - samples[i][k]);
                    let b = samples[i + SIDE][k]
                        + u * (samples[i + SIDE + 1][k] - samples[i + SIDE][k]);
                    a + v * (b - a)
                };
                let a = samples[i][3];
                let b = samples[i + 1][3];
                let c = samples[i + SIDE][3];
                let d = samples[i + SIDE + 1][3];
                let dx = (a - b) * (1. - v) + (c - d) * v;
                let dz = (a - c) * (1. - u) + (b - d) * u;
                let length = dx.abs() + dz.abs() + 2. * UNIT * (1 << level) as f32;
                let encode = |n: f32| ((15. * n / length).round() as i32 as u32) & 31;
                let rgb = |k| (at(k).clamp(0., 1.) * 255.).round() as u32;
                let wet = at(4).clamp(0., 1.);
                let water = (7. * wet).round() as u32;
                let depth = if water > 0 {
                    (4. * at(5) / wet).round().clamp(0., 48.) as u32
                } else {
                    0
                };
                result[level * (CELLS * CELLS) as usize + (z * CELLS + x) as usize] = Column {
                    water,
                    depth,
                    y: if a == b && a == c && a == d {
                        a
                    } else {
                        (at(3) / vertical_step).round() * vertical_step
                    },
                    slope: encode(dx) | (encode(dz) << 5),
                    rgb: rgb(0) | rgb(1) << 8 | rgb(2) << 16,
                };
            }
        }
    }
    result
}

fn face(x: i32, z: i32, axis: u32, level: usize, low: f32, high: Column) -> [u32; 4] {
    [
        (x as u32 & 2047) | ((z as u32 & 2047) << 11) | (high.slope << 22),
        high.rgb | (axis << 24) | ((level as u32) << 27) | (high.water << 29),
        if axis == 0 {
            (low + (high.depth << 10) as f32).to_bits()
        } else {
            low.to_bits()
        },
        high.y.to_bits(),
    ]
}

/// Merge coplanar, identically lit roofs inside a draw batch. Only zero-slope
/// roofs qualify because the horizon mixes relief lighting by face distance.
fn roofs(columns: &[Column], level: usize, cx: i32, cz: i32, output: &mut Vec<[u32; 4]>) {
    let step = 1 << level;
    #[derive(Clone, Copy)]
    struct RoofRun {
        row: i32,
        width: i32,
        index: usize,
        column: Column,
    }
    let mut previous = [None::<RoofRun>; CHUNK as usize];
    let column =
        |x, z| columns[level * (CELLS * CELLS) as usize + ((z + HALF) * CELLS + x + HALF) as usize];
    for z in cz..(cz + CHUNK).min(HALF) {
        let mut x = cx;
        while x < (cx + CHUNK).min(HALF) {
            if hole(level, x, z) {
                x += 1;
                continue;
            }
            let high = column(x, z);
            let mut width = 1;
            if high.slope == 0 {
                while x + width < (cx + CHUNK).min(HALF)
                    && !hole(level, x + width, z)
                    && column(x + width, z) == high
                {
                    width += 1;
                }
            }
            let slot = &mut previous[(x - cx) as usize];
            let index = if let Some(run) = slot.filter(|run| {
                high.slope == 0 && run.row == z - 1 && run.width == width && run.column == high
            }) {
                let face = &mut output[run.index];
                // The next five bits of the roof word encode its z span.
                face[2] = (f32::from_bits(face[2]) + CHUNK as f32).to_bits();
                run.index
            } else {
                output.push(face(x * step, z * step, 0, level, (width - 1) as f32, high));
                output.len() - 1
            };
            *slot = Some(RoofRun {
                row: z,
                width,
                index,
                column: high,
            });
            x += width;
        }
    }
}

pub(super) fn build(samples: &[Sample], voxel: f32) -> Mesh {
    let columns = columns(samples, voxel);
    let mut mesh = Mesh {
        faces: Vec::with_capacity((CELLS * CELLS) as usize * 6),
        batches: Vec::new(),
    };
    let mut directions: [Vec<[u32; 4]>; 5] = Default::default();
    for level in 0..LEVELS {
        let step = 1 << level;
        for cz in (-HALF..HALF).step_by(CHUNK as usize) {
            for cx in (-HALF..HALF).step_by(CHUNK as usize) {
                let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
                roofs(&columns, level, cx, cz, &mut directions[0]);
                for z in cz..(cz + CHUNK).min(HALF) {
                    for x in cx..(cx + CHUNK).min(HALF) {
                        if hole(level, x, z) {
                            continue;
                        }
                        let high = columns[level * (CELLS * CELLS) as usize
                            + ((z + HALF) * CELLS + x + HALF) as usize];
                        let (x, z) = (x * step, z * step);
                        min_y = min_y.min(high.y);
                        max_y = max_y.max(high.y);
                        for (axis, dx, dz) in [(1, 1, 0), (2, -1, 0), (3, 0, 1), (4, 0, -1)] {
                            // Place the lookup in the neighboring half-open cell.
                            let ax = x + if dx > 0 { step } else { 0 };
                            let az = z + if dz > 0 { step } else { 0 };
                            let nx = ax - i32::from(dx < 0);
                            let nz = az - i32::from(dz < 0);
                            let Some((other, i)) = address(nx, nz) else {
                                continue;
                            };
                            let wall_level = other.min(level);
                            let mut wall = |off, other: usize, index: usize| {
                                let low = columns[other * (CELLS * CELLS) as usize + index].y;
                                if high.y > low {
                                    directions[axis as usize].push(face(
                                        ax + if dx == 0 { off } else { 0 },
                                        az + if dz == 0 { off } else { 0 },
                                        axis,
                                        wall_level,
                                        low,
                                        high,
                                    ));
                                    min_y = min_y.min(low);
                                }
                            };
                            wall(0, other, i);
                            if other < level {
                                let half = step / 2;
                                let (other, i) = address(
                                    nx + if dx == 0 { half } else { 0 },
                                    nz + if dz == 0 { half } else { 0 },
                                )
                                .expect("second half of nested ring");
                                wall(half, other, i);
                            }
                        }
                    }
                }
                if !directions[0].is_empty() {
                    let faces = std::array::from_fn(|axis| {
                        let first = mesh.faces.len() as u32;
                        mesh.faces.append(&mut directions[axis]);
                        first..mesh.faces.len() as u32
                    });
                    mesh.batches.push(Batch {
                        faces,
                        min: [(cx * step) as f32 * UNIT, min_y, (cz * step) as f32 * UNIT],
                        max: [
                            ((cx + CHUNK).min(HALF) * step) as f32 * UNIT,
                            max_y,
                            ((cz + CHUNK).min(HALF) * step) as f32 * UNIT,
                        ],
                    });
                }
            }
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn neighboring_rings_own_half_open_boundaries() {
        for (x, z, level) in [
            (-160, -160, 0),
            (159, 159, 0),
            (-161, 0, 1),
            (160, 0, 1),
            (0, -321, 2),
            (0, 320, 2),
        ] {
            assert_eq!(address(x, z).unwrap().0, level);
        }
        assert!(address(640, 0).is_none());
        assert!(address(-641, 0).is_none());
    }
    #[test]
    fn wet_horizon_roofs_keep_their_depth_and_merge_only_matching_beds() {
        let mut c = vec![
            Column {
                y: 0.,
                rgb: 123,
                slope: 0,
                water: 7,
                depth: 16
            };
            (CELLS * CELLS) as usize
        ];
        c[(HALF * CELLS + HALF) as usize].depth = 2;
        let mut faces = vec![];
        roofs(&c, 0, 0, 0, &mut faces);
        let mut areas = [0; 2];
        for face in faces {
            assert_eq!(face[1] >> 29, 7);
            let packed = f32::from_bits(face[2]) as u32;
            let area = (1 + (packed & 31)) * (1 + ((packed >> 5) & 31));
            match packed >> 10 {
                2 => areas[0] += area,
                16 => areas[1] += area,
                depth => panic!("unexpected depth {depth}"),
            };
        }
        assert_eq!(areas, [1, (CHUNK * CHUNK - 1) as u32]);
    }

    #[test]
    fn flat_horizon_roofs_cover_each_column_once_without_buried_faces() {
        let lattice = vec![[0.5, 0.25, 0.1, 7.5, 0., 0.]; LEVELS * SIDE * SIDE];
        let mesh = build(&lattice, 0.0625);
        let ring = (CELLS * CELLS) as usize;
        assert!(mesh.faces.len() < 400);
        let mut covered = vec![false; ring * LEVELS];
        for f in &mesh.faces {
            let level = ((f[1] >> 27) & 3) as usize;
            let x = ((f[0] as i32) << 21 >> 21) / (1 << level);
            let z = (((f[0] >> 11) as i32) << 21 >> 21) / (1 << level);
            let packed = f32::from_bits(f[2]) as i32;
            for zz in z..z + 1 + packed / CHUNK {
                for xx in x..x + 1 + packed % CHUNK {
                    assert!(!hole(level, xx, zz));
                    assert!(!std::mem::replace(
                        &mut covered[level * ring + ((zz + HALF) * CELLS + xx + HALF) as usize],
                        true
                    ));
                }
            }
        }
        assert_eq!(
            covered.into_iter().filter(|v| *v).count(),
            ring + 2 * (ring - ring / 4)
        );
        assert!(mesh
            .faces
            .iter()
            .all(|f| (f[1] >> 24) & 7 == 0 && f32::from_bits(f[3]) == 7.5));
    }
}
