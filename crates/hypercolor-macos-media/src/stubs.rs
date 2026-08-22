use crate::{AutomationBackend, Capability, MediaError, MediaErrorKind, MediaPoll};

pub(crate) struct NativeAutomationBackend;

impl NativeAutomationBackend {
    pub(crate) const fn new() -> Self {
        Self
    }
}

impl AutomationBackend for NativeAutomationBackend {
    fn capability(&self) -> Capability {
        Capability::UnsupportedPlatform
    }

    fn connect(&mut self) -> Result<(), MediaError> {
        Err(MediaError::new(
            MediaErrorKind::UnsupportedCapability,
            None,
            "macOS media Automation is unavailable on this platform",
        ))
    }

    fn request_authorization(&mut self, adapter: crate::MediaAdapter) -> Result<(), MediaError> {
        Err(MediaError::new(
            MediaErrorKind::UnsupportedCapability,
            Some(adapter),
            "macOS media Automation is unavailable on this platform",
        ))
    }

    fn poll(&mut self) -> Result<MediaPoll, MediaError> {
        Err(MediaError::new(
            MediaErrorKind::UnsupportedCapability,
            None,
            "macOS media Automation is unavailable on this platform",
        ))
    }

    fn disconnect(&mut self) {}
}
