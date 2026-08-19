use std::sync::Arc;

use hypercolor_linux_input::{
    DeviceCapabilities, EvdevDeviceDescriptor, EvdevInputEvent, PendingEvents,
};
#[cfg(not(target_os = "linux"))]
use hypercolor_linux_input::{EvdevInputConfig, EvdevInputError, start_evdev_input};

fn descriptor() -> Arc<EvdevDeviceDescriptor> {
    Arc::new(EvdevDeviceDescriptor {
        source_id: Arc::from("linux:evdev:s3:d1:/dev/input/event0"),
        path: Arc::from("/dev/input/event0"),
        label: Arc::from("fixture"),
        capabilities: DeviceCapabilities {
            keyboard: true,
            pointer: false,
        },
        session_epoch: 3,
        device_generation: 1,
    })
}

#[test]
fn pending_batch_is_delivered_once_with_caller_epoch_and_topology() {
    let mut pending = PendingEvents::new();
    pending.push(EvdevInputEvent::DeviceArrived {
        device: descriptor(),
    });

    let mut observations = Vec::new();
    let first = pending.deliver(17, 3, 9, &mut |batch| {
        observations.push((
            batch.events.len(),
            batch.at_ms,
            batch.epoch,
            batch.topology_generation,
        ));
    });
    let second = pending.deliver(18, 3, 9, &mut |_| {});

    assert!(first);
    assert!(!second);
    assert_eq!(observations, vec![(1, 17, 3, 9)]);
}

#[cfg(not(target_os = "linux"))]
#[test]
fn native_factory_reports_unsupported_platform() {
    let error = start_evdev_input(
        EvdevInputConfig {
            keyboard: true,
            pointer: true,
            epoch: 1,
            clock: Arc::new(|| 0),
        },
        |_| {},
    )
    .err()
    .expect("non-Linux factory must fail");
    assert_eq!(error, EvdevInputError::UnsupportedPlatform);
}
