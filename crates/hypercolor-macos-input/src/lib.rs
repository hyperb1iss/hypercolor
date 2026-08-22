//! macOS host input capture vocabulary and native event decoding.
//!
//! Core Graphics and Core Foundation ownership stays inside this crate. The
//! public boundary contains only plain Rust values so canonical input folding
//! remains portable and deterministic in `hypercolor-core`.

mod decode;
mod process;
mod queue;
mod shared;

pub use decode::{
    NX_SUBTYPE_AUX_CONTROL_BUTTONS, decode_button_event, decode_media_key, decode_momentum_phase,
    decode_scroll_phase, event_masks, normalize_button_event, normalize_key_event,
    normalize_media_key_event, normalize_modifier_event, normalize_motion_event,
    normalize_scroll_event,
};
#[doc(hidden)]
pub use hypercolor_worker_retention::retention_service_identity as worker_retention_service_identity;
pub use process::current_process_audit_token_identity;
pub use shared::{
    EffectiveEventMasks, MacosArchitecture, MacosAuthorizationState, MacosCapabilityOwner,
    MacosDaemonOwnerConflict, MacosInputConfig, MacosInputDiagnostics, MacosInputError,
    MacosInputGapReason, MacosInputPublicationOutcome, MacosInputResult, MacosInputStatusSnapshot,
    MacosModifierFlags, MacosProtectedSourceState, MacosVirtualDesktop, MacosWorkerDegradation,
    MacosWorkerState, input_diagnostics_envelope,
};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    MacosInputSession, current_virtual_desktop, input_monitoring_granted, request_input_monitoring,
    secure_event_input_enabled,
};

#[cfg(not(target_os = "macos"))]
mod stubs;
#[cfg(not(target_os = "macos"))]
pub use stubs::{
    MacosInputSession, current_virtual_desktop, input_monitoring_granted, request_input_monitoring,
    secure_event_input_enabled,
};
