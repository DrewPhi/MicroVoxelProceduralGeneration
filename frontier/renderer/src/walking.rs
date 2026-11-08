//! Small axis-swept canonical-cell walking controller, independent of display LOD.
use frontier_world::{Cell, Material, World, VOXEL_SIZE};
use std::collections::HashMap;

pub const EYE_HEIGHT: f64 = 1.6875;
const BODY_HEIGHT: f64 = 1.75;
const HALF_WIDTH: f64 = 0.225;
const EPS: f64 = 1e-8;
const GRAVITY: f64 = 20.;
const JUMP_SPEED: f64 = 5.5;

#[derive(Default)]
pub struct WalkingCamera {
    vertical_velocity: f64,
    grounded: bool,
    jump_was_held: bool,
    last_eye: Option<[f64; 3]>,
    samples: HashMap<Cell, bool>,
}
impl WalkingCamera {
    pub fn reset(&mut self) {
        self.vertical_velocity = 0.;
        self.grounded = false;
        self.jump_was_held = false;
        self.last_eye = None;
        self.samples.clear();
    }
    pub fn advance(
        &mut self,
        world: &World,
        eye: [f64; 3],
        horizontal_velocity: [f64; 2],
        jump_held: bool,
        dt: f64,
    ) -> [f64; 3] {
        self.advance_sampled(eye, horizontal_velocity, jump_held, dt, |c| {
            world.blocking_material(c)
        })
    }
    fn advance_sampled(
        &mut self,
        mut eye: [f64; 3],
        horizontal: [f64; 2],
        jump_held: bool,
        dt: f64,
        mut material: impl FnMut(Cell) -> Material,
    ) -> [f64; 3] {
        if !eye
            .into_iter()
            .chain(horizontal)
            .chain([dt])
            .all(f64::is_finite)
            || dt <= 0.
        {
            return eye;
        }
        // Shared by sweeps/substeps; querying a canonical cell twice never
        // repeats procedural terrain or tree work within this simulation tick.
        self.samples.clear();
        let samples = &mut self.samples;
        let mut solid = |c| {
            *samples
                .entry(c)
                .or_insert_with(|| material(c).is_blocking())
        };
        let relocated = self
            .last_eye
            .is_none_or(|last| last.into_iter().zip(eye).any(|(a, b)| (a - b).abs() > 2.));
        if relocated {
            self.vertical_velocity = 0.;
            self.grounded = false;
            if overlaps(eye, &mut solid) {
                // Startup/teleport only. Ordinary frames use leading faces.
                let start = eye;
                for step in 1..=32 {
                    eye[1] = start[1] + step as f64 * VOXEL_SIZE;
                    if !overlaps(eye, &mut solid) {
                        break;
                    }
                }
                if overlaps(eye, &mut solid) {
                    eye = start;
                }
            }
        }
        let probe = sweep(eye, 1, -0.002, &mut solid);
        self.grounded = self.vertical_velocity <= 0. && probe > -0.002 + EPS;
        if self.grounded && jump_held && !self.jump_was_held {
            self.vertical_velocity = JUMP_SPEED;
            self.grounded = false;
        }
        self.jump_was_held = jump_held;
        let elapsed = dt.min(0.25);
        let speed = horizontal[0]
            .abs()
            .max(horizontal[1].abs())
            .max(self.vertical_velocity.abs() + GRAVITY * elapsed);
        let steps = (elapsed * 120.)
            .ceil()
            .max((elapsed * speed / (VOXEL_SIZE * 0.5)).ceil())
            .max(1.) as usize;
        let sub_dt = elapsed / steps as f64;
        for _ in 0..steps {
            for (axis, velocity) in [(0, horizontal[0]), (2, horizontal[1])] {
                let delta = velocity * sub_dt;
                let allowed = sweep(eye, axis, delta, &mut solid);
                if (allowed - delta).abs() > EPS && self.grounded {
                    let mut stepped = false;
                    for cells in 1..=3 {
                        let rise = cells as f64 * VOXEL_SIZE;
                        if (sweep(eye, 1, rise, &mut solid) - rise).abs() > EPS {
                            break;
                        }
                        let mut raised = eye;
                        raised[1] += rise;
                        if (sweep(raised, axis, delta, &mut solid) - delta).abs() <= EPS {
                            raised[axis] += delta;
                            eye = raised;
                            self.vertical_velocity = 0.;
                            stepped = true;
                            break;
                        }
                    }
                    if stepped {
                        continue;
                    }
                }
                eye[axis] += allowed;
            }
            self.vertical_velocity -= GRAVITY * sub_dt;
            let vertical = self.vertical_velocity * sub_dt;
            let allowed = sweep(eye, 1, vertical, &mut solid);
            eye[1] += allowed;
            self.grounded = vertical < 0. && (allowed - vertical).abs() > EPS;
            if (allowed - vertical).abs() > EPS {
                self.vertical_velocity = 0.;
            }
        }
        self.last_eye = Some(eye);
        eye
    }
}

