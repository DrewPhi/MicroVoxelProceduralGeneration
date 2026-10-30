//! Procedural, grid-aligned character assets consumed by previews and the player.
mod animation;
mod recipe;
mod volume;
pub use animation::{Clip, Pose};
pub use recipe::{CharacterPalette, CharacterRecipe};
use std::{collections::VecDeque, sync::Arc};
pub use volume::VoxelFrame;
/// Resolution/recipe/palette are fixed for a bank. A small FIFO keeps memory
/// bounded even if future code requests many facing directions and poses.
pub struct PoseCache {
    recipe: CharacterRecipe,
    palette: CharacterPalette,
    resolution: u16,
    frames: VecDeque<(Pose, Arc<VoxelFrame>)>,
    bytes: usize,
    limit: usize,
}
impl PoseCache {
    pub fn new(
        recipe: CharacterRecipe,
        palette: CharacterPalette,
        resolution: u16,
        limit: usize,
    ) -> Self {
        Self {
            recipe,
            palette,
            resolution,
            frames: VecDeque::new(),
            bytes: 0,
            limit,
        }
    }
    pub fn frame(&mut self, pose: Pose) -> Result<Arc<VoxelFrame>, String> {
        let pose = Pose::new(pose.clip, pose.frame)
            .facing(pose.heading)
            .holding_arm(pose.arm_forward)
            .looking(pose.head_yaw, pose.head_pitch);
        if let Some((_, frame)) = self.frames.iter().find(|(p, _)| *p == pose) {
            return Ok(Arc::clone(frame));
        }
        let frame = Arc::new(self.recipe.generate(self.resolution, pose, &self.palette)?);
        let size = frame.memory_bytes();
        if size > self.limit {
            return Err("pose exceeds cache memory budget".into());
        }
        while self.bytes + size > self.limit || self.frames.len() >= 16 {
            if let Some((_, old)) = self.frames.pop_front() {
                self.bytes -= old.memory_bytes();
            }
        }
        self.bytes += size;
        self.frames.push_back((pose, Arc::clone(&frame)));
        Ok(frame)
    }
    /// Retained cache memory; external Arc owners can keep evicted frames alive.
    pub fn memory_bytes(&self) -> usize {
        self.bytes
    }
}
