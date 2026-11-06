use super::*;
fn source(seed: u64) -> FbmTerrain {
    FbmTerrain::new(
        seed,
        FbmParams {
            octaves: 1,
            horizontal_scale: 1300.,
            ..Default::default()
        },
    )
    .unwrap()
}
#[test]
fn canonical_negative_columns_and_filter_parity() {
    let src = source(7);
    for (x, z) in [(-33., -1.), (-32., 0.), (-1., -32.), (0., 0.), (32., 31.)] {
        for step in [1., 4.] {
            let v = sample(&src, x, z, step);
            let mut height = 0.;
            let d = step * 0.25;
            for (x, z) in [
                (x - d, z - d),
                (x + d, z - d),
                (x - d, z + d),
                (x + d, z + d),
            ] {
                let h = src.height_m(x, z);
                assert_eq!((h / frontier_world::VOXEL_SIZE).fract(), 0.);
                height += h * 0.25;
            }
            assert_eq!(v.height[0], height as f32);
        }
    }
    assert_eq!(snap([-0.001, -32.001], 1), [-1, -2]);
    assert_eq!(snap([-128., 127.999], 4), [-1, 0]);
    assert_eq!(guaranteed_radius(1), 344.);
    assert!(guaranteed_radius(4) > 1000.);
}
#[test]
fn shared_page_edges_and_existing_collar_anchors_are_identical() {
    let src = source(91);
    let mut cache = Cache::new();
    let cover = cache
        .build_region(&src, [-1, -1], 0, 1, 2, || false)
        .unwrap();
    let a = &cover.pages[0];
    let b = &cover.pages[1];
    for z in 0..ROW {
        assert_eq!(a.samples[z * ROW + SIDE], b.samples[z * ROW]);
    }
    let native = crate::terrain::TerrainData::at_center(&src, [-32., -32.]);
    for z in -64..=0 {
        for x in [-64, 0] {
            let endpoint = native.collar_sample(x as f64, z as f64).unwrap();
            let p = sample(&src, x as f64, z as f64, 1.);
            assert_eq!(endpoint.0, p.height[0] as f64);
            assert_eq!(endpoint.1, p.normal[..3]);
            assert_eq!(endpoint.2, p.color[..3]);
        }
    }
}
#[test]
fn moving_and_returned_pages_reuse_content_but_revisions_and_sources_invalidate() {
    let src = source(7);
    let mut cache = Cache::new();
    let old = cache
        .build_region(&src, [-1, -1], 0, 1, 2, || false)
        .unwrap();
    let new = cache
        .build_region(&src, [0, -1], 0, 1, 2, || false)
        .unwrap();
    assert_eq!(cache.stats.generated_pages, 6);
    assert_eq!(cache.stats.reused_pages, 2);
    assert!(Arc::ptr_eq(&old.pages[1].samples, &new.pages[0].samples));
    let returned = cache
        .build_region(&src, [-1, -1], 0, 1, 2, || false)
        .unwrap();
    assert!(Arc::ptr_eq(
        &old.pages[0].samples,
        &returned.pages[0].samples
    ));
    let changed = cache
        .build_region(&src, [-1, -1], 1, 1, 2, || false)
        .unwrap();
    assert!(!Arc::ptr_eq(
        &old.pages[0].samples,
        &changed.pages[0].samples
    ));
    assert_eq!(cache.pages.len(), 4);
    let other = cache
        .build_region(&source(8), [-1, -1], 1, 1, 2, || false)
        .unwrap();
    assert!(!Arc::ptr_eq(
        &changed.pages[0].samples,
        &other.pages[0].samples
    ));
    assert!(!other.matches([-32., -32.], 0, 0, 1));
}
#[test]
fn cancellation_cannot_publish_partial_cover_and_cache_has_hard_page_limit() {
    let src = source(7);
    let mut cache = Cache::new();
    assert!(cache.build_region(&src, [0, 0], 0, 1, 2, || true).is_none());
    assert_eq!(cache.stats.cancelled_jobs, 1);
    // Admission evicts a whole completed page; retained caller Arcs remain valid.
    let p = Page {
        key: Key {
            tile: [0, 0],
            source_revision: 0,
            step: 1,
            source: [0; 8],
        },
        samples: vec![sample(&src, 0., 0., 1.); SAMPLES].into(),
    };
    for i in 0..CACHE_PAGES {
        let mut p = p.clone();
        p.key.tile = [i as i64 + 10, 10];
        cache.pages.push_back(p);
    }
    cache.source = Some((
        src.seed(),
        src.params(),
        src.source_version(),
        src.appearance_version(),
        0,
    ));
    cache.build_region(&src, [0, 0], 0, 1, 2, || false).unwrap();
    assert_eq!(cache.pages.len(), CACHE_PAGES);
}
#[test]
fn page_shader_validates() {
    let module = naga::front::wgsl::parse_str(&shader_source()).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn native_camera_epochs_reuse_cpu_pages_and_gpu_slots_but_source_revisions_invalidate() {
    let src = source(7);
    let mut cache = Cache::new();
    let initial = cache
        .build_region(&src, [-1, -1], 0, 1, 2, || false)
        .unwrap()
        .for_native_epoch(1);
    let slots: Vec<Option<Key>> = initial.pages.iter().map(|p| Some(p.key)).collect();
    for native_epoch in [2, 3, 17] {
        let cover = cache
            .build_region(&src, [-1, -1], 0, 1, 2, || false)
            .unwrap()
            .for_native_epoch(native_epoch);
        assert!(cover.matches([-32., -32.], 0, native_epoch, 1));
        assert!(!cover.matches([-32., -32.], 0, 1, 1));
        for (a, b) in initial.pages.iter().zip(&cover.pages) {
            assert!(Arc::ptr_eq(&a.samples, &b.samples));
            assert_eq!(a.key, b.key);
            assert!(
                cached_slot(&slots, b.key).is_some(),
                "native epoch must cause no source-page upload"
            );
        }
    }
    assert_eq!(cache.stats.generated_pages, 4);
    assert_eq!(cache.stats.reused_pages, 12);
    let edited = cache
        .build_region(&src, [-1, -1], 1, 1, 2, || false)
        .unwrap()
        .for_native_epoch(18);
    assert!(edited.matches([-32., -32.], 1, 18, 1));
    for page in &edited.pages {
        assert!(cached_slot(&slots, page.key).is_none());
    }
    assert_eq!(cache.stats.generated_pages, 8);
}

#[test]
fn source_refresh_publishes_with_current_native_owner_without_a_new_camera_epoch() {
    let src = source(7);
    let mut cache = Cache::new();
    let initial = cache
        .build_region(&src, [-1, -1], 0, 1, 2, || false)
        .unwrap()
        .for_native_epoch(17);
    let edited = cache
        .build_region(&src, [-1, -1], 1, 1, 2, || false)
        .unwrap()
        .for_native_epoch(17);
    let active = ([-32., -32.], 17);
    assert!(edited.can_publish(1, 1, active, active));
    assert!(!initial.can_publish(1, 1, active, active));
    assert!(!edited.can_publish(1, 1, ([-32., -32.], 16), active));
    assert!(!edited.can_publish(1, 1, ([0., -32.], 17), active));
    assert!(!edited.can_publish(1, 1, active, ([-32., -32.], 18)));
}
