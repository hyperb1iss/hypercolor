#![cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]

use std::sync::Arc;

use hypercolor_core::input::{InputData, InputSource, InteractionSource, WindowsHostInput};
use hypercolor_types::host_input::{
    HostInputBatch, HostInputCapabilities, HostInputDevice, HostInputEvent, HostKeyIdentity,
    HostKeySignal, HostRepeatEvidence,
};

#[test]
fn deterministic_fixture_publishes_neutral_batches_through_host_lifecycle() {
    let device = Arc::new(HostInputDevice {
        source_id: Arc::from("windows:fixture:kbd"),
        label: Arc::from("fixture keyboard"),
        capabilities: HostInputCapabilities {
            keyboard: true,
            pointer: false,
        },
        session_generation: 1,
        device_generation: 1,
    });
    let events = [
        HostInputEvent::DeviceArrived {
            device: Arc::clone(&device),
        },
        HostInputEvent::Key {
            device: Some(device),
            identity: HostKeyIdentity {
                key: Arc::from("a"),
                physical_code: Arc::from("windows:set1:none:1e"),
            },
            signal: HostKeySignal::Edge {
                pressed: true,
                repeat: HostRepeatEvidence::Unknown,
            },
        },
    ];
    let (mut source, fixture) = WindowsHostInput::new_deterministic_fixture(true, false);
    source.start().expect("fixture source starts");
    source
        .set_interaction_capture_active(true)
        .expect("fixture source activates");
    assert!(
        fixture
            .publish(HostInputBatch {
                events: &events,
                pointer: None,
                at_ms: 10,
                device_catalog_generation: 1,
            })
            .expect("active fixture accepts batch")
    );
    let (sample, _) = source.sample_and_drain_with_delta_secs(1.0 / 60.0);
    let InputData::Interaction(interaction) = sample.expect("sample succeeds") else {
        panic!("expected interaction data");
    };
    assert_eq!(interaction.keyboard.pressed_keys, ["a"]);
}
