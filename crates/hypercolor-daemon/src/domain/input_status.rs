//! Transport-independent input health projection.

use std::time::{Duration, Instant};

use hypercolor_core::input::{
    MacosArchitecture as CoreMacosArchitecture,
    MacosAuthorizationState as CoreMacosAuthorizationState,
    MacosCapabilityOwner as CoreMacosCapabilityOwner, MacosDaemonOwnerConflict,
    MacosInputPlatformStatus, MacosProtectedSourceState as CoreMacosProtectedSourceState,
    MacosScreenPlatformStatus, MacosScreenTimingStatus,
    MacosSelectionState as CoreMacosSelectionState,
    MacosTahoeCapabilities as CoreMacosTahoeCapabilities,
    MacosTahoeSelectionCapabilities as CoreMacosTahoeSelectionCapabilities, MacosTimingStatus,
    SourceFreshness, SourceIssue, SourceKind, SourcePlatformStatus, SourceState, SourceStatus,
};
use hypercolor_types::api::system::{
    InputSourceIssueStatus, InputSourcePlatformStatus, InputSourceStatus, InputStatus,
    MacosArchitecture, MacosAuthorizationState, MacosCapabilityOwner, MacosDaemonHandoverPhase,
    MacosDaemonOwnerConflictStatus, MacosDaemonOwnerRecoveryRequiredStatus,
    MacosDaemonOwnershipStatus, MacosFrameDrop, MacosInputTelemetry, MacosProtectedSourceState,
    MacosScreenTelemetry, MacosScreenTiming, MacosSelectionState, MacosTahoeCapabilities,
    MacosTahoeSelectionCapabilities, MacosTiming,
};

use crate::domain::context::PlatformContext;
use crate::macos_owner::{MacosDaemonOwner, MacosHandoverPhase, MacosOwnerSnapshot};

#[derive(Debug)]
pub(crate) struct InputDiagnostic {
    pub source_id: String,
    pub status: &'static str,
    pub detail: String,
}

/// Build the redacted input health snapshot used without protected control.
#[must_use]
pub(crate) fn input_status_snapshot(platform: &PlatformContext) -> InputStatus {
    input_status_snapshot_with_privacy(platform, false)
}

pub(crate) fn input_status_snapshot_with_privacy(
    platform: &PlatformContext,
    include_private_selection_ids: bool,
) -> InputStatus {
    let now = Instant::now();
    let registry = platform.source_status_snapshot();
    let statuses = registry
        .handles()
        .iter()
        .map(|handle| handle.snapshot_at(now))
        .collect::<Vec<_>>();
    let host_sources = statuses
        .iter()
        .filter(|source| is_host_interaction_source(source));
    let sources = statuses
        .iter()
        .map(|source| input_source_status(source, now, include_private_selection_ids))
        .collect();

    InputStatus {
        enabled: platform.input_enabled(),
        host_capture_registered: host_sources.clone().next().is_some(),
        host_capturing: host_sources
            .clone()
            .any(|source| matches!(source.state, SourceState::Live | SourceState::Degraded)),
        devices_opened: host_sources
            .clone()
            .map(|source| source.resource_count)
            .sum(),
        devices_denied: host_sources
            .clone()
            .map(|source| source.denied_resource_count)
            .sum(),
        degraded: host_sources
            .clone()
            .find_map(|source| source.issue.as_ref())
            .map(|issue| issue.code.to_string()),
        backends: host_sources
            .map(|source| source.backend.to_string())
            .collect(),
        source_graph_generation: registry.source_graph_generation(),
        sources,
    }
}

