use std::sync::Arc;

use hypercolor_core::input::{
    DeviceButtonHold, DeviceInputChildSlot, DeviceInputHandle, DeviceInputRegistryError, InputData,
    InteractionData, SourceState, TouchContact,
};
use hypercolor_driver_api::DeviceInputSink;
use hypercolor_types::device::DeviceId;
use hypercolor_types::device_input::{DeviceInputEdge, TouchPosition};
use hypercolor_types::event::{InputButtonState, InputEvent, TimedInputEvent, TouchPhase};

const ONE: i64 = 1 << 16;

fn at(x: f32, y: f32, pressure: f32) -> TouchPosition {
    TouchPosition { x, y, pressure }
}

fn began(contact: u32, position: TouchPosition) -> DeviceInputEdge {
    DeviceInputEdge::TouchBegan { contact, position }
}

fn moved(contact: u32, position: TouchPosition) -> DeviceInputEdge {
    DeviceInputEdge::TouchMoved { contact, position }
}

fn ended(contact: u32, position: TouchPosition) -> DeviceInputEdge {
    DeviceInputEdge::TouchEnded { contact, position }
}

fn button(name: &str, state: InputButtonState) -> DeviceInputEdge {
    DeviceInputEdge::Button {
        button: Arc::from(name),
        state,
    }
}

fn held(slot: &DeviceInputChildSlot) -> InteractionData {
    let sample = slot.latest().expect("child should publish held state");
    let InputData::Interaction(interaction) = sample.as_ref() else {
        panic!("device child must publish interaction data");
    };
    interaction.clone()
}

/// Read every retained event after `cursor`, returning the next cursor.
fn events_since(slot: &DeviceInputChildSlot, cursor: &mut u64) -> Vec<TimedInputEvent> {
    let mut events = Vec::new();
    *cursor = slot.read_events_since(*cursor, &mut events).next_cursor;
    events
}

/// Touch phases and contacts, in order.
fn touch_edges(events: &[TimedInputEvent]) -> Vec<(u32, TouchPhase)> {
    events
        .iter()
        .filter_map(|timed| match &timed.event {
            InputEvent::Touch { contact, phase, .. } => Some((*contact, *phase)),
            _ => None,
        })
        .collect()
}

fn button_edges(events: &[TimedInputEvent]) -> Vec<(String, InputButtonState)> {
    events
        .iter()
        .filter_map(|timed| match &timed.event {
            InputEvent::DeviceButton { button, state, .. } => Some((button.clone(), *state)),
            _ => None,
        })
        .collect()
}

fn demanded_handle() -> DeviceInputHandle {
    let handle = DeviceInputHandle::new();
    handle.set_demanded(true);
    handle
}

#[test]
fn attach_and_drop_follow_the_registry() {
    let handle = DeviceInputHandle::new();
    let registry = handle.registry();
    let device = DeviceId::new();
    assert!(registry.snapshot().children().is_empty());

    let attachment = handle.attach(device, "Lightpad Block M");
    let snapshot = registry.snapshot();
    let child = snapshot.child(device).expect("attached device is routable");
    assert_eq!(child.label(), "Lightpad Block M");
    assert_eq!(child.source_id(), format!("device:{device}"));
    assert_eq!(child.publication_id(), attachment.publication_id());
    let attached_generation = snapshot.generation();

    drop(attachment);
    let snapshot = registry.snapshot();
    assert!(snapshot.child(device).is_none());
    assert!(snapshot.generation() > attached_generation);
}

