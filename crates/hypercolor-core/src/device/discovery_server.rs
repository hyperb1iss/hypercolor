//! Hypercolor server discovery over mDNS/DNS-SD.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use reqwest::Client;
use serde::Deserialize;
use tokio::time::Instant;
use tracing::{debug, warn};

use hypercolor_types::server::{DiscoveredServer, ServerIdentity};

const HYPERCOLOR_SERVICE_TYPE: &str = "_hypercolor._tcp.local.";
const DEFAULT_API_BASE: &str = "/api/v1";
const PROBE_TIMEOUT: Duration = Duration::from_millis(750);
const MDNS_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

/// Browse the local network for Hypercolor daemons via mDNS.
///
/// # Errors
///
/// Returns an error if the local mDNS browser cannot be created.
pub async fn discover_servers(timeout: Duration) -> Result<Vec<DiscoveredServer>> {
    let daemon = ServiceDaemon::new().context("failed to create mDNS daemon")?;
    let receiver = daemon
        .browse(HYPERCOLOR_SERVICE_TYPE)
        .context("failed to start Hypercolor mDNS browse")?;
    let http = Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .context("failed to build discovery HTTP client")?;

    let deadline = Instant::now() + timeout;
    let mut discovered = HashMap::<String, DiscoveredServer>::new();

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }

        match tokio::time::timeout(remaining, async {
            receiver
                .recv_async()
                .await
                .map_err(|error| anyhow::anyhow!("mDNS recv error: {error}"))
        })
        .await
        {
            Ok(Ok(ServiceEvent::ServiceResolved(info))) => {
                if let Some(server) = resolve_server(&http, &info).await {
                    discovered.insert(server.identity.instance_id.clone(), server);
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                warn!(%error, "Hypercolor mDNS browse failed");
                break;
            }
            Err(_) => break,
        }
    }

    drain_mdns_shutdown(daemon).await;

    let mut servers: Vec<_> = discovered.into_values().collect();
    servers.sort_by(|left, right| {
        left.identity
            .instance_name
            .cmp(&right.identity.instance_name)
            .then_with(|| left.host.cmp(&right.host))
            .then_with(|| left.port.cmp(&right.port))
    });
    Ok(servers)
}

async fn resolve_server(http: &Client, info: &ServiceInfo) -> Option<DiscoveredServer> {
    let host = preferred_host(info)?;
    let port = info.get_port();
    let api_base = info.get_property_val_str("api").unwrap_or(DEFAULT_API_BASE);
    let fallback_identity = txt_identity(info);
    let fallback_auth_required = matches!(info.get_property_val_str("auth"), Some("api_key"));

    if let Some(probe) = probe_server(http, host, port, api_base).await {
        return Some(DiscoveredServer {
            identity: ServerIdentity {
                instance_id: probe.instance_id,
                instance_name: probe.instance_name,
                version: probe.version,
            },
            host,
            port,
            device_count: Some(probe.device_count),
            auth_required: probe.auth_required,
        });
    }

    fallback_identity.map(|identity| DiscoveredServer {
        identity,
        host,
        port,
        device_count: None,
        auth_required: fallback_auth_required,
    })
}

fn preferred_host(info: &ServiceInfo) -> Option<IpAddr> {
    let mut addresses: Vec<_> = info.get_addresses().iter().copied().collect();
    addresses.sort_by(|left, right| {
        left.is_ipv6()
            .cmp(&right.is_ipv6())
            .then_with(|| left.to_string().cmp(&right.to_string()))
    });
    addresses.into_iter().next()
}

fn txt_identity(info: &ServiceInfo) -> Option<ServerIdentity> {
    let instance_id = info.get_property_val_str("id")?.trim();
    let instance_name = info.get_property_val_str("name")?.trim();
    let version = info.get_property_val_str("version")?.trim();

    if instance_id.is_empty() || instance_name.is_empty() || version.is_empty() {
        return None;
    }

    Some(ServerIdentity {
        instance_id: instance_id.to_owned(),
        instance_name: instance_name.to_owned(),
        version: version.to_owned(),
    })
}

async fn probe_server(
    http: &Client,
    host: IpAddr,
    port: u16,
    api_base: &str,
) -> Option<ServerProbeData> {
    let url = format!(
        "http://{host}:{port}{}/server",
        normalize_api_base(api_base)
    );
    match http.get(&url).send().await {
        Ok(response) if response.status().is_success() => {
            match response.json::<ApiEnvelope<ServerProbeData>>().await {
                Ok(envelope) => envelope.data,
                Err(error) => {
                    debug!(%error, %url, "Failed to parse server discovery probe");
                    None
                }
            }
        }
        Ok(response) => {
            debug!(status = %response.status(), %url, "Server discovery probe failed");
            None
        }
        Err(error) => {
            debug!(%error, %url, "Server discovery probe request failed");
            None
        }
    }
}

fn normalize_api_base(api_base: &str) -> String {
    let trimmed = api_base.trim();
    if trimmed.is_empty() {
        return DEFAULT_API_BASE.to_owned();
    }

    if trimmed.starts_with('/') {
        trimmed.trim_end_matches('/').to_owned()
    } else {
        format!("/{}", trimmed.trim_end_matches('/'))
    }
}

async fn drain_mdns_shutdown(daemon: ServiceDaemon) {
    match daemon.shutdown() {
        Ok(receiver) => {
            match tokio::time::timeout(MDNS_SHUTDOWN_TIMEOUT, receiver.recv_async()).await {
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
        Err(error) => {
            debug!(%error, "Failed to request mDNS daemon shutdown");
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope<T> {
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct ServerProbeData {
    instance_id: String,
    instance_name: String,
    version: String,
    device_count: usize,
    auth_required: bool,
}
