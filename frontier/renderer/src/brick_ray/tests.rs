use super::*;
use glam::DVec3;

/// CPU f64 model of the WGSL's region-exit traversal, deliberately separate
/// from the authoritative world's unit-cell DDA and from mesh triangles.
fn trace(
    s: &BrickSnapshot,
    origin: DVec3,
    direction: DVec3,
    skip: bool,
) -> Option<(f64, u32, [i32; 3])> {
    if !origin.is_finite() || !direction.is_finite() || direction.length_squared() == 0. {
        return None;
    }
    let d = direction.normalize();
    let o = origin * 16.;
    let extent = DVec3::from_array(s.size.map(|n| n as f64));
    let mut entry = [f64::NEG_INFINITY; 3];
    let mut exit = [f64::INFINITY; 3];
    for a in 0..3 {
        if d[a] == 0. {
            if o[a] < 0. || o[a] >= extent[a] {
                return None;
            }
        } else {
            let t0 = -o[a] / d[a];
            let t1 = (extent[a] - o[a]) / d[a];
            entry[a] = t0.min(t1);
            exit[a] = t0.max(t1);
        }
    }
    let enter = entry.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let end = exit.into_iter().fold(f64::INFINITY, f64::min);
    let mut t = 0f64.max(enter);
    if t >= end {
        return None;
    }
    let forward = |p: DVec3| -> [i32; 3] {
        std::array::from_fn(|a| p[a].floor() as i32 - i32::from(d[a] < 0. && p[a] == p[a].floor()))
    };
    let mut p = forward(o + d * t);
    let mut entered_faces = 0u32;
    if enter >= 0. {
        for a in 0..3 {
            if entry[a] == enter {
                p[a] = if d[a] > 0. { 0 } else { s.size[a] as i32 - 1 };
                let face = a * 2 + usize::from(d[a] < 0.);
                entered_faces |= 1 << face;
            }
        }
    }
    for _ in 0..s.size.iter().sum::<usize>() + 8 {
        if (0..3).any(|a| p[a] < 0 || p[a] >= s.size[a] as i32) {
            return None;
        }
        let c = p.map(|v| v as usize);
        let b = c.map(|v| v / 8);
        let bi = (b[2] * s.brick_size[1] + b[1]) * s.brick_size[0] + b[0];
        let h = s.headers[bi * 2];
        let word = s.word(c);
        let exposed_entered = entered_faces & ((word >> 8) & 63);
        if exposed_entered != 0 {
            let face = exposed_entered.trailing_zeros() as usize;
            let mut normal = [0; 3];
            normal[face / 2] = if face % 2 == 0 { -1 } else { 1 };
            return Some((t / 16., word & 255, normal));
        }
        let (lo, hi) = if h & DENSE == 0 {
            let radius = if h == 0 && skip {
                s.headers[bi * 2 + 1] as i32
            } else {
                0
            };
            (
                std::array::from_fn(|a| ((b[a] as i32 - radius) * 8).max(0)),
                std::array::from_fn(|a| ((b[a] as i32 + radius + 1) * 8).min(s.size[a] as i32)),
            )
        } else {
            (p, p.map(|v| v + 1))
        };
        let next: [f64; 3] = std::array::from_fn(|a| {
            if d[a] == 0. {
                f64::INFINITY
            } else {
                ((if d[a] > 0. { hi[a] } else { lo[a] }) as f64 - o[a]) / d[a]
            }
        });
        let crossing = next.into_iter().fold(f64::INFINITY, f64::min);
        if crossing >= end {
            return None;
        }
        p = forward(o + d * crossing);
        for a in 0..3 {
            p[a] = p[a].clamp(lo[a], hi[a] - 1);
        }
        entered_faces = 0;
        for a in 0..3 {
            if next[a] == crossing {
                p[a] = if d[a] > 0. { hi[a] } else { lo[a] - 1 };
                let face = a * 2 + usize::from(d[a] < 0.);
                entered_faces |= 1 << face;
            }
        }
        t = crossing;
    }
    panic!("monotonic finite-volume crossing budget exhausted");
}

