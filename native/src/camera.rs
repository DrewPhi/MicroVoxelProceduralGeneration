//! The only logical camera position is an integer voxel cell. Fractional credits
//! accumulate speed; mouse angles and projection are continuous presentation.
use glam::{Mat4, Vec3};
use microvoxel::{
    generation::{Generator, Terrain},
    grid::CellPos,
    queries::sweep_point,
    World,
};
use std::collections::HashSet;
use winit::keyboard::KeyCode;

pub struct Camera {
    pub grid: microvoxel::GridScale,
    pub cell: CellPos,
    pub yaw: f64,
    pub pitch: f64,
    pub speed: f64,
    pub fov: f64,
    pub reverse: bool,
    pub keys: HashSet<KeyCode>,
    credit: [f64; 3],
}
impl Camera {
    pub fn spawn(world: &World<Terrain>, nonce: u64) -> Self {
        let grid = world.spec().grid;
        if let Some(survey) = world.baseline().survey() {
            let [x, z] = if nonce == 0 {
                survey.spawn
            } else {
                let h = microvoxel::painting::hash(nonce);
                let margin = grid.cells(5.).unwrap();
                let range = world.spec().extent_cells() - 2 * margin;
                [
                    margin + (h % range as u64) as i64,
                    margin + ((h >> 32) % range as u64) as i64,
                ]
            };
            let column = world.surface_column(x, z).expect("validated survey spawn");
            let mut camera = Self::at(CellPos::new(x, column.ground + grid.cells(1.7).unwrap(), z));
            camera.grid = grid;
            camera.yaw = 90f64.to_radians();
            camera.speed = 4.;
            return camera;
        }
        let mut selected = None;
        let world_scale = world.baseline().world_scale() as f64;
        let extent = world.spec().extent_m as f64;
        let margin = (64. / world_scale).min(extent * 0.1);
        let range = (extent - 2. * margin).max(grid.meters());
        // Keep the first launch reproducible for comparisons. R chooses another
        // dry location; no player, organism, or simulation state is introduced.
        for attempt in 0..128u64 {
            let h = microvoxel::painting::hash(nonce.wrapping_add(attempt));
            let (x, z) = if nonce == 0 && attempt == 0 {
                (
                    grid.cells(9316. / world_scale).unwrap(),
                    grid.cells(5037. / world_scale).unwrap(),
                )
            } else {
                (
                    grid.cells(margin + (h % 9872) as f64 / 9872. * range)
                        .unwrap(),
                    grid.cells(margin + ((h >> 24) % 9872) as f64 / 9872. * range)
                        .unwrap(),
                )
            };
            if let Some(c) = world.surface_column(x, z) {
                if c.water <= c.ground {
                    let pos =
                        CellPos::new(x, c.ground + (1.6875 / grid.meters()).round() as i64, z);
                    if camera_clear(world, pos) {
                        selected = Some(pos);
                        break;
                    }
                }
            }
        }
        let mut camera = Self::at(selected.expect("dry terrain spawn"));
        camera.grid = grid;
        // Face an open valley instead of putting the first view against a wall.
        let mut best = f64::INFINITY;
        for i in 0..16 {
            let heading = i as f64 * std::f64::consts::TAU / 16.;
            let sample = |distance: f64| {
                world.surface_column(
                    camera.cell.x + (heading.cos() * distance / grid.meters()) as i64,
                    camera.cell.z + (heading.sin() * distance / grid.meters()) as i64,
                )
            };
            if let (Some(near), Some(far)) = (sample(8.), sample(128.)) {
                let score = far.ground as f64 + 8. * ((near.ground - camera.cell.y).max(0) as f64);
                if score < best {
                    best = score;
                    camera.yaw = heading;
                }
            }
        }
        camera
    }
    pub fn at(cell: CellPos) -> Self {
        Self {
            grid: microvoxel::GridScale::default(),
            cell,
            yaw: -0.95,
            pitch: -0.13,
            speed: 4.0,
            fov: 65f64.to_radians(),
            reverse: false,
            keys: HashSet::new(),
            credit: [0.; 3],
        }
    }
    pub fn eye(&self) -> [f64; 3] {
        self.grid.point(self.cell)
    }
    pub fn direction(&self) -> [f64; 3] {
        let sign = if self.reverse { -1. } else { 1. };
        [
            sign * self.yaw.cos() * self.pitch.cos(),
            sign * self.pitch.sin(),
            sign * self.yaw.sin() * self.pitch.cos(),
        ]
    }
    pub fn rotate(&mut self, dx: f64, dy: f64) {
        self.yaw += dx * 0.002;
        self.pitch = (self.pitch - dy * 0.002).clamp(-1.54, 1.54);
    }
    pub fn clear(&mut self) {
        self.keys.clear();
        self.credit = [0.; 3];
    }
    pub fn update<G: Generator>(&mut self, world: &World<G>, dt: f64) {
        let axis = |a, b| i32::from(self.keys.contains(&a)) - i32::from(self.keys.contains(&b));
        let f = self.direction();
        let forward = axis(KeyCode::KeyW, KeyCode::KeyS) as f64;
        let strafe = axis(KeyCode::KeyD, KeyCode::KeyA) as f64;
        let vertical = axis(KeyCode::Space, KeyCode::KeyC) as f64;
        let mut direction = [
            f[0] * forward - self.yaw.sin() * strafe,
            f[1] * forward + vertical,
            f[2] * forward + self.yaw.cos() * strafe,
        ];
        let length = direction.iter().map(|v| v * v).sum::<f64>().sqrt();
        let boost = if self.keys.contains(&KeyCode::ShiftLeft)
            || self.keys.contains(&KeyCode::ShiftRight)
        {
            4.0
        } else {
            1.0
        };
        if length > 0. {
            for v in &mut direction {
                *v = *v / length * self.speed * boost;
            }
        }
        let mut delta = [0; 3];
        for i in 0..3 {
            let value = (self.credit[i] + direction[i] * dt / world.spec().grid.meters())
                .clamp(-2048., 2048.);
            delta[i] = (value + value.signum() * 1e-9).trunc() as i64;
            self.credit[i] = value - delta[i] as f64;
        }
        if delta != [0; 3] {
            let sweep = sweep_camera(world, self.cell, delta);
            self.cell = sweep.position;
            if sweep.blocked {
                self.credit = [0.; 3];
            }
        }
    }
    pub fn matrix(&self, width: u32, height: u32) -> Mat4 {
        let d = self.direction().map(|v| v as f32);
        Mat4::perspective_rh(
            self.fov as f32,
            width as f32 / height.max(1) as f32,
            (self.grid.meters() * 0.4).min(0.025) as f32,
            24_000.,
        ) * Mat4::look_to_rh(Vec3::ZERO, Vec3::from_array(d), Vec3::Y)
    }
}

