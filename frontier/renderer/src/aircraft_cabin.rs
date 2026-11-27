//! The cabin, controls and glazing are authored on the airframe's 6.25 cm grid.
use super::{voxels::Voxels, Part, ACCENT, BLACK, METAL, PINSTRIPE, WHITE};
use glam::{IVec3, Vec3};

pub(super) const EDGE: f32 = 0.0625;
pub(super) const DOOR_PIVOT: Vec3 = Vec3::new(0.65625, 0., 0.75);
const NAVY: [f32; 3] = [0.042, 0.078, 0.106];
const TRIM: [f32; 3] = [0.16, 0.18, 0.18];
const DASH: [f32; 3] = [0.074, 0.082, 0.087];
const BEZEL: [f32; 3] = [0.029, 0.035, 0.04];
const STITCH: [f32; 3] = [0.53, 0.44, 0.33];
const SEAT: [f32; 3] = [0.29, 0.21, 0.15];
const VINYL: [f32; 3] = [0.67, 0.49, 0.32];
const LINER: [f32; 3] = [0.68, 0.49, 0.32];

pub(super) struct Cabin {
    pub solids: [Voxels; Part::COUNT],
    pub panes: Vec<(Part, [Vec3; 4])>,
    pub displays: Vec<(u32, [Vec3; 4])>,
}

pub(super) fn block(cells: &mut Voxels, lo: [i32; 3], hi: [i32; 3], color: [f32; 3]) {
    for z in lo[2]..hi[2] {
        for y in lo[1]..hi[1] {
            for x in lo[0]..hi[0] {
                cells.insert(IVec3::new(x, y, z), color);
            }
        }
    }
}

fn side_box(vertices: &mut Voxels, side: i32, lo: [i32; 3], hi: [i32; 3], color: [f32; 3]) {
    let mut a = lo;
    let mut b = hi;
    if side < 0 {
        a[0] = -hi[0];
        b[0] = -lo[0];
    }
    block(vertices, a, b, color);
}

fn side_pane(side: i32, y: [i32; 2], z: [i32; 2]) -> [Vec3; 4] {
    let x = 10.5 * EDGE * side as f32;
    let [y0, y1] = y.map(|a| a as f32 * EDGE);
    let [z0, z1] = z.map(|a| a as f32 * EDGE);
    [
        Vec3::new(x, y1, z0),
        Vec3::new(x, y1, z1),
        Vec3::new(x, y0, z1),
        Vec3::new(x, y0, z0),
    ]
}

fn panel(x: [i32; 2], y: [i32; 2], z: f32) -> [Vec3; 4] {
    let [x0, x1] = x.map(|a| a as f32 * EDGE);
    let [y0, y1] = y.map(|a| a as f32 * EDGE);
    [
        Vec3::new(x1, y1, z),
        Vec3::new(x0, y1, z),
        Vec3::new(x0, y0, z),
        Vec3::new(x1, y0, z),
    ]
}

