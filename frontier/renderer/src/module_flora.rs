//! Instanced bounds over a shared atlas of native 4³ occupancy bricks.
use frontier_world::{module_flora, Material, World};
use wgpu::util::DeviceExt;

const BRICK: i64 = 4;
const TILE: i64 = 16;
const HEADER: usize = 16;
const CANDIDATE_SIDE: u64 =
    2 * (super::native_lattice::MAX_FAR as u64).div_ceil(module_flora::SITE_CELLS as u64 / 16) + 3;
const CAPACITY: u64 = CANDIDATE_SIDE * CANDIDATE_SIDE;

fn address(p: [usize; 3], dims: [usize; 3]) -> usize {
    p[0] + dims[0] * (p[2] + dims[2] * p[1])
}

fn atlas() -> Vec<u32> {
    let mut data = vec![0; HEADER * module_flora::TEMPLATE_COUNT];
    for (kind, tree) in module_flora::templates().iter().enumerate() {
        let min = [tree.min.x, tree.min.y, tree.min.z];
        let max = [tree.max.x, tree.max.y, tree.max.z];
        let lo = min.map(|p| p.div_euclid(TILE) * TILE);
        let dims = std::array::from_fn(|i| (max[i] - lo[i] + TILE - 1) as usize / TILE as usize);
        let index = kind * HEADER;
        let tiles = data.len();
        data[index..index + HEADER].copy_from_slice(&[
            lo[0] as u32,
            lo[1] as u32,
            lo[2] as u32,
            tiles as u32,
            dims[0] as u32,
            dims[1] as u32,
            dims[2] as u32,
            0,
            min[0] as u32,
            min[1] as u32,
            min[2] as u32,
            0,
            max[0] as u32,
            max[1] as u32,
            max[2] as u32,
            0,
        ]);
        data.resize(tiles + dims.iter().product::<usize>(), 0);
        for voxel in &tree.voxels {
            let p = [voxel.cell.x, voxel.cell.y, voxel.cell.z];
            let p: [usize; 3] = std::array::from_fn(|i| (p[i] - lo[i]) as usize);
            let tile = tiles + address(p.map(|v| v / TILE as usize), dims);
            if data[tile] == 0 {
                data[tile] = data.len() as u32;
                data.resize(data.len() + 64, 0);
            }
            let brick = data[tile] as usize + address(p.map(|v| (v / BRICK as usize) % 4), [4; 3]);
            if data[brick] == 0 {
                data[brick] = data.len() as u32;
                data.resize(data.len() + 4, 0);
            }
            let cell = address(p.map(|v| v % BRICK as usize), [4; 3]);
            let material = match voxel.material {
                Material::Wood => 1,
                Material::Leaf => 2,
                Material::Flower => 3,
                _ => panic!("unsupported canonical woody template material"),
            };
            let word = data[brick] as usize + cell / 16;
            data[word] |= material << (2 * (cell % 16));
        }
    }
    data
}

pub(crate) const SHADER: &str = include_str!("module_flora.wgsl");

pub(crate) struct ModuleFlora {
    place: wgpu::ComputePipeline,
    draw: wgpu::RenderPipeline,
    atlas: wgpu::Buffer,
    instances: wgpu::Buffer,
    indirect: wgpu::Buffer,
    enabled: bool,
    placement: Option<wgpu::BindGroup>,
    rendering: Option<wgpu::BindGroup>,
    edits: Option<wgpu::Buffer>,
    edit_count: usize,
    compute_terrain_depth: bool,
    depth: wgpu::Buffer,
}

fn buffer(binding: u32, b: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: b.as_entire_binding(),
    }
}