/// Return only source states that require operator attention.
#[must_use]
pub(crate) fn actionable_input_diagnostics(input: &InputStatus) -> Vec<InputDiagnostic> {
    input
        .sources
        .iter()
        .filter_map(|source| {
            let (status, fallback, issue) =
                if source.demanded && matches!(source.state.as_str(), "failed" | "unavailable") {
                    (
                        "fail",
                        "demanded source is unavailable",
                        source.lifecycle_issue.as_ref(),
                    )
                } else if source.demanded && source.state == "stopped" {
                    (
                        "warning",
                        "demanded source is stopped",
                        source.lifecycle_issue.as_ref(),
                    )
                } else if source.demanded
                    && matches!(source.state.as_str(), "live" | "degraded")
                    && source.freshness == "stale"
                {
                    (
                        "warning",
                        "demanded source data is stale",
                        source.freshness_issue.as_ref(),
                    )
                } else if source.configured && source.state == "degraded" {
                    (
                        "warning",
                        "configured source is degraded",
                        source.lifecycle_issue.as_ref(),
                    )
                } else {
                    return None;
                };

            Some(InputDiagnostic {
                source_id: source.source_id.clone(),
                status,
                detail: format_input_diagnostic_detail(issue, fallback),
            })
        })
        .collect()
}

pub(crate) fn input_source_status(
    source: &SourceStatus,
    now: Instant,
    include_private_selection_ids: bool,
) -> InputSourceStatus {
    let lifecycle_issue = source.issue.as_ref().map(input_source_issue_status);
    let freshness_issue = source
        .freshness_issue
        .as_ref()
        .map(input_source_issue_status);
    let issue = freshness_issue.clone().or_else(|| lifecycle_issue.clone());

    InputSourceStatus {
        source_id: source.source_id.to_string(),
        kind: source_kind_name(source.kind).to_owned(),
        backend: source.backend.to_string(),
        configured: source.configured,
        consented: source.consented,
        demanded: source.demanded,
        active_consumer_count: source.active_consumer_count,
        state: source_state_name(source.state).to_owned(),
        freshness: source_freshness_name(source.freshness).to_owned(),
        source_graph_generation: source.source_graph_generation,
        session_generation: source.session_generation,
        last_sample_age_ms: source
            .last_sample_at
            .map(|sampled_at| duration_ms(now.saturating_duration_since(sampled_at))),
        freshness_remaining_ms: source
            .freshness_deadline
            .map(|deadline| duration_ms(deadline.saturating_duration_since(now))),
        resource_count: source.resource_count,
        denied_resource_count: source.denied_resource_count,
        issue,
        lifecycle_issue,
        freshness_issue,
        platform: source.platform.as_deref().and_then(|platform| {
            input_source_platform_status(platform, now, include_private_selection_ids)
        }),
        retired: source.retired,
    }
}

fn input_source_platform_status(
    platform: &SourcePlatformStatus,
    now: Instant,
    include_private_selection_ids: bool,
) -> Option<InputSourcePlatformStatus> {
    match platform {
        SourcePlatformStatus::MacosInput(status) => Some(macos_input_platform_status(status, now)),
        SourcePlatformStatus::MacosScreen(status) => Some(macos_screen_platform_status(
            status,
            now,
            include_private_selection_ids,
        )),
        _ => None,
    }
}

fn macos_input_platform_status(
    status: &MacosInputPlatformStatus,
    now: Instant,
) -> InputSourcePlatformStatus {
    InputSourcePlatformStatus::MacosInput {
        keyboard: macos_protected_source_state(status.keyboard),
        pointer: macos_protected_source_state(status.pointer),
        keyboard_tcc: macos_authorization_state(status.keyboard_tcc),
        secure_input_active: status.secure_input_active,
        keyboard_owner: macos_capability_owner(status.keyboard_owner),
        pointer_owner: macos_capability_owner(status.pointer_owner),
        owner_conflict: status
            .owner_conflict
            .as_deref()
            .map(macos_daemon_owner_conflict),
        telemetry: MacosInputTelemetry {
            authorization_last_transition_age_ms: status
                .authorization_last_transition_at
                .map(|transition| duration_ms(now.saturating_duration_since(transition))),
            owner_designated_requirement_hash: status
                .owner_designated_requirement_hash
                .as_deref()
                .map(str::to_owned),
            host_architecture: status.host_architecture.map(macos_architecture),
            executable_architecture: macos_architecture(status.executable_architecture),
            translated_process: status.translated_process,
            capture_session_generation: status.capture_session_generation,
            topology_generation: status.topology_generation,
            queue_capacity: status.queue_capacity,
            queue_depth: status.queue_depth,
            input_events_received: status.input_events_received,
            input_events_published: status.input_events_published,
            input_events_dropped: status.input_events_dropped,
            tap_disabled_timeout: status.tap_disabled_timeout,
            tap_disabled_user_input: status.tap_disabled_user_input,
            tap_reenabled: status.tap_reenabled,
            state_gaps: status.state_gaps,
            callback_to_publication_timing: status
                .callback_to_publication_timing
                .as_ref()
                .map(macos_timing_status),
        },
    }
}

