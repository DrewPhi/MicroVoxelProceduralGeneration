//! Offscreen character inspection, using the same GPU surface/color pipeline.
//! PNGs/APNGs are rendered from real voxel poses, not hand-painted sprites.
use crate::gpu::Gpu;
use glam::{Mat4, Vec3};
use microvoxel::{
    character::{CharacterRecipe, Clip, Pose, PoseCache},
    materials::MaterialRegistry,
    render::{
        lod::{MeshKey, TileKey},
        surface::{FaceCollector, Surface},
        terrain::{SurfacePart, TerrainMesh},
    },
};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::BufWriter,
    path::Path,
    time::Instant,
};
const WIDTH: u32 = 512;
const HEIGHT: u32 = 640;
const RESOLUTIONS: [u16; 3] = [16, 32, 64];
const CACHE_BUDGET: usize = 16 * 1024 * 1024;
struct Asset {
    resolution: u16,
    materials: MaterialRegistry,
    poses: PoseCache,
}
impl Asset {
    fn new(seed: u64, resolution: u16) -> microvoxel::Result<Self> {
        let recipe = CharacterRecipe { seed };
        let mut materials = MaterialRegistry::default();
        let palette = recipe.register_palette(&mut materials)?;
        Ok(Self {
            resolution,
            materials,
            poses: PoseCache::new(recipe, palette, resolution, CACHE_BUDGET),
        })
    }
}
fn key(revision: u64) -> MeshKey {
    MeshKey {
        tile: TileKey {
            level: 13,
            x: 0,
            z: 0,
        },
        neighbors: [13; 4],
        revision,
    }
}
fn floor(resolution: u16) -> Result<Surface, String> {
    let r = i64::from(resolution);
    let mut faces = FaceCollector::default();
    // One-meter checker patches are made of whole cells, with no voxel outlines.
    for z in -r..r {
        for x in -r..r {
            let light = (x.div_euclid(r) + z.div_euclid(r)).rem_euclid(2) == 0;
            faces.add(
                0,
                [x, 0, z],
                if light {
                    [103, 122, 119]
                } else {
                    [88, 106, 104]
                },
            )?;
        }
    }
    Ok(faces.finish([0; 3], 1))
}
fn render(gpu: &mut Gpu, asset: &mut Asset, pose: Pose) -> microvoxel::Result<Vec<u8>> {
    let frame = asset.poses.frame(pose)?;
    let surface = frame.surface(&asset.materials, pose.preview_root_cells(asset.resolution))?;
    let model = key(1);
    let ground = key(2);
    let cover = BTreeSet::from([model, ground]);
    gpu.upload(
        model,
        TerrainMesh {
            origin: [0; 3],
            parts: vec![SurfacePart { step: 1, surface }],
        },
        &cover,
    )?;
    // Floor is replaced only when resolution changes between the comparison views.
    gpu.upload(
        ground,
        TerrainMesh {
            origin: [0; 3],
            parts: vec![SurfacePart {
                step: 1,
                surface: floor(asset.resolution)?,
            }],
        },
        &cover,
    )?;
    let eye = [2.6, 1.95, -4.4];
    let direction = Vec3::new(-2.6, -0.95, 4.4);
    let vertical = 2.6;
    let horizontal = vertical * WIDTH as f32 / HEIGHT as f32;
    let matrix = Mat4::orthographic_rh(
        -horizontal / 2.,
        horizontal / 2.,
        -vertical / 2.,
        vertical / 2.,
        0.01,
        30.,
    ) * Mat4::look_to_rh(Vec3::ZERO, direction, Vec3::Y);
    let texture = gpu.offscreen();
    gpu.render_at_scale(
        &texture.create_view(&Default::default()),
        eye,
        matrix,
        &cover,
        None,
        true,
        1. / f64::from(asset.resolution),
    );
    gpu.read_pixels(&texture)
}
fn join(images: &[Vec<u8>]) -> Vec<u8> {
    let row = WIDTH as usize * 4;
    let mut result = vec![0; row * HEIGHT as usize * images.len()];
    for y in 0..HEIGHT as usize {
        for (i, image) in images.iter().enumerate() {
            let start = (y * images.len() + i) * row;
            result[start..start + row].copy_from_slice(&image[y * row..(y + 1) * row]);
        }
    }
    result
}
fn png(path: &Path, width: u32, height: u32, pixels: &[u8]) -> microvoxel::Result<()> {
    let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(pixels)?;
    Ok(())
}
pub fn export(seed: u64, directory: &Path) -> microvoxel::Result<()> {
    fs::create_dir_all(directory)?;
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let mut gpu = pollster::block_on(Gpu::new(&instance, None, WIDTH, HEIGHT))?;
    let mut assets = RESOLUTIONS
        .into_iter()
        .map(|r| Asset::new(seed, r))
        .collect::<microvoxel::Result<Vec<_>>>()?;
    let start = Instant::now();
    let mut report=String::from("Real voxel character preview; no smooth skinning or rotated voxel faces.\nColumns: 6.25 cm (current world), 3.125 cm, 1.5625 cm. Same meter-scale recipe and seed.\nFiner resolutions are isolated asset experiments; world grid remains 6.25 cm.\n\n");
    let mut stills = Vec::new();
    for asset in &mut assets {
        let before = Instant::now();
        let frame = asset.poses.frame(Pose::new(Clip::Idle, 0))?;
        let generated = before.elapsed();
        let surface = frame.surface(&asset.materials, [0; 3])?;
        report.push_str(&format!("{} cells/m: {} occupied cells; {} frame bytes (cells + ownership); {} exposed faces -> {} rectangles; generation {:?}\n",asset.resolution,frame.solid_count(),frame.memory_bytes(),surface.unit_faces,surface.quads.len(),generated));
        stills.push(render(&mut gpu, asset, Pose::new(Clip::Idle, 0))?);
    }
    png(
        &directory.join("model_resolution.png"),
        WIDTH * 3,
        HEIGHT,
        &join(&stills),
    )?;
    let mut views = Vec::new();
    for heading in [0, 4, 8] {
        views.push(render(
            &mut gpu,
            &mut assets[0],
            Pose::new(Clip::Idle, 0).facing(heading),
        )?);
    }
    png(
        &directory.join("model_views.png"),
        WIDTH * 3,
        HEIGHT,
        &join(&views),
    )?;
    for (name, angles) in [
        ("model_head_yaw", [(-4, 0), (0, 0), (4, 0)]),
        ("model_head_pitch", [(0, -6), (0, 0), (0, 6)]),
    ] {
        let mut images = Vec::new();
        for (yaw, pitch) in angles {
            images.push(render(
                &mut gpu,
                &mut assets[0],
                Pose::new(Clip::Idle, 0).looking(yaw, pitch),
            )?);
        }
        png(
            &directory.join(format!("{name}.png")),
            WIDTH * 3,
            HEIGHT,
            &join(&images),
        )?;
    }
    report.push_str("model_head_yaw.png: native resolution, head -45/0/+45 degrees; body fixed.\nmodel_head_pitch.png: native resolution, head down/level/up 45 degrees; body fixed.\n");
    for clip in Clip::ALL {
        let path = directory.join(format!("{}.png", clip.name()));
        let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), WIDTH * 3, HEIGHT);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_animated(u32::from(clip.frames()), 0)?;
        encoder.set_frame_delay(1, clip.fps())?;
        encoder.set_blend_op(png::BlendOp::Source)?;
        let mut writer = encoder.write_header()?;
        // One frame at a time bounds image readback/encoding memory. The voxel
        // cache is independently capped; externally held frames are dropped.
        for i in 0..clip.frames() {
            let mut images = Vec::new();
            for asset in &mut assets {
                images.push(render(&mut gpu, asset, Pose::new(clip, i))?);
            }
            let pixels = join(&images);
            writer.write_image_data(&pixels)?;
            if clip == Clip::Walk && i % 3 == 0 {
                png(
                    &directory.join(format!("walk_pose_{i:02}.png")),
                    WIDTH * 3,
                    HEIGHT,
                    &pixels,
                )?;
            }
        }
        writer.finish()?;
        println!(
            "Saved grid-aligned {} animation ({} frames at {} poses/sec)",
            clip.name(),
            clip.frames(),
            clip.fps()
        );
    }
    for asset in &assets {
        report.push_str(&format!(
            "{} cells/m: retained pose cache {} bytes (limit {} bytes, max 16 entries)\n",
            asset.resolution,
            asset.poses.memory_bytes(),
            CACHE_BUDGET
        ));
    }
    report.push_str(&format!("GPU {}; export elapsed {:.2}s. Includes GPU readbacks/PNG encoding, not a gameplay benchmark.\nStandalone asset export; gameplay gravity/collision and F5 views run separately. Jump root motion here is a preview track in whole cells.\n",gpu.adapter_name,start.elapsed().as_secs_f64()));
    fs::write(directory.join("README.txt"), &report)?;
    println!("{report}\nSaved previews in {}", directory.display());
    Ok(())
}
