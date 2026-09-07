use std::sync::Arc;

use hypercolor_core::input::{HostInputFold, HostInputPublishOutcome, PointerMode, Q16_16_SCALE};
use hypercolor_types::event::{
    InputButtonState, InputEvent, PointerScrollPhase, PointerScrollUnit,
};
use hypercolor_types::host_input::{
    HostInputBatch, HostInputCapabilities, HostInputDevice, HostInputEvent, HostInputGapReason,
    HostKeyIdentity, HostKeySignal, HostPointerButton, HostPointerMotion, HostPointerSnapshot,
    HostRepeatEvidence,
};

fn device(id: &str, keyboard: bool, pointer: bool, generation: u64) -> Arc<HostInputDevice> {
    Arc::new(HostInputDevice {
        source_id: Arc::from(id),
        label: Arc::from(id),
        capabilities: HostInputCapabilities { keyboard, pointer },
        session_generation: 1,
        device_generation: generation,
    })
}

fn key(name: &str, pressed: bool, repeat: HostRepeatEvidence) -> HostInputEvent {
    HostInputEvent::Key {
        device: None,
        identity: HostKeyIdentity {
            key: Arc::from(name),
            physical_code: Arc::from(format!("test:{name}")),
        },
        signal: HostKeySignal::Edge { pressed, repeat },
    }
}

fn publish(sink: &hypercolor_core::input::HostInputSink, events: &[HostInputEvent], at_ms: u64) {
    assert_eq!(
        sink.publish(HostInputBatch {
            events,
            pointer: None,
            at_ms,
            device_catalog_generation: 1,
        }),
        HostInputPublishOutcome::Published
    );
}

#[test]
fn fold_unions_devices_classifies_repeats_and_synthesizes_ordered_releases() {
    let mut fold = HostInputFold::new(32);
    let sink = fold.begin_session(
        "host",
        HostInputCapabilities {
            keyboard: true,
            pointer: true,
        },
    );
    let first = device("keyboard-a", true, false, 1);
    let second = device("keyboard-b", true, false, 1);
    let events = [
        HostInputEvent::DeviceArrived {
            device: Arc::clone(&first),
        },
        HostInputEvent::DeviceArrived {
            device: Arc::clone(&second),
        },
        HostInputEvent::Key {
            device: Some(Arc::clone(&first)),
            identity: HostKeyIdentity {
                key: Arc::from("a"),
                physical_code: Arc::from("evdev:30"),
            },
            signal: HostKeySignal::Edge {
                pressed: true,
                repeat: HostRepeatEvidence::NotRepeat,
            },
        },
        HostInputEvent::Key {
            device: Some(Arc::clone(&second)),
            identity: HostKeyIdentity {
                key: Arc::from("b"),
                physical_code: Arc::from("windows:30"),
            },
            signal: HostKeySignal::Edge {
                pressed: true,
                repeat: HostRepeatEvidence::Unknown,
            },
        },
        HostInputEvent::Key {
            device: Some(Arc::clone(&first)),
            identity: HostKeyIdentity {
                key: Arc::from("a"),
                physical_code: Arc::from("evdev:30"),
            },
            signal: HostKeySignal::Edge {
                pressed: true,
                repeat: HostRepeatEvidence::Repeat,
            },
        },
        HostInputEvent::DeviceRemoved { device: first },
    ];
    publish(&sink, &events, 50);

    let sample = fold.sample_and_drain();
    assert_eq!(sample.interaction.keyboard.pressed_keys, ["b"]);
    assert_eq!(sample.interaction.keyboard.recent_keys, ["a", "b"]);
    assert_eq!(sample.events.len(), 4);
    assert!(matches!(
        &sample.events[2].event,
        InputEvent::Key {
            key,
            state: InputButtonState::Repeated,
            ..
        } if key == "a"
    ));
    assert!(matches!(
        &sample.events[3].event,
        InputEvent::Key {
            source_id,
            key,
            state: InputButtonState::Released,
        } if source_id == "keyboard-a" && key == "a"
    ));
}

#[test]
fn explicit_repeat_can_establish_held_state_without_becoming_recent() {
    let mut fold = HostInputFold::new(8);
    let sink = fold.begin_session(
        "host",
        HostInputCapabilities {
            keyboard: true,
            pointer: false,
        },
    );
    publish(&sink, &[key("x", true, HostRepeatEvidence::Repeat)], 10);
    let first = fold.sample_and_drain();
    assert_eq!(first.interaction.keyboard.pressed_keys, ["x"]);
    assert!(first.interaction.keyboard.recent_keys.is_empty());
    assert!(matches!(
        first.events[0].event,
        InputEvent::Key {
            state: InputButtonState::Repeated,
            ..
        }
    ));

    publish(
        &sink,
        &[HostInputEvent::StateGap {
            device: None,
            reason: HostInputGapReason::QueueOverflow,
        }],
        11,
    );
    let second = fold.sample_and_drain();
    assert!(second.interaction.keyboard.pressed_keys.is_empty());
    assert!(matches!(
        second.events[0].event,
        InputEvent::Key {
            state: InputButtonState::Released,
            ..
        }
    ));
}

