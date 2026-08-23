mod sensors;
mod status;

use super::{
    effect_health_status, get_sensors, get_status, get_system, input_source_status,
    input_status_snapshot, macos_daemon_ownership, macos_selection_state,
    macos_tahoe_selection_capabilities,
};
use crate::api::security::RequestAuthContext;
use crate::app_state::AppState;
use crate::macos_owner::{
    MacosDaemonOwner, MacosDaemonSessionAttestation, MacosHandoverPhase, MacosOwnerConflict,
    MacosOwnerIdentity, MacosOwnerRecoveryRequired, MacosOwnerSnapshot,
    MacosProtectedControlCredential, MacosServerSessionId,
};
use crate::performance::{
    CompositorBackendKind, FrameTimeline, FullFrameCopyMetrics, LatestFrameMetrics,
    OutputFrameSourceKind,
};
use crate::preview_runtime::{PreviewPixelFormat, PreviewStreamDemand};
use axum::body::to_bytes;
use axum::extract::{Extension, State};
use hypercolor_core::bus::CanvasFrame;
use hypercolor_core::input::screen::ScreenAdmissionCapacity;
use hypercolor_core::input::{
    InputData, InputSource, MacosArchitecture, MacosAuthorizationState, MacosCapabilityOwner,
    MacosDaemonOwnerConflict, MacosInputPlatformStatus, MacosProtectedSourceState,
    MacosScreenPlatformStatus, MacosScreenTimingStatus, MacosSelectionState,
    MacosTahoeCapabilities, MacosTahoeSelectionCapabilities, MacosTimingStatus, SourceFreshness,
    SourceKind, SourcePlatformStatus, SourceState, SourceStatus, SourceStatusHandle,
    SourceStatusReporter,
};
use hypercolor_types::canvas::Canvas;
use hypercolor_types::sensor::{SensorReading, SensorUnit, SystemSnapshot};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;

struct TestStatusSource {
    status: SourceStatusReporter,
}

impl TestStatusSource {
    fn new(platform: SourcePlatformStatus) -> Self {
        let mut status =
            SourceStatusReporter::new("test-screen", SourceKind::Screen, "test", true, true, false);
        status
            .set_platform(Some(platform))
            .expect("test platform status should publish");
        Self { status }
    }
}

