//! Raw Input decoding arithmetic.
//!
//! These run on every platform, which is the entire point of keeping the
//! arithmetic free of Windows types: the subtle conversions get covered by
//! Linux CI rather than only by whoever last built on a Windows machine.

use hypercolor_windows_input::decode::{
    AbsoluteSpace, CanonicalKeyReport, KEYBOARD_OVERRUN_MAKE_CODE, KeyCanonicalizer, KeyReport,
    MotionKind, RecordStep, ScreenRect, WHEEL_DELTA, button_edges, classify_key,
    is_horizontal_wheel, motion_kind, next_record, normalize_absolute, unknown_key_name,
    wheel_delta,
};
use hypercolor_windows_input::{RawButton, RawKeyPrefix};

const RI_KEY_BREAK: u16 = 1;
const RI_KEY_E0: u16 = 2;
const RI_KEY_E1: u16 = 4;
const RI_KEY_TERMSRV_SET_LED: u16 = 8;
const RI_KEY_TERMSRV_SHADOW: u16 = 0x10;

const RI_MOUSE_LEFT_DOWN: u32 = 0x0001;
const RI_MOUSE_LEFT_UP: u32 = 0x0002;
const RI_MOUSE_MIDDLE_DOWN: u32 = 0x0010;
const RI_MOUSE_BUTTON_4_DOWN: u32 = 0x0040;
const RI_MOUSE_BUTTON_5_UP: u32 = 0x0200;
const RI_MOUSE_WHEEL: u32 = 0x0400;
const RI_MOUSE_HWHEEL: u32 = 0x0800;

const MOUSE_MOVE_ABSOLUTE: u16 = 0x01;
const MOUSE_VIRTUAL_DESKTOP: u16 = 0x02;

// ── Key classification ─────────────────────────────────────────────────────

#[test]
fn plain_make_code_is_a_positional_press() {
    assert_eq!(
        classify_key(0x1E, 0),
        KeyReport::Edge {
            prefix: RawKeyPrefix::None,
            pressed: true,
        }
    );
}

#[test]
fn break_flag_marks_a_release() {
    assert_eq!(
        classify_key(0x1E, RI_KEY_BREAK),
        KeyReport::Edge {
            prefix: RawKeyPrefix::None,
            pressed: false,
        }
    );
}

#[test]
fn e0_prefix_survives_to_the_name_table() {
    assert_eq!(
        classify_key(0x48, RI_KEY_E0),
        KeyReport::Edge {
            prefix: RawKeyPrefix::E0,
            pressed: true,
        }
    );
}

#[test]
fn e1_is_not_collapsed_into_e0() {
    // A boolean `extended` would report these identically, which is exactly
    // the blindness the three-state prefix exists to prevent.
    let e1 = classify_key(0x1D, RI_KEY_E1);
    let e0 = classify_key(0x1D, RI_KEY_E0);
    assert_ne!(e1, e0);
    assert_eq!(
        e1,
        KeyReport::Edge {
            prefix: RawKeyPrefix::E1,
            pressed: true,
        }
    );
}

#[test]
fn e1_wins_when_both_prefix_bits_are_set() {
    assert_eq!(
        classify_key(0x1D, RI_KEY_E0 | RI_KEY_E1),
        KeyReport::Edge {
            prefix: RawKeyPrefix::E1,
            pressed: true,
        }
    );
}

#[test]
fn overrun_scan_code_is_rejected_rather_than_named() {
    // 0xFF is a rollover report, not a position. Letting it reach the table
    // would name a key nobody pressed, and mashing keys is the point of half
    // these effects.
    assert_eq!(
        classify_key(KEYBOARD_OVERRUN_MAKE_CODE, 0),
        KeyReport::Overrun
    );
    assert_eq!(
        classify_key(KEYBOARD_OVERRUN_MAKE_CODE, RI_KEY_BREAK),
        KeyReport::Overrun
    );
}

