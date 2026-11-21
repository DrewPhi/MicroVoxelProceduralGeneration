//! A small world-aligned irradiance input shared by the voxel, mesh and air passes.
use crate::{
    settings::VisualSettings,
    water_reflection::{Panorama, Reflections},
    Camera,
};
use frontier_world::{Cell, Material, World};
use wgpu::util::DeviceExt;

const SIDE: usize = 13;
const SPACING: f64 = 12.;
const COUNT: usize = SIDE * SIDE;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GroundUniform {
    origin_strength: [f32; 4],
    nodes: [[[f32; 4]; SIDE]; SIDE],
    water: Panorama,
}

impl GroundUniform {
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
    use bytemuck::Zeroable;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("nearby terrain color and elevation"),
        contents: bytemuck::bytes_of(&GroundUniform::zeroed()),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    })
}

#[derive(Default)]
pub(crate) struct GroundField {
    start: [i64; 2],
    source: Option<World>,
    nodes: Vec<([f32; 3], f64, Option<f64>)>,
    reflections: Reflections,
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
            .is_some_and(|old| old.shares_source_lineage(world));
        if !same_source || start != self.start {
            let mut nodes = Vec::with_capacity(COUNT);
            for z in start[1]..start[1] + SIDE as i64 {
                for x in start[0]..start[0] + SIDE as i64 {
                    let index = [x - self.start[0], z - self.start[1]];
                    let overlap = index.iter().all(|i| (0..SIDE as i64).contains(i));
                    let sample = if same_source && overlap {
                        self.nodes[index[0] as usize + SIDE * index[1] as usize]
                    } else {
                        sample_ground(world, x, z)
                    };
                    nodes.push(sample);
                }
            }
            self.start = start;
            self.source = Some(world.clone());
            self.nodes = nodes;
        }
        let mut water = bytemuck::Zeroable::zeroed();
        if let Some(terrain) = world
            .fbm_terrain()
            .filter(|t| t.params().recipe.has_static_water())
        {
            let mut nearest = f64::INFINITY;
            let mut anchor = None;
            let center = [eye.x, eye.z].map(|v| (v / 48.).round() * 48.);
            for (i, &(_, _, surface)) in self.nodes.iter().enumerate() {
                if let Some(y) = surface {
                    let x = (start[0] + (i % SIDE) as i64) as f64 * SPACING;
                    let z = (start[1] + (i / SIDE) as i64) as f64 * SPACING;
                    let distance = (x - center[0]).powi(2) + (z - center[1]).powi(2);
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
        GroundUniform {
            water,
            origin_strength: [
                (start[0] as f64 * SPACING - eye.x) as f32,
                (start[1] as f64 * SPACING - eye.z) as f32,
                settings.ground_bounce,
                SPACING as f32,
            ],
            nodes: std::array::from_fn(|z| {
                std::array::from_fn(|x| {
                    let ([r, g, b], y, _) = self.nodes[x + SIDE * z];
                    [r, g, b, (y - eye.y) as f32]
                })
            }),
        }
    }
}

fn sample_ground(world: &World, x: i64, z: i64) -> ([f32; 3], f64, Option<f64>) {
    let cell = Cell::from_meters([x as f64 * SPACING, 0., z as f64 * SPACING]);
    let column = world.column(cell.x, cell.z);
    let height = column.ground_height_m();
    let material = column.canonical_material_without_fluid(column.ground_height_cells() - 1);
    let color = if material == Material::Air || material == Material::Water {
        [0.27, 0.26, 0.24]
    } else {
        material.color()
    };
    let water = (column.height_cells() > column.ground_height_cells()).then(|| column.height_m());
    (color, height, water)
}
