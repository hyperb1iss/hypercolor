#[cfg(not(target_os = "linux"))]
use std::sync::Arc;

#[cfg(not(target_os = "linux"))]
use hypercolor_linux_input::{EvdevInputConfig, EvdevInputError, start_evdev_input};

#[cfg(not(target_os = "linux"))]
#[test]
fn native_factory_reports_unsupported_platform() {
    let error = start_evdev_input(
        EvdevInputConfig {
            keyboard: true,
            pointer: true,
            session_generation: 1,
            clock: Arc::new(|| 0),
        },
        |_| {},
    )
    .err()
    .expect("non-Linux factory must fail");
    assert_eq!(error, EvdevInputError::UnsupportedPlatform);
}
