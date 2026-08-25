//! Pure normalization for Core Graphics and AppKit scalar fields.

use std::sync::Arc;

use hypercolor_types::event::{PointerScrollPhase, PointerScrollUnit};
use hypercolor_types::host_input::{
    HostInputEvent, HostKeyIdentity, HostKeySignal, HostPointerButton, HostPointerMotion,
    HostPointerSnapshot, HostRepeatEvidence, host_key_name_from_macos,
    host_media_key_name_from_macos,
};

use crate::shared::{EffectiveEventMasks, MacosModifierFlags, MacosVirtualDesktop};

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

/// Normalize an AppKit subtype-8 packed media-key payload.
#[must_use]
pub fn decode_media_key(subtype: i16, data1: i64) -> Option<HostInputEvent> {
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
    normalize_media_key_event(nx_key_type, pressed, flags & 1 != 0)
}

/// Decode a mouse-button event type and native button number.
#[must_use]
pub fn decode_button_event(
    event_type: u32,
    button_number: u16,
) -> Option<(HostPointerButton, bool)> {
    match event_type {
        EVENT_LEFT_MOUSE_DOWN => Some((HostPointerButton::left(), true)),
        EVENT_LEFT_MOUSE_UP => Some((HostPointerButton::left(), false)),
        EVENT_RIGHT_MOUSE_DOWN => Some((HostPointerButton::right(), true)),
        EVENT_RIGHT_MOUSE_UP => Some((HostPointerButton::right(), false)),
        EVENT_OTHER_MOUSE_DOWN | EVENT_OTHER_MOUSE_UP => {
            let button = if button_number == 2 {
                HostPointerButton::middle()
            } else {
                HostPointerButton::new(format!(
                    "button{}",
                    u32::from(button_number).saturating_add(1)
                ))
            };
            Some((button, event_type == EVENT_OTHER_MOUSE_DOWN))
        }
        _ => None,
    }
}

/// Decode `kCGScrollWheelEventScrollPhase`.
#[must_use]
pub const fn decode_scroll_phase(raw: i64) -> Option<PointerScrollPhase> {
    match raw {
        0 => Some(PointerScrollPhase::None),
        1 => Some(PointerScrollPhase::Began),
        2 => Some(PointerScrollPhase::Changed),
        4 => Some(PointerScrollPhase::Ended),
        8 => Some(PointerScrollPhase::Cancelled),
        128 => Some(PointerScrollPhase::MayBegin),
        _ => None,
    }
}

/// Decode `kCGScrollWheelEventMomentumPhase`.
#[must_use]
pub const fn decode_momentum_phase(raw: i64) -> Option<PointerScrollPhase> {
    match raw {
        0 => Some(PointerScrollPhase::None),
        1 => Some(PointerScrollPhase::Began),
        2 => Some(PointerScrollPhase::Changed),
        3 => Some(PointerScrollPhase::Ended),
        _ => None,
    }
}

/// Normalize one direct keyboard edge.
#[must_use]
pub fn normalize_key_event(
    virtual_keycode: u16,
    pressed: bool,
    autorepeat: bool,
) -> Option<HostInputEvent> {
    let key = host_key_name_from_macos(virtual_keycode)?;
    Some(HostInputEvent::Key {
        device: None,
        identity: key_identity(key, format!("macos:key:{virtual_keycode:02x}")),
        signal: HostKeySignal::Edge {
            pressed,
            repeat: if autorepeat {
                HostRepeatEvidence::Repeat
            } else {
                HostRepeatEvidence::NotRepeat
            },
        },
    })
}

/// Normalize aggregate modifier flags without resolving held state.
#[must_use]
pub fn normalize_modifier_event(
    virtual_keycode: u16,
    flags: MacosModifierFlags,
) -> Option<HostInputEvent> {
    let (key, mask, counterpart) = modifier_key(virtual_keycode)?;
    Some(HostInputEvent::Key {
        device: None,
        identity: key_identity(key, format!("macos:key:{virtual_keycode:02x}")),
        signal: HostKeySignal::AggregateState {
            active: flags.contains(mask),
            active_counterpart: counterpart.map(Arc::from),
        },
    })
}

