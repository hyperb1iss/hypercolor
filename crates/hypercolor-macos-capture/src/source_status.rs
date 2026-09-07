use hypercolor_types::source_status::{
    SourceDiagnosticsDisplayField, SourceDiagnosticsEnvelope, SourceDiagnosticsEnvelopeError,
};
use std::sync::Arc;

use crate::{
    MacosCaptureContentStyle, MacosCaptureSelection, MacosHostArchitecture,
    MacosProtectedSourceState,
};

pub const SCREEN_DIAGNOSTICS_SCHEMA: &str = "macos.screen";
pub const SCREEN_DIAGNOSTICS_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosScreenSelectionSnapshot {
    pub revision: u64,
    pub selection: MacosCaptureSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosScreenAuthorizationState {
    Unknown,
    NotDetermined,
    Denied,
    Authorized,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosScreenOwnerConflict {
    pub active: Arc<str>,
    pub contender: Arc<str>,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MacosSourceTimingStatus {
    pub sample_count: u64,
    pub total_ns: u64,
    pub max_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MacosScreenTimingStatus {
    pub callback: MacosSourceTimingStatus,
    pub retain: MacosSourceTimingStatus,
    pub enqueue: MacosSourceTimingStatus,
    pub conversion: MacosSourceTimingStatus,
    pub cpu_reduction: MacosSourceTimingStatus,
    pub native_import: MacosSourceTimingStatus,
    pub native_reduction_submit: MacosSourceTimingStatus,
    pub publication: MacosSourceTimingStatus,
    pub capture_to_native_publication: MacosSourceTimingStatus,
    pub capture_to_converted_publication: MacosSourceTimingStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacosScreenTahoeStatus {
    pub host_architecture: MacosHostArchitecture,
    pub translated_process: bool,
    pub content_tone_mapping_info: bool,
    pub metal4: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosScreenTahoeSelectionStatus {
    pub source_id: Arc<str>,
    pub capture_session_generation: u64,
    pub hdr_capture: bool,
    pub dual_range_screenshots: bool,
}

#[derive(Debug, Clone)]
pub struct MacosScreenStatusSnapshot {
    pub state: MacosProtectedSourceState,
    pub authorization: MacosScreenAuthorizationState,
    pub owner: Arc<str>,
    pub selection: MacosScreenSelectionSnapshot,
    pub tahoe: MacosScreenTahoeStatus,
    pub tahoe_selection: Option<MacosScreenTahoeSelectionStatus>,
    pub owner_conflict: Option<MacosScreenOwnerConflict>,
    pub authorization_last_transition_age_ms: Option<u64>,
    pub owner_designated_requirement_hash: Option<Arc<str>>,
    pub executable_architecture: MacosHostArchitecture,
    pub capture_session_generation: Option<u64>,
    pub topology_generation: Option<u64>,
    pub resource_generation: Option<u64>,
    pub publication_plan_generation: Option<u64>,
    pub pixel_format: Option<Arc<str>>,
    pub dynamic_range: Option<Arc<str>>,
    pub color_space: Option<Arc<str>>,
    pub transfer_function: Option<Arc<str>>,
    pub display_scale: Option<f64>,
    pub native_width: Option<u32>,
    pub native_height: Option<u32>,
    pub queue_depth: usize,
    pub admitted_native_bytes: u64,
    pub pinned_generations: usize,
    pub frames_received: u64,
    pub frames_published: u64,
    pub frames_superseded: u64,
    pub frames_malformed: u64,
    pub frames_dropped: Vec<(Arc<str>, u64)>,
    pub frames_stale: u64,
    pub publication_path: Option<Arc<str>>,
    pub fallback_reason: Option<Arc<str>>,
    pub timing: MacosScreenTimingStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MacosScreenDiagnosticsDecodeError {
    #[error("screen diagnostics selection revision is missing or invalid")]
    InvalidRevision,
    #[error("screen diagnostics selection is missing or invalid")]
    InvalidSelection,
}

/// Build the opaque status envelope owned by this platform boundary.
pub fn screen_diagnostics_envelope(
    status: &MacosScreenStatusSnapshot,
) -> Result<SourceDiagnosticsEnvelope, SourceDiagnosticsEnvelopeError> {
    let selection = &status.selection.selection;
    let payload = screen_diagnostics_payload(status, true);
    let public_payload = screen_diagnostics_payload(status, false);
    let display = vec![
        SourceDiagnosticsDisplayField::new(
            "state",
            "Capture",
            protected_state_display_name(status.state),
        ),
        SourceDiagnosticsDisplayField::new(
            "authorization",
            "Authorization",
            authorization_state_display_name(status.authorization),
        ),
        SourceDiagnosticsDisplayField::new("owner", "Owner", owner_display_name(&status.owner)),
        SourceDiagnosticsDisplayField::new(
            "selection",
            "Selection",
            selection_display_name(selection),
        ),
        SourceDiagnosticsDisplayField::new(
            "stream",
            "Stream",
            stream_state_display_name(status.state),
        ),
    ];
    SourceDiagnosticsEnvelope::try_new_with_public_payload(
        SCREEN_DIAGNOSTICS_SCHEMA,
        SCREEN_DIAGNOSTICS_VERSION,
        display,
        payload,
        public_payload,
    )
}

fn screen_diagnostics_payload(
    status: &MacosScreenStatusSnapshot,
    include_private_selection_ids: bool,
) -> serde_json::Value {
    let selection = &status.selection.selection;
    serde_json::json!({
        "state": protected_state_name(status.state),
        "tcc": authorization_state_name(status.authorization),
        "owner": status.owner.as_ref(),
        "selection": encode_selection(selection, include_private_selection_ids),
        "selection_diagnostic_label": selection_diagnostic_label(selection),
        "selection_revision": status.selection.revision,
        "tahoe": tahoe_payload(status.tahoe),
        "tahoe_selection": status
            .tahoe_selection
            .as_ref()
            .map(|selection_status| tahoe_selection_payload(
                selection_status,
                selection,
                include_private_selection_ids,
            )),
        "owner_conflict": status.owner_conflict.as_ref().map(|conflict| serde_json::json!({
            "active": conflict.active.as_ref(),
            "contender": conflict.contender.as_ref(),
            "observed_at_ms": conflict.observed_at_ms,
        })),
        "authorization_last_transition_age_ms": status.authorization_last_transition_age_ms,
        "owner_designated_requirement_hash": status.owner_designated_requirement_hash.as_deref(),
        "executable_architecture": architecture_name(status.executable_architecture),
        "stream_state": stream_state_name(status.state),
        "capture_session_generation": status.capture_session_generation,
        "topology_generation": status.topology_generation,
        "resource_generation": status.resource_generation,
        "publication_plan_generation": status.publication_plan_generation,
        "pixel_format": status.pixel_format.as_deref(),
        "dynamic_range": status.dynamic_range.as_deref(),
        "color_space": status.color_space.as_deref(),
        "transfer_function": status.transfer_function.as_deref(),
        "display_scale": status.display_scale,
        "native_width": status.native_width,
        "native_height": status.native_height,
        "queue_depth": status.queue_depth,
        "admitted_native_bytes": status.admitted_native_bytes,
        "pinned_generations": status.pinned_generations,
        "frames_received": status.frames_received,
        "frames_published": status.frames_published,
        "frames_superseded": status.frames_superseded,
        "frames_malformed": status.frames_malformed,
        "frames_dropped": status.frames_dropped.iter().map(|(reason, count)| {
            serde_json::json!({"reason": reason.as_ref(), "count": count})
        }).collect::<Vec<_>>(),
        "frames_stale": status.frames_stale,
        "publication_path": status.publication_path.as_deref(),
        "fallback_reason": status.fallback_reason.as_deref(),
        "timing": screen_timing_payload(status.timing),
    })
}

const fn protected_state_name(state: MacosProtectedSourceState) -> &'static str {
    match state {
        MacosProtectedSourceState::Disabled => "disabled",
        MacosProtectedSourceState::NeedsUserAction => "needs_user_action",
        MacosProtectedSourceState::PermissionDenied => "permission_denied",
        MacosProtectedSourceState::NeedsProcessRestart => "needs_process_restart",
        MacosProtectedSourceState::NeedsSelection => "needs_selection",
        MacosProtectedSourceState::ReadyIdle => "ready_idle",
        MacosProtectedSourceState::Starting => "starting",
        MacosProtectedSourceState::Live => "live",
        MacosProtectedSourceState::Interrupted => "interrupted",
        MacosProtectedSourceState::Revoked => "revoked",
        MacosProtectedSourceState::Failed => "failed",
    }
}

const fn protected_state_display_name(state: MacosProtectedSourceState) -> &'static str {
    match state {
        MacosProtectedSourceState::Disabled => "Disabled",
        MacosProtectedSourceState::NeedsUserAction => "Needs authorization",
        MacosProtectedSourceState::PermissionDenied => "Permission denied",
        MacosProtectedSourceState::NeedsProcessRestart => "Restart required",
        MacosProtectedSourceState::NeedsSelection => "Needs selection",
        MacosProtectedSourceState::ReadyIdle => "Ready",
        MacosProtectedSourceState::Starting => "Starting",
        MacosProtectedSourceState::Live => "Live",
        MacosProtectedSourceState::Interrupted => "Interrupted",
        MacosProtectedSourceState::Revoked => "Permission revoked",
        MacosProtectedSourceState::Failed => "Failed",
    }
}

const fn authorization_state_name(state: MacosScreenAuthorizationState) -> &'static str {
    match state {
        MacosScreenAuthorizationState::Unknown => "unknown",
        MacosScreenAuthorizationState::NotDetermined => "not_determined",
        MacosScreenAuthorizationState::Denied => "denied",
        MacosScreenAuthorizationState::Authorized => "authorized",
    }
}

const fn authorization_state_display_name(state: MacosScreenAuthorizationState) -> &'static str {
    match state {
        MacosScreenAuthorizationState::Unknown => "Unknown",
        MacosScreenAuthorizationState::NotDetermined => "Not determined",
        MacosScreenAuthorizationState::Denied => "Denied",
        MacosScreenAuthorizationState::Authorized => "Authorized",
    }
}

fn owner_display_name(owner: &str) -> &str {
    match owner {
        "app_sidecar" => "App sidecar",
        "launchd_service" => "Launchd service",
        "homebrew_service" => "Homebrew service",
        "standalone" => "Standalone",
        "broker" => "Broker",
        "app" => "App",
        _ => owner,
    }
}

const fn architecture_name(architecture: MacosHostArchitecture) -> &'static str {
    match architecture {
        MacosHostArchitecture::AppleSilicon => "apple_silicon",
        MacosHostArchitecture::Intel => "intel",
    }
}

const fn stream_state_name(state: MacosProtectedSourceState) -> &'static str {
    match state {
        MacosProtectedSourceState::Starting | MacosProtectedSourceState::Live => "active",
        MacosProtectedSourceState::Interrupted
        | MacosProtectedSourceState::Revoked
        | MacosProtectedSourceState::Failed => "stopped",
        _ => "inactive",
    }
}

const fn stream_state_display_name(state: MacosProtectedSourceState) -> &'static str {
    match state {
        MacosProtectedSourceState::Starting | MacosProtectedSourceState::Live => "Active",
        MacosProtectedSourceState::Interrupted
        | MacosProtectedSourceState::Revoked
        | MacosProtectedSourceState::Failed => "Stopped",
        _ => "Inactive",
    }
}

