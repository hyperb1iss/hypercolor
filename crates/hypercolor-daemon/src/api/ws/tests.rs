use std::sync::{Arc, LazyLock, Mutex as StdMutex, PoisonError};

use axum::body::Bytes;
use axum::extract::ws::Utf8Bytes;
use axum::response::IntoResponse;
use tokio::sync::watch;

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_types::canvas::{
    Canvas, PublishedSurface, Rgba, linear_to_srgb_u8, srgb_u8_to_linear,
};
use hypercolor_types::controls::{ControlSurfaceEvent, ControlValue, ControlValueMap};
use hypercolor_types::device::{ConnectionType, DeviceOrigin};
use hypercolor_types::event::{FrameData, FrameTiming, HypercolorEvent, SpectrumData, ZoneColors};
use hypercolor_types::scene::{RenderGroupId, RenderGroupRole, SceneId};

use super::cache::{
    FrameRelayMessage, WS_CANVAS_BINARY_CACHE, WS_CANVAS_HEADER, WS_CANVAS_JPEG_BODY_BUILD_COUNT,
    WS_CANVAS_JPEG_BODY_CACHE_HIT_COUNT, WS_CANVAS_PAYLOAD_BUILD_COUNT,
    WS_CANVAS_PAYLOAD_CACHE_HIT_COUNT, WS_CANVAS_RAW_BODY_BUILD_COUNT,
    WS_CANVAS_RAW_BODY_CACHE_HIT_COUNT, WS_DISPLAY_PREVIEW_HEADER, WS_FRAME_PAYLOAD_BUILD_COUNT,
    WS_FRAME_PAYLOAD_CACHE, WS_FRAME_PAYLOAD_CACHE_HIT_COUNT, WS_SCREEN_CANVAS_HEADER,
    WS_SPECTRUM_PAYLOAD_BUILD_COUNT, WS_SPECTRUM_PAYLOAD_CACHE,
    WS_SPECTRUM_PAYLOAD_CACHE_HIT_COUNT, WS_WEB_VIEWPORT_CANVAS_HEADER, cached_frame_payload,
    cached_spectrum_payload, encode_cached_canvas_preview_binary, encode_canvas_binary_with_header,
    encode_canvas_preview_binary, encode_frame_binary, encode_frame_binary_selected,
    encode_spectrum_binary, reset_canvas_jpeg_body_cache_for_tests,
    reset_canvas_raw_body_cache_for_tests, reset_preview_jpeg_encoders_for_tests,
};
use super::command::{
    command_response_from_http, dispatch_command, normalize_command_path, parse_command_method,
};
use super::preview_encode::{
    PreviewJpegEncoder, PreviewRawEncoder, encode_canvas_jpeg_binary_stateless,
    encode_canvas_jpeg_payload_scaled_stateless,
};
use super::protocol::{
    ActiveFramesConfig, CanvasFormat, ChannelConfig, ChannelConfigPatch, ChannelSet, FrameFormat,
    FrameZoneSelection, FramesConfig, ServerMessage, SubscriptionState, WsChannel,
    event_message_parts, parse_channels, should_relay_event, to_snake_case,
    unique_sorted_channel_names, ws_capabilities,
};
use super::relays::{
    build_device_metrics_message, build_metrics_message, publish_subscriptions, relay_canvas,
    relay_device_metrics, relay_frames, relay_metrics, relay_screen_canvas, relay_spectrum,
    relay_web_viewport_canvas, sync_preview_receiver, try_enqueue_json,
};
use crate::api::AppState;
use crate::api::security::{RequestAuthContext, SecurityState};
use crate::device_metrics::{DeviceMetrics, DeviceMetricsSnapshot};
use crate::performance::{CompositorBackendKind, FrameTimeline, LatestFrameMetrics};
use crate::preview_runtime::{
    PreviewFrameReceiver, PreviewPixelFormat, PreviewRuntime, PreviewStreamDemand,
};
use crate::session::OutputPowerState;

static WS_CACHE_TEST_LOCK: LazyLock<StdMutex<()>> = LazyLock::new(|| StdMutex::new(()));

#[cfg(feature = "servo")]
type ServoEffectHealthForTests = hypercolor_core::effect::ServoTelemetrySnapshot;

#[cfg(feature = "servo")]
fn current_servo_effect_health() -> ServoEffectHealthForTests {
    hypercolor_core::effect::servo_telemetry_snapshot()
}

#[cfg(not(feature = "servo"))]
const fn current_servo_effect_health() -> ServoEffectHealthForTests {
    ServoEffectHealthForTests {
        soft_stalls_total: 0,
        breaker_opens_total: 0,
        session_creates_total: 0,
        session_create_failures_total: 0,
        session_create_wait_total_us: 0,
        session_create_wait_max_us: 0,
        page_loads_total: 0,
        page_load_failures_total: 0,
        page_load_wait_total_us: 0,
        page_load_wait_max_us: 0,
        detached_destroys_total: 0,
        detached_destroy_failures_total: 0,
        render_requests_total: 0,
        render_queue_wait_total_us: 0,
        render_queue_wait_max_us: 0,
    }
}

#[cfg(not(feature = "servo"))]
#[derive(Clone, Copy)]
struct ServoEffectHealthForTests {
    soft_stalls_total: u64,
    breaker_opens_total: u64,
    session_creates_total: u64,
    session_create_failures_total: u64,
    session_create_wait_total_us: u64,
    session_create_wait_max_us: u64,
    page_loads_total: u64,
    page_load_failures_total: u64,
    page_load_wait_total_us: u64,
    page_load_wait_max_us: u64,
    detached_destroys_total: u64,
    detached_destroy_failures_total: u64,
    render_requests_total: u64,
    render_queue_wait_total_us: u64,
    render_queue_wait_max_us: u64,
}

fn secured_state() -> Arc<AppState> {
    let mut state = AppState::new();
    state.security_state =
        SecurityState::with_keys(Some("hc_ak_control_test"), Some("hc_ak_r_read_test"));
    Arc::new(state)
}

fn reset_ws_payload_caches() {
    WS_FRAME_PAYLOAD_BUILD_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
    WS_FRAME_PAYLOAD_CACHE_HIT_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
    WS_CANVAS_PAYLOAD_BUILD_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
    WS_CANVAS_PAYLOAD_CACHE_HIT_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
    WS_CANVAS_RAW_BODY_BUILD_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
    WS_CANVAS_RAW_BODY_CACHE_HIT_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
    WS_CANVAS_JPEG_BODY_BUILD_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
    WS_CANVAS_JPEG_BODY_CACHE_HIT_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
    WS_SPECTRUM_PAYLOAD_BUILD_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
    WS_SPECTRUM_PAYLOAD_CACHE_HIT_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
    for shard in WS_FRAME_PAYLOAD_CACHE.iter() {
        shard.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }
    for shard in WS_CANVAS_BINARY_CACHE.iter() {
        shard.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }
    for shard in WS_SPECTRUM_PAYLOAD_CACHE.iter() {
        shard.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }
    reset_canvas_raw_body_cache_for_tests();
    reset_canvas_jpeg_body_cache_for_tests();
    reset_preview_jpeg_encoders_for_tests();
}

fn sample_frame() -> FrameData {
    FrameData {
        frame_number: 42,
        timestamp_ms: 1234,
        zones: vec![
            ZoneColors {
                zone_id: "left".to_owned(),
                colors: vec![[255, 0, 0], [128, 0, 0]],
            },
            ZoneColors {
                zone_id: "right".to_owned(),
                colors: vec![[0, 0, 255], [0, 0, 128]],
            },
        ],
    }
}

fn selected_frame_zones<'a>(
    zones: &'a [hypercolor_types::event::ZoneColors],
    selected: &[String],
) -> Vec<&'a hypercolor_types::event::ZoneColors> {
    FrameZoneSelection::new(selected).select(zones)
}

fn filter_frame_zones(
    zones: &[hypercolor_types::event::ZoneColors],
    selected: &[String],
) -> Vec<hypercolor_types::event::ZoneColors> {
    selected_frame_zones(zones, selected)
        .into_iter()
        .cloned()
        .collect()
}

