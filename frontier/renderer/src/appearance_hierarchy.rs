//! Experimental conserved appearance hierarchy (not the default renderer).
//!
//! A world-aligned 16-native-cell leaf stores one directional face-area density.
//! Every supported footprint integrates that SAME leaf field: parent integrals
//! equal the sum of child integrals instead of changing quadrature phase with
//! geometry LOD. This is a box-filter hierarchy, evaluated lazily without a
//! world-sized mip pyramid. Partial leaves make translation continuous.
//!
//! The leaf density is still a quadrature estimate, not an exact native mesh.
//! Native neighbour differences preserve directional micro-relief; three depth
//! bands approximate exposed column color. Neither this sampling nor conserved
//! restriction certifies invisible LOD changes or volumetric surface ownership.

/// Canonical column queried in native coordinates. Colors are at h-1,h-2,h-13.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AppearanceColumn {
    pub height: i64,
    pub colors: [[f32; 3]; 3],
}

pub(crate) const LEAF_EDGE: i64 = 16;
/// Explicit CPU-work gate: at most 17*17 leaves and 1,445 column queries.
/// The caller must explicitly fall back outside this experimental range.
pub(crate) const MAX_EDGE_CELLS: i64 = 256;

/// Top,+X,-X,+Z,-Z, each containing unnormalized RGB*area and native face area.
/// Footprint bounds are center +/- edge/2, including half-cell bounds for odd
/// edges. No geometry-LOD-dependent sample offsets or native-world allocation.
pub(crate) fn profile<F>(
    center: [i64; 2],
    edge: i64,
    sample: &mut F,
) -> Result<[[f32; 4]; 5], String>
where
    F: FnMut(i64, i64) -> Result<AppearanceColumn, String>,
{
    if !(1..=MAX_EDGE_CELLS).contains(&edge) {
        return Err(format!(
            "appearance hierarchy edge {edge} outside 1..={MAX_EDGE_CELLS}"
        ));
    }
    // Doubled i128 coordinates retain half-cell bounds and avoid overflow at
    // negative/very large native coordinates. Source addresses remain i64.
    let min = center.map(|c| 2 * i128::from(c) - i128::from(edge));
    let max = center.map(|c| 2 * i128::from(c) + i128::from(edge));
    let leaf = 2 * i128::from(LEAF_EDGE);
    let first = min.map(|c| c.div_euclid(leaf));
    let last = max.map(|c| (c - 1).div_euclid(leaf));
    let mut total = [[0_f64; 4]; 5];
    for z in first[1]..=last[1] {
        for x in first[0]..=last[0] {
            let extent = |axis: usize, cell: i128| {
                (max[axis].min((cell + 1) * leaf) - min[axis].max(cell * leaf)) as f64 * 0.5
            };
            let weight = extent(0, x) * extent(1, z);
            let address = |cell: i128| {
                i64::try_from(cell * i128::from(LEAF_EDGE) + i128::from(LEAF_EDGE / 2))
                    .map_err(|_| "appearance hierarchy source address overflow".to_string())
            };
            let sx = address(x)?;
            let sz = address(z)?;
            let column = sample(sx, sz)?;
            add(&mut total[0], column.colors[0], weight);
            for (axis, dx, dz) in [(1, 1, 0), (2, -1, 0), (3, 0, 1), (4, 0, -1)] {
                let nx = sx
                    .checked_add(dx)
                    .ok_or("appearance hierarchy neighbour overflow")?;
                let nz = sz
                    .checked_add(dz)
                    .ok_or("appearance hierarchy neighbour overflow")?;
                let neighbour = sample(nx, nz)?;
                // Do subtraction before conversion: high world offsets must not
                // destroy small native height differences through float rounding.
                let drop = (i128::from(column.height) - i128::from(neighbour.height)).max(0);
                let bands = [drop.min(1), (drop - 1).clamp(0, 11), (drop - 12).max(0)];
                for (cells, color) in bands.into_iter().zip(column.colors) {
                    add(&mut total[axis], color, cells as f64 * weight);
                }
            }
        }
    }
    let out = total.map(|row| row.map(|value| value as f32));
    if out.iter().flatten().any(|value| !value.is_finite()) {
        return Err("appearance hierarchy produced non-finite area".into());
    }
    Ok(out)
}

