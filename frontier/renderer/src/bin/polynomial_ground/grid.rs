use frontier_world::{FbmTerrain, Material};
use glam::Vec3;

const PALETTE: [[f32; 3]; 5] = [
    [0.25, 0.25, 0.24],
    [0.15, 0.29, 0.079],
    [0.29, 0.19, 0.098],
    [0.47, 0.37, 0.21],
    [0.82, 0.88, 0.93],
];

pub struct Lattice {
    pub origin: [i32; 2],
    pub side: usize,
    pub cell_width: i32,
    pub vertices: Vec<[i32; 2]>,
    pub roofs: Vec<i32>,
    pub generated: usize,
}

impl Lattice {
    pub fn generate(
        terrain: &FbmTerrain,
        origin: [i32; 2],
        side: usize,
        cell_width: i32,
        relief: bool,
        previous: Option<&Self>,
    ) -> Self {
        let width = side + 1;
        let mut vertices = vec![[0; 2]; width * width];
        let rows = width.div_ceil(8);
        let shift = previous.map(|old| {
            [
                (origin[0] - old.origin[0]) / cell_width,
                (origin[1] - old.origin[1]) / cell_width,
            ]
        });
        std::thread::scope(|scope| {
            for (chunk, block) in vertices.chunks_mut(rows * width).enumerate() {
                scope.spawn(move || {
                    for (index, output) in block.iter_mut().enumerate() {
                        let column = index % width;
                        let row = chunk * rows + index / width;
                        if let Some((old, [dx, dz])) = previous.zip(shift) {
                            let x = column as i32 + dx;
                            let z = row as i32 + dz;
                            if (0..=old.side as i32).contains(&x)
                                && (0..=old.side as i32).contains(&z)
                            {
                                *output = old.vertices[z as usize * (old.side + 1) + x as usize];
                                continue;
                            }
                        }
                        let x = i64::from(origin[0]) + column as i64 * i64::from(cell_width);
                        let z = i64::from(origin[1]) + row as i64 * i64::from(cell_width);
                        let column = terrain.column_cells(x, z);
                        let height = i32::try_from(column.ground_top).unwrap();
                        let material = match column.ground_surface {
                            Material::Grass => 1,
                            Material::Soil => 2,
                            Material::Sand => 3,
                            Material::Snow => 4,
                            _ => 0,
                        };
                        *output = [height, material];
                    }
                });
            }
        });
        let sun = Vec3::new(-0.46, 0.78, -0.44).normalize();
        for z in 0..width {
            for x in 0..width {
                let [left, right] = [x.saturating_sub(1), (x + 1).min(side)];
                let [top, bottom] = [z.saturating_sub(1), (z + 1).min(side)];
                let x_slope = (vertices[z * width + right][0] - vertices[z * width + left][0])
                    * (128 / cell_width)
                    / (right - left) as i32;
                let z_slope = (vertices[bottom * width + x][0] - vertices[top * width + x][0])
                    * (128 / cell_width)
                    / (bottom - top) as i32;
                let sample = &mut vertices[z * width + x][1];
                let material = *sample & 15;
                let normal = Vec3::new(
                    x_slope as f32 * (-1. / 128.),
                    1.,
                    z_slope as f32 * (-1. / 128.),
                )
                .normalize();
                let light = 0.38 + 0.83 * normal.dot(sun).max(0.);
                let color = PALETTE[material as usize]
                    .map(|c| (255. * c * light).clamp(0., 255.).round() as i32);
                *sample = material | (color[0] << 4) | (color[1] << 12) | (color[2] << 20);
            }
        }
        let mut roofs = vec![0; (4 * side * side - 1) / 3];
        let offset = (side * side - 1) / 3;
        let margin = i32::from(relief) * 4;
        for z in 0..side {
            for x in 0..side {
                let i = z * width + x;
                roofs[offset + z * side + x] = [
                    vertices[i][0],
                    vertices[i + 1][0],
                    vertices[i + width][0],
                    vertices[i + width + 1][0],
                ]
                .into_iter()
                .max()
                .unwrap()
                    + margin;
            }
        }
        let mut n = side;
        while n > 1 {
            let p = n / 2;
            let a = (n * n - 1) / 3;
            let b = (p * p - 1) / 3;
            for z in 0..p {
                for x in 0..p {
                    let i = a + 2 * (z * n + x);
                    roofs[b + z * p + x] = roofs[i]
                        .max(roofs[i + 1])
                        .max(roofs[i + n])
                        .max(roofs[i + n + 1]);
                }
            }
            n = p;
        }
        let reused = shift.map_or(0, |[dx, dz]| {
            width.saturating_sub(dx.unsigned_abs() as usize)
                * width.saturating_sub(dz.unsigned_abs() as usize)
        });
        Self {
            origin,
            side,
            cell_width,
            vertices,
            roofs,
            generated: width * width - reused,
        }
    }