const fn selection_display_name(selection: &MacosCaptureSelection) -> &'static str {
    match selection {
        MacosCaptureSelection::None => "None",
        MacosCaptureSelection::Display { .. } => "Display",
        MacosCaptureSelection::SessionScoped { .. } => "Session scoped",
    }
}

fn selection_diagnostic_label(selection: &MacosCaptureSelection) -> Option<&'static str> {
    match selection {
        MacosCaptureSelection::None => None,
        MacosCaptureSelection::Display { .. } => Some("display"),
        MacosCaptureSelection::SessionScoped { content_style } => {
            Some(content_style_name(*content_style))
        }
    }
}

fn tahoe_payload(status: MacosScreenTahoeStatus) -> serde_json::Value {
    serde_json::json!({
        "host_architecture": architecture_name(status.host_architecture),
        "translated_process": status.translated_process,
        "content_tone_mapping_info": status.content_tone_mapping_info,
        "metal4": status.metal4,
    })
}

fn tahoe_selection_payload(
    status: &MacosScreenTahoeSelectionStatus,
    selection: &MacosCaptureSelection,
    include_private_selection_ids: bool,
) -> serde_json::Value {
    let source_id = match selection {
        MacosCaptureSelection::Display { .. } if include_private_selection_ids => {
            status.source_id.as_ref()
        }
        MacosCaptureSelection::Display { .. } => "display",
        MacosCaptureSelection::None => "none",
        MacosCaptureSelection::SessionScoped { .. } => "session_scoped",
    };
    serde_json::json!({
        "source_id": source_id,
        "capture_session_generation": status.capture_session_generation,
        "hdr_capture": status.hdr_capture,
        "dual_range_screenshots": status.dual_range_screenshots,
    })
}

