use std::fs::File;

use ashpd::desktop::{
    CreateSessionOptions, PersistMode, Session,
    screencast::{
        CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
        StartCastOptions,
    },
};
use tracing::info;

use super::{PortalError, PortalRemote, PortalRequest, PortalStreamDescriptor};

/// Live portal selection retaining the native session until capture ends.
pub struct PortalSession {
    guard: PortalSessionGuard,
    remote: PortalRemote,
    restore_token: Option<String>,
}

impl PortalSession {
    /// Returns the selected source metadata.
    #[must_use]
    pub const fn descriptor(&self) -> &PortalStreamDescriptor {
        self.remote.descriptor()
    }

    /// Splits the session into its lifetime guard, remote, and replacement token.
    #[must_use]
    pub fn into_parts(self) -> (PortalSessionGuard, PortalRemote, Option<String>) {
        (self.guard, self.remote, self.restore_token)
    }
}

/// Opaque lifetime guard for the native XDG ScreenCast session.
pub struct PortalSessionGuard {
    _session: Session<Screencast>,
}

impl PortalSessionGuard {
    /// Closes the native portal session and consumes its lifetime authority.
    pub async fn close(self) -> Result<(), PortalError> {
        self._session
            .close()
            .await
            .map_err(|error| operation("failed to close ScreenCast portal session", error))
    }
}

/// Opens one monitor selection through the XDG ScreenCast portal.
pub async fn open_portal_session(request: &PortalRequest) -> Result<PortalSession, PortalError> {
    let proxy = Screencast::new()
        .await
        .map_err(|error| operation("failed to connect to XDG ScreenCast portal", error))?;
    let session = proxy
        .create_session(CreateSessionOptions::default())
        .await
        .map_err(|error| operation("failed to create ScreenCast portal session", error))?;

    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Hidden)
                .set_sources(Some(SourceType::Monitor.into()))
                .set_multiple(false)
                .set_restore_token(request.restore_token.as_deref())
                .set_persist_mode(PersistMode::ExplicitlyRevoked),
        )
        .await
        .map_err(|error| operation("failed to open ScreenCast source picker", error))?;

    let response = proxy
        .start(&session, None, StartCastOptions::default())
        .await
        .map_err(|error| operation("failed to start ScreenCast portal session", error))?
        .response()
        .map_err(|error| operation("screen capture request was denied or cancelled", error))?;
    let restore_token = response.restore_token().map(ToOwned::to_owned);
    let stream = response
        .streams()
        .first()
        .cloned()
        .ok_or_else(|| PortalError::Operation {
            operation: "portal did not return a monitor stream",
            detail: "empty stream response".to_owned(),
        })?;
    let file = File::from(
        proxy
            .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
            .await
            .map_err(|error| operation("failed to open PipeWire remote", error))?,
    );
    let source_name = stream
        .id()
        .or_else(|| stream.mapping_id())
        .unwrap_or("monitor")
        .to_owned();
    let logical_size = stream.size().and_then(|(width, height)| {
        Some((u32::try_from(width).ok()?, u32::try_from(height).ok()?))
    });
    let descriptor = PortalStreamDescriptor {
        source_name,
        node_id: stream.pipe_wire_node_id(),
        position: stream.position().unwrap_or_default(),
        logical_size,
    };

    info!(
        pipewire_node = descriptor.node_id,
        source = descriptor.source_name,
        restored = request.restore_token.is_some(),
        "Wayland screencast session established"
    );

    Ok(PortalSession {
        guard: PortalSessionGuard { _session: session },
        remote: PortalRemote { descriptor, file },
        restore_token,
    })
}

fn operation(operation: &'static str, error: impl std::fmt::Display) -> PortalError {
    PortalError::Operation {
        operation,
        detail: error.to_string(),
    }
}
