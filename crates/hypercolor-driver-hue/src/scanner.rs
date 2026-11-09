//! Hue bridge discovery scanner.

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::Instant;
use tracing::warn;

use hypercolor_driver_api::{CoverageGap, DiscoveryBatch, DiscoveryConnectBehavior};
use hypercolor_driver_support::discovery::{
    DEFAULT_PROBE_CONCURRENCY, ProbeBudget, Probed, ScanOutcome, probe_candidates,
};
use hypercolor_driver_support::{CredentialStore, MdnsBrowser, MdnsScope};
use hypercolor_types::device::DeviceFingerprint;

use super::bridge::{DEFAULT_HUE_API_PORT, HueBridgeClient};
use super::types::{
    HueDiscoveredBridge, HueEntertainmentConfig, HueLight, build_device_info,
    choose_entertainment_config, fingerprint_for_bridge,
};

const HUE_SERVICE_TYPE: &str = "_hue._tcp.local.";
const DEFAULT_SCAN_TIMEOUT: Duration = Duration::from_secs(2);
/// One bridge probe at worst: three REST reads at the client's 5 s timeout,
/// plus a moment so a probe's own timeout lands before the scan deadline.
const PROBE_ROUND: Duration = Duration::from_secs(16);

/// Persistable hint for a known Hue bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HueKnownBridge {
    pub bridge_id: String,
    pub ip: IpAddr,
    pub api_port: u16,
    pub name: String,
    pub model_id: String,
    pub sw_version: String,
}

impl HueKnownBridge {
    /// Create a minimal probe target from an IP address.
    #[must_use]
    pub fn from_ip(ip: IpAddr) -> Self {
        Self {
            bridge_id: String::new(),
            ip,
            api_port: DEFAULT_HUE_API_PORT,
            name: String::new(),
            model_id: String::new(),
            sw_version: String::new(),
        }
    }

    fn merge_from(&mut self, other: &Self) {
        if self.bridge_id.is_empty() {
            self.bridge_id.clone_from(&other.bridge_id);
        }
        if self.api_port == 0 {
            self.api_port = other.api_port;
        }
        if self.name.is_empty() {
            self.name.clone_from(&other.name);
        }
        if self.model_id.is_empty() {
            self.model_id.clone_from(&other.model_id);
        }
        if self.sw_version.is_empty() {
            self.sw_version.clone_from(&other.sw_version);
        }
    }
}

/// One bridge a scan will probe.
struct HueCandidate {
    bridge: HueKnownBridge,
    /// mDNS or N-UPnP vouched for this address, so it counts as seen even
    /// when its REST API does not answer.
    advertised: bool,
}

impl fmt::Display for HueCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.bridge.ip)
    }
}

/// Hue bridge discovery via mDNS, N-UPnP, and direct bridge probing.
pub struct HueScanner {
    scan_timeout: Duration,
    known_bridges: Vec<HueKnownBridge>,
    tracked: HashMap<IpAddr, DeviceFingerprint>,
    mdns_enabled: bool,
    credential_store: Arc<CredentialStore>,
    preferred_entertainment_config: Option<String>,
    nupnp_url: String,
}

impl HueScanner {
    /// Create a scanner with the default timeout and no known bridges.
    #[must_use]
    pub fn new(credential_store: Arc<CredentialStore>) -> Self {
        Self {
            scan_timeout: DEFAULT_SCAN_TIMEOUT,
            known_bridges: Vec::new(),
            tracked: HashMap::new(),
            mdns_enabled: true,
            credential_store,
            preferred_entertainment_config: None,
            nupnp_url: "https://discovery.meethue.com".to_owned(),
        }
    }

