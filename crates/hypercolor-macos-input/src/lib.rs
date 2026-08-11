//! macOS host input capture vocabulary and native event decoding.
//!
//! Core Graphics and Core Foundation ownership stays inside this crate. The
//! public boundary contains only plain Rust values so canonical input folding
//! remains portable and deterministic in `hypercolor-core`.

mod decode;
mod queue;
mod shared;

pub use decode::{
    NX_SUBTYPE_AUX_CONTROL_BUTTONS, decode_button_event, decode_media_key, decode_momentum_phase,
    decode_scroll_phase, event_masks,
};
pub use shared::{
    EffectiveEventMasks, MacosInputBatch, MacosInputConfig, MacosInputDiagnostics, MacosInputError,
    MacosInputEvent, MacosInputGapReason, MacosInputResult, MacosMediaKey, MacosModifierFlags,
    MacosPointerButton, MacosScrollPhase, MacosScrollUnit, MacosVirtualDesktop, MacosWorkerState,
};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    MacosInputSession, current_virtual_desktop, input_monitoring_granted, request_input_monitoring,
};

#[cfg(not(target_os = "macos"))]
mod stubs;
#[cfg(not(target_os = "macos"))]
pub use stubs::{
    MacosInputSession, current_virtual_desktop, input_monitoring_granted, request_input_monitoring,
};