fn bounds(eye: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    (
        [
            eye[0] - HALF_WIDTH,
            eye[1] - EYE_HEIGHT,
            eye[2] - HALF_WIDTH,
        ],
        [
            eye[0] + HALF_WIDTH,
            eye[1] - EYE_HEIGHT + BODY_HEIGHT,
            eye[2] + HALF_WIDTH,
        ],
    )
}
fn indices(min: [f64; 3], max: [f64; 3]) -> ([i64; 3], [i64; 3]) {
    (
        min.map(|p| ((p + EPS) / VOXEL_SIZE).floor() as i64),
        max.map(|p| ((p - EPS) / VOXEL_SIZE).floor() as i64),
    )
}
fn cell(p: [i64; 3]) -> Cell {
    Cell {
        x: p[0],
        y: p[1],
        z: p[2],
    }
}
fn overlaps(eye: [f64; 3], solid: &mut impl FnMut(Cell) -> bool) -> bool {
    let (min, max) = bounds(eye);
    let (lo, hi) = indices(min, max);
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                if solid(Cell { x, y, z }) {
                    return true;
                }
            }
        }
    }
    false
}
/// Tests exact body overlap; touching a voxel face does not count as penetration.
pub(crate) fn aabb_blocked(eye: [f64; 3], mut material: impl FnMut(Cell) -> Material) -> bool {
    overlaps(eye, &mut |c| material(c).is_blocking())
}
fn sweep(eye: [f64; 3], axis: usize, delta: f64, solid: &mut impl FnMut(Cell) -> bool) -> f64 {
    if delta == 0. {
        return 0.;
    }
    let (min, max) = bounds(eye);
    let (lo, hi) = indices(min, max);
    let a = (axis + 1) % 3;
    let b = (axis + 2) % 3;
    let positive = delta > 0.;
    let mut plane = if positive { hi[axis] + 1 } else { lo[axis] - 1 };
    let last = if positive {
        ((max[axis] + delta - EPS) / VOXEL_SIZE).floor() as i64
    } else {
        ((min[axis] + delta + EPS) / VOXEL_SIZE).floor() as i64
    };
    while if positive {
        plane <= last
    } else {
        plane >= last
    } {
        for v in lo[b]..=hi[b] {
            for u in lo[a]..=hi[a] {
                let mut p = [0; 3];
                p[axis] = plane;
                p[a] = u;
                p[b] = v;
                if solid(cell(p)) {
                    return if positive {
                        (plane as f64 * VOXEL_SIZE - max[axis] - EPS)
                            .max(0.)
                            .min(delta)
                    } else {
                        (((plane + 1) as f64 * VOXEL_SIZE - min[axis]) + EPS)
                            .min(0.)
                            .max(delta)
                    };
                }
            }
        }
        plane += if positive { 1 } else { -1 };
    }
    delta
}

