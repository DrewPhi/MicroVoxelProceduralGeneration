//! Lossless fetch layout for axis-aligned voxel faces. A tile shares the exact
//! f32 values that cannot be reconstructed from a 16-bit native grid address.
use crate::adaptive_geometry::NativeQuad;
use rustc_hash::FxHashMap;

const NEGATIVE_Y: u32 = 1 << 27;
const LOOKUP: u32 = 1 << 28;
const PROFILE_LIMIT: u32 = 1 << 17;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct PackedFace {
    coordinates: [u16; 4],
    axis_material: u32,
    width_profile: u32,
}

pub(super) struct PackedFaces {
    pub faces: Vec<PackedFace>,
    pub coordinates: Vec<f32>,
    max_profile: u32,
}

impl PackedFaces {
    pub(super) fn pack(quads: &[NativeQuad], profiles: u32, grid: f32) -> Option<Self> {
        // Shader arithmetic may flush a subnormal grid constant to zero.
        if !grid.is_normal() {
            return None;
        }
        let mut faces = Vec::with_capacity(quads.len());
        let mut coordinates = Vec::new();
        let mut offsets = FxHashMap::default();
        let mut max_profile = 0;
        let cells_per_meter = grid.recip();
        let grid_address = |meters: f32| {
            // f32 values above 2^23 are one unit apart. The low bits give a
            // candidate cell index. The bitwise check also rejects overflow.
            let index = (meters * cells_per_meter + 8_388_608.).to_bits() as u16;
            (meters.to_bits() == (f32::from(index) * grid).to_bits()).then_some(index)
        };
        for q in quads {
            if q.profile_index >= profiles.min(PROFILE_LIMIT)
                || q.material >= NEGATIVE_Y
                || q.axis_sign > 6
                || q.axis_sign & 3 == 3
            {
                return None;
            }
            max_profile = max_profile.max(q.profile_index);
            let values = [
                q.anchor[0],
                q.anchor[2],
                q.anchor[1].abs(),
                q.extent[0],
                q.extent[1],
            ];
            let mut material = q.material;
            if q.anchor[1].is_sign_negative() {
                material |= NEGATIVE_Y;
            }
            let [x, z, y, u, v] = match values.map(grid_address) {
                [Some(x), Some(z), Some(y), Some(u), Some(v)] if v < 32768 => [x, z, y, u, v],
                _ => {
                    // Non-binary grids can round (right-left) differently from
                    // (cell_count*grid). Adjacent roofs and walls share the
                    // original f32 values in their tile coordinate table.
                    material |= LOOKUP;
                    let distance_to_table = (profiles - q.profile_index).checked_mul(20)?;
                    let mut addresses = [0; 5];
                    for (i, value) in values.into_iter().enumerate() {
                        let offset = *offsets.entry(value.to_bits()).or_insert_with(|| {
                            coordinates.push(value);
                            (coordinates.len() - 1) as u32
                        });
                        addresses[i] =
                            u16::try_from(distance_to_table.checked_add(offset)?).ok()?;
                    }
                    if addresses[4] >= 32768 {
                        return None;
                    }
                    addresses
                }
            };
            faces.push(PackedFace {
                coordinates: [x, z, y, u],
                axis_material: (q.axis_sign << 29) | material,
                width_profile: (u32::from(v) << 17) | q.profile_index,
            });
        }
        // Sparse tiles must still save space after padding the coordinate table.
        if (coordinates.len() * 4).next_multiple_of(80) >= quads.len() * 16 {
            return None;
        }
        Some(Self {
            faces,
            coordinates,
            max_profile,
        })
    }

    pub(super) fn relocate(&mut self, profile_base: u32) -> bool {
        if profile_base >= PROFILE_LIMIT - self.max_profile {
            return false;
        }
        for face in &mut self.faces {
            face.width_profile += profile_base;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnormal_shader_grid_uses_original_float_records() {
        let q = NativeQuad {
            anchor: [0.; 3],
            extent: [1.; 2],
            axis_sign: 1,
            material: 1,
            profile_index: 0,
        };
        assert!(PackedFaces::pack(&[q], 1, f32::MIN_POSITIVE * 0.5).is_none());
    }

    #[test]
    fn roundtrips_faces_at_shell_seams_and_on_decimal_grids() {
        assert_eq!(std::mem::size_of::<PackedFace>(), 16);
        for grid in [0.0625, 0.1, 0.037] {
            let mut source = Vec::new();
            for i in 0..240 {
                let axis_sign = [0, 1, 2, 4, 5, 6][i % 6];
                let w = usize::from(axis_sign & 3 != 2);
                let mut q = NativeQuad {
                    anchor: [
                        if i % 5 == 0 { -0.0 } else { 23999. * grid },
                        -1999.9375,
                        3. * grid,
                    ],
                    extent: [if i % 4 == 0 { 3276.123 } else { grid * 12. }; 2],
                    axis_sign,
                    material: 0x03f74e17,
                    profile_index: i as u32 % 3,
                };
                q.extent[w] = if i % 3 == 0 {
                    56.0_f32.mul_add(grid, 0.) - 51. * grid
                } else {
                    grid
                };
                source.push(q);
            }
            let mut packed = PackedFaces::pack(&source, 3, grid).unwrap();
            assert!(!packed.coordinates.is_empty());
            assert!(!packed.relocate(PROFILE_LIMIT - 2));
            assert!(packed.relocate(90000));
            for (face, q) in packed.faces.iter().zip(source) {
                let width = (face.width_profile >> 17) as u16;
                let profile = face.width_profile & (PROFILE_LIMIT - 1);
                let [x, z, abs_y, u, v] = [
                    face.coordinates[0],
                    face.coordinates[1],
                    face.coordinates[2],
                    face.coordinates[3],
                    width,
                ]
                .map(|v| {
                    if face.axis_material & LOOKUP == 0 {
                        f32::from(v) * grid
                    } else {
                        packed.coordinates[(u32::from(v) - (90003 - profile) * 20) as usize]
                    }
                });
                let y = if face.axis_material & NEGATIVE_Y != 0 {
                    -abs_y
                } else {
                    abs_y
                };
                assert_eq!(
                    [x, y, z, u, v].map(f32::to_bits),
                    [
                        q.anchor[0],
                        q.anchor[1],
                        q.anchor[2],
                        q.extent[0],
                        q.extent[1]
                    ]
                    .map(f32::to_bits)
                );
                assert_eq!(profile, 90000 + q.profile_index);
                assert_eq!(face.axis_material >> 29, q.axis_sign);
                assert_eq!(face.axis_material & (NEGATIVE_Y - 1), q.material);
            }
        }
    }
}
