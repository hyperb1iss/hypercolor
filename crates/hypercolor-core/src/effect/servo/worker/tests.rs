use super::super::telemetry::servo_telemetry_snapshot;
use super::*;
use std::sync::atomic::Ordering;
use std::sync::mpsc::TryRecvError;

fn queued_render_command(
    session_id: ServoSessionId,
    script: &str,
) -> (WorkerCommand, mpsc::Receiver<Result<EffectRenderOutput>>) {
    queued_render_command_with_role(session_id, ServoProducerRole::SceneHtml, script)
}

fn queued_render_command_with_role(
    session_id: ServoSessionId,
    producer_role: ServoProducerRole,
    script: &str,
) -> (WorkerCommand, mpsc::Receiver<Result<EffectRenderOutput>>) {
    let (response_tx, response_rx) = mpsc::sync_channel(1);
    (
        WorkerCommand::Render {
            session_id,
            producer_role,
            scripts: vec![script.to_owned()],
            frame_payloads: Vec::new(),
            width: 320,
            height: 200,
            mode: ServoRenderMode::Cpu,
            submitted_at: Instant::now(),
            response_tx,
        },
        response_rx,
    )
}

#[test]
fn combined_script_appends_frame_payloads_through_stable_adapter() {
    let scripts = vec!["window.__hypercolorApplyFramePayload = function(payload) {};".to_owned()];
    let frame_payloads = vec![
        ServoFramePayload::from_json("{\"canvas\":{\"width\":320}}".to_owned())
            .expect("valid JSON object"),
    ];
    let mut buffer = String::new();

    combined_script(&mut buffer, &scripts, &frame_payloads);

    assert_eq!(
        buffer,
        concat!(
            "window.__hypercolorApplyFramePayload = function(payload) {};\n",
            "window.__hypercolorApplyFramePayload({\"canvas\":{\"width\":320}});\n"
        )
    );
    let adapter_index = buffer
        .find("window.__hypercolorApplyFramePayload = function")
        .expect("adapter assignment should be present");
    let delivery_index = buffer
        .find("window.__hypercolorApplyFramePayload({")
        .expect("payload delivery should be present");
    assert!(adapter_index < delivery_index);
}

#[test]
fn frame_payload_requires_json_object() {
    assert!(ServoFramePayload::from_json("not json".to_owned()).is_err());
    assert!(ServoFramePayload::from_json("[]".to_owned()).is_err());
    assert!(ServoFramePayload::from_json("{\"ok\":true}".to_owned()).is_ok());
}

#[test]
fn frame_payload_canonicalizes_json_for_script_embedding() {
    let payload = ServoFramePayload::from_json("{\"text\":\"line\u{2028}break\"}".to_owned())
        .expect("valid JSON object");

    assert_eq!(payload.as_json(), "{\"text\":\"line\\u2028break\"}");
}

#[test]
fn scheduler_coalesces_redundant_renders_by_session() {
    let mut scheduler = ServoWorkerScheduler::default();
    let before = servo_telemetry_snapshot();
    let session_id = ServoSessionId(42);
    let (first, first_rx) = queued_render_command(session_id, "old()");
    let (second, second_rx) = queued_render_command(session_id, "new()");

    scheduler.push(first);
    scheduler.push(second);

    let superseded = first_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("superseded render should receive a response");
    assert!(superseded.is_err());
    let after = servo_telemetry_snapshot();
    assert!(after.render_superseded_total > before.render_superseded_total);

    let Some(ScheduledServoWork::Render(render)) = scheduler.next() else {
        panic!("expected latest render work");
    };
    assert_eq!(render.session_id, session_id);
    assert_eq!(render.scripts, vec!["new()"]);
    assert_eq!(render.width, 320);
    assert_eq!(render.height, 200);
    assert!(matches!(second_rx.try_recv(), Err(TryRecvError::Empty)));
    assert!(scheduler.next().is_none());
    assert!(scheduler.is_empty());
}

