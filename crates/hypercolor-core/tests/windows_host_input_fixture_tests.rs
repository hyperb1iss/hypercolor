//! Windows Raw Input adapter-boundary fixture contracts.

#![cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]

use std::sync::Arc;

use hypercolor_core::input::{InputData, InputSource, SourceState, WindowsHostInput};
use hypercolor_types::event::{InputButtonState, InputEvent};
use hypercolor_windows_input::{
    RawButton, RawDeviceDescriptor, RawDeviceKind, RawInputBatch, RawInputEvent, RawKeyPrefix,
};

fn device(source_id: &'static str, kind: RawDeviceKind) -> Arc<RawDeviceDescriptor> {
    Arc::new(RawDeviceDescriptor {
        source_id: Arc::from(source_id),
        interface_path: Some(Arc::from(format!("fixture:{source_id}"))),
        label: Arc::from(format!("fixture {source_id}")),
        kind,
        session_generation: 1,
        device_generation: 1,
    })
}

#[test]
fn deterministic_fixture_uses_host_lifecycle_sampling_and_device_health() {
    let keyboard = device("keyboard-1", RawDeviceKind::Keyboard);
    let mouse = device("mouse-1", RawDeviceKind::Mouse);
    let events = vec![
        RawInputEvent::DeviceArrived {
            device: Arc::clone(&keyboard),
        },
        RawInputEvent::Key {
            device: keyboard,
            make_code: 0x1e,
            prefix: RawKeyPrefix::None,
            vkey: 0,
            pressed: true,
        },
        RawInputEvent::DeviceArrived {
            device: Arc::clone(&mouse),
        },
        RawInputEvent::Button {
            device: mouse,
            button: RawButton::Left,
            pressed: true,
        },
    ];
    let (mut source, fixture) = WindowsHostInput::new_deterministic_fixture(true, true);
    let status = source
        .source_status_handle()
        .expect("Windows host source exposes status");

    source.set_source_graph_generation(1);
    source.start().expect("deterministic source starts idle");
    assert!(!fixture.is_active());
    assert!(fixture.publish(&events, None, 100).is_err());
    source
        .set_interaction_capture_active(true)
        .expect("deterministic Raw Input activates without hardware");
    assert!(fixture.is_active());
    assert_eq!(status.snapshot().resource_count, 0);
    let first_epoch = source.epoch();
    assert_ne!(first_epoch, 0);
    assert!(
        fixture
            .publish(&events, None, 100)
            .expect("post-adapter batch is accepted")
    );

    let (sample, drained) = source.sample_and_drain_with_delta_secs(1.0 / 60.0);
    let InputData::Interaction(interaction) = sample.expect("fixture sample succeeds") else {
        panic!("expected interaction data");
    };
    assert_eq!(interaction.keyboard.pressed_keys, ["a"]);
    assert_eq!(interaction.mouse.buttons, ["left"]);
    assert_eq!(drained.len(), 2);
    assert!(matches!(
        drained[0].event,
        InputEvent::Key {
            state: InputButtonState::Pressed,
            ..
        }
    ));
    assert_eq!(status.snapshot().resource_count, 2);

    source
        .set_interaction_capture_active(false)
        .expect("deterministic Raw Input deactivates");
    assert!(!fixture.is_active());
    assert!(fixture.publish(&events, None, 200).is_err());
    source.set_source_graph_generation(2);
    source
        .set_interaction_capture_active(true)
        .expect("deterministic Raw Input reactivates");
    assert_ne!(source.epoch(), first_epoch);
    let (_, stale_events) = source.fold_and_snapshot(RawInputBatch {
        events: &events,
        cursor: None,
        at_ms: 300,
        epoch: first_epoch,
    });
    assert!(stale_events.is_empty());

    source.stop();
    assert!(!fixture.is_active());
    assert_eq!(status.snapshot().state, SourceState::Stopped);
}
