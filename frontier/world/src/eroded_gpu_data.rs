//! Packed source descriptors for the experimental GPU column evaluator.
//! The canonical source still constructs and orders both drainage graphs.
use super::{eroded_relief, fields_with_relief, region, REGION_BINS_PER_AXIS};
use crate::FbmParams;

const DIRECTORY_WORDS: usize = 7;
const REGION_WORDS: usize = 4;

pub(in crate::terrain) fn bounds_descriptors(
    seed: u64,
    params: FbmParams,
    lo: [f64; 2],
    hi: [f64; 2],
) -> Vec<f64> {
    // Two directory entries precede the arrays. All addresses count f64
    // words so the shader can bind the entire snapshot as one storage buffer.
    let mut data = vec![0.; DIRECTORY_WORDS * 2];
    let primary_width = (2400. * params.horizontal_scale / 1800.) * 4.;
    append_grid(
        &mut data,
        0,
        lo,
        hi,
        primary_width,
        REGION_BINS_PER_AXIS,
        |x, z| {
            let region = region(seed, params, x, z);
            let bins = region
                .bins
                .iter()
                .map(|bin| {
                    bin.iter()
                        .map(|&i| {
                            let r = region.reaches[i];
                            [
                                r.a.x,
                                r.a.z,
                                r.b.x,
                                r.b.z,
                                r.a.stage,
                                r.b.stage,
                                r.width,
                                r.width_b,
                                r.depth,
                                r.outer_radius(),
                                f64::from(u8::from(r.wet)),
                                f64::from(u8::from(r.lake)),
                            ]
                        })
                        .collect()
                })
                .collect();
            (region.origin, region.bin_size, bins)
        },
    );
    let (secondary_width, secondary_bins) = eroded_relief::gpu_tile_dimensions(params);
    append_grid(
        &mut data,
        DIRECTORY_WORDS,
        lo,
        hi,
        secondary_width,
        secondary_bins,
        |x, z| {
            eroded_relief::gpu_gully_tile(seed, params, x, z, |a, b| {
                fields_with_relief(seed, params, a, b, false)
            })
        },
    );
    data
}

fn append_grid(
    data: &mut Vec<f64>,
    directory: usize,
    lo: [f64; 2],
    hi: [f64; 2],
    tile_width: f64,
    bin_side: usize,
    get_region: impl Fn(f64, f64) -> ([f64; 2], f64, Vec<Vec<[f64; 12]>>),
) {
    let lo = lo.map(|a| (a / tile_width).floor() as i64);
    let hi = hi.map(|a| (a / tile_width).floor() as i64);
    let nx = (hi[0] - lo[0] + 1) as usize;
    let nz = (hi[1] - lo[1] + 1) as usize;
    let region_start = data.len();
    data[directory..directory + DIRECTORY_WORDS].copy_from_slice(&[
        lo[0] as f64,
        lo[1] as f64,
        nx as f64,
        nz as f64,
        tile_width,
        bin_side as f64,
        region_start as f64,
    ]);
    data.resize(region_start + nx * nz * REGION_WORDS, 0.);
    for (index, (x, z)) in (lo[1]..=hi[1])
        .flat_map(|z| (lo[0]..=hi[0]).map(move |x| (x, z)))
        .enumerate()
    {
        let (origin, spacing, bins) =
            get_region((x as f64 + 0.5) * tile_width, (z as f64 + 0.5) * tile_width);
        let region = region_start + REGION_WORDS * index;
        let bin_start = data.len();
        data[region..region + REGION_WORDS].copy_from_slice(&[
            origin[0],
            origin[1],
            spacing,
            bin_start as f64,
        ]);
        data.resize(bin_start + bins.len() * 2, 0.);
        for (i, bin) in bins.into_iter().enumerate() {
            data[bin_start + 2 * i] = data.len() as f64;
            data[bin_start + 2 * i + 1] = bin.len() as f64;
            for descriptor in bin {
                data.extend(descriptor);
            }
        }
    }
}
