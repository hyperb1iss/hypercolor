//! Raw Input decoding arithmetic, expressed over plain integers.
//!
//! Every subtle conversion in the Windows input path lives here rather than
//! inline in the pump: the signed wheel value hiding in a `u16`, the button
//! bitfield that can carry both edges of one click, the `RAWINPUT_ALIGN`
//! record walk, and absolute-pointer normalization. Keeping them free of
//! Windows types is what lets Linux CI test them — the pump then does nothing
//! but read struct fields and call in here.
//!
//! The constants are transcribed from `WinUser.h` (10.0.26100.0) and
//! cross-checked against the real ones by a Windows-only test, so a
//! transcription slip fails the build rather than silently mis-decoding.

use crate::shared::{RawButton, RawKeyPrefix};

/// A keyboard past its rollover limit reports this instead of a key. It is
/// not a position and must never reach the key table or the `VKey` fallback.
pub const KEYBOARD_OVERRUN_MAKE_CODE: u16 = 0xFF;

/// Scroll units per notch. Identical to evdev's `REL_WHEEL_HI_RES` unit, so
/// wheel travel passes through with no conversion.
pub const WHEEL_DELTA: i32 = 120;

/// Absolute pointer reports span this range over their chosen rect.
const ABSOLUTE_RANGE: f32 = 65535.0;

/// `RAWKEYBOARD::Flags` bits.
mod key_flags {
    pub const BREAK: u16 = 1;
    pub const E0: u16 = 2;
    pub const E1: u16 = 4;
    /// Terminal Services LED synchronization bookkeeping, not a key edge.
    pub const TERMSRV_SET_LED: u16 = 8;
    /// Terminal Services session shadowing bookkeeping, not a key edge.
    pub const TERMSRV_SHADOW: u16 = 0x10;
}

/// `RAWMOUSE::usButtonFlags` bits.
mod button_flags {
    pub const LEFT_DOWN: u32 = 0x0001;
    pub const LEFT_UP: u32 = 0x0002;
    pub const RIGHT_DOWN: u32 = 0x0004;
    pub const RIGHT_UP: u32 = 0x0008;
    pub const MIDDLE_DOWN: u32 = 0x0010;
    pub const MIDDLE_UP: u32 = 0x0020;
    pub const BUTTON_4_DOWN: u32 = 0x0040;
    pub const BUTTON_4_UP: u32 = 0x0080;
    pub const BUTTON_5_DOWN: u32 = 0x0100;
    pub const BUTTON_5_UP: u32 = 0x0200;
    pub const WHEEL: u32 = 0x0400;
    pub const HWHEEL: u32 = 0x0800;
}

/// `RAWMOUSE::usFlags` bits.
mod mouse_flags {
    pub const MOVE_ABSOLUTE: u16 = 0x01;
    pub const VIRTUAL_DESKTOP: u16 = 0x02;
}

/// Record stride: the SDK aligns the *address* `ptr + dwSize`, which is
/// equivalent to aligning the offset only because our buffer base is
/// 8-aligned. `Vec<u64>` backing is what makes that true.
const RECORD_ALIGN: usize = 8;

/// What a key report is, before the name table is consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyReport {
    /// A real key edge at a physical position.
    Edge { prefix: RawKeyPrefix, pressed: bool },
    /// Rollover overrun: the keyboard's own view of held state is now
    /// unreliable, so the decoder emits a `StateGap` in place of this record.
    Overrun,
    /// Terminal Services bookkeeping, carrying no key edge at all.
    Ignored,
}

/// Classify one `RAWKEYBOARD` report from its make code and flags.
#[must_use]
pub const fn classify_key(make_code: u16, flags: u16) -> KeyReport {
    if flags & (key_flags::TERMSRV_SET_LED | key_flags::TERMSRV_SHADOW) != 0 {
        return KeyReport::Ignored;
    }
    if make_code == KEYBOARD_OVERRUN_MAKE_CODE {
        return KeyReport::Overrun;
    }
    // E1 is checked first: the two bits are independent in the header, and an
    // E1 sequence must never be mistaken for a plain extended key.
    let prefix = if flags & key_flags::E1 != 0 {
        RawKeyPrefix::E1
    } else if flags & key_flags::E0 != 0 {
        RawKeyPrefix::E0
    } else {
        RawKeyPrefix::None
    };
    KeyReport::Edge {
        prefix,
        pressed: flags & key_flags::BREAK == 0,
    }
}

