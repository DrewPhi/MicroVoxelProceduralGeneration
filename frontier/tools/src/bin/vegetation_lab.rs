fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "output/vegetation-lab".into());
    frontier_renderer::vegetation_lab::run(std::path::Path::new(&output))
}
