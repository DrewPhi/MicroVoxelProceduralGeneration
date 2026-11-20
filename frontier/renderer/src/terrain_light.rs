//! A small world-aligned irradiance input shared by the voxel, mesh and air passes.
use crate::Camera;
use frontier_world::{Material, World, VOXEL_SIZE};
use wgpu::util::DeviceExt;

const SIDE: usize = 13;
const SPACING: f64 = 12.;
const COUNT: usize = SIDE * SIDE;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GroundUniform {
    origin_strength: [f32; 4],
    nodes: [[[f32; 4]; SIDE]; SIDE],
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
    nodes: Vec<([f32; 3], f64)>,
}

impl GroundField {
    pub(crate) fn prepare(
        &mut self,
        world: &World,
        camera: Camera,
        strength: f32,
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
        GroundUniform {
            origin_strength: [
                (start[0] as f64 * SPACING - eye.x) as f32,
                (start[1] as f64 * SPACING - eye.z) as f32,
                strength,
                SPACING as f32,
            ],
            nodes: std::array::from_fn(|z| {
                std::array::from_fn(|x| {
                    let ([r, g, b], y) = self.nodes[x + SIDE * z];
                    [r, g, b, (y - eye.y) as f32]
                })
            }),
        }
    }
}

fn sample_ground(world: &World, x: i64, z: i64) -> ([f32; 3], f64) {
    let step = (SPACING / VOXEL_SIZE) as i64;
    let column = world.column(x * step, z * step);
    let height = column.ground_height_m();
    let material = column.canonical_material_without_fluid((height / VOXEL_SIZE) as i64 - 1);
    let color = if material == Material::Air || material == Material::Water {
        [0.27, 0.26, 0.24]
    } else {
        material.color()
    };
    (color, height)
}
