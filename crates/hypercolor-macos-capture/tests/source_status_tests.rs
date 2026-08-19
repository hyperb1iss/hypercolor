use hypercolor_macos_capture::{
    MacosCaptureContentStyle, MacosCaptureSelection, MacosHostArchitecture,
    MacosProtectedSourceState, MacosScreenAuthorizationState, MacosScreenSelectionSnapshot,
    MacosScreenStatusSnapshot, MacosScreenTahoeSelectionStatus, MacosScreenTahoeStatus,
    MacosScreenTimingStatus, screen_diagnostics_envelope, screen_selection_snapshot,
};
use hypercolor_types::source_status::SourceDiagnosticsEnvelope;
use serde_json::json;

fn status(selection: MacosCaptureSelection) -> MacosScreenStatusSnapshot {
    MacosScreenStatusSnapshot {
        state: MacosProtectedSourceState::ReadyIdle,
        authorization: MacosScreenAuthorizationState::Authorized,
        owner: "app_sidecar".into(),
        selection: MacosScreenSelectionSnapshot {
            revision: 9,
            selection,
        },
        tahoe: MacosScreenTahoeStatus {
            host_architecture: MacosHostArchitecture::AppleSilicon,
            translated_process: false,
            content_tone_mapping_info: true,
            metal4: true,
        },
        tahoe_selection: None,
        owner_conflict: None,
        authorization_last_transition_age_ms: None,
        owner_designated_requirement_hash: None,
        executable_architecture: MacosHostArchitecture::AppleSilicon,
        capture_session_generation: None,
        topology_generation: None,
        resource_generation: None,
        publication_plan_generation: None,
        pixel_format: None,
        dynamic_range: None,
        color_space: None,
        transfer_function: None,
        display_scale: None,
        native_width: None,
        native_height: None,
        queue_depth: 3,
        admitted_native_bytes: 0,
        pinned_generations: 0,
        frames_received: 0,
        frames_published: 0,
        frames_superseded: 0,
        frames_malformed: 0,
        frames_dropped: Vec::new(),
        frames_stale: 0,
        publication_path: None,
        fallback_reason: None,
        timing: MacosScreenTimingStatus::default(),
    }
}

fn envelope(selection: MacosCaptureSelection) -> SourceDiagnosticsEnvelope {
    screen_diagnostics_envelope(&status(selection)).expect("fixture diagnostics should be bounded")
}

#[test]
fn selection_snapshot_decodes_supported_selection_shapes() {
    let display = screen_selection_snapshot(&envelope(MacosCaptureSelection::Display {
        source_id: "display:7a3f".into(),
    }))
    .expect("display selection should decode")
    .expect("screen diagnostics should match");
    assert_eq!(display.revision, 9);
    assert_eq!(
        display.selection,
        MacosCaptureSelection::Display {
            source_id: "display:7a3f".into()
        }
    );

    let session = screen_selection_snapshot(&envelope(MacosCaptureSelection::SessionScoped {
        content_style: MacosCaptureContentStyle::Application,
    }))
    .expect("session selection should decode")
    .expect("screen diagnostics should match");
    assert_eq!(
        session.selection,
        MacosCaptureSelection::SessionScoped {
            content_style: MacosCaptureContentStyle::Application
        }
    );

    let none = screen_selection_snapshot(&envelope(MacosCaptureSelection::None))
        .expect("empty selection should decode")
        .expect("screen diagnostics should match");
    assert_eq!(none.selection, MacosCaptureSelection::None);
}

#[test]
fn selection_snapshot_ignores_foreign_or_future_envelopes() {
    for envelope in [
        SourceDiagnosticsEnvelope::try_new(
            "other.backend",
            1,
            Vec::new(),
            json!({"selection_revision": 9, "selection": {"type": "none"}}),
        )
        .expect("foreign fixture should be bounded"),
        SourceDiagnosticsEnvelope::try_new(
            "macos.screen",
            2,
            Vec::new(),
            json!({"selection_revision": 9, "selection": {"type": "none"}}),
        )
        .expect("future fixture should be bounded"),
    ] {
        assert_eq!(
            screen_selection_snapshot(&envelope)
                .expect("foreign diagnostics should not fail decoding"),
            None
        );
    }
}

#[test]
fn selection_snapshot_rejects_malformed_operational_fields() {
    let malformed = SourceDiagnosticsEnvelope::try_new(
        "macos.screen",
        1,
        Vec::new(),
        json!({"selection_revision": 9, "selection": {"type": "display"}}),
    )
    .expect("malformed operational fixture remains a bounded envelope");
    assert!(screen_selection_snapshot(&malformed).is_err());
}

#[test]
fn diagnostics_present_neutral_labels_and_redact_session_identity() {
    let secret = "macos:session:com.secret.private:w42";
    let mut status = status(MacosCaptureSelection::SessionScoped {
        content_style: MacosCaptureContentStyle::Application,
    });
    status.state = MacosProtectedSourceState::NeedsProcessRestart;
    status.authorization = MacosScreenAuthorizationState::NotDetermined;
    status.tahoe_selection = Some(MacosScreenTahoeSelectionStatus {
        source_id: secret.into(),
        capture_session_generation: 12,
        hdr_capture: true,
        dual_range_screenshots: true,
    });

    let diagnostics =
        screen_diagnostics_envelope(&status).expect("session diagnostics should remain bounded");
    assert_eq!(
        diagnostics.payload()["tahoe_selection"]["source_id"],
        "session_scoped"
    );
    assert!(
        !serde_json::to_string(&diagnostics)
            .expect("serialize diagnostics")
            .contains(secret)
    );
    assert_eq!(diagnostics.display()[0].value, "Restart required");
    assert_eq!(diagnostics.display()[1].value, "Not determined");
    assert_eq!(diagnostics.display()[3].value, "Session scoped");
}
