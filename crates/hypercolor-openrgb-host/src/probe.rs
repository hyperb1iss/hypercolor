//! Probe an OpenRGB SDK server over the clean-room SDK client.

use std::net::SocketAddr;
use std::time::Duration;

use hypercolor_openrgb_sdk::{OpenRgbClient, OpenRgbClientConfig};
use tracing::debug;

use crate::types::ServerProbe;

/// Client name announced during the probe handshake.
pub const PROBE_CLIENT_NAME: &str = "Hypercolor Probe";

/// Default port OpenRGB's SDK server listens on.
pub const DEFAULT_SERVER_PORT: u16 = 6742;

/// Connect to an OpenRGB SDK server and report what it answers.
///
/// `timeout` bounds each step (connect, protocol negotiation, controller
/// count), so the worst case is a small multiple of it. The probe never
/// fails: an unreachable or misbehaving server comes back as
/// `reachable: false` with the error text in `error`.
pub async fn probe_server(addr: SocketAddr, timeout: Duration) -> ServerProbe {
    let config = OpenRgbClientConfig {
        client_name: PROBE_CLIENT_NAME.to_owned(),
        connect_timeout: timeout,
        read_timeout: timeout,
        write_timeout: timeout,
        ..OpenRgbClientConfig::default()
    };

    let mut client = match OpenRgbClient::connect(addr, config).await {
        Ok(client) => client,
        Err(error) => {
            debug!(%addr, %error, "openrgb server probe failed to connect");
            return ServerProbe {
                reachable: false,
                protocol_version: None,
                controller_count: None,
                error: Some(error.to_string()),
            };
        }
    };

    let protocol_version = client.protocol_version();
    match client.controller_count().await {
        Ok(count) => {
            debug!(%addr, protocol_version, count, "openrgb server probe succeeded");
            ServerProbe {
                reachable: true,
                protocol_version: Some(protocol_version),
                controller_count: Some(count),
                error: None,
            }
        }
        Err(error) => {
            debug!(%addr, %error, "openrgb server answered but controller count failed");
            ServerProbe {
                reachable: true,
                protocol_version: Some(protocol_version),
                controller_count: None,
                error: Some(error.to_string()),
            }
        }
    }
}