fn add(row: &mut [f64; 4], color: [f32; 3], area: f64) {
    for channel in 0..3 {
        row[channel] += f64::from(color[channel]) * area;
    }
    row[3] += area;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terrain(x: i64, z: i64) -> Result<AppearanceColumn, String> {
        let phase = (x.div_euclid(16) + 3 * z.div_euclid(16)).rem_euclid(8) as f32;
        Ok(AppearanceColumn {
            height: 2 * x - 3 * z + (x.rem_euclid(7) - z.rem_euclid(5)),
            colors: [
                [phase / 8., 0.5, 0.25],
                [0.25, 0.5, 0.75],
                [0.5, 0.25, 0.125],
            ],
        })
    }

    fn close(a: [[f32; 4]; 5], b: [[f32; 4]; 5]) {
        for (a, b) in a.into_iter().flatten().zip(b.into_iter().flatten()) {
            assert!(
                (a - b).abs() <= 2e-6 * a.abs().max(b.abs()).max(1.),
                "{a} != {b}"
            );
        }
    }

    #[test]
    fn restriction_conserves_every_direction_and_color_on_supported_levels() {
        for center in [[0, 0], [-64, -32], [-19, 37], [83, -97]] {
            for edge in [4, 8, 16, 32, 64, 128, 256] {
                let parent = profile(center, edge, &mut terrain).unwrap();
                let mut sum = [[0.; 4]; 5];
                for dz in [-edge / 4, edge / 4] {
                    for dx in [-edge / 4, edge / 4] {
                        let child =
                            profile([center[0] + dx, center[1] + dz], edge / 2, &mut terrain)
                                .unwrap();
                        for a in 0..5 {
                            for c in 0..4 {
                                sum[a][c] += child[a][c];
                            }
                        }
                    }
                }
                close(parent, sum);
                assert_eq!(parent[0][3], (edge * edge) as f32);
            }
        }
    }

    #[test]
    fn sliding_footprint_integrates_a_step_continuously_across_zero() {
        let mut step = |x, _| {
            Ok(AppearanceColumn {
                height: 0,
                colors: [[if x < 0 { 1. } else { 0. }, 0., 0.]; 3],
            })
        };
        // The red field occupies x<0. A 16-cell box centered on zero
        // continuously loses one 16-unit strip per native translation.
        // Euclidean alignment is essential to obtain this for x<0.
        for x in -10..=10 {
            let p = profile([x, -3], 16, &mut step).unwrap();
            let red = (8 - x).clamp(0, 16) as f32 * 16.;
            assert_eq!(p[0], [red, 0., 0., 256.]);
            assert_eq!(p[1..], [[0.; 4]; 4]);
        }
        let a = profile([-6, -3], 16, &mut terrain).unwrap();
        let b = profile([-5, -3], 16, &mut terrain).unwrap();
        let c = profile([-4, -3], 16, &mut terrain).unwrap();
        close(
            b,
            std::array::from_fn(|i| std::array::from_fn(|j| (a[i][j] + c[i][j]) * 0.5)),
        );
    }

    #[test]
    fn directional_area_and_depth_bands_match_a_native_plane() {
        let mut plane = |x, z| {
            Ok(AppearanceColumn {
                height: 14 * x + 2 * z,
                colors: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            })
        };
        let p = profile([-5, 13], 17, &mut plane).unwrap();
        assert_eq!(p[0], [289., 0., 0., 289.]);
        assert_eq!(p[1], [0.; 4]);
        assert_eq!(p[2], [289., 3179., 578., 4046.]);
        assert_eq!(p[3], [0.; 4]);
        assert_eq!(p[4], [289., 289., 0., 578.]);
    }

    #[test]
    fn work_and_inputs_are_bounded() {
        let mut calls = 0;
        let mut source = |x, z| {
            calls += 1;
            terrain(x, z)
        };
        profile([-1, -1], MAX_EDGE_CELLS, &mut source).unwrap();
        for edge in [0, -1, MAX_EDGE_CELLS + 1, i64::MAX] {
            assert!(profile([0, 0], edge, &mut source).is_err());
        }
        assert_eq!(calls, 17 * 17 * 5);
        let mut flat = |_, _| {
            Ok(AppearanceColumn {
                height: 0,
                colors: [[0.; 3]; 3],
            })
        };
        assert!(profile([i64::MAX, 0], 32, &mut flat).is_err());
        assert!(profile([0, 0], 16, &mut |_, _| Err("cancelled".into()))
            .unwrap_err()
            .contains("cancelled"));
    }
}
