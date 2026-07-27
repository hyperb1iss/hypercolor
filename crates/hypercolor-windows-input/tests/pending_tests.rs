//! The pending-event buffer's delivery contract.
//!
//! These exist because of a bug real hardware caught and no test could: the
//! drain cleared its buffer at the *start* of a slice instead of after
//! delivering, so whatever the last slice produced was still sitting there
//! when the worker flushed, and every key edge, button edge and motion delta
//! reached core twice. Both call sites were individually correct and the
//! composition was not.
//!
//! They run on every platform, unlike the pump loop that hid the defect.

use std::sync::Arc;

use hypercolor_windows_input::{
    PendingEvents, RawDeviceDescriptor, RawDeviceKind, RawInputBatch, RawInputEvent, RawKeyPrefix,
};

fn device() -> Arc<RawDeviceDescriptor> {
    Arc::new(RawDeviceDescriptor {
        source_id: Arc::from("test-device"),
        interface_path: Some(Arc::from("test-interface")),
        label: Arc::from("test keyboard"),
        kind: RawDeviceKind::Keyboard,
        session_generation: 1,
        device_generation: 1,
    })
}

fn key(make_code: u16, pressed: bool) -> RawInputEvent {
    RawInputEvent::Key {
        device: device(),
        make_code,
        prefix: RawKeyPrefix::None,
        vkey: 0,
        pressed,
    }
}

/// Collect what each sink invocation saw, one entry per delivered batch.
fn collect(pending: &mut PendingEvents, at_ms: u64) -> Vec<Vec<u16>> {
    let mut batches = Vec::new();
    pending.deliver(at_ms, 1, None, &mut |batch: RawInputBatch<'_>| {
        batches.push(
            batch
                .events
                .iter()
                .filter_map(|event| match event {
                    RawInputEvent::Key { make_code, .. } => Some(*make_code),
                    _ => None,
                })
                .collect(),
        );
    });
    batches
}

#[test]
fn delivering_hands_over_every_pending_event_once() {
    let mut pending = PendingEvents::new();
    pending.push(key(0x001E, true));
    pending.push(key(0x001E, false));

    let batches = collect(&mut pending, 100);
    assert_eq!(batches, vec![vec![0x001E, 0x001E]]);
}

#[test]
fn delivering_twice_does_not_repeat_the_batch() {
    // The regression. A second delivery with no intervening push must find
    // nothing, which is what makes the drain-then-flush sequence safe.
    let mut pending = PendingEvents::new();
    pending.push(key(0x001E, true));

    assert_eq!(collect(&mut pending, 100).len(), 1, "first delivers");
    assert!(
        collect(&mut pending, 101).is_empty(),
        "the second delivery must find an empty buffer"
    );
}

#[test]
fn an_empty_buffer_reports_that_it_delivered_nothing() {
    let mut pending = PendingEvents::new();
    let delivered = pending.deliver(100, 1, None, &mut |_| {
        panic!("an empty buffer must not invoke the sink");
    });
    assert!(!delivered);
}

#[test]
fn events_pushed_after_a_delivery_form_their_own_batch() {
    // Ordering across the drain-flush boundary: what arrives after a delivery
    // must not be merged into the batch that already went out, and must not
    // be lost either.
    let mut pending = PendingEvents::new();
    pending.push(key(0x001E, true));
    assert_eq!(collect(&mut pending, 100), vec![vec![0x001E]]);

    pending.push(key(0x001F, true));
    assert_eq!(collect(&mut pending, 200), vec![vec![0x001F]]);
}

#[test]
fn extending_preserves_order_against_events_already_queued() {
    // Device arrivals are queued by the control pass and input is appended
    // after, so an arrival must stay ahead of the input it precedes.
    let mut pending = PendingEvents::new();
    pending.extend([key(0x0001, true), key(0x0002, true)]);
    pending.push(key(0x0003, true));

    assert_eq!(
        collect(&mut pending, 100),
        vec![vec![0x0001, 0x0002, 0x0003]]
    );
}

#[test]
fn a_fresh_buffer_is_empty() {
    assert!(PendingEvents::new().is_empty());
}
