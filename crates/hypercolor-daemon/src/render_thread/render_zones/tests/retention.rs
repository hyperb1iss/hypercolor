use super::*;

#[test]
fn clear_inactive_zones_releases_cached_zone_state() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let zone = sample_zone(4, 4);
    let display_zone = sample_display_zone(4, 4);
    let display_target = display_zone
        .display_target
        .as_ref()
        .expect("display zone should have a target")
        .clone();
    let display_route = sample_display_route(display_target.device_id);
    let zone_canvas_frame = sample_zone_canvas_frame(&display_target, true);
    runtime.target_canvases.insert(zone.id, Canvas::new(4, 4));
    runtime
        .spatial_engines
        .insert(zone.id, SpatialEngine::new(zone.layout.clone()));
    runtime.retain_materialized_zone_frame(
        display_zone.id,
        100,
        SceneDependencyKey::new(1, 1),
        &display_target,
        &display_route,
        false,
        &zone_canvas_frame,
    );
    runtime.reconciled_dependency_key = Some(SceneDependencyKey::new(1, 1));

    assert!(runtime.has_inactive_zone_resources());

    runtime.clear_inactive_zones();

    assert!(!runtime.has_inactive_zone_resources());
    assert!(runtime.combined_led_layout.zones.is_empty());
}

