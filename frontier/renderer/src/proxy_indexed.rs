//! Lossless GPU source-position reuse, derived from immutable proxy triangles.
//! Source sampling, certification and the persistent cache format are unchanged.
use crate::adaptive_geometry::ProxyTriangle;
use std::collections::HashMap;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Triangle {
    pub positions: [u32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    pub profile_index: u32,
}
/// Positions and canonical colors remain exact; GPU normal arithmetic may round
/// differently from the CPU. Eligibility requires the original CPU flat normal.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct DerivedTriangle {
    pub positions: [u32; 3],
    pub material: u32,
    pub profile_index: u32,
}
pub(super) struct Packed {
    pub triangles: Vec<Triangle>,
    pub positions: Vec<[f32; 3]>,
    pub derived: Option<Vec<DerivedTriangle>>,
}
impl Packed {
    pub fn derive_attributes(&mut self, staging_estimate: &mut usize) -> bool {
        use frontier_world::Material;
        let palette = [
            Material::Grass,
            Material::Soil,
            Material::Stone,
            Material::Wood,
        ];
        // Validate the complete tile before allocating or discarding exact data.
        // Custom normals and non-palette colors retain mode1 losslessly.
        let material = |color: [f32; 3]| {
            palette
                .iter()
                .position(|m| m.color().map(f32::to_bits) == color.map(f32::to_bits))
                .map(|i| i as u32 + 1)
        };
        for triangle in &self.triangles {
            if material(triangle.color).is_none() {
                return false;
            }
            let [a, b, c] = triangle
                .positions
                .map(|i| glam::Vec3::from_array(self.positions[i as usize]));
            let cross = (b - a).cross(c - a);
            if !cross.is_finite()
                || cross.length_squared() == 0.
                || cross.normalize().to_array().map(f32::to_bits)
                    != triangle.normal.map(f32::to_bits)
            {
                return false;
            }
        }
        let compact: Vec<_> = self
            .triangles
            .iter()
            .map(|triangle| DerivedTriangle {
                positions: triangle.positions,
                material: material(triangle.color).unwrap(),
                profile_index: triangle.profile_index,
            })
            .collect();
        *staging_estimate = (*staging_estimate).max(
            self.triangles.capacity() * 40
                + compact.capacity() * 20
                + self.positions.capacity() * 12,
        );
        self.triangles = vec![];
        self.derived = Some(compact);
        true
    }
    pub fn relocate(&mut self, scalar_base: u32, profile_base: u32) {
        if let Some(derived) = &mut self.derived {
            for triangle in derived {
                triangle.profile_index += profile_base;
                for address in &mut triangle.positions {
                    *address = scalar_base + *address * 3;
                }
            }
        }
        for triangle in &mut self.triangles {
            triangle.profile_index += profile_base;
            for address in &mut triangle.positions {
                *address = scalar_base + *address * 3;
            }
        }
    }
}
pub(super) fn pack(input: &[ProxyTriangle], staging_estimate: &mut usize) -> Option<Packed> {
    let mut positions = Vec::new();
    let mut addresses = HashMap::<[u32; 3], u32>::new();
    let mut triangles = Vec::with_capacity(input.len());
    for triangle in input {
        let mut indices = [0; 3];
        for (i, p) in triangle.positions.iter().enumerate() {
            let bits = p.map(f32::to_bits);
            let next = u32::try_from(positions.len()).ok()?;
            indices[i] = *addresses.entry(bits).or_insert_with(|| {
                positions.push(*p);
                next
            });
        }
        triangles.push(Triangle {
            positions: indices,
            normal: triangle.normal,
            color: triangle.color,
            profile_index: triangle.profile_index,
        });
    }
    // Estimated peak temporary payload includes HashMap keys/values/control
    // bytes, but excludes allocator metadata, original immutable tile and wgpu
    // upload staging. Record declined attempts too.
    *staging_estimate =
        triangles.capacity() * 40 + positions.capacity() * 12 + addresses.capacity() * (12 + 4 + 1);
    // Include worst-case 319B record and 79B storage alignment padding. Tiny or poorly
    // shared tiles retain the current representation instead of spending more.
    if triangles
        .len()
        .checked_mul(40)?
        .checked_add(positions.len().checked_mul(12)?)?
        .checked_add(398)?
        >= input.len().checked_mul(64)?
    {
        return None;
    }
    Some(Packed {
        triangles,
        positions,
        derived: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indexed_source_decodes_every_ordered_float_bit_and_profile() {
        let source: Vec<_> = (0..32)
            .map(|i| ProxyTriangle {
                positions: [[-0., 2., 0.], [0., 2., 0.], [1., 3., 1.]],
                normal: [0.3, 0.7, -0.2],
                color: [0.18, 0.34, 0.1],
                profile_index: i,
            })
            .collect();
        let packed = pack(&source, &mut 0).unwrap();
        assert_eq!(packed.positions.len(), 3); // signed zero remains distinct
        assert_eq!(std::mem::size_of::<Triangle>(), 40);
        for (original, indexed) in source.iter().zip(&packed.triangles) {
            for i in 0..3 {
                assert_eq!(
                    original.positions[i].map(f32::to_bits),
                    packed.positions[indexed.positions[i] as usize].map(f32::to_bits)
                );
            }
            assert_eq!(
                original.normal.map(f32::to_bits),
                indexed.normal.map(f32::to_bits)
            );
            assert_eq!(
                original.color.map(f32::to_bits),
                indexed.color.map(f32::to_bits)
            );
            assert_eq!(original.profile_index, indexed.profile_index);
        }
    }
    #[test]
    fn unshared_and_tiny_payloads_keep_existing_layout() {
        assert!(pack(&[], &mut 0).is_none());
        let source: Vec<_> = (0..16)
            .map(|i| ProxyTriangle {
                positions: [[i as f32, 0., 0.], [i as f32, 1., 0.], [i as f32, 2., 0.]],
                normal: [1., 0., 0.],
                color: [0.; 3],
                profile_index: 0,
            })
            .collect();
        assert!(pack(&source, &mut 0).is_none());
    }
    #[test]
    fn canonical_fans_staircases_and_walls_decode_after_gpu_page_relocation() {
        use crate::adaptive_geometry::{build, pack_proxy_triangles, TileKey};
        use frontier_world::{FbmParams, World};
        let world = World::fbm(7, FbmParams::default()).unwrap();
        for edges in [[8; 4], [1; 4], [1, 4, 8, 2]] {
            let key = TileKey {
                min: [-47, -111],
                width_cells: 32,
                step_cells: 8,
                edge_steps: edges,
                source_revision: 0,
            };
            let mut tile = build(&world, key, || false).unwrap();
            pack_proxy_triangles(&mut tile).unwrap();
            assert!(!tile.proxy_triangles.is_empty());
            let mut packed = pack(&tile.proxy_triangles, &mut 0).unwrap();
            let mut derived = pack(&tile.proxy_triangles, &mut 0).unwrap();
            assert!(derived.derive_attributes(&mut 0));
            assert_eq!(
                derived.derived.as_ref().unwrap().len(),
                tile.proxy_triangles.len()
            );
            let scalar_base = 1000;
            let profile_base = 17;
            packed.relocate(scalar_base, profile_base);
            let mut scalars = vec![0.; scalar_base as usize];
            for p in &packed.positions {
                scalars.extend_from_slice(p);
            }
            for (original, indexed) in tile.proxy_triangles.iter().zip(&packed.triangles) {
                for i in 0..3 {
                    let address = indexed.positions[i] as usize;
                    assert_eq!(
                        original.positions[i].map(f32::to_bits),
                        [scalars[address], scalars[address + 1], scalars[address + 2]]
                            .map(f32::to_bits)
                    );
                }
                assert_eq!(
                    original.normal.map(f32::to_bits),
                    indexed.normal.map(f32::to_bits)
                );
                assert_eq!(
                    original.color.map(f32::to_bits),
                    indexed.color.map(f32::to_bits)
                );
                assert_eq!(original.profile_index + profile_base, indexed.profile_index);
            }
        }
    }
    #[test]
    fn derived_attributes_keep_palette_exact_and_reject_custom_normals_or_colors() {
        use frontier_world::Material;
        assert_eq!(std::mem::size_of::<DerivedTriangle>(), 20);
        let points = [[0., 0., 0.], [0., 0., 1.], [1., 0., 1.]];
        let make = || {
            (0..32)
                .map(|i| ProxyTriangle {
                    positions: points,
                    normal: [0., 1., 0.],
                    color: [
                        Material::Grass,
                        Material::Soil,
                        Material::Stone,
                        Material::Wood,
                    ][i % 4]
                        .color(),
                    profile_index: i as u32,
                })
                .collect::<Vec<_>>()
        };
        let input = make();
        let mut packed = pack(&input, &mut 0).unwrap();
        assert!(packed.derive_attributes(&mut 0));
        let palette = [
            Material::Grass,
            Material::Soil,
            Material::Stone,
            Material::Wood,
        ];
        for (original, triangle) in input.iter().zip(packed.derived.as_ref().unwrap()) {
            assert_eq!(
                palette[triangle.material as usize - 1]
                    .color()
                    .map(f32::to_bits),
                original.color.map(f32::to_bits)
            );
            assert_eq!(triangle.profile_index, original.profile_index);
        }
        packed.relocate(100, 17);
        assert_eq!(
            packed.derived.as_ref().unwrap()[0].positions,
            [100, 103, 106]
        );
        assert_eq!(packed.derived.as_ref().unwrap()[0].profile_index, 17);
        for color in [false, true] {
            let mut source = make();
            if color {
                source[7].color[0] += 0.01;
            } else {
                source[7].normal = [0., 0.99, 0.01];
            }
            let mut packed = pack(&source, &mut 0).unwrap();
            assert!(!packed.derive_attributes(&mut 0));
            assert!(packed.derived.is_none());
            assert_eq!(packed.triangles.len(), source.len());
        }
    }
}
