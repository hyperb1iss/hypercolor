//! System endpoints for daemon identity, status, sensors, and health.
//!
//! Provides daemon status overview and a lightweight health check
//! for monitoring and load balancer probes.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use axum::extract::{Extension, State};
use axum::response::{IntoResponse, Response};
use cpal::traits::{DeviceTrait, HostTrait};
use hypercolor_core::bus::PreviewKind;
use hypercolor_core::config::canonical_audio_device_id;
use hypercolor_core::engine::RenderLoopState;
#[cfg(target_os = "linux")]
use hypercolor_core::input::audio::linux;
use hypercolor_core::input::screen::{
    PixelExtent, ScreenAnalysisComputeCapacity, ScreenAnalysisResourcePlan, ScreenAnalysisWorkPlan,
};
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
    AudioDeviceInfo, AudioDevicesResponse, EffectHealthStatus, FullFrameCopySessionStatus,
    GpuCompositorProbeStatus, HealthChecks, HealthResponse, InputSourceIssueStatus,
    InputSourcePlatformStatus, InputSourceStatus, InputStatus, LatencyHistogramBucketStatus,
    LatencyHistogramStatus, LatencyPercentilesStatus, LatestFrameStatus, MacosArchitecture,
    MacosAuthorizationState, MacosCapabilityOwner, MacosDaemonHandoverPhase,
    MacosDaemonOwnerConflictStatus, MacosDaemonOwnerRecoveryRequiredStatus,
    MacosDaemonOwnershipStatus, MacosFrameDrop, MacosInputTelemetry, MacosProtectedSourceState,
    MacosScreenTelemetry, MacosScreenTiming, MacosSelectionState, MacosTahoeCapabilities,
    MacosTahoeSelectionCapabilities, MacosTiming, PreviewDemandStatus, PreviewRuntimeStatus,
    RenderAccelerationStatus, RenderLoopStatus, RenderSurfaceStatus, ScreenCaptureCapacityStatus,
    ServerInfo, SessionPerformanceStatus, SystemResource, SystemStatus,
};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::sensor::SystemSnapshot;
use tracing::{debug, warn};

use crate::api::envelope;
use crate::api::security::RequestAuthContext;
use crate::app_state::AppState;
use crate::macos_owner::{MacosDaemonOwner, MacosHandoverPhase, MacosOwnerSnapshot};
use crate::performance::LatestFrameMetrics;
use crate::preview_runtime::{PreviewDemandSummary, PreviewRuntime};
use crate::session::current_global_brightness;

use hypercolor_core::config::ConfigManager;

const DEFAULT_CONFIG_FILE_NAME: &str = "hypercolor.toml";
const MULTI_ZONE_CAPABILITIES: &[&str] = &[
    "multi-zone-sampling",
    "zone-crud",
    "zone-device-assignment",
    "zone-layout-edit",
    "zone-preview-frames",
    "scene-unassigned-behavior-write",
];

// ── Response Types ───────────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) struct InputDiagnostic {
    pub source_id: String,
    pub status: &'static str,
    pub detail: String,
}

/// Build the redacted input health snapshot used without protected control.
#[must_use]
pub(crate) fn input_status_snapshot(state: &AppState) -> InputStatus {
    input_status_snapshot_with_privacy(state, false)
}

