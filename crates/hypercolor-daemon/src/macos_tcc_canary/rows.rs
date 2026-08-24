#[cfg(feature = "screen-capture")]
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(feature = "screen-capture")]
use hypercolor_macos_capture::{
    MacosCaptureCadence, MacosCaptureSelection, MacosCaptureSelector, MacosFrameEvent,
    MacosProtectedSourceState, MacosScreenCaptureSession, MacosStreamRequest,
};
#[cfg(feature = "screen-capture")]
use hypercolor_macos_input::{
    MacosInputConfig, MacosInputError, MacosInputPublicationOutcome, MacosInputSession,
    MacosWorkerState, input_monitoring_granted, request_input_monitoring,
};

use super::{
    model::{
        MacosTccCanaryCapability, MacosTccCanaryLifecyclePhase, MacosTccCanaryOutcome,
        MacosTccCanaryRequest,
    },
    receipts::MacosTccCanaryCapabilityEvidence,
};

#[cfg(feature = "screen-capture")]
pub(super) fn execute_capabilities(
    request: &MacosTccCanaryRequest,
) -> Vec<MacosTccCanaryCapabilityEvidence> {
    let mut evidence = Vec::with_capacity(request.capabilities.len());
    for capability in &request.capabilities {
        match capability {
            MacosTccCanaryCapability::Keyboard => {
                evidence.push(execute_input_capability(request, true));
            }
            MacosTccCanaryCapability::Pointer => {
                evidence.push(execute_input_capability(request, false));
            }
            MacosTccCanaryCapability::Picker => {}
            MacosTccCanaryCapability::Stream => {}
        }
    }
    if request
        .capabilities
        .contains(&MacosTccCanaryCapability::Picker)
    {
        let (picker, stream) = execute_screen_capabilities(request);
        evidence.push(picker);
        if let Some(stream) = stream {
            evidence.push(stream);
        }
    }
    evidence
}

