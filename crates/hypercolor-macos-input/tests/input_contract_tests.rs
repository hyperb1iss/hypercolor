use std::sync::Arc;

use hypercolor_macos_input::{
    MacosInputBatch, MacosInputConfig, MacosInputError, MacosInputEvent, MacosInputGapReason,
    MacosModifierFlags, MacosPointerButton, MacosScrollPhase, MacosScrollUnit, MacosVirtualDesktop,
    NX_SUBTYPE_AUX_CONTROL_BUTTONS, decode_button_event, decode_media_key, decode_momentum_phase,
    decode_scroll_phase, event_masks,
};

#[test]
fn config_debug_omits_the_injected_clock() {
    let config = MacosInputConfig {
        keyboard: true,
        pointer: false,
        epoch: 42,
        clock: Arc::new(|| 7),
    };

    assert_eq!(
        format!("{config:?}"),
        "MacosInputConfig { keyboard: true, pointer: false, epoch: 42, .. }"
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

    assert_eq!(
        decode_media_key(NX_SUBTYPE_AUX_CONTROL_BUTTONS, pressed),
        Some(hypercolor_macos_input::MacosMediaKey {
            nx_key_type: 16,
            pressed: true,
            repeat: true,
        })
    );
    assert_eq!(
        decode_media_key(NX_SUBTYPE_AUX_CONTROL_BUTTONS, released),
        Some(hypercolor_macos_input::MacosMediaKey {
            nx_key_type: 18,
            pressed: false,
            repeat: false,
        })
    );
    assert_eq!(decode_media_key(7, pressed), None);
    assert_eq!(decode_media_key(8, 16_i64 << 16), None);
    assert_eq!(decode_media_key(8, -1), None);
}

#[test]
fn button_decoder_preserves_numbered_extras() {
    assert_eq!(
        decode_button_event(1, 0),
        Some((MacosPointerButton::Left, true))
    );
    assert_eq!(
        decode_button_event(4, 1),
        Some((MacosPointerButton::Right, false))
    );
    assert_eq!(
        decode_button_event(25, 2),
        Some((MacosPointerButton::Middle, true))
    );
    assert_eq!(
        decode_button_event(26, 7),
        Some((MacosPointerButton::Other(7), false))
    );
    assert_eq!(decode_button_event(5, 0), None);
}

#[test]
fn scroll_phases_use_core_graphics_native_values() {
    assert_eq!(decode_scroll_phase(0), Some(MacosScrollPhase::None));
    assert_eq!(decode_scroll_phase(1), Some(MacosScrollPhase::Began));
    assert_eq!(decode_scroll_phase(2), Some(MacosScrollPhase::Changed));
    assert_eq!(decode_scroll_phase(4), Some(MacosScrollPhase::Ended));
    assert_eq!(decode_scroll_phase(8), Some(MacosScrollPhase::Cancelled));
    assert_eq!(decode_scroll_phase(128), Some(MacosScrollPhase::MayBegin));
    assert_eq!(decode_scroll_phase(16), None);

    assert_eq!(decode_momentum_phase(0), Some(MacosScrollPhase::None));
    assert_eq!(decode_momentum_phase(1), Some(MacosScrollPhase::Began));
    assert_eq!(decode_momentum_phase(2), Some(MacosScrollPhase::Changed));
    assert_eq!(decode_momentum_phase(3), Some(MacosScrollPhase::Ended));
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
fn batch_carries_the_complete_plain_rust_vocabulary() {
    let events = [
        MacosInputEvent::Key {
            virtual_keycode: 0,
            pressed: true,
            autorepeat: false,
        },
        MacosInputEvent::ModifierFlags {
            virtual_keycode: 0x38,
            flags: MacosModifierFlags::SHIFT,
        },
        MacosInputEvent::Button {
            button: MacosPointerButton::Middle,
            pressed: true,
        },
        MacosInputEvent::Motion {
            x: -10.0,
            y: 30.0,
            delta_x: 2.0,
            delta_y: -1.0,
        },
        MacosInputEvent::Wheel {
            fixed_delta_x: 1 << 15,
            fixed_delta_y: -(1 << 16),
            unit: MacosScrollUnit::Pixels,
            phase: MacosScrollPhase::Changed,
            momentum_phase: MacosScrollPhase::Began,
        },
        MacosInputEvent::MediaKey {
            nx_key_type: 16,
            pressed: true,
            repeat: false,
        },
        MacosInputEvent::StateGap {
            reason: MacosInputGapReason::QueueOverflow,
        },
    ];
    let desktop =
        MacosVirtualDesktop::new(0.0, 0.0, 100.0, 100.0, 2).expect("fixture bounds are valid");
    let batch = MacosInputBatch {
        epoch: 4,
        at_ms: 55,
        events: &events,
        virtual_desktop: desktop,
    };

    assert_eq!(batch.epoch, 4);
    assert_eq!(batch.at_ms, 55);
    assert_eq!(batch.events, events);
    assert_eq!(batch.virtual_desktop, desktop);
}
