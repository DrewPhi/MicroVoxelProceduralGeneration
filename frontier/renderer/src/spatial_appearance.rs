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

pub(crate) fn is_header(profile: &Profile) -> bool {
    profile[0][3] == -1.
}

/// Validate CPU/cache data before upload. Raw face colors and area weights are
/// nonnegative; header positions and relative offsets may legitimately be negative.
pub(crate) fn valid_table(table: &[Profile]) -> bool {
    for (index, profile) in table.iter().enumerate() {
        if !profile.iter().flatten().all(|value| value.is_finite()) {
            return false;
        }
        if !is_header(profile) {
            if profile.iter().flatten().any(|value| *value < 0.) {
                return false;
            }
            continue;
        }
        if profile[0][2] <= 0. || profile[2..].iter().flatten().any(|value| *value != 0.) {
            return false;
        }
        for &offset in &profile[1] {
            if offset.fract() != 0. {
                return false;
            }
            let target = index as f64 + f64::from(offset);
            if target < 0. || target >= table.len() as f64 || is_header(&table[target as usize]) {
                return false;
            }
        }
    }
    true
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
}