#[test]
fn terminal_services_flags_carry_no_key_edge() {
    assert_eq!(
        classify_key(0x1E, RI_KEY_TERMSRV_SET_LED),
        KeyReport::Ignored
    );
    assert_eq!(
        classify_key(0x1E, RI_KEY_TERMSRV_SHADOW),
        KeyReport::Ignored
    );
    // Masked before the overrun check too: LED bookkeeping is not a gap.
    assert_eq!(
        classify_key(KEYBOARD_OVERRUN_MAKE_CODE, RI_KEY_TERMSRV_SET_LED),
        KeyReport::Ignored
    );
}

#[test]
fn pause_sequence_emits_one_canonical_edge_per_action() {
    const VK_PAUSE: u16 = 0x13;
    let canonicalizer = KeyCanonicalizer;

    assert_eq!(
        canonicalizer.canonicalize(0x1D, RI_KEY_E1, VK_PAUSE),
        CanonicalKeyReport::Edge {
            make_code: 0x1D,
            prefix: RawKeyPrefix::E1,
            vkey: VK_PAUSE,
            pressed: true,
        }
    );

    assert_eq!(
        canonicalizer.canonicalize(0x1D, RI_KEY_E1 | RI_KEY_BREAK, VK_PAUSE),
        CanonicalKeyReport::Edge {
            make_code: 0x1D,
            prefix: RawKeyPrefix::E1,
            vkey: VK_PAUSE,
            pressed: false,
        }
    );

    for flags in [0, RI_KEY_BREAK] {
        assert_eq!(
            canonicalizer.canonicalize(0x45, flags, 0xFF),
            CanonicalKeyReport::Ignored
        );
    }
}

#[test]
fn repeated_pause_sequences_preserve_each_logical_press() {
    let canonicalizer = KeyCanonicalizer;

    let reports = [
        canonicalizer.canonicalize(0x1D, RI_KEY_E1, 0x13),
        canonicalizer.canonicalize(0x1D, RI_KEY_E1, 0x13),
    ];

    assert_eq!(
        reports
            .into_iter()
            .filter(|report| matches!(report, CanonicalKeyReport::Edge { .. }))
            .count(),
        2
    );
}

#[test]
fn print_screen_suppresses_only_its_fake_shift_records() {
    let canonicalizer = KeyCanonicalizer;
    let reports = [
        canonicalizer.canonicalize(0x2A, RI_KEY_E0, 0x10),
        canonicalizer.canonicalize(0x37, RI_KEY_E0, 0x2C),
        canonicalizer.canonicalize(0x37, RI_KEY_E0 | RI_KEY_BREAK, 0x2C),
        canonicalizer.canonicalize(0x2A, RI_KEY_E0 | RI_KEY_BREAK, 0x10),
    ];

    assert_eq!(reports[0], CanonicalKeyReport::Ignored);
    assert_eq!(reports[3], CanonicalKeyReport::Ignored);
    assert_eq!(
        reports[1],
        CanonicalKeyReport::Edge {
            make_code: 0x37,
            prefix: RawKeyPrefix::E0,
            vkey: 0x2C,
            pressed: true,
        }
    );
    assert_eq!(
        reports[2],
        CanonicalKeyReport::Edge {
            make_code: 0x37,
            prefix: RawKeyPrefix::E0,
            vkey: 0x2C,
            pressed: false,
        }
    );
}

#[test]
fn shifted_numpad_navigation_drops_fake_shift_without_dropping_arrow() {
    let canonicalizer = KeyCanonicalizer;
    let reports = [
        canonicalizer.canonicalize(0x2A, RI_KEY_E0 | RI_KEY_BREAK, 0x10),
        canonicalizer.canonicalize(0x48, RI_KEY_E0, 0x26),
        canonicalizer.canonicalize(0x48, RI_KEY_E0 | RI_KEY_BREAK, 0x26),
        canonicalizer.canonicalize(0x2A, RI_KEY_E0, 0x10),
    ];

    assert_eq!(reports[0], CanonicalKeyReport::Ignored);
    assert_eq!(reports[3], CanonicalKeyReport::Ignored);
    assert!(matches!(
        reports[1],
        CanonicalKeyReport::Edge {
            make_code: 0x48,
            prefix: RawKeyPrefix::E0,
            pressed: true,
            ..
        }
    ));
    assert!(matches!(
        reports[2],
        CanonicalKeyReport::Edge {
            make_code: 0x48,
            prefix: RawKeyPrefix::E0,
            pressed: false,
            ..
        }
    ));
}