fn timing_payload(status: MacosSourceTimingStatus) -> serde_json::Value {
    serde_json::json!({
        "sample_count": status.sample_count,
        "total_ns": status.total_ns,
        "max_ns": status.max_ns,
        "p95_ns": status.p95_ns,
        "p99_ns": status.p99_ns,
    })
}

fn screen_timing_payload(status: MacosScreenTimingStatus) -> serde_json::Value {
    serde_json::json!({
        "callback": timing_payload(status.callback),
        "retain": timing_payload(status.retain),
        "enqueue": timing_payload(status.enqueue),
        "conversion": timing_payload(status.conversion),
        "cpu_reduction": timing_payload(status.cpu_reduction),
        "native_import": timing_payload(status.native_import),
        "native_reduction_submit": timing_payload(status.native_reduction_submit),
        "publication": timing_payload(status.publication),
        "capture_to_native_publication": timing_payload(status.capture_to_native_publication),
        "capture_to_converted_publication": timing_payload(
            status.capture_to_converted_publication,
        ),
    })
}

fn encode_selection(
    selection: &MacosCaptureSelection,
    include_private_selection_ids: bool,
) -> serde_json::Value {
    match selection {
        MacosCaptureSelection::None => serde_json::json!({"type": "none"}),
        MacosCaptureSelection::Display { source_id } if include_private_selection_ids => {
            serde_json::json!({"type": "display", "source_id": source_id.as_ref()})
        }
        MacosCaptureSelection::Display { .. } => {
            serde_json::json!({"type": "display", "source_id": "display"})
        }
        MacosCaptureSelection::SessionScoped { content_style } => serde_json::json!({
            "type": "session_scoped",
            "content_style": content_style_name(*content_style),
        }),
    }
}

