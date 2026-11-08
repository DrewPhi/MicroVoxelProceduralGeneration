//! Optional spatial appearance indirection. Headers and raw corner profiles share
//! one table; relative references survive rebasing the whole table on the GPU.

pub(crate) type Profile = [[f32; 4]; 5];

/// Encode a square cell and its four corner profile indices.
/// Invalid edge lengths deliberately remain invalid encodings for `valid_table`.
pub(crate) fn header(origin: [f32; 2], edge: f32, nodes: [u32; 4], at: u32) -> Profile {
    let mut result = [[0.; 4]; 5];
    result[0] = [origin[0], origin[1], 1. / edge, -1.];
    result[1] = nodes.map(|node| (i64::from(node) - i64::from(at)) as f32);
    result
}

/// Map descriptors are followed by packed relative node references. Each pack
/// reserves its top row for a tag and carries 16 references in the other rows.
pub(crate) fn map_header(
    origin: [f32; 2],
    spacing: f32,
    dimension: usize,
    refs: &[u32],
    at: u32,
) -> Vec<Profile> {
    let count = refs.len().div_ceil(16);
    let mut header = [[0.; 4]; 5];
    header[0] = [origin[0], origin[1], 1. / spacing, -2.];
    header[1] = [dimension as f32, count as f32, 0., 0.];
    let mut result = vec![header];
    for chunk in refs.chunks(16) {
        let mut record = [[0.; 4]; 5];
        record[0][3] = -4.;
        for (i, &reference) in chunk.iter().enumerate() {
            record[1 + i / 4][i % 4] = (i64::from(reference) - i64::from(at)) as f32;
        }
        result.push(record);
    }
    result
}

/// Center order is X,Z,Y to match the descriptor's map-coordinate convention.
pub(crate) fn visibility_header(center: [f32; 3], base: u32, at: u32) -> Profile {
    let mut result = [[0.; 4]; 5];
    result[0] = [center[0], center[1], center[2], -3.];
    result[1] = [(i64::from(base) - i64::from(at)) as f32, 0.03125, 0.25, 0.];
    result
}

/// Bilinear spatial nodes, each carrying one visibility-conditioned descriptor.
/// The fixed -5 -> -3 -> raw structure prevents recursive descriptor cycles.
pub(crate) fn visibility_spatial_header(
    origin: [f32; 2],
    edge: f32,
    nodes: [u32; 4],
    at: u32,
) -> Profile {
    let mut result = header(origin, edge, nodes, at);
    result[0][3] = -5.;
    result
}

pub(crate) fn is_header(profile: &Profile) -> bool {
    profile[0][3] < 0.
}

