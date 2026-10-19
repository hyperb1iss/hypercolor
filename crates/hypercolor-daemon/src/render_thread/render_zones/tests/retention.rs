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
        Some(30),
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
        Some(30),
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
        Some(30),
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
fn display_retention_renders_once_the_tick_reaches_the_deadline() {
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

    // A 30 fps frame retained at 100 ms is due at 133.33 ms; a whole
    // millisecond tick within the two millisecond tolerance counts.
    runtime.retain_direct_zone_frame(zone.id, 100, Some(30), dependency_key, &direct_frame);
    runtime.retain_materialized_zone_frame(
        zone.id,
        100,
        Some(30),
        dependency_key,
        &display_target,
        &display_route,
        false,
        &materialized_frame,
    );

    assert!(
        runtime
            .reuse_retained_direct_zone_frame(&zone, 131, &target_fps, dependency_key)
            .is_some()
    );
    assert!(
        runtime
            .reuse_retained_direct_zone_frame(&zone, 132, &target_fps, dependency_key)
            .is_none()
    );
    assert!(
        runtime
            .reuse_retained_materialized_zone_frame(
                zone.id,
                131,
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
                132,
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

    runtime.retain_direct_zone_frame(
        zone.id,
        100,
        Some(30),
        SceneDependencyKey::new(1, 1),
        &retained,
    );

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
        Some(30),
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

/// Drives both display face gates through `ticks` the way a render frame
/// does: reuse the retained frame while it holds, and render and retain a
/// fresh one once the face is due. Returns the ticks that rendered.
fn fresh_face_ticks(target_fps: u32, ticks: impl IntoIterator<Item = u64>) -> Vec<u64> {
    let mut runtime = ZoneRuntime::new(4, 4);
    let zone = sample_display_zone(4, 4);
    let display_target = zone
        .display_target
        .clone()
        .expect("display zone should have a target");
    let direct_frame = PendingDisplayZoneFrame {
        frame: ProducerFrame::Canvas(Canvas::new(4, 4)),
        display_target: display_target.clone(),
        empty_direct_shell: false,
    };
    let materialized_frame = sample_zone_canvas_frame(&display_target, true);
    let display_route = sample_display_route(display_target.device_id);
    let dependency_key = SceneDependencyKey::new(1, 1);
    let target_fps_by_zone = HashMap::from([(zone.id, target_fps)]);

    let mut fresh = Vec::new();
    for elapsed_ms in ticks {
        let direct_due = runtime
            .reuse_retained_direct_zone_frame(
                &zone,
                elapsed_ms,
                &target_fps_by_zone,
                dependency_key,
            )
            .is_none();
        let materialized_due = runtime
            .reuse_retained_materialized_zone_frame(
                zone.id,
                elapsed_ms,
                Some(target_fps),
                dependency_key,
                &display_target,
                &display_route,
                false,
            )
            .is_none();
        assert_eq!(
            direct_due, materialized_due,
            "display gates disagree at {elapsed_ms} ms"
        );
        if direct_due {
            runtime.retain_direct_zone_frame(
                zone.id,
                elapsed_ms,
                Some(target_fps),
                dependency_key,
                &direct_frame,
            );
            runtime.retain_materialized_zone_frame(
                zone.id,
                elapsed_ms,
                Some(target_fps),
                dependency_key,
                &display_target,
                &display_route,
                false,
                &materialized_frame,
            );
            fresh.push(elapsed_ms);
        }
    }
    fresh
}

/// Whole-millisecond ticks of a render loop running at `loop_fps`.
fn loop_ticks(loop_fps: u64, frames: std::ops::Range<u64>) -> impl Iterator<Item = u64> {
    frames.map(move |frame| frame * 1000 / loop_fps)
}

#[test]
fn display_cadence_paces_thirty_fps_evenly_on_a_sixty_fps_loop() {
    // Every second tick lands 33 or 34 ms after the last. Slots of the
    // uptime clock render this same sequence at 50, 33, and 17 ms gaps.
    let fresh = fresh_face_ticks(30, loop_ticks(60, 0..600));

    let every_second_tick: Vec<_> = loop_ticks(60, 0..600).step_by(2).collect();
    assert_eq!(fresh, every_second_tick);
}

#[test]
fn display_cadence_keeps_even_pacing_through_wake_jitter() {
    // Ticks wake up to a millisecond past their 60 fps deadlines in a
    // scattered pattern. The tick that anchors the cadence wakes nearly a
    // full millisecond late, so the ticks after it look early against it.
    let jittered_ticks = |frames: std::ops::Range<u64>| {
        frames.map(|frame| {
            let late_us = (997 + frame * 7919) % 1000;
            (500 + frame * 1_000_000 / 60 + late_us) / 1000
        })
    };

    let fresh = fresh_face_ticks(30, jittered_ticks(0..600));

    let every_second_tick: Vec<_> = jittered_ticks(0..600).step_by(2).collect();
    assert_eq!(fresh, every_second_tick);
}

#[test]
fn display_cadence_delivers_thirty_fps_on_a_forty_five_fps_loop() {
    // 450 ticks of 22.2 ms span ten seconds and hold 300 deadlines. Gating
    // on the last render time rendered every second tick, 22.5 fps; the
    // deadline renders two ticks in three and answers every deadline.
    let fresh = fresh_face_ticks(30, loop_ticks(45, 0..450));

    assert_eq!(fresh.len(), 300);
}

#[test]
fn display_cadence_renders_nearly_every_tick_of_a_thirty_fps_loop() {
    // Whole-millisecond 33 ms ticks run slightly ahead of 33.33 ms
    // deadlines, so 300 ticks (9.87 s) hold 297 deadlines. The face answers
    // each one and never renders faster than its target.
    let fresh = fresh_face_ticks(30, (0..300).map(|frame| frame * 33));

    assert_eq!(fresh.len(), 297);
}

#[test]
fn display_cadence_caps_a_fast_loop_at_the_target() {
    // A 60 fps loop against a 2 fps face renders exactly on each 500 ms
    // deadline, twice a second.
    let fresh = fresh_face_ticks(2, loop_ticks(60, 0..600));

    let deadlines: Vec<_> = (0..20).map(|frame| frame * 500).collect();
    assert_eq!(fresh, deadlines);
}

#[test]
fn display_cadence_resyncs_after_a_stall_instead_of_bursting() {
    // The loop stalls for a quarter second and skips fifteen ticks, then
    // resumes on its own grid. The face renders on the first tick back and
    // every second tick after it, rather than racing through the deadlines
    // it missed.
    let ticks = loop_ticks(60, 0..60).chain(loop_ticks(60, 75..135));
    let fresh = fresh_face_ticks(30, ticks);

    let after_stall: Vec<_> = fresh.into_iter().filter(|&tick| tick >= 1_250).collect();
    let every_second_tick: Vec<_> = loop_ticks(60, 75..135).step_by(2).collect();
    assert_eq!(after_stall, every_second_tick);
}

#[test]
fn display_cadence_resyncs_to_a_render_ahead_of_its_deadline() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let zone = sample_display_zone(4, 4);
    let display_target = zone
        .display_target
        .clone()
        .expect("display zone should have a target");
    let direct_frame = PendingDisplayZoneFrame {
        frame: ProducerFrame::Canvas(Canvas::new(4, 4)),
        display_target: display_target.clone(),
        empty_direct_shell: false,
    };
    let materialized_frame = sample_zone_canvas_frame(&display_target, true);
    let display_route = sample_display_route(display_target.device_id);
    let target_fps = HashMap::from([(zone.id, 30)]);
    let retain = |runtime: &mut ZoneRuntime, elapsed_ms, dependency_key| {
        runtime.retain_direct_zone_frame(
            zone.id,
            elapsed_ms,
            Some(30),
            dependency_key,
            &direct_frame,
        );
        runtime.retain_materialized_zone_frame(
            zone.id,
            elapsed_ms,
            Some(30),
            dependency_key,
            &display_target,
            &display_route,
            false,
            &materialized_frame,
        );
    };
    let holds = |runtime: &mut ZoneRuntime, elapsed_ms, dependency_key| {
        let direct = runtime
            .reuse_retained_direct_zone_frame(&zone, elapsed_ms, &target_fps, dependency_key)
            .is_some();
        let materialized = runtime
            .reuse_retained_materialized_zone_frame(
                zone.id,
                elapsed_ms,
                Some(30),
                dependency_key,
                &display_target,
                &display_route,
                false,
            )
            .is_some();
        assert_eq!(
            direct, materialized,
            "display gates disagree at {elapsed_ms} ms"
        );
        direct
    };

    // The frame retained at 100 ms is due at 133.33 ms, but a dependency
    // change renders the face at 110 ms. The next frame is due one interval
    // after that render, at 143.33 ms, not one after the deadline it beat.
    retain(&mut runtime, 100, SceneDependencyKey::new(1, 1));
    let changed = SceneDependencyKey::new(1, 2);
    assert!(!holds(&mut runtime, 110, changed));
    retain(&mut runtime, 110, changed);

    assert!(holds(&mut runtime, 141, changed));
    assert!(!holds(&mut runtime, 142, changed));
}
