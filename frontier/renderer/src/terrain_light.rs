//! A small world-aligned irradiance input shared by the voxel, mesh and air passes.
use crate::{
    settings::VisualSettings,
    water_reflection::{Panorama, Reflections},
    Camera,
};
use frontier_world::{voxel_size, Cell, FbmTerrain, Material, World};

const SIDE: usize = 13;
const SPACING: f64 = 12.;
const COUNT: usize = SIDE * SIDE;
const WATER_SIDE: usize = 27;
const WATER_COUNT: usize = WATER_SIDE * WATER_SIDE;
const WATER_SPACING: f64 = 4.;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GroundUniform {
    origin_strength: [f32; 4],
    heights: [[f32; 4]; COUNT.div_ceil(4)],
    materials: [[u32; 4]; COUNT.div_ceil(32)],
    water_options: [f32; 4],
    water_origin: [f32; 4],
    water_nodes: [[u32; 4]; WATER_COUNT.div_ceil(4)],
    water: Panorama,
}

pub(crate) const CLOUD_OFFSET: u64 = std::mem::size_of::<GroundUniform>() as u64;

impl GroundUniform {
    pub(crate) fn enable_clouds(
        &mut self,
        enabled: bool,
        eye: f64,
        distant: Option<f32>,
    ) -> Option<f32> {
        let nearby = (self.water_options[0] > 0.).then_some(eye as f32 + self.water_options[3]);
        let stage = enabled.then(|| nearby.or(distant)).flatten();
        self.water_options[0] = f32::from(stage.is_some());
        self.water_options[1] = (2060. - eye) as f32;
        let probe = stage.map(|y| {
            // Keep the sky probe at eye level along the waterline and lower it
            // continuously to the reflecting surface as the aircraft climbs.
            let t = ((eye as f32 - y).abs() / 150.).clamp(0., 1.);
            eye as f32 + (y - eye as f32) * t * t * (3. - 2. * t)
        });
        self.water_options[3] = probe.map_or(0., |y| y - eye as f32);
        probe
    }

    pub(crate) fn upload_bytes(&self) -> &[u8] {
        let bytes = bytemuck::bytes_of(self);
        if self.water.enabled() {
            return bytes;
        }
        // Clear the reflection flag when leaving a river. Inactive frames only
        // upload ground lighting and the panorama origin.
        &bytes[..std::mem::offset_of!(Self, water) + std::mem::size_of::<[f32; 4]>()]
    }
}

pub(crate) fn buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("nearby terrain color and elevation"),
        size: CLOUD_OFFSET + crate::atmosphere::water_clouds::BYTES,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[derive(Default)]
pub(crate) struct GroundField {
    start: [i64; 2],
    source: Option<World>,
    nodes: Vec<GroundSample>,
    reflections: Reflections,
    water_grid: WaterGrid,
}

impl GroundField {
    pub(crate) fn prepare(
        &mut self,
        world: &World,
        camera: Camera,
        settings: &VisualSettings,
        capture: bool,
    ) -> GroundUniform {
        let eye = camera.position;
        let start = [eye.x, eye.z].map(|v| (v / SPACING).floor() as i64 - (SIDE / 2) as i64);
        let same_source = self
            .source
            .as_ref()
            .is_some_and(|old| old.shares_terrain_edits(world));
        if !same_source || start != self.start {
            let mut nodes = Vec::with_capacity(COUNT);
            for z in start[1]..start[1] + SIDE as i64 {
                for x in start[0]..start[0] + SIDE as i64 {
                    let index = [x - self.start[0], z - self.start[1]];
                    let overlap = index.iter().all(|i| (0..SIDE as i64).contains(i));
                    let sample = if same_source && overlap {
                        self.nodes[index[0] as usize + SIDE * index[1] as usize]
                    } else {
                        sample_ground(world, x as f64 * SPACING, z as f64 * SPACING)
                    };
                    nodes.push(sample);
                }
            }
            self.start = start;
            self.source = Some(world.clone());
            self.nodes = nodes;
        }
        let mut water = bytemuck::Zeroable::zeroed();
        let mut cloud_stage = None;
        if let Some(terrain) = world
            .fbm_terrain()
            .filter(|t| t.params().recipe.has_static_water())
        {
            let mut nearest = f64::INFINITY;
            let mut nearest_wet = f64::INFINITY;
            let mut anchor = None;
            let center = [eye.x, eye.z].map(|v| (v / 48.).round() * 48.);
            for (i, node) in self.nodes.iter().enumerate() {
                if let Some(y) = node.water {
                    let x = (start[0] + (i % SIDE) as i64) as f64 * SPACING;
                    let z = (start[1] + (i / SIDE) as i64) as f64 * SPACING;
                    let distance = (x - center[0]).powi(2) + (z - center[1]).powi(2);
                    if distance < nearest_wet {
                        nearest_wet = distance;
                        cloud_stage = Some(y);
                    }
                    if distance < nearest && (y - eye.y).abs() < 180. {
                        nearest = distance;
                        anchor = Some([x, y + 0.125, z]);
                    }
                }
            }
            if let Some(anchor) = anchor {
                water = self.reflections.prepare(
                    terrain,
                    world.vegetation_enabled(),
                    eye,
                    anchor,
                    settings,
                    capture,
                );
            }
        }
        let close_to_terrain = world
            .fbm_terrain()
            .is_some_and(|t| t.params().recipe.has_static_water())
            && self
                .nodes
                .iter()
                .any(|s| (s.water.unwrap_or(s.bed) - eye.y).abs() < 140.);
        self.water_grid
            .prepare(world, [eye.x, eye.z], same_source, close_to_terrain);
        let mut heights = [[0.; 4]; COUNT.div_ceil(4)];
        let mut materials = [[0; 4]; COUNT.div_ceil(32)];
        for (i, node) in self.nodes.iter().enumerate() {
            heights[i / 4][i % 4] = (node.bed - eye.y) as f32;
            materials[i / 32][(i / 8) % 4] |= (node.material as u32) << (4 * (i % 8));
        }
        GroundUniform {
            water_options: [
                f32::from(cloud_stage.is_some()),
                0.,
                f32::from(self.water_grid.has_current),
                cloud_stage.map_or(0., |y| (y - eye.y) as f32),
            ],
            heights,
            materials,
            water_nodes: self.water_grid.words,
            water_origin: self.water_grid.start.map_or([0.; 4], |[x, z]| {
                [
                    (x as f64 * WATER_SPACING - eye.x) as f32,
                    (z as f64 * WATER_SPACING - eye.z) as f32,
                    WATER_SPACING as f32,
                    0.,
                ]
            }),
            water,
            origin_strength: [
                (start[0] as f64 * SPACING - eye.x) as f32,
                (start[1] as f64 * SPACING - eye.z) as f32,
                settings.ground_bounce,
                SPACING as f32,
            ],
        }
    }
}

