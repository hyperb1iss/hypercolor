#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

//! Audited XDG Portal and PipeWire capture boundary.

use std::fmt;
use std::fs::File;

mod channel;
mod model;

pub use channel::{LoopReceiver, LoopSendError, LoopSender, loop_channel};
pub use model::{
    BufferFault, CallbackAction, CaptureFormatRequest, D4Transform, DequeueOutcome, DmaBufIdentity,
    FormatEvent, FormatFault, FormatOffer, FormatOfferError, MetaFault, NegotiatedVideoFormat,
    PackedVideoFormat, PixelCrop, SpaBufferView, SpaChunk, StateChange, StreamError, StreamState,
    VideoFraction,
};

/// One source selected through the XDG ScreenCast portal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortalStreamDescriptor {
    source_name: String,
    node_id: u32,
    position: (i32, i32),
    logical_size: Option<(u32, u32)>,
}

impl PortalStreamDescriptor {
    /// Returns the stable portal source label.
    #[must_use]
    pub fn source_name(&self) -> &str {
        &self.source_name
    }

    /// Returns the PipeWire node selected by the portal.
    #[must_use]
    pub const fn node_id(&self) -> u32 {
        self.node_id
    }

    /// Returns the logical desktop position reported by the portal.
    #[must_use]
    pub const fn position(&self) -> (i32, i32) {
        self.position
    }

    /// Returns the logical source size when the portal supplied valid dimensions.
    #[must_use]
    pub const fn logical_size(&self) -> Option<(u32, u32)> {
        self.logical_size
    }
}

/// Portal selection inputs owned by the caller's persisted settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortalRequest {
    /// Restore token from an earlier explicitly persisted portal selection.
    pub restore_token: Option<String>,
}

/// PipeWire remote plus neutral source metadata.
#[derive(Debug)]
pub struct PortalRemote {
    descriptor: PortalStreamDescriptor,
    file: File,
}

impl PortalRemote {
    /// Returns the selected source metadata.
    #[must_use]
    pub const fn descriptor(&self) -> &PortalStreamDescriptor {
        &self.descriptor
    }

    /// Splits the remote into its metadata and transport file.
    #[must_use]
    pub fn into_parts(self) -> (PortalStreamDescriptor, File) {
        (self.descriptor, self.file)
    }
}

/// Failure at the portal ownership boundary.
#[derive(Debug, thiserror::Error)]
pub enum PortalError {
    /// The host does not provide the Linux portal boundary.
    #[error("XDG ScreenCast portal capture is unavailable on this platform")]
    UnsupportedPlatform,
    /// A portal operation failed or the user denied the request.
    #[error("{operation}: {detail}")]
    Operation {
        /// Stable operation label.
        operation: &'static str,
        /// Native diagnostic text.
        detail: String,
    },
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(not(target_os = "linux"))]
mod stubs;

#[cfg(target_os = "linux")]
pub use linux::{PortalSession, PortalSessionGuard, ProcessBuffer, open_portal_session};
#[cfg(not(target_os = "linux"))]
pub use stubs::{PortalSession, PortalSessionGuard, ProcessBuffer, open_portal_session};

#[cfg(target_os = "linux")]
pub use linux::{StreamControl, StreamSession, connect_stream};
#[cfg(not(target_os = "linux"))]
pub use stubs::{StreamControl, StreamSession, connect_stream};

/// Synchronous policy handler invoked by the native stream callbacks.
pub trait StreamEventHandler: 'static {
    /// Handles one translated format event.
    fn format_changed(&mut self, control: &StreamControl<'_>, event: FormatEvent)
    -> CallbackAction;

    /// Handles one translated stream-state change.
    fn state_changed(&mut self, event: StateChange) -> CallbackAction;

    /// Handles one exact native process opportunity.
    fn process(&mut self, buffer: ProcessBuffer<'_>) -> CallbackAction;
}

/// Native stream-construction failure that preserves the command receiver.
pub struct StreamConnectError<C: 'static> {
    error: StreamError,
    receiver: LoopReceiver<C>,
}

impl<C: 'static> StreamConnectError<C> {
    pub(crate) const fn new(error: StreamError, receiver: LoopReceiver<C>) -> Self {
        Self { error, receiver }
    }

    /// Splits the failure into its diagnostic and retained command receiver.
    #[must_use]
    pub fn into_parts(self) -> (StreamError, LoopReceiver<C>) {
        (self.error, self.receiver)
    }
}

impl<C: 'static> fmt::Debug for StreamConnectError<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StreamConnectError")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl<C: 'static> fmt::Display for StreamConnectError<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl<C: 'static> std::error::Error for StreamConnectError<C> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

#[cfg(test)]
mod tests {
    use super::{PortalError, PortalStreamDescriptor};

    #[test]
    fn portal_descriptor_preserves_neutral_source_topology() {
        let descriptor = PortalStreamDescriptor {
            source_name: "DP-1".to_owned(),
            node_id: 47,
            position: (-1920, 240),
            logical_size: Some((2560, 1440)),
        };

        assert_eq!(descriptor.source_name(), "DP-1");
        assert_eq!(descriptor.node_id(), 47);
        assert_eq!(descriptor.position(), (-1920, 240));
        assert_eq!(descriptor.logical_size(), Some((2560, 1440)));
    }

    #[test]
    fn unsupported_portal_error_has_a_stable_diagnostic() {
        assert_eq!(
            PortalError::UnsupportedPlatform.to_string(),
            "XDG ScreenCast portal capture is unavailable on this platform"
        );
    }
}