#[test]
fn wgsl_parses_and_validates() {
    let module = naga::front::wgsl::parse_str(SHADER)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(SHADER)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
}
#[test]
fn packed_cells_exactly_preserve_material_and_halo_faces() {
    let min = Cell {
        x: -19,
        y: -11,
        z: -5,
    };
    let size = [21, 19, 17];
    let sample = |p: Cell| {
        if (p.x * p.x + 3 * p.y + 7 * p.z).rem_euclid(13) < 6 {
            Material::Wood
        } else {
            Material::Air
        }
    };
    let s = BrickSnapshot::build(sample, min, size);
    for z in 0..size[2] {
        for y in 0..size[1] {
            for x in 0..size[0] {
                let p = Cell {
                    x: min.x + x as i64,
                    y: min.y + y as i64,
                    z: min.z + z as i64,
                };
                let word = s.word([x, y, z]);
                assert_eq!(word & 255, material_id(sample(p)) as u32);
                for a in 0..3 {
                    for positive in [false, true] {
                        let mut q = [p.x, p.y, p.z];
                        q[a] += if positive { 1 } else { -1 };
                        let exposed = sample(p) != Material::Air
                            && sample(Cell {
                                x: q[0],
                                y: q[1],
                                z: q[2],
                            }) == Material::Air;
                        assert_eq!(
                            word & (1 << (8 + a * 2 + usize::from(positive))) != 0,
                            exposed
                        );
                    }
                }
            }
        }
    }
}
#[test]
fn halo_uniform_solid_never_invents_crop_caps() {
    let s = BrickSnapshot::build(|_| Material::Stone, Cell::default(), [16; 3]);
    assert_eq!(s.dense_bricks, 0);
    assert!(s.dense.is_empty());
    for skip in [false, true] {
        assert!(trace(&s, DVec3::new(-1., 0.5, 0.5), DVec3::X, skip).is_none());
        assert!(trace(&s, DVec3::splat(0.5), DVec3::X, skip).is_none());
    }
}
#[test]
fn empty_distance_cubes_are_proven_empty_including_partial_bricks() {
    let s = BrickSnapshot::build(
        |p| {
            if p.x == 24 && p.y == 16 && p.z == 8 {
                Material::Stone
            } else {
                Material::Air
            }
        },
        Cell::default(),
        [35, 29, 23],
    );
    for z in 0..s.brick_size[2] {
        for y in 0..s.brick_size[1] {
            for x in 0..s.brick_size[0] {
                let i = (z * s.brick_size[1] + y) * s.brick_size[0] + x;
                if s.headers[i * 2] != 0 {
                    continue;
                }
                let r = s.headers[i * 2 + 1] as isize;
                for nz in
                    (z as isize - r).max(0)..=(z as isize + r).min(s.brick_size[2] as isize - 1)
                {
                    for ny in
                        (y as isize - r).max(0)..=(y as isize + r).min(s.brick_size[1] as isize - 1)
                    {
                        for nx in (x as isize - r).max(0)
                            ..=(x as isize + r).min(s.brick_size[0] as isize - 1)
                        {
                            let j = (nz as usize * s.brick_size[1] + ny as usize) * s.brick_size[0]
                                + nx as usize;
                            assert_eq!(s.headers[j * 2], 0);
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn boundaries_negative_world_origins_axis_zero_and_corner_ties() {
    let min = Cell {
        x: -17,
        y: -9,
        z: -21,
    };
    let sample = |p: Cell| {
        let c = [p.x - min.x, p.y - min.y, p.z - min.z];
        if c.iter().all(|&v| (8..16).contains(&v)) {
            Material::Wood
        } else {
            Material::Air
        }
    };
    let s = BrickSnapshot::build(sample, min, [25; 3]);
    let rays = [
        ([-1., 0.6, 0.6], [1., 0., 0.], 1.5, [-1, 0, 0]),
        ([2., 0.6, 0.6], [-1., 0., 0.], 1., [1, 0, 0]),
        ([-0.5, -0.5, -0.5], [1., 1., 1.], 3f64.sqrt(), [-1, 0, 0]),
        (
            [1.5, 1.5, 1.5],
            [-1., -1., -1.],
            0.5 * 3f64.sqrt(),
            [1, 0, 0],
        ),
        ([0.499, 0.6, 0.6], [1., 0., 0.], 0.001, [-1, 0, 0]),
        ([1.001, 0.6, 0.6], [-1., 0., 0.], 0.001, [1, 0, 0]),
    ];
    for (o, d, expected, normal) in rays {
        for skip in [false, true] {
            let hit = trace(&s, DVec3::from_array(o), DVec3::from_array(d), skip)
                .unwrap_or_else(|| panic!("miss o={o:?} d={d:?} skip={skip}"));
            assert!(
                (hit.0 - expected).abs() < 1e-10,
                "{o:?} {d:?}: {hit:?} expected {expected}"
            );
            assert_eq!(hit.1, 4);
            assert_eq!(hit.2, normal);
        }
    }
}
#[test]
fn fixture_cave_bridge_edits_match_canonical_rays() {
    let mut world = frontier_world::World::new(7);
    let min = Cell {
        x: -128,
        y: -32,
        z: -128,
    };
    let size = [256, 224, 256];
    // Sustained removal plus placement must be visible to new snapshots only.
    for x in 40..45 {
        for y in 64..68 {
            for z in -32..-16 {
                world.set(Cell { x, y, z }, Material::Air);
            }
        }
    }
    for y in 70..85 {
        world.set(Cell { x: 0, y, z: 0 }, Material::Wood);
    }
    let started = std::time::Instant::now();
    let s = BrickSnapshot::build(|p| world.material(p), min, size);
    eprintln!(
        "bounded fixture ray snapshot: {:.3}s, {} GPU bytes, {} dense and {} empty bricks",
        started.elapsed().as_secs_f64(),
        s.gpu_bytes(),
        s.dense_bricks,
        s.empty_bricks
    );
    let world_origin = DVec3::new(min.x as f64, min.y as f64, min.z as f64) / 16.;
    let mut checked = 0;
    for origin in [
        DVec3::new(-3.0123, 3.0234, -7.0345),
        DVec3::new(4.0123, 3.0234, -5.0345),
        DVec3::new(0.0123, 10.0234, 0.0345),
        DVec3::new(7.5123, 8.0234, -7.5345),
    ] {
        assert_eq!(
            world.material(Cell::from_meters(origin.to_array())),
            Material::Air
        );
        for y in -6..=6 {
            for x in -8..=8 {
                let d = DVec3::new(x as f64 * 0.13, y as f64 * 0.13, 1.).normalize();
                let reference =
                    frontier_world::raycast(&world, origin.to_array(), d.to_array(), 40.);
                for skip in [false, true] {
                    let got = trace(&s, origin - world_origin, d, skip);
                    // Outside the bounded mesh is deliberately not represented.
                    let reference = reference.filter(|h| {
                        let c = h.cell;
                        c.x >= min.x
                            && c.x < min.x + size[0] as i64
                            && c.y >= min.y
                            && c.y < min.y + size[1] as i64
                            && c.z >= min.z
                            && c.z < min.z + size[2] as i64
                    });
                    assert_eq!(
                        got.is_some(),
                        reference.is_some(),
                        "origin={origin:?} d={d:?} skip={skip} got={got:?} ref={reference:?}"
                    );
                    if let (Some(g), Some(r)) = (got, reference) {
                        assert!((g.0 - r.distance).abs() < 1e-9, "{g:?} {r:?}");
                        assert_eq!(g.1, material_id(r.material) as u32);
                        assert_eq!(g.2, r.normal);
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 100);
}

/// Canonical DDA advances both tied axes and picks X for its normal. The target
/// X face is hidden by a solid neighbor, but its Y face is exposed; the renderer
/// must hit the target and shade its actual exposed face rather than miss it.
/// The different normal is confined to this zero-area edge/corner convention.
#[test]
fn tied_entry_keeps_exposed_face_when_lowest_axis_face_is_hidden() {
    let mut world = frontier_world::World::new(7);
    let min = Cell {
        x: 0,
        y: 1600,
        z: 0,
    };
    let target = Cell {
        x: 1,
        y: 1601,
        z: 0,
    };
    world.set(target, Material::Wood);
    world.set(
        Cell {
            x: 0,
            y: 1601,
            z: 0,
        },
        Material::Stone,
    );
    let snapshot = BrickSnapshot::build(|c| world.material(c), min, [8; 3]);
    let local_origin = DVec3::splat(0.5) * 0.0625;
    let direction = DVec3::new(1., 1., 0.);
    let world_origin = local_origin + DVec3::new(0., 100., 0.);
    let canonical =
        frontier_world::raycast(&world, world_origin.to_array(), direction.to_array(), 1.).unwrap();
    assert_eq!(canonical.cell, target);
    assert_eq!(canonical.normal, [-1, 0, 0]);
    for skip in [false, true] {
        let hit = trace(&snapshot, local_origin, direction, skip)
            .expect("exposed tied Y face must retain occupied target");
        assert!((hit.0 - canonical.distance).abs() < 1e-12);
        assert_eq!(hit.1, material_id(Material::Wood) as u32);
        assert_eq!(hit.2, [0, -1, 0]);
    }
}
