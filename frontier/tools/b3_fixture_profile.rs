//! Standalone bounded diagnostic; compile against frozen release dependencies.
//! rustc --edition=2021 -O tools/b3_fixture_profile.rs -L dependency=target/release/deps
//!   --extern frontier_world=target/release/deps/libfrontier_world-<hash>.rlib
//!   --extern bytemuck=target/release/deps/libbytemuck-<hash>.rlib -o /tmp/b3-fixture-profile
#[path = "../renderer/src/mesh.rs"]
mod mesh;
use frontier_world::{Cell, Material, World};
use std::{hint::black_box, time::Instant};
fn main() {
    let mut world = World::new(7);
    for z in 0..8 {
        for y in 45..53 {
            for x in -52..-44 {
                world.set(Cell { x, y, z }, Material::Air);
            }
        }
    }
    println!("{{\"schema\":1,\"scope\":\"CPU-only 64-cubed bricks, frozen fixture source, no upload/publication; ms are independent wall intervals\",\"seed\":7,\"native_voxel_m\":0.0625,\"rows\":[");
    let mut first = true;
    for (name, min) in [
        (
            "edited-cave",
            Cell {
                x: -80,
                y: 16,
                z: 0,
            },
        ),
        (
            "bridge",
            Cell {
                x: 32,
                y: 40,
                z: -48,
            },
        ),
    ] {
        let side = 64usize;
        let halo = side + 2;
        for rep in 0..3 {
            let started = Instant::now();
            let mut cells = Vec::with_capacity(halo * halo * halo);
            for z in 0..halo {
                for y in 0..halo {
                    for x in 0..halo {
                        cells.push(world.material(Cell {
                            x: min.x + x as i64 - 1,
                            y: min.y + y as i64 - 1,
                            z: min.z + z as i64 - 1,
                        }));
                    }
                }
            }
            let source_ms = started.elapsed().as_secs_f64() * 1000.;
            let started = Instant::now();
            let cached = mesh::extract(
                |p| {
                    cells[(((p.z - min.z + 1) as usize * halo) + (p.y - min.y + 1) as usize) * halo
                        + (p.x - min.x + 1) as usize]
                },
                min,
                [side; 3],
            );
            let cached_ms = started.elapsed().as_secs_f64() * 1000.;
            let started = Instant::now();
            let direct = mesh::extract(|p| world.material(p), min, [side; 3]);
            let direct_ms = started.elapsed().as_secs_f64() * 1000.;
            assert_eq!(cached.indices, direct.indices);
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&cached.vertices),
                bytemuck::cast_slice::<_, u8>(&direct.vertices)
            );
            assert_eq!(cached.exposed_native_faces, direct.exposed_native_faces);
            if !first {
                println!(",");
            }
            first = false;
            print!("{{\"fixture\":\"{name}\",\"repeat\":{rep},\"source_materialize_ms\":{source_ms},\"cached_extract_ms\":{cached_ms},\"direct_extract_ms\":{direct_ms},\"faces\":{},\"vertices\":{},\"triangles\":{},\"halo_material_bytes\":{}}}",cached.exposed_native_faces,cached.vertices.len(),cached.indices.len()/3,cells.len()*std::mem::size_of::<Material>());
            black_box((cached, direct, cells));
        }
    }
    println!("]}}");
}
