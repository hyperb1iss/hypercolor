//! `hyper audio` -- audio input device listing.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, extract_str};

/// Audio input management.
#[derive(Debug, Args)]
pub struct AudioArgs {
    #[command(subcommand)]
    pub command: AudioCommand,
}

/// Audio subcommands.
#[derive(Debug, Subcommand)]
pub enum AudioCommand {
    /// List available audio input devices.
    Devices,
}

pub async fn execute(args: &AudioArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        AudioCommand::Devices => execute_devices(client, ctx).await,
    }
}

async fn execute_devices(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.get("/audio/devices").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            if let Some(devices) = response.get("items").and_then(serde_json::Value::as_array) {
                for device in devices {
                    if let Some(name) = device.get("name").and_then(serde_json::Value::as_str) {
                        println!("{name}");
                    }
                }
            }
        }
        OutputFormat::Table => {
            if let Some(devices) = response.get("items").and_then(serde_json::Value::as_array) {
                let rows: Vec<Vec<String>> = devices
                    .iter()
                    .map(|d| {
                        let active = d
                            .get("active")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false);
                        let marker = if active {
                            ctx.painter.success("*")
                        } else {
                            " ".to_string()
                        };
                        vec![
                            marker,
                            extract_str(d, "name"),
                            extract_str(d, "sample_rate"),
                            extract_str(d, "channels"),
                        ]
                    })
                    .collect();
                ctx.print_table(&["", "Device", "Rate", "Ch"], &rows);
            }
        }
    }

    Ok(())
}