fn macos_screen_platform_status(
    status: &MacosScreenPlatformStatus,
    now: Instant,
    include_private_selection_ids: bool,
) -> InputSourcePlatformStatus {
    InputSourcePlatformStatus::MacosScreen {
        state: macos_protected_source_state(status.state),
        tcc: macos_authorization_state(status.tcc),
        owner: macos_capability_owner(status.owner),
        selection: macos_selection_state(&status.selection),
        tahoe: macos_tahoe_capabilities(&status.tahoe),
        tahoe_selection: status.tahoe_selection.as_ref().map(|capabilities| {
            macos_tahoe_selection_capabilities(capabilities, include_private_selection_ids)
        }),
        owner_conflict: status
            .owner_conflict
            .as_deref()
            .map(macos_daemon_owner_conflict),
        telemetry: MacosScreenTelemetry {
            authorization_last_transition_age_ms: status
                .authorization_last_transition_at
                .map(|transition| duration_ms(now.saturating_duration_since(transition))),
            owner_designated_requirement_hash: status
                .owner_designated_requirement_hash
                .as_deref()
                .map(str::to_owned),
            executable_architecture: macos_architecture(status.executable_architecture),
            stream_state: status.stream_state.to_string(),
            capture_session_generation: status.capture_session_generation,
            topology_generation: status.topology_generation,
            resource_generation: status.resource_generation,
            publication_plan_generation: status.publication_plan_generation,
            pixel_format: status.pixel_format.as_deref().map(str::to_owned),
            dynamic_range: status.dynamic_range.as_deref().map(str::to_owned),
            color_space: status.color_space.as_deref().map(str::to_owned),
            transfer_function: status.transfer_function.as_deref().map(str::to_owned),
            selection_diagnostic_label: status
                .selection_diagnostic_label
                .as_deref()
                .map(str::to_owned),
            display_scale: status.display_scale_bits.map(f64::from_bits),
            native_width: status.native_width,
            native_height: status.native_height,
            queue_depth: status.queue_depth,
            admitted_native_bytes: status.admitted_native_bytes,
            pinned_generations: status.pinned_generations,
            frames_received: status.frames_received,
            frames_published: status.frames_published,
            frames_superseded: status.frames_superseded,
            frames_malformed: status.frames_malformed,
            frames_dropped: status
                .frames_dropped
                .iter()
                .map(|(reason, count)| MacosFrameDrop {
                    reason: reason.to_string(),
                    count: *count,
                })
                .collect(),
            frames_stale: status.frames_stale,
            publication_path: status.publication_path.as_deref().map(str::to_owned),
            fallback_reason: status.fallback_reason.as_deref().map(str::to_owned),
            timing: Some(macos_screen_timing_status(&status.timing)),
            callback_total_ns: status.callback_total_ns,
            callback_max_ns: status.callback_max_ns,
            retain_total_ns: status.retain_total_ns,
            retain_max_ns: status.retain_max_ns,
            conversion_total_ns: status.conversion_total_ns,
            conversion_max_ns: status.conversion_max_ns,
            cpu_reduction_total_ns: status.cpu_reduction_total_ns,
            cpu_reduction_max_ns: status.cpu_reduction_max_ns,
            native_import_total_ns: status.native_import_total_ns,
            native_import_max_ns: status.native_import_max_ns,
            native_reduction_submit_total_ns: status.native_reduction_submit_total_ns,
            native_reduction_submit_max_ns: status.native_reduction_submit_max_ns,
            publication_total_ns: status.publication_total_ns,
            publication_max_ns: status.publication_max_ns,
        },
    }
}

