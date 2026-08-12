//! macOS host-input folding and deterministic adapter-boundary contracts.

use hypercolor_core::input::{MacosHostInput, PointerMode, Q16_16_SCALE};
use hypercolor_core::types::event::{
    InputButtonState, InputEvent, PointerScrollPhase, PointerScrollUnit,
};
use hypercolor_macos_input::{
    MacosInputBatch, MacosInputEvent, MacosInputGapReason, MacosModifierFlags, MacosPointerButton,
    MacosScrollPhase, MacosScrollUnit, MacosVirtualDesktop,
};

fn desktop(topology_generation: u64) -> MacosVirtualDesktop {
    MacosVirtualDesktop::new(-200.0, -100.0, 400.0, 200.0, topology_generation)
        .expect("fixture desktop is valid")
}

fn fold(
    input: &mut MacosHostInput,
    events: &[MacosInputEvent],
) -> (
    hypercolor_core::input::InteractionData,
    Vec<hypercolor_core::types::event::TimedInputEvent>,
) {
    input.fold_and_snapshot(MacosInputBatch {
        epoch: input.epoch(),
        at_ms: 100,
        events,
        virtual_desktop: desktop(1),
    })
}

fn key_states(events: &[hypercolor_core::types::event::TimedInputEvent]) -> Vec<InputButtonState> {
    events
        .iter()
        .filter_map(|event| match event.event {
            InputEvent::Key { state, .. } => Some(state),
            _ => None,
        })
        .collect()
}

#[test]
fn native_repeat_and_impossible_edges_preserve_canonical_state() {
    let mut input = MacosHostInput::new(true, false);
    let events = [
        MacosInputEvent::Key {
            virtual_keycode: 0x00,
            pressed: true,
            autorepeat: false,
        },
        MacosInputEvent::Key {
            virtual_keycode: 0x00,
            pressed: true,
            autorepeat: true,
        },
        MacosInputEvent::Key {
            virtual_keycode: 0x00,
            pressed: false,
            autorepeat: false,
        },
        MacosInputEvent::Key {
            virtual_keycode: 0x00,
            pressed: false,
            autorepeat: false,
        },
        MacosInputEvent::Key {
            virtual_keycode: 0x00,
            pressed: true,
            autorepeat: true,
        },
    ];

    let (data, folded) = fold(&mut input, &events);

    assert!(data.keyboard.pressed_keys.is_empty());
    assert_eq!(data.keyboard.recent_keys, ["a"]);
    assert_eq!(
        key_states(&folded),
        [
            InputButtonState::Pressed,
            InputButtonState::Repeated,
            InputButtonState::Released,
            InputButtonState::Released,
            InputButtonState::Repeated,
        ]
    );
    assert_eq!(input.fold_diagnostics().impossible_key_edges, 2);
}

#[test]
fn modifier_flags_keep_sides_distinct_and_toggle_caps_lock() {
    let mut input = MacosHostInput::new(true, false);
    let shift = MacosModifierFlags::SHIFT;
    let caps = MacosModifierFlags::ALPHA_SHIFT;
    let events = [
        MacosInputEvent::ModifierFlags {
            virtual_keycode: 0x38,
            flags: shift,
        },
        MacosInputEvent::ModifierFlags {
            virtual_keycode: 0x3c,
            flags: shift,
        },
        MacosInputEvent::ModifierFlags {
            virtual_keycode: 0x38,
            flags: shift,
        },
        MacosInputEvent::ModifierFlags {
            virtual_keycode: 0x3c,
            flags: MacosModifierFlags::default(),
        },
        MacosInputEvent::ModifierFlags {
            virtual_keycode: 0x39,
            flags: caps,
        },
        MacosInputEvent::ModifierFlags {
            virtual_keycode: 0x39,
            flags: MacosModifierFlags::default(),
        },
    ];

    let (data, folded) = fold(&mut input, &events);

    assert!(data.keyboard.pressed_keys.is_empty());
    assert_eq!(
        key_states(&folded),
        [
            InputButtonState::Pressed,
            InputButtonState::Pressed,
            InputButtonState::Released,
            InputButtonState::Released,
            InputButtonState::Pressed,
            InputButtonState::Released,
        ]
    );
}

