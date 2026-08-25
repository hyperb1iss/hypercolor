//! Transport-independent input health projection.

use std::time::{Duration, Instant};

use hypercolor_core::input::{SourceFreshness, SourceIssue, SourceKind, SourceState, SourceStatus};
use hypercolor_types::api::system::{
    InputSourceIssueStatus, InputSourceStatus, InputStatus, MacosCapabilityOwner,
    MacosDaemonHandoverPhase, MacosDaemonOwnerConflictStatus,
    MacosDaemonOwnerRecoveryRequiredStatus, MacosDaemonOwnershipStatus,
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
    _include_private_selection_ids: bool,
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
        action_issue: source.action_issue.as_ref().map(input_source_issue_status),
        diagnostics: source.diagnostics.as_deref().cloned(),
        lifecycle_issue,
        freshness_issue,
        retired: source.retired,
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
        SourceKind::Sensors => "sensors",
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
