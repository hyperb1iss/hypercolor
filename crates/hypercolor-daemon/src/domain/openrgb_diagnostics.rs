//! The `openrgb` diagnose check: is the bridge reachable, what protocol
//! did it negotiate, how many controllers does it expose, and which of its
//! routes does the daemon hold output-disabled.
//!
//! The probe is a plain TCP connect plus the SDK handshake, so it stays
//! cfg-free; the SDK crate handles every platform the same way.

use std::net::SocketAddr;
use std::time::Duration;

use hypercolor_types::api::diagnose::DiagnoseCheck;
use hypercolor_types::config::DriverConfigEntry;

use crate::discovery::{DiscoveryRuntime, is_openrgb_bridge_device};

/// The OpenRGB bridge driver's module id.
pub const OPENRGB_DRIVER_ID: &str = crate::discovery::OPENRGB_DRIVER_ID;

const DEFAULT_ENDPOINT: &str = "127.0.0.1:6742";
const DEFAULT_TIMEOUT_MS: u64 = 750;
const MAX_TIMEOUT_MS: u64 = 10_000;

/// Which endpoints to probe and how long to wait on each.
///
/// Mirrors the bridge driver's own defaults so the check talks to the same
/// server the driver would.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRgbProbeConfig {
    pub endpoints: Vec<SocketAddr>,
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
}

impl Default for OpenRgbProbeConfig {
    fn default() -> Self {
        Self {
            endpoints: default_endpoints(),
            connect_timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            read_timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            write_timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
        }
    }
}

/// The subset of `drivers.openrgb` settings the probe reads, deserialized
/// through the same serde path the driver uses rather than picked out of
/// raw JSON.
#[derive(Debug, Default, serde::Deserialize)]
struct ProbeSettings {
    #[serde(default)]
    endpoints: Vec<String>,
    #[serde(default)]
    connect_timeout_ms: Option<u64>,
    #[serde(default)]
    read_timeout_ms: Option<u64>,
    #[serde(default)]
    write_timeout_ms: Option<u64>,
}

impl ProbeSettings {
    fn from_driver_entry(entry: &DriverConfigEntry) -> Self {
        serde_json::to_value(entry)
            .ok()
            .and_then(|settings| serde_json::from_value(settings).ok())
            .unwrap_or_default()
    }
}

impl OpenRgbProbeConfig {
    /// Read endpoints and timeouts from the raw `drivers.openrgb` entry.
    ///
    /// Unparseable endpoints are skipped; an entry with none falls back to
    /// the loopback default, the same way the driver does.
    #[must_use]
    pub fn from_driver_entry(entry: &DriverConfigEntry) -> Self {
        let settings = ProbeSettings::from_driver_entry(entry);
        let mut config = Self::default();
        let parsed: Vec<SocketAddr> = settings
            .endpoints
            .iter()
            .filter_map(|raw| raw.trim().parse().ok())
            .collect();
        if !parsed.is_empty() {
            config.endpoints = parsed;
        }
        config.connect_timeout = timeout_setting(settings.connect_timeout_ms);
        config.read_timeout = timeout_setting(settings.read_timeout_ms);
        config.write_timeout = timeout_setting(settings.write_timeout_ms);
        config
    }
}

fn default_endpoints() -> Vec<SocketAddr> {
    DEFAULT_ENDPOINT
        .parse()
        .map(|endpoint| vec![endpoint])
        .unwrap_or_default()
}

fn timeout_setting(millis: Option<u64>) -> Duration {
    Duration::from_millis(
        millis
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .clamp(1, MAX_TIMEOUT_MS),
    )
}

/// How one endpoint answered the probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenRgbProbeOutcome {
    /// The handshake completed.
    Reachable {
        protocol_version: u32,
        controller_count: u32,
    },
    /// The connection or handshake failed.
    Unreachable { error: String },
}

/// One probed endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRgbEndpointProbe {
    pub endpoint: SocketAddr,
    pub outcome: OpenRgbProbeOutcome,
}

/// A bridge route the daemon or the bridge reports as output-disabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDisabledRoute {
    pub name: String,
    pub reason: String,
}

/// Everything the check renders from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenRgbCheckState {
    /// The bridge driver is not compiled into this daemon.
    Unavailable,
    /// The driver is compiled in but disabled by config.
    Disabled,
    /// The driver is enabled; these are the probe results.
    Probed {
        probes: Vec<OpenRgbEndpointProbe>,
        disabled_routes: Vec<OutputDisabledRoute>,
    },
}

/// Connect to every configured endpoint and run the SDK handshake.
pub async fn probe_openrgb_endpoints(config: &OpenRgbProbeConfig) -> Vec<OpenRgbEndpointProbe> {
    let mut probes = Vec::with_capacity(config.endpoints.len());
    for endpoint in &config.endpoints {
        probes.push(OpenRgbEndpointProbe {
            endpoint: *endpoint,
            outcome: probe_endpoint(*endpoint, config).await,
        });
    }
    probes
}

