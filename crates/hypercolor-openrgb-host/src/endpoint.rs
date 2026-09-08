//! Configured endpoint policy shared by app and CLI lifecycle operations.

use hypercolor_types::config::DriverConfigEntry;
use std::{io, net::SocketAddr};

/// Select the first configured SDK endpoint for local host supervision.
pub fn configured_endpoint(config: &DriverConfigEntry) -> io::Result<SocketAddr> {
    let endpoints = config
        .settings
        .get("endpoints")
        .map(|value| serde_json::from_value::<Vec<SocketAddr>>(value.clone()))
        .transpose()?
        .unwrap_or_else(|| vec![([127, 0, 0, 1], crate::DEFAULT_SERVER_PORT).into()]);
    let endpoint = endpoints.first().copied().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "OpenRGB has no configured SDK endpoint",
        )
    })?;
    if !endpoint.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "OpenRGB's configured endpoint is remote; start its server on that host",
        ));
    }
    Ok(endpoint)
}
