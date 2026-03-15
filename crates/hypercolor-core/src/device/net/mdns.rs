//! Shared mDNS browse helpers for network backends.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use tracing::debug;

const SHUTDOWN_WAIT: Duration = Duration::from_millis(250);

/// Shared mDNS browser with the resolution behavior Hypercolor expects.
pub struct MdnsBrowser {
    daemon: ServiceDaemon,
}

/// One resolved mDNS service endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdnsService {
    pub name: String,
    pub host: IpAddr,
    pub port: u16,
    pub txt: HashMap<String, String>,
}

impl MdnsBrowser {
    /// Create a new mDNS browser.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `mdns-sd` daemon cannot start.
    pub fn new() -> Result<Self> {
        let daemon = ServiceDaemon::new().context("failed to create mDNS daemon")?;
        Ok(Self { daemon })
    }

    /// Browse for one service type until `timeout` expires.
    ///
    /// # Errors
    ///
    /// Returns an error if the browse cannot start or the daemon reports a
    /// receive failure before the timeout elapses.
    pub async fn browse(&self, service_type: &str, timeout: Duration) -> Result<Vec<MdnsService>> {
        let receiver = self
            .daemon
            .browse(service_type)
            .with_context(|| format!("failed to browse mDNS service type {service_type}"))?;

        let deadline = tokio::time::Instant::now() + timeout;
        let mut resolved = HashMap::new();

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
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
                    let Some(host) = preferred_host(info.get_addresses().iter().copied()) else {
                        debug!(
                            service = info.get_fullname(),
                            "resolved mDNS service without addresses; skipping"
                        );
                        continue;
                    };

                    let name = info.get_hostname().trim_end_matches('.').to_owned();
                    let txt = info
                        .get_properties()
                        .iter()
                        .map(|property| (property.key().to_owned(), property.val_str().to_owned()))
                        .collect::<HashMap<_, _>>();

                    resolved.insert(
                        (host, info.get_port()),
                        MdnsService {
                            name,
                            host,
                            port: info.get_port(),
                            txt,
                        },
                    );
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => return Err(error),
                Err(_) => break,
            }
        }

        let mut services: Vec<_> = resolved.into_values().collect();
        services.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then(left.port.cmp(&right.port))
                .then(left.host.to_string().cmp(&right.host.to_string()))
        });
        Ok(services)
    }

    /// Shut down the underlying `mdns-sd` daemon.
    pub fn shutdown(&self) {
        match self.daemon.shutdown() {
            Ok(receiver) => {
                if let Err(error) = receiver.recv_timeout(SHUTDOWN_WAIT) {
                    debug!(error = %error, "timed out waiting for mDNS daemon shutdown");
                }
            }
            Err(error) => {
                debug!(error = %error, "failed to request mDNS daemon shutdown");
            }
        }
    }
}

impl Drop for MdnsBrowser {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn preferred_host<I>(addresses: I) -> Option<IpAddr>
where
    I: IntoIterator<Item = IpAddr>,
{
    let mut all = addresses.into_iter().collect::<Vec<_>>();
    all.sort_by_key(std::string::ToString::to_string);
    all.iter()
        .copied()
        .find(IpAddr::is_ipv4)
        .or_else(|| all.into_iter().next())
}
