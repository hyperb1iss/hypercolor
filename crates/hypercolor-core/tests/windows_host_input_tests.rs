//! Windows host input folding.
//!
//! Driven directly through `fold_and_snapshot` rather than through a live Raw
//! Input session, for two reasons. Everything asserted here is a pure state
//! machine — repeat classification, held-state union, release synthesis,
//! absolute baselines, epoch rejection — and a live session makes those
//! *harder* to exercise, not easier: you cannot make real hardware produce a
//! rollover overrun or a stale-epoch batch on demand. And running them off
//! Windows means Linux CI covers them too.

use std::sync::Arc;

use hypercolor_core::input::{PointerMode, WindowsHostInput};
use hypercolor_core::types::event::{InputButtonState, InputEvent};
use hypercolor_windows_input::{
    RawButton, RawCursor, RawDeviceDescriptor, RawDeviceKind, RawInputBatch, RawInputEvent,
    RawKeyPrefix,
};

const KBD_A: &str = "\\\\?\\HID#VID_046D&PID_C52B#kbd-a";
const KBD_B: &str = "\\\\?\\HID#VID_1532&PID_0226#kbd-b";
const MOUSE: &str = "\\\\?\\HID#VID_046D&PID_C08B#mouse";

fn device(id: &str, kind: RawDeviceKind) -> Arc<RawDeviceDescriptor> {
    Arc::new(RawDeviceDescriptor {
        source_id: Arc::from(id),
        interface_path: Some(Arc::from(id)),
        label: Arc::from(format!("test {id}")),
        kind,
        session_generation: 1,
        device_generation: 1,
    })
}

fn key(id: &str, make_code: u16, pressed: bool) -> RawInputEvent {
    RawInputEvent::Key {
        device: device(id, RawDeviceKind::Keyboard),
        make_code,
        prefix: RawKeyPrefix::None,
        vkey: 0,
        pressed,
    }
}

fn absolute(id: &str, norm_x: f32, norm_y: f32) -> RawInputEvent {
    RawInputEvent::MotionAbsolute {
        device: device(id, RawDeviceKind::Mouse),
        norm_x,
        norm_y,
        virtual_desktop: true,
    }
}

fn arrived(id: &str, kind: RawDeviceKind) -> RawInputEvent {
    RawInputEvent::DeviceArrived {
        device: device(id, kind),
    }
}

/// Fold one batch at the source's current epoch and take the frame.
fn fold(
    input: &mut WindowsHostInput,
    events: &[RawInputEvent],
) -> (
    hypercolor_core::input::InteractionData,
    Vec<hypercolor_core::types::event::TimedInputEvent>,
) {
    let epoch = input.epoch();
    input.fold_and_snapshot(RawInputBatch {
        events,
        cursor: None,
        at_ms: 100,
        epoch,
    })
}

// ── Keys ───────────────────────────────────────────────────────────────────

#[test]
fn a_press_lands_in_held_state_and_recents() {
    let mut input = WindowsHostInput::new(true, true);
    let (data, events) = fold(&mut input, &[key(KBD_A, 0x1E, true)]);

    assert_eq!(data.keyboard.pressed_keys, vec!["a".to_owned()]);
    assert_eq!(data.keyboard.recent_keys, vec!["a".to_owned()]);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0].event,
        InputEvent::Key {
            state: InputButtonState::Pressed,
            ..
        }
    ));
    assert_eq!(
        events[0].physical_code.as_deref(),
        Some("windows:set1:none:1e")
    );
    assert_eq!(events[0].repeat_count, 1);
}

#[test]
fn a_release_clears_held_state() {
    let mut input = WindowsHostInput::new(true, true);
    fold(&mut input, &[key(KBD_A, 0x1E, true)]);
    let (data, events) = fold(&mut input, &[key(KBD_A, 0x1E, false)]);

    assert!(data.keyboard.pressed_keys.is_empty());
    assert!(matches!(
        &events[0].event,
        InputEvent::Key {
            state: InputButtonState::Released,
            ..
        }
    ));
}

