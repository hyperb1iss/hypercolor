use std::path::PathBuf;
use std::sync::Arc;

#[cfg(any(target_os = "linux", test))]
use hypercolor_types::host_input::{
    HostInputDevice, HostInputEvent, HostKeyIdentity, HostKeySignal, HostRepeatEvidence,
    host_key_name_from_evdev,
};

/// Capture configuration for one evdev session.
#[derive(Clone)]
pub struct EvdevInputConfig {
    /// Open keyboard-capable event nodes.
    pub keyboard: bool,
    /// Open relative-pointer event nodes.
    pub pointer: bool,
    /// Monotonic identity for this acquisition session.
    pub session_generation: u64,
    /// Shared monotonic clock sampled once for every published batch.
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl std::fmt::Debug for EvdevInputConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvdevInputConfig")
            .field("keyboard", &self.keyboard)
            .field("pointer", &self.pointer)
            .field("session_generation", &self.session_generation)
            .finish_non_exhaustive()
    }
}

/// Why an event node is not currently contributing input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceOpenState {
    Opened,
    PermissionDenied,
    Ignored,
    Failed(String),
}

/// Per-node result from the latest discovery pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceOpenStatus {
    pub path: PathBuf,
    pub label: String,
    pub state: DeviceOpenState,
}

/// Liveness of the acquisition worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvdevWorkerState {
    Running,
    Failed(String),
}

/// Errors from validating or starting an evdev session.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EvdevInputError {
    #[error("evdev host input is only available on Linux")]
    UnsupportedPlatform,
    #[error("no input kinds enabled for capture")]
    NothingToCapture,
    #[error("failed to spawn the evdev input worker: {0}")]
    WorkerSpawn(String),
    #[error("timed out waiting for the evdev input worker to become ready")]
    WorkerReadyTimeout,
    #[error("evdev input publication panicked during worker startup")]
    InitialPublicationPanicked,
}

pub type EvdevInputResult<T> = Result<T, EvdevInputError>;

#[cfg(any(target_os = "linux", test))]
pub(crate) fn normalize_evdev_key(
    device: &Arc<HostInputDevice>,
    code: u16,
    native_name: &str,
    value: i32,
) -> Option<HostInputEvent> {
    let (pressed, repeat) = match value {
        0 => (false, HostRepeatEvidence::NotRepeat),
        1 => (true, HostRepeatEvidence::NotRepeat),
        2 => (true, HostRepeatEvidence::Repeat),
        _ => return None,
    };
    let key = host_key_name_from_evdev(code).unwrap_or(native_name);
    Some(HostInputEvent::Key {
        device: Some(Arc::clone(device)),
        identity: HostKeyIdentity {
            key: Arc::from(key),
            physical_code: Arc::from(format!("evdev:{native_name}")),
        },
        signal: HostKeySignal::Edge { pressed, repeat },
    })
}

#[cfg(test)]
mod tests {
    use hypercolor_types::host_input::{HostInputCapabilities, HostInputDevice};

    use super::*;

    fn device() -> Arc<HostInputDevice> {
        Arc::new(HostInputDevice {
            source_id: Arc::from("linux:evdev:s4:d2:/dev/input/event7"),
            label: Arc::from("fixture"),
            capabilities: HostInputCapabilities {
                keyboard: true,
                pointer: false,
            },
            session_generation: 4,
            device_generation: 2,
        })
    }

    #[test]
    fn raw_evdev_fixture_normalizes_canonical_identity_and_repeat() {
        let event = normalize_evdev_key(&device(), 30, "KEY_A", 2)
            .expect("repeat fixture is a valid key signal");

        assert!(matches!(
            event,
            HostInputEvent::Key {
                identity: HostKeyIdentity { key, physical_code },
                signal: HostKeySignal::Edge {
                    pressed: true,
                    repeat: HostRepeatEvidence::Repeat,
                },
                ..
            } if &*key == "a" && &*physical_code == "evdev:KEY_A"
        ));
    }

    #[test]
    fn raw_evdev_fixture_preserves_extensible_unknown_name() {
        let event = normalize_evdev_key(&device(), 0x2ff, "KEY_VENDOR_MACRO", 1)
            .expect("press fixture is a valid key signal");

        assert!(matches!(
            event,
            HostInputEvent::Key {
                identity: HostKeyIdentity { key, physical_code },
                signal: HostKeySignal::Edge {
                    pressed: true,
                    repeat: HostRepeatEvidence::NotRepeat,
                },
                ..
            } if &*key == "KEY_VENDOR_MACRO"
                && &*physical_code == "evdev:KEY_VENDOR_MACRO"
        ));
    }

    #[test]
    fn raw_evdev_fixture_rejects_invalid_key_state() {
        assert!(normalize_evdev_key(&device(), 30, "KEY_A", 3).is_none());
    }
}
