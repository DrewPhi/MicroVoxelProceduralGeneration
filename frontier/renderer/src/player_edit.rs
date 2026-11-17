//! Charge-and-release native pickup. Monotonic time and a locked canonical
//! selection make input independent of render frequency and mouse repeats.
use crate::{naadf, Camera};
use frontier_world::{
    edit::{self, EditTransaction, HandSettings, Inventory},
    Cell, Material, World, VOXEL_SIZE,
};
use std::time::Instant;

#[derive(Clone, Default)]
pub struct Overlay {
    pub text: String,
    pub lines: Vec<[[f32; 2]; 2]>,
    pub charge: f32,
}
struct Gesture {
    anchor: Cell,
    started: Instant,
    candidates: Vec<(Cell, Material)>,
    count: usize,
}
pub struct PlayerEdit {
    pub enabled: bool,
    pub settings: HandSettings,
    gesture: Option<Gesture>,
    pub message: String,
}
impl PlayerEdit {
    pub fn new(enabled: bool, settings: HandSettings) -> Self {
        Self {
            enabled,
            settings,
            gesture: None,
            message: String::new(),
        }
    }
    pub fn cancel(&mut self) {
        self.gesture = None;
    }
    pub fn begin(
        &mut self,
        world: &World,
        inventory: &Inventory,
        camera: Camera,
        native: Option<&naadf::Experiment>,
    ) {
        if self.gesture.is_some() {
            return;
        }
        let Some(native) = native else {
            self.message = "Native terrain unavailable".into();
            return;
        };
        let Some(hit) = frontier_world::raycast_solid(
            world,
            camera.position.to_array(),
            camera.direction().as_dvec3().to_array(),
            self.settings.reach_m,
        ) else {
            return;
        };
        let candidates = edit::select(
            world,
            hit.cell,
            self.settings.max_cells().min(inventory.available()),
            self.settings,
            |c| native.owns_native_edit_cell(c) && reachable(c, camera, self.settings.reach_m),
        );
        if candidates.is_empty() {
            self.message = "Inventory full or native terrain unavailable".into();
            return;
        }
        self.gesture = Some(Gesture {
            anchor: hit.cell,
            started: Instant::now(),
            candidates,
            count: 1,
        });
        self.message.clear();
    }
    pub fn update(&mut self, world: &World, camera: Camera, native: Option<&naadf::Experiment>) {
        let Some(g) = &mut self.gesture else {
            return;
        };
        let hit = frontier_world::raycast_solid(
            world,
            camera.position.to_array(),
            camera.direction().as_dvec3().to_array(),
            self.settings.reach_m,
        );
        let valid = hit.is_some_and(|h| h.cell == g.anchor)
            && g.candidates.iter().all(|&(c, m)| {
                reachable(c, camera, self.settings.reach_m)
                    && native.is_some_and(|n| n.owns_native_edit_cell(c))
                    && world.material_without_fluid(c) == m
            });
        if !valid {
            self.gesture = None;
            self.message = "Pickup cancelled: aim, reach or terrain changed".into();
        } else {
            g.count = self
                .settings
                .count_at(g.started.elapsed().as_secs_f64())
                .min(g.candidates.len());
        }
    }
    pub fn release(
        &mut self,
        world: &World,
        inventory: &Inventory,
        camera: Camera,
        native: Option<&naadf::Experiment>,
    ) -> Option<EditTransaction> {
        self.update(world, camera, native);
        let g = self.gesture.take()?;
        Some(EditTransaction::pickup(
            inventory,
            g.candidates[..g.count].to_vec(),
        ))
    }
    pub fn placement(
        &mut self,
        world: &World,
        inventory: &Inventory,
        camera: Camera,
        physical_eye: [f64; 3],
        native: Option<&naadf::Experiment>,
    ) -> Option<EditTransaction> {
        self.cancel();
        let hit = frontier_world::raycast_solid(
            world,
            camera.position.to_array(),
            camera.direction().as_dvec3().to_array(),
            self.settings.reach_m,
        )?;
        if hit.normal == [0; 3] {
            return None;
        }
        let cell = Cell {
            x: hit.cell.x.checked_add(hit.normal[0] as i64)?,
            y: hit.cell.y.checked_add(hit.normal[1] as i64)?,
            z: hit.cell.z.checked_add(hit.normal[2] as i64)?,
        };
        if !native
            .is_some_and(|n| n.owns_native_edit_cell(hit.cell) && n.owns_native_edit_cell(cell))
            || !reachable(cell, camera, self.settings.reach_m)
            || intersects_player(cell, physical_eye)
        {
            self.message = "Placement blocked: reach, player or native ownership".into();
            return None;
        }
        if world.material(cell) == Material::Water {
            self.message = "Placement into water needs supported displacement".into();
            return None;
        }
        Some(EditTransaction::placement(
            inventory,
            cell,
            world.material_without_fluid(cell),
        ))
    }
    pub fn overlay(&self, inventory: &Inventory, camera: Camera, aspect: f32) -> Overlay {
        if !self.enabled {
            return Overlay::default();
        }
        let mut out = Overlay { text: format!("{:?}: {}  | inventory {}/{}  | 1–6 material\nL click/hold pickup · R place · {:.2} L max · {:.2} m reach\n{}",
            inventory.selected,inventory.counts[inventory.selected as usize],inventory.total(),inventory.capacity,
            self.settings.volume_liters,self.settings.reach_m,self.message), ..Default::default() };
        let voxel_liters = VOXEL_SIZE.powi(3) * 1000.;
        if self.settings.volume_liters < voxel_liters {
            out.text
                .push_str(&format!("\nGrid minimum: one voxel ({voxel_liters:.6} L)"));
        }
        if let Some(g) = &self.gesture {
            out.charge = ((g.started.elapsed().as_secs_f64() - self.settings.quick_seconds)
                / (self.settings.full_seconds - self.settings.quick_seconds))
                .clamp(0., 1.) as f32;
            out.text.push_str(&format!(
                "\nPickup: {} {:?} · {:.3} L",
                g.count,
                g.candidates[0].1,
                g.count as f64 * VOXEL_SIZE.powi(3) * 1000.
            ));
            let matrix = camera.matrix(camera.position, aspect);
            for &(cell, _) in &g.candidates[..g.count] {
                let p = [cell.x, cell.y, cell.z];
                let corners: [Option<[f32; 2]>; 8] = std::array::from_fn(|i| {
                    let local = glam::DVec3::new(
                        (p[0] as f64 + ((i & 1) as f64)) * VOXEL_SIZE,
                        (p[1] as f64 + (((i >> 1) & 1) as f64)) * VOXEL_SIZE,
                        (p[2] as f64 + (((i >> 2) & 1) as f64)) * VOXEL_SIZE,
                    ) - camera.position;
                    let clip = matrix * local.as_vec3().extend(1.);
                    (clip.w > 0.025).then(|| [clip.x / clip.w, clip.y / clip.w])
                });
                for i in 0..8 {
                    for axis in [1, 2, 4] {
                        if i & axis == 0 {
                            if let (Some(a), Some(b)) = (corners[i], corners[i | axis]) {
                                out.lines.push([a, b]);
                            }
                        }
                    }
                }
            }
        }
        out
    }
}
pub(crate) fn reachable(cell: Cell, camera: Camera, reach: f64) -> bool {
    let p = [cell.x, cell.y, cell.z];
    let eye = camera.position.to_array();
    (0..3)
        .map(|a| {
            let low = p[a] as f64 * VOXEL_SIZE;
            let high = low + VOXEL_SIZE;
            (eye[a] - eye[a].clamp(low, high)).powi(2)
        })
        .sum::<f64>()
        <= reach * reach
}
pub fn intersects_player(cell: Cell, eye: [f64; 3]) -> bool {
    let low = [
        eye[0] - 0.225,
        eye[1] - crate::walking::EYE_HEIGHT,
        eye[2] - 0.225,
    ];
    let high = [eye[0] + 0.225, low[1] + 1.75, eye[2] + 0.225];
    let p = [cell.x, cell.y, cell.z];
    (0..3).all(|a| p[a] as f64 * VOXEL_SIZE < high[a] && (p[a] as f64 + 1.) * VOXEL_SIZE > low[a])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn placement_rejects_player_body_and_reach_uses_native_negative_coordinates() {
        let mut camera = Camera::fixture();
        camera.position = glam::DVec3::new(-100., 20., -200.);
        let eye = camera.position.to_array();
        let feet = Cell::from_meters([eye[0], eye[1] - 1., eye[2]]);
        assert!(intersects_player(feet, eye));
        assert!(reachable(feet, camera, 2.));
        assert!(!reachable(
            Cell {
                x: feet.x - 64,
                ..feet
            },
            camera,
            2.
        ));
    }
}
