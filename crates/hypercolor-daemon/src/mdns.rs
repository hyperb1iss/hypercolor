//! mDNS publisher for advertising Hypercolor daemons on the local network.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceInfo, UnregisterStatus};
use tokio::time::timeout;
use tracing::{debug, info};

use hypercolor_types::server::ServerIdentity;

const HYPERCOLOR_SERVICE_TYPE: &str = "_hypercolor._tcp.local.";
const DEFAULT_API_BASE: &str = "/api/v1";
const MDNS_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

/// Active Hypercolor mDNS advertisement.
pub struct MdnsPublisher {
    daemon: ServiceDaemon,
    fullname: String,
}

impl MdnsPublisher {
    /// Publish this daemon on the local network if discovery is enabled.
    ///
    /// # Errors
    ///
    /// Returns an error if mDNS advertisement is requested but registration fails.
    pub fn new(
        identity: &ServerIdentity,
        bind: SocketAddr,
        publish_enabled: bool,
        auth_required: bool,
    ) -> Result<Option<Self>> {
        if !publish_enabled || bind.ip().is_loopback() {
            return Ok(None);
        }

        let daemon = ServiceDaemon::new().context("failed to create mDNS publisher")?;
        let host_name = host_name(identity);
        let auth = if auth_required { "api_key" } else { "none" };
        let properties = [
            ("id", identity.instance_id.as_str()),
            ("name", identity.instance_name.as_str()),
            ("version", identity.version.as_str()),
            ("api", DEFAULT_API_BASE),
            ("auth", auth),
        ];

        let service = if bind.ip().is_unspecified() {
            ServiceInfo::new(
                HYPERCOLOR_SERVICE_TYPE,
                &identity.instance_name,
                &host_name,
                "",
                bind.port(),
                &properties[..],
            )
            .context("failed to build Hypercolor mDNS service")?
            .enable_addr_auto()
        } else {
            ServiceInfo::new(
                HYPERCOLOR_SERVICE_TYPE,
                &identity.instance_name,
                &host_name,
                bind.ip().to_string(),
                bind.port(),
                &properties[..],
            )
            .context("failed to build Hypercolor mDNS service")?
        };

        let fullname = service.get_fullname().to_owned();
        daemon
            .register(service)
            .context("failed to register Hypercolor mDNS service")?;

        info!(fullname = %fullname, bind = %bind, "Published Hypercolor mDNS service");

        Ok(Some(Self { daemon, fullname }))
    }

    /// Stop advertising the daemon and shut down the publisher cleanly.
    pub async fn shutdown(self) {
        if let Ok(receiver) = self.daemon.unregister(&self.fullname) {
            match timeout(MDNS_SHUTDOWN_TIMEOUT, receiver.recv_async()).await {
                Ok(Ok(UnregisterStatus::OK)) => {}
                Ok(Ok(UnregisterStatus::NotFound)) => {
                    debug!(fullname = %self.fullname, "mDNS service already absent during shutdown");
                }
                Ok(Err(error)) => {
                    debug!(%error, "mDNS unregister channel closed");
                }
                Err(_) => {
                    debug!(
                        timeout_ms = MDNS_SHUTDOWN_TIMEOUT.as_millis(),
                        "Timed out waiting for mDNS unregister"
                    );
                }
            }
        }

        if let Ok(receiver) = self.daemon.shutdown() {
            match timeout(MDNS_SHUTDOWN_TIMEOUT, receiver.recv_async()).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    debug!(%error, "mDNS daemon shutdown channel closed");
                }
                Err(_) => {
                    debug!(
                        timeout_ms = MDNS_SHUTDOWN_TIMEOUT.as_millis(),
                        "Timed out waiting for mDNS daemon shutdown"
                    );
                }
            }
        }
    }
}

fn host_name(identity: &ServerIdentity) -> String {
    let base = sanitize_dns_label(&identity.instance_name);
    let suffix: String = identity.instance_id.chars().take(8).collect();
    format!("{base}-{suffix}.local.")
}

fn sanitize_dns_label(input: &str) -> String {
    let mut label = input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned();

    if label.is_empty() {
        label = "hypercolor".to_owned();
    }

    label.truncate(48);
    label
}
