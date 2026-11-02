//! Terrain colouring and PNG output.

use std::path::Path;

/// Height ramp: `t` in [0, 1] from lowest to highest point in view.
pub fn ramp(t: f64) -> [f64; 3] {
    const STOPS: [(f64, [f64; 3]); 5] = [
        (0.00, [0.20, 0.30, 0.18]),
        (0.35, [0.38, 0.48, 0.25]),
        (0.60, [0.52, 0.47, 0.38]),
        (0.82, [0.62, 0.60, 0.58]),
        (1.00, [0.95, 0.95, 0.96]),
    ];
    let t = t.clamp(0.0, 1.0);
    for w in STOPS.windows(2) {
        let ((a, ca), (b, cb)) = (w[0], w[1]);
        if t <= b {
            let f = (t - a) / (b - a);
            return std::array::from_fn(|i| ca[i] + (cb[i] - ca[i]) * f);
        }
    }
    STOPS[STOPS.len() - 1].1
}

/// Height ramp blended toward bare rock on steep slopes (`slope` = |∇h|).
pub fn terrain_color(t: f64, slope: f64) -> [f64; 3] {
    const ROCK: [f64; 3] = [0.47, 0.44, 0.41];
    let c = ramp(t);
    let r = ((slope - 0.55) / 0.45).clamp(0.0, 1.0);
    let r = r * r * (3.0 - 2.0 * r);
    std::array::from_fn(|i| c[i] + (ROCK[i] - c[i]) * r)
}

pub fn write_png(
    path: &Path,
    width: u32,
    height: u32,
    data: &[u8],
    color: png::ColorType,
) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(color);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(data))
        .map_err(|e| format!("{}: {e}", path.display()))
}