#[test]
fn a_held_key_repeats_without_machine_gunning_recents() {
    // Windows delivers auto-repeat as ordinary make codes with no repeat
    // marker. Classifying every one as a fresh press would fire onKeyPress
    // sixty times a second on Windows and once on Linux.
    let mut input = WindowsHostInput::new(true, true);
    fold(&mut input, &[key(KBD_A, 0x1E, true)]);

    let (data, events) = fold(
        &mut input,
        &[
            key(KBD_A, 0x1E, true),
            key(KBD_A, 0x1E, true),
            key(KBD_A, 0x1E, true),
        ],
    );

    assert!(
        data.keyboard.recent_keys.is_empty(),
        "repeats must not reach recent_keys, got {:?}",
        data.keyboard.recent_keys
    );
    assert_eq!(
        data.keyboard.pressed_keys,
        vec!["a".to_owned()],
        "the key stays held exactly once"
    );
    assert_eq!(events.len(), 3, "the edges are still delivered to the bus");
    assert!(events.iter().all(|timed| matches!(
        &timed.event,
        InputEvent::Key {
            state: InputButtonState::Repeated,
            ..
        }
    )));
    assert!(events.iter().all(|timed| {
        timed.physical_code.as_deref() == Some("windows:set1:none:1e") && timed.repeat_count == 1
    }));
}

#[test]
fn a_key_held_across_a_release_presses_fresh_again() {
    let mut input = WindowsHostInput::new(true, true);
    fold(&mut input, &[key(KBD_A, 0x1E, true)]);
    fold(&mut input, &[key(KBD_A, 0x1E, false)]);
    let (data, _) = fold(&mut input, &[key(KBD_A, 0x1E, true)]);

    assert_eq!(
        data.keyboard.recent_keys,
        vec!["a".to_owned()],
        "a genuine re-press is not a repeat"
    );
}

#[test]
fn held_keys_union_across_two_keyboards() {
    let mut input = WindowsHostInput::new(true, true);
    let (data, _) = fold(
        &mut input,
        &[key(KBD_A, 0x1E, true), key(KBD_B, 0x30, true)],
    );
    assert_eq!(
        data.keyboard.pressed_keys,
        vec!["a".to_owned(), "b".to_owned()]
    );
}

#[test]
fn a_release_on_one_keyboard_does_not_clear_anothers_hold() {
    // Two keyboards can hold the same physical key. Tracking held state
    // globally rather than per source would let either release clear both.
    let mut input = WindowsHostInput::new(true, true);
    fold(
        &mut input,
        &[key(KBD_A, 0x1E, true), key(KBD_B, 0x1E, true)],
    );
    let (data, _) = fold(&mut input, &[key(KBD_A, 0x1E, false)]);

    assert_eq!(
        data.keyboard.pressed_keys,
        vec!["a".to_owned()],
        "the second keyboard is still holding it"
    );
}

// ── Buttons and wheel ──────────────────────────────────────────────────────

#[test]
fn buttons_track_per_source_and_set_the_down_flag() {
    let mut input = WindowsHostInput::new(true, true);
    let (data, events) = fold(
        &mut input,
        &[RawInputEvent::Button {
            device: device(MOUSE, RawDeviceKind::Mouse),
            button: RawButton::Left,
            pressed: true,
        }],
    );
    assert_eq!(data.mouse.buttons, vec!["left".to_owned()]);
    assert!(data.mouse.down);
    assert_eq!(
        events[0].physical_code.as_deref(),
        Some("windows:button:left")
    );
}

#[test]
fn a_click_in_one_batch_leaves_nothing_held() {
    let mut input = WindowsHostInput::new(true, true);
    let (data, events) = fold(
        &mut input,
        &[
            RawInputEvent::Button {
                device: device(MOUSE, RawDeviceKind::Mouse),
                button: RawButton::Left,
                pressed: true,
            },
            RawInputEvent::Button {
                device: device(MOUSE, RawDeviceKind::Mouse),
                button: RawButton::Left,
                pressed: false,
            },
        ],
    );
    assert!(data.mouse.buttons.is_empty());
    assert!(!data.mouse.down);
    assert_eq!(events.len(), 2, "both edges reach the bus, down then up");
}

#[test]
fn wheel_travel_reaches_the_event_bus_unscaled() {
    let mut input = WindowsHostInput::new(true, true);
    let (_, events) = fold(
        &mut input,
        &[RawInputEvent::Wheel {
            device: device(MOUSE, RawDeviceKind::Mouse),
            delta_hi_res: -120,
        }],
    );
    assert!(matches!(
        &events[0].event,
        InputEvent::MouseWheel {
            delta_hi_res: -120,
            ..
        }
    ));
    assert_eq!(
        events[0].physical_code.as_deref(),
        Some("windows:wheel:vertical")
    );
}

