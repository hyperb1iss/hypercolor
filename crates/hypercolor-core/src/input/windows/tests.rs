use std::sync::Arc;

use hypercolor_windows_input::{
    RawDeviceDescriptor, RawDeviceKind, RawInputBatch, RawInputEvent, RawKeyPrefix,
};

use super::{WindowsHostInput, fold_batch};
use crate::input::InputSource;

#[test]
fn failed_start_fences_every_late_initial_batch_from_the_event_drain() {
    let mut input = WindowsHostInput::new(true, true);
    let failed_epoch = input.rotate_epoch_and_clear();
    input.rotate_epoch_and_clear();
    assert_ne!(input.epoch(), failed_epoch);

    let events = [RawInputEvent::Key {
        device: Arc::new(RawDeviceDescriptor {
            source_id: Arc::from("test-keyboard"),
            interface_path: Some(Arc::from("test-interface")),
            label: Arc::from("test keyboard"),
            kind: RawDeviceKind::Keyboard,
            session_generation: 1,
            device_generation: 1,
        }),
        make_code: 0x1e,
        prefix: RawKeyPrefix::None,
        vkey: 0,
        pressed: true,
    }];
    fold_batch(
        &input.shared,
        RawInputBatch {
            events: &events,
            cursor: None,
            at_ms: 1,
            epoch: failed_epoch,
        },
        input.event_limit,
    );

    assert!(
        input.drain_events().is_empty(),
        "a late initial batch from a failed session must never reach consumers"
    );
}

#[test]
fn zero_event_limit_drops_every_edge_without_growing_the_queues() {
    let mut input = WindowsHostInput::new(true, true);
    input.event_limit = 0;
    let epoch = input.epoch();
    let device = Arc::new(RawDeviceDescriptor {
        source_id: Arc::from("test-keyboard"),
        interface_path: Some(Arc::from("test-interface")),
        label: Arc::from("test keyboard"),
        kind: RawDeviceKind::Keyboard,
        session_generation: 1,
        device_generation: 1,
    });
    let events = [
        RawInputEvent::Key {
            device: Arc::clone(&device),
            make_code: 0x1e,
            prefix: RawKeyPrefix::None,
            vkey: 0,
            pressed: true,
        },
        RawInputEvent::Key {
            device,
            make_code: 0x1e,
            prefix: RawKeyPrefix::None,
            vkey: 0,
            pressed: false,
        },
    ];

    let (snapshot, drained) = input.fold_and_snapshot(RawInputBatch {
        events: &events,
        cursor: None,
        at_ms: 1,
        epoch,
    });

    assert!(drained.is_empty());
    assert!(snapshot.keyboard.recent_keys.is_empty());
    assert_eq!(snapshot.batch.dropped_events, 2);
}