#[derive(Clone, Copy)]
struct GroundSample {
    material: Material,
    bed: f64,
    water: Option<f64>,
}

/// Two bits of mean aeration for a native inland water column. Near water
/// resolves the continuous response and its wave crests in the shader.
pub(crate) fn rapids(terrain: &FbmTerrain, x: i64, z: i64, top: i64) -> u8 {
    if top <= 0 {
        return 0;
    }
    let [x, z] = [x, z].map(|v| (v as f64 + 0.5) * voxel_size());
    terrain.river_course(x, z).map_or(0, |(_, grade)| {
        let t = (((grade / 0.14).sqrt() - 0.5) / 0.48).clamp(0., 1.);
        (3. * t * t * (3. - 2. * t)).round() as u8
    })
}

fn sample_ground(world: &World, x: f64, z: f64) -> GroundSample {
    let cell = Cell::from_meters([x, 0., z]);
    let column = world.column(cell.x, cell.z);
    let bed = column.ground_height_cells();
    GroundSample {
        material: column.canonical_material_without_fluid(bed - 1),
        bed: column.ground_height_m(),
        water: (column.height_cells() > bed).then(|| column.height_m()),
    }
}

// Pack native depth, bed material, grade and two signed course components in
// four bytes. Zero is dry. A wet lake or ocean has zero course components.
fn wet_node(world: &World, x: f64, z: f64) -> u32 {
    let c = Cell::from_meters([x, 0., z]);
    let column = world.column(c.x, c.z);
    let bed = column.ground_height_cells();
    let top = column.height_cells();
    if top <= bed {
        return 0;
    }
    let depth = ((top - bed) as f64 * voxel_size()).min(12.);
    let material = column.canonical_material_without_fluid(bed - 1);
    let mut word = 1 + (depth * 16.).round() as u32 | ((material as u32) << 8);
    if top > 0 {
        if let Some(([dx, dz], grade)) = world.fbm_terrain().and_then(|t| t.river_course(x, z)) {
            let direction = |d: f64| ((d.clamp(-1., 1.) * 63.).round() as i32 as u32) & 127;
            let grade = ((grade / 0.14).sqrt().min(1.) * 63.).round() as u32;
            word |= (grade << 12) | (direction(dx) << 18) | (direction(dz) << 25);
        }
    }
    word
}

struct WaterGrid {
    start: Option<[i64; 2]>,
    words: [[u32; 4]; WATER_COUNT.div_ceil(4)],
    has_current: bool,
}

impl Default for WaterGrid {
    fn default() -> Self {
        Self {
            start: None,
            words: [[0; 4]; WATER_COUNT.div_ceil(4)],
            has_current: false,
        }
    }
}