// ── Devices ────────────────────────────────────────────────────────────────

#[test]
fn a_mouse_arrival_makes_the_pointer_present() {
    let mut input = WindowsHostInput::new(true, true);
    let epoch = input.epoch();
    let events = [arrived(MOUSE, RawDeviceKind::Mouse)];
    let (data, _) = input.fold_and_snapshot(RawInputBatch {
        events: &events,
        cursor: Some(RawCursor {
            x: 960,
            y: 540,
            norm_x: 0.5,
            norm_y: 0.5,
        }),
        at_ms: 1,
        epoch,
    });

    assert_eq!(data.mouse.mode, PointerMode::Absolute);
    assert!((data.mouse.norm_x - 0.5).abs() < 1e-6);
    assert_eq!(
        data.mouse.x, 960,
        "Windows reports real pixels, unlike Linux"
    );
    assert!(!data.mouse.injected, "a host pointer is not injected");
}

#[test]
fn a_keyboard_alone_leaves_the_pointer_absent() {
    let mut input = WindowsHostInput::new(true, true);
    let (data, _) = fold(&mut input, &[arrived(KBD_A, RawDeviceKind::Keyboard)]);
    assert_eq!(data.mouse.mode, PointerMode::None);
}

#[test]
fn removing_a_device_synthesizes_releases_for_everything_it_held() {
    // Same invariant as the Linux backend: unplugging a keyboard mid-hold must
    // never leave a key stuck on forever.
    let mut input = WindowsHostInput::new(true, true);
    fold(
        &mut input,
        &[
            arrived(KBD_A, RawDeviceKind::Keyboard),
            key(KBD_A, 0x1E, true),
            key(KBD_A, 0x30, true),
        ],
    );

    let (data, events) = fold(
        &mut input,
        &[RawInputEvent::DeviceRemoved {
            device: device(KBD_A, RawDeviceKind::Keyboard),
        }],
    );

    assert!(data.keyboard.pressed_keys.is_empty());
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|timed| matches!(
        &timed.event,
        InputEvent::Key {
            state: InputButtonState::Released,
            ..
        }
    )));
    assert!(events.iter().all(|timed| timed.physical_code.is_none()));
}

#[test]
fn removing_one_device_leaves_anothers_held_state_alone() {
    let mut input = WindowsHostInput::new(true, true);
    fold(
        &mut input,
        &[key(KBD_A, 0x1E, true), key(KBD_B, 0x30, true)],
    );
    let (data, _) = fold(
        &mut input,
        &[RawInputEvent::DeviceRemoved {
            device: device(KBD_A, RawDeviceKind::Keyboard),
        }],
    );
    assert_eq!(data.keyboard.pressed_keys, vec!["b".to_owned()]);
}

// ── State gap ──────────────────────────────────────────────────────────────

#[test]
fn a_rollover_overrun_releases_that_sources_held_keys_immediately() {
    // A keyboard past its rollover limit no longer knows what it is holding.
    // Waiting for a "quiet moment" would leave keys stuck for as long as the
    // user keeps mashing — which during a mashing burst is the entire time.
    let mut input = WindowsHostInput::new(true, true);
    fold(
        &mut input,
        &[
            key(KBD_A, 0x1E, true),
            key(KBD_A, 0x30, true),
            key(KBD_B, 0x2E, true),
        ],
    );

    let (data, events) = fold(
        &mut input,
        &[RawInputEvent::StateGap {
            device: device(KBD_A, RawDeviceKind::Keyboard),
        }],
    );

    assert_eq!(
        data.keyboard.pressed_keys,
        vec!["c".to_owned()],
        "only the overrun source is released"
    );
    assert_eq!(events.len(), 2);
}

#[test]
fn a_state_gap_is_a_barrier_for_events_later_in_the_same_batch() {
    // The gap and the keys that follow it arrive in one batch. If the gap were
    // applied after them, the fresh press would be released by the gap that
    // preceded it in the stream.
    let mut input = WindowsHostInput::new(true, true);
    fold(&mut input, &[key(KBD_A, 0x1E, true)]);

    let (data, _) = fold(
        &mut input,
        &[
            RawInputEvent::StateGap {
                device: device(KBD_A, RawDeviceKind::Keyboard),
            },
            key(KBD_A, 0x30, true),
        ],
    );

    assert_eq!(
        data.keyboard.pressed_keys,
        vec!["b".to_owned()],
        "the key pressed after the gap survives it"
    );
}

