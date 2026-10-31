//! Diagnostic export only. The world core owns environmental classifications.
use microvoxel::generation::{
    debug::{DebugImage, DebugLayer},
    Terrain,
};
use std::{
    fs::{self, File},
    io::BufWriter,
    path::Path,
};
pub fn export(terrain: &Terrain, directory: &Path) -> microvoxel::Result<()> {
    fs::create_dir_all(directory)?;
    let mut legends=format!("Long-term regional estimates, not live weather. Map covers {} km square.\nX increases right, Z increases down. Diagnostic colors are not voxel palettes.\n\n", microvoxel::generation::Generator::spec(terrain).extent_m as f64 * terrain.world_scale() as f64 / 1000.);
    for layer in DebugLayer::ALL {
        let image = terrain.debug_image(layer, 512)?;
        write_png(&image, &directory.join(format!("{layer}.png")))?;
        legends.push_str(&format!("{layer}: {}\n", layer.legend()));
    }
    fs::write(directory.join("legend.txt"), legends)?;
    fs::write(
        directory.join("summary.txt"),
        format!(
            "Config: {:?}\nRegional arrays/bins: {:.2} MiB\n{}",
            terrain.config(),
            terrain.regional_memory_bytes() as f64 / 1048576.,
            terrain.environment_statistics()
        ),
    )?;
    println!("Saved 12 diagnostic PNG maps to {}", directory.display());
    Ok(())
}
fn write_png(image: &DebugImage, path: &Path) -> microvoxel::Result<()> {
    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(path)?),
        image.size as u32,
        image.size as u32,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.rgb)?;
    Ok(())
}
