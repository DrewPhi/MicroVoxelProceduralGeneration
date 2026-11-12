//! Coherent concentric startup snapshots. Partial GPU arenas never own terrain.
//! Doubling limits repeated horizontal coverage to less than one third of the
//! final area; source caches survive between snapshots. Final-radius streaming
//! continues to use the existing incremental strips.

pub(super) fn enabled() -> bool {
    std::env::var_os("FRONTIER_NAADF_PROGRESSIVE_STARTUP").is_none_or(|v| v != "0")
}

pub(super) fn first_radius(target: u32, progressive: bool) -> u32 {
    if progressive {
        target.min(8)
    } else {
        target
    }
}

pub(super) fn next_radius(resident: u32, target: u32) -> u32 {
    resident.saturating_mul(2).min(target)
}

pub(super) fn contains(outer: [[f64; 3]; 2], inner: [[f64; 3]; 2]) -> bool {
    (0..3).all(|a| outer[0][a] <= inner[0][a] && outer[1][a] >= inner[1][a])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_radii_reach_requested_extent_without_overshoot() {
        for target in 1..=256 {
            let mut r = first_radius(target, true);
            assert!(r > 0 && r <= target);
            while r < target {
                let next = next_radius(r, target);
                assert!(next > r && next <= target);
                r = next;
            }
            assert_eq!(r, target);
            assert_eq!(first_radius(target, false), target);
        }
    }

    #[test]
    fn outgoing_coverage_only_needs_restoration_when_not_contained() {
        let old = [[-8.; 3], [8.; 3]];
        assert!(contains([[-16.; 3], [16.; 3]], old));
        assert!(contains(old, old));
        for axis in 0..3 {
            let mut shifted = [[-16.; 3], [16.; 3]];
            shifted[0][axis] = -7.;
            assert!(!contains(shifted, old));
        }
    }
}
