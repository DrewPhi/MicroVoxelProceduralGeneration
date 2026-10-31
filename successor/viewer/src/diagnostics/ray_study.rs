//! Opt-in native-voxel representation study. Production selection is unchanged.
use super::ray_study_gpu::{Bench, Hit, Ray};
use crate::{gpu::Gpu, runtime::Runtime, settings::Settings};
use microvoxel::{
    generation::{FlatTerrain, Generator, GeneratorIdentity, Run, Terrain},
    materials::{Appearance, MaterialId, Voxel},
    render::{
        lod::{MeshKey, TileKey},
        octree::{self, Compact, DagBuilder, HybridBrick, Occupancy, Tree},
        ray::{self, Brick},
        terrain::Sampler,
    },
    CellPos, Edit, World,
};
use std::{
    fmt::Write as _,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
struct Case {
    name: &'static str,
    brick: Brick,
    svo: Compact,
    dag: Compact,
    attributes: Vec<u32>,
    height: u32,
    build: [f64; 3],
    raw_nodes: usize,
}
fn local(mut brick: Brick) -> Brick {
    let [x, _, z] = brick.bounds[0];
    for node in brick.words[..brick.nodes * 8].chunks_exact_mut(8) {
        node[0] -= x as u32;
        node[4] -= x as u32;
        node[2] -= z as u32;
        node[6] -= z as u32;
    }
    brick.bounds[1][0] -= x;
    brick.bounds[1][2] -= z;
    brick.bounds[0][0] = 0;
    brick.bounds[0][2] = 0;
    brick
}
fn key(x: u32, z: u32, revision: u64) -> MeshKey {
    MeshKey {
        corner_levels: None,
        tile: TileKey { level: 13, x, z },
        neighbors: [13; 4],
        revision,
    }
}
fn make<G: Generator>(
    name: &'static str,
    world: Arc<World<G>>,
    key: MeshKey,
) -> microvoxel::Result<Case> {
    let mut sampler = Sampler::new(world);
    let t = Instant::now();
    let brick = local(sampler.ray_brick(key, || false)?);
    let column_ms = t.elapsed().as_secs_f64() * 1000.;
    let occ = Occupancy::from_brick(&brick)?;
    let t = Instant::now();
    let tree = Tree::build(&occ);
    let svo = tree.compact();
    let svo_ms = t.elapsed().as_secs_f64() * 1000.;
    let t = Instant::now();
    let mut builder = DagBuilder::default();
    let root = builder.insert(&tree);
    let dag = (*builder.snapshot(root, tree.height)).clone();
    let dag_ms = t.elapsed().as_secs_f64() * 1000.;
    let attributes = octree::attributes(&brick);
    Ok(Case {
        name,
        brick,
        svo,
        dag,
        attributes,
        height: tree.height,
        build: [column_ms, svo_ms, dag_ms],
        raw_nodes: tree.nodes.len(),
    })
}
struct Checker;
impl Generator for Checker {
    fn identity(&self) -> GeneratorIdentity {
        FlatTerrain {
            height: 128,
            water: 128,
            seed: 42,
        }
        .identity()
    }
    fn voxel(&self, p: CellPos) -> Option<Voxel> {
        Some(if (0..128).contains(&p.y) && (p.x + p.y + p.z) & 1 == 0 {
            Voxel::new(MaterialId::ROCK, Appearance(0))
        } else {
            Voxel::AIR
        })
    }
    fn appearance_at(&self, _: CellPos, _: MaterialId) -> Appearance {
        Appearance(0)
    }
    fn runs(&self, x: i64, z: i64, r: std::ops::Range<i64>) -> Option<Vec<Run>> {
        let mut runs = Vec::new();
        for y in r.start.max(0)..r.end.min(128) {
            if (x + y + z) & 1 == 0 {
                runs.push(Run {
                    start: y,
                    end: y + 1,
                    material: MaterialId::ROCK,
                });
            } else {
                runs.push(Run {
                    start: y,
                    end: y + 1,
                    material: MaterialId::AIR,
                });
            }
        }
        if r.end > 128 {
            runs.push(Run {
                start: 128,
                end: r.end,
                material: MaterialId::AIR,
            });
        }
        Some(runs)
    }
}
fn rays(case: &Case) -> Vec<Ray> {
    let high = case.brick.bounds[1][1] as f32;
    let data = &case.brick.words;
    let mut tops = Vec::new();
    for node in data[..case.brick.nodes * 8].chunks_exact(8) {
        if node[3] & ray::LEAF != 0 {
            tops.push(node[5]);
        }
    }
    tops.sort_unstable();
    let mid = tops[tops.len() / 2] as f32;
    (0..4096)
        .map(|i| {
            let x = (i % 64) as f32 * 0.625 - 4. + 0.231;
            let z = (i / 64) as f32 * 0.625 - 4. + 0.413;
            let family = i % 4;
            let (origin, direction) = match family {
                0 => ([x, high + 32., z], [0., -1., 0.]),
                1 => (
                    [-8., mid + ((i % 23) as f32 - 11.) * 0.7 + 0.317, z],
                    [1., -0.13, 0.037],
                ),
                2 => ([-8., high + 40., z], [1., 0.01, 0.031]),
                _ if case.name == "cave" => (
                    [16.317, 64.231, 16.413],
                    [x - 16., (i % 17) as f32 - 8. + 0.17, z - 16.],
                ),
                _ => ([x, high + 16., -8.], [0.017, -0.53, 1.]),
            };
            let length = direction.iter().map(|v| v * v).sum::<f32>().sqrt();
            Ray {
                origin: [origin[0], origin[1], origin[2], 0.],
                direction: [
                    direction[0] / length,
                    direction[1] / length,
                    direction[2] / length,
                    0.,
                ],
            }
        })
        .collect()
}
fn validate(
    case: &Case,
    rays: &[Ray],
    hits: &[Hit],
    reference: Option<&[Hit]>,
    attributes: bool,
) -> microvoxel::Result<()> {
    for (i, (ray, hit)) in rays.iter().zip(hits).enumerate() {
        if let Some(reference) = reference {
            let expected = reference[i];
            if hit.hit != expected.hit
                || (hit.hit != 0
                    && (hit.cell != expected.cell || (attributes && hit.color != expected.color)))
            {
                return Err(format!(
                    "{} ray {i}: structure mismatch {hit:?}, reference {expected:?}",
                    case.name
                )
                .into());
            }
        } else if i % 4 == 0 || i % 31 == 0 {
            let origin = std::array::from_fn(|j| ray.origin[j] as f64);
            let direction = std::array::from_fn(|j| ray.direction[j] as f64);
            let expected = ray::trace(&case.brick, origin, direction);
            if hit.hit != u32::from(expected.is_some()) {
                return Err(format!("{} ray {i}: CPU/GPU occupancy mismatch", case.name).into());
            }
            if let Some((cell, color, _)) = expected {
                let rgb = hit.color.to_le_bytes();
                if hit.cell.map(i64::from) != cell || (attributes && rgb[..3] != color) {
                    return Err(format!(
                        "{} ray {i}: CPU/GPU cell/color mismatch {hit:?} / {cell:?} {color:?}",
                        case.name
                    )
                    .into());
                }
            }
        }
    }
    Ok(())
}
fn sharing(world: Arc<World<Terrain>>, report: &mut String) -> microvoxel::Result<()> {
    writeln!(
        report,
        "\nSubtree sharing across 16 adjacent native terrain bricks:"
    )?;
    let mut sampler = Sampler::new(world);
    let mut pool = DagBuilder::default();
    let mut raw = 0;
    let mut local = 0;
    let mut attributes = 0;
    let t = Instant::now();
    for i in 0..16 {
        let brick = sampler.ray_brick(key(2125 + i % 4, 3125 + i / 4, 0), || false)?;
        let tree = Tree::build(&Occupancy::from_brick(&brick)?);
        raw += tree.nodes.len();
        let mut single = DagBuilder::default();
        single.insert(&tree);
        local += single.nodes.len();
        pool.insert(&tree);
        attributes += octree::attributes(&brick).len() * 4;
    }
    writeln!(report,"SVO internal nodes {raw}; independently deduplicated nodes {local}; shared DAG-pool nodes {}; attributes {attributes} bytes; build+analysis {:.3} ms. Exact subtree equality, not whole-brick fingerprints.",pool.nodes.len(),t.elapsed().as_secs_f64()*1000.)?;
    Ok(())
}
fn edit_study(report: &mut String) -> microvoxel::Result<()> {
    for freeze_every_edit in [false, true] {
        let mut world = World::new(FlatTerrain {
            height: 128,
            water: 128,
            seed: 42,
        });
        let original = Sampler::new(Arc::new(World::new(FlatTerrain {
            height: 128,
            water: 128,
            seed: 42,
        })))
        .ray_brick(key(1, 1, 0), || false)?;
        let mut hybrid = HybridBrick::editable(Arc::new(original));
        hybrid.freeze()?;
        let old = hybrid.frozen.clone().unwrap();
        let t = Instant::now();
        for i in 0..16 {
            world.apply(&[Edit::set(
                CellPos::new(40 + i % 8, 63 + i / 8, 40),
                Voxel::AIR,
            )])?;
            // Reconstruct a world snapshot from sparse edits. The benchmark does not
            // introduce a concurrent mutable-world protocol into the running game.
            let mut snapshot = World::new(FlatTerrain {
                height: 128,
                water: 128,
                seed: 42,
            });
            let edits: Vec<_> = (0..=i)
                .map(|j| Edit::set(CellPos::new(40 + j % 8, 63 + j / 8, 40), Voxel::AIR))
                .collect();
            snapshot.apply(&edits)?;
            let revision = snapshot.revision();
            let brick =
                Sampler::new(Arc::new(snapshot)).ray_brick(key(1, 1, revision), || false)?;
            hybrid.replace_edited(Arc::new(brick));
            if freeze_every_edit {
                hybrid.freeze()?;
            }
        }
        let edits_ms = t.elapsed().as_secs_f64() * 1000.;
        let t = Instant::now();
        if !freeze_every_edit {
            hybrid.freeze()?;
        }
        assert!(old.occupied([8, 63, 8]));
        assert!(!hybrid.frozen.as_ref().unwrap().occupied([8, 63, 8]));
        writeln!(report,"Hybrid edits: freeze_each={freeze_every_edit}, 16 edits + column snapshot rebuilds {edits_ms:.3} ms, final idle freeze {:.3} ms; old immutable snapshot preserved.",t.elapsed().as_secs_f64()*1000.)?;
    }
    Ok(())
}
fn frame_profile(world: Arc<World<Terrain>>, report: &mut String) -> microvoxel::Result<()> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let gpu = pollster::block_on(Gpu::new(&instance, None, 1280, 720))?;
    let mut rt = Runtime::new(gpu, world, 768, 0)?;
    rt.apply_settings(Settings {
        ray: true,
        quality: 75.,
        native_radius: 6.,
        memory_mib: 256,
        antialias: false,
        transitions: false,
        ..Default::default()
    });
    rt.prepare_still_capture();
    "4250,200,6250,125,-5.14"
        .parse::<crate::capture::Viewpoint>()?
        .apply(&mut rt)?;
    rt.gpu.hud_visible = false;
    crate::wait_for_cover(&mut rt, Instant::now() + Duration::from_secs(120))?;
    let texture = rt.gpu.offscreen();
    let view = texture.create_view(&Default::default());
    let cover = rt.cover.clone();
    writeln!(report,"\nWhole-frame stage profile: 1280x720, fixed valley cover ({} tiles), AA/fades disabled, no actor; synchronized CPU+GPU time, not hardware GPU timestamps:",cover.len())?;
    for stage in [0, 1, 2, 1, 0] {
        rt.gpu.study_ray_stage(stage);
        for _ in 0..5 {
            rt.gpu.render(&view, &rt.camera, &cover, None);
            rt.gpu.device.poll(wgpu::Maintain::Wait);
        }
        let mut times = Vec::new();
        for _ in 0..30 {
            let t = Instant::now();
            rt.gpu.render(&view, &rt.camera, &cover, None);
            rt.gpu.device.poll(wgpu::Maintain::Wait);
            times.push(t.elapsed().as_secs_f64() * 1000.);
        }
        times.sort_by(f64::total_cmp);
        writeln!(
            report,
            "stage {stage} (0=full,1=geometry+depth,2=sky/setup): median {:.3} ms, p95 {:.3} ms",
            times[15], times[28]
        )?;
    }
    Ok(())
}
pub fn run(world: Arc<World<Terrain>>) -> microvoxel::Result<()> {
    if world.spec() != microvoxel::WorldSpec::default() {
        return Err("ray-study currently uses default-grid fixtures; use terrain-quality for scale comparisons".into());
    }
    let directory = Path::new("output/ray_study");
    std::fs::create_dir_all(directory)?;
    let mut report=String::from("Voxel representation study\nNative 32x32 columns, same occupancy and uncompressed existing attributes.\nSVO/DAG are opt-in immutable experiments; editable world storage and production renderer remain unchanged.\n");
    frame_profile(world.clone(), &mut report)?;
    let mut cave = World::new(FlatTerrain {
        height: 128,
        water: 128,
        seed: 42,
    });
    let mut edits = Vec::new();
    for z in 40..56 {
        for x in 40..56 {
            for y in 48..80 {
                edits.push(Edit::set(CellPos::new(x, y, z), Voxel::AIR));
            }
        }
    }
    cave.apply(&edits)?;
    let cases = vec![
        make(
            "flat",
            Arc::new(World::new(FlatTerrain {
                height: 30000,
                water: 30000,
                seed: 42,
            })),
            key(1, 1, 0),
        )?,
        make("grass", world.clone(), key(2125, 3125, 0))?,
        make("cliff", world.clone(), key(625, 4375, 0))?,
        make("cave", Arc::new(cave), key(1, 1, 1))?,
        make(
            "checker_stress",
            Arc::new(World::new(Checker)),
            key(1, 1, 0),
        )?,
    ];
    let bench = Bench::new()?;
    writeln!(
        report,
        "\nCompute adapter: {}; hardware timestamps: {}",
        bench.adapter, bench.timestamps
    )?;
    let mut csv=String::from("scene,structure,attributes,gpu_ms_p50,gpu_ms_p95,sync_ms_p50,sync_ms_p95,geometry_bytes,attribute_bytes,total_bytes,visits_per_ray,aabb_tests_per_ray,logical_words_per_ray,hits\n");
    for case in cases {
        writeln!(report,"\n{}: column build {:.3} ms, additional SVO conversion/packing {:.3} ms, additional DAG dedup/packing {:.3} ms",case.name,case.build[0],case.build[1],case.build[2])?;
        let color_bytes = (case.brick.words.len() - (case.brick.nodes + case.brick.runs) * 8) * 4;
        writeln!(report,"Current bytes: BVH {}, run headers {}, colors {}; SVO {} nodes/{} bytes; DAG {} nodes/{} bytes; alternate attributes (including direct column index) {} bytes.",case.brick.nodes*32,case.brick.runs*32,color_bytes,case.raw_nodes,case.svo.bytes(),case.dag.nodes,case.dag.bytes(),case.attributes.len()*4)?;
        let rays = rays(&case);
        let mut reference = None;
        for (mode, name, tree) in [
            (0, "column", None),
            (1, "svo", Some(&case.svo)),
            (2, "dag", Some(&case.dag)),
        ] {
            for with_attributes in [false, true] {
                let source = if mode == 0 {
                    &case.brick.words
                } else {
                    &case.attributes
                };
                let words = tree.map_or(&[][..], |t| t.words.as_slice());
                let params = [
                    mode,
                    tree.map_or(0, |t| t.root),
                    case.height,
                    u32::from(with_attributes),
                    rays.len() as u32,
                    case.brick.bounds[1][1] as u32,
                    0,
                    0,
                ];
                let result = bench.run(source, words, params, &rays)?;
                // Geometry-only column data establishes hit/cell reference; colored column
                // data then becomes the attribute reference for both alternate structures.
                validate(
                    &case,
                    &rays,
                    &result.hits,
                    reference.as_deref(),
                    with_attributes && mode != 0,
                )?;
                if mode == 0 {
                    validate(&case, &rays, &result.hits, None, with_attributes)?;
                    reference = Some(result.hits.clone());
                }
                let geometry = tree.map_or(case.brick.nodes * 32, |t| t.bytes());
                let attrs = if mode == 0 {
                    (case.brick.words.len() - case.brick.nodes * 8) * 4
                } else {
                    case.attributes.len() * 4
                };
                let average = |f: fn(&Hit) -> u32| {
                    result.hits.iter().map(|h| f(h) as f64).sum::<f64>() / rays.len() as f64
                };
                let hits = result.hits.iter().filter(|h| h.hit != 0).count();
                writeln!(
                    csv,
                    "{},{},{},{:.6},{:.6},{:.6},{:.6},{},{},{},{:.3},{:.3},{:.3},{}",
                    case.name,
                    name,
                    with_attributes,
                    result.gpu[0],
                    result.gpu[1],
                    result.wall[0],
                    result.wall[1],
                    geometry,
                    attrs,
                    geometry + attrs,
                    average(|h| h.visits),
                    average(|h| h.tests),
                    average(|h| h.loads),
                    hits
                )?;
                println!("{} {name} attributes={with_attributes}: GPU {:.3} ms, nodes {:.1}, logical words {:.1}; validated {}/{} rays",case.name,result.gpu[0],average(|h|h.visits),average(|h|h.loads),rays.len(),rays.len());
            }
        }
        std::fs::write(directory.join("metrics.csv"), &csv)?;
        std::fs::write(directory.join("report.txt"), &report)?;
    }
    sharing(world, &mut report)?;
    edit_study(&mut report)?;
    writeln!(report,"\nCompute results include counters and output writes. Logical words are instrumented source-level reads, not measured DRAM traffic/cache misses. Geometry-only mode still binds complete input buffers. SVO construction currently converts a column snapshot; reported conversion is additional, not an optimized direct generator. Checkerboard is a synthetic stress case. Detailed metrics: metrics.csv.\n")?;
    std::fs::write(directory.join("report.txt"), &report)?;
    print!("{report}");
    Ok(())
}