#[test]
fn touches_announce_begin_and_end_while_moves_update_holds() {
    let handle = demanded_handle();
    let device = DeviceId::new();
    let attachment = handle.attach(device, "pad");
    let slot = attachment.slot();
    let mut cursor = 0;

    attachment
        .inject(&[began(2, at(0.5, 0.25, 0.75))])
        .expect("inject");
    let events = events_since(&slot, &mut cursor);
    assert_eq!(touch_edges(&events), vec![(2, TouchPhase::Began)]);
    let InputEvent::Touch {
        device_id,
        x_q16_16,
        y_q16_16,
        pressure_q16_16,
        ..
    } = &events[0].event
    else {
        panic!("expected a touch event");
    };
    assert_eq!(*device_id, device);
    assert_eq!(
        (*x_q16_16, *y_q16_16, *pressure_q16_16),
        (ONE / 2, ONE / 4, ONE * 3 / 4)
    );
    let generation = held(&slot).generation;

    attachment
        .inject(&[moved(2, at(0.75, 0.25, 0.5))])
        .expect("inject");
    assert!(events_since(&slot, &mut cursor).is_empty());
    let snapshot = held(&slot);
    assert!(snapshot.generation > generation);
    assert_eq!(snapshot.device.touches.len(), 1);
    let touch = snapshot.device.touches[0];
    assert_eq!((touch.device_id, touch.contact), (device, 2));
    assert_eq!((touch.x, touch.y, touch.pressure), (0.75, 0.25, 0.5));

    attachment
        .inject(&[ended(2, at(0.8, 0.3, 0.0))])
        .expect("inject");
    assert_eq!(
        touch_edges(&events_since(&slot, &mut cursor)),
        vec![(2, TouchPhase::Ended)]
    );
    assert!(held(&slot).device.touches.is_empty());
}

#[test]
fn repeated_begin_ends_the_previous_contact_first() {
    let handle = demanded_handle();
    let attachment = handle.attach(DeviceId::new(), "pad");
    let slot = attachment.slot();
    let mut cursor = 0;

    attachment
        .inject(&[began(0, at(0.1, 0.1, 0.5)), began(0, at(0.9, 0.9, 0.5))])
        .expect("inject");

    assert_eq!(
        touch_edges(&events_since(&slot, &mut cursor)),
        vec![
            (0, TouchPhase::Began),
            (0, TouchPhase::Ended),
            (0, TouchPhase::Began)
        ]
    );
    assert_eq!(held(&slot).device.touches[0].x, 0.9);
}

#[test]
fn moves_and_ends_without_a_begin_are_ignored() {
    let handle = demanded_handle();
    let attachment = handle.attach(DeviceId::new(), "pad");
    let slot = attachment.slot();
    let mut cursor = 0;
    let generation = held(&slot).generation;

    attachment
        .inject(&[moved(4, at(0.5, 0.5, 0.5)), ended(4, at(0.5, 0.5, 0.0))])
        .expect("inject");

    assert!(events_since(&slot, &mut cursor).is_empty());
    assert_eq!(held(&slot).generation, generation);
}

#[test]
fn positions_clamp_and_non_finite_input_never_strands_a_contact() {
    let handle = demanded_handle();
    let attachment = handle.attach(DeviceId::new(), "pad");
    let slot = attachment.slot();
    let mut cursor = 0;

    attachment
        .inject(&[began(0, at(f32::NAN, 0.5, 0.5))])
        .expect("inject");
    assert!(events_since(&slot, &mut cursor).is_empty());
    assert_eq!(held(&slot).batch.dropped_events, 1);

    attachment
        .inject(&[
            began(1, at(1.5, -0.5, 2.0)),
            moved(1, at(f32::INFINITY, 0.5, 0.5)),
        ])
        .expect("inject");
    let touch = held(&slot).device.touches[0];
    assert_eq!((touch.x, touch.y, touch.pressure), (1.0, 0.0, 1.0));

    attachment
        .inject(&[ended(1, at(f32::NAN, f32::NAN, f32::NAN))])
        .expect("inject");
    let events = events_since(&slot, &mut cursor);
    assert_eq!(
        touch_edges(&events),
        vec![(1, TouchPhase::Began), (1, TouchPhase::Ended)]
    );
    let InputEvent::Touch { x_q16_16, .. } = &events[1].event else {
        panic!("expected a touch end");
    };
    assert_eq!(*x_q16_16, ONE, "end falls back to the last good position");
    assert!(held(&slot).device.touches.is_empty());
}