#[tokio::test]
async fn metrics_message_includes_latest_frame_timeline() {
    let state = Arc::new(AppState::new());
    let mut preview_rx = state.preview_runtime.canvas_receiver();
    let mut screen_preview_rx = state.preview_runtime.screen_canvas_receiver();
    preview_rx.update_demand(PreviewStreamDemand {
        fps: 20,
        format: PreviewPixelFormat::Jpeg,
        width: 640,
        height: 360,
    });
    screen_preview_rx.update_demand(PreviewStreamDemand {
        fps: 30,
        format: PreviewPixelFormat::Rgba,
        width: 0,
        height: 0,
    });
    let canvas_frame = CanvasFrame::from_canvas(&Canvas::new(2, 1), 88, 44);
    let screen_frame = CanvasFrame::from_canvas(&Canvas::new(1, 1), 45, 21);
    let _ = state.event_bus.canvas_sender().send(canvas_frame.clone());
    let _ = state
        .event_bus
        .screen_canvas_sender()
        .send(screen_frame.clone());
    state
        .preview_runtime
        .record_canvas_publication(canvas_frame.frame_number, canvas_frame.timestamp_ms);
    state
        .preview_runtime
        .record_screen_canvas_publication(screen_frame.frame_number, screen_frame.timestamp_ms);
    {
        let mut performance = state.performance.write().await;
        performance.record_effect_error();
        performance.record_effect_error();
        performance.record_effect_fallback_applied();
        performance.record_frame(LatestFrameMetrics {
            timestamp_ms: 1234,
            input_us: 200,
            producer_us: 900,
            producer_render_us: 640,
            producer_scene_compose_us: 110,
            composition_us: 300,
            render_us: 1_200,
            sample_us: 150,
            push_us: 250,
            postprocess_us: 0,
            publish_us: 180,
            publish_frame_data_us: 70,
            publish_group_canvas_us: 20,
            publish_preview_us: 80,
            publish_events_us: 10,
            overhead_us: 70,
            total_us: 1_850,
            wake_late_us: 220,
            jitter_us: 440,
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
            cpu_sampling_late_readback: true,
            cpu_readback_skipped: true,
            compositor_backend: CompositorBackendKind::Gpu,
            logical_layer_count: 2,
            render_group_count: 1,
            scene_active: true,
            scene_transition_active: true,
            render_surface_slot_count: 6,
            render_surface_free_slots: 1,
            render_surface_published_slots: 4,
            render_surface_dequeued_slots: 1,
            scene_pool_saturation_reallocs: 0,
            direct_pool_saturation_reallocs: 0,
            scene_pool_grown_slots: 0,
            direct_pool_grown_slots: 0,
            scene_pool_slot_count: 10,
            scene_pool_max_slots: 12,
            direct_pool_slot_count: 6,
            direct_pool_max_slots: 8,
            scene_pool_shared_published_slots: 9,
            scene_pool_max_ref_count: 3,
            direct_pool_shared_published_slots: 4,
            direct_pool_max_ref_count: 2,
            canvas_receiver_count: 2,
            full_frame_copy_count: 1,
            full_frame_copy_bytes: 2_048,
            output_errors: 1,
            timeline: FrameTimeline {
                frame_token: 77,
                budget_us: 16_666,
                scene_snapshot_done_us: 120,
                input_done_us: 320,
                producer_done_us: 1_040,
                composition_done_us: 1_340,
                sample_done_us: 1_490,
                output_done_us: 1_740,
                publish_done_us: 1_820,
                frame_done_us: 1_850,
            },
        });
    }
    {
        let mut display_frames = state.display_frames.write().await;
        display_frames.record_write_attempt(false);
        display_frames.record_write_success();
        display_frames.record_write_attempt(true);
        display_frames.record_write_failure();
    }

    let ServerMessage::Metrics { data, .. } = build_metrics_message(&state, 0.0).await else {
        panic!("expected metrics message");
    };
    let json = serde_json::to_value(&data).expect("metrics payload should serialize");
    let servo_health = current_servo_effect_health();
    let usb_actor_metrics = hypercolor_core::device::usb_actor_metrics_snapshot();

    assert_eq!(json["timeline"]["frame_token"], 77);
    assert_eq!(json["timeline"]["compositor_backend"], "gpu");
    assert_eq!(json["timeline"]["gpu_zone_sampling"], true);
    assert_eq!(json["timeline"]["gpu_sample_deferred"], true);
    assert_eq!(json["timeline"]["gpu_sample_stale"], true);
    assert_eq!(json["timeline"]["gpu_sample_retry_hit"], true);
    assert_eq!(json["timeline"]["gpu_sample_queue_saturated"], true);
    assert_eq!(json["timeline"]["gpu_sample_wait_blocked"], true);
    assert_eq!(json["timeline"]["gpu_sample_cpu_fallback"], true);
    assert_eq!(json["timeline"]["cpu_sampling_late_readback"], true);
    assert_eq!(json["timeline"]["cpu_readback_skipped"], true);
    assert_eq!(json["timeline"]["budget_ms"], 16.67);
    assert_eq!(json["timeline"]["wake_late_ms"], 0.22);
    assert_eq!(json["pacing"]["push_avg_ms"], 0.25);
    assert_eq!(json["pacing"]["push_p95_ms"], 0.25);
    assert_eq!(json["pacing"]["publish_avg_ms"], 0.18);
    assert_eq!(json["pacing"]["publish_p95_ms"], 0.18);
    assert_eq!(json["pacing"]["gpu_zone_sampling"], 1);
    assert_eq!(json["pacing"]["gpu_sample_deferred"], 1);
    assert_eq!(json["pacing"]["gpu_sample_stale"], 1);
    assert_eq!(json["pacing"]["gpu_sample_retry_hit"], 1);
    assert_eq!(json["pacing"]["gpu_sample_queue_saturated"], 1);
    assert_eq!(json["pacing"]["gpu_sample_wait_blocked"], 1);
    assert_eq!(json["pacing"]["gpu_sample_cpu_fallback"], 1);
    assert_eq!(json["pacing"]["cpu_sampling_late_readback"], 1);
    assert_eq!(json["pacing"]["output_error_frames"], 1);
    assert_eq!(json["pacing"]["full_frame_copy_frames"], 1);
    assert_eq!(json["render_surfaces"]["scene_pool_slot_count"], 10);
    assert_eq!(json["render_surfaces"]["scene_pool_max_slots"], 12);
    assert_eq!(json["render_surfaces"]["direct_pool_slot_count"], 6);
    assert_eq!(json["render_surfaces"]["direct_pool_max_slots"], 8);
    assert_eq!(
        json["render_surfaces"]["scene_pool_shared_published_slots"],
        9
    );
    assert_eq!(json["render_surfaces"]["scene_pool_max_ref_count"], 3);
    assert_eq!(
        json["render_surfaces"]["direct_pool_shared_published_slots"],
        4
    );
    assert_eq!(json["render_surfaces"]["direct_pool_max_ref_count"], 2);
    assert_eq!(json["effect_health"]["errors_total"], 2);
    assert_eq!(json["effect_health"]["fallbacks_applied_total"], 1);
    assert_eq!(
        json["effect_health"]["servo_soft_stalls_total"],
        servo_health.soft_stalls_total
    );
    assert_eq!(
        json["effect_health"]["servo_breaker_opens_total"],
        servo_health.breaker_opens_total
    );
    assert_eq!(
        json["effect_health"]["servo_session_creates_total"],
        servo_health.session_creates_total
    );
    assert_eq!(
        json["effect_health"]["servo_session_create_failures_total"],
        servo_health.session_create_failures_total
    );
    assert_eq!(
        json["effect_health"]["servo_session_create_wait_total_ms"],
        std::time::Duration::from_micros(servo_health.session_create_wait_total_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_session_create_wait_max_ms"],
        std::time::Duration::from_micros(servo_health.session_create_wait_max_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_page_loads_total"],
        servo_health.page_loads_total
    );
    assert_eq!(
        json["effect_health"]["servo_page_load_failures_total"],
        servo_health.page_load_failures_total
    );
    assert_eq!(
        json["effect_health"]["servo_page_load_wait_total_ms"],
        std::time::Duration::from_micros(servo_health.page_load_wait_total_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_page_load_wait_max_ms"],
        std::time::Duration::from_micros(servo_health.page_load_wait_max_us).as_secs_f64() * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_detached_destroys_total"],
        servo_health.detached_destroys_total
    );
    assert_eq!(
        json["effect_health"]["servo_detached_destroy_failures_total"],
        servo_health.detached_destroy_failures_total
    );
    assert_eq!(
        json["effect_health"]["servo_render_requests_total"],
        servo_health.render_requests_total
    );
    assert_eq!(
        json["effect_health"]["servo_render_queue_wait_total_ms"],
        std::time::Duration::from_micros(servo_health.render_queue_wait_total_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_queue_wait_max_ms"],
        std::time::Duration::from_micros(servo_health.render_queue_wait_max_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(json["display_output"]["write_attempts_total"], 2);
    assert_eq!(json["display_output"]["write_successes_total"], 1);
    assert_eq!(json["display_output"]["write_failures_total"], 1);
    assert_eq!(json["display_output"]["retry_attempts_total"], 1);
    assert_eq!(
        json["display_output"]["display_lane"]["display_frames_total"],
        usb_actor_metrics.display_frames_total
    );
    assert_eq!(
        json["display_output"]["display_lane"]["display_frames_delayed_for_led_total"],
        usb_actor_metrics.display_frames_delayed_for_led_total
    );
    assert_eq!(
        json["display_output"]["display_lane"]["display_led_priority_wait_total_ms"],
        std::time::Duration::from_micros(usb_actor_metrics.display_led_priority_wait_total_us)
            .as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["display_output"]["display_lane"]["display_led_priority_wait_max_ms"],
        std::time::Duration::from_micros(usb_actor_metrics.display_led_priority_wait_max_us)
            .as_secs_f64()
            * 1000.0
    );
    assert!(json["display_output"]["last_failure_age_ms"].is_number());
    assert_eq!(json["timeline"]["logical_layer_count"], 2);
    assert_eq!(json["timeline"]["render_group_count"], 1);
    assert_eq!(json["timeline"]["scene_active"], true);
    assert_eq!(json["timeline"]["scene_transition_active"], true);
    assert_eq!(json["timeline"]["scene_snapshot_done_ms"], 0.12);
    assert_eq!(json["timeline"]["composition_done_ms"], 1.34);
    assert_eq!(json["timeline"]["frame_done_ms"], 1.85);
    assert_eq!(json["stages"]["producer_effect_rendering_ms"], 0.64);
    assert_eq!(json["stages"]["producer_preview_compose_ms"], 0.11);
    assert_eq!(json["stages"]["publish_frame_data_ms"], 0.07);
    assert_eq!(json["stages"]["publish_group_canvas_ms"], 0.02);
    assert_eq!(json["stages"]["publish_preview_ms"], 0.08);
    assert_eq!(json["stages"]["publish_events_ms"], 0.01);
    assert_eq!(json["fps"]["ceiling"], 60);
    assert_eq!(json["render_surfaces"]["slot_count"], 6);
    assert_eq!(json["render_surfaces"]["published_slots"], 4);
    assert_eq!(json["render_surfaces"]["canvas_receivers"], 2);
    assert_eq!(
        json["render_surfaces"]["preview_pool_saturation_reallocs"],
        0
    );
    assert_eq!(json["render_surfaces"]["preview_pool_grown_slots"], 0);
    assert_eq!(json["preview"]["canvas_receivers"], 1);
    assert_eq!(json["preview"]["screen_canvas_receivers"], 1);
    assert_eq!(json["preview"]["canvas_frames_published"], 1);
    assert_eq!(json["preview"]["screen_canvas_frames_published"], 1);
    assert_eq!(json["preview"]["latest_canvas_frame_number"], 88);
    assert_eq!(json["preview"]["latest_screen_canvas_frame_number"], 45);
    assert_eq!(json["preview"]["canvas_demand"]["subscribers"], 1);
    assert_eq!(json["preview"]["canvas_demand"]["max_fps"], 20);
    assert_eq!(json["preview"]["canvas_demand"]["max_width"], 640);
    assert_eq!(json["preview"]["canvas_demand"]["max_height"], 360);
    assert_eq!(json["preview"]["canvas_demand"]["any_jpeg"], true);
    assert_eq!(json["preview"]["screen_canvas_demand"]["subscribers"], 1);
    assert_eq!(
        json["preview"]["screen_canvas_demand"]["any_full_resolution"],
        true
    );
    assert_eq!(json["preview"]["screen_canvas_demand"]["any_rgba"], true);
}

#[test]
fn device_metrics_message_uses_shared_snapshot() {
    let state = Arc::new(AppState::new());
    let device_id = hypercolor_types::device::DeviceId::new();
    state.device_metrics.store(Arc::new(DeviceMetricsSnapshot {
        taken_at_ms: 2_500,
        items: vec![DeviceMetrics {
            id: device_id,
            fps_actual: 58.5,
            fps_target: 60,
            payload_bps_estimate: 2_048,
            avg_latency_ms: 11,
            frames_sent: 300,
            frames_dropped: 4,
            errors_total: 1,
            last_error: Some("socket timeout".to_owned()),
            last_sent_ago_ms: Some(12),
        }],
    }));

    let ServerMessage::DeviceMetrics { data, .. } = build_device_metrics_message(&state) else {
        panic!("expected device_metrics message");
    };

    assert_eq!(data.taken_at_ms, 2_500);
    assert_eq!(data.items.len(), 1);
    assert_eq!(data.items[0].id, device_id);
    assert_eq!(data.items[0].payload_bps_estimate, 2_048);
}

#[tokio::test]
async fn relay_metrics_wakes_when_subscription_changes() {
    let state = Arc::new(AppState::new());
    let initial_subscriptions = SubscriptionState::default();
    let (subscriptions_tx, subscriptions_rx) = watch::channel(initial_subscriptions.clone());
    let (json_tx, mut json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(1);

    let relay_handle = tokio::spawn(relay_metrics(Arc::clone(&state), json_tx, subscriptions_rx));

    let mut subscriptions = initial_subscriptions;
    subscriptions.channels.insert(WsChannel::Metrics);
    subscriptions.config.metrics.interval_ms = 100;
    publish_subscriptions(&subscriptions_tx, &subscriptions);

    let message = tokio::time::timeout(std::time::Duration::from_millis(250), json_rx.recv())
        .await
        .expect("metrics relay should wake without idle polling");
    assert!(message.is_some());

    relay_handle.abort();
}

#[tokio::test]
async fn relay_device_metrics_wakes_when_subscription_changes() {
    let state = Arc::new(AppState::new());
    state.device_metrics.store(Arc::new(DeviceMetricsSnapshot {
        taken_at_ms: 4_200,
        items: vec![DeviceMetrics {
            id: hypercolor_types::device::DeviceId::new(),
            fps_actual: 60.0,
            fps_target: 60,
            payload_bps_estimate: 512,
            avg_latency_ms: 8,
            frames_sent: 42,
            frames_dropped: 0,
            errors_total: 0,
            last_error: None,
            last_sent_ago_ms: Some(7),
        }],
    }));
    let initial_subscriptions = SubscriptionState::default();
    let (subscriptions_tx, subscriptions_rx) = watch::channel(initial_subscriptions.clone());
    let (json_tx, mut json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(1);

    let relay_handle = tokio::spawn(relay_device_metrics(
        Arc::clone(&state),
        json_tx,
        subscriptions_rx,
    ));

    let mut subscriptions = initial_subscriptions;
    subscriptions.channels.insert(WsChannel::DeviceMetrics);
    subscriptions.config.device_metrics.interval_ms = 100;
    publish_subscriptions(&subscriptions_tx, &subscriptions);

    let message = tokio::time::timeout(std::time::Duration::from_millis(250), json_rx.recv())
        .await
        .expect("device_metrics relay should wake without idle polling")
        .expect("device_metrics relay should emit a message");
    let payload: serde_json::Value =
        serde_json::from_str(message.as_str()).expect("device_metrics payload should parse");
    assert_eq!(payload["type"], "device_metrics");
    assert_eq!(payload["data"]["taken_at_ms"], 4_200);

    relay_handle.abort();
}

#[tokio::test]
async fn relay_frames_wakes_when_subscription_changes() {
    let initial_subscriptions = SubscriptionState::default();
    let (subscriptions_tx, subscriptions_rx) = watch::channel(initial_subscriptions.clone());
    let (json_tx, _json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(1);
    let (binary_tx, mut binary_rx) = tokio::sync::mpsc::channel::<Bytes>(1);
    let state = Arc::new(AppState::new());
    let _ = state.event_bus.frame_sender().send(sample_frame());

    let relay_handle = tokio::spawn(relay_frames(
        Arc::clone(&state),
        json_tx,
        binary_tx,
        subscriptions_rx,
    ));
    assert_eq!(state.event_bus.frame_receiver_count(), 0);

    let mut subscriptions = initial_subscriptions;
    subscriptions.channels.insert(WsChannel::Frames);
    publish_subscriptions(&subscriptions_tx, &subscriptions);

    let payload = tokio::time::timeout(std::time::Duration::from_millis(250), binary_rx.recv())
        .await
        .expect("frame relay should wake on subscription changes")
        .expect("frame relay should publish the latest cached frame");
    assert_eq!(payload.first().copied(), Some(0x01));
    assert_eq!(state.event_bus.frame_receiver_count(), 1);

    subscriptions.channels.remove(WsChannel::Frames);
    publish_subscriptions(&subscriptions_tx, &subscriptions);
    tokio::time::timeout(std::time::Duration::from_millis(250), async {
        loop {
            if state.event_bus.frame_receiver_count() == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("frame receiver should be dropped after unsubscribe");

    relay_handle.abort();
}

#[tokio::test]
async fn relay_spectrum_subscribes_lazily() {
    let initial_subscriptions = SubscriptionState::default();
    let (subscriptions_tx, subscriptions_rx) = watch::channel(initial_subscriptions.clone());
    let (json_tx, _json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(1);
    let (binary_tx, mut binary_rx) = tokio::sync::mpsc::channel::<Bytes>(1);
    let state = Arc::new(AppState::new());
    let _ = state
        .event_bus
        .spectrum_sender()
        .send(SpectrumData::empty());

    let relay_handle = tokio::spawn(relay_spectrum(
        Arc::clone(&state),
        json_tx,
        binary_tx,
        subscriptions_rx,
    ));
    assert_eq!(state.event_bus.spectrum_receiver_count(), 0);

    let mut subscriptions = initial_subscriptions;
    subscriptions.channels.insert(WsChannel::Spectrum);
    publish_subscriptions(&subscriptions_tx, &subscriptions);

    let payload = tokio::time::timeout(std::time::Duration::from_millis(250), binary_rx.recv())
        .await
        .expect("spectrum relay should wake on subscription changes")
        .expect("spectrum relay should publish the latest cached spectrum");
    assert_eq!(payload.first().copied(), Some(0x02));
    assert_eq!(state.event_bus.spectrum_receiver_count(), 1);

    subscriptions.channels.remove(WsChannel::Spectrum);
    publish_subscriptions(&subscriptions_tx, &subscriptions);
    tokio::time::timeout(std::time::Duration::from_millis(250), async {
        loop {
            if state.event_bus.spectrum_receiver_count() == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("spectrum receiver should be dropped after unsubscribe");

    relay_handle.abort();
}

async fn assert_backpressure_notice_does_not_repeat(
    expected_channel: &'static str,
    relay_handle: tokio::task::JoinHandle<()>,
    json_rx: &mut tokio::sync::mpsc::Receiver<Utf8Bytes>,
) {
    let first = tokio::time::timeout(std::time::Duration::from_millis(300), json_rx.recv())
        .await
        .expect("relay should emit an initial backpressure notice")
        .expect("backpressure notice should be delivered");
    let payload: serde_json::Value =
        serde_json::from_str(first.as_str()).expect("backpressure notice should parse");

    assert_eq!(payload["type"], "backpressure");
    assert_eq!(payload["channel"], expected_channel);
    assert_eq!(payload["dropped_frames"], 1);
    assert_eq!(payload["recommendation"], "reduce_fps");

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(700), json_rx.recv())
            .await
            .is_err(),
        "relay should not keep retrying the same payload after backpressure"
    );

    relay_handle.abort();
    let _ = relay_handle.await;
}

#[tokio::test]
async fn relay_canvas_clears_pending_send_after_backpressure() {
    let state = Arc::new(AppState::new());
    let mut subscriptions = SubscriptionState::default();
    subscriptions.channels.insert(WsChannel::Canvas);
    let (_subscriptions_tx, subscriptions_rx) = watch::channel(subscriptions);
    let (_power_state_tx, power_state_rx) = watch::channel(OutputPowerState::default());
    let (json_tx, mut json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(4);
    let (binary_tx, mut binary_rx) = tokio::sync::mpsc::channel::<Bytes>(1);
    binary_tx
        .try_send(Bytes::from_static(b"occupied"))
        .expect("binary queue should start full");

    let relay_handle = tokio::spawn(relay_canvas(
        Arc::clone(&state.preview_runtime),
        power_state_rx,
        json_tx,
        binary_tx,
        subscriptions_rx,
    ));

    assert_backpressure_notice_does_not_repeat("canvas", relay_handle, &mut json_rx).await;
    let _ = binary_rx.recv().await;
}

#[tokio::test]
async fn relay_screen_canvas_clears_pending_send_after_backpressure() {
    let state = Arc::new(AppState::new());
    let mut subscriptions = SubscriptionState::default();
    subscriptions.channels.insert(WsChannel::ScreenCanvas);
    let (_subscriptions_tx, subscriptions_rx) = watch::channel(subscriptions);
    let (json_tx, mut json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(4);
    let (binary_tx, mut binary_rx) = tokio::sync::mpsc::channel::<Bytes>(1);
    binary_tx
        .try_send(Bytes::from_static(b"occupied"))
        .expect("binary queue should start full");

    let relay_handle = tokio::spawn(relay_screen_canvas(
        Arc::clone(&state.preview_runtime),
        json_tx,
        binary_tx,
        subscriptions_rx,
    ));

    assert_backpressure_notice_does_not_repeat("screen_canvas", relay_handle, &mut json_rx).await;
    let _ = binary_rx.recv().await;
}

#[tokio::test]
async fn relay_web_viewport_canvas_clears_pending_send_after_backpressure() {
    let state = Arc::new(AppState::new());
    let mut subscriptions = SubscriptionState::default();
    subscriptions.channels.insert(WsChannel::WebViewportCanvas);
    let (_subscriptions_tx, subscriptions_rx) = watch::channel(subscriptions);
    let (json_tx, mut json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(4);
    let (binary_tx, mut binary_rx) = tokio::sync::mpsc::channel::<Bytes>(1);
    binary_tx
        .try_send(Bytes::from_static(b"occupied"))
        .expect("binary queue should start full");

    let relay_handle = tokio::spawn(relay_web_viewport_canvas(
        Arc::clone(&state.preview_runtime),
        json_tx,
        binary_tx,
        subscriptions_rx,
    ));

    assert_backpressure_notice_does_not_repeat("web_viewport_canvas", relay_handle, &mut json_rx)
        .await;
    let _ = binary_rx.recv().await;
}

#[test]
fn parse_channels_accepts_supported_channel() {
    let channels = vec![
        "events".to_owned(),
        "frames".to_owned(),
        "spectrum".to_owned(),
        "canvas".to_owned(),
        "screen_canvas".to_owned(),
        "metrics".to_owned(),
        "device_metrics".to_owned(),
    ];
    let parsed = parse_channels(&channels).expect("events should parse");
    assert_eq!(
        parsed,
        vec![
            WsChannel::Events,
            WsChannel::Frames,
            WsChannel::Spectrum,
            WsChannel::Canvas,
            WsChannel::ScreenCanvas,
            WsChannel::Metrics,
            WsChannel::DeviceMetrics,
        ]
    );
}

#[test]
fn parse_channels_rejects_unknown_channel() {
    let channels = vec!["unknown".to_owned()];
    let error = parse_channels(&channels).expect_err("unknown channel should fail");
    assert_eq!(error.code, "invalid_request");
}

#[test]
fn channel_config_apply_patch_supports_all_channels() {
    let mut config = ChannelConfig::default();
    let patch: ChannelConfigPatch = serde_json::from_value(serde_json::json!({
        "frames": {"fps": 30, "format": "binary"},
        "spectrum": {"fps": 20, "bins": 32},
        "canvas": {"fps": 60, "format": "jpeg", "width": 320, "height": 0},
        "screen_canvas": {"fps": 24, "format": "jpeg", "width": 480, "height": 270},
        "metrics": {"interval_ms": 500},
        "device_metrics": {"interval_ms": 250}
    }))
    .expect("valid json patch");

    config
        .apply_patch(patch)
        .expect("full channel config patch should be accepted");

    let json = serde_json::to_value(config).expect("config serializes");
    assert_eq!(json["canvas"]["fps"], 60);
    assert_eq!(json["canvas"]["format"], "jpeg");
    assert_eq!(json["canvas"]["width"], 320);
    assert_eq!(json["canvas"]["height"], 0);
    assert_eq!(json["screen_canvas"]["fps"], 24);
    assert_eq!(json["screen_canvas"]["format"], "jpeg");
    assert_eq!(json["screen_canvas"]["width"], 480);
    assert_eq!(json["screen_canvas"]["height"], 270);
    assert_eq!(json["metrics"]["interval_ms"], 500);
    assert_eq!(json["device_metrics"]["interval_ms"], 250);
}

#[test]
fn channel_config_defaults_are_stable() {
    let config = ChannelConfig::default();
    let json = serde_json::to_value(config).expect("config serializes");

    assert_eq!(json["frames"]["fps"], 30);
    assert_eq!(json["frames"]["format"], "binary");
    assert_eq!(json["spectrum"]["bins"], 64);
    assert_eq!(json["canvas"]["fps"], 15);
    assert_eq!(json["canvas"]["width"], 0);
    assert_eq!(json["canvas"]["height"], 0);
    assert_eq!(json["screen_canvas"]["fps"], 15);
    assert_eq!(json["screen_canvas"]["width"], 0);
    assert_eq!(json["screen_canvas"]["height"], 0);
    assert_eq!(json["metrics"]["interval_ms"], 1000);
    assert_eq!(json["device_metrics"]["interval_ms"], 1000);
}

#[test]
fn unique_channel_names_are_sorted() {
    let names =
        unique_sorted_channel_names(&[WsChannel::Events, WsChannel::Events, WsChannel::Events]);
    assert_eq!(names, vec!["events"]);
}

#[test]
fn snake_case_conversion_handles_camel_case() {
    assert_eq!(to_snake_case("DeviceDiscovered"), "device_discovered");
    assert_eq!(to_snake_case("Paused"), "paused");
}

#[test]
fn event_message_parts_unwraps_payload() {
    let event = HypercolorEvent::DeviceDiscoveryStarted {
        targets: vec!["fixture-driver".to_owned()],
    };

    let (event_name, event_data) = event_message_parts(&event);
    assert_eq!(event_name, "device_discovery_started");
    assert_eq!(event_data["targets"], serde_json::json!(["fixture-driver"]));
    assert!(event_data.get("type").is_none());
}

#[test]
fn event_message_parts_serializes_device_origin() {
    let event = HypercolorEvent::DeviceConnected {
        device_id: "fixture-device".to_owned(),
        name: "Fixture Device".to_owned(),
        origin: DeviceOrigin::native("fixture-driver", "usb", ConnectionType::Usb)
            .with_protocol_id("fixture/protocol"),
        led_count: 64,
        zones: vec![],
    };

    let (event_name, event_data) = event_message_parts(&event);
    assert_eq!(event_name, "device_connected");
    assert_eq!(event_data["origin"]["driver_id"], "fixture-driver");
    assert_eq!(event_data["origin"]["backend_id"], "usb");
    assert_eq!(event_data["origin"]["transport"], "usb");
    assert_eq!(event_data["origin"]["protocol_id"], "fixture/protocol");
    assert!(event_data.get("backend_id").is_none());
}

#[test]
fn event_message_parts_defaults_to_empty_object_for_unit_events() {
    let (event_name, event_data) = event_message_parts(&HypercolorEvent::Resumed);
    assert_eq!(event_name, "resumed");
    assert_eq!(event_data, serde_json::json!({}));
}

#[test]
fn event_message_parts_serializes_control_surface_changed() {
    let event = HypercolorEvent::ControlSurfaceChanged(ControlSurfaceEvent::ValuesChanged {
        surface_id: "driver:fixture".to_owned(),
        revision: 42,
        values: ControlValueMap::from([("dedup_threshold".to_owned(), ControlValue::Integer(7))]),
    });

    let (event_name, event_data) = event_message_parts(&event);
    assert_eq!(event_name, "control_surface_changed");
    assert_eq!(event_data["kind"], "values_changed");
    assert_eq!(event_data["surface_id"], "driver:fixture");
    assert_eq!(event_data["revision"], 42);
    assert_eq!(event_data["values"]["dedup_threshold"]["value"], 7);
}

#[test]
fn event_message_parts_serializes_render_group_changed() {
    let group_id = RenderGroupId::new();
    let event = HypercolorEvent::RenderGroupChanged {
        scene_id: SceneId::DEFAULT,
        group_id,
        role: RenderGroupRole::Display,
        kind: hypercolor_types::event::RenderGroupChangeKind::ControlsPatched,
    };

    let (event_name, event_data) = event_message_parts(&event);
    assert_eq!(event_name, "render_group_changed");
    assert_eq!(event_data["scene_id"], SceneId::DEFAULT.to_string());
    assert_eq!(event_data["group_id"], group_id.to_string());
    assert_eq!(event_data["role"], "display");
    assert_eq!(event_data["kind"], "controls_patched");
}

#[test]
fn event_message_parts_serializes_effect_degraded() {
    let group_id = RenderGroupId::new();
    let event = HypercolorEvent::EffectDegraded {
        effect_id: "effect-1".to_owned(),
        group_id: Some(group_id),
        group_name: Some("Display Face".to_owned()),
        state: hypercolor_types::event::EffectDegradationState::Failed,
        reason: Some("boom".to_owned()),
    };

    let (event_name, event_data) = event_message_parts(&event);
    assert_eq!(event_name, "effect_degraded");
    assert_eq!(event_data["effect_id"], "effect-1");
    assert_eq!(event_data["group_id"], group_id.to_string());
    assert_eq!(event_data["group_name"], "Display Face");
    assert_eq!(event_data["state"], "failed");
    assert_eq!(event_data["reason"], "boom");
}

#[test]
fn event_message_parts_serializes_active_scene_changed() {
    let current = SceneId::new();
    let event = HypercolorEvent::ActiveSceneChanged {
        previous: Some(SceneId::DEFAULT),
        current,
        current_name: "Movie Night".to_owned(),
        current_kind: hypercolor_types::scene::SceneKind::Named,
        current_mutation_mode: hypercolor_types::scene::SceneMutationMode::Snapshot,
        current_snapshot_locked: true,
        reason: hypercolor_types::event::SceneChangeReason::UserActivate,
    };

    let (event_name, event_data) = event_message_parts(&event);
    assert_eq!(event_name, "active_scene_changed");
    assert_eq!(event_data["previous"], SceneId::DEFAULT.to_string());
    assert_eq!(event_data["current"], current.to_string());
    assert_eq!(event_data["current_name"], "Movie Night");
    assert_eq!(event_data["current_kind"], "named");
    assert_eq!(event_data["current_mutation_mode"], "snapshot");
    assert_eq!(event_data["current_snapshot_locked"], true);
    assert_eq!(event_data["reason"], "user_activate");
}

#[test]
fn frame_rendered_events_are_suppressed_when_metrics_are_subscribed() {
    let channels = ChannelSet::from_channels(&[WsChannel::Events, WsChannel::Metrics]);
    let event = HypercolorEvent::FrameRendered {
        frame_number: 7,
        timing: FrameTiming {
            producer_us: 0,
            composition_us: 0,
            render_us: 0,
            sample_us: 0,
            push_us: 0,
            total_us: 0,
            budget_us: 16_666,
        },
    };

    assert!(!should_relay_event(&event, channels));
}

#[test]
fn frame_rendered_events_are_suppressed_when_device_metrics_are_subscribed() {
    let channels = ChannelSet::from_channels(&[WsChannel::Events, WsChannel::DeviceMetrics]);
    let event = HypercolorEvent::FrameRendered {
        frame_number: 7,
        timing: FrameTiming {
            producer_us: 0,
            composition_us: 0,
            render_us: 0,
            sample_us: 0,
            push_us: 0,
            total_us: 0,
            budget_us: 16_666,
        },
    };

    assert!(!should_relay_event(&event, channels));
}

#[test]
fn frame_rendered_events_pass_through_for_event_only_clients() {
    let channels = ChannelSet::from_channels(&[WsChannel::Events]);
    let event = HypercolorEvent::FrameRendered {
        frame_number: 7,
        timing: FrameTiming {
            producer_us: 0,
            composition_us: 0,
            render_us: 0,
            sample_us: 0,
            push_us: 0,
            total_us: 0,
            budget_us: 16_666,
        },
    };

    assert!(should_relay_event(&event, channels));
}

#[test]
fn ws_capabilities_include_commands() {
    let capabilities = ws_capabilities();
    assert!(capabilities.contains(&"events".to_owned()));
    assert!(capabilities.contains(&"frames".to_owned()));
    assert!(capabilities.contains(&"spectrum".to_owned()));
    assert!(capabilities.contains(&"canvas".to_owned()));
    assert!(capabilities.contains(&"screen_canvas".to_owned()));
    assert!(capabilities.contains(&"metrics".to_owned()));
    assert!(capabilities.contains(&"device_metrics".to_owned()));
    assert!(capabilities.contains(&"display_preview".to_owned()));
    assert!(capabilities.contains(&"commands".to_owned()));
    assert!(capabilities.contains(&"canvas_format_jpeg".to_owned()));
}

#[test]
fn websocket_manifest_matches_protocol_constants() {
    let manifest: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../protocol/websocket-v1.json"
    )))
    .expect("websocket protocol manifest should parse");

    let manifest_channels = manifest["channels"]
        .as_array()
        .expect("manifest channels should be an array")
        .iter()
        .map(|channel| {
            channel["name"]
                .as_str()
                .expect("manifest channel should have a name")
                .to_owned()
        })
        .collect::<Vec<_>>();
    let protocol_channels = WsChannel::SUPPORTED
        .iter()
        .map(|channel| channel.as_str().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(manifest_channels, protocol_channels);

    let manifest_capabilities = manifest["capabilities"]
        .as_array()
        .expect("manifest capabilities should be an array")
        .iter()
        .map(|capability| {
            capability
                .as_str()
                .expect("manifest capability should be a string")
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(manifest_capabilities, ws_capabilities());

    let binary_tags = manifest["binary_messages"]
        .as_array()
        .expect("manifest binary messages should be an array")
        .iter()
        .map(|message| {
            let name = message["name"]
                .as_str()
                .expect("binary message should have a name");
            let tag = message["tag"]
                .as_u64()
                .and_then(|value| u8::try_from(value).ok())
                .expect("binary message tag should fit in u8");
            (name, tag)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(binary_tags["led_frame"], 0x01);
    assert_eq!(binary_tags["spectrum"], 0x02);
    assert_eq!(binary_tags["canvas"], WS_CANVAS_HEADER);
    assert_eq!(binary_tags["screen_canvas"], WS_SCREEN_CANVAS_HEADER);
    assert_eq!(
        binary_tags["web_viewport_canvas"],
        WS_WEB_VIEWPORT_CANVAS_HEADER
    );
    assert_eq!(binary_tags["display_preview"], WS_DISPLAY_PREVIEW_HEADER);
}

#[test]
fn display_preview_patch_tri_state_distinguishes_missing_null_and_value() {
    // Three JSON shapes the client can send:
    //   - key absent → device_id stays `None` (leave as-is)
    //   - `null`     → device_id becomes `Some(None)` (explicit clear)
    //   - a string   → device_id becomes `Some(Some(...))` (set target)
    // Without the custom deserializer, `null` and "missing" collapse to
    // the same `None`, losing the explicit-clear path.

    let absent: ChannelConfigPatch =
        serde_json::from_value(serde_json::json!({ "display_preview": { "fps": 10 } }))
            .expect("fps-only patch should deserialize");
    let absent_display = absent.display_preview.expect("display_preview present");
    assert!(absent_display.device_id.is_none(), "missing key → None");

    let null_value: ChannelConfigPatch =
        serde_json::from_value(serde_json::json!({ "display_preview": { "device_id": null } }))
            .expect("null device_id should deserialize");
    let null_display = null_value.display_preview.expect("display_preview present");
    assert_eq!(
        null_display.device_id,
        Some(None),
        "null key → Some(None) (explicit clear)"
    );

    let set_value: ChannelConfigPatch = serde_json::from_value(
        serde_json::json!({ "display_preview": { "device_id": "device-abc" } }),
    )
    .expect("string device_id should deserialize");
    let set_display = set_value.display_preview.expect("display_preview present");
    assert_eq!(
        set_display.device_id,
        Some(Some("device-abc".to_owned())),
        "string value → Some(Some(value))"
    );
}

#[test]
fn display_preview_patch_applies_tri_state_to_config() {
    let mut config = ChannelConfig::default();

    // Start with a set target.
    let set_patch: ChannelConfigPatch = serde_json::from_value(serde_json::json!({
        "display_preview": { "device_id": "device-abc", "fps": 20 }
    }))
    .expect("valid set patch");
    config.apply_patch(set_patch).expect("set applied");
    assert_eq!(
        config.display_preview.device_id.as_deref(),
        Some("device-abc")
    );
    assert_eq!(config.display_preview.fps, 20);

    // Missing key leaves device_id as-is but updates fps.
    let leave_patch: ChannelConfigPatch =
        serde_json::from_value(serde_json::json!({ "display_preview": { "fps": 15 } }))
            .expect("valid fps-only patch");
    config.apply_patch(leave_patch).expect("fps-only applied");
    assert_eq!(
        config.display_preview.device_id.as_deref(),
        Some("device-abc")
    );
    assert_eq!(config.display_preview.fps, 15);

    // null explicitly clears the target.
    let clear_patch: ChannelConfigPatch = serde_json::from_value(serde_json::json!({
        "display_preview": { "device_id": null }
    }))
    .expect("valid clear patch");
    config.apply_patch(clear_patch).expect("clear applied");
    assert!(config.display_preview.device_id.is_none());
    assert_eq!(config.display_preview.fps, 15);
}

#[test]
fn display_preview_patch_rejects_empty_device_id_string() {
    let mut config = ChannelConfig::default();
    let bad: ChannelConfigPatch =
        serde_json::from_value(serde_json::json!({ "display_preview": { "device_id": "   " } }))
            .expect("empty whitespace still deserializes");
    let err = config
        .apply_patch(bad)
        .expect_err("empty-string device_id should be rejected");
    let message = format!("{err:?}");
    assert!(
        message.contains("device_id") || message.contains("non-empty"),
        "expected device_id validation error, got: {message}"
    );
}

#[test]
fn display_preview_patch_fps_must_be_in_range() {
    let mut config = ChannelConfig::default();
    let too_high: ChannelConfigPatch = serde_json::from_value(serde_json::json!({
        "display_preview": { "fps": 120 }
    }))
    .expect("high fps deserializes");
    config
        .apply_patch(too_high)
        .expect_err("fps above 30 should be rejected");

    let too_low: ChannelConfigPatch = serde_json::from_value(serde_json::json!({
        "display_preview": { "fps": 0 }
    }))
    .expect("zero fps deserializes");
    config
        .apply_patch(too_low)
        .expect_err("fps of 0 should be rejected");
}

#[tokio::test]
async fn try_enqueue_json_drops_when_queue_is_full() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(1);

    assert!(try_enqueue_json(&tx, "first".to_owned(), "test"));
    assert!(!try_enqueue_json(&tx, "second".to_owned(), "test"));

    assert_eq!(rx.recv().await.as_deref(), Some("first"));
    drop(tx);
    assert!(rx.recv().await.is_none());
}

#[test]
fn sync_preview_receiver_subscribes_only_while_requested() {
    let runtime = PreviewRuntime::new(Arc::new(HypercolorBus::new()));
    let mut receiver = None::<PreviewFrameReceiver>;

    sync_preview_receiver(&mut receiver, true, || runtime.canvas_receiver());
    assert!(receiver.is_some());
    assert_eq!(runtime.canvas_receiver_count(), 1);

    sync_preview_receiver(&mut receiver, true, || runtime.canvas_receiver());
    assert_eq!(runtime.canvas_receiver_count(), 1);

    sync_preview_receiver(&mut receiver, false, || runtime.canvas_receiver());
    assert!(receiver.is_none());
    assert_eq!(runtime.canvas_receiver_count(), 0);
}

#[test]
fn sync_preview_receiver_drops_screen_subscription_cleanly() {
    let runtime = PreviewRuntime::new(Arc::new(HypercolorBus::new()));
    let mut receiver = None::<PreviewFrameReceiver>;

    sync_preview_receiver(&mut receiver, true, || runtime.screen_canvas_receiver());
    assert!(receiver.is_some());
    assert_eq!(runtime.screen_canvas_receiver_count(), 1);

    sync_preview_receiver(&mut receiver, false, || runtime.screen_canvas_receiver());
    assert!(receiver.is_none());
    assert_eq!(runtime.screen_canvas_receiver_count(), 0);
}

#[test]
fn parse_command_method_rejects_invalid_values() {
    let error = parse_command_method("BREW").expect_err("BREW should be rejected");
    assert_eq!(error.code, "invalid_request");
}

#[test]
fn normalize_command_path_adds_api_prefix() {
    assert_eq!(
        normalize_command_path("/status").expect("path should normalize"),
        "/api/v1/status"
    );
    assert_eq!(
        normalize_command_path("/api/v1/status").expect("path should stay stable"),
        "/api/v1/status"
    );
}

#[test]
fn normalize_command_path_rejects_relative_paths() {
    let error = normalize_command_path("status").expect_err("relative path must fail");
    assert_eq!(error.code, "invalid_request");
}

#[tokio::test]
async fn command_response_from_http_unwraps_data_envelope() {
    let response = (
        axum::http::StatusCode::OK,
        axum::Json(serde_json::json!({
            "data": {"ok": true}
        })),
    )
        .into_response();
    let message = command_response_from_http("cmd_test".to_owned(), response).await;
    match message {
        ServerMessage::Response {
            id,
            status,
            data,
            error,
        } => {
            assert_eq!(id, "cmd_test");
            assert_eq!(status, 200);
            assert_eq!(data, Some(serde_json::json!({"ok": true})));
            assert!(error.is_none());
        }
        _ => panic!("expected response variant"),
    }
}

#[tokio::test]
async fn command_response_from_http_unwraps_error_envelope() {
    let response = (
        axum::http::StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({
            "error": {"code": "not_found", "message": "missing resource"}
        })),
    )
        .into_response();
    let message = command_response_from_http("cmd_missing".to_owned(), response).await;
    match message {
        ServerMessage::Response {
            id,
            status,
            data,
            error,
        } => {
            assert_eq!(id, "cmd_missing");
            assert_eq!(status, 404);
            assert!(data.is_none());
            assert_eq!(
                error,
                Some(serde_json::json!({
                    "code": "not_found",
                    "message": "missing resource"
                }))
            );
        }
        _ => panic!("expected response variant"),
    }
}

#[tokio::test]
async fn dispatch_command_routes_to_status() {
    let state = Arc::new(AppState::new());
    let message = dispatch_command(
        &state,
        RequestAuthContext::unsecured(),
        "cmd_status".to_owned(),
        "GET".to_owned(),
        "/status".to_owned(),
        None,
    )
    .await;

    match message {
        ServerMessage::Response {
            id,
            status,
            data,
            error,
        } => {
            assert_eq!(id, "cmd_status");
            assert_eq!(status, 200);
            let payload = data.expect("status command should return payload");
            assert!(payload.get("running").is_some());
            assert!(error.is_none());
        }
        _ => panic!("expected command response"),
    }
}

#[tokio::test]
async fn dispatch_command_rejects_invalid_method() {
    let state = Arc::new(AppState::new());
    let message = dispatch_command(
        &state,
        RequestAuthContext::unsecured(),
        "cmd_bad_method".to_owned(),
        "BREW".to_owned(),
        "/status".to_owned(),
        None,
    )
    .await;

    match message {
        ServerMessage::Response {
            id,
            status,
            data,
            error,
        } => {
            assert_eq!(id, "cmd_bad_method");
            assert_eq!(status, 400);
            assert!(data.is_none());
            assert_eq!(
                error.and_then(|value| value.get("code").cloned()),
                Some(serde_json::json!("invalid_request"))
            );
        }
        _ => panic!("expected command response"),
    }
}

#[tokio::test]
async fn dispatch_command_preserves_secured_ws_auth_context() {
    let state = secured_state();
    let message = dispatch_command(
        &state,
        RequestAuthContext::read_only(),
        "cmd_status".to_owned(),
        "GET".to_owned(),
        "/status".to_owned(),
        None,
    )
    .await;

    match message {
        ServerMessage::Response {
            id,
            status,
            data,
            error,
        } => {
            assert_eq!(id, "cmd_status");
            assert_eq!(status, 200);
            assert!(data.is_some());
            assert!(error.is_none());
        }
        _ => panic!("expected command response"),
    }
}

#[tokio::test]
async fn dispatch_command_requires_auth_context_when_security_is_enabled() {
    let state = secured_state();
    let message = dispatch_command(
        &state,
        RequestAuthContext::unsecured(),
        "cmd_status".to_owned(),
        "GET".to_owned(),
        "/status".to_owned(),
        None,
    )
    .await;

    match message {
        ServerMessage::Response {
            status,
            data,
            error,
            ..
        } => {
            assert_eq!(status, 401);
            assert!(data.is_none());
            assert_eq!(
                error.and_then(|value| value.get("code").cloned()),
                Some(serde_json::json!("unauthorized"))
            );
        }
        _ => panic!("expected command response"),
    }
}

#[test]
fn frame_binary_encoder_writes_header_and_payload() {
    let frame = FrameData {
        frame_number: 42,
        timestamp_ms: 1234,
        zones: vec![ZoneColors {
            zone_id: "zone_a".to_owned(),
            colors: vec![[255, 0, 0], [0, 255, 0]],
        }],
    };

    let encoded = encode_frame_binary(&frame);
    assert_eq!(encoded[0], 0x01);
    assert_eq!(
        u32::from_le_bytes([encoded[1], encoded[2], encoded[3], encoded[4]]),
        42
    );
    assert_eq!(
        u32::from_le_bytes([encoded[5], encoded[6], encoded[7], encoded[8]]),
        1234
    );
    assert_eq!(encoded[9], 1);
}

#[test]
fn filtered_frame_binary_encoder_writes_selected_zone_count_and_payload() {
    let frame = FrameData {
        frame_number: 42,
        timestamp_ms: 1234,
        zones: vec![
            ZoneColors {
                zone_id: "left".to_owned(),
                colors: vec![[255, 0, 0]],
            },
            ZoneColors {
                zone_id: "right".to_owned(),
                colors: vec![[0, 0, 255], [0, 255, 0]],
            },
        ],
    };

    let encoded =
        encode_frame_binary_selected(&frame, &FrameZoneSelection::new(&["right".to_owned()]));

    assert_eq!(encoded[0], 0x01);
    assert_eq!(encoded[9], 1);
    assert_eq!(u16::from_le_bytes([encoded[10], encoded[11]]), 5);
    assert_eq!(&encoded[12..17], b"right");
    assert_eq!(u16::from_le_bytes([encoded[17], encoded[18]]), 2);
    assert_eq!(&encoded[19..25], &[0, 0, 255, 0, 255, 0]);
}

#[test]
fn spectrum_binary_encoder_uses_requested_bin_count() {
    let spectrum = SpectrumData {
        timestamp_ms: 77,
        level: 0.5,
        bass: 0.4,
        mid: 0.3,
        treble: 0.2,
        beat: true,
        beat_confidence: 0.9,
        bpm: None,
        bins: vec![0.0; 64],
    };

    let encoded = encode_spectrum_binary(&spectrum, 16);
    assert_eq!(encoded[0], 0x02);
    assert_eq!(encoded[5], 16);
    assert_eq!(encoded[22], 1);
}

#[test]
fn filter_frame_zones_respects_named_subset() {
    let zones = vec![
        ZoneColors {
            zone_id: "left".to_owned(),
            colors: vec![[255, 0, 0]],
        },
        ZoneColors {
            zone_id: "right".to_owned(),
            colors: vec![[0, 0, 255]],
        },
    ];

    let filtered = filter_frame_zones(&zones, &["right".to_owned()]);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].zone_id, "right");

    let all = filter_frame_zones(&zones, &["all".to_owned()]);
    assert_eq!(all.len(), 2);
}

#[test]
fn cached_frame_payload_reuses_binary_bytes_for_matching_requests() {
    let _guard = WS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    reset_ws_payload_caches();

    let frame = sample_frame();
    let config = ActiveFramesConfig::new(FramesConfig {
        fps: 30,
        format: FrameFormat::Binary,
        zones: vec!["right".to_owned()],
    });

    let first = cached_frame_payload(&frame, &config);
    let second = cached_frame_payload(&frame, &config);

    match (first, second) {
        (FrameRelayMessage::Binary(first), FrameRelayMessage::Binary(second)) => {
            assert_eq!(first, second);
            assert_eq!(first.as_ptr(), second.as_ptr());
        }
        _ => panic!("expected binary relay payloads"),
    }

    assert_eq!(
        WS_FRAME_PAYLOAD_BUILD_COUNT.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        WS_FRAME_PAYLOAD_CACHE_HIT_COUNT.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[test]
fn cached_frame_payload_keys_selection_and_format_separately() {
    let _guard = WS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    reset_ws_payload_caches();

    let frame = sample_frame();
    let left_binary = cached_frame_payload(
        &frame,
        &ActiveFramesConfig::new(FramesConfig {
            fps: 30,
            format: FrameFormat::Binary,
            zones: vec!["left".to_owned()],
        }),
    );
    let right_binary = cached_frame_payload(
        &frame,
        &ActiveFramesConfig::new(FramesConfig {
            fps: 30,
            format: FrameFormat::Binary,
            zones: vec!["right".to_owned()],
        }),
    );
    let left_json = cached_frame_payload(
        &frame,
        &ActiveFramesConfig::new(FramesConfig {
            fps: 30,
            format: FrameFormat::Json,
            zones: vec!["left".to_owned()],
        }),
    );

    match (left_binary, right_binary, left_json) {
        (
            FrameRelayMessage::Binary(left_binary),
            FrameRelayMessage::Binary(right_binary),
            FrameRelayMessage::Json(left_json),
        ) => {
            assert_ne!(left_binary, right_binary);
            assert!(left_json.contains("\"zone_id\":\"left\""));
            assert!(!left_json.contains("\"zone_id\":\"right\""));
        }
        _ => panic!("unexpected relay payload variants"),
    }

    assert_eq!(
        WS_FRAME_PAYLOAD_BUILD_COUNT.load(std::sync::atomic::Ordering::Relaxed),
        3
    );
    assert_eq!(
        WS_FRAME_PAYLOAD_CACHE_HIT_COUNT.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[test]
fn cached_spectrum_payload_reuses_bytes_for_matching_requests() {
    let _guard = WS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    reset_ws_payload_caches();

    let spectrum = SpectrumData {
        timestamp_ms: 77,
        level: 0.5,
        bass: 0.4,
        mid: 0.3,
        treble: 0.2,
        beat: true,
        beat_confidence: 0.9,
        bpm: None,
        bins: vec![0.0; 64],
    };

    let first = cached_spectrum_payload(&spectrum, 16);
    let second = cached_spectrum_payload(&spectrum, 16);

    assert_eq!(first, second);
    assert_eq!(first.as_ptr(), second.as_ptr());
    assert_eq!(
        WS_SPECTRUM_PAYLOAD_BUILD_COUNT.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        WS_SPECTRUM_PAYLOAD_CACHE_HIT_COUNT.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[test]
fn cached_spectrum_payload_keys_bin_count_separately() {
    let _guard = WS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    reset_ws_payload_caches();

    let spectrum = SpectrumData {
        timestamp_ms: 77,
        level: 0.5,
        bass: 0.4,
        mid: 0.3,
        treble: 0.2,
        beat: true,
        beat_confidence: 0.9,
        bpm: None,
        bins: vec![0.0; 64],
    };

    let small = cached_spectrum_payload(&spectrum, 16);
    let large = cached_spectrum_payload(&spectrum, 32);

    assert_ne!(small, large);
    assert_eq!(
        WS_SPECTRUM_PAYLOAD_BUILD_COUNT.load(std::sync::atomic::Ordering::Relaxed),
        2
    );
    assert_eq!(
        WS_SPECTRUM_PAYLOAD_CACHE_HIT_COUNT.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[test]
fn canvas_binary_encoder_writes_spec_header_and_rgb_payload() {
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));
    canvas.set_pixel(1, 0, Rgba::new(40, 50, 60, 200));
    let frame = CanvasFrame::from_canvas(&canvas, 7, 99);

    let encoded = encode_canvas_binary_with_header(&frame, CanvasFormat::Rgb, WS_CANVAS_HEADER);
    assert_eq!(encoded[0], WS_CANVAS_HEADER);
    assert_eq!(
        u32::from_le_bytes([encoded[1], encoded[2], encoded[3], encoded[4]]),
        7
    );
    assert_eq!(
        u32::from_le_bytes([encoded[5], encoded[6], encoded[7], encoded[8]]),
        99
    );
    assert_eq!(u16::from_le_bytes([encoded[9], encoded[10]]), 2);
    assert_eq!(u16::from_le_bytes([encoded[11], encoded[12]]), 1);
    assert_eq!(encoded[13], 0);
    assert_eq!(&encoded[14..20], &[10, 20, 30, 40, 50, 60]);
}

#[test]
fn canvas_binary_encoder_writes_rgba_payload_without_repacking() {
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));
    canvas.set_pixel(1, 0, Rgba::new(40, 50, 60, 200));
    let frame = CanvasFrame::from_canvas(&canvas, 7, 99);

    let encoded = encode_canvas_binary_with_header(&frame, CanvasFormat::Rgba, WS_CANVAS_HEADER);
    assert_eq!(encoded[13], 1);
    assert_eq!(&encoded[14..22], &[10, 20, 30, 255, 40, 50, 60, 200]);
}

#[test]
fn canvas_binary_encoder_writes_jpeg_payload() {
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));
    canvas.set_pixel(1, 0, Rgba::new(40, 50, 60, 200));
    let frame = CanvasFrame::from_canvas(&canvas, 7, 99);

    let encoded = encode_canvas_jpeg_binary_stateless(&frame, WS_CANVAS_HEADER, 1.0)
        .expect("JPEG preview encoding should succeed");
    assert_eq!(encoded[0], WS_CANVAS_HEADER);
    assert_eq!(encoded[13], 2);
    assert!(encoded.len() > 14);
}

#[test]
fn canvas_binary_encoder_bilinear_scales_rgb_payload_and_updates_header() {
    let mut canvas = Canvas::new(2, 2);
    canvas.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));
    canvas.set_pixel(1, 0, Rgba::new(40, 50, 60, 255));
    canvas.set_pixel(0, 1, Rgba::new(70, 80, 90, 255));
    canvas.set_pixel(1, 1, Rgba::new(100, 110, 120, 255));
    let frame = CanvasFrame::from_canvas(&canvas, 7, 99);

    let encoded = super::cache::try_encode_cached_canvas_binary_with_header_scaled(
        &frame,
        CanvasFormat::Rgb,
        WS_CANVAS_HEADER,
        1,
        0,
    )
    .expect("scaled preview payload should encode");

    assert_eq!(u16::from_le_bytes([encoded[9], encoded[10]]), 1);
    assert_eq!(u16::from_le_bytes([encoded[11], encoded[12]]), 1);
    assert_eq!(&encoded[14..17], &[55, 65, 75]);
}

#[test]
fn canvas_preview_binary_applies_brightness_without_mutating_source() {
    let mut canvas = Canvas::new(1, 1);
    canvas.set_pixel(0, 0, Rgba::new(255, 128, 0, 200));
    let frame = CanvasFrame::from_canvas(&canvas, 7, 99);

    let encoded = encode_canvas_preview_binary(&frame, CanvasFormat::Rgba, 0.5);
    let expected = [
        linear_to_srgb_u8(srgb_u8_to_linear(255) * 0.5),
        linear_to_srgb_u8(srgb_u8_to_linear(128) * 0.5),
        linear_to_srgb_u8(srgb_u8_to_linear(0) * 0.5),
        200,
    ];

    assert_eq!(&encoded[14..18], &expected);
    assert_eq!(frame.rgba_bytes(), &[255, 128, 0, 200]);
}

#[test]
fn canvas_preview_binary_zero_brightness_preserves_alpha() {
    let mut canvas = Canvas::new(1, 1);
    canvas.set_pixel(0, 0, Rgba::new(90, 80, 70, 123));
    let frame = CanvasFrame::from_canvas(&canvas, 5, 44);

    let encoded = encode_canvas_preview_binary(&frame, CanvasFormat::Rgba, 0.0);

    assert_eq!(&encoded[14..18], &[0, 0, 0, 123]);
}

#[test]
fn canvas_preview_jpeg_binary_keys_brightness_separately() {
    let mut canvas = Canvas::new(1, 1);
    canvas.set_pixel(0, 0, Rgba::new(255, 255, 255, 255));
    let frame = CanvasFrame::from_canvas(&canvas, 7003, 9903);

    let full = encode_canvas_jpeg_binary_stateless(&frame, WS_CANVAS_HEADER, 1.0)
        .expect("full-brightness JPEG preview encoding should succeed");
    let dimmed = encode_canvas_jpeg_binary_stateless(&frame, WS_CANVAS_HEADER, 0.0)
        .expect("dimmed JPEG preview encoding should succeed");

    assert_ne!(full, dimmed);
}

#[test]
fn canvas_preview_jpeg_binary_scales_header_dimensions() {
    let mut canvas = Canvas::new(2, 2);
    canvas.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));
    canvas.set_pixel(1, 0, Rgba::new(40, 50, 60, 255));
    canvas.set_pixel(0, 1, Rgba::new(70, 80, 90, 255));
    canvas.set_pixel(1, 1, Rgba::new(100, 110, 120, 255));
    let frame = CanvasFrame::from_canvas(&canvas, 7, 99);

    let encoded = encode_canvas_jpeg_payload_scaled_stateless(&frame, WS_CANVAS_HEADER, 1.0, 1, 0)
        .expect("scaled JPEG preview encoding should succeed");

    assert_eq!(u16::from_le_bytes([encoded[9], encoded[10]]), 1);
    assert_eq!(u16::from_le_bytes([encoded[11], encoded[12]]), 1);
    assert_eq!(encoded[13], 2);
}

#[test]
fn cached_canvas_preview_binary_reuses_bytes_for_matching_requests() {
    let mut canvas = Canvas::new(1, 1);
    canvas.set_pixel(0, 0, Rgba::new(90, 80, 70, 123));
    let frame = CanvasFrame::from_canvas(&canvas, 7001, 9901);

    let first = encode_cached_canvas_preview_binary(&frame, CanvasFormat::Rgba, 0.5);
    let second = encode_cached_canvas_preview_binary(&frame, CanvasFormat::Rgba, 0.5);

    assert_eq!(first, second);
    assert_eq!(first.as_ptr(), second.as_ptr());
}

#[test]
fn cached_canvas_preview_jpeg_reuses_bytes_for_matching_requests() {
    let _guard = WS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    reset_ws_payload_caches();

    let mut canvas = Canvas::new(1, 1);
    canvas.set_pixel(0, 0, Rgba::new(90, 80, 70, 255));
    let frame = CanvasFrame::from_canvas(&canvas, 7004, 9904);

    let first = encode_cached_canvas_preview_binary(&frame, CanvasFormat::Jpeg, 1.0);
    let second = encode_cached_canvas_preview_binary(&frame, CanvasFormat::Jpeg, 1.0);

    assert_eq!(first, second);
    assert_eq!(first.as_ptr(), second.as_ptr());
    assert!(
        WS_CANVAS_PAYLOAD_BUILD_COUNT.load(std::sync::atomic::Ordering::Relaxed) >= 1,
        "expected at least one cached JPEG build"
    );
    assert!(
        WS_CANVAS_PAYLOAD_CACHE_HIT_COUNT.load(std::sync::atomic::Ordering::Relaxed) >= 1,
        "expected at least one cached JPEG hit"
    );
}

#[test]
fn cached_canvas_preview_jpeg_reuses_body_for_metadata_only_updates() {
    let _guard = WS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    reset_ws_payload_caches();

    let mut canvas = Canvas::new(1, 1);
    canvas.set_pixel(0, 0, Rgba::new(90, 80, 70, 255));
    let surface = PublishedSurface::from_owned_canvas(canvas, 7005, 9905);
    let first = CanvasFrame::from_surface(surface.with_frame_metadata(7005, 9905));
    let second = CanvasFrame::from_surface(surface.with_frame_metadata(7006, 9906));

    let first_payload = encode_cached_canvas_preview_binary(&first, CanvasFormat::Jpeg, 1.0);
    let second_payload = encode_cached_canvas_preview_binary(&second, CanvasFormat::Jpeg, 1.0);

    assert_ne!(&first_payload[..14], &second_payload[..14]);
    assert_eq!(&first_payload[14..], &second_payload[14..]);
    assert_eq!(
        WS_CANVAS_JPEG_BODY_BUILD_COUNT.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        WS_CANVAS_JPEG_BODY_CACHE_HIT_COUNT.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[test]
fn cached_canvas_preview_rgb_reuses_body_for_metadata_only_updates() {
    let _guard = WS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    reset_ws_payload_caches();

    let mut canvas = Canvas::new(1, 1);
    canvas.set_pixel(0, 0, Rgba::new(12, 34, 56, 255));
    let surface = PublishedSurface::from_owned_canvas(canvas, 7007, 9907);
    let first = CanvasFrame::from_surface(surface.with_frame_metadata(7007, 9907));
    let second = CanvasFrame::from_surface(surface.with_frame_metadata(7008, 9908));

    let first_payload = encode_cached_canvas_preview_binary(&first, CanvasFormat::Rgb, 1.0);
    let second_payload = encode_cached_canvas_preview_binary(&second, CanvasFormat::Rgb, 1.0);

    assert_ne!(&first_payload[..14], &second_payload[..14]);
    assert_eq!(&first_payload[14..], &second_payload[14..]);
    assert!(
        WS_CANVAS_RAW_BODY_BUILD_COUNT.load(std::sync::atomic::Ordering::Relaxed) >= 1,
        "expected at least one raw body build"
    );
    assert!(
        WS_CANVAS_RAW_BODY_CACHE_HIT_COUNT.load(std::sync::atomic::Ordering::Relaxed) >= 1,
        "expected at least one raw body cache hit"
    );
}

#[test]
fn cached_canvas_preview_scaled_rgba_reuses_body_for_metadata_only_updates() {
    let _guard = WS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    reset_ws_payload_caches();

    let mut canvas = Canvas::new(2, 2);
    canvas.set_pixel(0, 0, Rgba::new(12, 34, 56, 255));
    canvas.set_pixel(1, 0, Rgba::new(78, 90, 12, 255));
    canvas.set_pixel(0, 1, Rgba::new(34, 56, 78, 255));
    canvas.set_pixel(1, 1, Rgba::new(90, 12, 34, 200));
    let surface = PublishedSurface::from_owned_canvas(canvas, 7010, 9910);
    let first = CanvasFrame::from_surface(surface.with_frame_metadata(7010, 9910));
    let second = CanvasFrame::from_surface(surface.with_frame_metadata(7011, 9911));

    let first_payload = super::cache::try_encode_cached_canvas_preview_binary(
        &first,
        CanvasFormat::Rgba,
        1.0,
        1,
        0,
    )
    .expect("scaled RGBA preview should encode");
    let second_payload = super::cache::try_encode_cached_canvas_preview_binary(
        &second,
        CanvasFormat::Rgba,
        1.0,
        1,
        0,
    )
    .expect("scaled RGBA preview should encode");

    assert_ne!(&first_payload[..14], &second_payload[..14]);
    assert_eq!(&first_payload[14..], &second_payload[14..]);
    assert!(
        WS_CANVAS_RAW_BODY_BUILD_COUNT.load(std::sync::atomic::Ordering::Relaxed) >= 1,
        "expected at least one scaled RGBA raw body build"
    );
    assert!(
        WS_CANVAS_RAW_BODY_CACHE_HIT_COUNT.load(std::sync::atomic::Ordering::Relaxed) >= 1,
        "expected at least one scaled RGBA raw body cache hit"
    );
}

#[test]
fn cached_canvas_preview_rgb_reuses_body_across_headers() {
    let _guard = WS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    reset_ws_payload_caches();

    let mut canvas = Canvas::new(1, 1);
    canvas.set_pixel(0, 0, Rgba::new(98, 76, 54, 255));
    let frame = CanvasFrame::from_canvas(&canvas, 7009, 9909);

    let preview_payload = encode_cached_canvas_preview_binary(&frame, CanvasFormat::Rgb, 1.0);
    let screen_payload = super::cache::try_encode_cached_canvas_binary_with_header(
        &frame,
        CanvasFormat::Rgb,
        WS_SCREEN_CANVAS_HEADER,
    )
    .expect("screen preview payload should encode");

    assert_ne!(preview_payload[0], screen_payload[0]);
    assert_eq!(&preview_payload[14..], &screen_payload[14..]);
    assert!(
        WS_CANVAS_RAW_BODY_BUILD_COUNT.load(std::sync::atomic::Ordering::Relaxed) >= 1,
        "expected at least one raw body build"
    );
    assert!(
        WS_CANVAS_RAW_BODY_CACHE_HIT_COUNT.load(std::sync::atomic::Ordering::Relaxed) >= 1,
        "expected at least one raw body cache hit"
    );
}

#[test]
fn preview_jpeg_encoder_reuses_state_across_frames() {
    let mut canvas = Canvas::new(1, 1);
    canvas.set_pixel(0, 0, Rgba::new(90, 80, 70, 255));
    let frame = CanvasFrame::from_canvas(&canvas, 5, 44);
    let mut encoder = PreviewJpegEncoder::new().expect("JPEG preview encoder should initialize");

    let first = encoder
        .encode(&frame, WS_CANVAS_HEADER, 1.0)
        .expect("first JPEG preview encode should succeed");
    let second = encoder
        .encode(&frame, WS_CANVAS_HEADER, 0.5)
        .expect("second JPEG preview encode should succeed");

    assert_ne!(first, second);
}

#[test]
fn preview_raw_encoder_reuses_state_across_formats_and_sizes() {
    let mut canvas = Canvas::new(2, 2);
    canvas.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));
    canvas.set_pixel(1, 0, Rgba::new(40, 50, 60, 255));
    canvas.set_pixel(0, 1, Rgba::new(70, 80, 90, 255));
    canvas.set_pixel(1, 1, Rgba::new(100, 110, 120, 200));
    let frame = CanvasFrame::from_canvas(&canvas, 8, 45);
    let mut encoder = PreviewRawEncoder::new();

    let scaled_rgb = encoder.encode_scaled_body(&frame, CanvasFormat::Rgb, 1.0, 1, 0);
    let dimmed_rgba = encoder.encode_scaled_body(&frame, CanvasFormat::Rgba, 0.5, 0, 0);

    assert_eq!(scaled_rgb.len(), 3);
    assert_eq!(dimmed_rgba.len(), 16);
    assert_eq!(dimmed_rgba[3], 255);
    assert_eq!(dimmed_rgba[15], 200);
    assert_ne!(dimmed_rgba[..3], frame.rgba_bytes()[..3]);
}

#[test]
fn cached_canvas_preview_binary_keys_brightness_separately() {
    let mut canvas = Canvas::new(1, 1);
    canvas.set_pixel(0, 0, Rgba::new(255, 128, 0, 200));
    let frame = CanvasFrame::from_canvas(&canvas, 7002, 9902);

    let full = encode_cached_canvas_preview_binary(&frame, CanvasFormat::Rgba, 1.0);
    let dimmed = encode_cached_canvas_preview_binary(&frame, CanvasFormat::Rgba, 0.5);

    assert_ne!(full, dimmed);
}

#[test]
fn cached_canvas_preview_binary_keys_dimensions_separately() {
    let _guard = WS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    reset_ws_payload_caches();

    let mut canvas = Canvas::new(2, 2);
    canvas.set_pixel(0, 0, Rgba::new(255, 128, 0, 200));
    canvas.set_pixel(1, 0, Rgba::new(128, 255, 0, 200));
    canvas.set_pixel(0, 1, Rgba::new(0, 128, 255, 200));
    canvas.set_pixel(1, 1, Rgba::new(64, 64, 64, 200));
    let frame = CanvasFrame::from_canvas(&canvas, 7002, 9902);

    let full = super::cache::try_encode_cached_canvas_preview_binary(
        &frame,
        CanvasFormat::Rgba,
        1.0,
        0,
        0,
    )
    .expect("full-size cached preview should encode");
    let scaled = super::cache::try_encode_cached_canvas_preview_binary(
        &frame,
        CanvasFormat::Rgba,
        1.0,
        1,
        0,
    )
    .expect("scaled cached preview should encode");

    assert_ne!(full, scaled);
}

#[test]
fn screen_canvas_binary_encoder_uses_distinct_header() {
    let mut canvas = Canvas::new(1, 1);
    canvas.set_pixel(0, 0, Rgba::new(90, 80, 70, 255));
    let frame = CanvasFrame::from_canvas(&canvas, 5, 44);

    let encoded =
        encode_canvas_binary_with_header(&frame, CanvasFormat::Rgb, WS_SCREEN_CANVAS_HEADER);
    assert_eq!(encoded[0], WS_SCREEN_CANVAS_HEADER);
    assert_eq!(&encoded[14..17], &[90, 80, 70]);
}