#[cfg(feature = "screen-capture")]
fn execute_input_capability(
    request: &MacosTccCanaryRequest,
    keyboard: bool,
) -> MacosTccCanaryCapabilityEvidence {
    let capability = if keyboard {
        MacosTccCanaryCapability::Keyboard
    } else {
        MacosTccCanaryCapability::Pointer
    };
    let preflight_before = keyboard.then(input_monitoring_granted);
    let request_result = (keyboard && request.allow_input_prompt).then(request_input_monitoring);
    let event_count = Arc::new(AtomicU64::new(0));
    let callback_count = Arc::clone(&event_count);
    let clock_started = Instant::now();
    let session = MacosInputSession::start(
        MacosInputConfig {
            keyboard,
            pointer: !keyboard,
            clock: Arc::new(move || {
                u64::try_from(clock_started.elapsed().as_millis()).unwrap_or(u64::MAX)
            }),
        },
        move |batch| {
            callback_count.fetch_add(
                u64::try_from(batch.events.len()).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
            MacosInputPublicationOutcome::Published
        },
    );
    let mut session = match session {
        Ok(session) => session,
        Err(error) => {
            let outcome = match error {
                MacosInputError::PermissionDenied if request_result == Some(true) => {
                    MacosTccCanaryOutcome::NeedsProcessRestart
                }
                MacosInputError::PermissionDenied => MacosTccCanaryOutcome::Denied,
                _ => MacosTccCanaryOutcome::Failed,
            };
            return MacosTccCanaryCapabilityEvidence {
                capability,
                outcome,
                resulting_api_state: resulting_api_state(capability, outcome).to_owned(),
                typed_error: Some(input_error_code(&error).to_owned()),
                tcc_preflight_before: preflight_before,
                tcc_request_result: request_result,
                tcc_preflight_after: keyboard.then(input_monitoring_granted),
                requested_tap_mask: None,
                tap_mask: None,
                tap_created: Some(false),
                tap_enabled: Some(false),
                run_loop_started: Some(false),
                redacted_event_count: Some(0),
                picker_presented: None,
                picker_selected: None,
                stream_started: None,
                first_complete_frame: None,
                first_frame_monotonic_ns: None,
                resource_live_before_revocation: None,
                resource_failed_after_revocation: None,
            };
        }
    };
    let requested_masks = session.effective_masks();
    let requested_tap_mask = if keyboard {
        requested_masks.keyboard
    } else {
        requested_masks.pointer
    };
    let installed_masks = match session.installed_masks() {
        Ok(masks) => masks,
        Err(error) => {
            session.stop();
            return MacosTccCanaryCapabilityEvidence {
                capability,
                outcome: MacosTccCanaryOutcome::Failed,
                resulting_api_state: resulting_api_state(capability, MacosTccCanaryOutcome::Failed)
                    .to_owned(),
                typed_error: Some(input_error_code(&error).to_owned()),
                tcc_preflight_before: preflight_before,
                tcc_request_result: request_result,
                tcc_preflight_after: keyboard.then(input_monitoring_granted),
                requested_tap_mask: Some(requested_tap_mask),
                tap_mask: None,
                tap_created: Some(true),
                tap_enabled: Some(true),
                run_loop_started: Some(true),
                redacted_event_count: Some(0),
                picker_presented: None,
                picker_selected: None,
                stream_started: None,
                first_complete_frame: None,
                first_frame_monotonic_ns: None,
                resource_live_before_revocation: None,
                resource_failed_after_revocation: None,
            };
        }
    };
    let tap_mask = if keyboard {
        installed_masks.keyboard
    } else {
        installed_masks.pointer
    };
    if tap_mask != requested_tap_mask {
        session.stop();
        let outcome = if keyboard && (request_result == Some(true) || input_monitoring_granted()) {
            MacosTccCanaryOutcome::NeedsProcessRestart
        } else {
            MacosTccCanaryOutcome::Failed
        };
        return MacosTccCanaryCapabilityEvidence {
            capability,
            outcome,
            resulting_api_state: resulting_api_state(capability, outcome).to_owned(),
            typed_error: Some("installed_tap_mask_incomplete".to_owned()),
            tcc_preflight_before: preflight_before,
            tcc_request_result: request_result,
            tcc_preflight_after: keyboard.then(input_monitoring_granted),
            requested_tap_mask: Some(requested_tap_mask),
            tap_mask: Some(tap_mask),
            tap_created: Some(true),
            tap_enabled: Some(true),
            run_loop_started: Some(true),
            redacted_event_count: Some(0),
            picker_presented: None,
            picker_selected: None,
            stream_started: None,
            first_complete_frame: None,
            first_frame_monotonic_ns: None,
            resource_live_before_revocation: None,
            resource_failed_after_revocation: None,
        };
    }
    let deadline = Instant::now() + request.timeout();
    let mut live_before_revocation = false;
    let outcome = loop {
        let count = event_count.load(Ordering::Relaxed);
        live_before_revocation |= count > 0;
        match session.worker_state() {
            MacosWorkerState::PermissionRevoked => break MacosTccCanaryOutcome::Revoked,
            MacosWorkerState::Failed(_) => break MacosTccCanaryOutcome::Failed,
            MacosWorkerState::Running | MacosWorkerState::Degraded(_) => {}
        }
        if request.lifecycle_phase != MacosTccCanaryLifecyclePhase::RevokeWhileLive && count > 0 {
            break MacosTccCanaryOutcome::Passed;
        }
        if Instant::now() >= deadline {
            break MacosTccCanaryOutcome::TimedOut;
        }
        thread::park_timeout(Duration::from_millis(10));
    };
    session.stop();
    let final_count = event_count.load(Ordering::Relaxed);
    MacosTccCanaryCapabilityEvidence {
        capability,
        outcome,
        resulting_api_state: resulting_api_state(capability, outcome).to_owned(),
        typed_error: (outcome == MacosTccCanaryOutcome::Failed)
            .then(|| "input_worker_failed".to_owned()),
        tcc_preflight_before: preflight_before,
        tcc_request_result: request_result,
        tcc_preflight_after: keyboard.then(input_monitoring_granted),
        requested_tap_mask: Some(requested_tap_mask),
        tap_mask: Some(tap_mask),
        tap_created: Some(true),
        tap_enabled: Some(true),
        run_loop_started: Some(true),
        redacted_event_count: Some(final_count),
        picker_presented: None,
        picker_selected: None,
        stream_started: None,
        first_complete_frame: None,
        first_frame_monotonic_ns: None,
        resource_live_before_revocation: (request.lifecycle_phase
            == MacosTccCanaryLifecyclePhase::RevokeWhileLive)
            .then_some(live_before_revocation),
        resource_failed_after_revocation: (request.lifecycle_phase
            == MacosTccCanaryLifecyclePhase::RevokeWhileLive)
            .then_some(outcome == MacosTccCanaryOutcome::Revoked),
    }
}

#[cfg(feature = "screen-capture")]
fn execute_screen_capabilities(
    request: &MacosTccCanaryRequest,
) -> (
    MacosTccCanaryCapabilityEvidence,
    Option<MacosTccCanaryCapabilityEvidence>,
) {
    let deadline = Instant::now() + request.timeout();
    let stream_requested = request
        .capabilities
        .contains(&MacosTccCanaryCapability::Stream);
    let preflight_before = MacosScreenCaptureSession::screen_authorized();
    let stream_request = MacosStreamRequest::new(MacosCaptureCadence::NativeRefresh, true)
        .expect("native refresh is a valid canary cadence");
    let authorization_session =
        MacosScreenCaptureSession::new(stream_request, MacosCaptureSelector::SessionScoped);
    let Ok(authorization_session) = authorization_session else {
        let picker = failed_screen_evidence(
            MacosTccCanaryCapability::Picker,
            preflight_before,
            "capture_session_start_failed",
        );
        let stream = stream_requested.then(|| {
            failed_screen_evidence(
                MacosTccCanaryCapability::Stream,
                preflight_before,
                "capture_session_start_failed",
            )
        });
        return (picker, stream);
    };
    let request_result = request
        .allow_screen_prompt
        .then(|| authorization_session.request_authorization())
        .map(|state| {
            !matches!(
                state,
                MacosProtectedSourceState::PermissionDenied
                    | MacosProtectedSourceState::NeedsUserAction
            )
        });
    let preflight_after_request = MacosScreenCaptureSession::screen_authorized();
    if stream_requested && request_result == Some(true) && preflight_after_request {
        let diagnostic = authorization_session.begin_post_authorization_stream_diagnostic();
        let outcome = diagnostic
            .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
            .and_then(|transaction| {
                transaction.wait_until(deadline).map_err(|error| {
                    if matches!(
                        error,
                        hypercolor_macos_capture::MacosNativeTransactionError::TimedOut { .. }
                    ) {
                        mpsc::RecvTimeoutError::Timeout
                    } else {
                        mpsc::RecvTimeoutError::Disconnected
                    }
                })
            });
        match outcome {
            Ok(MacosProtectedSourceState::ReadyIdle) => {}
            Ok(MacosProtectedSourceState::NeedsProcessRestart) => {
                authorization_session.stop();
                return post_authorization_restart_evidence(
                    preflight_before,
                    request_result,
                    preflight_after_request,
                );
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                authorization_session.stop();
                return post_authorization_failure_evidence(
                    preflight_before,
                    request_result,
                    preflight_after_request,
                    MacosTccCanaryOutcome::Failed,
                    "post_authorization_stream_diagnostic_failed",
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                authorization_session.stop();
                return post_authorization_failure_evidence(
                    preflight_before,
                    request_result,
                    preflight_after_request,
                    MacosTccCanaryOutcome::TimedOut,
                    "post_authorization_stream_diagnostic_timed_out",
                );
            }
        }
    }
    authorization_session.stop();
    drop(authorization_session);
    let session =
        MacosScreenCaptureSession::new(stream_request, MacosCaptureSelector::SessionScoped);
    let Ok(session) = session else {
        let picker = failed_screen_evidence(
            MacosTccCanaryCapability::Picker,
            preflight_after_request,
            "capture_session_restart_failed",
        );
        let stream = stream_requested.then(|| {
            failed_screen_evidence(
                MacosTccCanaryCapability::Stream,
                preflight_after_request,
                "capture_session_restart_failed",
            )
        });
        return (picker, stream);
    };
    if stream_requested {
        session.set_capture_active(true);
    }
    let present_result = session.present_picker();
    if present_result.is_err() {
        session.stop();
        let outcome = if !preflight_after_request && request_result != Some(true) {
            MacosTccCanaryOutcome::Denied
        } else {
            MacosTccCanaryOutcome::Failed
        };
        let picker = MacosTccCanaryCapabilityEvidence {
            capability: MacosTccCanaryCapability::Picker,
            outcome,
            resulting_api_state: resulting_api_state(MacosTccCanaryCapability::Picker, outcome)
                .to_owned(),
            typed_error: Some("picker_presentation_failed".to_owned()),
            tcc_preflight_before: Some(preflight_before),
            tcc_request_result: request_result,
            tcc_preflight_after: Some(preflight_after_request),
            requested_tap_mask: None,
            tap_mask: None,
            tap_created: None,
            tap_enabled: None,
            run_loop_started: None,
            redacted_event_count: None,
            picker_presented: Some(false),
            picker_selected: Some(false),
            stream_started: None,
            first_complete_frame: None,
            first_frame_monotonic_ns: None,
            resource_live_before_revocation: None,
            resource_failed_after_revocation: None,
        };
        let stream = stream_requested.then(|| MacosTccCanaryCapabilityEvidence {
            capability: MacosTccCanaryCapability::Stream,
            resulting_api_state: resulting_api_state(MacosTccCanaryCapability::Stream, outcome)
                .to_owned(),
            ..picker.clone()
        });
        return (picker, stream);
    }

    let started = Instant::now();
    let mailbox = session.mailbox();
    let mut selected = false;
    let mut first_frame_monotonic_ns = None;
    let mut live_before_revocation = false;
    let mut revocation_preflight_observed = false;
    let mut resource_failed_after_revocation = false;
    loop {
        selected |= !matches!(session.selection(), MacosCaptureSelection::None);
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(50));
        if stream_requested && let Some(delivery) = mailbox.wait_latest(wait) {
            match delivery {
                Ok(MacosFrameEvent::Frame(_)) => {
                    selected = true;
                    live_before_revocation = true;
                    first_frame_monotonic_ns =
                        Some(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
                    if request.lifecycle_phase != MacosTccCanaryLifecyclePhase::RevokeWhileLive {
                        break;
                    }
                }
                Ok(MacosFrameEvent::Lifecycle(_)) | Ok(MacosFrameEvent::RecoverableError(_)) => {}
                Err(_) if revocation_preflight_observed => {
                    resource_failed_after_revocation = true;
                    break;
                }
                Err(_) => {}
            }
        } else if !stream_requested {
            thread::park_timeout(wait);
        }
        if live_before_revocation && !MacosScreenCaptureSession::screen_authorized() {
            revocation_preflight_observed = true;
            resource_failed_after_revocation |= matches!(
                session.status(),
                MacosProtectedSourceState::PermissionDenied
                    | MacosProtectedSourceState::Revoked
                    | MacosProtectedSourceState::Interrupted
                    | MacosProtectedSourceState::Failed
            );
            if resource_failed_after_revocation {
                break;
            }
        }
        if Instant::now() >= deadline || (!stream_requested && selected) {
            break;
        }
    }
    session.stop();
    let preflight_after = MacosScreenCaptureSession::screen_authorized();
    let picker_outcome = if selected {
        MacosTccCanaryOutcome::Passed
    } else if session.status() == MacosProtectedSourceState::NeedsSelection {
        MacosTccCanaryOutcome::Cancelled
    } else {
        MacosTccCanaryOutcome::TimedOut
    };
    let picker = MacosTccCanaryCapabilityEvidence {
        capability: MacosTccCanaryCapability::Picker,
        outcome: picker_outcome,
        resulting_api_state: resulting_api_state(MacosTccCanaryCapability::Picker, picker_outcome)
            .to_owned(),
        typed_error: (picker_outcome != MacosTccCanaryOutcome::Passed)
            .then(|| "picker_did_not_select".to_owned()),
        tcc_preflight_before: Some(preflight_before),
        tcc_request_result: request_result,
        tcc_preflight_after: Some(preflight_after),
        requested_tap_mask: None,
        tap_mask: None,
        tap_created: None,
        tap_enabled: None,
        run_loop_started: None,
        redacted_event_count: None,
        picker_presented: Some(true),
        picker_selected: Some(selected),
        stream_started: stream_requested.then_some(selected),
        first_complete_frame: None,
        first_frame_monotonic_ns: None,
        resource_live_before_revocation: None,
        resource_failed_after_revocation: None,
    };
    let stream = stream_requested.then(|| {
        let revoked = live_before_revocation
            && revocation_preflight_observed
            && resource_failed_after_revocation;
        let outcome = if revoked {
            MacosTccCanaryOutcome::Revoked
        } else if first_frame_monotonic_ns.is_some() {
            MacosTccCanaryOutcome::Passed
        } else {
            MacosTccCanaryOutcome::TimedOut
        };
        MacosTccCanaryCapabilityEvidence {
            capability: MacosTccCanaryCapability::Stream,
            outcome,
            resulting_api_state: resulting_api_state(MacosTccCanaryCapability::Stream, outcome)
                .to_owned(),
            typed_error: (outcome != MacosTccCanaryOutcome::Passed
                && outcome != MacosTccCanaryOutcome::Revoked)
                .then(|| "first_complete_frame_missing".to_owned()),
            tcc_preflight_before: Some(preflight_before),
            tcc_request_result: request_result,
            tcc_preflight_after: Some(preflight_after),
            requested_tap_mask: None,
            tap_mask: None,
            tap_created: None,
            tap_enabled: None,
            run_loop_started: None,
            redacted_event_count: None,
            picker_presented: Some(true),
            picker_selected: Some(selected),
            stream_started: Some(selected),
            first_complete_frame: Some(first_frame_monotonic_ns.is_some()),
            first_frame_monotonic_ns,
            resource_live_before_revocation: (request.lifecycle_phase
                == MacosTccCanaryLifecyclePhase::RevokeWhileLive)
                .then_some(live_before_revocation),
            resource_failed_after_revocation: (request.lifecycle_phase
                == MacosTccCanaryLifecyclePhase::RevokeWhileLive)
                .then_some(resource_failed_after_revocation),
        }
    });
    (picker, stream)
}

#[cfg(feature = "screen-capture")]
fn post_authorization_restart_evidence(
    preflight_before: bool,
    request_result: Option<bool>,
    preflight_after: bool,
) -> (
    MacosTccCanaryCapabilityEvidence,
    Option<MacosTccCanaryCapabilityEvidence>,
) {
    let picker = post_authorization_evidence(
        MacosTccCanaryCapability::Picker,
        MacosTccCanaryOutcome::Failed,
        preflight_before,
        request_result,
        preflight_after,
        "stream_restart_required_before_picker",
    );
    let stream = post_authorization_evidence(
        MacosTccCanaryCapability::Stream,
        MacosTccCanaryOutcome::NeedsProcessRestart,
        preflight_before,
        request_result,
        preflight_after,
        "post_authorization_stream_requires_restart",
    );
    (picker, Some(stream))
}

#[cfg(feature = "screen-capture")]
fn post_authorization_failure_evidence(
    preflight_before: bool,
    request_result: Option<bool>,
    preflight_after: bool,
    outcome: MacosTccCanaryOutcome,
    typed_error: &str,
) -> (
    MacosTccCanaryCapabilityEvidence,
    Option<MacosTccCanaryCapabilityEvidence>,
) {
    let picker = post_authorization_evidence(
        MacosTccCanaryCapability::Picker,
        MacosTccCanaryOutcome::Failed,
        preflight_before,
        request_result,
        preflight_after,
        typed_error,
    );
    let stream = post_authorization_evidence(
        MacosTccCanaryCapability::Stream,
        outcome,
        preflight_before,
        request_result,
        preflight_after,
        typed_error,
    );
    (picker, Some(stream))
}

#[cfg(feature = "screen-capture")]
fn post_authorization_evidence(
    capability: MacosTccCanaryCapability,
    outcome: MacosTccCanaryOutcome,
    preflight_before: bool,
    request_result: Option<bool>,
    preflight_after: bool,
    typed_error: &str,
) -> MacosTccCanaryCapabilityEvidence {
    MacosTccCanaryCapabilityEvidence {
        capability,
        outcome,
        resulting_api_state: resulting_api_state(capability, outcome).to_owned(),
        typed_error: Some(typed_error.to_owned()),
        tcc_preflight_before: Some(preflight_before),
        tcc_request_result: request_result,
        tcc_preflight_after: Some(preflight_after),
        requested_tap_mask: None,
        tap_mask: None,
        tap_created: None,
        tap_enabled: None,
        run_loop_started: None,
        redacted_event_count: None,
        picker_presented: Some(false),
        picker_selected: Some(false),
        stream_started: (capability == MacosTccCanaryCapability::Stream).then_some(false),
        first_complete_frame: (capability == MacosTccCanaryCapability::Stream).then_some(false),
        first_frame_monotonic_ns: None,
        resource_live_before_revocation: None,
        resource_failed_after_revocation: None,
    }
}

#[cfg(feature = "screen-capture")]
fn failed_screen_evidence(
    capability: MacosTccCanaryCapability,
    preflight: bool,
    typed_error: &str,
) -> MacosTccCanaryCapabilityEvidence {
    MacosTccCanaryCapabilityEvidence {
        capability,
        outcome: if preflight {
            MacosTccCanaryOutcome::Failed
        } else {
            MacosTccCanaryOutcome::Denied
        },
        resulting_api_state: resulting_api_state(
            capability,
            if preflight {
                MacosTccCanaryOutcome::Failed
            } else {
                MacosTccCanaryOutcome::Denied
            },
        )
        .to_owned(),
        typed_error: Some(typed_error.to_owned()),
        tcc_preflight_before: Some(preflight),
        tcc_request_result: None,
        tcc_preflight_after: Some(MacosScreenCaptureSession::screen_authorized()),
        requested_tap_mask: None,
        tap_mask: None,
        tap_created: None,
        tap_enabled: None,
        run_loop_started: None,
        redacted_event_count: None,
        picker_presented: Some(false),
        picker_selected: Some(false),
        stream_started: (capability == MacosTccCanaryCapability::Stream).then_some(false),
        first_complete_frame: (capability == MacosTccCanaryCapability::Stream).then_some(false),
        first_frame_monotonic_ns: None,
        resource_live_before_revocation: None,
        resource_failed_after_revocation: None,
    }
}

#[cfg(feature = "screen-capture")]
fn input_error_code(error: &MacosInputError) -> &'static str {
    match error {
        MacosInputError::UnsupportedPlatform => "unsupported_platform",
        MacosInputError::NothingToCapture => "nothing_to_capture",
        MacosInputError::PermissionDenied => "permission_denied",
        MacosInputError::InvalidVirtualDesktop => "invalid_virtual_desktop",
        MacosInputError::DisplayTopology(_) => "display_topology_failed",
        MacosInputError::NoActiveDisplays => "no_active_displays",
        MacosInputError::WorkerSpawn(_) => "worker_spawn_failed",
        MacosInputError::WorkerReadyTimeout => "worker_ready_timeout",
        MacosInputError::TapCreation(_) => "tap_creation_failed",
        MacosInputError::RunLoopSource(_) => "run_loop_source_failed",
        MacosInputError::TapInspection(_) => "tap_inspection_failed",
        MacosInputError::AuditToken(_) => "audit_token_failed",
    }
}

pub(super) const fn resulting_api_state(
    capability: MacosTccCanaryCapability,
    outcome: MacosTccCanaryOutcome,
) -> &'static str {
    match outcome {
        MacosTccCanaryOutcome::Passed => match capability {
            MacosTccCanaryCapability::Picker => "ready_idle",
            MacosTccCanaryCapability::Keyboard
            | MacosTccCanaryCapability::Pointer
            | MacosTccCanaryCapability::Stream => "live",
        },
        MacosTccCanaryOutcome::Denied => "permission_denied",
        MacosTccCanaryOutcome::Revoked => "revoked",
        MacosTccCanaryOutcome::NeedsProcessRestart => "needs_process_restart",
        MacosTccCanaryOutcome::Cancelled => "needs_selection",
        MacosTccCanaryOutcome::TimedOut => "interrupted",
        MacosTccCanaryOutcome::Failed => "failed",
    }
}
