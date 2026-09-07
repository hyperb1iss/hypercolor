use std::sync::Arc;

use hypercolor_macos_input::{
    MacosInputConfig, MacosInputError, MacosModifierFlags, MacosVirtualDesktop,
    NX_SUBTYPE_AUX_CONTROL_BUTTONS, decode_button_event, decode_media_key, decode_momentum_phase,
    decode_scroll_phase, event_masks, input_monitoring_granted, normalize_button_event,
    normalize_key_event, normalize_modifier_event, normalize_motion_event, normalize_scroll_event,
    request_input_monitoring,
};
use hypercolor_types::event::{PointerScrollPhase, PointerScrollUnit};
use hypercolor_types::host_input::{
    HostInputBatch, HostInputEvent, HostKeySignal, HostPointerButton, HostPointerMotion,
    HostRepeatEvidence,
};

#[test]
fn config_debug_omits_the_injected_clock() {
    let config = MacosInputConfig {
        keyboard: true,
        pointer: false,
        clock: Arc::new(|| 7),
    };

    assert_eq!(
        format!("{config:?}"),
        "MacosInputConfig { keyboard: true, pointer: false, .. }"
    );
    assert_eq!((config.clock)(), 7);
}

#[test]
fn masks_keep_keyboard_and_pointer_consent_independent() {
    let keyboard = event_masks(true, false);
    let pointer = event_masks(false, true);
    let both = event_masks(true, true);

    assert_ne!(keyboard.keyboard, 0);
    assert_eq!(keyboard.pointer, 0);
    assert_eq!(pointer.keyboard, 0);
    assert_ne!(pointer.pointer, 0);
    assert_eq!(both.keyboard, keyboard.keyboard);
    assert_eq!(both.pointer, pointer.pointer);
    assert_eq!(event_masks(false, false), Default::default());
}

#[test]
fn media_decoder_accepts_only_valid_subtype_eight_payloads() {
    let pressed = (16_i64 << 16) | (0x0a_i64 << 8) | 1;
    let released = (18_i64 << 16) | (0x0b_i64 << 8);

    let Some(HostInputEvent::Key {
        identity, signal, ..
    }) = decode_media_key(NX_SUBTYPE_AUX_CONTROL_BUTTONS, pressed)
    else {
        panic!("valid media key should normalize");
    };
    assert_eq!(identity.key.as_ref(), "MediaPlayPause");
    assert_eq!(
        signal,
        HostKeySignal::Edge {
            pressed: true,
            repeat: HostRepeatEvidence::Repeat,
        }
    );
    assert!(matches!(
        decode_media_key(NX_SUBTYPE_AUX_CONTROL_BUTTONS, released),
        Some(HostInputEvent::Key {
            signal: HostKeySignal::Edge {
                pressed: false,
                repeat: HostRepeatEvidence::NotRepeat
            },
            ..
        })
    ));
    assert_eq!(decode_media_key(7, pressed), None);
    assert_eq!(decode_media_key(8, 16_i64 << 16), None);
    assert_eq!(decode_media_key(8, -1), None);
}

#[test]
fn button_decoder_preserves_numbered_extras() {
    assert_eq!(
        decode_button_event(1, 0).map(|(button, edge)| (button.as_str().to_owned(), edge)),
        Some(("left".to_owned(), true))
    );
    assert_eq!(
        decode_button_event(4, 1).map(|(button, edge)| (button.as_str().to_owned(), edge)),
        Some(("right".to_owned(), false))
    );
    assert_eq!(
        decode_button_event(25, 2).map(|(button, edge)| (button.as_str().to_owned(), edge)),
        Some(("middle".to_owned(), true))
    );
    assert_eq!(
        decode_button_event(26, 7).map(|(button, edge)| (button.as_str().to_owned(), edge)),
        Some(("button8".to_owned(), false))
    );
    assert_eq!(decode_button_event(5, 0), None);
}

#[test]
fn scroll_phases_use_core_graphics_native_values() {
    assert_eq!(decode_scroll_phase(0), Some(PointerScrollPhase::None));
    assert_eq!(decode_scroll_phase(1), Some(PointerScrollPhase::Began));
    assert_eq!(decode_scroll_phase(2), Some(PointerScrollPhase::Changed));
    assert_eq!(decode_scroll_phase(4), Some(PointerScrollPhase::Ended));
    assert_eq!(decode_scroll_phase(8), Some(PointerScrollPhase::Cancelled));
    assert_eq!(decode_scroll_phase(128), Some(PointerScrollPhase::MayBegin));
    assert_eq!(decode_scroll_phase(16), None);

    assert_eq!(decode_momentum_phase(0), Some(PointerScrollPhase::None));
    assert_eq!(decode_momentum_phase(1), Some(PointerScrollPhase::Began));
    assert_eq!(decode_momentum_phase(2), Some(PointerScrollPhase::Changed));
    assert_eq!(decode_momentum_phase(3), Some(PointerScrollPhase::Ended));
    assert_eq!(decode_momentum_phase(4), None);
}