impl InputSource for TestStatusSource {
    fn name(&self) -> &'static str {
        "test-screen"
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }

    fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn stop(&mut self) {}

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        false
    }

    fn is_screen_source(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn public_system_identity_exposes_only_the_attested_session_id() {
    let tempdir = tempfile::tempdir().expect("server test data dir should be created");
    let session_id = MacosServerSessionId::from_bytes([0x33; 16]);
    let credential = MacosProtectedControlCredential::from_bytes([0x77; 32]);
    let attestation = MacosDaemonSessionAttestation {
        schema_version: crate::macos_owner::MACOS_DAEMON_SESSION_ATTESTATION_SCHEMA_VERSION,
        owner: MacosDaemonOwner::AppSidecar,
        owner_epoch: 7,
        owner_identity: MacosOwnerIdentity::new(
            "audit-server",
            tempdir.path().join("hypercolor-daemon"),
            "requirement-server",
            4242,
        )
        .expect("fixture identity should be valid"),
        server_session_id: session_id.clone(),
        protected_control_credential: credential.clone(),
    };
    let mut state = AppState::new_with_data_dir(tempdir.path().join("data"));
    state.install_macos_daemon_session(&attestation);

    let response = get_system(
        State(Arc::new(state)),
        Extension(RequestAuthContext::preflight()),
    )
    .await;
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("server response should read");
    let value: Value = serde_json::from_slice(&bytes).expect("server response should be JSON");

    assert_eq!(
        value["data"]["identity"]["server_session_id"],
        session_id.as_str()
    );
    assert!(!String::from_utf8_lossy(&bytes).contains(credential.expose_secret()));
}

fn source_status_fixture(platform: Option<SourcePlatformStatus>) -> SourceStatus {
    SourceStatus {
        source_id: Arc::from("fixture:source"),
        kind: SourceKind::Interaction,
        backend: Arc::from("fixture"),
        configured: true,
        consented: true,
        demanded: true,
        active_consumer_count: 2,
        state: SourceState::Live,
        freshness: SourceFreshness::NotApplicable,
        source_graph_generation: 7,
        session_generation: 11,
        last_sample_at: None,
        freshness_deadline: None,
        resource_count: 2,
        denied_resource_count: 0,
        issue: None,
        freshness_issue: None,
        platform: platform.map(Arc::new),
        retired: false,
    }
}

const fn timing_fixture(
    sample_count: u64,
    total_ns: u64,
    max_ns: u64,
    p95_ns: u64,
    p99_ns: u64,
) -> MacosTimingStatus {
    MacosTimingStatus {
        sample_count,
        total_ns,
        max_ns,
        p95_ns,
        p99_ns,
    }
}

#[test]
fn input_source_status_serializes_macos_input_platform() {
    let platform = SourcePlatformStatus::MacosInput(MacosInputPlatformStatus {
        keyboard: MacosProtectedSourceState::NeedsProcessRestart,
        pointer: MacosProtectedSourceState::Live,
        keyboard_tcc: MacosAuthorizationState::Authorized,
        secure_input_active: true,
        keyboard_owner: MacosCapabilityOwner::AppSidecar,
        pointer_owner: MacosCapabilityOwner::Broker,
        owner_conflict: Some(Arc::new(MacosDaemonOwnerConflict {
            active: MacosCapabilityOwner::LaunchdService,
            contender: MacosCapabilityOwner::HomebrewService,
            observed_at_ms: 1_725_000_000_123,
        })),
        authorization_last_transition_at: None,
        owner_designated_requirement_hash: None,
        host_architecture: Some(MacosArchitecture::AppleSilicon),
        executable_architecture: MacosArchitecture::Intel,
        translated_process: Some(true),
        capture_session_generation: Some(31),
        topology_generation: Some(5),
        queue_capacity: Some(2_048),
        queue_depth: Some(7),
        input_events_received: Some(1_000),
        input_events_published: Some(990),
        input_events_dropped: Some(10),
        tap_disabled_timeout: Some(2),
        tap_disabled_user_input: Some(1),
        tap_reenabled: Some(3),
        state_gaps: Some(4),
        callback_to_publication_timing: Some(timing_fixture(990, 1_980_000, 4_000, 2_000, 3_000)),
    });
    let status = input_source_status(&source_status_fixture(Some(platform)), Instant::now(), true);
    let value = serde_json::to_value(status).expect("input status should serialize");

    assert_eq!(
        value["platform"],
        json!({
            "type": "macos_input",
            "keyboard": "needs_process_restart",
            "pointer": "live",
            "keyboard_tcc": "authorized",
            "secure_input_active": true,
            "keyboard_owner": "app_sidecar",
            "pointer_owner": "broker",
            "owner_conflict": {
                "active": "launchd_service",
                "contender": "homebrew_service",
                "observed_at_ms": 1_725_000_000_123_u64
            },
            "telemetry": {
                "host_architecture": "apple_silicon",
                "executable_architecture": "intel",
                "translated_process": true,
                "capture_session_generation": 31,
                "topology_generation": 5,
                "queue_capacity": 2048,
                "queue_depth": 7,
                "input_events_received": 1000,
                "input_events_published": 990,
                "input_events_dropped": 10,
                "tap_disabled_timeout": 2,
                "tap_disabled_user_input": 1,
                "tap_reenabled": 3,
                "state_gaps": 4,
                "callback_to_publication_timing": {
                    "sample_count": 990,
                    "total_ns": 1_980_000,
                    "max_ns": 4_000,
                    "p95_ns": 2_000,
                    "p99_ns": 3_000
                }
            }
        })
    );
}

#[test]
fn system_status_serializes_authoritative_macos_daemon_ownership() {
    let value = serde_json::to_value(macos_daemon_ownership(&MacosOwnerSnapshot {
        active_owner: MacosDaemonOwner::DirectLaunchd,
        owner_epoch: 42,
        conflict: Some(MacosOwnerConflict {
            active_owner: MacosDaemonOwner::DirectLaunchd,
            active_epoch: 42,
            contender_owner: MacosDaemonOwner::Homebrew,
            observed_at_ms: 1_725_000_000_789,
        }),
        recovery_required: Some(MacosOwnerRecoveryRequired {
            requested_owner: MacosDaemonOwner::AppSidecar,
            prior_owner: MacosDaemonOwner::Homebrew,
            phase: MacosHandoverPhase::RollbackStopRequested,
        }),
    }))
    .expect("macOS daemon ownership should serialize");

    assert_eq!(
        value,
        json!({
            "active_owner": "launchd_service",
            "owner_epoch": 42,
            "conflict": {
                "active": "launchd_service",
                "contender": "homebrew_service",
                "observed_at_ms": 1_725_000_000_789_u64
            },
            "recovery_required": {
                "requested_owner": "app_sidecar",
                "prior_owner": "homebrew_service",
                "phase": "rollback_stop_requested"
            }
        })
    );
}

#[tokio::test]
async fn input_source_status_serializes_macos_screen_platform() {
    let platform = SourcePlatformStatus::MacosScreen(MacosScreenPlatformStatus {
        state: MacosProtectedSourceState::Interrupted,
        tcc: MacosAuthorizationState::Denied,
        owner: MacosCapabilityOwner::Standalone,
        selection: MacosSelectionState::SessionScoped {
            content_style: Arc::from("multiple_windows"),
        },
        selection_diagnostic_label: Some(Arc::from("multiple_windows")),
        selection_revision: 17,
        tahoe: MacosTahoeCapabilities {
            host_architecture: MacosArchitecture::AppleSilicon,
            translated_process: true,
            content_tone_mapping_info: true,
            metal4: false,
        },
        tahoe_selection: Some(MacosTahoeSelectionCapabilities {
            source_id: Arc::from("macos:session:multiple-windows:w42:a18:com.secret.private"),
            capture_session_generation: 29,
            hdr_capture: true,
            dual_range_screenshots: true,
        }),
        owner_conflict: Some(Arc::new(MacosDaemonOwnerConflict {
            active: MacosCapabilityOwner::Standalone,
            contender: MacosCapabilityOwner::App,
            observed_at_ms: 1_725_000_000_456,
        })),
        authorization_last_transition_at: None,
        owner_designated_requirement_hash: None,
        executable_architecture: MacosArchitecture::Intel,
        stream_state: Arc::from("stopped"),
        capture_session_generation: Some(29),
        topology_generation: Some(3),
        resource_generation: Some(8),
        publication_plan_generation: Some(13),
        pixel_format: Some(Arc::from("rgba16_float")),
        dynamic_range: Some(Arc::from("high")),
        color_space: Some(Arc::from("display_p3")),
        transfer_function: Some(Arc::from("linear")),
        display_scale_bits: Some(2.0_f64.to_bits()),
        native_width: Some(3_840),
        native_height: Some(2_160),
        queue_depth: 8,
        admitted_native_bytes: 268_435_456,
        pinned_generations: Some(2),
        frames_received: 120,
        frames_published: 116,
        frames_superseded: 2,
        frames_malformed: 1,
        frames_dropped: Arc::from([(Arc::from("validation"), 2)]),
        frames_stale: 1,
        publication_path: Some(Arc::from("cpu_fallback")),
        fallback_reason: Some(Arc::from("native_descriptor_incompatible")),
        timing: MacosScreenTimingStatus {
            callback: timing_fixture(10, 900, 90, 80, 90),
            retain: timing_fixture(10, 400, 40, 30, 40),
            enqueue: timing_fixture(10, 300, 30, 20, 30),
            conversion: timing_fixture(10, 700, 70, 60, 70),
            cpu_reduction: timing_fixture(10, 1_100, 110, 100, 110),
            native_import: timing_fixture(10, 600, 60, 50, 60),
            native_reduction_submit: timing_fixture(10, 800, 80, 70, 80),
            publication: timing_fixture(10, 500, 50, 40, 50),
            capture_to_native_publication: timing_fixture(
                8, 8_000_000, 1_200_000, 1_000_000, 1_200_000,
            ),
            capture_to_converted_publication: timing_fixture(
                6, 9_000_000, 1_800_000, 1_600_000, 1_800_000,
            ),
        },
        callback_total_ns: 900,
        callback_max_ns: 90,
        retain_total_ns: 400,
        retain_max_ns: 40,
        conversion_total_ns: 700,
        conversion_max_ns: 70,
        cpu_reduction_total_ns: 1_100,
        cpu_reduction_max_ns: 110,
        native_import_total_ns: 600,
        native_import_max_ns: 60,
        native_reduction_submit_total_ns: 800,
        native_reduction_submit_max_ns: 80,
        publication_total_ns: 500,
        publication_max_ns: 50,
    });
    let state = AppState::new();
    state
        .input_manager
        .lock()
        .await
        .add_source(Box::new(TestStatusSource::new(platform.clone())));
    let source = source_status_fixture(Some(platform));
    let status = input_source_status(&source, Instant::now(), true);
    let value = serde_json::to_value(status).expect("screen status should serialize");

    assert_eq!(value["active_consumer_count"], 2);
    let platform = &value["platform"];
    assert_eq!(platform["type"], "macos_screen");
    assert_eq!(platform["state"], "interrupted");
    assert_eq!(platform["tcc"], "denied");
    assert_eq!(platform["owner"], "standalone");
    assert_eq!(
        platform["selection"],
        json!({"type": "session_scoped", "content_style": "multiple_windows"})
    );
    assert_eq!(platform["tahoe"]["host_architecture"], "apple_silicon");
    assert_eq!(
        platform["tahoe_selection"]["capture_session_generation"],
        29
    );
    assert_eq!(
        platform["tahoe_selection"]["source_id"],
        "macos:session:multiple-windows:w42:a18:com.secret.private"
    );
    assert_eq!(platform["owner_conflict"]["contender"], "app");
    let telemetry = &platform["telemetry"];
    assert_eq!(telemetry["executable_architecture"], "intel");
    assert_eq!(telemetry["stream_state"], "stopped");
    assert_eq!(telemetry["capture_session_generation"], 29);
    assert_eq!(telemetry["topology_generation"], 3);
    assert_eq!(telemetry["resource_generation"], 8);
    assert_eq!(telemetry["publication_plan_generation"], 13);
    assert_eq!(telemetry["pixel_format"], "rgba16_float");
    assert_eq!(telemetry["dynamic_range"], "high");
    assert_eq!(telemetry["color_space"], "display_p3");
    assert_eq!(telemetry["transfer_function"], "linear");
    assert_eq!(telemetry["selection_diagnostic_label"], "multiple_windows");
    assert_eq!(telemetry["display_scale"], 2.0);
    assert_eq!(telemetry["native_width"], 3_840);
    assert_eq!(telemetry["native_height"], 2_160);
    assert_eq!(telemetry["queue_depth"], 8);
    assert_eq!(telemetry["admitted_native_bytes"], 268_435_456_u64);
    assert_eq!(telemetry["pinned_generations"], 2);
    assert_eq!(
        telemetry["frames_dropped"],
        json!([{"reason": "validation", "count": 2}])
    );
    assert_eq!(telemetry["frames_stale"], 1);
    assert_eq!(telemetry["frames_malformed"], 1);
    assert_eq!(telemetry["publication_path"], "cpu_fallback");
    assert_eq!(
        telemetry["fallback_reason"],
        "native_descriptor_incompatible"
    );
    assert_eq!(telemetry["callback_total_ns"], 900);
    assert_eq!(telemetry["retain_total_ns"], 400);
    assert_eq!(telemetry["conversion_total_ns"], 700);
    assert_eq!(telemetry["cpu_reduction_total_ns"], 1_100);
    assert_eq!(telemetry["native_import_total_ns"], 600);
    assert_eq!(telemetry["native_reduction_submit_total_ns"], 800);
    assert_eq!(telemetry["publication_total_ns"], 500);
    assert_eq!(telemetry["timing"]["callback"]["sample_count"], 10);
    assert_eq!(telemetry["timing"]["enqueue"]["p99_ns"], 30);
    assert_eq!(
        telemetry["timing"]["capture_to_native_publication"]["p95_ns"],
        1_000_000
    );
    assert_eq!(
        telemetry["timing"]["capture_to_converted_publication"]["sample_count"],
        6
    );

    let remote = input_source_status(&source, Instant::now(), false);
    let remote = serde_json::to_value(remote).expect("remote screen status should serialize");
    assert_eq!(
        remote["platform"]["tahoe_selection"]["source_id"],
        "session_scoped"
    );
    assert!(!remote.to_string().contains("com.secret.private"));
    assert!(!remote.to_string().contains("w42"));

    let public = serde_json::to_value(input_status_snapshot(&state.domains.platform))
        .expect("public input status should serialize");
    assert!(!public.to_string().contains("com.secret.private"));
    assert!(!public.to_string().contains("w42"));
    assert!(public.to_string().contains("session_scoped"));

    let state = Arc::new(state);
    let anonymous = get_system(
        State(Arc::clone(&state)),
        Extension(RequestAuthContext::preflight()),
    )
    .await;
    let read = get_system(
        State(Arc::clone(&state)),
        Extension(RequestAuthContext::read_only()),
    )
    .await;
    let control = get_system(State(state), Extension(RequestAuthContext::control())).await;
    let anonymous = to_bytes(anonymous.into_body(), usize::MAX)
        .await
        .expect("anonymous system response should read");
    let read = to_bytes(read.into_body(), usize::MAX)
        .await
        .expect("read system response should read");
    let control = to_bytes(control.into_body(), usize::MAX)
        .await
        .expect("control system response should read");
    let anonymous: Value =
        serde_json::from_slice(&anonymous).expect("anonymous system response should parse");
    let read: Value = serde_json::from_slice(&read).expect("read system response should parse");
    let control: Value =
        serde_json::from_slice(&control).expect("control system response should parse");

    assert!(anonymous["data"]["identity"]["instance_id"].is_string());
    assert!(anonymous["data"].get("status").is_none());
    let read_screen = read["data"]["status"]["input"]["sources"]
        .as_array()
        .and_then(|sources| {
            sources
                .iter()
                .find(|source| source["platform"]["type"] == "macos_screen")
        })
        .expect("read status should include the macOS screen source");
    let control_screen = control["data"]["status"]["input"]["sources"]
        .as_array()
        .and_then(|sources| {
            sources
                .iter()
                .find(|source| source["platform"]["type"] == "macos_screen")
        })
        .expect("control status should include the macOS screen source");
    assert_eq!(
        read_screen["platform"]["tahoe_selection"]["source_id"],
        "session_scoped"
    );
    assert_eq!(
        control_screen["platform"]["tahoe_selection"]["source_id"],
        "macos:session:multiple-windows:w42:a18:com.secret.private"
    );
}

#[test]
fn input_source_status_omits_absent_platform() {
    let status = input_source_status(&source_status_fixture(None), Instant::now(), true);
    let value = serde_json::to_value(status).expect("source status should serialize");

    assert!(value.get("platform").is_none());
}

#[test]
fn macos_selection_status_preserves_public_shapes() {
    let empty = serde_json::to_value(macos_selection_state(&MacosSelectionState::None))
        .expect("empty selection should serialize");
    let display = serde_json::to_value(macos_selection_state(&MacosSelectionState::Display {
        source_id: Arc::from("display:7a3f"),
    }))
    .expect("display selection should serialize");

    assert_eq!(empty, json!({ "type": "none" }));
    assert_eq!(
        display,
        json!({ "type": "display", "source_id": "display:7a3f" })
    );

    let display_capabilities = macos_tahoe_selection_capabilities(
        &MacosTahoeSelectionCapabilities {
            source_id: Arc::from("display:7a3f"),
            capture_session_generation: 1,
            hdr_capture: false,
            dual_range_screenshots: false,
        },
        false,
    );
    assert_eq!(display_capabilities.source_id, "display:7a3f");
}

#[test]
fn macos_platform_json_tolerates_future_fields() {
    #[derive(Debug, Deserialize)]
    struct TolerantInputSourceStatus {
        platform: Option<TolerantPlatformStatus>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum TolerantPlatformStatus {
        MacosScreen { state: String },
    }

    let value = json!({
        "platform": {
            "type": "macos_screen",
            "state": "live",
            "future_probe": { "available": true }
        },
        "future_source_field": 42
    });
    let status: TolerantInputSourceStatus =
        serde_json::from_value(value).expect("unknown fields should remain additive");
    let Some(TolerantPlatformStatus::MacosScreen { state }) = status.platform else {
        panic!("fixture should decode the macOS screen variant");
    };

    assert_eq!(state, "live");
}

#[test]
fn macos_platform_status_is_present_in_openapi() {
    let document = crate::api::openapi_document();
    let value = serde_json::to_value(document).expect("OpenAPI should serialize");
    let schemas = value["components"]["schemas"]
        .as_object()
        .expect("OpenAPI should contain component schemas");

    assert!(schemas.contains_key("InputSourcePlatformStatus"));
    assert!(schemas.contains_key("MacosDaemonOwnershipStatus"));
    assert!(schemas.contains_key("MacosDaemonOwnerConflictStatus"));
    assert!(schemas.contains_key("MacosDaemonOwnerRecoveryRequiredStatus"));
    assert!(schemas.contains_key("MacosDaemonHandoverPhase"));
    assert!(schemas.contains_key("MacosSelectionState"));
    assert!(schemas.contains_key("MacosArchitecture"));
    assert!(schemas.contains_key("MacosTahoeCapabilities"));
    assert!(schemas.contains_key("MacosTahoeSelectionCapabilities"));
    assert!(schemas.contains_key("MacosInputTelemetry"));
    assert!(schemas.contains_key("MacosScreenTelemetry"));
    assert!(schemas.contains_key("MacosTiming"));
    assert!(schemas.contains_key("MacosScreenTiming"));
    assert!(schemas.contains_key("MacosFrameDrop"));
    let platform_schema = &schemas["InputSourcePlatformStatus"];
    let encoded = serde_json::to_string(platform_schema).expect("schema should encode");
    assert!(encoded.contains("macos_input"));
    assert!(encoded.contains("macos_screen"));
}