const fn content_style_name(style: MacosCaptureContentStyle) -> &'static str {
    match style {
        MacosCaptureContentStyle::Window => "window",
        MacosCaptureContentStyle::MultipleWindows => "multiple_windows",
        MacosCaptureContentStyle::Application => "application",
        MacosCaptureContentStyle::MultipleApplications => "multiple_applications",
        MacosCaptureContentStyle::Mixed => "mixed",
    }
}

/// Decode the operational selection fields owned by this platform boundary.
pub fn screen_selection_snapshot(
    envelope: &SourceDiagnosticsEnvelope,
) -> Result<Option<MacosScreenSelectionSnapshot>, MacosScreenDiagnosticsDecodeError> {
    if envelope.schema() != SCREEN_DIAGNOSTICS_SCHEMA
        || envelope.version() != SCREEN_DIAGNOSTICS_VERSION
    {
        return Ok(None);
    }
    let payload = envelope.payload();
    let revision = payload
        .get("selection_revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or(MacosScreenDiagnosticsDecodeError::InvalidRevision)?;
    let selection = decode_selection(
        payload
            .get("selection")
            .ok_or(MacosScreenDiagnosticsDecodeError::InvalidSelection)?,
    )?;
    Ok(Some(MacosScreenSelectionSnapshot {
        revision,
        selection,
    }))
}

fn decode_selection(
    value: &serde_json::Value,
) -> Result<MacosCaptureSelection, MacosScreenDiagnosticsDecodeError> {
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("none") => Ok(MacosCaptureSelection::None),
        Some("display") => value
            .get("source_id")
            .and_then(serde_json::Value::as_str)
            .filter(|source_id| !source_id.is_empty())
            .map(|source_id| MacosCaptureSelection::Display {
                source_id: Arc::from(source_id),
            })
            .ok_or(MacosScreenDiagnosticsDecodeError::InvalidSelection),
        Some("session_scoped") => value
            .get("content_style")
            .and_then(serde_json::Value::as_str)
            .and_then(parse_content_style)
            .map(|content_style| MacosCaptureSelection::SessionScoped { content_style })
            .ok_or(MacosScreenDiagnosticsDecodeError::InvalidSelection),
        _ => Err(MacosScreenDiagnosticsDecodeError::InvalidSelection),
    }
}

fn parse_content_style(value: &str) -> Option<MacosCaptureContentStyle> {
    match value {
        "window" => Some(MacosCaptureContentStyle::Window),
        "multiple_windows" => Some(MacosCaptureContentStyle::MultipleWindows),
        "application" => Some(MacosCaptureContentStyle::Application),
        "multiple_applications" => Some(MacosCaptureContentStyle::MultipleApplications),
        "mixed" => Some(MacosCaptureContentStyle::Mixed),
        _ => None,
    }
}
