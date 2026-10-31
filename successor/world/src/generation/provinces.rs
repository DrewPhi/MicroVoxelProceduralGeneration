//! Cheap tectonic-inspired uplift, not a simulation of plate history.
//! Jittered 72 km plate sites carry velocities. Convergent boundaries uplift
//! long belts; interiors remain broad lowlands, hills or plateaus. Coordinates
//! are always source meters: voxel size and render LOD never enter this model.
use super::noise::{mix, noise, smoothstep};

const PLATE_PITCH: f64 = 72_000.;
#[derive(Clone, Copy)]
struct Plate {
    position: [f64; 2],
    velocity: [f64; 2],
    distance: f64,
}
fn random(hash: u64) -> f64 {
    (mix(hash) >> 11) as f64 / (1u64 << 53) as f64
}
#[derive(Clone, Copy)]
pub(super) struct Province {
    pub belt: f64,
    pub foothills: f64,
    pub plateau: f64,
    pub basin: f64,
    pub hill_country: f64,
}
pub(super) fn province(x: f64, z: f64, seed: u64) -> Province {
    // Slow domain warping curves boundaries without chopping them into blobs.
    let p = [
        x + 3500. * noise(x, z, 36_000., seed ^ 3001),
        z + 3500. * noise(x + 791., z - 1731., 36_000., seed ^ 3002),
    ];
    let cell = p.map(|v| (v / PLATE_PITCH).floor() as i64);
    let mut plates = [Plate {
        position: [0.; 2],
        velocity: [0.; 2],
        distance: 0.,
    }; 9];
    let mut index = 0;
    for dz in -1..=1 {
        for dx in -1..=1 {
            let gx = cell[0] + dx;
            let gz = cell[1] + dz;
            let h = mix(seed ^ mix(gx as u64) ^ mix(gz as u64).rotate_left(27));
            let position = [
                (gx as f64 + 0.3 + 0.4 * random(h)) * PLATE_PITCH,
                (gz as f64 + 0.3 + 0.4 * random(h ^ 11)) * PLATE_PITCH,
            ];
            let angle = random(h ^ 22) * std::f64::consts::TAU;
            plates[index] = Plate {
                position,
                velocity: [angle.cos(), angle.sin()],
                distance: (position[0] - p[0]).hypot(position[1] - p[1]),
            };
            index += 1;
        }
    }
    let nearest = plates
        .iter()
        .map(|p| p.distance)
        .fold(f64::INFINITY, f64::min);
    let width = 4200. + 1600. * noise(x, z, 28_000., seed ^ 3010);
    let mut belt = 0.;
    let mut foothills = 0.;
    // All locally relevant pairs contribute at junctions. A compact smooth
    // ownership field avoids discontinuities when nearest-site ranks change.
    for i in 0..plates.len() {
        for j in i + 1..plates.len() {
            let a = plates[i];
            let b = plates[j];
            let ownership = 1. - smoothstep(1000., 18_000., a.distance.max(b.distance) - nearest);
            if ownership <= 0. {
                continue;
            }
            let delta = [b.position[0] - a.position[0], b.position[1] - a.position[1]];
            let length = delta[0].hypot(delta[1]);
            let convergence = ((a.velocity[0] - b.velocity[0]) * delta[0]
                + (a.velocity[1] - b.velocity[1]) * delta[1])
                / (2. * length);
            let activity = smoothstep(-0.05, 0.65, convergence);
            let distance =
                (a.distance * a.distance - b.distance * b.distance).abs() / (2. * length);
            let core = activity * ownership * (-(distance / width).powi(2)).exp();
            let apron = activity * ownership * (-(distance / (width * 2.8)).powi(2)).exp();
            belt = 1. - (1. - belt) * (1. - core);
            foothills = 1. - (1. - foothills) * (1. - apron);
        }
    }
    Province {
        belt,
        foothills,
        plateau: smoothstep(
            -0.12,
            0.30,
            noise(x - 917., z + 1201., 32_000., seed ^ 3042),
        ),
        basin: smoothstep(0.05, 0.40, noise(x + 2153., z - 731., 27_000., seed ^ 3043)),
        hill_country: smoothstep(-0.10, 0.35, noise(x + 57., z + 527., 22_000., seed ^ 3044)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provinces_are_continuous_across_plate_search_cells() {
        for seed in [1, 42, 99] {
            for x in [-72_000., 0., 72_000.] {
                for z in [-15_000., 5000., 95_000.] {
                    let a = province(x - 0.01, z, seed);
                    let b = province(x + 0.01, z, seed);
                    assert!((a.belt - b.belt).abs() < 0.001);
                    assert!((a.foothills - b.foothills).abs() < 0.001);
                }
            }
        }
    }
    #[test]
    fn ranges_have_long_coherent_regions_and_quiet_interiors() {
        for seed in [1, 42, 99] {
            let mut quiet = 0;
            let mut mountain = 0;
            let mut joined = 0;
            for z in 0..200 {
                for x in 0..200 {
                    let p = province(x as f64 * 500., z as f64 * 500., seed);
                    quiet += usize::from(p.belt < 0.03);
                    if p.belt > 0.5 {
                        mountain += 1;
                        let neighbor = province(x as f64 * 500. + 500., z as f64 * 500., seed);
                        joined += usize::from(neighbor.belt > 0.3);
                    }
                }
            }
            assert!(quiet > 4000, "seed {seed}: no extensive quiet terrain");
            assert!(mountain > 100, "seed {seed}: no mountain belts");
            assert!(
                joined * 10 > mountain * 9,
                "seed {seed}: broken mountain belt"
            );
        }
    }
}