#[test]
fn scheduler_keeps_one_fair_render_slot_per_session() {
    let mut scheduler = ServoWorkerScheduler::default();
    let first_session = ServoSessionId(1);
    let second_session = ServoSessionId(2);
    let (first, first_rx) = queued_render_command(first_session, "first-old()");
    let (second, _second_rx) = queued_render_command(second_session, "second()");
    let (latest_first, _latest_first_rx) = queued_render_command(first_session, "first-new()");

    scheduler.push(first);
    scheduler.push(second);
    scheduler.push(latest_first);

    let superseded = first_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("superseded render should receive a response");
    assert!(superseded.is_err());

    let Some(ScheduledServoWork::Render(render)) = scheduler.next() else {
        panic!("expected first session render");
    };
    assert_eq!(render.session_id, first_session);
    assert_eq!(render.scripts, vec!["first-new()"]);

    let Some(ScheduledServoWork::Render(render)) = scheduler.next() else {
        panic!("expected second session render");
    };
    assert_eq!(render.session_id, second_session);
    assert_eq!(render.scripts, vec!["second()"]);

    assert!(scheduler.next().is_none());
}

#[test]
fn scheduler_bounds_heavy_session_churn_to_one_slot() {
    let mut scheduler = ServoWorkerScheduler::default();
    let display_session = ServoSessionId(1);
    let led_session = ServoSessionId(2);
    let (display, display_rx) = queued_render_command(display_session, "display-face-0()");
    let (led, led_rx) = queued_render_command(led_session, "led-html()");
    let mut superseded_receivers = vec![display_rx];
    let mut latest_display_rx = None;

    scheduler.push(display);
    scheduler.push(led);
    for frame in 1..=64 {
        if let Some(rx) = latest_display_rx.take() {
            superseded_receivers.push(rx);
        }
        let script = format!("display-face-{frame}()");
        let (display, display_rx) = queued_render_command(display_session, &script);
        scheduler.push(display);
        latest_display_rx = Some(display_rx);
    }

    for rx in superseded_receivers {
        let superseded = rx
            .recv_timeout(Duration::from_millis(100))
            .expect("superseded display render should receive a response");
        assert!(superseded.is_err());
    }

    let Some(ScheduledServoWork::Render(render)) = scheduler.next() else {
        panic!("expected latest display render");
    };
    assert_eq!(render.session_id, display_session);
    assert_eq!(render.scripts, vec!["display-face-64()"]);

    let Some(ScheduledServoWork::Render(render)) = scheduler.next() else {
        panic!("expected led render after display slot");
    };
    assert_eq!(render.session_id, led_session);
    assert_eq!(render.scripts, vec!["led-html()"]);
    assert!(matches!(led_rx.try_recv(), Err(TryRecvError::Empty)));
    assert!(scheduler.next().is_none());
}

#[test]
fn scheduler_preserves_lifecycle_command_order() {
    let mut scheduler = ServoWorkerScheduler::default();
    let (create_tx, _create_rx) = mpsc::sync_channel(1);
    let (render, _render_rx) = queued_render_command(ServoSessionId(7), "tick()");
    let (shutdown_tx, _shutdown_rx) = mpsc::sync_channel(1);

    scheduler.push(WorkerCommand::CreateSession {
        session_id: ServoSessionId(7),
        producer_role: ServoProducerRole::SceneHtml,
        width: 320,
        height: 200,
        response_tx: create_tx,
    });
    scheduler.push(render);
    scheduler.push(WorkerCommand::Shutdown {
        response_tx: shutdown_tx,
    });

    assert!(matches!(
        scheduler.next(),
        Some(ScheduledServoWork::Command(
            WorkerCommand::CreateSession { .. }
        ))
    ));
    assert!(matches!(
        scheduler.next(),
        Some(ScheduledServoWork::Render(PendingRenderCommand { .. }))
    ));
    assert!(matches!(
        scheduler.next(),
        Some(ScheduledServoWork::Command(WorkerCommand::Shutdown { .. }))
    ));
    assert!(scheduler.next().is_none());
}

