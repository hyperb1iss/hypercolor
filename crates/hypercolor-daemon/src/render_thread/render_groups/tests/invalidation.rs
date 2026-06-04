use super::*;

#[test]
fn retained_scene_invalidates_when_registry_generation_changes() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let mut registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut replacement = builtin_entry(&registry, "rainbow");
    replacement.metadata.id = solid_id;
    let mut group = sample_group(4, 4);
    group.effect_id = Some(solid_id);
    group.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
    let display_group_target_fps = HashMap::new();
    let mut zones = Vec::new();

    let first = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("single group should render");
    let ProducerFrame::Surface(first_surface) = &first.scene_frame else {
        panic!("single group should publish a surface-backed scene frame");
    };

    assert!(
        runtime
            .reuse_scene(SceneDependencyKey::new(1, registry.generation()))
            .is_some(),
        "retained scene should be reusable before the registry changes"
    );

    registry.register(replacement);

    assert!(
        runtime
            .reuse_scene(SceneDependencyKey::new(1, registry.generation()))
            .is_none(),
        "registry generation changes should invalidate retained scene reuse"
    );

    let second = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        1,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("registry generation change should force a rerender");
    let ProducerFrame::Surface(second_surface) = &second.scene_frame else {
        panic!("single group should keep publishing a surface-backed scene frame");
    };

    assert_ne!(
        second_surface.get_pixel(0, 0),
        first_surface.get_pixel(0, 0),
        "same group revision should still rebuild when the registry entry changes"
    );
}

#[test]
fn retained_direct_canvas_invalidates_when_registry_generation_changes() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let mut registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut replacement = builtin_entry(&registry, "rainbow");
    replacement.metadata.id = solid_id;
    let mut group = sample_display_group(4, 4);
    group.effect_id = Some(solid_id);
    group.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
    let display_group_target_fps = HashMap::from([(group.id, 30)]);
    let mut zones = Vec::new();

    let first = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("display group should render");
    let [(_, first_frame)] = &first.group_canvases[..] else {
        panic!("display group should publish a direct surface");
    };

    registry.register(replacement);

    let second = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        10,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("registry generation change should bypass retained direct-canvas reuse");
    let [(_, second_frame)] = &second.group_canvases[..] else {
        panic!("display group should keep publishing a direct surface");
    };

    assert_ne!(
        second_frame.surface_for_test().get_pixel(0, 0),
        first_frame.surface_for_test().get_pixel(0, 0),
        "direct canvases should rerender immediately when the active registry entry changes"
    );
    assert!(
        second_frame.surface_for_test().storage_identity()
            != first_frame.surface_for_test().storage_identity()
            || second_frame.surface_for_test().generation()
                != first_frame.surface_for_test().generation(),
        "the retained direct surface should not be reused across registry generations"
    );
}

#[test]
fn retained_direct_canvas_invalidates_when_groups_revision_changes() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut group = sample_display_group(4, 4);
    group.effect_id = Some(solid_id);
    group.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
    let display_group_target_fps = HashMap::from([(group.id, 30)]);
    let mut zones = Vec::new();

    let first = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("display group should render");
    let [(_, first_frame)] = &first.group_canvases[..] else {
        panic!("display group should publish a direct surface");
    };

    let second = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        2,
        10,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("group revision change should bypass retained direct-canvas reuse");
    let [(_, second_frame)] = &second.group_canvases[..] else {
        panic!("display group should keep publishing a direct surface");
    };

    assert!(
        second_frame.surface_for_test().storage_identity()
            != first_frame.surface_for_test().storage_identity()
            || second_frame.surface_for_test().generation()
                != first_frame.surface_for_test().generation(),
        "the retained direct surface should not be reused across group revisions"
    );
}

#[test]
fn empty_display_group_does_not_reuse_previous_face_surface() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut group = sample_display_group(4, 4);
    group.effect_id = Some(solid_id);
    group.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
    let display_group_target_fps = HashMap::from([(group.id, 30)]);
    let mut zones = Vec::new();

    let first = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("display group should render the assigned face");
    let [(_, first_frame)] = &first.group_canvases[..] else {
        panic!("display group should publish a direct surface");
    };
    assert_eq!(
        first_frame.surface_for_test().get_pixel(0, 0),
        Rgba::new(0, 255, 0, 255)
    );

    group.effect_id = None;
    group.controls.clear();
    group.control_bindings.clear();
    group.preset_id = None;
    group.layers.clear();

    let cleared = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        10,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("empty display group should render a transparent shell");
    let [(_, cleared_frame)] = &cleared.group_canvases[..] else {
        panic!("empty display group should still publish a direct surface");
    };

    assert!(cleared_frame.empty_direct_shell);
    assert_eq!(
        cleared_frame.surface_for_test().get_pixel(0, 0),
        Rgba::TRANSPARENT
    );
}

#[test]
fn zero_zone_display_group_reuses_retained_surface_until_target_interval() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut group = sample_display_group(4, 4);
    group.effect_id = Some(solid_id);
    group.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
    let display_group_target_fps = HashMap::from([(group.id, 30)]);
    let mut zones = Vec::new();

    let first = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("display group should render");
    let [(_, first_frame)] = &first.group_canvases[..] else {
        panic!("display group should publish a direct surface");
    };

    let second = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        10,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("display group should reuse retained surface");
    let [(_, second_frame)] = &second.group_canvases[..] else {
        panic!("display group should keep publishing a direct surface");
    };

    let third = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        40,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("display group should rerender once its interval elapses");
    let [(_, third_frame)] = &third.group_canvases[..] else {
        panic!("display group should keep publishing a direct surface");
    };

    assert_eq!(
        first_frame.surface_for_test().storage_identity(),
        second_frame.surface_for_test().storage_identity()
    );
    assert_eq!(
        first_frame.surface_for_test().generation(),
        second_frame.surface_for_test().generation()
    );
    assert_eq!(
        first_frame.surface_for_test().get_pixel(0, 0),
        Rgba::new(0, 255, 0, 255)
    );
    assert_eq!(
        third_frame.surface_for_test().get_pixel(0, 0),
        Rgba::new(0, 255, 0, 255)
    );
    assert!(
        third_frame.surface_for_test().storage_identity()
            != second_frame.surface_for_test().storage_identity()
            || third_frame.surface_for_test().generation()
                != second_frame.surface_for_test().generation()
    );
}