impl ModuleFlora {
    pub fn new(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
        samples: u32,
        compute_terrain_depth: bool,
        capture_distance: bool,
    ) -> Self {
        let place = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("canonical woody placement"),
            layout: None,
            module: shader,
            entry_point: Some("flora_place"),
            compilation_options: Default::default(),
            cache: None,
        });
        let constants = std::collections::HashMap::from([
            ("FLORA_MSAA".to_owned(), f64::from(samples > 1)),
            ("CAPTURE_DISTANCE".to_owned(), f64::from(capture_distance)),
            (
                "FLORA_TERRAIN_DEPTH".to_owned(),
                f64::from(compute_terrain_depth),
            ),
        ]);
        let draw = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shared native woody rays"),
            layout: None,
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("flora_vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("flora_fragment"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });
        let atlas = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("shared native woody atlas"),
            contents: bytemuck::cast_slice(&atlas()),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("visible canonical plant roots"),
            size: CAPACITY * 32,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let indirect = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("woody instance draw arguments"),
            contents: bytemuck::cast_slice(&[6u32, 0, 0, 0]),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST,
        });
        let depth = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fallback native forest depth"),
            size: 8,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        Self {
            place,
            draw,
            atlas,
            instances,
            indirect,
            depth,
            enabled: false,
            placement: None,
            rendering: None,
            edits: None,
            edit_count: 0,
            compute_terrain_depth,
        }
    }

    pub fn publish(
        &mut self,
        device: &wgpu::Device,
        world: &World,
        view: &wgpu::Buffer,
        lattice: &wgpu::Buffer,
        controls: &wgpu::Buffer,
        detail: &wgpu::TextureView,
        hits: &wgpu::Buffer,
        native_view: &wgpu::Buffer,
    ) {
        let mut edits: Vec<[i32; 4]> = world
            .terrain_edits()
            .filter_map(|(p, _)| {
                Some([
                    i32::try_from(p.x).ok()?,
                    i32::try_from(p.y).ok()?,
                    i32::try_from(p.z).ok()?,
                    0,
                ])
            })
            .collect();
        edits.sort_unstable();
        self.edit_count = edits.len();
        edits.insert(0, [self.edit_count as i32, 0, 0, 0]);
        let edits = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native woody edit tombstones"),
            contents: bytemuck::cast_slice(&edits),
            usage: wgpu::BufferUsages::STORAGE,
        });
        self.placement = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("woody placement resources"),
            layout: &self.place.get_bind_group_layout(0),
            entries: &[
                buffer(0, view),
                buffer(32, lattice),
                buffer(33, controls),
                wgpu::BindGroupEntry {
                    binding: 40,
                    resource: wgpu::BindingResource::TextureView(detail),
                },
                buffer(41, &self.atlas),
                buffer(42, &self.instances),
                buffer(44, &self.indirect),
            ],
        }));
        self.enabled = world.module_flora_enabled();
        let hits = if self.compute_terrain_depth {
            hits
        } else {
            &self.depth
        };
        let resources = [
            buffer(0, view),
            buffer(38, hits),
            buffer(9, native_view),
            buffer(32, lattice),
            buffer(41, &self.atlas),
            buffer(43, &self.instances),
            buffer(45, &edits),
        ];
        self.rendering = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("woody tracing resources"),
            layout: &self.draw.get_bind_group_layout(0),
            entries: &resources,
        }));
        self.edits = Some(edits);
    }

    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, far: f32) {
        if !self.enabled {
            return;
        }
        let Some(bind) = &self.placement else {
            return;
        };
        encoder.clear_buffer(&self.indirect, 4, Some(4));
        let side = 2 * (far / 16.).ceil() as u32 + 3;
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("native woody projection cull"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.place);
        pass.set_bind_group(0, bind, &[]);
        pass.dispatch_workgroups(side.div_ceil(8), side.div_ceil(8), 1);
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(bind) = self.rendering.as_ref().filter(|_| self.enabled) {
            pass.set_pipeline(&self.draw);
            pass.set_bind_group(0, bind, &[]);
            pass.draw_indirect(&self.indirect, 0);
        }
    }

    pub fn json(&self) -> String {
        let bytes = self.atlas.size()
            + self.instances.size()
            + self.indirect.size()
            + self.edits.as_ref().map_or(0, wgpu::Buffer::size);
        format!(
            r#""vegetation":{{"mode":"shared_native_modules","enabled":{},"gpu_bytes":{bytes},"atlas_bytes":{},"templates":{},"edit_count":{},"grass_band_cells":{}}}"#,
            self.enabled,
            self.atlas.size(),
            module_flora::TEMPLATE_COUNT,
            self.edit_count,
            module_flora::GRASS_HEIGHT
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mesh, settings::VisualSettings, Camera, Gpu};
    use frontier_world::{
        native_lattice::relief_seed, raycast, Cell, FbmParams, FbmRecipe, FbmTerrain,
    };
    use glam::DVec3;

    #[test]
    #[ignore = "requires a Vulkan GPU for production tree and ground cover ray readback"]
    fn modular_flora_depth_and_cut_cells_match_world_raycast() {
        const SEED: u64 = 6401777571733596131;
        let terrain = FbmTerrain::new(
            SEED,
            FbmParams {
                recipe: FbmRecipe::Eroded,
                ..Default::default()
            },
        )
        .unwrap()
        .with_native_detail();
        let mut world = World::from_fbm_terrain(terrain.clone())
            .with_vegetation()
            .with_module_flora(true);
        let (eye, look) = (-13..-3)
            .flat_map(|x| (-13..-3).map(move |z| [x, z]))
            .filter_map(|site| module_flora::woody_site(site, relief_seed(SEED)))
            .filter(|(_, kind, _)| *kind < 11)
            .find_map(|(root, kind, rotation)| {
                let surface = terrain.column_cells(root[0], root[1]);
                if surface.ground_top != surface.visible_top
                    || surface.ground_surface != Material::Grass
                {
                    return None;
                }
                let record = frontier_world::vegetation::PlantRecord {
                    root: Cell {
                        x: root[0],
                        y: surface.ground_top,
                        z: root[1],
                    },
                    rotation,
                    species: frontier_world::vegetation::Species::Rowan,
                    seed: 0,
                    variant: 0,
                };
                let trunk = module_flora::templates()[kind]
                    .voxels
                    .iter()
                    .filter(|v| v.material == Material::Wood && v.cell.y > 12)
                    .min_by_key(|v| (v.cell.y - 26).abs() + v.cell.x.abs() + v.cell.z.abs())?;
                let target = DVec3::from_array(record.world_cell(trunk.cell).center_m());
                [
                    DVec3::new(1.3, 0.5, -2.8),
                    DVec3::new(-2.1, 0.7, -1.9),
                    DVec3::new(2.4, 0.5, 1.2),
                    DVec3::new(-1.2, 0.7, 2.6),
                ]
                .into_iter()
                .find_map(|offset| {
                    let eye = target + offset;
                    let look = -offset.normalize();
                    let hit = raycast(&world, eye.to_array(), look.to_array(), 5.)?;
                    (hit.material == Material::Wood && hit.distance > 1.4).then_some((eye, look))
                })
            })
            .expect("woodland fixture has a visible trunk");
        let camera = Camera {
            position: eye,
            yaw: look.x.atan2(look.z) as f32,
            pitch: look.y.asin() as f32,
            roll: 0.13,
            far: 60.,
            fov_y: 51.,
        };
        let settings = VisualSettings {
            view_distance: camera.far,
            native_lattice: true,
            native_detail: true,
            module_flora: true,
            vegetation: true,
            capture_terrain_distance: true,
            msaa: 1,
            ..Default::default()
        };
        let mesh = mesh::Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
            origin: Cell::default(),
            exposed_native_faces: 0,
        };
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let mut gpu = pollster::block_on(Gpu::new(
            &instance,
            None,
            Some(wgpu::TextureFormat::Rgba32Float),
            &mesh,
            1,
            false,
            false,
        ))
        .unwrap();
        gpu.configure_visuals(&world, &settings);
        gpu.init_rolling(&world, camera).unwrap();

        let path =
            std::env::temp_dir().join(format!("frontier-module-audit-{}.png", std::process::id()));
        let width = 65u32;
        let height = 49u32;
        let forward = camera.direction();
        let right = forward.cross(camera.view_up()).normalize();
        let up = right.cross(forward).normalize();
        let scale = (camera.fov_y.to_radians() * 0.5).tan();
        let right = right * scale * width as f32 / height as f32;
        let up = up * scale;
        let mut prior = Vec::new();
        for edited in [false, true] {
            gpu.capture(&world, &camera, &path, width, height).unwrap();
            let bytes = std::fs::read(path.with_extension("linear_rgb.f32")).unwrap();
            let coverage = std::fs::read(path.with_extension("coverage.f32")).unwrap();
            let mut cuts = Vec::new();
            let mut hits = 0;
            let mut wood = 0;
            let mut plants = 0;
            let mut differences = 0;
            let mut worst = 0f64;
            for y in 0..height {
                for x in 0..width {
                    let i = (y * width + x) as usize;
                    let present =
                        f32::from_le_bytes(coverage[i * 4..i * 4 + 4].try_into().unwrap()) == 1.;
                    let distance =
                        f32::from_le_bytes(bytes[i * 12..i * 12 + 4].try_into().unwrap());
                    let u = ((2 * x + 1) as f32 - width as f32) / width as f32;
                    let v = ((2 * y + 1) as f32 - height as f32) / height as f32;
                    let ray = forward + right * u - up * v;
                    let expected = raycast(
                        &world,
                        eye.to_array(),
                        ray.as_dvec3().to_array(),
                        f64::from(camera.far),
                    );
                    assert_eq!(
                        present,
                        expected.is_some(),
                        "module flora coverage at ({x},{y}), edited={edited}"
                    );
                    if let Some(hit) = expected {
                        hits += 1;
                        let error = (f64::from(distance) - hit.distance).abs();
                        worst = worst.max(error);
                        assert!(error < 0.025, "module flora distance at ({x},{y}), edited={edited}: GPU {distance} CPU {hit:?}");
                        wood += usize::from(hit.material == Material::Wood);
                        if matches!(
                            hit.material,
                            Material::Wood | Material::Leaf | Material::Flower
                        ) {
                            plants += 1;
                            if x % 3 != 0 && y % 3 != 0 {
                                cuts.push((hit.cell, Material::Air));
                            }
                        }
                    }
                    if edited {
                        differences += usize::from(prior[i] != (present, distance));
                    } else {
                        prior.push((present, distance));
                    }
                }
            }
            eprintln!("edited={edited}: {hits} hits, {plants} plant, {wood} wood, {differences} changed pixels, {worst:.6} m maximum error");
            if !edited {
                assert!(
                    plants > 30 && wood > 0,
                    "fixture must exercise ground and woody visibility"
                );
                world.set_batch(cuts);
                gpu.update_rolling(&world, camera).unwrap();
            } else {
                assert!(
                    differences > 20,
                    "live edits must expose subsequent visible cells"
                );
            }
        }
        for extension in [
            "png",
            "linear_rgb.f32",
            "coverage.f32",
            "depth.f32",
            "capture.json",
        ] {
            let _ = std::fs::remove_file(path.with_extension(extension));
        }
    }
}
