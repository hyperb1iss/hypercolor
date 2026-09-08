//! Shared guided setup reads for REST and MCP.

use hypercolor_openrgb_host as host;
use hypercolor_types::api::system::{
    OpenRgbEndpointStatus, OpenRgbInstallHint, OpenRgbPermissionStatus, OpenRgbStatus,
};

use super::context::DeviceContext;
use super::openrgb_diagnostics::{OpenRgbProbeConfig, OpenRgbProbeOutcome};

/// Inspect the daemon host and configured SDK endpoints without changing ownership.
pub async fn openrgb_status(devices: &DeviceContext) -> OpenRgbStatus {
    let config = devices.config_snapshot().unwrap_or_default();
    let registry = devices.driver_registry();
    let driver = registry.get(super::openrgb_diagnostics::OPENRGB_DRIVER_ID);
    let compiled = driver.is_some();
    let enabled = driver
        .is_some_and(|driver| crate::network::module_enabled(&config, &driver.module_descriptor()));
    let bridge_config =
        crate::network::driver_config_entry(&config, super::openrgb_diagnostics::OPENRGB_DRIVER_ID);
    let probe_config = OpenRgbProbeConfig::from_driver_entry(&bridge_config);
    let (binary, probes) = tokio::join!(
        host::detect_binary(),
        super::openrgb_diagnostics::probe_openrgb_endpoints(&probe_config),
    );
    let probes = probes
        .into_iter()
        .map(|probe| {
            let (reachable, protocol_version, controller_count, error) = match probe.outcome {
                OpenRgbProbeOutcome::Reachable {
                    protocol_version,
                    controller_count,
                } => (true, Some(protocol_version), Some(controller_count), None),
                OpenRgbProbeOutcome::Unreachable { error } => (false, None, None, Some(error)),
            };
            OpenRgbEndpointStatus {
                endpoint: probe.endpoint.to_string(),
                reachable,
                protocol_version,
                controller_count,
                error,
            }
        })
        .collect();
    let runtime = devices.discovery_runtime();
    let coverage = crate::discovery::collect_device_coverage(&runtime)
        .await
        .into_iter()
        .map(crate::discovery::JoinedCoverageRow::into_api_row)
        .collect();
    let output_disabled_count = super::openrgb_diagnostics::output_disabled_routes(&runtime)
        .await
        .len();
    OpenRgbStatus {
        compiled,
        enabled,
        platform: std::env::consts::OS.to_owned(),
        binary_path: binary
            .as_ref()
            .map(|binary| binary.path.to_string_lossy().into_owned()),
        binary_version: binary.and_then(|binary| binary.version),
        bridge_config,
        probes,
        install_hints: host::install_hints()
            .into_iter()
            .map(|hint| OpenRgbInstallHint {
                command: hint.command,
                note: hint.note,
            })
            .collect(),
        permission_checks: host::permission_checks()
            .into_iter()
            .map(|check| OpenRgbPermissionStatus {
                id: check.id,
                ok: check.ok,
                detail: check.detail,
                remedy: check.remedy,
            })
            .collect(),
        coverage,
        output_disabled_count,
    }
}