#[test]
fn genuine_unprefixed_shift_edges_survive_and_fake_extended_shifts_do_not() {
    let canonicalizer = KeyCanonicalizer;

    assert!(matches!(
        canonicalizer.canonicalize(0x2A, 0, 0xA0),
        CanonicalKeyReport::Edge {
            make_code: 0x2A,
            prefix: RawKeyPrefix::None,
            pressed: true,
            ..
        }
    ));
    assert!(matches!(
        canonicalizer.canonicalize(0x36, 0, 0xA1),
        CanonicalKeyReport::Edge {
            make_code: 0x36,
            prefix: RawKeyPrefix::None,
            pressed: true,
            ..
        }
    ));
    for (make_code, vkey) in [(0x2A, 0xA0), (0x36, 0xA1)] {
        assert_eq!(
            canonicalizer.canonicalize(make_code, RI_KEY_E0, vkey),
            CanonicalKeyReport::Ignored
        );
        assert_eq!(
            canonicalizer.canonicalize(make_code, RI_KEY_E0 | RI_KEY_BREAK, vkey),
            CanonicalKeyReport::Ignored
        );
    }
}

#[test]
fn e1_prefix_is_self_contained_and_does_not_leak_to_the_next_record() {
    let canonicalizer = KeyCanonicalizer;
    assert_eq!(
        canonicalizer.canonicalize(0x1D, RI_KEY_E1, 0x13),
        CanonicalKeyReport::Edge {
            make_code: 0x1D,
            prefix: RawKeyPrefix::E1,
            vkey: 0x13,
            pressed: true,
        }
    );
    assert!(matches!(
        canonicalizer.canonicalize(0x1E, 0, 0x41),
        CanonicalKeyReport::Edge {
            make_code: 0x1E,
            prefix: RawKeyPrefix::None,
            ..
        }
    ));
}

#[test]
fn overrun_does_not_change_the_next_records_prefix() {
    let canonicalizer = KeyCanonicalizer;
    assert_eq!(
        canonicalizer.canonicalize(KEYBOARD_OVERRUN_MAKE_CODE, 0, 0),
        CanonicalKeyReport::Overrun
    );
    assert_eq!(
        canonicalizer.canonicalize(KEYBOARD_OVERRUN_MAKE_CODE, 0, 0xFF),
        CanonicalKeyReport::Overrun
    );
    assert!(matches!(
        canonicalizer.canonicalize(0x1E, 0, 0x41),
        CanonicalKeyReport::Edge {
            prefix: RawKeyPrefix::None,
            ..
        }
    ));
}

// ── Buttons ────────────────────────────────────────────────────────────────

#[test]
fn single_button_edges_decode() {
    assert_eq!(
        button_edges(RI_MOUSE_LEFT_DOWN),
        vec![(RawButton::Left, true)]
    );
    assert_eq!(
        button_edges(RI_MOUSE_LEFT_UP),
        vec![(RawButton::Left, false)]
    );
    assert_eq!(
        button_edges(RI_MOUSE_MIDDLE_DOWN),
        vec![(RawButton::Middle, true)]
    );
}

#[test]
fn extra_buttons_use_the_evdev_names() {
    assert_eq!(
        button_edges(RI_MOUSE_BUTTON_4_DOWN),
        vec![(RawButton::Side, true)]
    );
    assert_eq!(
        button_edges(RI_MOUSE_BUTTON_5_UP),
        vec![(RawButton::Extra, false)]
    );
    assert_eq!(RawButton::Side.canonical_name(), "side");
    assert_eq!(RawButton::Extra.canonical_name(), "extra");
}

#[test]
fn a_click_faster_than_the_report_interval_yields_both_edges_in_order() {
    // One report can carry DOWN and UP for the same button. Reading only the
    // last flag would swallow the click entirely, which is precisely the input
    // a shockwave effect is built for.
    assert_eq!(
        button_edges(RI_MOUSE_LEFT_DOWN | RI_MOUSE_LEFT_UP),
        vec![(RawButton::Left, true), (RawButton::Left, false)]
    );
}