#[test]
fn contacts_beyond_the_per_device_cap_are_dropped() {
    let handle = demanded_handle();
    let attachment = handle.attach(DeviceId::new(), "pad");
    let slot = attachment.slot();

    let edges: Vec<_> = (0..33)
        .map(|contact| began(contact, at(0.5, 0.5, 0.5)))
        .collect();
    attachment.inject(&edges).expect("inject");

    let snapshot = held(&slot);
    assert_eq!(snapshot.device.touches.len(), 32);
    assert_eq!(snapshot.batch.dropped_events, 1);
}

#[test]
fn buttons_press_once_and_release_only_when_held() {
    let handle = demanded_handle();
    let device = DeviceId::new();
    let attachment = handle.attach(device, "keys");
    let slot = attachment.slot();
    let mut cursor = 0;

    attachment
        .inject(&[
            button("down", InputButtonState::Released),
            button("mode", InputButtonState::Pressed),
            button("mode", InputButtonState::Pressed),
            button("mode", InputButtonState::Repeated),
        ])
        .expect("inject");
    assert_eq!(
        button_edges(&events_since(&slot, &mut cursor)),
        vec![("mode".to_owned(), InputButtonState::Pressed)]
    );
    let holds = held(&slot).device.buttons;
    assert_eq!(holds.len(), 1);
    assert_eq!((holds[0].device_id, &*holds[0].button), (device, "mode"));

    attachment
        .inject(&[button("mode", InputButtonState::Released)])
        .expect("inject");
    assert_eq!(
        button_edges(&events_since(&slot, &mut cursor)),
        vec![("mode".to_owned(), InputButtonState::Released)]
    );
    assert!(held(&slot).device.buttons.is_empty());
}

#[test]
fn undemanded_input_publishes_nothing() {
    let handle = DeviceInputHandle::new();
    let attachment = handle.attach(DeviceId::new(), "pad");
    let slot = attachment.slot();
    let mut cursor = 0;
    let generation = held(&slot).generation;

    attachment
        .inject(&[
            began(0, at(0.5, 0.5, 0.5)),
            button("mode", InputButtonState::Pressed),
        ])
        .expect("inject");

    assert!(events_since(&slot, &mut cursor).is_empty());
    let snapshot = held(&slot);
    assert!(snapshot.device.is_empty());
    assert_eq!(snapshot.generation, generation);
}

#[test]
fn losing_demand_cancels_announced_holds_with_explicit_edges() {
    let handle = demanded_handle();
    let attachment = handle.attach(DeviceId::new(), "pad");
    let slot = attachment.slot();
    let mut cursor = 0;
    attachment
        .inject(&[
            began(0, at(0.5, 0.5, 0.5)),
            button("mode", InputButtonState::Pressed),
        ])
        .expect("inject");
    events_since(&slot, &mut cursor);

    handle.set_demanded(false);

    let events = events_since(&slot, &mut cursor);
    assert_eq!(touch_edges(&events), vec![(0, TouchPhase::Cancelled)]);
    assert_eq!(
        button_edges(&events),
        vec![("mode".to_owned(), InputButtonState::Released)]
    );
    assert!(held(&slot).device.is_empty());
}

#[test]
fn presses_that_span_a_demand_change_stay_invisible_until_they_lift() {
    let handle = demanded_handle();
    let attachment = handle.attach(DeviceId::new(), "pad");
    let slot = attachment.slot();
    let mut cursor = 0;

    // Announced before demand drops, still physically down after it returns.
    attachment
        .inject(&[began(0, at(0.2, 0.2, 0.5))])
        .expect("inject");
    handle.set_demanded(false);
    // Lands while undemanded.
    attachment
        .inject(&[
            began(1, at(0.8, 0.8, 0.5)),
            button("up", InputButtonState::Pressed),
        ])
        .expect("inject");
    handle.set_demanded(true);
    events_since(&slot, &mut cursor);

    attachment
        .inject(&[
            moved(0, at(0.3, 0.3, 0.5)),
            moved(1, at(0.7, 0.7, 0.5)),
            ended(0, at(0.3, 0.3, 0.0)),
            ended(1, at(0.7, 0.7, 0.0)),
            button("up", InputButtonState::Released),
        ])
        .expect("inject");
    assert!(events_since(&slot, &mut cursor).is_empty());
    assert!(held(&slot).device.is_empty());

    // A fresh press on the same contact routes normally.
    attachment
        .inject(&[began(0, at(0.5, 0.5, 0.5))])
        .expect("inject");
    assert_eq!(
        touch_edges(&events_since(&slot, &mut cursor)),
        vec![(0, TouchPhase::Began)]
    );
}