    pub fn height(&self, cell: [i32; 2], seed: u32, relief: bool) -> i32 {
        let [x, z] = [cell[0] - self.origin[0], cell[1] - self.origin[1]];
        let cell_width = self.cell_width;
        let width = self.side + 1;
        let patch = (z / cell_width) as usize * width + (x / cell_width) as usize;
        let u = i64::from(2 * (x % cell_width) + 1);
        let v = i64::from(2 * (z % cell_width) + 1);
        let d = i64::from(2 * cell_width);
        let [a, b, c, e] = [patch, patch + 1, patch + width, patch + width + 1]
            .map(|i| i64::from(self.vertices[i][0]));
        // The reference uses a single signed i64 numerator.
        let value = (a * (d - u) * (d - v) + b * u * (d - v) + c * (d - u) * v + e * u * v)
            .div_euclid(d * d) as i32;
        if relief {
            value + ((3 * band(cell, 4, seed) + band(cell, 2, seed ^ 0x2d1189a1)) >> 7)
        } else {
            value
        }
    }

    pub fn contains(&self, position: [f64; 3], radius: f64) -> bool {
        [position[0], position[2]]
            .into_iter()
            .zip(self.origin)
            .all(|(point, cell)| {
                let start = f64::from(cell) / 16.;
                point - radius >= start
                    && point + radius < start + self.side as f64 * f64::from(self.cell_width) / 16.
            })
    }
}

fn hash(x: i32, z: i32, seed: u32) -> i32 {
    let mut v = (x as u32).wrapping_mul(0x9e3779b9) ^ (z as u32).wrapping_mul(0x85ebca6b) ^ seed;
    v = (v ^ (v >> 16)).wrapping_mul(0x7feb352d);
    v = (v ^ (v >> 15)).wrapping_mul(0x846ca68b);
    ((v ^ (v >> 16)) & 255) as i32 - 128
}

fn band([x, z]: [i32; 2], power: u32, seed: u32) -> i32 {
    let width = 1 << power;
    let [u, v] = [x & (width - 1), z & (width - 1)];
    let [x, z] = [x >> power, z >> power];
    let a = hash(x, z, seed) * (width - u) + hash(x + 1, z, seed) * u;
    let b = hash(x, z + 1, seed) * (width - u) + hash(x + 1, z + 1, seed) * u;
    (a * (width - v) + b * v) >> (2 * power)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::{FbmParams, FbmRecipe};

    #[test]
    fn overlapping_window_matches_fresh_generation() {
        let terrain = FbmTerrain::new(
            7,
            FbmParams {
                recipe: FbmRecipe::Eroded,
                ..FbmParams::default()
            },
        )
        .unwrap();
        for width in [64, 128] {
            let initial =
                Lattice::generate(&terrain, [-64 * width, 128 * width], 8, width, true, None);
            let origin = [initial.origin[0] + width * 3, initial.origin[1] - width * 2];
            let shifted = Lattice::generate(&terrain, origin, 8, width, true, Some(&initial));
            let fresh = Lattice::generate(&terrain, origin, 8, width, true, None);
            assert_eq!(shifted.generated, 81 - 6 * 7);
            assert_eq!(shifted.vertices, fresh.vertices);
            assert_eq!(shifted.roofs, fresh.roofs);
        }
    }
}
