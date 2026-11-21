//! Exposed faces of the nested flight horizon. Coordinates are relative to
//! the patch origin and packed in units of each face’s own ring.
use super::{Lattice, NativeRoof, LEVELS, SIDE, SPACING};
use std::ops::Range;

const CELLS: i32 = 2 * (SIDE as i32 - 1);
const RING: usize = (CELLS * CELLS) as usize;
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
    soil_rgb: u32,
    rock_rgb: u32,
}

impl Column {
    fn same_roof(self, other: Self) -> bool {
        self.y == other.y
            && self.rgb == other.rgb
            && self.slope == other.slope
            && self.water == other.water
            && self.depth == other.depth
    }

    fn wall_rgb(self, low: f32, voxel: f32) -> u32 {
        let height = self.y - low;
        let water = (self.depth as f32 * 0.25).min(height);
        let wet = self.water as f32 / 7.;
        // Integrate the native surface cell, eleven subsurface cells and the
        // remaining rock over the exposed wall. Wet columns start at the bed.
        let bands = |h: f32| {
            let skin = h.min(voxel);
            let soil = (h - skin).min(11. * voxel);
            [skin, soil, h - skin - soil]
        };
        let dry_bands = bands(height);
        let weight = if self.water == 0 || self.depth == 0 {
            if height <= voxel || (self.rgb == self.soil_rgb && self.rgb == self.rock_rgb) {
                return self.rgb;
            }
            dry_bands.map(|h| h / height)
        } else {
            let wet_bands = bands(height - water);
            std::array::from_fn(|k| (dry_bands[k] + wet * (wet_bands[k] - dry_bands[k])) / height)
        };
        let water_rgb = frontier_world::Material::Water.color();
        let mut packed = 0;
        for (k, channel) in water_rgb.into_iter().enumerate() {
            let shift = k * 8;
            let byte = |rgb| ((rgb >> shift) & 255u32) as f32;
            let color = weight[0] * byte(self.rgb)
                + weight[1] * byte(self.soil_rgb)
                + weight[2] * byte(self.rock_rgb)
                + 255. * channel * (wet * water / height);
            packed |= (color.round().clamp(0., 255.) as u32) << shift;
        }
        packed
    }
}

pub(super) struct Batch {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub faces: [Range<u32>; 5],
    pub wet_center: Option<glam::Vec3>,
}

pub(super) struct Mesh {
    pub faces: Vec<[u32; 4]>,
    pub batches: Vec<Batch>,
}

fn extent(level: usize) -> i32 {
    2 * super::sample_radius(level) as i32
}

// Each half-open square selects its finest available ring on all four sides.
fn address(first: usize, x: i32, z: i32) -> Option<(usize, usize)> {
    for level in first..LEVELS {
        let x = x.div_euclid(1 << level);
        let z = z.div_euclid(1 << level);
        let half = extent(level);
        if (-half..half).contains(&x) && (-half..half).contains(&z) {
            return Some((level, ((z + HALF) * CELLS + x + HALF) as usize));
        }
    }
    None
}

fn hole(first: usize, level: usize, x: i32, z: i32) -> bool {
    level > first
        && [x, z]
            .into_iter()
            .all(|v| (-extent(level - 1) / 2..extent(level - 1) / 2).contains(&v))
}