pub(super) fn build() -> Cabin {
    let mut cabin = Cabin {
        solids: std::array::from_fn(|_| Voxels::new()),
        panes: Vec::new(),
        displays: Vec::new(),
    };
    let body = &mut cabin.solids[Part::Body as usize];
    // Bevel the belly and crown by one cell. Tuck the aft roof sides in
    // toward the narrower rear glazing before the tailcone begins.
    block(body, [-10, -9, -34], [10, -8, 17], ACCENT);
    block(body, [-11, -8, -32], [11, -7, 17], ACCENT);
    block(body, [-10, -8, -34], [10, -7, -32], ACCENT);
    block(body, [-11, 12, -26], [11, 13, 12], WHITE);
    block(body, [-10, 12, -29], [10, 13, -26], WHITE);
    block(body, [-9, 13, -27], [9, 14, 12], WHITE);
    block(body, [-10, 11, -28], [10, 12, 12], LINER);
    block(body, [-11, 12, 11], [11, 13, 13], NAVY);
    block(body, [-11, 1, 16], [11, 3, 17], NAVY);
    // Narrow pillars and an uninterrupted, raked windscreen.
    for row in 3..12 {
        let z = 16 - (row - 3) / 2;
        for x in [-11, 10] {
            block(body, [x, row, z], [x + 1, row + 1, z + 1], NAVY);
        }
    }
    let mut glass = panel([-10, 10], [3, 12], 16.5 * EDGE);
    for corner in &mut glass[..2] {
        corner.z = 12.5 * EDGE;
    }
    cabin.panes.push((Part::Body, glass));
    let mut aft_glass = panel([-9, 9], [5, 12], -28.5 * EDGE);
    for corner in &mut aft_glass[2..] {
        corner.z = -33.5 * EDGE;
    }
    cabin.panes.push((Part::Body, aft_glass));
    block(body, [-10, -7, -34], [10, 5, -33], WHITE);
    block(body, [-10, -7, -34], [10, -4, -33], ACCENT);
    block(body, [-10, -4, -34], [10, -3, -33], PINSTRIPE);
    for y in 5..12 {
        let z = -34 + (y - 4) * 5 / 7;
        for x in [-10, 9] {
            block(body, [x, y, z], [x + 1, y + 1, z + 2], NAVY);
        }
    }

    for side in [-1, 1] {
        side_box(body, side, [9, -7, -33], [10, 5, -30], WHITE);
        side_box(body, side, [10, -7, -31], [11, 4, -12], WHITE);
        side_box(body, side, [9, 11, -29], [10, 12, -26], WHITE);
        side_box(body, side, [10, 11, -27], [11, 12, 13], WHITE);
        side_box(body, side, [10, 4, -13], [11, 11, -12], NAVY);
        side_box(body, side, [10, -7, 12], [11, 3, 17], WHITE);
        let mut quarter = side_pane(side, [3, 12], [12, 17]);
        quarter[1].z = 12.5 * EDGE;
        cabin.panes.push((Part::Body, quarter));
        cabin
            .panes
            .push((Part::Body, side_pane(side, [4, 11], [-26, -13])));
        let mut aft_quarter = side_pane(side, [4, 11], [-33, -26]);
        aft_quarter[0].z = -29.2 * EDGE;
        for i in [0, 3] {
            aft_quarter[i].x += (aft_quarter[i].z + 26. * EDGE) * (0.2 * side as f32);
        }
        cabin.panes.push((Part::Body, aft_quarter));
        side_box(body, side, [9, -7, -33], [10, -4, -30], ACCENT);
        side_box(body, side, [9, -4, -33], [10, -3, -30], PINSTRIPE);
        for (a, b) in [(-31, -12), (12, 17)] {
            side_box(body, side, [10, -7, a], [11, -4, b], ACCENT);
            side_box(body, side, [10, -4, a], [11, -3, b], PINSTRIPE);
        }
        side_box(body, side, [10, -7, -12], [11, -6, 12], NAVY);
        side_box(body, side, [11, -10, -2], [13, -9, 3], METAL);
        side_box(body, side, [11, -9, -2], [13, -8, 3], BLACK);
        for y in [-2, 3] {
            side_box(body, side, [11, y, 11], [12, y + 1, 12], METAL);
        }
    }

    let interior = &mut cabin.solids[Part::Cabin as usize];
    block(interior, [-10, -7, -33], [10, -6, 15], TRIM);
    block(interior, [-2, 10, -15], [2, 11, -7], TRIM);
    block(interior, [-1, 10, -13], [1, 11, -10], [0.83, 0.76, 0.57]);
    for side in [-1, 1] {
        side_box(interior, side, [7, 9, 2], [9, 10, 8], LINER);
        side_box(interior, side, [7, 9, 7], [8, 10, 8], TRIM);
    }
    // Cargo shelf and rear quarter trim. The aft bench has two separate belts.
    block(interior, [-9, -6, -32], [9, -5, -26], TRIM);
    block(interior, [-9, -5, -29], [9, -1, -24], SEAT);
    block(interior, [-8, -2, -28], [8, 0, -23], VINYL);
    block(interior, [-9, -1, -31], [9, 7, -27], SEAT);
    block(interior, [-8, 1, -27], [8, 6, -26], VINYL);
    for side in [-1, 1] {
        side_box(interior, side, [2, 5, -31], [7, 8, -27], SEAT);
        side_box(interior, side, [4, 1, -26], [5, 6, -25], NAVY);
        side_box(interior, side, [4, 0, -25], [5, 1, -23], NAVY);
        side_box(interior, side, [2, 0, -24], [3, 1, -23], METAL);
        side_box(interior, side, [9, -5, -30], [10, 3, -13], LINER);

        // Shaped front seats on metal runners. The cards and belts are voxels.
        side_box(interior, side, [2, -6, -13], [8, -5, -2], METAL);
        side_box(interior, side, [1, -5, -13], [9, -3, -4], SEAT);
        side_box(interior, side, [2, -3, -12], [8, -2, -4], VINYL);
        side_box(interior, side, [1, -3, -15], [9, 5, -11], SEAT);
        side_box(interior, side, [2, -1, -12], [8, 5, -10], VINYL);
        side_box(interior, side, [3, 5, -15], [7, 8, -12], VINYL);
        side_box(interior, side, [2, -1, -16], [8, 2, -15], TRIM);
        side_box(interior, side, [8, -3, -12], [9, -2, -7], NAVY);
        side_box(interior, side, [8, -2, -8], [9, -1, -7], METAL);
        side_box(interior, side, [2, -1, -10], [3, 4, -9], STITCH);
        side_box(interior, side, [7, -1, -10], [8, 4, -9], STITCH);
        side_box(interior, side, [9, -5, 7], [10, 1, 15], LINER);
        side_box(interior, side, [9, 0, 7], [10, 1, 13], SEAT);
    }
    // Two recessed displays share one glare shield. Backup instruments and
    // their switchgear occupy the lower face, between the two control columns.
    block(interior, [-10, -6, 14], [10, 3, 16], DASH);
    block(interior, [-11, 1, 15], [11, 3, 17], BEZEL);
    block(interior, [-9, 3, 14], [9, 4, 17], BEZEL);
    block(interior, [-10, 2, 14], [10, 3, 16], BEZEL);
    let face_depth = 14. * EDGE - 0.002;
    for (kind, x) in [(7, [1, 9]), (8, [-9, -1])] {
        block(interior, [x[0] - 1, -2, 14], [x[1] + 1, 3, 15], BEZEL);
        cabin.displays.push((kind, panel(x, [-2, 3], face_depth)));
    }
    // Compact mechanical backup instruments.
    for (kind, x) in [(1, [2, 4]), (2, [-1, 1]), (3, [-4, -2])] {
        let face = panel(x, [-5, -3], face_depth).map(|p| p + Vec3::Y * (0.5 * EDGE));
        cabin.displays.push((kind, face));
    }

    for side in [-1, 1] {
        let part = if side > 0 {
            Part::LeftDoor
        } else {
            Part::RightDoor
        };
        let door = &mut cabin.solids[part as usize];
        side_box(door, side, [10, -3, -12], [11, 3, 12], WHITE);
        side_box(door, side, [10, -7, -12], [11, -4, 12], ACCENT);
        side_box(door, side, [10, -4, -12], [11, -3, 12], PINSTRIPE);
        side_box(door, side, [9, -5, -11], [10, 1, 10], LINER);
        side_box(door, side, [9, 0, -11], [10, 1, 10], SEAT);
        side_box(door, side, [8, -4, -9], [9, -3, 5], TRIM);
        side_box(door, side, [10, 3, -12], [11, 4, 12], NAVY);
        side_box(door, side, [10, 4, -12], [11, 11, -11], NAVY);
        side_box(door, side, [10, 4, 11], [11, 11, 12], NAVY);
        side_box(door, side, [10, 11, -12], [11, 12, 12], WHITE);
        side_box(door, side, [9, 1, -8], [10, 2, -4], METAL);
        side_box(door, side, [11, 1, -8], [12, 2, -5], BLACK);
        cabin
            .panes
            .push((part, side_pane(side, [4, 11], [-11, 11])));
    }

    cabin
}