    /// Create a scanner seeded with persisted/known-bridge hints.
    #[must_use]
    pub fn with_options(
        known_bridges: Vec<HueKnownBridge>,
        credential_store: Arc<CredentialStore>,
        timeout: Duration,
        mdns_enabled: bool,
        preferred_entertainment_config: Option<String>,
    ) -> Self {
        Self {
            scan_timeout: timeout,
            known_bridges,
            tracked: HashMap::new(),
            mdns_enabled,
            credential_store,
            preferred_entertainment_config,
            nupnp_url: "https://discovery.meethue.com".to_owned(),
        }
    }

    /// Name the bridges the host tracks, so a scan that cannot read one
    /// renews it instead of replacing what the host verified.
    #[must_use]
    pub fn with_tracked(mut self, tracked: HashMap<IpAddr, DeviceFingerprint>) -> Self {
        self.tracked = tracked;
        self
    }

    /// Override the N-UPnP discovery endpoint.
    #[must_use]
    pub fn with_nupnp_url(mut self, nupnp_url: impl Into<String>) -> Self {
        self.nupnp_url = nupnp_url.into();
        self
    }

    /// Run discovery and return rich bridge details.
    ///
    /// # Errors
    ///
    /// Returns an error when mDNS cannot run and no known bridge gives the
    /// scan anything else to look at.
    pub async fn scan_bridges(&mut self) -> Result<ScanOutcome<HueDiscoveredBridge>> {
        let started = Instant::now();
        let known_hosts: Vec<IpAddr> = self.known_bridges.iter().map(|bridge| bridge.ip).collect();
        let mut candidates: HashMap<IpAddr, HueCandidate> = self
            .known_bridges
            .iter()
            .cloned()
            .map(|bridge| {
                (
                    bridge.ip,
                    HueCandidate {
                        bridge,
                        advertised: false,
                    },
                )
            })
            .collect();
        let mut gaps = Vec::new();

        if self.mdns_enabled {
            let browse = MdnsBrowser::scan(
                MdnsScope::Network,
                HUE_SERVICE_TYPE,
                self.scan_timeout,
                &known_hosts,
            )
            .await?;
            gaps = browse.gaps;
            for service in browse.found {
                let discovered = HueKnownBridge {
                    bridge_id: service
                        .txt
                        .get("bridgeid")
                        .cloned()
                        .unwrap_or_default()
                        .to_ascii_lowercase(),
                    ip: service.host,
                    api_port: service.port,
                    name: service.txt.get("name").cloned().unwrap_or(service.name),
                    model_id: service.txt.get("modelid").cloned().unwrap_or_default(),
                    sw_version: service.txt.get("swversion").cloned().unwrap_or_default(),
                };
                merge_advertised(&mut candidates, discovered);
            }
        }

        if candidates.is_empty() {
            match HueBridgeClient::discover_bridges_with_url(&self.nupnp_url).await {
                Ok(bridges) => {
                    for bridge in bridges {
                        let discovered = HueKnownBridge {
                            bridge_id: bridge.bridge_id,
                            ip: bridge.ip,
                            api_port: DEFAULT_HUE_API_PORT,
                            name: String::new(),
                            model_id: String::new(),
                            sw_version: String::new(),
                        };
                        merge_advertised(&mut candidates, discovered);
                    }
                }
                Err(error) => {
                    warn!(error = %error, "Hue N-UPnP discovery failed");
                    gaps.push(CoverageGap::SourceFailed {
                        source_name: "Hue N-UPnP discovery".to_owned(),
                        reason: format!("{error:#}"),
                    });
                }
            }
        }

        // Known bridges go first, so the deadline falls on unsolicited ones.
        let mut candidates: Vec<HueCandidate> = candidates.into_values().collect();
        candidates.sort_by_key(|candidate| !known_hosts.contains(&candidate.bridge.ip));
        let budget = ProbeBudget {
            deadline: started + self.scan_timeout + PROBE_ROUND,
            in_flight: DEFAULT_PROBE_CONCURRENCY,
        };
        let mut outcome = probe_candidates(candidates, budget, |candidate: HueCandidate| {
            let credential_store = Arc::clone(&self.credential_store);
            let preferred_entertainment_config = self.preferred_entertainment_config.clone();
            let tracked = self.tracked.get(&candidate.bridge.ip).cloned();
            async move {
                build_discovered_bridge(
                    candidate,
                    tracked,
                    credential_store,
                    preferred_entertainment_config,
                )
                .await
            }
        })
        .await;
        outcome.gaps.extend(gaps);

        outcome
            .found
            .sort_by(|left, right| left.info.name.cmp(&right.info.name));
        Ok(outcome)
    }
}