#[cfg(test)]
mod tests {
    use super::*;
    fn floor(c: Cell) -> Material {
        if c.y < 0 {
            Material::Stone
        } else {
            Material::Air
        }
    }
    #[test]
    fn thin_wood_wall_stops_sprint_and_slides() {
        let mut walker = WalkingCamera::default();
        let sample = |c: Cell| {
            if c.x == 16 && c.y >= 0 && c.y < 64 {
                Material::Wood
            } else {
                floor(c)
            }
        };
        let mut eye = [0., EYE_HEIGHT, 0.];
        for _ in 0..20 {
            eye = walker.advance_sampled(eye, [18., 3.], false, 1. / 60., sample);
        }
        assert!(eye[0] <= 1. - HALF_WIDTH);
        assert!((eye[2] - 1.).abs() < 1e-6);
        assert!(!aabb_blocked(eye, sample));
    }
    #[test]
    fn leaves_and_flowers_do_not_block_the_body() {
        assert!(!aabb_blocked([0., EYE_HEIGHT, 0.], |_| Material::Leaf));
        assert!(!aabb_blocked([0., EYE_HEIGHT, 0.], |_| Material::Flower));
        assert!(aabb_blocked([0., EYE_HEIGHT, 0.], |_| Material::Wood));
    }
    #[test]
    fn resting_frame_samples_support_only_and_reuses_substep_queries() {
        let mut walker = WalkingCamera::default();
        let eye = walker.advance_sampled([0., EYE_HEIGHT, 0.], [0., 0.], false, 1. / 60., floor);
        let mut queries = 0;
        walker.advance_sampled(eye, [0., 0.], false, 0.1, |c| {
            queries += 1;
            floor(c)
        });
        // The 0.45m footprint spans at most eight cells in each direction.
        // Twelve gravity substeps must not resample the 28-cell-high body.
        assert!(queries <= 64, "resting queries: {queries}");
    }
    #[test]
    fn gravity_lands_and_jump_requires_release() {
        let mut walker = WalkingCamera::default();
        let mut eye = [0., EYE_HEIGHT + 3., 0.];
        for _ in 0..120 {
            eye = walker.advance_sampled(eye, [0., 0.], false, 1. / 60., floor);
        }
        assert!((eye[1] - EYE_HEIGHT).abs() < 1e-6);
        let airborne = walker.advance_sampled(eye, [0., 0.], true, 1. / 60., floor);
        assert!(airborne[1] > eye[1]);
        eye = airborne;
        for _ in 0..180 {
            eye = walker.advance_sampled(eye, [0., 0.], true, 1. / 60., floor);
        }
        assert!((eye[1] - EYE_HEIGHT).abs() < 1e-6);
    }
    #[test]
    fn three_cell_step_climbs_but_four_cell_step_blocks() {
        for height in [3, 4] {
            let sample = |c: Cell| {
                if c.x >= 8 && c.y < height {
                    Material::Stone
                } else {
                    floor(c)
                }
            };
            let mut walker = WalkingCamera::default();
            let mut eye = [0., EYE_HEIGHT, 0.];
            for _ in 0..60 {
                eye = walker.advance_sampled(eye, [2., 0.], false, 1. / 60., sample);
            }
            if height == 3 {
                assert!(eye[0] > 1.);
                assert!((eye[1] - EYE_HEIGHT - 3. * VOXEL_SIZE).abs() < 1e-6);
            } else {
                assert!(eye[0] <= 0.5 - HALF_WIDTH);
            }
            assert!(!aabb_blocked(eye, sample));
        }
    }
    #[test]
    fn upward_sweep_blocks_low_ceiling_and_negative_wall() {
        let sample = |c: Cell| {
            if c.y == 30 || c.x == -8 {
                Material::Wood
            } else {
                Material::Air
            }
        };
        let eye = [0., EYE_HEIGHT, 0.];
        let rise = sweep(eye, 1, 4., &mut |c| sample(c).is_blocking());
        assert!((rise - (30. * VOXEL_SIZE - BODY_HEIGHT)).abs() < 1e-6);
        let left = sweep(eye, 0, -5., &mut |c| sample(c).is_blocking());
        assert!(left >= -7. * VOXEL_SIZE + HALF_WIDTH);
    }
}