#[test]
fn a_new_attachment_supersedes_the_previous_lease() {
    let handle = demanded_handle();
    let registry = handle.registry();
    let device = DeviceId::new();
    let first = handle.attach(device, "pad");
    let second = handle.attach(device, "pad");
    assert_ne!(first.publication_id(), second.publication_id());

    assert_eq!(
        first.inject(&[began(0, at(0.5, 0.5, 0.5))]),
        Err(DeviceInputRegistryError::ChildClosed)
    );
    drop(first);

    let snapshot = registry.snapshot();
    assert_eq!(snapshot.children().len(), 1);
    assert_eq!(
        snapshot
            .child(device)
            .expect("successor stays attached")
            .publication_id(),
        second.publication_id()
    );
    second
        .inject(&[began(0, at(0.5, 0.5, 0.5))])
        .expect("successor accepts input");
}

#[test]
fn driver_sink_leases_report_supersession() {
    let handle = demanded_handle();
    let sink: Arc<dyn DeviceInputSink> = Arc::new(handle.clone());
    let device = DeviceId::new();

    let first = sink.attach(device, "pad");
    assert!(first.publish(&[began(0, at(0.5, 0.5, 0.5))]));
    let second = sink.attach(device, "pad");

    assert!(!first.publish(&[ended(0, at(0.5, 0.5, 0.0))]));
    assert!(second.publish(&[began(0, at(0.5, 0.5, 0.5))]));
    drop(first);
    assert!(handle.registry().snapshot().child(device).is_some());
}

#[test]
fn source_status_follows_demand() {
    let handle = DeviceInputHandle::new();
    let _attachment = handle.attach(DeviceId::new(), "pad");
    let status = handle.registry().snapshot().children()[0].status().clone();
    assert!(!status.snapshot().demanded);

    handle.set_demanded(true);
    let live = status.snapshot();
    assert!(live.demanded);
    assert_eq!(live.state, SourceState::Live);
    assert_eq!(live.resource_count, 1);

    let second = handle.attach(DeviceId::new(), "keys");
    assert_eq!(status.snapshot().resource_count, 2);
    drop(second);
    assert_eq!(
        status.snapshot().resource_count,
        1,
        "detaching refreshes the live resource count"
    );

    handle.set_demanded(false);
    assert!(!status.snapshot().demanded);

    handle.set_demanded(true);
    assert_eq!(
        status.snapshot().state,
        SourceState::Live,
        "every re-demand starts a fresh session"
    );
}

#[test]
fn merged_snapshots_union_device_holds_per_device() {
    let lightpad = DeviceId::new();
    let lumi = DeviceId::new();
    let contact = |device_id, x| TouchContact {
        device_id,
        contact: 0,
        x,
        y: 0.5,
        pressure: 0.5,
    };
    let hold = |device_id| DeviceButtonHold {
        device_id,
        button: Arc::from("mode"),
    };

    let mut first = InteractionData::default();
    first.device.touches.push(contact(lightpad, 0.1));
    first.device.buttons.push(hold(lightpad));
    let mut second = InteractionData::default();
    second.device.touches.push(contact(lightpad, 0.9));
    second.device.touches.push(contact(lumi, 0.3));
    second.device.buttons.push(hold(lumi));

    let mut by_ref = first.clone();
    by_ref.merge_from_ref(&second);
    first.merge_from(second);

    assert_eq!(first.device, by_ref.device);
    assert_eq!(
        first.device.touches,
        vec![contact(lightpad, 0.1), contact(lumi, 0.3)],
        "the same contact on the same device is held once"
    );
    assert_eq!(first.device.buttons, vec![hold(lightpad), hold(lumi)]);
}