#[test]
fn aggregate_modifier_evidence_resolves_overlap_and_caps_lock_without_backend_state() {
    let mut fold = HostInputFold::new(16);
    let sink = fold.begin_session(
        "macos",
        HostInputCapabilities {
            keyboard: true,
            pointer: false,
        },
    );
    let modifier =
        |name: &'static str, counterpart: Option<&'static str>, active: bool| HostInputEvent::Key {
            device: None,
            identity: HostKeyIdentity {
                key: Arc::from(name),
                physical_code: Arc::from(format!("macos:{name}")),
            },
            signal: HostKeySignal::AggregateState {
                active,
                active_counterpart: counterpart.map(Arc::from),
            },
        };
    publish(
        &sink,
        &[
            modifier("ShiftLeft", Some("ShiftRight"), true),
            modifier("ShiftRight", Some("ShiftLeft"), true),
            modifier("ShiftLeft", Some("ShiftRight"), true),
            modifier("CapsLock", None, true),
            modifier("CapsLock", None, true),
        ],
        20,
    );
    let sample = fold.sample_and_drain();
    assert_eq!(
        sample.interaction.keyboard.pressed_keys,
        ["CapsLock", "ShiftRight"]
    );
    assert_eq!(sample.events.len(), 4);
    assert!(matches!(
        sample.events[2].event,
        InputEvent::Key {
            state: InputButtonState::Released,
            ..
        }
    ));
}

#[test]
fn source_gap_releases_buttons_without_dropping_exact_scroll() {
    let mut fold = HostInputFold::new(16);
    let sink = fold.begin_session(
        "pointer",
        HostInputCapabilities {
            keyboard: false,
            pointer: true,
        },
    );
    publish(
        &sink,
        &[
            HostInputEvent::Button {
                device: None,
                button: HostPointerButton::left(),
                pressed: true,
                physical_code: Arc::from("test:left"),
            },
            HostInputEvent::Scroll {
                device: None,
                delta_x_q16_16: 0,
                delta_y_q16_16: Q16_16_SCALE / 2,
                unit: PointerScrollUnit::Line120,
                phase: PointerScrollPhase::None,
                momentum_phase: PointerScrollPhase::None,
                physical_code: Arc::from("test:scroll"),
            },
            HostInputEvent::StateGap {
                device: None,
                reason: HostInputGapReason::SynchronizationLost,
            },
            HostInputEvent::Scroll {
                device: None,
                delta_x_q16_16: 0,
                delta_y_q16_16: Q16_16_SCALE / 2,
                unit: PointerScrollUnit::Line120,
                phase: PointerScrollPhase::None,
                momentum_phase: PointerScrollPhase::None,
                physical_code: Arc::from("test:scroll"),
            },
        ],
        30,
    );
    let sample = fold.sample_and_drain();
    assert!(sample.interaction.mouse.buttons.is_empty());
    assert_eq!(
        sample
            .events
            .iter()
            .filter(|event| matches!(&event.event, InputEvent::PointerScroll { .. }))
            .count(),
        2
    );
    assert_eq!(fold.diagnostics().state_gaps, 1);
}

