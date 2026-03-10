use std::error::Error;
use std::time::Duration;

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::razer::{RAZER_VENDOR_ID, RazerReport, razer_crc};
use zerocopy::{FromZeros, IntoBytes};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ResponseStatus};
use hypercolor_hal::registry::{HidRawReportMode, TransportType};
use hypercolor_hal::transport::Transport;
#[cfg(not(target_os = "linux"))]
use hypercolor_hal::transport::hidapi::UsbHidApiTransport;
#[cfg(target_os = "linux")]
use hypercolor_hal::transport::hidraw::UsbHidRawTransport;

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
    let mut report = RazerReport::new_zeroed();
    report.transaction_id = transaction_id;
    report.data_size = data_size;
    report.command_class = command_class;
    report.command_id = command_id;
    report.args[..args.len()].copy_from_slice(args);
    report.crc = razer_crc(&report);
    report.as_bytes().to_vec()
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

fn expected_mamba_transport() -> TransportType {
    #[cfg(target_os = "linux")]
    {
        TransportType::UsbHidRaw {
            interface: 0,
            report_id: 0x00,
            report_mode: HidRawReportMode::FeatureReport,
            usage_page: Some(0x0001),
            usage: Some(0x0002),
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        TransportType::UsbHidApi {
            interface: Some(0),
            report_id: 0x00,
            report_mode: HidRawReportMode::FeatureReport,
            usage_page: Some(0x0001),
            usage: Some(0x0002),
        }
    }
}

#[cfg(target_os = "linux")]
async fn open_mamba_transport(
    usb: &nusb::DeviceInfo,
    interface: u8,
    report_id: u8,
    report_mode: HidRawReportMode,
    usage_page: Option<u16>,
    usage: Option<u16>,
) -> Result<Box<dyn Transport>, hypercolor_hal::transport::TransportError> {
    let device_usb_path = usb_path(usb);

    UsbHidRawTransport::open(
        RAZER_VENDOR_ID,
        PID_MAMBA_ELITE,
        interface,
        report_id,
        report_mode,
        usb.serial_number(),
        Some(&device_usb_path),
        usage_page,
        usage,
    )
    .await
    .map(|transport| -> Box<dyn Transport> { Box::new(transport) })
}

#[cfg(not(target_os = "linux"))]
#[expect(
    clippy::unused_async,
    reason = "The Linux test helper is async, so the non-Linux fallback keeps the same call shape for shared hardware probes"
)]
async fn open_mamba_transport(
    usb: &nusb::DeviceInfo,
    interface: u8,
    report_id: u8,
    report_mode: HidRawReportMode,
    usage_page: Option<u16>,
    usage: Option<u16>,
) -> Result<Box<dyn Transport>, hypercolor_hal::transport::TransportError> {
    let device_usb_path = usb_path(usb);

    UsbHidApiTransport::open(
        RAZER_VENDOR_ID,
        PID_MAMBA_ELITE,
        Some(interface),
        report_id,
        report_mode,
        usb.serial_number(),
        Some(&device_usb_path),
        usage_page,
        usage,
    )
    .map(|transport| -> Box<dyn Transport> { Box::new(transport) })
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
    let transport = open_mamba_transport(
        &usb,
        0,
        0x00,
        HidRawReportMode::FeatureReport,
        Some(0x0001),
        Some(0x0002),
    )
    .await?;

    for transaction_id in [0x1F_u8, 0x3F_u8] {
        match probe_transaction_id(transport.as_ref(), transaction_id).await {
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

        match probe_extended_effect_query(transport.as_ref(), transaction_id).await {
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
        } => (
            interface.expect("Mamba Elite HIDAPI descriptor should pin an interface"),
            report_id,
            report_mode,
            usage_page,
            usage,
        ),
        TransportType::UsbHidRaw {
            interface,
            report_id,
            report_mode,
            usage_page,
            usage,
        } => (interface, report_id, report_mode, usage_page, usage),
        other => panic!("expected hidapi transport for Mamba Elite, got {other:?}"),
    };
    assert_eq!(descriptor.transport, expected_mamba_transport());

    let usb = nusb::list_devices()
        .await?
        .find(|device| {
            device.vendor_id() == RAZER_VENDOR_ID && device.product_id() == PID_MAMBA_ELITE
        })
        .expect("Razer Mamba Elite USB device should be present");
    let transport =
        open_mamba_transport(&usb, interface, report_id, report_mode, usage_page, usage).await?;

    let protocol = (descriptor.protocol.build)();
    let total_leds = usize::try_from(protocol.total_leds()).expect("LED count should fit usize");
    let white = vec![[0x40, 0x00, 0x40]; total_leds];
    let clear = vec![[0x00, 0x00, 0x00]; total_leds];

    let result = async {
        run_commands(
            protocol.as_ref(),
            transport.as_ref(),
            protocol.init_sequence(),
        )
        .await?;
        run_commands(
            protocol.as_ref(),
            transport.as_ref(),
            protocol.encode_frame(&white),
        )
        .await?;
        tokio::time::sleep(IDENTIFY_FLASH_MS).await;
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    let _ = run_commands(
        protocol.as_ref(),
        transport.as_ref(),
        protocol.encode_frame(&clear),
    )
    .await;
    let _ = transport.close().await;

    result
}
