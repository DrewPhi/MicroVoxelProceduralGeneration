use microvoxel::{
    character::{CharacterRecipe, Clip, Pose},
    generation::FlatTerrain,
    materials::{Appearance, MaterialId, MaterialRegistry, Voxel},
    physics::{camera_boom, VoxelShape},
    player::{Player, PlayerInput, TICK_SECONDS},
    CellPos, Edit, World,
};
fn flat() -> World<FlatTerrain> {
    World::new(FlatTerrain {
        height: 8,
        water: 8,
        seed: 42,
    })
}
fn block() -> Voxel {
    Voxel::new(MaterialId::ROCK, Appearance(0))
}
fn simulate(world: &World<FlatTerrain>, player: &mut Player, ticks: usize, input: PlayerInput) {
    for _ in 0..ticks {
        player.advance(world, TICK_SECONDS, input).unwrap();
        assert!(
            player.clear(world),
            "player overlaps terrain at {:?}",
            player.root
        );
    }
}
#[test]
fn gravity_landing_jump_and_held_jump_are_grid_aligned() {
    let world = flat();
    let mut p = Player::new(42, CellPos::new(0, 80, 0)).unwrap();
    simulate(&world, &mut p, 180, PlayerInput::default());
    assert_eq!(p.root.y, 8);
    assert!(p.grounded);
    let mut peak = 8;
    for _ in 0..150 {
        simulate(
            &world,
            &mut p,
            1,
            PlayerInput {
                jump: true,
                ..Default::default()
            },
        );
        peak = peak.max(p.root.y);
    }
    assert!((18..=25).contains(&peak));
    assert_eq!(p.root.y, 8);
    assert!(p.grounded);
}
#[test]
fn fixed_ticks_agree_across_render_rates() {
    let world = flat();
    let mut outputs = Vec::new();
    for fps in [30, 60, 120, 144] {
        let mut p = Player::new(42, CellPos::new(0, 8, 0)).unwrap();
        for _ in 0..fps * 2 {
            p.advance(
                &world,
                1. / fps as f64,
                PlayerInput {
                    forward: 1.,
                    yaw: 0.,
                    ..Default::default()
                },
            )
            .unwrap();
        }
        assert!(p.clear(&world));
        assert_eq!(p.ticks, 120);
        outputs.push((p.root, p.pose));
    }
    assert!(outputs.windows(2).all(|v| v[0] == v[1]));
    assert_eq!(outputs[0].0.x, 96);
}
#[test]
fn walls_stop_sprints_and_diagonals_slide_without_tunneling() {
    let mut world = flat();
    let mut edits = Vec::new();
    for z in -160..160 {
        for y in 8..50 {
            edits.push(Edit::set(CellPos::new(20, y, z), block()));
        }
    }
    world.apply(&edits).unwrap();
    let mut p = Player::new(42, CellPos::new(0, 8, 0)).unwrap();
    simulate(
        &world,
        &mut p,
        120,
        PlayerInput {
            forward: 1.,
            strafe: 1.,
            sprint: true,
            ..Default::default()
        },
    );
    assert!(p.root.x < 20 && p.root.z > 60, "{:?}", p.root);
    let recipe = CharacterRecipe::default();
    let mut registry = MaterialRegistry::default();
    let palette = recipe.register_palette(&mut registry).unwrap();
    let f = recipe
        .generate(16, Pose::new(Clip::Idle, 0), &palette)
        .unwrap();
    let shape = VoxelShape::from_frame(&f);
    let end = shape.sweep_axis(&world, CellPos::new(0, 8, 0), 0, 1000);
    assert!(end.x < 20);
    assert!(shape.clear(&world, end));
}
#[test]
fn small_steps_climb_and_low_ceilings_stop_upward_motion() {
    let mut world = flat();
    let mut edits = Vec::new();
    for x in 16..34 {
        for z in -12..12 {
            for y in 8..10 {
                edits.push(Edit::set(CellPos::new(x, y, z), block()));
            }
        }
    }
    world.apply(&edits).unwrap();
    let mut p = Player::new(42, CellPos::new(0, 8, 0)).unwrap();
    let mut peak = 8;
    for _ in 0..100 {
        simulate(
            &world,
            &mut p,
            1,
            PlayerInput {
                forward: 1.,
                ..Default::default()
            },
        );
        peak = peak.max(p.root.y);
    }
    assert!(peak >= 10);
    assert!(p.root.x > 34);
    let mut ceiling = flat();
    let mut edits = Vec::new();
    for x in -12..12 {
        for z in -12..12 {
            edits.push(Edit::set(CellPos::new(x, 40, z), block()));
        }
    }
    ceiling.apply(&edits).unwrap();
    let mut p = Player::new(42, CellPos::new(0, 8, 0)).unwrap();
    simulate(&ceiling, &mut p, 1, Default::default());
    simulate(
        &ceiling,
        &mut p,
        90,
        PlayerInput {
            jump: true,
            ..Default::default()
        },
    );
    assert_eq!(p.root.y, 8);
}
#[test]
fn a_pose_cannot_put_the_held_arm_into_a_wall() {
    let mut world = flat();
    let recipe = CharacterRecipe::default();
    let mut registry = MaterialRegistry::default();
    let palette = recipe.register_palette(&mut registry).unwrap();
    let old = recipe
        .generate(16, Pose::new(Clip::Idle, 0), &palette)
        .unwrap();
    let held = recipe
        .generate(16, Pose::new(Clip::Idle, 0).holding_arm(true), &palette)
        .unwrap();
    let cell = held
        .occupied()
        .find(|(p, _)| old.voxel(*p).is_air() && p[2] < -3)
        .unwrap()
        .0;
    world
        .apply(&[Edit::set(
            CellPos::new(cell[0] as i64, 8 + cell[1] as i64, cell[2] as i64),
            block(),
        )])
        .unwrap();
    let mut p = Player::new(42, CellPos::new(0, 8, 0)).unwrap();
    simulate(
        &world,
        &mut p,
        30,
        PlayerInput {
            yaw: -std::f64::consts::FRAC_PI_2,
            arm_forward: true,
            ..Default::default()
        },
    );
    assert!(!p.pose.arm_forward);
    assert!(p.clear(&world));
}
#[test]
fn collision_uses_occupied_cells_and_camera_checks_its_entire_volume() {
    let mut world = World::new(FlatTerrain {
        height: 0,
        water: 0,
        seed: 42,
    });
    world
        .apply(&[Edit::set(CellPos::new(1, 1, 0), block())])
        .unwrap();
    let shape = VoxelShape::new(vec![[0, 0, 0], [2, 0, 0]]);
    assert_eq!(
        shape.move_one(&world, CellPos::default(), 1, 1),
        Some(CellPos::new(0, 1, 0))
    );
    world
        .apply(&[Edit::set(CellPos::new(5, 0, 0), block())])
        .unwrap();
    let end = camera_boom(&world, CellPos::new(0, 2, 0), [12, 0, 0], 2);
    assert!(end.x < 5);
}
#[test]
fn water_is_ground_and_spawn_checks_the_whole_model() {
    let world = World::new(FlatTerrain {
        height: 8,
        water: 16,
        seed: 42,
    });
    let mut p = Player::new(42, CellPos::default()).unwrap();
    assert!(p.place_above(&world, 40, 40));
    simulate(&world, &mut p, 60, PlayerInput::default());
    assert_eq!(p.root.y, 16);
}