#[test]
fn media_keys_and_extra_buttons_use_canonical_names() {
    let mut input = MacosHostInput::new(true, true);
    let events = [
        MacosInputEvent::MediaKey {
            nx_key_type: 16,
            pressed: true,
            repeat: false,
        },
        MacosInputEvent::Button {
            button: MacosPointerButton::Other(3),
            pressed: true,
        },
    ];

    let (data, folded) = fold(&mut input, &events);

    assert_eq!(data.keyboard.pressed_keys, ["MediaPlayPause"]);
    assert_eq!(data.mouse.buttons, ["button4"]);
    assert!(matches!(
        &folded[0].event,
        InputEvent::Key { key, .. } if key == "MediaPlayPause"
    ));
    assert!(matches!(
        &folded[1].event,
        InputEvent::MouseButton { button, .. } if button == "button4"
    ));
}

#[test]
fn physical_wheel_emits_exact_axes_then_legacy_vertical_shadow() {
    let mut input = MacosHostInput::new(false, true);
    let events = [MacosInputEvent::Wheel {
        fixed_delta_x: Q16_16_SCALE,
        fixed_delta_y: -2 * Q16_16_SCALE,
        unit: MacosScrollUnit::Notches,
        phase: MacosScrollPhase::Changed,
        momentum_phase: MacosScrollPhase::None,
    }];

    let (_, folded) = fold(&mut input, &events);

    assert!(matches!(
        folded[0].event,
        InputEvent::PointerScroll {
            delta_x_q16_16: 7_864_320,
            delta_y_q16_16: -15_728_640,
            unit: PointerScrollUnit::Line120,
            phase: PointerScrollPhase::Changed,
            momentum_phase: PointerScrollPhase::None,
            ..
        }
    ));
    assert!(matches!(
        folded[1].event,
        InputEvent::MouseWheel {
            delta_hi_res: -240,
            ..
        }
    ));
}

#[test]
fn subunit_wheel_motion_carries_fractional_remainder() {
    let mut input = MacosHostInput::new(false, true);
    let wheel = [MacosInputEvent::Wheel {
        fixed_delta_x: 0,
        fixed_delta_y: 1,
        unit: MacosScrollUnit::Notches,
        phase: MacosScrollPhase::None,
        momentum_phase: MacosScrollPhase::None,
    }];
    let mut legacy_total = 0;

    for _ in 0..547 {
        let (_, folded) = fold(&mut input, &wheel);
        legacy_total += folded
            .iter()
            .filter_map(|event| match event.event {
                InputEvent::MouseWheel { delta_hi_res, .. } => Some(delta_hi_res),
                _ => None,
            })
            .sum::<i32>();
    }

    assert_eq!(legacy_total, 1);
}

#[test]
fn continuous_scroll_preserves_pixels_and_phases_without_legacy_shadow() {
    let mut input = MacosHostInput::new(false, true);
    let events = [MacosInputEvent::Wheel {
        fixed_delta_x: 3 * Q16_16_SCALE,
        fixed_delta_y: -4 * Q16_16_SCALE,
        unit: MacosScrollUnit::Pixels,
        phase: MacosScrollPhase::Began,
        momentum_phase: MacosScrollPhase::MayBegin,
    }];

    let (_, folded) = fold(&mut input, &events);

    assert_eq!(folded.len(), 1);
    assert!(matches!(
        folded[0].event,
        InputEvent::PointerScroll {
            unit: PointerScrollUnit::Pixels,
            phase: PointerScrollPhase::Began,
            momentum_phase: PointerScrollPhase::MayBegin,
            ..
        }
    ));
}