#[test]
fn scheduler_alternates_scene_and_display_render_lanes_before_barrier() {
    let mut scheduler = ServoWorkerScheduler::default();
    let scene_a = ServoSessionId(1);
    let scene_b = ServoSessionId(2);
    let display = ServoSessionId(3);

    let (scene_first, _scene_first_rx) =
        queued_render_command_with_role(scene_a, ServoProducerRole::SceneHtml, "scene-a()");
    let (scene_second, _scene_second_rx) =
        queued_render_command_with_role(scene_b, ServoProducerRole::SceneHtml, "scene-b()");
    let (display_work, _display_rx) =
        queued_render_command_with_role(display, ServoProducerRole::DisplayFaceHtml, "display()");

    scheduler.push(scene_first);
    scheduler.push(scene_second);
    scheduler.push(display_work);

    let Some(ScheduledServoWork::Render(render)) = scheduler.next() else {
        panic!("expected first scene render");
    };
    assert_eq!(render.session_id, scene_a);
    assert_eq!(render.producer_role, ServoProducerRole::SceneHtml);

    let Some(ScheduledServoWork::Render(render)) = scheduler.next() else {
        panic!("expected display render to alternate after scene lane");
    };
    assert_eq!(render.session_id, display);
    assert_eq!(render.producer_role, ServoProducerRole::DisplayFaceHtml);

    let Some(ScheduledServoWork::Render(render)) = scheduler.next() else {
        panic!("expected remaining scene render");
    };
    assert_eq!(render.session_id, scene_b);
    assert_eq!(render.producer_role, ServoProducerRole::SceneHtml);
    assert!(scheduler.next().is_none());
}

#[cfg(feature = "servo-gpu-import")]
#[test]
fn cached_gpu_frame_reuse_requires_policy_no_ready_and_matching_size() {
    assert!(can_reuse_cached_gpu_frame(true, false, 480, 480, 480, 480));
    assert!(!can_reuse_cached_gpu_frame(
        false, false, 480, 480, 480, 480
    ));
    assert!(!can_reuse_cached_gpu_frame(true, true, 480, 480, 480, 480));
    assert!(!can_reuse_cached_gpu_frame(true, false, 480, 480, 640, 480));
    assert!(!can_reuse_cached_gpu_frame(true, false, 480, 480, 480, 640));
}

#[test]
fn scheduler_treats_lifecycle_commands_as_render_barriers() {
    let mut scheduler = ServoWorkerScheduler::default();
    let session_id = ServoSessionId(7);
    let (old_render, old_rx) = queued_render_command(session_id, "old()");
    let (destroy_tx, _destroy_rx) = mpsc::sync_channel(1);
    let (new_render, new_rx) = queued_render_command(session_id, "new()");

    scheduler.push(old_render);
    scheduler.push(WorkerCommand::DestroySession {
        session_id,
        response_tx: destroy_tx,
    });
    scheduler.push(new_render);

    assert!(matches!(old_rx.try_recv(), Err(TryRecvError::Empty)));
    assert!(matches!(new_rx.try_recv(), Err(TryRecvError::Empty)));

    let Some(ScheduledServoWork::Render(render)) = scheduler.next() else {
        panic!("expected old render before destroy");
    };
    assert_eq!(render.scripts, vec!["old()"]);

    assert!(matches!(
        scheduler.next(),
        Some(ScheduledServoWork::Command(
            WorkerCommand::DestroySession { .. }
        ))
    ));

    let Some(ScheduledServoWork::Render(render)) = scheduler.next() else {
        panic!("expected new render after destroy");
    };
    assert_eq!(render.scripts, vec!["new()"]);

    assert!(scheduler.next().is_none());
}

#[test]
fn fatal_classifier_keeps_session_local_page_failures_local() {
    let page_timeout = anyhow!(
        "timed out waiting for Servo page load completion (expected_url=Some(\"file:///bad.html\"), current_url=about:blank)"
    );
    let javascript_timeout = anyhow!("timed out waiting for JavaScript callback");
    let javascript_error = anyhow!("javascript evaluation failed: TypeError: boom");

    assert!(!servo_worker_is_fatal_error(&page_timeout));
    assert!(!servo_worker_is_fatal_error(&javascript_timeout));
    assert!(!servo_worker_is_fatal_error(&javascript_error));
}

