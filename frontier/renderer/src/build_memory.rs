//! Conservative admission model for simultaneous adaptive builders. This charges
//! Vec growth, old/new output overlap, deduplication and local source workspace.
//! The explicit 2x allowance covers allocator/hash-table implementation overhead;
//! this is an accounted heap model, not a process RSS or thread-stack ceiling.
use crate::adaptive_geometry::TileKey;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BuildMemoryStats {
    pub budget_bytes: usize,
    pub reserved_bytes: usize,
    pub peak_reserved_bytes: usize,
    pub evicted_arc_bytes: usize,
    pub refused_jobs: u64,
    pub last_batch_jobs: usize,
}

pub(crate) fn reservation(key: TileKey) -> Option<usize> {
    let w = key.width_cells as u128;
    let step = key.step_cells as u128;
    if w == 0 || w > 1 << 20 || step == 0 || step > w || w % step != 0 {
        return None;
    }
    let g = w / step;
    if g * g > 65_536
        || key
            .edge_steps
            .iter()
            .any(|&e| e == 0 || e as u128 > step || step % e as u128 != 0)
    {
        return None;
    }
    let perimeter: u128 = key.edge_steps.iter().map(|&e| w / e as u128).sum();
    if perimeter > 65_536 {
        return None;
    }
    let halo = (w + 2) * (w + 2);
    let modeled = if step == 1 {
        if w > 128 {
            return None;
        }
        // One top per column. An internal undirected neighbor pair exposes at
        // most one side, with <=3 material spans; the outer boundary has 4*w
        // edges. Splitting greedy faces at profile blocks cannot exceed this
        // fully unmerged per-column bound.
        let quads = w * w + 3 * (2 * w * (w - 1) + 4 * w);
        // Old and new expanded meshes (4 vertices + 6 indices per quad), each
        // at <=2x logical Vec capacity, plus packed quads, direction bins and
        // profile-address capacity. These are charged together conservatively
        // even though several phases release their inputs before later phases.
        quads * (4 * (4 * 36 + 6 * 4) + 32 + 40 + 32)
            + halo * 64
            + w * w * 32
            + ((w / 16 + 2).pow(2) + 1) * 256
    } else {
        let native_edges = key.edge_steps.iter().filter(|&&e| e == 1).count() as u128;
        // Interior fans, subdivided perimeter fans (including native stair
        // jumps), then <=3 two-triangle material spans per native boundary cell.
        let vertices =
            (3 * (4 * g * g + 2 * perimeter + 4 * g + 6 * native_edges * w)).min(1_000_000);
        // Base vectors double on growth. Compaction can coexist with original
        // vertices, a preallocated hash table, remap, and old/new shrink buffers.
        let compact = vertices.min(500_000) * 252;
        let columns = if w <= 128 {
            halo * 32
        } else {
            // At most 7x7 appearance probes per patch, each with four neighbors,
            // plus center/corners and perimeter probes; generation also has an
            // explicit 300k unique local-column limit.
            (250 * g * g + 4 * perimeter).min(300_000) * 256
        };
        // Packing is performed before admission while every worker output may
        // remain live. Reserve the new 64-byte primitive alongside its inputs.
        vertices * 88 + (vertices / 3) * 64 + compact + columns + (g * g + 1) * 160
    };
    // Fixed temporary vectors, clipping/affine workspace, small allocations;
    // factor two is deliberately retained pending measured allocation auditing.
    let deferred_profiles = if crate::integrated_profiles::enabled()
        && w == 128
        && step >= 8
        && key.source_revision == 0
    {
        // A bounded canonical page conversion can hold the sampled columns
        // and packed GPU columns together. Jobs and the retained payload also
        // remain live until publication; use the existing conservative factor.
        130 * 130 * (std::mem::size_of::<crate::column_mesh::SampledColumn>() as u128 + 4)
            + 512 * 32
    } else {
        0
    };
    usize::try_from((modeled + deferred_profiles + 1024 * 1024) * 2).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(width: u32, step: u32) -> TileKey {
        TileKey {
            min: [0, 0],
            width_cells: width,
            step_cells: step,
            edge_steps: [step; 4],
            source_revision: 0,
        }
    }
    #[test]
    fn native_admission_is_bounded_and_default_budget_allows_two_full_jobs() {
        let full = reservation(key(128, 1)).unwrap();
        assert!(full > 128 * 1024 * 1024);
        assert!(full * 2 <= 512 * 1024 * 1024);
        assert!(full * 3 > 512 * 1024 * 1024);
        assert!(reservation(key(64, 1)).unwrap() < full);
        assert_eq!(reservation(key(256, 1)), None);
    }
    #[test]
    fn giant_invalid_work_is_rejected_without_allocation() {
        assert_eq!(reservation(key(u32::MAX, 1)), None);
        assert_eq!(reservation(key(1 << 20, 2)), None);
        let mut invalid = key(128, 2);
        invalid.edge_steps[0] = 0;
        assert_eq!(reservation(invalid), None);
    }
    #[test]
    fn native_edges_and_dense_proxy_work_increase_reservation() {
        let coarse = key(128, 16);
        let mut joined = coarse;
        joined.edge_steps = [1; 4];
        assert!(reservation(joined).unwrap() > reservation(coarse).unwrap());
        assert!(reservation(key(128, 2)).unwrap() > reservation(coarse).unwrap());
        assert!(reservation(key(1 << 20, 1 << 17)).unwrap() < 32 * 1024 * 1024);
    }
    #[test]
    fn native_count_bound_covers_steep_profiles_and_unaligned_blocks() {
        let world = frontier_world::World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        for (width, min) in [(64, [1, -511]), (128, [-127, -1023]), (128, [0, -512])] {
            let mut key = key(width, 1);
            key.min = min;
            let tile = crate::adaptive_geometry::build(&world, key, || false).unwrap();
            let w = width as usize;
            let quads = 7 * w * w + 6 * w;
            assert!(tile.native_quads.len() <= quads);
            assert!(tile.native_quads.capacity() <= 2 * quads);
            assert!(tile.native_bins.capacity() <= 2 * quads);
            assert!(tile.bytes <= reservation(key).unwrap());
            assert!(tile.exact);
        }
    }
    #[test]
    fn proxy_payload_capacity_fits_reservation_with_native_join_edges() {
        let world = frontier_world::World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        for width in [64, 128] {
            for step in [2, 8, 16] {
                let mut key = key(width, step);
                key.min = [-127, -511];
                key.edge_steps = [1, step, 1, step];
                let tile = crate::adaptive_geometry::build(&world, key, || false).unwrap();
                assert!(tile.bytes <= reservation(key).unwrap());
                assert!(tile.sampled_columns <= (width as usize + 2).pow(2));
            }
        }
    }
}
