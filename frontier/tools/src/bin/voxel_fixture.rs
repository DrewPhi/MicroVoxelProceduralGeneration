fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: voxel_fixture OUTPUT_DIRECTORY")?;
    frontier_renderer::export_voxel_fixtures(std::path::Path::new(&path))
}
