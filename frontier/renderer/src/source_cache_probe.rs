//! Codec-only evidence. This deliberately does not claim meshing/publication speed.
#[cfg(test)]
mod tests {
    use crate::column_mesh::SampledColumn;
    use frontier_world::{Cell, FbmParams, FbmRecipe, Material, World};
    use std::time::Instant;
    fn columns(world: &World, page: [i64; 2]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for i in 0..1024 {
            let c = SampledColumn::canonical(world, page[0] + i % 32, page[1] + i / 32);
            bytes.extend_from_slice(&(i as u16).to_le_bytes());
            bytes.extend_from_slice(&c.height.to_le_bytes());
            bytes.extend(c.bands.map(|m| m as u8));
        }
        bytes
    }
    fn brick(world: &World, base: Cell) -> Vec<u8> {
        let mut bytes = Vec::new();
        for z in 0..32 {
            for y in 0..32 {
                for x in 0..32 {
                    bytes.push(world.material(Cell {
                        x: base.x + x,
                        y: base.y + y,
                        z: base.z + z,
                    }) as u8);
                }
            }
        }
        bytes
    }
    fn percentile(values: &mut [f64], p: f64) -> f64 {
        values.sort_by(f64::total_cmp);
        values[((values.len() - 1) as f64 * p).ceil() as usize]
    }
    #[test]
    #[ignore = "manual codec latency probe; serialize with other timing runs"]
    fn codec_probe() {
        let legacy = World::fbm(7, FbmParams::default()).unwrap();
        let regional = World::fbm(
            7,
            FbmParams {
                recipe: FbmRecipe::Regional,
                ..FbmParams::default()
            },
        )
        .unwrap();
        let mut fixture = World::new(7);
        let mut cases = Vec::new();
        for (name, world) in [("legacy_columns", &legacy), ("regional_columns", &regional)] {
            for p in [[-32, -32], [0, 0], [1024, -2048], [8192, 4096]] {
                cases.push((name, columns(world, p)));
            }
        }
        for (name, base) in [
            (
                "empty_brick",
                Cell {
                    x: 0,
                    y: 10000,
                    z: 0,
                },
            ),
            (
                "dense_brick",
                Cell {
                    x: 0,
                    y: -10000,
                    z: 0,
                },
            ),
            (
                "cave_brick",
                Cell {
                    x: -64,
                    y: 32,
                    z: 16,
                },
            ),
        ] {
            cases.push((name, brick(&fixture, base)));
        }
        for z in 0..32 {
            for y in 0..32 {
                for x in 0..32 {
                    if (x * 73 + y * 37 + z * 17) % 7 < 3 {
                        fixture.set(
                            Cell { x, y, z },
                            if x % 2 == 0 {
                                Material::Air
                            } else {
                                Material::Wood
                            },
                        );
                    }
                }
            }
        }
        cases.push(("edited_brick", brick(&fixture, Cell::default())));
        println!("case,codec,raw_bytes,compressed_bytes,encode_p50_us,encode_p95_us,decode_p50_us,decode_p95_us,codec_context_bytes");
        for (name, raw) in cases {
            for codec in ["none", "lz4", "zstd1"] {
                let mut output = vec![
                    0u8;
                    lz4_flex::block::get_maximum_output_size(raw.len())
                        .max(zstd::zstd_safe::compress_bound(raw.len()))
                ];
                let mut restored = vec![0u8; raw.len()];
                let mut enc = Vec::new();
                let mut dec = Vec::new();
                let mut compressor = (codec == "zstd1").then(zstd::zstd_safe::CCtx::create);
                let mut decompressor = (codec == "zstd1").then(zstd::zstd_safe::DCtx::create);
                let mut n = 0;
                for pass in 0..105 {
                    let start = Instant::now();
                    n = match codec {
                        "none" => {
                            output[..raw.len()].copy_from_slice(&raw);
                            raw.len()
                        }
                        "lz4" => lz4_flex::block::compress_into(&raw, &mut output).unwrap(),
                        _ => compressor
                            .as_mut()
                            .unwrap()
                            .compress(&mut output[..], &raw, 1)
                            .unwrap(),
                    };
                    let e = start.elapsed().as_secs_f64() * 1e6;
                    let start = Instant::now();
                    let count = match codec {
                        "none" => {
                            restored.copy_from_slice(&output[..n]);
                            n
                        }
                        "lz4" => {
                            lz4_flex::block::decompress_into(&output[..n], &mut restored).unwrap()
                        }
                        _ => decompressor
                            .as_mut()
                            .unwrap()
                            .decompress(&mut restored[..], &output[..n])
                            .unwrap(),
                    };
                    let d = start.elapsed().as_secs_f64() * 1e6;
                    assert_eq!(count, raw.len());
                    assert_eq!(restored, raw);
                    if pass >= 5 {
                        enc.push(e);
                        dec.push(d);
                    }
                }
                let context = match codec {
                    "none" => 0,
                    "lz4" => 8192 + 64,
                    _ => {
                        compressor.as_ref().unwrap().sizeof()
                            + decompressor.as_ref().unwrap().sizeof()
                    }
                };
                let e50 = percentile(&mut enc, 0.5);
                let e95 = percentile(&mut enc, 0.95);
                let d50 = percentile(&mut dec, 0.5);
                let d95 = percentile(&mut dec, 0.95);
                println!(
                    "{name},{codec},{},{n},{e50:.3},{e95:.3},{d50:.3},{d95:.3},{context}",
                    raw.len()
                );
            }
        }
    }
}
