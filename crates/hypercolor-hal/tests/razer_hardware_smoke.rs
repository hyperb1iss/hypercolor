#![cfg(target_os = "linux")]

use std::error::Error;
use std::time::Duration;

use hypercolor_hal::database::{ProtocolDatabase, TransportType};
use hypercolor_hal::drivers::razer::{PID_BLADE_15_LATE_2021_ADVANCED, RAZER_VENDOR_ID};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ResponseStatus};
use hypercolor_hal::transport::Transport;
use hypercolor_hal::transport::control::UsbControlTransport;

const HARDWARE_TEST_ENV: &str = "HYPERCOLOR_TEST_RAZER_HARDWARE";
const RESPONSE_TIMEOUT: Duration = Duration::from_millis(1_000);
const IDENTIFY_FLASH_MS: Duration = Duration::from_millis(150);

fn hardware_tests_enabled() -> bool {
    std::env::var(HARDWARE_TEST_ENV)
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes")
        })
        .unwrap_or(false)
}

async fn run_commands(
    protocol: &dyn Protocol,
    transport: &dyn Transport,
    commands: Vec<ProtocolCommand>,
) -> Result<(), Box<dyn Error>> {
    for command in commands {
        if command.expects_response {
            let response = transport
                .send_receive(&command.data, RESPONSE_TIMEOUT)
                .await?;
            let parsed = protocol.parse_response(&response)?;
            assert_eq!(
                parsed.status,
                ResponseStatus::Ok,
                "device returned non-OK response for command {:02X?}",
                command.data
            );
        } else {
            transport.send(&command.data).await?;
        }

        if !command.post_delay.is_zero() {
            tokio::time::sleep(command.post_delay).await;
        }
    }

    Ok(())
}

#[tokio::test]
#[ignore = "requires a connected Blade 15 (Late 2021 Advanced) and HYPERCOLOR_TEST_RAZER_HARDWARE=1"]
async fn blade_identify_sequence_writes_without_protocol_errors() -> Result<(), Box<dyn Error>> {
    if !hardware_tests_enabled() {
        eprintln!("skipping hardware smoke test; set {HARDWARE_TEST_ENV}=1 to enable it");
        return Ok(());
    }

    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BLADE_15_LATE_2021_ADVANCED)
        .expect("Blade 15 descriptor should exist");
    let (interface, report_id) = match descriptor.transport {
        TransportType::UsbControl {
            interface,
            report_id,
        } => (interface, report_id),
        other => panic!("expected control transport for Blade 15, got {other:?}"),
    };

    let usb = nusb::list_devices()
        .await?
        .find(|device| {
            device.vendor_id() == RAZER_VENDOR_ID
                && device.product_id() == PID_BLADE_15_LATE_2021_ADVANCED
        })
        .expect("Blade 15 USB device should be present");
    let transport = UsbControlTransport::new(usb.open().await?, interface, report_id).await?;
    let protocol = (descriptor.protocol.build)();
    let total_leds = usize::try_from(protocol.total_leds()).expect("LED count should fit usize");
    let white = vec![[255, 255, 255]; total_leds];
    let clear = vec![[0, 0, 0]; total_leds];

    let result = async {
        run_commands(protocol.as_ref(), &transport, protocol.init_sequence()).await?;
        run_commands(protocol.as_ref(), &transport, protocol.encode_frame(&white)).await?;
        tokio::time::sleep(IDENTIFY_FLASH_MS).await;
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    let _ = run_commands(protocol.as_ref(), &transport, protocol.encode_frame(&clear)).await;
    let _ = run_commands(protocol.as_ref(), &transport, protocol.shutdown_sequence()).await;
    let _ = transport.close().await;

    result
}
