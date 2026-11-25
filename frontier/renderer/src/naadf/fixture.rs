//! Small canonical, dense exports for independent traversal/layout experiments.
//! This is a benchmark input, never an additional runtime world representation.
use super::data;
use frontier_world::{
    terrain::{FbmParams, FbmRecipe},
    vegetation::Species,
    World,
};
use std::{io::Write, path::Path, sync::atomic::AtomicU32};
pub(crate) fn export(directory: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(directory)?;
    let world = World::fbm(
        7,
        FbmParams {
            recipe: FbmRecipe::Alpine,
            ..Default::default()
        },
    )?
    .with_vegetation();
    let tree = world
        .vegetation_records([-1024, -1024], [1024, 1024])
        .into_iter()
        .find(|r| r.species == Species::Rowan)
        .ok_or("no canonical rowan in fixture search")?;
    let tree_pos = [tree.root.x as f64 / 16., tree.root.z as f64 / 16.];
    for (name, xz, height) in [
        ("meadow", [-41., -250.], 1.7),
        ("rowan", tree_pos, 3.7),
        ("mountain", [0., 3000.], 1.7),
    ] {
        let y = world.height_m(xz[0], xz[1]) + height;
        let snapshot = data::build_with_root_mode(
            &world,
            [xz[0], y, xz[1]],
            4,
            128 * 1048576,
            &AtomicU32::new(0),
            false,
            || false,
        )?;
        let side = 128usize;
        let mut file = std::io::BufWriter::new(std::fs::File::create(
            directory.join(format!("{name}.vox")),
        )?);
        for word in [0x46564f58u32, 1, side as u32, 0] {
            file.write_all(&word.to_le_bytes())?;
        }
        // X fastest, then Y, then Z. Zero air; nonzero pigment+1. All colors
        // and leaf proof encodings are irrelevant to this visibility-only test.
        for z in 0..side {
            for y in 0..side {
                for x in 0..side {
                    let value = snapshot.material([x, y, z]);
                    file.write_all(
                        &(if value == 0 {
                            0
                        } else {
                            (value & 0x3fffffff) + 1
                        })
                        .to_le_bytes(),
                    )?;
                }
            }
        }
        let metadata=format!("{{\"seed\":7,\"recipe\":\"alpine\",\"native_voxel_m\":0.0625,\"side_cells\":128,\"bounds_m\":{:?},\"source_revision\":{},\"vegetation\":true,\"clipped_fixture\":true}}\n",snapshot.bounds(),snapshot.source_revision);
        std::fs::write(directory.join(format!("{name}.json")), metadata)?;
        let prepared = super::leaf::Prepared::new(snapshot, false);
        let s = &prepared.snapshot;
        let mut hierarchy = std::io::BufWriter::new(std::fs::File::create(
            directory.join(format!("{name}.aadf")),
        )?);
        for word in [
            0x41414446,
            1,
            s.chunks_per_edge,
            s.chunks.len() as u32,
            s.blocks.len() as u32,
            s.voxels.len() as u32,
            prepared.encoding.bits(),
        ] {
            hierarchy.write_all(&word.to_le_bytes())?;
        }
        hierarchy.write_all(bytemuck::cast_slice(&s.chunks))?;
        hierarchy.write_all(bytemuck::cast_slice(&s.blocks))?;
        hierarchy.write_all(bytemuck::cast_slice(&s.voxels))?;
        eprintln!("exported {name}: {:?}", s.bounds());
    }
    Ok(())
}
