//! Spatial ranges of opaque block faces in the uploaded instance array.
use crate::adaptive_geometry::{NativeBin, NativeQuad};
use glam::Vec3;
use std::ops::Range;

/// Split a direction bin into four contiguous quadrants. A long greedy face
/// belongs to one quadrant and its complete extent contributes to the bounds.
/// Assignment uses the anchor only. Geometry and material words are copied.
pub(super) fn partition(
    source: &[NativeQuad],
    output: &mut [NativeQuad],
    bins: &[NativeBin],
    center: Vec3,
) -> (Vec<NativeBin>, Vec<[Vec3; 2]>) {
    let mut ranges = Vec::with_capacity(4 * bins.len());
    let mut bounds = Vec::with_capacity(4 * bins.len());
    for bin in bins {
        let from = bin.first_quad as usize;
        let faces = &source[from..from + bin.quad_count as usize];
        let mut counts = [0; 4];
        let quadrant = |q: &NativeQuad| {
            usize::from(q.anchor[0] >= center.x) + 2 * usize::from(q.anchor[2] >= center.z)
        };
        for q in faces {
            counts[quadrant(q)] += 1;
        }
        let mut next = [from; 4];
        for i in 1..4 {
            next[i] = next[i - 1] + counts[i - 1];
        }
        let starts = next;
        let mut boxes = [[Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)]; 4];
        let axis = (bin.axis_sign & 3) as usize;
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        for q in faces {
            let i = quadrant(q);
            output[next[i]] = *q;
            next[i] += 1;
            let lo = Vec3::from_array(q.anchor);
            let mut hi = lo;
            hi[u] += q.extent[0];
            hi[v] += q.extent[1];
            boxes[i][0] = boxes[i][0].min(lo);
            boxes[i][1] = boxes[i][1].max(hi);
        }
        for i in 0..4 {
            if counts[i] > 0 {
                let [lo, hi] = boxes[i];
                ranges.push(NativeBin {
                    axis_sign: bin.axis_sign,
                    first_quad: starts[i] as u32,
                    quad_count: counts[i] as u32,
                    min_plane: lo[axis],
                    max_plane: hi[axis],
                });
                bounds.push([lo, hi]);
            }
        }
    }
    (ranges, bounds)
}

/// Every contiguous run in the visibility mask references adjacent records
/// in the same GPU buffer and can be issued as a single indexed draw.
pub(super) fn visible_runs(
    bins: &[NativeBin],
    mut mask: u32,
) -> impl Iterator<Item = Range<u32>> + '_ {
    std::iter::from_fn(move || {
        if mask == 0 {
            return None;
        }
        let first = mask.trailing_zeros();
        let length = (mask >> first).trailing_ones();
        let last = first + length - 1;
        mask &= u32::MAX.checked_shl(last + 1).unwrap_or(0);
        let end = bins[last as usize];
        Some(bins[first as usize].first_quad..end.first_quad + end.quad_count)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quadrants_preserve_faces_and_bound_greedy_extents() {
        let mut source = vec![];
        let mut bins = vec![];
        for sign in [0, 1, 2, 4, 5, 6] {
            let start = source.len() as u32;
            for [x, z] in [[11., -2.], [-5., 4.], [8., 11.], [-2., -3.], [-1., -2.]] {
                source.push(NativeQuad {
                    anchor: [x, 2., z],
                    extent: [23., 31.],
                    axis_sign: sign,
                    material: source.len() as u32,
                    profile_index: 7,
                });
            }
            bins.push(NativeBin {
                axis_sign: sign,
                first_quad: start,
                quad_count: 5,
                min_plane: -5.,
                max_plane: 11.,
            });
        }
        let mut output = source.clone();
        let (regions, bounds) = partition(&source, &mut output, &bins, Vec3::ZERO);
        assert_eq!(regions.len(), 24);
        let mut ids: Vec<_> = output.iter().map(|q| q.material).collect();
        ids.sort_unstable();
        assert_eq!(ids, (0..source.len() as u32).collect::<Vec<_>>());
        for (r, [lo, hi]) in regions.iter().zip(bounds) {
            for q in &output[r.first_quad as usize..(r.first_quad + r.quad_count) as usize] {
                let axis = (q.axis_sign & 3) as usize;
                let p = Vec3::from(q.anchor);
                let mut far = p;
                far[(axis + 1) % 3] += q.extent[0];
                far[(axis + 2) % 3] += q.extent[1];
                assert!(p.cmpge(lo).all() && far.cmple(hi).all());
                assert_eq!(r.axis_sign, q.axis_sign);
                assert_eq!(
                    bytemuck::bytes_of(q),
                    bytemuck::bytes_of(&source[q.material as usize])
                );
            }
        }
    }
    #[test]
    fn masked_runs_include_both_ends_of_a_full_word() {
        let bins: Vec<_> = (0..32)
            .map(|i| NativeBin {
                axis_sign: 1,
                first_quad: 3 * i,
                quad_count: 3,
                min_plane: 0.,
                max_plane: 1.,
            })
            .collect();
        assert_eq!(visible_runs(&bins, u32::MAX).collect::<Vec<_>>(), [0..96]);
        assert_eq!(
            visible_runs(&bins, 0x8000_001b).collect::<Vec<_>>(),
            [0..6, 9..15, 93..96]
        );
        assert_eq!(visible_runs(&bins, 0).count(), 0);
    }
}