#[test]
fn motion_normalizes_negative_origins_and_resets_on_topology_change() {
    let mut input = MacosHostInput::new(false, true);
    let first = [MacosInputEvent::Motion {
        x: -100.0,
        y: 0.0,
        delta_x: 90.0,
        delta_y: 90.0,
    }];
    let second = [MacosInputEvent::Motion {
        x: 100.0,
        y: 50.0,
        delta_x: 20.0,
        delta_y: -10.0,
    }];

    let (first_data, _) = fold(&mut input, &first);
    let (second_data, _) = fold(&mut input, &second);
    let (reset_data, _) = input.fold_and_snapshot(MacosInputBatch {
        epoch: input.epoch(),
        at_ms: 101,
        events: &second,
        virtual_desktop: desktop(2),
    });

    assert_eq!(first_data.mouse.mode, PointerMode::Absolute);
    assert_eq!((first_data.mouse.x, first_data.mouse.y), (-100, 0));
    assert_eq!(
        (first_data.mouse.norm_x, first_data.mouse.norm_y),
        (0.25, 0.5)
    );
    assert!((second_data.batch.motion.dx - 0.05).abs() < f32::EPSILON);
    assert!((second_data.batch.motion.dy + 0.05).abs() < f32::EPSILON);
    assert_eq!(reset_data.batch.motion.dx, 0.0);
    assert_eq!(reset_data.batch.motion.dy, 0.0);
    assert_eq!(input.fold_diagnostics().topology_resets, 2);
}

#[test]
fn state_gap_synthesizes_releases_and_stale_epoch_is_inert() {
    let mut input = MacosHostInput::new(true, true);
    let held = [
        MacosInputEvent::Key {
            virtual_keycode: 0x00,
            pressed: true,
            autorepeat: false,
        },
        MacosInputEvent::Button {
            button: MacosPointerButton::Left,
            pressed: true,
        },
    ];
    fold(&mut input, &held);
    let gap = [MacosInputEvent::StateGap {
        reason: MacosInputGapReason::QueueOverflow,
    }];

    let (data, releases) = fold(&mut input, &gap);
    let (_, stale) = input.fold_and_snapshot(MacosInputBatch {
        epoch: input.epoch().wrapping_add(1),
        at_ms: 102,
        events: &held,
        virtual_desktop: desktop(1),
    });

    assert!(data.keyboard.pressed_keys.is_empty());
    assert!(data.mouse.buttons.is_empty());
    assert_eq!(releases.len(), 2);
    assert!(releases.iter().all(|event| matches!(
        event.event,
        InputEvent::Key {
            state: InputButtonState::Released,
            ..
        } | InputEvent::MouseButton {
            state: InputButtonState::Released,
            ..
        }
    )));
    assert!(stale.is_empty());
    assert_eq!(input.fold_diagnostics().state_gaps, 1);
}

#[cfg(feature = "macos-native-fixtures")]
mod fixtures {
    use std::sync::Arc;

    use hypercolor_core::input::{
        InputData, InputSource, MacosAuthorizationState, MacosCapabilityOwner,
        MacosDaemonOwnerConflict, MacosHostInput, MacosInputFixtureBackend,
        MacosProtectedSourceState, SourcePlatformStatus, SourceState,
    };
    use hypercolor_macos_input::{MacosInputEvent, event_masks};

    use super::desktop;

