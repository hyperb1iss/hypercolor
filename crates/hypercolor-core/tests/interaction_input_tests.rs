#![cfg(target_os = "macos")]

use device_query::Keycode;
use hypercolor_core::input::{InteractionBatch, InteractionInput};
use hypercolor_core::types::event::{InputButtonState, InputEvent};

#[test]
fn publishes_canonical_key_edges_with_sampled_state() {
    let mut source = InteractionInput::new();
    let polls = [
        (vec![Keycode::A], 10),
        (vec![Keycode::A], 15),
        (vec![Keycode::B], 20),
    ];

    let (snapshot, events) =
        source.fold_polled_key_sequence_for_testing(&polls, InteractionBatch::MAX_EVENTS);

    assert_eq!(snapshot.keyboard.pressed_keys, ["b"]);
    assert_eq!(snapshot.keyboard.recent_keys, ["a", "b"]);
    assert_eq!(snapshot.batch.dropped_events, 0);
    assert_eq!(events.len(), 3);
    assert!(matches!(
        &events[0].event,
        InputEvent::Key {
            source_id,
            key,
            state: InputButtonState::Pressed,
        } if source_id == "host:device_query" && key == "a"
    ));
    assert!(matches!(
        &events[1].event,
        InputEvent::Key {
            source_id,
            key,
            state: InputButtonState::Released,
        } if source_id == "host:device_query" && key == "a"
    ));
    assert!(matches!(
        &events[2].event,
        InputEvent::Key {
            source_id,
            key,
            state: InputButtonState::Pressed,
        } if source_id == "host:device_query" && key == "b"
    ));
    assert_eq!(
        events.iter().map(|event| event.at_ms).collect::<Vec<_>>(),
        [10, 20, 20]
    );
    assert!(events.iter().all(|event| {
        event.seq == 0 && event.physical_code.is_none() && event.repeat_count == 1
    }));
}

#[test]
fn canonical_events_exceed_the_legacy_recent_limit() {
    let mut source = InteractionInput::new();
    let keys = vec![
        Keycode::A,
        Keycode::B,
        Keycode::C,
        Keycode::D,
        Keycode::E,
        Keycode::F,
        Keycode::G,
        Keycode::H,
        Keycode::I,
        Keycode::J,
        Keycode::K,
        Keycode::L,
        Keycode::M,
        Keycode::N,
        Keycode::O,
        Keycode::P,
        Keycode::Q,
        Keycode::R,
        Keycode::S,
        Keycode::T,
        Keycode::U,
        Keycode::V,
        Keycode::W,
        Keycode::X,
        Keycode::Y,
        Keycode::Z,
        Keycode::Key0,
        Keycode::Key1,
        Keycode::Key2,
        Keycode::Key3,
        Keycode::Key4,
        Keycode::Key5,
        Keycode::Key6,
        Keycode::Key7,
        Keycode::Key8,
        Keycode::Key9,
        Keycode::Up,
        Keycode::Down,
        Keycode::Left,
        Keycode::Right,
    ];

    let (snapshot, events) =
        source.fold_polled_key_sequence_for_testing(&[(keys, 10)], InteractionBatch::MAX_EVENTS);
    let projected = projected_recent_keys(&events);

    assert_eq!(events.len(), 40);
    assert_eq!(snapshot.keyboard.pressed_keys.len(), 40);
    assert_eq!(snapshot.keyboard.recent_keys, projected);
    assert_eq!(snapshot.keyboard.recent_keys.len(), 40);
    assert_eq!(snapshot.batch.dropped_events, 0);
}

#[test]
fn overflow_projects_recents_from_256_retained_events() {
    let mut source = InteractionInput::new();
    let mut polls = Vec::with_capacity(261);
    for index in 0_u64..130 {
        polls.push((vec![Keycode::A], index * 2 + 1));
        polls.push((Vec::new(), index * 2 + 2));
    }
    polls.push((vec![Keycode::B], 261));

    let (snapshot, events) =
        source.fold_polled_key_sequence_for_testing(&polls, InteractionBatch::MAX_EVENTS);
    let projected = projected_recent_keys(&events);

    assert_eq!(snapshot.keyboard.pressed_keys, ["b"]);
    assert_eq!(events.len(), InteractionBatch::MAX_EVENTS);
    assert_eq!(events.first().map(|event| event.at_ms), Some(6));
    assert_eq!(events.last().map(|event| event.at_ms), Some(261));
    assert_eq!(snapshot.keyboard.recent_keys, projected);
    assert_eq!(snapshot.keyboard.recent_keys.len(), 128);
    assert_eq!(
        snapshot.keyboard.recent_keys.last().map(String::as_str),
        Some("b")
    );
    assert_eq!(snapshot.batch.dropped_events, 5);
}

fn projected_recent_keys(events: &[hypercolor_core::types::event::TimedInputEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.event {
            InputEvent::Key {
                key,
                state: InputButtonState::Pressed,
                ..
            } => Some(key.clone()),
            _ => None,
        })
        .collect()
}
