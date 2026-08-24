mod sensors;
mod status;

use super::{
    effect_health_status, get_sensors, get_status, get_system, input_source_status,
    macos_daemon_ownership,
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
use hypercolor_core::input::{SourceFreshness, SourceKind, SourceState, SourceStatus};
use hypercolor_types::canvas::Canvas;
use hypercolor_types::sensor::{SensorReading, SensorUnit, SystemSnapshot};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Instant;

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
            "/Applications/Hypercolor.app/Contents/MacOS/hypercolor-daemon",
            "requirement-server",
            4242,
        )
        .expect("fixture identity should be valid"),
        server_session_id: session_id.clone(),
        protected_control_credential: credential.clone(),
    };
    let mut state = AppState::new_with_data_dir(tempdir.path().join("data"));
    state.server_session_id = Some(attestation.server_session_id.as_str().to_owned());
    state
        .security_state
        .install_macos_daemon_session(&attestation);

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

fn source_status_fixture() -> SourceStatus {
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
        action_issue: None,
        diagnostics: None,
        retired: false,
    }
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

#[test]
fn input_source_status_omits_retired_platform_contract() {
    let status = input_source_status(&source_status_fixture(), Instant::now(), true);
    let value = serde_json::to_value(status).expect("source status should serialize");

    assert!(value.get("platform").is_none());
}

#[test]
fn neutral_input_status_excludes_retired_macos_platform_schemas() {
    let document = crate::api::openapi_document();
    let value = serde_json::to_value(document).expect("OpenAPI should serialize");
    let schemas = value["components"]["schemas"]
        .as_object()
        .expect("OpenAPI should contain component schemas");

    for retired in [
        "InputSourcePlatformStatus",
        "MacosArchitecture",
        "MacosAuthorizationState",
        "MacosFrameDrop",
        "MacosInputTelemetry",
        "MacosProtectedSourceState",
        "MacosScreenTelemetry",
        "MacosScreenTiming",
        "MacosSelectionState",
        "MacosTahoeCapabilities",
        "MacosTahoeSelectionCapabilities",
        "MacosTiming",
    ] {
        assert!(
            !schemas.contains_key(retired),
            "retired platform schema {retired} should stay deleted"
        );
    }
    assert!(schemas.contains_key("MacosDaemonOwnershipStatus"));
    assert!(schemas.contains_key("MacosDaemonOwnerConflictStatus"));
    assert!(schemas.contains_key("MacosDaemonOwnerRecoveryRequiredStatus"));
    assert!(schemas.contains_key("MacosDaemonHandoverPhase"));
}
