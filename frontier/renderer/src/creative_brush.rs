//! Face-anchored, native-cell stamps for the creative terrain editor.
use crate::{live_water::LiveWater, naadf::Experiment, player_edit, Camera};
use frontier_world::{Cell, Material, RayHit, TerrainUndo, World};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size([i64; 3]);
impl FromStr for Size {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = "--creative-brush requires three positive X,Y,Z dimensions with at most 65536 native cells";
        let mut fields = s.split(',');
        let mut size = [0; 3];
        let mut cells = 1u64;
        for axis in &mut size {
            *axis = fields
                .next()
                .ok_or(invalid)?
                .trim()
                .parse::<i64>()
                .map_err(|_| invalid)?;
            if *axis <= 0 {
                return Err(invalid);
            }
            cells = cells.checked_mul(*axis as u64).ok_or(invalid)?;
        }
        if fields.next().is_some() || cells > 65536 {
            return Err(invalid);
        }
        Ok(Self(size))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bounds {
    min: [i64; 3],
    end: [i64; 3],
}
impl Size {
    fn bounds(self, hit: RayHit, place: bool) -> Option<Bounds> {
        let cell = [hit.cell.x, hit.cell.y, hit.cell.z];
        if hit.normal == [0; 3] {
            return None;
        }
        let mut min = cell;
        let mut end = cell;
        for axis in 0..3 {
            let n = self.0[axis];
            let offset = match (hit.normal[axis], place) {
                (0, _) => -(n / 2),
                (1, true) => 1,
                (-1, true) => -n,
                (1, false) => 1 - n,
                (-1, false) => 0,
                _ => return None,
            };
            min[axis] = cell[axis].checked_add(offset)?;
            end[axis] = min[axis].checked_add(n)?;
        }
        Some(Bounds { min, end })
    }
}
struct LastStamp {
    undo: TerrainUndo,
    bounds: Bounds,
    dug: bool,
}
pub struct Brush {
    size: Size,
    last: Option<LastStamp>,
    target: Option<(u64, Camera, Option<RayHit>)>,
    message: String,
}
impl Brush {
    pub fn new(size: Size) -> Self {
        Self {
            size,
            last: None,
            target: None,
            message: String::new(),
        }
    }
    fn hit(&mut self, world: &World, camera: Camera) -> Option<RayHit> {
        if let Some((rev, pose, hit)) = self.target {
            if rev == world.revision() && pose == camera {
                return hit;
            }
        }
        let hit = frontier_world::raycast_solid(
            world,
            camera.position.to_array(),
            camera.direction().as_dvec3().to_array(),
            8.,
        );
        self.target = Some((world.revision(), camera, hit));
        hit
    }
    fn target_bounds(
        &mut self,
        world: &World,
        camera: Camera,
        native: Option<&Experiment>,
        place: bool,
    ) -> Result<Bounds, &'static str> {
        let hit = self
            .hit(world, camera)
            .ok_or("Aim at a solid face within 8 m")?;
        let bounds = self
            .size
            .bounds(hit, place)
            .ok_or("Aim at an exposed face")?;
        if !native.is_some_and(|n| {
            n.owns_native_edit_cell(hit.cell) && n.owns_native_edit_bounds(bounds.min, bounds.end)
        }) {
            return Err("The brush extends beyond resident native terrain");
        }
        Ok(bounds)
    }
    pub fn overlay(
        &mut self,
        world: &World,
        camera: Camera,
        native: Option<&Experiment>,
        material: Material,
        aspect: f32,
        grabbed: bool,
    ) -> player_edit::Overlay {
        let [x, y, z] = self.size.0;
        let mut out = player_edit::Overlay {text:format!("Creative brush  {x} × {y} × {z} cells  ·  {material:?}\nL dig  ·  R place in outline  ·  1–6 material  ·  Z undo\n{}",self.message),..Default::default()};
        if grabbed {
            if let Ok(bounds) = self.target_bounds(world, camera, native, true) {
                out.outline(
                    bounds.min,
                    bounds.end,
                    camera.matrix(camera.position, aspect),
                    camera,
                );
            }
        }
        out
    }
    pub fn stamp(
        &mut self,
        world: &mut World,
        live: &mut LiveWater,
        camera: Camera,
        eye: [f64; 3],
        native: Option<&Experiment>,
        place: bool,
    ) {
        let result = self
            .target_bounds(world, camera, native, place)
            .and_then(|bounds| {
                let values = Self::collect(world, live, bounds, place, eye)?;
                let undo = world
                    .set_batch_reversible(values)
                    .ok_or("No cells changed")?;
                live.terrain_changed(world, undo.cells());
                let count = undo.cells().len();
                self.last = Some(LastStamp {
                    undo,
                    bounds,
                    dug: !place,
                });
                Ok(count)
            });
        self.message = match result {
            Ok(count) => format!(
                "{} {count} native voxels",
                if place { "Placed" } else { "Removed" }
            ),
            Err(reason) => reason.into(),
        };
    }
    fn collect(
        world: &World,
        live: &LiveWater,
        bounds: Bounds,
        place: bool,
        eye: [f64; 3],
    ) -> Result<Vec<(Cell, Material)>, &'static str> {
        let material = if place {
            live.inventory.selected
        } else {
            Material::Air
        };
        let mut values = Vec::new();
        for x in bounds.min[0]..bounds.end[0] {
            for z in bounds.min[2]..bounds.end[2] {
                let column = world.column(x, z);
                for y in bounds.min[1]..bounds.end[1] {
                    let c = Cell { x, y, z };
                    let old = column.canonical_material(y);
                    if old == Material::Water || live.fluid.mass(c).is_some_and(|m| m > 0) {
                        return Err("The brush intersects water");
                    }
                    if place != (old == Material::Air) {
                        continue;
                    }
                    if place && player_edit::intersects_player(c, eye) {
                        return Err("The brush intersects the player");
                    }
                    values.push((c, material));
                }
            }
        }
        Ok(values)
    }
    pub fn undo(
        &mut self,
        world: &mut World,
        live: &mut LiveWater,
        eye: [f64; 3],
        native: Option<&Experiment>,
    ) {
        let Some(last) = &self.last else {
            self.message = "No stamp to undo".into();
            return;
        };
        if !native.is_some_and(|n| n.owns_native_edit_bounds(last.bounds.min, last.bounds.end)) {
            self.message = "Return to the stamp to undo".into();
            return;
        }
        if last.dug
            && last.undo.cells().iter().any(|&c| {
                player_edit::intersects_player(c, eye)
                    || world.material(c) == Material::Water
                    || live.fluid.mass(c).is_some_and(|m| m > 0)
            })
        {
            self.message = "The restored terrain would intersect the player or water".into();
            return;
        }
        if world.undo_terrain_batch(&last.undo) {
            live.terrain_changed(world, last.undo.cells());
            self.message = format!("Restored {} native voxels", last.undo.cells().len());
        } else {
            self.message = "Terrain changed after this stamp".into();
        }
        self.last = None;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dimensions_and_signed_face_anchors() {
        let brush: Size = "4,2,3".parse().unwrap();
        for invalid in [
            "0,1,1",
            "-1,1,1",
            "7,1",
            "2,3,4,1",
            "256,2,256",
            "9223372036854775807,3,1",
        ] {
            assert!(invalid.parse::<Size>().is_err());
        }
        for axis in 0..3 {
            for sign in [-1, 1] {
                let mut normal = [0; 3];
                normal[axis] = sign;
                let hit = RayHit {
                    cell: Cell {
                        x: -12,
                        y: 7,
                        z: -4,
                    },
                    normal,
                    distance: 1.,
                    material: Material::Stone,
                };
                let place = brush.bounds(hit, true).unwrap();
                let dig = brush.bounds(hit, false).unwrap();
                let cell = [hit.cell.x, hit.cell.y, hit.cell.z];
                for a in 0..3 {
                    assert_eq!(place.end[a] - place.min[a], brush.0[a]);
                    assert!(dig.min[a] <= cell[a] && cell[a] < dig.end[a]);
                    if a != axis {
                        assert_eq!(dig.min[a], place.min[a]);
                    }
                }
                if sign > 0 {
                    assert_eq!(dig.end[axis], place.min[axis]);
                } else {
                    assert_eq!(place.end[axis], dig.min[axis]);
                }
            }
        }
    }
}