#[test]
fn unknown_space_blocks_a_swept_body() {
    use microvoxel::generation::{Generator, GeneratorIdentity};
    struct Edge;
    impl Generator for Edge {
        fn identity(&self) -> GeneratorIdentity {
            FlatTerrain {
                height: 0,
                water: 0,
                seed: 1,
            }
            .identity()
        }
        fn voxel(&self, p: CellPos) -> Option<Voxel> {
            if p.x < 5 {
                Some(Voxel::AIR)
            } else {
                None
            }
        }
    }
    let world = World::new(Edge);
    let shape = VoxelShape::new(vec![[0, 0, 0], [1, 0, 0]]);
    assert_eq!(
        shape.sweep_axis(&world, CellPos::default(), 0, 100),
        CellPos::new(3, 0, 0)
    );
}

#[test]
fn arm_changes_ignore_unchanged_feet_but_sweep_intermediate_space() {
    let mut world = flat();
    let mut p = Player::new(42, CellPos::new(0, 8, 0)).unwrap();
    // A small rock fits between the calves and must not freeze a hand change.
    let gap = [0, 4, 0];
    assert!(p.frame().voxel(gap).is_air());
    world
        .apply(&[Edit::set(CellPos::new(0, 12, 0), block())])
        .unwrap();
    simulate(
        &world,
        &mut p,
        30,
        PlayerInput {
            yaw: -std::f64::consts::FRAC_PI_2,
            arm_forward: true,
            ..Default::default()
        },
    );
    assert!(p.pose.arm_forward);
    let old = VoxelShape::new(vec![[0, 0, 0]]);
    let new = VoxelShape::new(vec![[2, 0, 0]]);
    world
        .apply(&[Edit::set(CellPos::new(1, 20, 0), block())])
        .unwrap();
    let root = CellPos::new(0, 20, 0);
    assert!(old.clear(&world, root) && new.clear(&world, root));
    assert!(!old.transition_clear(&new, &world, root));
}

#[test]
fn head_leads_idle_turns_and_body_follows_when_settled_or_walking() {
    let world = flat();
    let mut p = Player::new(42, CellPos::new(0, 8, 0)).unwrap();
    let forward = -std::f64::consts::FRAC_PI_2;
    simulate(
        &world,
        &mut p,
        40,
        PlayerInput {
            yaw: forward,
            ..Default::default()
        },
    );
    let original = p.pose.heading;
    simulate(
        &world,
        &mut p,
        1,
        PlayerInput {
            yaw: forward - 30f64.to_radians(),
            pitch: 30f64.to_radians(),
            ..Default::default()
        },
    );
    assert_eq!(p.pose.heading, original);
    assert!(p.pose.head_yaw > 0);
    assert_eq!(p.pose.head_pitch, 4);
    simulate(
        &world,
        &mut p,
        60,
        PlayerInput {
            yaw: forward - 30f64.to_radians(),
            ..Default::default()
        },
    );
    assert_ne!(p.pose.heading, original);
    assert!(p.pose.head_yaw.abs() <= 1);
    simulate(
        &world,
        &mut p,
        12,
        PlayerInput {
            forward: 1.,
            yaw: forward - 90f64.to_radians(),
            ..Default::default()
        },
    );
    assert_eq!(p.pose.heading, 4);
    assert!(p.pose.head_yaw.abs() <= 1);
}

#[test]
fn steady_diagonal_walk_does_not_wobble_the_torso_between_grid_steps() {
    let world = flat();
    let mut p = Player::new(42, CellPos::new(0, 8, 0)).unwrap();
    let input = PlayerInput {
        forward: 1.,
        yaw: 0.37,
        ..Default::default()
    };
    simulate(&world, &mut p, 30, input);
    let heading = p.pose.heading;
    for _ in 0..120 {
        simulate(&world, &mut p, 1, input);
        assert_eq!(p.pose.heading, heading);
    }
}
