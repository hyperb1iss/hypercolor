//! Guided native-first OpenRGB setup.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use hypercolor_openrgb_host as host;
use hypercolor_types::api::config::ConfigMutationResponse;
use hypercolor_types::api::devices::{DeviceListResponse, DeviceSummary};
use hypercolor_types::api::drivers::DriverListResponse;
use hypercolor_types::api::system::OpenRgbStatus;

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, urlencoded};

/// OpenRGB fallback setup commands.
#[derive(Debug, Args)]
pub struct OpenRgbArgs {
    #[command(subcommand)]
    pub command: OpenRgbCommand,
}

/// Inspect and configure the optional bridge.
#[derive(Debug, Subcommand)]
pub enum OpenRgbCommand {
    /// Inspect installation, SDK endpoints, permissions, and coverage.
    Status,
    /// Show install commands and platform prerequisites.
    Hints,
    /// Write the local managed detector partition from daemon coverage.
    Partition,
    /// Start or adopt a local SDK server through its managed owner.
    Start,
    /// Stop servers started by Hypercolor on this daemon's host.
    Stop,
    /// Persist an explicitly chosen zone LED count and apply it live.
    Resize {
        /// Bridge device ID.
        device: String,
        /// Exact OpenRGB zone name.
        zone: String,
        /// Requested LED count (clamped to the controller's advertised range).
        size: u32,
    },
}

/// Execute a guided setup command.
///
/// # Errors
/// Returns errors from host inspection, configuration validation, or the daemon.
pub async fn execute(args: &OpenRgbArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        OpenRgbCommand::Start | OpenRgbCommand::Stop => {
            let stopping = matches!(args.command, OpenRgbCommand::Stop);
            let reply = if stopping {
                super::openrgb_lifecycle::execute_stop(client).await?
            } else {
                super::openrgb_lifecycle::execute_start(client).await?
            };
            if ctx.format == OutputFormat::Json {
                ctx.print_json(&reply.status)?;
            } else {
                println!("{}", lifecycle_status_message(&reply.status, stopping));
            }
        }
        OpenRgbCommand::Hints => {
            let hints = match client.get::<OpenRgbStatus>("/system/openrgb").await {
                Ok(status) => status.install_hints,
                Err(error) if client.is_loopback() => {
                    ctx.warning(&format!(
                        "Daemon guidance unavailable ({error}); showing local host hints"
                    ));
                    host::install_hints()
                        .into_iter()
                        .map(|hint| hypercolor_types::api::system::OpenRgbInstallHint {
                            command: hint.command,
                            note: hint.note,
                        })
                        .collect()
                }
                Err(error) => return Err(error),
            };
            if ctx.format == OutputFormat::Json {
                ctx.print_json(&hints)?;
            } else {
                for hint in hints {
                    println!("{}\n{}", hint.command, hint.note);
                }
            }
        }
        OpenRgbCommand::Status => {
            let status: OpenRgbStatus = client.get("/system/openrgb").await?;
            render_status(&status, ctx)?;
        }
        OpenRgbCommand::Partition => {
            let data_dir = client.local_data_dir().await?;
            let drivers: DriverListResponse = client.get_list("/drivers").await?;
            let devices: DeviceListResponse = client.get_list("/devices").await?;
            let plan = host::partition_driver_ids(
                &drivers
                    .items
                    .iter()
                    .map(host::DriverFacts::from)
                    .collect::<Vec<_>>(),
                &devices
                    .items
                    .iter()
                    .map(host::DeviceFacts::from)
                    .collect::<Vec<_>>(),
                &host::known_detector_driver_ids(),
            );
            let disabled = host::detector_prefixes_for_drivers(&plan.disabled_driver_ids);
            let released = host::detector_prefixes_for_drivers(&plan.re_enable_driver_ids);
            let dir = host::managed_config_dir(&data_dir);
            let report = host::write_detector_partition(&dir, &disabled, &released, None)?;
            if ctx.format == OutputFormat::Json {
                ctx.print_json(&report)?;
            } else {
                ctx.success(&format!(
                    "Wrote {} ({} detectors disabled)",
                    dir.config_path().display(),
                    report.disabled.len()
                ));
            }
        }
        OpenRgbCommand::Resize { device, zone, size } => {
            let info: DeviceSummary = client
                .get(&format!("/devices/{}", urlencoded(device)))
                .await?;
            let fingerprint = info
                .bridge
                .as_ref()
                .and_then(|bridge| bridge.fingerprint.as_deref())
                .context("Device has no OpenRGB bridge fingerprint; rescan before resizing")?;
            if !info.segments.iter().any(|segment| segment.name == *zone) {
                bail!(
                    "Device has no zone named {zone:?}; use the exact zone name from devices info"
                );
            }
            let sizes = resize_patch(fingerprint, zone, *size)?;
            let response: ConfigMutationResponse = client
                .patch("/config/keys/drivers.openrgb.zone_sizes?live=true", &sizes)
                .await?;
            if ctx.format == OutputFormat::Json {
                ctx.print_json(&response)?;
            } else if response.live {
                ctx.success("Saved zone size and applied the bridge configuration");
            } else if !response.requires_restart {
                ctx.success(
                    "Saved zone size; the bridge applies changed settings during reconciliation",
                );
            } else {
                bail!(
                    "Zone size was saved, but live apply did not run; restart the daemon to apply it"
                );
            }
        }
    }
    Ok(())
}