/// Decode `usButtonFlags` into ordered button edges.
///
/// One report can carry both the DOWN and UP flag for the same button. Both
/// are emitted, DOWN before UP — a deterministic policy, not a claim that
/// Windows preserved the hardware ordering, which it does not promise.
/// Reading only the last flag would swallow fast clicks, which is exactly the
/// input a shockwave effect is built for.
#[must_use]
pub fn button_edges(flags: u32) -> Vec<(RawButton, bool)> {
    const TABLE: [(u32, u32, RawButton); 5] = [
        (
            button_flags::LEFT_DOWN,
            button_flags::LEFT_UP,
            RawButton::Left,
        ),
        (
            button_flags::RIGHT_DOWN,
            button_flags::RIGHT_UP,
            RawButton::Right,
        ),
        (
            button_flags::MIDDLE_DOWN,
            button_flags::MIDDLE_UP,
            RawButton::Middle,
        ),
        (
            button_flags::BUTTON_4_DOWN,
            button_flags::BUTTON_4_UP,
            RawButton::Side,
        ),
        (
            button_flags::BUTTON_5_DOWN,
            button_flags::BUTTON_5_UP,
            RawButton::Extra,
        ),
    ];

    let mut edges = Vec::new();
    for (down, up, button) in TABLE {
        if flags & down != 0 {
            edges.push((button, true));
        }
        if flags & up != 0 {
            edges.push((button, false));
        }
    }
    edges
}

/// Vertical wheel travel, or `None` when this report carries no wheel.
///
/// `usButtonData` is declared `u16` but carries a signed value: scroll-down
/// arrives as `0xFF88`. Widening the `u16` yields 65416 instead of −120, so
/// the reinterpretation is mandatory. `RI_MOUSE_HWHEEL` returns `None` — the
/// shared event contract has no horizontal axis, and reporting horizontal
/// scroll through the vertical channel would be a silent lie.
#[must_use]
pub const fn wheel_delta(flags: u32, button_data: u16) -> Option<i32> {
    if flags & button_flags::WHEEL == 0 {
        return None;
    }
    Some(button_data.cast_signed() as i32)
}

/// Whether this report carries a horizontal wheel, which is deliberately
/// dropped rather than folded into the vertical channel.
#[must_use]
pub const fn is_horizontal_wheel(flags: u32) -> bool {
    flags & button_flags::HWHEEL != 0
}

/// A screen rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl ScreenRect {
    /// Normalize a pixel position within this rect to `[0,1]²`.
    #[must_use]
    pub fn normalize(&self, x: i32, y: i32) -> (f32, f32) {
        let width = f64::from(self.width.max(1));
        let height = f64::from(self.height.max(1));
        let norm_x = (f64::from(x) - f64::from(self.left)) / width;
        let norm_y = (f64::from(y) - f64::from(self.top)) / height;
        (clamp_unit(norm_x), clamp_unit(norm_y))
    }
}

/// Which coordinate space an absolute mouse report used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbsoluteSpace {
    /// `MOUSE_VIRTUAL_DESKTOP` set: the range spans the whole virtual desktop.
    VirtualDesktop,
    /// Default: the range spans the primary monitor only.
    PrimaryMonitor,
}

/// How a `RAWMOUSE` reported its motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionKind {
    Relative,
    Absolute(AbsoluteSpace),
}

/// Classify `RAWMOUSE::usFlags` into a motion kind.
#[must_use]
pub const fn motion_kind(flags: u16) -> MotionKind {
    if flags & mouse_flags::MOVE_ABSOLUTE == 0 {
        return MotionKind::Relative;
    }
    if flags & mouse_flags::VIRTUAL_DESKTOP == 0 {
        MotionKind::Absolute(AbsoluteSpace::PrimaryMonitor)
    } else {
        MotionKind::Absolute(AbsoluteSpace::VirtualDesktop)
    }
}