// ── Motion ─────────────────────────────────────────────────────────────────

#[test]
fn relative_motion_sums_and_accumulates_path_length() {
    let mut input = WindowsHostInput::new(true, true);
    let (data, _) = fold(
        &mut input,
        &[
            RawInputEvent::MotionRelative {
                device: device(MOUSE, RawDeviceKind::Mouse),
                dx: 600,
                dy: 0,
            },
            RawInputEvent::MotionRelative {
                device: device(MOUSE, RawDeviceKind::Mouse),
                dx: -600,
                dy: 0,
            },
        ],
    );

    assert!(
        data.batch.motion.dx.abs() < 1e-6,
        "equal and opposite deltas net to zero"
    );
    assert!(
        data.batch.motion.distance > 0.9,
        "but the path length is real, so a fast shake still reports velocity: {}",
        data.batch.motion.distance
    );
}

#[test]
fn the_first_absolute_report_after_a_reset_emits_no_delta() {
    // A tablet appearing at the far corner has not swept across the desk.
    let mut input = WindowsHostInput::new(true, true);
    let (data, _) = fold(&mut input, &[absolute(MOUSE, 0.9, 0.9)]);
    assert!(data.batch.motion.dx.abs() < 1e-6);
    assert!(data.batch.motion.dy.abs() < 1e-6);
}

#[test]
fn delayed_duplicate_arrival_preserves_the_absolute_baseline() {
    let mut input = WindowsHostInput::new(true, true);
    fold(
        &mut input,
        &[
            arrived(MOUSE, RawDeviceKind::Mouse),
            absolute(MOUSE, 0.2, 0.3),
        ],
    );
    fold(&mut input, &[arrived(MOUSE, RawDeviceKind::Mouse)]);
    let (data, _) = fold(&mut input, &[absolute(MOUSE, 0.7, 0.3)]);

    assert!(
        (data.batch.motion.dx - 0.5).abs() < 1e-6,
        "metadata refresh must not reset a baseline established by first data"
    );
}

#[test]
fn absolute_motion_differences_successive_positions() {
    let mut input = WindowsHostInput::new(true, true);
    fold(&mut input, &[absolute(MOUSE, 0.25, 0.25)]);
    let (data, _) = fold(&mut input, &[absolute(MOUSE, 0.75, 0.25)]);
    assert!((data.batch.motion.dx - 0.5).abs() < 1e-6);
    assert!(data.batch.motion.dy.abs() < 1e-6);
}

#[test]
fn a_device_arriving_resets_its_absolute_baseline() {
    // Handles are recycled, so the source id can outlive the device that owned
    // it. A new device's first report must not be a delta from its predecessor.
    let mut input = WindowsHostInput::new(true, true);
    fold(&mut input, &[absolute(MOUSE, 0.1, 0.1)]);
    let (data, _) = fold(
        &mut input,
        &[
            arrived(MOUSE, RawDeviceKind::Mouse),
            absolute(MOUSE, 0.9, 0.9),
        ],
    );
    assert!(
        data.batch.motion.distance < 1e-6,
        "arrival resets the baseline, so this is a first report"
    );
}

// ── Generation ─────────────────────────────────────────────────────────────

#[test]
fn the_generation_advances_when_held_state_changes() {
    let mut input = WindowsHostInput::new(true, true);
    let (first, _) = fold(&mut input, &[key(KBD_A, 0x1E, true)]);
    let (second, _) = fold(&mut input, &[key(KBD_A, 0x30, true)]);
    assert_ne!(first.generation, second.generation);
}

#[test]
fn an_idle_frame_does_not_advance_the_generation() {
    let mut input = WindowsHostInput::new(true, true);
    fold(&mut input, &[key(KBD_A, 0x1E, true)]);
    let (first, _) = fold(&mut input, &[]);
    let (second, _) = fold(&mut input, &[]);
    assert_eq!(
        first.generation, second.generation,
        "nothing changed, so renderers can skip the frame"
    );
}

