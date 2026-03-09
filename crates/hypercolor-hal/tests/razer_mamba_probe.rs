use std::error::Error;
use std::time::Duration;

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::razer::{RAZER_REPORT_LEN, RAZER_VENDOR_ID, razer_crc};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ResponseStatus};
use hypercolor_hal::registry::{HidRawReportMode, TransportType};
use hypercolor_hal::transport::Transport;
use hypercolor_hal::transport::hidapi::UsbHidApiTransport;

const HARDWARE_TEST_ENV: &str = "HYPERCOLOR_TEST_RAZER_HARDWARE";
const PID_MAMBA_ELITE: u16 = 0x006C;
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

fn usb_path(usb: &nusb::DeviceInfo) -> String {
    let ports = usb
        .port_chain()
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(".");

    if ports.is_empty() {
        usb.bus_id().to_owned()
    } else {
        format!("{}-{ports}", usb.bus_id())
    }
}

fn raw_razer_packet(
    transaction_id: u8,
    data_size: u8,
    command_class: u8,
    command_id: u8,
    args: &[u8],
) -> Vec<u8> {
    let mut packet = [0_u8; RAZER_REPORT_LEN];
    packet[1] = transaction_id;
    packet[5] = data_size;
    packet[6] = command_class;
    packet[7] = command_id;
    packet[8..8 + args.len()].copy_from_slice(args);
    packet[88] = razer_crc(&packet);
    packet.to_vec()
}

async fn run_commands(
    protocol: &dyn Protocol,
    transport: &dyn Transport,
    commands: Vec<ProtocolCommand>,
) -> Result<(), Box<dyn Error>> {
    for command in commands {
        if command.expects_response {
            let response = if command.response_delay.is_zero() {
                transport
                    .send_receive(&command.data, RESPONSE_TIMEOUT)
                    .await?
            } else {
                transport.send(&command.data).await?;
                tokio::time::sleep(command.response_delay).await;
                transport.receive(RESPONSE_TIMEOUT).await?
            };
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

async fn probe_transaction_id(
    transport: &dyn Transport,
    transaction_id: u8,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let serial_query = raw_razer_packet(transaction_id, 0x02, 0x00, 0x82, &[]);
    Ok(transport
        .send_receive(&serial_query, RESPONSE_TIMEOUT)
        .await?)
}

async fn probe_extended_effect_query(
    transport: &dyn Transport,
    transaction_id: u8,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let query = raw_razer_packet(transaction_id, 0x06, 0x0F, 0x82, &[0x00]);
    Ok(transport.send_receive(&query, RESPONSE_TIMEOUT).await?)
}

#[tokio::test]
#[ignore = "manual Mamba Elite transaction probe on connected hardware"]
async fn mamba_elite_transaction_probe() -> Result<(), Box<dyn Error>> {
    if !hardware_tests_enabled() {
        eprintln!("skipping hardware probe; set {HARDWARE_TEST_ENV}=1 to enable it");
        return Ok(());
    }

    let usb = nusb::list_devices()
        .await?
        .find(|device| {
            device.vendor_id() == RAZER_VENDOR_ID && device.product_id() == PID_MAMBA_ELITE
        })
        .expect("Razer Mamba Elite USB device should be present");
    let device_usb_path = usb_path(&usb);
    let transport = UsbHidApiTransport::open(
        RAZER_VENDOR_ID,
        PID_MAMBA_ELITE,
        0,
        0x00,
        HidRawReportMode::FeatureReport,
        usb.serial_number(),
        Some(&device_usb_path),
        Some(0x0001),
        Some(0x0002),
    )?;

    for transaction_id in [0x1F_u8, 0x3F_u8] {
        match probe_transaction_id(&transport, transaction_id).await {
            Ok(response) => {
                let preview_len = response.len().min(24);
                eprintln!(
                    "mamba transaction 0x{transaction_id:02X} response: {:02X?}",
                    &response[..preview_len]
                );
            }
            Err(error) => {
                eprintln!("mamba transaction 0x{transaction_id:02X} failed: {error}");
            }
        }

        match probe_extended_effect_query(&transport, transaction_id).await {
            Ok(response) => {
                let preview_len = response.len().min(24);
                eprintln!(
                    "mamba extended query 0x{transaction_id:02X} response: {:02X?}",
                    &response[..preview_len]
                );
            }
            Err(error) => {
                eprintln!("mamba extended query 0x{transaction_id:02X} failed: {error}");
            }
        }
    }

    transport.close().await?;

    Ok(())
}

#[tokio::test]
#[ignore = "manual Mamba Elite protocol smoke test on connected hardware"]
async fn mamba_elite_protocol_smoke() -> Result<(), Box<dyn Error>> {
    if !hardware_tests_enabled() {
        eprintln!("skipping hardware probe; set {HARDWARE_TEST_ENV}=1 to enable it");
        return Ok(());
    }

    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_MAMBA_ELITE)
        .expect("Razer Mamba Elite descriptor should exist");
    let (interface, report_id, report_mode, usage_page, usage) = match descriptor.transport {
        TransportType::UsbHidApi {
            interface,
            report_id,
            report_mode,
            usage_page,
            usage,
        } => (interface, report_id, report_mode, usage_page, usage),
        other => panic!("expected hidapi transport for Mamba Elite, got {other:?}"),
    };

    let usb = nusb::list_devices()
        .await?
        .find(|device| {
            device.vendor_id() == RAZER_VENDOR_ID && device.product_id() == PID_MAMBA_ELITE
        })
        .expect("Razer Mamba Elite USB device should be present");
    let device_usb_path = usb_path(&usb);
    let transport = UsbHidApiTransport::open(
        RAZER_VENDOR_ID,
        PID_MAMBA_ELITE,
        interface,
        report_id,
        report_mode,
        usb.serial_number(),
        Some(&device_usb_path),
        usage_page,
        usage,
    )?;

    let protocol = (descriptor.protocol.build)();
    let total_leds = usize::try_from(protocol.total_leds()).expect("LED count should fit usize");
    let white = vec![[0x40, 0x00, 0x40]; total_leds];
    let clear = vec![[0x00, 0x00, 0x00]; total_leds];

    let result = async {
        run_commands(protocol.as_ref(), &transport, protocol.init_sequence()).await?;
        run_commands(protocol.as_ref(), &transport, protocol.encode_frame(&white)).await?;
        tokio::time::sleep(IDENTIFY_FLASH_MS).await;
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    let _ = run_commands(protocol.as_ref(), &transport, protocol.encode_frame(&clear)).await;
    let _ = transport.close().await;

    result
}
