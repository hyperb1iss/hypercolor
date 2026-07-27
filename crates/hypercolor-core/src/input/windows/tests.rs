use std::sync::Arc;

use hypercolor_windows_input::{RawInputBatch, RawInputEvent, RawKeyPrefix};

use super::{WindowsHostInput, fold_batch};
use crate::input::InputSource;

#[test]
fn failed_start_fences_every_late_initial_batch_from_the_event_drain() {
    let mut input = WindowsHostInput::new(true, true);
    let failed_epoch = input.rotate_epoch_and_clear();
    input.rotate_epoch_and_clear();
    assert_ne!(input.epoch(), failed_epoch);

    let events = [RawInputEvent::Key {
        source_id: Arc::from("test-keyboard"),
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
