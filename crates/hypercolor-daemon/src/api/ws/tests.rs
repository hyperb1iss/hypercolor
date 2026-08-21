use std::collections::BTreeSet;
use std::num::NonZeroU32;
use std::sync::{Arc, LazyLock, Mutex as StdMutex, PoisonError};
use std::time::{Duration, SystemTime};

use axum::body::Bytes;
use axum::extract::ws::{Message, Utf8Bytes};
use axum::response::IntoResponse;
use tokio::sync::{RwLock, watch};
use tokio_util::sync::CancellationToken;

use hypercolor_core::bus::{CanvasFrame, HypercolorBus, ZonePreviewFrame};
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::input::screen::{
    PixelExtent, ScreenExtentRequest, ScreenPublicationExecutorRequest, ScreenPublicationKind,
};
use hypercolor_core::input::{
    BrowserConnectionIncarnation, BrowserInputChildKey, BrowserInputHandle, BrowserInputSource,
    BrowserPreviewId, InputData, InputGraphHandle, InputManager, InputSource, SourceIssue,
    SourceKind, SourceSessionSlot, SourceStatusHandle, SourceStatusReporter,
};
use hypercolor_core::scene::SceneManager;
use hypercolor_leptos_ext::ws::registry::{
    CanvasFormat, FramesConfig, InteractivePreviewConfig, InteractivePreviewTarget, TopicId,
    TopicSet,
};
use hypercolor_leptos_ext::ws::topic::{TopicSelector, TopicSubscription};
use hypercolor_leptos_ext::ws::{
    DisplayPreviewFrame as WireDisplayPreviewFrame, HYPERCOLOR_WS_PROTOCOL, HYPERCOLOR_WS_VERSION,
    InteractivePreviewFrame as WireInteractivePreviewFrame, PREVIEW_MIN_MESSAGE_BYTES,
    PreviewFrame as WirePreviewFrame, PreviewFrameChannel,
    PreviewPixelFormat as WirePreviewPixelFormat, PreviewStreamId, PreviewTransportLimits,
    TimedInputEventPayload,
};
use hypercolor_types::canvas::{
    Canvas, PublishedSurface, Rgba, linear_to_srgb_u8, srgb_u8_to_linear,
};
use hypercolor_types::config::InteractionRoutePolicy;
use hypercolor_types::controls::{ControlSurfaceEvent, ControlValue, ControlValueMap};
use hypercolor_types::device::{ConnectionType, DeviceId, DeviceOrigin};
use hypercolor_types::event::{
    FrameData, FrameTiming, HypercolorEvent, MacosDaemonHandoverPhaseEvent,
    MacosDaemonOwnerConflictEvent, MacosDaemonOwnerEvent, MacosDaemonOwnerRecoveryRequiredEvent,
    SpectrumData, TimedInputEvent, ZoneColors,
};
use hypercolor_types::scene::{SceneId, ZoneId, ZoneRole};
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::SamplingMode;

use super::cache::{
    WS_CANVAS_BINARY_CACHE, WS_CANVAS_HEADER, WS_CANVAS_JPEG_BODY_BUILD_COUNT,
    WS_CANVAS_JPEG_BODY_CACHE_HIT_COUNT, WS_CANVAS_PAYLOAD_BUILD_COUNT,
    WS_CANVAS_PAYLOAD_CACHE_HIT_COUNT, WS_CANVAS_RAW_BODY_BUILD_COUNT,
    WS_CANVAS_RAW_BODY_CACHE_HIT_COUNT, WS_DISPLAY_PREVIEW_HEADER,
    WS_DISPLAY_PREVIEW_PAYLOAD_CACHE_MAX_BYTES, WS_FRAME_PAYLOAD_BUILD_COUNT,
    WS_FRAME_PAYLOAD_CACHE, WS_FRAME_PAYLOAD_CACHE_HIT_COUNT, WS_SCREEN_CANVAS_HEADER,
    WS_SPECTRUM_PAYLOAD_BUILD_COUNT, WS_SPECTRUM_PAYLOAD_CACHE,
    WS_SPECTRUM_PAYLOAD_CACHE_HIT_COUNT, WS_WEB_VIEWPORT_CANVAS_HEADER, WS_ZONE_PREVIEW_HEADER,
    WS_ZONE_PREVIEW_HEADER_LEN, cached_display_preview_payload, cached_frame_payload,
    cached_spectrum_payload, encode_cached_canvas_preview_binary, encode_canvas_binary_with_header,
    encode_canvas_preview_binary, encode_frame_binary, encode_frame_binary_selected,
    encode_spectrum_binary, led_frame_codec_manifest, put_bytes_lru,
    reset_canvas_jpeg_body_cache_for_tests, reset_canvas_raw_body_cache_for_tests,
    reset_display_preview_payload_cache_for_tests, reset_preview_jpeg_encoders_for_tests,
    try_encode_cached_zone_preview_binary_scaled,
};
use super::command::{
    command_response_from_http, dispatch_command, normalize_command_path, parse_command_method,
};
use super::preview_encode::{
    PreviewJpegEncoder, PreviewRawEncoder, encode_canvas_jpeg_binary_stateless,
    encode_canvas_jpeg_payload_scaled_stateless,
};
use super::protocol::{
    ActiveFramesConfig, BrowserInputEdgeWire, ClientMessage, FrameZoneSelection,
    InputButtonStateWire, MAX_INPUT_INJECT_EVENTS, MAX_INPUT_NAME_BYTES, MAX_INPUT_SCROLL_Q16_16,
    MAX_INPUT_WHEEL_DELTA, ServerMessage, SubscriptionState, TopicSelection, WsProtocolError,
    deserialize_finite_coordinate, event_message_parts, json_payload_manifest, parse_selectors,
    parse_subscriptions, should_relay_event, validate_interactive_preview_id,
    validate_interactive_preview_shape, ws_capabilities,
};

fn assert_manifested_json_payload(schema: &str, data: &serde_json::Value) {
    let manifest = json_payload_manifest();
    let entry = &manifest[schema];
    let required = entry["required_fields"]
        .as_array()
        .expect("required fields are an array")
        .iter()
        .map(|field| field.as_str().expect("field name is a string"))
        .collect::<BTreeSet<_>>();
    let allowed = required
        .iter()
        .copied()
        .chain(
            entry["optional_fields"]
                .as_object()
                .expect("optional fields are an object")
                .keys()
                .map(String::as_str),
        )
        .collect::<BTreeSet<_>>();
    let actual = data
        .as_object()
        .expect("payload data is an object")
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert!(required.is_subset(&actual), "{schema:?} required fields");
    assert!(actual.is_subset(&allowed), "{schema:?} unknown fields");
}

#[test]
fn websocket_native_errors_project_canonical_domain_codes() {
    let malformed = WsProtocolError::invalid_request("bad request");
    assert_eq!(
        malformed.code,
        crate::domain::DomainError::malformed("bad request").code()
    );

    let forbidden = WsProtocolError::forbidden("denied", serde_json::json!({}));
    assert_eq!(
        forbidden.code,
        crate::domain::DomainError::forbidden("denied").code()
    );

    let validation = WsProtocolError::invalid_config("fps", "must be positive");
    assert_eq!(
        validation.code,
        crate::domain::DomainError::validation("bad config").code()
    );
}
use super::relays::{
    PreviewOutboundItem, PreviewOutboundLimits, PreviewOutboundReceiver, PreviewOutboundSender,
    PreviewPublication, PreviewPublishOutcome, PreviewRelayPublish, PreviewSendCursor,
    build_device_metrics_message, build_metrics_message, preview_outbound_channel,
    preview_outbound_channel_with_limits, publish_preview_until_cancelled,
    publish_preview_while_subscribed, publish_subscriptions, relay_device_metrics,
    relay_display_preview, relay_events, relay_frames, relay_metrics, relay_screen_zones,
    relay_sensors, relay_spectrum, relay_zone_preview, sync_preview_receiver, try_enqueue_json,
};
use super::session::{
    BrowserPreviewSession, WsInputDemandLeases, authorize_subscription_topics, build_hello_state,
    spawn_test_local_socket, validated_zone_layout_preview,
};

#[tokio::test]
async fn hello_reports_a_destructive_stop_as_not_running_and_paused() {
    let state = AppState::new();
    state.render_loop.write().await.start();
    crate::session::set_output_stopped(&state.power_state, &state.event_bus);

    let hello = build_hello_state(&state).await;
    assert!(!hello.running);
    assert!(hello.paused);
}
use crate::api::security::{RequestAuthContext, SecurityState};
use crate::app_state::AppState;
use crate::device_metrics::{DeviceMetrics, DeviceMetricsSnapshot};
use crate::display_frames::{DisplayFrameRuntime, DisplayFrameSnapshot};
use crate::interaction_routing::InteractionRoutingControl;
use crate::interactive_preview::{
    InteractivePreviewAcceleration, InteractivePreviewContext, InteractivePreviewExecutor,
};
use crate::performance::{
    CompositorBackendKind, FrameTimeline, FullFrameCopyMetrics, LatestFrameMetrics,
    OutputFrameSourceKind,
};
use crate::preview_runtime::{
    PreviewFrameReceiver, PreviewPixelFormat, PreviewRuntime, PreviewStreamDemand,
};
use crate::render_thread::{InputPublicationConsumer, InputPublicationDemandHandle};
use crate::startup::input_status_events::InputStatusEventPublisher;

/// Selectors for the authorization tests, in the shape the wire parse
/// produces.
fn selections(topics: &[TopicId]) -> Vec<TopicSelection> {
    topics
        .iter()
        .map(|topic| TopicSelection {
            topic: *topic,
            key: None,
        })
        .collect()
}

/// Membership set for the routing tests, built the way the registry
/// builds one.
fn topic_set(topics: &[TopicId]) -> TopicSet {
    let mut set = TopicSet::EMPTY;
    for topic in topics {
        set.insert(*topic);
    }
    set
}

#[test]
fn websocket_input_demand_leases_follow_subscription_lifetime() {
    let demands = InputPublicationDemandHandle::new();
    let base_screen_extent = PixelExtent::new(1_920, 1_080).expect("fixture extent");
    let mut leases = WsInputDemandLeases::new(demands.clone(), 60, base_screen_extent, 8, 6);
    let mut subscriptions = SubscriptionState::default();

    leases
        .synchronize(&subscriptions)
        .expect("empty subscription demand synchronizes");
    assert_eq!(
        demands.registration_count(InputPublicationConsumer::PassiveStream),
        0
    );

    subscriptions = subscriptions
        .subscribed_unkeyed(
            &["screen_canvas"],
            serde_json::json!({"screen_canvas": {"width": 5_120, "height": 0}}),
        )
        .expect("screen canvas subscribe applies");
    leases
        .synchronize(&subscriptions)
        .expect("partial-axis screen demand synchronizes");
    assert_eq!(
        leases.screen_requested_extent(),
        PixelExtent::new(5_120, 2_880).ok()
    );
    let canvas_only = demands.screen_branches();
    assert_eq!(canvas_only.len(), 1);
    assert_eq!(
        canvas_only[0].request().executor(),
        &ScreenPublicationExecutorRequest::Cpu
    );
    let ScreenExtentRequest::Bounded(canvas_bounds) = canvas_only[0].request().extent() else {
        panic!("width-only canvas request remains bounded");
    };
    assert_eq!(canvas_bounds.max_width().map(NonZeroU32::get), Some(5_120));
    assert_eq!(canvas_bounds.max_height(), None);
    // A refused cadence never reaches the config store, and the lease
    // it would have moved stays exactly where it was.
    let canvas_revision = demands.revision();
    let refused = subscriptions
        .subscribed_unkeyed(
            &["screen_canvas"],
            serde_json::json!({"screen_canvas": {"fps": 0}}),
        )
        .expect_err("a zero cadence is refused before it can be stored");
    assert_eq!(refused.code, "validation_error");
    leases
        .synchronize(&subscriptions)
        .expect("the live subscription still synchronizes");
    assert_eq!(demands.revision(), canvas_revision);
    assert_eq!(demands.screen_branches(), canvas_only);

    subscriptions = subscriptions
        .subscribed_unkeyed(
            &["spectrum", "screen_canvas", "screen_zones", "input_events"],
            serde_json::json!({
                "spectrum": {"fps": 24},
                "screen_canvas": {"height": 720}
            }),
        )
        .expect("mixed subscribe applies");
    leases
        .synchronize(&subscriptions)
        .expect("wide screen demand synchronizes");
    assert_eq!(
        demands.registration_count(InputPublicationConsumer::PassiveStream),
        3
    );
    assert_eq!(demands.requested_hz(SourceKind::Audio), 24);
    assert_eq!(demands.requested_hz(SourceKind::Screen), 15);
    assert_eq!(
        leases.screen_requested_extent(),
        PixelExtent::new(5_120, 720).ok()
    );
    let mixed_branches = demands.screen_branches();
    assert_eq!(mixed_branches.len(), 2);
    assert!(
        mixed_branches.iter().all(|branch| {
            branch.request().executor() == &ScreenPublicationExecutorRequest::Cpu
        })
    );
    let ScreenExtentRequest::Bounded(canvas_bounds) = mixed_branches[0].request().extent() else {
        panic!("two-axis canvas request remains bounded");
    };
    assert_eq!(canvas_bounds.max_width().map(NonZeroU32::get), Some(5_120));
    assert_eq!(canvas_bounds.max_height().map(NonZeroU32::get), Some(720));
    assert!(matches!(
        mixed_branches[1].request().kind(),
        ScreenPublicationKind::Zones { columns, rows }
            if columns.get() == 8 && rows.get() == 6
    ));
    assert_eq!(demands.requested_hz(SourceKind::Interaction), 60);

    subscriptions = subscriptions
        .subscribed_unkeyed(&["spectrum"], serde_json::json!({"spectrum": {"fps": 48}}))
        .expect("spectrum cadence patch applies")
        .unsubscribed_unkeyed(&["screen_canvas"]);
    leases
        .synchronize(&subscriptions)
        .expect("screen zone demand synchronizes");
    assert_eq!(demands.requested_hz(SourceKind::Audio), 48);
    assert_eq!(demands.requested_hz(SourceKind::Screen), 15);
    assert_eq!(leases.screen_requested_extent(), Some(base_screen_extent));
    let zone_only = demands.screen_branches();
    assert_eq!(zone_only.len(), 1);
    assert!(matches!(
        zone_only[0].request().kind(),
        ScreenPublicationKind::Zones { .. }
    ));

    subscriptions = subscriptions.unsubscribed_unkeyed(&["screen_zones", "input_events"]);
    leases
        .synchronize(&subscriptions)
        .expect("removed screen demand synchronizes");
    assert_eq!(
        demands.registration_count(InputPublicationConsumer::PassiveStream),
        1
    );
    assert_eq!(demands.requested_hz(SourceKind::Screen), 0);
    assert_eq!(demands.requested_hz(SourceKind::Interaction), 0);
    assert_eq!(leases.screen_requested_extent(), None);

    drop(leases);
    assert_eq!(
        demands.registration_count(InputPublicationConsumer::PassiveStream),
        0
    );
    assert_eq!(demands.requested_hz(SourceKind::Audio), 0);
}

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
        renderer_loads_total: 0,
        renderer_load_failures_total: 0,
        renderer_load_wait_total_us: 0,
        renderer_load_wait_max_us: 0,
        detached_destroys_total: 0,
        detached_destroy_failures_total: 0,
        destroy_wait_total_us: 0,
        destroy_wait_max_us: 0,
        render_requests_total: 0,
        render_queue_wait_total_us: 0,
        render_queue_wait_max_us: 0,
        render_scene_requests_total: 0,
        render_scene_queue_wait_total_us: 0,
        render_scene_queue_wait_max_us: 0,
        render_display_requests_total: 0,
        render_display_queue_wait_total_us: 0,
        render_display_queue_wait_max_us: 0,
        render_queue_depth: 0,
        render_queue_depth_max: 0,
        render_superseded_total: 0,
        render_pending_age_max_us: 0,
        render_cpu_frames_total: 0,
        render_cached_frames_total: 0,
        render_gpu_frames_total: 0,
        render_gpu_import_failures_total: 0,
        render_gpu_import_fallbacks_total: 0,
        render_gpu_import_fallback_reason: None,
        render_gpu_import_windows_sync_mode: None,
        render_gpu_import_stale_frame_total: 0,
        render_gpu_import_adapter_mismatch_total: 0,
        render_gpu_import_slot_count: 0,
        render_gpu_import_pending_slots: 0,
        render_gpu_import_pending_slots_max: 0,
        render_gpu_import_completed_slots: 0,
        render_gpu_import_available_slots: 0,
        render_gpu_import_available_slots_min: 0,
        render_gpu_import_oldest_pending_age_max_us: 0,
        render_gpu_import_blit_total_us: 0,
        render_gpu_import_blit_max_us: 0,
        render_gpu_import_sync_total_us: 0,
        render_gpu_import_sync_max_us: 0,
        render_gpu_import_total_us: 0,
        render_gpu_import_max_us: 0,
        render_evaluate_scripts_total_us: 0,
        render_evaluate_scripts_max_us: 0,
        render_event_loop_total_us: 0,
        render_event_loop_max_us: 0,
        render_paint_total_us: 0,
        render_paint_max_us: 0,
        render_readback_total_us: 0,
        render_readback_max_us: 0,
        render_frame_total_us: 0,
        render_frame_max_us: 0,
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
    renderer_loads_total: u64,
    renderer_load_failures_total: u64,
    renderer_load_wait_total_us: u64,
    renderer_load_wait_max_us: u64,
    detached_destroys_total: u64,
    detached_destroy_failures_total: u64,
    destroy_wait_total_us: u64,
    destroy_wait_max_us: u64,
    render_requests_total: u64,
    render_queue_wait_total_us: u64,
    render_queue_wait_max_us: u64,
    render_scene_requests_total: u64,
    render_scene_queue_wait_total_us: u64,
    render_scene_queue_wait_max_us: u64,
    render_display_requests_total: u64,
    render_display_queue_wait_total_us: u64,
    render_display_queue_wait_max_us: u64,
    render_queue_depth: u64,
    render_queue_depth_max: u64,
    render_superseded_total: u64,
    render_pending_age_max_us: u64,
    render_cpu_frames_total: u64,
    render_cached_frames_total: u64,
    render_gpu_frames_total: u64,
    render_gpu_import_failures_total: u64,
    render_gpu_import_fallbacks_total: u64,
    render_gpu_import_fallback_reason: Option<&'static str>,
    render_gpu_import_windows_sync_mode: Option<&'static str>,
    render_gpu_import_stale_frame_total: u64,
    render_gpu_import_adapter_mismatch_total: u64,
    render_gpu_import_slot_count: u64,
    render_gpu_import_pending_slots: u64,
    render_gpu_import_pending_slots_max: u64,
    render_gpu_import_completed_slots: u64,
    render_gpu_import_available_slots: u64,
    render_gpu_import_available_slots_min: u64,
    render_gpu_import_oldest_pending_age_max_us: u64,
    render_gpu_import_blit_total_us: u64,
    render_gpu_import_blit_max_us: u64,
    render_gpu_import_sync_total_us: u64,
    render_gpu_import_sync_max_us: u64,
    render_gpu_import_total_us: u64,
    render_gpu_import_max_us: u64,
    render_evaluate_scripts_total_us: u64,
    render_evaluate_scripts_max_us: u64,
    render_event_loop_total_us: u64,
    render_event_loop_max_us: u64,
    render_paint_total_us: u64,
    render_paint_max_us: u64,
    render_readback_total_us: u64,
    render_readback_max_us: u64,
    render_frame_total_us: u64,
    render_frame_max_us: u64,
}

fn secured_state() -> Arc<AppState> {
    let mut state = AppState::new();
    state.security_state =
        SecurityState::with_keys(Some("hc_ak_control_test"), Some("hc_ak_r_read_test"));
    Arc::new(state)
}

struct StatusEventTestSource {
    status: SourceStatusReporter,
    session_slot: SourceSessionSlot,
    running: bool,
}

impl StatusEventTestSource {
    fn new(session_slot: SourceSessionSlot) -> Self {
        Self::with_id("status-event-test", session_slot)
    }

    fn with_id(source_id: &'static str, session_slot: SourceSessionSlot) -> Self {
        Self {
            status: SourceStatusReporter::new(
                source_id,
                SourceKind::Interaction,
                "test_backend",
                true,
                true,
                true,
            ),
            session_slot,
            running: false,
        }
    }
}

impl InputSource for StatusEventTestSource {
    fn name(&self) -> &'static str {
        "Status Event Test"
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if let Some(session) = self.status.begin_session()? {
            self.session_slot.store(session);
        }
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.session_slot.clear();
        self.status.stop();
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_interaction_source(&self) -> bool {
        true
    }
}