    #[test]
    fn denied_keyboard_permission_keeps_pointer_capture_live() {
        let backend =
            MacosInputFixtureBackend::new(false, true, event_masks(false, true), true, desktop(1));
        let (mut source, fixture) = MacosHostInput::new_deterministic_fixture(true, true, backend);
        let status = source
            .source_status_handle()
            .expect("macOS host source exposes status");

        source.set_source_graph_generation(1);
        source.start().expect("fixture starts idle");
        assert!(!fixture.is_active());
        source
            .set_interaction_capture_active(true)
            .expect("pointer capture activates without keyboard permission");
        assert!(fixture.is_active());
        assert_eq!(status.snapshot().state, SourceState::Degraded);
        assert_eq!(status.snapshot().resource_count, 1);
        let snapshot = status.snapshot();
        let Some(SourcePlatformStatus::MacosInput(platform)) = snapshot.platform.as_deref() else {
            panic!("fixture should publish macOS input platform status");
        };
        assert_eq!(
            platform.keyboard,
            MacosProtectedSourceState::NeedsUserAction
        );
        assert_eq!(platform.pointer, MacosProtectedSourceState::Live);
        assert_eq!(
            platform.keyboard_tcc,
            MacosAuthorizationState::NotDetermined
        );
        assert_eq!(platform.keyboard_owner, MacosCapabilityOwner::Standalone);
        assert_eq!(platform.pointer_owner, MacosCapabilityOwner::Standalone);
        assert_eq!(
            status
                .snapshot()
                .issue
                .as_ref()
                .expect("permission issue is published")
                .code
                .as_ref(),
            "macos_input_permission_denied"
        );

        fixture
            .publish(
                &[MacosInputEvent::Button {
                    button: hypercolor_macos_input::MacosPointerButton::Left,
                    pressed: true,
                }],
                100,
            )
            .expect("pointer batch publishes");
        let InputData::Interaction(sample) = source.sample().expect("fixture sample succeeds")
        else {
            panic!("expected interaction sample");
        };
        assert_eq!(sample.mouse.buttons, ["left"]);
        assert_eq!(status.snapshot().state, SourceState::Degraded);
    }

    #[test]
    fn fixture_masks_and_epochs_enforce_demand_lifecycle() {
        let backend =
            MacosInputFixtureBackend::new(true, true, event_masks(true, false), true, desktop(1));
        let (mut source, fixture) = MacosHostInput::new_deterministic_fixture(true, false, backend);
        source.set_source_graph_generation(1);
        source.start().expect("fixture starts idle");
        source
            .set_interaction_capture_active(true)
            .expect("keyboard fixture activates");
        let first_epoch = fixture.active_epoch().expect("fixture owns one epoch");

        source
            .set_interaction_capture_active(false)
            .expect("fixture deactivates");
        assert!(!fixture.is_active());
        source.set_source_graph_generation(2);
        source
            .set_interaction_capture_active(true)
            .expect("fixture reactivates");
        assert_ne!(fixture.active_epoch(), Some(first_epoch));
        assert!(
            !fixture
                .publish_with_epoch(
                    first_epoch,
                    &[MacosInputEvent::Key {
                        virtual_keycode: 0x00,
                        pressed: true,
                        autorepeat: false,
                    }],
                    200,
                )
                .expect("stale publication is rejected without an error")
        );

        source.stop();
        assert!(!fixture.is_active());
    }

    #[test]
    fn empty_effective_masks_publish_unavailable_status() {
        let backend =
            MacosInputFixtureBackend::new(true, true, event_masks(false, false), true, desktop(1));
        let (mut source, fixture) = MacosHostInput::new_deterministic_fixture(true, true, backend);
        let status = source
            .source_status_handle()
            .expect("macOS host source exposes status");

        source.set_source_graph_generation(1);
        source.start().expect("fixture starts idle");
        source
            .set_interaction_capture_active(true)
            .expect("empty masks produce typed status");

        assert!(!fixture.is_active());
        assert_eq!(status.snapshot().state, SourceState::Unavailable);
        assert_eq!(
            status
                .snapshot()
                .issue
                .as_ref()
                .expect("mask issue is published")
                .code
                .as_ref(),
            "macos_input_tap_create_failed"
        );
        let snapshot = status.snapshot();
        let Some(SourcePlatformStatus::MacosInput(platform)) = snapshot.platform.as_deref() else {
            panic!("fixture should publish macOS input platform status");
        };
        assert_eq!(
            platform.keyboard,
            MacosProtectedSourceState::NeedsProcessRestart
        );
        assert_eq!(platform.pointer, MacosProtectedSourceState::Failed);
        assert_eq!(platform.keyboard_tcc, MacosAuthorizationState::Authorized);
    }