#[test]
fn materialized_zone_reuse_obeys_cadence_and_route_identity() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let zone = sample_display_zone(4, 4);
    let display_target = zone
        .display_target
        .as_ref()
        .expect("display zone should have a target")
        .clone();
    let display_route = sample_display_route(display_target.device_id);
    let dependency_key = SceneDependencyKey::new(1, 1);
    let zone_canvas_frame = sample_zone_canvas_frame(&display_target, true);

    runtime.retain_materialized_zone_frame(
        zone.id,
        100,
        dependency_key,
        &display_target,
        &display_route,
        false,
        &zone_canvas_frame,
    );

    let reused = runtime
        .reuse_retained_materialized_zone_frame(
            zone.id,
            120,
            Some(30),
            dependency_key,
            &display_target,
            &display_route,
            false,
        )
        .expect("retained materialized frame should be reused within cadence");
    assert_eq!(reused.display_target, zone_canvas_frame.display_target);

    assert!(
        runtime
            .reuse_retained_materialized_zone_frame(
                zone.id,
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
            .reuse_retained_materialized_zone_frame(
                zone.id,
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
            .reuse_retained_materialized_zone_frame(
                zone.id,
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
            .reuse_retained_materialized_zone_frame(
                zone.id,
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
            .reuse_retained_materialized_zone_frame(
                zone.id,
                120,
                None,
                dependency_key,
                &display_target,
                &display_route,
                false,
            )
            .is_none()
    );

    let unfinalized_zone = sample_display_zone(4, 4);
    let unfinalized_target = unfinalized_zone
        .display_target
        .as_ref()
        .expect("display zone should have a target")
        .clone();
    let unfinalized_route = sample_display_route(unfinalized_target.device_id);
    let unfinalized_frame = sample_zone_canvas_frame(&unfinalized_target, false);
    runtime.retain_materialized_zone_frame(
        unfinalized_zone.id,
        100,
        dependency_key,
        &unfinalized_target,
        &unfinalized_route,
        false,
        &unfinalized_frame,
    );
    assert!(
        runtime
            .reuse_retained_materialized_zone_frame(
                unfinalized_zone.id,
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
fn display_retention_allows_thirty_fps_on_sixty_fps_ticks() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let zone = sample_display_zone(4, 4);
    let display_target = zone
        .display_target
        .as_ref()
        .expect("display zone should have a target")
        .clone();
    let direct_frame = PendingDisplayZoneFrame {
        frame: ProducerFrame::Canvas(Canvas::new(4, 4)),
        display_target: display_target.clone(),
        empty_direct_shell: false,
    };
    let materialized_frame = sample_zone_canvas_frame(&display_target, true);
    let display_route = sample_display_route(display_target.device_id);
    let dependency_key = SceneDependencyKey::new(1, 1);
    let target_fps = HashMap::from([(zone.id, 30)]);

    runtime.retain_direct_zone_frame(zone.id, 100, dependency_key, &direct_frame);
    runtime.retain_materialized_zone_frame(
        zone.id,
        100,
        dependency_key,
        &display_target,
        &display_route,
        false,
        &materialized_frame,
    );

    assert!(
        runtime
            .reuse_retained_direct_zone_frame(&zone, 132, &target_fps, dependency_key)
            .is_some()
    );
    assert!(
        runtime
            .reuse_retained_direct_zone_frame(&zone, 133, &target_fps, dependency_key)
            .is_none()
    );
    assert!(
        runtime
            .reuse_retained_materialized_zone_frame(
                zone.id,
                132,
                Some(30),
                dependency_key,
                &display_target,
                &display_route,
                false,
            )
            .is_some()
    );
    assert!(
        runtime
            .reuse_retained_materialized_zone_frame(
                zone.id,
                133,
                Some(30),
                dependency_key,
                &display_target,
                &display_route,
                false,
            )
            .is_none()
    );
}

#[test]
fn latest_direct_zone_reuse_keeps_display_face_visible_across_dependency_change() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let zone = sample_display_zone(4, 4);
    let display_target = zone
        .display_target
        .as_ref()
        .expect("display zone should have a target")
        .clone();
    let retained = PendingDisplayZoneFrame {
        frame: ProducerFrame::Canvas(Canvas::new(4, 4)),
        display_target: display_target.clone(),
        empty_direct_shell: false,
    };

    runtime.retain_direct_zone_frame(zone.id, 100, SceneDependencyKey::new(1, 1), &retained);

    let reused = runtime
        .reuse_latest_direct_zone_frame(&zone)
        .expect("pending display face should reuse the previous direct frame");
    assert_eq!(reused.display_target, display_target);

    let mut changed_target = zone.clone();
    changed_target
        .display_target
        .as_mut()
        .expect("display zone should have a target")
        .opacity = 0.5;
    assert!(
        runtime
            .reuse_latest_direct_zone_frame(&changed_target)
            .is_none()
    );

    let mut changed_size = zone;
    changed_size.layout.canvas_width += 1;
    assert!(
        runtime
            .reuse_latest_direct_zone_frame(&changed_size)
            .is_none()
    );
}

#[test]
fn latest_materialized_zone_reuse_ignores_cadence_for_missed_frames() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let zone = sample_display_zone(4, 4);
    let display_target = zone
        .display_target
        .as_ref()
        .expect("display zone should have a target")
        .clone();
    let display_route = sample_display_route(display_target.device_id);
    let dependency_key = SceneDependencyKey::new(1, 1);
    let zone_canvas_frame = sample_zone_canvas_frame(&display_target, true);

    runtime.retain_materialized_zone_frame(
        zone.id,
        100,
        dependency_key,
        &display_target,
        &display_route,
        false,
        &zone_canvas_frame,
    );

    assert!(
        runtime
            .reuse_retained_materialized_zone_frame(
                zone.id,
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
        .reuse_latest_materialized_zone_frame(zone.id, &display_target, &display_route, false)
        .expect("latest materialized frame should latch when a fresh frame misses");
    assert_eq!(reused.display_target, zone_canvas_frame.display_target);

    let mut changed_route = display_route.clone();
    changed_route.width += 1;
    assert!(
        runtime
            .reuse_latest_materialized_zone_frame(zone.id, &display_target, &changed_route, false,)
            .is_none()
    );

    let mut changed_target = display_target.clone();
    changed_target.opacity = 0.5;
    assert!(
        runtime
            .reuse_latest_materialized_zone_frame(zone.id, &changed_target, &display_route, false,)
            .is_none()
    );
}