#[test]
fn motion_projection_separates_device_catalog_and_coordinate_generations() {
    let mut fold = HostInputFold::new(16);
    let sink = fold.begin_session(
        "pointer",
        HostInputCapabilities {
            keyboard: false,
            pointer: true,
        },
    );
    let device = device("tablet", false, true, 1);
    let first = [HostInputEvent::Motion {
        device: Some(Arc::clone(&device)),
        motion: HostPointerMotion::Absolute {
            norm_x: 0.25,
            norm_y: 0.25,
            coordinate_space_generation: 7,
        },
    }];
    assert_eq!(
        sink.publish(HostInputBatch {
            events: &first,
            pointer: Some(HostPointerSnapshot {
                x: -100.0,
                y: 40.0,
                norm_x: 0.25,
                norm_y: 0.25,
                coordinate_space_generation: 7,
            }),
            at_ms: 40,
            device_catalog_generation: 1,
        }),
        HostInputPublishOutcome::Published
    );
    let second = [HostInputEvent::Motion {
        device: Some(device),
        motion: HostPointerMotion::Absolute {
            norm_x: 0.5,
            norm_y: 0.75,
            coordinate_space_generation: 7,
        },
    }];
    assert_eq!(
        sink.publish(HostInputBatch {
            events: &second,
            pointer: None,
            at_ms: 41,
            device_catalog_generation: 2,
        }),
        HostInputPublishOutcome::Published
    );
    let sample = fold.sample_and_drain();
    assert_eq!(sample.interaction.mouse.mode, PointerMode::Absolute);
    assert_eq!(
        (sample.interaction.mouse.x, sample.interaction.mouse.y),
        (-100, 40)
    );
    assert_eq!(sample.interaction.batch.motion.dx, 0.25);
    assert_eq!(sample.interaction.batch.motion.dy, 0.5);
    assert_eq!(fold.diagnostics().coordinate_space_resets, 0);
    assert_eq!(fold.diagnostics().device_catalog_generation, 2);
}

#[test]
fn independent_pointer_spaces_do_not_reset_each_others_baselines() {
    let mut fold = HostInputFold::new(16);
    let sink = fold.begin_session(
        "pointer",
        HostInputCapabilities {
            keyboard: false,
            pointer: true,
        },
    );
    let tablet = device("tablet", false, true, 1);
    let first = [HostInputEvent::Motion {
        device: Some(Arc::clone(&tablet)),
        motion: HostPointerMotion::Absolute {
            norm_x: 0.1,
            norm_y: 0.2,
            coordinate_space_generation: 41,
        },
    }];
    let _ = sink.publish(HostInputBatch {
        events: &first,
        pointer: Some(HostPointerSnapshot {
            x: 10.0,
            y: 20.0,
            norm_x: 0.1,
            norm_y: 0.2,
            coordinate_space_generation: 9,
        }),
        at_ms: 50,
        device_catalog_generation: 1,
    });
    let second = [HostInputEvent::Motion {
        device: Some(tablet),
        motion: HostPointerMotion::Absolute {
            norm_x: 0.4,
            norm_y: 0.6,
            coordinate_space_generation: 41,
        },
    }];
    let _ = sink.publish(HostInputBatch {
        events: &second,
        pointer: Some(HostPointerSnapshot {
            x: 11.0,
            y: 21.0,
            norm_x: 0.11,
            norm_y: 0.21,
            coordinate_space_generation: 9,
        }),
        at_ms: 51,
        device_catalog_generation: 1,
    });

    let sample = fold.sample_and_drain();
    assert!((sample.interaction.batch.motion.dx - 0.3).abs() < f32::EPSILON);
    assert!((sample.interaction.batch.motion.dy - 0.4).abs() < f32::EPSILON);
    assert_eq!(fold.diagnostics().coordinate_space_resets, 0);
}

#[test]
fn metadata_refresh_preserves_same_incarnation_pointer_state() {
    let mut fold = HostInputFold::new(16);
    let sink = fold.begin_session(
        "pointer",
        HostInputCapabilities {
            keyboard: false,
            pointer: true,
        },
    );
    let original = device("tablet", false, true, 1);
    let first = [
        HostInputEvent::DeviceArrived {
            device: Arc::clone(&original),
        },
        HostInputEvent::Motion {
            device: Some(Arc::clone(&original)),
            motion: HostPointerMotion::Absolute {
                norm_x: 0.1,
                norm_y: 0.2,
                coordinate_space_generation: 7,
            },
        },
        HostInputEvent::Scroll {
            device: Some(Arc::clone(&original)),
            delta_x_q16_16: 0,
            delta_y_q16_16: 1_i64 << 15,
            unit: PointerScrollUnit::Line120,
            phase: PointerScrollPhase::None,
            momentum_phase: PointerScrollPhase::None,
            physical_code: Arc::from("test:scroll"),
        },
    ];
    publish(&sink, &first, 60);

    let refreshed = Arc::new(HostInputDevice {
        source_id: Arc::clone(&original.source_id),
        label: Arc::from("renamed tablet"),
        capabilities: original.capabilities,
        session_generation: original.session_generation,
        device_generation: original.device_generation,
    });
    let second = [
        HostInputEvent::DeviceArrived {
            device: Arc::clone(&refreshed),
        },
        HostInputEvent::Motion {
            device: Some(Arc::clone(&refreshed)),
            motion: HostPointerMotion::Absolute {
                norm_x: 0.4,
                norm_y: 0.6,
                coordinate_space_generation: 7,
            },
        },
        HostInputEvent::Scroll {
            device: Some(refreshed),
            delta_x_q16_16: 0,
            delta_y_q16_16: 1_i64 << 15,
            unit: PointerScrollUnit::Line120,
            phase: PointerScrollPhase::None,
            momentum_phase: PointerScrollPhase::None,
            physical_code: Arc::from("test:scroll"),
        },
    ];
    publish(&sink, &second, 61);

    let sample = fold.sample_and_drain();
    assert!((sample.interaction.batch.motion.dx - 0.3).abs() < f32::EPSILON);
    assert!((sample.interaction.batch.motion.dy - 0.4).abs() < f32::EPSILON);
    assert_eq!(
        sample
            .events
            .iter()
            .filter(|event| {
                matches!(
                    &event.event,
                    InputEvent::PointerScroll {
                        delta_y_q16_16,
                        ..
                    } if *delta_y_q16_16 == Q16_16_SCALE / 2
                )
            })
            .count(),
        2,
        "metadata refresh preserves both exact half-step scroll events"
    );
}