#[cfg(feature = "builtin-drivers")]
async fn probe_endpoint(endpoint: SocketAddr, config: &OpenRgbProbeConfig) -> OpenRgbProbeOutcome {
    use hypercolor_openrgb_sdk::{OpenRgbClient, OpenRgbClientConfig};

    let client_config = OpenRgbClientConfig {
        client_name: "Hypercolor Diagnose".to_owned(),
        connect_timeout: config.connect_timeout,
        read_timeout: config.read_timeout,
        write_timeout: config.write_timeout,
        ..OpenRgbClientConfig::default()
    };
    let mut client = match OpenRgbClient::connect(endpoint, client_config).await {
        Ok(client) => client,
        Err(error) => {
            return OpenRgbProbeOutcome::Unreachable {
                error: error.to_string(),
            };
        }
    };
    let protocol_version = client.protocol_version();
    match client.controller_count().await {
        Ok(controller_count) => OpenRgbProbeOutcome::Reachable {
            protocol_version,
            controller_count,
        },
        Err(error) => OpenRgbProbeOutcome::Unreachable {
            error: format!("handshake succeeded but controller count failed: {error}"),
        },
    }
}

#[cfg(not(feature = "builtin-drivers"))]
async fn probe_endpoint(
    _endpoint: SocketAddr,
    _config: &OpenRgbProbeConfig,
) -> OpenRgbProbeOutcome {
    OpenRgbProbeOutcome::Unreachable {
        error: "the OpenRGB SDK is not compiled into this daemon".to_owned(),
    }
}

/// Bridge routes the daemon will not write through, with their reasons.
pub async fn output_disabled_routes(runtime: &DiscoveryRuntime) -> Vec<OutputDisabledRoute> {
    let locks = runtime.bridge_output_locks.snapshot();
    let mut routes = Vec::new();
    for tracked in runtime.device_registry.list().await {
        let metadata = runtime
            .device_registry
            .metadata_for_id(&tracked.info.id)
            .await
            .unwrap_or_default();
        if !is_openrgb_bridge_device(&tracked.info, &metadata) {
            continue;
        }
        let reason = locks
            .get(&tracked.info.id)
            .map(|lock| lock.reason.clone())
            .or_else(|| {
                let advertised_disabled = metadata
                    .get("output_enabled")
                    .is_some_and(|flag| flag.trim().eq_ignore_ascii_case("false"));
                advertised_disabled.then(|| {
                    metadata
                        .get("disabled_reason")
                        .map(|reason| reason.trim().to_owned())
                        .filter(|reason| !reason.is_empty())
                        .unwrap_or_else(|| {
                            "the bridge reports the controller as output-disabled".to_owned()
                        })
                })
            });
        if let Some(reason) = reason {
            routes.push(OutputDisabledRoute {
                name: tracked.info.name.clone(),
                reason,
            });
        }
    }
    routes.sort_by(|left, right| left.name.cmp(&right.name));
    routes
}

/// Render the check. Pass when the bridge is disabled or every endpoint
/// answered, warning when the driver is enabled but an endpoint did not.
#[must_use]
pub fn openrgb_check(state: &OpenRgbCheckState) -> DiagnoseCheck {
    let (status, detail) = match state {
        OpenRgbCheckState::Unavailable => (
            "pass",
            "bridge driver is not compiled into this daemon".to_owned(),
        ),
        OpenRgbCheckState::Disabled => ("pass", "bridge disabled".to_owned()),
        OpenRgbCheckState::Probed {
            probes,
            disabled_routes,
        } => {
            let unreachable = probes
                .iter()
                .filter(|probe| matches!(probe.outcome, OpenRgbProbeOutcome::Unreachable { .. }))
                .count();
            let status = if probes.is_empty() || unreachable > 0 {
                "warning"
            } else {
                "pass"
            };
            let mut parts: Vec<String> = probes
                .iter()
                .map(|probe| match &probe.outcome {
                    OpenRgbProbeOutcome::Reachable {
                        protocol_version,
                        controller_count,
                    } => format!(
                        "{}: reachable, protocol v{protocol_version}, {controller_count} controller(s)",
                        probe.endpoint
                    ),
                    OpenRgbProbeOutcome::Unreachable { error } => {
                        format!("{}: unreachable ({error})", probe.endpoint)
                    }
                })
                .collect();
            if probes.is_empty() {
                parts.push("no endpoints configured".to_owned());
            }
            if disabled_routes.is_empty() {
                parts.push("output-disabled routes: 0".to_owned());
            } else {
                let listed = disabled_routes
                    .iter()
                    .map(|route| format!("{}: {}", route.name, route.reason))
                    .collect::<Vec<_>>()
                    .join("; ");
                parts.push(format!(
                    "output-disabled routes: {} ({listed})",
                    disabled_routes.len()
                ));
            }
            (status, parts.join("; "))
        }
    };

    DiagnoseCheck {
        category: "drivers".to_owned(),
        name: "openrgb".to_owned(),
        status: status.to_owned(),
        detail,
    }
}