/// Normalize an absolute mouse report to `[0,1]²` of the virtual desktop.
///
/// Reports arrive in a 0..65535 space over either the primary monitor or the
/// whole virtual desktop, chosen by `MOUSE_VIRTUAL_DESKTOP`. Both are mapped
/// into virtual-desktop coordinates so an absolute device and the sampled
/// cursor mean the same thing — a primary-monitor report on a multi-monitor
/// desk would otherwise claim the whole canvas.
#[must_use]
pub fn normalize_absolute(
    last_x: i32,
    last_y: i32,
    space: AbsoluteSpace,
    primary: ScreenRect,
    virtual_screen: ScreenRect,
) -> (f32, f32) {
    let unit_x = f64::from(last_x) / f64::from(ABSOLUTE_RANGE);
    let unit_y = f64::from(last_y) / f64::from(ABSOLUTE_RANGE);
    match space {
        AbsoluteSpace::VirtualDesktop => (clamp_unit(unit_x), clamp_unit(unit_y)),
        AbsoluteSpace::PrimaryMonitor => {
            let pixel_x = f64::from(primary.left) + unit_x * f64::from(primary.width);
            let pixel_y = f64::from(primary.top) + unit_y * f64::from(primary.height);
            let width = f64::from(virtual_screen.width.max(1));
            let height = f64::from(virtual_screen.height.max(1));
            (
                clamp_unit((pixel_x - f64::from(virtual_screen.left)) / width),
                clamp_unit((pixel_y - f64::from(virtual_screen.top)) / height),
            )
        }
    }
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "clamped to [0,1] before narrowing, so the f32 is exact-ish and never saturates"
)]
fn clamp_unit(value: f64) -> f32 {
    value.clamp(0.0, 1.0) as f32
}

/// Outcome of stepping to the next record in a `GetRawInputBuffer` batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordStep {
    /// Byte offset of the next record.
    Next(usize),
    /// The batch ended exactly at the buffer's used length.
    End,
    /// `dwSize` was implausible or would walk past the used length. The batch
    /// terminates here rather than reading whatever follows.
    Malformed,
}

/// Advance past one record in a buffered read.
///
/// The SDK's `NEXTRAWINPUTBLOCK` aligns the resulting *address* up to
/// `RAWINPUT_ALIGN`, which on x64 is `sizeof(QWORD)`. Aligning the offset
/// instead is only equivalent because the buffer base is 8-aligned; this
/// crate backs it with a `Vec<u64>` to guarantee that.
#[must_use]
pub fn next_record(offset: usize, dw_size: u32, used_len: usize, min_size: usize) -> RecordStep {
    let size = dw_size as usize;
    if size < min_size {
        return RecordStep::Malformed;
    }
    let Some(end) = offset.checked_add(size) else {
        return RecordStep::Malformed;
    };
    if end > used_len {
        return RecordStep::Malformed;
    }
    let next = end.next_multiple_of(RECORD_ALIGN);
    if next >= used_len {
        RecordStep::End
    } else {
        RecordStep::Next(next)
    }
}

/// Stable synthetic name for a scan code the canonical table does not know.
///
/// Remapping keyboards, firmware-quirky devices, and RDP all emit codes
/// outside canonical set 1. A silently dropped key is worse for an effect
/// author to debug than an honestly named unknown one. The prefix is part of
/// the name so an unrecognized `E1` sequence cannot collide with an
/// unprefixed code of the same value.
#[must_use]
pub fn unknown_key_name(make_code: u16, prefix: RawKeyPrefix) -> String {
    match prefix {
        RawKeyPrefix::None => format!("Unknown{make_code:04X}"),
        RawKeyPrefix::E0 => format!("UnknownE0{make_code:04X}"),
        RawKeyPrefix::E1 => format!("UnknownE1{make_code:04X}"),
    }
}
