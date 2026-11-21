//! A small world-aligned irradiance input shared by the voxel, mesh and air passes.
use crate::{
    settings::VisualSettings,
    water_reflection::{Panorama, Reflections},
    Camera,
};
use frontier_world::{Cell, Material, World};

const SIDE: usize = 13;
const SPACING: f64 = 12.;
const COUNT: usize = SIDE * SIDE;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GroundUniform {
    origin_strength: [f32; 4],
    nodes: [[[f32; 4]; SIDE]; SIDE],
    water_options: [f32; 4],
    currents: [[u32; 4]; COUNT.div_ceil(4)],
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
        let mut currents = [[0; 4]; COUNT.div_ceil(4)];
        let mut has_current = false;
        for (i, node) in self.nodes.iter().enumerate() {
            currents[i / 4][i % 4] = node.current;
            has_current |= node.current != 0;
        }
        GroundUniform {
            water_options: [
                f32::from(cloud_stage.is_some()),
                0.,
                f32::from(has_current),
                cloud_stage.map_or(0., |y| (y - eye.y) as f32),
            ],
            currents,
            water,
            origin_strength: [
                (start[0] as f64 * SPACING - eye.x) as f32,
                (start[1] as f64 * SPACING - eye.z) as f32,
                settings.ground_bounce,
                SPACING as f32,
            ],
            nodes: std::array::from_fn(|z| {
                std::array::from_fn(|x| {
                    let node = self.nodes[x + SIDE * z];
                    let [r, g, b] = node.color;
                    [r, g, b, (node.bed - eye.y) as f32]
                })
            }),
        }
    }
}

#[derive(Clone, Copy)]
struct GroundSample {
    color: [f32; 3],
    bed: f64,
    water: Option<f64>,
    current: u32,
}

fn sample_ground(world: &World, x: i64, z: i64) -> GroundSample {
    let [x, z] = [x, z].map(|v| v as f64 * SPACING);
    let cell = Cell::from_meters([x, 0., z]);
    let column = world.column(cell.x, cell.z);
    let height = column.ground_height_m();
    let material = column.canonical_material_without_fluid(column.ground_height_cells() - 1);
    let color = if material == Material::Air || material == Material::Water {
        [0.27, 0.26, 0.24]
    } else {
        material.color()
    };
    let water = (column.height_cells() > column.ground_height_cells()).then(|| column.height_m());
    let current = water
        .filter(|&y| y > 0.)
        .and_then(|_| world.fbm_terrain()?.river_course(x, z))
        .map_or(0, |([dx, dz], grade)| {
            let encode = |v: f64| (v.clamp(-1., 1.) * 127.).round() as i8 as u8;
            u32::from_le_bytes([encode(dx), encode(dz), encode((grade / 0.14).sqrt()), 127])
        });
    GroundSample {
        color,
        bed: height,
        water,
        current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

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
