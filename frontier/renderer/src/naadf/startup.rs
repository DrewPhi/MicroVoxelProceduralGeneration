//! Reserve a bounded address space once; publish only completed inner regions.
//! Expansion uses the same shell/strip transactions as player following.
use super::stream::Region;

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

pub(super) fn next_region(current: Region, target: u32, center: [f64; 3]) -> Region {
    let radius = ((current.hi[0] - current.lo[0]) as u32 / 2 + 4).min(target) as i64;
    let center = center.map(|meters| meters / super::stream::root_size_m());
    Region {
        lo: center.map(|v| v.floor() as i64 - radius),
        hi: center.map(|v| v.floor() as i64 + radius),
    }
}

pub(super) fn contains(outer: [[f64; 3]; 2], inner: [[f64; 3]; 2]) -> bool {
    (0..3).all(|a| outer[0][a] <= inner[0][a] && outer[1][a] >= inner[1][a])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn growth_is_bounded_and_preserves_stationary_interior() {
        for target in 1..=256 {
            let r = first_radius(target, true) as i64;
            let mut region = Region {
                lo: [-r; 3],
                hi: [r; 3],
            };
            while region.dims()[0] < target as usize * 2 {
                let next = next_region(region, target, [0.; 3]);
                assert!(contains(next.bounds(), region.bounds()));
                assert!(next.dims()[0] > region.dims()[0]);
                assert!(next.dims()[0] <= target as usize * 2);
                assert!(next.dims()[0] <= region.dims()[0] + 8);
                region = next;
            }
            assert_eq!(region.dims(), [target as usize * 2; 3]);
        }
    }
    #[test]
    fn moving_growth_uses_integer_world_bounds_and_never_exceeds_target() {
        let a = Region {
            lo: [-8; 3],
            hi: [8; 3],
        };
        let b = next_region(a, 10, [-15.1, 5.3, 70.]);
        assert_eq!(b.dims(), [20; 3]);
        assert_eq!(b.lo, [-26, -5, 60]);
        assert!(!contains(b.bounds(), a.bounds()));
    }
}

/// Reserve likely surface payload from the bootstrap density when RAM allows.
/// This is capacity only: no additional source voxels are evaluated. Keeping
/// one backing allocation avoids driver fragmentation from repeated resizing.
pub(super) fn arena_reservation(
    used: [usize; 3],
    floor: [usize; 3],
    limits: [usize; 3],
    initial_edge: usize,
    storage_edge: usize,
    allowance: usize,
) -> [usize; 3] {
    let mut wanted = floor;
    for i in 0..2 {
        let density_estimate = (used[i] as u128 * storage_edge as u128 * storage_edge as u128 * 3)
            / (initial_edge as u128 * initial_edge as u128 * 2);
        wanted[i] = floor[i].max(density_estimate.min(limits[i] as u128) as usize);
    }
    let base = floor.iter().sum::<usize>();
    let extra = wanted.iter().zip(floor).map(|(w, f)| w - f).sum::<usize>();
    if extra == 0 || allowance <= base {
        return floor;
    }
    let available = (allowance - base).min(extra);
    std::array::from_fn(|i| {
        let growth = ((wanted[i] - floor[i]) as u128 * available as u128 / extra as u128) as usize;
        floor[i] + (growth & !255)
    })
}

#[cfg(test)]
mod reservation_tests {
    use super::*;
    #[test]
    fn density_reservation_is_bounded_and_leaves_palette_unscaled() {
        let mib = 1048576;
        let floor = [8 * mib, 24 * mib, 2 * mib];
        let used = [100_000, 600_000, 8_000];
        for allowance in [34 * mib, 128 * mib, 512 * mib, 2048 * mib] {
            let capacity = arena_reservation(used, floor, [1024 * mib; 3], 16, 512, allowance);
            assert!(capacity.iter().sum::<usize>() <= allowance);
            assert!((0..3).all(|i| capacity[i] >= floor[i] && capacity[i] <= 1024 * mib));
            assert_eq!(capacity[2], floor[2]);
        }
        let capacity = arena_reservation(used, floor, [1024 * mib; 3], 16, 512, 2048 * mib);
        assert!(capacity[1] > 800 * mib);
        assert_eq!(
            arena_reservation(used, floor, [1024 * mib; 3], 16, 512, 0),
            floor
        );
    }
}

/// Sparse milestones are also useful without opening the in-game menu.
pub(super) fn log_residency(resident: f64, target: f64, seconds: f64) {
    if resident <= 8. || resident % 32. < super::stream::root_size_m() || resident >= target {
        eprintln!("NAADF residency: {resident:.2}/{target:.2} m ({seconds:.2}s)");
    }
}
