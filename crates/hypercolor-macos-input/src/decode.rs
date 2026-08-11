//! Pure decoding for Core Graphics and AppKit scalar fields.

use crate::shared::{EffectiveEventMasks, MacosMediaKey, MacosPointerButton, MacosScrollPhase};

pub const NX_SUBTYPE_AUX_CONTROL_BUTTONS: i16 = 8;

const EVENT_LEFT_MOUSE_DOWN: u32 = 1;
const EVENT_LEFT_MOUSE_UP: u32 = 2;
const EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
const EVENT_RIGHT_MOUSE_UP: u32 = 4;
const EVENT_MOUSE_MOVED: u32 = 5;
const EVENT_LEFT_MOUSE_DRAGGED: u32 = 6;
const EVENT_RIGHT_MOUSE_DRAGGED: u32 = 7;
const EVENT_KEY_DOWN: u32 = 10;
const EVENT_KEY_UP: u32 = 11;
const EVENT_FLAGS_CHANGED: u32 = 12;
const EVENT_SYSTEM_DEFINED: u32 = 14;
const EVENT_SCROLL_WHEEL: u32 = 22;
const EVENT_OTHER_MOUSE_DOWN: u32 = 25;
const EVENT_OTHER_MOUSE_UP: u32 = 26;
const EVENT_OTHER_MOUSE_DRAGGED: u32 = 27;

const fn event_mask(event_type: u32) -> u64 {
    1_u64 << event_type
}

/// Build independent keyboard and pointer masks from requested capabilities.
#[must_use]
pub const fn event_masks(keyboard: bool, pointer: bool) -> EffectiveEventMasks {
    let keyboard_mask = if keyboard {
        event_mask(EVENT_KEY_DOWN)
            | event_mask(EVENT_KEY_UP)
            | event_mask(EVENT_FLAGS_CHANGED)
            | event_mask(EVENT_SYSTEM_DEFINED)
    } else {
        0
    };
    let pointer_mask = if pointer {
        event_mask(EVENT_MOUSE_MOVED)
            | event_mask(EVENT_LEFT_MOUSE_DRAGGED)
            | event_mask(EVENT_RIGHT_MOUSE_DRAGGED)
            | event_mask(EVENT_OTHER_MOUSE_DRAGGED)
            | event_mask(EVENT_LEFT_MOUSE_DOWN)
            | event_mask(EVENT_LEFT_MOUSE_UP)
            | event_mask(EVENT_RIGHT_MOUSE_DOWN)
            | event_mask(EVENT_RIGHT_MOUSE_UP)
            | event_mask(EVENT_OTHER_MOUSE_DOWN)
            | event_mask(EVENT_OTHER_MOUSE_UP)
            | event_mask(EVENT_SCROLL_WHEEL)
    } else {
        0
    };
    EffectiveEventMasks {
        keyboard: keyboard_mask,
        pointer: pointer_mask,
    }
}

/// Decode an AppKit subtype-8 packed media-key payload.
#[must_use]
pub fn decode_media_key(subtype: i16, data1: i64) -> Option<MacosMediaKey> {
    if subtype != NX_SUBTYPE_AUX_CONTROL_BUTTONS {
        return None;
    }
    let packed = u32::try_from(data1).ok()?;
    let nx_key_type = u16::try_from(packed >> 16).ok()?;
    let flags = u16::try_from(packed & 0xffff).ok()?;
    let state = u8::try_from(flags >> 8).ok()?;
    let pressed = match state {
        0x0a => true,
        0x0b => false,
        _ => return None,
    };
    Some(MacosMediaKey {
        nx_key_type,
        pressed,
        repeat: flags & 1 != 0,
    })
}

/// Decode a mouse-button event type and native button number.
#[must_use]
pub const fn decode_button_event(
    event_type: u32,
    button_number: u16,
) -> Option<(MacosPointerButton, bool)> {
    match event_type {
        EVENT_LEFT_MOUSE_DOWN => Some((MacosPointerButton::Left, true)),
        EVENT_LEFT_MOUSE_UP => Some((MacosPointerButton::Left, false)),
        EVENT_RIGHT_MOUSE_DOWN => Some((MacosPointerButton::Right, true)),
        EVENT_RIGHT_MOUSE_UP => Some((MacosPointerButton::Right, false)),
        EVENT_OTHER_MOUSE_DOWN | EVENT_OTHER_MOUSE_UP => {
            let button = if button_number == 2 {
                MacosPointerButton::Middle
            } else {
                MacosPointerButton::Other(button_number)
            };
            Some((button, event_type == EVENT_OTHER_MOUSE_DOWN))
        }
        _ => None,
    }
}

/// Decode `kCGScrollWheelEventScrollPhase`.
#[must_use]
pub const fn decode_scroll_phase(raw: i64) -> Option<MacosScrollPhase> {
    match raw {
        0 => Some(MacosScrollPhase::None),
        1 => Some(MacosScrollPhase::Began),
        2 => Some(MacosScrollPhase::Changed),
        4 => Some(MacosScrollPhase::Ended),
        8 => Some(MacosScrollPhase::Cancelled),
        128 => Some(MacosScrollPhase::MayBegin),
        _ => None,
    }
}

/// Decode `kCGScrollWheelEventMomentumPhase`.
#[must_use]
pub const fn decode_momentum_phase(raw: i64) -> Option<MacosScrollPhase> {
    match raw {
        0 => Some(MacosScrollPhase::None),
        1 => Some(MacosScrollPhase::Began),
        2 => Some(MacosScrollPhase::Changed),
        3 => Some(MacosScrollPhase::Ended),
        _ => None,
    }
}
