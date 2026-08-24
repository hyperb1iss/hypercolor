use std::sync::Arc;

use hypercolor_core::input::WindowsHostInput;
use hypercolor_types::event::{InputButtonState, InputEvent};
use hypercolor_types::host_input::{
    HostInputBatch, HostInputCapabilities, HostInputDevice, HostInputEvent, HostKeyIdentity,
    HostKeySignal, HostPointerMotion, HostRepeatEvidence,
};

fn device(source_id: &str, keyboard: bool, pointer: bool) -> Arc<HostInputDevice> {
    Arc::new(HostInputDevice {
        source_id: Arc::from(source_id),
        label: Arc::from(source_id),
        capabilities: HostInputCapabilities { keyboard, pointer },
        session_generation: 1,
        device_generation: 1,
    })
}

fn batch(events: &[HostInputEvent]) -> HostInputBatch<'_> {
    HostInputBatch {
        events,
        pointer: None,
        at_ms: 100,
        device_catalog_generation: 1,
    }
}

#[test]
fn adapter_delegates_unknown_repeat_evidence_to_shared_fold() {
    let keyboard = device("windows:kbd", true, false);
    let key = || HostInputEvent::Key {
        device: Some(Arc::clone(&keyboard)),
        identity: HostKeyIdentity {
            key: Arc::from("a"),
            physical_code: Arc::from("windows:set1:none:1e"),
        },
        signal: HostKeySignal::Edge {
            pressed: true,
            repeat: HostRepeatEvidence::Unknown,
        },
    };
    let events = [
        HostInputEvent::DeviceArrived {
            device: Arc::clone(&keyboard),
        },
        key(),
        key(),
    ];
    let mut input = WindowsHostInput::new(true, false);
    let (snapshot, folded) = input.fold_and_snapshot(batch(&events));

    assert_eq!(snapshot.keyboard.pressed_keys, ["a"]);
    assert_eq!(snapshot.keyboard.recent_keys, ["a"]);
    assert!(matches!(
        folded[1].event,
        InputEvent::Key {
            state: InputButtonState::Repeated,
            ..
        }
    ));
}

#[test]
fn adapter_delegates_device_removal_release_synthesis() {
    let keyboard = device("windows:kbd", true, false);
    let press = HostInputEvent::Key {
        device: Some(Arc::clone(&keyboard)),
        identity: HostKeyIdentity {
            key: Arc::from("a"),
            physical_code: Arc::from("windows:set1:none:1e"),
        },
        signal: HostKeySignal::Edge {
            pressed: true,
            repeat: HostRepeatEvidence::Unknown,
        },
    };
    let mut input = WindowsHostInput::new(true, false);
    input.fold_and_snapshot(batch(&[press]));
    let (snapshot, folded) =
        input.fold_and_snapshot(batch(&[HostInputEvent::DeviceRemoved { device: keyboard }]));

    assert!(snapshot.keyboard.pressed_keys.is_empty());
    assert!(matches!(
        folded[0].event,
        InputEvent::Key {
            state: InputButtonState::Released,
            ..
        }
    ));
}

#[test]
fn adapter_preserves_explicit_relative_scale() {
    let mouse = device("windows:mouse", false, true);
    let events = [HostInputEvent::Motion {
        device: Some(mouse),
        motion: HostPointerMotion::Relative {
            delta_x: 300.0,
            delta_y: 400.0,
            units_per_x: 1200.0,
            units_per_y: 1200.0,
        },
    }];
    let mut input = WindowsHostInput::new(false, true);
    let (snapshot, _) = input.fold_and_snapshot(batch(&events));
    assert!((snapshot.batch.motion.distance - 5.0 / 12.0).abs() < 1e-6);
}