impl WaterGrid {
    fn prepare(&mut self, world: &World, eye: [f64; 2], same_source: bool, active: bool) {
        if !active {
            self.start = None;
            self.has_current = false;
            return;
        }
        let start = eye.map(|v| (v / WATER_SPACING).floor() as i64 - WATER_SIDE as i64 / 2);
        if same_source && self.start == Some(start) {
            return;
        }
        let previous = self.start.filter(|_| same_source);
        let mut words = [[0; 4]; WATER_COUNT.div_ceil(4)];
        let mut has_current = false;
        for i in 0..WATER_COUNT {
            let x = start[0] + (i % WATER_SIDE) as i64;
            let z = start[1] + (i / WATER_SIDE) as i64;
            let overlap = previous.and_then(|[ox, oz]| {
                let (u, v) = (x - ox, z - oz);
                let range = 0..WATER_SIDE as i64;
                (range.contains(&u) && range.contains(&v))
                    .then(|| u as usize + v as usize * WATER_SIDE)
            });
            let word = overlap.map_or_else(
                || wet_node(world, x as f64 * WATER_SPACING, z as f64 * WATER_SPACING),
                |j| self.words[j / 4][j % 4],
            );
            has_current |= word >> 18 != 0;
            words[i / 4][i % 4] = word;
        }
        self.words = words;
        self.start = Some(start);
        self.has_current = has_current;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    #[test]
    fn aeration_follows_the_native_water_column_and_reach_grade() {
        use frontier_world::{FbmParams, FbmRecipe};
        for (seed, recipe, x, z, expected) in [
            (673, FbmRecipe::Highlands, -2230., -1835., 3),
            (997, FbmRecipe::Watershed, -814.1769, 993., 0),
            (40961, FbmRecipe::Highlands, 360., -3000., 0),
        ] {
            let terrain = FbmTerrain::new(
                seed,
                FbmParams {
                    recipe,
                    ..FbmParams::default()
                },
            )
            .unwrap();
            let c = Cell::from_meters([x, 0., z]);
            let [bed, top] = terrain.column_tops(c.x, c.z);
            assert!(top > bed);
            assert_eq!(rapids(&terrain, c.x, c.z, top), expected);
        }
    }

    #[test]
    fn four_metre_water_survey_tracks_negative_origins_and_edited_bed_materials() {
        use frontier_world::{FbmParams, FbmRecipe};
        let mut world = World::fbm(
            673,
            FbmParams {
                recipe: FbmRecipe::Highlands,
                ..FbmParams::default()
            },
        )
        .unwrap();
        let eye = [-2246., -1835.];
        let mut grid = WaterGrid::default();
        grid.prepare(&world, eye, false, true);
        let moved = [eye[0] + 4., eye[1] - 8.];
        grid.prepare(&world, moved, true, true);
        let mut fresh = WaterGrid::default();
        fresh.prepare(&world, moved, false, true);
        assert_eq!(grid.words, fresh.words);
        let i = (0..WATER_COUNT)
            .find(|&i| grid.words[i / 4][i % 4] >> 18 != 0)
            .unwrap();
        let start = grid.start.unwrap();
        let x = (start[0] + (i % WATER_SIDE) as i64) as f64 * WATER_SPACING;
        let z = (start[1] + (i / WATER_SIDE) as i64) as f64 * WATER_SPACING;
        let mut c = Cell::from_meters([x, 0., z]);
        let column = world.column(c.x, c.z);
        c.y = column.ground_height_cells() - 1;
        let word = grid.words[i / 4][i % 4];
        let depth = (column.height_m() - column.ground_height_m()).min(12.);
        assert_eq!(word & 255, 1 + (16. * depth).round() as u32);
        let (direction, _) = world.fbm_terrain().unwrap().river_course(x, z).unwrap();
        let decode = |v: u32| ((v << 25) as i32 >> 25) as f64 / 63.;
        assert!((decode(word >> 18) - direction[0]).abs() <= 0.5 / 63.);
        assert!((decode(word >> 25) - direction[1]).abs() <= 0.5 / 63.);
        world.set_batch([(c, Material::Wood)]);
        grid.prepare(&world, moved, false, true);
        assert_eq!((grid.words[i / 4][i % 4] >> 8) & 15, Material::Wood as u32);
        grid.prepare(&world, moved, true, false);
        assert!(grid.start.is_none() && !grid.has_current);
    }

    #[test]
    fn reflection_probe_follows_the_water_through_flight_altitudes() {
        for eye in [178., 182., 500., 2250.] {
            let mut ground = GroundUniform::zeroed();
            ground.water_options = [1., 0., 0., -eye as f32];
            assert_eq!(ground.enable_clouds(true, eye, Some(30.)), Some(0.));
            assert_eq!(ground.water_options[3], -eye as f32);
        }
        let mut dry = GroundUniform::zeroed();
        assert_eq!(dry.enable_clouds(true, 2400., None), None);
        assert_eq!(dry.enable_clouds(true, 2400., Some(680.)), Some(680.));
        let mut off = GroundUniform::zeroed();
        assert_eq!(off.enable_clouds(false, 2400., Some(680.)), None);
    }
}
