//! Small deterministic pose vocabulary. Time selects a whole pose, never a
//! fractional voxel transform. A controller can advance walking by distance.
use std::f64::consts::TAU;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Clip {
    Idle,
    Walk,
    Jump,
    Wave,
}
impl Clip {
    pub const ALL: [Self; 4] = [Self::Idle, Self::Walk, Self::Jump, Self::Wave];
    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Walk => "walk",
            Self::Jump => "jump",
            Self::Wave => "wave",
        }
    }
    pub fn frames(self) -> u8 {
        match self {
            Self::Idle => 8,
            Self::Walk | Self::Jump => 12,
            Self::Wave => 16,
        }
    }
    pub fn fps(self) -> u16 {
        match self {
            Self::Idle => 6,
            _ => 12,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pose {
    pub clip: Clip,
    pub frame: u8,
    pub heading: u8,
    pub arm_forward: bool,
    /// Relative head yaw in 11.25-degree steps; pitch in 7.5-degree steps.
    pub head_yaw: i8,
    pub head_pitch: i8,
}
impl Pose {
    pub fn new(clip: Clip, frame: u8) -> Self {
        Self {
            clip,
            frame: frame % clip.frames(),
            heading: 0,
            arm_forward: false,
            head_yaw: 0,
            head_pitch: 0,
        }
    }
    pub fn facing(mut self, heading: u8) -> Self {
        self.heading = heading % 16;
        self
    }
    pub fn holding_arm(mut self, forward: bool) -> Self {
        self.arm_forward = forward;
        self
    }
    pub fn looking(mut self, yaw: i8, pitch: i8) -> Self {
        self.head_yaw = yaw.clamp(-6, 6);
        self.head_pitch = pitch.clamp(-12, 12);
        self
    }
    pub(super) fn phase(self) -> f64 {
        TAU * f64::from(self.frame % self.clip.frames()) / f64::from(self.clip.frames())
    }
    /// Preview root trajectory only. Future gameplay must use the physics root,
    /// not add this decorative trajectory on top of a physical jump.
    pub fn preview_root_cells(self, cells_per_meter: u16) -> [i32; 3] {
        let height = if self.clip == Clip::Jump {
            0.35 * (self.phase() * 0.5).sin().max(0.)
        } else {
            0.
        };
        [0, (height * f64::from(cells_per_meter)).round() as i32, 0]
    }
}
