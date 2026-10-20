use std::sync::Arc;

use hypercolor_hal::transport::input::TransportInputSink;
use hypercolor_types::device::DeviceId;
use hypercolor_types::device_input::{DeviceInputEdge, TouchPosition};
use hypercolor_types::event::{InputEvent, TimedInputEvent, TouchPhase};

use super::UsbDeviceInput;
use crate::input::{DeviceInputChildSlot, DeviceInputHandle};

fn pad_press(contact: u32) -> DeviceInputEdge {
    DeviceInputEdge::TouchBegan {
        contact,
        position: TouchPosition {
            x: 0.5,
            y: 0.5,
            pressure: 1.0,
        },
        segment: Some(Arc::from("Pads")),
    }
}

fn demanded_host() -> DeviceInputHandle {
    let host = DeviceInputHandle::new();
    host.set_demanded(true);
    host
}

fn bridge(host: &DeviceInputHandle, device: DeviceId) -> UsbDeviceInput {
    UsbDeviceInput::new(Arc::new(host.clone()), device, "Ableton Push 2")
}

fn child(host: &DeviceInputHandle, device: DeviceId) -> Option<DeviceInputChildSlot> {
    host.registry().snapshot().child(device).cloned()
}

fn touch_phases(slot: &DeviceInputChildSlot) -> Vec<(u32, TouchPhase)> {
    let mut events: Vec<TimedInputEvent> = Vec::new();
    slot.read_events_since(0, &mut events);
    events
        .iter()
        .filter_map(|timed| match &timed.event {
            InputEvent::Touch { contact, phase, .. } => Some((*contact, *phase)),
            _ => None,
        })
        .collect()
}

#[test]
fn input_before_connect_finishes_is_dropped_and_attaches_nothing() {
    let host = demanded_host();
    let device = DeviceId::new();
    let input = bridge(&host, device);

    input.publish(&[pad_press(0)]);
    input.interrupted();

    assert!(
        child(&host, device).is_none(),
        "a device still connecting is not an input source"
    );
}

#[test]
fn activation_attaches_under_the_device_id_and_publishes() {
    let host = demanded_host();
    let device = DeviceId::new();
    let input = bridge(&host, device);

    input.activate();
    let slot = child(&host, device).expect("a connected device is an input source");
    assert_eq!(slot.label(), "Ableton Push 2");
    input.publish(&[pad_press(3)]);

    assert_eq!(touch_phases(&slot), vec![(3, TouchPhase::Began)]);
    // Activating again keeps the same lease.
    input.activate();
    assert_eq!(
        child(&host, device).map(|slot| slot.publication_id()),
        Some(slot.publication_id())
    );
}

#[test]
fn an_interruption_cancels_held_touches_and_keeps_input_flowing() {
    let host = demanded_host();
    let device = DeviceId::new();
    let input = bridge(&host, device);
    input.activate();
    let first = child(&host, device).expect("attached");
    input.publish(&[pad_press(3)]);

    input.interrupted();

    // A retired child leaves the registry, which is what makes the router
    // cancel every hold a consumer observed on it.
    assert!(!first.is_active(), "the superseded lease retires");
    assert_eq!(touch_phases(&first), vec![(3, TouchPhase::Began)]);
    let second = child(&host, device).expect("a fresh lease replaces it");
    assert_ne!(second.publication_id(), first.publication_id());
    input.publish(&[pad_press(5)]);
    assert_eq!(touch_phases(&second), vec![(5, TouchPhase::Began)]);
}

#[test]
fn closing_retires_the_lease_for_good() {
    let host = demanded_host();
    let device = DeviceId::new();
    let input = bridge(&host, device);
    input.activate();
    let slot = child(&host, device).expect("attached");

    input.closed();

    assert!(!slot.is_active());
    assert!(child(&host, device).is_none());
    // Nothing after the close brings the device back.
    input.publish(&[pad_press(0)]);
    input.interrupted();
    input.activate();
    input.close();
    assert!(child(&host, device).is_none());
}

#[test]
fn input_that_ends_before_connect_finishes_never_attaches() {
    let host = demanded_host();
    let device = DeviceId::new();
    let input = bridge(&host, device);

    input.closed();
    input.activate();

    assert!(child(&host, device).is_none());
}

#[test]
fn a_reconnect_supersedes_a_bridge_that_was_never_closed() {
    let host = demanded_host();
    let device = DeviceId::new();
    let stale = bridge(&host, device);
    stale.activate();
    let stale_slot = child(&host, device).expect("attached");

    let fresh = bridge(&host, device);
    fresh.activate();
    let fresh_slot = child(&host, device).expect("attached");
    assert!(!stale_slot.is_active());

    // The stale bridge can neither publish into, take over, nor retire its
    // successor.
    stale.publish(&[pad_press(1)]);
    stale.interrupted();
    stale.close();
    assert!(fresh_slot.is_active());
    assert_eq!(
        child(&host, device).map(|slot| slot.publication_id()),
        Some(fresh_slot.publication_id())
    );
    assert!(touch_phases(&fresh_slot).is_empty());
}

#[test]
fn a_superseded_bridge_stops_before_an_interruption_can_take_the_device_back() {
    let host = demanded_host();
    let device = DeviceId::new();
    let stale = bridge(&host, device);
    stale.activate();
    let fresh = bridge(&host, device);
    fresh.activate();
    let fresh_slot = child(&host, device).expect("attached");

    // Only an interruption, with no publish first to notice the takeover.
    stale.interrupted();
    stale.activate();

    assert!(fresh_slot.is_active());
    assert_eq!(
        child(&host, device).map(|slot| slot.publication_id()),
        Some(fresh_slot.publication_id())
    );
    fresh.publish(&[pad_press(2)]);
    assert_eq!(touch_phases(&fresh_slot), vec![(2, TouchPhase::Began)]);
}
