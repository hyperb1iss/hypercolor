use std::env;
use std::fmt::Write as _;
use std::fs;
use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hypercolor_openrgb_sdk::{OpenRgbClient, OpenRgbClientConfig, parse_controller_data};

const DEFAULT_ENDPOINT: &str = "127.0.0.1:6742";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let endpoint = args
        .next()
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned())
        .parse::<SocketAddr>()?;
    let output_path = args.next();
    if args.next().is_some() {
        return Err("usage: cargo run -p hypercolor-openrgb-sdk --example capture_corpus -- [addr] [output.md]".into());
    }

    let mut client = OpenRgbClient::connect(
        endpoint,
        OpenRgbClientConfig {
            client_name: "Hypercolor Corpus Capture".to_owned(),
            connect_timeout: Duration::from_secs(3),
            read_timeout: Duration::from_secs(3),
            write_timeout: Duration::from_secs(3),
            ..OpenRgbClientConfig::default()
        },
    )
    .await?;
    let protocol_version = client.protocol_version();
    let controller_count = client.controller_count().await?;

    let mut report = String::new();
    writeln!(report, "# OpenRGB SDK Packet Corpus Capture")?;
    writeln!(report)?;
    writeln!(report, "- endpoint: `{endpoint}`")?;
    writeln!(report, "- captured_unix_seconds: `{}`", unix_seconds())?;
    writeln!(
        report,
        "- negotiated_protocol_version: `{protocol_version}`"
    )?;
    writeln!(report, "- controller_count: `{controller_count}`")?;
    writeln!(
        report,
        "- warning: `payload_hex_unredacted` contains raw controller data, including serial/location strings; scrub before committing fixtures"
    )?;
    writeln!(report)?;

    for controller_index in 0..controller_count {
        let payload = client.controller_data_payload(controller_index).await?;
        writeln!(report, "## Controller {controller_index}")?;
        writeln!(report)?;
        writeln!(report, "- payload_len: `{}`", payload.len())?;
        writeln!(report, "- payload_hex_unredacted: `{}`", hex(&payload))?;
        match parse_controller_data(&payload, protocol_version) {
            Ok(controller) => {
                writeln!(report, "- name: `{}`", controller.name)?;
                writeln!(report, "- vendor: `{}`", controller.vendor)?;
                writeln!(report, "- description: `{}`", controller.description)?;
                writeln!(
                    report,
                    "- serial_present: `{}`",
                    !controller.serial.is_empty()
                )?;
                writeln!(
                    report,
                    "- location_present: `{}`",
                    !controller.location.is_empty()
                )?;
                writeln!(report, "- modes: `{}`", controller.modes.len())?;
                writeln!(report, "- zones: `{}`", controller.zones.len())?;
                writeln!(report, "- leds: `{}`", controller.leds.len())?;
                writeln!(report, "- colors: `{}`", controller.colors.len())?;
            }
            Err(error) => {
                writeln!(report, "- parse_error: `{error}`")?;
            }
        }
        writeln!(report)?;
    }

    if let Some(output_path) = output_path {
        fs::write(output_path, report)?;
    } else {
        print!("{report}");
    }

    Ok(())
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(output, "{byte:02x}").expect("writing to string should not fail");
    }
    output
}
