//! Optical properties of the native water cells under an exact terrain face.
//! Face geometry and collision continue to use the canonical column mesh.
use super::column_mesh::ColumnHeights;
use crate::spatial_appearance::Profile;
use frontier_world::{voxel_size, FbmTerrain};

pub(super) const TAG: f32 = -8.;
const SIDE: usize = 16;

/// The four high bits identify one-cell falls into a neighboring wet column.
/// A zero word means dry. Depth is in sixteenths of a metre, with one reserved for water.
fn cell_word(heights: &ColumnHeights, x: i64, z: i64) -> u16 {
    let Some(c) = heights.column(x, z) else {
        return 0;
    };
    if c.height <= c.ground_height {
        return 0;
    }
    let depth = ((c.height - c.ground_height) as f64 * voxel_size()).min(12.);
    let mut word = c.bands[0] as u16 | ((1 + (16. * depth).round() as u16) << 4);
    for (i, (dx, dz)) in [(-1, 0), (1, 0), (0, -1), (0, 1)].into_iter().enumerate() {
        if heights.column(x + dx, z + dz).is_some_and(|neighbor| {
            neighbor.height > neighbor.ground_height && neighbor.height == c.height - 1
        }) {
            word |= 1 << (12 + i);
        }
    }
    word
}

pub(super) fn append(
    table: &mut Vec<Profile>,
    heights: &ColumnHeights,
    terrain: &FbmTerrain,
    origin: [i64; 2],
    wet: [i64; 2],
) -> u32 {
    let id = table.len() as u32;
    let top = heights.height(wet[0], wet[1]).unwrap_or(0);
    let grade = crate::terrain_light::rapids(terrain, wet[0], wet[1], top);
    let mut header = [[0.; 4]; 5];
    header[0] = [f32::from(grade), 0., 0., TAG];
    let words: [[u16; SIDE]; SIDE] = std::array::from_fn(|z| {
        std::array::from_fn(|x| cell_word(heights, origin[0] + x as i64, origin[1] + z as i64))
    });
    let first = words[0][0];
    if first != 0 && words.iter().flatten().all(|&w| w == first) {
        header[0][1] = f32::from(first);
        table.push(header);
        return id;
    }
    table.push(header);
    for row in words {
        let mut record = [[0.; 4]; 5];
        record[0][3] = -4.;
        for (i, word) in row.into_iter().enumerate() {
            record[1 + i / 4][i % 4] = f32::from(word);
        }
        table.push(record);
    }
    id
}

pub(crate) fn valid_word(value: f32) -> bool {
    if value == 0. {
        return true;
    }
    if value.fract() != 0. || !(0. ..=65535.).contains(&value) {
        return false;
    }
    let word = value as u16;
    word & 15 < 10 && (1..=193).contains(&((word >> 4) & 255))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::column_mesh;
    use frontier_world::{Cell, FbmParams, FbmRecipe, World};

    #[test]
    fn exact_water_profiles_reproduce_native_beds_and_wet_neighbors() {
        for (seed, x, z) in [(673, -2233.55, -1841.28), (40961, 360., -3000.)] {
            let world = World::fbm(
                seed,
                FbmParams {
                    recipe: FbmRecipe::Highlands,
                    ..FbmParams::default()
                },
            )
            .unwrap();
            let c = Cell::from_meters([x, 0., z]);
            let origin = [c.x, c.z].map(|n| n.div_euclid(16) * 16);
            let mesh = column_mesh::build(
                &world,
                Cell {
                    x: origin[0],
                    y: 0,
                    z: origin[1],
                },
                [16, 16],
            )
            .unwrap();
            let terrain = world.fbm_terrain().unwrap();
            let mut profiles = vec![[[0.; 4]; 5]];
            let id = append(&mut profiles, &mesh.heights, terrain, origin, [c.x, c.z]) as usize;
            assert!(crate::spatial_appearance::valid_table(&profiles));
            let header = profiles[id][0];
            assert_eq!(header[3], TAG);
            let mut wet = 0;
            let mut drops = 0;
            for z in 0..16 {
                for x in 0..16 {
                    let word = if header[1] > 0. {
                        header[1]
                    } else {
                        profiles[id + 1 + z][1 + x / 4][x % 4]
                    } as u16;
                    let (wx, wz) = (origin[0] + x as i64, origin[1] + z as i64);
                    let col = terrain.column_cells(wx, wz);
                    if col.visible_top == col.ground_top {
                        assert_eq!(word, 0);
                        continue;
                    }
                    wet += 1;
                    assert_eq!(word & 15, col.ground_surface as u16);
                    let depth = ((col.visible_top - col.ground_top) as f64 * voxel_size()).min(12.);
                    assert_eq!((word >> 4) & 255, 1 + (16. * depth).round() as u16);
                    for (i, (dx, dz)) in [(-1, 0), (1, 0), (0, -1), (0, 1)].into_iter().enumerate()
                    {
                        let [bed, top] = terrain.column_tops(wx + dx, wz + dz);
                        let step = top > bed && top == col.visible_top - 1;
                        assert_eq!(word & (1 << (12 + i)) != 0, step);
                        drops += usize::from(step);
                    }
                }
            }
            assert!(wet > 0);
            if seed == 673 {
                assert!(drops > 0);
            }
            if profiles.len() > 2 {
                profiles[id + 1][1][0] = 65536.;
                assert!(!crate::spatial_appearance::valid_table(&profiles));
            }
        }
    }
}