fn input_status_snapshot_with_privacy(
    state: &AppState,
    include_private_selection_ids: bool,
) -> InputStatus {
    let now = Instant::now();
    let registry = state.input_status.snapshot();
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
        enabled: state
            .config_manager
            .as_ref()
            .is_some_and(|manager| manager.get().input.enabled),
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

fn input_source_status(
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

fn macos_daemon_ownership(snapshot: &MacosOwnerSnapshot) -> MacosDaemonOwnershipStatus {
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

fn macos_selection_state(selection: &CoreMacosSelectionState) -> MacosSelectionState {
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

fn macos_tahoe_selection_capabilities(
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

// ── Handlers ─────────────────────────────────────────────────────────────

/// Build a full status response for trusted in-process callers.
pub async fn get_status(State(state): State<Arc<AppState>>) -> Response {
    envelope::ok(system_status_with_privacy(state, true).await)
}

async fn system_status_with_privacy(
    state: Arc<AppState>,
    include_private_selection_ids: bool,
) -> SystemStatus {
    let device_count = state.device_registry.len().await;
    let effect_count = state.domains.effects.len().await;
    let scene_count = state.scene_manager.snapshot().await.scene_count();
    let subscribers = state.event_bus.subscriber_count();

    // Query the live effect engine for the active effect name.
    let active_effect = crate::api::effects::active_primary_effect(state.as_ref())
        .await
        .map(|(_, effect)| effect.name);
    let (active_scene, active_scene_snapshot_locked) = {
        let scene_manager = state.scene_manager.snapshot().await;
        scene_manager.active_scene().map_or((None, false), |scene| {
            (Some(scene.name.clone()), scene.blocks_runtime_mutation())
        })
    };

    let (performance, input_time_histogram) = {
        let performance = state.performance.read().await;
        (
            performance.snapshot(),
            performance.input_time_histogram_snapshot(),
        )
    };

    // Query the live render loop for timing data.
    let render_loop_status = {
        let rl = state.render_loop.read().await;
        let snapshot = rl.stats();
        let capacity_fps = if snapshot.state == RenderLoopState::Running {
            round_1(paced_fps(
                snapshot.avg_frame_time.as_secs_f64(),
                snapshot.tier.fps(),
            ))
        } else {
            0.0
        };
        RenderLoopStatus {
            state: snapshot.state.to_string(),
            fps_tier: snapshot.tier.to_string(),
            target_fps: snapshot.tier.fps(),
            ceiling_fps: snapshot.max_tier.fps(),
            capacity_fps,
            delivered_fps: if snapshot.state == RenderLoopState::Running {
                round_1(performance.delivered_fps)
            } else {
                0.0
            },
            actual_fps: capacity_fps,
            consecutive_misses: snapshot.consecutive_misses,
            total_frames: snapshot.total_frames,
        }
    };
    let running = render_loop_is_operational(render_loop_status.state.as_str());
    let latest_frame = if render_loop_status.state == "running" {
        performance.latest_frame.as_ref().map(|frame| {
            latest_frame_status(frame, state.start_time.elapsed().as_secs_f64() * 1000.0)
        })
    } else {
        None
    };
    let session_performance = SessionPerformanceStatus {
        input_stage: LatencyPercentilesStatus {
            sample_count: performance.input_time_sample_count,
            avg_ms: round_2(performance.input_time.avg_ms),
            p95_ms: round_2(performance.input_time.p95_ms),
            p99_ms: round_2(performance.input_time.p99_ms),
            max_ms: round_2(performance.input_time.max_ms),
            cumulative_histogram: Some(LatencyHistogramStatus {
                bucket_width_us: input_time_histogram.bucket_width_us,
                overflow_bucket_index: input_time_histogram.overflow_bucket_index,
                snapshot_frame_token: performance
                    .latest_frame
                    .as_ref()
                    .map(|frame| frame.timeline.frame_token),
                buckets: input_time_histogram
                    .buckets
                    .into_iter()
                    .map(|bucket| LatencyHistogramBucketStatus {
                        bucket_index: bucket.bucket_index,
                        count: bucket.count,
                    })
                    .collect(),
            }),
        },
        full_frame_cpu_copies: FullFrameCopySessionStatus {
            count: performance.full_frame_copy_count_total,
            frames: performance.full_frame_copy_frames_total,
            bytes: performance.full_frame_copy_bytes_total,
        },
    };
    let servo_health = servo_effect_health_counts();
    let pipeline_health = render_pipeline_health_counts();
    let effect_health = EffectHealthStatus {
        errors_total: performance.effect_health.errors_total,
        fallbacks_applied_total: performance.effect_health.fallbacks_applied_total,
        producer_gpu_readback_failures_total: performance
            .effect_health
            .producer_gpu_readback_failures_total,
        servo_soft_stalls_total: servo_health.soft_stalls_total,
        servo_breaker_opens_total: servo_health.breaker_opens_total,
        servo_session_creates_total: servo_health.session_creates_total,
        servo_session_create_failures_total: servo_health.session_create_failures_total,
        servo_session_create_wait_total_ms: us_to_ms_f64(servo_health.session_create_wait_total_us),
        servo_session_create_wait_max_ms: us_to_ms_f64(servo_health.session_create_wait_max_us),
        servo_page_loads_total: servo_health.page_loads_total,
        servo_page_load_failures_total: servo_health.page_load_failures_total,
        servo_page_load_wait_total_ms: us_to_ms_f64(servo_health.page_load_wait_total_us),
        servo_page_load_wait_max_ms: us_to_ms_f64(servo_health.page_load_wait_max_us),
        servo_detached_destroys_total: servo_health.detached_destroys_total,
        servo_detached_destroy_failures_total: servo_health.detached_destroy_failures_total,
        servo_render_requests_total: servo_health.render_requests_total,
        servo_render_queue_wait_total_ms: us_to_ms_f64(servo_health.render_queue_wait_total_us),
        servo_render_queue_wait_max_ms: us_to_ms_f64(servo_health.render_queue_wait_max_us),
        servo_render_scene_requests_total: servo_health.render_scene_requests_total,
        servo_render_scene_queue_wait_total_ms: us_to_ms_f64(
            servo_health.render_scene_queue_wait_total_us,
        ),
        servo_render_scene_queue_wait_max_ms: us_to_ms_f64(
            servo_health.render_scene_queue_wait_max_us,
        ),
        servo_render_display_requests_total: servo_health.render_display_requests_total,
        servo_render_display_queue_wait_total_ms: us_to_ms_f64(
            servo_health.render_display_queue_wait_total_us,
        ),
        servo_render_display_queue_wait_max_ms: us_to_ms_f64(
            servo_health.render_display_queue_wait_max_us,
        ),
        servo_render_cpu_frames_total: servo_health.render_cpu_frames_total,
        servo_render_cached_frames_total: servo_health.render_cached_frames_total,
        servo_render_gpu_frames_total: servo_health.render_gpu_frames_total,
        servo_gpu_import_failures_total: servo_health.render_gpu_import_failures_total,
        servo_gpu_import_fallbacks_total: servo_health.render_gpu_import_fallbacks_total,
        servo_gpu_import_fallback_reason: servo_health
            .render_gpu_import_fallback_reason
            .map(str::to_owned),
        servo_gpu_import_windows_sync_mode: servo_health
            .render_gpu_import_windows_sync_mode
            .map(str::to_owned),
        servo_gpu_import_stale_frame_total: servo_health.render_gpu_import_stale_frame_total,
        servo_gpu_import_adapter_mismatch_total: servo_health
            .render_gpu_import_adapter_mismatch_total,
        servo_gpu_import_slot_count: servo_health.render_gpu_import_slot_count,
        servo_gpu_import_pending_slots: servo_health.render_gpu_import_pending_slots,
        servo_gpu_import_pending_slots_max: servo_health.render_gpu_import_pending_slots_max,
        servo_gpu_import_completed_slots: servo_health.render_gpu_import_completed_slots,
        servo_gpu_import_available_slots: servo_health.render_gpu_import_available_slots,
        servo_gpu_import_available_slots_min: servo_health.render_gpu_import_available_slots_min,
        servo_gpu_import_oldest_pending_age_max_ms: us_to_ms_f64(
            servo_health.render_gpu_import_oldest_pending_age_max_us,
        ),
        servo_gpu_import_blit_total_ms: us_to_ms_f64(servo_health.render_gpu_import_blit_total_us),
        servo_gpu_import_blit_max_ms: us_to_ms_f64(servo_health.render_gpu_import_blit_max_us),
        servo_gpu_import_sync_total_ms: us_to_ms_f64(servo_health.render_gpu_import_sync_total_us),
        servo_gpu_import_sync_max_ms: us_to_ms_f64(servo_health.render_gpu_import_sync_max_us),
        servo_gpu_import_total_ms: us_to_ms_f64(servo_health.render_gpu_import_total_us),
        servo_gpu_import_max_ms: us_to_ms_f64(servo_health.render_gpu_import_max_us),
        producer_cpu_frames_total: pipeline_health.cpu_producer_frames,
        producer_gpu_frames_total: pipeline_health.gpu_producer_frames,
        producer_gpu_cpu_materialization_blocked_total: pipeline_health
            .gpu_cpu_materialization_blocked_total,
        sparkleflinger_gpu_source_upload_skipped_total: pipeline_health.skipped_gpu_source_uploads,
        sparkleflinger_media_texture_allocations_total: pipeline_health
            .media_texture_allocations_total,
        sparkleflinger_media_texture_upload_bytes_total: pipeline_health
            .media_texture_upload_bytes_total,
        sparkleflinger_display_finalize_rgba_attempts_total: pipeline_health
            .display_finalize_rgba_attempts_total,
        sparkleflinger_display_finalize_yuv_attempts_total: pipeline_health
            .display_finalize_yuv_attempts_total,
        sparkleflinger_display_finalize_successes_total: pipeline_health
            .display_finalize_successes_total,
        sparkleflinger_display_finalize_misses_total: pipeline_health.display_finalize_misses_total,
        sparkleflinger_display_finalize_latches_total: pipeline_health
            .display_finalize_latches_total,
        sparkleflinger_display_finalize_blocking_wait_total_ms: us_to_ms_f64(
            pipeline_health.display_finalize_blocking_wait_total_us,
        ),
        sparkleflinger_display_finalize_blocking_wait_max_ms: us_to_ms_f64(
            pipeline_health.display_finalize_blocking_wait_max_us,
        ),
        sparkleflinger_display_finalize_surface_reallocs_total: pipeline_health
            .display_finalize_surface_reallocs_total,
        servo_render_evaluate_scripts_total_ms: us_to_ms_f64(
            servo_health.render_evaluate_scripts_total_us,
        ),
        servo_render_evaluate_scripts_max_ms: us_to_ms_f64(
            servo_health.render_evaluate_scripts_max_us,
        ),
        servo_render_event_loop_total_ms: us_to_ms_f64(servo_health.render_event_loop_total_us),
        servo_render_event_loop_max_ms: us_to_ms_f64(servo_health.render_event_loop_max_us),
        servo_render_paint_total_ms: us_to_ms_f64(servo_health.render_paint_total_us),
        servo_render_paint_max_ms: us_to_ms_f64(servo_health.render_paint_max_us),
        servo_render_readback_total_ms: us_to_ms_f64(servo_health.render_readback_total_us),
        servo_render_readback_max_ms: us_to_ms_f64(servo_health.render_readback_max_us),
        servo_render_frame_total_ms: us_to_ms_f64(servo_health.render_frame_total_us),
        servo_render_frame_max_ms: us_to_ms_f64(servo_health.render_frame_max_us),
    };
    let preview_runtime = preview_runtime_status(&state.preview_runtime);

    let input_status = input_status_snapshot_with_privacy(&state, include_private_selection_ids);
    let audio_available = input_status.sources.iter().any(|source| {
        source.kind == "audio"
            && !source.retired
            && !matches!(source.state.as_str(), "unavailable" | "failed")
    });
    let screen_capture_capacity = {
        let capacity_snapshot = state.screen_capacity_status.snapshot();
        let policy = capacity_snapshot.policy();
        if policy.capacity_enforced() {
            let resource_snapshot = capacity_snapshot.physical();
            let resource_capacity = resource_snapshot.capacity();
            let total_capacity = policy.total_capacity();
            let publication_capacity = policy.publication_capacity();
            let demand = policy.capture_demand();
            let analysis = policy.analysis_resource_plan();
            let analysis_work = policy.analysis_work_plan();
            let analysis_compute = policy.analysis_compute_capacity();
            let extent = demand.requested_extent();
            ScreenCaptureCapacityStatus {
                admission_enforced: true,
                physical_transition_byte_capacity: Some(resource_capacity.byte_budget()),
                physical_transition_backend_capacity: Some(resource_capacity.backend_capacity()),
                physical_reserved_bytes: Some(resource_snapshot.reserved_bytes()),
                physical_available_bytes: Some(resource_snapshot.available_bytes()),
                steady_total_byte_budget: Some(total_capacity.byte_budget()),
                steady_total_backend_capacity: Some(total_capacity.backend_capacity()),
                steady_publication_byte_budget: Some(publication_capacity.byte_budget()),
                transition_publication_backend_capacity: Some(
                    publication_capacity.backend_capacity(),
                ),
                analysis_width: extent.map(PixelExtent::width),
                analysis_height: extent.map(PixelExtent::height),
                analysis_retained_bytes: analysis.map(ScreenAnalysisResourcePlan::retained_bytes),
                analysis_peak_bytes: analysis.map(ScreenAnalysisResourcePlan::peak_bytes),
                analysis_weighted_work_units_per_frame: analysis_work
                    .map(ScreenAnalysisWorkPlan::weighted_work_units_per_frame),
                analysis_weighted_work_units_per_second: analysis_work
                    .map(ScreenAnalysisWorkPlan::weighted_work_units_per_second),
                analysis_parallel_capacity_per_second: analysis_compute.and_then(
                    ScreenAnalysisComputeCapacity::total_parallel_weighted_work_units_per_second,
                ),
                analysis_serial_capacity_per_second: analysis_compute
                    .map(|capacity| capacity.serial_weighted_work_units_per_second().get()),
                analysis_worker_count: analysis_compute
                    .and_then(|capacity| u64::try_from(capacity.worker_count().get()).ok()),
            }
        } else {
            ScreenCaptureCapacityStatus::without_capacity(false)
        }
    };

    let uptime_seconds = state.start_time.elapsed().as_secs();
    let config_path = config_path(&state).display().to_string();
    let data_dir = ConfigManager::data_dir().display().to_string();
    let cache_dir = ConfigManager::cache_dir().display().to_string();
    let macos_daemon_ownership = state
        .macos_daemon_ownership
        .load_full()
        .as_deref()
        .map(macos_daemon_ownership);

    SystemStatus {
        running,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        config_path,
        data_dir,
        cache_dir,
        uptime_seconds,
        device_count,
        effect_count,
        scene_count,
        active_effect,
        active_scene,
        active_scene_snapshot_locked,
        global_brightness: brightness_percent(current_global_brightness(&state.power_state)),
        audio_available,
        capture_available: capture_input_available(),
        screen_capture_capacity,
        input: input_status,
        macos_daemon_ownership,
        compositor_acceleration: render_acceleration_status(&state.render_acceleration),
        render_loop: render_loop_status,
        session_performance,
        latest_frame,
        effect_health,
        preview_runtime,
        event_bus_subscribers: subscribers,
        capabilities: MULTI_ZONE_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_owned())
            .collect(),
    }
}

/// `GET /api/v1/system` -- Public identity with authorized daemon status.
pub(crate) async fn get_system(
    State(state): State<Arc<AppState>>,
    Extension(auth_context): Extension<RequestAuthContext>,
) -> Response {
    let identity = server_info(&state).await;
    let status = if auth_context.can_read_system_status() {
        Some(
            system_status_with_privacy(Arc::clone(&state), auth_context.can_protected_control())
                .await,
        )
    } else {
        None
    };

    envelope::ok(SystemResource { identity, status })
}

/// `GET /api/v1/system/sensors` — Latest system sensor snapshot.
pub async fn get_sensors(State(state): State<Arc<AppState>>) -> Response {
    envelope::ok(latest_sensor_snapshot(&state).await.as_ref().clone())
}

async fn server_info(state: &AppState) -> ServerInfo {
    ServerInfo {
        instance_id: state.server_identity.instance_id.clone(),
        instance_name: state.server_identity.instance_name.clone(),
        version: state.server_identity.version.clone(),
        server_session_id: state.server_session_id.clone(),
        device_count: state.device_registry.len().await,
        auth_required: state.security_state.security_enabled(),
    }
}

/// `GET /health` — Lightweight health check (no envelope).
pub async fn health_check(State(state): State<Arc<AppState>>) -> Response {
    let uptime_seconds = state.start_time.elapsed().as_secs();
    let render_loop = {
        let render_loop = state.render_loop.read().await;
        render_loop_health(render_loop.stats().state).to_owned()
    };
    let device_count = state.device_registry.len().await;
    let device_backends = {
        let backend_manager = state.backend_manager.lock().await;
        backend_health(backend_manager.backend_count(), device_count).to_owned()
    };
    let event_bus = event_bus_health(&state.event_bus).to_owned();
    let checks = HealthChecks {
        render_loop,
        device_backends,
        event_bus,
    };

    let health = overall_health(&checks);
    let resp = HealthResponse {
        status: health.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        uptime_seconds,
        checks,
    };

    let status = match health {
        "healthy" => axum::http::StatusCode::OK,
        _ => axum::http::StatusCode::SERVICE_UNAVAILABLE,
    };

    (status, axum::Json(resp)).into_response()
}

fn config_path(state: &AppState) -> PathBuf {
    state.config_manager.as_ref().map_or_else(
        || ConfigManager::config_dir().join(DEFAULT_CONFIG_FILE_NAME),
        |manager| manager.path().to_path_buf(),
    )
}

async fn latest_sensor_snapshot(state: &AppState) -> Arc<SystemSnapshot> {
    let input_manager = state.input_manager.lock().await;
    input_manager
        .latest_sensor_snapshot()
        .unwrap_or_else(|| Arc::new(SystemSnapshot::empty()))
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "brightness is clamped to 0-100 percent before narrowing to a byte"
)]
fn brightness_percent(brightness: f32) -> u8 {
    let scaled = (brightness.clamp(0.0, 1.0) * 100.0).round();
    if scaled <= 0.0 {
        0
    } else if scaled >= 100.0 {
        100
    } else {
        scaled as u8
    }
}

fn render_loop_health(state: RenderLoopState) -> &'static str {
    match state {
        RenderLoopState::Running => "ok",
        RenderLoopState::Created | RenderLoopState::Paused => "idle",
        RenderLoopState::Stopped => "degraded",
    }
}

fn backend_health(backend_count: usize, device_count: usize) -> &'static str {
    if backend_count == 0 && device_count > 0 {
        "degraded"
    } else if backend_count == 0 {
        "idle"
    } else {
        "ok"
    }
}

fn event_bus_health(bus: &hypercolor_core::bus::HypercolorBus) -> &'static str {
    if bus.subscriber_count() == 0
        && bus.frame_receiver_count() == 0
        && bus.spectrum_receiver_count() == 0
        && bus.canvas_receiver_count() == 0
    {
        "idle"
    } else {
        "ok"
    }
}