#[test]
fn fatal_classifier_keeps_transport_failures_global() {
    let worker_timeout = anyhow!("timed out waiting for Servo worker readiness after 10000ms");
    let client_page_timeout = anyhow!("timed out waiting for Servo page load after 10000ms");
    let disconnected = anyhow!("Servo worker disconnected before returning a frame");
    let send_failure = anyhow!("failed to send render command to Servo worker");

    assert!(servo_worker_is_fatal_error(&worker_timeout));
    assert!(servo_worker_is_fatal_error(&client_page_timeout));
    assert!(servo_worker_is_fatal_error(&disconnected));
    assert!(servo_worker_is_fatal_error(&send_failure));
}

#[test]
fn cached_canvas_reuse_waits_for_settled_no_ready_frames() {
    let cached = Canvas::new(320, 200);

    assert!(!can_reuse_cached_canvas(
        false,
        0,
        1,
        Some(&cached),
        320,
        200
    ));
    assert!(can_reuse_cached_canvas(
        false,
        0,
        STATIC_CANVAS_REUSE_NO_READY_FRAMES,
        Some(&cached),
        320,
        200
    ));
    assert!(!can_reuse_cached_canvas(
        false,
        1,
        STATIC_CANVAS_REUSE_NO_READY_FRAMES,
        Some(&cached),
        320,
        200
    ));
    assert!(!can_reuse_cached_canvas(
        true,
        0,
        STATIC_CANVAS_REUSE_NO_READY_FRAMES,
        Some(&cached),
        320,
        200
    ));
}

#[test]
fn cached_canvas_reuse_requires_matching_dimensions() {
    let cached = Canvas::new(320, 200);

    assert!(!can_reuse_cached_canvas(
        false,
        0,
        STATIC_CANVAS_REUSE_NO_READY_FRAMES,
        Some(&cached),
        640,
        360
    ));
    assert!(!can_reuse_cached_canvas(
        false,
        0,
        STATIC_CANVAS_REUSE_NO_READY_FRAMES,
        None,
        320,
        200
    ));
}

#[test]
fn transparent_readback_reuses_visible_cached_canvas() {
    use hypercolor_types::canvas::Rgba;

    let mut cached = Canvas::new(320, 200);
    cached.fill(Rgba::new(12, 34, 56, 255));
    let transparent = Canvas::from_vec(vec![0; 320 * 200 * 4], 320, 200);
    let mut visible = Canvas::new(320, 200);
    visible.fill(Rgba::new(12, 34, 56, 255));

    assert!(should_reuse_cached_canvas_after_transparent_readback(
        Some(&cached),
        &transparent,
        320,
        200
    ));
    assert!(!should_reuse_cached_canvas_after_transparent_readback(
        Some(&cached),
        &visible,
        320,
        200
    ));
    assert!(!should_reuse_cached_canvas_after_transparent_readback(
        Some(&cached),
        &transparent,
        480,
        480
    ));
    assert!(!should_reuse_cached_canvas_after_transparent_readback(
        Some(&transparent),
        &transparent,
        320,
        200
    ));
}

#[test]
fn trimmed_servo_preferences_use_transparent_shell_background() {
    assert_eq!(
        trimmed_servo_preferences().shell_background_color_rgba,
        [0.0, 0.0, 0.0, 0.0]
    );
}

#[test]
fn trimmed_servo_preferences_leave_jit_enabled() {
    let preferences = trimmed_servo_preferences();
    assert!(!preferences.js_disable_jit);
    assert!(preferences.js_baseline_jit_enabled);
    assert!(preferences.js_ion_enabled);
    assert!(preferences.js_offthread_compilation_enabled);
}

#[test]
fn trimmed_servo_preferences_tighten_embedder_gc_policy() {
    let preferences = trimmed_servo_preferences();
    assert_eq!(preferences.js_mem_gc_empty_chunk_count_min, 0);
    assert_eq!(preferences.js_mem_gc_high_frequency_heap_growth_max, 150);
    assert_eq!(preferences.js_mem_gc_high_frequency_heap_growth_min, 120);
    assert_eq!(preferences.js_mem_gc_high_frequency_high_limit_mb, 128);
    assert_eq!(preferences.js_mem_gc_high_frequency_low_limit_mb, 64);
    assert_eq!(preferences.js_mem_gc_low_frequency_heap_growth, 120);
}