impl HueScanner {
    /// Discover configured and mDNS-visible Hue bridges.
    ///
    /// # Errors
    ///
    /// See [`HueScanner::scan_bridges`].
    pub async fn scan(&mut self) -> Result<DiscoveryBatch> {
        let outcome = self.scan_bridges().await?;
        Ok(outcome.into_batch(HueDiscoveredBridge::into_discovered))
    }
}

/// Fold an address something outside the scan vouched for into `candidates`.
fn merge_advertised(candidates: &mut HashMap<IpAddr, HueCandidate>, bridge: HueKnownBridge) {
    candidates
        .entry(bridge.ip)
        .and_modify(|existing| {
            existing.bridge.merge_from(&bridge);
            existing.advertised = true;
        })
        .or_insert(HueCandidate {
            bridge,
            advertised: true,
        });
}

/// Probe one bridge.
///
/// A bridge nothing advertised that does not answer has not been seen, so it
/// holds the device tracked at its address and leaves its siblings free to
/// retire. A bridge the host tracks that answers but cannot be read keeps
/// what the host verified: lights or entertainment areas that merely failed
/// to load must not look like a bridge that has none.
async fn build_discovered_bridge(
    candidate: HueCandidate,
    tracked: Option<DeviceFingerprint>,
    credential_store: Arc<CredentialStore>,
    preferred_entertainment_config: Option<String>,
) -> Result<Probed<HueDiscoveredBridge>> {
    let HueCandidate {
        bridge: candidate,
        advertised,
    } = candidate;
    let identity = match HueBridgeClient::with_port(candidate.ip, candidate.api_port)
        .bridge_identity()
        .await
    {
        Ok(identity) => Some(identity),
        Err(_) if advertised => None,
        Err(error) => return Ok(Probed::silent(tracked, format!("{error:#}"))),
    };
    let bridge_id = identity
        .as_ref()
        .map(|identity| identity.bridge_id.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| candidate.bridge_id.clone());
    let bridge_name = identity
        .as_ref()
        .map(|identity| identity.name.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| candidate.name.clone());
    let model_id = identity
        .as_ref()
        .map(|identity| identity.model_id.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| candidate.model_id.clone());
    let sw_version = identity
        .as_ref()
        .map(|identity| identity.sw_version.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| candidate.sw_version.clone());
    if bridge_id.is_empty() {
        let unnamed = anyhow!("Hue bridge at {} did not name itself", candidate.ip);
        return Probed::failed(tracked, unnamed);
    }
    // A live identity naming another bridge means this address changed hands,
    // so nothing here speaks for the bridge tracked at it.
    let tracked = tracked.filter(|fingerprint| {
        identity.is_none() || *fingerprint == fingerprint_for_bridge(&bridge_id)
    });

    let Some((api_key, _client_key)) =
        load_bridge_credentials(&credential_store, &bridge_id, candidate.ip).await
    else {
        let info = build_device_info(
            &bridge_id,
            &bridge_name,
            Some(model_id.as_str()),
            Some(sw_version.as_str()),
            None,
            &[],
        );
        return Ok(Probed::Described(build_discovered_payload(
            &candidate,
            &bridge_id,
            &bridge_name,
            &model_id,
            &sw_version,
            info,
            None,
            Vec::new(),
            DiscoveryConnectBehavior::Deferred,
        )));
    };

    let client =
        HueBridgeClient::authenticated_with_port(candidate.ip, candidate.api_port, api_key);
    let lights = client.lights().await;
    let configs = client.entertainment_configs().await;
    if (lights.is_err() || configs.is_err())
        && let Some(fingerprint) = tracked
    {
        warn!(
            ip = %candidate.ip,
            bridge_id = %bridge_id,
            "keeping tracked Hue bridge as it was; discovery could not read it"
        );
        return Ok(Probed::Present(fingerprint));
    }
    let lights = lights.unwrap_or_else(|error| {
        warn!(
            ip = %candidate.ip,
            bridge_id = %bridge_id,
            error = %error,
            "failed to fetch Hue lights during discovery"
        );
        Vec::new()
    });
    let entertainment_config = configs.ok().and_then(|configs| {
        choose_entertainment_config(
            preferred_entertainment_config.as_deref(),
            configs.as_slice(),
        )
    });
    let connect_behavior = if entertainment_config.is_some() {
        DiscoveryConnectBehavior::AutoConnect
    } else {
        DiscoveryConnectBehavior::Deferred
    };

    let info = build_device_info(
        &bridge_id,
        &bridge_name,
        Some(model_id.as_str()),
        Some(sw_version.as_str()),
        entertainment_config.as_ref(),
        lights.as_slice(),
    );

    Ok(Probed::Described(build_discovered_payload(
        &candidate,
        &bridge_id,
        &bridge_name,
        &model_id,
        &sw_version,
        info,
        entertainment_config,
        lights,
        connect_behavior,
    )))
}