#[test]
fn unrelated_flag_bits_produce_no_button_edges() {
    assert!(button_edges(RI_MOUSE_WHEEL).is_empty());
    assert!(button_edges(0).is_empty());
}

// ── Wheel ──────────────────────────────────────────────────────────────────

#[test]
fn scroll_up_is_one_positive_notch() {
    let data = u16::try_from(WHEEL_DELTA).expect("WHEEL_DELTA fits a u16");
    assert_eq!(wheel_delta(RI_MOUSE_WHEEL, data), Some(WHEEL_DELTA));
}

#[test]
fn scroll_down_reinterprets_the_u16_as_signed() {
    // usButtonData is declared u16 but carries a signed value: widening it
    // directly yields 65416 instead of -120, and every downward scroll would
    // read as a huge upward one.
    assert_eq!(wheel_delta(RI_MOUSE_WHEEL, 0xFF88), Some(-WHEEL_DELTA));
}

#[test]
fn sub_notch_hi_res_values_pass_through_unscaled() {
    // 1/120-notch units are already evdev's REL_WHEEL_HI_RES unit, so a
    // high-resolution wheel needs no conversion in either direction.
    assert_eq!(wheel_delta(RI_MOUSE_WHEEL, 30), Some(30));
    assert_eq!(wheel_delta(RI_MOUSE_WHEEL, 0xFFE2), Some(-30));
}

#[test]
fn horizontal_wheel_is_dropped_not_folded_into_vertical() {
    // The shared event contract has no axis. Reporting horizontal scroll
    // through the vertical channel would be a silent lie to every effect.
    assert_eq!(wheel_delta(RI_MOUSE_HWHEEL, 120), None);
    assert!(is_horizontal_wheel(RI_MOUSE_HWHEEL));
    assert!(!is_horizontal_wheel(RI_MOUSE_WHEEL));
}

#[test]
fn a_report_with_no_wheel_flag_has_no_wheel() {
    assert_eq!(wheel_delta(RI_MOUSE_LEFT_DOWN, 120), None);
}

// ── Motion ─────────────────────────────────────────────────────────────────

#[test]
fn motion_kind_reads_the_coordinate_space_flags() {
    assert_eq!(motion_kind(0), MotionKind::Relative);
    assert_eq!(
        motion_kind(MOUSE_MOVE_ABSOLUTE),
        MotionKind::Absolute(AbsoluteSpace::PrimaryMonitor)
    );
    assert_eq!(
        motion_kind(MOUSE_MOVE_ABSOLUTE | MOUSE_VIRTUAL_DESKTOP),
        MotionKind::Absolute(AbsoluteSpace::VirtualDesktop)
    );
    // VIRTUAL_DESKTOP without MOVE_ABSOLUTE is still relative motion.
    assert_eq!(motion_kind(MOUSE_VIRTUAL_DESKTOP), MotionKind::Relative);
}

#[test]
fn virtual_desktop_absolute_normalizes_across_the_full_range() {
    let virtual_screen = ScreenRect {
        left: -1920,
        top: 0,
        width: 3840,
        height: 1080,
    };
    let primary = ScreenRect {
        left: 0,
        top: 0,
        width: 1920,
        height: 1080,
    };

    let (x, y) = normalize_absolute(0, 0, AbsoluteSpace::VirtualDesktop, primary, virtual_screen);
    assert!((x - 0.0).abs() < 1e-4, "left edge maps to 0, got {x}");
    assert!((y - 0.0).abs() < 1e-4);

    let (x, _) = normalize_absolute(
        65535,
        65535,
        AbsoluteSpace::VirtualDesktop,
        primary,
        virtual_screen,
    );
    assert!((x - 1.0).abs() < 1e-4, "right edge maps to 1, got {x}");
}

