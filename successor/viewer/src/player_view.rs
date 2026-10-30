//! Camera presentation of a fixed-grid player; camera motion is also whole cells.
use crate::camera::Camera;
use microvoxel::{generation::Generator, physics::camera_boom, player::Player, World};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewMode {
    #[default]
    First,
    Rear,
    Front,
}
impl ViewMode {
    pub fn next(self) -> Self {
        match self {
            Self::First => Self::Rear,
            Self::Rear => Self::Front,
            Self::Front => Self::First,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::First => "First person",
            Self::Rear => "Rear third person",
            Self::Front => "Front third person",
        }
    }
}
pub fn sync<G: Generator>(
    world: &World<G>,
    player: &Player,
    camera: &mut Camera,
    view: ViewMode,
    width: u32,
    height: u32,
) {
    let scale = world.spec().grid.meters();
    camera.grid = world.spec().grid;
    let anchor = player
        .root
        .offset([0, (1.5 / scale).round() as i64, 0])
        .unwrap_or(player.root);
    camera.reverse = false;
    camera.cell = if view == ViewMode::First {
        anchor
    } else {
        let sign = if view == ViewMode::Rear { -1. } else { 1. };
        let delta = camera
            .direction()
            .map(|v| (v * sign * 3.5 / scale).round() as i64);
        // Protect the near-plane corners, including wide FOV/aspect ratios.
        let extent =
            0.025 * (camera.fov * 0.5).tan() * (width as f64 / height.max(1) as f64).max(1.);
        let radius = ((extent / scale).ceil() as i32 + 1).clamp(1, 32);
        camera_boom(world, anchor, delta, radius)
    };
    camera.reverse = view == ViewMode::Front;
}

#[cfg(test)]
mod tests {
    use super::*;
    use microvoxel::{
        generation::FlatTerrain,
        materials::{Appearance, MaterialId, Voxel},
        CellPos, Edit,
    };
    #[test]
    fn f5_views_keep_player_root_and_retract_before_walls() {
        let mut world = World::new(FlatTerrain {
            height: 8,
            water: 8,
            seed: 42,
        });
        let p = Player::new(42, CellPos::new(0, 8, 0)).unwrap();
        let mut camera = Camera::at(CellPos::default());
        camera.yaw = 0.;
        camera.pitch = 0.;
        sync(&world, &p, &mut camera, ViewMode::First, 1280, 720);
        assert_eq!(camera.cell, CellPos::new(0, 32, 0));
        assert_eq!(ViewMode::First.next().next().next(), ViewMode::First);
        let mut edits = Vec::new();
        for y in 8..64 {
            for z in -8..8 {
                edits.push(Edit::set(
                    CellPos::new(-20, y, z),
                    Voxel::new(MaterialId::ROCK, Appearance(0)),
                ));
            }
        }
        world.apply(&edits).unwrap();
        sync(&world, &p, &mut camera, ViewMode::Rear, 1280, 720);
        assert!(camera.cell.x > -20 && camera.cell.x < 0);
        assert!(!camera.reverse);
        sync(&world, &p, &mut camera, ViewMode::Front, 1280, 720);
        assert!(camera.cell.x > 0);
        assert!(camera.direction()[0] < 0.);
        sync(&world, &p, &mut camera, ViewMode::First, 1280, 720);
        assert!(!camera.reverse);
        assert_eq!(p.root, CellPos::new(0, 8, 0));
    }
}