fn camera_clear<G: Generator>(world: &World<G>, pos: CellPos) -> bool {
    (0..8).all(|corner| {
        let offset = std::array::from_fn(|axis| if corner & (1 << axis) == 0 { -1 } else { 0 });
        pos.offset(offset)
            .and_then(|p| world.voxel_at(p).known())
            .is_some_and(|v| v.is_air())
    })
}

/// A camera at an integer grid corner needs clearance on both sides of that
/// corner. Sweep the eight cells around its one-voxel-radius box; otherwise a
/// point can stop exactly on a floor and let the near plane see inside terrain.
fn sweep_camera<G: Generator>(
    world: &World<G>,
    start: CellPos,
    delta: [i64; 3],
) -> microvoxel::queries::Sweep {
    let mut earliest = microvoxel::queries::Sweep {
        position: start.offset(delta).unwrap_or(start),
        blocked: false,
        visited: 0,
    };
    let dominant = (0..3).max_by_key(|&i| delta[i].unsigned_abs()).unwrap();
    let origin = [start.x, start.y, start.z];
    let progress = |p: CellPos| ([p.x, p.y, p.z][dominant] - origin[dominant]).unsigned_abs();
    for corner in 0..8 {
        let offset = std::array::from_fn(|axis| if corner & (1 << axis) == 0 { -1 } else { 0 });
        let Some(corner_start) = start.offset(offset) else {
            earliest.position = start;
            earliest.blocked = true;
            return earliest;
        };
        let sweep = sweep_point(world, corner_start, delta);
        let center = sweep.position.offset(offset.map(|v| -v)).unwrap_or(start);
        if progress(center) < progress(earliest.position) {
            earliest.position = center;
        }
        earliest.blocked |= sweep.blocked;
        earliest.visited += sweep.visited;
    }
    earliest
}

#[cfg(test)]
mod tests {
    use super::*;
    use microvoxel::generation::FlatTerrain;
    #[test]
    fn translation_stays_on_grid_and_agrees_across_frame_rates() {
        let world = World::new(FlatTerrain {
            height: 0,
            water: 0,
            seed: 1,
        });
        for fps in [30, 60, 120] {
            let mut camera = Camera {
                grid: Default::default(),
                cell: CellPos::new(10, 20, 30),
                yaw: 0.,
                pitch: 0.,
                speed: 4.,
                fov: 1.,
                reverse: false,
                keys: [KeyCode::KeyW].into(),
                credit: [0.; 3],
            };
            for _ in 0..fps {
                camera.update(&world, 1. / fps as f64);
            }
            assert_eq!(camera.cell, CellPos::new(74, 20, 30));
        }
    }
    #[test]
    fn camera_keeps_near_plane_outside_solid_faces() {
        let world = World::new(FlatTerrain {
            height: 32,
            water: 32,
            seed: 1,
        });
        let start = CellPos::new(20, 40, 20);
        let sweep = sweep_camera(&world, start, [0, -20, 0]);
        assert!(sweep.blocked);
        assert_eq!(sweep.position.y, 33);
        assert!(world
            .voxel_at(sweep.position.offset([-1, -1, -1]).unwrap())
            .known()
            .unwrap()
            .is_air());
    }
}