/// Describe a lifecycle result without claiming readiness during startup.
#[must_use]
pub fn lifecycle_status_message(status: &serde_json::Value, stopping: bool) -> String {
    if stopping {
        return if status.get("stopped").and_then(serde_json::Value::as_bool) == Some(true) {
            "Stopped the Hypercolor-managed OpenRGB server".to_owned()
        } else {
            "No Hypercolor-managed OpenRGB server was running".to_owned()
        };
    }
    if let Some(message) = status
        .get("last_error")
        .and_then(serde_json::Value::as_str)
        .or_else(|| status.get("message").and_then(serde_json::Value::as_str))
    {
        return message.to_owned();
    }
    if status
        .get("probe")
        .and_then(|probe| probe.get("reachable"))
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        if status.get("adopted").and_then(serde_json::Value::as_bool) == Some(true) {
            "Using the existing OpenRGB SDK server".to_owned()
        } else {
            "The managed OpenRGB SDK server is ready".to_owned()
        }
    } else {
        "OpenRGB startup is in progress".to_owned()
    }
}

/// Build a merge patch containing only the explicitly selected zone.
///
/// # Errors
/// Rejects missing controller or zone identifiers.
pub fn resize_patch(
    fingerprint: &str,
    zone: &str,
    size: u32,
) -> Result<BTreeMap<String, BTreeMap<String, u32>>> {
    if fingerprint.is_empty() || zone.is_empty() {
        bail!("Controller fingerprint and zone name must be nonempty");
    }
    Ok(BTreeMap::from([(
        fingerprint.to_owned(),
        BTreeMap::from([(zone.to_owned(), size)]),
    )]))
}

fn render_status(status: &OpenRgbStatus, ctx: &OutputContext) -> Result<()> {
    if ctx.format == OutputFormat::Json {
        return ctx.print_json(status);
    }
    println!(
        "OpenRGB: {}",
        status.binary_path.as_deref().unwrap_or("not installed")
    );
    println!(
        "Bridge: {}",
        if status.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    if let Some(version) = &status.binary_version {
        println!("Version: {version}");
    }
    println!("Host platform: {}", status.platform);
    println!(
        "Ownership mode: {}",
        status
            .bridge_config
            .settings
            .get("ownership")
            .and_then(|ownership| ownership.get("mode"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("disabled")
    );
    let rows: Vec<_> = status
        .probes
        .iter()
        .map(|probe| {
            vec![
                probe.endpoint.clone(),
                if probe.reachable {
                    "reachable".to_owned()
                } else {
                    probe.error.clone().unwrap_or_else(|| "unreachable".into())
                },
                probe
                    .protocol_version
                    .map_or_else(|| "-".into(), |version| version.to_string()),
                probe
                    .controller_count
                    .map_or_else(|| "-".into(), |count| count.to_string()),
            ]
        })
        .collect();
    ctx.print_table(&["Endpoint", "Status", "Protocol", "Controllers"], &rows);
    println!("Output-disabled routes: {}", status.output_disabled_count);
    for check in status.permission_checks.iter().filter(|check| !check.ok) {
        println!("{}: {}", check.id, check.detail);
        if let Some(remedy) = &check.remedy {
            println!("Remedy: {remedy}");
        }
    }
    for row in status.coverage.iter().filter(|row| row.bridge.is_some()) {
        println!("{}: {:?}", row.identity.label, row.active);
    }
    Ok(())
}