async fn load_bridge_credentials(
    credential_store: &CredentialStore,
    bridge_id: &str,
    ip: IpAddr,
) -> Option<(String, String)> {
    for key in [bridge_id.to_owned(), format!("ip:{ip}")] {
        let Some(credentials) = credential_store.get_driver_json("hue", &key).await else {
            continue;
        };
        let Some(api_key) = credentials.get("api_key").and_then(Value::as_str) else {
            continue;
        };
        let Some(client_key) = credentials.get("client_key").and_then(Value::as_str) else {
            continue;
        };
        return Some((api_key.to_owned(), client_key.to_owned()));
    }
    None
}

#[expect(
    clippy::too_many_arguments,
    reason = "scanner payload assembly keeps bridge identity, transport, and config metadata explicit"
)]
fn build_discovered_payload(
    candidate: &HueKnownBridge,
    bridge_id: &str,
    bridge_name: &str,
    model_id: &str,
    sw_version: &str,
    info: hypercolor_types::device::DeviceInfo,
    entertainment_config: Option<HueEntertainmentConfig>,
    lights: Vec<HueLight>,
    connect_behavior: DiscoveryConnectBehavior,
) -> HueDiscoveredBridge {
    let mut metadata = HashMap::new();
    metadata.insert("ip".to_owned(), candidate.ip.to_string());
    metadata.insert("api_port".to_owned(), candidate.api_port.to_string());
    metadata.insert("bridge_id".to_owned(), bridge_id.to_owned());
    if !bridge_name.is_empty() {
        metadata.insert("bridge_name".to_owned(), bridge_name.to_owned());
    }
    if !model_id.is_empty() {
        metadata.insert("model_id".to_owned(), model_id.to_owned());
    }
    if !sw_version.is_empty() {
        metadata.insert("sw_version".to_owned(), sw_version.to_owned());
    }
    if let Some(config) = entertainment_config.as_ref() {
        metadata.insert("entertainment_config_id".to_owned(), config.id.clone());
        metadata.insert("entertainment_config_name".to_owned(), config.name.clone());
    }

    HueDiscoveredBridge {
        bridge_id: bridge_id.to_owned(),
        ip: candidate.ip,
        api_port: candidate.api_port,
        info,
        entertainment_config,
        lights,
        connect_behavior,
        metadata,
    }
}