#[test]
fn a_sub_bucket_pixel_move_still_advances_the_generation() {
    // On a wide virtual desktop several distinct pixel positions collapse into
    // one 1/10,000 normalized bucket. Keying the generation on normalized
    // coordinates alone would leave a cursor move invisible to effects reading
    // the pixel fields.
    let mut input = WindowsHostInput::new(true, true);
    let epoch = input.epoch();
    let arrival = [arrived(MOUSE, RawDeviceKind::Mouse)];
    let (first, _) = input.fold_and_snapshot(RawInputBatch {
        events: &arrival,
        cursor: Some(RawCursor {
            x: 1000,
            y: 500,
            norm_x: 0.25,
            norm_y: 0.5,
        }),
        at_ms: 1,
        epoch,
    });
    let (second, _) = input.fold_and_snapshot(RawInputBatch {
        events: &[],
        cursor: Some(RawCursor {
            x: 1002,
            y: 500,
            // Same normalized bucket, two pixels apart.
            norm_x: 0.25,
            norm_y: 0.5,
        }),
        at_ms: 2,
        epoch,
    });

    assert_ne!(first.generation, second.generation);
    assert_eq!(second.mouse.x, 1002);
}

// ── Epoch ──────────────────────────────────────────────────────────────────

#[test]
fn a_batch_from_a_retired_session_is_inert() {
    // The hazard: a wedged pump is detached by a bounded join, wakes later,
    // and folds into a session that has since restarted. Without the epoch it
    // would resurrect held state that was deliberately cleared.
    let mut input = WindowsHostInput::new(true, true);
    let live = input.epoch();

    let events = [key(KBD_A, 0x1E, true)];
    let (data, drained) = input.fold_and_snapshot(RawInputBatch {
        events: &events,
        cursor: None,
        at_ms: 1,
        epoch: live.wrapping_add(9),
    });

    assert!(
        data.keyboard.pressed_keys.is_empty(),
        "a stale batch must not touch held state"
    );
    assert!(drained.is_empty(), "and must not reach the event bus");
}

#[test]
fn a_batch_at_the_live_epoch_is_applied() {
    let mut input = WindowsHostInput::new(true, true);
    let (data, _) = fold(&mut input, &[key(KBD_A, 0x1E, true)]);
    assert_eq!(data.keyboard.pressed_keys, vec!["a".to_owned()]);
}

// ── Overflow ───────────────────────────────────────────────────────────────

#[test]
fn the_event_queue_drops_oldest_and_counts_what_it_dropped() {
    let mut input = WindowsHostInput::new(true, true);
    let mut events = Vec::new();
    for _ in 0..300 {
        events.push(key(KBD_A, 0x1E, true));
        events.push(key(KBD_A, 0x1E, false));
    }
    let (data, drained) = fold(&mut input, &events);

    assert_eq!(drained.len(), 256, "bounded at the event limit");
    assert!(
        data.batch.dropped_events > 0,
        "and the loss is reported rather than hidden"
    );
}

#[test]
fn a_coordinate_space_change_resets_the_absolute_baseline() {
    // MOUSE_VIRTUAL_DESKTOP can flip mid-session. The two normalizations are
    // measured against different rects, so differencing across the boundary
    // would invent a jump the pointer never made — the same reason an arrival
    // resets the baseline.
    let mut input = WindowsHostInput::new(true, true);
    fold(
        &mut input,
        &[RawInputEvent::MotionAbsolute {
            device: device(MOUSE, RawDeviceKind::Mouse),
            norm_x: 0.1,
            norm_y: 0.1,
            virtual_desktop: false,
        }],
    );
    let (data, _) = fold(
        &mut input,
        &[RawInputEvent::MotionAbsolute {
            device: device(MOUSE, RawDeviceKind::Mouse),
            norm_x: 0.9,
            norm_y: 0.9,
            virtual_desktop: true,
        }],
    );
    assert!(
        data.batch.motion.distance < 1e-6,
        "a space change emits no delta, got {}",
        data.batch.motion.distance
    );
}

#[test]
fn motion_still_accumulates_while_the_space_is_stable() {
    let mut input = WindowsHostInput::new(true, true);
    fold(&mut input, &[absolute(MOUSE, 0.25, 0.25)]);
    let (data, _) = fold(&mut input, &[absolute(MOUSE, 0.75, 0.25)]);
    assert!((data.batch.motion.dx - 0.5).abs() < 1e-6);
}
