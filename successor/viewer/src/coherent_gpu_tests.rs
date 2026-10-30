//! Real rasterization checks for the successor's connected terrain path.
use crate::{camera::Camera, gpu::Gpu};
use microvoxel::{
    generation::{Column, Generator, GeneratorIdentity, Run},
    materials::{MaterialId, Voxel},
    render::{
        lod::{MeshKey, TileKey},
        terrain::Sampler,
    },
    CellPos, World,
};
use std::{collections::BTreeSet, ops::Range, sync::Arc};
struct Ridge {
    base: [i64; 2],
}
impl Generator for Ridge {
    fn identity(&self) -> GeneratorIdentity {
        GeneratorIdentity {
            seed: 1,
            version: 1,
            definition: 1,
        }
    }
    fn column_hint(&self, x: i64, z: i64) -> Option<Column> {
        let h = 32 + ((x - self.base[0] - 64).abs() / 3) + ((z - self.base[1] - 96).abs() / 5);
        Some(Column {
            ground: h,
            water: 0,
            bedrock: h,
            surface: MaterialId::GRASS,
            subsoil: MaterialId::GRASS,
            rock: MaterialId::GRASS,
        })
    }
    fn voxel(&self, p: CellPos) -> Option<Voxel> {
        Some(Voxel::new(
            self.column_hint(p.x, p.z)?.material_at(p.y),
            self.appearance_at(p, MaterialId::GRASS),
        ))
    }
    fn runs(&self, x: i64, z: i64, r: Range<i64>) -> Option<Vec<Run>> {
        Some(self.column_hint(x, z)?.runs(r))
    }
}
#[test]
#[ignore = "requires graphics adapter"]
fn connected_ridge_and_native_boundary_have_no_sky_pixels() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let mut gpu = pollster::block_on(Gpu::new(&instance, None, 256, 256)).unwrap();
    gpu.detail_settings(&crate::settings::Settings {
        haze: 0.,
        shadows: false,
        antialias: false,
        ..Default::default()
    });
    gpu.hud_visible = false;
    let mut sampler = Sampler::new(Arc::new(World::new(Ridge { base: [0, 0] })));
    // Left: two exact native tiles. Right: one connected step-two patch.
    let keys = [
        MeshKey {
            tile: TileKey {
                level: 12,
                x: 1,
                z: 1,
            },
            neighbors: [13, 12, 12, 12],
            revision: 0,
        },
        MeshKey {
            tile: TileKey {
                level: 13,
                x: 1,
                z: 2,
            },
            neighbors: [13, 12, 13, 13],
            revision: 0,
        },
        MeshKey {
            tile: TileKey {
                level: 13,
                x: 1,
                z: 3,
            },
            neighbors: [13, 12, 13, 13],
            revision: 0,
        },
    ];
    let cover: BTreeSet<_> = keys.into_iter().collect();
    for key in keys {
        gpu.upload(key, sampler.display_mesh(key, || false).unwrap(), &cover)
            .unwrap();
    }
    let mut camera = Camera::at(CellPos::new(64, 70, 96));
    camera.pitch = -std::f64::consts::FRAC_PI_2;
    camera.yaw = 0.;
    let texture = gpu.offscreen();
    let view = texture.create_view(&Default::default());
    gpu.render(&view, &camera, &cover, None);
    let pixels = gpu.read_pixels(&texture).unwrap();
    for y in 48..208 {
        for x in 48..208 {
            let p = &pixels[(y * 256 + x) * 4..][..3];
            assert!(p[1] > p[2], "sky/crack pixel at {x},{y}: {p:?}");
        }
    }
}
#[test]
#[ignore = "requires graphics adapter"]
fn connected_two_to_one_boundary_at_large_world_coordinates_has_no_sky_pixels() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let mut gpu = pollster::block_on(Gpu::new(&instance, None, 512, 512)).unwrap();
    gpu.detail_settings(&crate::settings::Settings {
        haze: 0.,
        shadows: false,
        antialias: false,
        ..Default::default()
    });
    gpu.hud_visible = false;
    let base = [68096, 100096];
    let mut sampler = Sampler::new(Arc::new(World::new(Ridge { base })));
    let keys = [
        MeshKey {
            tile: TileKey {
                level: 11,
                x: (base[0] / 128 + 1) as u32,
                z: (base[1] / 128 + 1) as u32,
            },
            neighbors: [12, 11, 11, 11],
            revision: 0,
        },
        MeshKey {
            tile: TileKey {
                level: 12,
                x: (base[0] / 64 + 1) as u32,
                z: (base[1] / 64 + 2) as u32,
            },
            neighbors: [12, 11, 12, 12],
            revision: 0,
        },
        MeshKey {
            tile: TileKey {
                level: 12,
                x: (base[0] / 64 + 1) as u32,
                z: (base[1] / 64 + 3) as u32,
            },
            neighbors: [12, 11, 12, 12],
            revision: 0,
        },
    ];
    let cover: BTreeSet<_> = keys.into_iter().collect();
    for key in keys {
        gpu.upload(key, sampler.display_mesh(key, || false).unwrap(), &cover)
            .unwrap();
    }
    let texture = gpu.offscreen();
    let view = texture.create_view(&Default::default());
    for yaw in [0., 0.37, 1.1] {
        let mut camera = Camera::at(CellPos::new(base[0] + 128, 150, base[1] + 192));
        camera.pitch = -1.45;
        camera.yaw = yaw;
        gpu.render(&view, &camera, &cover, None);
        let pixels = gpu.read_pixels(&texture).unwrap();
        for y in 180..332 {
            for x in 180..332 {
                let p = &pixels[(y * 512 + x) * 4..][..3];
                assert!(p[1] > p[2], "sky/crack pixel at {x},{y} yaw {yaw}: {p:?}");
            }
        }
    }
}
