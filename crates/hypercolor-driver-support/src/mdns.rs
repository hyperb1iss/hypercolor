//! Shared mDNS browse helpers for network backends.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use mdns_sd::{IfKind, ScopedIp, ServiceDaemon, ServiceEvent};
use tracing::debug;

const SHUTDOWN_WAIT: Duration = Duration::from_millis(250);

/// Which network interfaces an mDNS daemon may query and announce on.
///
/// `mdns-sd` 0.21 offers no interface selection at construction: it joins
/// its multicast group on every interface when the daemon thread starts,
/// before any scope command lands, so a loopback daemon still emits those
/// IGMP and MLD membership reports. Every query, response, and
/// announcement after that stays on loopback.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MdnsScope {
    /// Every IPv4 interface on the host, which is what discovery needs.
    #[default]
    Network,
    /// Only the IPv4 loopback interface, so traffic never leaves the host.
    Loopback,
}

impl MdnsScope {
    /// Restrict `daemon` to this scope.
    ///
    /// Apply it before the first browse or register call. The daemon
    /// processes commands in order, so later calls see the restricted set.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon has already shut down.
    pub fn restrict(self, daemon: &ServiceDaemon) -> Result<()> {
        match self {
            // Network backends currently expect routable IPv4 addresses for
            // their follow-up HTTP/UDP traffic. Disabling IPv6 browsing also
            // avoids the noisy utun multicast send failures seen on macOS
            // VPN/tunnel interfaces.
            Self::Network => {
                if let Err(error) = daemon.disable_interface(IfKind::IPv6) {
                    debug!(error = %error, "failed to disable IPv6 mDNS interfaces");
                }
            }
            Self::Loopback => {
                daemon
                    .disable_interface(IfKind::All)
                    .context("failed to drop non-loopback mDNS interfaces")?;
                daemon
                    .enable_interface(IfKind::LoopbackV4)
                    .context("failed to enable the IPv4 loopback mDNS interface")?;
            }
        }
        Ok(())
    }
}

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
    /// Create a new mDNS browser over every IPv4 interface.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `mdns-sd` daemon cannot start.
    pub fn new() -> Result<Self> {
        Self::with_scope(MdnsScope::Network)
    }

    /// Create a new mDNS browser limited to `scope`.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `mdns-sd` daemon cannot start or
    /// cannot be restricted to `scope`.
    pub fn with_scope(scope: MdnsScope) -> Result<Self> {
        let daemon = ServiceDaemon::new().context("failed to create mDNS daemon")?;
        let browser = Self { daemon };
        scope.restrict(&browser.daemon)?;
        Ok(browser)
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
                    let Some(host) =
                        preferred_host(info.get_addresses().iter().map(ScopedIp::to_ip_addr))
                    else {
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