/// A visible roof represents the native column at its world-aligned center.
/// Its color can cover a wider area, but averaging elevations erases crests.
pub(super) fn sample_roofs(
    center: [i64; 2],
    first: usize,
    previous: Option<&Lattice>,
    mut roof: impl FnMut(f64, f64) -> NativeRoof,
) -> Vec<NativeRoof> {
    let mut roofs = vec![NativeRoof::default(); (LEVELS - first) * RING];
    for level in first..LEVELS {
        let base = (level - first) * RING;
        let step = f64::from(UNIT) * (1 << level) as f64;
        let old = previous
            .filter(|lattice| level >= lattice.first)
            .and_then(|lattice| {
                let scale = 1_i128 << (LEVELS - level);
                let [dx, dz] = std::array::from_fn(|k| {
                    (i128::from(center[k]) - i128::from(lattice.center[k])) * scale
                });
                (dx.abs() < i128::from(CELLS) && dz.abs() < i128::from(CELLS)).then(|| {
                    (
                        &lattice.roofs[(level - lattice.first) * RING..],
                        lattice.first,
                        dx as i32,
                        dz as i32,
                    )
                })
            });
        let index = |x, z| ((z + HALF) * CELLS + x + HALF) as usize;
        let half = extent(level);
        for z in -half..half {
            for x in -half..half {
                if hole(first, level, x, z) {
                    continue;
                }
                let value = &mut roofs[base + index(x, z)];
                if let Some((heights, old_first, dx, dz)) = old {
                    let (ox, oz) = (x + dx, z + dz);
                    if (-half..half).contains(&ox)
                        && (-half..half).contains(&oz)
                        && !hole(old_first, level, ox, oz)
                    {
                        *value = heights[index(ox, oz)];
                        continue;
                    }
                }
                *value = roof(
                    center[0] as f64 * SPACING + (x as f64 + 0.5) * step,
                    center[1] as f64 * SPACING + (z as f64 + 0.5) * step,
                );
            }
        }
    }
    roofs
}

fn columns(lattice: &Lattice) -> Vec<Column> {
    let first = lattice.first;
    let samples = &lattice.samples;
    let mut result = vec![Column::default(); (LEVELS - first) * RING];
    for level in first..LEVELS {
        let half = extent(level);
        for z in HALF - half..HALF + half {
            for x in HALF - half..HALF + half {
                if hole(first, level, x - HALF, z - HALF) {
                    continue;
                }
                let i = (level - first) * SIDE * SIDE + (z / 2) as usize * SIDE + (x / 2) as usize;
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
                let rgb = |k| {
                    let byte = |i| (at(i).clamp(0., 1.) * 255.).round() as u32;
                    byte(k) | byte(k + 1) << 8 | byte(k + 2) << 16
                };
                let index = (level - first) * RING + (z * CELLS + x) as usize;
                let roof = lattice.roofs[index];
                // The center supplies both physical planes. Neighbors only
                // estimate how much of an already wet roof is covered.
                let water = if roof.depth > 0. {
                    (7. * at(4)).round().clamp(1., 7.) as u32
                } else {
                    0
                };
                result[index] = Column {
                    water,
                    depth: (4. * roof.depth).round() as u32,
                    y: roof.height,
                    slope: encode(dx) | (encode(dz) << 5),
                    rgb: rgb(0),
                    soil_rgb: rgb(5),
                    rock_rgb: rgb(8),
                };
            }
        }
    }
    result
}

