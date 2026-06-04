use super::*;

#[test]
fn clear_inactive_groups_releases_cached_group_state() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let group = sample_group(4, 4);
    let display_group = sample_display_group(4, 4);
    let display_target = display_group
        .display_target
        .as_ref()
        .expect("display group should have a target")
        .clone();
    let display_route = sample_display_route(display_target.device_id);
    let group_canvas_frame = sample_group_canvas_frame(&display_target, true);
    runtime.target_canvases.insert(group.id, Canvas::new(4, 4));
    runtime
        .spatial_engines
        .insert(group.id, SpatialEngine::new(group.layout.clone()));
    runtime.retain_materialized_group_frame(
        display_group.id,
        100,
        SceneDependencyKey::new(1, 1),
        &display_target,
        &display_route,
        false,
        &group_canvas_frame,
    );
    runtime.reconciled_dependency_key = Some(SceneDependencyKey::new(1, 1));

    assert!(runtime.has_inactive_group_resources());

    runtime.clear_inactive_groups();

    assert!(!runtime.has_inactive_group_resources());
    assert!(runtime.combined_led_layout.zones.is_empty());
}

#[test]
fn materialized_group_reuse_obeys_cadence_and_route_identity() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let group = sample_display_group(4, 4);
    let display_target = group
        .display_target
        .as_ref()
        .expect("display group should have a target")
        .clone();
    let display_route = sample_display_route(display_target.device_id);
    let dependency_key = SceneDependencyKey::new(1, 1);
    let group_canvas_frame = sample_group_canvas_frame(&display_target, true);

    runtime.retain_materialized_group_frame(
        group.id,
        100,
        dependency_key,
        &display_target,
        &display_route,
        false,
        &group_canvas_frame,
    );

    let reused = runtime
        .reuse_retained_materialized_group_frame(
            group.id,
            120,
            Some(30),
            dependency_key,
            &display_target,
            &display_route,
            false,
        )
        .expect("retained materialized frame should be reused within cadence");
    assert_eq!(reused.display_target, group_canvas_frame.display_target);

    assert!(
        runtime
            .reuse_retained_materialized_group_frame(
                group.id,
                120,
                Some(30),
                SceneDependencyKey::new(2, 1),
                &display_target,
                &display_route,
                false,
            )
            .is_none()
    );
    assert!(
        runtime
            .reuse_retained_materialized_group_frame(
                group.id,
                140,
                Some(30),
                dependency_key,
                &display_target,
                &display_route,
                false,
            )
            .is_none()
    );

    let mut changed_route = display_route.clone();
    changed_route.width += 1;
    assert!(
        runtime
            .reuse_retained_materialized_group_frame(
                group.id,
                120,
                Some(30),
                dependency_key,
                &display_target,
                &changed_route,
                false,
            )
            .is_none()
    );

    let mut changed_target = display_target.clone();
    changed_target.opacity = 0.5;
    assert!(
        runtime
            .reuse_retained_materialized_group_frame(
                group.id,
                120,
                Some(30),
                dependency_key,
                &changed_target,
                &display_route,
                false,
            )
            .is_none()
    );
    assert!(
        runtime
            .reuse_retained_materialized_group_frame(
                group.id,
                120,
                None,
                dependency_key,
                &display_target,
                &display_route,
                false,
            )
            .is_none()
    );

    let unfinalized_group = sample_display_group(4, 4);
    let unfinalized_target = unfinalized_group
        .display_target
        .as_ref()
        .expect("display group should have a target")
        .clone();
    let unfinalized_route = sample_display_route(unfinalized_target.device_id);
    let unfinalized_frame = sample_group_canvas_frame(&unfinalized_target, false);
    runtime.retain_materialized_group_frame(
        unfinalized_group.id,
        100,
        dependency_key,
        &unfinalized_target,
        &unfinalized_route,
        false,
        &unfinalized_frame,
    );
    assert!(
        runtime
            .reuse_retained_materialized_group_frame(
                unfinalized_group.id,
                120,
                Some(30),
                dependency_key,
                &unfinalized_target,
                &unfinalized_route,
                false,
            )
            .is_none()
    );
}

#[test]
fn latest_direct_group_reuse_keeps_display_face_visible_across_dependency_change() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let group = sample_display_group(4, 4);
    let display_target = group
        .display_target
        .as_ref()
        .expect("display group should have a target")
        .clone();
    let retained = PendingGroupCanvasFrame {
        frame: ProducerFrame::Canvas(Canvas::new(4, 4)),
        display_target: display_target.clone(),
        empty_direct_shell: false,
    };

    runtime.retain_direct_group_frame(group.id, 100, SceneDependencyKey::new(1, 1), &retained);

    let reused = runtime
        .reuse_latest_direct_group_frame(&group)
        .expect("pending display face should reuse the previous direct frame");
    assert_eq!(reused.display_target, display_target);

    let mut changed_target = group.clone();
    changed_target
        .display_target
        .as_mut()
        .expect("display group should have a target")
        .opacity = 0.5;
    assert!(
        runtime
            .reuse_latest_direct_group_frame(&changed_target)
            .is_none()
    );

    let mut changed_size = group;
    changed_size.layout.canvas_width += 1;
    assert!(
        runtime
            .reuse_latest_direct_group_frame(&changed_size)
            .is_none()
    );
}

#[test]
fn latest_materialized_group_reuse_ignores_cadence_for_missed_frames() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let group = sample_display_group(4, 4);
    let display_target = group
        .display_target
        .as_ref()
        .expect("display group should have a target")
        .clone();
    let display_route = sample_display_route(display_target.device_id);
    let dependency_key = SceneDependencyKey::new(1, 1);
    let group_canvas_frame = sample_group_canvas_frame(&display_target, true);

    runtime.retain_materialized_group_frame(
        group.id,
        100,
        dependency_key,
        &display_target,
        &display_route,
        false,
        &group_canvas_frame,
    );

    assert!(
        runtime
            .reuse_retained_materialized_group_frame(
                group.id,
                140,
                Some(30),
                dependency_key,
                &display_target,
                &display_route,
                false,
            )
            .is_none()
    );

    let reused = runtime
        .reuse_latest_materialized_group_frame(group.id, &display_target, &display_route, false)
        .expect("latest materialized frame should latch when a fresh frame misses");
    assert_eq!(reused.display_target, group_canvas_frame.display_target);

    let mut changed_route = display_route.clone();
    changed_route.width += 1;
    assert!(
            runtime
                .reuse_latest_materialized_group_frame(
                    group.id,
                    &display_target,
                    &changed_route,
                    false,
                )
                .is_none()
        );

    let mut changed_target = display_target.clone();
    changed_target.opacity = 0.5;
    assert!(
            runtime
                .reuse_latest_materialized_group_frame(
                    group.id,
                    &changed_target,
                    &display_route,
                    false,
                )
                .is_none()
        );
}