    #[test]
    fn capability_owner_updates_both_input_kinds() {
        let backend =
            MacosInputFixtureBackend::new(true, true, event_masks(true, true), true, desktop(1));
        let (mut source, _) = MacosHostInput::new_deterministic_fixture(true, true, backend);
        let status = source
            .source_status_handle()
            .expect("macOS host source exposes status");

        source
            .set_macos_daemon_ownership(
                MacosCapabilityOwner::AppSidecar,
                Some(MacosDaemonOwnerConflict {
                    active: MacosCapabilityOwner::AppSidecar,
                    contender: MacosCapabilityOwner::HomebrewService,
                    observed_at_ms: 42,
                }),
                Some(Arc::from("designated-app-sidecar")),
            )
            .expect("owner update should publish");

        let snapshot = status.snapshot();
        let Some(SourcePlatformStatus::MacosInput(platform)) = snapshot.platform.as_deref() else {
            panic!("fixture should publish macOS input platform status");
        };
        assert_eq!(platform.keyboard_owner, MacosCapabilityOwner::AppSidecar);
        assert_eq!(platform.pointer_owner, MacosCapabilityOwner::AppSidecar);
        assert_eq!(
            platform.owner_conflict.as_deref(),
            Some(&MacosDaemonOwnerConflict {
                active: MacosCapabilityOwner::AppSidecar,
                contender: MacosCapabilityOwner::HomebrewService,
                observed_at_ms: 42,
            })
        );
        assert_eq!(
            platform.owner_designated_requirement_hash.as_deref(),
            Some("designated-app-sidecar")
        );
    }

    #[test]
    fn permission_request_fixture_reports_owner_restart_result() {
        let backend =
            MacosInputFixtureBackend::new(false, true, event_masks(true, true), true, desktop(1));
        let (_, fixture) = MacosHostInput::new_deterministic_fixture(true, true, backend);

        assert!(
            fixture
                .request_input_monitoring_and_restart_owner()
                .expect("owner restart succeeds")
        );
    }

    #[test]
    fn authorization_action_publishes_granted_tcc_without_graph_locking() {
        let backend =
            MacosInputFixtureBackend::new(false, true, event_masks(true, true), true, desktop(1));
        let (mut source, _) = MacosHostInput::new_deterministic_fixture(true, true, backend);
        let status = source
            .source_status_handle()
            .expect("macOS host source exposes status");
        let action = source
            .input_authorization_action()
            .expect("keyboard source should expose authorization");

        assert!(action().expect("fixture authorization should succeed"));
        source
            .sample()
            .expect("source should consume action result");

        let snapshot = status.snapshot();
        let Some(SourcePlatformStatus::MacosInput(platform)) = snapshot.platform.as_deref() else {
            panic!("fixture should publish macOS input platform status");
        };
        assert_eq!(platform.keyboard_tcc, MacosAuthorizationState::Authorized);
        assert_eq!(platform.keyboard, MacosProtectedSourceState::ReadyIdle);
        assert!(platform.authorization_last_transition_at.is_some());
        assert_eq!(
            platform.executable_architecture,
            if cfg!(target_arch = "aarch64") {
                hypercolor_core::input::MacosArchitecture::AppleSilicon
            } else {
                hypercolor_core::input::MacosArchitecture::Intel
            }
        );
        if cfg!(target_os = "macos") {
            assert!(platform.host_architecture.is_some());
            assert!(platform.translated_process.is_some());
        } else {
            assert_eq!(platform.host_architecture, None);
            assert_eq!(platform.translated_process, None);
        }
    }
}
