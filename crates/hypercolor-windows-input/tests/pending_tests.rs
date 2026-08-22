use std::sync::Arc;

use hypercolor_types::host_input::{
    HostInputBatch, HostInputEvent, HostKeyIdentity, HostKeySignal, HostRepeatEvidence,
};
use hypercolor_windows_input::PendingEvents;

fn key(name: &str, pressed: bool) -> HostInputEvent {
    HostInputEvent::Key {
        device: None,
        identity: HostKeyIdentity {
            key: Arc::from(name),
            physical_code: Arc::from(format!("windows:fixture:{name}")),
        },
        signal: HostKeySignal::Edge {
            pressed,
            repeat: HostRepeatEvidence::Unknown,
        },
    }
}

fn collect(pending: &mut PendingEvents, at_ms: u64) -> Vec<Vec<String>> {
    let mut batches = Vec::new();
    pending.deliver(at_ms, 1, None, &mut |batch: HostInputBatch<'_>| {
        batches.push(
            batch
                .events
                .iter()
                .filter_map(|event| match event {
                    HostInputEvent::Key { identity, .. } => Some(identity.key.to_string()),
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
    pending.push(key("a", true));
    pending.push(key("a", false));
    assert_eq!(collect(&mut pending, 100), vec![vec!["a", "a"]]);
}

#[test]
fn delivering_twice_does_not_repeat_the_batch() {
    let mut pending = PendingEvents::new();
    pending.push(key("a", true));
    assert_eq!(collect(&mut pending, 100).len(), 1);
    assert!(collect(&mut pending, 101).is_empty());
}

#[test]
fn an_empty_buffer_reports_that_it_delivered_nothing() {
    let mut pending = PendingEvents::new();
    assert!(!pending.deliver(100, 1, None, &mut |_| {
        panic!("an empty buffer must not invoke the sink");
    }));
}

#[test]
fn events_after_delivery_form_their_own_ordered_batch() {
    let mut pending = PendingEvents::new();
    pending.extend([key("a", true), key("b", true)]);
    pending.push(key("c", true));
    assert_eq!(collect(&mut pending, 100), vec![vec!["a", "b", "c"]]);
    pending.push(key("d", true));
    assert_eq!(collect(&mut pending, 200), vec![vec!["d"]]);
}

#[test]
fn a_fresh_buffer_is_empty() {
    assert!(PendingEvents::new().is_empty());
}
