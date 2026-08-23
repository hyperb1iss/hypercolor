//! `hyper audio` -- audio input device listing.

use anyhow::Result;
use clap::{Args, Subcommand};

use hypercolor_types::api::system::AudioDevicesResponse;

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat};

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
    let response: AudioDevicesResponse = client.get("/system/audio-devices").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            for device in &response.devices {
                println!("{}", device.name);
            }
        }
        OutputFormat::Table => {
            let rows: Vec<Vec<String>> = response
                .devices
                .iter()
                .map(|device| {
                    let active = device.id == response.current;
                    let marker = if active {
                        ctx.painter.success("\u{2726}")
                    } else {
                        " ".to_string()
                    };
                    let name_display = if active {
                        ctx.painter.keyword(&device.name)
                    } else {
                        ctx.painter.name(&device.name)
                    };
                    vec![
                        marker,
                        name_display,
                        ctx.painter.id(&device.id),
                        ctx.painter.muted(&device.description),
                    ]
                })
                .collect();
            ctx.print_table(&["", "Device", "ID", "Description"], &rows);
        }
    }

    Ok(())
}
