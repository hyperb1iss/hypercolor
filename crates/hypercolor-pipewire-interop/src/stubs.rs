use std::marker::PhantomData;

use super::{
    CallbackAction, DequeueOutcome, FormatOffer, LoopReceiver, NegotiatedVideoFormat, PortalError,
    PortalRemote, PortalRequest, PortalStreamDescriptor, SpaBufferView, StreamConnectError,
    StreamError, StreamEventHandler,
};

/// Stub portal selection on hosts without XDG ScreenCast support.
pub enum PortalSession {}

impl PortalSession {
    /// Unreachable: no stub portal session can exist.
    #[must_use]
    pub fn descriptor(&self) -> &PortalStreamDescriptor {
        match *self {}
    }

    /// Unreachable: no stub portal session can exist.
    #[must_use]
    pub fn into_parts(self) -> (PortalSessionGuard, PortalRemote, Option<String>) {
        match self {}
    }
}

/// Stub portal lifetime guard on hosts without XDG ScreenCast support.
pub struct PortalSessionGuard;

/// Stub process callback authority on hosts without PipeWire.
pub struct ProcessBuffer<'a> {
    _lifetime: PhantomData<&'a ()>,
}

impl ProcessBuffer<'_> {
    /// Reports that no native buffer can be dequeued on this platform.
    pub fn visit<V>(self, _visitor: impl FnOnce(SpaBufferView<'_>) -> V) -> DequeueOutcome<V> {
        DequeueOutcome::Empty
    }
}

/// Stub stream control on hosts without PipeWire.
pub struct StreamControl<'a> {
    _lifetime: PhantomData<&'a ()>,
}

impl StreamControl<'_> {
    /// Reports that native stream activity cannot be changed on this platform.
    pub fn set_active(&self, _active: bool) -> Result<(), StreamError> {
        Err(StreamError::UnsupportedPlatform)
    }

    /// Reports that native stream formats cannot be changed on this platform.
    pub fn update_format(&self, _offer: &FormatOffer) -> Result<(), StreamError> {
        Err(StreamError::UnsupportedPlatform)
    }

    /// Reports that native buffer contracts cannot be changed on this platform.
    pub fn acknowledge_format(&self, _format: NegotiatedVideoFormat) -> Result<(), StreamError> {
        Err(StreamError::UnsupportedPlatform)
    }
}

/// Stub native stream session on hosts without PipeWire.
pub struct StreamSession<C: 'static, H, K>
where
    H: StreamEventHandler,
    K: Fn(&StreamControl<'_>, C) -> CallbackAction + 'static,
{
    _types: PhantomData<(C, H, K)>,
}

impl<C: 'static, H, K> StreamSession<C, H, K>
where
    H: StreamEventHandler,
    K: Fn(&StreamControl<'_>, C) -> CallbackAction + 'static,
{
    /// Reports that the native stream loop is unavailable on this platform.
    pub fn run(&mut self) -> Result<(), StreamError> {
        Err(StreamError::UnsupportedPlatform)
    }

    /// Stub sessions cannot be constructed, so disconnect is unreachable.
    pub fn disconnect(self) -> (LoopReceiver<C>, Result<(), StreamError>) {
        unreachable!("unsupported hosts cannot construct a native stream session")
    }
}

/// Reports that native stream construction is unavailable on this platform.
///
/// # Errors
///
/// Always returns the unsupported-platform error and retained receiver.
pub fn connect_stream<C, H, K>(
    _remote: PortalRemote,
    _offer: &FormatOffer,
    receiver: LoopReceiver<C>,
    _handler: H,
    _command_handler: K,
) -> Result<StreamSession<C, H, K>, StreamConnectError<C>>
where
    C: 'static,
    H: StreamEventHandler,
    K: Fn(&StreamControl<'_>, C) -> CallbackAction + 'static,
{
    Err(StreamConnectError::new(
        StreamError::UnsupportedPlatform,
        receiver,
    ))
}

impl PortalSessionGuard {
    /// Reports that the native portal session is unavailable on this platform.
    pub async fn close(self) -> Result<(), PortalError> {
        Err(PortalError::UnsupportedPlatform)
    }
}

/// Reports that XDG ScreenCast capture is unavailable on this platform.
pub async fn open_portal_session(_request: &PortalRequest) -> Result<PortalSession, PortalError> {
    Err(PortalError::UnsupportedPlatform)
}
