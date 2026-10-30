//! Fixed-tick grid character controller. Character occupancy remains distinct
//! from terrain storage; moving never creates world edits or fractional cells.
use crate::{
    character::{CharacterRecipe, Clip, Pose, PoseCache, VoxelFrame},
    generation::Generator,
    materials::MaterialRegistry,
    physics::VoxelShape,
    CellPos, World,
};
use std::{collections::VecDeque, sync::Arc};
pub const TICK_SECONDS: f64 = 1. / 60.;
#[derive(Clone, Copy, Debug, Default)]
pub struct PlayerInput {
    pub forward: f64,
    pub strafe: f64,
    pub yaw: f64,
    pub pitch: f64,
    pub sprint: bool,
    pub jump: bool,
    pub arm_forward: bool,
}
struct BodyFrame {
    frame: Arc<VoxelFrame>,
    shape: VoxelShape,
}
pub struct Player {
    scale: crate::GridScale,
    pub root: CellPos,
    pub grounded: bool,
    pub vertical_velocity: f64,
    pub pose: Pose,
    pub ticks: u64,
    pub step_height_cells: i64,
    pub materials: MaterialRegistry,
    bank: PoseCache,
    bodies: VecDeque<(Pose, Arc<BodyFrame>)>,
    body: Arc<BodyFrame>,
    credit: [f64; 3],
    accumulator: f64,
    pending_jump: bool,
    jump_down: bool,
    travel: f64,
    last_movement: u64,
    last_look: i16,
    steady_look_ticks: u16,
}
impl Player {
    pub fn new(seed: u64, root: CellPos) -> Result<Self, String> {
        Self::with_scale(seed, root, crate::GridScale::default())
    }
    pub fn with_scale(seed: u64, root: CellPos, scale: crate::GridScale) -> Result<Self, String> {
        let resolution = (1. / scale.meters()).round() as u16;
        if resolution > 128 || (resolution as f64 * scale.meters() - 1.).abs() > 1e-8 {
            return Err("interactive character supports integer 4..128 cells/m; use bounded terrain diagnostics for finer grids".into());
        }
        let recipe = CharacterRecipe { seed };
        let mut materials = MaterialRegistry::default();
        let palette = recipe.register_palette(&mut materials)?;
        let mut bank = PoseCache::new(recipe, palette, resolution, 32 * 1024 * 1024);
        let pose = Pose::new(Clip::Idle, 0);
        let frame = bank.frame(pose)?;
        let body = Arc::new(BodyFrame {
            shape: VoxelShape::from_frame(&frame),
            frame,
        });
        Ok(Self {
            scale,
            root,
            grounded: false,
            vertical_velocity: 0.,
            pose,
            ticks: 0,
            step_height_cells: (0.25 / scale.meters()).round() as i64,
            materials,
            bank,
            bodies: VecDeque::from([(pose, Arc::clone(&body))]),
            body,
            credit: [0.; 3],
            accumulator: 0.,
            pending_jump: false,
            jump_down: false,
            travel: 0.,
            last_movement: 0,
            last_look: 0,
            steady_look_ticks: 0,
        })
    }
    pub fn frame(&self) -> &VoxelFrame {
        &self.body.frame
    }
    pub fn clear<G: Generator>(&self, world: &World<G>) -> bool {
        self.body.shape.clear(world, self.root)
    }
    pub fn request_jump(&mut self) {
        self.pending_jump = true;
    }
    pub fn reset_input(&mut self) {
        self.pending_jump = false;
        self.jump_down = false;
        self.accumulator = 0.;
        self.credit[0] = 0.;
        self.credit[2] = 0.;
    }
    pub fn place_above<G: Generator>(&mut self, world: &World<G>, x: i64, z: i64) -> bool {
        let mut top = 0;
        // Spawn above the whole standing footprint, including head/arms, then
        // let gravity settle the actual voxel shape. Column queries include edits.
        for dz in self.body.shape.min[2]..self.body.shape.max[2] {
            for dx in self.body.shape.min[0]..self.body.shape.max[0] {
                let Some(c) = world.surface_column(x + i64::from(dx), z + i64::from(dz)) else {
                    return false;
                };
                top = top.max(c.ground.max(c.water));
            }
        }
        let candidate = CellPos::new(x, top + 1, z);
        if !self.body.shape.clear(world, candidate) {
            return false;
        }
        self.root = candidate;
        self.vertical_velocity = 0.;
        self.credit = [0.; 3];
        self.grounded = false;
        self.accumulator = 0.;
        self.pending_jump = false;
        true
    }
    pub fn advance<G: Generator>(
        &mut self,
        world: &World<G>,
        dt: f64,
        input: PlayerInput,
    ) -> Result<(), String> {
        if !dt.is_finite()
            || dt < 0.
            || !input.yaw.is_finite()
            || !input.pitch.is_finite()
            || !input.forward.is_finite()
            || !input.strafe.is_finite()
        {
            return Ok(());
        }
        if input.jump && !self.jump_down {
            self.pending_jump = true;
        }
        self.jump_down = input.jump;
        // Bound catch-up after pauses. Ordinary frame rates run identical ticks.
        self.accumulator += dt.min(0.25);
        while self.accumulator + 1e-10 >= TICK_SECONDS {
            self.accumulator = (self.accumulator - TICK_SECONDS).max(0.);
            self.tick(world, input)?;
        }
        Ok(())
    }
    fn body_for(&mut self, pose: Pose) -> Result<Arc<BodyFrame>, String> {
        if let Some((_, b)) = self.bodies.iter().find(|(p, _)| *p == pose) {
            return Ok(Arc::clone(b));
        }
        let frame = self.bank.frame(pose)?;
        let body = Arc::new(BodyFrame {
            shape: VoxelShape::from_frame(&frame),
            frame,
        });
        let mut retained: usize = self
            .bodies
            .iter()
            .map(|(_, b)| b.frame.memory_bytes() + b.shape.memory_bytes())
            .sum();
        while (self.bodies.len() >= 16
            || retained + body.frame.memory_bytes() + body.shape.memory_bytes() > 32 * 1024 * 1024)
            && !self.bodies.is_empty()
        {
            if let Some((_, old)) = self.bodies.pop_front() {
                retained =
                    retained.saturating_sub(old.frame.memory_bytes() + old.shape.memory_bytes());
            }
        }
        self.bodies.push_back((pose, Arc::clone(&body)));
        Ok(body)
    }
    fn horizontal<G: Generator>(&mut self, world: &World<G>, axis: usize, delta: i64) {
        for _ in 0..delta.unsigned_abs() {
            if let Some(next) = self
                .body
                .shape
                .move_one(world, self.root, axis, delta.signum())
            {
                self.root = next;
                continue;
            }
            if self.grounded {
                // Raise only as much as needed, checking every upward cell and
                // the complete path over the obstacle, including head clearance.
                let mut raised = self.root;
                for height in 1..=self
                    .step_height_cells
                    .clamp(0, (0.25 / self.scale.meters()).ceil() as i64)
                {
                    let Some(up) = self.body.shape.move_one(world, raised, 1, 1) else {
                        break;
                    };
                    raised = up;
                    if let Some(over) =
                        self.body
                            .shape
                            .move_one(world, raised, axis, delta.signum())
                    {
                        self.root = self.body.shape.sweep_axis(world, over, 1, -height);
                        break;
                    }
                }
            }
            break;
        }
    }
    fn try_pose<G: Generator>(
        &mut self,
        world: &World<G>,
        candidate: Pose,
    ) -> Result<bool, String> {
        if candidate == self.pose {
            return Ok(true);
        }
        let next = self.body_for(candidate)?;
        if self
            .body
            .shape
            .transition_clear(&next.shape, world, self.root)
        {
            self.pose = candidate;
            self.body = next;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn tick<G: Generator>(&mut self, world: &World<G>, input: PlayerInput) -> Result<(), String> {
        self.ticks += 1;
        self.grounded = self.body.shape.move_one(world, self.root, 1, -1).is_none();
        if self.pending_jump && self.grounded {
            self.vertical_velocity = 6.;
            self.grounded = false;
            self.credit[1] = 0.;
        }
        self.pending_jump = false;
        if !self.grounded || self.vertical_velocity > 0. {
            self.vertical_velocity = (self.vertical_velocity - 20. * TICK_SECONDS).max(-40.);
        } else {
            self.vertical_velocity = 0.;
            self.credit[1] = 0.;
        }
        let length = input.forward.hypot(input.strafe).max(1.);
        let speed = if input.sprint { 5.5 } else { 3. };
        let velocity = [
            (input.yaw.cos() * input.forward - input.yaw.sin() * input.strafe) / length * speed,
            self.vertical_velocity,
            (input.yaw.sin() * input.forward + input.yaw.cos() * input.strafe) / length * speed,
        ];
        let mut delta = [0; 3];
        for a in 0..3 {
            let value = self.credit[a] + velocity[a] * TICK_SECONDS / self.scale.meters();
            delta[a] = (value + value.signum() * 1e-9).trunc() as i64;
            self.credit[a] = value - delta[a] as f64;
        }
        let before = self.root;
        self.horizontal(world, 0, delta[0]);
        self.horizontal(world, 2, delta[2]);
        let target_y = self.root.y + delta[1];
        self.root = self.body.shape.sweep_axis(world, self.root, 1, delta[1]);
        if self.root.y != target_y {
            self.vertical_velocity = 0.;
            self.credit[1] = 0.;
        }
        self.grounded = self.body.shape.move_one(world, self.root, 1, -1).is_none()
            && self.vertical_velocity <= 0.;
        let moved = ((self.root.x - before.x) as f64).hypot((self.root.z - before.z) as f64)
            * self.scale.meters();
        self.travel += moved;
        if moved > 0. {
            self.last_movement = self.ticks;
        }
        let clip = if !self.grounded {
            Clip::Jump
        } else if (input.forward != 0. || input.strafe != 0.) && self.ticks - self.last_movement < 3
        {
            Clip::Walk
        } else {
            Clip::Idle
        };
        let index = match clip {
            Clip::Walk => (self.travel / 1.2 * 12.) as u64 % 12,
            Clip::Jump => {
                if self.vertical_velocity > 0. {
                    3
                } else {
                    9
                }
            }
            _ => (self.ticks / 10) % 8,
        } as u8;
        let look = heading_steps(input.yaw, 32);
        if look == self.last_look {
            self.steady_look_ticks = self.steady_look_ticks.saturating_add(1);
        } else {
            self.steady_look_ticks = 0;
            self.last_look = look;
        }
        let look_body = heading_steps(input.yaw, 16);
        let relative = wrapped_steps(look - i16::from(self.pose.heading) * 2, 32);
        let moving =
            (input.forward != 0. || input.strafe != 0.) && self.ticks - self.last_movement < 3;
        let target = if moving {
            // Face the travel direction while walking, but keep backward walking
            // facing forward and limit shoulder twist during strafing.
            // Use the travel intent once movement is accepted. Individual
            // quantized X/Z steps would otherwise make diagonal shoulders wobble.
            let movement_yaw = velocity[2].atan2(velocity[0]);
            let mut offset = wrapped_steps(heading_steps(movement_yaw, 16) - look_body, 16);
            if offset > 4 {
                offset -= 8;
            } else if offset < -4 {
                offset += 8;
            }
            (look_body + offset.clamp(-2, 2)).rem_euclid(16)
        } else {
            look_body
        };
        let follow =
            moving || relative.abs() > 4 || (self.steady_look_ticks >= 30 && self.ticks % 6 == 0);
        let difference = wrapped_steps(target - i16::from(self.pose.heading), 16);
        let heading = (i16::from(self.pose.heading) + if follow { difference.signum() } else { 0 })
            .rem_euclid(16) as u8;
        let pitch = (input.pitch.to_degrees() / 7.5).round().clamp(-12., 12.) as i8;
        let head_yaw = wrapped_steps(look - i16::from(heading) * 2, 32).clamp(-6, 6) as i8;
        let candidate = Pose::new(clip, index)
            .facing(heading)
            .holding_arm(input.arm_forward)
            .looking(head_yaw, pitch);
        if !self.try_pose(world, candidate)? {
            // A blocked torso/foot change need not freeze a clear head movement.
            let head_yaw =
                wrapped_steps(look - i16::from(self.pose.heading) * 2, 32).clamp(-6, 6) as i8;
            self.try_pose(
                world,
                self.pose
                    .looking(head_yaw, pitch)
                    .holding_arm(input.arm_forward),
            )?;
        }
        Ok(())
    }
}

fn wrapped_steps(value: i16, steps: i16) -> i16 {
    (value + steps / 2).rem_euclid(steps) - steps / 2
}
fn heading_steps(yaw: f64, steps: i16) -> i16 {
    (((-yaw - std::f64::consts::FRAC_PI_2) / std::f64::consts::TAU * f64::from(steps)).round()
        as i64)
        .rem_euclid(i64::from(steps)) as i16
}