#[test]
fn modifier_flags_preserve_distinct_native_bits() {
    let flags = MacosModifierFlags::from_bits(
        MacosModifierFlags::SHIFT.bits() | MacosModifierFlags::COMMAND.bits(),
    );

    assert!(flags.contains(MacosModifierFlags::SHIFT));
    assert!(flags.contains(MacosModifierFlags::COMMAND));
    assert!(!flags.contains(MacosModifierFlags::CONTROL));
    assert_eq!(flags.bits(), (1 << 17) | (1 << 20));
}

#[test]
fn virtual_desktop_normalizes_negative_origins_and_clamps_edges() {
    let desktop = MacosVirtualDesktop::new(-1920.0, -120.0, 4480.0, 1560.0, 9)
        .expect("fixture bounds are valid");

    assert_eq!(desktop.normalize(-1920.0, -120.0), (0.0, 0.0));
    assert_eq!(desktop.normalize(320.0, 660.0), (0.5, 0.5));
    assert_eq!(desktop.normalize(4000.0, -500.0), (1.0, 0.0));
    assert_eq!(desktop.topology_generation, 9);
}

#[test]
fn virtual_desktop_rejects_nonfinite_and_empty_bounds() {
    assert_eq!(
        MacosVirtualDesktop::new(0.0, 0.0, 0.0, 100.0, 1),
        Err(MacosInputError::InvalidVirtualDesktop)
    );
    assert_eq!(
        MacosVirtualDesktop::new(f64::NAN, 0.0, 100.0, 100.0, 1),
        Err(MacosInputError::InvalidVirtualDesktop)
    );
}

#[test]
fn normalizers_emit_the_complete_shared_vocabulary() {
    let desktop =
        MacosVirtualDesktop::new(0.0, 0.0, 100.0, 100.0, 2).expect("fixture bounds are valid");
    let (motion, pointer) = normalize_motion_event(desktop, 25.0, 75.0);
    let events = [
        normalize_key_event(0, true, false).expect("key normalizes"),
        normalize_modifier_event(0x38, MacosModifierFlags::SHIFT).expect("modifier normalizes"),
        normalize_button_event(HostPointerButton::middle(), true),
        motion,
        normalize_scroll_event(
            1 << 15,
            -(1 << 16),
            true,
            PointerScrollPhase::Changed,
            PointerScrollPhase::Began,
        ),
    ];
    let batch = HostInputBatch {
        events: &events,
        pointer: Some(pointer),
        at_ms: 55,
        device_catalog_generation: 0,
    };

    assert_eq!(batch.at_ms, 55);
    assert_eq!(batch.events, events);
    assert_eq!(batch.pointer, Some(pointer));
    assert!(matches!(
        batch.events[1],
        HostInputEvent::Key {
            signal: HostKeySignal::AggregateState { active: true, .. },
            ..
        }
    ));
    assert!(matches!(
        batch.events[3],
        HostInputEvent::Motion {
            motion: HostPointerMotion::Absolute {
                coordinate_space_generation: 2,
                ..
            },
            ..
        }
    ));
    assert!(matches!(
        batch.events[4],
        HostInputEvent::Scroll {
            unit: PointerScrollUnit::Pixels,
            ..
        }
    ));
}

#[test]
fn permission_preflight_is_a_read_only_boolean_probe() {
    let granted = input_monitoring_granted();
    assert!(matches!(granted, true | false));

    #[cfg(target_os = "macos")]
    let _request: fn() -> bool = request_input_monitoring;
    #[cfg(not(target_os = "macos"))]
    assert!(!request_input_monitoring());
}

#[test]
fn empty_session_is_rejected_before_platform_access() {
    let error = hypercolor_macos_input::MacosInputSession::start(
        MacosInputConfig {
            keyboard: false,
            pointer: false,
            clock: Arc::new(|| 0),
        },
        |_| hypercolor_macos_input::MacosInputPublicationOutcome::Published,
    )
    .err()
    .expect("empty capture must fail");

    #[cfg(target_os = "macos")]
    assert_eq!(error, MacosInputError::NothingToCapture);
    #[cfg(not(target_os = "macos"))]
    assert_eq!(error, MacosInputError::UnsupportedPlatform);
}

#[cfg(target_os = "macos")]
#[test]
fn current_virtual_desktop_reports_positive_finite_geometry() {
    let desktop = hypercolor_macos_input::current_virtual_desktop()
        .expect("the test host has an active display");

    assert!(desktop.origin_x.is_finite());
    assert!(desktop.origin_y.is_finite());
    assert!(desktop.width.is_finite() && desktop.width > 0.0);
    assert!(desktop.height.is_finite() && desktop.height > 0.0);
}