#[test]
fn servo_readback_buffers_reuse_exclusive_retired_canvas_storage() {
    let pixels = vec![0x7f; 16];
    let original_ptr = pixels.as_ptr();
    let mut buffers = ServoReadbackBuffers::default();
    buffers.retire_canvas(Canvas::from_vec(pixels, 2, 2));

    let reused = buffers.take_buffer(16);

    assert_eq!(reused.as_ptr(), original_ptr);
    assert!(buffers.retired_canvases.is_empty());
}

#[test]
fn servo_readback_buffers_wait_for_downstream_canvas_release() {
    let pixels = vec![0x3f; 16];
    let original_ptr = pixels.as_ptr();
    let canvas = Canvas::from_vec(pixels, 2, 2);
    let downstream = canvas.clone();
    let mut buffers = ServoReadbackBuffers::default();
    buffers.retire_canvas(canvas);

    let first = buffers.take_buffer(16);
    assert_ne!(first.as_ptr(), original_ptr);
    assert_eq!(buffers.retired_canvases.len(), 1);

    drop(downstream);
    let reused = buffers.take_buffer(16);
    assert_eq!(reused.as_ptr(), original_ptr);
}

#[test]
fn servo_worker_shutdown_joins_thread() {
    let (mut worker, stopped) = test_support::spawn_test_worker();

    worker.shutdown().expect("worker shutdown should succeed");

    assert!(stopped.load(Ordering::SeqCst));
    assert!(worker.command_tx.is_none());
    assert!(worker.thread_handle.is_none());
}

#[test]
fn poisoned_shared_worker_requires_daemon_restart() {
    let _lock = test_support::SHARED_WORKER_STATE_TEST_LOCK
        .lock()
        .expect("shared worker test lock");
    reset_shared_servo_worker_state();

    install_poisoned_shared_worker("test failure");

    let result = acquire_servo_worker();
    assert!(result.is_err(), "poisoned worker should fail closed");
    let error = result.err().expect("poisoned worker should fail closed");
    assert!(
        error
            .to_string()
            .contains("Servo runtime is unrecoverable until the daemon restarts")
    );

    reset_shared_servo_worker_state();
}

#[test]
fn shutdown_clears_poisoned_shared_worker_state() {
    let _lock = test_support::SHARED_WORKER_STATE_TEST_LOCK
        .lock()
        .expect("shared worker test lock");
    reset_shared_servo_worker_state();

    install_poisoned_shared_worker("test failure");

    shutdown_shared_servo_worker().expect("shutdown should clear poisoned state");

    assert!(shared_worker_is_vacant());
}

#[test]
fn process_exit_shutdown_retires_shared_worker() {
    let _lock = test_support::SHARED_WORKER_STATE_TEST_LOCK
        .lock()
        .expect("shared worker test lock");
    reset_shared_servo_worker_state();
    let (worker, stopped) = test_support::spawn_test_worker();
    install_running_shared_worker(worker);

    shutdown_servo_runtime().expect("process-exit shutdown should stop shared worker");

    assert!(stopped.load(Ordering::SeqCst));
    let result = acquire_servo_worker();
    assert!(
        result.is_err(),
        "Servo runtime should not restart after process-exit shutdown"
    );
    reset_shared_servo_worker_state();
}

#[test]
fn load_completion_url_matches_exact_expected_url() {
    assert!(load_completion_url_matches(
        Some("https://example.com"),
        Some("https://example.com")
    ));
}

#[test]
fn load_completion_url_matches_redirected_url() {
    assert!(load_completion_url_matches(
        Some("https://example.com"),
        Some("https://www.example.com/en")
    ));
}

#[test]
fn load_completion_url_rejects_blank_page() {
    assert!(!load_completion_url_matches(
        Some("https://example.com"),
        Some("about:blank")
    ));
    assert!(!load_completion_url_matches(
        Some("https://example.com"),
        None
    ));
}