fn macos_timing_status(status: &MacosTimingStatus) -> MacosTiming {
    MacosTiming {
        sample_count: status.sample_count,
        total_ns: status.total_ns,
        max_ns: status.max_ns,
        p95_ns: status.p95_ns,
        p99_ns: status.p99_ns,
    }
}

fn macos_screen_timing_status(status: &MacosScreenTimingStatus) -> MacosScreenTiming {
    MacosScreenTiming {
        callback: macos_timing_status(&status.callback),
        retain: macos_timing_status(&status.retain),
        enqueue: macos_timing_status(&status.enqueue),
        conversion: macos_timing_status(&status.conversion),
        cpu_reduction: macos_timing_status(&status.cpu_reduction),
        native_import: macos_timing_status(&status.native_import),
        native_reduction_submit: macos_timing_status(&status.native_reduction_submit),
        publication: macos_timing_status(&status.publication),
        capture_to_native_publication: macos_timing_status(&status.capture_to_native_publication),
        capture_to_converted_publication: macos_timing_status(
            &status.capture_to_converted_publication,
        ),
    }
}

const fn macos_protected_source_state(
    state: CoreMacosProtectedSourceState,
) -> MacosProtectedSourceState {
    match state {
        CoreMacosProtectedSourceState::Disabled => MacosProtectedSourceState::Disabled,
        CoreMacosProtectedSourceState::NeedsUserAction => {
            MacosProtectedSourceState::NeedsUserAction
        }
        CoreMacosProtectedSourceState::PermissionDenied => {
            MacosProtectedSourceState::PermissionDenied
        }
        CoreMacosProtectedSourceState::NeedsProcessRestart => {
            MacosProtectedSourceState::NeedsProcessRestart
        }
        CoreMacosProtectedSourceState::NeedsSelection => MacosProtectedSourceState::NeedsSelection,
        CoreMacosProtectedSourceState::ReadyIdle => MacosProtectedSourceState::ReadyIdle,
        CoreMacosProtectedSourceState::Starting => MacosProtectedSourceState::Starting,
        CoreMacosProtectedSourceState::Live => MacosProtectedSourceState::Live,
        CoreMacosProtectedSourceState::Interrupted => MacosProtectedSourceState::Interrupted,
        CoreMacosProtectedSourceState::Revoked => MacosProtectedSourceState::Revoked,
        CoreMacosProtectedSourceState::Failed => MacosProtectedSourceState::Failed,
    }
}

const fn macos_authorization_state(state: CoreMacosAuthorizationState) -> MacosAuthorizationState {
    match state {
        CoreMacosAuthorizationState::Unknown => MacosAuthorizationState::Unknown,
        CoreMacosAuthorizationState::NotDetermined => MacosAuthorizationState::NotDetermined,
        CoreMacosAuthorizationState::Denied => MacosAuthorizationState::Denied,
        CoreMacosAuthorizationState::Authorized => MacosAuthorizationState::Authorized,
    }
}

const fn macos_capability_owner(owner: CoreMacosCapabilityOwner) -> MacosCapabilityOwner {
    match owner {
        CoreMacosCapabilityOwner::AppSidecar => MacosCapabilityOwner::AppSidecar,
        CoreMacosCapabilityOwner::App => MacosCapabilityOwner::App,
        CoreMacosCapabilityOwner::LaunchdService => MacosCapabilityOwner::LaunchdService,
        CoreMacosCapabilityOwner::HomebrewService => MacosCapabilityOwner::HomebrewService,
        CoreMacosCapabilityOwner::Broker => MacosCapabilityOwner::Broker,
        CoreMacosCapabilityOwner::Standalone => MacosCapabilityOwner::Standalone,
    }
}