#[test]
fn primary_monitor_absolute_is_remapped_into_virtual_desktop_space() {
    // A monitor sitting left of primary makes the virtual desktop start at a
    // negative x. A primary-monitor report that skipped this remap would claim
    // the whole canvas instead of its right-hand half.
    let virtual_screen = ScreenRect {
        left: -1920,
        top: 0,
        width: 3840,
        height: 1080,
    };
    let primary = ScreenRect {
        left: 0,
        top: 0,
        width: 1920,
        height: 1080,
    };

    let (left, _) =
        normalize_absolute(0, 0, AbsoluteSpace::PrimaryMonitor, primary, virtual_screen);
    assert!(
        (left - 0.5).abs() < 1e-3,
        "primary's left edge sits at the virtual desktop's midpoint, got {left}"
    );

    let (right, _) = normalize_absolute(
        65535,
        0,
        AbsoluteSpace::PrimaryMonitor,
        primary,
        virtual_screen,
    );
    assert!(
        (right - 1.0).abs() < 1e-3,
        "primary's right edge is the virtual desktop's right edge, got {right}"
    );
}

#[test]
fn normalization_clamps_rather_than_escaping_the_unit_square() {
    let rect = ScreenRect {
        left: 0,
        top: 0,
        width: 1920,
        height: 1080,
    };
    let (x, y) = rect.normalize(-500, 5000);
    assert!((0.0..=1.0).contains(&x));
    assert!((0.0..=1.0).contains(&y));
    assert_eq!(x, 0.0);
    assert_eq!(y, 1.0);
}

#[test]
fn a_degenerate_rect_does_not_divide_by_zero() {
    let rect = ScreenRect {
        left: 0,
        top: 0,
        width: 0,
        height: 0,
    };
    let (x, y) = rect.normalize(10, 10);
    assert!(x.is_finite() && y.is_finite());
}

// ── Buffered record walk ───────────────────────────────────────────────────

const HEADER: usize = 24;

#[test]
fn records_advance_by_qword_aligned_stride() {
    // dwSize 40 is already 8-aligned, so the next record starts right after.
    assert_eq!(next_record(0, 40, 400, HEADER), RecordStep::Next(40));
    // dwSize 36 rounds up to 40: the SDK aligns the resulting address, and
    // aligning the offset is equivalent only because the buffer base is
    // 8-aligned, which the Vec<u64> backing guarantees.
    assert_eq!(next_record(0, 36, 400, HEADER), RecordStep::Next(40));
    assert_eq!(next_record(40, 33, 400, HEADER), RecordStep::Next(80));
}

#[test]
fn a_record_ending_at_the_buffer_end_terminates_the_batch() {
    assert_eq!(next_record(360, 40, 400, HEADER), RecordStep::End);
}

#[test]
fn a_record_claiming_more_than_the_buffer_holds_is_malformed() {
    // Walking on would read whatever follows the buffer, so the batch stops.
    assert_eq!(next_record(360, 80, 400, HEADER), RecordStep::Malformed);
}

#[test]
fn a_record_smaller_than_its_header_is_malformed() {
    assert_eq!(next_record(0, 8, 400, HEADER), RecordStep::Malformed);
    assert_eq!(next_record(0, 0, 400, HEADER), RecordStep::Malformed);
}

#[test]
fn an_overflowing_size_is_malformed_rather_than_wrapping() {
    assert_eq!(
        next_record(usize::MAX - 8, u32::MAX, 400, HEADER),
        RecordStep::Malformed
    );
}

// ── Unknown keys ───────────────────────────────────────────────────────────

#[test]
fn unknown_key_names_are_stable_and_prefix_qualified() {
    // Remapping keyboards, quirky firmware, and RDP all emit codes outside
    // canonical set 1. A silently dropped key is worse to debug than an
    // honestly named unknown one.
    assert_eq!(unknown_key_name(0x7F, RawKeyPrefix::None), "Unknown007F");
    assert_eq!(unknown_key_name(0x7F, RawKeyPrefix::E0), "UnknownE0007F");
    assert_eq!(unknown_key_name(0x7F, RawKeyPrefix::E1), "UnknownE1007F");
}

#[test]
fn an_unknown_e1_sequence_cannot_collide_with_an_unprefixed_code() {
    assert_ne!(
        unknown_key_name(0x1D, RawKeyPrefix::E1),
        unknown_key_name(0x1D, RawKeyPrefix::None)
    );
    assert_ne!(
        unknown_key_name(0x1D, RawKeyPrefix::E1),
        unknown_key_name(0x1D, RawKeyPrefix::E0)
    );
}