fn status_event_state() -> (Arc<AppState>, SourceSessionSlot) {
    let session_slot = SourceSessionSlot::new();
    let mut input_manager = InputManager::new();
    input_manager.add_source(Box::new(StatusEventTestSource::new(session_slot.clone())));
    input_manager
        .start_all()
        .expect("status event test source should start");
    let input_status = input_manager.source_status_registry();
    let screen_capacity_status = input_manager.screen_capacity_status_handle();

    let mut state = AppState::new();
    state.input_manager = Arc::new(tokio::sync::Mutex::new(input_manager));
    state.screen_capacity_status = screen_capacity_status;
    state.input_status = input_status;
    (Arc::new(state), session_slot)
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
    reset_display_preview_payload_cache_for_tests();
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
    let tempdir = tempfile::tempdir().expect("metrics test data dir should be created");
    let state = Arc::new(AppState::new_with_data_dir(tempdir.path().join("data")));
    state.render_loop.write().await.start();
    let mut preview_rx = state.preview_runtime.canvas_receiver();
    let mut scene_preview_rx = state.preview_runtime.scene_canvas_receiver();
    let mut screen_preview_rx = state.preview_runtime.screen_canvas_receiver();
    preview_rx.update_demand(PreviewStreamDemand {
        fps: 20,
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
        performance.record_effect_error();
        performance.record_effect_fallback_applied();
        performance.record_frame(&LatestFrameMetrics {
            timestamp_ms: 1234,
            input_sampled: true,
            input_us: 200,
            deferred_sample_us: 60,
            producer_us: 900,
            producer_render_us: 640,
            producer_scene_compose_us: 110,
            composition_us: 300,
            render_us: 1_200,
            preview_advance_us: 45,
            sample_us: 150,
            sample_dispatch_us: 95,
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
            preview_surface: true,
            scene_canvas_forced_surface: true,
            cpu_readback_skipped: true,
            gpu_readback_failed: true,
            compositor_backend: CompositorBackendKind::Gpu,
            output_frame_source: OutputFrameSourceKind::PublishedFrame,
            output_reuses_published_frame: true,
            output_brightness_bits: 1.0_f32.to_bits(),
            output_brightness_generation: 17,
            output_routing_signature: 23,
            output_zone_shape_signature: 29,
            output_unassigned_behavior_generation: 31,
            devices_written: 7,
            total_leds: 512,
            logical_layer_count: 2,
            render_group_count: 1,
            scene_active: true,
            scene_transition_active: true,
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
            scene_pool_free_slots: 4,
            scene_pool_published_slots: 5,
            scene_pool_dequeued_slots: 1,
            direct_pool_free_slots: 2,
            direct_pool_published_slots: 3,
            direct_pool_dequeued_slots: 1,
            preview_pool_slot_count: 2,
            preview_pool_free_slots: 1,
            preview_pool_published_slots: 1,
            preview_pool_dequeued_slots: 0,
            compositor_pool_slot_count: 4,
            compositor_pool_free_slots: 2,
            compositor_pool_published_slots: 1,
            compositor_pool_dequeued_slots: 1,
            canvas_receiver_count: 2,
            producer_full_frame_copy: FullFrameCopyMetrics {
                count: 1,
                bytes: 512,
                reason: Some("producer_test"),
            },
            publication_full_frame_copy: FullFrameCopyMetrics {
                count: 1,
                bytes: 1_536,
                reason: Some("publication_test"),
            },
            full_frame_copy_count: 2,
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
    assert_eq!(json["timeline"]["output_frame_source"], "published_frame");
    assert_eq!(json["timeline"]["output_reuses_published_frame"], true);
    assert_eq!(json["timeline"]["output_brightness_generation"], 17);
    assert_eq!(json["timeline"]["output_routing_signature"], 23);
    assert_eq!(json["timeline"]["output_zone_shape_signature"], 29);
    assert_eq!(
        json["timeline"]["output_unassigned_behavior_generation"],
        31
    );
    assert_eq!(json["timeline"]["devices_written"], 7);
    assert_eq!(json["timeline"]["total_leds"], 512);
    assert_eq!(json["timeline"]["gpu_zone_sampling"], true);
    assert_eq!(json["timeline"]["gpu_sample_deferred"], true);
    assert_eq!(json["timeline"]["gpu_sample_stale"], true);
    assert_eq!(json["timeline"]["gpu_sample_retry_hit"], true);
    assert_eq!(json["timeline"]["gpu_sample_queue_saturated"], true);
    assert_eq!(json["timeline"]["gpu_sample_wait_blocked"], true);
    assert_eq!(json["timeline"]["gpu_sample_cpu_fallback"], true);
    assert_eq!(json["timeline"]["preview_surface"], true);
    assert_eq!(json["timeline"]["scene_canvas_forced_surface"], true);
    assert_eq!(json["timeline"]["cpu_readback_skipped"], true);
    assert_eq!(json["timeline"]["gpu_readback_failed"], true);
    assert_eq!(json["timeline"]["budget_ms"], 16.67);
    assert_eq!(json["timeline"]["wake_late_ms"], 0.22);
    assert_eq!(json["input_latency"]["sample_count"], 1);
    assert_eq!(json["input_latency"]["p95_ms"], 0.2);
    assert_eq!(json["input_latency"]["p99_ms"], 0.2);
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
    assert_eq!(json["pacing"]["preview_surface"], 1);
    assert_eq!(json["pacing"]["scene_canvas_forced_surface"], 1);
    assert_eq!(json["pacing"]["gpu_readback_failed_frames"], 1);
    assert_eq!(json["pacing"]["output_error_frames"], 1);
    assert_eq!(json["pacing"]["full_frame_copy_frames"], 1);
    assert_eq!(json["pacing"]["output_current_frame"], 0);
    assert_eq!(json["pacing"]["output_published_frame"], 1);
    assert_eq!(json["pacing"]["output_routed_reuse"], 0);
    assert_eq!(json["pacing"]["output_reused_published_frame"], 1);
    assert_eq!(json["copies"]["session_full_frame_count"], 2);
    assert_eq!(json["copies"]["session_full_frame_frames"], 1);
    assert_eq!(json["copies"]["session_full_frame_bytes"], 2_048);
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
    assert_eq!(json["render_surfaces"]["scene_pool_free_slots"], 4);
    assert_eq!(json["render_surfaces"]["scene_pool_published_slots"], 5);
    assert_eq!(json["render_surfaces"]["scene_pool_dequeued_slots"], 1);
    assert_eq!(json["render_surfaces"]["direct_pool_free_slots"], 2);
    assert_eq!(json["render_surfaces"]["direct_pool_published_slots"], 3);
    assert_eq!(json["render_surfaces"]["direct_pool_dequeued_slots"], 1);
    assert_eq!(json["render_surfaces"]["preview_pool_slot_count"], 2);
    assert_eq!(json["render_surfaces"]["preview_pool_free_slots"], 1);
    assert_eq!(json["render_surfaces"]["preview_pool_published_slots"], 1);
    assert_eq!(json["render_surfaces"]["preview_pool_dequeued_slots"], 0);
    assert_eq!(json["render_surfaces"]["compositor_pool_slot_count"], 4);
    assert_eq!(json["render_surfaces"]["compositor_pool_free_slots"], 2);
    assert_eq!(
        json["render_surfaces"]["compositor_pool_published_slots"],
        1
    );
    assert_eq!(json["render_surfaces"]["compositor_pool_dequeued_slots"], 1);
    assert_eq!(json["copies"]["full_frame_count"], 2);
    assert_eq!(json["copies"]["full_frame_kb"], 2.0);
    assert_eq!(json["copies"]["producer_full_frame_count"], 1);
    assert_eq!(json["copies"]["producer_full_frame_kb"], 0.5);
    assert_eq!(json["copies"]["producer_reason"], "producer_test");
    assert_eq!(json["copies"]["publication_full_frame_count"], 1);
    assert_eq!(json["copies"]["publication_full_frame_kb"], 1.5);
    assert_eq!(json["copies"]["publication_reason"], "publication_test");
    assert_eq!(json["effect_health"]["errors_total"], 2);
    assert_eq!(json["effect_health"]["fallbacks_applied_total"], 1);
    assert_eq!(
        json["effect_health"]["producer_gpu_readback_failures_total"],
        1
    );
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
        json["effect_health"]["servo_renderer_loads_total"],
        servo_health.renderer_loads_total
    );
    assert_eq!(
        json["effect_health"]["servo_renderer_load_failures_total"],
        servo_health.renderer_load_failures_total
    );
    assert_eq!(
        json["effect_health"]["servo_renderer_load_wait_total_ms"],
        std::time::Duration::from_micros(servo_health.renderer_load_wait_total_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_renderer_load_wait_max_ms"],
        std::time::Duration::from_micros(servo_health.renderer_load_wait_max_us).as_secs_f64()
            * 1000.0
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
        json["effect_health"]["servo_destroy_wait_total_ms"],
        std::time::Duration::from_micros(servo_health.destroy_wait_total_us).as_secs_f64() * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_destroy_wait_max_ms"],
        std::time::Duration::from_micros(servo_health.destroy_wait_max_us).as_secs_f64() * 1000.0
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
    assert_eq!(
        json["effect_health"]["servo_render_scene_requests_total"],
        servo_health.render_scene_requests_total
    );
    assert_eq!(
        json["effect_health"]["servo_render_scene_queue_wait_total_ms"],
        std::time::Duration::from_micros(servo_health.render_scene_queue_wait_total_us)
            .as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_scene_queue_wait_max_ms"],
        std::time::Duration::from_micros(servo_health.render_scene_queue_wait_max_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_display_requests_total"],
        servo_health.render_display_requests_total
    );
    assert_eq!(
        json["effect_health"]["servo_render_display_queue_wait_total_ms"],
        std::time::Duration::from_micros(servo_health.render_display_queue_wait_total_us)
            .as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_display_queue_wait_max_ms"],
        std::time::Duration::from_micros(servo_health.render_display_queue_wait_max_us)
            .as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_queue_depth"],
        servo_health.render_queue_depth
    );
    assert_eq!(
        json["effect_health"]["servo_render_queue_depth_max"],
        servo_health.render_queue_depth_max
    );
    assert_eq!(
        json["effect_health"]["servo_render_superseded_total"],
        servo_health.render_superseded_total
    );
    assert_eq!(
        json["effect_health"]["servo_render_pending_age_max_ms"],
        std::time::Duration::from_micros(servo_health.render_pending_age_max_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_cpu_frames_total"],
        servo_health.render_cpu_frames_total
    );
    assert_eq!(
        json["effect_health"]["servo_render_cached_frames_total"],
        servo_health.render_cached_frames_total
    );
    assert_eq!(
        json["effect_health"]["servo_render_gpu_frames_total"],
        servo_health.render_gpu_frames_total
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_failures_total"],
        servo_health.render_gpu_import_failures_total
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_fallbacks_total"],
        servo_health.render_gpu_import_fallbacks_total
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_fallback_reason"].as_str(),
        servo_health.render_gpu_import_fallback_reason
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_windows_sync_mode"].as_str(),
        servo_health.render_gpu_import_windows_sync_mode
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_stale_frame_total"],
        servo_health.render_gpu_import_stale_frame_total
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_adapter_mismatch_total"],
        servo_health.render_gpu_import_adapter_mismatch_total
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_slot_count"],
        servo_health.render_gpu_import_slot_count
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_pending_slots"],
        servo_health.render_gpu_import_pending_slots
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_pending_slots_max"],
        servo_health.render_gpu_import_pending_slots_max
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_completed_slots"],
        servo_health.render_gpu_import_completed_slots
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_available_slots"],
        servo_health.render_gpu_import_available_slots
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_available_slots_min"],
        servo_health.render_gpu_import_available_slots_min
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_oldest_pending_age_max_ms"],
        std::time::Duration::from_micros(servo_health.render_gpu_import_oldest_pending_age_max_us)
            .as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_blit_total_ms"],
        std::time::Duration::from_micros(servo_health.render_gpu_import_blit_total_us)
            .as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_blit_max_ms"],
        std::time::Duration::from_micros(servo_health.render_gpu_import_blit_max_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_sync_total_ms"],
        std::time::Duration::from_micros(servo_health.render_gpu_import_sync_total_us)
            .as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_sync_max_ms"],
        std::time::Duration::from_micros(servo_health.render_gpu_import_sync_max_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_total_ms"],
        std::time::Duration::from_micros(servo_health.render_gpu_import_total_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_gpu_import_max_ms"],
        std::time::Duration::from_micros(servo_health.render_gpu_import_max_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_evaluate_scripts_total_ms"],
        std::time::Duration::from_micros(servo_health.render_evaluate_scripts_total_us)
            .as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_evaluate_scripts_max_ms"],
        std::time::Duration::from_micros(servo_health.render_evaluate_scripts_max_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_event_loop_total_ms"],
        std::time::Duration::from_micros(servo_health.render_event_loop_total_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_event_loop_max_ms"],
        std::time::Duration::from_micros(servo_health.render_event_loop_max_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_paint_total_ms"],
        std::time::Duration::from_micros(servo_health.render_paint_total_us).as_secs_f64() * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_paint_max_ms"],
        std::time::Duration::from_micros(servo_health.render_paint_max_us).as_secs_f64() * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_readback_total_ms"],
        std::time::Duration::from_micros(servo_health.render_readback_total_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_readback_max_ms"],
        std::time::Duration::from_micros(servo_health.render_readback_max_us).as_secs_f64()
            * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_frame_total_ms"],
        std::time::Duration::from_micros(servo_health.render_frame_total_us).as_secs_f64() * 1000.0
    );
    assert_eq!(
        json["effect_health"]["servo_render_frame_max_ms"],
        std::time::Duration::from_micros(servo_health.render_frame_max_us).as_secs_f64() * 1000.0
    );
    assert!(json["effect_health"]["producer_gpu_cpu_materialization_blocked_total"].is_number());
    assert!(json["effect_health"]["sparkleflinger_media_texture_allocations_total"].is_number());
    assert!(json["effect_health"]["sparkleflinger_media_texture_upload_bytes_total"].is_number());
    assert!(
        json["effect_health"]["sparkleflinger_display_finalize_rgba_attempts_total"].is_number()
    );
    assert!(
        json["effect_health"]["sparkleflinger_display_finalize_yuv_attempts_total"].is_number()
    );
    assert!(json["effect_health"]["sparkleflinger_display_finalize_successes_total"].is_number());
    assert!(json["effect_health"]["sparkleflinger_display_finalize_misses_total"].is_number());
    assert!(json["effect_health"]["sparkleflinger_display_finalize_latches_total"].is_number());
    assert!(
        json["effect_health"]["sparkleflinger_display_finalize_blocking_wait_total_ms"].is_number()
    );
    assert!(
        json["effect_health"]["sparkleflinger_display_finalize_blocking_wait_max_ms"].is_number()
    );
    assert!(
        json["effect_health"]["sparkleflinger_display_finalize_surface_reallocs_total"].is_number()
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
    assert_eq!(json["timeline"]["deferred_sample_ms"], 0.06);
    assert_eq!(json["timeline"]["preview_advance_ms"], 0.05);
    assert_eq!(json["stages"]["producer_effect_rendering_ms"], 0.64);
    assert_eq!(json["stages"]["producer_preview_compose_ms"], 0.11);
    assert_eq!(json["stages"]["publish_frame_data_ms"], 0.07);
    assert_eq!(json["stages"]["publish_group_canvas_ms"], 0.02);
    assert_eq!(json["stages"]["publish_preview_ms"], 0.08);
    assert_eq!(json["stages"]["publish_events_ms"], 0.01);
    assert_eq!(json["fps"]["ceiling"], 60);
    assert!(json["fps"].get("actual").is_none());
    assert_eq!(json["fps"]["delivered"], 0.0);
    assert_eq!(json["render_surfaces"]["canvas_receivers"], 2);
    assert_eq!(json["render_surfaces"]["scene_pool_saturation_reallocs"], 0);
    assert_eq!(json["render_surfaces"]["scene_pool_grown_slots"], 0);
    assert_eq!(json["preview"]["canvas_receivers"], 1);
    assert_eq!(json["preview"]["scene_canvas_receivers"], 1);
    assert_eq!(json["preview"]["screen_canvas_receivers"], 1);
    assert_eq!(json["preview"]["canvas_frames_published"], 1);
    assert_eq!(json["preview"]["scene_canvas_frames_published"], 1);
    assert_eq!(json["preview"]["screen_canvas_frames_published"], 1);
    assert_eq!(json["preview"]["latest_canvas_frame_number"], 88);
    assert_eq!(json["preview"]["latest_scene_canvas_frame_number"], 66);
    assert_eq!(json["preview"]["latest_screen_canvas_frame_number"], 45);
    assert_eq!(json["preview"]["canvas_demand"]["subscribers"], 1);
    assert_eq!(json["preview"]["canvas_demand"]["max_fps"], 20);
    assert_eq!(json["preview"]["canvas_demand"]["max_width"], 640);
    assert_eq!(json["preview"]["canvas_demand"]["max_height"], 360);
    assert_eq!(json["preview"]["canvas_demand"]["any_jpeg"], true);
    assert_eq!(json["preview"]["scene_canvas_demand"]["subscribers"], 1);
    assert_eq!(json["preview"]["scene_canvas_demand"]["max_fps"], 12);
    assert_eq!(json["preview"]["scene_canvas_demand"]["max_width"], 320);
    assert_eq!(json["preview"]["scene_canvas_demand"]["max_height"], 180);
    assert_eq!(json["preview"]["scene_canvas_demand"]["any_rgb"], true);
    assert_eq!(json["preview"]["screen_canvas_demand"]["subscribers"], 1);
    assert_eq!(
        json["preview"]["screen_canvas_demand"]["any_full_resolution"],
        true
    );
    assert_eq!(json["preview"]["screen_canvas_demand"]["any_rgba"], true);
}

/// The tolerant subset a metrics client decodes from the timeline payload:
/// unknown keys are ignored and absent keys fall back to zero, so a payload
/// from either side of a version skew still decodes.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
#[allow(
    clippy::struct_field_names,
    reason = "field names must match the protocol keys they decode"
)]
struct ClientTimelineSubset {
    input_done_ms: f64,
    deferred_sample_ms: f64,
    producer_done_ms: f64,
    composition_done_ms: f64,
    preview_advance_ms: f64,
    sampling_done_ms: f64,
}

#[tokio::test]
async fn metrics_timeline_carries_hidden_stage_durations_tolerantly() {
    let tempdir = tempfile::tempdir().expect("metrics test data dir should be created");
    let state = Arc::new(AppState::new_with_data_dir(tempdir.path().join("data")));
    // The metrics builder zeroes the whole frame timeline while the render
    // loop is idle.
    state.render_loop.write().await.start();
    {
        let mut performance = state.performance.write().await;
        performance.record_frame(&LatestFrameMetrics {
            deferred_sample_us: 250,
            preview_advance_us: 1_400,
            timeline: FrameTimeline {
                frame_token: 5,
                input_done_us: 320,
                producer_done_us: 1_040,
                composition_done_us: 1_340,
                sample_done_us: 2_900,
                ..FrameTimeline::default()
            },
            ..LatestFrameMetrics::default()
        });
    }

    let ServerMessage::Metrics { data, .. } = build_metrics_message(&state, 0.0).await else {
        panic!("expected metrics message");
    };
    let json = serde_json::to_value(&data).expect("metrics payload should serialize");
    let timeline = json["timeline"].clone();

    assert_eq!(timeline["deferred_sample_ms"], 0.25);
    assert_eq!(timeline["preview_advance_ms"], 1.4);

    let decoded: ClientTimelineSubset =
        serde_json::from_value(timeline.clone()).expect("current payload should decode");
    assert_eq!(decoded.deferred_sample_ms, 0.25);
    assert_eq!(decoded.preview_advance_ms, 1.4);
    assert_eq!(decoded.producer_done_ms, 1.04);
    assert_eq!(decoded.sampling_done_ms, 2.9);

    // Older daemon: the two duration keys are simply absent.
    let mut legacy = timeline.clone();
    let legacy_object = legacy
        .as_object_mut()
        .expect("timeline payload should be an object");
    legacy_object.remove("deferred_sample_ms");
    legacy_object.remove("preview_advance_ms");
    let legacy_decoded: ClientTimelineSubset =
        serde_json::from_value(legacy).expect("legacy payload should still decode");
    assert_eq!(legacy_decoded.deferred_sample_ms, 0.0);
    assert_eq!(legacy_decoded.preview_advance_ms, 0.0);
    assert_eq!(legacy_decoded.producer_done_ms, 1.04);
    assert_eq!(legacy_decoded.sampling_done_ms, 2.9);

    // Newer daemon: an unknown stage key must not break an older client.
    let mut future = timeline;
    future
        .as_object_mut()
        .expect("timeline payload should be an object")
        .insert("future_stage_ms".to_owned(), serde_json::json!(3.5));
    let future_decoded: ClientTimelineSubset =
        serde_json::from_value(future).expect("future payload should still decode");
    assert_eq!(future_decoded.deferred_sample_ms, 0.25);
    assert_eq!(future_decoded.input_done_ms, 0.32);
}

#[test]
fn device_metrics_message_uses_shared_snapshot() {
    let state = Arc::new(AppState::new());
    let device_id = hypercolor_types::device::DeviceId::new();
    state.device_metrics.store(Arc::new(DeviceMetricsSnapshot {
        taken_at_ms: 2_500,
        items: vec![DeviceMetrics {
            id: device_id,
            backend_id: "usb".to_owned(),
            mapped_layout_ids: vec!["layout-device".to_owned()],
            uses_frame_sink: true,
            worker_finished: false,
            worker_recoveries: 3,
            delivered_fps: 58.5,
            accepted_fps: 60.0,
            fps_sent: 58.5,
            fps_queued: 60.0,
            fps_actual: 58.5,
            fps_target: 60,
            target_interval_ms: Some(17),
            payload_bps_estimate: 2_048,
            avg_latency_ms: 11,
            avg_queue_wait_ms: 3,
            avg_write_ms: 8,
            avg_transport_latency_ms: 8,
            frames_received: 302,
            accepted: 304,
            frames_sent: 300,
            transport_started: 301,
            transport_completed: 300,
            transport_failed: 1,
            completed_payload_bytes: 153_600,
            frames_suppressed: 0,
            frames_dropped: 4,
            coalesced: 4,
            coalesced_target_cadence: 3,
            coalesced_backend_overrun: 1,
            errors_total: 1,
            write_failure_warnings_total: 1,
            last_error: Some("socket timeout".to_owned()),
            last_sent_ago_ms: Some(12),
            last_sequence: 302,
            queue_generation: 12,
            last_transport_started_sequence: 302,
            last_transport_completed_sequence: 301,
            last_transport_failed_sequence: 302,
        }],
    }));

    let ServerMessage::DeviceMetrics { data, .. } = build_device_metrics_message(&state) else {
        panic!("expected device_metrics message");
    };

    assert_eq!(data.taken_at_ms, 2_500);
    assert_eq!(data.items.len(), 1);
    assert_eq!(data.items[0].id, device_id);
    assert_eq!(data.items[0].backend_id, "usb");
    assert!(data.items[0].uses_frame_sink);
    assert_eq!(data.items[0].worker_recoveries, 3);
    assert_eq!(data.items[0].avg_queue_wait_ms, 3);
    assert_eq!(data.items[0].avg_write_ms, 8);
    assert_eq!(data.items[0].avg_transport_latency_ms, 8);
    assert_eq!(data.items[0].transport_completed, 300);
    assert_eq!(data.items[0].transport_failed, 1);
    assert_eq!(data.items[0].coalesced_target_cadence, 3);
    assert_eq!(data.items[0].coalesced_backend_overrun, 1);
    assert_eq!(data.items[0].payload_bps_estimate, 2_048);
}

#[tokio::test]
async fn relay_metrics_wakes_when_subscription_changes() {
    let state = Arc::new(AppState::new());
    let initial_subscriptions = SubscriptionState::default();
    let (subscriptions_tx, subscriptions_rx) = watch::channel(initial_subscriptions.clone());
    let (json_tx, mut json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(1);

    let relay_handle = tokio::spawn(relay_metrics(Arc::clone(&state), json_tx, subscriptions_rx));

    let subscriptions = initial_subscriptions
        .subscribed_unkeyed(&["metrics"], serde_json::json!({"metrics": {"fps": 10.0}}))
        .expect("metrics subscribe applies");
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
            backend_id: "usb".to_owned(),
            mapped_layout_ids: vec!["layout-device".to_owned()],
            uses_frame_sink: true,
            worker_finished: false,
            worker_recoveries: 0,
            delivered_fps: 60.0,
            accepted_fps: 60.0,
            fps_sent: 60.0,
            fps_queued: 60.0,
            fps_actual: 60.0,
            fps_target: 60,
            target_interval_ms: Some(17),
            payload_bps_estimate: 512,
            avg_latency_ms: 8,
            avg_queue_wait_ms: 2,
            avg_write_ms: 6,
            avg_transport_latency_ms: 6,
            frames_received: 42,
            accepted: 42,
            frames_sent: 42,
            transport_started: 42,
            transport_completed: 42,
            transport_failed: 0,
            completed_payload_bytes: 21_504,
            frames_suppressed: 0,
            frames_dropped: 0,
            coalesced: 0,
            coalesced_target_cadence: 0,
            coalesced_backend_overrun: 0,
            errors_total: 0,
            write_failure_warnings_total: 0,
            last_error: None,
            last_sent_ago_ms: Some(7),
            last_sequence: 42,
            queue_generation: 13,
            last_transport_started_sequence: 42,
            last_transport_completed_sequence: 42,
            last_transport_failed_sequence: 0,
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

    let subscriptions = initial_subscriptions
        .subscribed_unkeyed(
            &["device_metrics"],
            serde_json::json!({"device_metrics": {"fps": 10.0}}),
        )
        .expect("device metrics subscribe applies");
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
async fn relay_sensors_streams_latest_snapshot_from_watch() {
    let state = Arc::new(AppState::new());
    let mut initial = SystemSnapshot::empty();
    initial.cpu_load_percent = 42.0;
    initial.polled_at_ms = 1_000;
    let (sensor_tx, sensor_rx) = watch::channel(Arc::new(initial));
    state
        .input_manager
        .lock()
        .await
        .set_sensor_snapshot_receiver(sensor_rx);

    let initial_subscriptions = SubscriptionState::default();
    let (subscriptions_tx, subscriptions_rx) = watch::channel(initial_subscriptions.clone());
    let (json_tx, mut json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(2);

    let relay_handle = tokio::spawn(relay_sensors(Arc::clone(&state), json_tx, subscriptions_rx));

    let subscriptions = initial_subscriptions
        .subscribed_unkeyed(&["sensors"], serde_json::Value::Null)
        .expect("sensors subscribe applies");
    publish_subscriptions(&subscriptions_tx, &subscriptions);

    let message = tokio::time::timeout(std::time::Duration::from_millis(250), json_rx.recv())
        .await
        .expect("sensors relay should wake without idle polling")
        .expect("sensors relay should emit current snapshot");
    let payload: serde_json::Value =
        serde_json::from_str(message.as_str()).expect("sensors payload should parse");
    assert_eq!(payload["type"], "sensors");
    assert_eq!(payload["data"]["cpu_load_percent"], 42.0);
    assert_eq!(payload["data"]["polled_at_ms"], 1_000);

    let mut next = SystemSnapshot::empty();
    next.cpu_load_percent = 55.0;
    next.polled_at_ms = 2_000;
    sensor_tx.send_replace(Arc::new(next));

    let message = tokio::time::timeout(std::time::Duration::from_millis(250), json_rx.recv())
        .await
        .expect("sensors relay should follow watch changes")
        .expect("sensors relay should emit updated snapshot");
    let payload: serde_json::Value =
        serde_json::from_str(message.as_str()).expect("sensors payload should parse");
    assert_eq!(payload["type"], "sensors");
    assert_eq!(payload["data"]["cpu_load_percent"], 55.0);
    assert_eq!(payload["data"]["polled_at_ms"], 2_000);

    relay_handle.abort();
}

#[tokio::test]
async fn relay_sensors_coalesces_to_latest_snapshot_while_output_is_full() {
    let state = Arc::new(AppState::new());
    let mut initial = SystemSnapshot::empty();
    initial.polled_at_ms = 1_000;
    let (sensor_tx, sensor_rx) = watch::channel(Arc::new(initial));
    state
        .input_manager
        .lock()
        .await
        .set_sensor_snapshot_receiver(sensor_rx);

    let initial_subscriptions = SubscriptionState::default();
    let (subscriptions_tx, subscriptions_rx) = watch::channel(initial_subscriptions.clone());
    let (json_tx, mut json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(1);
    json_tx
        .try_send("occupied".into())
        .expect("queue accepts its first message");

    let relay_handle = tokio::spawn(relay_sensors(Arc::clone(&state), json_tx, subscriptions_rx));
    let subscriptions = initial_subscriptions
        .subscribed_unkeyed(&["sensors"], serde_json::Value::Null)
        .expect("sensors subscribe applies");
    publish_subscriptions(&subscriptions_tx, &subscriptions);

    for polled_at_ms in [2_000, 3_000, 4_000] {
        let mut snapshot = SystemSnapshot::empty();
        snapshot.polled_at_ms = polled_at_ms;
        sensor_tx.send_replace(Arc::new(snapshot));
        tokio::task::yield_now().await;
    }

    assert_eq!(
        json_rx
            .recv()
            .await
            .expect("occupied payload remains")
            .as_str(),
        "occupied"
    );
    let message = tokio::time::timeout(std::time::Duration::from_millis(250), json_rx.recv())
        .await
        .expect("sensors relay should resume after capacity returns")
        .expect("sensors relay should emit the coalesced snapshot");
    let payload: serde_json::Value =
        serde_json::from_str(message.as_str()).expect("sensors payload should parse");
    assert_eq!(payload["type"], "sensors");
    assert_eq!(payload["data"]["polled_at_ms"], 4_000);

    relay_handle.abort();
}

#[tokio::test]
async fn relay_frames_wakes_when_subscription_changes() {
    let initial_subscriptions = SubscriptionState::default();
    let (subscriptions_tx, subscriptions_rx) = watch::channel(initial_subscriptions.clone());
    let (json_tx, _json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(1);
    let (binary_tx, mut binary_rx) = tokio::sync::mpsc::channel::<Bytes>(1);
    let state = Arc::new(AppState::new());
    let _ = state.event_bus.frame_lane().send(sample_frame());

    let relay_handle = tokio::spawn(relay_frames(
        Arc::clone(&state),
        json_tx,
        binary_tx,
        subscriptions_rx,
    ));
    assert_eq!(state.event_bus.frame_receiver_count(), 0);

    let mut subscriptions = initial_subscriptions
        .subscribed_unkeyed(&["frames"], serde_json::Value::Null)
        .expect("frames subscribe applies");
    publish_subscriptions(&subscriptions_tx, &subscriptions);

    let payload = tokio::time::timeout(std::time::Duration::from_millis(250), binary_rx.recv())
        .await
        .expect("frame relay should wake on subscription changes")
        .expect("frame relay should publish the latest cached frame");
    assert_eq!(payload.first().copied(), Some(0x01));
    assert_eq!(state.event_bus.frame_receiver_count(), 1);

    subscriptions = subscriptions.unsubscribed_unkeyed(&["frames"]);
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
    let _ = state.event_bus.spectrum_lane().send(SpectrumData::empty());

    let relay_handle = tokio::spawn(relay_spectrum(
        Arc::clone(&state),
        json_tx,
        binary_tx,
        subscriptions_rx,
    ));
    assert_eq!(state.event_bus.spectrum_receiver_count(), 0);

    let mut subscriptions = initial_subscriptions
        .subscribed_unkeyed(&["spectrum"], serde_json::Value::Null)
        .expect("spectrum subscribe applies");
    publish_subscriptions(&subscriptions_tx, &subscriptions);

    let payload = tokio::time::timeout(std::time::Duration::from_millis(250), binary_rx.recv())
        .await
        .expect("spectrum relay should wake on subscription changes")
        .expect("spectrum relay should publish the latest cached spectrum");
    assert_eq!(payload.first().copied(), Some(0x02));
    assert_eq!(state.event_bus.spectrum_receiver_count(), 1);

    subscriptions = subscriptions.unsubscribed_unkeyed(&["spectrum"]);
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

async fn publish_display_preview_snapshot(
    display_frames: &Arc<RwLock<DisplayFrameRuntime>>,
    device_id: DeviceId,
    frame_number: u64,
) {
    display_frames.write().await.set_frame(
        device_id,
        DisplayFrameSnapshot {
            jpeg_data: Arc::new(jpeg_test_payload(32, 32, 16)),
            width: 32,
            height: 32,
            circular: false,
            frame_number,
            captured_at: SystemTime::UNIX_EPOCH + Duration::from_millis(frame_number),
        },
    );
}

async fn wait_for_display_preview_subscribers(
    display_frames: &Arc<RwLock<DisplayFrameRuntime>>,
    expected: usize,
) {
    tokio::time::timeout(Duration::from_millis(250), async {
        loop {
            if display_frames
                .read()
                .await
                .metrics_snapshot()
                .preview_subscribers
                == expected
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("display preview subscriber count should settle");
}

/// The device every display-preview cache test attributes its frames to.
fn test_display_device() -> hypercolor_types::device::DeviceId {
    hypercolor_types::device::DeviceId(uuid::Uuid::from_u128(0x0d15_71a7_0000_0001))
}

fn decoded_display_preview(payload: &Bytes) -> WireDisplayPreviewFrame {
    assert_eq!(payload.first().copied(), Some(WS_DISPLAY_PREVIEW_HEADER));
    WireDisplayPreviewFrame::decode_bytes(payload)
        .expect("display preview payload decodes as a keyed display frame")
}

fn display_preview_payload_frame_number(payload: &Bytes) -> u32 {
    decoded_display_preview(payload).frame_number
}

#[test]
fn byte_bounded_lru_evicts_by_weight_and_rejects_oversized_entries() {
    let mut cache = std::collections::VecDeque::new();

    assert!(put_bytes_lru(
        &mut cache,
        1_u8,
        Bytes::from_static(b"12345"),
        4,
        8,
    ));
    assert!(put_bytes_lru(
        &mut cache,
        2_u8,
        Bytes::from_static(b"67890"),
        4,
        8,
    ));
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.front().map(|(key, _)| *key), Some(2));

    assert!(!put_bytes_lru(
        &mut cache,
        3_u8,
        Bytes::from_static(b"oversized"),
        4,
        8,
    ));
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.front().map(|(key, _)| *key), Some(2));
}

fn display_preview_test_frame(frame_number: u32, payload_len: usize) -> Bytes {
    WireDisplayPreviewFrame {
        device_id: test_display_device().to_string(),
        frame_number,
        timestamp_ms: frame_number,
        width: 1,
        height: 1,
        format: WirePreviewPixelFormat::Jpeg,
        payload: Bytes::from(jpeg_test_payload(1, 1, payload_len)),
    }
    .encode()
    .expect("display preview test frame")
}

fn preview_test_frame(
    channel: PreviewFrameChannel,
    frame_number: u32,
    payload_len: usize,
) -> Bytes {
    WirePreviewFrame {
        channel,
        frame_number,
        timestamp_ms: frame_number,
        width: 1,
        height: 1,
        format: WirePreviewPixelFormat::Jpeg,
        payload: Bytes::from(jpeg_test_payload(1, 1, payload_len)),
    }
    .try_encode()
    .expect("preview test frame")
}

fn jpeg_test_payload(width: u16, height: u16, payload_len: usize) -> Vec<u8> {
    let mut payload = vec![
        0xFF,
        0xD8,
        0xFF,
        0xC0,
        0x00,
        0x07,
        0x08,
        height.to_be_bytes()[0],
        height.to_be_bytes()[1],
        width.to_be_bytes()[0],
        width.to_be_bytes()[1],
    ];
    assert!(payload_len >= payload.len());
    payload.resize(payload_len, 0);
    payload
}

#[test]
fn subscribe_wire_has_no_transport_negotiation() {
    let message: ClientMessage = serde_json::from_value(serde_json::json!({
        "type": "subscribe",
        "topics": [{ "topic": "events" }]
    }))
    .expect("subscribe parses");
    assert!(matches!(message, ClientMessage::Subscribe { .. }));
}

async fn receive_direct_preview(receiver: &PreviewOutboundReceiver) -> Bytes {
    let publication = tokio::time::timeout(Duration::from_millis(250), async {
        loop {
            if let PreviewOutboundItem::Publication(publication) = receiver.recv().await {
                break publication;
            }
        }
    })
    .await
    .expect("preview publication should arrive");
    let mut cursor = PreviewSendCursor::new(publication, super::protocol::MAX_WS_MESSAGE_BYTES)
        .expect("direct preview cursor");
    assert!(!cursor.is_chunked());
    cursor
        .next_message()
        .expect("direct preview encoding")
        .expect("direct preview message")
}

#[tokio::test(start_paused = true)]
async fn relay_screen_zones_paces_each_connection_and_sends_the_latest_frame() {
    let state = Arc::new(AppState::new());
    let subscriptions = SubscriptionState::default()
        .subscribed_unkeyed(
            &["screen_zones"],
            serde_json::json!({"screen_zones": {"fps": 10}}),
        )
        .expect("screen zones subscribe applies");
    let (_subscriptions_tx, subscriptions_rx) = watch::channel(subscriptions);
    let (preview_tx, preview_rx) = preview_outbound_channel();
    let relay_handle = tokio::spawn(relay_screen_zones(
        Arc::clone(&state.preview_runtime),
        subscriptions_rx,
        preview_tx,
    ));

    let initial = receive_direct_preview(&preview_rx).await;
    let initial = hypercolor_leptos_ext::ws::ScreenZonesFrame::decode(&initial)
        .expect("initial screen zones frame decodes");
    assert_eq!(initial.frame_number, 0);

    for frame_number in [1, 2] {
        state
            .event_bus
            .screen_zones_lane()
            .send_replace(hypercolor_core::bus::ScreenZonesFrame {
                frame_number,
                timestamp_ms: frame_number,
                source_width: 1,
                source_height: 1,
                grid_cols: 1,
                grid_rows: 1,
                letterbox: [0; 4],
                colors: vec![[
                    u8::try_from(frame_number).expect("small frame number"),
                    2,
                    3,
                ]]
                .into(),
            });
        tokio::task::yield_now().await;
    }

    tokio::time::advance(Duration::from_millis(99)).await;
    assert!(
        preview_rx.try_recv().is_none(),
        "the connection's 10 fps cadence must not inherit a faster producer cadence"
    );
    tokio::time::advance(Duration::from_millis(1)).await;
    let latest = receive_direct_preview(&preview_rx).await;
    let latest = hypercolor_leptos_ext::ws::ScreenZonesFrame::decode(&latest)
        .expect("paced screen zones frame decodes");
    assert_eq!(latest.frame_number, 2);

    relay_handle.abort();
    let _ = relay_handle.await;
}

#[tokio::test]
async fn relay_zone_preview_cancels_streams_retired_by_scene_changes() {
    let state = Arc::new(AppState::new());
    let subscriptions = SubscriptionState::default()
        .subscribed_unkeyed(
            &["zone_preview"],
            serde_json::json!({"zone_preview": {"fps": 60}}),
        )
        .expect("zone preview subscribe applies");
    let (_subscriptions_tx, subscriptions_rx) = watch::channel(subscriptions);
    let (preview_tx, preview_rx) = preview_outbound_channel();
    let relay_handle = tokio::spawn(relay_zone_preview(
        Arc::clone(&state.preview_runtime),
        preview_tx,
        subscriptions_rx,
    ));

    let scene_a = SceneId::new();
    let scene_b = SceneId::new();
    let zone_a = ZoneId::new();
    let zone_b = ZoneId::new();
    let mut canvas = Canvas::new(1, 1);
    canvas.set_pixel(0, 0, Rgba::new(1, 2, 3, 255));
    state.event_bus.zone_preview_lane().send_replace(
        vec![ZonePreviewFrame {
            scene_id: scene_a,
            zone_id: zone_a,
            frame: CanvasFrame::from_canvas(&canvas, 1, 1),
        }]
        .into(),
    );

    let first = tokio::time::timeout(Duration::from_millis(250), async {
        loop {
            if let PreviewOutboundItem::Publication(publication) = preview_rx.recv().await {
                break publication;
            }
        }
    })
    .await
    .expect("first scene zone preview should publish");
    assert_eq!(
        first.stream(),
        &PreviewStreamId::Zone {
            scene_id: *scene_a.0.as_bytes(),
            zone_id: *zone_a.0.as_bytes(),
        }
    );

    state.event_bus.zone_preview_lane().send_replace(
        vec![ZonePreviewFrame {
            scene_id: scene_b,
            zone_id: zone_b,
            frame: CanvasFrame::from_canvas(&canvas, 2, 2),
        }]
        .into(),
    );

    let retired_stream = PreviewStreamId::Zone {
        scene_id: *scene_a.0.as_bytes(),
        zone_id: *zone_a.0.as_bytes(),
    };
    let active_stream = PreviewStreamId::Zone {
        scene_id: *scene_b.0.as_bytes(),
        zone_id: *zone_b.0.as_bytes(),
    };
    let (cancellation, second) = tokio::time::timeout(Duration::from_millis(250), async {
        let mut cancellation = None;
        let mut second = None;
        while cancellation.is_none() || second.is_none() {
            match preview_rx.recv().await {
                PreviewOutboundItem::Cancellation(message) => cancellation = Some(message),
                PreviewOutboundItem::Publication(publication) => second = Some(publication),
            }
        }
        (
            cancellation.expect("cancellation present"),
            second.expect("replacement present"),
        )
    })
    .await
    .expect("scene switch should cancel the retired stream and publish the active one");
    assert_eq!(cancellation.stream, retired_stream);
    assert_eq!(second.stream(), &active_stream);
    assert!(!preview_rx.is_current(&first));
    assert!(preview_rx.is_current(&second));

    preview_rx.complete(&first);
    preview_rx.complete(&second);
    relay_handle.abort();
    let _ = relay_handle.await;
}

fn try_receive_preview_publication(
    receiver: &PreviewOutboundReceiver,
) -> Option<PreviewPublication> {
    loop {
        match receiver.try_recv()? {
            PreviewOutboundItem::Publication(publication) => return Some(publication),
            PreviewOutboundItem::Cancellation(_) => {}
        }
    }
}

/// A keyed display preview frame for one device.
fn display_preview_frame_for(device_id: &str, frame_number: u32, payload_len: usize) -> Bytes {
    WireDisplayPreviewFrame {
        device_id: device_id.to_owned(),
        frame_number,
        timestamp_ms: frame_number,
        width: 1,
        height: 1,
        format: WirePreviewPixelFormat::Jpeg,
        payload: Bytes::from(jpeg_test_payload(1, 1, payload_len)),
    }
    .encode()
    .expect("keyed display preview test frame")
}

#[test]
fn cancelling_one_key_leaves_a_sibling_subscription_streaming() {
    // Retiring one display must not cancel another's stream. The router
    // holds one entry per stream identity, so this is the check that the
    // per-key match is a key match and not a topic match.
    let (sender, receiver) = preview_outbound_channel();
    for device in ["device-a", "device-b"] {
        sender
            .publish(
                PreviewStreamId::Display(device.to_owned()),
                display_preview_frame_for(device, 1, 64),
                None,
            )
            .expect("each device publishes its own stream");
    }

    let cancelled = sender
        .cancel_subscription(TopicId::DisplayPreview, Some("device-a"))
        .expect("cancelling one key succeeds");
    assert_eq!(cancelled, 1, "exactly one stream is retired");

    let mut cancellations = Vec::new();
    let mut publications = Vec::new();
    while let Some(item) = receiver.try_recv() {
        match item {
            PreviewOutboundItem::Cancellation(cancellation) => {
                cancellations.push(cancellation.stream.clone());
            }
            PreviewOutboundItem::Publication(publication) => {
                publications.push(publication.stream().clone());
            }
        }
    }
    assert_eq!(
        cancellations,
        vec![PreviewStreamId::Display("device-a".to_owned())]
    );
    assert!(
        publications.contains(&PreviewStreamId::Display("device-b".to_owned())),
        "the sibling keeps streaming: {publications:?}"
    );
    assert!(
        !publications.contains(&PreviewStreamId::Display("device-a".to_owned())),
        "the retired key's queued publication is dropped: {publications:?}"
    );
}

#[test]
fn cancelling_a_keyed_topic_without_a_key_retires_every_key() {
    // A topic-wide teardown (the connection closing, say) has no key, and
    // that has to mean "all of them" rather than "none of them".
    let (sender, _receiver) = preview_outbound_channel();
    for device in ["device-a", "device-b"] {
        sender
            .publish(
                PreviewStreamId::Display(device.to_owned()),
                display_preview_frame_for(device, 1, 64),
                None,
            )
            .expect("each device publishes its own stream");
    }

    assert_eq!(
        sender
            .cancel_subscription(TopicId::DisplayPreview, None)
            .expect("a keyless cancel succeeds"),
        2
    );
}

#[test]
fn a_keyed_display_publication_survives_chunking_and_reassembly() {
    // Display preview joined the chunked transport with a new stream kind,
    // so a publication that has to be split must come back naming the same
    // device — narrow and wide alike.
    for (label, width, height) in [("narrow", 64_u32, 64_u32), ("wide", 70_001, 1)] {
        let device_id = "3f2504e0-4f89-11d3-9a0c-0305e82c3301";
        let frame = WireDisplayPreviewFrame {
            device_id: device_id.to_owned(),
            frame_number: 9,
            timestamp_ms: 11,
            width,
            height,
            format: WirePreviewPixelFormat::Rgb,
            payload: Bytes::from(vec![0x5A; (width as usize) * (height as usize) * 3]),
        };
        let encoded = frame.encode().expect("display frame encodes");
        let metadata = hypercolor_leptos_ext::ws::PreviewPublicationMetadata {
            stream: PreviewStreamId::Display(device_id.to_owned()),
            publication_id: 7,
            frame_number: frame.frame_number,
            timestamp_ms: frame.timestamp_ms,
            width: frame.width,
            height: frame.height,
            format: frame.format,
        };
        let chunks =
            hypercolor_leptos_ext::ws::split_preview_publication(&encoded, &metadata, 4096)
                .unwrap_or_else(|error| panic!("{label} display publication splits: {error}"));
        assert!(chunks.len() > 1, "{label} publication is actually chunked");

        let mut reassembler = hypercolor_leptos_ext::ws::PreviewChunkReassembler::new(
            hypercolor_leptos_ext::ws::PreviewReassemblyLimits::default(),
        );
        let mut completed = None;
        for chunk in &chunks {
            completed = reassembler
                .push(chunk)
                .unwrap_or_else(|error| panic!("{label} chunk is accepted: {error}"));
        }
        let completed = completed.unwrap_or_else(|| panic!("{label} publication reassembles"));
        assert_eq!(completed.metadata, metadata, "{label} metadata round-trips");
        let decoded = WireDisplayPreviewFrame::decode_bytes(&completed.encoded)
            .unwrap_or_else(|error| panic!("{label} reassembled frame decodes: {error}"));
        assert_eq!(decoded, frame, "{label} frame round-trips");
    }
}

#[test]
fn cached_display_preview_payload_reuses_bytes_for_matching_snapshot() {
    let _guard = WS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    reset_ws_payload_caches();
    let snapshot = DisplayFrameSnapshot {
        jpeg_data: Arc::new(jpeg_test_payload(32, 32, 16)),
        width: 32,
        height: 32,
        circular: false,
        frame_number: 17,
        captured_at: SystemTime::UNIX_EPOCH + Duration::from_millis(99),
    };

    let first = cached_display_preview_payload(test_display_device(), &snapshot)
        .expect("first display preview payload");
    let second = cached_display_preview_payload(test_display_device(), &snapshot)
        .expect("second display preview payload");

    assert_eq!(display_preview_payload_frame_number(&first), 17);
    assert_eq!(first, second);
    assert_eq!(first.as_ptr(), second.as_ptr());
}

#[test]
fn cached_display_preview_payload_skips_cache_for_large_payloads() {
    let _guard = WS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    reset_ws_payload_caches();
    let large_jpeg = jpeg_test_payload(512, 512, 300 * 1024);
    let snapshot = DisplayFrameSnapshot {
        jpeg_data: Arc::new(large_jpeg),
        width: 512,
        height: 512,
        circular: false,
        frame_number: 21,
        captured_at: SystemTime::UNIX_EPOCH + Duration::from_millis(101),
    };

    let first = cached_display_preview_payload(test_display_device(), &snapshot)
        .expect("first display preview payload");
    let second = cached_display_preview_payload(test_display_device(), &snapshot)
        .expect("second display preview payload");

    assert_eq!(display_preview_payload_frame_number(&first), 21);
    assert_eq!(first, second);
    assert_ne!(first.as_ptr(), second.as_ptr());
}

fn display_preview_snapshot(jpeg_len: usize, frame_number: u64) -> DisplayFrameSnapshot {
    DisplayFrameSnapshot {
        jpeg_data: Arc::new(jpeg_test_payload(256, 256, jpeg_len)),
        width: 256,
        height: 256,
        circular: false,
        frame_number,
        captured_at: SystemTime::UNIX_EPOCH + Duration::from_millis(frame_number),
    }
}

#[test]
fn cached_display_preview_payload_respects_the_size_boundary() {
    let _guard = WS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    reset_ws_payload_caches();

    // Derive the wire-header length from a probe payload so the boundary math
    // tracks the real header layout instead of a hard-coded guess.
    let probe_payload_len = 16;
    let probe = cached_display_preview_payload(
        test_display_device(),
        &display_preview_snapshot(probe_payload_len, 30),
    )
    .expect("display preview probe payload");
    let header_len = probe.len() - probe_payload_len;
    reset_ws_payload_caches();

    // The cache key includes the jpeg Arc's storage address, so both calls
    // must share one snapshot; per-call snapshots only matched before when
    // the allocator happened to reuse the freed Vec's address.
    let at_limit = WS_DISPLAY_PREVIEW_PAYLOAD_CACHE_MAX_BYTES - header_len;
    let snapshot = display_preview_snapshot(at_limit, 31);
    let first = cached_display_preview_payload(test_display_device(), &snapshot)
        .expect("first display preview payload");
    let second = cached_display_preview_payload(test_display_device(), &snapshot)
        .expect("second display preview payload");
    assert_eq!(
        first.as_ptr(),
        second.as_ptr(),
        "a payload exactly at the cache size limit should be cached"
    );

    reset_ws_payload_caches();
    let snapshot = display_preview_snapshot(at_limit + 1, 32);
    let first = cached_display_preview_payload(test_display_device(), &snapshot)
        .expect("first display preview payload");
    let second = cached_display_preview_payload(test_display_device(), &snapshot)
        .expect("second display preview payload");
    assert_ne!(
        first.as_ptr(),
        second.as_ptr(),
        "a payload one byte over the limit should skip the cache"
    );
}

#[test]
fn preview_router_replaces_same_stream_with_latest() {
    let (sender, receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: 256,
        max_connection_bytes: 512,
    });
    let stream = PreviewStreamId::Passive(PreviewFrameChannel::Canvas);

    assert_eq!(
        sender
            .publish(
                stream.clone(),
                preview_test_frame(PreviewFrameChannel::Canvas, 1, 32),
                None,
            )
            .expect("first preview publication"),
        PreviewPublishOutcome::Queued
    );
    assert_eq!(
        sender
            .publish(
                stream,
                preview_test_frame(PreviewFrameChannel::Canvas, 2, 32),
                None,
            )
            .expect("replacement preview publication"),
        PreviewPublishOutcome::Replaced
    );

    let publication =
        try_receive_preview_publication(&receiver).expect("latest preview publication");
    let mut cursor = PreviewSendCursor::new(publication, super::protocol::MAX_WS_MESSAGE_BYTES)
        .expect("latest preview cursor");
    let encoded = cursor
        .next_message()
        .expect("latest preview encoding")
        .expect("latest preview message");
    let decoded = WirePreviewFrame::decode_bytes(&encoded).expect("latest preview frame");
    assert_eq!(decoded.frame_number, 2);
    assert!(receiver.try_recv().is_none());
}

#[tokio::test]
async fn preview_router_retries_the_latest_stream_after_capacity_frees() {
    let canvas = preview_test_frame(PreviewFrameChannel::Canvas, 1, 64);
    let screen = preview_test_frame(PreviewFrameChannel::ScreenCanvas, 2, 64);
    let publication_bytes = canvas.len().max(screen.len());
    let (sender, receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: publication_bytes,
        max_connection_bytes: publication_bytes,
    });

    sender
        .publish(
            PreviewStreamId::Passive(PreviewFrameChannel::Canvas),
            canvas,
            None,
        )
        .expect("canvas preview publication");
    let waiting_sender = sender.clone();
    let waiting = tokio::spawn(async move {
        super::relays::publish_preview(
            &waiting_sender,
            PreviewStreamId::Passive(PreviewFrameChannel::ScreenCanvas),
            screen,
            "screen_canvas",
        )
        .await
    });
    tokio::task::yield_now().await;
    assert!(
        !waiting.is_finished(),
        "a second stream waits instead of evicting an unsent latest value"
    );

    let canvas = try_receive_preview_publication(&receiver).expect("canvas preview publication");
    receiver.complete(&canvas);
    assert!(waiting.await.expect("waiting publication task"));

    let publication = try_receive_preview_publication(&receiver)
        .expect("screen preview publishes after capacity frees");
    let mut cursor = PreviewSendCursor::new(publication, super::protocol::MAX_WS_MESSAGE_BYTES)
        .expect("remaining preview cursor");
    let encoded = cursor
        .next_message()
        .expect("remaining preview encoding")
        .expect("remaining preview message");
    let decoded = WirePreviewFrame::decode_bytes(&encoded).expect("remaining preview frame");
    assert_eq!(decoded.channel, PreviewFrameChannel::ScreenCanvas);
    assert!(receiver.try_recv().is_none());
}

#[tokio::test]
async fn preview_capacity_wait_yields_to_subscription_changes() {
    let canvas = preview_test_frame(PreviewFrameChannel::Canvas, 1, 64);
    let screen = preview_test_frame(PreviewFrameChannel::ScreenCanvas, 2, 64);
    let publication_bytes = canvas.len().max(screen.len());
    let (sender, receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: publication_bytes,
        max_connection_bytes: publication_bytes,
    });
    sender
        .publish(
            PreviewStreamId::Passive(PreviewFrameChannel::Canvas),
            canvas,
            None,
        )
        .expect("canvas preview publication");
    let in_flight =
        try_receive_preview_publication(&receiver).expect("canvas preview moves in flight");

    let (subscriptions_tx, mut subscriptions_rx) = watch::channel(SubscriptionState::default());
    let waiting_sender = sender.clone();
    let waiting = tokio::spawn(async move {
        publish_preview_while_subscribed(
            &waiting_sender,
            PreviewStreamId::Passive(PreviewFrameChannel::ScreenCanvas),
            screen,
            "screen_canvas",
            &mut subscriptions_rx,
        )
        .await
    });
    tokio::task::yield_now().await;
    assert!(
        !waiting.is_finished(),
        "publication should be capacity-bound"
    );

    subscriptions_tx
        .send(SubscriptionState::default())
        .expect("subscription relay remains live");
    assert_eq!(
        waiting.await.expect("waiting publication task"),
        PreviewRelayPublish::SubscriptionChanged
    );
    assert!(receiver.try_recv().is_none());
    receiver.complete(&in_flight);
}

#[tokio::test]
async fn preview_capacity_wait_yields_to_task_cancellation() {
    let canvas = preview_test_frame(PreviewFrameChannel::Canvas, 1, 64);
    let display = display_preview_test_frame(2, 64);
    let publication_bytes = canvas.len().max(display.len());
    let (sender, receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: publication_bytes,
        max_connection_bytes: publication_bytes,
    });
    sender
        .publish(
            PreviewStreamId::Passive(PreviewFrameChannel::Canvas),
            canvas,
            None,
        )
        .expect("canvas preview publication");
    let in_flight =
        try_receive_preview_publication(&receiver).expect("canvas preview moves in flight");

    let cancel = CancellationToken::new();
    let waiting_cancel = cancel.clone();
    let waiting_sender = sender.clone();
    let waiting = tokio::spawn(async move {
        publish_preview_until_cancelled(
            &waiting_sender,
            PreviewStreamId::Display(test_display_device().to_string()),
            display,
            "display_preview",
            &waiting_cancel,
        )
        .await
    });
    tokio::task::yield_now().await;
    assert!(
        !waiting.is_finished(),
        "publication should be capacity-bound"
    );

    cancel.cancel();
    assert_eq!(waiting.await.expect("waiting publication task"), None);
    assert!(receiver.try_recv().is_none());
    receiver.complete(&in_flight);
}

#[tokio::test]
async fn relay_display_preview_reattaches_after_frame_stream_reopens() {
    let state = Arc::new(AppState::new());
    let config = crate::simulators::SimulatedDisplayConfig {
        id: DeviceId::new(),
        name: "WS Preview Display".to_owned(),
        width: 240,
        height: 160,
        circular: false,
        enabled: true,
    }
    .normalized();
    let device_id = state.device_registry.add(config.device_info()).await;
    let display_frames = Arc::new(RwLock::new(DisplayFrameRuntime::new()));
    let subscriptions = SubscriptionState::default()
        .subscribed(vec![
            TopicSubscription::keyed("display_preview", device_id.to_string())
                .with_config(serde_json::json!({"fps": 30})),
        ])
        .expect("display preview subscribe applies");
    let (_subscriptions_tx, subscriptions_rx) = watch::channel(subscriptions);
    let (preview_tx, preview_rx) = preview_outbound_channel();

    let relay_handle = tokio::spawn(relay_display_preview(
        Arc::clone(&state),
        Arc::clone(&display_frames),
        preview_tx,
        subscriptions_rx,
    ));

    wait_for_display_preview_subscribers(&display_frames, 1).await;
    publish_display_preview_snapshot(&display_frames, device_id, 1).await;
    let first = receive_direct_preview(&preview_rx).await;
    let decoded = decoded_display_preview(&first);
    assert_eq!(decoded.frame_number, 1);
    assert_eq!(
        decoded.device_id,
        device_id.to_string(),
        "a keyed display frame names the device it came from"
    );

    display_frames.write().await.remove(device_id);
    let cancellation = tokio::time::timeout(Duration::from_millis(250), preview_rx.recv())
        .await
        .expect("display removal should retire the wire stream");
    let PreviewOutboundItem::Cancellation(cancellation) = cancellation else {
        panic!("display removal should emit a cancellation");
    };
    assert_eq!(
        cancellation.stream,
        PreviewStreamId::Display(device_id.to_string())
    );
    tokio::time::timeout(Duration::from_millis(250), async {
        loop {
            let runtime = display_frames.read().await;
            if runtime.frame(device_id).is_none()
                && runtime.metrics_snapshot().preview_subscribers == 1
            {
                break;
            }
            drop(runtime);
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("display preview relay should reattach after the sender closes");

    publish_display_preview_snapshot(&display_frames, device_id, 2).await;
    let second = receive_direct_preview(&preview_rx).await;
    assert_eq!(display_preview_payload_frame_number(&second), 2);

    relay_handle.abort();
    let _ = relay_handle.await;
}

#[tokio::test]
async fn relay_display_preview_attaches_when_an_unknown_device_connects() {
    let state = Arc::new(AppState::new());
    let display_frames = Arc::new(RwLock::new(DisplayFrameRuntime::new()));
    let config = crate::simulators::SimulatedDisplayConfig {
        id: DeviceId::new(),
        name: "Late WS Preview Display".to_owned(),
        width: 240,
        height: 160,
        circular: false,
        enabled: true,
    }
    .normalized();
    let unknown_device_id = config.id;
    let subscriptions = SubscriptionState::default()
        .subscribed(vec![TopicSubscription::keyed(
            "display_preview",
            unknown_device_id.to_string(),
        )])
        .expect("display preview subscribe applies");
    let (_subscriptions_tx, subscriptions_rx) = watch::channel(subscriptions);
    let (preview_tx, preview_rx) = preview_outbound_channel();

    let relay_handle = tokio::spawn(relay_display_preview(
        Arc::clone(&state),
        Arc::clone(&display_frames),
        preview_tx,
        subscriptions_rx,
    ));

    tokio::time::sleep(Duration::from_millis(50)).await;
    let metrics = display_frames.read().await.metrics_snapshot();
    assert_eq!(metrics.preview_subscribers, 0);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), preview_rx.recv())
            .await
            .is_err()
    );

    let device_id = state.device_registry.add(config.device_info()).await;
    let tracked = state
        .device_registry
        .get(&device_id)
        .await
        .expect("connected display is registered");
    let led_count = tracked.info.total_led_count();
    state.event_bus.publish(HypercolorEvent::DeviceConnected {
        device_id: device_id.to_string(),
        name: tracked.info.name.clone(),
        origin: tracked.info.origin.clone(),
        led_count,
        zones: Vec::new(),
    });
    wait_for_display_preview_subscribers(&display_frames, 1).await;
    publish_display_preview_snapshot(&display_frames, device_id, 1).await;
    let frame = receive_direct_preview(&preview_rx).await;
    assert_eq!(
        decoded_display_preview(&frame).device_id,
        device_id.to_string()
    );

    relay_handle.abort();
    let _ = relay_handle.await;
}

#[test]
fn parse_subscriptions_accepts_supported_topics() {
    let entries: Vec<TopicSubscription> = [
        "events",
        "frames",
        "spectrum",
        "canvas",
        "screen_canvas",
        "frame_events",
        "metrics",
        "device_metrics",
    ]
    .into_iter()
    .map(TopicSubscription::unkeyed)
    .collect();
    let parsed = parse_subscriptions(&entries).expect("unkeyed topics should parse");
    let topics: Vec<TopicId> = parsed
        .iter()
        .map(|request| request.selection.topic)
        .collect();
    assert_eq!(
        topics,
        vec![
            TopicId::Events,
            TopicId::Frames,
            TopicId::Spectrum,
            TopicId::Canvas,
            TopicId::ScreenCanvas,
            TopicId::FrameEvents,
            TopicId::Metrics,
            TopicId::DeviceMetrics,
        ]
    );
    assert!(
        parsed.iter().all(|request| request.selection.key.is_none()),
        "none of these topics takes a key"
    );
}

#[test]
fn parse_subscriptions_rejects_an_unknown_topic() {
    let error = parse_subscriptions(&[TopicSubscription::unkeyed("unknown")])
        .expect_err("unknown topic should fail");
    assert_eq!(error.code, "malformed_request");
}

#[test]
fn parse_subscriptions_carries_the_key_through_to_the_selection() {
    let parsed = parse_subscriptions(&[
        TopicSubscription::keyed("display_preview", "  device-abc  "),
        TopicSubscription::keyed("interactive_preview", "stage"),
    ])
    .expect("keyed topics should parse");

    assert_eq!(parsed[0].selection.key.as_deref(), Some("device-abc"));
    assert_eq!(parsed[1].selection.key.as_deref(), Some("stage"));
}

#[test]
fn parse_subscriptions_refuses_a_keyless_keyed_topic() {
    let error = parse_subscriptions(&[TopicSubscription::unkeyed("display_preview")])
        .expect_err("display_preview needs a device");
    assert_eq!(error.code, "malformed_request");
    assert!(error.message.contains("display_preview"));
}

#[test]
fn parse_subscriptions_refuses_two_entries_for_one_subscription() {
    // A client that sends the same subscription twice does not agree with
    // itself about which config wins; resolving that silently would hide
    // it from the only party who can fix it.
    let error = parse_subscriptions(&[
        TopicSubscription::unkeyed("metrics").with_config(serde_json::json!({"fps": 5.0})),
        TopicSubscription::unkeyed("metrics").with_config(serde_json::json!({"fps": 4.0})),
    ])
    .expect_err("a repeated subscription is refused");
    assert_eq!(error.code, "malformed_request");

    // Two keys of one topic are two subscriptions, not a duplicate.
    parse_subscriptions(&[
        TopicSubscription::keyed("display_preview", "device-a"),
        TopicSubscription::keyed("display_preview", "device-b"),
    ])
    .expect("distinct keys are distinct subscriptions");
}

#[test]
fn parse_selectors_requires_at_least_one_entry() {
    let error = parse_selectors(&[]).expect_err("an empty unsubscribe is refused");
    assert_eq!(error.code, "malformed_request");
}

#[test]
fn read_only_auth_rejects_private_capture_subscriptions() {
    let channels = selections(&[
        TopicId::Events,
        TopicId::ScreenCanvas,
        TopicId::ScreenZones,
        TopicId::InputEvents,
    ]);
    let error = authorize_subscription_topics(RequestAuthContext::read_only(), &channels)
        .expect_err("read-only clients must not subscribe to capture-demand channels");

    assert_eq!(error.code, "forbidden");
    assert_eq!(
        error.details,
        Some(serde_json::json!({
            "topics": ["screen_canvas", "screen_zones", "input_events"],
            "required_tier": "control"
        }))
    );
}

#[test]
fn unsecured_loopback_auth_rejects_private_capture_subscriptions() {
    let channels = selections(&[
        TopicId::ScreenCanvas,
        TopicId::ScreenZones,
        TopicId::InputEvents,
    ]);

    let error = authorize_subscription_topics(RequestAuthContext::unsecured(), &channels)
        .expect_err("loopback locality must not authorize sensitive subscriptions");

    assert_eq!(error.code, "forbidden");
    assert_eq!(
        error.details,
        Some(serde_json::json!({
            "topics": ["screen_canvas", "screen_zones", "input_events"],
            "required_tier": "control"
        }))
    );
}

#[tokio::test]
async fn rejected_private_subscription_creates_no_input_demand() {
    let state = Arc::new(AppState::new());
    let mut socket = spawn_test_local_socket(
        Arc::clone(&state),
        &tokio::runtime::Handle::current(),
        RequestAuthContext::unsecured(),
    );
    let hello = socket.recv().await.expect("test socket should emit hello");
    assert!(matches!(hello, Message::Text(_)));

    socket
        .send(Message::Text(
            serde_json::json!({
                "type": "subscribe",
                "topics": [
                    {"topic": "screen_canvas"},
                    {"topic": "screen_zones"},
                    {"topic": "input_events"}
                ]
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("test socket should accept subscription request");
    let rejection = socket
        .recv()
        .await
        .expect("test socket should emit subscription rejection");
    let Message::Text(rejection) = rejection else {
        panic!("subscription rejection should be JSON text");
    };
    let rejection: serde_json::Value =
        serde_json::from_str(rejection.as_str()).expect("rejection should be JSON");
    assert_eq!(rejection["type"], "error");
    assert_eq!(rejection["code"], "forbidden");

    assert_eq!(
        state
            .input_publication_demands
            .registration_count(InputPublicationConsumer::PassiveStream),
        0
    );
    assert_eq!(
        state
            .input_publication_demands
            .requested_hz(SourceKind::Screen),
        0
    );
    assert_eq!(
        state
            .input_publication_demands
            .requested_hz(SourceKind::Interaction),
        0
    );

    socket.shutdown().await;
}

#[test]
fn read_only_auth_allows_non_capture_preview_subscriptions() {
    let channels = selections(&[
        TopicId::Events,
        TopicId::Metrics,
        TopicId::Canvas,
        TopicId::WebViewportCanvas,
    ]);

    authorize_subscription_topics(RequestAuthContext::read_only(), &channels)
        .expect("read-only clients may subscribe to non-capture channels");
}

#[test]
fn control_auth_allows_private_capture_subscriptions() {
    let channels = selections(&[
        TopicId::ScreenCanvas,
        TopicId::ScreenZones,
        TopicId::InputEvents,
    ]);

    authorize_subscription_topics(RequestAuthContext::control(), &channels)
        .expect("control clients may subscribe to capture preview channels");
}

#[test]
fn zone_layout_preview_client_messages_deserialize() {
    let zone_id = ZoneId::new().to_string();
    let preview: ClientMessage = serde_json::from_value(serde_json::json!({
        "type": "zone_layout_preview",
        "zone_id": zone_id,
        "layout": {
            "id": "zone-layout",
            "name": "Zone Layout",
            "description": null,
            "canvas_width": 320,
            "canvas_height": 200,
            "zones": [],
            "default_sampling_mode": {"type": "bilinear"},
            "default_edge_behavior": "clamp",
            "spaces": null,
            "version": 1
        }
    }))
    .expect("preview message should deserialize");

    match preview {
        ClientMessage::ZoneLayoutPreview {
            zone_id: parsed_zone_id,
            layout,
        } => {
            assert_eq!(parsed_zone_id, zone_id);
            assert_eq!(layout.id, "zone-layout");
        }
        _ => panic!("expected zone_layout_preview variant"),
    }

    let clear: ClientMessage = serde_json::from_value(serde_json::json!({
        "type": "zone_layout_preview_clear",
        "zone_id": zone_id
    }))
    .expect("clear message should deserialize");

    match clear {
        ClientMessage::ZoneLayoutPreviewClear {
            zone_id: parsed_zone_id,
        } => assert_eq!(parsed_zone_id, zone_id),
        _ => panic!("expected zone_layout_preview_clear variant"),
    }

    for stale in [
        serde_json::json!({
            "type": "zone_layout_preview_clear",
            "scene_id": SceneId::new().to_string(),
            "zone_id": zone_id
        }),
        serde_json::json!({
            "type": "zone_layout_preview",
            "scene_id": SceneId::new().to_string(),
            "zone_id": zone_id,
            "layout": {
                "id": "zone-layout",
                "name": "Zone Layout",
                "canvas_width": 320,
                "canvas_height": 200,
                "zones": [],
                "default_sampling_mode": {"type": "bilinear"},
                "default_edge_behavior": "clamp",
                "version": 1
            }
        }),
    ] {
        serde_json::from_value::<ClientMessage>(stale)
            .expect_err("the deleted scene selector must fail loudly");
    }
}

#[tokio::test]
async fn zone_layout_preview_rejects_invalid_sampling_radii() {
    let state = AppState::new();
    let manager = state.scene_manager.snapshot().await;
    let scene = manager
        .get(&SceneId::DEFAULT)
        .expect("default scene should exist");
    let group = scene.primary_zone().expect("primary zone should exist");

    for radius in [-1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut layout = group.layout.clone();
        layout.default_sampling_mode = SamplingMode::AreaAverage {
            radius_x: radius,
            radius_y: 0.0,
        };

        let error = validated_zone_layout_preview(scene, group.id, layout)
            .expect_err("invalid radii must be rejected before preview state changes");
        assert_eq!(error.code, "malformed_request");
        assert!(error.message.contains("radius_x"));
    }
}

#[test]
fn topic_config_apply_patch_supports_every_configurable_topic() {
    let state = SubscriptionState::default()
        .subscribed_unkeyed(
            &[
                "frames",
                "spectrum",
                "canvas",
                "screen_canvas",
                "screen_zones",
                "metrics",
                "device_metrics",
            ],
            serde_json::json!({
                "frames": {"fps": 30},
                "spectrum": {"fps": 20, "bins": 32},
                "canvas": {"fps": 60, "format": "jpeg", "width": 320, "height": 0},
                "screen_canvas": {"fps": 24, "format": "jpeg", "width": 480, "height": 270},
                "screen_zones": {"fps": 12},
                "metrics": {"fps": 2.0},
                "device_metrics": {"fps": 4.0}
            }),
        )
        .expect("full channel config patch should be accepted");

    let json = state.config_by_topic();
    assert_eq!(
        json["screen_zones"]["fps"], 12,
        "screen_zones paces itself instead of borrowing screen_canvas's cadence"
    );
    assert_eq!(json["canvas"]["fps"], 60);
    assert_eq!(json["canvas"]["format"], "jpeg");
    assert_eq!(json["canvas"]["width"], 320);
    assert_eq!(json["canvas"]["height"], 0);
    assert_eq!(json["screen_canvas"]["fps"], 24);
    assert_eq!(json["screen_canvas"]["format"], "jpeg");
    assert_eq!(json["screen_canvas"]["width"], 480);
    assert_eq!(json["screen_canvas"]["height"], 270);
    assert_eq!(json["metrics"]["fps"], 2.0);
    assert_eq!(json["device_metrics"]["fps"], 4.0);
}

#[test]
fn topic_config_admits_wide_shapes_and_preserves_auto_dimensions() {
    let state = SubscriptionState::default()
        .subscribed_unkeyed(
            &["canvas", "screen_canvas"],
            serde_json::json!({
                "canvas": {"width": 100_000, "height": 1_000},
                "screen_canvas": {"width": u32::MAX, "height": 0}
            }),
        )
        .expect("wide shapes are admitted");

    let json = state.config_by_topic();
    assert_eq!(json["canvas"]["width"], 100_000);
    assert_eq!(json["canvas"]["height"], 1_000);
    assert_eq!(json["screen_canvas"]["width"], u32::MAX);
    assert_eq!(json["screen_canvas"]["height"], 0);
}

#[test]
fn topic_config_rejects_over_budget_shape_transactionally() {
    let live = SubscriptionState::default()
        .subscribed_unkeyed(&["canvas", "zone_preview"], serde_json::Value::Null)
        .expect("bare subscribe applies");

    let error = live
        .subscribed_unkeyed(
            &["canvas", "zone_preview"],
            serde_json::json!({
                "canvas": {"fps": 60},
                "zone_preview": {"width": 32_768, "height": 4_097}
            }),
        )
        .expect_err("over-budget shape is rejected");

    assert_eq!(error.code, "validation_error");
    // The valid stanza in the same request did not land either.
    let json = live.config_by_topic();
    assert_eq!(json["canvas"]["fps"], 15);
    assert_eq!(json["zone_preview"]["width"], 0);
    assert_eq!(json["zone_preview"]["height"], 0);
}

#[test]
fn topic_config_defaults_are_stable() {
    let json = SubscriptionState::default()
        .subscribed_unkeyed(
            &[
                "frames",
                "spectrum",
                "canvas",
                "screen_canvas",
                "metrics",
                "device_metrics",
            ],
            serde_json::Value::Null,
        )
        .expect("bare subscribe applies")
        .config_by_topic();

    assert_eq!(json["frames"]["fps"], 30);
    assert!(
        json["frames"].get("format").is_none(),
        "the frames topic has one encoding; the JSON toggle had no consumers"
    );
    assert_eq!(json["spectrum"]["bins"], 64);
    assert_eq!(json["canvas"]["fps"], 15);
    assert_eq!(json["canvas"]["width"], 0);
    assert_eq!(json["canvas"]["height"], 0);
    assert_eq!(json["screen_canvas"]["fps"], 15);
    assert_eq!(json["screen_canvas"]["width"], 0);
    assert_eq!(json["screen_canvas"]["height"], 0);
    assert_eq!(json["metrics"]["fps"], 1.0);
    assert_eq!(json["device_metrics"]["fps"], 1.0);
}

#[test]
fn non_null_config_for_a_configless_topic_is_refused() {
    let error = SubscriptionState::default()
        .subscribed_unkeyed(&["sensors"], serde_json::json!({"sensors": {"fps": 10}}))
        .expect_err("sensors takes no config");

    assert_eq!(error.code, "validation_error");
    assert_eq!(
        error.details,
        Some(serde_json::json!({
            "field": "config.sensors",
            "reason": "topic accepts no config"
        }))
    );
}

#[test]
fn a_subscribe_carries_its_config_inside_each_selector() {
    let raw = r#"{"type":"subscribe","topics":[{"topic":"metrics","config":{"fps":0.5}},{"topic":"display_preview","key":"device-abc","config":{"fps":9}}]}"#;
    let message: ClientMessage = serde_json::from_str(raw).expect("a keyed subscribe parses");
    let ClientMessage::Subscribe { topics, .. } = message else {
        panic!("expected a subscribe");
    };

    assert_eq!(topics.len(), 2);
    assert_eq!(topics[0].topic, "metrics");
    assert_eq!(topics[0].key, None);
    assert_eq!(
        topics[0].config.as_ref().expect("metrics config")["fps"],
        0.5
    );
    assert_eq!(topics[1].topic, "display_preview");
    assert_eq!(topics[1].key.as_deref(), Some("device-abc"));
    assert_eq!(topics[1].config.as_ref().expect("display config")["fps"], 9);
}

#[test]
fn a_subscribe_entry_refuses_fields_it_does_not_define() {
    // The entry owns exactly three fields; anything else is a client
    // mistake the wire must not silently drop.
    let raw = r#"{"type":"subscribe","topics":[{"topic":"metrics","cfg":{"fps":0.5}}]}"#;
    serde_json::from_str::<ClientMessage>(raw)
        .expect_err("an unknown selector field must fail loudly");
}

#[test]
fn an_absent_or_null_selector_config_is_no_config_at_all() {
    for raw in [
        r#"{"type":"subscribe","topics":[{"topic":"metrics"}]}"#,
        r#"{"type":"subscribe","topics":[{"topic":"metrics","config":null}]}"#,
    ] {
        let message: ClientMessage =
            serde_json::from_str(raw).expect("subscribe without config parses");
        let ClientMessage::Subscribe { topics, .. } = message else {
            panic!("expected a subscribe");
        };
        assert!(
            topics[0]
                .config
                .as_ref()
                .is_none_or(serde_json::Value::is_null),
            "{raw}"
        );
    }
}

#[test]
fn a_null_stanza_leaves_a_configurable_topic_alone() {
    let state = SubscriptionState::default()
        .subscribed_unkeyed(&["metrics"], serde_json::json!({"metrics": {"fps": 4.0}}))
        .expect("metrics subscribe applies")
        .subscribed_unkeyed(&["metrics"], serde_json::json!({"metrics": null}))
        .expect("a null stanza is not a patch");

    assert_eq!(state.config_by_topic()["metrics"]["fps"], 4.0);
}

#[test]
fn config_for_an_unrecognized_channel_is_ignored() {
    let state = SubscriptionState::default()
        .subscribed_unkeyed(&["metrics"], serde_json::json!({"lasers": {"fps": 1}}))
        .expect("a stanza for no known topic is not a subscribe failure");

    let config = state.config_by_topic();
    assert!(config.get("lasers").is_none());
    assert_eq!(config["metrics"]["fps"], 1.0);
}

#[test]
fn unsubscribing_keeps_the_config_a_resubscribe_reinstates() {
    let configured = SubscriptionState::default()
        .subscribed_unkeyed(&["metrics"], serde_json::json!({"metrics": {"fps": 4.0}}))
        .expect("metrics subscribe applies");
    assert!(configured.live_table_agrees_with_membership());
    assert!(!configured.has_dormant_config(TopicId::Metrics, None));

    // Unsubscribing parks the config rather than dropping it, and the
    // live table stops claiming a topic nobody is subscribed to.
    let dropped = configured.unsubscribed_unkeyed(&["metrics"]);
    assert!(!dropped.contains(TopicId::Metrics));
    assert!(dropped.config_by_topic().get("metrics").is_none());
    assert!(dropped.live_table_agrees_with_membership());
    assert!(dropped.has_dormant_config(TopicId::Metrics, None));

    let restored = dropped
        .subscribed_unkeyed(&["metrics"], serde_json::Value::Null)
        .expect("resubscribe applies");
    assert_eq!(
        restored.config_by_topic()["metrics"]["fps"],
        4.0,
        "a resubscribe reinstates the client's own cadence, not the default"
    );
    assert!(restored.live_table_agrees_with_membership());
    assert!(
        !restored.has_dormant_config(TopicId::Metrics, None),
        "a reinstated config moves back rather than being copied"
    );
}

#[test]
fn the_live_table_never_claims_an_unsubscribed_topic() {
    // Every shape that writes config: subscribe, patch-without-subscribe,
    // unsubscribe, resubscribe. The live table has to agree with
    // membership after each one, because that is what any_for promises.
    let mut state = SubscriptionState::default();
    assert!(state.live_table_agrees_with_membership());

    state = state
        .subscribed(vec![
            TopicSubscription::unkeyed("frames").with_config(serde_json::json!({"fps": 12})),
            TopicSubscription::unkeyed("canvas"),
            TopicSubscription::keyed("display_preview", "device-abc")
                .with_config(serde_json::json!({"fps": 9})),
        ])
        .expect("a keyed and unkeyed subscribe applies");
    assert!(state.live_table_agrees_with_membership());

    state = state.unsubscribed_unkeyed(&["frames"]);
    assert!(state.live_table_agrees_with_membership());
    assert!(state.has_dormant_config(TopicId::Frames, None));

    state = state.unsubscribed(vec![TopicSelector::keyed("display_preview", "device-abc")]);
    assert!(state.live_table_agrees_with_membership());
    assert!(!state.contains(TopicId::DisplayPreview));
    assert!(state.has_dormant_config(TopicId::DisplayPreview, Some("device-abc")));

    state = state
        .subscribed(vec![
            TopicSubscription::unkeyed("frames"),
            TopicSubscription::keyed("display_preview", "device-abc"),
        ])
        .expect("resubscribe applies");
    assert!(state.live_table_agrees_with_membership());
    assert_eq!(state.config_by_topic()["frames"]["fps"], 12);
    assert_eq!(
        state.config_by_topic()["display_preview"]["device-abc"]["fps"],
        9
    );
}

#[test]
fn a_patch_can_only_target_a_subscription_the_same_request_names() {
    // Config rides inside its selector, so there is no way to spell
    // "configure frames without subscribing to it" any more.
    let state = SubscriptionState::default()
        .subscribed_unkeyed(&["events"], serde_json::json!({"frames": {"fps": 12}}))
        .expect("a config map entry for an unnamed topic is simply not read");

    assert!(state.config_by_topic().get("frames").is_none());
    assert!(!state.has_dormant_config(TopicId::Frames, None));
    assert!(state.live_table_agrees_with_membership());
    assert_eq!(
        state
            .subscribed_unkeyed(&["frames"], serde_json::Value::Null)
            .expect("frames subscribe applies")
            .config_by_topic()["frames"]["fps"],
        30,
        "frames keeps its default because nothing ever configured it"
    );
}

#[test]
fn one_topic_holds_a_subscription_per_key() {
    let state = SubscriptionState::default()
        .subscribed(vec![
            TopicSubscription::keyed("display_preview", "device-a")
                .with_config(serde_json::json!({"fps": 5})),
            TopicSubscription::keyed("display_preview", "device-b")
                .with_config(serde_json::json!({"fps": 25})),
        ])
        .expect("two devices are two subscriptions");

    assert!(state.holds(TopicId::DisplayPreview, Some("device-a")));
    assert!(state.holds(TopicId::DisplayPreview, Some("device-b")));
    assert!(!state.holds(TopicId::DisplayPreview, Some("device-c")));

    let configs = state.keyed_configs::<hypercolor_leptos_ext::ws::registry::DisplayPreviewConfig>(
        TopicId::DisplayPreview,
    );
    assert_eq!(configs.len(), 2);
    assert_eq!(configs[0].0, "device-a");
    assert_eq!(configs[0].1.fps, 5);
    assert_eq!(configs[1].0, "device-b");
    assert_eq!(configs[1].1.fps, 25);

    // Retiring one key leaves the other live, and the topic with it.
    let remaining = state.unsubscribed(vec![TopicSelector::keyed("display_preview", "device-a")]);
    assert!(!remaining.holds(TopicId::DisplayPreview, Some("device-a")));
    assert!(remaining.holds(TopicId::DisplayPreview, Some("device-b")));
    assert!(remaining.contains(TopicId::DisplayPreview));
    assert!(remaining.live_table_agrees_with_membership());
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
fn event_message_parts_serializes_zone_changed() {
    let zone_id = ZoneId::new();
    let event = HypercolorEvent::ZoneChanged {
        scene_id: SceneId::DEFAULT,
        zone_id,
        role: ZoneRole::Display,
        kind: hypercolor_types::event::ZoneChangeKind::ControlsPatched,
    };

    let (event_name, event_data) = event_message_parts(&event);
    assert_eq!(event_name, "zone_changed");
    assert_eq!(event_data["scene_id"], SceneId::DEFAULT.to_string());
    assert_eq!(event_data["zone_id"], zone_id.to_string());
    assert_eq!(event_data["role"], "display");
    assert_eq!(event_data["kind"], "controls_patched");
}

#[test]
fn event_message_parts_serializes_effect_degraded() {
    let zone_id = ZoneId::new();
    let event = HypercolorEvent::EffectDegraded {
        effect_id: "effect-1".to_owned(),
        zone_id: Some(zone_id),
        zone_name: Some("Display Face".to_owned()),
        state: hypercolor_types::event::EffectDegradationState::Failed,
        reason: Some("boom".to_owned()),
    };

    let (event_name, event_data) = event_message_parts(&event);
    assert_eq!(event_name, "effect_degraded");
    assert_eq!(event_data["effect_id"], "effect-1");
    assert_eq!(event_data["zone_id"], zone_id.to_string());
    assert_eq!(event_data["zone_name"], "Display Face");
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
fn event_message_parts_exposes_input_status_as_a_dedicated_safe_event() {
    let event = HypercolorEvent::ExtensionStateChanged {
        source: hypercolor_core::bus::INPUT_STATUS_EVENT_SOURCE.to_owned(),
        kind: hypercolor_core::bus::INPUT_STATUS_EVENT_KIND.to_owned(),
        payload: serde_json::json!({
            "source_id": "host-interaction",
            "active_consumer_count": 3,
            "state": "failed",
            "session_generation": 9,
        }),
    };

    let (event_name, event_data) = event_message_parts(&event);
    assert_eq!(event_name, "input_source_status_changed");
    assert_eq!(event_data["source_id"], "host-interaction");
    assert_eq!(event_data["active_consumer_count"], 3);
    assert_eq!(event_data["state"], "failed");
    assert_eq!(event_data["session_generation"], 9);
}

#[test]
fn frame_rendered_events_require_frame_events_even_with_metrics() {
    let channels = topic_set(&[TopicId::Events, TopicId::Metrics]);
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
fn frame_rendered_events_require_frame_events_even_with_device_metrics() {
    let channels = topic_set(&[TopicId::Events, TopicId::DeviceMetrics]);
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
fn frame_rendered_events_are_suppressed_for_event_only_clients() {
    let channels = topic_set(&[TopicId::Events]);
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
fn frame_rendered_events_pass_through_for_frame_event_clients() {
    let channels = topic_set(&[TopicId::FrameEvents]);
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

fn sample_input_event() -> HypercolorEvent {
    HypercolorEvent::InputEventReceived {
        event: TimedInputEvent {
            event: hypercolor_types::event::InputEvent::Key {
                source_id: "host:/dev/input/event3".into(),
                key: "a".into(),
                state: hypercolor_types::event::InputButtonState::Repeated,
            },
            at_ms: 700,
            seq: 41,
            physical_code: Some("evdev:key:30".into()),
            repeat_count: 3,
        },
    }
}

#[test]
fn input_event_websocket_payload_conforms_to_shared_timed_schema() {
    let (name, data) = event_message_parts(&sample_input_event());
    let decoded = TimedInputEventPayload::decode(&data).expect("decode shared input payload");

    assert_manifested_json_payload("timed_input_event_v1", &data);

    assert_eq!(name, "input_event_received");
    assert_eq!(decoded.at_ms, 700);
    assert_eq!(decoded.seq, 41);
    assert_eq!(decoded.physical_code.as_deref(), Some("evdev:key:30"));
    assert_eq!(decoded.repeat_count, 3);
    assert_eq!(decoded.event["source_id"], "host:/dev/input/event3");
    assert_eq!(decoded.event["key"], "a");
    assert_eq!(decoded.event["state"], "repeated");
}

#[test]
fn macos_ownership_payload_manifest_matches_the_event_serializer() {
    let event = HypercolorEvent::MacosDaemonOwnershipChanged {
        active_owner: MacosDaemonOwnerEvent::AppSidecar,
        owner_epoch: 0x0807_0605_0403_0201,
        conflict: Some(MacosDaemonOwnerConflictEvent {
            active: MacosDaemonOwnerEvent::AppSidecar,
            contender: MacosDaemonOwnerEvent::Standalone,
            observed_at_ms: 0x1817_1615_1413_1211,
        }),
        recovery_required: Some(MacosDaemonOwnerRecoveryRequiredEvent {
            requested_owner: MacosDaemonOwnerEvent::LaunchdService,
            prior_owner: MacosDaemonOwnerEvent::AppSidecar,
            phase: MacosDaemonHandoverPhaseEvent::RollbackPending,
        }),
    };
    let (name, data) = event_message_parts(&event);

    assert_eq!(name, "macos_daemon_ownership_changed");
    assert_manifested_json_payload("macos_daemon_ownership_changed_v1", &data);
}

#[tokio::test]
async fn input_event_relay_preserves_equal_timestamps_and_sequence_gaps() {
    let bus = HypercolorBus::new();
    let event_rx = bus.subscribe_all();
    let subscriptions = SubscriptionState::default()
        .subscribed_unkeyed(&["input_events"], serde_json::Value::Null)
        .expect("input events subscribe applies");
    let (_subscriptions_tx, subscriptions_rx) = watch::channel(subscriptions);
    let (json_tx, mut json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(4);
    let relay_handle = tokio::spawn(relay_events(event_rx, json_tx, subscriptions_rx));

    let first = sample_input_event();
    let mut second = sample_input_event();
    let HypercolorEvent::InputEventReceived { event } = &mut second else {
        panic!("sample should be an input event");
    };
    event.seq = 43;
    event.repeat_count = 2;
    bus.publish(first);
    bus.publish(second);

    for (expected_seq, expected_repeat_count) in [(41, 3), (43, 2)] {
        let json = tokio::time::timeout(Duration::from_secs(1), json_rx.recv())
            .await
            .expect("input relay should respond")
            .expect("input relay should remain open");
        let wire: serde_json::Value = serde_json::from_str(json.as_str()).expect("relay JSON");
        assert_eq!(wire["event"], "input_event_received");
        let decoded = TimedInputEventPayload::decode(&wire["data"])
            .expect("relay should use shared timed schema");
        assert_eq!(decoded.at_ms, 700);
        assert_eq!(decoded.seq, expected_seq);
        assert_eq!(decoded.repeat_count, expected_repeat_count);
    }

    relay_handle.abort();
    let _ = relay_handle.await;
}

#[tokio::test]
async fn every_lagged_event_topic_emits_a_resync_hint() {
    for topic in ["events", "frame_events", "input_events"] {
        let bus = HypercolorBus::new();
        let event_rx = bus.subscribe_all();
        for _ in 0..300 {
            bus.publish(HypercolorEvent::Paused);
        }
        let mut subscriptions = SubscriptionState::default();
        if topic != "events" {
            subscriptions = subscriptions
                .unsubscribed_unkeyed(&["events"])
                .subscribed_unkeyed(&[topic], serde_json::Value::Null)
                .expect("event topic subscribe applies");
        }
        let (_subscriptions_tx, subscriptions_rx) = watch::channel(subscriptions);
        let (json_tx, mut json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(1);
        let relay_handle = tokio::spawn(relay_events(event_rx, json_tx, subscriptions_rx));

        let json = tokio::time::timeout(Duration::from_secs(1), json_rx.recv())
            .await
            .expect("lagged event relay should respond")
            .expect("lagged event relay should remain open");
        let wire: serde_json::Value = serde_json::from_str(json.as_str()).expect("relay JSON");
        assert_eq!(wire["event"], "resync_required", "topic {topic}");
        assert!(
            wire["data"]["dropped_events"]
                .as_u64()
                .is_some_and(|count| count > 0),
            "topic {topic}"
        );

        relay_handle.abort();
        let _ = relay_handle.await;
    }
}

#[tokio::test]
async fn worker_failure_relay_invalidates_input_status_immediately() {
    let (state, session_slot) = status_event_state();
    let event_rx = state.event_bus.subscribe_all();
    let _publisher =
        InputStatusEventPublisher::start(state.input_status.clone(), Arc::clone(&state.event_bus));
    let (_subscriptions_tx, subscriptions_rx) = watch::channel(SubscriptionState::default());
    let (json_tx, mut json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(8);
    let relay_handle = tokio::spawn(relay_events(event_rx, json_tx, subscriptions_rx));

    let initial = tokio::time::timeout(Duration::from_secs(1), json_rx.recv())
        .await
        .expect("initial source status should relay")
        .expect("relay should remain open");
    let initial: serde_json::Value =
        serde_json::from_str(initial.as_str()).expect("initial relay JSON");
    assert_eq!(initial["event"], "input_source_status_changed");
    assert_manifested_json_payload("input_source_status_changed_v1", &initial["data"]);
    assert_eq!(initial["data"]["source_id"], "status-event-test");

    let worker = session_slot
        .load()
        .expect("test worker should retain the active source session");
    assert!(worker.failed(SourceIssue::new(
        "worker_exited",
        "worker exited unexpectedly",
        true,
    )));

    let failure = tokio::time::timeout(Duration::from_secs(1), json_rx.recv())
        .await
        .expect("worker failure should invalidate without polling")
        .expect("relay should remain open");
    let failure: serde_json::Value =
        serde_json::from_str(failure.as_str()).expect("failure relay JSON");
    assert_eq!(failure["event"], "input_source_status_changed");
    assert_eq!(failure["data"]["state"], "failed");
    assert_eq!(failure["data"]["lifecycle_issue_code"], "worker_exited");

    relay_handle.abort();
    let _ = relay_handle.await;
}

#[tokio::test]
async fn input_status_publisher_runs_without_websocket_clients() {
    let (state, session_slot) = status_event_state();
    let mut event_rx = state.event_bus.subscribe_all();
    let _publisher =
        InputStatusEventPublisher::start(state.input_status.clone(), Arc::clone(&state.event_bus));

    let initial = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
        .await
        .expect("publisher should emit the initial status without a websocket")
        .expect("event bus should remain open");
    let (_, initial) = event_message_parts(&initial.event);
    assert_eq!(initial["source_id"], "status-event-test");

    let worker = session_slot
        .load()
        .expect("test worker should retain the active source session");
    assert!(worker.failed(SourceIssue::new(
        "zero_client_failure",
        "worker exited without a websocket client",
        true,
    )));

    let failure = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
        .await
        .expect("publisher should emit the failure without a websocket")
        .expect("event bus should remain open");
    let (_, failure) = event_message_parts(&failure.event);
    assert_eq!(failure["state"], "failed");
    assert_eq!(failure["lifecycle_issue_code"], "zero_client_failure");
}

#[tokio::test]
async fn one_input_status_publisher_fans_out_once_to_multiple_websocket_relays() {
    let (state, session_slot) = status_event_state();
    let first_event_rx = state.event_bus.subscribe_all();
    let second_event_rx = state.event_bus.subscribe_all();
    let (_, first_subscriptions) = watch::channel(SubscriptionState::default());
    let (_, second_subscriptions) = watch::channel(SubscriptionState::default());
    let (first_tx, mut first_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(8);
    let (second_tx, mut second_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(8);
    let first_relay = tokio::spawn(relay_events(first_event_rx, first_tx, first_subscriptions));
    let second_relay = tokio::spawn(relay_events(
        second_event_rx,
        second_tx,
        second_subscriptions,
    ));
    let _publisher =
        InputStatusEventPublisher::start(state.input_status.clone(), Arc::clone(&state.event_bus));

    for receiver in [&mut first_rx, &mut second_rx] {
        let initial = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("each websocket relay should receive the initial status")
            .expect("websocket relay should remain open");
        let initial: serde_json::Value =
            serde_json::from_str(initial.as_str()).expect("initial relay JSON");
        assert_eq!(initial["event"], "input_source_status_changed");
    }

    let worker = session_slot
        .load()
        .expect("test worker should retain the active source session");
    assert!(worker.failed(SourceIssue::new(
        "multi_client_failure",
        "worker exited with multiple websocket clients",
        true,
    )));

    for receiver in [&mut first_rx, &mut second_rx] {
        let failure = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("each websocket relay should receive the failure")
            .expect("websocket relay should remain open");
        let failure: serde_json::Value =
            serde_json::from_str(failure.as_str()).expect("failure relay JSON");
        assert_eq!(
            failure["data"]["lifecycle_issue_code"],
            "multi_client_failure"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), receiver.recv())
                .await
                .is_err(),
            "one publisher must not duplicate a transition per client"
        );
    }

    first_relay.abort();
    second_relay.abort();
}

#[tokio::test]
async fn input_status_publisher_rebuilds_watchers_after_graph_change() {
    let (state, _) = status_event_state();
    let mut event_rx = state.event_bus.subscribe_all();
    let _publisher =
        InputStatusEventPublisher::start(state.input_status.clone(), Arc::clone(&state.event_bus));
    tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
        .await
        .expect("initial status should publish")
        .expect("event bus should remain open");

    {
        let mut manager = state.input_manager.lock().await;
        manager.add_source(Box::new(StatusEventTestSource::with_id(
            "status-event-added",
            SourceSessionSlot::new(),
        )));
        manager
            .start_all()
            .expect("new status event source should start");
    }

    let added = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let event = event_rx.recv().await.expect("event bus should remain open");
            let (event_name, payload) = event_message_parts(&event.event);
            if event_name == "input_source_status_changed"
                && payload["source_id"] == "status-event-added"
            {
                break payload;
            }
        }
    })
    .await
    .expect("graph reconciliation should attach the added source watcher");
    assert_eq!(added["state"], "starting");
}

#[test]
fn input_events_never_relay_on_the_default_events_channel() {
    let channels = topic_set(&[TopicId::Events]);
    assert!(!should_relay_event(&sample_input_event(), channels));
}

#[test]
fn input_events_relay_only_on_the_input_events_channel() {
    let channels = topic_set(&[TopicId::InputEvents]);
    assert!(should_relay_event(&sample_input_event(), channels));
}

#[test]
fn input_events_channel_requires_control_subscription() {
    assert!(TopicId::InputEvents.requires_control());
    assert_eq!(TopicId::parse("input_events"), Some(TopicId::InputEvents));
    assert_eq!(TopicId::InputEvents.as_str(), "input_events");
}

#[test]
fn default_subscription_excludes_input_events() {
    let initial = SubscriptionState::default();
    assert!(initial.contains(TopicId::Events));
    assert!(!initial.contains(TopicId::InputEvents));
}

#[test]
fn input_inject_message_parses_all_edge_kinds() {
    use hypercolor_core::input::BrowserInputEdge;
    use hypercolor_types::event::{InputButtonState, PointerScrollPhase, PointerScrollUnit};

    let raw = r#"{
        "type": "input_inject",
        "preview_id": "main",
        "events": [
            {"kind": "key", "key": "a", "state": "pressed"},
            {"kind": "button", "button": "left", "state": "released"},
            {"kind": "move", "nx": 0.5, "ny": 0.25},
            {"kind": "wheel", "delta_hi_res": -240},
            {
                "kind": "scroll",
                "delta_x_q16_16": 98304,
                "delta_y_q16_16": -131072,
                "unit": "pixels",
                "phase": "changed",
                "momentum_phase": "began"
            },
            {
                "kind": "scroll",
                "delta_x_q16_16": 0,
                "delta_y_q16_16": 65536,
                "unit": "line120"
            }
        ]
    }"#;

    let ClientMessage::InputInject { preview_id, events } =
        serde_json::from_str::<ClientMessage>(raw).expect("input_inject parses")
    else {
        panic!("expected InputInject");
    };
    assert_eq!(preview_id, "main");
    assert_eq!(events.len(), 6);

    let edges: Vec<BrowserInputEdge> = events
        .into_iter()
        .map(BrowserInputEdgeWire::into_edge)
        .collect();
    assert_eq!(
        edges[0],
        BrowserInputEdge::Key {
            key: "a".into(),
            state: InputButtonState::Pressed,
        }
    );
    assert_eq!(
        edges[1],
        BrowserInputEdge::Button {
            button: "left".into(),
            state: InputButtonState::Released,
        }
    );
    assert_eq!(
        edges[2],
        BrowserInputEdge::Move {
            norm_x: 0.5,
            norm_y: 0.25,
        }
    );
    assert_eq!(edges[3], BrowserInputEdge::Wheel { delta_hi_res: -240 });
    assert_eq!(
        edges[4],
        BrowserInputEdge::Scroll {
            delta_x_q16_16: 98_304,
            delta_y_q16_16: -131_072,
            unit: PointerScrollUnit::Pixels,
            phase: PointerScrollPhase::Changed,
            momentum_phase: PointerScrollPhase::Began,
        }
    );
    assert_eq!(
        edges[5],
        BrowserInputEdge::Scroll {
            delta_x_q16_16: 0,
            delta_y_q16_16: 65_536,
            unit: PointerScrollUnit::Line120,
            phase: PointerScrollPhase::None,
            momentum_phase: PointerScrollPhase::None,
        }
    );
}

#[test]
fn input_inject_rejects_batches_before_the_bounded_vector_can_grow() {
    let events = std::iter::repeat_n(
        r#"{"kind":"wheel","delta_hi_res":1}"#,
        MAX_INPUT_INJECT_EVENTS + 1,
    )
    .collect::<Vec<_>>()
    .join(",");
    let raw = format!(r#"{{"type":"input_inject","preview_id":"main","events":[{events}]}}"#);

    let error = serde_json::from_str::<ClientMessage>(&raw)
        .expect_err("oversized input batch must be rejected");

    assert!(error.to_string().contains("at most"));

    let exact_events = std::iter::repeat_n(
        r#"{"kind":"wheel","delta_hi_res":1}"#,
        MAX_INPUT_INJECT_EVENTS,
    )
    .collect::<Vec<_>>()
    .join(",");
    let exact =
        format!(r#"{{"type":"input_inject","preview_id":"main","events":[{exact_events}]}}"#);
    let ClientMessage::InputInject { events, .. } =
        serde_json::from_str::<ClientMessage>(&exact).expect("maximum input batch must parse")
    else {
        panic!("expected InputInject");
    };
    assert_eq!(events.len(), MAX_INPUT_INJECT_EVENTS);
}

#[test]
fn input_inject_rejects_invalid_names_buttons_coordinates_and_wheel_deltas() {
    use serde::de::value::{Error as ValueError, F32Deserializer};

    let long_name = "a".repeat(MAX_INPUT_NAME_BYTES + 1);
    let oversized = serde_json::json!({
        "type": "input_inject",
        "preview_id": "main",
        "events": [{"kind": "key", "key": long_name, "state": "pressed"}]
    });
    assert!(
        serde_json::from_value::<ClientMessage>(oversized).is_err(),
        "oversized key name must be rejected"
    );

    for key in ["", "line\nbreak", "escape\u{1b}"] {
        let payload = serde_json::json!({
            "type": "input_inject",
            "preview_id": "main",
            "events": [{"kind": "key", "key": key, "state": "pressed"}]
        });
        assert!(
            serde_json::from_value::<ClientMessage>(payload).is_err(),
            "empty and control-bearing key names must be rejected"
        );
    }

    let invalid_button = serde_json::json!({
        "type": "input_inject",
        "preview_id": "main",
        "events": [{"kind": "button", "button": "side", "state": "pressed"}]
    });
    assert!(
        serde_json::from_value::<ClientMessage>(invalid_button).is_err(),
        "unknown pointer buttons must be rejected"
    );

    for coordinate in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(
            deserialize_finite_coordinate(F32Deserializer::<ValueError>::new(coordinate)).is_err(),
            "non-finite coordinate must be rejected"
        );
    }

    for coordinate in ["NaN", "Infinity", "1e999", "-1e999"] {
        let raw = format!(
            r#"{{"type":"input_inject","preview_id":"main","events":[{{"kind":"move","nx":{coordinate},"ny":0.5}}]}}"#
        );
        assert!(
            serde_json::from_str::<ClientMessage>(&raw).is_err(),
            "adversarial coordinate {coordinate} must be rejected"
        );
    }

    for coordinate in [-0.01, 1.01] {
        let payload = serde_json::json!({
            "type": "input_inject",
            "preview_id": "main",
            "events": [{"kind": "move", "nx": coordinate, "ny": 0.5}]
        });
        assert!(
            serde_json::from_value::<ClientMessage>(payload).is_err(),
            "out-of-range normalized coordinate must be rejected"
        );
    }

    for delta in [
        MAX_INPUT_WHEEL_DELTA.saturating_add(1),
        MAX_INPUT_WHEEL_DELTA.saturating_neg().saturating_sub(1),
    ] {
        let payload = serde_json::json!({
            "type": "input_inject",
            "preview_id": "main",
            "events": [{"kind": "wheel", "delta_hi_res": delta}]
        });
        assert!(
            serde_json::from_value::<ClientMessage>(payload).is_err(),
            "amplified wheel delta must be rejected"
        );
    }

    for delta in [
        MAX_INPUT_SCROLL_Q16_16.saturating_add(1),
        MAX_INPUT_SCROLL_Q16_16.saturating_neg().saturating_sub(1),
        i64::MIN,
    ] {
        for axis in ["delta_x_q16_16", "delta_y_q16_16"] {
            let mut edge = serde_json::json!({
                "kind": "scroll",
                "delta_x_q16_16": 0,
                "delta_y_q16_16": 0,
                "unit": "line120"
            });
            edge[axis] = serde_json::json!(delta);
            let payload = serde_json::json!({
                "type": "input_inject",
                "preview_id": "main",
                "events": [edge]
            });
            assert!(
                serde_json::from_value::<ClientMessage>(payload).is_err(),
                "amplified {axis} scroll delta must be rejected"
            );
        }
    }

    for delta in [MAX_INPUT_SCROLL_Q16_16, -MAX_INPUT_SCROLL_Q16_16] {
        let payload = serde_json::json!({
            "type": "input_inject",
            "preview_id": "main",
            "events": [{
                "kind": "scroll",
                "delta_x_q16_16": delta,
                "delta_y_q16_16": delta,
                "unit": "pixels"
            }]
        });
        assert!(
            serde_json::from_value::<ClientMessage>(payload).is_ok(),
            "inclusive scroll bound must be accepted"
        );
    }

    let missing_unit = serde_json::json!({
        "type": "input_inject",
        "preview_id": "main",
        "events": [{
            "kind": "scroll",
            "delta_x_q16_16": 0,
            "delta_y_q16_16": 0
        }]
    });
    assert!(
        serde_json::from_value::<ClientMessage>(missing_unit).is_err(),
        "scroll unit must be required"
    );
}

#[test]
fn interactive_preview_ids_are_bounded_but_otherwise_opaque() {
    for preview_id in ["", "line\nbreak", "escape\u{1b}"] {
        validate_interactive_preview_id(preview_id)
            .expect_err("empty and control-bearing preview ids must be rejected");
    }
    validate_interactive_preview_id(&"a".repeat(129))
        .expect_err("oversized preview id must be rejected");
    for preview_id in ["main canvas", "preview/1", "映像-💜"] {
        validate_interactive_preview_id(preview_id).expect("opaque preview id must be accepted");
    }
}

fn browser_preview_test_context() -> (
    BrowserInputSource,
    BrowserInputHandle,
    InteractionRoutingControl,
) {
    let mut source = BrowserInputSource::new();
    source.start().expect("browser source should start");
    let handle = source.handle();
    let routing = InteractionRoutingControl::new(
        handle.registry(),
        1,
        InteractionRoutePolicy::Host,
        InteractionRoutePolicy::Browser,
    );
    (source, handle, routing)
}

async fn browser_preview_test_executor(
    routing: InteractionRoutingControl,
) -> Arc<InteractivePreviewExecutor> {
    let event_bus = Arc::new(HypercolorBus::new());
    Arc::new(
        InteractivePreviewExecutor::start_cpu(InteractivePreviewContext {
            scene_manager: crate::domain::scene::SceneService::in_memory(
                SceneManager::new(),
                Arc::clone(&event_bus),
            ),
            effect_registry: Arc::new(RwLock::new(EffectRegistry::new(Vec::new()))),
            asset_library: None,
            event_bus,
            input_graph: InputGraphHandle::default(),
            sensor_snapshots: None,
            interaction_routing: routing,
            input_demands: InputPublicationDemandHandle::new(),
            canvas_width: 64,
            canvas_height: 64,
            acceleration: InteractivePreviewAcceleration::cpu(),
            resource_capacity_bytes: 64 * 1024 * 1024,
        })
        .await
        .expect("CPU interactive preview executor should start"),
    )
}

fn browser_preview_session(
    handle: BrowserInputHandle,
    routing: InteractionRoutingControl,
    executor: Arc<InteractivePreviewExecutor>,
) -> (
    BrowserPreviewSession,
    PreviewOutboundSender,
    PreviewOutboundReceiver,
) {
    let (outbound, receiver) = preview_outbound_channel();
    (
        BrowserPreviewSession::new(handle, routing, Some(executor), outbound.clone()),
        outbound,
        receiver,
    )
}

fn pressed_key(key: &str) -> BrowserInputEdgeWire {
    BrowserInputEdgeWire::Key {
        key: key.to_owned(),
        state: InputButtonStateWire::Pressed,
    }
}

fn interactive_preview_config() -> InteractivePreviewConfig {
    InteractivePreviewConfig {
        target: InteractivePreviewTarget::ActiveScene,
        fps: 60,
        width: 640,
        height: 480,
        format: CanvasFormat::Rgba,
    }
}

/// Drive a set of interactive preview keys the way one subscribe does.
///
/// Reconciliation is against the connection's whole live subscription
/// set, so a caller that wants two previews open names both.
async fn subscribe_interactive_previews(
    previews: &mut BrowserPreviewSession,
    keys: &[(&str, InteractivePreviewConfig)],
) -> Result<(), super::protocol::WsProtocolError> {
    let entries = keys
        .iter()
        .map(|(preview_id, config)| {
            TopicSubscription::keyed("interactive_preview", *preview_id)
                .with_config(serde_json::to_value(*config).expect("config serializes"))
        })
        .collect();
    let state = SubscriptionState::default()
        .subscribed(entries)
        .expect("an interactive preview subscribe applies");
    previews.reconcile(&state).await
}

/// Drive one interactive preview key the way a subscribe does.
async fn subscribe_interactive_preview(
    previews: &mut BrowserPreviewSession,
    preview_id: &str,
    config: InteractivePreviewConfig,
) -> Result<(), super::protocol::WsProtocolError> {
    subscribe_interactive_previews(previews, &[(preview_id, config)]).await
}

/// Retire every interactive preview the way an unsubscribe does.
async fn unsubscribe_all_interactive_previews(previews: &mut BrowserPreviewSession) {
    previews
        .reconcile(&SubscriptionState::default())
        .await
        .expect("retiring previews cannot refuse");
}

#[test]
fn interactive_preview_commands_are_addressed_and_acknowledged() {
    // Opening and closing are subscribe and unsubscribe now; only the
    // messages addressed at an already-open preview remain their own.
    let inject: ClientMessage = serde_json::from_value(serde_json::json!({
        "type": "input_inject",
        "preview_id": "main canvas",
        "events": []
    }))
    .expect("inject command should parse");
    assert!(matches!(
        inject,
        ClientMessage::InputInject { preview_id, .. } if preview_id == "main canvas"
    ));

    let claim: ClientMessage = serde_json::from_value(serde_json::json!({
        "type": "interactive_preview_claim_authoritative",
        "preview_id": "main canvas"
    }))
    .expect("claim command should parse");
    assert!(matches!(
        claim,
        ClientMessage::InteractivePreviewClaimAuthoritative { preview_id }
            if preview_id == "main canvas"
    ));

    let release: ClientMessage = serde_json::from_value(serde_json::json!({
        "type": "interactive_preview_release_authoritative",
        "preview_id": "main canvas"
    }))
    .expect("release command should parse");
    assert!(matches!(
        release,
        ClientMessage::InteractivePreviewReleaseAuthoritative { preview_id }
            if preview_id == "main canvas"
    ));

    // The old bespoke session verbs are gone rather than aliased.
    for retired in [
        serde_json::json!({
            "type": "interactive_preview_open",
            "preview_id": "main canvas",
            "fps": 60,
            "width": 640,
            "height": 480,
            "format": "rgba"
        }),
        serde_json::json!({
            "type": "interactive_preview_close",
            "preview_id": "main canvas"
        }),
    ] {
        serde_json::from_value::<ClientMessage>(retired)
            .expect_err("the interactive preview session verbs are deleted");
    }
}

#[test]
fn an_interactive_preview_subscribe_carries_its_whole_config() {
    let raw = r#"{"type":"subscribe","topics":[{"topic":"interactive_preview","key":"main canvas","config":{"target":"active_scene","fps":60,"width":640,"height":480,"format":"rgba"}}]}"#;
    let message: ClientMessage =
        serde_json::from_str(raw).expect("an interactive preview subscribe parses");
    let ClientMessage::Subscribe { topics, .. } = message else {
        panic!("expected a subscribe");
    };

    let state = SubscriptionState::default()
        .subscribed(topics)
        .expect("the subscribe applies");
    let config: InteractivePreviewConfig =
        state.config_of(TopicId::InteractivePreview, Some("main canvas"));
    assert_eq!(config, interactive_preview_config());
}

#[test]
fn interactive_preview_dimensions_use_format_aware_shape_admission() {
    let wide = SubscriptionState::default()
        .subscribed(vec![
            TopicSubscription::keyed("interactive_preview", "wide").with_config(
                serde_json::json!({"width": 100_000, "height": 1_000, "format": "rgba"}),
            ),
        ])
        .expect("a wide interactive preview fits the publication budget");
    let wide_config: InteractivePreviewConfig =
        wide.config_of(TopicId::InteractivePreview, Some("wide"));
    assert_eq!(wide_config.width, 100_000);
    assert_eq!(wide_config.height, 1_000);
    validate_interactive_preview_shape(100_000, 1_000, CanvasFormat::Rgba)
        .expect("wide shape fits the publication budget");

    let jpeg_error = validate_interactive_preview_shape(65_536, 1, CanvasFormat::Jpeg)
        .expect_err("JPEG exposes its format-level axis ceiling");
    assert!(jpeg_error.message.contains("JPEG preview axes"));
    validate_interactive_preview_shape(65_536, 1, CanvasFormat::Rgba)
        .expect("raw previews retain u32 axes within the byte budget");

    let zero = SubscriptionState::default().subscribed(vec![
        TopicSubscription::keyed("interactive_preview", "empty")
            .with_config(serde_json::json!({"width": 0, "height": 1})),
    ]);
    assert!(zero.is_err(), "a zero-width preview lane is refused");

    let over_budget = SubscriptionState::default()
        .subscribed(vec![
            TopicSubscription::keyed("interactive_preview", "huge").with_config(
                serde_json::json!({"width": 32_768, "height": 4_097, "format": "rgba"}),
            ),
        ])
        .expect_err("over-budget interactive shape is rejected");
    assert_eq!(over_budget.code, "malformed_request");

    let error = validate_interactive_preview_shape(32_768, 4_097, CanvasFormat::Rgba)
        .expect_err("over-budget interactive shape is rejected");
    assert_eq!(error.code, "malformed_request");
}

#[test]
fn an_interactive_preview_subscribe_rejects_invalid_render_config() {
    for (field, value) in [("fps", 0), ("fps", 61), ("width", 0), ("height", 0)] {
        let mut patch = serde_json::json!({
            "fps": 60,
            "width": 640,
            "height": 480,
            "format": "rgba"
        });
        patch[field] = value.into();
        let error = SubscriptionState::default()
            .subscribed(vec![
                TopicSubscription::keyed("interactive_preview", "main").with_config(patch),
            ])
            .expect_err("out-of-range interactive preview config must be rejected");
        assert_eq!(error.code, "validation_error");
    }

    SubscriptionState::default()
        .subscribed(vec![
            TopicSubscription::keyed("interactive_preview", "main")
                .with_config(serde_json::json!({"target": "another_connection"})),
        ])
        .expect_err("unsupported interactive preview target must be rejected");
}

#[tokio::test]
async fn a_refused_preview_subscribe_restores_the_shape_it_had_already_resized() {
    // Reconciliation walks the requested previews in key order, so a
    // request whose later preview is unopenable can reach an earlier
    // one's lane before it learns that. The whole subscribe is abandoned,
    // so the resize has to be abandoned with it. The keys here are
    // deliberately ordered: "alpha" is resized before "omega" refuses.
    let (_source, handle, routing) = browser_preview_test_context();
    let executor = browser_preview_test_executor(routing.clone()).await;
    let (mut session, outbound, frames) = browser_preview_session(handle, routing, executor);

    let original = interactive_preview_config();
    subscribe_interactive_preview(&mut session, "alpha", original)
        .await
        .expect("the first preview opens");
    let publication = session
        .publication_id("alpha")
        .expect("the first preview publishes");
    let wire = WireInteractivePreviewFrame {
        preview_id: "alpha".to_owned(),
        frame_number: 7,
        timestamp_ms: 11,
        width: 1,
        height: 1,
        format: WirePreviewPixelFormat::Rgba,
        payload: Bytes::from_static(&[1, 2, 3, 255]),
    }
    .encode()
    .expect("addressed frame should encode");
    outbound
        .publish(
            PreviewStreamId::Interactive("alpha".to_owned()),
            wire,
            Some(publication),
        )
        .expect("existing preview frame should enter the router");
    let in_flight = frames
        .try_recv()
        .expect("existing preview publication should be ready");
    assert!(matches!(&in_flight, PreviewOutboundItem::Publication(_)));

    let mut resized = original;
    resized.width = 320;
    resized.height = 240;
    // This shape fits the wire's publication budget but not the
    // executor's resource capacity, so it is the lane — not config
    // admission — that refuses, after "alpha" was already resized.
    let mut refused = original;
    refused.width = 4_096;
    refused.height = 4_096;
    let error =
        subscribe_interactive_previews(&mut session, &[("alpha", resized), ("omega", refused)])
            .await
            .expect_err("an unopenable preview refuses the whole reconcile");
    assert_eq!(error.code, "malformed_request");

    assert!(
        session.publication_id("omega").is_none(),
        "the refused preview left no lane behind"
    );
    assert_eq!(
        session.publication_id("alpha"),
        Some(publication),
        "the surviving preview keeps its identity"
    );
    assert_eq!(
        session.preview_config("alpha"),
        Some(original),
        "the surviving preview is back at the shape it had"
    );
    assert!(
        frames.try_recv().is_none(),
        "a refused reconcile must not queue a wire cancellation"
    );
    if let PreviewOutboundItem::Publication(publication) = &in_flight {
        frames.complete(publication);
    }
}

#[tokio::test]
async fn interactive_preview_input_requires_same_connection_open_and_stays_isolated() {
    let (_source, handle, routing) = browser_preview_test_context();
    let executor = browser_preview_test_executor(routing.clone()).await;
    let (mut first, _first_tx, _first_rx) =
        browser_preview_session(handle.clone(), routing.clone(), Arc::clone(&executor));
    let (mut second, _second_tx, _second_rx) =
        browser_preview_session(handle.clone(), routing, executor);
    subscribe_interactive_preview(&mut first, "shared", interactive_preview_config())
        .await
        .expect("first preview should open");
    let first_connection = first.connection_incarnation().get();
    let first_publication = first
        .publication_id("shared")
        .expect("the first preview publishes")
        .get();

    let mut resized = interactive_preview_config();
    resized.width = 960;
    subscribe_interactive_preview(&mut first, "shared", resized)
        .await
        .expect("resubscribing with a new shape preserves the preview identity");
    assert_eq!(
        first
            .publication_id("shared")
            .expect("the resized preview still publishes")
            .get(),
        first_publication
    );

    let error = second
        .inject("shared".to_owned(), vec![pressed_key("foreign")])
        .expect_err("another connection cannot address the first preview");
    assert_eq!(error.code, "malformed_request");

    subscribe_interactive_preview(&mut second, "shared", interactive_preview_config())
        .await
        .expect("same opaque id should open independently");
    let second_connection = second.connection_incarnation().get();
    let second_publication = second
        .publication_id("shared")
        .expect("the second preview publishes")
        .get();
    assert_ne!(first_connection, second_connection);
    assert_ne!(first_publication, second_publication);

    let first_ack = first
        .inject("shared".to_owned(), vec![pressed_key("first")])
        .expect("first connection should inject");
    assert!(matches!(
        first_ack,
        ServerMessage::InputInjected {
            accepted_events: 1,
            ..
        }
    ));
    second
        .inject("shared".to_owned(), vec![pressed_key("second")])
        .expect("second connection should inject");

    let registry = handle.registry().snapshot();
    let first_key = BrowserInputChildKey::new(
        BrowserConnectionIncarnation::new(first_connection),
        BrowserPreviewId::new("shared"),
    );
    let second_key = BrowserInputChildKey::new(
        BrowserConnectionIncarnation::new(second_connection),
        BrowserPreviewId::new("shared"),
    );
    let first_latest = registry
        .child(&first_key)
        .and_then(hypercolor_core::input::BrowserInputChildSlot::latest)
        .expect("first child should publish state");
    let second_latest = registry
        .child(&second_key)
        .and_then(hypercolor_core::input::BrowserInputChildSlot::latest)
        .expect("second child should publish state");
    let InputData::Interaction(first_interaction) = first_latest.as_ref() else {
        panic!("first child should publish interaction data");
    };
    let InputData::Interaction(second_interaction) = second_latest.as_ref() else {
        panic!("second child should publish interaction data");
    };
    assert_eq!(first_interaction.keyboard.pressed_keys, ["first"]);
    assert_eq!(second_interaction.keyboard.pressed_keys, ["second"]);
}

#[tokio::test]
async fn interactive_preview_authoritative_claims_conflict_and_release_idempotently() {
    let (_source, handle, routing) = browser_preview_test_context();
    let executor = browser_preview_test_executor(routing.clone()).await;
    let (mut first, _first_tx, _first_rx) =
        browser_preview_session(handle.clone(), routing.clone(), Arc::clone(&executor));
    let (mut second, _second_tx, _second_rx) =
        browser_preview_session(handle, routing.clone(), executor);
    subscribe_interactive_preview(&mut first, "first", interactive_preview_config())
        .await
        .expect("first preview should open");
    subscribe_interactive_preview(&mut second, "second", interactive_preview_config())
        .await
        .expect("second preview should open");

    let claim = first
        .claim_authoritative("first".to_owned())
        .expect("first claim should succeed");
    assert!(matches!(
        claim,
        ServerMessage::InteractivePreviewAuthoritativeClaimed {
            already_owned: false,
            ..
        }
    ));
    let repeated = first
        .claim_authoritative("first".to_owned())
        .expect("same-owner claim should be idempotent");
    assert!(matches!(
        repeated,
        ServerMessage::InteractivePreviewAuthoritativeClaimed {
            already_owned: true,
            ..
        }
    ));

    let conflict = second
        .claim_authoritative("second".to_owned())
        .expect_err("conflicting claim must not steal ownership");
    assert_eq!(conflict.code, "conflict");

    assert!(matches!(
        first.release_authoritative("first".to_owned()),
        ServerMessage::InteractivePreviewAuthoritativeReleased { released: true, .. }
    ));
    assert!(matches!(
        first.release_authoritative("first".to_owned()),
        ServerMessage::InteractivePreviewAuthoritativeReleased {
            released: false,
            ..
        }
    ));
    second
        .claim_authoritative("second".to_owned())
        .expect("released ownership should hand off cleanly");
    assert!(routing.snapshot().authoritative_browser.is_some());
}

#[tokio::test]
async fn interactive_preview_aborted_future_closes_all_and_releases_once() {
    let (_source, handle, routing) = browser_preview_test_context();
    let observed_routing = routing.clone();
    let observed_registry = handle.registry();
    let executor = browser_preview_test_executor(routing.clone()).await;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut session, _outbound, _frames) = browser_preview_session(handle, routing, executor);
        subscribe_interactive_previews(
            &mut session,
            &[
                ("main", interactive_preview_config()),
                ("inspector", interactive_preview_config()),
            ],
        )
        .await
        .expect("both previews should open");
        session
            .claim_authoritative("main".to_owned())
            .expect("main preview should claim authoritative input");
        ready_tx.send(()).expect("test receiver should remain open");
        std::future::pending::<()>().await;
    });

    ready_rx.await.expect("preview task should become ready");
    assert_eq!(observed_registry.snapshot().children().len(), 2);
    let generation_before_abort = observed_routing.snapshot().generation;
    task.abort();
    task.await.expect_err("preview task should be cancelled");

    assert!(observed_registry.snapshot().children().is_empty());
    let after_abort = observed_routing.snapshot();
    assert!(after_abort.authoritative_browser.is_none());
    assert_eq!(after_abort.generation, generation_before_abort + 1);
}

#[tokio::test]
async fn interactive_preview_explicit_close_and_drop_are_exactly_once() {
    let (_source, handle, routing) = browser_preview_test_context();
    let executor = browser_preview_test_executor(routing.clone()).await;
    let (mut session, _outbound, _frames) =
        browser_preview_session(handle.clone(), routing.clone(), Arc::clone(&executor));
    subscribe_interactive_preview(&mut session, "main", interactive_preview_config())
        .await
        .expect("preview should open");
    session
        .claim_authoritative("main".to_owned())
        .expect("preview should claim authoritative input");

    unsubscribe_all_interactive_previews(&mut session).await;
    let generation_after_close = routing.snapshot().generation;
    // Retiring an already-retired preview is a no-op, not a second
    // teardown: the routing generation must not move again.
    unsubscribe_all_interactive_previews(&mut session).await;
    drop(session);

    assert_eq!(routing.snapshot().generation, generation_after_close);
    assert!(handle.registry().snapshot().children().is_empty());
    assert_eq!(executor.lane_count(), 0);
    assert_eq!(
        executor
            .resource_snapshot()
            .used
            .total_bytes()
            .expect("preview resource ledger should remain representable"),
        0
    );
}

#[tokio::test]
async fn interactive_preview_sender_rejects_queued_frame_from_closed_publication() {
    let (_source, handle, routing) = browser_preview_test_context();
    let executor = browser_preview_test_executor(routing.clone()).await;
    let (mut session, outbound, _frames) = browser_preview_session(handle, routing, executor);
    subscribe_interactive_preview(&mut session, "same", interactive_preview_config())
        .await
        .expect("first preview should open");
    let first_publication = session
        .publication_id("same")
        .expect("first publication should be active")
        .get();
    let first_publication_id = session
        .publication_id("same")
        .expect("first publication should be active");
    let wire = WireInteractivePreviewFrame {
        preview_id: "same".to_owned(),
        frame_number: 7,
        timestamp_ms: 11,
        width: 1,
        height: 1,
        format: WirePreviewPixelFormat::Rgba,
        payload: Bytes::from_static(&[1, 2, 3, 255]),
    }
    .encode()
    .expect("addressed frame should encode");
    outbound
        .publish(
            hypercolor_leptos_ext::ws::PreviewStreamId::Interactive("same".to_owned()),
            wire.clone(),
            Some(first_publication_id),
        )
        .expect("old publication frame should enter the preview router");

    unsubscribe_all_interactive_previews(&mut session).await;
    subscribe_interactive_preview(&mut session, "same", interactive_preview_config())
        .await
        .expect("same id should reopen with a new publication");
    let second_publication = session
        .publication_id("same")
        .expect("second publication should be active")
        .get();
    let second_publication_id = session
        .publication_id("same")
        .expect("second publication should be active");
    assert_ne!(first_publication, second_publication);
    assert!(!session.is_current_publication("same", first_publication_id));
    assert!(session.is_current_publication("same", second_publication_id));
    let decoded = WireInteractivePreviewFrame::decode_bytes(&wire)
        .expect("public binary frame should remain independently decodable");
    assert_eq!(decoded.preview_id, "same");
}

#[tokio::test]
async fn an_interactive_preview_subscription_streams_addressed_frames_from_a_real_lane() {
    let (_source, handle, routing) = browser_preview_test_context();
    let executor = browser_preview_test_executor(routing.clone()).await;
    let (mut session, _outbound, frames) = browser_preview_session(handle, routing, executor);
    let mut config = interactive_preview_config();
    config.width = 16;
    config.height = 8;
    subscribe_interactive_preview(&mut session, "live", config)
        .await
        .expect("interactive preview should open a real lane");

    let publication = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let PreviewOutboundItem::Publication(publication) = frames.recv().await {
                break publication;
            }
        }
    })
    .await
    .expect("real preview lane should publish within one second");
    let (preview_id, publication_id) = publication
        .interactive_fence()
        .expect("interactive publication carries its input fence");
    assert!(session.is_current_publication(preview_id, publication_id));
    let mut cursor = PreviewSendCursor::new(publication, super::protocol::MAX_WS_MESSAGE_BYTES)
        .expect("interactive publication cursor");
    let frame = cursor
        .next_message()
        .expect("interactive publication encoding")
        .expect("interactive publication contains one message");
    let decoded = WireInteractivePreviewFrame::decode_bytes(&frame)
        .expect("relayed interactive preview frame should decode");
    assert_eq!(decoded.preview_id, "live");
    assert_eq!((decoded.width, decoded.height), (16, 8));
    assert_eq!(decoded.format, WirePreviewPixelFormat::Rgba);
    assert_eq!(decoded.payload.len(), 16 * 8 * 4);
}

#[tokio::test]
async fn an_interactive_preview_subscribe_without_an_executor_creates_no_input_attachment() {
    let (_source, handle, routing) = browser_preview_test_context();
    let registry = handle.registry();
    let (outbound, _frames) = preview_outbound_channel();
    let mut session = BrowserPreviewSession::new(handle, routing, None, outbound);

    let error =
        subscribe_interactive_preview(&mut session, "unavailable", interactive_preview_config())
            .await
            .expect_err("subscribing must fail when no render executor exists");
    assert_eq!(error.code, "service_unavailable");
    assert_eq!(
        error
            .details
            .as_ref()
            .and_then(|details| details["preview_id"].as_str()),
        Some("unavailable")
    );
    assert!(registry.snapshot().children().is_empty());
}

#[test]
fn ws_capabilities_include_commands() {
    let capabilities = ws_capabilities();
    assert!(capabilities.contains(&"events".to_owned()));
    assert!(capabilities.contains(&"frame_events".to_owned()));
    assert!(capabilities.contains(&"frames".to_owned()));
    assert!(capabilities.contains(&"spectrum".to_owned()));
    assert!(capabilities.contains(&"canvas".to_owned()));
    assert!(capabilities.contains(&"screen_canvas".to_owned()));
    assert!(capabilities.contains(&"zone_preview".to_owned()));
    assert!(capabilities.contains(&"metrics".to_owned()));
    assert!(capabilities.contains(&"device_metrics".to_owned()));
    assert!(capabilities.contains(&"sensors".to_owned()));
    assert!(capabilities.contains(&"display_preview".to_owned()));
    assert!(capabilities.contains(&"interactive_preview".to_owned()));
    assert!(capabilities.contains(&"input_events".to_owned()));
    assert!(capabilities.contains(&"commands".to_owned()));
    assert!(capabilities.contains(&"canvas_format_jpeg".to_owned()));
    assert!(capabilities.contains(&"interactive_previews".to_owned()));
    assert!(capabilities.contains(&"wide_preview_frames".to_owned()));
    assert!(capabilities.contains(&"preview_chunking".to_owned()));
}

#[test]
fn websocket_manifest_matches_protocol_constants() {
    let manifest: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../protocol/websocket-v1.json"
    )))
    .expect("websocket protocol manifest should parse");

    let manifest_topics = manifest["topics"]
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
    let protocol_channels = TopicId::ALL
        .iter()
        .map(|topic| topic.as_str().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(manifest_topics, protocol_channels);

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
    assert_eq!(manifest["version"], HYPERCOLOR_WS_VERSION);
    assert_eq!(manifest["subprotocol"], HYPERCOLOR_WS_PROTOCOL);
    let default_subscriptions = SubscriptionState::default()
        .live_subscriptions()
        .map(|subscription| subscription.topic.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        manifest["default_subscriptions"],
        serde_json::json!(default_subscriptions)
    );
    let preview_defaults = PreviewTransportLimits::default();
    assert_eq!(
        manifest["preview_transport"]["max_publication_decoded_bytes"],
        preview_defaults.max_decoded_publication_bytes
    );
    assert_eq!(
        manifest["preview_transport"]["max_publication_encoded_bytes"],
        preview_defaults.max_encoded_publication_bytes
    );
    assert_eq!(
        manifest["preview_transport"]["max_connection_bytes"],
        preview_defaults.max_connection_bytes
    );
    assert_eq!(
        manifest["preview_transport"]["max_message_bytes"],
        preview_defaults.max_message_bytes
    );
    assert_eq!(
        manifest["preview_transport"]["max_reassembly_state_bytes"],
        preview_defaults.max_reassembly_state_bytes
    );
    assert_eq!(
        manifest["preview_transport"]["max_tombstone_bytes"],
        preview_defaults.max_tombstone_bytes
    );
    assert_eq!(
        manifest["preview_transport"]["max_sender_state_bytes"],
        preview_defaults.max_sender_state_bytes
    );
    assert_eq!(
        manifest["preview_transport"]["max_cursor_state_bytes"],
        preview_defaults.max_cursor_state_bytes
    );
    assert_eq!(
        manifest["preview_transport"]["partial_idle_ms"],
        preview_defaults.max_idle_ms
    );
    assert_eq!(
        manifest["preview_transport"]["min_message_bytes"],
        PREVIEW_MIN_MESSAGE_BYTES
    );
    assert_eq!(manifest["preview_transport"]["jpeg_max_axis"], u16::MAX);
    assert!(manifest["preview_transport"].get("negotiation").is_none());
    for channel in [
        "canvas",
        "screen_canvas",
        "web_viewport_canvas",
        "zone_preview",
    ] {
        assert!(
            manifest["topic_config"][channel]["width"]
                .get("max")
                .is_none()
        );
        assert!(
            manifest["topic_config"][channel]["height"]
                .get("max")
                .is_none()
        );
    }

    let input_channel = manifest_topics
        .iter()
        .position(|channel| channel == "input_events")
        .and_then(|index| {
            manifest["topics"]
                .as_array()
                .and_then(|channels| channels.get(index))
        })
        .expect("input_events manifest channel");
    assert_eq!(input_channel["payload_schema"], "timed_input_event_v1");
    assert_eq!(
        manifest["json_payloads"]["timed_input_event_v1"]["schema_version"],
        hypercolor_leptos_ext::ws::INPUT_EVENT_PAYLOAD_SCHEMA
    );
    let ownership = &manifest["json_payloads"]["macos_daemon_ownership_changed_v1"];
    assert_eq!(ownership["schema_version"], 1);
    assert_eq!(ownership["topic"], "events");
    assert_eq!(ownership["event"], "macos_daemon_ownership_changed");
    assert_eq!(
        ownership["required_fields"],
        serde_json::json!(["active_owner", "owner_epoch"])
    );
    assert_eq!(
        ownership["optional_fields"]["conflict"],
        serde_json::Value::Null
    );

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
        binary_tags["screen_zones"],
        hypercolor_leptos_ext::ws::SCREEN_ZONES_FRAME_TAG
    );
    assert_eq!(
        binary_tags["web_viewport_canvas"],
        WS_WEB_VIEWPORT_CANVAS_HEADER
    );
    assert_eq!(binary_tags["display_preview"], WS_DISPLAY_PREVIEW_HEADER);
    assert_eq!(binary_tags["zone_preview"], WS_ZONE_PREVIEW_HEADER);
    assert_eq!(
        binary_tags["interactive_preview"],
        hypercolor_leptos_ext::ws::INTERACTIVE_PREVIEW_FRAME_TAG
    );
    assert_eq!(
        binary_tags["wide_preview"],
        hypercolor_leptos_ext::ws::WIDE_PREVIEW_FRAME_TAG
    );
    assert_eq!(
        binary_tags["wide_zone_preview"],
        hypercolor_leptos_ext::ws::WIDE_ZONE_PREVIEW_FRAME_TAG
    );
    assert_eq!(
        binary_tags["wide_interactive_preview"],
        hypercolor_leptos_ext::ws::WIDE_INTERACTIVE_PREVIEW_FRAME_TAG
    );
    assert_eq!(
        binary_tags["wide_screen_zones"],
        hypercolor_leptos_ext::ws::WIDE_SCREEN_ZONES_FRAME_TAG
    );
    assert_eq!(
        binary_tags["extended_screen_zones"],
        hypercolor_leptos_ext::ws::EXTENDED_SCREEN_ZONES_FRAME_TAG
    );
    assert_eq!(
        binary_tags["preview_chunk"],
        hypercolor_leptos_ext::ws::PREVIEW_CHUNK_FRAME_TAG
    );

    let client_messages = manifest["json_messages"]["client"]
        .as_array()
        .expect("client message inventory")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    for message in [
        "subscribe",
        "unsubscribe",
        "input_inject",
        "interactive_preview_claim_authoritative",
        "interactive_preview_release_authoritative",
    ] {
        assert!(client_messages.contains(message), "missing {message}");
    }
    // Opening and closing an interactive preview are subscribe and
    // unsubscribe now; the session verbs are deleted, not aliased.
    for retired in ["interactive_preview_open", "interactive_preview_close"] {
        assert!(
            !client_messages.contains(retired),
            "{retired} should be gone from the manifest"
        );
    }

    let server_messages = manifest["json_messages"]["server"]
        .as_array()
        .expect("server message inventory")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    for retired in ["interactive_preview_opened", "interactive_preview_closed"] {
        assert!(
            !server_messages.contains(retired),
            "{retired} should be gone from the manifest"
        );
    }
}

#[test]
fn a_display_preview_subscription_is_named_by_its_device() {
    // The target used to be a tri-state config field a client retargeted
    // in place. It is the subscription key now, so following a second
    // display is a second subscription rather than a retarget.
    let one = SubscriptionState::default()
        .subscribed(vec![
            TopicSubscription::keyed("display_preview", "device-abc")
                .with_config(serde_json::json!({"fps": 20})),
        ])
        .expect("a keyed display preview subscribe applies");
    assert_eq!(
        one.config_by_topic()["display_preview"],
        serde_json::json!({"device-abc": {"fps": 20}})
    );

    let two = one
        .subscribed(vec![
            TopicSubscription::keyed("display_preview", "device-def")
                .with_config(serde_json::json!({"fps": 5})),
        ])
        .expect("a second display is a second subscription");
    assert_eq!(
        two.config_by_topic()["display_preview"],
        serde_json::json!({"device-abc": {"fps": 20}, "device-def": {"fps": 5}})
    );

    // A subscribe without a key cannot reach a keyed topic at all.
    let error = SubscriptionState::default()
        .subscribed_unkeyed(&["display_preview"], serde_json::Value::Null)
        .expect_err("display_preview needs a device");
    assert_eq!(error.code, "malformed_request");
}

#[test]
fn a_display_preview_key_must_name_a_real_device() {
    let error = SubscriptionState::default()
        .subscribed(vec![TopicSubscription::keyed("display_preview", "   ")])
        .expect_err("whitespace is not a device");
    assert_eq!(error.code, "malformed_request");
    assert!(error.message.contains("device id"));
}

#[test]
fn display_preview_cadence_must_be_in_range() {
    for fps in [0, 120] {
        let error = SubscriptionState::default()
            .subscribed(vec![
                TopicSubscription::keyed("display_preview", "device-abc")
                    .with_config(serde_json::json!({"fps": fps})),
            ])
            .expect_err("out-of-range cadence should be rejected");
        assert_eq!(error.code, "validation_error");
        assert_eq!(
            error.details,
            Some(serde_json::json!({
                "field": "config.display_preview.fps",
                "reason": "expected 1..=30"
            }))
        );
    }
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
    assert_eq!(error.code, "malformed_request");
}

#[test]
fn normalize_command_path_adds_api_prefix() {
    assert_eq!(
        normalize_command_path("/system").expect("path should normalize"),
        "/api/v1/system"
    );
    assert_eq!(
        normalize_command_path("/api/v1/system").expect("path should stay stable"),
        "/api/v1/system"
    );
}

#[test]
fn normalize_command_path_rejects_relative_paths() {
    let error = normalize_command_path("system").expect_err("relative path must fail");
    assert_eq!(error.code, "malformed_request");
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
async fn dispatch_command_routes_to_system() {
    let state = Arc::new(AppState::new());
    let message = dispatch_command(
        &state,
        RequestAuthContext::unsecured(),
        "cmd_status".to_owned(),
        "GET".to_owned(),
        "/system".to_owned(),
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
            let payload = data.expect("system command should return payload");
            assert!(payload.get("identity").is_some());
            assert!(payload.get("status").is_some());
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
        "/system".to_owned(),
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
                Some(serde_json::json!("malformed_request"))
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
        "/system".to_owned(),
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
async fn dispatch_command_rejects_unsecured_protected_capture_access() {
    let state = Arc::new(AppState::new());
    let message = dispatch_command(
        &state,
        RequestAuthContext::unsecured(),
        "cmd_capture_monitors".to_owned(),
        "GET".to_owned(),
        "/capture/monitors".to_owned(),
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
            assert_eq!(status, 403);
            assert!(data.is_none());
            assert_eq!(
                error.and_then(|value| value.get("code").cloned()),
                Some(serde_json::json!("forbidden"))
            );
        }
        _ => panic!("expected command response"),
    }
}

#[tokio::test]
async fn dispatch_command_allows_control_protected_capture_access() {
    let state = secured_state();
    let message = dispatch_command(
        &state,
        RequestAuthContext::control(),
        "cmd_capture_monitors".to_owned(),
        "GET".to_owned(),
        "/capture/monitors".to_owned(),
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
        "/capture/monitors".to_owned(),
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
    assert_eq!(u16::from_le_bytes([encoded[9], encoded[10]]), 1);
}

#[test]
fn led_frame_manifest_layout_matches_the_production_encoder() {
    assert_eq!(
        led_frame_codec_manifest()["layout"],
        serde_json::json!([
            ["u8", "tag"],
            ["u32_le", "frame_number"],
            ["u32_le", "timestamp_ms"],
            ["u16_le", "zone_count"],
            ["repeated_zone", "zones"],
        ])
    );
    let frame = FrameData {
        frame_number: 0x0403_0201,
        timestamp_ms: 0x0807_0605,
        zones: vec![
            ZoneColors {
                zone_id: "a".to_owned(),
                colors: vec![[0x11, 0x22, 0x33]],
            },
            ZoneColors {
                zone_id: "bc".to_owned(),
                colors: vec![[0x44, 0x55, 0x66], [0x77, 0x88, 0x99]],
            },
        ],
    };

    assert_eq!(
        encode_frame_binary(&frame),
        [
            0x01, 1, 2, 3, 4, 5, 6, 7, 8, 2, 0, 1, 0, b'a', 1, 0, 0x11, 0x22, 0x33, 2, 0, b'b',
            b'c', 2, 0, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99,
        ]
    );
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
    assert_eq!(u16::from_le_bytes([encoded[9], encoded[10]]), 1);
    assert_eq!(u16::from_le_bytes([encoded[11], encoded[12]]), 5);
    assert_eq!(&encoded[13..18], b"right");
    assert_eq!(u16::from_le_bytes([encoded[18], encoded[19]]), 2);
    assert_eq!(&encoded[20..26], &[0, 0, 255, 0, 255, 0]);
}

#[test]
fn the_frame_zone_count_survives_past_the_old_u8_ceiling() {
    let zones = (0..300)
        .map(|index| ZoneColors {
            zone_id: format!("z{index}"),
            colors: vec![[1, 2, 3]],
        })
        .collect::<Vec<_>>();
    let frame = FrameData {
        frame_number: 1,
        timestamp_ms: 1,
        zones,
    };

    let encoded = encode_frame_binary(&frame);

    assert_eq!(
        u16::from_le_bytes([encoded[9], encoded[10]]),
        300,
        "a u8 count silently truncated this to 44"
    );
}

#[test]
fn frame_binary_encoder_truncates_payload_at_the_u16_zone_limit() {
    let zone = ZoneColors {
        zone_id: "z".to_owned(),
        colors: Vec::new(),
    };
    let frame = FrameData {
        frame_number: 1,
        timestamp_ms: 2,
        zones: vec![zone; usize::from(u16::MAX) + 1],
    };

    let encoded = encode_frame_binary(&frame);

    assert_eq!(u16::from_le_bytes([encoded[9], encoded[10]]), u16::MAX);
    assert_eq!(encoded.len(), 11 + usize::from(u16::MAX) * 5);
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
        zones: vec!["right".to_owned()],
    });

    let first = cached_frame_payload(&frame, &config);
    let second = cached_frame_payload(&frame, &config);

    assert_eq!(first, second);
    assert_eq!(first.as_ptr(), second.as_ptr());

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
fn cached_frame_payload_keys_each_zone_selection_separately() {
    let _guard = WS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    reset_ws_payload_caches();

    let frame = sample_frame();
    let left = cached_frame_payload(
        &frame,
        &ActiveFramesConfig::new(FramesConfig {
            fps: 30,
            zones: vec!["left".to_owned()],
        }),
    );
    let right = cached_frame_payload(
        &frame,
        &ActiveFramesConfig::new(FramesConfig {
            fps: 30,
            zones: vec!["right".to_owned()],
        }),
    );
    let both = cached_frame_payload(
        &frame,
        &ActiveFramesConfig::new(FramesConfig {
            fps: 30,
            zones: vec!["all".to_owned()],
        }),
    );

    assert_ne!(left, right);
    assert_ne!(left, both);

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
fn zone_preview_binary_encoder_writes_addressed_header_and_rgb_payload() {
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));
    canvas.set_pixel(1, 0, Rgba::new(40, 50, 60, 200));
    let scene_id = SceneId::new();
    let zone_id = ZoneId::new();
    let frame = ZonePreviewFrame {
        scene_id,
        zone_id,
        frame: CanvasFrame::from_canvas(&canvas, 7, 99),
    };

    let encoded = try_encode_cached_zone_preview_binary_scaled(&frame, CanvasFormat::Rgb, 0, 0)
        .expect("zone preview payload should encode");

    assert_eq!(encoded[0], WS_ZONE_PREVIEW_HEADER);
    assert_eq!(
        u32::from_le_bytes([encoded[1], encoded[2], encoded[3], encoded[4]]),
        7
    );
    assert_eq!(
        u32::from_le_bytes([encoded[5], encoded[6], encoded[7], encoded[8]]),
        99
    );
    assert_eq!(&encoded[9..25], scene_id.0.as_bytes());
    assert_eq!(&encoded[25..41], zone_id.0.as_bytes());
    assert_eq!(u16::from_le_bytes([encoded[41], encoded[42]]), 2);
    assert_eq!(u16::from_le_bytes([encoded[43], encoded[44]]), 1);
    assert_eq!(encoded[45], 0);
    assert_eq!(
        &encoded[WS_ZONE_PREVIEW_HEADER_LEN..WS_ZONE_PREVIEW_HEADER_LEN + 6],
        &[10, 20, 30, 40, 50, 60]
    );
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
fn canvas_preview_rgb_binary_applies_brightness_without_alpha() {
    let mut canvas = Canvas::new(1, 1);
    canvas.set_pixel(0, 0, Rgba::new(255, 128, 0, 200));
    let frame = CanvasFrame::from_canvas(&canvas, 7, 99);

    let encoded = encode_canvas_preview_binary(&frame, CanvasFormat::Rgb, 0.5);
    let expected = [
        linear_to_srgb_u8(srgb_u8_to_linear(255) * 0.5),
        linear_to_srgb_u8(srgb_u8_to_linear(128) * 0.5),
        linear_to_srgb_u8(srgb_u8_to_linear(0) * 0.5),
    ];

    assert_eq!(&encoded[14..17], &expected);
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

    let scaled_rgb = encoder
        .encode_scaled_body(&frame, CanvasFormat::Rgb, 1.0, 1, 0)
        .expect("scaled RGB body");
    let dimmed_rgba = encoder
        .encode_scaled_body(&frame, CanvasFormat::Rgba, 0.5, 0, 0)
        .expect("dimmed RGBA body");

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

// ── Shared wire-codec conformance ────────────────────────────────────
//
// The web UI and TUI decode binary payloads with the shared codec in
// `hypercolor-leptos-ext::ws`. These round-trips pin the daemon's
// hand-tuned encoders to that single wire definition — any drift fails
// here instead of breaking clients at runtime.

use hypercolor_leptos_ext::ws as shared_wire;

#[test]
fn canvas_payload_decodes_with_shared_codec() {
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));
    canvas.set_pixel(1, 0, Rgba::new(40, 50, 60, 200));
    let frame = CanvasFrame::from_canvas(&canvas, 7, 99);

    let encoded = encode_canvas_binary_with_header(&frame, CanvasFormat::Rgb, WS_CANVAS_HEADER);
    let decoded = shared_wire::PreviewFrame::decode(&encoded)
        .expect("shared codec must decode daemon canvas payloads");

    assert_eq!(decoded.channel, shared_wire::PreviewFrameChannel::Canvas);
    assert_eq!(decoded.frame_number, 7);
    assert_eq!(decoded.timestamp_ms, 99);
    assert_eq!(decoded.width, 2);
    assert_eq!(decoded.height, 1);
    assert_eq!(decoded.format, shared_wire::PreviewPixelFormat::Rgb);
    assert_eq!(decoded.payload.as_ref(), &[10, 20, 30, 40, 50, 60]);
}

#[test]
fn screen_and_web_viewport_payloads_decode_with_shared_codec() {
    let mut canvas = Canvas::new(1, 1);
    canvas.set_pixel(0, 0, Rgba::new(1, 2, 3, 255));
    let frame = CanvasFrame::from_canvas(&canvas, 1, 2);

    for (header, channel) in [
        (
            WS_SCREEN_CANVAS_HEADER,
            shared_wire::PreviewFrameChannel::ScreenCanvas,
        ),
        (
            WS_WEB_VIEWPORT_CANVAS_HEADER,
            shared_wire::PreviewFrameChannel::WebViewportCanvas,
        ),
    ] {
        let encoded = encode_canvas_binary_with_header(&frame, CanvasFormat::Rgba, header);
        let decoded = shared_wire::PreviewFrame::decode(&encoded)
            .expect("shared codec must decode daemon preview payloads");
        assert_eq!(decoded.channel, channel);
        assert_eq!(decoded.format, shared_wire::PreviewPixelFormat::Rgba);
    }
}

#[test]
fn zone_preview_payload_decodes_with_shared_codec() {
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));
    canvas.set_pixel(1, 0, Rgba::new(40, 50, 60, 200));
    let scene_id = SceneId::new();
    let zone_id = ZoneId::new();
    let frame = ZonePreviewFrame {
        scene_id,
        zone_id,
        frame: CanvasFrame::from_canvas(&canvas, 7, 99),
    };

    let encoded = try_encode_cached_zone_preview_binary_scaled(&frame, CanvasFormat::Rgb, 0, 0)
        .expect("zone preview payload should encode");
    let decoded = shared_wire::ZonePreviewFrame::decode(&encoded)
        .expect("shared codec must decode daemon zone preview payloads");

    assert_eq!(&decoded.scene_id, scene_id.0.as_bytes());
    assert_eq!(&decoded.zone_id, zone_id.0.as_bytes());
    assert_eq!(decoded.frame_number, 7);
    assert_eq!(decoded.timestamp_ms, 99);
    assert_eq!(decoded.width, 2);
    assert_eq!(decoded.height, 1);
    assert_eq!(decoded.format, shared_wire::PreviewPixelFormat::Rgb);
    assert_eq!(decoded.payload.as_ref(), &[10, 20, 30, 40, 50, 60]);
}

#[test]
fn display_preview_payload_decodes_with_shared_codec() {
    let _guard = WS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    reset_ws_payload_caches();

    let snapshot = display_preview_snapshot(64, 5);
    let payload = cached_display_preview_payload(test_display_device(), &snapshot)
        .expect("display preview payload");
    let decoded = shared_wire::DisplayPreviewFrame::decode(&payload)
        .expect("shared codec must decode daemon display preview payloads");

    assert_eq!(decoded.device_id, test_display_device().to_string());
    assert_eq!(decoded.format, shared_wire::PreviewPixelFormat::Jpeg);
    assert_eq!(decoded.width, 256);
    assert_eq!(decoded.height, 256);
    assert_eq!(decoded.payload.len(), 64);
}

#[test]
fn spectrum_payload_decodes_with_shared_codec() {
    let spectrum = SpectrumData {
        timestamp_ms: 77,
        level: 0.5,
        bass: 0.4,
        mid: 0.3,
        treble: 0.2,
        beat: true,
        beat_confidence: 0.9,
        bpm: None,
        bins: vec![0.25; 64],
    };

    let encoded = encode_spectrum_binary(&spectrum, 16);
    let decoded = shared_wire::SpectrumFrame::decode(&encoded)
        .expect("shared codec must decode daemon spectrum payloads");

    assert_eq!(decoded.timestamp_ms, 77);
    assert!((decoded.level - 0.5).abs() < f32::EPSILON);
    assert!((decoded.bass - 0.4).abs() < f32::EPSILON);
    assert!((decoded.mid - 0.3).abs() < f32::EPSILON);
    assert!((decoded.treble - 0.2).abs() < f32::EPSILON);
    assert!(decoded.beat);
    assert!((decoded.beat_confidence - 0.9).abs() < f32::EPSILON);
    assert_eq!(decoded.bins.len(), 16);
    assert!(decoded.bins.iter().all(|bin| (bin - 0.25).abs() < 1e-6));
}

// ── Screen Zones Wire Format ──────────────────────────────────────────────

#[test]
fn screen_zones_encoding_round_trips_through_shared_wire_format() {
    let colors: Vec<[u8; 3]> = (0..12)
        .map(|i| {
            let base = u8::try_from(i * 20).unwrap_or(255);
            [base, base.saturating_add(1), base.saturating_add(2)]
        })
        .collect();
    let frame = hypercolor_core::bus::ScreenZonesFrame {
        frame_number: 99,
        timestamp_ms: 5_000,
        source_width: 3840,
        source_height: 2160,
        grid_cols: 4,
        grid_rows: 3,
        letterbox: [1, 1, 0, 0],
        colors: Arc::new(colors.clone()),
    };

    let encoded = super::relays::encode_screen_zones_frame(&frame)
        .expect("screen zones encoding should succeed");
    let decoded = hypercolor_leptos_ext::ws::ScreenZonesFrame::decode(&encoded)
        .expect("daemon encoding must decode with the shared wire format");

    assert_eq!(decoded.frame_number, 99);
    assert_eq!(decoded.timestamp_ms, 5_000);
    assert_eq!(decoded.source_width, 3840);
    assert_eq!(decoded.source_height, 2160);
    assert_eq!(decoded.grid_cols, 4);
    assert_eq!(decoded.grid_rows, 3);
    assert_eq!(decoded.letterbox, [1, 1, 0, 0]);
    assert_eq!(decoded.zone_rgb(0, 0), Some(colors[0]));
    assert_eq!(decoded.zone_rgb(2, 3), Some(colors[11]));
}

#[test]
fn screen_zones_encoding_preserves_wide_grid_dimensions() {
    let colors = vec![[1, 2, 3]; 256];
    let frame = hypercolor_core::bus::ScreenZonesFrame {
        frame_number: 100,
        timestamp_ms: 5_001,
        source_width: 3840,
        source_height: 2160,
        grid_cols: 256,
        grid_rows: 1,
        letterbox: [0, 0, 256, 0],
        colors: Arc::new(colors),
    };

    let encoded = super::relays::encode_screen_zones_frame(&frame)
        .expect("wide screen zones encoding should succeed");
    let decoded = hypercolor_leptos_ext::ws::ScreenZonesFrame::decode(&encoded)
        .expect("wide daemon encoding must decode with the shared wire format");

    assert_eq!(
        encoded[0],
        hypercolor_leptos_ext::ws::EXTENDED_SCREEN_ZONES_FRAME_TAG
    );
    assert_eq!(decoded.grid_cols, 256);
    assert_eq!(decoded.grid_rows, 1);
    assert_eq!(decoded.letterbox, [0, 0, 256, 0]);
    assert_eq!(decoded.zone_rgb(0, 255), Some([1, 2, 3]));
}

#[test]
fn screen_zones_empty_frame_encodes_as_no_signal() {
    let frame = hypercolor_core::bus::ScreenZonesFrame::default();
    let encoded = super::relays::encode_screen_zones_frame(&frame)
        .expect("empty screen zones encoding should succeed");
    let decoded = hypercolor_leptos_ext::ws::ScreenZonesFrame::decode(&encoded)
        .expect("empty zones frame must remain decodable");

    assert_eq!(decoded.grid_cols, 0);
    assert_eq!(decoded.grid_rows, 0);
    assert!(decoded.payload.is_empty());
}