fn macos_daemon_owner_conflict(
    conflict: &MacosDaemonOwnerConflict,
) -> MacosDaemonOwnerConflictStatus {
    MacosDaemonOwnerConflictStatus {
        active: macos_capability_owner(conflict.active),
        contender: macos_capability_owner(conflict.contender),
        observed_at_ms: conflict.observed_at_ms,
    }
}

const fn macos_daemon_owner(owner: MacosDaemonOwner) -> MacosCapabilityOwner {
    match owner {
        MacosDaemonOwner::AppSidecar => MacosCapabilityOwner::AppSidecar,
        MacosDaemonOwner::DirectLaunchd => MacosCapabilityOwner::LaunchdService,
        MacosDaemonOwner::Homebrew => MacosCapabilityOwner::HomebrewService,
        MacosDaemonOwner::Standalone => MacosCapabilityOwner::Standalone,
    }
}

pub(crate) fn macos_daemon_ownership(snapshot: &MacosOwnerSnapshot) -> MacosDaemonOwnershipStatus {
    MacosDaemonOwnershipStatus {
        active_owner: macos_daemon_owner(snapshot.active_owner),
        owner_epoch: snapshot.owner_epoch,
        conflict: snapshot
            .conflict
            .map(|conflict| MacosDaemonOwnerConflictStatus {
                active: macos_daemon_owner(conflict.active_owner),
                contender: macos_daemon_owner(conflict.contender_owner),
                observed_at_ms: conflict.observed_at_ms,
            }),
        recovery_required: snapshot.recovery_required.map(|recovery| {
            MacosDaemonOwnerRecoveryRequiredStatus {
                requested_owner: macos_daemon_owner(recovery.requested_owner),
                prior_owner: macos_daemon_owner(recovery.prior_owner),
                phase: macos_daemon_handover_phase(recovery.phase),
            }
        }),
    }
}

const fn macos_daemon_handover_phase(phase: MacosHandoverPhase) -> MacosDaemonHandoverPhase {
    match phase {
        MacosHandoverPhase::Prepared => MacosDaemonHandoverPhase::Prepared,
        MacosHandoverPhase::AutostartsConfigured => MacosDaemonHandoverPhase::AutostartsConfigured,
        MacosHandoverPhase::StopRequested => MacosDaemonHandoverPhase::StopRequested,
        MacosHandoverPhase::OutgoingOwnerStopped => MacosDaemonHandoverPhase::OutgoingOwnerStopped,
        MacosHandoverPhase::AwaitingGuardRelease => MacosDaemonHandoverPhase::AwaitingGuardRelease,
        MacosHandoverPhase::GuardReleased => MacosDaemonHandoverPhase::GuardReleased,
        MacosHandoverPhase::StartRequested => MacosDaemonHandoverPhase::StartRequested,
        MacosHandoverPhase::RequestedOwnerStarted => {
            MacosDaemonHandoverPhase::RequestedOwnerStarted
        }
        MacosHandoverPhase::CommitPending => MacosDaemonHandoverPhase::CommitPending,
        MacosHandoverPhase::Committed => MacosDaemonHandoverPhase::Committed,
        MacosHandoverPhase::RollbackPending => MacosDaemonHandoverPhase::RollbackPending,
        MacosHandoverPhase::RollbackAutostartsRestored => {
            MacosDaemonHandoverPhase::RollbackAutostartsRestored
        }
        MacosHandoverPhase::RollbackStopRequested => {
            MacosDaemonHandoverPhase::RollbackStopRequested
        }
        MacosHandoverPhase::RollbackOwnerStopped => MacosDaemonHandoverPhase::RollbackOwnerStopped,
        MacosHandoverPhase::RollbackAwaitingGuardRelease => {
            MacosDaemonHandoverPhase::RollbackAwaitingGuardRelease
        }
        MacosHandoverPhase::RollbackGuardReleased => {
            MacosDaemonHandoverPhase::RollbackGuardReleased
        }
        MacosHandoverPhase::RollbackStartRequested => {
            MacosDaemonHandoverPhase::RollbackStartRequested
        }
        MacosHandoverPhase::PriorOwnerStarted => MacosDaemonHandoverPhase::PriorOwnerStarted,
        MacosHandoverPhase::RollbackCommitPending => {
            MacosDaemonHandoverPhase::RollbackCommitPending
        }
        MacosHandoverPhase::RolledBack => MacosDaemonHandoverPhase::RolledBack,
    }
}