/// Normalize one media-key edge.
#[must_use]
pub fn normalize_media_key_event(
    nx_key_type: u16,
    pressed: bool,
    repeat: bool,
) -> Option<HostInputEvent> {
    let key = host_media_key_name_from_macos(nx_key_type)?;
    Some(HostInputEvent::Key {
        device: None,
        identity: key_identity(key, format!("macos:nx:{nx_key_type}")),
        signal: HostKeySignal::Edge {
            pressed,
            repeat: if repeat {
                HostRepeatEvidence::Repeat
            } else {
                HostRepeatEvidence::NotRepeat
            },
        },
    })
}

/// Normalize a Core Graphics button report.
#[must_use]
pub fn normalize_button_event(button: HostPointerButton, pressed: bool) -> HostInputEvent {
    let physical_code = Arc::from(format!("macos:button:{}", button.as_str()));
    HostInputEvent::Button {
        device: None,
        button,
        pressed,
        physical_code,
    }
}

/// Normalize a global cursor point and retain its coordinate-space identity.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "normalized coordinates are bounded before conversion to the shared f32 representation"
)]
pub fn normalize_motion_event(
    desktop: MacosVirtualDesktop,
    x: f64,
    y: f64,
) -> (HostInputEvent, HostPointerSnapshot) {
    let (norm_x, norm_y) = desktop.normalize(x, y);
    let norm_x = norm_x as f32;
    let norm_y = norm_y as f32;
    let generation = desktop.topology_generation;
    (
        HostInputEvent::Motion {
            device: None,
            motion: HostPointerMotion::Absolute {
                norm_x,
                norm_y,
                coordinate_space_generation: generation,
            },
        },
        HostPointerSnapshot {
            x,
            y,
            norm_x,
            norm_y,
            coordinate_space_generation: generation,
        },
    )
}

/// Normalize one exact Q16.16 wheel report.
#[must_use]
pub fn normalize_scroll_event(
    fixed_delta_x: i64,
    fixed_delta_y: i64,
    continuous: bool,
    phase: PointerScrollPhase,
    momentum_phase: PointerScrollPhase,
) -> HostInputEvent {
    let (delta_x_q16_16, delta_y_q16_16, unit) = if continuous {
        (fixed_delta_x, fixed_delta_y, PointerScrollUnit::Pixels)
    } else {
        (
            scale_line120(fixed_delta_x),
            scale_line120(fixed_delta_y),
            PointerScrollUnit::Line120,
        )
    };
    HostInputEvent::Scroll {
        device: None,
        delta_x_q16_16,
        delta_y_q16_16,
        unit,
        phase,
        momentum_phase,
        physical_code: Arc::from("macos:scroll"),
    }
}

fn key_identity(key: &'static str, physical_code: String) -> HostKeyIdentity {
    HostKeyIdentity {
        key: Arc::from(key),
        physical_code: Arc::from(physical_code),
    }
}

fn modifier_key(
    virtual_keycode: u16,
) -> Option<(&'static str, MacosModifierFlags, Option<&'static str>)> {
    match virtual_keycode {
        0x38 => Some(("ShiftLeft", MacosModifierFlags::SHIFT, Some("ShiftRight"))),
        0x3c => Some(("ShiftRight", MacosModifierFlags::SHIFT, Some("ShiftLeft"))),
        0x3b => Some((
            "ControlLeft",
            MacosModifierFlags::CONTROL,
            Some("ControlRight"),
        )),
        0x3e => Some((
            "ControlRight",
            MacosModifierFlags::CONTROL,
            Some("ControlLeft"),
        )),
        0x3a => Some(("AltLeft", MacosModifierFlags::ALTERNATE, Some("AltRight"))),
        0x3d => Some(("AltRight", MacosModifierFlags::ALTERNATE, Some("AltLeft"))),
        0x37 => Some(("MetaLeft", MacosModifierFlags::COMMAND, Some("MetaRight"))),
        0x36 => Some(("MetaRight", MacosModifierFlags::COMMAND, Some("MetaLeft"))),
        0x39 => Some(("CapsLock", MacosModifierFlags::ALPHA_SHIFT, None)),
        _ => None,
    }
}

fn scale_line120(value: i64) -> i64 {
    value.checked_mul(120).unwrap_or_else(|| {
        if value.is_negative() {
            i64::MIN
        } else {
            i64::MAX
        }
    })
}