fn overall_health(checks: &HealthChecks) -> &'static str {
    if [
        checks.render_loop.as_str(),
        checks.device_backends.as_str(),
        checks.event_bus.as_str(),
    ]
    .contains(&"degraded")
    {
        "degraded"
    } else {
        "healthy"
    }
}

fn render_loop_is_operational(state: &str) -> bool {
    state != "stopped"
}

fn render_acceleration_status(
    resolution: &crate::startup::CompositorAccelerationResolution,
) -> RenderAccelerationStatus {
    RenderAccelerationStatus {
        requested_mode: render_acceleration_mode_name(resolution.requested_mode).to_owned(),
        effective_mode: render_acceleration_mode_name(resolution.effective_mode).to_owned(),
        fallback_reason: resolution.fallback_reason.map(str::to_owned),
        servo_gpu_import_mode: servo_gpu_import_mode_name().to_owned(),
        servo_gpu_import_attempting: servo_gpu_import_attempting(),
        gpu_probe: resolution
            .gpu_probe
            .as_ref()
            .map(|probe| GpuCompositorProbeStatus {
                adapter_name: probe.adapter_name.clone(),
                adapter_device_type: probe.adapter_device_type.to_owned(),
                backend: probe.backend.to_owned(),
                texture_format: probe.texture_format.to_owned(),
                max_texture_dimension_2d: probe.max_texture_dimension_2d,
                max_storage_textures_per_shader_stage: probe.max_storage_textures_per_shader_stage,
                software_adapter_reason: probe.software_adapter_reason.map(str::to_owned),
                servo_gpu_import_backend_compatible: probe.servo_gpu_import_backend_compatible,
                servo_gpu_import_backend_reason: probe
                    .servo_gpu_import_backend_reason
                    .map(str::to_owned),
                linux_servo_gpu_import_backend_compatible: probe
                    .linux_servo_gpu_import_backend_compatible,
                linux_servo_gpu_import_backend_reason: probe
                    .linux_servo_gpu_import_backend_reason
                    .map(str::to_owned),
            }),
    }
}

