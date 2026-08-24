use super::*;

#[expect(
    clippy::too_many_lines,
    reason = "Status response assertions cover many nested metrics fields in one scenario"
)]
#[tokio::test]
async fn status_includes_latest_frame_surface_stats() {
    let tempdir = tempfile::tempdir().expect("status test data dir should be created");
    let state = Arc::new(AppState::new_with_data_dir(tempdir.path().join("data")));
    state.render_loop.write().await.start();
    let mut preview_rx = state.preview_runtime.canvas_receiver();
    let mut scene_preview_rx = state.preview_runtime.scene_canvas_receiver();
    let mut screen_preview_rx = state.preview_runtime.screen_canvas_receiver();
    preview_rx.update_demand(PreviewStreamDemand {
        fps: 24,
        format: PreviewPixelFormat::Jpeg,
        width: 640,
        height: 360,
    });
    scene_preview_rx.update_demand(PreviewStreamDemand {
        fps: 12,
        format: PreviewPixelFormat::Rgb,
        width: 320,
        height: 180,
    });
    screen_preview_rx.update_demand(PreviewStreamDemand {
        fps: 30,
        format: PreviewPixelFormat::Rgba,
        width: 0,
        height: 0,
    });
    let canvas_frame = CanvasFrame::from_canvas(&Canvas::new(2, 1), 88, 44);
    let scene_frame = CanvasFrame::from_canvas(&Canvas::new(2, 1), 66, 33);
    let screen_frame = CanvasFrame::from_canvas(&Canvas::new(1, 1), 45, 21);
    let _ = state.event_bus.canvas_lane().send(canvas_frame.clone());
    let _ = state
        .event_bus
        .scene_canvas_lane()
        .send(scene_frame.clone());
    let _ = state
        .event_bus
        .screen_canvas_lane()
        .send(screen_frame.clone());
    state
        .preview_runtime
        .note_canvas_frame(canvas_frame.frame_number, canvas_frame.timestamp_ms);
    state
        .preview_runtime
        .note_scene_canvas_frame(scene_frame.frame_number, scene_frame.timestamp_ms);
    state
        .preview_runtime
        .note_screen_canvas_frame(screen_frame.frame_number, screen_frame.timestamp_ms);
    {
        let mut performance = state.performance.write().await;
        performance.record_effect_error();
        performance.record_effect_fallback_applied();
        let frame = LatestFrameMetrics {
            timestamp_ms: 40,
            input_sampled: true,
            input_us: 100,
            deferred_sample_us: 40,
            producer_us: 500,
            producer_render_us: 320,
            producer_scene_compose_us: 60,
            composition_us: 200,
            render_us: 700,
            preview_advance_us: 25,
            sample_us: 150,
            sample_dispatch_us: 90,
            push_us: 250,
            postprocess_us: 0,
            publish_us: 120,
            publish_frame_data_us: 30,
            publish_zone_canvas_us: 20,
            publish_preview_us: 60,
            publish_events_us: 10,
            overhead_us: 50,
            total_us: 1_270,
            wake_late_us: 90,
            jitter_us: 30,
            reused_inputs: false,
            reused_canvas: false,
            retained_effect: false,
            retained_screen: false,
            composition_bypassed: false,
            gpu_zone_sampling: true,
            gpu_sample_deferred: true,
            gpu_sample_stale: true,
            gpu_sample_retry_hit: true,
            gpu_sample_queue_saturated: true,
            gpu_sample_wait_blocked: true,
            gpu_sample_cpu_fallback: true,
            preview_surface: true,
            scene_canvas_forced_surface: true,
            cpu_readback_skipped: true,
            gpu_readback_failed: true,
            compositor_backend: CompositorBackendKind::GpuFallback,
            output_frame_source: OutputFrameSourceKind::RoutedReuse,
            output_reuses_published_frame: true,
            output_brightness_bits: 1.0_f32.to_bits(),
            output_brightness_generation: 5,
            output_routing_signature: 7,
            output_zone_shape_signature: 11,
            output_unassigned_behavior_generation: 13,
            devices_written: 3,
            total_leds: 144,
            logical_layer_count: 2,
            render_zone_count: 1,
            scene_active: true,
            scene_transition_active: false,
            scene_pool_saturation_reallocs: 0,
            direct_pool_saturation_reallocs: 0,
            scene_pool_grown_slots: 0,
            direct_pool_grown_slots: 0,
            scene_pool_slot_count: 6,
            scene_pool_max_slots: 0,
            direct_pool_slot_count: 0,
            direct_pool_max_slots: 0,
            scene_pool_shared_published_slots: 0,
            scene_pool_max_ref_count: 0,
            direct_pool_shared_published_slots: 0,
            direct_pool_max_ref_count: 0,
            scene_pool_free_slots: 1,
            scene_pool_published_slots: 4,
            scene_pool_dequeued_slots: 1,
            direct_pool_free_slots: 0,
            direct_pool_published_slots: 0,
            direct_pool_dequeued_slots: 0,
            preview_pool_slot_count: 0,
            preview_pool_free_slots: 0,
            preview_pool_published_slots: 0,
            preview_pool_dequeued_slots: 0,
            compositor_pool_slot_count: 0,
            compositor_pool_free_slots: 0,
            compositor_pool_published_slots: 0,
            compositor_pool_dequeued_slots: 0,
            canvas_receiver_count: 2,
            producer_full_frame_copy: FullFrameCopyMetrics {
                count: 1,
                bytes: 128_000,
                reason: Some("producer_test"),
            },
            publication_full_frame_copy: FullFrameCopyMetrics {
                count: 1,
                bytes: 128_000,
                reason: Some("publication_test"),
            },
            full_frame_copy_count: 2,
            full_frame_copy_bytes: 256_000,
            output_errors: 0,
            timeline: FrameTimeline {
                frame_token: 77,
                budget_us: 16_666,
                scene_snapshot_done_us: 80,
                input_done_us: 180,
                producer_done_us: 680,
                composition_done_us: 880,
                sample_done_us: 1_030,
                output_done_us: 1_280,
                publish_done_us: 1_400,
                frame_done_us: 1_450,
            },
        };
        performance.record_frame(&frame);
        drop(performance);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        state.performance.write().await.record_frame(&frame);
    }
    state
        .input_manager()
        .set_screen_capacity_plan(
            ScreenAdmissionCapacity::new(2_000_000, 2_000_000),
            ScreenAdmissionCapacity::new(123, 456),
            ScreenAdmissionCapacity::new(123, 456),
        )
        .expect("empty manager should accept test capacity");

    let response = get_status(State(state)).await;
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("status body should read");
    let json: Value = serde_json::from_slice(&body).expect("status should serialize");

    assert_eq!(json["data"]["render_loop"]["target_fps"], 60);
    assert_eq!(json["data"]["render_loop"]["ceiling_fps"], 60);
    assert_eq!(json["data"]["render_loop"]["capacity_fps"], 60.0);
    let delivered_fps = json["data"]["render_loop"]["delivered_fps"]
        .as_f64()
        .expect("delivered_fps should be numeric");
    assert!(delivered_fps > 0.0);
    assert!(delivered_fps < 60.0);
    assert_eq!(json["data"]["render_loop"]["actual_fps"], 60.0);
    assert_eq!(
        json["data"]["session_performance"]["input_stage"]["sample_count"],
        2
    );
    assert_eq!(
        json["data"]["session_performance"]["input_stage"]["p95_ms"],
        0.1
    );
    assert_eq!(
        json["data"]["session_performance"]["input_stage"]["p99_ms"],
        0.1
    );
    assert_eq!(
        json["data"]["session_performance"]["input_stage"]["cumulative_histogram"]["bucket_width_us"],
        100
    );
    assert_eq!(
        json["data"]["session_performance"]["input_stage"]["cumulative_histogram"]["overflow_bucket_index"],
        4096
    );
    assert_eq!(
        json["data"]["session_performance"]["input_stage"]["cumulative_histogram"]["snapshot_frame_token"],
        77
    );
    assert_eq!(
        json["data"]["session_performance"]["input_stage"]["cumulative_histogram"]["buckets"],
        serde_json::json!([{ "bucket_index": 1, "count": 2 }])
    );
    assert_eq!(
        json["data"]["session_performance"]["full_frame_cpu_copies"]["count"],
        4
    );
    assert_eq!(
        json["data"]["session_performance"]["full_frame_cpu_copies"]["frames"],
        2
    );
    assert_eq!(
        json["data"]["session_performance"]["full_frame_cpu_copies"]["bytes"],
        512_000
    );
    assert_eq!(
        json["data"]["compositor_acceleration"]["requested_mode"],
        "cpu"
    );
    assert_eq!(
        json["data"]["compositor_acceleration"]["effective_mode"],
        "cpu"
    );
    assert!(json["data"]["compositor_acceleration"]["fallback_reason"].is_null());
    assert!(json["data"]["compositor_acceleration"]["gpu_probe"].is_null());
    assert_eq!(
        json["data"]["screen_capture_capacity"]["admission_enforced"],
        true
    );
    assert_eq!(
        json["data"]["screen_capture_capacity"]["physical_transition_byte_capacity"],
        2_000_000
    );
    assert_eq!(
        json["data"]["screen_capture_capacity"]["physical_transition_backend_capacity"],
        2_000_000
    );
    assert_eq!(
        json["data"]["screen_capture_capacity"]["physical_reserved_bytes"],
        0
    );
    assert_eq!(
        json["data"]["screen_capture_capacity"]["physical_available_bytes"],
        2_000_000
    );
    assert_eq!(
        json["data"]["screen_capture_capacity"]["steady_total_byte_budget"],
        123
    );
    assert_eq!(
        json["data"]["screen_capture_capacity"]["steady_publication_byte_budget"],
        123
    );
    assert!(json["data"]["screen_capture_capacity"]["analysis_retained_bytes"].is_null());
    assert_eq!(json["data"]["latest_frame"]["frame_token"], 77);
    assert_eq!(
        json["data"]["latest_frame"]["compositor_backend"],
        "gpu_fallback"
    );
    assert_eq!(
        json["data"]["latest_frame"]["output_frame_source"],
        "routed_reuse"
    );
    assert_eq!(
        json["data"]["latest_frame"]["output_reuses_published_frame"],
        true
    );
    assert_eq!(
        json["data"]["latest_frame"]["output_brightness_generation"],
        5
    );
    assert_eq!(json["data"]["latest_frame"]["output_routing_signature"], 7);
    assert_eq!(
        json["data"]["latest_frame"]["output_zone_shape_signature"],
        11
    );
    assert_eq!(
        json["data"]["latest_frame"]["output_unassigned_behavior_generation"],
        13
    );
    assert_eq!(json["data"]["latest_frame"]["devices_written"], 3);
    assert_eq!(json["data"]["latest_frame"]["total_leds"], 144);
    assert_eq!(json["data"]["latest_frame"]["gpu_zone_sampling"], true);
    assert_eq!(json["data"]["latest_frame"]["gpu_sample_deferred"], true);
    assert_eq!(json["data"]["latest_frame"]["gpu_sample_stale"], true);
    assert_eq!(json["data"]["latest_frame"]["gpu_sample_retry_hit"], true);
    assert_eq!(
        json["data"]["latest_frame"]["gpu_sample_queue_saturated"],
        true
    );
    assert_eq!(
        json["data"]["latest_frame"]["gpu_sample_wait_blocked"],
        true
    );
    assert_eq!(
        json["data"]["latest_frame"]["gpu_sample_cpu_fallback"],
        true
    );
    assert_eq!(json["data"]["latest_frame"]["preview_surface"], true);
    assert_eq!(
        json["data"]["latest_frame"]["scene_canvas_forced_surface"],
        true
    );
    assert_eq!(json["data"]["latest_frame"]["jitter_ms"], 0.03);
    assert_eq!(json["data"]["latest_frame"]["input_sampling_ms"], 0.1);
    assert_eq!(json["data"]["latest_frame"]["producer_ms"], 0.5);
    assert_eq!(json["data"]["latest_frame"]["producer_render_ms"], 0.32);
    assert_eq!(
        json["data"]["latest_frame"]["producer_preview_compose_ms"],
        0.06
    );
    assert_eq!(json["data"]["latest_frame"]["composition_ms"], 0.2);
    assert_eq!(json["data"]["latest_frame"]["effect_rendering_ms"], 0.7);
    assert_eq!(json["data"]["latest_frame"]["spatial_sampling_ms"], 0.15);
    assert_eq!(json["data"]["latest_frame"]["device_output_ms"], 0.25);
    assert_eq!(json["data"]["latest_frame"]["preview_postprocess_ms"], 0.0);
    assert_eq!(json["data"]["latest_frame"]["event_bus_ms"], 0.12);
    assert_eq!(
        json["data"]["latest_frame"]["coordination_overhead_ms"],
        0.05
    );
    assert_eq!(json["data"]["latest_frame"]["publish_frame_data_ms"], 0.03);
    assert_eq!(json["data"]["latest_frame"]["publish_zone_canvas_ms"], 0.02);
    assert!(
        json["data"]["latest_frame"]
            .get("publish_group_canvas_ms")
            .is_none()
    );
    assert!(
        json["data"]["latest_frame"]
            .get("render_group_count")
            .is_none()
    );
    assert_eq!(json["data"]["latest_frame"]["publish_preview_ms"], 0.06);
    assert_eq!(json["data"]["latest_frame"]["publish_events_ms"], 0.01);
    assert_eq!(json["data"]["latest_frame"]["cpu_readback_skipped"], true);
    assert_eq!(json["data"]["latest_frame"]["gpu_readback_failed"], true);
    assert_eq!(
        json["data"]["latest_frame"]["render_surfaces"]["scene_pool_slot_count"],
        6
    );
    assert_eq!(
        json["data"]["latest_frame"]["render_surfaces"]["preview_pool_slot_count"],
        0
    );
    assert_eq!(
        json["data"]["latest_frame"]["render_surfaces"]["compositor_pool_slot_count"],
        0
    );
    assert_eq!(
        json["data"]["latest_frame"]["render_surfaces"]["canvas_receivers"],
        2
    );
    assert_eq!(json["data"]["latest_frame"]["full_frame_copy_count"], 2);
    assert_eq!(json["data"]["latest_frame"]["full_frame_copy_kb"], 250.0);
    assert_eq!(
        json["data"]["latest_frame"]["producer_full_frame_copy_count"],
        1
    );
    assert_eq!(
        json["data"]["latest_frame"]["producer_full_frame_copy_kb"],
        125.0
    );
    assert_eq!(
        json["data"]["latest_frame"]["producer_full_frame_copy_reason"],
        "producer_test"
    );
    assert_eq!(
        json["data"]["latest_frame"]["publication_full_frame_copy_count"],
        1
    );
    assert_eq!(
        json["data"]["latest_frame"]["publication_full_frame_copy_kb"],
        125.0
    );
    assert_eq!(
        json["data"]["latest_frame"]["publication_full_frame_copy_reason"],
        "publication_test"
    );
    assert_eq!(json["data"]["latest_frame"]["output_errors"], 0);
    let expected_effect_health = serde_json::to_value(super::effect_health_status(
        crate::performance::EffectHealthSummary {
            errors_total: 1,
            fallbacks_applied_total: 1,
            producer_gpu_readback_failures_total: 2,
        },
    ))
    .expect("effect health status should serialize");
    assert_eq!(json["data"]["effect_health"], expected_effect_health);
    assert_eq!(json["data"]["preview_runtime"]["canvas_receivers"], 1);
    assert_eq!(json["data"]["preview_runtime"]["scene_canvas_receivers"], 1);
    assert_eq!(
        json["data"]["preview_runtime"]["screen_canvas_receivers"],
        1
    );
    assert_eq!(
        json["data"]["preview_runtime"]["canvas_frames_published"],
        1
    );
    assert_eq!(
        json["data"]["preview_runtime"]["scene_canvas_frames_published"],
        1
    );
    assert_eq!(
        json["data"]["preview_runtime"]["screen_canvas_frames_published"],
        1
    );
    assert_eq!(
        json["data"]["preview_runtime"]["latest_canvas_frame_number"],
        88
    );
    assert_eq!(
        json["data"]["preview_runtime"]["latest_scene_canvas_frame_number"],
        66
    );
    assert_eq!(
        json["data"]["preview_runtime"]["latest_screen_canvas_frame_number"],
        45
    );
    assert_eq!(
        json["data"]["preview_runtime"]["canvas_demand"]["subscribers"],
        1
    );
    assert_eq!(
        json["data"]["preview_runtime"]["canvas_demand"]["max_fps"],
        24
    );
    assert_eq!(
        json["data"]["preview_runtime"]["canvas_demand"]["max_width"],
        640
    );
    assert_eq!(
        json["data"]["preview_runtime"]["canvas_demand"]["max_height"],
        360
    );
    assert_eq!(
        json["data"]["preview_runtime"]["canvas_demand"]["any_jpeg"],
        true
    );
    assert_eq!(
        json["data"]["preview_runtime"]["scene_canvas_demand"]["subscribers"],
        1
    );
    assert_eq!(
        json["data"]["preview_runtime"]["scene_canvas_demand"]["max_fps"],
        12
    );
    assert_eq!(
        json["data"]["preview_runtime"]["scene_canvas_demand"]["max_width"],
        320
    );
    assert_eq!(
        json["data"]["preview_runtime"]["scene_canvas_demand"]["max_height"],
        180
    );
    assert_eq!(
        json["data"]["preview_runtime"]["scene_canvas_demand"]["any_rgb"],
        true
    );
    assert_eq!(
        json["data"]["preview_runtime"]["screen_canvas_demand"]["subscribers"],
        1
    );
    assert_eq!(
        json["data"]["preview_runtime"]["screen_canvas_demand"]["any_full_resolution"],
        true
    );
    assert_eq!(
        json["data"]["preview_runtime"]["screen_canvas_demand"]["any_rgba"],
        true
    );
}