#[test]
fn relative_motion_owns_the_virtual_cursor_projection() {
    let mut fold = HostInputFold::new(8);
    let sink = fold.begin_session(
        "relative-pointer",
        HostInputCapabilities {
            keyboard: false,
            pointer: true,
        },
    );
    publish(
        &sink,
        &[HostInputEvent::Motion {
            device: None,
            motion: HostPointerMotion::Relative {
                delta_x: 300.0,
                delta_y: 600.0,
                units_per_x: 1_200.0,
                units_per_y: 1_200.0,
            },
        }],
        50,
    );
    let sample = fold.sample_and_drain();
    assert_eq!(sample.interaction.mouse.mode, PointerMode::Virtual);
    assert_eq!(sample.interaction.mouse.norm_x, 0.25);
    assert_eq!(sample.interaction.mouse.norm_y, 0.5);
    assert_eq!(
        sample.interaction.batch.motion.distance,
        0.25_f32.hypot(0.5)
    );
}

#[test]
fn stale_publishers_are_inert_after_session_rotation() {
    let mut fold = HostInputFold::new(8);
    let stale = fold.begin_session(
        "old",
        HostInputCapabilities {
            keyboard: true,
            pointer: false,
        },
    );
    let current = fold.begin_session(
        "new",
        HostInputCapabilities {
            keyboard: true,
            pointer: false,
        },
    );
    let events = [key("a", true, HostRepeatEvidence::NotRepeat)];
    assert_eq!(
        stale.publish(HostInputBatch {
            events: &events,
            pointer: None,
            at_ms: 60,
            device_catalog_generation: 1,
        }),
        HostInputPublishOutcome::Stale
    );
    publish(&current, &events, 61);
    assert_eq!(
        fold.sample_and_drain().interaction.keyboard.pressed_keys,
        ["a"]
    );
}

#[test]
fn snapshot_and_event_drains_remain_independent() {
    let mut fold = HostInputFold::new(8);
    let sink = fold.begin_session(
        "host",
        HostInputCapabilities {
            keyboard: true,
            pointer: false,
        },
    );
    publish(
        &sink,
        &[HostInputEvent::Key {
            device: None,
            identity: HostKeyIdentity {
                key: Arc::from("a"),
                physical_code: Arc::from("test:a"),
            },
            signal: HostKeySignal::Edge {
                pressed: true,
                repeat: HostRepeatEvidence::NotRepeat,
            },
        }],
        80,
    );

    let snapshot = fold.sample();
    assert_eq!(snapshot.keyboard.pressed_keys, ["a"]);
    assert_eq!(snapshot.keyboard.recent_keys, ["a"]);
    let events = fold.drain_events();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0].event,
        InputEvent::Key {
            state: InputButtonState::Pressed,
            ..
        }
    ));
    assert!(fold.drain_events().is_empty());
}

#[test]
fn final_event_eviction_preserves_folded_held_state() {
    let mut fold = HostInputFold::new(2);
    let sink = fold.begin_session(
        "host",
        HostInputCapabilities {
            keyboard: true,
            pointer: false,
        },
    );
    publish(
        &sink,
        &[
            key("a", true, HostRepeatEvidence::NotRepeat),
            key("b", true, HostRepeatEvidence::NotRepeat),
            key("c", true, HostRepeatEvidence::NotRepeat),
        ],
        70,
    );
    let sample = fold.sample_and_drain();
    assert_eq!(sample.interaction.keyboard.pressed_keys, ["a", "b", "c"]);
    assert_eq!(sample.events.len(), 2);
    assert_eq!(sample.interaction.batch.dropped_events, 1);
}