/// Validate every descriptor, owned reference pack, and raw target before upload.
/// Signed relative references survive relocation of the entire table.
pub(crate) fn valid_table(table: &[Profile]) -> bool {
    if table.iter().flatten().flatten().any(|v| !v.is_finite()) {
        return false;
    }
    let target = |index: usize, offset: f32| -> Option<usize> {
        if offset.fract() != 0. {
            return None;
        }
        let destination = index as f64 + f64::from(offset);
        if destination < 0. || destination >= table.len() as f64 {
            return None;
        }
        Some(destination as usize)
    };
    let raw_target = |index: usize, offset: f32| -> Option<usize> {
        let destination = target(index, offset)?;
        (!is_header(&table[destination])).then_some(destination)
    };
    let zeros = |p: &Profile| p[2..].iter().flatten().all(|v| *v == 0.);
    let mut owned = vec![false; table.len()];
    for (index, p) in table.iter().enumerate() {
        match p[0][3] {
            -1. => {
                if p[0][2] <= 0. || !zeros(p) {
                    return false;
                }
                if p[1]
                    .iter()
                    .any(|&offset| raw_target(index, offset).is_none())
                {
                    return false;
                }
            }
            -2. => {
                let dimension = p[1][0];
                if p[0][2] <= 0.
                    || dimension.fract() != 0.
                    || !(2. ..=10.).contains(&dimension)
                    || p[1][2..] != [0., 0.]
                    || !zeros(p)
                {
                    return false;
                }
                let nodes = (dimension as usize).pow(2);
                let packs = nodes.div_ceil(16);
                if p[1][1] != packs as f32 || index + packs >= table.len() {
                    return false;
                }
                for pack in 0..packs {
                    let slot = index + 1 + pack;
                    let record = &table[slot];
                    if owned[slot] || record[0] != [0., 0., 0., -4.] {
                        return false;
                    }
                    owned[slot] = true;
                    for lane in 0..16 {
                        let offset = record[1 + lane / 4][lane % 4];
                        if pack * 16 + lane < nodes {
                            if raw_target(index, offset).is_none() {
                                return false;
                            }
                        } else if offset != 0. {
                            return false;
                        }
                    }
                }
            }
            -3. => {
                if p[1][1..] != [0.03125, 0.25, 0.] || !zeros(p) {
                    return false;
                }
                let Some(base) = raw_target(index, p[1][0]) else {
                    return false;
                };
                if base.checked_add(8).is_none_or(|end| end > table.len()) {
                    return false;
                }
                if table[base..base + 8].iter().any(is_header) {
                    return false;
                }
            }
            -5. => {
                if p[0][2] <= 0. || !zeros(p) {
                    return false;
                }
                if p[1].iter().any(|&offset| {
                    target(index, offset).is_none_or(|node| table[node][0][3] != -3.)
                }) {
                    return false;
                }
            }
            -4. => {} // Ownership is checked after all map descriptors.
            tag if tag < 0. => return false,
            _ => {
                if p.iter().flatten().any(|v| *v < 0.) {
                    return false;
                }
            }
        }
    }
    table
        .iter()
        .enumerate()
        .all(|(i, p)| p[0][3] != -4. || owned[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corners() -> Vec<Profile> {
        vec![[[0.25, 0.5, 0.75, 1.]; 5]; 4]
    }

    #[test]
    fn negative_positions_and_backward_offsets_are_valid() {
        let mut table = corners();
        table.push(header([-64., -128.], 32., [0, 1, 2, 3], 4));
        assert_eq!(table[4][0], [-64., -128., 1. / 32., -1.]);
        assert_eq!(table[4][1], [-4., -3., -2., -1.]);
        assert!(valid_table(&table));
        // Prefixing profiles moves header and nodes together: no rewrite needed.
        table.insert(0, [[0.; 4]; 5]);
        assert!(valid_table(&table));
    }

    #[test]
    fn forward_references_and_zero_weight_nodes_are_valid() {
        let mut table = vec![header([0., 0.], 1., [1, 2, 3, 4], 0)];
        table.extend(vec![[[0.; 4]; 5]; 4]);
        assert!(valid_table(&table));
    }

    #[test]
    fn invalid_references_and_nested_headers_are_rejected() {
        for offset in [-1., 0., 5., 1.5, f32::MAX] {
            let mut table = vec![header([0., 0.], 1., [1, 2, 3, 4], 0)];
            table.extend(corners());
            table[0][1][0] = offset;
            assert!(!valid_table(&table), "offset {offset}");
        }
        let mut table = corners();
        table.push(header([0., 0.], 1., [0, 1, 2, 3], 4));
        table.push(header([0., 0.], 1., [0, 1, 2, 4], 5));
        assert!(!valid_table(&table));
    }

    #[test]
    fn invalid_edges_nonfinite_values_and_negative_raw_values_are_rejected() {
        for edge in [0., -1., f32::INFINITY, f32::NAN] {
            let mut table = corners();
            table.push(header([0., 0.], edge, [0, 1, 2, 3], 4));
            assert!(!valid_table(&table));
        }
        for value in [-0.01, f32::NAN, f32::INFINITY] {
            let mut table = corners();
            table[2][3][1] = value;
            assert!(!valid_table(&table));
        }
        let mut table = corners();
        table.push(header([f32::NAN, 0.], 1., [0, 1, 2, 3], 4));
        assert!(!valid_table(&table));
    }

    #[test]
    fn map_packs_and_visibility_survive_relocation() {
        let mut table = vec![[[0.25, 0.5, 0.75, 1.]; 5]; 25];
        let refs: Vec<_> = (0..25).collect();
        table.extend(map_header([-64., -32.], 4., 5, &refs, 25));
        table.push(visibility_header([-16., -32., 10.], 0, table.len() as u32));
        assert!(valid_table(&table));
        assert_eq!(table[26][1], [-25., -24., -23., -22.]);
        table.insert(0, [[0.; 4]; 5]);
        assert!(valid_table(&table));
    }

    #[test]
    fn malformed_packs_nested_targets_and_visibility_ranges_are_rejected() {
        let make = || {
            let mut table = vec![[[1.; 4]; 5]; 25];
            table.extend(map_header(
                [0., 0.],
                4.,
                5,
                &(0..25).collect::<Vec<_>>(),
                25,
            ));
            table
        };
        let mut table = make();
        table[26][1][0] = 0.;
        assert!(!valid_table(&table));
        let mut table = make();
        table[26][1][0] = 1.5;
        assert!(!valid_table(&table));
        let mut table = make();
        table[27][4][3] = 1.;
        assert!(!valid_table(&table));
        let mut table = make();
        table[25][1][1] = 1.;
        assert!(!valid_table(&table));
        let mut table = make();
        table[27][0][3] = -2.;
        assert!(!valid_table(&table));
        let mut table = make();
        table.push(table[26]);
        assert!(!valid_table(&table));
        let mut table = make();
        table.push(visibility_header([0.; 3], 23, 28));
        assert!(!valid_table(&table));
        let mut table = make();
        table.push(visibility_header([0.; 3], 0, 28));
        table[0] = header([0., 0.], 1., [1, 2, 3, 4], 0);
        assert!(!valid_table(&table));
    }

    fn spatial_visibility_table() -> Vec<Profile> {
        let mut table = vec![[[0.25, 0.5, 0.75, 1.]; 5]; 32];
        for i in 0..4 {
            table.push(visibility_header([i as f32, 0., 10.], i * 8, 32 + i));
        }
        table.push(visibility_spatial_header(
            [-32., -64.],
            16.,
            [32, 33, 34, 35],
            36,
        ));
        table
    }

    #[test]
    fn spatial_visibility_headers_preserve_relative_references_when_rebased() {
        let mut table = spatial_visibility_table();
        assert_eq!(table[36][0], [-32., -64., 1. / 16., -5.]);
        assert_eq!(table[36][1], [-4., -3., -2., -1.]);
        assert!(valid_table(&table));
        table.insert(0, [[0.; 4]; 5]);
        assert!(valid_table(&table));
        // Forward node references are legal as well.
        let descriptor = table.pop().unwrap();
        table.insert(0, descriptor);
        table[0][1] = [34., 35., 36., 37.];
        // Moving only the descriptor does not change references inside -3 nodes.
        assert!(valid_table(&table));
    }

    #[test]
    fn spatial_visibility_requires_exactly_one_conditional_descriptor_level() {
        for offset in [-36., 0., 1., 1.5, f32::MAX] {
            let mut table = spatial_visibility_table();
            table[36][1][0] = offset;
            assert!(!valid_table(&table));
        }
        let mut table = spatial_visibility_table();
        table[32] = visibility_spatial_header([0.; 2], 1., [32; 4], 32);
        assert!(!valid_table(&table));
        let mut table = spatial_visibility_table();
        table[32][1][0] = 4.; // Conditional node cannot point at spatial header.
        assert!(!valid_table(&table));
        for edge in [0., -1., f32::NAN, f32::INFINITY] {
            let mut table = spatial_visibility_table();
            table[36] = visibility_spatial_header([0.; 2], edge, [32, 33, 34, 35], 36);
            assert!(!valid_table(&table));
        }
    }
}