pub(crate) fn macos_selection_state(selection: &CoreMacosSelectionState) -> MacosSelectionState {
    match selection {
        CoreMacosSelectionState::None => MacosSelectionState::None,
        CoreMacosSelectionState::Display { source_id } => MacosSelectionState::Display {
            source_id: source_id.to_string(),
        },
        CoreMacosSelectionState::SessionScoped { content_style } => {
            MacosSelectionState::SessionScoped {
                content_style: content_style.to_string(),
            }
        }
    }
}

pub(crate) fn macos_tahoe_selection_capabilities(
    capabilities: &CoreMacosTahoeSelectionCapabilities,
    include_private_selection_ids: bool,
) -> MacosTahoeSelectionCapabilities {
    MacosTahoeSelectionCapabilities {
        source_id: if include_private_selection_ids
            || !capabilities.source_id.starts_with("macos:session:")
        {
            capabilities.source_id.to_string()
        } else {
            "session_scoped".to_owned()
        },
        capture_session_generation: capabilities.capture_session_generation,
        hdr_capture: capabilities.hdr_capture,
        dual_range_screenshots: capabilities.dual_range_screenshots,
    }
}

fn macos_tahoe_capabilities(capabilities: &CoreMacosTahoeCapabilities) -> MacosTahoeCapabilities {
    MacosTahoeCapabilities {
        host_architecture: macos_architecture(capabilities.host_architecture),
        translated_process: capabilities.translated_process,
        content_tone_mapping_info: capabilities.content_tone_mapping_info,
        metal4: capabilities.metal4,
    }
}

const fn macos_architecture(architecture: CoreMacosArchitecture) -> MacosArchitecture {
    match architecture {
        CoreMacosArchitecture::AppleSilicon => MacosArchitecture::AppleSilicon,
        CoreMacosArchitecture::Intel => MacosArchitecture::Intel,
    }
}

fn input_source_issue_status(issue: &SourceIssue) -> InputSourceIssueStatus {
    InputSourceIssueStatus {
        code: issue.code.to_string(),
        message: issue.message.to_string(),
        remediation: issue.remediation.as_ref().map(ToString::to_string),
        retryable: issue.retryable,
    }
}

fn format_input_diagnostic_detail(
    issue: Option<&InputSourceIssueStatus>,
    fallback: &str,
) -> String {
    let Some(issue) = issue else {
        return fallback.to_owned();
    };
    issue.remediation.as_ref().map_or_else(
        || format!("{}: {}", issue.code, issue.message),
        |remediation| format!("{}: {}; {}", issue.code, issue.message, remediation),
    )
}

fn is_host_interaction_source(source: &SourceStatus) -> bool {
    source.kind == SourceKind::Interaction && source.backend.as_ref() != "browser"
}

const fn source_kind_name(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Audio => "audio",
        SourceKind::Screen => "screen",
        SourceKind::Interaction => "interaction",
        SourceKind::Media => "media",
        SourceKind::Network => "network",
    }
}

const fn source_state_name(state: SourceState) -> &'static str {
    match state {
        SourceState::Stopped => "stopped",
        SourceState::Starting => "starting",
        SourceState::Live => "live",
        SourceState::Degraded => "degraded",
        SourceState::Unavailable => "unavailable",
        SourceState::Failed => "failed",
    }
}

const fn source_freshness_name(freshness: SourceFreshness) -> &'static str {
    match freshness {
        SourceFreshness::NotApplicable => "not_applicable",
        SourceFreshness::AwaitingSample => "awaiting_sample",
        SourceFreshness::Fresh => "fresh",
        SourceFreshness::Stale => "stale",
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
