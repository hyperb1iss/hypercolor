//! macOS host input capture vocabulary and native event decoding.
//!
//! Core Graphics and Core Foundation ownership stays inside this crate. The
//! public boundary contains only plain Rust values so canonical input folding
//! remains portable and deterministic in `hypercolor-core`.

mod decode;
mod shared;

pub use decode::{
    NX_SUBTYPE_AUX_CONTROL_BUTTONS, decode_button_event, decode_media_key, decode_momentum_phase,
    decode_scroll_phase, event_masks,
};
pub use shared::{
    EffectiveEventMasks, MacosInputBatch, MacosInputConfig, MacosInputError, MacosInputEvent,
    MacosInputGapReason, MacosInputResult, MacosMediaKey, MacosModifierFlags, MacosPointerButton,
    MacosScrollPhase, MacosScrollUnit, MacosVirtualDesktop, MacosWorkerState,
};
