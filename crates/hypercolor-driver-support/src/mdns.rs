//! Shared mDNS browse helpers for network backends.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use hypercolor_driver_api::CoverageGap;
use mdns_sd::{IfKind, ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent};
use tracing::debug;

use crate::discovery::{CandidateAdmission, KnownTarget, ScanOutcome};

const SHUTDOWN_WAIT: Duration = Duration::from_millis(250);

/// Hosts one browse retains beyond the ones the caller already knows. Any
/// host on the LAN can advertise, so this bounds what advertisements can
/// make a scan keep and then probe.
const MAX_BROWSED_HOSTS: usize = 256;

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

    /// Run one browse as a leg of a scan that also probes `known_hosts`.
    ///
    /// # Errors
    ///
    /// Returns an error when mDNS cannot run and no known host gives the scan
    /// anything else to look at. With known hosts to fall back on, the
    /// failure is a gap instead, so what they answer still applies.
    pub async fn scan(
        scope: MdnsScope,
        service_type: &str,
        timeout: Duration,
        known_hosts: &[IpAddr],
    ) -> Result<ScanOutcome<MdnsService>> {
        let browse = async {
            Self::with_scope(scope)?
                .browse(service_type, timeout, known_hosts)
                .await
        };
        match browse.await {
            Err(error) if !known_hosts.is_empty() => Ok(ScanOutcome {
                gaps: vec![CoverageGap::SourceFailed {
                    source_name: format!("mDNS browse of {service_type}"),
                    reason: format!("{error:#}"),
                }],
                ..ScanOutcome::default()
            }),
            outcome => outcome,
        }
    }

    /// Browse for one service type until `timeout` expires.
    ///
    /// `known_hosts` are always admitted, however many other hosts
    /// advertise.
    ///
    /// # Errors
    ///
    /// Returns an error if the browse cannot start. A failure after that
    /// leaves a gap beside whatever resolved before it.
    pub async fn browse(
        &self,
        service_type: &str,
        timeout: Duration,
        known_hosts: &[IpAddr],
    ) -> Result<ScanOutcome<MdnsService>> {
        let receiver = self
            .daemon
            .browse(service_type)
            .with_context(|| format!("failed to browse mDNS service type {service_type}"))?;

        let deadline = tokio::time::Instant::now() + timeout;
        let mut admission = CandidateAdmission::new(
            MAX_BROWSED_HOSTS,
            known_hosts.iter().copied().map(KnownTarget::Address),
        );
        let mut failure = None;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            match tokio::time::timeout(remaining, receiver.recv_async()).await {
                Ok(Ok(ServiceEvent::ServiceResolved(info))) => {
                    admit_resolved(&mut admission, &info);
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    failure = Some(CoverageGap::ReceiveFailed {
                        reason: format!("mDNS recv error: {error}"),
                    });
                    break;
                }
                Err(_) => break,
            }
        }

        let mut outcome = admission.finish();
        outcome.gaps.extend(failure);
        outcome.found.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then(left.port.cmp(&right.port))
                .then(left.host.to_string().cmp(&right.host.to_string()))
        });
        Ok(outcome)
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

/// Admit one resolved service under its host. Keying by host keeps one
/// address that advertises on many ports from claiming more than one slot.
fn admit_resolved(admission: &mut CandidateAdmission<IpAddr, MdnsService>, info: &ResolvedService) {
    let Some(host) = preferred_host(info.get_addresses().iter().map(ScopedIp::to_ip_addr)) else {
        debug!(
            service = info.get_fullname(),
            "resolved mDNS service without addresses; skipping"
        );
        return;
    };

    // Fingerprints derive from this name, and an empty key is not one.
    let name = match info.get_hostname().trim_end_matches('.') {
        "" => host.to_string(),
        name => name.to_owned(),
    };
    let txt = info
        .get_properties()
        .iter()
        .map(|property| (property.key().to_owned(), property.val_str().to_owned()))
        .collect::<HashMap<_, _>>();
    admission.offer_latest(
        host,
        host,
        MdnsService {
            name,
            host,
            port: info.get_port(),
            txt,
        },
    );
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

#[cfg(test)]
mod tests {
    use mdns_sd::ServiceInfo;

    use super::*;

    fn resolved(host: IpAddr, port: u16) -> ResolvedService {
        ServiceInfo::new(
            "_flood._tcp.local.",
            &format!("flood-{host}"),
            "flood.local.",
            host,
            port,
            None,
        )
        .expect("valid service")
        .as_resolved_service()
    }

    fn host(index: u32) -> IpAddr {
        IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + index))
    }

    #[test]
    fn a_flood_of_advertisements_retains_a_bounded_set_and_the_known_host() {
        let known = host(9_999);
        let mut admission = CandidateAdmission::new(4, [KnownTarget::Address(known)]);
        for index in 0..500 {
            admit_resolved(&mut admission, &resolved(host(index), 80));
        }
        admit_resolved(&mut admission, &resolved(known, 80));

        let outcome = admission.finish();
        assert_eq!(outcome.found.len(), 5);
        assert_eq!(outcome.found[0].host, known);
        assert_eq!(
            outcome.gaps,
            vec![CoverageGap::AdmissionFull { refused: 496 }]
        );
    }

    #[test]
    fn a_service_with_an_empty_hostname_is_named_after_its_address() {
        let mut admission = CandidateAdmission::new(1, []);
        let mut service = resolved(host(1), 80);
        service.host = ".".to_owned();
        admit_resolved(&mut admission, &service);

        assert_eq!(admission.finish().found[0].name, host(1).to_string());
    }

    #[test]
    fn one_address_on_many_ports_holds_one_slot_and_reports_its_latest() {
        let mut admission = CandidateAdmission::new(1, []);
        for port in 8_000..8_100 {
            admit_resolved(&mut admission, &resolved(host(1), port));
        }

        let outcome = admission.finish();
        assert_eq!(outcome.found.len(), 1);
        assert_eq!(outcome.found[0].port, 8_099);
        assert!(outcome.gaps.is_empty());
    }
}
