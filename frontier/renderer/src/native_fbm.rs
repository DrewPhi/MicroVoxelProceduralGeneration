//! Bounded exact-native comparator for the same FBM field as the surface proxy.
//! Startup-only extraction; neither streaming nor a native/surface handoff.
use crate::mesh;
use frontier_world::{voxel_size, Cell, World};

const MIN_XZ: i64 = -64;
const WIDTH: usize = 128;
const HALO_WIDTH: usize = WIDTH + 2;
const MAX_HEIGHT_CELLS: usize = 1024;
const MAX_HALO_CELLS: usize = 32_000_000;

type NativeReference = (mesh::Mesh, [f64; 3], [f64; 3]);

/// Extract a 128-cell-wide crop with 32 cells below the lowest source top and
/// 16 above the highest. Samples the complete neighbor halo, so crop
/// boundaries never create false solid caps. Source heights define the bounded
/// crop; edits are authoritative inside it, but edits outside it are not shown.
/// Returns an explicit error instead of reducing source relief to fit a budget.
pub fn build(world: &World) -> Result<NativeReference, String> {
    if world.fbm_terrain().is_none() {
        return Err("native FBM comparator requires World::fbm".into());
    }
    let mut columns = Vec::with_capacity(HALO_WIDTH * HALO_WIDTH);
    let (mut lowest, mut highest) = (i64::MAX, i64::MIN);
    for z in 0..HALO_WIDTH {
        for x in 0..HALO_WIDTH {
            let column = world.column(MIN_XZ + x as i64 - 1, MIN_XZ + z as i64 - 1);
            let height = column.height_cells();
            lowest = lowest.min(height);
            highest = highest.max(height);
            columns.push(column);
        }
    }
    let bottom = lowest - 32;
    let height = (highest - lowest + 48) as usize;
    if height > MAX_HEIGHT_CELLS {
        return Err(format!(
            "native FBM crop needs {:.2} m vertically. The {MAX_HEIGHT_CELLS}-cell limit is {:.2} m on this grid.",
            height as f64 * voxel_size(),
            MAX_HEIGHT_CELLS as f64 * voxel_size()
        ));
    }
    let halo_cells = HALO_WIDTH * HALO_WIDTH * (height + 2);
    if halo_cells > MAX_HALO_CELLS {
        return Err(format!(
            "native FBM crop needs {halo_cells} halo cells; limit is {MAX_HALO_CELLS}"
        ));
    }
    let min = Cell {
        x: MIN_XZ,
        y: bottom,
        z: MIN_XZ,
    };
    let mesh = mesh::extract(
        |cell| {
            let x = (cell.x - (MIN_XZ - 1)) as usize;
            let z = (cell.z - (MIN_XZ - 1)) as usize;
            columns[z * HALO_WIDTH + x].material(cell.y)
        },
        min,
        [WIDTH, height, WIDTH],
    );
    let center = world.height_m(0.0, 0.0);
    let position = [7.0, highest as f64 * voxel_size() + 5.0, -8.0];
    let target = [0.0, center, 0.0];
    let horizontal_limit = WIDTH as f64 * voxel_size() / 2.;
    eprintln!("FBM exact native crop: x/z=[-{horizontal_limit},{horizontal_limit})m, y=[{:.3},{:.3})m; {} halo cells; {} exposed faces, {} triangles; no distant coverage", bottom as f64 * voxel_size(), (bottom + height as i64) as f64 * voxel_size(), halo_cells, mesh.exposed_native_faces, mesh.indices.len()/3);
    Ok((mesh, position, target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::{FbmParams, Material};

    #[test]
    fn flat_native_crop_has_only_canonical_top_no_boundary_caps() {
        let world = World::fbm(
            7,
            FbmParams {
                amplitude: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        let (mesh, position, target) = build(&world).unwrap();
        assert_eq!(mesh.exposed_native_faces, WIDTH * WIDTH);
        assert_eq!(mesh.indices.len(), 6);
        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(target, [0.0; 3]);
        assert!(position[1] > target[1]);
        for vertex in &mesh.vertices {
            assert_eq!(vertex.normal, [0.0, 1.0, 0.0]);
            assert_eq!(vertex.color, Material::Grass.color());
            assert_eq!(
                vertex.position[1] as f64 + mesh.origin.y as f64 * voxel_size(),
                0.0
            );
        }
    }
    #[test]
    fn native_surface_faces_match_independent_column_difference_oracle() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let (mesh, _, _) = build(&world).unwrap();
        let mut expected = 0;
        for z in MIN_XZ..MIN_XZ + WIDTH as i64 {
            for x in MIN_XZ..MIN_XZ + WIDTH as i64 {
                let h = (world.height_m(x as f64 * voxel_size(), z as f64 * voxel_size())
                    / voxel_size()) as i64;
                expected += 1; // one horizontal top face per native column
                for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let neighbor = (world.height_m(
                        (x + dx) as f64 * voxel_size(),
                        (z + dz) as f64 * voxel_size(),
                    ) / voxel_size()) as i64;
                    expected += (h - neighbor).max(0) as usize;
                }
            }
        }
        assert_eq!(mesh.exposed_native_faces, expected);
        assert!(mesh.vertices.iter().all(|v| v.normal != [0.0, -1.0, 0.0]));
    }
    #[test]
    fn comparator_rejects_wrong_source_and_excessive_relief() {
        assert!(build(&World::new(7))
            .err()
            .unwrap()
            .contains("requires World::fbm"));
        let steep = World::fbm(
            7,
            FbmParams {
                horizontal_scale: 16.0,
                amplitude: 100_000.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(build(&steep).err().unwrap().contains(&format!(
            "{MAX_HEIGHT_CELLS}-cell limit is {:.2} m",
            MAX_HEIGHT_CELLS as f64 * voxel_size()
        )));
    }
}