#[cfg(feature = "servo-gpu-import")]
fn servo_gpu_import_mode_name() -> &'static str {
    match hypercolor_core::effect::servo_gpu_import_mode() {
        hypercolor_types::config::ServoGpuImportMode::Off => "off",
        hypercolor_types::config::ServoGpuImportMode::Auto => "auto",
        hypercolor_types::config::ServoGpuImportMode::On => "on",
    }
}

#[cfg(not(feature = "servo-gpu-import"))]
const fn servo_gpu_import_mode_name() -> &'static str {
    "unavailable"
}

#[cfg(feature = "servo-gpu-import")]
fn servo_gpu_import_attempting() -> bool {
    hypercolor_core::effect::servo_gpu_import_should_attempt()
}

#[cfg(not(feature = "servo-gpu-import"))]
const fn servo_gpu_import_attempting() -> bool {
    false
}

const fn render_acceleration_mode_name(mode: RenderAccelerationMode) -> &'static str {
    match mode {
        RenderAccelerationMode::Cpu => "cpu",
        RenderAccelerationMode::Auto => "auto",
        RenderAccelerationMode::Gpu => "gpu",
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ServoEffectHealthCounts {
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
    render_scene_requests_total: u64,
    render_scene_queue_wait_total_us: u64,
    render_scene_queue_wait_max_us: u64,
    render_display_requests_total: u64,
    render_display_queue_wait_total_us: u64,
    render_display_queue_wait_max_us: u64,
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

#[cfg(feature = "servo")]
fn servo_effect_health_counts() -> ServoEffectHealthCounts {
    let snapshot = hypercolor_core::effect::servo_telemetry_snapshot();
    ServoEffectHealthCounts {
        soft_stalls_total: snapshot.soft_stalls_total,
        breaker_opens_total: snapshot.breaker_opens_total,
        session_creates_total: snapshot.session_creates_total,
        session_create_failures_total: snapshot.session_create_failures_total,
        session_create_wait_total_us: snapshot.session_create_wait_total_us,
        session_create_wait_max_us: snapshot.session_create_wait_max_us,
        page_loads_total: snapshot.page_loads_total,
        page_load_failures_total: snapshot.page_load_failures_total,
        page_load_wait_total_us: snapshot.page_load_wait_total_us,
        page_load_wait_max_us: snapshot.page_load_wait_max_us,
        detached_destroys_total: snapshot.detached_destroys_total,
        detached_destroy_failures_total: snapshot.detached_destroy_failures_total,
        render_requests_total: snapshot.render_requests_total,
        render_queue_wait_total_us: snapshot.render_queue_wait_total_us,
        render_queue_wait_max_us: snapshot.render_queue_wait_max_us,
        render_scene_requests_total: snapshot.render_scene_requests_total,
        render_scene_queue_wait_total_us: snapshot.render_scene_queue_wait_total_us,
        render_scene_queue_wait_max_us: snapshot.render_scene_queue_wait_max_us,
        render_display_requests_total: snapshot.render_display_requests_total,
        render_display_queue_wait_total_us: snapshot.render_display_queue_wait_total_us,
        render_display_queue_wait_max_us: snapshot.render_display_queue_wait_max_us,
        render_cpu_frames_total: snapshot.render_cpu_frames_total,
        render_cached_frames_total: snapshot.render_cached_frames_total,
        render_gpu_frames_total: snapshot.render_gpu_frames_total,
        render_gpu_import_failures_total: snapshot.render_gpu_import_failures_total,
        render_gpu_import_fallbacks_total: snapshot.render_gpu_import_fallbacks_total,
        render_gpu_import_fallback_reason: snapshot.render_gpu_import_fallback_reason,
        render_gpu_import_windows_sync_mode: snapshot.render_gpu_import_windows_sync_mode,
        render_gpu_import_stale_frame_total: snapshot.render_gpu_import_stale_frame_total,
        render_gpu_import_adapter_mismatch_total: snapshot.render_gpu_import_adapter_mismatch_total,
        render_gpu_import_slot_count: snapshot.render_gpu_import_slot_count,
        render_gpu_import_pending_slots: snapshot.render_gpu_import_pending_slots,
        render_gpu_import_pending_slots_max: snapshot.render_gpu_import_pending_slots_max,
        render_gpu_import_completed_slots: snapshot.render_gpu_import_completed_slots,
        render_gpu_import_available_slots: snapshot.render_gpu_import_available_slots,
        render_gpu_import_available_slots_min: snapshot.render_gpu_import_available_slots_min,
        render_gpu_import_oldest_pending_age_max_us: snapshot
            .render_gpu_import_oldest_pending_age_max_us,
        render_gpu_import_blit_total_us: snapshot.render_gpu_import_blit_total_us,
        render_gpu_import_blit_max_us: snapshot.render_gpu_import_blit_max_us,
        render_gpu_import_sync_total_us: snapshot.render_gpu_import_sync_total_us,
        render_gpu_import_sync_max_us: snapshot.render_gpu_import_sync_max_us,
        render_gpu_import_total_us: snapshot.render_gpu_import_total_us,
        render_gpu_import_max_us: snapshot.render_gpu_import_max_us,
        render_evaluate_scripts_total_us: snapshot.render_evaluate_scripts_total_us,
        render_evaluate_scripts_max_us: snapshot.render_evaluate_scripts_max_us,
        render_event_loop_total_us: snapshot.render_event_loop_total_us,
        render_event_loop_max_us: snapshot.render_event_loop_max_us,
        render_paint_total_us: snapshot.render_paint_total_us,
        render_paint_max_us: snapshot.render_paint_max_us,
        render_readback_total_us: snapshot.render_readback_total_us,
        render_readback_max_us: snapshot.render_readback_max_us,
        render_frame_total_us: snapshot.render_frame_total_us,
        render_frame_max_us: snapshot.render_frame_max_us,
    }
}

#[cfg(not(feature = "servo"))]
const fn servo_effect_health_counts() -> ServoEffectHealthCounts {
    ServoEffectHealthCounts {
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
        render_scene_requests_total: 0,
        render_scene_queue_wait_total_us: 0,
        render_scene_queue_wait_max_us: 0,
        render_display_requests_total: 0,
        render_display_queue_wait_total_us: 0,
        render_display_queue_wait_max_us: 0,
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

#[derive(Debug, Clone, Copy, Default)]
struct RenderPipelineHealthCounts {
    cpu_producer_frames: u64,
    gpu_producer_frames: u64,
    gpu_cpu_materialization_blocked_total: u64,
    skipped_gpu_source_uploads: u64,
    media_texture_allocations_total: u64,
    media_texture_upload_bytes_total: u64,
    display_finalize_rgba_attempts_total: u64,
    display_finalize_yuv_attempts_total: u64,
    display_finalize_successes_total: u64,
    display_finalize_misses_total: u64,
    display_finalize_latches_total: u64,
    display_finalize_blocking_wait_total_us: u64,
    display_finalize_blocking_wait_max_us: u64,
    display_finalize_surface_reallocs_total: u64,
}

fn render_pipeline_health_counts() -> RenderPipelineHealthCounts {
    let producer = crate::render_thread::producer_frame_counts();
    let gpu = gpu_sparkleflinger_health_counts();
    RenderPipelineHealthCounts {
        cpu_producer_frames: producer.cpu_frames,
        gpu_producer_frames: producer.gpu_frames,
        gpu_cpu_materialization_blocked_total: producer.gpu_cpu_materialization_blocked,
        skipped_gpu_source_uploads: gpu.source_upload_skipped_total,
        media_texture_allocations_total: gpu.media_texture_allocations_total,
        media_texture_upload_bytes_total: gpu.media_texture_upload_bytes_total,
        display_finalize_rgba_attempts_total: gpu.display_finalize_rgba_attempts_total,
        display_finalize_yuv_attempts_total: gpu.display_finalize_yuv_attempts_total,
        display_finalize_successes_total: gpu.display_finalize_successes_total,
        display_finalize_misses_total: gpu.display_finalize_misses_total,
        display_finalize_latches_total: gpu.display_finalize_latches_total,
        display_finalize_blocking_wait_total_us: gpu.display_finalize_blocking_wait_total_us,
        display_finalize_blocking_wait_max_us: gpu.display_finalize_blocking_wait_max_us,
        display_finalize_surface_reallocs_total: gpu.display_finalize_surface_reallocs_total,
    }
}

#[cfg(feature = "wgpu")]
fn gpu_sparkleflinger_health_counts() -> GpuSparkleFlingerHealthCounts {
    let snapshot =
        crate::render_thread::sparkleflinger::gpu::gpu_sparkleflinger_telemetry_snapshot();
    GpuSparkleFlingerHealthCounts {
        source_upload_skipped_total: snapshot.source_upload_skipped_total,
        media_texture_allocations_total: snapshot.media_texture_allocations_total,
        media_texture_upload_bytes_total: snapshot.media_texture_upload_bytes_total,
        display_finalize_rgba_attempts_total: snapshot.display_finalize_rgba_attempts_total,
        display_finalize_yuv_attempts_total: snapshot.display_finalize_yuv_attempts_total,
        display_finalize_successes_total: snapshot.display_finalize_successes_total,
        display_finalize_misses_total: snapshot.display_finalize_misses_total,
        display_finalize_latches_total: snapshot.display_finalize_latches_total,
        display_finalize_blocking_wait_total_us: snapshot.display_finalize_blocking_wait_total_us,
        display_finalize_blocking_wait_max_us: snapshot.display_finalize_blocking_wait_max_us,
        display_finalize_surface_reallocs_total: snapshot.display_finalize_surface_reallocs_total,
    }
}

#[cfg(not(feature = "wgpu"))]
const fn gpu_sparkleflinger_health_counts() -> GpuSparkleFlingerHealthCounts {
    GpuSparkleFlingerHealthCounts {
        source_upload_skipped_total: 0,
        media_texture_allocations_total: 0,
        media_texture_upload_bytes_total: 0,
        display_finalize_rgba_attempts_total: 0,
        display_finalize_yuv_attempts_total: 0,
        display_finalize_successes_total: 0,
        display_finalize_misses_total: 0,
        display_finalize_latches_total: 0,
        display_finalize_blocking_wait_total_us: 0,
        display_finalize_blocking_wait_max_us: 0,
        display_finalize_surface_reallocs_total: 0,
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct GpuSparkleFlingerHealthCounts {
    source_upload_skipped_total: u64,
    media_texture_allocations_total: u64,
    media_texture_upload_bytes_total: u64,
    display_finalize_rgba_attempts_total: u64,
    display_finalize_yuv_attempts_total: u64,
    display_finalize_successes_total: u64,
    display_finalize_misses_total: u64,
    display_finalize_latches_total: u64,
    display_finalize_blocking_wait_total_us: u64,
    display_finalize_blocking_wait_max_us: u64,
    display_finalize_surface_reallocs_total: u64,
}

fn latest_frame_status(frame: &LatestFrameMetrics, render_elapsed_ms: f64) -> LatestFrameStatus {
    let frame_age_ms = if frame.timestamp_ms > 0 {
        (render_elapsed_ms - f64::from(frame.timestamp_ms)).max(0.0)
    } else {
        0.0
    };

    LatestFrameStatus {
        frame_token: frame.timeline.frame_token,
        compositor_backend: frame.compositor_backend.as_str().to_owned(),
        output_frame_source: frame.output_frame_source.as_str().to_owned(),
        output_reuses_published_frame: frame.output_reuses_published_frame,
        output_brightness_bits: frame.output_brightness_bits,
        output_brightness_generation: frame.output_brightness_generation,
        output_routing_signature: frame.output_routing_signature,
        output_zone_shape_signature: frame.output_zone_shape_signature,
        output_unassigned_behavior_generation: frame.output_unassigned_behavior_generation,
        devices_written: frame.devices_written,
        total_leds: frame.total_leds,
        gpu_zone_sampling: frame.gpu_zone_sampling,
        gpu_sample_deferred: frame.gpu_sample_deferred,
        gpu_sample_stale: frame.gpu_sample_stale,
        gpu_sample_retry_hit: frame.gpu_sample_retry_hit,
        gpu_sample_queue_saturated: frame.gpu_sample_queue_saturated,
        gpu_sample_wait_blocked: frame.gpu_sample_wait_blocked,
        gpu_sample_cpu_fallback: frame.gpu_sample_cpu_fallback,
        preview_surface: frame.preview_surface,
        scene_canvas_forced_surface: frame.scene_canvas_forced_surface,
        cpu_readback_skipped: frame.cpu_readback_skipped,
        gpu_readback_failed: frame.gpu_readback_failed,
        total_ms: round_2(us_to_ms(frame.total_us)),
        wake_late_ms: round_2(us_to_ms(frame.wake_late_us)),
        jitter_ms: round_2(us_to_ms(frame.jitter_us)),
        frame_age_ms: round_2(frame_age_ms),
        input_sampling_ms: round_2(us_to_ms(frame.input_us)),
        producer_ms: round_2(us_to_ms(frame.producer_us)),
        producer_render_ms: round_2(us_to_ms(frame.producer_render_us)),
        producer_scene_compose_ms: round_2(us_to_ms(frame.producer_scene_compose_us)),
        composition_ms: round_2(us_to_ms(frame.composition_us)),
        effect_rendering_ms: round_2(us_to_ms(frame.render_us)),
        spatial_sampling_ms: round_2(us_to_ms(frame.sample_us)),
        device_output_ms: round_2(us_to_ms(frame.push_us)),
        preview_postprocess_ms: round_2(us_to_ms(frame.postprocess_us)),
        event_bus_ms: round_2(us_to_ms(frame.publish_us)),
        coordination_overhead_ms: round_2(us_to_ms(frame.overhead_us)),
        publish_frame_data_ms: round_2(us_to_ms(frame.publish_frame_data_us)),
        publish_group_canvas_ms: round_2(us_to_ms(frame.publish_group_canvas_us)),
        publish_preview_ms: round_2(us_to_ms(frame.publish_preview_us)),
        publish_events_ms: round_2(us_to_ms(frame.publish_events_us)),
        logical_layer_count: frame.logical_layer_count,
        render_group_count: frame.render_group_count,
        full_frame_copy_count: frame.full_frame_copy_count,
        full_frame_copy_kb: round_2(bytes_to_kib(frame.full_frame_copy_bytes)),
        producer_full_frame_copy_count: frame.producer_full_frame_copy.count,
        producer_full_frame_copy_kb: round_2(bytes_to_kib(frame.producer_full_frame_copy.bytes)),
        producer_full_frame_copy_reason: frame.producer_full_frame_copy.reason.map(str::to_owned),
        publication_full_frame_copy_count: frame.publication_full_frame_copy.count,
        publication_full_frame_copy_kb: round_2(bytes_to_kib(
            frame.publication_full_frame_copy.bytes,
        )),
        publication_full_frame_copy_reason: frame
            .publication_full_frame_copy
            .reason
            .map(str::to_owned),
        output_errors: frame.output_errors,
        render_surfaces: RenderSurfaceStatus {
            canvas_receivers: frame.canvas_receiver_count,
            scene_pool_slot_count: frame.scene_pool_slot_count,
            scene_pool_free_slots: frame.scene_pool_free_slots,
            scene_pool_published_slots: frame.scene_pool_published_slots,
            scene_pool_dequeued_slots: frame.scene_pool_dequeued_slots,
            direct_pool_slot_count: frame.direct_pool_slot_count,
            direct_pool_free_slots: frame.direct_pool_free_slots,
            direct_pool_published_slots: frame.direct_pool_published_slots,
            direct_pool_dequeued_slots: frame.direct_pool_dequeued_slots,
            preview_pool_slot_count: frame.preview_pool_slot_count,
            preview_pool_free_slots: frame.preview_pool_free_slots,
            preview_pool_published_slots: frame.preview_pool_published_slots,
            preview_pool_dequeued_slots: frame.preview_pool_dequeued_slots,
            compositor_pool_slot_count: frame.compositor_pool_slot_count,
            compositor_pool_free_slots: frame.compositor_pool_free_slots,
            compositor_pool_published_slots: frame.compositor_pool_published_slots,
            compositor_pool_dequeued_slots: frame.compositor_pool_dequeued_slots,
        },
    }
}

fn preview_runtime_status(runtime: &PreviewRuntime) -> PreviewRuntimeStatus {
    let snapshot = runtime.snapshot();
    let canvas = snapshot.preview(PreviewKind::Canvas);
    let scene_canvas = snapshot.preview(PreviewKind::SceneCanvas);
    let screen_canvas = snapshot.preview(PreviewKind::ScreenCanvas);
    let zone_preview = snapshot.zone_preview;
    PreviewRuntimeStatus {
        canvas_receivers: canvas.receivers,
        scene_canvas_receivers: scene_canvas.receivers,
        screen_canvas_receivers: screen_canvas.receivers,
        zone_preview_receivers: zone_preview.receivers,
        canvas_frames_published: canvas.frames_published,
        scene_canvas_frames_published: scene_canvas.frames_published,
        screen_canvas_frames_published: screen_canvas.frames_published,
        zone_preview_frames_published: zone_preview.frames_published,
        latest_canvas_frame_number: canvas.latest_frame_number,
        latest_scene_canvas_frame_number: scene_canvas.latest_frame_number,
        latest_screen_canvas_frame_number: screen_canvas.latest_frame_number,
        latest_zone_preview_frame_number: zone_preview.latest_frame_number,
        canvas_demand: preview_demand_status(runtime.canvas_demand()),
        scene_canvas_demand: preview_demand_status(runtime.scene_canvas_demand()),
        screen_canvas_demand: preview_demand_status(runtime.screen_canvas_demand()),
        zone_preview_demand: preview_demand_status(runtime.zone_preview_demand()),
    }
}

fn preview_demand_status(summary: PreviewDemandSummary) -> PreviewDemandStatus {
    PreviewDemandStatus {
        subscribers: summary.subscribers,
        max_fps: summary.max_fps,
        max_width: summary.max_width,
        max_height: summary.max_height,
        any_full_resolution: summary.any_full_resolution,
        any_rgb: summary.any_rgb,
        any_rgba: summary.any_rgba,
        any_jpeg: summary.any_jpeg,
    }
}

fn paced_fps(avg_frame_secs: f64, target_fps: u32) -> f64 {
    if avg_frame_secs <= 0.0 {
        return f64::from(target_fps);
    }

    (1.0 / avg_frame_secs).clamp(0.0, f64::from(target_fps))
}

fn us_to_ms(value: u32) -> f64 {
    f64::from(value) / 1000.0
}

fn us_to_ms_f64(value: u64) -> f64 {
    std::time::Duration::from_micros(value).as_secs_f64() * 1000.0
}

fn bytes_to_kib(value: u32) -> f64 {
    f64::from(value) / 1024.0
}

fn round_1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn round_2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

// ── Audio Devices ────────────────────────────────────────────────────────

/// `GET /api/v1/system/audio-devices` — Enumerate audio input devices.
pub async fn list_audio_devices(State(state): State<Arc<AppState>>) -> Response {
    let current = current_audio_device_id(&state);
    let devices = audio_device_options(&current);

    envelope::ok(AudioDevicesResponse { devices, current })
}

pub(crate) fn capture_input_available() -> bool {
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        return true;
    }
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn current_audio_device_id(state: &AppState) -> String {
    state.config_manager.as_ref().map_or_else(
        || "default".to_owned(),
        |manager| canonical_audio_device_id(&manager.get().audio.device),
    )
}

fn audio_device_options(current: &str) -> Vec<AudioDeviceInfo> {
    let mut devices = vec![
        default_audio_device(),
        microphone_audio_device(),
        disabled_audio_device(),
    ];

    match enumerate_audio_input_devices() {
        Ok(mut enumerated) => devices.append(&mut enumerated),
        Err(error) => {
            warn!(
                %error,
                "Failed to enumerate audio input devices; returning fallback settings options"
            );
        }
    }

    if should_include_current_device(current, &devices) {
        devices.push(AudioDeviceInfo {
            id: current.to_owned(),
            name: current.to_owned(),
            description: "Configured device (currently unavailable)".to_owned(),
        });
    }

    dedupe_audio_devices(&mut devices);
    devices.sort_by_cached_key(|device| {
        let rank = match device.id.as_str() {
            "default" => 0,
            "microphone" => 1,
            "none" => 2,
            _ => 3,
        };
        (rank, device.name.to_ascii_lowercase())
    });
    devices
}

fn enumerate_audio_input_devices() -> anyhow::Result<Vec<AudioDeviceInfo>> {
    #[cfg(target_os = "linux")]
    if let Ok(devices) = enumerate_linux_audio_input_devices()
        && !devices.is_empty()
    {
        return Ok(devices);
    }

    enumerate_cpal_audio_input_devices()
}

#[cfg(target_os = "linux")]
fn enumerate_linux_audio_input_devices() -> anyhow::Result<Vec<AudioDeviceInfo>> {
    Ok(linux::enumerate_named_audio_sources()?
        .into_iter()
        .map(|source| AudioDeviceInfo {
            id: source.id,
            name: source.name,
            description: source.description,
        })
        .collect())
}

fn enumerate_cpal_audio_input_devices() -> anyhow::Result<Vec<AudioDeviceInfo>> {
    let host = cpal::default_host();
    let mut devices = Vec::new();
    let mut filtered = Vec::new();

    for device in host
        .input_devices()
        .context("failed to enumerate input devices")?
    {
        let description = match device.description() {
            Ok(description) => description,
            Err(error) => {
                warn!(%error, "Skipping audio device with unreadable description");
                continue;
            }
        };

        let name = description.name().trim().to_owned();
        if name.is_empty() {
            continue;
        }

        if !should_offer_named_audio_device(&name) {
            filtered.push(name);
            continue;
        }

        devices.push(AudioDeviceInfo {
            id: name.clone(),
            name: name.clone(),
            description: name,
        });
    }

    if !filtered.is_empty() {
        debug!(
            filtered = ?filtered,
            "Filtered unsupported or synthetic audio devices from the input list"
        );
    }
    debug!(
        count = devices.len(),
        "Enumerated named audio capture devices"
    );

    Ok(devices)
}

fn default_audio_device() -> AudioDeviceInfo {
    AudioDeviceInfo {
        id: "default".to_owned(),
        name: "System Monitor".to_owned(),
        description: "Prefer the active system output monitor source".to_owned(),
    }
}

fn microphone_audio_device() -> AudioDeviceInfo {
    AudioDeviceInfo {
        id: "microphone".to_owned(),
        name: "Default Microphone".to_owned(),
        description: "Capture from the default input device".to_owned(),
    }
}

fn disabled_audio_device() -> AudioDeviceInfo {
    AudioDeviceInfo {
        id: "none".to_owned(),
        name: "Disabled".to_owned(),
        description: "Send silence to audio-reactive effects".to_owned(),
    }
}

fn should_include_current_device(current: &str, devices: &[AudioDeviceInfo]) -> bool {
    !current.trim().is_empty()
        && !devices
            .iter()
            .any(|device| device.id.eq_ignore_ascii_case(current))
}

fn dedupe_audio_devices(devices: &mut Vec<AudioDeviceInfo>) {
    let mut seen = HashSet::new();
    devices.retain(|device| seen.insert(device.id.to_ascii_lowercase()));
}

#[doc(hidden)]
pub fn should_offer_named_audio_device(name: &str) -> bool {
    let normalized = name.trim();
    !normalized.is_empty()
        && !is_monitorish_device_name(normalized)
        && !is_serverish_device_name(normalized)
}

fn is_serverish_device_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    [
        "sound server",
        "pipewire",
        "pulseaudio",
        "default alsa output",
        "default output",
        "discard all samples",
        "rate converter plugin",
        "plugin for channel",
        "plugin using speex",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn is_monitorish_device_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    ["monitor", "loopback", "what u hear", "stereo mix"]
        .iter()
        .any(|needle| normalized.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::{
        get_sensors, get_status, get_system, input_source_status, input_status_snapshot,
        macos_daemon_ownership, macos_selection_state, macos_tahoe_selection_capabilities,
        us_to_ms_f64,
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
        MacosTahoeCapabilities, MacosTahoeSelectionCapabilities, MacosTimingStatus,
        SourceFreshness, SourceKind, SourcePlatformStatus, SourceState, SourceStatus,
        SourceStatusHandle, SourceStatusReporter,
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
            let mut status = SourceStatusReporter::new(
                "test-screen",
                SourceKind::Screen,
                "test",
                true,
                true,
                false,
            );
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
            callback_to_publication_timing: Some(timing_fixture(
                990, 1_980_000, 4_000, 2_000, 3_000,
            )),
        });
        let status =
            input_source_status(&source_status_fixture(Some(platform)), Instant::now(), true);
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

        let public = serde_json::to_value(input_status_snapshot(&state))
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
                publish_group_canvas_us: 20,
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
                render_group_count: 1,
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
            .input_manager
            .lock()
            .await
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
        let servo_health = super::servo_effect_health_counts();

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
        assert_eq!(
            json["data"]["latest_frame"]["publish_group_canvas_ms"],
            0.02
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
        assert_eq!(json["data"]["effect_health"]["errors_total"], 1);
        assert_eq!(json["data"]["effect_health"]["fallbacks_applied_total"], 1);
        assert_eq!(
            json["data"]["effect_health"]["producer_gpu_readback_failures_total"],
            2
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_soft_stalls_total"],
            servo_health.soft_stalls_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_breaker_opens_total"],
            servo_health.breaker_opens_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_session_creates_total"],
            servo_health.session_creates_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_session_create_failures_total"],
            servo_health.session_create_failures_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_page_loads_total"],
            servo_health.page_loads_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_page_load_failures_total"],
            servo_health.page_load_failures_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_detached_destroys_total"],
            servo_health.detached_destroys_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_detached_destroy_failures_total"],
            servo_health.detached_destroy_failures_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_requests_total"],
            servo_health.render_requests_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_queue_wait_total_ms"],
            us_to_ms_f64(servo_health.render_queue_wait_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_queue_wait_max_ms"],
            us_to_ms_f64(servo_health.render_queue_wait_max_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_scene_requests_total"],
            servo_health.render_scene_requests_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_scene_queue_wait_total_ms"],
            us_to_ms_f64(servo_health.render_scene_queue_wait_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_scene_queue_wait_max_ms"],
            us_to_ms_f64(servo_health.render_scene_queue_wait_max_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_display_requests_total"],
            servo_health.render_display_requests_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_display_queue_wait_total_ms"],
            us_to_ms_f64(servo_health.render_display_queue_wait_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_display_queue_wait_max_ms"],
            us_to_ms_f64(servo_health.render_display_queue_wait_max_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_cpu_frames_total"],
            servo_health.render_cpu_frames_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_cached_frames_total"],
            servo_health.render_cached_frames_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_gpu_import_slot_count"],
            servo_health.render_gpu_import_slot_count
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_gpu_import_pending_slots"],
            servo_health.render_gpu_import_pending_slots
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_gpu_import_available_slots"],
            servo_health.render_gpu_import_available_slots
        );
        assert!(
            json["data"]["effect_health"]["sparkleflinger_display_finalize_blocking_wait_total_ms"]
                .is_number()
        );
        assert!(
            json["data"]["effect_health"]["sparkleflinger_media_texture_allocations_total"]
                .is_number()
        );
        assert!(
            json["data"]["effect_health"]["producer_gpu_cpu_materialization_blocked_total"]
                .is_number()
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_evaluate_scripts_total_ms"],
            us_to_ms_f64(servo_health.render_evaluate_scripts_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_evaluate_scripts_max_ms"],
            us_to_ms_f64(servo_health.render_evaluate_scripts_max_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_event_loop_total_ms"],
            us_to_ms_f64(servo_health.render_event_loop_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_event_loop_max_ms"],
            us_to_ms_f64(servo_health.render_event_loop_max_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_paint_total_ms"],
            us_to_ms_f64(servo_health.render_paint_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_paint_max_ms"],
            us_to_ms_f64(servo_health.render_paint_max_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_readback_total_ms"],
            us_to_ms_f64(servo_health.render_readback_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_readback_max_ms"],
            us_to_ms_f64(servo_health.render_readback_max_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_frame_total_ms"],
            us_to_ms_f64(servo_health.render_frame_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_frame_max_ms"],
            us_to_ms_f64(servo_health.render_frame_max_us)
        );
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

    #[tokio::test]
    async fn sensors_endpoint_returns_latest_snapshot() {
        let state = Arc::new(AppState::new());
        let snapshot = Arc::new(SystemSnapshot {
            cpu_load_percent: 51.0,
            cpu_loads: vec![48.0, 54.0],
            cpu_temp_celsius: Some(72.5),
            gpu_temp_celsius: None,
            gpu_load_percent: None,
            gpu_vram_used_mb: None,
            ram_used_percent: 44.0,
            ram_used_mb: 8192.0,
            ram_total_mb: 16384.0,
            components: vec![SensorReading::new(
                "Package id 0",
                72.5,
                SensorUnit::Celsius,
                None,
                Some(100.0),
                None,
            )],
            polled_at_ms: 1234,
        });
        let (_tx, rx) = watch::channel(snapshot);
        state
            .input_manager
            .lock()
            .await
            .set_sensor_snapshot_receiver(rx);

        let response = get_sensors(State(state)).await;
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("sensor body should read");
        let json: Value = serde_json::from_slice(&body).expect("sensor response should serialize");

        assert_eq!(json["data"]["cpu_load_percent"], 51.0);
        assert_eq!(json["data"]["cpu_temp_celsius"], 72.5);
        assert_eq!(json["data"]["polled_at_ms"], 1234);
    }
}