fn face(x: i32, z: i32, axis: u32, level: usize, low: f32, high: Column) -> [u32; 4] {
    let (x, z) = (x >> level, z >> level);
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
fn roofs(
    columns: &[Column],
    first: usize,
    level: usize,
    cx: i32,
    cz: i32,
    output: &mut Vec<[u32; 4]>,
) {
    let step = 1 << level;
    let half = extent(level);
    #[derive(Clone, Copy)]
    struct RoofRun {
        row: i32,
        width: i32,
        index: usize,
        column: Column,
    }
    let mut previous = [None::<RoofRun>; CHUNK as usize];
    let column = |x, z| columns[(level - first) * RING + ((z + HALF) * CELLS + x + HALF) as usize];
    for z in cz..(cz + CHUNK).min(half) {
        let mut x = cx;
        while x < (cx + CHUNK).min(half) {
            if hole(first, level, x, z) {
                x += 1;
                continue;
            }
            let high = column(x, z);
            let mut width = 1;
            if high.slope == 0 {
                while x + width < (cx + CHUNK).min(half)
                    && !hole(first, level, x + width, z)
                    && column(x + width, z).same_roof(high)
                {
                    width += 1;
                }
            }
            let slot = &mut previous[(x - cx) as usize];
            let index = if let Some(run) = slot.filter(|run| {
                high.slope == 0
                    && run.row == z - 1
                    && run.width == width
                    && run.column.same_roof(high)
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

pub(super) fn build(lattice: &Lattice, voxel: f32) -> Mesh {
    let first = lattice.first;
    let columns = columns(lattice);
    let mut mesh = Mesh {
        faces: Vec::with_capacity((CELLS * CELLS) as usize * 6),
        batches: Vec::new(),
    };
    let mut directions: [Vec<[u32; 4]>; 5] = Default::default();
    for level in first..LEVELS {
        let step = 1 << level;
        let half = extent(level);
        let inner = if level > first { extent(level - 1) } else { 0 };
        let base = (level - first) * RING;
        for cz in (-half..half).step_by(CHUNK as usize) {
            for cx in (-half..half).step_by(CHUNK as usize) {
                let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
                let mut wet = glam::Vec4::ZERO;
                roofs(&columns, first, level, cx, cz, &mut directions[0]);
                for z in cz..(cz + CHUNK).min(half) {
                    for x in cx..(cx + CHUNK).min(half) {
                        // Twice the cell-center radius is odd. Only the first
                        // row outside the hole and the last row of the ring
                        // can have a neighbor at a different resolution.
                        let radius = (2 * x + 1).abs().max((2 * z + 1).abs());
                        if radius < inner {
                            continue;
                        }
                        let edge = radius == 2 * half - 1 || (inner != 0 && radius == inner + 1);
                        let index = (z + HALF) * CELLS + x + HALF;
                        let high = columns[base + index as usize];
                        let (x, z) = (x * step, z * step);
                        if high.water > 0 {
                            wet += glam::Vec4::new(
                                (x as f32 + step as f32 * 0.5) * UNIT,
                                high.y,
                                (z as f32 + step as f32 * 0.5) * UNIT,
                                1.,
                            ) * high.water as f32;
                        }
                        min_y = min_y.min(high.y);
                        max_y = max_y.max(high.y);
                        for (axis, dx, dz) in [(1, 1, 0), (2, -1, 0), (3, 0, 1), (4, 0, -1)] {
                            // Place the lookup in the neighboring half-open cell.
                            let ax = x + if dx > 0 { step } else { 0 };
                            let az = z + if dz > 0 { step } else { 0 };
                            let nx = ax - i32::from(dx < 0);
                            let nz = az - i32::from(dz < 0);
                            let (other, i) = if edge {
                                let Some(neighbor) = address(first, nx, nz) else {
                                    continue;
                                };
                                neighbor
                            } else {
                                (level, (index + dz * CELLS + dx) as usize)
                            };
                            let wall_level = other.min(level);
                            let mut wall = |off, other: usize, index: usize| {
                                let low = columns[(other - first) * RING + index].y;
                                if high.y > low {
                                    directions[axis as usize].push(face(
                                        ax + if dx == 0 { off } else { 0 },
                                        az + if dz == 0 { off } else { 0 },
                                        axis,
                                        wall_level,
                                        low,
                                        Column {
                                            rgb: high.wall_rgb(low, voxel),
                                            water: 0,
                                            ..high
                                        },
                                    ));
                                    min_y = min_y.min(low);
                                }
                            };
                            wall(0, other, i);
                            if other < level {
                                let half = step / 2;
                                let (other, i) = address(
                                    first,
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
                        wet_center: (wet.w > 0.).then(|| wet.truncate() / wet.w),
                        faces,
                        min: [(cx * step) as f32 * UNIT, min_y, (cz * step) as f32 * UNIT],
                        max: [
                            ((cx + CHUNK).min(half) * step) as f32 * UNIT,
                            max_y,
                            ((cz + CHUNK).min(half) * step) as f32 * UNIT,
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
            (-96, -96, 0),
            (95, 95, 0),
            (-97, 0, 1),
            (96, 0, 1),
            (0, -321, 2),
            (0, 320, 2),
        ] {
            assert_eq!(address(0, x, z).unwrap().0, level);
        }
        for first in [0, 1] {
            assert_eq!(address(first, 0, 0).unwrap().0, first);
            assert_eq!(address(first, -1280, 1279).unwrap().0, 3);
            assert!(address(first, 1280, 0).is_none());
            assert!(address(first, -1281, 0).is_none());
        }
    }
    #[test]
    fn snow_caps_and_flooded_cliffs_expose_their_native_material_bands() {
        let snow = Column {
            y: 8.,
            rgb: 0xffffff,
            soil_rgb: 0x777777,
            rock_rgb: 0x333333,
            ..Default::default()
        };
        for voxel in [0.0625, 0.1] {
            assert_eq!(snow.wall_rgb(snow.y - voxel * 0.5, voxel), 0xffffff);
            let rock = snow.wall_rgb(-100., voxel) & 255;
            assert!((51..=52).contains(&rock));
        }
        let flooded = Column {
            water: 7,
            depth: 16,
            ..snow
        };
        assert_eq!(flooded.wall_rgb(6., 0.0625), 0x4a3009);
        assert_ne!(flooded.wall_rgb(-100., 0.0625), 0x4a3009);
        assert!(snow.same_roof(Column {
            rock_rgb: 0,
            ..snow
        }));
    }

    #[test]
    fn wet_horizon_roofs_keep_their_depth_and_merge_only_matching_beds() {
        let mut c = vec![
            Column {
                y: 0.,
                rgb: 123,
                slope: 0,
                water: 7,
                depth: 16,
                ..Default::default()
            };
            (CELLS * CELLS) as usize
        ];
        c[(HALF * CELLS + HALF) as usize].depth = 2;
        let mut faces = vec![];
        roofs(&c, 0, 0, 0, 0, &mut faces);
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
        for first in [0, 1] {
            let lattice = super::super::lattice(
                [0, 0],
                first,
                None,
                |_, _| [0.5, 0.25, 0.1, 7.5, 0., 0.3, 0.2, 0.1, 0.3, 0.3, 0.3],
                |_, _| NativeRoof { height: 7.5, depth: 0. },
            );
            let mesh = build(&lattice, 0.0625);
            assert!(mesh.faces.len() <= 100 * (LEVELS - first));
            assert!(mesh.batches.iter().all(|batch| batch.wet_center.is_none()));
            let mut covered = vec![false; RING * (LEVELS - first)];
            for f in &mesh.faces {
                let level = ((f[1] >> 27) & 3) as usize;
                let x = (f[0] as i32) << 21 >> 21;
                let z = ((f[0] >> 11) as i32) << 21 >> 21;
                let packed = f32::from_bits(f[2]) as i32;
                for zz in z..z + 1 + packed / CHUNK {
                    for xx in x..x + 1 + packed % CHUNK {
                        assert!(!hole(first, level, xx, zz));
                        assert!(!std::mem::replace(
                            &mut covered[(level - first) * RING
                                + ((zz + HALF) * CELLS + xx + HALF) as usize],
                            true
                        ));
                    }
                }
            }
            assert_eq!(
                covered.into_iter().filter(|v| *v).count(),
                (first..LEVELS)
                    .map(|level| {
                        let count = (2 * extent(level)).pow(2);
                        let cut = if level == first {
                            0
                        } else {
                            extent(level - 1).pow(2)
                        };
                        (count - cut) as usize
                    })
                    .sum::<usize>()
            );
            assert!(mesh
                .faces
                .iter()
                .all(|f| (f[1] >> 24) & 7 == 0 && f32::from_bits(f[3]) == 7.5));
        }
    }

    #[test]
    fn water_probes_sample_the_horizon_batch_elevation_and_extent() {
        let lattice = super::super::lattice(
            [0, 0],
            1,
            None,
            |_, _| [0.5, 0.3, 0.2, 80., 1., 0.2, 0.3, 0.1, 0.3, 0.3, 0.3],
            |_, _| NativeRoof { height: 80., depth: 4. },
        );
        let mesh = build(&lattice, 0.0625);
        assert!(!mesh.batches.is_empty());
        for b in &mesh.batches {
            let p = b.wet_center.unwrap();
            assert!((p.y - 80.).abs() < 1e-4);
            assert!(p.x >= b.min[0] && p.x <= b.max[0] && p.z >= b.min[2] && p.z <= b.max[2]);
        }
    }

    #[test]
    fn horizon_water_and_optical_depth_belong_to_the_native_roof() {
        let lattice = super::super::lattice(
            [0, 0],
            1,
            None,
            |x, _| {
                let mut sample = [0.; 11];
                sample[4] = f32::from(x < 0.);
                sample
            },
            |x, _| {
                if x < 0. {
                    NativeRoof { height: 17., depth: 0. }
                } else {
                    NativeRoof { height: 2., depth: 0.5 }
                }
            },
        );
        let roofs = columns(&lattice);
        let column = |x| roofs[(HALF * CELLS + x + HALF) as usize];
        // Coarse samples on the left say all water, on the right all land.
        assert_eq!((column(-8).water, column(-8).depth), (0, 0));
        assert_eq!((column(8).water, column(8).depth), (1, 2));
    }

    #[test]
    fn coarser_wall_meets_both_fine_neighbors_at_their_native_roofs() {
        // The negative X edge of each inner ring meets one coarse wall.
        // Give its two fine neighbors distinct heights to expose a missing
        // half-wall, wrong level in the face packet, or reversed edge.
        for first in [0, 1] {
            let spacing = f64::from(UNIT) * (1 << first) as f64;
            let edge = -f64::from(extent(first)) * spacing;
            let lattice = super::super::lattice(
                [0, 0],
                first,
                None,
                |_, _| [0.; 11],
                |x, z| {
                    let height = if x == edge - spacing && z == spacing {
                        70.
                    } else if x == edge + 0.5 * spacing && z == 0.5 * spacing {
                        7.
                    } else if x == edge + 0.5 * spacing && z == 1.5 * spacing {
                        11.
                    } else {
                        0.
                    };
                    NativeRoof { height, depth: 0. }
                },
            );
            let mut walls = Vec::new();
            for f in build(&lattice, 0.0625).faces {
                if (f[1] >> 24) & 7 == 1 && f32::from_bits(f[3]) == 70. {
                    assert_eq!(((f[1] >> 27) & 3) as usize, first);
                    let x = (f[0] as i32) << 21 >> 21;
                    let z = ((f[0] >> 11) as i32) << 21 >> 21;
                    assert_eq!(x, -extent(first));
                    walls.push((z, f32::from_bits(f[2]) as i32));
                }
            }
            walls.sort_unstable();
            assert_eq!(walls, [(0, 7), (1, 11)]);
        }
    }

    #[test]
    fn narrow_crest_between_lighting_samples_has_one_roof_and_four_risers() {
        let lattice = super::super::lattice(
            [-1, 1],
            0,
            None,
            |_, _| [0.; 11],
            |x, z| {
                let height = if x == -SPACING - 0.5 * f64::from(UNIT) && z == SPACING + 0.5 * f64::from(UNIT) {
                    73.0625
                } else {
                    0.
                };
                NativeRoof { height, depth: 0. }
            },
        );
        let mesh = build(&lattice, 0.0625);
        let mut axes = [0; 5];
        for face in mesh
            .faces
            .iter()
            .filter(|face| f32::from_bits(face[3]) == 73.0625)
        {
            axes[((face[1] >> 24) & 7) as usize] += 1;
            assert_eq!(f32::from_bits(face[2]), 0.);
        }
        assert_eq!(axes, [1; 5]);
    }
}
